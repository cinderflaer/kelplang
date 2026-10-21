use super::*;

// ---------------------------------------------------------------------------
// MISSING-k4Nvz: `std/ws`, the WebSocket client. A client nobody has spoken to
// is not shipped, so this is a real server: it reads the handshake, checks the
// `Sec-WebSocket-Key` by computing the accept value the client must compare
// against (a Rust RFC 3174 SHA-1, the test's own copy), answers `101`, and
// then runs one scripted exchange so that every layer the module claims is
// exercised: a text frame, a message split across two continuation frames, a
// ping whose pong is read back, and a close handshake.
//
// Nothing here is canned bytes: the accept value is derived from the key the
// *client* sent, so a wrong `sha1` or a wrong base64 in `std/ws` fails the
// handshake instead of passing a fixture that hard-coded an answer. And every
// frame the server reads is checked for the mask bit and unmasked before its
// payload is compared, so a client that forgot to mask fails here.
// ---------------------------------------------------------------------------

/// The GUID of RFC 6455 1.3, beside the Rust SHA-1 below.
pub(crate) const WS_TEST_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// SHA-1 of `data` (RFC 3174), for the accept value. The test's own copy, so
/// the handshake check does not lean on the module it is checking.
pub(crate) fn test_sha1(data: &[u8]) -> [u8; 20] {
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    for block in msg.chunks(64) {
        let mut w = [0u32; 80];
        for (i, word) in w.iter_mut().take(16).enumerate() {
            *word = u32::from_be_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i / 20 {
                0 => ((b & c) | (!b & d), 0x5A827999u32),
                1 => (b ^ c ^ d, 0x6ED9EBA1),
                2 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }
    let mut out = [0u8; 20];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

/// Standard base64, padded: the accept value is 20 bytes, so this always ends
/// in one `=`.
pub(crate) fn test_base64(data: &[u8]) -> String {
    const ALPHABET: &[u8] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(triple >> 18) as usize & 63] as char);
        out.push(ALPHABET[(triple >> 12) as usize & 63] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[(triple >> 6) as usize & 63] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[triple as usize & 63] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// The client's handshake head, up to (not including) the blank line.
pub(crate) fn read_ws_head<S: Read>(stream: &mut S) -> String {
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            return String::from_utf8_lossy(&buf[..pos]).to_string();
        }
        match stream.read(&mut chunk) {
            Ok(0) => panic!("the client closed during the handshake"),
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) => panic!("failed to read the handshake: {e}"),
        }
    }
}

/// The value of `name` in a handshake head, case-insensitively, or the empty
/// string.
pub(crate) fn ws_header(head: &str, name: &str) -> String {
    head.lines()
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .find(|(n, _)| n.trim().eq_ignore_ascii_case(name))
        .map(|(_, v)| v.trim().to_string())
        .unwrap_or_default()
}

/// One unmasked server frame. Every frame this side sends has `FIN` set except
/// the first half of the fragmented message, which is the `fin` argument.
pub(crate) fn ws_frame(opcode: u8, fin: bool, payload: &[u8]) -> Vec<u8> {
    assert!(payload.len() <= 125, "this gate writes no extended lengths");
    let mut out = vec![(if fin { 0x80 } else { 0 }) | opcode, payload.len() as u8];
    out.extend_from_slice(payload);
    out
}

/// One frame from the client: its opcode and its unmasked payload. The mask
/// bit is required (RFC 6455 5.1), which is the half of the framing a client
/// can get wrong without the server noticing anything else.
pub(crate) fn read_ws_frame<S: Read>(stream: &mut S) -> (u8, Vec<u8>) {
    let mut header = [0u8; 2];
    stream
        .read_exact(&mut header)
        .expect("the client closed instead of sending a frame");
    let opcode = header[0] & 0x0f;
    assert!(
        header[1] & 0x80 != 0,
        "the client sent an unmasked frame (opcode {opcode:#x}), which RFC 6455 5.1 forbids"
    );
    let mut len = (header[1] & 0x7f) as usize;
    if len == 126 {
        let mut ext = [0u8; 2];
        stream.read_exact(&mut ext).expect("short extended length");
        len = u16::from_be_bytes(ext) as usize;
    } else if len == 127 {
        let mut ext = [0u8; 8];
        stream.read_exact(&mut ext).expect("short extended length");
        len = u64::from_be_bytes(ext) as usize;
    }
    let mut key = [0u8; 4];
    stream.read_exact(&mut key).expect("short masking key");
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload).expect("short frame payload");
    for (i, byte) in payload.iter_mut().enumerate() {
        *byte ^= key[i % 4];
    }
    (opcode, payload)
}

pub(crate) fn serve_ws(listener: TcpListener, accepted: Arc<AtomicUsize>) {
    let _ = listener.set_nonblocking(true);
    let deadline = Instant::now() + ACCEPT_DEADLINE;
    let mut stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break Some(stream),
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() > deadline {
                    break None;
                }
                thread::sleep(Duration::from_millis(2));
            }
            Err(_) => break None,
        }
    };
    let Some(stream) = stream.take() else { return };
    // Non-blocking was only for the accept loop; an accepted socket inherits
    // the listener's mode on Linux. The timeouts are what stop a client that
    // stalls mid-handshake from hanging the suite.
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(20)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(20)));
    serve_ws_conn(stream, &accepted);
}

/// The `wss` gate: the same exchange, after the TLS handshake, so the client's
/// own `std/tls` path is what carries the WebSocket framing.
pub(crate) fn serve_ws_tls(
    listener: TcpListener,
    acceptor: openssl::ssl::SslAcceptor,
    accepted: Arc<AtomicUsize>,
) {
    let _ = listener.set_nonblocking(true);
    let deadline = Instant::now() + ACCEPT_DEADLINE;
    let mut stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break Some(stream),
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() > deadline {
                    break None;
                }
                thread::sleep(Duration::from_millis(2));
            }
            Err(_) => break None,
        }
    };
    let Some(stream) = stream.take() else { return };
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(20)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(20)));
    if let Ok(tls) = acceptor.accept(stream) {
        serve_ws_conn(tls, &accepted);
    }
}

/// The scripted exchange, on one accepted connection. Every expectation here
/// is an assertion rather than a return value: a client that gets a step
/// wrong fails the test in this thread, and `WsServer::finish` turns that into
/// the test's own failure.
pub(crate) fn serve_ws_conn<S: Read + Write>(mut stream: S, accepted: &AtomicUsize) {
    accepted.fetch_add(1, Ordering::SeqCst);
    ws_gate_exchange(&mut stream);
}

/// The gate's scripted exchange over any byte stream: plain TCP for `ws`, a
/// TLS server stream for `wss`.
pub(crate) fn ws_gate_exchange<S: Read + Write>(stream: &mut S) {
    let head = read_ws_head(stream);
    assert!(
        head.starts_with("GET /gateway HTTP/1.1"),
        "the request line was not the gate's own path:\n{head}"
    );
    assert!(
        head.to_lowercase().contains("upgrade: websocket")
            && head.to_lowercase().contains("connection: upgrade"),
        "the handshake did not ask for an upgrade:\n{head}"
    );
    assert!(
        ws_header(&head, "sec-websocket-version") == "13",
        "the handshake did not name version 13:\n{head}"
    );
    let key = ws_header(&head, "sec-websocket-key");
    assert!(!key.is_empty(), "the handshake carried no Sec-WebSocket-Key:\n{head}");
    assert_eq!(
        key.len(),
        24,
        "a 16-byte key in base64 is 24 characters, and this was {key:?}"
    );
    let accept = test_base64(&test_sha1(format!("{key}{WS_TEST_GUID}").as_bytes()));
    let response = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n\
         Connection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
    );
    stream
        .write_all(response.as_bytes())
        .expect("failed to write the 101");

    stream.write_all(&ws_frame(0x1, true, b"hello")).expect("text frame");
    stream.write_all(&ws_frame(0x1, false, b"frag")).expect("first fragment");
    stream.write_all(&ws_frame(0x0, true, b"ment")).expect("continuation");
    stream.write_all(&ws_frame(0x9, true, b"ping?")).expect("ping frame");

    let (opcode, payload) = read_ws_frame(stream);
    assert_eq!(
        opcode, 0xA,
        "the client answered the ping with opcode {opcode:#x}, not a pong"
    );
    assert_eq!(payload, b"ping?", "the pong carried a different payload");

    let mut close = 1000u16.to_be_bytes().to_vec();
    close.extend_from_slice(b"done");
    stream.write_all(&ws_frame(0x8, true, &close)).expect("close frame");

    let (opcode, payload) = read_ws_frame(stream);
    assert_eq!(
        opcode, 0x8,
        "the client's closing frame was opcode {opcode:#x}, not a close"
    );
    assert_eq!(
        &payload[..2],
        &1000u16.to_be_bytes(),
        "the client's close code was not 1000"
    );
    assert_eq!(&payload[2..], b"bye", "the client's close reason was not 'bye'");
}

/// The `std/ws` gate: one connection, one scripted exchange. It lives in a
/// thread for the duration of one test, like `CannedServer`.
pub(crate) struct WsServer {
    pub(crate) port: u16,
    pub(crate) accepted: Arc<AtomicUsize>,
    pub(crate) handle: Option<thread::JoinHandle<()>>,
}

impl WsServer {
    pub(crate) fn start() -> WsServer {
        let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind loopback");
        let port = listener.local_addr().expect("local address").port();
        let accepted = Arc::new(AtomicUsize::new(0));
        let counter = accepted.clone();
        let handle = thread::spawn(move || serve_ws(listener, counter));
        WsServer { port, accepted, handle: Some(handle) }
    }

    pub(crate) fn accepted(&self) -> usize {
        self.accepted.load(Ordering::SeqCst)
    }

    /// Waits for the serving thread, so its assertions reach this test: a
    /// panic in that thread is a failure here, not a silent pass.
    pub(crate) fn finish(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.join().expect("the WebSocket gate thread failed; see its message");
        }
    }
}

/// The `wss` twin of `WsServer`: a TLS acceptor in front of the same scripted
/// exchange, so the client's own `std/tls` handshake and encrypted framing are
/// what the test exercises.
pub(crate) struct WsTlsServer {
    pub(crate) port: u16,
    pub(crate) accepted: Arc<AtomicUsize>,
    pub(crate) handle: Option<thread::JoinHandle<()>>,
    pub(crate) cert: TestCert,
}

impl WsTlsServer {
    pub(crate) fn start() -> WsTlsServer {
        let cert = generate_test_cert();
        let acceptor = tls_acceptor(&cert, None);
        let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind loopback");
        let port = listener.local_addr().expect("local address").port();
        let accepted = Arc::new(AtomicUsize::new(0));
        let counter = accepted.clone();
        let handle = thread::spawn(move || serve_ws_tls(listener, acceptor, counter));
        WsTlsServer { port, accepted, handle: Some(handle), cert }
    }

    pub(crate) fn accepted(&self) -> usize {
        self.accepted.load(Ordering::SeqCst)
    }

    pub(crate) fn finish(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.join().expect("the wss gate thread failed; see its message");
        }
    }
}

#[test]
pub(crate) fn a_websocket_client_speaks_the_handshake_and_every_frame_kind() {
    let mut server = WsServer::start();
    let bin = out_path("ws_gate");
    let build = kelpc()
        .arg("build")
        .arg(fixture("tests/fixtures/ws_gate.kp"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected the ws_gate fixture to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&bin)
        .arg(server.port.to_string())
        .output()
        .expect("failed to run the client");
    server.finish();
    assert!(
        run.status.success(),
        "expected the client to exit successfully, got:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let expected = format!(
        "url 127.0.0.1 {} /gateway\nconnected\ntext hello\nfrag fragment\n\
         ping ping?\nclose 1000 done\nclosed\n",
        server.port
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        expected,
        "unexpected stdout for 'ws_gate'"
    );
    assert_eq!(server.accepted(), 1, "the client opened more than one connection");
}

#[test]
pub(crate) fn a_wss_client_runs_the_websocket_framing_over_tls() {
    // The replacement for the old `wss_is_refused_by_name_because_v1_has_no_tls`
    // test: `wss` now parses, the handshake verifies the server's certificate
    // through the default store (`SSL_CERT_FILE`, which is what the harness
    // points at the server's own PEM), and the same scripted exchange the plain
    // `ws` gate runs happens over the TLS stream.
    let mut server = WsTlsServer::start();
    let bin = out_path("ws_wss");
    let build = kelpc()
        .arg("build")
        .arg(fixture("tests/fixtures/ws_wss.kp"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected the ws_wss fixture to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let ssl = server.cert.cert_path.to_str().expect("the cert path is utf-8").to_string();
    let run = Command::new(&bin)
        .arg(server.port.to_string())
        .env_remove("SSL_CERT_FILE")
        .env_remove("SSL_CERT_DIR")
        .env("SSL_CERT_FILE", ssl)
        .output()
        .expect("failed to run the client");
    server.finish();
    assert!(
        run.status.success(),
        "expected the wss client to exit successfully, got:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let expected = format!(
        "url 127.0.0.1 {} wss\nconnected\ntext hello\nfrag fragment\n\
         ping ping?\nclose 1000 done\nclosed\n",
        server.port
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        expected,
        "unexpected stdout for 'ws_wss'"
    );
    assert_eq!(server.accepted(), 1, "the client opened more than one connection");
}

#[test]
pub(crate) fn a_ws_try_connect_tells_a_refused_dial_from_a_non_101_answer() {
    // Two ports, two failures. The refused half dials a port this test
    // released (bind 0, learn the port, drop the listener), so nothing is
    // listening on it and the dial is refused without a server; the answered
    // half is the canned server, which answers `403`. The program branches on
    // the variant and exits 0 either way, which is the whole point: a
    // reconnect loop can tell "nothing is there" from "that is not a
    // WebSocket server". The `403` reason is the text `ws.connect` panics
    // with, byte for byte, because the panicking form is one `panic` over the
    // reason a `try_connect` produced.
    let mut server = CannedServer::start(one_connection(&[STATUS_403]));
    let dead = TcpListener::bind("127.0.0.1:0").expect("failed to bind loopback");
    let dead_port = dead.local_addr().expect("local address").port();
    drop(dead);

    let bin = out_path("ws_try_connect");
    let build = kelpc()
        .arg("build")
        .arg(fixture("tests/fixtures/ws_try_connect.kp"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected the ws_try_connect fixture to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&bin)
        .arg(dead_port.to_string())
        .arg(server.port.to_string())
        .output()
        .expect("failed to run the client");
    server.finish();
    assert!(
        run.status.success(),
        "expected the client to exit successfully, got:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let expected = "dial refused net.connect: connection failed\n  errno 111: Connection refused\n\
         handshake 403 ws.connect: the server answered '403' instead of '101 Switching Protocols'\n\
         survived\n";
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        expected,
        "unexpected stdout for 'ws_try_connect'"
    );
    assert_eq!(server.accepted(), 1, "the client opened more than one connection");
}

#[test]
pub(crate) fn a_wss_url_parses_with_its_scheme_and_default_port() {
    assert_fixture_runs_with_stdin(
        "ws_wss_parse",
        "",
        "gateway.discord.gg 443 /?v=10&encoding=json true\n8443 true\n80 false\n",
    );
}

#[test]
pub(crate) fn a_ws_host_outside_the_grant_is_denied_at_the_connect_call() {
    // The grant names `127.0.0.1` and every port on it; the program asks for
    // `example.com:80`. The check matches the host as written, before
    // `getaddrinfo` runs, and it runs inside `std/ws` at the handshake's own
    // `net.connect`, so no socket is opened and this needs no server.
    assert_project_denied_at_runtime(
        "ws_net_denied",
        &project("ws_net_denied").join("main.kp"),
        "'example.com:80' is outside the net capability granted to module 'std/ws'",
    );
}

#[test]
pub(crate) fn a_ws_host_outside_the_grant_is_denied_at_the_fallible_connect_too() {
    // The same denial the panicking form gets, at the same dial inside
    // `std/ws`: whichever of the two a caller reaches for, the grant holds.
    assert_project_denied_at_runtime(
        "ws_try_net_denied",
        &project("ws_try_net_denied").join("main.kp"),
        "'example.com:80' is outside the net capability granted to module 'std/ws'",
    );
}

#[test]
pub(crate) fn a_ws_program_that_does_not_grant_rand_does_not_build() {
    // `std/ws` draws the handshake key and every masking key from `std/rand`,
    // and a module holds exactly what its importer granted it, so `net` alone
    // is not a working grant. The refusal names what was not granted, at the
    // call inside the module.
    assert_project_rejected(
        "ws_rand_denied",
        &project("ws_rand_denied").join("main.kp"),
        "needs the 'rand' capability, which module 'std/ws' was not granted",
    );
}

// ---------------------------------------------------------------------------
// The two compiler fixes S4 forced. Both were found by the first real HTTP
// request: a request whose line ending is not CRLF is not a request.
// ---------------------------------------------------------------------------

#[test]
pub(crate) fn carriage_return_is_an_escape_like_any_other() {
    // `"\r"` used to decode to the letter `r`, because the lexer passed an
    // unknown escape through as the character itself. Fixing that exposed the
    // second half: `c_string_literal` did not escape CR either, so the
    // *generated C* had a raw carriage return inside a string literal, which
    // is not a string literal at all. Each escape is now pinned as a byte
    // value, so a change of meaning fails here rather than in a malformed
    // protocol on the wire.
    assert_compiles_and_runs("tests/fixtures/escapes.kp", "13\n2\n10\n9\n92\n34\n");
}

#[test]
pub(crate) fn a_block_comment_spans_and_nests_and_an_escape_writes_several_lines() {
    // `/* ... */` is new syntax (`design/syntax.md (kelp wiki)` §2.1): it may span lines, it
    // nests, and it may sit inline before a statement. The fixture also pins
    // one `print` writing two lines through `\n`, and `print!` with no
    // terminating newline.
    assert_compiles_and_runs(
        "tests/fixtures/block_comments.kp",
        "first\nsecond\nno newline here\ndone\n",
    );
}

#[test]
pub(crate) fn a_documentation_comment_is_a_comment_to_the_compiler() {
    // `//!` starts a documentation block (`design/syntax.md (kelp wiki)` §2.1). The compiler
    // reads it as a comment, so the fixture has to build and run like any
    // other program; what is refused is the trailing spelling, below.
    assert_compiles_and_runs("tests/fixtures/documentation_comment.kp", "7\n");
}

#[test]
pub(crate) fn source_location_is_the_kelp_call_site() {
    assert_compiles_and_runs("tests/fixtures/source_location.kp", "5\ntrue\n");
}

#[test]
pub(crate) fn trace_records_events_and_spans_on_stderr() {
    let bin = out_path("trace");
    let build = kelpc()
        .arg("build")
        .arg(fixture("tests/fixtures/trace.kp"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected trace fixture to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&bin)
        .output()
        .expect("failed to run trace fixture");
    assert!(
        run.status.success(),
        "trace fixture failed:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout), "done\n");
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.contains("trace\tevent\t"),
        "missing event record:\n{stderr}"
    );
    assert!(
        stderr.contains("trace\tbegin\t"),
        "missing begin record:\n{stderr}"
    );
    assert!(
        stderr.contains("trace\tend\t"),
        "missing end record:\n{stderr}"
    );
    assert!(
        stderr.contains("ready\\tset"),
        "text field was not escaped:\n{stderr}"
    );
}

#[test]
pub(crate) fn a_documentation_comment_after_code_is_rejected() {
    assert_rejected_with(
        "tests/fixtures/doc_comment_trailing.kp",
        "must start a line",
    );
}

#[test]
pub(crate) fn a_backslash_dollar_escape_writes_a_literal_dollar_sign() {
    assert_compiles_and_runs(
        "tests/fixtures/escape_dollar.kp",
        "36\n21\n$KELPC is unset, ${x}\n",
    );
}

#[test]
pub(crate) fn sys_reports_the_build_target() {
    assert_compiles_and_runs("tests/fixtures/sys_target.kp", "linux\ntrue\n");
}

#[test]
pub(crate) fn an_unknown_string_escape_is_rejected_by_name() {
    // The old fallback made `"\q"` mean `"q"`: a typo was silently a different
    // string. It is now a diagnostic that names the supported set.
    let bin = out_path("unknown_escape");
    let build = kelpc()
        .arg("build")
        .arg(fixture("tests/fixtures/unknown_escape.kp"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        !build.status.success(),
        "expected an unknown escape to be rejected, but it built successfully"
    );
    let stderr = String::from_utf8_lossy(&build.stderr);
    assert!(
        stderr.contains("unknown escape '\\q'"),
        "expected the diagnostic to name the escape, got:\n{stderr}"
    );
}

#[test]
pub(crate) fn the_download_example_writes_exactly_what_it_received() {
    // `examples/download.kp` is a program, not a snippet: it is compiled and run
    // against a canned server, and the file it writes is compared byte for
    // byte. The body deliberately holds a NUL and invalid utf-8, which is the
    // case a `String` could not carry, `fs.write` takes `Bytes` for exactly
    // this, and a download that went through text would corrupt it.
    let body: Vec<u8> = b"line\r\nnul:\0 hi:\xff\xfe".to_vec();
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(&body);

    let mut server = CannedServer::start(vec![vec![response]]);
    let bin = out_path("download_example");
    let build = kelpc()
        .arg("build")
        .arg(fixture("tests/fixtures/examples/download.kp"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected examples/download.kp to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let out = out_path("download_example_out");
    let part = PathBuf::from(format!("{}.part", out.display()));
    let _ = std::fs::remove_file(&out);
    let _ = std::fs::remove_file(&part);
    let url = format!("http://127.0.0.1:{}/payload.bin", server.port);
    let run = Command::new(&bin)
        .arg(&url)
        .arg(&out)
        .output()
        .expect("failed to run the compiled binary");
    server.finish();

    assert!(
        run.status.success(),
        "expected the download to succeed, got:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        std::fs::read(&out).expect("the output file exists"),
        body,
        "the file on disk is not what the server sent"
    );
    assert!(
        !part.exists(),
        "the `.part` file should have been renamed into place"
    );
    let stdout = String::from_utf8_lossy(&run.stdout);
    let expected_size = format!("{}", body.len());
    assert!(
        stdout.starts_with(&format!("Content-Length: {expected_size}\n"))
            && stdout.ends_with(&format!(" bytes to {}\n", out.display())),
        "unexpected stdout:\n{stdout}"
    );
    let _ = std::fs::remove_file(&out);
}

/// MK6-S2-M1: `examples/download_pool.kp` fetches N paths from one host with
/// four worker threads, each holding its own keep-alive `Pool`. The canned
/// server accepts four connections and answers two requests on each, which is
/// the shape a worker's own pool produces (one handshake, then reuse): a
/// worker whose second request opened a *fifth* connection would mean its
/// pool was not being reused, and a run that hung would mean fewer than four
/// workers ever started.
#[test]
pub(crate) fn the_download_pool_example_uses_one_kept_alive_connection_per_worker() {
    let body = b"data".to_vec();
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    let mut per_connection = response.clone();
    per_connection.extend_from_slice(&body);
    let per_connection = per_connection;
    let connections: Vec<Vec<Vec<u8>>> = (0..4)
        .map(|_| vec![per_connection.clone(), per_connection.clone()])
        .collect();
    let mut server = CannedServer::start(connections);

    let bin = out_path("download_pool_example");
    let build = kelpc()
        .arg("build")
        .arg(fixture("tests/fixtures/examples/download_pool.kp"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected examples/download_pool.kp to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let run = Command::new(&bin)
        .arg("127.0.0.1")
        .arg(server.port.to_string())
        .args(["/a", "/b", "/c", "/d", "/e", "/f", "/g", "/h"])
        .output()
        .expect("failed to run the compiled binary");
    server.finish();

    assert!(
        run.status.success(),
        "expected the pooled download to succeed, got:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(server.accepted(), 4, "expected exactly one connection per worker");
    let stdout = String::from_utf8_lossy(&run.stdout);
    for i in 0..4 {
        assert!(
            stdout.contains(&format!("worker {i}: 2 ok, 0 failed, 8 bytes\n")),
            "missing worker {i}'s report:\n{stdout}"
        );
    }
    assert!(
        stdout.ends_with("total: 8 ok, 0 failed, 32 bytes\n"),
        "unexpected stdout:\n{stdout}"
    );
}
