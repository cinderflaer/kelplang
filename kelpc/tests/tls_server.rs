//! Server-side TLS, client certificates and ALPN in `std/tls` (task st3).
//!
//! `plan/tls-server.md` is the design note. Every case is one run of
//! `tests/hazards/tls_server_matrix.kp`: one program, a server on a second
//! thread and a client on the main one, over `127.0.0.1` on an ephemeral port.
//! The certificates are made here with the `openssl` CLI into this process's
//! own scratch directory (a tiny CA, a `localhost` server certificate, a client
//! certificate, a second CA with its own client certificate, an expired server
//! certificate); a machine without the CLI prints one line and passes.
//!
//! The backend follows the suite's switch: `KELPC_ORACLE=c` runs the C oracle,
//! unset runs the default LLVM backend. Both must print the same lines.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;

fn backend() -> &'static str {
    match std::env::var("KELPC_ORACLE").as_deref() {
        Ok("c") => "c",
        _ => "llvm",
    }
}

fn kelpc_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_kelpc"))
}

fn kelpc() -> Command {
    let mut cmd = Command::new(kelpc_bin());
    cmd.env_remove("KELPC_BACKEND");
    if backend() == "c" {
        cmd.env("KELPC_ORACLE", "c");
    } else {
        cmd.env_remove("KELPC_ORACLE");
    }
    cmd
}

fn scratch(name: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("kelpc_tls_server_{}_{}", std::process::id(), name));
    p
}

fn openssl<S: AsRef<str>>(dir: &Path, args: &[S]) -> bool {
    Command::new("openssl")
        .args(args.iter().map(|a| a.as_ref()))
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn write(dir: &Path, name: &str, text: &str) {
    std::fs::write(dir.join(name), text).expect("write a scratch file");
}

/// Makes the certificates once for the whole process. `None` (and a printed
/// line) when the `openssl` CLI is missing or cannot make them.
fn certs() -> Option<&'static PathBuf> {
    static CERTS: OnceLock<Option<PathBuf>> = OnceLock::new();
    CERTS
        .get_or_init(|| {
            if !openssl(Path::new("."), &["version"]) {
                println!("skipped: the openssl command line tool is missing");
                return None;
            }
            let dir = scratch("certs");
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("a scratch directory for the certificates");
            match make_certs(&dir) {
                Ok(()) => Some(dir),
                Err(what) => {
                    println!("skipped: openssl could not make the test certificates ({what})");
                    None
                }
            }
        })
        .as_ref()
}

fn make_certs(dir: &Path) -> Result<(), String> {
    let ec = ["-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:prime256v1", "-nodes"];
    let step = |ok: bool, what: &str| if ok { Ok(()) } else { Err(what.to_string()) };
    write(dir, "server.ext", "subjectAltName=DNS:localhost\nbasicConstraints=CA:FALSE\n");
    write(dir, "client.ext", "extendedKeyUsage=clientAuth\nbasicConstraints=CA:FALSE\n");
    write(dir, "junk.pem", "this is not a PEM file\n");
    for (stem, cn) in [("ca", "Kelp Test CA"), ("other_ca", "Other Test CA")] {
        let mut a: Vec<String> = vec!["req".into(), "-x509".into()];
        a.extend(ec.iter().map(|s| s.to_string()));
        for x in [
            "-keyout",
            &format!("{stem}.key"),
            "-out",
            &format!("{stem}.pem"),
            "-subj",
            &format!("/CN={cn}"),
            "-days",
            "2",
            "-addext",
            "basicConstraints=critical,CA:TRUE",
            "-addext",
            "keyUsage=critical,keyCertSign,cRLSign",
        ] {
            a.push(x.to_string());
        }
        step(openssl(dir, &a), "a CA")?;
    }
    // (key stem, CN, signing CA, extension file, output stem)
    let leaves = [
        ("server", "localhost", "ca", "server.ext", "server"),
        ("other_server", "localhost", "ca", "server.ext", "other_server"),
        ("client", "kelp-client", "ca", "client.ext", "client"),
        ("other_client", "kelp-intruder", "other_ca", "client.ext", "other_client"),
    ];
    for (key, cn, ca, ext, out) in leaves {
        let mut a: Vec<String> = vec!["req".into(), "-new".into()];
        a.extend(ec.iter().map(|s| s.to_string()));
        for x in ["-keyout", &format!("{key}.key"), "-out", &format!("{key}.csr"), "-subj", &format!("/CN={cn}")] {
            a.push(x.to_string());
        }
        step(openssl(dir, &a), "a key and request")?;
        let sign: Vec<String> = [
            "x509",
            "-req",
            "-in",
            &format!("{key}.csr"),
            "-CA",
            &format!("{ca}.pem"),
            "-CAkey",
            &format!("{ca}.key"),
            "-CAcreateserial",
            "-out",
            &format!("{out}.pem"),
            "-days",
            "2",
            "-extfile",
            ext,
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        step(openssl(dir, &sign), "a certificate")?;
    }
    // The expired server certificate, for the key of `server`: OpenSSL 3.4's
    // `x509 -not_before/-not_after`, or `openssl ca -startdate/-enddate` on an
    // older one.
    let direct = openssl(
        dir,
        &[
            "x509",
            "-req",
            "-in",
            "server.csr",
            "-CA",
            "ca.pem",
            "-CAkey",
            "ca.key",
            "-CAcreateserial",
            "-out",
            "expired.pem",
            "-not_before",
            "20200101000000Z",
            "-not_after",
            "20200102000000Z",
            "-extfile",
            "server.ext",
        ],
    );
    if !direct {
        write(
            dir,
            "ca.cnf",
            "[ca]\ndefault_ca = d\n[d]\ndatabase = index.txt\nnew_certs_dir = .\nserial = serial.txt\n\
             default_md = sha256\npolicy = p\nunique_subject = no\n[p]\ncommonName = supplied\n",
        );
        write(dir, "index.txt", "");
        write(dir, "serial.txt", "01\n");
        step(
            openssl(
                dir,
                &[
                    "ca",
                    "-batch",
                    "-config",
                    "ca.cnf",
                    "-cert",
                    "ca.pem",
                    "-keyfile",
                    "ca.key",
                    "-in",
                    "server.csr",
                    "-out",
                    "expired.pem",
                    "-startdate",
                    "20200101000000Z",
                    "-enddate",
                    "20200102000000Z",
                    "-extfile",
                    "server.ext",
                    "-notext",
                ],
            ),
            "an expired certificate",
        )?;
    }
    // The key variants `tests/hazards/tls_reload_matrix.kp` reads: an
    // encrypted copy (pass phrase "swordfish"), a world-readable copy and the
    // plain key pinned to 0600.
    step(
        openssl(
            dir,
            &["pkey", "-in", "server.key", "-aes256", "-passout", "pass:swordfish", "-out", "server_enc.key"],
        ),
        "an encrypted key",
    )?;
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::copy(dir.join("server.key"), dir.join("server_open.key")).map_err(|e| e.to_string())?;
        let mode = |name: &str, bits: u32| {
            std::fs::set_permissions(dir.join(name), std::fs::Permissions::from_mode(bits)).map_err(|e| e.to_string())
        };
        mode("server.key", 0o600)?;
        mode("server_enc.key", 0o600)?;
        mode("server_open.key", 0o644)?;
    }
    Ok(())
}

/// The reload program, built once per process for the backend under test.
fn reload_bin() -> &'static PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let bin = scratch(&format!("reload_{}", backend()));
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/hazards/tls_reload_matrix.kp");
        let out = kelpc().arg("build").arg(&src).arg("-o").arg(&bin).output().expect("failed to spawn kelpc");
        assert!(
            out.status.success(),
            "the TLS reload program did not compile:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        bin
    })
}

fn run_reload(mode: &str) -> Option<Vec<String>> {
    let dir = certs()?;
    let out: Output = Command::new("sh")
        .arg("-c")
        .arg("ulimit -v 1048576; ulimit -t 30; exec timeout 60 \"$0\" \"$@\"")
        .arg(reload_bin())
        .arg(mode)
        .arg(dir)
        .env_remove("SSL_CERT_FILE")
        .env_remove("SSL_CERT_DIR")
        .stdin(Stdio::null())
        .output()
        .expect("failed to run the TLS reload program");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        out.status.success(),
        "scenario '{mode}' exited with {:?}\nstdout: {stdout}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    Some(stdout.lines().map(|l| l.to_string()).collect())
}

fn expect_reload(mode: &str, want: &[&str]) {
    if let Some(got) = run_reload(mode) {
        assert_eq!(got, want, "scenario '{mode}' on the {} backend", backend());
    }
}

#[test]
fn an_encrypted_private_key_needs_its_pass_phrase() {
    // MISSING-Uc6Kj: right phrase builds; none or a wrong one is a typed error
    // that quotes neither the phrase nor the key.
    expect_reload(
        "encrypted_keys",
        &[
            "encrypted with the phrase ok",
            "encrypted without a phrase handshake_failed",
            "encrypted with a wrong phrase handshake_failed",
            "plain with a phrase ok",
        ],
    );
}

#[test]
fn read_key_refuses_a_key_file_others_can_read() {
    expect_reload("key_perms", &["key good ok", "key open io mode=0644", "key gone io"]);
}

#[test]
fn a_reload_replaces_the_certificate_for_new_handshakes_and_a_bad_one_changes_nothing() {
    expect_reload(
        "reload",
        &[
            "round 1 echo=hello",
            "reload expired none",
            "round 2 expired",
            "reload mismatched handshake_failed",
            "round 3 expired",
            "reload valid none",
            "round 4 echo=hello",
            "server accepted 4 completed 2",
        ],
    );
}

#[test]
fn listen_on_binds_the_named_host() {
    expect_reload("listen_on", &["listen_on loopback accepted"]);
}

/// The matrix program, built once per process for the backend under test.
fn matrix_bin() -> &'static PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let bin = scratch(&format!("matrix_{}", backend()));
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/hazards/tls_server_matrix.kp");
        let out = kelpc().arg("build").arg(&src).arg("-o").arg(&bin).output().expect("failed to spawn kelpc");
        assert!(
            out.status.success(),
            "the TLS server program did not compile:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        bin
    })
}

/// One scenario: the program's stdout lines. Bounded in memory, CPU and wall
/// time; the scenario ends by itself (every wait inside has a deadline).
fn run(mode: &str) -> Option<Vec<String>> {
    let dir = certs()?;
    let bin = matrix_bin();
    let out: Output = Command::new("sh")
        .arg("-c")
        .arg("ulimit -v 1048576; ulimit -t 30; exec timeout 60 \"$0\" \"$@\"")
        .arg(bin)
        .arg(mode)
        .arg(dir)
        .env_remove("SSL_CERT_FILE")
        .env_remove("SSL_CERT_DIR")
        .stdin(Stdio::null())
        .output()
        .expect("failed to run the TLS server program");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        out.status.success(),
        "scenario '{mode}' exited with {:?}\nstdout: {stdout}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    Some(stdout.lines().map(|l| l.to_string()).collect())
}

fn expect(mode: &str, want: &[&str]) {
    if let Some(got) = run(mode) {
        assert_eq!(got, want, "scenario '{mode}' on the {} backend", backend());
    }
}

#[test]
fn a_server_and_a_client_handshake_and_echo() {
    expect("echo", &["client: ok alpn=- echo=hello", "server: ok alpn=- peer=- echoed=5"]);
}

#[test]
fn a_tls_13_floor_is_accepted_by_a_current_client() {
    expect("min13", &["client: ok alpn=- echo=hello", "server: ok alpn=- peer=- echoed=5"]);
}

#[test]
fn alpn_picks_the_servers_first_choice_the_client_offers() {
    // The client lists http/1.1 first and h2 second; the server prefers h2.
    expect("alpn_h2", &["client: ok alpn=h2 echo=hello", "server: ok alpn=h2 peer=- echoed=5"]);
}

#[test]
fn alpn_falls_back_to_the_one_protocol_both_speak() {
    expect(
        "alpn_h1",
        &["client: ok alpn=http/1.1 echo=hello", "server: ok alpn=http/1.1 peer=- echoed=5"],
    );
}

#[test]
fn alpn_with_no_protocol_in_common_is_a_typed_error_on_both_sides() {
    expect("alpn_none", &["client: handshake handshake_failed", "server: accept handshake_failed"]);
}

#[test]
fn a_server_without_alpn_negotiates_nothing_even_when_the_client_offers() {
    // The server configured none, so it makes no choice; the client's offer is
    // simply unanswered and `alpn()` is `None` on both streams.
    expect("alpn_client_only", &["client: ok alpn=- echo=hello", "server: ok alpn=- peer=- echoed=5"]);
}

#[test]
fn mtls_accepts_a_client_certificate_from_the_configured_ca() {
    expect(
        "mtls_ok",
        &["client: ok alpn=- echo=hello", "server: ok alpn=- peer=CN=kelp-client echoed=5"],
    );
}

#[test]
fn mtls_refuses_a_client_with_no_certificate() {
    if let Some(got) = run("mtls_none") {
        // TLS 1.3 lets the client finish its side first, so the refusal shows
        // as its first read; either way it is an error value, never an echo.
        assert_eq!(got.len(), 2, "{got:?}");
        assert!(got[0].starts_with("client: ") && !got[0].contains(" ok "), "{got:?}");
        assert_eq!(got[1], "server: accept handshake_failed");
    }
}

#[test]
fn mtls_refuses_a_client_certificate_from_another_ca() {
    if let Some(got) = run("mtls_otherca") {
        assert_eq!(got.len(), 2, "{got:?}");
        assert!(got[0].starts_with("client: ") && !got[0].contains(" ok "), "{got:?}");
        assert_eq!(got[1], "server: accept untrusted");
    }
}

#[test]
fn a_certificate_for_another_host_name_is_refused() {
    expect("wronghost", &["client: handshake host_mismatch", "server: accept handshake_failed"]);
}

#[test]
fn an_expired_server_certificate_is_refused() {
    expect("expired", &["client: handshake expired", "server: accept handshake_failed"]);
}

#[test]
fn a_peer_that_ends_without_close_notify_is_a_truncation() {
    // The data the server sent arrives; the end that follows is not a clean
    // end of stream.
    expect("unclean", &["client: second read peer_closed_uncleanly", "server: unclean sent alpn=-"]);
}

#[test]
fn a_bad_server_configuration_is_a_typed_error_never_a_panic() {
    expect(
        "config_errors",
        &[
            "config ok",
            "config: mismatch handshake_failed",
            "config: missing io",
            "config: junk cert handshake_failed",
            "config junk key handshake_failed",
            "config: bad alpn handshake_failed",
            "config: junk ca handshake_failed",
        ],
    );
}

#[test]
fn an_https_get_reaches_an_http_serve_tls_handler_with_a_private_ca() {
    // The first GET has no private CA, so the default store refuses the
    // certificate and the server's loop goes on; the second names the CA and
    // is answered by the handler.
    expect("https", &["untrusted get refused", "https status 200", "hello over tls", "served 1"]);
}

#[test]
fn an_https_server_can_require_a_client_certificate_and_the_client_can_present_one() {
    expect(
        "https_mtls",
        &["https mtls without a certificate refused", "https mtls status 200", "served 1"],
    );
}
