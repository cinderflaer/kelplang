//! End-to-end regression suite: compiles `.x` fixtures with the real `xc`
//! binary, then either runs the resulting binary and checks its stdout, or
//! (for fixtures that should be rejected) checks that `xc build` fails.

use std::path::{Path, PathBuf};
use std::process::Command;

fn xc_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_xc"))
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(name)
}

fn out_path(name: &str) -> PathBuf {
    let mut p = PathBuf::from(std::env::var("CARGO_TARGET_TMPDIR").unwrap_or_else(|_| {
        std::env::temp_dir().to_string_lossy().to_string()
    }));
    p.push(format!("xc_test_{name}"));
    p
}

/// Compiles `source` and asserts the resulting binary's stdout equals
/// `expected` exactly.
fn assert_compiles_and_runs(source: &str, expected: &str) {
    let name = Path::new(source).file_stem().unwrap().to_string_lossy().to_string();
    let bin = out_path(&name);
    let build = Command::new(xc_bin())
        .arg("build")
        .arg(fixture(source))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn xc");
    assert!(
        build.status.success(),
        "expected '{source}' to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let run = Command::new(&bin).output().expect("failed to run compiled binary");
    assert!(
        run.status.success(),
        "expected the binary for '{source}' to exit successfully, got:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert_eq!(stdout, expected, "unexpected stdout for '{source}'");
}

/// Asserts that `xc build` rejects `source` (a compile error is expected).
fn assert_rejected(source: &str) {
    let name = Path::new(source).file_stem().unwrap().to_string_lossy().to_string();
    let bin = out_path(&name);
    let build = Command::new(xc_bin())
        .arg("build")
        .arg(fixture(source))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn xc");
    assert!(
        !build.status.success(),
        "expected '{source}' to be rejected by the compiler, but it built successfully"
    );
}

#[test]
fn hello_prints_greeting() {
    assert_compiles_and_runs("../examples/hello.x", "hello, world\n");
}

#[test]
fn fib_computes_correctly() {
    assert_compiles_and_runs("../examples/fib.x", "55\n");
}

#[test]
fn if_expression_classifies_correctly() {
    assert_compiles_and_runs("tests/fixtures/if_expr.x", "-1\n0\n1\n");
}

#[test]
fn type_mismatch_is_rejected() {
    assert_rejected("tests/fixtures/type_mismatch.x");
}

#[test]
fn datac_fields_and_mutation_work() {
    assert_compiles_and_runs("tests/fixtures/datac_basic.x", "3\n4\n25\n11\n");
}

#[test]
fn while_loop_accumulates_correctly() {
    assert_compiles_and_runs("tests/fixtures/while_accumulator.x", "55\n");
}

#[test]
fn assignment_to_immutable_binding_is_rejected() {
    assert_rejected("tests/fixtures/assign_to_immutable.x");
}

#[test]
fn assignment_to_val_field_is_rejected() {
    assert_rejected("tests/fixtures/val_field_immutable.x");
}

#[test]
fn when_desugars_to_if_else_chain() {
    assert_compiles_and_runs("tests/fixtures/when_expr.x", "pos\nneg\nzero\n");
}

#[test]
fn named_and_positional_args_match_correctly() {
    assert_compiles_and_runs("tests/fixtures/named_args.x", "7\n7\n7\n");
}

#[test]
fn use_after_move_is_rejected() {
    assert_rejected("tests/fixtures/use_after_move.x");
}

#[test]
fn double_move_in_loop_is_rejected() {
    assert_rejected("tests/fixtures/double_move_in_loop.x");
}

#[test]
fn move_then_reinit_is_allowed() {
    assert_compiles_and_runs("tests/fixtures/move_then_reinit.x", "3\n3\n7\n");
}

#[test]
fn field_read_does_not_move_the_container() {
    assert_compiles_and_runs("tests/fixtures/field_read_does_not_move.x", "3\n4\n7\n");
}

#[test]
fn borrow_basic_shared_and_exclusive_work() {
    assert_compiles_and_runs("tests/fixtures/borrow_basic.x", "1\n3\n2\n");
}

#[test]
fn simultaneous_shared_and_exclusive_borrow_is_rejected() {
    assert_rejected("tests/fixtures/borrow_conflict.x");
}

#[test]
fn conflicting_borrows_in_the_same_call_are_rejected() {
    assert_rejected("tests/fixtures/same_call_aliasing.x");
}

#[test]
fn writing_through_a_shared_borrow_is_rejected() {
    assert_rejected("tests/fixtures/write_through_shared_borrow.x");
}

#[test]
fn moving_a_borrowed_value_is_rejected() {
    assert_rejected("tests/fixtures/move_while_borrowed.x");
}

#[test]
fn receiver_methods_resolve_and_mutate_through_self() {
    assert_compiles_and_runs("tests/fixtures/receiver_methods.x", "12.566360\n28.274310\n");
}

#[test]
fn mut_receiver_method_on_val_binding_is_rejected() {
    assert_rejected("tests/fixtures/receiver_mut_on_val.x");
}
