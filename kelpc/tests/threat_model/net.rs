use super::*;

pub(crate) const NET: &[&str] = &["std/net"];

pub(crate) fn connect_prog(host: &str, port: u16) -> String {
    format!("    val s = result.expect(net.connect(\"{host}\", {port}), \"net.connect: the call failed\")\n    print(\"connected\")\n    net.close(s)")
}

/// N1: a host the grant does not name is denied before any dial: another
/// loopback address, a name that is the same machine, a numeric spelling of the
/// same address, a suffix trick, and an injected `:port` in the host text.
#[test]
pub(crate) fn n01_a_host_outside_the_grant_is_denied_before_the_dial() {
    for host in [
        "127.0.0.2",
        "localhost",
        "127.1",
        "0x7f.0.0.1",
        "2130706433",
        "127.0.0.1.invalid",
        "127.0.0.1:9",
        " 127.0.0.1",
        "127.0.0.1 ",
        "LOCALHOST",
    ] {
        let lab = Lab::new("n01");
        lab.simple("net = [\"127.0.0.1:9\"]", NET, &connect_prog(host, 9));
        lab.denied("is outside the net capability");
    }
}

/// N2: a grant on a *name* does not cover that name's address: matching is on
/// the string the code wrote, so `localhost:*` does not admit `127.0.0.1`.
#[test]
pub(crate) fn n02_a_name_grant_does_not_cover_the_address() {
    let lab = Lab::new("n02");
    lab.simple("net = [\"localhost:9\"]", NET, &connect_prog("127.0.0.1", 9));
    lab.denied("is outside the net capability");
}

/// N3: the port is part of the grant: another port on the granted host is
/// outside it, and the port cannot be smuggled through the host text.
#[test]
pub(crate) fn n03_a_port_outside_the_grant_is_denied() {
    for (host, port) in [("127.0.0.1", 10u16), ("127.0.0.1:9", 9u16), ("127.0.0.1:9:", 9u16)] {
        let lab = Lab::new("n03");
        lab.simple("net = [\"127.0.0.1:9\"]", NET, &connect_prog(host, port));
        lab.denied("is outside the net capability");
    }
}

/// N4: a numeric-looking host pattern is a statement about addresses, and a
/// host *name* that fits it is not one: `127.0.0.*` must not admit
/// `127.0.0.zzz`, which is a DNS name its owner can point anywhere. The
/// matcher's verdict (`host_match`) is `true` for it, so the refusal is the
/// capability check's own rule that a numeric pattern admits only an IPv4
/// literal. Run through `net.connect` (denied before any lookup).
#[test]
pub(crate) fn n04_a_numeric_pattern_admits_only_ip_literals() {
    for host in ["127.0.0.zzz", "127.0.0.1x", "127.0.0.example"] {
        let lab = Lab::new("n04");
        lab.simple("net = [\"127.0.0.*:9\"]", NET, &connect_prog(host, 9));
        lab.denied("is outside the net capability");
    }
}

/// N4b: `net.resolve` is held to the same rule, since resolving a name the
/// numeric pattern should not admit is already a lookup of an attacker-chosen
/// name.
#[test]
pub(crate) fn n04b_resolve_applies_the_same_numeric_rule() {
    let lab = Lab::new("n04b");
    lab.simple(
        "net = [\"127.0.0.*\"]",
        NET,
        "    print(net.resolve(\"127.0.0.zzz\").len())",
    );
    lab.denied("is outside the net capability");
}

/// The text of `/proc/net/tcp` lines for `port`, as `local_address st` pairs.
pub(crate) fn listener_line(stdout: &str, port: u16) -> Option<String> {
    let want = format!(":{port:04X}");
    stdout.lines().find_map(|l| {
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() > 3 && f[1].ends_with(&want) && f[3] == "0A" {
            Some(f[1].to_string())
        } else {
            None
        }
    })
}

/// N5: a loopback-only grant gives a loopback-only listener. `net.listen`
/// has no host argument and used to bind `0.0.0.0` whatever the grant said, so
/// a module granted `127.0.0.1:*` could serve the whole network. The program
/// reads its own `/proc/net/tcp` to see the address it is really bound to.
#[test]
pub(crate) fn n05_a_loopback_grant_binds_loopback_only() {
    for (grant, want) in [
        ("127.0.0.1:*", "0100007F"),
        ("localhost:*", "0100007F"),
        ("127.0.0.1:0", "0100007F"),
        ("*", "00000000"),
    ] {
        let lab = Lab::new("n05");
        lab.simple(
            &format!("net = [\"{grant}\"], fs = [\"/proc/*/net/tcp\"]"),
            &["std/net", "std/fs", "std/io"],
            "    val l = net.listen(0)\n    print(l.port())\n    val f = result.expect(fs.open(\"/proc/net/tcp\", \"r\"), \"fs.open: the call failed\")\n    print(String.from_bytes(f.read_all()))\n    l.close()",
        );
        for out in lab.runs(&[]) {
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
            let port: u16 = stdout.lines().next().unwrap().trim().parse().unwrap();
            let line = listener_line(&stdout, port)
                .unwrap_or_else(|| panic!("no LISTEN line for port {port}:\n{stdout}"));
            assert_eq!(
                line.split(':').next().unwrap(),
                want,
                "grant '{grant}' bound {line}"
            );
        }
    }
}

/// N5b: the HTTP server goes through the same `net.listen`, so the same
/// loopback grant gives a loopback server.
#[test]
pub(crate) fn n05b_the_http_server_binds_loopback_under_a_loopback_grant() {
    let lab = Lab::new("n05b");
    lab.src(
        "main.kp",
        "impl \"std/http\" caps(net = [\"127.0.0.1:*\"])\nimpl \"probe\" caps(fs = [\"/proc/*/net/tcp\"])\n\nfun main() {\n    val s = http.listen(0)\n    print(s.port())\n    print(probe.table())\n    s.close()\n}\n",
    );
    lab.src(
        "probe.kp",
        "impl \"std/fs\"\nimpl \"std/io\"\n\npub fun table() => String {\n    val f = result.expect(fs.open(\"/proc/net/tcp\", \"r\"), \"fs.open: the call failed\")\n    return String.from_bytes(f.read_all())\n}\n",
    );
    for out in lab.runs(&[]) {
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let port: u16 = stdout.lines().next().unwrap().trim().parse().unwrap();
        let line = listener_line(&stdout, port).expect("a LISTEN line");
        assert_eq!(line.split(':').next().unwrap(), "0100007F", "{line}");
    }
}

/// N6: the listen port is checked against the grant's port: another port, and
/// the "any port" request `0` under a one-port grant, are denied.
#[test]
pub(crate) fn n06_a_listen_port_outside_the_grant_is_denied() {
    for port in [0u16, 8081, 80] {
        let lab = Lab::new("n06");
        lab.simple(
            "net = [\"127.0.0.1:8080\"]",
            NET,
            &format!("    val l = net.listen({port})\n    print(\"bound\")\n    l.close()"),
        );
        lab.denied("is outside the net capability");
    }
}

/// N6b (open item 55): `net.listen_on(host, port)` checks the host as well as
/// the port, so a grant for `127.0.0.1:8080` refuses the wildcard address and
/// every other host and port, where `net.listen(8080)` only compared the port.
#[test]
pub(crate) fn n06b_listen_on_checks_the_host_against_the_grant() {
    for (host, port) in [("0.0.0.0", 8080u16), ("*", 8080), ("127.0.0.2", 8080), ("127.0.0.1", 8081)] {
        let lab = Lab::new("n06b");
        lab.simple(
            "net = [\"127.0.0.1:8080\"]",
            NET,
            &format!("    val l = net.listen_on(\"{host}\", {port})\n    print(\"bound\")\n    l.close()"),
        );
        lab.denied("is outside the net capability");
    }
}

/// A one-shot loopback HTTP server: answers the first request with a 302 to
/// `location` (`PORT` is replaced by the server's own port), and reports how
/// many connections it took in the following second.
pub(crate) fn redirect_server(location: &str) -> (u16, std::thread::JoinHandle<usize>) {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().unwrap().port();
    let response = format!(
        "HTTP/1.1 302 Found\r\nLocation: {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        location.replace("PORT", &port.to_string())
    );
    let handle = std::thread::spawn(move || {
        let mut taken = 0;
        let (mut c, _) = listener.accept().expect("accept");
        taken += 1;
        let mut buf = [0u8; 2048];
        let mut seen = Vec::new();
        while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = c.read(&mut buf).unwrap_or(0);
            if n == 0 {
                break;
            }
            seen.extend_from_slice(&buf[..n]);
        }
        let _ = c.write_all(response.as_bytes());
        drop(c);
        listener.set_nonblocking(true).unwrap();
        for _ in 0..10 {
            if listener.accept().is_ok() {
                taken += 1;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        taken
    });
    (port, handle)
}

/// N7: a redirect cannot lead an `http` client to a host its grant does not
/// name: the follow-up request is a fresh `net.connect` under the same grant.
/// Another loopback address and a name for this machine are both refused, and
/// the server saw exactly the one connection.
#[test]
pub(crate) fn n07_a_redirect_to_an_unlisted_host_is_refused() {
    for location in ["http://127.0.0.2:PORT/loot", "http://localhost:PORT/loot"] {
        let lab = Lab::new("n07");
        lab.src(
            "main.kp",
            "impl \"std/http\" caps(net = [\"127.0.0.1:*\"])\nimpl \"std/process\"\n\nfun main() {\n    val args = process.args()\n    val r = result.expect(http.get(args.get(1).as_str()), \"http.get: the call failed\")\n    print(r.status)\n}\n",
        );
        let (port, server) = redirect_server(location);
        let url = format!("http://127.0.0.1:{port}/start");
        let b = lab.build();
        assert!(b.status.success(), "{}", String::from_utf8_lossy(&b.stderr));
        let out = lab.run(&[&url]);
        let taken = server.join().unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            !out.status.success() && stderr.contains("is outside the net capability"),
            "redirect to {location} was not refused: {stderr}"
        );
        assert_eq!(taken, 1, "a second connection reached the server");
    }
}

/// N8: `std/ws` and `std/tls` are in the `net` domain: a module without the
/// grant cannot name `ws.connect`, `tls.handshake` or `tls.accept`, and with a
/// narrow grant `ws.connect` to another host is denied by the same check.
#[test]
pub(crate) fn n08_ws_and_tls_are_behind_the_net_gate() {
    for (import, body) in [
        ("std/ws", "    val c = result.expect(ws.connect(\"127.0.0.1\", 9, \"/\"), \"ws.connect: the call failed\")"),
        ("std/tls", "    print(tls.server_options().floor)"),
    ] {
        let lab = Lab::new("n08a");
        lab.simple("", &[import], body);
        lab.refused(&["needs the 'net' capability"]);
    }
    let lab = Lab::new("n08b");
    lab.src(
        "main.kp",
        "impl \"util\" caps(net = [\"127.0.0.1:9\"], rand = [])\n\nfun main() {\n    util.go()\n}\n",
    );
    lab.src(
        "util.kp",
        "impl \"std/ws\" caps(net = [\"127.0.0.1:9\"], rand = [])\n\npub fun go() {\n    val c = result.expect(ws.connect(\"127.0.0.1\", 10, \"/\"), \"ws.connect: the call failed\")\n}\n",
    );
    lab.denied("is outside the net capability");
}

/// N9: TLS trust roots never come from a path the runtime opens. The old
/// `ca_file`/`ca_dir` options let a module with only a `net` grant make the
/// handshake open any readable path (and trust whatever it held). They are gone:
/// the options take PEM bytes (`with_ca_pem`), and the only way to turn a path
/// into bytes is `std/tlsfile`, which needs an `fs` grant.
#[test]
pub(crate) fn n09_a_net_only_module_cannot_name_a_ca_path() {
    for body in [
        "    val o = tls.client_options().with_ca_file(\"__T__/secret/s.txt\")",
        "    val o = tls.TlsOptions(ca_file = String.from_str(\"__T__/secret/s.txt\"), ca_dir = String.empty(), timeout_ms = 0)",
        "    val o = tls.TlsOptions(ca_pem = Bytes.empty(), ca_dir = String.from_str(\"__T__/secret\"), timeout_ms = 0)",
    ] {
        let lab = Lab::new("n09a");
        lab.simple("net = [\"127.0.0.1:9\"]", &["std/net", "std/tls"], body);
        let out = lab.build();
        assert!(!out.status.success(), "a CA path option compiled: {body}");
        lab.assert_untouched();
    }
}

/// N9b: reading a CA file is an `fs` read: a module with a `net` grant and no
/// `fs` grant cannot use `std/tlsfile`, and the refusal names `fs`.
#[test]
pub(crate) fn n09b_reading_a_ca_file_needs_the_fs_capability() {
    let lab = Lab::new("n09b");
    lab.simple(
        "net = [\"127.0.0.1:9\"]",
        &["std/net", "std/tls", "std/tlsfile"],
        "    when tlsfile.ca_pem(\"__T__/secret/s.txt\") {\n        Ok(_) -> print(\"read\")\n        Err(_) -> print(\"denied\")\n    }",
    );
    lab.refused(&["needs the 'fs' capability"]);
}

/// N9c: with an `fs` grant the CA path is checked by the `fs` matcher (same
/// normalisation and link rules as `fs.read`): a path outside the grant,
/// existing, missing or reached through `..`, is the standard `fs` denial and
/// the file is never opened; one inside is read.
#[test]
pub(crate) fn n09c_a_ca_path_outside_the_fs_grant_is_denied_like_fs_read() {
    for path in ["__T__/secret/s.txt", "__T__/secret/missing.pem", "__T__/data/../secret/s.txt"] {
        let lab = Lab::new("n09c");
        let grant = format!("fs = [\"{}/data/**\"]", lab.root.display());
        lab.src("main.kp", &format!("impl \"util\" caps({grant})\n\nfun main() {{\n    util.go()\n}}\n"));
        lab.src(
            "util.kp",
            &format!("impl \"std/tls\"\nimpl \"std/tlsfile\" caps({grant})\n\npub fun go() {{\n    when tlsfile.ca_pem(\"{path}\") {{\n        Ok(b) -> print(b.len())\n        Err(e) -> print(\"typed error\")\n    }}\n}}\n"),
        );
        lab.denied("is outside the fs capability");
    }
    let lab = Lab::new("n09c_in");
    let grant = format!("fs = [\"{}/data/**\"]", lab.root.display());
    lab.src("main.kp", &format!("impl \"util\" caps({grant})\n\nfun main() {{\n    util.go()\n}}\n"));
    lab.src(
        "util.kp",
        &format!("impl \"std/tls\"\nimpl \"std/tlsfile\" caps({grant})\n\npub fun go() {{\n    when tlsfile.ca_pem(\"__T__/data/ok.txt\") {{\n        Ok(b) -> print(b.len())\n        Err(e) -> print(\"typed error\")\n    }}\n}}\n"),
    );
    lab.prints("5\n");
}

// ---------------------------------------------------------------------------
// handles
// ---------------------------------------------------------------------------

/// H1: a module that holds no grant cannot build a descriptor-carrying handle
/// from a number. `io.File(fd = n)` was a constructor any module could call, so
/// a dependency with no authority at all could read, write or close every
/// descriptor another module had opened (a secret file, a socket). The refusal
/// is at compile time, and names the capability that would be needed.
#[test]
pub(crate) fn h01_a_module_without_a_grant_cannot_forge_a_descriptor_handle() {
    for (what, imports, body, needle) in [
        (
            "file_scan",
            &["std/io", "std/str"][..],
            "    var i: i32 = 3\n    while i < 40 {\n        val f = io.File(fd = i)\n        if f.is_open() {\n            print(String.from_bytes(f.read(16)))\n        }\n        i = i + 1\n    }",
            "wraps an operating-system descriptor",
        ),
        (
            "socket",
            &["std/net"][..],
            "    val s = net.Socket(fd = 5)\n    net.send_text(s, \"x\")",
            "needs the 'net' capability, which module 'util' was not granted",
        ),
        (
            "listener",
            &["std/net"][..],
            "    val l = net.Listener(fd = 5)\n    print(l.port())",
            "needs the 'net' capability, which module 'util' was not granted",
        ),
        (
            "child",
            &["std/process", "std/io"][..],
            "    val c = process.Child(pid = 1, stdin = io.File(fd = 0), stdout = io.File(fd = 1), stderr = io.File(fd = 2), exited = false, code = 0, signal = 0)\n    c.kill(9)",
            "wraps an operating-system descriptor",
        ),
    ] {
        let lab = Lab::new(&format!("h01_{what}"));
        // The secret is opened (and, as programs do, not closed) by a module
        // that was granted it; the module under test has no grant.
        lab.src(
            "main.kp",
            "impl \"opener\" caps(fs = [\"__T__/secret/**\"])\nimpl \"util\"\n\nfun main() {\n    opener.open_and_leak()\n    util.go()\n}\n",
        );
        lab.src(
            "opener.kp",
            "impl \"std/fs\"\n\npub fun open_and_leak() {\n    val f = result.expect(fs.open(\"__T__/secret/s.txt\", \"r\"), \"fs.open: the call failed\")\n}\n",
        );
        let mut util = String::new();
        for i in imports {
            util.push_str(&format!("impl \"{i}\"\n"));
        }
        if !imports.contains(&"std/result") {
            util.push_str("impl \"std/result\"\n");
        }
        util.push_str(&format!("\npub fun go() {{\n{body}\n}}\n"));
        lab.src("util.kp", &util);
        lab.refused(&[needle]);
    }
}

/// H2: holding one domain does not let a module forge a handle of another's:
/// a module with only `fs` cannot build a socket or a child.
#[test]
pub(crate) fn h02_a_domain_mints_only_its_own_handles() {
    for (grant, imports, body) in [
        ("fs = [\"/**\"]", &["std/fs", "std/net"][..], "    val s = net.Socket(fd = 5)"),
        (
            "net = [\"*\"]",
            &["std/net", "std/process", "std/io"][..],
            "    val c = process.Child(pid = 1, stdin = io.File(fd = 0), stdout = io.File(fd = 1), stderr = io.File(fd = 2), exited = false, code = 0, signal = 0)",
        ),
    ] {
        let lab = Lab::new("h02");
        lab.simple(grant, imports, body);
        lab.refused(&["needs the"]);
    }
}

/// H3 (known gap): two modules that each hold a domain can still forge
/// between each other's descriptors. `io.File(fd = n)` is open to any module
/// that holds `fs`, `net` or `process`, so a module granted only `data/**` can
/// read a descriptor another module opened on `secret/`. Closing it needs
/// opaque handle types (no integer constructor outside the library) and an
/// explicit, gated `from_fd`; see `reference/capability-threat-model.md` (kelp wiki), Known gaps.
#[test]
#[ignore = "known gap: descriptors are small integers, forgeable between two modules that both hold a domain (plan/threat-model.md H3)"]
pub(crate) fn h03_a_granted_module_can_read_a_descriptor_another_module_opened() {
    let lab = Lab::new("h03");
    lab.src(
        "main.kp",
        "impl \"opener\" caps(fs = [\"__T__/secret/**\"])\nimpl \"util\" caps(fs = [\"__T__/data/**\"])\n\nfun main() {\n    opener.open_and_leak()\n    util.go()\n}\n",
    );
    lab.src(
        "opener.kp",
        "impl \"std/fs\"\n\npub fun open_and_leak() {\n    val f = result.expect(fs.open(\"__T__/secret/s.txt\", \"r\"), \"fs.open: the call failed\")\n}\n",
    );
    lab.src(
        "util.kp",
        "impl \"std/fs\"\nimpl \"std/io\"\n\npub fun go() {\n    var i: i32 = 3\n    while i < 40 {\n        val f = io.File(fd = i)\n        if f.is_open() {\n            print(String.from_bytes(f.read(16)))\n        }\n        i = i + 1\n    }\n}\n",
    );
    // The attack is expected to be refused; today it leaks.
    let b = lab.build();
    if b.status.success() {
        let out = lab.run(&[]);
        assert!(
            !String::from_utf8_lossy(&out.stdout).contains(TOKEN),
            "the secret leaked through a forged descriptor"
        );
    }
}

// ---------------------------------------------------------------------------
// unsafe audit list
// ---------------------------------------------------------------------------
