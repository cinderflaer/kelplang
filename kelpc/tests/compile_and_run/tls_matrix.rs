use super::*;

// ---------------------------------------------------------------------------
// Release goal 1 (stable TLS), part 2: the hostile-conditions matrix. Every case
// dials 127.0.0.1 only and runs against a server started in this thread, with the
// same per-server throwaway certificate the failure matrix above uses. What these
// cases add are the conditions a happy path never reaches: a body far larger than
// one read, a peer that dribbles, a peer that resets, a peer that alerts
// mid-record, many clients at once, a silent peer with no deadline at all, the
// SNI name itself, an `https` redirect to a second local port, `Connection:
// close` against keep-alive, and a header block larger than the read buffer.
// ---------------------------------------------------------------------------

/// A digest as lowercase hex, the spelling `std/encoding`'s `hex_encode` gives, so
/// a fixture's own SHA-1 can be compared with this file's `test_sha1`.
pub(crate) fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// Byte `i` of the 1 MiB payload `tls_partial.kp` builds, matching that fixture's
/// `payload_byte`.
pub(crate) fn partial_payload_byte(i: usize) -> u8 {
    ((i * 31 + 7) % 256) as u8
}

pub(crate) fn partial_payload(n: usize) -> Vec<u8> {
    (0..n).map(partial_payload_byte).collect()
}

/// The 10 MiB body case (a) reads back. A position-dependent pattern rather than a
/// constant, so a chunk that duplicated or dropped a byte moves the SHA-1.
pub(crate) fn big_body(n: usize) -> Vec<u8> {
    (0..n).map(|i| ((i * 131 + 17) % 253) as u8).collect()
}

/// The body `SlowTlsServer` writes one byte at a time.
pub(crate) fn slow_reply(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i % 251) as u8).collect()
}

/// The server for case (b): drain the client's 1 MiB one byte at a time, record
/// what arrived, then answer with its own body one byte at a time. An `SSL_read`
/// of one byte still consumes a whole record underneath, so this drives the
/// partial-read and partial-write loops of one stream in both directions.
pub(crate) struct SlowTlsServer {
    pub(crate) port: u16,
    pub(crate) received: Arc<Mutex<Option<Vec<u8>>>>,
    pub(crate) handle: Option<thread::JoinHandle<()>>,
    pub(crate) cert: TestCert,
}

impl SlowTlsServer {
    pub(crate) fn start(message_len: usize, reply_len: usize) -> SlowTlsServer {
        let cert = generate_test_cert();
        let acceptor = tls_acceptor(&cert, None);
        let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind loopback");
        let port = listener.local_addr().expect("local address").port();
        let received = Arc::new(Mutex::new(None));
        let slot = received.clone();
        let handle = thread::spawn(move || {
            let _ = listener.set_nonblocking(true);
            let deadline = Instant::now() + ACCEPT_DEADLINE;
            let stream = loop {
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
            let Some(stream) = stream else { return };
            let _ = stream.set_nonblocking(false);
            let mut tls = acceptor.accept(stream).expect("the slow server's handshake");
            let mut one = [0u8; 1];
            let mut got = Vec::with_capacity(message_len);
            for _ in 0..message_len {
                if tls.read_exact(&mut one).is_err() {
                    break;
                }
                got.push(one[0]);
            }
            *slot.lock().expect("the received slot") = Some(got);
            for b in slow_reply(reply_len) {
                if tls.write_all(std::slice::from_ref(&b)).is_err() {
                    break;
                }
            }
            let _ = tls.shutdown();
        });
        SlowTlsServer { port, received, handle: Some(handle), cert }
    }

    pub(crate) fn finish(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }

    pub(crate) fn received(&self) -> Vec<u8> {
        self.received.lock().expect("the received slot").clone().unwrap_or_default()
    }
}

/// The server for case (e): one TLS connection per client, and each one echoes the
/// request head it read back byte for byte, then shuts down cleanly. A client
/// compares the echo against what it sent, so two streams that crossed produce a
/// different request, and the accept count pins that every client really got its
/// own connection.
pub(crate) struct EchoTlsServer {
    pub(crate) port: u16,
    pub(crate) accepted: Arc<AtomicUsize>,
    pub(crate) handle: Option<thread::JoinHandle<()>>,
    pub(crate) cert: TestCert,
}

impl EchoTlsServer {
    pub(crate) fn start(clients: usize) -> EchoTlsServer {
        let cert = generate_test_cert();
        let acceptor = Arc::new(tls_acceptor(&cert, None));
        let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind loopback");
        let port = listener.local_addr().expect("local address").port();
        let accepted = Arc::new(AtomicUsize::new(0));
        let counter = accepted.clone();
        let handle = thread::spawn(move || {
            let _ = listener.set_nonblocking(true);
            let deadline = Instant::now() + Duration::from_secs(60);
            let mut workers = Vec::new();
            for _ in 0..clients {
                let stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break Some(stream),
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            if Instant::now() > deadline {
                                break None;
                            }
                            thread::sleep(Duration::from_millis(1));
                        }
                        Err(_) => break None,
                    }
                };
                let Some(stream) = stream else { break };
                let _ = stream.set_nonblocking(false);
                let acceptor = acceptor.clone();
                let counter = counter.clone();
                workers.push(thread::spawn(move || {
                    let mut tls = acceptor.accept(stream).expect("the echo server's handshake");
                    counter.fetch_add(1, Ordering::SeqCst);
                    let mut head: Vec<u8> = Vec::new();
                    let mut one = [0u8; 1];
                    while tls.read(&mut one).unwrap_or(0) == 1 {
                        head.push(one[0]);
                        if head.ends_with(b"\r\n\r\n") {
                            break;
                        }
                    }
                    tls.write_all(&head).expect("the echo write");
                    let _ = tls.shutdown();
                }));
            }
            for worker in workers {
                let _ = worker.join();
            }
        });
        EchoTlsServer { port, accepted, handle: Some(handle), cert }
    }

    pub(crate) fn accepted(&self) -> usize {
        self.accepted.load(Ordering::SeqCst)
    }

    pub(crate) fn finish(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// The server for case (g): it records the server name the client sent through SNI
/// before it answers, so the test compares it with the host string the fixture
/// handed `handshake` rather than with whatever the socket resolved to. The
/// certificate carries only a `localhost` DNS name, which is what makes the
/// fixture's `handshake(s, "localhost")` verify even though it dialed 127.0.0.1.
pub(crate) struct SniTlsServer {
    pub(crate) port: u16,
    pub(crate) names: Arc<Mutex<Vec<String>>>,
    pub(crate) handle: Option<thread::JoinHandle<()>>,
    pub(crate) cert: TestCert,
}

impl SniTlsServer {
    pub(crate) fn start() -> SniTlsServer {
        let cert = generate_mismatched_test_cert();
        let acceptor = tls_acceptor(&cert, None);
        let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind loopback");
        let port = listener.local_addr().expect("local address").port();
        let names = Arc::new(Mutex::new(Vec::new()));
        let slot = names.clone();
        let handle = thread::spawn(move || {
            let _ = listener.set_nonblocking(true);
            let deadline = Instant::now() + ACCEPT_DEADLINE;
            let stream = loop {
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
            let Some(stream) = stream else { return };
            let _ = stream.set_nonblocking(false);
            let mut tls = acceptor.accept(stream).expect("the SNI server's handshake");
            let name = tls
                .ssl()
                .servername(openssl::ssl::NameType::HOST_NAME)
                .unwrap_or("")
                .to_string();
            slot.lock().expect("the SNI slot").push(name);
            if read_http_request(&mut tls) {
                let _ = tls.write_all(BODY_200_HELLO);
            }
            let _ = tls.shutdown();
        });
        SniTlsServer { port, names, handle: Some(handle), cert }
    }

    pub(crate) fn finish(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }

    pub(crate) fn names(&self) -> Vec<String> {
        self.names.lock().expect("the SNI slot").clone()
    }
}

#[test]
pub(crate) fn a_ten_mib_body_is_byte_for_byte_at_small_and_large_read_sizes() {
    // (a) The same 10 MiB body read in 1 KiB pieces and in 64 KiB pieces. Both
    // the length and the SHA-1 have to match the bytes the server actually sent,
    // so a chunk boundary that lost or duplicated a byte is caught.
    let body = big_body(10 * 1024 * 1024);
    // The payload goes straight onto the stream: this case is about the read
    // loop's chunking, so the body's end is a clean `close_notify` rather than a
    // header the fixture would have to parse. The `std/http` 10 MiB path is the
    // same framing `tls_partial` and the project tests already exercise.
    let expected = format!("{}\n{}\n", body.len(), hex_lower(&test_sha1(&body)));
    for mode in ["small", "large"] {
        let mut server = CannedTlsServer::start(one_connection(&[body.as_slice()]));
        let ssl = server.cert.cert_path.to_str().expect("the cert path is utf-8").to_string();
        let stdout = tls_fixture_stdout(
            "tls_big_body",
            &[server.port.to_string(), mode.to_string()],
            &[("SSL_CERT_FILE", ssl.as_str())],
        );
        server.finish();
        assert_eq!(stdout, expected, "the body read in {mode} pieces did not match");
        assert_eq!(server.accepted(), 1, "the client opened more than one connection");
    }
}

#[test]
pub(crate) fn one_mib_written_through_write_all_arrives_whole_against_a_dribbling_peer() {
    // (b) The client hands 1 MiB to `write_all` in one call while the server
    // drains one byte at a time, and the server answers one byte at a time while
    // the client reads 4 KiB at once. Both sides hash what they saw.
    const SENT: usize = 1048576;
    const REPLY: usize = 200_000;
    let mut server = SlowTlsServer::start(SENT, REPLY);
    let ssl = server.cert.cert_path.to_str().expect("the cert path is utf-8").to_string();
    let stdout = tls_fixture_stdout(
        "tls_partial",
        &[server.port.to_string(), REPLY.to_string()],
        &[("SSL_CERT_FILE", ssl.as_str())],
    );
    server.finish();
    let reply = slow_reply(REPLY);
    let expected = format!(
        "{SENT}\n{}\n{}\n",
        reply.len(),
        hex_lower(&test_sha1(&reply))
    );
    assert_eq!(stdout, expected);
    assert_eq!(
        hex_lower(&test_sha1(&server.received())),
        hex_lower(&test_sha1(&partial_payload(SENT))),
        "the server did not drain exactly what `write_all` sent"
    );
}

#[test]
pub(crate) fn a_record_that_fails_authentication_mid_stream_is_not_a_clean_end_of_stream() {
    // A partial response, then a record the client cannot authenticate. The read
    // must be an error: a peer that wrote garbage mid-body may never look like a
    // complete response. (`openssl`'s crate exposes no way to send a fatal alert
    // after the handshake, since every record is authenticated by then, so the
    // "peer that gives up mid-stream" signal is a corrupted record here and a
    // `close_notify` in `a_close_notify_mid_body_is_not_a_complete_response`.)
    let mut server = RudeTlsServer::start(TlsBehavior::CorruptRecord);
    let ssl = server.cert.cert_path.to_str().expect("the cert path is utf-8").to_string();
    let stdout = tls_fixture_stdout(
        "tls_bad_record",
        &[server.port.to_string()],
        &[("SSL_CERT_FILE", ssl.as_str())],
    );
    server.finish();
    let stdout = stdout.trim();
    assert_ne!(stdout, "clean", "a bad record was reported as a clean end of stream");
    assert!(
        stdout.starts_with("io ") || stdout == "peer_closed_uncleanly",
        "unexpected error for a bad record: {stdout}"
    );
}

#[test]
pub(crate) fn a_close_notify_mid_body_is_not_a_complete_response() {
    // (c) The alert a peer can actually send mid-stream: a clean `close_notify`
    // where a `Content-Length` still promised more. `std/tls` is right to report
    // a clean end of stream; `std/http` is what must refuse to call the short
    // body complete, and this projects the whole path through `http.get`.
    let response = b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\npartial";
    assert_project_served_tls_denied(
        "https_get",
        &project("https_get"),
        one_connection(&[response]),
        "the connection closed before the whole body arrived",
    );
}

#[test]
pub(crate) fn a_tcp_reset_during_the_handshake_is_a_typed_error() {
    // (d) The server accepts and closes with the ClientHello unread, which is a
    // RST. The handshake has to come back with an error at once.
    let mut server = RudeTlsServer::start(TlsBehavior::ResetHandshake);
    let ssl = server.cert.cert_path.to_str().expect("the cert path is utf-8").to_string();
    let stdout = tls_fixture_stdout(
        "tls_reset_handshake",
        &[server.port.to_string()],
        &[("SSL_CERT_FILE", ssl.as_str())],
    );
    server.finish();
    let stdout = stdout.trim();
    assert!(
        stdout.starts_with("io ") || stdout == "peer_closed_uncleanly",
        "unexpected error for a reset handshake: {stdout}"
    );
    assert_eq!(server.accepted(), 1, "the client opened more than one connection");
}

#[test]
pub(crate) fn a_tcp_reset_during_a_read_is_a_typed_error() {
    // (d) The handshake completes, the client sends a request, and the server
    // closes with that request unread, which is a RST. The write and the read are
    // both reported, and neither may look like a clean end of stream.
    let mut server = RudeTlsServer::start(TlsBehavior::ResetRead);
    let ssl = server.cert.cert_path.to_str().expect("the cert path is utf-8").to_string();
    let stdout = tls_fixture_stdout(
        "tls_reset_read",
        &[server.port.to_string()],
        &[("SSL_CERT_FILE", ssl.as_str())],
    );
    server.finish();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 2, "expected a write line and a read line, got:\n{stdout}");
    assert!(!stdout.contains("clean"), "a reset was reported as a clean read:\n{stdout}");
    assert!(!stdout.contains("data"), "a reset produced data:\n{stdout}");
    assert_eq!(server.accepted(), 1, "the client opened more than one connection");
}

#[test]
pub(crate) fn sixty_four_concurrent_clients_get_their_own_streams_and_leak_no_descriptors() {
    // (e) 64 workers, each with its own handshake, request, echo check and close.
    // The fixture prints how many read back their own request and the
    // `/proc/self/fd` count before and after, so a crossed stream and a leaked
    // descriptor are both failures here.
    const CLIENTS: usize = 64;
    let mut server = EchoTlsServer::start(CLIENTS);
    let ssl = server.cert.cert_path.to_str().expect("the cert path is utf-8").to_string();
    let stdout = tls_fixture_stdout(
        "tls_concurrent",
        &[server.port.to_string()],
        &[("SSL_CERT_FILE", ssl.as_str())],
    );
    server.finish();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 3, "expected the ok count and two fd counts, got:\n{stdout}");
    let ok: i64 = lines[0].parse().expect("the ok count is a number");
    let before: i64 = lines[1].parse().expect("the before count is a number");
    let after: i64 = lines[2].parse().expect("the after count is a number");
    assert_eq!(ok, CLIENTS as i64, "workers that did not read their own echo:\n{stdout}");
    assert_eq!(after, before, "leaked descriptors: {before} before, {after} after");
    assert_eq!(server.accepted(), CLIENTS, "the server accepted a different number of connections");
}

#[test]
pub(crate) fn a_silent_peer_respects_the_socket_deadline_and_has_none_by_default() {
    // (f) The same never-answering server for both halves, closing after a fixed
    // 1200 ms hold. With a 400 ms deadline the client gives up first and reports
    // `Timeout`; without one it waits for the close, so it cannot finish before
    // the hold and cannot report `Timeout`. That difference is the whole check.
    let hold = Duration::from_millis(900);

    // Built first, so the clock below measures the client's own wait and not
    // the compiler's.
    let no_deadline = build_fixture_bin("tls_handshake_no_timeout");
    let deadline = build_fixture_bin("tls_handshake_socket_timeout");

    let mut without = RudeTlsServer::start(TlsBehavior::NeverHandshakeThenClose);
    let started = Instant::now();
    let out = run_tls_binary(&no_deadline, &[without.port.to_string()], &[]);
    let waited = started.elapsed();
    without.finish();
    assert!(out.status.success(), "the no-timeout client failed:\n{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    assert!(
        !stdout.starts_with("timeout"),
        "a handshake with no deadline reported `{stdout}`"
    );
    assert!(
        waited >= hold,
        "the no-timeout client returned after {waited:?}, before the server's hold"
    );

    let mut with = RudeTlsServer::start(TlsBehavior::NeverHandshakeThenClose);
    let started = Instant::now();
    let out = run_tls_binary(&deadline, &[with.port.to_string()], &[]);
    let waited = started.elapsed();
    with.finish();
    assert!(out.status.success(), "the deadline client failed:\n{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "timeout\n");
    assert!(
        waited < hold,
        "the deadline client waited {waited:?}, past its own 400 ms deadline"
    );
}

#[test]
pub(crate) fn the_server_name_sent_is_the_host_given_to_handshake() {
    // (g) The client dials 127.0.0.1 but hands `handshake` the name `localhost`,
    // and the certificate names only `localhost`. The server records the SNI name
    // it was actually sent, so a handshake that passed the resolved address, or
    // sent no SNI at all, fails here even when verification passes.
    const HOST: &str = "localhost";
    let mut server = SniTlsServer::start();
    let ssl = server.cert.cert_path.to_str().expect("the cert path is utf-8").to_string();
    let stdout = tls_fixture_stdout(
        "tls_sni",
        &[server.port.to_string(), HOST.to_string()],
        &[("SSL_CERT_FILE", ssl.as_str())],
    );
    server.finish();
    assert_eq!(stdout, "ok\n");
    assert_eq!(
        server.names(),
        vec![HOST.to_string()],
        "the server received a different SNI name"
    );
}

#[test]
pub(crate) fn an_https_redirect_reaches_a_second_local_port() {
    // (h) Server B is up first so server A's `Location` can name its port. Both
    // use the same certificate, so the one `SSL_CERT_FILE` the client is given
    // trusts both hops; the body can only come from B.
    let cert = generate_test_cert();
    let mut second = CannedTlsServer::start_with_cert(cert.clone(), one_connection(&[BODY_200_HELLO]));
    let location = format!(
        "HTTP/1.1 301 Moved Permanently\r\nLocation: https://127.0.0.1:{}/final\r\nContent-Length: 0\r\n\r\n",
        second.port
    );
    let mut first = CannedTlsServer::start_with_cert(cert, one_connection(&[location.as_bytes()]));

    let bin = out_path("https_redirect");
    let build = kelpc()
        .arg("build")
        .arg(project("https_redirect"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected the https_redirect project to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let ssl = first.cert.cert_path.to_str().expect("the cert path is utf-8").to_string();
    let run = Command::new(&bin)
        .arg(first.port.to_string())
        .env_remove("SSL_CERT_FILE")
        .env_remove("SSL_CERT_DIR")
        .env("SSL_CERT_FILE", ssl)
        .output()
        .expect("failed to run the redirect client");
    first.finish();
    second.finish();

    assert!(
        run.status.success(),
        "expected the redirect client to exit successfully, got:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout), "200\nhello\ntrue\n");
    assert_eq!(first.accepted(), 1, "the redirecting server served a different number of connections");
    assert_eq!(second.accepted(), 1, "the redirect target served a different number of connections");
}

#[test]
pub(crate) fn connection_close_is_dropped_from_the_pool_and_keep_alive_is_kept() {
    // (i) The first connection answers `Connection: close`, so the pool has to
    // drop it and dial again for the second request; the third request then rides
    // the second connection. Two accepted connections is the whole assertion.
    const CLOSED: &[u8] =
        b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 5\r\n\r\nfirst";
    let connections = vec![
        vec![CLOSED.to_vec()],
        vec![BODY_200_HELLO.to_vec(), BODY_200_HELLO.to_vec()],
    ];
    assert_project_served(
        "http_close_vs_keepalive",
        &project("http_close_vs_keepalive"),
        connections,
        2,
        "200\nfalse\n0\n200\ntrue\n1\n200\ntrue\n1\n",
    );
}

#[test]
pub(crate) fn a_hundred_kib_header_block_survives_std_http() {
    // (j) The response head is 100 000 bytes of header, far more than the 4 KiB
    // the client reads at a time, so locating the blank line and copying the head
    // both span many refills. The body after it has to start at the right byte.
    let big = "a".repeat(100_000);
    let response =
        format!("HTTP/1.1 200 OK\r\nX-Big: {big}\r\nContent-Length: 4\r\n\r\ndone").into_bytes();
    assert_project_served_tls(
        "https_big_headers",
        &project("https_big_headers"),
        one_connection(&[response.as_slice()]),
        1,
        &format!("200\n{}\ndone\n", big.len()),
    );
}

/// Compiles a project, runs it with the canned server's port as `argv[1]`,
/// asserts its stdout and how many connections the server accepted.
pub(crate) fn assert_project_served(
    name: &str,
    target: &Path,
    connections: Vec<Vec<Vec<u8>>>,
    expected_connections: usize,
    expected: &str,
) {
    let mut server = CannedServer::start(connections);
    let bin = out_path(name);
    let build = kelpc()
        .arg("build")
        .arg(target)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected project '{name}' to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&bin)
        .arg(server.port.to_string())
        .output()
        .expect("failed to run the compiled binary");
    server.finish();
    assert!(
        run.status.success(),
        "expected the binary for '{name}' to exit successfully, got:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        expected,
        "unexpected stdout for '{name}'"
    );
    assert_eq!(
        server.accepted(),
        expected_connections,
        "'{name}' made an unexpected number of connections"
    );
}

/// As `assert_project_served`, for a project expected to be denied at runtime by
/// the capability check while the server is up.
pub(crate) fn assert_project_served_denied(
    name: &str,
    target: &Path,
    connections: Vec<Vec<Vec<u8>>>,
    needle: &str,
) {
    let mut server = CannedServer::start(connections);
    let bin = out_path(name);
    let build = kelpc()
        .arg("build")
        .arg(target)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected project '{name}' to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&bin)
        .arg(server.port.to_string())
        .output()
        .expect("failed to run the compiled binary");
    server.finish();
    assert!(
        !run.status.success(),
        "expected '{name}' to be denied at runtime, but it exited successfully"
    );
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.contains(needle),
        "expected the denial for '{name}' to mention '{needle}', got:\n{stderr}"
    );
}

#[test]
pub(crate) fn a_pooled_client_reuses_one_connection_for_five_requests() {
    // Five requests, one accepted connection: keep-alive and the pool are what
    // make the second, third, fourth and fifth requests cost nothing.
    assert_project_served(
        "http_get",
        &project("http_get"),
        one_connection(&[
            BODY_200_HELLO,
            BODY_200_HELLO,
            CHUNKED_200,
            STATUS_404,
            BODY_200_CLOSE,
        ]),
        1,
        "200\nhello\ntrue\ntrue\nfalse\nyes\n1\n200\n1\n200\nchunk!\ntrue\n404\nfalse\n200\nbyebye\nfalse\n0\n",
    );
}

#[test]
pub(crate) fn a_relative_redirect_is_followed_on_the_pooled_connection() {
    // Two hops on one connection (a relative `Location` keeps the host), then a
    // POST on a second: `http.post_text` goes through `Client.send`, which
    // never consults a pool.
    assert_project_served(
        "http_redirect",
        &project("http_redirect"),
        vec![
            vec![REDIRECT_301.to_vec(), BODY_200_HELLO.to_vec()],
            vec![echo_response("payload")],
        ],
        2,
        "200\nhello\n1\n200\ngot:payload\n",
    );
}

#[test]
pub(crate) fn rate_limit_headers_parse_and_absence_reads_as_none() {
    // MISSING-p5Xtr: a `429` naming `Retry-After` and the five
    // `X-RateLimit-*` bucket headers, then a plain `200` naming none of
    // them, the second response is the "absent, not a panic" half, and it
    // also exercises `remaining = 0` on the first response reading as
    // `Some(0)` rather than being confused with "absent". Neither response
    // asks to close, so both ride the one connection `BODY_200_HELLO`'s
    // sibling tests already rely on for keep-alive.
    assert_project_served(
        "http_rate_limit",
        &project("http_rate_limit"),
        one_connection(&[STATUS_429_RATE_LIMITED, BODY_200_HELLO]),
        1,
        "429\n2\n10\n0\n1719796939.000000\n0.500000\nabcd1234\n200\nnone\nnone\nnone\nnone\nnone\nnone\n",
    );
}

#[test]
pub(crate) fn the_pool_ceiling_comes_from_the_manifest() {
    // `http_pool = 0` in this project's `coral.toml` reaches `std/http` as the
    // compiled-in ceiling, so the same two requests now cost two connections.
    // This is the regression test for S4's only compiler-side change.
    assert_project_served(
        "http_pool_off",
        &project("http_pool_off"),
        n_connections(BODY_200_HELLO, 2),
        2,
        "0\n200\n200\n0\n",
    );
}

#[test]
pub(crate) fn a_redirect_to_a_host_the_grant_does_not_name_is_denied() {
    // The grant names `127.0.0.1`; the redirect names `localhost`. The check
    // matches the host as written, so the denial happens even though both names
    // are the same machine, and it happens before any connection to port 80.
    assert_project_served_denied(
        "http_redirect_denied",
        &project("http_redirect_denied"),
        one_connection(&[REDIRECT_ELSEWHERE]),
        "outside the net capability granted to module 'std/http'",
    );
}

#[test]
pub(crate) fn an_https_get_completes_the_tls_handshake_and_reads_the_body() {
    // MK5-S4b-M1 (open item 37): `parse_url` no longer panics on `https`, and
    // the same `Client.send` path that plain `http_get` exercises now goes
    // through `std/tls`'s handshake first. One connection, one request: the
    // point is the handshake and the certificate check succeeding against the
    // server's self-signed cert (via `SSL_CERT_FILE`), not keep-alive, which
    // `a_pooled_client_reuses_one_connection_for_five_requests` already covers
    // for plain `http`.
    assert_project_served_tls(
        "https_get",
        &project("https_get"),
        one_connection(&[BODY_200_HELLO]),
        1,
        "200\nhello\ntrue\n",
    );
}

pub(crate) const TRUNCATED_200: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nabc";
pub(crate) const LOOP_302: &[u8] = b"HTTP/1.1 302 Found\r\nLocation: /loop\r\nContent-Length: 0\r\n\r\n";

#[test]
pub(crate) fn a_fallible_send_reports_a_dial_an_early_close_and_a_redirect_loop_as_values() {
    // MISSING-p7Kdm. Three connections on the canned server, in the order the
    // program makes them: a fresh `try_get` whose body stops after 3 of 10
    // bytes, a pooled request that does the same (the pool must not keep that
    // half-read connection), and a pooled redirect loop followed by a good
    // response on the same kept connection (six `302`s to one `Location`
    // exhaust the client's five hops; the seventh response answers the request
    // after it). The refused half dials a port this test released.
    let mut loop_then_hello: Vec<Vec<u8>> = vec![LOOP_302.to_vec(); 6];
    loop_then_hello.push(BODY_200_HELLO.to_vec());
    let mut server = CannedServer::start(vec![
        vec![TRUNCATED_200.to_vec()],
        vec![TRUNCATED_200.to_vec()],
        loop_then_hello,
    ]);
    let dead = TcpListener::bind("127.0.0.1:0").expect("failed to bind loopback");
    let dead_port = dead.local_addr().expect("local address").port();
    drop(dead);

    let bin = out_path("http_try_send");
    let build = kelpc()
        .arg("build")
        .arg(fixture("tests/fixtures/http_try_send.kp"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected the http_try_send fixture to compile, but it failed:\n{}",
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
    let cut = "exchange http: the connection closed before the whole body arrived";
    let expected = format!(
        "refused dial\nfresh {cut}\npool-cut {cut}\npool kept 0\n\
         pool-loop redirects http: too many redirects\npool kept 1\n\
         pool-hello ok 200 hello\npool kept 1\nsurvived\n"
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        expected,
        "unexpected stdout for 'http_try_send'"
    );
    assert_eq!(server.accepted(), 3, "the client opened an unexpected number of connections");
}

#[test]
pub(crate) fn a_failed_tls_handshake_is_a_dial_error_value() {
    // MISSING-p7Kdm: the server presents a self-signed certificate and the
    // client is not told to trust it (no `SSL_CERT_FILE`), so the handshake
    // fails. That is `HttpErr.Dial` (nothing was sent), not a panic.
    let mut server = CannedTlsServer::start(one_connection(&[]));
    let bin = out_path("https_try_dial");
    let build = kelpc()
        .arg("build")
        .arg(fixture("tests/fixtures/https_try_dial.kp"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected the https_try_dial fixture to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    // Trust store pinned away from the test cert, so it cannot vouch for it.
    let run = Command::new(&bin)
        .arg(server.port.to_string())
        .env("SSL_CERT_FILE", "/dev/null")
        .env("SSL_CERT_DIR", "/dev/null")
        .output()
        .expect("failed to run the client");
    server.finish();
    assert!(
        run.status.success(),
        "expected the client to exit successfully, got:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "dial true\nsurvived\n",
        "unexpected stdout for 'https_try_dial'"
    );
}

#[test]
pub(crate) fn https_to_a_host_the_grant_does_not_name_is_denied() {
    // Same shape as `a_redirect_to_a_host_the_grant_does_not_name_is_denied`,
    // but for `https` directly: the grant names `127.0.0.1:80`\
    // (the `http` default port), the URL asks for `127.0.0.1:<TLS port>`, and
    // the capability check denies it before a socket is ever opened, so the
    // server here accepts nothing, and `SSL_CERT_FILE` is never consulted.
    assert_project_served_tls_denied(
        "https_get_denied",
        &project("https_get_denied"),
        one_connection(&[BODY_200_HELLO]),
        "outside the net capability granted to module 'std/http'",
    );
}
