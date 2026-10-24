//! End-to-end regression suite: compiles `.kp` fixtures with the real `kelpc`
//! binary, then either runs the resulting binary and checks its stdout, or
//! (for fixtures that should be rejected) checks that `kelpc build` fails.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

fn kelpc_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_kelpc"))
}

/// A `kelpc` invocation. The suite compiles a fixture, runs the result and
/// compares stdout, so it never names a code generator: `kelpc` has one, the
/// LLVM backend. The retired variables (`KELPC_BACKEND` and the `KELPC_ORACLE`
/// switch, removed with the C backend on 2026-10-10) are stripped so a
/// developer's stale export cannot turn every build in the run into the refusal
/// those variables now earn.
fn kelpc() -> Command {
    let mut cmd = Command::new(kelpc_bin());
    cmd.env_remove("KELPC_BACKEND");
    cmd.env_remove("KELPC_ORACLE");
    cmd
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(name)
}

fn out_path(name: &str) -> PathBuf {
    let mut p = PathBuf::from(std::env::var("CARGO_TARGET_TMPDIR").unwrap_or_else(|_| {
        std::env::temp_dir().to_string_lossy().to_string()
    }));
    // The name of the test is part of the path: two tests may build one
    // fixture, and a second test's `exec` of a file the first is still writing
    // fails with `Text file busy` (BUG-r4Nc8). libtest names each test's thread
    // after the test, so the thread name is the test name.
    let test = std::thread::current().name().unwrap_or("main").to_string();
    p.push(format!("kelpc_test_{test}_{name}"));
    p
}

/// Compiles `source` and asserts the resulting binary's stdout equals
/// `expected` exactly.
fn assert_compiles_and_runs(source: &str, expected: &str) {
    let name = Path::new(source).file_stem().unwrap().to_string_lossy().to_string();
    let bin = out_path(&name);
    let build = kelpc()
        .arg("build")
        .arg(fixture(source))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
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

/// Like `assert_compiles_and_runs`, and also runs the binary with `env` set and
/// asserts that its stderr contains `stderr_line`.
fn assert_compiles_and_runs_with_stderr(
    source: &str,
    expected: &str,
    env: &[(&str, &str)],
    stderr_line: &str,
) {
    let name = Path::new(source).file_stem().unwrap().to_string_lossy().to_string();
    let bin = out_path(&name);
    let build = kelpc()
        .arg("build")
        .arg(fixture(source))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected '{source}' to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let mut run = Command::new(&bin);
    for (k, v) in env {
        run.env(k, v);
    }
    let run = run.output().expect("failed to run compiled binary");
    assert!(
        run.status.success(),
        "expected the binary for '{source}' to exit successfully, got:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert_eq!(stdout, expected, "unexpected stdout for '{source}'");
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.contains(stderr_line),
        "expected stderr of '{source}' to contain '{stderr_line}', got:\n{stderr}"
    );
}

/// Asserts that `kelpc build` rejects `source` (a compile error is expected).
fn assert_rejected(source: &str) {
    let name = Path::new(source).file_stem().unwrap().to_string_lossy().to_string();
    let bin = out_path(&name);
    let build = kelpc()
        .arg("build")
        .arg(fixture(source))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        !build.status.success(),
        "expected '{source}' to be rejected by the compiler, but it built successfully"
    );
}

/// Asserts that `kelpc build` rejects `source`, that the diagnostic mentions
/// every name in `names`, and that it mentions none of the names in `absent`.
fn assert_rejected_naming(source: &str, names: &[&str], absent: &[&str]) {
    let name = Path::new(source).file_stem().unwrap().to_string_lossy().to_string();
    let bin = out_path(&name);
    let build = kelpc()
        .arg("build")
        .arg(fixture(source))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        !build.status.success(),
        "expected '{source}' to be rejected by the compiler, but it built successfully"
    );
    let stderr = String::from_utf8_lossy(&build.stderr);
    for n in names {
        assert!(stderr.contains(n), "expected the error for '{source}' to name {n}, got:\n{stderr}");
    }
    for n in absent {
        assert!(!stderr.contains(n), "expected the error for '{source}' not to name {n}, got:\n{stderr}");
    }
}

/// Asserts that `kelpc build` rejects `source`, and that the diagnostic
/// mentions `needle`, so a fixture cannot pass for the wrong reason.
fn assert_rejected_with(source: &str, needle: &str) {
    let name = Path::new(source).file_stem().unwrap().to_string_lossy().to_string();
    let bin = out_path(&name);
    let build = kelpc()
        .arg("build")
        .arg(fixture(source))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        !build.status.success(),
        "expected '{source}' to be rejected by the compiler, but it built successfully"
    );
    let stderr = String::from_utf8_lossy(&build.stderr);
    assert!(
        stderr.contains(needle),
        "expected the error for '{source}' to mention '{needle}', got:\n{stderr}"
    );
}

#[test]
fn hello_prints_greeting() {
    assert_compiles_and_runs("tests/fixtures/examples/hello.kp", "hello, world\n");
}

#[test]
fn fib_computes_correctly() {
    assert_compiles_and_runs("tests/fixtures/examples/fib.kp", "55\n");
}

#[test]
fn if_expression_classifies_correctly() {
    assert_compiles_and_runs("tests/fixtures/if_expr.kp", "-1\n0\n1\n");
}

#[test]
fn type_mismatch_is_rejected() {
    assert_rejected("tests/fixtures/type_mismatch.kp");
}

#[test]
fn datac_fields_and_mutation_work() {
    assert_compiles_and_runs("tests/fixtures/datac_basic.kp", "3\n4\n25\n11\n");
}

#[test]
fn while_loop_accumulates_correctly() {
    assert_compiles_and_runs("tests/fixtures/while_accumulator.kp", "55\n");
}

#[test]
fn nested_while_keeps_the_outer_accumulator_live() {
    assert_compiles_and_runs("tests/fixtures/nested_while_registers.kp", "66\n");
}

#[test]
fn assignment_to_immutable_binding_is_rejected() {
    assert_rejected("tests/fixtures/assign_to_immutable.kp");
}

#[test]
fn assignment_to_val_field_is_rejected() {
    assert_rejected("tests/fixtures/val_field_immutable.kp");
}

#[test]
fn when_desugars_to_if_else_chain() {
    assert_compiles_and_runs("tests/fixtures/when_expr.kp", "pos\nneg\nzero\n");
}

#[test]
fn named_and_positional_args_match_correctly() {
    assert_compiles_and_runs("tests/fixtures/named_args.kp", "7\n7\n7\n");
}

#[test]
fn field_shorthand_builds_the_same_values_as_the_named_form() {
    assert_compiles_and_runs(
        "tests/fixtures/field_shorthand.kp",
        "example\n8080\n3\n7\n9\n1\n2.500000\n",
    );
}

#[test]
fn field_shorthand_without_a_local_is_refused_with_the_name() {
    assert_rejected_with("tests/fixtures/field_shorthand_no_local.kp", "no local named 'host'");
}

#[test]
fn field_shorthand_naming_no_field_is_refused_with_the_name() {
    assert_rejected_with("tests/fixtures/field_shorthand_no_field.kp", "has no field named 'hostname'");
}

#[test]
fn use_after_move_is_rejected() {
    assert_rejected("tests/fixtures/use_after_move.kp");
}

#[test]
fn double_move_in_loop_is_rejected() {
    assert_rejected("tests/fixtures/double_move_in_loop.kp");
}

#[test]
fn move_then_reinit_is_allowed() {
    assert_compiles_and_runs("tests/fixtures/move_then_reinit.kp", "3\n3\n7\n");
}

#[test]
fn field_read_does_not_move_the_container() {
    assert_compiles_and_runs("tests/fixtures/field_read_does_not_move.kp", "3\n4\n7\n");
}

#[test]
fn borrow_basic_shared_and_exclusive_work() {
    assert_compiles_and_runs("tests/fixtures/borrow_basic.kp", "1\n3\n2\n");
}

#[test]
fn simultaneous_shared_and_exclusive_borrow_is_rejected() {
    assert_rejected("tests/fixtures/borrow_conflict.kp");
}

#[test]
fn conflicting_borrows_in_the_same_call_are_rejected() {
    assert_rejected("tests/fixtures/same_call_aliasing.kp");
}

#[test]
fn writing_through_a_shared_borrow_is_rejected() {
    assert_rejected("tests/fixtures/write_through_shared_borrow.kp");
}

#[test]
fn a_borrow_of_a_borrow_is_reported_as_such() {
    // Not an inference failure naming `is_none`'s `T`: the operand is already
    // a borrow, and the rule is documented.
    assert_rejected_with(
        "tests/fixtures/borrow_of_a_borrow.kp",
        "this is a borrow of a borrow",
    );
}

#[test]
fn moving_a_borrowed_value_is_rejected() {
    assert_rejected("tests/fixtures/move_while_borrowed.kp");
}

/// A bound borrow ends at its last use, not at the end of its block (roadmap
/// open item 30, `kelp wiki page language/ownership.md`). The three accepted programs mutate, move or
/// reassign the owner after the borrowing binding is dead; the six refused
/// ones keep it live with a later use.
#[test]
fn a_bound_borrow_ends_at_its_last_use() {
    assert_compiles_and_runs(
        "tests/fixtures/nll_get_then_push.kp",
        "alpha\n3\n",
    );
    assert_compiles_and_runs(
        "tests/fixtures/nll_if_branch_then_mutate.kp",
        "alpha\n2\n",
    );
    assert_compiles_and_runs(
        "tests/fixtures/nll_last_use_before_loop.kp",
        "alpha\n3\n",
    );
}

#[test]
fn a_live_borrow_still_refuses_its_owners_mutation() {
    assert_rejected_with(
        "tests/fixtures/nll_mutate_before_use.kp",
        "already borrowed as shared",
    );
    assert_rejected_with(
        "tests/fixtures/nll_borrow_used_next_iteration.kp",
        "already borrowed as shared",
    );
    assert_rejected_with(
        "tests/fixtures/nll_scoped_block_mutates_owner.kp",
        "already borrowed as shared",
    );
    assert_rejected_with(
        "tests/fixtures/nll_defer_use_after_mutation.kp",
        "already borrowed as shared",
    );
    assert_rejected_with(
        "tests/fixtures/nll_borrow_returned_from_local.kp",
        "cannot return a borrow of 'local'",
    );
    assert_rejected_with(
        "tests/fixtures/nll_view_after_reassign.kp",
        "cannot assign to 's' while it is borrowed",
    );
}

/// A reference binding remembers the owner its value came from, so a pointer
/// local can be a cursor (roadmap open item 33, `kelp wiki page language/ownership.md`). The
/// three accepted programs walk an owned `Box` list iteratively, index a list
/// and re-point a cursor to a different owner; the refused ones keep a mutation
/// of a live owner, a second exclusive cursor, a returned frame-local cursor, a
/// moved root, or a root only one arm of an `if` assigned.
#[test]
fn a_reference_binding_remembers_its_root() {
    assert_compiles_and_runs(
        "tests/fixtures/cursor_walks_a_box_list.kp",
        "2000\n1999001\n",
    );
    assert_compiles_and_runs("tests/fixtures/cursor_over_a_list_by_index.kp", "60\n");
    assert_compiles_and_runs(
        "tests/fixtures/cursor_retarget_releases_the_old_owner.kp",
        "2\n10\n",
    );
}

#[test]
fn an_unsound_cursor_use_is_still_refused() {
    assert_rejected_with(
        "tests/fixtures/cursor_mutate_while_used.kp",
        "cannot assign to 'list' while it is borrowed",
    );
    assert_rejected_with(
        "tests/fixtures/cursor_two_mut_cursors.kp",
        "already borrowed exclusively",
    );
    assert_rejected_with(
        "tests/fixtures/cursor_returned_from_local.kp",
        "cannot return a borrow of 'local'",
    );
    assert_rejected_with(
        "tests/fixtures/cursor_root_moved.kp",
        "cannot move 'list' while it is borrowed",
    );
    assert_rejected_with(
        "tests/fixtures/cursor_branch_union.kp",
        "cannot assign to 'b' while it is borrowed",
    );
    assert_rejected_with(
        "tests/fixtures/cursor_alias_reassign.kp",
        "cannot assign to 'list' while it is borrowed",
    );
    assert_rejected_with(
        "tests/fixtures/cursor_mut_alias.kp",
        "already borrowed exclusively",
    );
    // Both halves of a value-position `if` join: `w` may point into either
    // owner, so mutating either is refused.
    assert_rejected_with(
        "tests/fixtures/cursor_branch_join.kp",
        "cannot assign to 'a' while it is borrowed",
    );
    assert_rejected_with(
        "tests/fixtures/cursor_branch_join2.kp",
        "cannot assign to 'b' while it is borrowed",
    );
}

#[test]
fn receiver_methods_resolve_and_mutate_through_self() {
    assert_compiles_and_runs("tests/fixtures/receiver_methods.kp", "12.566360\n28.274310\n");
}

#[test]
fn mut_receiver_method_on_val_binding_is_rejected() {
    assert_rejected("tests/fixtures/receiver_mut_on_val.kp");
}

#[test]
fn a_bare_mut_parameter_in_arithmetic_names_the_deref_fix() {
    // `mut T` is a borrow, so `sink += 1` reads the reference. The error has
    // to name the operand and `*sink`, not just print the `mut i64` type.
    assert_rejected_with(
        "tests/fixtures/mut_param_bare_read.kp",
        "write '*sink' to read through it",
    );
}

// ---------------------------------------------------------------------------
// M5: multi-file projects (`impl` imports).
//
// A project lives in `tests/projects/<name>/`. `kelpc build` accepts either the
// project directory (using `coral.toml`'s entry point) or any `.kp` file inside
// it; a directory with no manifest is supported too, in which case the file's
// own directory is the module root.
// ---------------------------------------------------------------------------

/// Compiles a fixture, pipes `stdin` into it, and asserts its stdout. This is
/// the only way to test `io`: its input is stdin, so it cannot be a
/// command-line argument, and `Command::output()` gives the child no stdin at
/// all (every read is end of file).
fn assert_fixture_runs_with_stdin(name: &str, stdin: &str, expected: &str) {
    run_fixture_with_stdin(name, name, stdin, expected)
}

/// The general form, for a fixture whose two tests need two binaries: the
/// binary name is a per-test `tag` so two concurrently running tests do not
/// race for one output file.
fn run_fixture_with_stdin(tag: &str, name: &str, stdin: &str, expected: &str) {
    let source = format!("tests/fixtures/{name}.kp");
    let bin = out_path(tag);
    let build = kelpc()
        .arg("build")
        .arg(fixture(&source))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected '{source}' to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let mut child = Command::new(&bin)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("failed to run compiled binary");
    child
        .stdin
        .take()
        .expect("stdin was piped")
        .write_all(stdin.as_bytes())
        .expect("failed to write to the child's stdin");
    let run = child.wait_with_output().expect("failed to wait for compiled binary");
    assert!(
        run.status.success(),
        "expected the binary for '{name}' to exit successfully, got:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        expected,
        "unexpected stdout for '{name}'"
    );
}

/// As `assert_project_runs`, with arguments appended to `argv`, which is what
/// `process.args()` reads back.
fn assert_project_runs_with_args(name: &str, target: &Path, args: &[&str], expected: &str) {
    let bin = out_path(name);
    let build = kelpc()
        .arg("build")
        .arg(target)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected project '{name}' to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let run = Command::new(&bin)
        .args(args)
        .output()
        .expect("failed to run the compiled binary");
    assert!(
        run.status.success(),
        "expected the binary for '{name}' to exit successfully, got:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        expected,
        "unexpected stdout for project '{name}'"
    );
}

fn project(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("projects")
        .join(name)
}

/// Builds a project target (directory or `.kp` file), runs the binary, and
/// asserts its stdout.
fn assert_project_runs(name: &str, target: &Path, expected: &str) {
    let bin = out_path(name);
    let build = kelpc()
        .arg("build")
        .arg(target)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected project '{name}' to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let run = Command::new(&bin).output().expect("failed to run compiled binary");
    assert!(
        run.status.success(),
        "expected the binary for '{name}' to exit successfully, got:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        expected,
        "unexpected stdout for project '{name}'"
    );
}

/// Asserts that building a project target is rejected, and that the error
/// message mentions `needle` (so the fixture can't pass for the wrong
/// reason).
fn assert_project_rejected(name: &str, target: &Path, needle: &str) {
    let bin = out_path(name);
    let build = kelpc()
        .arg("build")
        .arg(target)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        !build.status.success(),
        "expected project '{name}' to be rejected, but it built successfully"
    );
    let stderr = String::from_utf8_lossy(&build.stderr);
    assert!(
        stderr.contains(needle),
        "expected the error for '{name}' to mention '{needle}', got:\n{stderr}"
    );
}

#[path = "compile_and_run/imports.rs"]
mod imports;

#[path = "compile_and_run/heap.rs"]
mod heap;

#[path = "compile_and_run/caps_unsafe.rs"]
mod caps_unsafe;
pub(crate) use caps_unsafe::*;

#[path = "compile_and_run/externs_gating.rs"]
mod externs_gating;

#[path = "compile_and_run/std_os.rs"]
mod std_os;

#[path = "compile_and_run/consts_arrays.rs"]
mod consts_arrays;

#[path = "compile_and_run/const_infer.rs"]
mod const_infer;

#[path = "compile_and_run/index_int.rs"]
mod index_int;

#[path = "compile_and_run/numeric_names.rs"]
mod numeric_names;

#[path = "compile_and_run/collections.rs"]
mod collections;
pub(crate) use collections::*;

#[path = "compile_and_run/boxes_operators.rs"]
mod boxes_operators;

#[path = "compile_and_run/traits.rs"]
mod traits;

#[path = "compile_and_run/std_io_net.rs"]
mod std_io_net;
pub(crate) use std_io_net::*;

#[path = "compile_and_run/tls.rs"]
mod tls;
pub(crate) use tls::*;

#[path = "compile_and_run/tls_matrix.rs"]
mod tls_matrix;

#[path = "compile_and_run/ws_compiler_fixes.rs"]
mod ws_compiler_fixes;
pub(crate) use ws_compiler_fixes::*;

#[path = "compile_and_run/enums.rs"]
mod enums;

#[path = "compile_and_run/closures.rs"]
mod closures;

#[path = "compile_and_run/cli_backend.rs"]
mod cli_backend;

#[path = "compile_and_run/atomics.rs"]
mod atomics;

#[path = "compile_and_run/tuples.rs"]
mod tuples;

#[path = "compile_and_run/tuples_more.rs"]
mod tuples_more;

#[path = "compile_and_run/digit_separators.rs"]
mod digit_separators;
pub(crate) use tuples_more::*;

#[path = "compile_and_run/asm_freestanding.rs"]
mod asm_freestanding;

#[path = "compile_and_run/irq_sv39_defer.rs"]
mod irq_sv39_defer;

#[path = "compile_and_run/alloc_hooks.rs"]
mod alloc_hooks;

#[path = "compile_and_run/interrupt.rs"]
mod interrupt;
pub(crate) use alloc_hooks::*;

#[path = "compile_and_run/arch_stack.rs"]
mod arch_stack;

#[path = "compile_and_run/incremental.rs"]
mod incremental;

#[path = "compile_and_run/http_server.rs"]
mod http_server;

#[path = "compile_and_run/http_low.rs"]
mod http_low;
pub(crate) use http_low::*;

#[path = "compile_and_run/http_polyp.rs"]
mod http_polyp;
pub(crate) use http_polyp::*;

#[path = "compile_and_run/http_helpers.rs"]
mod http_helpers;
pub(crate) use http_helpers::*;

#[path = "compile_and_run/eval_order.rs"]
mod eval_order;

// Gitea #223: `()` and `Result<(), E>` are spelled as the issue asks. The
// success arm is built from `()`, matched, passed through `?`, and the `Err`
// arm still carries its reason.
#[test]
fn result_of_unit_issue_223_spellings_build_match_and_propagate() {
    assert_compiles_and_runs(
        "tests/fixtures/result_unit_issue_223.kp",
        "ok\nfailed\nok\nfailed\nunit ok\nunit failed\n",
    );
}

#[test]
fn unit_spelled_as_empty_parens_is_still_refused_as_a_tuple_element() {
    assert_rejected_with("tests/fixtures/tuple_unit_type_element.kp", "is 'unit'");
}

// Gitea #280: an expression body with no written return type infers it. A
// recursive body keeps its explicit type, and a `when` body or a borrow body
// is refused without one.
#[test]
fn expression_body_infers_its_return_type_issue_280() {
    assert_compiles_and_runs("tests/fixtures/expr_body_infers_return.kp", "5\n16\n");
}

#[test]
fn expression_body_calls_another_inferred_body_issue_280() {
    assert_compiles_and_runs("tests/fixtures/expr_body_infers_through_callees.kp", "6\n");
}

#[test]
fn recursive_expression_body_without_a_type_is_refused_issue_280() {
    assert_rejected_naming("tests/fixtures/expr_body_recursive_needs_type.kp", &["recursive", "fact", "=> T"], &[]);
}

#[test]
fn recursive_expression_body_with_a_type_builds_issue_280() {
    assert_compiles_and_runs("tests/fixtures/expr_body_recursive_with_type.kp", "120\n");
}

#[test]
fn mutually_recursive_expression_bodies_without_a_type_are_refused_issue_280() {
    assert_rejected_naming("tests/fixtures/expr_body_mutual_recursion_needs_type.kp", &["recursive", "=> T"], &[]);
}

#[test]
fn when_expression_body_without_a_type_is_refused_issue_280() {
    assert_rejected_naming("tests/fixtures/expr_body_when_needs_type.kp", &["several arms", "=> T"], &[]);
}

#[test]
fn borrowing_expression_body_without_a_type_is_refused_issue_280() {
    assert_rejected_naming("tests/fixtures/expr_body_borrow_needs_type.kp", &["returns a borrow", "=> T"], &[]);
}

#[test]
fn expression_method_body_infers_and_calls_an_inferred_method_issue_280() {
    assert_compiles_and_runs("tests/fixtures/expr_body_method_infers.kp", "24\n");
}

#[test]
fn recursive_expression_method_without_a_type_is_refused_issue_280() {
    assert_rejected_naming("tests/fixtures/expr_body_method_recursive_needs_type.kp", &["recursive", "shrink", "=> T"], &[]);
}

#[test]
fn public_expression_body_cannot_infer_a_private_type_issue_280() {
    assert_rejected_naming("tests/fixtures/expr_body_pub_private_type.kp", &["Hidden", "=> T"], &[]);
}
