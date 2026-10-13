//! The `=>` spelling of the arrow. `=>` and `->` lex as one token, so every
//! position that takes `->` (`when` arms, the parameter header of a trailing
//! block) takes `=>` too, and `>=` and `==` still lex as comparisons. Each
//! fixture under `tests/arrow/` is compiled with the real `kelpc`, run, and its
//! stdout checked against the output written here.

use std::path::{Path, PathBuf};
use std::process::Command;

fn kelpc() -> Command {
    let mut cmd = Command::new(PathBuf::from(env!("CARGO_BIN_EXE_kelpc")));
    cmd.env_remove("KELPC_BACKEND");
    cmd.env_remove("KELPC_ORACLE");
    cmd
}

/// Builds `tests/arrow/<name>.kp`, runs it and asserts its stdout is `expected`.
fn assert_runs(name: &str, expected: &str) {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("arrow")
        .join(format!("{name}.kp"));
    let mut bin = PathBuf::from(
        std::env::var("CARGO_TARGET_TMPDIR")
            .unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().to_string()),
    );
    bin.push(format!("kelpc_arrow_both_{name}"));

    let build = kelpc()
        .arg("build")
        .arg(&source)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected '{name}.kp' to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let run = Command::new(&bin).output().expect("failed to run compiled binary");
    assert!(
        run.status.success(),
        "expected the binary for '{name}.kp' to exit successfully, got:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        expected,
        "unexpected stdout for '{name}.kp'"
    );
}

#[test]
fn fat_arrow_in_a_when_without_scrutinee() {
    assert_runs("when_cond", "pos\nneg\nzero\n");
}

#[test]
fn fat_arrow_in_a_when_over_an_enum() {
    assert_runs("when_enum", "12\n10\n");
}

#[test]
fn fat_arrow_in_a_tuple_when() {
    assert_runs("when_tuple", "100\n7\n40\n5\n");
}

#[test]
fn fat_arrow_ends_a_trailing_block_header() {
    assert_runs("block_params", "4\n5\n6\n150\n34\n");
}

#[test]
fn comparisons_and_the_arrow_coexist() {
    assert_runs("compare_ops", "ge\neq\nle\nne\ngt\n");
}
