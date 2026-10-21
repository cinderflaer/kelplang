use super::*;

// ---------------------------------------------------------------------------
// S4: the `std/fs` path family, called through its raw externs.
//
// The fixture calls the raw externs directly rather than through the gated
// wrappers, so each body is exercised on its own.
// ---------------------------------------------------------------------------

#[test]
pub(crate) fn the_fs_path_family_reads_writes_and_walks() {
    // One call per extern: `exists`/`is_file`/`is_dir`/`is_symlink`, `mkdir`,
    // `mkdir_all` (twice, so the `EEXIST` tolerance is pinned), `write_text`,
    // `write`, `read`, `symlink`, `read_link`, `list` and `remove`. The listing
    // is sorted by `alphasort`, and the link is reported without being
    // descended into.
    let _scratch = TestScratch::new();
    let target = rerouted_copy(&fixture("tests/fixtures/fs_path_family.kp"), "fs_path_family_src");
    assert_project_runs(
        "fs_path_family",
        &target,
        &reroute_tmp("false\ntrue\nfalse\n6\nhello\n\ntrue\nxyz\ntrue\ntrue\nfalse\n/tmp/kelp_fs_path_family/a.txt\nfalse\na.txt\nlink\nsub\nfalse\n"),
    );
}

#[test]
pub(crate) fn fs_set_mode_changes_the_bits_metadata_reports() {
    // 420 is octal 644 and 493 is octal 755.
    let _scratch = TestScratch::new();
    let target = rerouted_copy(&fixture("tests/fixtures/fs_set_mode.kp"), "fs_set_mode_src");
    assert_project_runs("fs_set_mode", &target, "420\n493\n");
}

// ---------------------------------------------------------------------------
// S2: `io` (the standard descriptors as streams, buffered line reading) and
// `process` (argv, running a program, spawning one).
//
// `io` is ungated, so its fixtures are ordinary single files; `process` is
// gated, so its granted and denied cases live in projects, where the grant is
// written at the `impl` line.
// ---------------------------------------------------------------------------

#[test]
pub(crate) fn io_reads_stdin_as_lines_and_writes_the_standard_descriptors() {
    // One `io.read_line`, then `io.read_lines` for the rest, through the same
    // process-wide reader; a `File` over stdout writing a borrowed view; and
    // `eprint` on stderr, which the harness checks only for "did not fail".
    // The last line is deliberately not newline-terminated: the reader has to
    // return it at end of file rather than dropping it, which is the case a
    // `read_line` written as "read until newline" gets wrong.
    assert_fixture_runs_with_stdin(
        "io_stdio",
        "a\nb\nc\nd",
        "a\n3\nb\nc\nd\ndirect6\ntrue\ntrue\n",
    );
}

#[test]
pub(crate) fn io_reports_end_of_file_instead_of_a_line() {
    // The same program with nothing on stdin: `ok = false` on the first read,
    // no lines, and a clean exit.
    run_fixture_with_stdin(
        "io_stdio_eof",
        "io_stdio",
        "",
        "(no header)\n0\ndirect6\ntrue\ntrue\n",
    );
}

#[test]
pub(crate) fn a_held_buf_reader_reads_lines_and_consumes_the_rest() {
    // The buffered type is an owned `datac`, so this is the caller-threaded
    // form: one line out, the remainder of the stream, then `read_into` at end
    // of file (0 bytes, buffer untouched).
    assert_fixture_runs_with_stdin(
        "io_streams",
        "l1\nl2\n",
        "true\nl1\n3\nl2\n\ntrue\n0\n4\ntrue\ntrue\nfalse\n",
    );
}

#[test]
pub(crate) fn io_writes_bytes_text_and_a_borrowed_view() {
    // The three write bodies take their buffer as a borrow (`const kelp_Bytes*`
    // in the C signature), which a body that read the outgoing argument area
    // would miss and write nothing: each returns the byte count it wrote, and
    // the raw bytes reach stdout ahead of the buffered `print` because the body
    // flushes the matching stdio stream first.
    assert_compiles_and_runs(
        "tests/fixtures/io_write_bodies.kp",
        "bytes\n6\ntext\n5\nstr\n4\n",
    );
}

#[test]
pub(crate) fn process_runs_a_program_with_argv_and_spawns_one_with_pipes() {
    // `run` with a real argv array (the child prints its own `$1`/`$2`), an
    // exit code, separated stdout/stderr capture, then `spawn` with a write to
    // the child's stdin and a read of its stdout, `is_running`/`kill`/`wait`
    // on a child that is blocked, and a failed `exec` reported as code 127.
    // Also under AddressSanitizer, where the parent's buffers and pipes are
    // leak-checked.
    assert_compiles_and_runs(
        "tests/fixtures/process_spawn.kp",
        "0\none-two\n7\n0\n0\noops\n0\nhello\n\ntrue\n137\nfalse\n127\ntrue\n1\ntrue\ntrue\n",
    );
}

#[test]
pub(crate) fn a_granted_module_runs_a_program_and_reads_its_own_argv() {
    // The grant is `runner`'s, not `main`'s: `process.run` is checked against
    // the patterns of the module that named it, and `process.args` is behind
    // the same gate even though it names no resource.
    assert_project_runs_with_args(
        "process_run",
        &project("process_run").join("main.kp"),
        &["one", "two"],
        "0\nhi\n3\noops\none,two\n3\n",
    );
}

#[test]
pub(crate) fn a_signal_killed_child_is_told_apart_from_one_that_exited_with_that_code() {
    assert_project_runs(
        "process_signal",
        &project("process_signal").join("main.kp"),
        "143\n15\n143\n0\n9\n",
    );
}

#[test]
pub(crate) fn run_inherit_leaves_the_child_on_the_parents_descriptors() {
    // The child's `echo` lands between the parent's own prints, in order,
    // because the parent flushes before the fork and nothing is captured.
    assert_project_runs(
        "process_inherit",
        &project("process_inherit").join("main.kp"),
        "before\nfrom the child\nafter\n3\n0\n0\n15\n",
    );
}

#[test]
pub(crate) fn reading_argv_needs_the_gate_but_no_program_grant() {
    // An empty pattern list is a real grant: `runner` may name `std/process`
    // (so `process.args`/`process.exit` resolve) and may not execute anything.
    // It also pins the emitted allowlist for an empty grant, which used to be
    // skipped and left the generated C naming a symbol that did not exist.
    assert_project_runs_with_args(
        "process_argv_only",
        &project("process_argv_only").join("main.kp"),
        &["alpha", "beta"],
        "3\nalpha,beta\n",
    );
}

#[test]
pub(crate) fn a_granted_module_cannot_run_a_program_outside_its_grant() {
    // `runner` holds `process = ["/bin/true"]` and asks for `/bin/sh`; the
    // check runs before the fork, and the denial names the module, the call
    // site, and the grant.
    assert_project_denied_at_runtime(
        "process_denied",
        &project("process_denied").join("main.kp"),
        "is outside the process capability granted to module 'runner'",
    );
}

#[test]
pub(crate) fn a_child_gets_a_working_directory_and_a_fallible_run_reports_why() {
    // The marker files are the proof that `chdir` happened in the *child*: the
    // shell's relative redirection lands in the directory the parent named, and
    // the parent's own directory is untouched. `/tmp` as a working directory
    // (`dir_outside_grant`) pins the other half: a cwd is not a resource, so it
    // is not matched against the program allowlist.
    let dir = caps_scratch("process_in");
    std::fs::write(dir.join("not-exec"), "x").unwrap();
    assert_project_runs(
        "process_in",
        &caps_project("process_in").join("main.kp"),
        "0\ntrue\n0\n126\ntrue\nout-of-grant-cwd\ntrue\nthere\nnot-found\nnot-executable\ndir-not-found\nok\n",
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("marker.txt")).unwrap(),
        "hi",
        "`run_in` did not run the child in the directory it was given"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("spawned.txt")).unwrap(),
        "hi2",
        "`spawn_in` did not run the child in the directory it was given"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// S3: `net`, the capability vocabulary (`host`, `host:port`, `*.host`,
// `*:port`, `*`), sockets, and the one-process loopback test.
//
// Every `net` test is one process, loopback only, and never a fixed port: the
// listener binds 0 and `Listener.port` reports what the kernel assigned. The
// internet is never touched, so nothing here can flake on a network.
// ---------------------------------------------------------------------------

#[test]
pub(crate) fn the_entry_module_holds_net_as_the_whole_resource_wildcard() {
    // A single-file program, so `main` holds every domain with a `*` pattern:
    // `net` resolves with no `caps(...)` clause, and the runtime check is
    // the `*` the vocabulary gained in S3 (before it, `net` was a path domain
    // and every host would have been denied by the path normalizer).
    assert_compiles_and_runs("tests/fixtures/net_loopback.kp", "hello\n");
}

#[test]
pub(crate) fn a_granted_module_speaks_the_net_vocabulary_and_round_trips_bytes() {
    // The grant is `peer`'s, not `main`'s, and it is written in the vocabulary
    // open item 32 was about: `127.0.0.1:*`, a host and a wildcard port. The
    // program binds port 0, learns the port, connects, accepts, and exchanges
    // bytes in both directions, printing `send`'s byte count, `recv_into`'s,
    // both payloads, and whether the peer address is the requested loopback.
    assert_project_runs(
        "caps_net",
        &project("caps_net").join("main.kp"),
        "4 4 +more pong loopback bound\n",
    );
}

#[test]
pub(crate) fn the_net_check_matches_the_host_as_written() {
    // `util` holds `127.0.0.1:*` and connects to `localhost`, the same
    // machine under another name. The check runs before `getaddrinfo`, so this
    // is denied, and the denial prints the string the code wrote. The granted
    // pattern (not the host as written) is what the assertion reads, per the
    // determinism rule.
    assert_project_denied_at_runtime(
        "caps_net_denied",
        &project("caps_net_denied").join("main.kp"),
        "127.0.0.1:*",
    );
}

#[test]
pub(crate) fn a_socket_reports_readiness_without_blocking() {
    // `poll` on a connected socket: false on a bounded wait with nothing sent,
    // true once the peer writes, and true again at end of file after
    // `shutdown_write`, the three answers a single-threaded event loop needs.
    assert_compiles_and_runs("tests/fixtures/net_poll.kp", "false\ntrue\ntrue\nnow\ntrue\ntrue\n");
}

#[test]
pub(crate) fn resolving_a_name_returns_its_addresses() {
    // An address literal resolves to itself (deterministic); `localhost` is
    // asserted only as "resolved to something", since which addresses it maps
    // to is the machine's business. Never the internet.
    assert_compiles_and_runs(
        "tests/fixtures/net_resolve.kp",
        "1\n127.0.0.1\ntrue\n",
    );
}

#[test]
pub(crate) fn a_silent_peer_times_out_instead_of_hanging() {
    // `set_timeout` makes a blocking `recv` bounded: the panic names the
    // timeout rather than the program hanging until the harness gives up.
    assert_fixture_panics("net_timeout", "net.recv: timed out");
}

#[test]
pub(crate) fn resolving_a_host_outside_the_grant_is_denied() {
    // The `Host` check (no port) is enforced too, and the denial happens before
    // `getaddrinfo`, so this test never touches the network.
    assert_project_denied_at_runtime(
        "caps_net_resolve_denied",
        &project("caps_net_resolve_denied").join("main.kp"),
        "is outside the net capability granted to module 'util'",
    );
}

#[test]
pub(crate) fn a_net_pattern_that_is_not_a_host_is_rejected() {
    // The `net` vocabulary has its own validator now, so this is not the
    // "patterns are absolute paths" error a path domain gives: it names what a
    // host pattern looks like.
    assert_project_rejected(
        "caps_bad_net_pattern",
        &project("caps_bad_net_pattern").join("main.kp"),
        "is not a host",
    );
}

/// The denial's first field is `host:port` as the code wrote it, so the
/// port half is the *call's* own argument rather than the peer's address. That
/// string can only come out right when the host and the port were read from
/// their own argument slots, which makes this the behavioural pin for the
/// argument-position trap that `caps_net`'s admitted case cannot be (a swap
/// there would deny a call the grant covers, and the failure would read as a
/// missing grant rather than a misread argument).
#[test]
pub(crate) fn the_host_port_denial_names_the_host_as_written_and_its_own_port() {
    assert_project_denied_at_runtime(
        "caps_net_denied",
        &project("caps_net_denied").join("main.kp"),
        "'localhost:1' is outside the net capability granted to module 'util'",
    );
}

#[test]
pub(crate) fn a_refused_dial_is_a_value_and_not_the_end_of_the_program() {
    // `try_connect` against a port the program itself released: the bind, the
    // port the kernel assigned and the close are the whole preparation, so the
    // refusal is deterministic and no server is needed. The reason is the text
    // `net.connect` panics with, and the process exits 0 rather than dying.
    assert_compiles_and_runs(
        "tests/fixtures/net_try_connect.kp",
        "err net.connect: connection failed\n  errno 111: Connection refused\nsurvived\n",
    );
}

/// The `net` half of the fallible dial, denied: a fallible form must not be a
/// way around the grant the panicking one obeys. `util` holds `127.0.0.1:*` and
/// asks for `localhost`, so the check refuses before any resolution runs and no
/// `Err` is ever built.
#[test]
pub(crate) fn the_fallible_dial_is_checked_like_the_panicking_one() {
    assert_project_denied_at_runtime(
        "caps_net_try_denied",
        &project("caps_net_try_denied").join("main.kp"),
        "'localhost:1' is outside the net capability granted to module 'util'",
    );
}

#[test]
pub(crate) fn a_reconnect_loop_survives_three_refused_dials_and_gives_up() {
    // The fixture that could not exist before this: three dials at a port
    // nobody owns, one line per attempt, and a clean exit at the end instead
    // of a panic on the first one. The fpl discord library's gateway loop is
    // this shape, and `MISSING-w9Kda` is the dial that ended it.
    assert_compiles_and_runs(
        "tests/fixtures/net_try_connect_retry.kp",
        "attempt 1: net.connect: connection failed\n  errno 111: Connection refused\n\
         attempt 2: net.connect: connection failed\n  errno 111: Connection refused\n\
         attempt 3: net.connect: connection failed\n  errno 111: Connection refused\n\
         gave up\n",
    );
}

// ---------------------------------------------------------------------------
// S4: `std/http`. The client blocks inside `send` until the response arrives,
// so, unlike S3's `net` fixtures, which could be both ends in one process,
// an HTTP test needs a real server in another process (or, here, in another
// thread of the harness).
//
// The helper binds loopback, hands the port to the compiled program through
// argv, serves canned bytes, and counts accepted connections. That count is
// the point: it turns "the pool reused the socket" into an assertion instead of
// a timing inference, and it is the only way to see a connection that was made
// and not reused.
// ---------------------------------------------------------------------------

/// A canned 200 with a body and one extra header.
pub(crate) const BODY_200_HELLO: &[u8] =
    b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nX-Test: yes\r\n\r\nhello";

/// The same, with no `Content-Length`: the body ends at end of file, which is
/// the one framing that cannot be pooled.
pub(crate) const BODY_200_CLOSE: &[u8] = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\nbyebye";

/// `chunk!`, split into two chunks and terminated, with no trailers.
pub(crate) const CHUNKED_200: &[u8] =
    b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nchunk\r\n1\r\n!\r\n0\r\n\r\n";

pub(crate) const STATUS_404: &[u8] = b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n";

/// MISSING-p5Xtr: a `429` naming a delay and a spent per-route bucket, in the
/// shape Discord (the issue's worked example) sends it.
pub(crate) const STATUS_429_RATE_LIMITED: &[u8] = b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 2\r\nX-RateLimit-Limit: 10\r\nX-RateLimit-Remaining: 0\r\nX-RateLimit-Reset: 1719796939\r\nX-RateLimit-Reset-After: 0.500\r\nX-RateLimit-Bucket: abcd1234\r\nContent-Length: 0\r\n\r\n";

/// The answer the `ws.try_connect` gate serves: a peer that is not a WebSocket
/// server, which is the `Handshake` half of that call's error type.
pub(crate) const STATUS_403: &[u8] = b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n";

/// A relative redirect, so the follow-up keeps the same host (and therefore the
/// same pooled connection).
pub(crate) const REDIRECT_301: &[u8] =
    b"HTTP/1.1 301 Moved Permanently\r\nLocation: /a\r\nContent-Length: 0\r\n\r\n";

/// A redirect to a *different name* for the same machine. Port 80 is never
/// reached: the capability check denies the host string first, which is the
/// point of the test.
pub(crate) const REDIRECT_ELSEWHERE: &[u8] =
    b"HTTP/1.1 301 Moved Permanently\r\nLocation: http://localhost/a\r\nContent-Length: 0\r\n\r\n";

/// A 200 whose body echoes the request's, so a POST can assert what arrived.
pub(crate) fn echo_response(body: &str) -> Vec<u8> {
    format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\ngot:{body}", body.len() + 4)
        .into_bytes()
}

/// One connection carrying `responses`, in order.
pub(crate) fn one_connection(responses: &[&[u8]]) -> Vec<Vec<Vec<u8>>> {
    vec![responses.iter().map(|r| r.to_vec()).collect()]
}

/// `n` connections, one response each.
pub(crate) fn n_connections(response: &[u8], n: usize) -> Vec<Vec<Vec<u8>>> {
    (0..n).map(|_| vec![response.to_vec()]).collect()
}

/// A loopback server serving canned responses, and counting the connections it
/// accepted. It lives in a thread for the duration of one test.
pub(crate) struct CannedServer {
    pub(crate) port: u16,
    pub(crate) accepted: Arc<AtomicUsize>,
    pub(crate) handle: Option<thread::JoinHandle<()>>,
}

impl CannedServer {
    /// `connections[i]` is the list of responses to write on the `i`-th
    /// accepted connection, one per request read from it.
    pub(crate) fn start(connections: Vec<Vec<Vec<u8>>>) -> CannedServer {
        let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind loopback");
        let port = listener.local_addr().expect("local address").port();
        let accepted = Arc::new(AtomicUsize::new(0));
        let counter = accepted.clone();
        let handle = thread::spawn(move || serve(listener, connections, counter));
        CannedServer { port, accepted, handle: Some(handle) }
    }

    pub(crate) fn accepted(&self) -> usize {
        self.accepted.load(Ordering::SeqCst)
    }

    /// Waits for the serving thread, so the connection count is final.
    pub(crate) fn finish(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Reads one request: the head, plus the `Content-Length` bytes of body. Returns
/// false when the peer closed before a request arrived. Generic over `Read`
/// rather than `TcpStream` specifically, so the TLS server below
/// (`CannedTlsServer`) can run the identical request-framing logic over an
/// `SslStream<TcpStream>` instead of duplicating it.
pub(crate) fn read_http_request<S: Read>(stream: &mut S) -> bool {
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..pos]).to_string();
            let mut want = 0usize;
            for line in head.split("\r\n").skip(1) {
                if let Some((name, value)) = line.split_once(':') {
                    if name.eq_ignore_ascii_case("content-length") {
                        want = value.trim().parse().unwrap_or(0);
                    }
                }
            }
            let mut have = buf.len() - (pos + 4);
            while have < want {
                match stream.read(&mut chunk) {
                    Ok(0) => return false,
                    Ok(n) => {
                        buf.extend_from_slice(&chunk[..n]);
                        have += n;
                    }
                    Err(_) => return false,
                }
            }
            return true;
        }
        match stream.read(&mut chunk) {
            Ok(0) => return false,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(_) => return false,
        }
    }
}

pub(crate) fn serve(
    listener: TcpListener,
    connections: Vec<Vec<Vec<u8>>>,
    accepted: Arc<AtomicUsize>,
) {
    let _ = listener.set_nonblocking(true);
    // A bounded wait: if the client opens fewer connections than the test
    // canned (because it failed, or reused more than expected), the thread
    // gives up and the assertion on `accepted` reports the difference. Without
    // this, a wrong client would hang the suite instead of failing it.
    let deadline = Instant::now() + ACCEPT_DEADLINE;
    for responses in connections {
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
        let Some(mut stream) = stream.take() else { return };
        // Non-blocking was only for the accept loop: an accepted socket
        // inherits the listener's mode on Linux, and a non-blocking read here
        // would report `WouldBlock` for a request that is still in flight.
        let _ = stream.set_nonblocking(false);
        accepted.fetch_add(1, Ordering::SeqCst);
        for response in &responses {
            if !read_http_request(&mut stream) {
                break;
            }
            if stream.write_all(response).is_err() {
                break;
            }
        }
        // Dropping the stream closes it, which is what a server that has said
        // everything it intends to say does. A client that tries to reuse a
        // connection the server has closed sees end of file, exactly as it
        // would against a real peer that timed the connection out.
    }
}
