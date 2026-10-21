use super::*;

// ---------------------------------------------------------------------------
// st2: the HTTP server (`kelp wiki page stdlib/http.md`).
//
// The Rust test is the *client*. The server is a compiled Kelp program from
// `tests/hazards/` (it waits for clients, so it is not a fixture: the sweeps run
// every fixture with no timeout), started on port 0 with the port read from its
// first stdout line. Every program ends by itself after the number of requests
// it was told to answer, runs under `ulimit -v`/`ulimit -t` and `timeout`, and
// every socket here has a read timeout, so a hang is a failed assertion, not a
// stuck suite. The programs are built by `kelpc()`, with the LLVM backend.
// ---------------------------------------------------------------------------

/// The hazard program `name`, built once per test process (the builds of two
/// tests that need the same program would otherwise race for one output file).
pub(crate) fn http_hazard_bin(name: &str) -> PathBuf {
    static BUILT: Mutex<Vec<(String, PathBuf)>> = Mutex::new(Vec::new());
    let mut built = BUILT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, p)) = built.iter().find(|(n, _)| n == name) {
        return p.clone();
    }
    let bin = out_path(&format!("hazard_{name}"));
    let build = kelpc()
        .arg("build")
        .arg(fixture(&format!("tests/hazards/{name}.kp")))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected hazard '{name}' to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    built.push((name.to_string(), bin.clone()));
    bin
}

/// A running server program: its port, and what it printed when it ended.
pub(crate) struct HttpServer {
    pub(crate) child: std::process::Child,
    pub(crate) port: u16,
    pub(crate) out: BufReader<std::process::ChildStdout>,
}

impl HttpServer {
    /// Starts hazard `name` with `args`, bounded in memory, CPU and wall time.
    pub(crate) fn start(name: &str, args: &[&str]) -> HttpServer {
        HttpServer::start_bin(&http_hazard_bin(name), args, true)
    }

    /// As `start` for an already-built program. `cap_memory` is false only for
    /// an AddressSanitizer build, which reserves far more address space than
    /// `ulimit -v` allows.
    pub(crate) fn start_bin(bin: &Path, args: &[&str], cap_memory: bool) -> HttpServer {
        let script = if cap_memory {
            "ulimit -v 1048576; ulimit -t 30; exec timeout 40 \"$0\" \"$@\""
        } else {
            "ulimit -t 30; exec timeout 40 \"$0\" \"$@\""
        };
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(script)
            .arg(&bin)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to start the server program");
        let mut out = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        out.read_line(&mut line).expect("the server printed no port");
        let port: u16 = line
            .trim()
            .parse()
            .unwrap_or_else(|_| panic!("the server's first line is not a port: {line:?}"));
        HttpServer { child, port, out }
    }

    /// The default limits with the request count and the knobs a case changes:
    /// `max_requests`, `max_body`, head/body/idle timeouts in ms, per connection.
    pub(crate) fn start_with(max_requests: u32, max_body: u32, head_ms: u32, body_ms: u32, idle_ms: u32, per_conn: u32) -> HttpServer {
        let args = [max_requests, max_body, head_ms, body_ms, idle_ms, per_conn].map(|n| n.to_string());
        let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        HttpServer::start("http_server", &refs)
    }

    pub(crate) fn start_default(max_requests: u32) -> HttpServer {
        HttpServer::start_with(max_requests, 1 << 20, 10_000, 30_000, 5_000, 100)
    }

    pub(crate) fn client(&self) -> HttpClient {
        HttpClient::connect(self.port)
    }

    /// Waits for the program to end (it ends by itself), checks it exited zero,
    /// and returns `(requests it answered, descriptors before, descriptors
    /// after)` from its last line. Equal descriptor counts are the leak and
    /// double-close gate: every connection and the listener were closed once.
    pub(crate) fn finish(mut self) -> (u64, u64, u64) {
        let deadline = Instant::now() + Duration::from_secs(30);
        let status = loop {
            match self.child.try_wait().expect("wait on the server") {
                Some(s) => break s,
                None => {
                    assert!(Instant::now() < deadline, "the server program did not end");
                    thread::sleep(Duration::from_millis(20));
                }
            }
        };
        let mut rest = String::new();
        self.out.read_to_string(&mut rest).unwrap();
        let mut err = String::new();
        if let Some(mut e) = self.child.stderr.take() {
            let _ = e.read_to_string(&mut err);
        }
        assert!(
            !err.contains("AddressSanitizer") && !err.contains("LeakSanitizer"),
            "AddressSanitizer flagged the server:\n{err}"
        );
        assert!(status.success(), "the server exited with {status:?}\nstdout: {rest}\nstderr: {err}");
        let last = rest.lines().last().unwrap_or_default().to_string();
        let words: Vec<&str> = last.split_whitespace().collect();
        assert!(
            words.len() == 5 && words[0] == "served" && words[2] == "fds",
            "unexpected last line {last:?}\nstderr: {err}"
        );
        let n = |i: usize| words[i].parse::<u64>().unwrap();
        let (served, before, after) = (n(1), n(3), n(4));
        assert_eq!(before, after, "the server leaked or double-closed a descriptor: {last}");
        (served, before, after)
    }
}

impl Drop for HttpServer {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

#[derive(Debug)]
pub(crate) struct HttpReply {
    pub(crate) status: u16,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: Vec<u8>,
}

impl HttpReply {
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub(crate) fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// A loopback client with a read timeout on every read.
pub(crate) struct HttpClient {
    pub(crate) stream: TcpStream,
    pub(crate) buf: Vec<u8>,
}

use std::net::TcpStream;

impl HttpClient {
    pub(crate) fn connect(port: u16) -> HttpClient {
        let stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to the server");
        stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        stream.set_write_timeout(Some(Duration::from_secs(10))).unwrap();
        HttpClient { stream, buf: Vec::new() }
    }

    /// Writes `bytes`, ignoring a failure: a server that has already refused the
    /// request may have closed, and the answer is still waiting to be read.
    pub(crate) fn send(&mut self, bytes: &[u8]) {
        let _ = self.stream.write_all(bytes);
    }

    /// Reads more into the buffer; false at end of stream or on a reset.
    pub(crate) fn fill(&mut self) -> bool {
        let mut chunk = [0u8; 65536];
        match self.stream.read(&mut chunk) {
            Ok(0) => false,
            Ok(n) => {
                self.buf.extend_from_slice(&chunk[..n]);
                true
            }
            Err(e) => {
                assert!(
                    e.kind() != std::io::ErrorKind::WouldBlock && e.kind() != std::io::ErrorKind::TimedOut,
                    "the server did not answer in time (buffered {:?})",
                    String::from_utf8_lossy(&self.buf)
                );
                false
            }
        }
    }

    /// The next response. `head_only` is for the answer to `HEAD`, whose
    /// `Content-Length` describes a body that is not sent.
    pub(crate) fn read_response(&mut self, head_only: bool) -> HttpReply {
        let end = loop {
            if let Some(i) = self.buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break i + 4;
            }
            assert!(
                self.fill(),
                "the connection ended before a response head arrived (buffered {:?})",
                String::from_utf8_lossy(&self.buf)
            );
        };
        let head = String::from_utf8_lossy(&self.buf[..end - 4]).to_string();
        self.buf.drain(..end);
        let mut lines = head.split("\r\n");
        let status_line = lines.next().unwrap();
        let status: u16 = status_line.split(' ').nth(1).unwrap().parse().unwrap();
        let headers: Vec<(String, String)> = lines
            .map(|l| {
                let (k, v) = l.split_once(':').unwrap();
                (k.to_string(), v.trim().to_string())
            })
            .collect();
        let mut reply = HttpReply { status, headers, body: Vec::new() };
        let bodyless = head_only || status < 200 || status == 204 || status == 304;
        if !bodyless {
            let len: usize = reply
                .header("content-length")
                .expect("a response with a body carries Content-Length")
                .parse()
                .unwrap();
            while self.buf.len() < len {
                assert!(self.fill(), "the connection ended inside a response body");
            }
            reply.body = self.buf.drain(..len).collect();
        }
        reply
    }

    /// True when the server has closed this connection and sent nothing more.
    pub(crate) fn at_eof(&mut self) -> bool {
        if !self.buf.is_empty() {
            return false;
        }
        !self.fill() && self.buf.is_empty()
    }
}

#[test]
pub(crate) fn http_server_answers_a_get_and_echoes_a_post() {
    let server = HttpServer::start_default(2);
    let mut c = server.client();
    c.send(b"GET / HTTP/1.1\r\nHost: t\r\n\r\n");
    let r = c.read_response(false);
    assert_eq!(r.status, 200);
    assert_eq!(r.text(), "hello");
    assert_eq!(r.header("content-type"), Some("text/plain; charset=utf-8"));
    assert_eq!(r.header("content-length"), Some("5"));
    assert_eq!(r.header("connection"), None);
    c.send(b"POST /echo HTTP/1.1\r\nHost: t\r\nContent-Length: 11\r\nConnection: close\r\n\r\nbinary\x00\xff\x01\x7f!");
    let r = c.read_response(false);
    assert_eq!(r.status, 200);
    assert_eq!(r.body, b"binary\x00\xff\x01\x7f!");
    assert_eq!(r.header("connection"), Some("close"));
    assert!(c.at_eof());
    assert_eq!(server.finish().0, 2);
}

#[test]
pub(crate) fn http_server_routes_on_path_segments_and_builds_responses() {
    let server = HttpServer::start_default(8);
    let mut c = server.client();
    let get = |c: &mut HttpClient, target: &str| {
        c.send(format!("GET {target} HTTP/1.1\r\nHost: t\r\n\r\n").as_bytes());
        c.read_response(false)
    };
    let r = get(&mut c, "/users/42");
    assert_eq!((r.status, r.text().as_str()), (200, "{\"id\":\"42\"}"));
    assert_eq!(r.header("content-type"), Some("application/json"));
    let r = get(&mut c, "/seg//a%2Fb/%7Ec/");
    assert_eq!(r.text(), "seg|a/b|~c");
    let r = get(&mut c, "/users");
    assert_eq!(r.status, 404);
    let r = get(&mut c, "/nope");
    assert_eq!((r.status, r.text().as_str()), (404, "not found\n"));
    // An empty response carries no body: a 204 has no Content-Length, a 418 has
    // a zero one.
    c.send(b"GET /nocontent HTTP/1.1\r\nHost: t\r\n\r\n");
    let r = c.read_response(false);
    assert_eq!(r.status, 204);
    assert_eq!(r.header("content-length"), None);
    let r = get(&mut c, "/teapot");
    assert_eq!((r.status, r.header("content-length")), (418, Some("0")));
    let r = get(&mut c, "/bad");
    assert_eq!((r.status, r.text().as_str()), (400, "nope"));
    // The request's own accessors, and a handler-added header.
    c.send(b"GET /hdr HTTP/1.1\r\nHost: t\r\nx-TEST:   padded value  \r\n\r\n");
    let r = c.read_response(false);
    assert_eq!(r.text(), "padded value|has|absent");
    assert_eq!(r.header("x-reply"), Some("yes"));
    assert_eq!(server.finish().0, 8);
}

#[test]
pub(crate) fn http_server_decodes_the_query_and_splits_the_target() {
    let server = HttpServer::start_default(2);
    let mut c = server.client();
    c.send(b"GET /q?name=J%C3%BCn%20e+x&n=7&n=8 HTTP/1.1\r\nHost: t\r\n\r\n");
    let r = c.read_response(false);
    assert_eq!(r.text(), "J\u{fc}n e x,7,<none>");
    assert_eq!(r.header("content-type"), Some("text/html; charset=utf-8"));
    c.send(b"GET /method/a?x=1?y=2 HTTP/1.1\r\nHost: t\r\n\r\n");
    let r = c.read_response(false);
    assert_eq!(r.text(), "GET /method/a?x=1?y=2 /method/a x=1?y=2");
    assert_eq!(server.finish().0, 2);
}

#[test]
pub(crate) fn http_server_keeps_a_connection_alive_for_three_requests() {
    let server = HttpServer::start_default(3);
    let mut c = server.client();
    for i in 0..3 {
        c.send(format!("POST /echo HTTP/1.1\r\nHost: t\r\nContent-Length: 1\r\n\r\n{i}").as_bytes());
        let r = c.read_response(false);
        assert_eq!(r.text(), i.to_string());
        // The server marks the last answer it will give (its request budget).
        assert_eq!(r.header("connection"), if i == 2 { Some("close") } else { None });
    }
    assert!(c.at_eof());
    assert_eq!(server.finish().0, 3);
}

#[test]
pub(crate) fn http_server_answers_pipelined_requests_in_order() {
    let server = HttpServer::start_default(4);
    let mut c = server.client();
    // Two requests in one write, the first with a body, then a second write
    // that carries two more: the read-ahead of one is the start of the next.
    c.send(b"POST /echo HTTP/1.1\r\nHost: t\r\nContent-Length: 5\r\n\r\nfirstGET /method/second HTTP/1.1\r\nHost: t\r\n\r\n");
    assert_eq!(c.read_response(false).text(), "first");
    assert_eq!(c.read_response(false).text(), "GET /method/second /method/second ");
    c.send(b"GET /seg/3 HTTP/1.1\r\nHost: t\r\n\r\nGET /seg/4 HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n");
    assert_eq!(c.read_response(false).text(), "seg|3");
    assert_eq!(c.read_response(false).text(), "seg|4");
    assert!(c.at_eof());
    assert_eq!(server.finish().0, 4);
}

#[test]
pub(crate) fn http_server_reads_a_chunked_request_body_and_discards_trailers() {
    let server = HttpServer::start_default(3);
    let mut c = server.client();
    c.send(b"POST /echo HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: Chunked\r\n\r\n4\r\nWiki\r\n5;ext=1\r\npedia\r\nE\r\n in\r\n\r\nchunks.\r\n0\r\nX-Trailer: a\r\nX-More: b\r\n\r\n");
    assert_eq!(c.read_response(false).text(), "Wikipedia in\r\n\r\nchunks.");
    // The trailers were consumed: the next request on the connection parses.
    c.send(b"POST /len HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n");
    assert_eq!(c.read_response(false).text(), "0");
    // A body sent one byte at a time still frames.
    let wire = b"POST /echo HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n3\r\nabc\r\n0\r\n\r\n";
    for b in wire.iter() {
        c.send(&[*b]);
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(c.read_response(false).text(), "abc");
    assert_eq!(server.finish().0, 3);
}

#[test]
pub(crate) fn http_server_sends_headers_only_for_head() {
    let server = HttpServer::start_default(3);
    let mut c = server.client();
    c.send(b"HEAD / HTTP/1.1\r\nHost: t\r\n\r\n");
    let r = c.read_response(true);
    assert_eq!((r.status, r.header("content-length")), (200, Some("5")));
    assert!(r.body.is_empty());
    // No body bytes followed: the next answer is the next request's.
    c.send(b"GET /method/x HTTP/1.1\r\nHost: t\r\n\r\n");
    assert_eq!(c.read_response(false).text(), "GET /method/x /method/x ");
    c.send(b"HEAD /nope HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n");
    let r = c.read_response(true);
    assert_eq!(r.status, 404);
    assert!(c.at_eof());
    assert_eq!(server.finish().0, 3);
}

#[test]
pub(crate) fn http_server_answers_expect_100_continue_before_the_body() {
    let server = HttpServer::start_default(2);
    let mut c = server.client();
    c.send(b"POST /echo HTTP/1.1\r\nHost: t\r\nContent-Length: 4\r\nExpect: 100-continue\r\n\r\n");
    // The interim answer arrives before any body byte is sent.
    let interim = c.read_response(false);
    assert_eq!(interim.status, 100);
    c.send(b"body");
    let r = c.read_response(false);
    assert_eq!((r.status, r.text().as_str()), (200, "body"));
    // A request with no body does not get one.
    c.send(b"GET / HTTP/1.1\r\nHost: t\r\nExpect: 100-continue\r\nConnection: close\r\n\r\n");
    assert_eq!(c.read_response(false).status, 200);
    assert_eq!(server.finish().0, 2);
}

#[test]
pub(crate) fn http_server_honours_connection_headers_and_http_1_0() {
    let server = HttpServer::start_default(5);
    // HTTP/1.0 closes unless it asks to keep the connection.
    let mut c = server.client();
    c.send(b"GET / HTTP/1.0\r\n\r\n");
    let r = c.read_response(false);
    assert_eq!((r.status, r.header("connection")), (200, Some("close")));
    assert!(c.at_eof());
    let mut c = server.client();
    c.send(b"GET / HTTP/1.0\r\nConnection: keep-alive\r\n\r\n");
    let r = c.read_response(false);
    assert_eq!(r.header("connection"), Some("keep-alive"));
    c.send(b"GET /seg/b HTTP/1.0\r\nConnection: Keep-Alive\r\n\r\n");
    assert_eq!(c.read_response(false).text(), "seg|b");
    c.send(b"GET / HTTP/1.0\r\n\r\n");
    assert_eq!(c.read_response(false).header("connection"), Some("close"));
    assert!(c.at_eof());
    // HTTP/1.1 with `Connection: close` closes; so does a handler that asks.
    let mut c = server.client();
    c.send(b"GET / HTTP/1.1\r\nHost: t\r\nConnection: keep-alive, Close\r\n\r\n");
    assert_eq!(c.read_response(false).header("connection"), Some("close"));
    assert!(c.at_eof());
    assert_eq!(server.finish().0, 5);
}

#[test]
pub(crate) fn http_server_closes_when_the_handler_asks_and_after_the_per_connection_cap() {
    // `/close` sets `Connection: close` itself.
    let server = HttpServer::start_default(3);
    let mut c = server.client();
    c.send(b"GET /close HTTP/1.1\r\nHost: t\r\n\r\n");
    let r = c.read_response(false);
    assert_eq!((r.text().as_str(), r.header("connection")), ("bye", Some("close")));
    assert!(c.at_eof());
    // A cap of 2 requests per connection: the second answer says close.
    let mut c = server.client();
    c.send(b"GET / HTTP/1.1\r\nHost: t\r\n\r\n");
    assert_eq!(c.read_response(false).header("connection"), None);
    c.send(b"GET / HTTP/1.1\r\nHost: t\r\n\r\n");
    assert_eq!(c.read_response(false).header("connection"), Some("close"));
    assert!(c.at_eof());
    assert_eq!(server.finish().0, 3);
    let server = HttpServer::start_with(4, 1 << 20, 10_000, 30_000, 5_000, 2);
    let mut c = server.client();
    for i in 0..2 {
        c.send(b"GET / HTTP/1.1\r\nHost: t\r\n\r\n");
        let r = c.read_response(false);
        assert_eq!(r.header("connection"), if i == 1 { Some("close") } else { None });
    }
    assert!(c.at_eof());
    let mut c = server.client();
    for _ in 0..2 {
        c.send(b"GET / HTTP/1.1\r\nHost: t\r\n\r\n");
        assert_eq!(c.read_response(false).status, 200);
    }
    assert!(c.at_eof());
    assert_eq!(server.finish().0, 4);
}

/// One request that breaks a limit or a framing rule, against a fresh server
/// with a 100-byte body limit and a 300 ms head timeout: the status it is
/// answered with, `Connection: close`, and a closed connection.
pub(crate) fn http_refused(request: &[u8], status: u16) {
    let server = HttpServer::start_with(1, 100, 300, 300, 300, 100);
    let mut c = server.client();
    c.send(request);
    let r = c.read_response(false);
    assert_eq!(
        r.status,
        status,
        "wrong status for {:?}: {r:?}",
        String::from_utf8_lossy(&request[..request.len().min(60)])
    );
    assert_eq!(r.header("connection"), Some("close"), "an error response closes");
    assert!(!r.body.is_empty(), "an error response explains itself");
    assert!(c.at_eof(), "the connection must close after a {status}");
    assert_eq!(server.finish().0, 1);
}

#[test]
pub(crate) fn http_server_refuses_oversized_requests_with_the_right_status() {
    // 431: one header over 16 KiB of headers, and more than 100 headers.
    let mut big = b"GET / HTTP/1.1\r\nHost: t\r\nX-Big: ".to_vec();
    big.extend(std::iter::repeat(b'a').take(17_000));
    big.extend_from_slice(b"\r\n\r\n");
    http_refused(&big, 431);
    let mut many = b"GET / HTTP/1.1\r\nHost: t\r\n".to_vec();
    for i in 0..101 {
        many.extend_from_slice(format!("X-{i}: v\r\n").as_bytes());
    }
    many.extend_from_slice(b"\r\n");
    http_refused(&many, 431);
    // 414: a target past the 8 KiB request-line limit.
    let mut long = b"GET /".to_vec();
    long.extend(std::iter::repeat(b'a').take(9_000));
    long.extend_from_slice(b" HTTP/1.1\r\nHost: t\r\n\r\n");
    http_refused(&long, 414);
    // 413: a Content-Length over the limit (the body is sent, the server never
    // reads it), a chunk over it, and chunks that add up past it.
    let mut body = b"POST /echo HTTP/1.1\r\nHost: t\r\nContent-Length: 101\r\n\r\n".to_vec();
    body.extend(std::iter::repeat(b'x').take(101));
    http_refused(&body, 413);
    http_refused(b"POST /echo HTTP/1.1\r\nHost: t\r\nContent-Length: 99999999999999999999999\r\n\r\n", 413);
    http_refused(b"POST /echo HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n65\r\n", 413);
    let mut sum = b"POST /echo HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
    for _ in 0..3 {
        sum.extend_from_slice(b"3C\r\n");
        sum.extend(std::iter::repeat(b'y').take(60));
        sum.extend_from_slice(b"\r\n");
    }
    http_refused(&sum, 413);
}

#[test]
pub(crate) fn http_server_refuses_malformed_heads_with_400() {
    http_refused(b"GARBAGE\r\n\r\n", 400);
    http_refused(b"GET /\r\n\r\n", 400);
    http_refused(b"GET  / HTTP/1.1\r\nHost: t\r\n\r\n", 400);
    http_refused(b"GET x HTTP/1.1\r\nHost: t\r\n\r\n", 400);
    http_refused(b"G(T / HTTP/1.1\r\nHost: t\r\n\r\n", 400);
    http_refused(b"GET / HTTP/x.y\r\nHost: t\r\n\r\n", 400);
    http_refused(b"\r\nGET / HTTP/1.1\r\nHost: t\r\n\r\n", 400);
    // A bare LF, a NUL, a byte above 127: heads are CRLF ASCII.
    http_refused(b"GET / HTTP/1.1\nHost: t\n\n", 400);
    http_refused(b"GET / HTTP/1.1\r\nHost: t\x00\r\n\r\n", 400);
    http_refused(b"GET / HTTP/1.1\r\nHost: t\xff\r\n\r\n", 400);
    // A header with no colon, a space before the colon, an empty name.
    http_refused(b"GET / HTTP/1.1\r\nHost t\r\n\r\n", 400);
    http_refused(b"GET / HTTP/1.1\r\nHost : t\r\n\r\n", 400);
    http_refused(b"GET / HTTP/1.1\r\n: t\r\n\r\n", 400);
    // Folded headers are refused, with a space and with a tab.
    http_refused(b"GET / HTTP/1.1\r\nHost: t\r\nX-A: one\r\n two\r\n\r\n", 400);
    http_refused(b"GET / HTTP/1.1\r\nHost: t\r\nX-A: one\r\n\ttwo\r\n\r\n", 400);
    // A chunk size that is not hexadecimal, and a chunk not followed by CRLF.
    http_refused(b"POST /echo HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n", 400);
    http_refused(b"POST /echo HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabcXY0\r\n\r\n", 400);
}

#[test]
pub(crate) fn http_server_refuses_request_smuggling_shapes() {
    // Content-Length twice with different values, a list, a sign, a non-number.
    http_refused(b"POST /echo HTTP/1.1\r\nHost: t\r\nContent-Length: 3\r\nContent-Length: 4\r\n\r\nabc", 400);
    http_refused(b"POST /echo HTTP/1.1\r\nHost: t\r\nContent-Length: 3, 3\r\n\r\nabc", 400);
    http_refused(b"POST /echo HTTP/1.1\r\nHost: t\r\nContent-Length: +3\r\n\r\nabc", 400);
    http_refused(b"POST /echo HTTP/1.1\r\nHost: t\r\nContent-Length: \r\n\r\n", 400);
    // Content-Length together with Transfer-Encoding, either order.
    http_refused(b"POST /echo HTTP/1.1\r\nHost: t\r\nContent-Length: 3\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n", 400);
    http_refused(b"POST /echo HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\nContent-Length: 3\r\n\r\n0\r\n\r\n", 400);
    // Two Transfer-Encoding headers; a chunked framing in HTTP/1.0.
    http_refused(b"POST /echo HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n", 400);
    http_refused(b"POST /echo HTTP/1.0\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n", 400);
    // An unknown transfer coding is 501, whether alone or before `chunked`.
    http_refused(b"POST /echo HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: gzip\r\n\r\n", 501);
    http_refused(b"POST /echo HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: gzip, chunked\r\n\r\n", 501);
}

#[test]
pub(crate) fn http_server_refuses_versions_expectations_and_missing_lengths() {
    http_refused(b"GET / HTTP/2.0\r\nHost: t\r\n\r\n", 505);
    http_refused(b"GET / HTTP/0.9\r\nHost: t\r\n\r\n", 505);
    http_refused(b"GET / HTTP/1.2\r\nHost: t\r\n\r\n", 505);
    http_refused(b"POST /echo HTTP/1.1\r\nHost: t\r\n\r\n", 411);
    http_refused(b"PUT /echo HTTP/1.1\r\nHost: t\r\n\r\n", 411);
    http_refused(b"GET / HTTP/1.1\r\nHost: t\r\nExpect: nonsense\r\n\r\n", 417);
}

#[test]
pub(crate) fn http_server_times_out_a_request_that_stalls() {
    // 408: the head stops halfway, and the body stops halfway (300 ms limits).
    let started = Instant::now();
    http_refused(b"GET / HTTP/1.1\r\nHo", 408);
    http_refused(b"POST /echo HTTP/1.1\r\nHost: t\r\nContent-Length: 10\r\n\r\nabc", 408);
    http_refused(b"POST /echo HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nab", 408);
    assert!(started.elapsed() < Duration::from_secs(20));
}

#[test]
pub(crate) fn http_server_cuts_off_a_slowloris_client_at_the_head_deadline() {
    // The head deadline is 500 ms. The client sends a valid request one byte
    // every 100 ms, which would take 4 s: every `recv` succeeds well inside any
    // per-read timeout, so only a deadline over the whole head ends this.
    let server = HttpServer::start_with(1, 1 << 20, 500, 500, 500, 100);
    let mut c = server.client();
    let mut writer = c.stream.try_clone().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    let wire = b"GET / HTTP/1.1\r\nHost: slow\r\nX-Pad: aaaaaaaaaaaaaaaaaaaa\r\n\r\n".to_vec();
    let trickler = thread::spawn(move || {
        for b in &wire[..wire.len() - 1] {
            if stop2.load(Ordering::SeqCst) || writer.write_all(&[*b]).is_err() {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
    });
    let started = Instant::now();
    let r = c.read_response(false);
    let took = started.elapsed();
    stop.store(true, Ordering::SeqCst);
    trickler.join().unwrap();
    assert_eq!(r.status, 408, "{r:?}");
    assert_eq!(r.header("connection"), Some("close"));
    assert!(took >= Duration::from_millis(400), "cut off before the deadline: {took:?}");
    assert!(took < Duration::from_millis(3000), "not cut off by the deadline: {took:?}");
    assert!(c.at_eof());
    assert_eq!(server.finish().0, 1);
}

#[test]
pub(crate) fn http_server_closes_an_idle_keep_alive_connection_without_an_answer() {
    // The idle limit is 300 ms: the connection is closed with no 408 (nothing
    // was being requested), and the server goes on to the next client.
    let server = HttpServer::start_with(2, 1 << 20, 10_000, 10_000, 300, 100);
    let mut c = server.client();
    c.send(b"GET / HTTP/1.1\r\nHost: t\r\n\r\n");
    assert_eq!(c.read_response(false).status, 200);
    let started = Instant::now();
    assert!(c.at_eof(), "the idle connection was not closed");
    assert!(started.elapsed() < Duration::from_secs(5));
    let mut c2 = server.client();
    c2.send(b"GET / HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n");
    assert_eq!(c2.read_response(false).status, 200);
    assert_eq!(server.finish().0, 2);
}

#[test]
pub(crate) fn http_server_survives_a_client_that_disconnects_mid_body() {
    let server = HttpServer::start_default(1);
    {
        let mut c = server.client();
        c.send(b"POST /echo HTTP/1.1\r\nHost: t\r\nContent-Length: 100\r\n\r\nonly ten b");
        // Dropped here: the server reads an end of stream inside the body.
    }
    {
        let mut c = server.client();
        c.send(b"POST /echo HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n9\r\nabc");
    }
    {
        let mut c = server.client();
        c.send(b"GET / HTTP/1.1\r\nHost: t\r\nX-Cut:");
    }
    let mut c = server.client();
    c.send(b"GET / HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n");
    assert_eq!(c.read_response(false).text(), "hello");
    assert_eq!(server.finish().0, 1);
}

#[test]
pub(crate) fn http_server_survives_a_client_that_resets_mid_response() {
    // `/big` is a 4 MiB body. The client reads a little and closes with the rest
    // unread, which the kernel turns into a reset: the server's write fails, it
    // closes its end and serves the next client. The reset request counts (its
    // handler ran), which is why the budget is 2.
    let server = HttpServer::start_default(2);
    {
        let mut c = server.client();
        c.send(b"GET /big HTTP/1.1\r\nHost: t\r\n\r\n");
        assert!(c.fill());
    }
    let mut c = server.client();
    c.send(b"GET / HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n");
    assert_eq!(c.read_response(false).text(), "hello");
    assert_eq!(server.finish().0, 2);
}

#[test]
pub(crate) fn http_server_serves_eight_concurrent_clients_on_four_workers() {
    // 4 worker threads accept on one listener. `/id` sleeps 200 ms and answers
    // with the serving thread's id, so a sequential server would show one id
    // and take 1.6 s.
    let server = HttpServer::start("http_server_threaded", &["4", "1500"]);
    let port = server.port;
    let started = Instant::now();
    let clients: Vec<_> = (0..8)
        .map(|_| {
            thread::spawn(move || {
                let mut c = HttpClient::connect(port);
                c.send(b"GET /id HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n");
                let r = c.read_response(false);
                assert_eq!(r.status, 200);
                r.text()
            })
        })
        .collect();
    let ids: Vec<String> = clients.into_iter().map(|h| h.join().unwrap()).collect();
    let took = started.elapsed();
    let mut distinct = ids.clone();
    distinct.sort();
    distinct.dedup();
    assert!(distinct.len() >= 3, "the clients were not served by separate workers: {ids:?}");
    assert!(distinct.len() <= 4, "more thread ids than workers: {ids:?}");
    assert!(took < Duration::from_millis(1500), "the clients were served one at a time: {took:?}");
    assert_eq!(server.finish().0, 8);
}

#[test]
pub(crate) fn http_listen_follows_the_net_grant_port() {
    // `http.listen` is `net.listen`: a grant that names other ports refuses it
    // before anything is bound, and a grant that covers the port lets a server
    // bind port 0 and report it.
    assert_project_denied_at_runtime(
        "caps_http_listen_denied",
        &project("caps_http_listen_denied").join("main.kp"),
        "127.0.0.1:1",
    );
    assert_project_runs(
        "caps_http_listen",
        &project("caps_http_listen").join("main.kp"),
        "listening\ntrue\n",
    );
}
