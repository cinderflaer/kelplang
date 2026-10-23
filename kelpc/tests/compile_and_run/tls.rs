use super::*;

// ---------------------------------------------------------------------------
// MK5-S4b-M1 (open item 37): `std/tls`'s and `https`'s own test server. Same
// canned-connections shape as `CannedServer` above, except every accepted
// `TcpStream` goes through an `SslAcceptor`'s handshake first, using a
// throwaway self-signed certificate generated once per server (the `openssl`
// crate, not a CLI subprocess, see `Cargo.toml`'s `[dev-dependencies]`
// comment). The compiled `.kp` binary under test is started with
// `SSL_CERT_FILE` pointing at that certificate's own PEM file, which is what
// lets `std/tls`'s unconditional `SSL_CTX_set_default_verify_paths()`
// (there is no `insecure` flag in v1, on purpose) accept it: OpenSSL reads
// that variable as an override of the compiled-in trust store, so this needs
// no write to the machine's real one.
// ---------------------------------------------------------------------------

/// A throwaway self-signed certificate and key for one test server, PEM-encoded
/// on disk (`openssl::ssl::SslAcceptor::set_certificate_file` and the
/// `SSL_CERT_FILE` env var both want a path, not bytes in memory). Subject
/// and SAN are both `127.0.0.1`, since every fixture that uses this dials
/// that literal.
pub(crate) struct TestCert {
    pub(crate) cert_path: PathBuf,
    pub(crate) key_path: PathBuf,
    // Kept alive only so the temp directory is not removed while the paths
    // above are still in use; nothing reads it. Shared rather than owned, so
    // one certificate can back two servers (the `https` redirect case needs a
    // second local port that the client's one `SSL_CERT_FILE` also trusts).
    pub(crate) _dir: Arc<tempfile_dir::TempDir>,
}

impl Clone for TestCert {
    fn clone(&self) -> TestCert {
        TestCert {
            cert_path: self.cert_path.clone(),
            key_path: self.key_path.clone(),
            _dir: self._dir.clone(),
        }
    }
}

/// A tiny drop-cleaned temp directory, written by hand rather than pulling in
/// a `tempfile` dependency for one struct: `std::env::temp_dir()` plus the
/// test's own pid and an atomic counter (two TLS fixtures in one process
/// would otherwise collide on one directory), removed on `Drop`.
mod tempfile_dir {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    pub struct TempDir(PathBuf);

    impl TempDir {
        pub fn new(label: &str) -> TempDir {
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let dir = std::env::temp_dir()
                .join(format!("kelpc_{label}_{}_{n}", std::process::id()));
            std::fs::create_dir_all(&dir).expect("failed to create a scratch directory");
            TempDir(dir)
        }

        pub fn path(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

/// Which names the throwaway certificate carries. `LoopbackIp` is the server
/// every trusted fixture dials as `127.0.0.1`; `DnsLocalhost` is the same key
/// with only a DNS SAN, so dialing `127.0.0.1` is a name mismatch.
pub(crate) enum CertSubject {
    LoopbackIp,
    DnsLocalhost,
}

pub(crate) fn generate_test_cert() -> TestCert {
    generate_test_cert_with(CertSubject::LoopbackIp, true)
}

/// As `generate_test_cert`, with `notBefore`/`notAfter` in the past: the
/// certificate a machine's clock has run past, which verification must refuse.
pub(crate) fn generate_expired_test_cert() -> TestCert {
    generate_test_cert_with(CertSubject::LoopbackIp, false)
}

/// As `generate_test_cert`, with `localhost` as its only SAN.
pub(crate) fn generate_mismatched_test_cert() -> TestCert {
    generate_test_cert_with(CertSubject::DnsLocalhost, true)
}

pub(crate) fn generate_test_cert_with(subject: CertSubject, valid: bool) -> TestCert {
    use openssl::asn1::Asn1Time;
    use openssl::bn::{BigNum, MsbOption};
    use openssl::hash::MessageDigest;
    use openssl::pkey::PKey;
    use openssl::rsa::Rsa;
    use openssl::x509::extension::{BasicConstraints, SubjectAlternativeName};
    use openssl::x509::{X509, X509NameBuilder};

    let rsa = Rsa::generate(2048).expect("failed to generate an RSA key for the test cert");
    let key = PKey::from_rsa(rsa).expect("failed to wrap the RSA key");

    let mut name = X509NameBuilder::new().expect("failed to start a name builder");
    name.append_entry_by_text("CN", "127.0.0.1").expect("failed to set CN");
    let name = name.build();

    let mut serial = BigNum::new().expect("failed to allocate a serial number");
    serial
        .rand(64, MsbOption::MAYBE_ZERO, false)
        .expect("failed to randomize the serial number");

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs() as i64;
    let (not_before, not_after) = if valid {
        (
            Asn1Time::days_from_now(0).expect("not-before time"),
            Asn1Time::days_from_now(1).expect("not-after time"),
        )
    } else {
        (
            Asn1Time::from_unix(now - 3 * 86400).expect("past not-before time"),
            Asn1Time::from_unix(now - 2 * 86400).expect("past not-after time"),
        )
    };

    let mut builder = X509::builder().expect("failed to start a certificate builder");
    builder.set_version(2).expect("failed to set the certificate version");
    builder
        .set_serial_number(&serial.to_asn1_integer().expect("serial as an ASN.1 integer"))
        .expect("failed to set the serial number");
    builder.set_subject_name(&name).expect("failed to set the subject name");
    builder.set_issuer_name(&name).expect("failed to set the issuer name (self-signed)");
    builder.set_pubkey(&key).expect("failed to set the public key");
    builder
        .set_not_before(&not_before)
        .expect("failed to set not-before");
    builder
        .set_not_after(&not_after)
        .expect("failed to set not-after");
    builder
        .append_extension(BasicConstraints::new().critical().build().expect("basic constraints"))
        .expect("failed to append basic constraints");
    let mut san = SubjectAlternativeName::new();
    match subject {
        CertSubject::LoopbackIp => {
            san.ip("127.0.0.1");
        }
        CertSubject::DnsLocalhost => {
            san.dns("localhost");
        }
    }
    builder
        .append_extension(san.build(&builder.x509v3_context(None, None)).expect("subject alternative name"))
        .expect("failed to append the subject alternative name");
    builder
        .sign(&key, MessageDigest::sha256())
        .expect("failed to self-sign the test certificate");
    let cert = builder.build();

    let dir = tempfile_dir::TempDir::new("tls_test_cert");
    let cert_path = dir.path().join("cert.pem");
    let key_path = dir.path().join("key.pem");
    std::fs::write(&cert_path, cert.to_pem().expect("certificate as PEM"))
        .expect("failed to write the test certificate");
    std::fs::write(&key_path, key.private_key_to_pem_pkcs8().expect("private key as PEM"))
        .expect("failed to write the test private key");
    TestCert { cert_path, key_path, _dir: Arc::new(dir) }
}

/// How long a test server waits for the client to connect. The server starts
/// before the client fixture is built, and a fixture that imports `std/str`
/// takes tens of seconds to compile (the case table), so this must cover the
/// build, not just the connect.
pub(crate) const ACCEPT_DEADLINE: Duration = Duration::from_secs(120);

/// As `CannedServer`, but every accepted connection is a TLS server socket
/// (`SslAcceptor::accept`) before the same canned request/response loop runs
/// over it.
pub(crate) struct CannedTlsServer {
    pub(crate) port: u16,
    pub(crate) accepted: Arc<AtomicUsize>,
    pub(crate) handle: Option<thread::JoinHandle<()>>,
    pub(crate) cert: TestCert,
}

impl CannedTlsServer {
    pub(crate) fn start(connections: Vec<Vec<Vec<u8>>>) -> CannedTlsServer {
        CannedTlsServer::start_with_cert(generate_test_cert(), connections)
    }

    /// The same server over a caller-supplied certificate (the expired and
    /// host-mismatch fixtures mint their own) and, for the TLS-1.1 case, a
    /// caller-supplied protocol window.
    pub(crate) fn start_with_cert(cert: TestCert, connections: Vec<Vec<Vec<u8>>>) -> CannedTlsServer {
        CannedTlsServer::start_with(cert, connections, None)
    }

    pub(crate) fn start_with(
        cert: TestCert,
        connections: Vec<Vec<Vec<u8>>>,
        version: Option<openssl::ssl::SslVersion>,
    ) -> CannedTlsServer {
        let acceptor = tls_acceptor(&cert, version);
        let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind loopback");
        let port = listener.local_addr().expect("local address").port();
        let accepted = Arc::new(AtomicUsize::new(0));
        let counter = accepted.clone();
        let handle = thread::spawn(move || {
            serve_tls(listener, acceptor, connections, counter)
        });
        CannedTlsServer { port, accepted, handle: Some(handle), cert }
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

/// An `SslAcceptor` for `cert`, optionally pinned to one protocol version.
/// `Some(TLS1_1)` needs security level 0, because OpenSSL 3 refuses to
/// negotiate TLS 1.1 at the default level; the point of that server is the
/// protocol alert, not a working connection.
pub(crate) fn tls_acceptor(cert: &TestCert, version: Option<openssl::ssl::SslVersion>) -> openssl::ssl::SslAcceptor {
    use openssl::ssl::{SslAcceptor, SslFiletype, SslMethod};

    let mut builder = SslAcceptor::mozilla_intermediate_v5(SslMethod::tls_server())
        .expect("failed to start an SslAcceptor");
    builder
        .set_private_key_file(&cert.key_path, SslFiletype::PEM)
        .expect("failed to load the test private key");
    builder
        .set_certificate_file(&cert.cert_path, SslFiletype::PEM)
        .expect("failed to load the test certificate");
    if let Some(v) = version {
        builder.set_min_proto_version(Some(v)).expect("failed to set the minimum version");
        builder.set_max_proto_version(Some(v)).expect("failed to set the maximum version");
        builder.set_security_level(0);
    }
    builder.build()
}

pub(crate) fn serve_tls(
    listener: TcpListener,
    acceptor: openssl::ssl::SslAcceptor,
    connections: Vec<Vec<Vec<u8>>>,
    accepted: Arc<AtomicUsize>,
) {
    let _ = listener.set_nonblocking(true);
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
        let Some(stream) = stream.take() else { return };
        let _ = stream.set_nonblocking(false);
        let mut tls = match acceptor.accept(stream) {
            Ok(tls) => tls,
            // A client that never completes the handshake (the denial
            // fixtures: the capability check panics before dialing at all)
            // just means this connection is never accepted, which the
            // caller's `accepted()` assertion already covers.
            Err(_) => continue,
        };
        accepted.fetch_add(1, Ordering::SeqCst);
        for response in &responses {
            if !read_http_request(&mut tls) {
                break;
            }
            if tls.write_all(response).is_err() {
                break;
            }
        }
        let _ = tls.shutdown();
    }
}

/// As `assert_project_served`, but the server is `CannedTlsServer` and the
/// compiled binary is started with `SSL_CERT_FILE` pointing at the server's
/// own self-signed certificate, so `std/tls`'s ordinary (non-optional)
/// certificate verification accepts it.
pub(crate) fn assert_project_served_tls(
    name: &str,
    target: &Path,
    connections: Vec<Vec<Vec<u8>>>,
    expected_connections: usize,
    expected: &str,
) {
    let mut server = CannedTlsServer::start(connections);
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
        .env("SSL_CERT_FILE", &server.cert.cert_path)
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
        "'{name}' made an unexpected number of TLS connections"
    );
}

/// As `assert_project_served_denied`, for the TLS server: the capability
/// check runs before any connection, TLS or otherwise, so this needs no
/// `SSL_CERT_FILE`, the denial happens before a handshake could use it.
pub(crate) fn assert_project_served_tls_denied(
    name: &str,
    target: &Path,
    connections: Vec<Vec<Vec<u8>>>,
    needle: &str,
) {
    let mut server = CannedTlsServer::start(connections);
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
        .env("SSL_CERT_FILE", &server.cert.cert_path)
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

// ---------------------------------------------------------------------------
// Release goal 1 (stable TLS): the local failure matrix. Every fixture dials
// `127.0.0.1` only. The certificate is minted per server, so no machine store
// is touched: a trusted case points `SSL_CERT_FILE` at the server's own PEM,
// an untrusted one removes that variable, and the private-CA case passes the
// PEM path as an argument and leaves the store alone.
// ---------------------------------------------------------------------------

/// What a one-connection TLS server does after `accept`.
pub(crate) enum TlsBehavior {
    /// Never answer the ClientHello, so the client's handshake times out.
    NeverHandshake,
    /// Complete the handshake, then close the raw socket with no
    /// `close_notify`: the truncation case.
    Truncate,
    /// Complete the handshake, then stay silent until the test says stop.
    Silent,
    /// Complete the handshake, write a partial response head and body, then
    /// write a raw record the client cannot authenticate and hold. The read of
    /// it has to be an error rather than a clean end of stream.
    CorruptRecord,
    /// Accept the TCP connection and close it without ever reading the
    /// ClientHello. That unread data turns the close into a RST, which is the
    /// reset a handshake has to survive as a typed error.
    ResetHandshake,
    /// Complete the handshake and close without reading the client's request,
    /// which is a RST for the same reason.
    ResetRead,
    /// Never answer the ClientHello, and close the socket after a fixed hold
    /// rather than waiting for the test to say stop, so a client with no
    /// deadline still ends.
    NeverHandshakeThenClose,
}

/// A one-connection server for the failure cases `CannedTlsServer`'s
/// request/response loop cannot express: a peer that never speaks, a peer that
/// speaks and then truncates, and a peer that speaks and then goes quiet.
pub(crate) struct RudeTlsServer {
    pub(crate) port: u16,
    pub(crate) accepted: Arc<AtomicUsize>,
    pub(crate) stop: Arc<AtomicBool>,
    pub(crate) handle: Option<thread::JoinHandle<()>>,
    pub(crate) cert: TestCert,
}

impl RudeTlsServer {
    pub(crate) fn start(behavior: TlsBehavior) -> RudeTlsServer {
        let cert = generate_test_cert();
        // The mid-stream garbage is written as a raw record, which only a TLS 1.2
        // connection still reads without an AEAD record layer sealing it; the
        // point here is the client's failure to authenticate, not the version.
        let version = match behavior {
            TlsBehavior::CorruptRecord => Some(openssl::ssl::SslVersion::TLS1_2),
            _ => None,
        };
        let acceptor = tls_acceptor(&cert, version);
        let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind loopback");
        let port = listener.local_addr().expect("local address").port();
        let accepted = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let counter = accepted.clone();
        let stop_flag = stop.clone();
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
            counter.fetch_add(1, Ordering::SeqCst);
            match behavior {
                TlsBehavior::NeverHandshake => {
                    while !stop_flag.load(Ordering::SeqCst) && Instant::now() < deadline {
                        thread::sleep(Duration::from_millis(5));
                    }
                }
                TlsBehavior::Truncate => {
                    if let Ok(tls) = acceptor.accept(stream) {
                        // Dropping the SslStream frees the SSL object but runs
                        // no SSL_shutdown, so the TcpStream closes with no
                        // close_notify: the truncation a client must detect.
                        drop(tls);
                    }
                }
                TlsBehavior::Silent => {
                    if let Ok(tls) = acceptor.accept(stream) {
                        while !stop_flag.load(Ordering::SeqCst) && Instant::now() < deadline {
                            thread::sleep(Duration::from_millis(5));
                        }
                        drop(tls);
                    }
                }
                TlsBehavior::CorruptRecord => {
                    if let Ok(mut tls) = acceptor.accept(stream) {
                        let _ = tls.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\npartial");
                        // A record that is not a valid sealed record of this
                        // session: content type 21, a length of 2 and two
                        // bytes that are not a real alert. It goes in through
                        // the underlying socket because the acceptor's own
                        // `SslStream` has no public call for an arbitrary
                        // post-handshake record.
                        let _ = tls.get_mut().write_all(&[21, 3, 3, 0, 2, 2, 80]);
                        while !stop_flag.load(Ordering::SeqCst) && Instant::now() < deadline {
                            thread::sleep(Duration::from_millis(5));
                        }
                        drop(tls);
                    }
                }
                TlsBehavior::ResetHandshake => {
                    // The ClientHello is on its way; give it time to land in the
                    // receive queue unread, then let `stream` drop at the end of
                    // this function, which is the RST.
                    thread::sleep(Duration::from_millis(150));
                }
                TlsBehavior::ResetRead => {
                    if let Ok(tls) = acceptor.accept(stream) {
                        // The request the client sends right after the handshake
                        // lands unread, so this close is a RST.
                        thread::sleep(Duration::from_millis(250));
                        drop(tls);
                    }
                }
                TlsBehavior::NeverHandshakeThenClose => {
                    thread::sleep(Duration::from_millis(1200));
                }
            }
        });
        RudeTlsServer { port, accepted, stop, handle: Some(handle), cert }
    }

    pub(crate) fn accepted(&self) -> usize {
        self.accepted.load(Ordering::SeqCst)
    }

    pub(crate) fn finish(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Builds `tests/fixtures/<name>.kp` and runs it with `args`, `SSL_CERT_FILE`
/// and `SSL_CERT_DIR` removed unless `env` sets them. The removal is what makes
/// the untrusted fixtures deterministic: a developer's own `SSL_CERT_FILE` must
/// not stand in for the test CA.
pub(crate) fn run_tls_fixture(name: &str, args: &[String], env: &[(&str, &str)]) -> std::process::Output {
    let bin = build_fixture_bin(name);
    run_tls_binary(&bin, args, env)
}

/// Builds `tests/fixtures/<name>.kp` and returns the binary, so a test that has to
/// time the *run* rather than the build can do the two separately.
pub(crate) fn build_fixture_bin(name: &str) -> PathBuf {
    let bin = out_path(name);
    let build = kelpc()
        .arg("build")
        .arg(fixture(&format!("tests/fixtures/{name}.kp")))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected '{name}' to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    bin
}

/// Runs an already-built fixture binary with `args`, `SSL_CERT_FILE` and
/// `SSL_CERT_DIR` removed unless `env` sets them.
pub(crate) fn run_tls_binary(bin: &Path, args: &[String], env: &[(&str, &str)]) -> std::process::Output {
    let mut cmd = Command::new(bin);
    for a in args {
        cmd.arg(a);
    }
    cmd.env_remove("SSL_CERT_FILE");
    cmd.env_remove("SSL_CERT_DIR");
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().expect("failed to run the compiled binary")
}

/// `run_tls_fixture` for a fixture that must exit zero, returning its stdout.
pub(crate) fn tls_fixture_stdout(name: &str, args: &[String], env: &[(&str, &str)]) -> String {
    let out = run_tls_fixture(name, args, env);
    assert!(
        out.status.success(),
        "expected '{name}' to exit successfully, got {:?}:\nstdout: {}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

#[test]
pub(crate) fn a_tls_handle_cannot_be_read_as_an_integer() {
    // Release goal 1: `TlsStream.handle` is the opaque `tls.TlsHandle`, so the
    // `usize` the module used to carry is gone and any integer use is a type
    // error. `std/tls` is compiler-shipped, so the fixture names the handle
    // with no `[extern.*]` section of its own.
    assert_rejected_with(
        "tests/fixtures/tls_handle_not_an_integer.kp",
        "cannot assign value of type 'std.tls.TlsHandle' to 'n' declared as 'usize'",
    );
}

#[test]
pub(crate) fn a_tls_handle_cannot_be_forged_from_an_integer() {
    // The other direction: no conversion from an integer to a handle exists,
    // so "no TLS" cannot be spelled `0` at a handle's type.
    assert_rejected_with(
        "tests/fixtures/tls_handle_not_forged.kp",
        "declared to return 'std.tls.TlsHandle' but body evaluates to 'i64'",
    );
}

#[test]
pub(crate) fn every_try_tls_operation_succeeds_against_a_local_server() {
    // The Ok path of `try_handshake`, `try_write`, `try_write_all`,
    // `try_read`, `try_read_into` and `try_shutdown`. `SSL_CERT_FILE` is the
    // server's own certificate, which is exactly the default store `std/tls`
    // reads on NixOS and every other machine.
    let mut server = CannedTlsServer::start(one_connection(&[BODY_200_HELLO, BODY_200_HELLO]));
    let ssl = server.cert.cert_path.to_str().expect("the cert path is utf-8").to_string();
    let stdout = tls_fixture_stdout(
        "tls_try_ok",
        &[server.port.to_string()],
        &[("SSL_CERT_FILE", ssl.as_str())],
    );
    server.finish();
    assert_eq!(stdout, "true\ntrue\ntrue\ntrue\ntrue\n");
    assert_eq!(server.accepted(), 1, "the client opened more than one connection");
}

#[test]
pub(crate) fn a_certificate_outside_the_store_is_untrusted() {
    // No `SSL_CERT_FILE`: the minted certificate is the only chain, and the
    // machine's store does not vouch for it.
    let mut server = CannedTlsServer::start(one_connection(&[BODY_200_HELLO]));
    let stdout = tls_fixture_stdout(
        "tls_error",
        &[server.port.to_string(), "127.0.0.1".to_string()],
        &[],
    );
    server.finish();
    assert_eq!(stdout, "untrusted\n");
}

#[test]
pub(crate) fn an_expired_certificate_is_reported_as_expired() {
    let mut server = CannedTlsServer::start_with_cert(
        generate_expired_test_cert(),
        one_connection(&[BODY_200_HELLO]),
    );
    let ssl = server.cert.cert_path.to_str().expect("the cert path is utf-8").to_string();
    let stdout = tls_fixture_stdout(
        "tls_error",
        &[server.port.to_string(), "127.0.0.1".to_string()],
        &[("SSL_CERT_FILE", ssl.as_str())],
    );
    server.finish();
    assert_eq!(stdout, "expired\n");
}

#[test]
pub(crate) fn a_certificate_that_names_another_host_is_refused() {
    // The certificate's only SAN is `localhost`; the client dials and verifies
    // against `127.0.0.1`, so the name check refuses it even though the store
    // trusts the certificate.
    let mut server = CannedTlsServer::start_with_cert(
        generate_mismatched_test_cert(),
        one_connection(&[BODY_200_HELLO]),
    );
    let ssl = server.cert.cert_path.to_str().expect("the cert path is utf-8").to_string();
    let stdout = tls_fixture_stdout(
        "tls_error",
        &[server.port.to_string(), "127.0.0.1".to_string()],
        &[("SSL_CERT_FILE", ssl.as_str())],
    );
    server.finish();
    assert_eq!(stdout, "host_mismatch\n");
}

#[test]
pub(crate) fn a_tls_1_1_only_server_is_reported_as_protocol_too_old() {
    use openssl::ssl::SslVersion;

    let mut server = CannedTlsServer::start_with(
        generate_test_cert(),
        one_connection(&[BODY_200_HELLO]),
        Some(SslVersion::TLS1_1),
    );
    let ssl = server.cert.cert_path.to_str().expect("the cert path is utf-8").to_string();
    let stdout = tls_fixture_stdout(
        "tls_error",
        &[server.port.to_string(), "127.0.0.1".to_string()],
        &[("SSL_CERT_FILE", ssl.as_str())],
    );
    server.finish();
    assert_eq!(stdout, "protocol_too_old\n");
}

#[test]
pub(crate) fn a_stream_that_ends_without_a_close_notify_is_a_truncation() {
    let mut server = RudeTlsServer::start(TlsBehavior::Truncate);
    let ssl = server.cert.cert_path.to_str().expect("the cert path is utf-8").to_string();
    let stdout = tls_fixture_stdout(
        "tls_truncated",
        &[server.port.to_string()],
        &[("SSL_CERT_FILE", ssl.as_str())],
    );
    server.finish();
    assert_eq!(stdout, "peer_closed_uncleanly\n");
    assert_eq!(server.accepted(), 1, "the client opened more than one connection");
}

#[test]
pub(crate) fn a_silent_peer_times_out_the_handshake() {
    let mut server = RudeTlsServer::start(TlsBehavior::NeverHandshake);
    let stdout = tls_fixture_stdout("tls_timeout_handshake", &[server.port.to_string()], &[]);
    server.finish();
    assert_eq!(stdout, "timeout\n");
}

#[test]
pub(crate) fn a_silent_peer_times_out_a_read() {
    // The handshake completes, and then the server says nothing: the read
    // deadline `set_timeout` applies is what turns that into `Timeout`.
    let mut server = RudeTlsServer::start(TlsBehavior::Silent);
    let ssl = server.cert.cert_path.to_str().expect("the cert path is utf-8").to_string();
    let stdout = tls_fixture_stdout(
        "tls_timeout_read",
        &[server.port.to_string()],
        &[("SSL_CERT_FILE", ssl.as_str())],
    );
    server.finish();
    assert_eq!(stdout, "timeout\n");
}

#[test]
pub(crate) fn a_private_ca_file_is_added_to_the_roots() {
    // No `SSL_CERT_FILE`: the certificate is trusted only because
    // `handshake_with`'s `TlsOptions.ca_file` names it.
    let mut server = CannedTlsServer::start(one_connection(&[BODY_200_HELLO]));
    let ca = server.cert.cert_path.to_str().expect("the cert path is utf-8").to_string();
    let stdout = tls_fixture_stdout("tls_ca_file", &[server.port.to_string(), ca], &[]);
    server.finish();
    assert_eq!(stdout, "ok\n");
    assert_eq!(server.accepted(), 1, "the client opened more than one connection");
}

#[test]
pub(crate) fn a_private_ca_directory_adds_its_certificates_and_ignores_other_files() {
    // Release goal 1, trust by directory: `tlsfile.ca_dir_pem` reads every `.pem`
    // and `.crt` file in the directory through the `fs` gate and hands the joined
    // PEM text to the handshake. A file that is not a certificate is never read.
    let mut server = CannedTlsServer::start(one_connection(&[BODY_200_HELLO]));
    let dir = tempfile_dir::TempDir::new("ca_dir");
    std::fs::copy(&server.cert.cert_path, dir.path().join("ca.pem"))
        .expect("failed to copy the test certificate into the CA directory");
    std::fs::write(dir.path().join("README.txt"), "not a certificate\n")
        .expect("failed to write the bystander file");
    let shown = dir.path().display().to_string();
    let stdout = tls_fixture_stdout("tls_ca_dir", &[server.port.to_string(), shown], &[]);
    server.finish();
    assert_eq!(stdout, "ok\n");
    assert_eq!(server.accepted(), 1, "the client opened more than one connection");
}

#[test]
pub(crate) fn an_empty_private_ca_directory_is_a_typed_error_not_the_system_store() {
    // A directory that holds no CA certificate is a configuration mistake. It is
    // refused by name rather than read as "no extra roots", so a typo cannot
    // quietly fall back to the system store.
    let mut server = RudeTlsServer::start(TlsBehavior::NeverHandshakeThenClose);
    let dir = tempfile_dir::TempDir::new("ca_dir_empty");
    let shown = dir.path().display().to_string();
    let stdout = tls_fixture_stdout("tls_ca_dir", &[server.port.to_string(), shown.clone()], &[]);
    server.finish();
    assert_eq!(
        stdout,
        format!("io tls: the directory {shown} holds no .pem or .crt CA certificate\n")
    );
}

#[test]
pub(crate) fn a_peer_that_stops_reading_times_out_a_write() {
    // The handshake completes and the server then never reads, so the client's
    // `try_write_all` of 64 MiB stalls once the socket buffers are full. The write
    // deadline `set_timeout` puts on the socket turns that stall into `Timeout`.
    let mut server = RudeTlsServer::start(TlsBehavior::Silent);
    let ssl = server.cert.cert_path.to_str().expect("the cert path is utf-8").to_string();
    let stdout = tls_fixture_stdout(
        "tls_timeout_write",
        &[server.port.to_string()],
        &[("SSL_CERT_FILE", ssl.as_str())],
    );
    server.finish();
    assert_eq!(stdout, "timeout\n");
}

#[test]
pub(crate) fn a_certificate_from_an_unknown_ca_is_untrusted() {
    // The leaf is signed by a CA that no store holds and that nothing names, so
    // it is untrusted even though its name matches and it is in date.
    let mut server = CannedTlsServer::start_with_cert(
        generate_ca_signed_test_cert(),
        one_connection(&[BODY_200_HELLO]),
    );
    let stdout = tls_fixture_stdout(
        "tls_error",
        &[server.port.to_string(), "127.0.0.1".to_string()],
        &[],
    );
    server.finish();
    assert_eq!(stdout, "untrusted\n");
}

#[test]
pub(crate) fn a_private_ca_that_signed_the_server_certificate_is_trusted() {
    // The same leaf, with the CA's PEM added through `tls_ca_file`: the one
    // thing that changes the outcome is naming the CA, never a weaker check.
    let cert = generate_ca_signed_test_cert();
    let ca = cert._dir.path().join("ca.pem").to_str().expect("the CA path is utf-8").to_string();
    let mut server = CannedTlsServer::start_with_cert(cert, one_connection(&[BODY_200_HELLO]));
    let stdout = tls_fixture_stdout("tls_ca_file", &[server.port.to_string(), ca], &[]);
    server.finish();
    assert_eq!(stdout, "ok\n");
    assert_eq!(server.accepted(), 1, "the client opened more than one connection");
}

/// A certificate for `127.0.0.1` signed by a fresh CA, with the leaf's PEM and key
/// as the server's own files and the CA's PEM written beside them as `ca.pem`.
/// Neither the leaf nor the CA is in any store, so only a CA file that names the
/// CA makes a client trust the leaf.
pub(crate) fn generate_ca_signed_test_cert() -> TestCert {
    use openssl::asn1::Asn1Time;
    use openssl::bn::{BigNum, MsbOption};
    use openssl::hash::MessageDigest;
    use openssl::pkey::{PKey, Private};
    use openssl::rsa::Rsa;
    use openssl::x509::extension::{BasicConstraints, SubjectAlternativeName};
    use openssl::x509::{X509, X509Name, X509NameBuilder};

    fn new_key() -> PKey<Private> {
        let rsa = Rsa::generate(2048).expect("failed to generate an RSA key for the test CA");
        PKey::from_rsa(rsa).expect("failed to wrap the RSA key")
    }

    fn name(cn: &str) -> X509Name {
        let mut b = X509NameBuilder::new().expect("failed to start a name builder");
        b.append_entry_by_text("CN", cn).expect("failed to set CN");
        b.build()
    }

    fn serial() -> openssl::asn1::Asn1Integer {
        let mut s = BigNum::new().expect("failed to allocate a serial number");
        s.rand(64, MsbOption::MAYBE_ZERO, false)
            .expect("failed to randomize the serial number");
        s.to_asn1_integer().expect("serial as an ASN.1 integer")
    }

    let ca_key = new_key();
    let ca_name = name("kelpc test CA");
    let mut ca = X509::builder().expect("failed to start the CA builder");
    ca.set_version(2).expect("failed to set the CA version");
    ca.set_serial_number(&serial()).expect("failed to set the CA serial");
    ca.set_subject_name(&ca_name).expect("failed to set the CA subject");
    ca.set_issuer_name(&ca_name).expect("failed to set the CA issuer");
    ca.set_pubkey(&ca_key).expect("failed to set the CA public key");
    ca.set_not_before(&Asn1Time::days_from_now(0).expect("CA not-before"))
        .expect("failed to set the CA not-before");
    ca.set_not_after(&Asn1Time::days_from_now(1).expect("CA not-after"))
        .expect("failed to set the CA not-after");
    ca.append_extension(BasicConstraints::new().critical().ca().build().expect("CA constraints"))
        .expect("failed to append the CA constraints");
    ca.sign(&ca_key, MessageDigest::sha256()).expect("failed to sign the test CA");
    let ca_cert = ca.build();

    let leaf_key = new_key();
    let mut leaf = X509::builder().expect("failed to start the leaf builder");
    leaf.set_version(2).expect("failed to set the leaf version");
    leaf.set_serial_number(&serial()).expect("failed to set the leaf serial");
    leaf.set_subject_name(&name("127.0.0.1")).expect("failed to set the leaf subject");
    leaf.set_issuer_name(ca_cert.subject_name()).expect("failed to set the leaf issuer");
    leaf.set_pubkey(&leaf_key).expect("failed to set the leaf public key");
    leaf.set_not_before(&Asn1Time::days_from_now(0).expect("leaf not-before"))
        .expect("failed to set the leaf not-before");
    leaf.set_not_after(&Asn1Time::days_from_now(1).expect("leaf not-after"))
        .expect("failed to set the leaf not-after");
    leaf.append_extension(BasicConstraints::new().critical().build().expect("leaf constraints"))
        .expect("failed to append the leaf constraints");
    let mut san = SubjectAlternativeName::new();
    san.ip("127.0.0.1");
    let san = san
        .build(&leaf.x509v3_context(Some(&ca_cert), None))
        .expect("leaf subject alternative name");
    leaf.append_extension(san).expect("failed to append the subject alternative name");
    leaf.sign(&ca_key, MessageDigest::sha256()).expect("failed to sign the leaf");
    let leaf_cert = leaf.build();

    let dir = tempfile_dir::TempDir::new("tls_ca_signed");
    let cert_path = dir.path().join("cert.pem");
    let key_path = dir.path().join("key.pem");
    std::fs::write(&cert_path, leaf_cert.to_pem().expect("leaf as PEM"))
        .expect("failed to write the leaf certificate");
    std::fs::write(&key_path, leaf_key.private_key_to_pem_pkcs8().expect("leaf key as PEM"))
        .expect("failed to write the leaf key");
    std::fs::write(dir.path().join("ca.pem"), ca_cert.to_pem().expect("CA as PEM"))
        .expect("failed to write the CA certificate");
    TestCert { cert_path, key_path, _dir: Arc::new(dir) }
}
