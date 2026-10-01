//! The compiler's own memory floor (`src/guard.rs`): with a floor no machine
//! can meet, `kelpc build` ends itself with status 75 and a message naming the
//! variable that changes it; with the guard turned off the same build succeeds.

use std::path::PathBuf;
use std::process::Command;

fn kelpc() -> Command {
    Command::new(env!("CARGO_BIN_EXE_kelpc"))
}

fn hello() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../examples/hello.kp")
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
