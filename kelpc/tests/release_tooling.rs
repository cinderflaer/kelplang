//! The release tooling: `build/install.sh` and `kelpc version --verbose`.
//!
//! The installer is exercised with a stub binary and a temporary prefix. Its two
//! test-only environment overrides (`KELP_INSTALL_PLATFORM`,
//! `KELP_INSTALL_SKIP_CHECKS`) keep the tests independent of the host's `cc`,
//! LLVM and architecture. The reproducibility check itself is
//! `build/verify-reproducible.sh`, which needs two full release builds and is
//! run by hand; its last result is recorded in `plan/reproducible-builds.md`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn install_script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../build/install.sh")
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kelp-release-tooling-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A stub "release binary" and its real SHA-256 (via `sha256sum`).
fn stub(dir: &Path) -> (PathBuf, String) {
    let file = dir.join("kelpc-26.09.9-linux-x86_64");
    fs::write(&file, "#!/bin/sh\necho stub kelpc\n").unwrap();
    let out = Command::new("sha256sum").arg(&file).output().unwrap();
    let hex = String::from_utf8(out.stdout)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_string();
    (file, hex)
}

fn install(args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new("sh");
    cmd.arg(install_script()).args(args);
    cmd.env("KELP_INSTALL_SKIP_CHECKS", "1");
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.output().expect("failed to run install.sh")
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn a_wrong_checksum_is_refused_and_installs_nothing() {
    let dir = scratch("wrong");
    let (file, _) = stub(&dir);
    let prefix = dir.join("prefix");
    let bad = "0".repeat(64);
    let out = install(
        &["--from", file.to_str().unwrap(), "--sha256", &bad, "--prefix", prefix.to_str().unwrap()],
        &[],
    );
    assert!(!out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("checksum mismatch"), "{}", text(&out));
    assert!(!prefix.join("bin/kelpc").exists());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_missing_verification_is_refused_unless_explicitly_skipped() {
    let dir = scratch("noverify");
    let (file, _) = stub(&dir);
    let prefix = dir.join("prefix");
    let p = prefix.to_str().unwrap();
    let out = install(&["--from", file.to_str().unwrap(), "--prefix", p], &[]);
    assert!(!out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("without verification"), "{}", text(&out));
    assert!(!prefix.join("bin/kelpc").exists());

    let out = install(
        &["--from", file.to_str().unwrap(), "--prefix", p, "--insecure-skip-verify"],
        &[],
    );
    assert!(out.status.success(), "{}", text(&out));
    assert!(prefix.join("bin/kelpc").exists());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_right_checksum_or_a_sums_file_installs() {
    let dir = scratch("right");
    let (file, hex) = stub(&dir);
    let p1 = dir.join("p1");
    let out = install(
        &["--from", file.to_str().unwrap(), "--sha256", &hex, "--prefix", p1.to_str().unwrap()],
        &[],
    );
    assert!(out.status.success(), "{}", text(&out));
    assert!(p1.join("bin/kelpc").exists());

    let sums = dir.join("SHA256SUMS");
    fs::write(&sums, format!("{} other-file\n{hex}  kelpc-26.09.9-linux-x86_64\n", "1".repeat(64))).unwrap();
    let p2 = dir.join("p2");
    let out = install(
        &["--from", file.to_str().unwrap(), "--sums", sums.to_str().unwrap(), "--prefix", p2.to_str().unwrap()],
        &[],
    );
    assert!(out.status.success(), "{}", text(&out));
    assert!(p2.join("bin/kelpc").exists());

    fs::write(&sums, format!("{}  kelpc-26.09.9-linux-x86_64\n", "2".repeat(64))).unwrap();
    let p3 = dir.join("p3");
    let out = install(
        &["--from", file.to_str().unwrap(), "--sums", sums.to_str().unwrap(), "--prefix", p3.to_str().unwrap()],
        &[],
    );
    assert!(!out.status.success(), "{}", text(&out));
    assert!(!p3.join("bin/kelpc").exists());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn uninstall_removes_only_what_was_installed() {
    let dir = scratch("uninstall");
    let (file, hex) = stub(&dir);
    let prefix = dir.join("prefix");
    let p = prefix.to_str().unwrap();
    // Files the installer does not own, in the very directories it uses.
    fs::create_dir_all(prefix.join("bin")).unwrap();
    fs::create_dir_all(prefix.join("share/kelpc")).unwrap();
    fs::write(prefix.join("bin/other-tool"), "x").unwrap();
    fs::write(prefix.join("share/kelpc/mine.txt"), "x").unwrap();

    let out = install(&["--from", file.to_str().unwrap(), "--sha256", &hex, "--prefix", p], &[]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(prefix.join("bin/kelpc").exists());

    let out = install(&["--uninstall", "--prefix", p], &[]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(!prefix.join("bin/kelpc").exists());
    assert!(!prefix.join("share/kelpc/manifest").exists());
    assert!(prefix.join("bin/other-tool").exists());
    assert!(prefix.join("share/kelpc/mine.txt").exists());

    // A second uninstall has nothing to do and says so.
    let out = install(&["--uninstall", "--prefix", p], &[]);
    assert!(!out.status.success());
    assert!(text(&out).contains("nothing to uninstall"), "{}", text(&out));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn an_unsupported_platform_is_refused_with_a_clear_message() {
    let dir = scratch("platform");
    let (file, hex) = stub(&dir);
    let prefix = dir.join("prefix");
    for fake in ["Darwin arm64", "Linux aarch64"] {
        let out = install(
            &["--from", file.to_str().unwrap(), "--sha256", &hex, "--prefix", prefix.to_str().unwrap()],
            &[("KELP_INSTALL_PLATFORM", fake)],
        );
        assert!(!out.status.success(), "{}", text(&out));
        let msg = text(&out);
        assert!(msg.contains("unsupported platform") && msg.contains(fake) && msg.contains("Linux x86_64"), "{msg}");
        assert!(!prefix.join("bin/kelpc").exists());
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn missing_dependencies_are_named() {
    let dir = scratch("deps");
    let (file, hex) = stub(&dir);
    let prefix = dir.join("prefix");
    // A PATH holding only `tr` and `cat` finds no `ld.lld`: the message must say so.
    let tools = dir.join("tools");
    fs::create_dir_all(&tools).unwrap();
    for tool in ["tr", "cat"] {
        let found = Command::new("sh").args(["-c", &format!("command -v {tool}")]).output().unwrap();
        let found = String::from_utf8(found.stdout).unwrap();
        std::os::unix::fs::symlink(found.trim(), tools.join(tool)).unwrap();
    }
    let out = Command::new("/bin/sh")
        .arg(install_script())
        .args(["--from", file.to_str().unwrap(), "--sha256", &hex, "--prefix", prefix.to_str().unwrap()])
        .env_clear()
        .env("PATH", tools.to_str().unwrap())
        .env("KELP_INSTALL_PLATFORM", "Linux x86_64")
        .env("HOME", dir.to_str().unwrap())
        .output()
        .unwrap();
    assert!(!out.status.success(), "{}", text(&out));
    let msg = text(&out);
    assert!(msg.contains("missing runtime dependencies") && msg.contains("ld.lld"), "{msg}");
    assert!(msg.contains("Debian/Ubuntu") && msg.contains("NixOS") && msg.contains("Fedora"), "{msg}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn version_verbose_reports_the_baked_in_fields() {
    let out = Command::new(env!("CARGO_BIN_EXE_kelpc"))
        .args(["version", "--verbose"])
        .output()
        .expect("failed to spawn kelpc");
    assert!(out.status.success(), "{}", text(&out));
    let printed = String::from_utf8_lossy(&out.stdout).to_string();
    let mut lines = printed.lines();
    assert!(lines.next().unwrap().starts_with("kelpc "), "{printed}");
    let field = |name: &str| -> String {
        printed
            .lines()
            .find_map(|l| l.strip_prefix(&format!("{name}: ")))
            .unwrap_or_else(|| panic!("no '{name}:' line in\n{printed}"))
            .to_string()
    };
    let commit = field("commit");
    assert!(
        commit == "unknown" || (commit.len() == 40 && commit.chars().all(|c| c.is_ascii_hexdigit())),
        "commit '{commit}'"
    );
    let date = field("build date");
    assert!(
        date == "unknown" || (date.len() == 10 && date.as_bytes()[4] == b'-' && date.as_bytes()[7] == b'-'),
        "date '{date}'"
    );
    assert!(field("rustc").starts_with("rustc ") || field("rustc") == "unknown");
    let llvm = field("llvm");
    assert!(llvm.starts_with("21.") || llvm == "unknown", "llvm '{llvm}'");

    // The plain form stays one line.
    let plain = Command::new(env!("CARGO_BIN_EXE_kelpc")).arg("version").output().unwrap();
    assert_eq!(String::from_utf8_lossy(&plain.stdout).lines().count(), 1);
}
