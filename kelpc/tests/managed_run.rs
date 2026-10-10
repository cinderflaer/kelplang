//! Managed mode, end to end: a program whose allocations exceed the initial
//! heap must collect and still print the right sum. Needs the collector
//! archive (`KELP_GC_LIB`, built from `kelpc/gc` with `cargo build --release`),
//! so the test is `#[ignore]`d and run with `--ignored`.

use std::path::{Path, PathBuf};
use std::process::Command;

fn kelpc() -> Command {
    Command::new(env!("CARGO_BIN_EXE_kelpc"))
}

/// Build the churn fixture into a scratch directory and return the binary.
fn build_churn(dir: &Path) -> PathBuf {
    let _ = std::fs::remove_dir_all(dir);
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/managed/churn");
    std::fs::create_dir_all(dir.join("src")).expect("create scratch dir");
    std::fs::copy(src.join("src/main.kp"), dir.join("src/main.kp")).expect("copy main.kp");
    std::fs::copy(src.join("coral.toml"), dir.join("coral.toml")).expect("copy coral.toml");
    let bin = dir.join("churn");
    let build = kelpc()
        .arg("build")
        .arg(dir)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the managed build failed:\n{}{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );
    bin
}

#[test]
#[ignore = "needs KELP_GC_LIB: the collector archive from kelpc/gc"]
fn managed_program_that_exceeds_the_initial_heap_collects_and_prints_the_sum() {
    assert!(std::env::var_os("KELP_GC_LIB").is_some(), "set KELP_GC_LIB to libkelp_gc.a");
    let dir = std::env::temp_dir().join(format!("kelpc-managed-run-{}", std::process::id()));
    let bin = build_churn(&dir);
    let script = format!("ulimit -v 1048576; ulimit -t 10; timeout 10 '{}'", bin.display());
    let run = Command::new("sh").arg("-c").arg(&script).output().expect("run managed program");
    let expected = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/managed/churn/expected.txt"),
    )
    .expect("read expected.txt");
    assert!(run.status.success(), "exit {:?}: {}", run.status, String::from_utf8_lossy(&run.stderr));
    assert_eq!(String::from_utf8_lossy(&run.stdout), expected);
}
