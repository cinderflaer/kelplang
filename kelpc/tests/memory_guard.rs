//! The compiler's own memory floor (`src/guard.rs`): with a floor no machine
//! can meet, `kelpc build` ends itself with status 75 and a message naming the
//! variable that changes it; with the guard turned off the same build succeeds.

use std::path::PathBuf;
use std::process::Command;

fn kelpc() -> Command {
    Command::new(env!("CARGO_BIN_EXE_kelpc"))
}

fn hello() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/examples/hello.kp")
}

fn out(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kelpc_memory_guard_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    dir.join(name)
}

#[test]
fn a_floor_no_machine_meets_ends_the_build_with_status_75() {
    let bin = out("refused");
    let run = kelpc()
        .env("KELPC_MIN_AVAILABLE_MB", "1000000000000")
        .arg("build")
        .arg(hello())
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert_eq!(run.status.code(), Some(75), "stderr: {}", String::from_utf8_lossy(&run.stderr));
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(stderr.contains("kelpc stopped itself"), "unexpected stderr: {stderr}");
    assert!(stderr.contains("KELPC_MIN_AVAILABLE_MB"), "unexpected stderr: {stderr}");
}

/// The floor with unit workers running (roadmap A-14 slice 3): the guard's
/// `_exit(75)` is process-wide, so it must end an `--incremental` build with
/// workers the same way it ends a whole-program one, not crash or hang. The
/// worker count is spelled out so the parallel path is the one under test.
#[test]
fn the_floor_ends_an_incremental_build_with_workers() {
    let bin = out("refused_incremental");
    let run = kelpc()
        .env("KELPC_MIN_AVAILABLE_MB", "1000000000000")
        .env("KELPC_JOBS", "4")
        .arg("build")
        .arg(hello())
        .arg("-o")
        .arg(&bin)
        .arg("--incremental")
        .output()
        .expect("failed to spawn kelpc");
    assert_eq!(run.status.code(), Some(75), "stderr: {}", String::from_utf8_lossy(&run.stderr));
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(stderr.contains("kelpc stopped itself"), "unexpected stderr: {stderr}");
    assert!(stderr.contains("KELPC_MIN_AVAILABLE_MB"), "unexpected stderr: {stderr}");
}

#[test]
fn a_floor_of_zero_turns_the_guard_off() {
    let bin = out("allowed");
    let run = kelpc()
        .env("KELPC_MIN_AVAILABLE_MB", "0")
        .arg("build")
        .arg(hello())
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        run.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
}
