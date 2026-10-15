//! Tidepool heap, end to end: programs whose allocations exceed the initial
//! heap must collect and still print the right sums. Needs the collector
//! archive (`KELP_GC_LIB`, built from `kelpc/gc` with `cargo build --release`),
//! so the tests are `#[ignore]`d and run with `--ignored`.

use std::path::{Path, PathBuf};
use std::process::Command;

fn kelpc() -> Command {
    Command::new(env!("CARGO_BIN_EXE_kelpc"))
}

/// Build the named fixture under `tests/managed/` into a scratch directory and
/// run it. Returns the binary's stdout after checking the exit status.
fn build_and_run(fixture: &str) -> (String, String) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/managed").join(fixture);
    let dir = std::env::temp_dir().join(format!("kelpc-managed-{fixture}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).expect("create scratch dir");
    std::fs::copy(root.join("src/main.kp"), dir.join("src/main.kp")).expect("copy main.kp");
    std::fs::copy(root.join("coral.toml"), dir.join("coral.toml")).expect("copy coral.toml");
    let bin = dir.join(fixture);
    let build = kelpc()
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
    let script = format!("ulimit -v 1048576; ulimit -t 10; timeout 10 '{}'", bin.display());
    let run = Command::new("sh").arg("-c").arg(&script).output().expect("run managed program");
    let stderr = String::from_utf8_lossy(&run.stderr).to_string();
    assert!(run.status.success(), "{fixture}: exit {:?}: {stderr}", run.status);
    (String::from_utf8_lossy(&run.stdout).to_string(), stderr)
}

fn expected(fixture: &str) -> String {
    std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/managed").join(fixture).join("expected.txt"),
    )
    .expect("read expected.txt")
}

#[test]
#[ignore = "needs KELP_GC_LIB: the collector archive from kelpc/gc"]
fn managed_program_that_exceeds_the_initial_heap_collects_and_prints_the_sum() {
    assert!(std::env::var_os("KELP_GC_LIB").is_some(), "set KELP_GC_LIB to libkelp_gc.a");
    let (stdout, _) = build_and_run("churn");
    assert_eq!(stdout, expected("churn"));
}

#[test]
#[ignore = "needs KELP_GC_LIB: the collector archive from kelpc/gc"]
fn managed_enum_program_that_exceeds_the_initial_heap_collects_and_prints_the_sum() {
    assert!(std::env::var_os("KELP_GC_LIB").is_some(), "set KELP_GC_LIB to libkelp_gc.a");
    let (stdout, _) = build_and_run("enum_churn");
    assert_eq!(stdout, expected("enum_churn"));
}
