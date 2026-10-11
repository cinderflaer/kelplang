//! Managed runtime, end to end: a worker leaves the collector when it ends, a
//! thread blocked in `thread.join` does not hold up a collection, and a long
//! runtime scan polls the collector. Each program allocates past its heap, so
//! the collection must stop every attached thread; a thread that never reaches
//! a safepoint makes the collector abort at its stop timeout, which is short
//! here so a regression fails in seconds.
//!
//! Needs the collector archive (`KELP_GC_LIB`, built from `kelpc/gc` with
//! `cargo build --release`), so the tests are `#[ignore]`d and run with
//! `--include-ignored`.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Builds the named fixture under `tests/managed/` and runs it with the given
/// collector stop timeout. Returns the binary's stdout after checking the exit
/// status.
fn build_and_run(fixture: &str, stop_timeout_ms: u32) -> String {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/managed").join(fixture);
    let dir = std::env::temp_dir().join(format!("kelpc-managed-rt-{fixture}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).expect("create scratch dir");
    std::fs::copy(root.join("src/main.kp"), dir.join("src/main.kp")).expect("copy main.kp");
    std::fs::copy(root.join("coral.toml"), dir.join("coral.toml")).expect("copy coral.toml");
    let bin = dir.join(fixture);
    let build = Command::new(env!("CARGO_BIN_EXE_kelpc"))
        .arg("build")
        .arg(&dir)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the managed build of {fixture} failed:\n{}{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );
    let script = format!("ulimit -v 4194304; ulimit -t 60; timeout 60 '{}'", bin.display());
    let run = Command::new("sh")
        .arg("-c")
        .arg(&script)
        .env("KELP_GC_STOP_TIMEOUT_MS", stop_timeout_ms.to_string())
        .output()
        .expect("run managed program");
    let stderr = String::from_utf8_lossy(&run.stderr).to_string();
    assert!(run.status.success(), "{fixture}: exit {:?}: {stderr}", run.status);
    String::from_utf8_lossy(&run.stdout).to_string()
}

fn expected(fixture: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/managed").join(fixture).join("expected.txt");
    std::fs::read_to_string(path).expect("read expected.txt")
}

fn require_archive() {
    assert!(std::env::var_os("KELP_GC_LIB").is_some(), "set KELP_GC_LIB to libkelp_gc.a");
}

#[test]
#[ignore = "needs KELP_GC_LIB: the collector archive from kelpc/gc"]
fn a_worker_that_ends_leaves_the_collector() {
    require_archive();
    assert_eq!(build_and_run("worker_detach", 3000), expected("worker_detach"));
}

#[test]
#[ignore = "needs KELP_GC_LIB: the collector archive from kelpc/gc"]
fn a_thread_blocked_in_join_does_not_hold_up_a_collection() {
    require_archive();
    assert_eq!(build_and_run("join_collect", 3000), expected("join_collect"));
}

#[test]
#[ignore = "needs KELP_GC_LIB: the collector archive from kelpc/gc"]
fn a_long_text_scan_polls_the_collector() {
    require_archive();
    assert_eq!(build_and_run("text_poll", 20), expected("text_poll"));
}
