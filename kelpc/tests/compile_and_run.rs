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

/// The backend this run of the suite is validating.
///
/// The suite is backend-agnostic by construction: it compiles a fixture, runs
/// the result, and compares stdout, so it never names a code generator. Which
/// one runs is therefore a property of the *invocation*, not of the test, and
/// the project's oracle switch (`KELPC_ORACLE`) is the only input that varies
/// it (the MK7 record in `plan/done.md`). The switch names only the C backend, and
/// unset means the compiler's own default (`Backend::DEFAULT`), which since MK7
/// S7 is `llvm`, so an unqualified run exercises what an unqualified
/// `kelpc build` does and `KELPC_ORACLE=c` points the whole suite at the oracle
/// instead, which is how the C backend stays covered. `the_default_backend_is_llvm`
/// below pins the compiler's side of that agreement by reading which
/// intermediate a build with no backend setting writes.
fn backend() -> String {
    match std::env::var("KELPC_ORACLE").as_deref() {
        Ok("c") => "c".to_string(),
        _ => "llvm".to_string(),
    }
}

/// A `kelpc` invocation pinned to the backend under test.
///
/// The oracle switch is passed explicitly rather than inherited so that the
/// suite's child processes cannot disagree with `backend()` about which backend
/// is being exercised; an inherited value that differs from this one would make
/// a failure impossible to attribute. The retired `KELPC_BACKEND` is stripped
/// for the same reason: a developer's stale export must not turn every build in
/// the run into the refusal that variable now earns.
fn kelpc() -> Command {
    let mut cmd = Command::new(kelpc_bin());
    cmd.env_remove("KELPC_BACKEND");
    if backend() == "c" {
        cmd.env("KELPC_ORACLE", "c");
    } else {
        cmd.env_remove("KELPC_ORACLE");
    }
    cmd
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(name)
}

fn out_path(name: &str) -> PathBuf {
    let mut p = PathBuf::from(std::env::var("CARGO_TARGET_TMPDIR").unwrap_or_else(|_| {
        std::env::temp_dir().to_string_lossy().to_string()
    }));
    // The backend is part of the path because a second backend produces a
    // different binary from the same fixture. Without it, two backends running
    // concurrently (or one leaving an artifact behind) would race for the same
    // file, and the loser's binary would be executed in the winner's place.
    //
    // The name of the test is part of it for the same reason inside one
    // backend: two tests may build one fixture, and a second test's `exec` of a
    // file the first is still writing fails with `Text file busy`
    // (BUG-r4Nc8). libtest names each test's thread after the test, so the
    // thread name is the test name.
    let test = std::thread::current().name().unwrap_or("main").to_string();
    p.push(format!("kelpc_test_{}_{test}_{name}", backend()));
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

/// Asserts that `kelpc build` rejects `source`, and that the diagnostic
/// mentions `needle` — so a fixture cannot pass for the wrong reason.
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
    assert_compiles_and_runs("../examples/hello.kp", "hello, world\n");
}

#[test]
fn fib_computes_correctly() {
    assert_compiles_and_runs("../examples/fib.kp", "55\n");
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

/// Source mapping is the one property of the C backend that only the *generated
/// file* can answer: a `#line` marker is what the C compiler reads, so no
/// program's stdout asserts it. Everything else in this file is asserted by
/// running a program; this test reads `build/<name>.c` on purpose (roadmap open
/// item 20, and A-16's source-mapping half).
///
/// A marker **per statement** is the point: it names the `.kp` file and the line
/// the statement is written on, and a multi-file build names each module's own
/// file rather than the entry module's.
#[test]
fn the_generated_c_maps_statements_back_to_their_kp_lines() {
    // `KELPC_ORACLE=c` explicitly: the file this reads is one only that
    // backend writes, so the test must not follow the suite's default.
    let source = fixture("tests/fixtures/when_expr.kp");
    let bin = out_path("line_markers_single");
    let build = kelpc()
        .env("KELPC_ORACLE", "c")
        .args(["build"])
        .arg(&source)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected 'when_expr.kp' to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let generated = std::fs::read_to_string(bin.with_extension("c"))
        .expect("the generated C sits beside the binary");
    // Lines 8 and 10 are `print(classify(5))` and `print(classify(0))`; the
    // marker has to name the statement's own line, which is the difference
    // between "bisect the function" and "read the expression".
    for line in [8, 10] {
        let marker = format!("#line {line} \"{}\"", source.display());
        assert!(
            generated.contains(&marker),
            "the generated C has no '{marker}' directive:\n{generated}"
        );
    }

    // A multi-file build: `shapes` is `main.kp`, `geom.kp` and
    // `shapes/circle.kp`, and a statement in each has to be mapped to the file
    // it was written in.
    let root = project("shapes").join("src");
    let bin = out_path("line_markers_project");
    let build = kelpc()
        .env("KELPC_ORACLE", "c")
        .args(["build"])
        .arg(root.join("main.kp"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected the 'shapes' project to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let generated = std::fs::read_to_string(bin.with_extension("c"))
        .expect("the generated C sits beside the binary");
    for (module, line) in [("main.kp", 5usize), ("geom.kp", 1), ("shapes/circle.kp", 5)] {
        let marker = format!("#line {line} \"{}\"", root.join(module).display());
        assert!(
            generated.contains(&marker),
            "no '{marker}' directive: the imported module's statements are not \
             mapped to '{module}'"
        );
    }
}

#[test]
fn named_and_positional_args_match_correctly() {
    assert_compiles_and_runs("tests/fixtures/named_args.kp", "7\n7\n7\n");
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
/// open item 30, `plan/nll.md`). The three accepted programs mutate, move or
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
/// local can be a cursor (roadmap open item 33, `plan/provenance.md`). The
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
// M5: multi-file projects (`impl` imports, `alias.kpconf`-free form).
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

/// As `assert_project_runs`, with arguments appended to `argv` — which is what
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

#[test]
fn multi_file_project_with_manifest_runs() {
    // `main.kp` imports `shapes/circle` and `geom`; `shapes/circle.kp` itself
    // imports `geom`, exercising a nested import with its own alias table.
    assert_project_runs(
        "shapes",
        &project("shapes"),
        "12.566360\n6.000000\n50.265440\n28.274310\n",
    );
}

#[test]
fn manifest_less_sibling_import_runs() {
    // No `coral.toml`: the entry file's own directory is the module root.
    assert_project_runs(
        "loose",
        &project("loose").join("main.kp"),
        "42\n",
    );
}

#[test]
fn import_namespace_reference_runs() {
    // `impl "util"` plus a qualified type in a signature (`util.Tally`): the
    // namespace is the module's own name, which is the spelling the `as` clause
    // used to override.
    assert_project_runs("namespace", &project("namespace"), "7\n4\n");
}

#[test]
fn same_name_in_two_modules_does_not_collide() {
    // Strict module-qualified names are what make this legal: `left.Item`
    // and `right.Item` are distinct structs, and `left.describe` /
    // `right.describe` are distinct functions.
    assert_project_runs(
        "same_name",
        &project("same_name").join("main.kp"),
        "6\n9\n4\n13\n",
    );
}

#[test]
fn import_cycle_is_rejected() {
    assert_project_rejected(
        "cycle",
        &project("cycle").join("a.kp"),
        "import cycle: a -> b -> a",
    );
}

#[test]
fn missing_module_is_rejected() {
    assert_project_rejected(
        "unknown_module",
        &project("unknown_module").join("main.kp"),
        "cannot find module 'nope'",
    );
}

#[test]
fn duplicate_import_alias_is_rejected() {
    assert_project_rejected(
        "duplicate_alias",
        &project("duplicate_alias").join("main.kp"),
        "alias 'circle' is already used by another import",
    );
}

#[test]
fn parent_directory_import_is_rejected() {
    assert_project_rejected(
        "dotdot",
        &project("dotdot").join("main.kp"),
        "must not contain '.' or '..' segments",
    );
}

#[test]
fn non_identifier_module_path_is_rejected() {
    assert_project_rejected(
        "bad_path",
        &project("bad_path").join("main.kp"),
        "is not a valid module name",
    );
}

#[test]
fn unknown_module_member_is_rejected() {
    assert_project_rejected(
        "unknown_member",
        &project("unknown_member").join("main.kp"),
        "module 'prov' has no item 'Nope'",
    );
}

#[test]
fn unqualified_imported_item_is_rejected() {
    // Strict module-qualified namespace: a bare `Thing(...)` never resolves
    // to another module's `Thing`, and the error suggests the qualified form.
    assert_project_rejected(
        "unqualified_import",
        &project("unqualified_import").join("main.kp"),
        "did you mean 'prov.Thing'?",
    );
}

#[test]
fn an_exported_item_is_reachable_from_another_module() {
    // A-13: `pub` on a `const`, a `datac` and a `fun` is what makes each one
    // reachable through the module alias. The receiver method carries no
    // `pub`: it follows its declaring type, and `p.sum()` reaches it through
    // the value.
    assert_project_runs(
        "visibility_import",
        &project("visibility_import").join("main.kp"),
        "10\n42\n",
    );
}

#[test]
fn a_private_item_is_not_reachable_from_another_module() {
    // Private by default: `hidden` exists, so the refusal is about its
    // visibility, not about a missing item (`unknown_member` pins that text).
    assert_project_rejected(
        "visibility_private",
        &project("visibility_private").join("main.kp"),
        "'lib.hidden' is private",
    );
}

#[test]
fn a_private_item_is_not_offered_by_the_did_you_mean_hint() {
    let target = project("visibility_private_hint").join("main.kp");
    let build = kelpc()
        .arg("build")
        .arg(&target)
        .arg("-o")
        .arg(out_path("visibility_private_hint"))
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        !build.status.success(),
        "expected project 'visibility_private_hint' to be rejected, but it built"
    );
    let stderr = String::from_utf8_lossy(&build.stderr);
    assert!(
        stderr.contains("call to undefined function 'Thing'"),
        "expected the unqualified name to be undefined, got:\n{stderr}"
    );
    assert!(
        !stderr.contains("did you mean"),
        "the hint must not offer a private item, got:\n{stderr}"
    );
}

#[test]
fn a_private_type_is_not_nameable_in_a_type_position() {
    // The name resolver returned early for a canonical key before running the
    // export check, so a `datac` could cross a module boundary in a type
    // position (`describe`'s parameter) with no refusal at all.
    assert_project_rejected(
        "visibility_type_private",
        &project("visibility_type_private").join("main.kp"),
        "'lib.Hidden' is private",
    );
}

#[test]
fn an_exported_type_is_nameable_in_every_type_position() {
    // The acceptance half of the same gate: `pub` types in a parameter type, a
    // return type, a field type, a local annotation, a generic argument, and a
    // `List`, `Box` and array element. It also pins the bare names that must
    // keep resolving: `local` is the entry module's own private function and
    // `add` is `lib`'s, and both are called bare from inside their module.
    assert_project_runs(
        "visibility_type_pub",
        &project("visibility_type_pub").join("main.kp"),
        "1\n5\n8\n21\n3\n1\n42\n42\n",
    );
}

#[test]
fn the_entry_modules_private_items_are_not_reachable_by_their_bare_name() {
    // The entry module is the only one whose keys carry no prefix, so a lookup
    // on the name as written reached its private `helper` from any module. A
    // bare name is the calling module's own, so the call is undefined here, not
    // private (`visibility_private` pins the private text).
    assert_project_rejected(
        "visibility_bare_call",
        &project("visibility_bare_call").join("main.kp"),
        "call to undefined function 'helper'",
    );
}

#[test]
fn a_project_module_under_a_std_path_is_not_a_shipped_module() {
    // The module id `std/main` looks shipped and is not: the file is a project
    // file, so its items are private by default like any other module's. The
    // entry module itself still names `Hidden`; another module names it in a
    // type position, and a bare name there is that module's own, so the
    // reference is undefined rather than private. The shipped-module predicate
    // that decides this is pinned directly by the unit test beside it in
    // `infer.rs`.
    assert_project_rejected(
        "visibility_std_path",
        &project("visibility_std_path").join("src/std/main.kp"),
        "unknown type 'Hidden'",
    );
}

#[test]
fn a_public_function_may_not_name_a_private_parameter_type() {
    // A-13 private-in-public: an item another module can reach must not name,
    // in its signature, a type its own module does not export. The private
    // type is in the *same* module here, so nothing in the cross-module name
    // gate ever sees the reference; the signature is what leaks it.
    assert_rejected_with(
        "tests/fixtures/visibility_pub_fun_param.kp",
        "'describe' is exported but its signature names 'Hidden'",
    );
}

#[test]
fn a_public_function_may_not_name_a_private_return_type() {
    // The return half of the same signature (A-13).
    assert_rejected_with(
        "tests/fixtures/visibility_pub_fun_return.kp",
        "'make' is exported but its signature names 'Secret'",
    );
}

#[test]
fn a_public_datac_may_not_name_a_private_field_type() {
    // A field is reachable through a value of the exported type (A-13).
    assert_rejected_with(
        "tests/fixtures/visibility_pub_datac_field.kp",
        "'Boxed' is exported but its signature names 'Hidden'",
    );
}

#[test]
fn a_method_on_an_exported_datac_may_not_name_a_private_type() {
    // A method is reached through a value of its declaring type, so an
    // exported `datac` makes the method's signature reachable too (A-13).
    assert_rejected_with(
        "tests/fixtures/visibility_pub_method_type.kp",
        "'Boxed.stash' is exported but its signature names 'Hidden'",
    );
}

#[test]
fn a_public_enum_may_not_name_a_private_payload_type() {
    // A payload is matched out of the exported variant (A-13).
    assert_rejected_with(
        "tests/fixtures/visibility_pub_enum_payload.kp",
        "'Shape' is exported but its signature names 'Hidden'",
    );
}

#[test]
fn a_public_const_may_not_name_a_private_type() {
    // `pub const LIMIT: lib.Hidden` names a type module 'lib' does not export
    // (A-13). The rule is checked before a const's initializer is folded, so a
    // const whose type is the one that leaks is refused for that reason first.
    assert_project_rejected(
        "visibility_const_type",
        &project("visibility_const_type").join("main.kp"),
        "'LIMIT' is exported but its signature names 'lib.Hidden'",
    );
}

#[test]
fn a_public_extern_function_may_not_name_a_private_type() {
    // An `extern fun` has no body, so its parameters and return type are its
    // whole signature (A-13).
    assert_rejected_with(
        "tests/fixtures/visibility_pub_extern_type.kp",
        "'poke' is exported but its signature names 'Hidden'",
    );
}

#[test]
fn a_public_function_may_not_name_a_private_type_inside_a_list() {
    // A container names the types inside it, so the rule recurses (A-13).
    assert_rejected_with(
        "tests/fixtures/visibility_pub_list_type.kp",
        "'lookup' is exported but its signature names 'Hidden'",
    );
}

#[test]
fn a_public_function_may_not_name_a_private_type_inside_a_map() {
    // Both of a `Map`'s arguments are named types (A-13).
    assert_rejected_with(
        "tests/fixtures/visibility_pub_map_type.kp",
        "'tally' is exported but its signature names 'Hidden'",
    );
}

#[test]
fn a_public_function_may_not_name_a_private_type_inside_an_array() {
    // An array is its element and its length, and an array of arrays recurses
    // through the element (A-13).
    assert_rejected_with(
        "tests/fixtures/visibility_pub_array_type.kp",
        "'grid' is exported but its signature names 'Hidden'",
    );
}

#[test]
fn a_public_function_may_not_name_a_private_borrowed_type() {
    // A borrow names its pointee (A-13).
    assert_rejected_with(
        "tests/fixtures/visibility_pub_borrow.kp",
        "'peek' is exported but its signature names 'Hidden'",
    );
}

#[test]
fn a_public_function_may_not_return_a_private_type_inside_a_box() {
    // `Box` names its element type, so the walk reaches it like any other
    // container's argument (A-13).
    assert_rejected_with(
        "tests/fixtures/visibility_pub_box_type.kp",
        "'stash' is exported but its signature names 'Hidden'",
    );
}

#[test]
fn a_public_datac_may_not_name_a_private_type_inside_a_function_type() {
    // A function type names its parameters and its return, so the walk
    // recurses through both (A-13).
    assert_rejected_with(
        "tests/fixtures/visibility_pub_fn_type.kp",
        "'Callbacks' is exported but its signature names 'Hidden'",
    );
}

#[test]
fn an_exported_signature_may_not_name_a_private_type_declared_after_it() {
    // The check runs over the merged program with the whole export set in
    // hand, so the order of two declarations in one file does not decide the
    // answer (A-13, `plan/syntax.md` 13.4).
    assert_rejected_with(
        "tests/fixtures/visibility_pub_late_type.kp",
        "'describe' is exported but its signature names 'Hidden'",
    );
}

#[test]
fn an_exported_signature_may_name_every_type_its_module_exports() {
    // The acceptance half: a `pub` item naming a `pub` type in its own module,
    // another module's `pub` type through its alias, a generic instantiation
    // of an exported type, a compiler-shipped `std/...` type (which the
    // transitional rule exports wholesale), a generic's own parameter, and a
    // private item naming a private type, all compile and run.
    assert_project_runs(
        "visibility_pub_signature",
        &project("visibility_pub_signature").join("main.kp"),
        "0\n2\n2\n5\n4\n7\n",
    );
}

#[test]
fn unshipped_stdlib_module_is_rejected() {
    // `std/tls` shipped in MK5-S4b-M1 (open item 37) and is no longer the
    // example here; `std/zzz_unshipped` is a placeholder name reserved by
    // this fixture alone (never a real module), so this test needs no
    // updating the next time a real one ships. The error names what *is*
    // shipped so the reservation stays useful.
    assert_project_rejected(
        "stdlib_import",
        &project("stdlib_import").join("main.kp"),
        "stdlib module 'zzz_unshipped' is not implemented yet (shipped today: std/mem",
    );
}

#[test]
fn the_compiler_internal_std_namespace_is_unavailable_to_projects() {
    assert_project_rejected(
        "std_internal_import",
        &project("std_internal_import").join("main.kp"),
        "compiler-reserved std/internal namespace",
    );
    assert_project_rejected(
        "std_internal_entry",
        &project("std_internal_entry"),
        "below the compiler-reserved std/internal namespace",
    );
}

#[test]
fn a_local_that_shadows_a_module_names_the_shadowing() {
    // The local wins, so the call is not the module's. The message says that
    // much, and names the module only when that module really has the method
    // the call was written for.
    assert_rejected_with(
        "tests/fixtures/shadowed_module_alias.kp",
        "'path' is a local binding here, so 'path.join' is not the module 'path'",
    );
}

#[test]
fn a_grant_pattern_that_is_not_a_path_is_rejected() {
    assert_project_rejected(
        "caps_bad_path_pattern",
        &project("caps_bad_path_pattern").join("main.kp"),
        "capability patterns are absolute paths",
    );
}

// ---------------------------------------------------------------------------
// M5.2: passdowns (`pd`) and string templates.
// ---------------------------------------------------------------------------

#[test]
fn passdowns_are_read_by_referencing_functions_only() {
    // `banner`/`label` reference passdowns (and become wired); `unrelated`
    // references none; `main` reads the generated globals directly.
    assert_compiles_and_runs(
        "tests/fixtures/passdowns.kp",
        "shapes\nshapes\n[shapes] hits = 0\n42\n",
    );
}

#[test]
fn a_datac_passdown_is_read_through_a_shared_borrow() {
    assert_compiles_and_runs("tests/fixtures/passdown_datac.kp", "(3, 4) sum=7\n7\n");
}

#[test]
fn a_local_can_shadow_a_passdown() {
    assert_compiles_and_runs("tests/fixtures/pd_shadowed.kp", "local=inner\npd=outer\n");
}

#[test]
fn a_passdown_is_forwarded_through_a_caller_that_does_not_name_it() {
    // The call site's hidden argument is a borrow of the storage in scope:
    // the caller's own hidden parameter when it has one (so the forwarded
    // value stays a pointer, not a pointer to a pointer), the global's
    // address when it does not — `main`, or a function that only calls a
    // function reading the passdown.
    assert_compiles_and_runs(
        "tests/fixtures/pd_argument_forwarding.kp",
        "ok\nforwarded\nbc\nforwarded\nbc\ndirect\nFAIL\n2\n2\n",
    );
}

#[test]
fn a_pd_var_is_writable_from_a_function_and_from_main() {
    // M6.4 replaced M5.2's "read-only inside a function": a `pd var` is
    // reached through an exclusive borrow, so writes work and stay checked.
    assert_compiles_and_runs(
        "tests/fixtures/pd_var_write.kp",
        "count = 5\ncount = 100\n",
    );
}

#[test]
fn a_pd_var_no_function_writes_is_a_shared_borrow() {
    // Roadmap item 17: the borrow a function holds is computed from whether
    // it writes the passdown, so a read-only `pd var` is shared and can be
    // both an argument and a hidden parameter of the same call.
    assert_compiles_and_runs(
        "tests/fixtures/pd_var_read_only_shared.kp",
        "shapes\nshapes\nshapes\n",
    );
    assert_compiles_and_runs(
        "tests/fixtures/pd_var_read_only_datac.kp",
        "api\n3\napi\n",
    );
}

#[test]
fn a_pd_var_write_reaches_the_callers_that_hold_it() {
    // The upgrade follows the call graph, and the edge is followed even when
    // the callee does not name the passdown (a pure forwarder takes the
    // global's address itself) — a shared borrow held across that write is
    // the hazard the mode exists to prevent.
    assert_compiles_and_runs(
        "tests/fixtures/pd_write_reaches_callers.kp",
        "0\n2\n2\n",
    );
    // The other trigger a callee's own references cannot show: the passdown is
    // handed to a `mut` parameter, so the caller writes it without `add` ever
    // naming a passdown.
    assert_compiles_and_runs(
        "tests/fixtures/pd_var_mut_argument.kp",
        "2\n2\n",
    );
}

#[test]
fn a_pd_var_of_a_two_word_datac_is_writable_through_its_borrow() {
    // The write is `*p = v` on the passdown's hidden borrow, and the operand is
    // a use: missing it puts the write in another parameter's storage instead
    // of refusing it.
    assert_compiles_and_runs(
        "tests/fixtures/pd_var_datac_reassign.kp",
        "1,2\n3,6\n",
    );
}

#[test]
fn a_scalar_local_can_be_borrowed_and_written_through_the_pointer() {
    // MK7 §5: the caller's half of a `mut` out-parameter — `&mut x` on a scalar
    // local — plus `*p` and `*p = v` through the pointer it produces, a pointer
    // binding held across a call, and two live locals around the store (the
    // live-range probe: a missed touch hands the pointer's register to `keep`
    // and the store writes through it).
    assert_compiles_and_runs(
        "tests/fixtures/scalar_borrow.kp",
        "13\n30\n30\n12\n20\n",
    );
}

#[test]
fn a_float_scalar_can_be_borrowed_and_written_through_the_pointer() {
    // The same borrow with an `f64` pointee: only the load and the store change
    // register file, so a wrong mnemonic prints a plausible wrong number rather
    // than failing to build.
    assert_compiles_and_runs(
        "tests/fixtures/scalar_borrow_f64.kp",
        "3.000000\n3.250000\n3.250000\n",
    );
}

#[test]
fn passdown_storage_is_a_global_that_main_initializes() {
    // What a backend without the C backend's hidden-argument plumbing does
    // instead: one global per `pd`, assigned by a sequence at the top of
    // `main`, and a hidden parameter whose value is that object's address. An
    // initializer that reads an earlier passdown pins the initialization order,
    // and `bump` pins the case where the caller has no hidden parameter and so
    // passes the global's own address.
    assert_compiles_and_runs(
        "tests/fixtures/passdown_globals.kp",
        "10 20 0\n1 2 3\n10 20 2\n1 2 3\n10 20 12\n1 2 3\n3\n",
    );
}

#[test]
fn writing_a_pd_val_inside_a_function_is_rejected() {
    assert_rejected("tests/fixtures/pd_val_write.kp");
}

#[test]
fn a_pd_var_cannot_be_borrowed_twice_in_one_call() {
    // The documented cost of M6.4's design, pinned so it reads as a rule.
    let bin = out_path("pd_var_double_borrow");
    let build = kelpc()
        .arg("build")
        .arg(fixture("tests/fixtures/pd_var_double_borrow.kp"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        !build.status.success(),
        "expected a `pd var` borrowed exclusively and shared in one call to be rejected"
    );
    let stderr = String::from_utf8_lossy(&build.stderr);
    assert!(
        stderr.contains("already borrowed exclusively"),
        "expected a borrow-conflict message, got:\n{stderr}"
    );
}

#[test]
fn reading_a_copy_passdown_twice_in_one_call_is_allowed() {
    assert_compiles_and_runs(
        "tests/fixtures/pd_var_copy_read_twice.kp",
        "2\n20\n",
    );
}

#[test]
fn a_method_receiver_is_borrowed_after_its_own_arguments() {
    // MISSING-n4Bz7: `counts.insert(k, counts.get_or(..) + 1)` and its List and
    // String-keyed spellings. Each was "already borrowed exclusively" before,
    // because the receiver's exclusive borrow started before the arguments were
    // checked.
    assert_compiles_and_runs(
        "tests/fixtures/borrow_two_phase.kp",
        "2\n2\n42\n3\n",
    );
}

#[test]
fn two_phase_borrows_still_reject_aliasing() {
    // The other half: a shared borrow of the receiver that outlives the
    // arguments is still a conflict, and so is a second exclusive borrow.
    assert_rejected_with(
        "tests/fixtures/borrow_two_phase_alias.kp",
        "already borrowed as shared",
    );
    assert_rejected_with(
        "tests/fixtures/borrow_two_phase_nested.kp",
        "already borrowed exclusively",
    );
}

#[test]
fn reading_a_passdown_before_it_is_declared_is_rejected() {
    assert_rejected("tests/fixtures/pd_forward_read.kp");
}

#[test]
fn templates_print_literals_and_values() {
    assert_compiles_and_runs(
        "tests/fixtures/templates.kp",
        "100% of {nothing} is 50%\narea = 12.566360\nr=2.000000 doubled=4.000000\nno newline here!\n",
    );
}

#[test]
fn a_template_outside_print_is_rejected() {
    // The message may not claim a missing owned `String`: `String` shipped in
    // MK5-S1, and `syntax.md` 2.5 names the ways to build one.
    assert_rejected_with(
        "tests/fixtures/template_not_printed.kp",
        "a template is not a value",
    );
}

#[test]
fn a_passdown_is_not_visible_from_another_module() {
    assert_project_rejected(
        "pd_module_scope",
        &project("pd_module_scope").join("main.kp"),
        "is a passdown of module 'util'",
    );
}

// ---------------------------------------------------------------------------
// M5.3: `%*alias*%` text substitution (`alias.kpconf` + inline `alias`).
// ---------------------------------------------------------------------------

#[test]
fn aliases_substitute_text_in_expressions_and_strings() {
    // Also pins the pure-text semantics: `%*doubled*%` is `40 + 2 * 2`.
    assert_compiles_and_runs(
        "tests/fixtures/aliases.kp",
        "42\n44\nsay aliasword\n",
    );
}

#[test]
fn an_alias_kpconf_entry_can_be_an_impl_path() {
    assert_project_runs("alias_kpconf", &project("alias_kpconf"), "42\n");
}

#[test]
fn an_alias_from_an_imported_module_is_visible() {
    // The table is flat and global: `%*greeting*%` is declared in `util.kp`
    // and used in `main.kp`.
    assert_project_runs(
        "alias_cross_file",
        &project("alias_cross_file"),
        "hello from an imported alias\n",
    );
}

#[test]
fn an_alias_cycle_is_rejected_even_when_unused() {
    assert_rejected("tests/fixtures/alias_cycle.kp");
}

#[test]
fn an_unknown_alias_is_rejected() {
    assert_rejected("tests/fixtures/alias_unknown.kp");
}

#[test]
fn an_alias_cycle_across_two_files_names_both_sites() {
    // `a` is defined in `alias.kpconf`, `b` in `main.kp`; the diagnostic points
    // at the reference that closed the cycle and notes the definition, so
    // both files appear.
    let bin = out_path("alias_kpconf_cycle");
    let build = kelpc()
        .arg("build")
        .arg(project("alias_kpconf_cycle"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(!build.status.success(), "expected a rejection");
    let stderr = String::from_utf8_lossy(&build.stderr);
    for needle in ["alias cycle: a -> b -> a", "alias.kpconf:", "main.kp:"] {
        assert!(
            stderr.contains(needle),
            "expected the error to mention '{needle}', got:\n{stderr}"
        );
    }
}

// ---------------------------------------------------------------------------
// M6.1: the builtin owned heap byte buffer (`Bytes`).
// ---------------------------------------------------------------------------

#[test]
fn bytes_construct_measure_index_mutate_clone_and_print() {
    assert_compiles_and_runs(
        "tests/fixtures/heap_bytes.kp",
        "5\n104\nHello\nHello\nfalse\n0\n",
    );
}

#[test]
fn a_bytes_value_moves() {
    // `Bytes` is non-Copy, so binding it to a second name moves it and the
    // first name is dead (M3's rule, now with a type that makes it matter).
    assert_rejected("tests/fixtures/heap_use_after_move.kp");
}

#[test]
fn a_moved_from_binding_can_be_reassigned() {
    assert_compiles_and_runs("tests/fixtures/heap_move_then_rebind.kp", "2\n1\n");
}

#[test]
fn an_owned_value_can_be_returned_by_a_function() {
    // The callee must not free it; the caller must.
    assert_compiles_and_runs("tests/fixtures/heap_returned.kp", "5\nhello\n");
}

#[test]
fn an_owned_local_dies_at_the_end_of_its_block() {
    assert_compiles_and_runs("tests/fixtures/heap_scope_drop.kp", "first\nafter\n");
}

#[test]
fn drops_follow_control_flow() {
    // A loop body, a `return` from inside a branch with another owned local
    // still live, and a block used as a value.
    assert_compiles_and_runs(
        "tests/fixtures/heap_drops_in_control_flow.kp",
        "loop\nloop\nextra\nkeep\n5\n",
    );
}

#[test]
fn an_owned_local_in_a_desugared_block_is_dropped_once_and_in_its_own_block() {
    // BUG-1: `if let` desugars to a `when` whose synthesized `else` block
    // carried the `then` block's span, and `for` builds an element block
    // inside a `while` body that carries the loop body's span too. A drop
    // plan keyed by span then emitted the arm's `xs` on the `else` path (where
    // the name is not declared, so `cc` refused the program) and `inner`
    // twice. The fixture declares an owned local in each position.
    assert_compiles_and_runs(
        "tests/fixtures/heap_drops_in_desugared_blocks.kp",
        "1\n1\n",
    );
}

#[test]
fn reassigning_an_owned_binding_frees_the_old_value() {
    assert_compiles_and_runs("tests/fixtures/heap_reassign.kp", "three\nagain\n");
}

#[test]
fn ownership_through_branches_frees_exactly_once() {
    assert_compiles_and_runs(
        "tests/fixtures/heap_branches.kp",
        "second\nelse\nfirst\n",
    );
}

#[test]
fn a_move_one_path_takes_is_freed_on_the_path_that_did_not() {
    // The drop-plan defect `memory_gate.rs` found in `std_str_order`: a move
    // out of an owned local that only one path through a branch takes used to
    // mark the binding moved for *both* paths, so the branch that kept the
    // value leaked it. The fixture also covers the assignment after the join,
    // which frees the old value — the path that moved it cleared the slot, so
    // that free finds nothing there.
    assert_compiles_and_runs(
        "tests/fixtures/owned_conditional_move.kp",
        "live\nfresh\nfresh\n",
    );
}

#[test]
fn an_owned_local_bound_in_a_loop_body_is_dropped_on_both_paths() {
    // The defect's own minimal repro (MK7 §5): the owned local is bound inside
    // the loop body, moved on one path through it and abandoned on the other,
    // so the body's end-of-block drop has to run on the path that left the value
    // behind — and the two `while`-pass plan write must not lose it.
    assert_compiles_and_runs("tests/fixtures/owned_move_in_loop.kp", "hi\n");
}

#[test]
fn an_assignment_through_a_dereferenced_borrow_frees_the_old_value() {
    // roadmap open item 15, case 2: `*p = value` (and `p.field = value`
    // through a borrowed base) used to leak the value it overwrote, because
    // `DropPlan::at_assign` only ever recorded a plain binding, and neither
    // target has one — there is no declaration to key a `DropTarget` to,
    // only a place. Both backends now free the old value at the target's own
    // address instead of resolving a binding.
    assert_compiles_and_runs("tests/fixtures/assign_through_deref.kp", "second\nbeta\n");
}

#[test]
fn assigning_an_owned_field_frees_the_field_s_old_value() {
    // roadmap open item 15, case 3: `h.name = s` used to leak the field's old
    // buffer for the same reason case 2 did. Also covers `h.name = h.name`
    // and a field reassigned after being partially moved out of: both leave
    // the field's slot already zeroed by the time this drop runs, so the
    // fixture is what pins that freeing zero stays a safe no-op rather than
    // a double free.
    assert_compiles_and_runs(
        "tests/fixtures/assign_owned_field.kp",
        "beta\ngamma\ndelta\nepsilon\n",
    );
}

#[test]
fn a_rebound_or_shadowed_owned_local_is_dropped_for_its_own_value() {
    // BUG-n7Wq3: the owned set kept one entry per *name* per scope, so a second
    // `var a` in one block replaced the first entry and the buffer the first
    // binding owned had no drop left that reached it, while the C backend
    // resolved the one remaining drop to the newer binding. Both buffers are
    // freed now, the nested block where one spelling is two bindings of two
    // scopes stays pinned beside them, and a partial move after the rebind pins
    // that the clear names the binding that owns the value it empties.
    assert_compiles_and_runs(
        "tests/fixtures/owned_rebind.kp",
        "first\nsecond\ninner\nouter\nnew\n2\n",
    );
}

#[test]
fn a_bytes_parameter_is_a_borrow_not_a_move() {
    assert_compiles_and_runs(
        "tests/fixtures/heap_param_borrow.kp",
        "2\nhi\nXi\nfalse\n",
    );
}

#[test]
fn a_static_str_datac_field_is_allowed() {
    assert_compiles_and_runs("tests/fixtures/datac_str_field.kp", "x\n");
}

#[test]
fn printing_a_temporary_owned_buffer_works() {
    // Until M6.3 this was a diagnostic telling the author to bind a `val`
    // first; now codegen hoists the temporary and drops it after the printf,
    // and the fixture runs under AddressSanitizer to prove it is freed.
    assert_compiles_and_runs(
        "tests/fixtures/heap_temporary_print.kp",
        "x\ny\nAB\nw = CD\n",
    );
}

#[test]
fn a_buffer_in_a_print_template_takes_two_argument_slots() {
    // A `%.*s` interpolation is two variadic arguments (length, then pointer),
    // so the register numbering counts arguments per piece — every piece after
    // a buffer one sits behind both of its arguments, and the three buffer
    // shapes (an owned temporary, a borrow of a name, a `View`) each supply
    // them from somewhere different. `heap_temporary_print` above is the
    // single-temporary case this slice was credited with; this fixture is the
    // numbering case, with a scalar, a float and a second buffer behind the
    // first buffer piece.
    assert_compiles_and_runs(
        "tests/fixtures/template_buffer_pieces.kp",
        "1 owned=one scalar=7 float=1.500000\n\
         2 borrowed=named after=9\n\
         3 view=bcd tail=42\n\
         4 bytes=by len=5\n\
         5 two buffers=ABcd\n",
    );
}

// ---------------------------------------------------------------------------
// M6.2: owned `String`, `extern fun`, the first stdlib module, owned `datac`
// fields, and owned passdowns.
// ---------------------------------------------------------------------------

#[test]
fn string_constructs_measures_and_round_trips_through_bytes() {
    // `len` is bytes (h + é(2) + l + l + o = 6) and `char_count` is
    // characters (5) — the distinction the two functions exist to make.
    assert_compiles_and_runs(
        "tests/fixtures/string_basic.kp",
        "6\n5\nfalse\nhéllo\nhéllo\n6\nhéllo\ntrue\n0\n",
    );
}

// ---------------------------------------------------------------------------
// S1: growing an owned buffer in place, and failing deliberately.
// ---------------------------------------------------------------------------

#[test]
fn a_string_grows_in_place_and_survives_growth() {
    // Every append re-terminates the buffer, so printing `s` after each one
    // is the check that growth kept the `str` view valid.
    assert_compiles_and_runs(
        "tests/fixtures/string_build.kp",
        "0\nhi\nhi there\n8\nhi there!\n9\n0\ntrue\n0\ntrue\nx\n",
    );
}

#[test]
fn bytes_grow_by_push_and_append() {
    assert_compiles_and_runs(
        "tests/fixtures/bytes_build.kp",
        "0\n2\n1\n2\n5\n97\n98\n99\n7\nfalse\n0\ntrue\n1\n9\n",
    );
}

#[test]
fn growing_a_string_needs_an_exclusive_borrow_and_a_var_binding() {
    // `realloc` can move the buffer a live `str` view points into, and a
    // `val` binding is not writable — both are compile errors, which is what
    // makes growth sound here without a lifetime syntax.
    assert_rejected("tests/fixtures/grow_while_borrowed.kp");
    assert_rejected("tests/fixtures/push_to_val_string.kp");
}

#[test]
fn panic_reports_its_message_and_assert_reports_failure() {
    assert_fixture_panics("panic_msg", "panic: deliberate failure");
    assert_fixture_panics("assert_fail", "panic: assertion failed");
}

#[test]
fn a_panic_message_may_be_an_owned_string() {
    // MISSING-8: the borrowed `str` was the only spelling, so a message built
    // in an error path had to be bound to a `val` or spelled `.as_str()`
    // first. An owned temporary and an owned binding both reach `kelp_panic`
    // with the buffer's own bytes.
    assert_fixture_panics("panic_owned_message", "panic: 7");
    assert_fixture_panics("panic_bound_message", "panic: bound message");
    assert_rejected_with(
        "tests/fixtures/panic_not_a_message.kp",
        "a panic message is a 'str' or an owned 'String'",
    );
}

#[test]
fn stdlib_text_builds_numbers_replaces_and_parses() {
    // Concatenation, replacement, and both live edges of the decimal
    // conversion: `u64`'s maximum and `i64`'s minimum, round-tripped through
    // the parsers. Both have a literal spelling now that a `u64` context admits
    // the whole `u64` range (MISSING-x8Wq3); the parser path is what this
    // fixture is testing.
    assert_compiles_and_runs(
        "tests/fixtures/stdlib_build.kp",
        "foobar\na:b:c\naaa\n0\n145\n-145\n18446744073709551615\n-9223372036854775808\n9\n",
    );
}

#[test]
fn stdlib_parsing_rejects_anything_but_a_whole_number() {
    // `Result<T, ParseErr>` since MK5-S6-M1: `is_ok`/`is_err`/`unwrap` are the
    // helpers, and `Empty` is a distinguishable reason from `Invalid`.
    assert_compiles_and_runs(
        "tests/fixtures/stdlib_parse.kp",
        "true\n123\ntrue\n-9223372036854775808\ntrue\ntrue\ntrue\ntrue\n1\n2\n0\ntrue\n18446744073709551615\ntrue\ntrue\ntrue\n250.000000\ntrue\n104\n233\n3\n0\n5\n",
    );
}

#[test]
fn stdlib_fmt_formats_numbers_hex_padding_and_bytes() {
    assert_compiles_and_runs(
        "tests/fixtures/stdlib_fmt.kp",
        "0\n-1234\n4294967295\n3.142\n1.5\n0\nff\nbeef\n007\nlong\n7..\n61 62\n\n007!\n0\n999\n1,000\n1,234,567\n18,446,744,073,709,551,615\n-1,234,567\n0 B\n1023 B\n1.00 KiB\n1.50 KiB\n1.00 TiB\n",
    );
}

/// MK7 §5: the owned `String` and the borrowed `View`, both ways round.
///
/// `String` is a MEMORY-class aggregate like `Bytes`; `View` is the first heap
/// type that is *two* INTEGER eightbytes, so it travels in a register pair
/// rather than through the hidden pointer, a shape no heap fixture had reached
/// before this one. A wrong register pair is a wrong string rather than a build
/// failure, which is why the printed bytes are pinned here.
#[test]
fn string_and_view_construct_measure_slice_and_convert() {
    assert_compiles_and_runs(
        "tests/fixtures/string_view_basic.kp",
        "the quick brown fox\n19\nfalse\ntrue\nquick\nthe\nhe quick brown fox\ntrue\ntrue\nfalse\ntrue\nthe\n3\ntrue\n0\ntrue\nquick\n5\nowned text\ntext\nraw\n3\n",
    );
}

/// The bug the spec's own uncertainty list found, and the reason it mattered:
/// `String.with_capacity(0)` left `data == NULL` while claiming to be an empty
/// string, so the first thing that treated it as a C string (`as_str` through
/// `strlen`) segfaulted. This fixture crashed the C backend before the fix and
/// now passes, which is what makes it worth keeping: the hole was in a
/// constructor nothing in the corpus called with zero.
#[test]
fn a_string_with_zero_capacity_is_a_valid_empty_string() {
    assert_compiles_and_runs(
        "tests/fixtures/string_with_capacity_zero.kp",
        "0\ntrue\n0\ntrue\n\n0\n3\nabc\nabcd\n4\ntrue\n0\ntrue\n0\n",
    );
}

/// A literal that does not fit the type it lands on is refused at its own line
/// rather than narrowed to that type (BUG-t6Mz2), on every path that decides a
/// literal's type: the `i32` default when nothing determines one, a type the
/// context supplies, and the declared type a `const` folds into. The one
/// literal wider than its type that survives is the one a sign covers, and
/// `int_literal_width.kp` pins what both backends do with it.
///
/// Before the refusal, the fixture that stood here asserted the narrowing this
/// now rejects: `print(1000000000007)` printed `-727379961`, by the same rule
/// that applied to `val x: i32 = 1000000000007`, and the two backends had once
/// disagreed about it because each narrowed the literal itself.
#[test]
fn an_out_of_range_literal_is_refused_at_its_own_line() {
    assert_rejected_with(
        "tests/fixtures/int_literal_out_of_range.kp",
        "'4294967295' does not fit 'i32', the type a literal takes when nothing determines one",
    );
    assert_rejected_with(
        "tests/fixtures/int_literal_method_receiver.kp",
        "'4294967295' does not fit 'i32', the type a literal takes when nothing determines one",
    );
    assert_rejected_with(
        "tests/fixtures/int_literal_const_out_of_range.kp",
        "'4294967295' does not fit 'i32': write a type that holds it",
    );
    assert_compiles_and_runs(
        "tests/fixtures/int_literal_typed.kp",
        "4294967295\n4294967295\n-2147483648\n",
    );
    assert_compiles_and_runs(
        "tests/fixtures/int_literal_width.kp",
        "-2147483648\n-147483634\n-2147483647\n2147483648\n147483634\n147483633\n-2147483648\n255\n65535\n999993014\n",
    );
}

/// A literal may spell the whole `u64` range where the context asks for a `u64`
/// (MISSING-x8Wq3). The scanner used to stop at `i64::MAX`, so the top half of
/// `u64` could not be written down and a mask or a maximum sentinel was spelled
/// as arithmetic. The range comes from the type the literal lands on, which is
/// the rule the lexical page already states, so the language needs no suffix;
/// and a value wider than the type it lands on is still refused at its own line,
/// naming that type (BUG-t6Mz2).
#[test]
fn a_u64_literal_takes_the_range_of_its_context() {
    // The item's repro: the hex mask and the decimal spelling of the same value,
    // both a `u64` from the annotation on the binding, plus the same two
    // spellings of 2^63, the first value `i64` cannot hold.
    assert_compiles_and_runs(
        "tests/fixtures/u64_literal_range.kp",
        "18446744073709551615\n18446744073709551615\n9223372036854775808\n9223372036854775808\n9223372036854775807\n",
    );
    // The same rule on every other path that gives a literal a type: a `const`'s
    // declared type, a call argument against the parameter's type, a `fun`'s
    // return type, and the `i64` minimum spelled as its magnitude under a sign.
    assert_compiles_and_runs(
        "tests/fixtures/u64_literal_contexts.kp",
        "18446744073709551615\n18446744073709551615\n18446744073709551615\n-9223372036854775808\n",
    );
    assert_rejected_with(
        "tests/fixtures/u64_literal_beyond_max.kp",
        "'18446744073709551616' is larger than the largest integer literal, 18446744073709551615",
    );
    assert_rejected_with(
        "tests/fixtures/u64_literal_does_not_narrow.kp",
        "'18446744073709551615' does not fit 'i32': write a type that holds it",
    );
    assert_rejected_with(
        "tests/fixtures/u64_literal_needs_a_context.kp",
        "'18446744073709551615' does not fit 'i32', the type a literal takes when nothing determines one",
    );
}

#[test]
fn integer_conversions_widen_exactly_and_narrow_with_a_check() {
    assert_compiles_and_runs(
        "tests/fixtures/int_conversions.kp",
        "65\n65\n65\n65\n65\n300\n300\n251\n-7\n-7\n9\n9\n9\n4294967295\n5\n6\n",
    );
    // A narrowing that cannot represent the value panics instead of
    // truncating, which is why `-1 as u8` is a runtime failure and not 255.
    assert_fixture_panics("int_conversion_overflow", "i64.to_u8: value out of range");
}

#[test]
fn a_fallible_conversion_returns_an_option_instead_of_panicking() {
    // MISSING-RbhdW: `try_to_<T>` is the twin of `to_<T>` for every pair whose
    // plain conversion can lose a value. Both bounds of both sign directions
    // are exercised, at the boundary and one past it, plus the receiver being
    // read once (the count `noisy()` prints) and the result used through
    // `if let`, `when` and `?`.
    assert_compiles_and_runs(
        "tests/fixtures/try_to_conversion.kp",
        "255\ntrue\ntrue\n7\n-2147483648\ntrue\ntrue\ntrue\n7\ntrue\ntrue\n255\ntrue\ntrue\n7\n5\n5\n2\n255\nnone\n9\ntrue\n",
    );
    // There is no `try_to` where the plain conversion cannot fail: a widening
    // is already exact, and the diagnostic says which method to write instead.
    assert_rejected_with(
        "tests/fixtures/try_to_widening.kp",
        "a try_to_<T> twin of every conversion that can lose a value",
    );
    // The `Option` is the stdlib's own, so a program with no `std/option`
    // anywhere in its graph is told to import it rather than getting an
    // internal error about an unknown type.
    assert_rejected_with(
        "tests/fixtures/try_to_without_option.kp",
        "add `impl \"std/option\"` to the file that writes the call",
    );
}

/// The remainder of a division at every integer width. This is the fixture that
/// was missing when a narrow remainder was moved into a sixty-four-bit
/// destination: `%` on `i32`/`u8`/`u16` came out wrong at a width no fixture had
/// reached, because the only fixtures dividing a narrow value were refused for
/// other reasons first.
#[test]
fn remainder_works_at_every_integer_width() {
    assert_compiles_and_runs(
        "tests/fixtures/int_mod_narrow.kp",
        "1\n-1\n1\n-1\n6\n3\n4\n15\n2\n535\n999993014\n-999993014\n",
    );
}

#[test]
fn narrow_integer_arithmetic_wraps_at_its_own_width() {
    assert_compiles_and_runs(
        "tests/fixtures/int_narrow_width_arithmetic.kp",
        "44\n100\n156\n32\n2\n0\n64\n236\n172\n32\n50\n55\n255\n44464\n24064\n5535\n10000\n88\n244\nsmaller\n44\n32\n4\n4\n",
    );
}

#[test]
fn a_failed_syscall_reports_errno_and_strerror() {
    // `ENOENT` is 2 on Linux, and glibc and musl both spell it this way; the
    // message line is asserted separately so a stable prefix is what a
    // golden test would pin, and the errno line is the new detail.
    assert_fixture_panics_all(
        "fs_errno",
        &[
            "fs.read: cannot open file",
            "errno 2: No such file or directory",
        ],
    );
}

#[test]
fn fs_has_fallible_read_and_open() {
    // MISSING-d6Qy1: a failure is an `io.IoErr` value (errno plus text) rather
    // than a panic, and the invalid-UTF-8 case carries errno 0 because no
    // syscall failed. `try_open`'s mode is not a path argument: the `"r"`
    // below would be denied by the capability check if it were.
    for stale in [
        "/tmp/kelp_fs_try_read.txt",
        "/tmp/kelp_fs_try_read.bin",
    ] {
        let _ = std::fs::remove_file(stale);
    }
    assert_compiles_and_runs(
        "tests/fixtures/fs_fallible.kp",
        "hello\n\n2\nNo such file or directory\nhello\n\n0\nfs.try_read_text: input is not valid utf-8\ntrue\n22\n2\n",
    );
}

#[test]
fn fs_mutators_have_fallible_twins() {
    // MISSING-k7Rwq: `None` is success, a failure is an `io.IoErr`, and the run
    // goes on after each one (ENOENT 2, EEXIST 17, ENOTDIR 20).
    let _ = std::fs::remove_dir_all("/tmp/kelp_fs_try_mut");
    assert_compiles_and_runs(
        "tests/fixtures/fs_try_mutators.kp",
        "mkdir_all\n0\nwrite_text\n0\nappend\n0\nwrite\n0\nrename\n0\nsymlink\n0\nremove\n0\nwrite_text_missing_dir\n2\nwrite_missing_dir\n2\nappend_missing_dir\n2\nremove_missing\n2\nrename_missing\n2\nsymlink_exists\n17\nmkdir_all_under_file\n20\n3\n2\nremove_all\n0\nfalse\nremove_all_missing\n2\ndone\n",
    );
}

#[test]
fn a_long_constant_array_literal_is_a_global_plus_a_copy() {
    // MISSING-Nf3Rb: 300 elements is past the threshold. The binding gets its own
    // copy (the mutation of `copy` does not reach `table`), a literal with a
    // non-constant element keeps the aggregate form, and a literal passed
    // straight to a call still works.
    assert_compiles_and_runs(
        "tests/fixtures/array_literal_big.kp",
        "36344\n99\n0\n36443\n50100\n36344\n",
    );
}

#[test]
fn a_pool_closes_connections_that_idled_too_long() {
    // Open item 56: `close_idle(after_ms)` and `set_idle_timeout(ms)`.
    assert_compiles_and_runs(
        "tests/fixtures/http_pool_idle.kp",
        "warm 4\nclosed fresh 0\nkept 4\nclosed all 4\nleft 0\nfalse\nafter stale checkout 0\ndone\n",
    );
}

#[test]
fn http_builds_a_multipart_form_body() {
    // MISSING-j2Wka: the exact wire bytes, with CR shown as `\r`.
    assert_compiles_and_runs("tests/fixtures/http_multipart.kp", "multipart/form-data; boundary=xXboundaryXx\n--xXboundaryXx\\r\nContent-Disposition: form-data; name=\"content\"\\r\n\\r\nhello\\r\n--xXboundaryXx\\r\nContent-Disposition: form-data; name=\"a%22b%0D%0AX: y\"\\r\n\\r\nv\\r\n--xXboundaryXx\\r\nContent-Disposition: form-data; name=\"file\"; filename=\"n%22ote.txt\"\\r\nContent-Type: text/plain\\r\n\\r\nline1\nline2\\r\n--xXboundaryXx--\\r\n\nmultipart/form-data; boundary=xXboundaryXx\nPOST\n");
}

#[test]
fn owned_datac_fields_are_dropped_with_the_struct() {
    assert_compiles_and_runs(
        "tests/fixtures/string_field.kp",
        "first\n7\nafter scope\nsecond\n7\ndone\n",
    );
}

/// The same mechanism with all three owned heap shapes in one struct: a
/// `String`, a `Bytes`, and a nested `datac` that itself owns a `String`, with
/// the `i32` scalar deliberately *not* last so a heap field follows a non-heap
/// one. This is the shape that pins the generated per-`datac` drop on order:
/// dropping `Row` must recurse through `Label` into its `String`, and reading
/// the fields through a borrowed `Row` must not steal them.
///
/// The `View` here is a local borrow (`moved.name.view()`), not a field:
/// `is_borrowed_field_ty` (`kelpc/src/types.rs`) refuses a `View`/`&str`/`Ref`
/// datac field outright, which `view_field.kp` and
/// `a_view_cannot_be_stored_in_a_datac` pin, so a borrow that owns nothing is
/// expressible only outside the struct. It is printed and then dropped without
/// being freed, which is the one way a heap type in this fixture must *not*
/// reach a free.
#[test]
fn datac_owned_fields_recurse_and_drop_once() {
    assert_compiles_and_runs(
        "tests/fixtures/datac_owned_fields.kp",
        "alpha\n7\nraw\nnested\n7\nafter scope\nbeta\n9\nmore\ndeep\nbeta\n4\ndeep\ndone\n",
    );
}

#[test]
fn an_owned_passdown_is_freed_at_exit() {
    assert_compiles_and_runs("tests/fixtures/pd_owned.kp", "== x ==\n== x ==\n");
}

#[test]
fn an_owned_passdown_no_expression_reads_is_still_dropped() {
    // An owned `pd` is dropped by the generated `atexit` handler, so the
    // container function set its drop calls has to exist even when nothing
    // reads the value. The differential fuzzer found the `llvm` backend
    // reporting an internal error for exactly this shape.
    assert_compiles_and_runs("tests/fixtures/pd_owned_container.kp", "ok\n");
}

#[test]
fn a_compiler_shipped_stdlib_module_can_be_imported() {
    // `impl "std/mem"` — the module's Kelp body calls a compiler-supplied
    // `extern` for the allocation itself.
    assert_compiles_and_runs("tests/fixtures/stdlib_mem.kp", "3\nhi\n0\n");
}

#[test]
fn the_str_stdlib_module_runs_from_kelp_source() {
    // `std/str` is written in Kelp; its only compiler-supplied pieces are the
    // two byte reads it wraps.
    assert_compiles_and_runs(
        "tests/fixtures/stdlib_str.kp",
        "HI THERE\n8\n8\ntrue\nfalse\nhello\n98\nababab\n6\n",
    );
}

#[test]
fn the_path_stdlib_module_runs_from_kelp_source() {
    // `std/path` imports `std/str`, so this also exercises a stdlib module
    // importing another one.
    assert_compiles_and_runs(
        "tests/fixtures/stdlib_path.kp",
        "/tmp/x.txt\n/tmp/x.txt\n/abs\ntrue\nfalse\n",
    );
}

#[test]
fn the_encoding_stdlib_module_runs_from_kelp_source() {
    // MISSING-FZqe6: the alphabet-and-codec module. `std/encoding` imports
    // `std/str`, `std/option`, `std/result` and `std/fmt`, so this fixture also
    // exercises the owned payloads those modules hand back (`Result<Bytes, _>`
    // and `Result<String, _>`) without a `?` or a `Copy` payload to lean on.
    assert_compiles_and_runs(
        "tests/fixtures/stdlib_encoding.kp",
        "aGVsbG8=\naGk=\nYQ==\n\nhello\nhi\na\ntrue\ntrue\nNBSWY3DP\nNBUQ====\nhello\ntrue\ntrue\nCn8eVZg\n2g\nhello\ntrue\n4869\nHi\nHello\ntrue\ntrue\na%20b%2Fc%3Fd%3De%26f\nplain-._~\na b/c\ntrue\ntrue\na+b%2Bc\na b+c\ntrue\n00000000  48 69                                             |Hi|\n\nda39a3ee5e6b4b0d3255bfef95601890afd80709\na9993e364706816aba3e25717850c26c9cd0d89d\n84983e441c3bd26ebaae4aa1f95129e5e54670f1\n",
    );
}

// `std/regex`: a linear-time regular expression engine in pure Kelp. Each
// fixture prints its own verdict, so a failure names the case; the table and the
// differential check both end in a count that has to be zero.
#[test]
fn the_regex_conformance_table_passes() {
    assert_compiles_and_runs("tests/fixtures/stdlib_regex_table.kp", "cases: 183, failed: 0\n");
}

#[test]
fn the_regex_refuses_bad_patterns_with_their_message_and_offset() {
    assert_compiles_and_runs("tests/fixtures/stdlib_regex_errors.kp", "cases: 45, failed: 0\n");
}

#[test]
fn the_regex_api_finds_captures_replaces_and_splits() {
    assert_compiles_and_runs(
        "tests/fixtures/stdlib_regex_api.kp",
        include_str!("fixtures/stdlib_regex_api.expected"),
    );
}

#[test]
fn the_regex_is_linear_on_patterns_that_explode_a_backtracker() {
    assert_compiles_and_runs(
        "tests/fixtures/stdlib_regex_linear.kp",
        include_str!("fixtures/stdlib_regex_linear.expected"),
    );
}

#[test]
fn the_regex_agrees_with_a_slow_backtracking_reference() {
    assert_compiles_and_runs(
        "tests/fixtures/stdlib_regex_differential.kp",
        "differing cases: 0\n",
    );
}

#[test]
fn regex_is_match_panics_on_a_bad_pattern() {
    assert_fixture_panics("stdlib_regex_panic", "regex.is_match: unclosed group");
}

// A body that ends in `while true { ... }` never falls through, so a function
// returning an owned type needs no fallthrough `ret` — which it could not build
// one for anyway, a `String` having no zero value. Both backends agree here now;
// before, the LLVM one refused the shape as a compiler defect.
#[test]
fn a_function_whose_body_ends_in_an_endless_loop_returns_an_owned_value() {
    assert_compiles_and_runs(
        "tests/fixtures/owned_return_endless_loop.kp",
        "xxx\n\n4\n9\n",
    );
}

// `std/tz`: time zones over the system TZif database. The synthetic fixture
// builds its own TZif byte arrays, so it pins the parser, the footer rule and
// the corrupt-data refusals on a machine with no database at all; the zone
// fixture needs one and prints a single skip line when there is none, which the
// test expects instead of the table (the same probe, `zoneinfo_has_the_test_zones`).
#[test]
fn the_tz_conformance_table_passes() {
    assert_compiles_and_runs(
        "tests/fixtures/stdlib_tz_synthetic.kp",
        "gap: gap\nambiguous: ambiguous 1730611800 1730615400\none: one 1710052200\ncases: 116, failed: 0\n",
    );
}

#[test]
fn the_tz_module_reads_the_system_zoneinfo_database() {
    let expected = if zoneinfo_has_the_test_zones() {
        "cases: 57, failed: 0\n"
    } else {
        "tz: no zoneinfo database\n"
    };
    assert_compiles_and_runs("tests/fixtures/stdlib_tz_zones.kp", expected);
}

/// Whether the host carries every zone `stdlib_tz_zones.kp` reads, in `$TZDIR`
/// or in `/usr/share/zoneinfo` — the same probe the fixture makes, so the two
/// agree about whether the database is there.
fn zoneinfo_has_the_test_zones() -> bool {
    let tzdir = std::env::var("TZDIR").unwrap_or_default();
    let dirs = [tzdir, "/usr/share/zoneinfo".to_string()];
    [
        "America/New_York",
        "Europe/Berlin",
        "Asia/Kolkata",
        "Australia/Lord_Howe",
    ]
    .iter()
    .all(|zone| {
        dirs.iter()
            .any(|dir| !dir.is_empty() && Path::new(dir).join(zone).is_file())
    })
}

#[test]
fn an_extern_without_a_compiler_supplied_body_is_rejected() {
    assert_rejected("tests/fixtures/extern_no_body.kp");
}

#[test]
fn an_extern_receiver_declaration_is_rejected() {
    assert_rejected("tests/fixtures/extern_receiver.kp");
}

#[test]
fn comparing_owned_values_is_rejected() {
    // Without this rule the comparison reached `cc` as `kelp_a == kelp_b` on two
    // `kelp_String` structs, which is where the author would have learned what
    // happened instead of at the type checker.
    assert_rejected("tests/fixtures/compare_owned.kp");
}

#[test]
fn an_owned_value_in_statement_position_is_rejected() {
    assert_rejected("tests/fixtures/owned_statement.kp");
}

// ---------------------------------------------------------------------------
// M7: capability-based security. A module's grant is received from its
// importer, never self-declared; the gate is a resolution error; the matching
// is a compiler-inserted runtime check against the caller's compiled-in
// allowlist.
// ---------------------------------------------------------------------------

/// The placeholder a capability fixture's source carries instead of a literal
/// scratch root (BUG-q8Vkr). A capability pattern has to be a literal, absolute
/// path in the fixture's own source text (a grant is source), so the harness
/// cannot simply hand the fixture a per-run directory the way `assert_*`
/// passes an expected string: the substitution has to happen in the text
/// itself, before `kelpc` ever lexes it. This mirrors `std/http`'s
/// `__KELP_HTTP_POOL__` (`stdlib.rs::HTTP_POOL_PLACEHOLDER`), substituted into
/// shipped module text before lexing; `caps_project` is the fixture-suite
/// equivalent, substituting into a throwaway copy instead of compiler-shipped
/// text.
const SCRATCH_PLACEHOLDER: &str = "__KELP_SCRATCH__";

/// The root every capability fixture's scratch paths are substituted under for
/// this process, computed once and reused for the life of the test binary.
///
/// Two suites running at once, each in its own worktree, used to both write
/// and delete the literal `/tmp/kelp_caps_*` trees the fixtures' source named,
/// which is the flake BUG-q8Vkr records. Keying the root on this process's own
/// id keeps two concurrent `cargo test` invocations apart even when they share
/// one `/tmp` (a `TMPDIR` pointed elsewhere works the same way, since
/// `std::env::temp_dir()` is the base rather than a literal `/tmp`).
fn scratch_root() -> &'static Path {
    static ROOT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    ROOT.get_or_init(|| {
        std::env::temp_dir().join(format!("kelpc_test_scratch_{}", std::process::id()))
    })
}

/// A scratch directory the capability fixtures point at. Their patterns are
/// absolute (a grant must be), so the path has to exist before the program
/// runs and is removed afterwards.
fn caps_scratch(name: &str) -> PathBuf {
    let dir = scratch_root().join(format!("kelp_caps_{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("could not create the capability scratch directory");
    dir
}

/// A capability fixture project, copied into a throwaway, process-unique
/// directory with `SCRATCH_PLACEHOLDER` substituted for `scratch_root()` in
/// every file's text. The original under `tests/projects/<name>` is untouched
/// (it is shared with every other worktree checked out from the same
/// history), and the copy is deliberately thrown away and rewritten on every
/// call, the same way `caps_scratch`'s directory is.
///
/// Callers use this exactly like `project(name)`: bare, for a manifest-driven
/// project directory (`coral.toml` + `src/`, C-2's `[caps]` fixtures), or
/// `.join("main.kp")`/`.join("util.kp")` for a flat, manifest-less one (every
/// other capability fixture) whose build target or denial-message path is one
/// file beside its siblings.
fn caps_project(name: &str) -> PathBuf {
    let src_dir = project(name);
    let dst_dir = out_path(&format!("{name}_src"));
    let _ = std::fs::remove_dir_all(&dst_dir);
    std::fs::create_dir_all(&dst_dir).expect("could not create the substituted project copy");
    let root = scratch_root().to_string_lossy().into_owned();
    copy_substituted(&src_dir, &dst_dir, &root);
    dst_dir
}

/// `caps_project`'s recursive half: copies every file under `src` to the same
/// relative path under `dst`, substituting `SCRATCH_PLACEHOLDER` for `root` in
/// each one's text, and recreates every subdirectory rather than skipping it.
/// A flat fixture (`main.kp`, `util.kp` beside each other) never recurses; a
/// manifest-driven one (`coral.toml` at the root, `.kp` files under `src/`)
/// needs the recursion to bring `src/` along at all.
fn copy_substituted(src: &Path, dst: &Path, root: &str) {
    for entry in std::fs::read_dir(src).expect("could not read fixture project dir") {
        let entry = entry.expect("could not read a fixture project dir entry");
        let path = entry.path();
        let target = dst.join(entry.file_name());
        if path.is_dir() {
            std::fs::create_dir_all(&target)
                .expect("could not create a substituted project subdirectory");
            copy_substituted(&path, &target, root);
        } else {
            let text = std::fs::read_to_string(&path).expect("could not read fixture source");
            let substituted = text.replace(SCRATCH_PLACEHOLDER, root);
            std::fs::write(&target, substituted)
                .expect("could not write the substituted fixture source");
        }
    }
}

/// Builds a caps project, runs it, and asserts the run fails with `needle` on
/// stderr — the runtime-denial path, which only a *granted* module can reach.
fn assert_project_denied_at_runtime(name: &str, target: &Path, needle: &str) {
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
    let run = Command::new(&bin).output().expect("failed to run the binary");
    assert!(
        !run.status.success(),
        "expected '{name}' to be denied at runtime, but it exited successfully"
    );
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.contains(needle),
        "expected the denial for '{name}' to mention '{needle}', got:\n{stderr}"
    );
}

#[test]
fn a_granted_module_reads_inside_its_grant() {
    let dir = caps_scratch("granted");
    std::fs::write(dir.join("note.txt"), "hello caps").unwrap();
    assert_project_runs(
        "caps_granted",
        &caps_project("caps_granted").join("main.kp"),
        "hello caps\ntrue\ntrue\n",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_ungranted_module_cannot_name_the_gated_namespace() {
    assert_project_rejected(
        "caps_ungranted",
        &caps_project("caps_ungranted").join("main.kp"),
        "needs the 'fs' capability, which module 'util' was not granted",
    );
}

#[test]
fn a_module_cannot_delegate_more_than_it_holds() {
    assert_project_rejected(
        "caps_escalation",
        &caps_project("caps_escalation").join("main.kp"),
        "can delegate less than it has, never more",
    );
}

// ---------------------------------------------------------------------------
// C-2 (roadmap open item 18): coral.toml's [caps] section restricts the
// entry module's own capability set, which the entry module could not
// restrict any other way — an ordinary `caps(...)` clause on one of its own
// `impl` lines grants the *imported* module, never main itself. A project
// with no [caps] section keeps the old default (main holds everything, every
// project above this point in the file has none), so these three are the
// only fixtures pinning the new surface.
// ---------------------------------------------------------------------------

#[test]
fn the_entry_modules_own_caps_section_can_leave_a_domain_ungranted() {
    // This project's coral.toml [caps] grants only 'env', so main's direct
    // reference to 'fs' resolves against nothing — a resolution error at the
    // same place and with the same wording an imported module's ungranted
    // reference gets. Nothing in this fixture touches disk (the build never
    // gets past resolution), so it needs no scratch directory at all.
    assert_project_rejected(
        "caps_entry_ungranted",
        &caps_project("caps_entry_ungranted"),
        "needs the 'fs' capability, which module 'main' was not granted",
    );
}

#[test]
fn a_caps_restricted_entry_module_can_still_reach_what_it_was_granted() {
    let dir = caps_scratch("entry_restricted");
    assert_project_runs(
        "caps_entry_restricted",
        &caps_project("caps_entry_restricted"),
        "true\nhi\n",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_caps_restricted_entry_module_is_denied_outside_its_own_pattern_at_runtime() {
    // 'fs' is held (resolution passes), but only for a scratch directory this
    // project's main.kp never names (it reads '/tmp', not the granted
    // directory), so the denial is the runtime pattern check, not the
    // resolution gate above — and it names 'main', the way
    // a_path_outside_the_grant_is_denied_at_runtime names 'util' for a
    // delegated module. No scratch directory needs to exist on disk for that
    // same reason (a_path_outside_the_grant_is_denied_at_runtime creates none
    // either): the check is a pattern match against the argument, not a
    // lookup inside the granted directory.
    assert_project_denied_at_runtime(
        "caps_entry_pattern_denied",
        &caps_project("caps_entry_pattern_denied"),
        "outside the fs capability granted to module 'main'",
    );
}

#[test]
fn a_delegated_wildcard_lets_the_import_reach_every_resource() {
    // `caps(*)` is the whole-resource wildcard of every domain, handed to the
    // import. `helper` reads one file in each of two unrelated trees, which no
    // single narrow pattern could cover, so a passing run is the wildcard
    // travelling with the import.
    let first = caps_scratch("star");
    let second = caps_scratch("star_elsewhere");
    std::fs::write(first.join("first.txt"), "hello ").unwrap();
    std::fs::write(second.join("second.txt"), "world").unwrap();
    assert_project_runs(
        "caps_star",
        &caps_project("caps_star").join("main.kp"),
        "hello world\n",
    );
    let _ = std::fs::remove_dir_all(&first);
    let _ = std::fs::remove_dir_all(&second);
}

#[test]
fn a_full_grant_elides_the_runtime_path_check_but_a_narrow_one_keeps_it() {
    // A path domain's whole-resource pattern (`/**`) matches any string
    // unconditionally in `glob_match` (`std/rt.kp`), so when a module's own
    // compiled-in pattern list for `fs` holds that literal pattern, the
    // `kelp_cap_check_path` call at a gated call site is provably always
    // going to pass and `cap_plan` drops it before codegen rather than
    // emitting a runtime call that can only ever succeed
    // (`codegen_c.rs::Gen::cap_plan`, `codegen_llvm.rs`'s mirror). This is a
    // codegen-shape assertion, so it reads the generated C the way
    // `the_generated_c_maps_statements_back_to_their_kp_lines` does, and pins
    // `KELPC_ORACLE=c` explicitly for the same reason that test does: the
    // artifact under test is one only that backend writes.
    //
    // `caps_elide_full_grant` (`util` holds `caps(fs = ["/**"])` outright) is
    // the positive case: its one `fs.read_text` call site must compile to no
    // `kelp_cap_check_path` call at all, while still reading the file
    // correctly. It uses its own scratch directory, not one another test also
    // writes to, so there is no cross-test race on the setup.
    let full = caps_scratch("elide_full_grant");
    std::fs::write(full.join("note.txt"), "hello caps").unwrap();

    let bin = out_path("caps_elide_full_grant");
    let build = kelpc()
        .env("KELPC_ORACLE", "c")
        .arg("build")
        .arg(caps_project("caps_elide_full_grant").join("main.kp"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected 'caps_elide_full_grant' to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&bin).output().expect("failed to run the binary");
    assert!(
        run.status.success(),
        "expected 'caps_elide_full_grant' to run cleanly"
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "hello caps\n",
        "the elided check must not change the program's observable result"
    );
    let generated = std::fs::read_to_string(bin.with_extension("c"))
        .expect("the generated C sits beside the binary");
    assert!(
        !generated.contains("kelp_cap_check_path(\""),
        "a module holding the full '/**' grant for 'fs' must have no \
         'kelp_cap_check_path' call left at its call sites, since every one \
         of them is provably going to pass:\n{generated}"
    );

    // The negative case: `caps_elide_narrow_grant`'s `util` module holds a
    // narrower pattern (not `/**`), so its call to `fs.read_text` still has
    // to carry the runtime check. Eliding it there would be the correctness
    // hole this task exists to avoid: a narrower grant silently starting to
    // admit everything.
    //
    // Denial itself, for a narrower grant, is already covered by
    // `a_path_outside_the_grant_is_denied_at_a_primitive_too` (project
    // `caps_raw_denied`), so this only re-proves the check is still *emitted*
    // rather than duplicating that denial test.
    let narrow = caps_scratch("elide_narrow_grant");
    std::fs::write(narrow.join("note.txt"), "hello caps").unwrap();
    let bin2 = out_path("caps_elide_narrow_grant");
    let build2 = kelpc()
        .env("KELPC_ORACLE", "c")
        .arg("build")
        .arg(caps_project("caps_elide_narrow_grant").join("main.kp"))
        .arg("-o")
        .arg(&bin2)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build2.status.success(),
        "expected 'caps_elide_narrow_grant' to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build2.stderr)
    );
    let generated2 = std::fs::read_to_string(bin2.with_extension("c"))
        .expect("the generated C sits beside the binary");
    assert!(
        generated2.contains("kelp_cap_check_path(\""),
        "a module holding a narrower-than-'/**' grant for 'fs' must keep its \
         runtime 'kelp_cap_check_path' call:\n{generated2}"
    );

    let _ = std::fs::remove_dir_all(&full);
    let _ = std::fs::remove_dir_all(&narrow);
}

#[test]
fn a_whole_resource_name_grant_elides_the_defining_modules_own_check() {
    // A name domain's wrapper calls its own gated `extern fun` (`std/env`'s
    // `has` over `has_raw`, one line each), and that inner call carries a
    // `CapCheck::Name` against `std/env`'s own granted list. When that list is
    // exactly the domain's whole-resource `*`, the inner check is provably
    // going to pass for every name its callers can reach: an `env` grant may
    // not name a `/` (`resolve.rs::validate_cap_pattern` refuses one) and `*`
    // crosses none, so the call site's own check has already admitted the
    // name. `cap_plan` drops it before codegen, in both backends
    // (`codegen_c.rs::Gen::cap_plan` and its mirror in `codegen_llvm.rs`).
    //
    // This is a codegen-shape assertion, so it reads the generated C the way
    // the path-domain elision test above does, and pins `KELPC_ORACLE=c`
    // explicitly for the same reason: the artifact under test is one only that
    // backend writes. The expected count is exact and stated: ONE
    // `kelp_cap_check_name` call for the `env.has` access, the entry module's
    // own. Without the elision there are five, the four inside `std/env`'s
    // wrappers included, which the narrow case below pins.
    let bin = out_path("caps_elide_name_full");
    let build = kelpc()
        .env("KELPC_ORACLE", "c")
        .arg("build")
        .arg(project("caps_elide_name_full").join("main.kp"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected 'caps_elide_name_full' to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&bin).output().expect("failed to run the binary");
    assert!(
        run.status.success(),
        "expected 'caps_elide_name_full' to run cleanly"
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "true\n",
        "the elided check must not change the program's observable result"
    );
    let generated = std::fs::read_to_string(bin.with_extension("c"))
        .expect("the generated C sits beside the binary");
    assert_eq!(
        generated.matches("kelp_cap_check_name(\"").count(),
        1,
        "a whole-resource ('*') grant to 'std/env' must leave exactly one \
         'kelp_cap_check_name' call for this access, the entry module's own:\n{generated}"
    );
    assert!(
        generated.contains("kelp_cap_check_name(\"env\", \"PATH\", kelp_caps_main_env"),
        "the one check left must be the entry module's own, against its own \
         pattern list:\n{generated}"
    );
    assert!(
        !generated.contains("kelp_cap_check_name(\"env\", kelp_name, kelp_caps_std_env_env"),
        "the check the elision removes is the one inside 'std/env' itself, \
         against 'std/env's own list:\n{generated}"
    );

    // The negative control: `std/env` holds a narrower list than `*`, so its
    // own internal check is the effective gate for what its wrappers may read
    // and is kept. The entry module holds `*`, which admits `HOME`, so this
    // check is the only thing that can deny it, and the denial names
    // `std/env` rather than the entry module.
    let bin2 = out_path("caps_elide_name_narrow");
    let build2 = kelpc()
        .env("KELPC_ORACLE", "c")
        .arg("build")
        .arg(project("caps_elide_name_narrow").join("main.kp"))
        .arg("-o")
        .arg(&bin2)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build2.status.success(),
        "expected 'caps_elide_name_narrow' to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build2.stderr)
    );
    let generated2 = std::fs::read_to_string(bin2.with_extension("c"))
        .expect("the generated C sits beside the binary");
    assert_eq!(
        generated2.matches("kelp_cap_check_name(\"").count(),
        5,
        "a narrower grant to 'std/env' must keep all four internal \
         'kelp_cap_check_name' calls beside the entry module's own:\n{generated2}"
    );
    let run2 = Command::new(&bin2).output().expect("failed to run the binary");
    assert!(
        !run2.status.success(),
        "expected 'caps_elide_name_narrow' to be denied at runtime"
    );
    let stderr2 = String::from_utf8_lossy(&run2.stderr);
    assert!(
        stderr2.contains("'HOME' is outside the env capability granted to module 'std/env'"),
        "expected the denial to name 'std/env' and the refused name, got:\n{stderr2}"
    );
    assert!(
        stderr2.contains("PATH"),
        "expected the denial to name the granted pattern, got:\n{stderr2}"
    );
}

#[test]
fn a_wildcard_caller_outside_the_domain_module_keeps_its_own_check() {
    // The safety boundary, and the test that would catch the elision being
    // broadened past the domain's own defining module: `util` holds the
    // whole-resource `*` for `env` and is not `std/env`, so its own
    // `CapCheck::Name` is emitted exactly as before, and its own
    // `*`-does-not-cross-`/` rule is what denies the slashy name. `std/env`
    // holds `*` in this same program, so the elision is active: if it had
    // reached `util` too, there would be no check left at all and this run
    // would succeed. Never an exact literal grant: an `env` pattern may not
    // contain a `/` at all, which
    // `an_env_grant_cannot_name_a_slashy_variable` pins as a refusal.
    let bin = out_path("caps_elide_name_wild_caller");
    let build = kelpc()
        .env("KELPC_ORACLE", "c")
        .arg("build")
        .arg(project("caps_elide_name_wild_caller").join("main.kp"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected 'caps_elide_name_wild_caller' to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let generated = std::fs::read_to_string(bin.with_extension("c"))
        .expect("the generated C sits beside the binary");
    assert_eq!(
        generated.matches("kelp_cap_check_name(\"").count(),
        1,
        "'std/env's own checks are elided here, and 'util's own is not: \
         exactly one 'kelp_cap_check_name' call is left:\n{generated}"
    );
    assert!(
        generated.contains("kelp_cap_check_name(\"env\", \"has/nested\", kelp_caps_util_env"),
        "the one check left must be the ordinary '*' holder's own, not an \
         elided one:\n{generated}"
    );
    let run = Command::new(&bin).output().expect("failed to run the binary");
    assert!(
        !run.status.success(),
        "a name with a '/' in it reached only through a '*' grant must still \
         be denied"
    );
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.contains("'has/nested' is outside the env capability granted to module 'util'"),
        "expected 'util's own check to refuse the slashy name, got:\n{stderr}"
    );
}

#[test]
fn an_env_grant_cannot_name_a_slashy_variable() {
    // The scenario the elision was approved on was an exact, literal grant for
    // a name that contains a `/`, which `std/env`'s own wildcard could never
    // admit and the elision would have made newly sufficient. No such program
    // can be written: `resolve.rs::validate_cap_pattern` refuses a `/` in an
    // `env` pattern by name, at the `impl` line, so the only grant that can
    // ever admit a slashy name is none at all. That is also why the elision
    // cannot admit one where a grant could not.
    assert_project_rejected(
        "caps_elide_name_slashy_grant",
        &project("caps_elide_name_slashy_grant").join("main.kp"),
        "'has/slash' is not a variable name",
    );
}

#[test]
fn a_module_cannot_delegate_the_wildcard_it_does_not_hold() {
    // `mid` holds one narrow `fs` pattern, so `caps(*)` on its own import
    // claims a wildcard it was never given; the refusal names the wildcard.
    assert_project_rejected(
        "caps_star_escalation",
        &caps_project("caps_star_escalation").join("main.kp"),
        "cannot give 'inner' the whole-resource wildcard for 'fs' with `caps(*)`",
    );
}

#[test]
fn a_named_domain_beside_the_wildcard_is_refused() {
    // A fixture, not a project: the mixed clause is a syntax error, and the
    // fixtures are where the suite keeps programs that do not parse (the
    // formatter is right to refuse one and must not be asked to format it).
    assert_rejected_with(
        "tests/fixtures/caps_star_mixing.kp",
        "already means every domain",
    );
}

#[test]
fn the_wildcard_on_a_module_that_reaches_no_domain_is_refused() {
    assert_project_rejected(
        "caps_star_ungated",
        &project("caps_star_ungated").join("main.kp"),
        "'std/str' is ungated: it reaches no capability domain",
    );
}

#[test]
fn an_ungranted_importer_is_named_as_the_line_to_change() {
    // `main` imports `store` with no `caps(...)`, so the error names `store`
    // even though the grant that has to change is `main`'s bare `impl` line.
    assert_project_rejected(
        "caps_delegate_from_nothing",
        &project("caps_delegate_from_nothing").join("main.kp"),
        "'store' holds nothing here because the module that imports it wrote no",
    );
}

#[test]
fn a_pattern_containing_dotdot_is_rejected() {
    assert_project_rejected(
        "caps_bad_pattern",
        &project("caps_bad_pattern").join("main.kp"),
        "contains a '..' segment",
    );
}

#[test]
fn an_unknown_capability_domain_is_rejected() {
    assert_project_rejected(
        "caps_bad_domain",
        &project("caps_bad_domain").join("main.kp"),
        "'disk' is not a capability domain",
    );
}

// ---------------------------------------------------------------------------
// Stage n (MK9-S2, roadmap B-3): `unsafe` and raw pointers. `Ptr<T>` is a
// builtin generic like `Atomic<T>`, its operations are `Ptr#<method>` on both
// backends, and the manifest's `[unsafe] modules` list is the audit rule: only
// a module named there may contain an `unsafe` block or an `unsafe fun`.
// ---------------------------------------------------------------------------

#[test]
fn a_raw_pointer_allocates_writes_reads_volatile_and_frees() {
    assert_project_runs(
        "unsafe_ptr",
        &project("unsafe_ptr"),
        "100\n99\n65\n42\n42\ntrue\n4096\n",
    );
}

/// Kernel gap 2: `Ptr.from_fn(f)` is the address of a declared `fun` or a
/// non-capturing `fun` literal, and it is legal outside `unsafe`. Two distinct
/// functions have distinct, non-null addresses, and the same function answers
/// the same address twice.
#[test]
fn a_function_address_is_a_checked_pointer() {
    assert_compiles_and_runs(
        "tests/fixtures/ptr_from_fn.kp",
        "true\ntrue\ntrue\ntrue\ntrue\ntrue\n",
    );
}

/// A closure carries the environment its captures were moved into, so its code
/// address alone is not the value: `Ptr.from_fn` refuses one and says why.
#[test]
fn a_closure_has_no_standalone_function_address() {
    assert_rejected_with(
        "tests/fixtures/ptr_from_fn_closure.kp",
        "it carries an environment the code address alone cannot describe",
    );
}

#[test]
fn an_unsafe_fun_is_callable_from_an_unsafe_block() {
    assert_project_runs("unsafe_fn", &project("unsafe_fn"), "77\n");
}

#[test]
fn a_module_with_unsafe_but_no_manifest_entry_is_rejected() {
    assert_project_rejected(
        "unsafe_not_listed",
        &project("unsafe_not_listed"),
        "module 'main' is not listed in coral.toml [unsafe] modules",
    );
}

#[test]
fn an_unsafe_pointer_read_outside_a_block_is_rejected() {
    assert_project_rejected(
        "unsafe_outside_block",
        &project("unsafe_outside_block"),
        "'Ptr.read' is an unsafe operation",
    );
}

#[test]
fn an_unsafe_fun_called_outside_a_block_is_rejected() {
    assert_project_rejected(
        "unsafe_fn_outside",
        &project("unsafe_fn_outside"),
        "'poke' is an 'unsafe fun'",
    );
}

#[test]
fn a_pointer_to_an_owned_type_is_rejected() {
    assert_project_rejected(
        "unsafe_bad_elem",
        &project("unsafe_bad_elem"),
        "'String' is an owned heap value",
    );
}

// ---------------------------------------------------------------------------
// Kernel stage 2 (MK9-S1-M2, roadmap B-2): layout control. `repr(c)` (the
// default, now guaranteed), `repr(packed)`, `repr(align(N))`, the compile-time
// queries `size_of`/`align_of`/`offset_of`, and explicit discriminants on a
// field-less enum with `to_int`/`from_int`. Every fixture here runs on the
// default (LLVM) backend and on the C oracle, because the suite is run twice.
// ---------------------------------------------------------------------------

#[test]
fn a_packed_datac_has_the_c_layout_of_its_fields() {
    assert_compiles_and_runs(
        "tests/fixtures/repr_packed.kp",
        "7\n1\n0\n1\n5\n7\n287454020\n21862\n",
    );
}

#[test]
fn a_repr_c_datac_keeps_the_default_padded_layout() {
    assert_compiles_and_runs(
        "tests/fixtures/repr_c_padded.kp",
        "12\n4\n0\n4\n8\n10\n8\n4\n4\n4\n8\n8\n8\n",
    );
}

#[test]
fn a_layout_query_folds_in_an_array_length_and_a_constant() {
    assert_compiles_and_runs(
        "tests/fixtures/layout_query_in_a_constant.kp",
        "8\n4\n4\n8\n",
    );
}

#[test]
fn a_repr_align_datac_raises_its_own_and_an_embedded_type_s_alignment() {
    assert_compiles_and_runs("tests/fixtures/repr_align.kp", "16\n16\n0\n8\n32\n16\n16\n");
}

#[test]
fn an_enum_round_trips_its_discriminants_through_to_int_and_from_int() {
    assert_compiles_and_runs(
        "tests/fixtures/enum_discriminant.kp",
        "10\n2\n1\n10\n99\n1\n8\n8\n4\n-5\n-5\n4\n2\n",
    );
}

#[test]
fn a_repr_align_that_is_not_a_power_of_two_is_rejected() {
    assert_rejected_with(
        "tests/fixtures/repr_align_not_power_of_two.kp",
        "power of two",
    );
}

#[test]
fn a_duplicate_enum_discriminant_is_rejected() {
    assert_rejected_with(
        "tests/fixtures/enum_discriminant_duplicate.kp",
        "repeats the discriminant",
    );
}

#[test]
fn an_enum_with_a_payload_cannot_have_a_discriminant() {
    assert_rejected_with(
        "tests/fixtures/enum_discriminant_with_payload.kp",
        "may have an explicit discriminant",
    );
}

#[test]
fn offset_of_a_field_the_datac_does_not_have_is_rejected() {
    assert_rejected_with(
        "tests/fixtures/offset_of_missing_field.kp",
        "has no field 'y'",
    );
}

/// The same stage, end to end against a real C compiler: a packed datac is
/// handed to a project-local C helper through a user `extern fun`, and the C
/// helper's `sizeof`/`_Alignof`/`offsetof` are printed beside Kelp's own
/// `size_of`/`align_of`/`offset_of`. The last line is the helper reading three
/// fields at C's own offsets, so a backend whose struct body is not byte for
/// byte the C layout prints a different number.
#[test]
fn a_packed_datac_crosses_to_a_c_helper_and_the_two_layouts_agree() {
    assert_project_runs(
        "ffi_layout",
        &project("ffi_layout"),
        "7\n7\n1\n1\n1\n1\n5\n5\n24037596794668035\n",
    );
}

// ---------------------------------------------------------------------------
// Kernel stage 3 (MK9-S8, roadmap B-10): endianness and unaligned access. The
// integer byte-order conversions (`swap_bytes`, `to_be`/`to_le` and the static
// `u32.from_be`/`u32.from_le`), the bounds-checked byte-order readers and
// writers on a `Bytes`, and `Ptr.read_unaligned`/`write_unaligned`. Every
// fixture here runs on the default (LLVM) backend and on the C oracle, because
// the suite is run twice.
// ---------------------------------------------------------------------------

#[test]
fn integer_byte_order_conversions_reverse_the_right_bytes() {
    // 23 hand-computed values: `0x12345678u32.swap_bytes()` is `0x78563412`,
    // `to_le` is the identity on this little-endian target, `to_be` is not, and
    // swapping twice is the identity.
    assert_compiles_and_runs(
        "tests/fixtures/endian_scalars.kp",
        "3412\n78563412\n807060504030201\n78563412\n807060504030201\n-16777217\n\
         78563412\n12345678\n78563412\n12345678\n\
         3412\n1234\n3412\n1234\n\
         807060504030201\n102030405060708\n807060504030201\n102030405060708\n\
         807060504030201\n102030405060708\n807060504030201\n807060504030201\n102030405060708\n",
    );
}

#[test]
fn a_wire_header_round_trips_through_both_byte_orders() {
    // A 10-byte header read field by field in both orders, then rebuilt with
    // `write_*` and compared byte for byte with the pushed one.
    assert_compiles_and_runs(
        "tests/fixtures/bytes_endian.kp",
        "10\n4b50\n504b\n102\n201\ndeadbeef\nefbeadde\n1f90\n901f\n\
         -559038737\n-272716322\n\
         807060504030201\n102030405060708\n578437695752307201\n\
         504b0102\n2014b50\n504b0102deadbeef\n1f90\ndeadbeef\n\
         50 4b 01 02 de ad be ef 90 1f\ntrue\n\
         11223344\n44332211\n102030405060708\n807060504030201\n\
         102030405060708\n807060504030201\n-2\nfeffffff\n-1\nffffffff\n\
         -2\n-72057594037927937\n-2\nfeffffffffffffff\n",
    );
}

#[test]
fn an_out_of_bounds_byte_order_read_panics() {
    assert_fixture_panics("bytes_read_oob", "Bytes.read_u32_le: offset out of bounds");
}

#[test]
fn an_out_of_bounds_byte_order_write_panics() {
    assert_fixture_panics("bytes_write_oob", "Bytes.write_u16_be: offset out of bounds");
}

/// An offset near `usize`'s maximum wraps `offset + width` back under the
/// buffer's length, so a naive test would read out of bounds instead of
/// panicking.
#[test]
fn an_offset_that_would_wrap_the_bounds_check_panics() {
    assert_fixture_panics(
        "bytes_read_wrapped_oob",
        "Bytes.read_u16_le: offset out of bounds",
    );
}

/// `Ptr.read_unaligned`/`write_unaligned`: every access is at an odd address,
/// which is exactly what the aligned `read`/`write` are not allowed to do.
#[test]
fn an_unaligned_pointer_read_and_write_round_trip() {
    assert_project_runs(
        "unsafe_unaligned",
        &project("unsafe_unaligned"),
        "12345678\nbeef\n102030405060708\n5060708\nfreed\n",
    );
}

// ---------------------------------------------------------------------------
// MK9-S1-M1 (roadmap B-1): a user module's own `extern fun`, bound to a
// native symbol through coral.toml's `[extern."<module id>"]` section instead
// of a compiler-supplied body from `externs.rs`. Gated exactly like a stdlib
// module's domain (same `gated` map, same resolution-time refusal), plus two
// manifest-shape checks the stdlib path has no equivalent for: an unknown
// domain, and a symbol outside the module's own allowlist.
// ---------------------------------------------------------------------------

#[test]
fn a_manifest_assigned_ffi_module_calls_its_allowlisted_native_symbol() {
    assert_project_runs("ffi_getpid", &project("ffi_getpid"), "true\n");
}

#[test]
fn an_ungranted_ffi_module_cannot_name_the_gated_namespace() {
    assert_project_rejected(
        "ffi_denied",
        &project("ffi_denied"),
        "needs the 'process' capability, which module 'util' was not granted",
    );
}

#[test]
fn a_manifest_assigned_ffi_ui_module_is_reached_with_the_wildcard_grant() {
    assert_project_runs("ffi_ui_domain", &project("ffi_ui_domain"), "true\n");
}

#[test]
fn an_ffi_ui_grant_that_names_a_pattern_is_rejected() {
    // A display call carries no argument to match, so `ui = ["wayland-0"]`
    // would read as enforced and deny nothing; only the wildcard is accepted
    // (plan/ffi-display-domain.md).
    assert_project_rejected(
        "ffi_ui_pattern",
        &project("ffi_ui_pattern"),
        "patterns are not enforceable in the 'ui' domain yet",
    );
}

#[test]
fn an_ungranted_ffi_ui_module_cannot_name_the_gated_namespace() {
    assert_project_rejected(
        "ffi_ui_denied",
        &project("ffi_ui_denied"),
        "needs the 'ui' capability, which module 'util' was not granted",
    );
}

#[test]
fn an_extern_manifest_section_with_an_unknown_domain_is_rejected() {
    assert_project_rejected(
        "ffi_bad_domain",
        &project("ffi_bad_domain"),
        "'disk' is not a capability domain",
    );
}

#[test]
fn an_extern_fun_outside_its_manifest_symbol_allowlist_is_rejected() {
    assert_project_rejected(
        "ffi_bad_symbol",
        &project("ffi_bad_symbol"),
        "is not in this module's coral.toml",
    );
}

#[test]
fn a_manifest_ffi_header_key_includes_the_native_symbols_header() {
    assert_project_runs("ffi_header", &project("ffi_header"), "true\n");
}

#[test]
fn a_manifest_ffi_include_dir_reaches_the_compiled_c() {
    assert_project_runs("ffi_include_dir", &project("ffi_include_dir"), "42\n");
}

#[test]
fn a_manifest_ffi_pkg_config_supplies_the_compile_flags() {
    let target = project("ffi_pkg_config");
    let bin = out_path("ffi_pkg_config");
    let build = kelpc()
        .env("PKG_CONFIG_PATH", target.join("pkgconfig"))
        .arg("build")
        .arg(&target)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected 'ffi_pkg_config' to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&bin).output().expect("failed to run compiled binary");
    assert_eq!(String::from_utf8_lossy(&run.stdout), "42\n");
}

#[test]
fn an_unknown_ffi_pkg_config_package_fails_the_build() {
    let target = project("ffi_unknown_pkg_config");
    let bin = out_path("ffi_unknown_pkg_config");
    let build = kelpc()
        .env_remove("PKG_CONFIG_PATH")
        .arg("build")
        .arg(&target)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        !build.status.success(),
        "expected 'ffi_unknown_pkg_config' to be rejected, but it built successfully"
    );
    let stderr = String::from_utf8_lossy(&build.stderr);
    assert!(
        stderr.contains("pkg-config failed") && stderr.contains("kelp-nonexistent-pkg-9zz"),
        "expected the error to name pkg-config and the package, got:\n{stderr}"
    );
}

#[test]
fn an_empty_ffi_header_key_is_rejected() {
    assert_project_rejected(
        "ffi_bad_header",
        &project("ffi_bad_header"),
        "header' must not be empty",
    );
}

#[test]
fn an_ffi_header_key_with_a_parent_segment_is_rejected() {
    assert_project_rejected(
        "ffi_bad_header_dotdot",
        &project("ffi_bad_header_dotdot"),
        "header' must not contain",
    );
}

#[test]
fn a_manifest_ffi_symbol_rename_binds_the_kelp_name_to_the_c_symbol() {
    assert_project_runs("ffi_rename", &project("ffi_rename"), "true\n");
}

#[test]
fn an_ffi_symbol_rename_to_a_non_identifier_is_rejected() {
    assert_project_rejected(
        "ffi_bad_rename",
        &project("ffi_bad_rename"),
        "C symbol for '[extern.\"sys\"] symbols' must be a C identifier",
    );
}

#[test]
fn an_ffi_opaque_handle_allocates_compares_and_frees() {
    // Gap 1: "malloc" returns an opaque "Buffer", "==" compares two handles
    // of that type, and "free" takes one back, all straight through the C
    // trampoline.
    assert_project_runs("ffi_handle", &project("ffi_handle"), "true\n");
}

#[test]
fn an_ffi_extern_type_outside_an_extern_section_is_rejected() {
    assert_project_rejected(
        "ffi_handle_no_section",
        &project("ffi_handle_no_section"),
        "is only legal in a module with a coral.toml",
    );
}

#[test]
fn an_ffi_opaque_handle_upcasts_to_its_parent_and_casts_back_down() {
    // Gap 1: "Buffer" is a "Block". The "Buffer" from "malloc" converts
    // implicitly to "Block" at a call argument, at a `val` binding and at a
    // return, and "Buffer.cast(block)" is the unchecked way back down.
    assert_project_runs("ffi_handle_upcast", &project("ffi_handle_upcast"), "true\n");
}

#[test]
fn an_ffi_opaque_handle_carries_a_null_value() {
    // Gap 1: "fopen" returns the null handle on failure, and "File.null()"
    // spells the same value; "is_null()" is true for both and the only method
    // a handle has.
    assert_project_runs("ffi_handle_null", &project("ffi_handle_null"), "true\ntrue\n");
}

#[test]
fn an_implicit_ffi_handle_downcast_is_rejected() {
    // Gap 1: only a child converts up to its parent implicitly, so a "Block"
    // returned where a "Buffer" is expected is a type error, not an implicit
    // (and unchecked) downcast.
    assert_project_rejected(
        "ffi_handle_bad_downcast",
        &project("ffi_handle_bad_downcast"),
        "declared to return",
    );
}

#[test]
fn an_ffi_handle_null_outside_an_extern_section_is_rejected() {
    // Gap 1: a handle operation is legal only where an `[extern.*]` section
    // exists, even when the handle type itself is imported from an extern
    // module.
    assert_project_rejected(
        "ffi_handle_null_outside",
        &project("ffi_handle_null_outside"),
        "is only legal in a module with a coral.toml",
    );
}

#[test]
fn printing_an_ffi_opaque_handle_is_rejected() {
    assert_project_rejected(
        "ffi_handle_print",
        &project("ffi_handle_print"),
        "opaque native handle with no printable value",
    );
}

/// Gap 4: a user `extern fun`'s `move closure(...)` parameter expands into the
/// native `(func, user_data, destroy_notify)` triple. The C backend is the one
/// that emits the expansion, so this pins `KELPC_ORACLE=c` and runs it even
/// when the suite's backend is LLVM (which refuses the shape by name, see the
/// test below). The helper calls the callback twice and then its `destroy`
/// notify, so the printed order proves the thunk reordered the arguments, and
/// the AddressSanitizer rebuild proves the environment was freed exactly once.
#[test]
fn an_ffi_callback_closure_crosses_to_native_code() {
    let target = project("ffi_callback");
    let bin = out_path("ffi_callback");
    let build = Command::new(kelpc_bin())
        .env("KELPC_ORACLE", "c")
        .arg("build")
        .arg(&target)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected 'ffi_callback' to compile on the C backend, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&bin).output().expect("failed to run compiled binary");
    assert!(
        run.status.success(),
        "expected the 'ffi_callback' binary to exit successfully, got:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "2\n3\n2\n4\n2\n8\n9\n2\n",
        "unexpected stdout for 'ffi_callback'"
    );

    // The callback's boxed environment is the one allocation the fixture makes,
    // so a missed `destroy` is a leak and a double one is a use-after-free.
    // Rebuild the generated C with AddressSanitizer, which reports both at
    // exit; the project's own include dir is what finds the helper.
    let asan_bin = out_path("ffi_callback_asan");
    let cc = Command::new("cc")
        .arg("-fsanitize=address")
        .arg("-O1")
        .arg("-I")
        .arg(target.join("include"))
        .arg("-o")
        .arg(&asan_bin)
        .arg(bin.with_extension("c"))
        .output()
        .expect("failed to spawn cc");
    if cc.status.success() {
        let asan_run = Command::new(&asan_bin)
            .output()
            .expect("failed to run the instrumented binary");
        assert!(
            asan_run.status.success(),
            "AddressSanitizer flagged 'ffi_callback':\n{}",
            String::from_utf8_lossy(&asan_run.stderr)
        );
    }
}

/// The same project on the default (LLVM) backend, the one a program is built
/// with unless the oracle switch says otherwise. It must print exactly what the
/// C backend prints: the closure crosses as a pointer to the caller's slot, the
/// per-program C object boxes it, and the thunk and destructor are the same
/// generated text.
#[test]
fn an_llvm_ffi_callback_closure_crosses_to_native_code() {
    let target = project("ffi_callback");
    let bin = out_path("ffi_callback_llvm");
    let build = Command::new(kelpc_bin())
        .env_remove("KELPC_ORACLE")
        .arg("build")
        .arg(&target)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected 'ffi_callback' to compile on the default backend, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&bin).output().expect("failed to run compiled binary");
    assert!(
        run.status.success(),
        "expected the llvm 'ffi_callback' binary to exit successfully, got:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "2\n3\n2\n4\n2\n8\n9\n2\n",
        "the llvm build must print what the C build prints"
    );
}

#[test]
fn an_ffi_callback_closure_parameter_that_moves_is_rejected() {
    assert_project_rejected(
        "ffi_callback_move_param",
        &project("ffi_callback_move_param"),
        "forwards every argument by value",
    );
}

#[test]
fn an_ffi_extern_fun_with_two_callback_closures_is_rejected() {
    assert_project_rejected(
        "ffi_callback_two",
        &project("ffi_callback_two"),
        "at most one 'move closure(...)' parameter",
    );
}

/// Gap 7: a user `extern fun` whose last parameter is `...` accepts any number
/// of extra arguments after the fixed ones. Each distinct list of extra types
/// gets one generated fixed-arity C wrapper, the call goes through it, and the
/// C compiler applies the default argument promotions, so neither backend ever
/// emits a variadic call itself. The fixture calls with 0, 1 and 3 extra `i32`s
/// and once through a second helper that reads a promoted `f32` as a `double`
/// and a `str` as a `const char*`.
#[test]
fn an_ffi_variadic_extern_accepts_extra_arguments() {
    assert_project_runs("ffi_variadic", &project("ffi_variadic"), "0\n7\n6\n11\n");
}

/// The same project on the default (LLVM) backend, whose per-program C object
/// carries the wrappers that the generated C calls. It must print exactly what
/// the C backend prints; pinning LLVM here keeps the wrappers covered even
/// when the suite is pointed at the oracle.
#[test]
fn an_llvm_ffi_variadic_extern_accepts_extra_arguments() {
    let target = project("ffi_variadic");
    let bin = out_path("ffi_variadic_llvm");
    let build = Command::new(kelpc_bin())
        .env_remove("KELPC_ORACLE")
        .arg("build")
        .arg(&target)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected 'ffi_variadic' to compile on the default backend, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&bin).output().expect("failed to run compiled binary");
    assert!(
        run.status.success(),
        "expected the llvm 'ffi_variadic' binary to exit successfully, got:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "0\n7\n6\n11\n",
        "the llvm build must print what the C build prints"
    );
}

#[test]
fn an_ffi_extern_accepts_function_pointers() {
    assert_project_runs("ffi_fn_pointer", &project("ffi_fn_pointer"), "41\n42\n");
}

#[test]
fn an_llvm_ffi_extern_accepts_function_pointers() {
    let target = project("ffi_fn_pointer");
    let bin = out_path("ffi_fn_pointer_llvm");
    let build = Command::new(kelpc_bin())
        .env_remove("KELPC_ORACLE")
        .arg("build")
        .arg(&target)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected 'ffi_fn_pointer' to compile on the default backend, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&bin).output().expect("failed to run compiled binary");
    assert!(
        run.status.success(),
        "expected the llvm 'ffi_fn_pointer' binary to exit successfully, got:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "41\n42\n",
        "the llvm build must print what the C build prints"
    );
}

#[test]
fn an_ffi_variadic_marker_that_is_not_last_is_rejected() {
    // A fixture, not a project: `...` followed by another parameter is a
    // syntax error, and the fixtures are where the suite keeps programs that
    // do not parse (the formatter is right to refuse one).
    assert_rejected_with(
        "tests/fixtures/ffi_variadic_not_last.kp",
        "must be the last parameter of an 'extern fun'",
    );
}

#[test]
fn an_ffi_variadic_extra_argument_of_a_c_unsafe_type_is_rejected() {
    assert_project_rejected(
        "ffi_variadic_bad_arg",
        &project("ffi_variadic_bad_arg"),
        "extra argument 1 of 'helper.kelp_va_sum' has type 'String'",
    );
}

#[test]
fn a_granted_module_can_write_rename_size_and_remove_inside_its_grant() {
    let dir = caps_scratch("write");
    assert_project_runs(
        "caps_write",
        &caps_project("caps_write").join("main.kp"),
        "true\n5\nfalse\nhello\nfalse\ntrue\n3\n",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_path_outside_the_grant_is_denied_at_runtime() {
    assert_project_denied_at_runtime(
        "caps_denied",
        &caps_project("caps_denied").join("main.kp"),
        "outside the fs capability granted to module 'util'",
    );
}

#[test]
fn a_symlink_inside_the_grant_pointing_outside_is_denied() {
    let dir = caps_scratch("symlink");
    std::os::unix::fs::symlink("/etc/passwd", dir.join("escape")).unwrap();
    assert_project_denied_at_runtime(
        "caps_symlink",
        &caps_project("caps_symlink").join("main.kp"),
        "/etc/passwd",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_gated_resource_argument_is_evaluated_once_and_the_control_is_denied() {
    // The C backend's capability check used to re-evaluate the resource
    // argument, so the check admitted one path and the call read another. This
    // project pins both halves of that fix in one run.
    //
    // `util.inside_once()` is the fail-open shape: `mk()` names a path inside
    // the grant the first time it runs and `/etc/passwd` the second, and it
    // prints `SIDE` on every call. The argument is evaluated once now, so the
    // stdout is one `SIDE` and the in-grant file's contents; before the fix it
    // was two and the contents of `/etc/passwd`.
    //
    // `util.outside()` is the control, out of the grant on every evaluation,
    // and its denial is asserted whole rather than by substring, so the path,
    // the location, the `granted:` line and its indentation are all pinned.
    let dir = caps_scratch("double_eval");
    std::fs::write(dir.join("a"), "FILECONTENT").unwrap();
    let target = caps_project("caps_double_eval").join("main.kp");
    let bin = out_path("caps_double_eval");
    let build = kelpc()
        .arg("build")
        .arg(&target)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected project 'caps_double_eval' to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&bin).output().expect("failed to run the binary");
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "SIDE\nFILECONTENT\n",
        "the resource argument was not evaluated exactly once, or the call read \
         a different path than the check admitted"
    );
    assert_eq!(run.status.code(), Some(1), "the control must be denied");
    // `util.kp:23:36` is `fs.read_text("/etc/passwd")` in the control, as the
    // compiler was invoked with the file. (`pub` on the control's own line is
    // what moved the column from 27; A-13.)
    let expected = format!(
        "{}:23:36: panic: '/etc/passwd' is outside the fs capability granted to module 'util'\n  \
         granted:\n    {}/kelp_caps_double_eval/**\n",
        caps_project("caps_double_eval").join("util.kp").display(),
        scratch_root().display()
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stderr),
        expected,
        "the capability denial's text is not what the compiler writes"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// MK7 extern gating: the four routes that made a grant advisory (`BUG-wQ9tL`).
//
// A gated module's item is gated wherever it is *named*: its `extern fun`
// primitives and its receiver methods sit in the gate table beside its free
// functions, and the naming sites the check has to reach are the direct call,
// a function value, and a `pd` initializer. One test per route, each refused or
// denied where before it read the file.
// ---------------------------------------------------------------------------

#[test]
fn an_ungranted_module_cannot_name_another_modules_primitive() {
    // Before the fix this built and printed the length of `/etc/hostname` on
    // both backends: the gate table held only a gated module's non-extern free
    // functions, so a user module could reach around the wrapper to the raw
    // syscall with no grant at all.
    assert_project_rejected(
        "caps_raw_ungranted",
        &project("caps_raw_ungranted").join("main.kp"),
        "needs the 'fs' capability, which module 'util' was not granted",
    );
}

#[test]
fn a_path_outside_the_grant_is_denied_at_a_primitive_too() {
    // The domain is held, so the name resolves and the *pattern* check is what
    // has to fire. A raw call goes through the same `cap_plan` a wrapper call
    // does, so its denial names the same module, call site and grant.
    assert_project_denied_at_runtime(
        "caps_raw_denied",
        &caps_project("caps_raw_denied").join("main.kp"),
        "outside the fs capability granted to module 'util'",
    );
}

#[test]
fn a_granted_module_reads_inside_its_grant_through_a_primitive() {
    // The positive half of the same rule: a non-entry module that holds the
    // domain names the primitive and reads a path inside its grant. A fix that
    // hid the primitives instead would fail here.
    let dir = caps_scratch("raw_granted");
    std::fs::write(dir.join("note.txt"), "hello caps").unwrap();
    assert_project_runs(
        "caps_raw_granted",
        &caps_project("caps_raw_granted").join("main.kp"),
        "hello caps\ntrue\n",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_position_tables_keep_the_mode_and_the_directory_out_of_the_check() {
    // `fs.open_raw(path, "r")` must be checked on the path and not on the mode
    // (a relative "r" denies), and `process.spawn_raw(dir, program, ...)` on the
    // program and not on the directory. The mirror case is the next test.
    let dir = caps_scratch("raw_positions");
    std::fs::write(dir.join("note.txt"), "note content\n").unwrap();
    assert_project_runs(
        "caps_raw_positions",
        &caps_project("caps_raw_positions").join("main.kp"),
        "true\ntrue\ntrue\n",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_spawn_check_matches_the_program_and_not_the_directory() {
    // The mirror of the case above: the directory is inside the grant and the
    // program is not, so the denial has to name the *program*. Checking
    // argument 0 would have denied the directory and let the program through.
    assert_project_denied_at_runtime(
        "caps_raw_positions_denied",
        &caps_project("caps_raw_positions_denied").join("main.kp"),
        "is outside the process capability granted to module 'spawner'",
    );
}

#[test]
fn a_primitive_checks_its_resource_argument_once() {
    // The eval-once invariant at a primitive call site, the shape
    // `a_gated_resource_argument_is_evaluated_once_and_the_control_is_denied`
    // pins for `fs.read_text`. `mk()` prints on every call and names an
    // in-grant path the first time and `/etc/passwd` the second, so a stdout
    // other than one `SIDE` and the in-grant file's length is the check and the
    // call reading different evaluations. The control's denial is asserted
    // whole, so the path, the location and the `granted:` block are pinned.
    let dir = caps_scratch("raw_double_eval");
    std::fs::write(dir.join("a"), "FILECONTENT").unwrap();
    let target = caps_project("caps_raw_double_eval").join("main.kp");
    let bin = out_path("caps_raw_double_eval");
    let build = kelpc()
        .arg("build")
        .arg(&target)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected project 'caps_raw_double_eval' to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&bin).output().expect("failed to run the binary");
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "SIDE\n11\n",
        "the primitive's resource argument was not evaluated exactly once"
    );
    assert_eq!(run.status.code(), Some(1), "the control must be denied");
    let expected = format!(
        "{}:20:35: panic: '/etc/passwd' is outside the fs capability granted to module 'util'\n  \
         granted:\n    {}/kelp_caps_raw_double_eval/**\n",
        caps_project("caps_raw_double_eval").join("util.kp").display(),
        scratch_root().display()
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stderr),
        expected,
        "the primitive's capability denial is not what the compiler writes"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_ungranted_module_cannot_name_a_gated_item_as_a_function_value() {
    // A function value names the item without a call site, and before the fix
    // `check_capability` was reached only from the direct-call arm, so both
    // spellings built and read `/etc/hostname` with no grant. The domain gate
    // is the half that can apply to a name, and it applies to both.
    assert_project_rejected(
        "caps_value_ungranted",
        &project("caps_value_ungranted").join("main.kp"),
        "needs the 'fs' capability, which module 'util' was not granted",
    );
    assert_project_rejected(
        "caps_raw_value_ungranted",
        &project("caps_raw_value_ungranted").join("main.kp"),
        "needs the 'fs' capability, which module 'util' was not granted",
    );
}

#[test]
fn an_ungranted_module_cannot_name_a_gated_modules_receiver_method() {
    // A method of a gated module reaches the syscall without naming any of the
    // module's free functions, so the gate has to be on the method's own key
    // (`Type#method`). Before the fix there was no gate check on the method path
    // at all, and `Child.kill(0)` ran a real `kill(2)` with no grant.
    assert_project_rejected(
        "caps_method_ungranted",
        &project("caps_method_ungranted").join("main.kp"),
        "needs the 'process' capability, which module 'util' was not granted",
    );
    // The same rule through another gated module's methods, so this is the gate
    // table's answer and not a special case for `std/process`.
    assert_project_rejected(
        "caps_method_net_ungranted",
        &project("caps_method_net_ungranted").join("main.kp"),
        "needs the 'net' capability, which module 'util' was not granted",
    );
}

#[test]
fn a_granted_module_calls_a_gated_receiver_method_with_no_pattern() {
    // The positive control: `caller` holds `process` with no program at all,
    // because `Child.wait`/`Child.is_running` name no resource the grant
    // governs. A false denial here would leave every tool that manages its own
    // children unbuildable.
    //
    assert_project_runs(
        "caps_method_granted",
        &project("caps_method_granted").join("main.kp"),
        "true\n",
    );
}

#[test]
fn a_pd_initializer_outside_the_grant_is_denied_at_runtime() {
    // The initializer is checked at compile time against its module's grant and
    // has to be checked at run time against the same module's patterns. The
    // denial is pinned whole here, including the module name and the `granted:`
    // block from `util`'s own allowlist.
    let target = caps_project("pd_gated_denied").join("main.kp");
    let bin = out_path("pd_gated_denied");
    let build = kelpc()
        .arg("build")
        .arg(&target)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected project 'pd_gated_denied' to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&bin).output().expect("failed to run the binary");
    assert_eq!(run.status.code(), Some(1), "the initializer must be denied");
    let expected = format!(
        "{}:3:21: panic: '/etc/passwd' is outside the fs capability granted to module 'util'\n  \
         granted:\n    {}/kelp_caps_pd_gated_denied/**\n",
        caps_project("pd_gated_denied").join("util.kp").display(),
        scratch_root().display()
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stderr),
        expected,
        "the `pd` initializer's capability denial is not what the compiler writes"
    );
}

#[test]
fn a_pd_initializer_inside_the_grant_still_runs() {
    let dir = caps_scratch("pd_gated_granted");
    std::fs::write(dir.join("note.txt"), "x").unwrap();
    assert_project_runs(
        "pd_gated_granted",
        &caps_project("pd_gated_granted").join("main.kp"),
        "true\n",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_granted_module_creates_a_symlink_inside_its_grant() {
    // `fs.symlink` checks *both* of its paths, so a link can only be created
    // where the grant reaches and can only point where the grant reaches. The
    // project proves the link is a link (removing the target dangles it), then
    // removes it through `fs.remove`, whose check resolves the link's own path.
    let dir = caps_scratch("link");
    assert_project_runs(
        "caps_link",
        &caps_project("caps_link").join("main.kp"),
        "true\nhello link\nfalse\n0\n",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_symlink_whose_target_is_outside_the_grant_cannot_be_created() {
    // The link's own path is inside the grant; the target is not, and the
    // denial names the *target* — the check runs in argument order, and it is
    // what keeps a grant closed under link creation.
    let dir = caps_scratch("link_denied");
    assert_project_denied_at_runtime(
        "caps_link_denied",
        &caps_project("caps_link_denied").join("main.kp"),
        "'/etc/passwd' is outside the fs capability granted to module 'util'",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_granted_env_reads_sets_and_unsets_only_the_granted_names() {
    // MK5-S5: the bodies are real now. `HOME` is read through the grant, and
    // `KELP_ENV_PROBE` is set, read back, unset and confirmed gone — all inside
    // the child process, so the harness's environment is untouched.
    assert_project_runs(
        "caps_env",
        &project("caps_env").join("main.kp"),
        "true\ntrue\ntrue\n",
    );
}

#[test]
fn an_ungranted_env_name_is_denied_before_the_call() {
    // The name is the governed resource: a grant on PATH does not let the
    // holder ask whether HOME exists, so the panic names the attempted name
    // and the granted pattern.
    assert_project_denied_at_runtime(
        "caps_env_denied",
        &project("caps_env_denied").join("main.kp"),
        "'HOME' is outside the env capability granted to module 'util'",
    );
}

// ---------------------------------------------------------------------------
// perf-cap-llvm-inline: `CapCheck::Name`'s two fast paths, hand-built as
// inline LLVM IR in `codegen_llvm.rs::emit_cap_checks` instead of a call into
// `cap_glue.c`'s separately-compiled `kelp_cap_check_name`. These are
// functional tests (allow and deny, run through the default `llvm` backend);
// the generated-IR shape itself (no `call ... @kelp_cap_check_name` left at
// the specialized call site) is inspected by hand, not asserted here, the
// same way `a_full_grant_elides_the_runtime_path_check_but_a_narrow_one_keeps_it`
// reads generated text only for the C-backend elision it is about.
// ---------------------------------------------------------------------------

#[test]
fn the_lone_wildcard_name_grant_admits_a_slash_free_name() {
    // `env = ["*"]` is the domain's whole-resource pattern. `cap_glue.c`'s own
    // fast path (and now the LLVM backend's inline IR) reads it as "the name
    // has no `/`", so a plain name passes.
    assert_project_runs(
        "caps_name_wildcard_allowed",
        &project("caps_name_wildcard_allowed").join("main.kp"),
        "true\n",
    );
}

#[test]
fn the_lone_wildcard_name_grant_denies_a_name_with_a_slash() {
    // The same `*` grant does not cross a `/` byte: a name that has one is
    // denied even though `*` is the whole-resource pattern for the domain.
    assert_project_denied_at_runtime(
        "caps_name_wildcard_denied",
        &project("caps_name_wildcard_denied").join("main.kp"),
        "'has/nested' is outside the env capability granted to module 'util'",
    );
}

#[test]
fn a_single_literal_name_grant_admits_the_exact_granted_name() {
    // One pattern, no `*` anywhere: the fast path is a direct byte compare
    // against the granted text, and the exact name it names passes.
    assert_project_runs(
        "caps_name_literal_allowed",
        &project("caps_name_literal_allowed").join("main.kp"),
        "true\n",
    );
}

#[test]
fn a_single_literal_name_grant_denies_a_name_that_is_only_a_prefix_match() {
    // `PATH` granted, `PATHX` asked for: a naive prefix compare would let
    // this through. The fast path compares the full length AND confirms
    // `name[pattern.len()]` is the terminator, so a longer name sharing the
    // pattern as a prefix is still denied, not silently admitted.
    assert_project_denied_at_runtime(
        "caps_name_literal_prefix_denied",
        &project("caps_name_literal_prefix_denied").join("main.kp"),
        "'PATHX' is outside the env capability granted to module 'util'",
    );
}

#[test]
fn time_and_rand_are_gated_domains_with_no_named_resource() {
    assert_compiles_and_runs(
        "tests/fixtures/caps_time_rand.kp",
        "true\ntrue\ntrue\n4\nslept\n",
    );
}

#[test]
fn path_normalize_is_the_function_the_capability_checker_uses() {
    assert_compiles_and_runs(
        "tests/fixtures/path_normalize.kp",
        "/a/b/c\n/a/c\n/a\n/tmp/scratch\n",
    );
}

// ---------------------------------------------------------------------------
// M6.3: `const`, stdlib-only argument-type overloading, and `&str` return
// elision (which is what the rest of the `str`/`path` surface needed).
// ---------------------------------------------------------------------------

#[test]
fn const_declarations_fold_at_compile_time() {
    assert_compiles_and_runs(
        "tests/fixtures/consts.kp",
        "42\n1.500000\n84\nx\ntrue\n",
    );
}

#[test]
fn std_math_overloads_resolve_on_argument_type() {
    assert_compiles_and_runs(
        "tests/fixtures/stdlib_math.kp",
        "3.141593\n6.283185\n7\n7.500000\n3\n9\n10\n1.414214\n2.000000\n-1.000000\ntrue\nfalse\n",
    );
}

/// MK7 S3A: the `extern fun` bodies in one program, one per C signature shape
/// they have (a scalar return, a 24-byte `Bytes`/`String` returned through the
/// hidden pointer, a `&str` return, a `*i64` out-parameter, and a body that
/// calls into `std/rt`). This suite is backend-agnostic, so it validates the C
/// backend's copy of the same bodies and the LLVM backend's declared calls to
/// them against one literal output.
#[test]
fn extern_bodies_lower_with_their_c_signature() {
    assert_compiles_and_runs(
        "tests/fixtures/extern_signatures.kp",
        "1.414214\n1024.000000\n2.000000\nfalse\nhi\nff\n3.14\nello\n42\ntrue\n/a/b/c\n/a/c\n",
    );
}

#[test]
fn str_views_borrow_and_owned_truncations_allocate() {
    assert_compiles_and_runs(
        "tests/fixtures/string_views.kp",
        "hello\n5\nhello  \n7\nello\ntrue\ntrue\ntrue\ntrue\nfalse\n2\ntrue\nraw\n5\nown\n",
    );
}

#[test]
fn path_components_split_a_path() {
    assert_compiles_and_runs(
        "tests/fixtures/path_parts.kp",
        "notes.txt\ntxt\n/tmp\nbare.txt\ntxt\n\n/var/log\ntrue\n",
    );
}

#[test]
fn a_str_becomes_a_string_where_one_is_expected() {
    assert_compiles_and_runs(
        "tests/fixtures/str_to_string_coercion.kp",
        "a literal\nassigned\nt\nb\npushed\nv\nthen\nsome\na title\nhey!\n7\ntrue\n1\n",
    );
}

/// `str.less` is the ordering `str` never had, and `str.is_digit` is the digit
/// predicate that four libraries in the catalogue wrote by hand.
///
/// `str` refuses `==`, `<` and the rest (they would compare pointers — see
/// `str_comparison.kp` next door), and `std/str` offered no comparison instead, so
/// `fpl/json` carries a private `str_less` for its key sorting and `fpl/json`,
/// `fpl/decimal`, `fpl/encoding` and `fpl/httpx` each have their own digit test.
/// This fixture is the two functions used the way a caller uses them — a number
/// parser that stops at the first non-digit, and an insertion sort — plus the two
/// rules their doc comments commit to, because a codepoint comparison instead of a
/// byte comparison would pass a predicate test and silently change a sort.
#[test]
fn a_str_is_ordered_by_byte_and_a_digit_is_decimal() {
    assert_compiles_and_runs(
        "tests/fixtures/std_str_order.kp",
        "true\nfalse\nfalse\ntrue\nfalse\ntrue\nfalse\ntrue\ntrue\ntrue\ntrue\ntrue\nfalse\nfalse\nfalse\nfalse\n12\n2\napple\nfig\npear\n",
    );
}

#[test]
fn a_view_carries_its_length_and_slices_without_allocating() {
    assert_compiles_and_runs(
        "tests/fixtures/view.kp",
        "the quick brown fox\n19\nfalse\ntrue\ntrue\nquick\nthe\nhe quick brown fox\ntrue\ntrue\nfalse\ntrue\nthe\nhe quick brown fox\n3\ntrue\n0\ntrue\nquick\nthe\nowned text\ntext\nquick brown\n11\n",
    );
}

#[test]
fn a_view_needs_an_earlier_owner_in_its_datac() {
    assert_rejected_with(
        "tests/fixtures/view_field.kp",
        "construct it from a string literal or from an earlier String/Bytes field",
    );
}

#[test]
fn a_view_cannot_be_a_list_element() {
    assert_rejected("tests/fixtures/view_element.kp");
}

#[test]
fn a_returned_view_must_come_from_a_reference_parameter() {
    assert_rejected("tests/fixtures/view_returned_from_local.kp");
}

#[test]
fn a_view_of_a_temporary_bound_to_a_local_is_allowed() {
    assert_compiles_and_runs("tests/fixtures/view_of_temporary.kp", "x\n");
}

#[test]
fn a_literal_takes_its_sibling_operands_type_on_either_side() {
    assert_compiles_and_runs(
        "tests/fixtures/literal_operand_typing.kp",
        "true\n4\n4\n3000000005\n3000000005\n-2\n-2\nfalse\n240\n240\npositive\n55\n55\n241\n",
    );
}

#[test]
fn a_const_is_not_a_literal_for_operand_typing() {
    assert_rejected("tests/fixtures/literal_const_is_not_a_literal.kp");
}

#[test]
fn bitwise_operators_shift_mask_and_complement() {
    assert_compiles_and_runs(
        "tests/fixtures/bitwise.kp",
        "8\n14\n6\n243\n16\n64\n32\nfalse\ntrue\n228\n184\n173\ntrue\n1\n-1\n-4\n",
    );
}

#[test]
fn bitwise_operators_on_bool_are_rejected() {
    assert_rejected("tests/fixtures/bitwise_bool.kp");
}

/// `0x` and `0b` literals (A-9). They are the same values as their decimal
/// spellings, so the assertion is the masks and the encoder constants a
/// byte-level program writes, not the spellings themselves.
#[test]
fn hexadecimal_and_binary_literals_are_ordinary_integers() {
    assert_compiles_and_runs(
        "tests/fixtures/int_literals.kp",
        "255\n10\n511\ntrue\n196\n184\n173\n1\n0\n255\n4294967295\n2147483647\n4294967295\ntrue\n",
    );
}

/// A malformed radix literal is named at its own span rather than split into
/// two tokens: `0x` needs a digit, and `0b12` has a digit binary does not
/// have.
#[test]
fn a_malformed_radix_literal_is_rejected_by_name() {
    assert_rejected_with(
        "tests/fixtures/hex_literal_without_digits.kp",
        "needs at least one hex digit",
    );
    assert_rejected_with(
        "tests/fixtures/binary_literal_with_a_decimal_digit.kp",
        "invalid digit '2' in a binary literal",
    );
}

#[test]
fn float_arithmetic_comparisons_and_unordered_nan() {
    assert_compiles_and_runs(
        "tests/fixtures/asm_float_scalars.kp",
        "true\ntrue\ntrue\ntrue\ntrue\ntrue\ntrue\ntrue\nfalse\ntrue\nfalse\nfalse\nfalse\nfalse\n",
    );
}

#[test]
fn a_view_keeps_its_provenance_through_a_pattern_binding() {
    assert_compiles_and_runs(
        "tests/fixtures/borrow_provenance_and_arms.kp",
        "hello\ntrue\nfalse\nfalse\nhello\nnone\n42\ntrue\nfalse\n",
    );
}

/// A function that returns a borrow other than `&str`: a `datac`, a `String`, a
/// `Bytes`, and a returned `if` whose arms both derive from the reference
/// parameter. Each is a shape a return-class classifier can get wrong, so the
/// stdout of all four is pinned.
#[test]
fn a_function_may_return_a_borrow_of_any_type() {
    assert_compiles_and_runs(
        "tests/fixtures/borrow_returned.kp",
        "3,4\n3\n4\nhello\n5\n3\n",
    );
}

#[test]
fn character_literals_take_the_type_their_context_asks_for() {
    assert_compiles_and_runs(
        "tests/fixtures/char_literals.kp",
        "34\n48\n53\n10\n9\n92\n39\ntrue\ntrue\ntrue\ntrue\nfalse\ntrue\nfalse\ntrue\ntrue\nfalse\n122\n",
    );
}

/// Byte literals (H-11). The assertion is the byte values and the escapes,
/// including the `\xNN` pair that agrees across a byte and a character
/// literal.
#[test]
fn byte_literals_are_u8_and_carry_hex_escapes() {
    assert_compiles_and_runs(
        "tests/fixtures/byte_literals.kp",
        "10\n13\n9\n34\n92\n39\n0\n32\n255\ntrue\ntrue\ntrue\nfalse\n4\nAbc\ntrue\n233\ntrue\n48\n49\n50\n",
    );
}

/// `isize` is `ptrdiff_t`, and the C backend emitted that name without
/// including `<stddef.h>`, so every shape of the type failed to compile. One
/// shape per way a type can appear, which is why this is a fixture and not a
/// one-line repro: a local, a parameter, a return, a `datac` field and a `List`
/// element.
#[test]
fn isize_works_in_every_position_a_type_can_appear_in() {
    assert_compiles_and_runs(
        "tests/fixtures/isize_kinds.kp",
        "6\n-4\n-3\n7\n-1\n1\n-7\ntrue\ntrue\ntrue\ntrue\n",
    );
}

/// A byte literal is one ASCII character or a `\xNN` escape, it is a `u8` and
/// not the type its context asks for, and the escape is exactly two hex
/// digits in both literal forms.
#[test]
fn a_byte_literal_is_one_byte_and_a_u8() {
    assert_rejected_with(
        "tests/fixtures/byte_literal_holds_two_bytes.kp",
        "a byte literal holds exactly one byte",
    );
    assert_rejected_with(
        "tests/fixtures/byte_literal_is_not_ascii.kp",
        "a byte literal is one ASCII character or a '\\xNN' escape",
    );
    assert_rejected_with(
        "tests/fixtures/byte_literal_does_not_widen.kp",
        "a byte literal is a 'u8', not a 'i64'",
    );
    assert_rejected_with(
        "tests/fixtures/hex_escape_needs_two_digits.kp",
        "a hex escape is written '\\xNN', with two hex digits",
    );
    assert_rejected_with(
        "tests/fixtures/string_hex_escape_is_not_text.kp",
        "not a text character",
    );
}

#[test]
fn a_character_literal_too_large_for_its_type_is_rejected() {
    assert_rejected("tests/fixtures/char_literal_does_not_fit.kp");
}

#[test]
fn integer_to_float_conversions_widen_and_round() {
    assert_compiles_and_runs(
        "tests/fixtures/float_conversions.kp",
        "16777217.000000\n16777216.000000\n-7.000000\n-3.500000\n200.000000\n200.000000\n3000000000.000000\n0.500000\n",
    );
}

#[test]
fn printing_a_string_or_a_float_uses_the_right_register_file() {
    // `%s` reads a pointer out of the *integer* register file and `%f` reads a
    // value out of `xmm`, a variadic call says how many vector registers it used
    // in `al`, and the two files are numbered independently, so a template
    // interpolating one of each is where getting that wrong is silent rather
    // than a crash. The u64 cases are the other half: bit 63 set is the one
    // integer-to-float conversion a plain conversion alone gets wrong.
    assert_compiles_and_runs(
        "tests/fixtures/asm_float_print.kp",
        "2.500000\n0.250000\n0.125000\nn=7 half=2.500000 ok=false s=kelp\n2.500000 0.250000\n\
         18446744073709551616.000000\n9223372036854775808.000000\n9223372036854775808.000000\n\
         kelp\nw=kelp\n",
    );
}

#[test]
fn an_integral_float_literal_stays_a_float() {
    // A wide literal used to make `cc` reject the file (it was emitted as an
    // integer constant), and `-0.0` used to reach C as the *integer* zero, so
    // the sign was gone and an expression of two such literals was evaluated
    // in integer arithmetic.
    assert_compiles_and_runs(
        "tests/fixtures/float_integral_literal.kp",
        "100000000000000000000\n2.0\nnegative zero\nnegative product\n4.500000\n",
    );
}

#[test]
fn a_non_finite_float_literal_emits_a_math_h_constant() {
    // An overflowing literal is infinity and a folded `0.0 / 0.0` is a NaN;
    // neither has a literal spelling, and emitting `Display`'s `inf`/`NaN`
    // does not compile.
    assert_compiles_and_runs(
        "tests/fixtures/float_nonfinite_literal.kp",
        "infinite\nnan\n",
    );
}

#[test]
fn returning_a_borrow_of_the_wrong_parameter_is_rejected() {
    assert_rejected("tests/fixtures/elision_wrong_param.kp");
}

#[test]
fn returning_a_view_of_a_local_is_rejected() {
    assert_rejected("tests/fixtures/elision_not_from_param.kp");
}

#[test]
fn a_live_view_keeps_its_owner_pinned() {
    assert_rejected("tests/fixtures/view_pins_owner.kp");
}

#[test]
fn a_view_releases_its_owner_at_scope_end() {
    assert_compiles_and_runs(
        "tests/fixtures/view_released_at_scope_end.kp",
        "hi\nhi\n",
    );
}

#[test]
fn comparing_str_values_is_rejected() {
    assert_rejected("tests/fixtures/str_comparison.kp");
}

#[test]
fn a_const_that_does_not_fold_is_rejected() {
    assert_rejected("tests/fixtures/const_not_constant.kp");
}

// ---------------------------------------------------------------------------
// A-18: a fixed-size, stack-allocated array; A-19: compile-time evaluation
// ---------------------------------------------------------------------------

#[test]
fn a_fixed_size_array_is_a_value_and_its_length_is_in_its_type() {
    // `.len()` is the length the type already carries, `.get`/`.set` are
    // bounds-checked against it, a nested array is held inline, and `for`
    // walks one because its desugar is exactly `.len()` plus `.get(i)`.
    assert_compiles_and_runs(
        "tests/fixtures/array_basic.kp",
        "3\n20\nfalse\n7\n6\n2\n2\n2\n",
    );
}

#[test]
fn an_array_element_may_own_memory_a_datac_or_another_array() {
    // `[Gate; 4]` as a `datac` field is the IDT/GDT shape, `[String; 3]`
    // makes the array own its elements (so `set` frees the one it replaces,
    // and a `datac` holding one owns it transitively), a `mut` parameter writes
    // through the borrow while a `move` one takes the value over, an
    // owned-element array is borrowed by a plain parameter (so the caller's
    // array survives the call), and `Box<[i32; 3]>` is a fixed-size buffer kept
    // past its frame.
    assert_compiles_and_runs(
        "tests/fixtures/array_owned.kp",
        "9 2\n6\nbee\n3\n2\n3\n9\n2\n9\n",
    );
}

/// A-18 with the two element kinds the scalar array fixtures do not reach: a
/// float element (`[f64; 2]`, whose element classifier has to answer `SSE`
/// rather than the heap routines' integer default) and an array wide enough to
/// be `MEMORY` class in both directions (`[u8; 300]` as an argument and as a
/// return), plus `[[i32; 5]; 2]`, whose twenty-byte *element* is `MEMORY` class
/// itself and therefore comes back through the hidden pointer, and an
/// owned-element array *literal* handed to a borrow parameter, which is the one
/// drop site an array temporary reaches.
#[test]
fn a_float_element_and_a_memory_class_array_are_values_too() {
    assert_compiles_and_runs(
        "tests/fixtures/array_wide_and_float.kp",
        "1.500000\n2.250000\n3.750000\n4.500000\n9\n9\n7\n10\n4\n3\n",
    );
}

#[test]
fn an_out_of_bounds_array_read_panics() {
    assert_fixture_panics("array_bounds", "Array.get: index out of bounds");
}

#[test]
fn an_out_of_bounds_array_write_panics_before_the_store() {
    assert_fixture_panics("array_bounds_set", "Array.get: index out of bounds");
}

#[test]
fn an_array_length_that_is_not_a_constant_is_rejected() {
    assert_rejected_with(
        "tests/fixtures/array_length_is_a_constant.kp",
        "is not a constant here",
    );
}

#[test]
fn an_array_literal_of_the_wrong_length_is_rejected() {
    assert_rejected_with(
        "tests/fixtures/array_literal_length_mismatch.kp",
        "element(s) is expected here",
    );
}

#[test]
fn a_repeat_literal_over_an_owning_element_is_rejected() {
    // `[x; N]` copies `x` into every slot, so an element that owns memory
    // would end up with N owners of one buffer.
    assert_rejected_with(
        "tests/fixtures/array_repeat_owns_memory.kp",
        "needs an element that owns nothing",
    );
    assert_rejected_with(
        "tests/fixtures/array_repeat_owned_datac.kp",
        "needs an element that owns nothing",
    );
}

#[test]
fn a_borrow_cannot_be_an_array_element() {
    assert_rejected_with(
        "tests/fixtures/array_of_a_borrow.kp",
        "cannot be a container element",
    );
}

#[test]
fn a_container_cannot_be_an_array_element() {
    // A-10 slice 1 widened the `List` element set, not the array one: an array
    // is fixed-size, so a `List`/`Box`/`Map` element stays refused there.
    assert_rejected_with(
        "tests/fixtures/array_container_element.kp",
        "'List<i32>' cannot be an array element",
    );
}

#[test]
fn a_const_may_call_a_function_and_the_call_is_evaluated_at_compile_time() {
    // A-19: parameters are bound, a `while` loop runs and a recursive call
    // terminates, all inside the compiler. The result is a literal at every use
    // site, so no backend sees the call.
    assert_compiles_and_runs(
        "tests/fixtures/const_fn.kp",
        "24\n120\n128\n144\n",
    );
}

// ---------------------------------------------------------------------------
// A-20: const type parameters, and `impl Trait` in argument position. Every
// fixture here runs on the default (LLVM) backend and on the C oracle, because
// the suite is run twice.
// ---------------------------------------------------------------------------

#[test]
fn a_const_parameter_is_a_value_a_length_and_a_repeat_count() {
    // `Buf<const N: usize>` infers `N` from its construction, `sum<const N>`
    // infers `N` from an array argument's own length, `N` is read as a value
    // (`i < N`), used as an array length (`[i32; N]`) and used as a repeat
    // count (`[0; N]`), and a `bool` const parameter is read too.
    assert_compiles_and_runs(
        "tests/fixtures/const_generics_basic.kp",
        "3\n3\n6\n100\n7\n3\n0\n15\n1\n5\n",
    );
}

#[test]
fn two_const_instantiations_of_one_ring_buffer_keep_their_own_state() {
    // `Ring<4>` and `Ring<8>` are two monomorphized types, so the loop writes
    // two independent buffers and each keeps its own head and count.
    assert_compiles_and_runs(
        "tests/fixtures/const_generics_two_sizes.kp",
        "4\n8\n30\n44\n4\n8\n",
    );
}

#[test]
fn a_const_argument_may_be_a_compile_time_expression() {
    // `Buf<SIZE * 2>`, `Buf<WIDTH>` where `WIDTH` is a `const` computed by a
    // plain `fun`, and `capacity<twice(n = 4)>()`: the existing constant
    // evaluation is the whole of it.
    assert_compiles_and_runs(
        "tests/fixtures/const_generics_expr_arg.kp",
        "6\n6\n6\n8\n",
    );
}

#[test]
fn a_const_length_mismatch_names_both_lengths() {
    assert_rejected_with(
        "tests/fixtures/const_generics_mismatch.kp",
        "[i32; 3]",
    );
    assert_rejected_with(
        "tests/fixtures/const_generics_mismatch.kp",
        "[i32; 4]",
    );
}

#[test]
fn arithmetic_on_a_const_parameter_in_a_type_is_refused() {
    assert_rejected_with(
        "tests/fixtures/const_generics_arithmetic.kp",
        "arithmetic on one in a type",
    );
}

#[test]
fn impl_trait_arguments_are_fresh_bounded_type_parameters() {
    // `fun pair_area(a: impl Shape, b: &impl Shape)` is two parameters, so two
    // different types may be passed, and the trait's methods dispatch
    // statically inside the body.
    assert_compiles_and_runs(
        "tests/fixtures/impl_trait_args.kp",
        "19\nsquare\nrect\n",
    );
    // The shape A-12 recorded as "a later slice" now compiles and runs with no
    // output: `impl Trait` is a parameter type, not a refusal.
    assert_compiles_and_runs("tests/fixtures/traits_impl_trait_argument.kp", "");
}

#[test]
fn impl_trait_in_return_position_is_refused() {
    assert_rejected_with(
        "tests/fixtures/impl_trait_return_refused.kp",
        "not allowed in return position",
    );
}

#[test]
fn impl_trait_in_a_field_is_refused() {
    assert_rejected_with(
        "tests/fixtures/impl_trait_field_refused.kp",
        "only legal as a whole parameter type",
    );
}

/// The two instantiations of one const-generic buffer are two copies, read
/// back out of the generated C: this is a codegen-shape assertion, so it pins
/// `KELPC_ORACLE=c`, whose `<out>.c` is the artifact under test. The LLVM half
/// of the same claim is the freestanding `llvm-nm` test below, which finds both
/// mangled names in the bare-metal image.
#[test]
fn a_const_generics_two_instantiations_emit_two_copies() {
    let bin = out_path("const_generics_two_copies");
    let build = kelpc()
        .env("KELPC_ORACLE", "c")
        .arg("build")
        .arg(fixture("tests/fixtures/const_generics_two_sizes.kp"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected the ring fixture to compile on the oracle, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let generated = std::fs::read_to_string(bin.with_extension("c"))
        .expect("the generated C sits beside the binary");
    for name in [
        "kelp_Ring__push__l__4__g__",
        "kelp_Ring__push__l__8__g__",
        "kelp_Ring__total__l__4__g__",
        "kelp_Ring__total__l__8__g__",
    ] {
        assert!(
            generated.contains(name),
            "the generated C must hold a copy for each const value, and '{name}' is missing:\n{generated}"
        );
    }
}

/// A-20's freestanding half: a const-generic ring buffer in the kernel image
/// links with no libc symbol, and both `Ring<4>` and `Ring<8>` are emitted (the
/// same acceptance test as `freestanding_min`, run on the const-generic
/// project). It needs the `dev-kernel.nix` tools; a shell without them prints a
/// skip.
#[test]
fn a_freestanding_const_generic_buffer_links_with_no_undefined_symbols() {
    if !have_ld_lld() {
        println!(
            "skipping the freestanding const-generic test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = out_path("freestanding_constgen");
    let build = Command::new(kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .env_remove("KELPC_ORACLE")
        .arg("build")
        .arg(project("freestanding_constgen"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the freestanding const-generic build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let path = bin.to_str().expect("the artifact path is utf-8");
    let header = tool("llvm-readelf", &["-h", path]);
    assert!(header.contains("RISC-V"), "not a RISC-V image:\n{header}");
    let symbols = tool("llvm-nm", &[path]);
    for name in [
        "kelp_Ring__push__l__4__g__",
        "kelp_Ring__push__l__8__g__",
    ] {
        assert!(
            symbols.contains(name),
            "the freestanding image must hold one copy per const value, and '{name}' is missing:\n{symbols}"
        );
    }
    let undefined: Vec<&str> = symbols
        .lines()
        .filter(|line| line.split_whitespace().next() == Some("U"))
        .collect();
    assert!(
        undefined.is_empty(),
        "the freestanding image has undefined symbols:\n{}",
        undefined.join("\n")
    );
}

#[test]
fn a_modules_own_items_are_reachable_bare_inside_it() {
    // `util.kp` calls `doubled` and constructs `Pair` without qualifying
    // either, even though `resolve.rs` stores both prefixed.
    assert_project_runs(
        "self_call",
        &project("self_call").join("main.kp"),
        "12\n3\n",
    );
}

/// The M6 gate, run rather than argued: every `Bytes` fixture is rebuilt with
/// AddressSanitizer and executed, so a double free, a use-after-free, or a
/// leak fails the suite. If `cc` has no AddressSanitizer the check is skipped
/// — the one place in this suite that depends on a toolchain feature.
///
/// It works by rebuilding `kelpc`'s generated `.c` with `cc -fsanitize=address`,
/// so it is specific to the C backend: another backend writes no `.c` for it to
/// instrument, and `memory_gate.rs`'s valgrind sweep is the gate that covers a
/// backend writing its own object instead.
#[test]
fn heap_fixtures_are_double_free_and_leak_clean() {
    let fixtures = [
        "heap_bytes",
        "heap_move_then_rebind",
        "heap_returned",
        "heap_scope_drop",
        "heap_drops_in_control_flow",
        "heap_drops_in_desugared_blocks",
        "heap_reassign",
        "heap_branches",
        "heap_param_borrow",
        "string_basic",
        "string_build",
        "bytes_build",
        "stdlib_build",
        "stdlib_parse",
        "stdlib_fmt",
        "string_field",
        "datac_owned_fields",
        "pd_owned",
        // A passdown forwarded through a caller that does not name it: the
        // hidden argument is the global's address there, while an owned
        // passdown still has to be freed at exit by exactly one handler.
        "pd_argument_forwarding",
        "stdlib_mem",
        "stdlib_str",
        "stdlib_path",
        "stdlib_math",
        "stdlib_encoding",
        "string_views",
        "path_parts",
        "view_released_at_scope_end",
        "heap_temporary_print",
        // MK7 §5's temporary-drop slice: the field of an owned temporary, read
        // in `print` position, as a call argument, as a method receiver and
        // through a `move` parameter, with a `List` field as well as `String`
        // ones. Every read has to free the whole temporary exactly once, so
        // this is the fixture the staged-and-freed path is leak-checked by.
        "heap_field_of_owned_temporary",
        // The same staged-and-freed temporary as a `when`/`if let` scrutinee,
        // where the field read is the *matched* one and a second owned field
        // is never named at all, so only the staged root's own drop can free
        // it.
        "scrutinee_field_of_temporary",
        // A borrow of a chain that steps through a borrow: nothing is staged
        // for it, so the only thing under test is that the pointee's own drop
        // still frees the `String` exactly once.
        "borrow_chain_through_a_borrow",
        // MK7 §5's print slice: the same staged-and-freed temporary, but one
        // per buffer piece and with a scalar, a float and a second buffer
        // behind the first — the register-numbering case.
        "template_buffer_pieces",
        "list_basic",
        "list_owned",
        "list_field",
        "stdlib_list",
        "borrow_temp_arg",
        "list_datac_element",
        "closure_datac_borrow_param",
        "stdlib_sort",
        "list_slice_insert",
        // MISSING-7: the whole-list copy, over a `Copy` element and an owned
        // one, with the original mutated after the clone, so a second handle
        // would free the buffer twice.
        "list_clone",
        // MISSING-jL9tE: the derived deep copy, with the original cleared
        // before the clone is read, so a shared buffer is a use-after-free and
        // a shallow `sort` that lost an element's buffer is a leak. The enum
        // half of the same fixture is the switch-on-tag clone.
        "list_datac_clone_sort",
        // MK7 §5's sort/contains/slice slice: the element kinds neither of the
        // two above reaches — a signed and an unsigned integer and `Bytes`,
        // whose comparator is a length-aware `memcmp` — plus a slice of a
        // `Copy` element and a slice of an owned one.
        "list_sort_kinds",
        // MK7: the list paths a shift or a replacement could get wrong —
        // `insert`/`remove`/`reverse` over owned elements, six `realloc`s
        // that move every element, and `set` freeing the element it replaces.
        "list_insert_remove_shifts",
        "list_growth_and_rebuild",
        "list_owned_replacement",
        // A-18: an array is a by-value struct, so its drop has to free what it
        // holds. `array_owned` is the one that owns something: three
        // `String`s with one replaced through `set`, a `datac` array, and a
        // `Box<[i32; 3]>`, and `array_basic` is the scalar and nested case
        // that must free *nothing* (a spurious drop there would be a free of
        // stack memory).
        "array_owned",
        "array_basic",
        // A-18's float element and its `MEMORY`-class widths: the second is
        // where a byte count that is off by one would read or write past the
        // end of a frame slot, and it is a value that travels in memory rather
        // than through a register, so ASAN can see it.
        "array_wide_and_float",
        "path_polish",
        "box_basic",
        "box_recursive",
        "recursive_datac",
        "box_field_take",
        "box_owned_elements",
        "map_basic",
        "map_owned",
        "map_datac",
        // MISSING-qM4Zx: `get_or`/`values` deep-copy a `datac` value out of
        // the map, so the value's own `String` is cloned into the result and
        // the map keeps its own. A shallow copy would be a double free at
        // scope exit and a missing one would leak.
        "map_datac_value_clone",
        // A-6: a `datac` key with an owned `String` field and an `enum` key
        // with an owned `String` payload. The entry owns a deep copy of the
        // key, so a drop that missed it is a leak and a double drop is a
        // use-after-free.
        "map_nested_datac_key",
        "map_enum_key",
        // A-12: a declared key pair, whose type holds a `f64` (outside the
        // derived set) and an owned `String` the declared `hash` and `eq`
        // walk. The entry owns a deep copy of the key, so a drop that missed
        // it is a leak and a double drop is a use-after-free.
        "map_declared_key",
        "map_declared_key_enum",
        "io_stdio",
        "io_streams",
        "process_spawn",
        "net_loopback",
        "net_poll",
        "net_resolve",
        // A-1: an owned payload, a recursive `Box` chain, and an enum inside
        // `List`/`Map` — each one a place a tag switch could free the wrong
        // variant or leak one.
        "enum_owned",
        "enum_recursive",
        "enum_container",
        // MK7 §5's enum slice: the same tag switch reached through a `List`
        // element, a `Map` value and a `Box` element, plus an enum nested
        // inside an enum — each one a generated drop calling another one.
        "enum_in_containers",
        // MK7 §5's enum slice: a pre-existing staging bug (a shared slot for a
        // staged aggregate value), found by `generic_recursive` once an `enum`
        // no longer refused it. This fixture owns three nested `Box` chains.
        "nested_literal_staging",
        // A-2: an owned payload in a generic instantiation, and a generic
        // type that recurses through a `Box` — the instantiation's generated
        // drop and the recursive drop path.
        "generic_owned",
        "generic_recursive",
        // A-2: a generic *function* instantiated over an owned type, so the
        // per-instantiation body owns and drops like any other function.
        "generic_function_owned",
        // A-2: a generic *method* over an instantiation holding an owned
        // payload, a tree of instantiations built by nested inference, and a
        // map keyed by a scalar while holding owned values.
        "generic_method_basic",
        "generic_nested_inference",
        "map_key_types",
        // MK5-S6-M1: an owned payload bound by `if let`, owned payloads
        // moved through `?`, and a frame that owns a local it propagates a
        // failure past (`?`'s early return must free the frame).
        "if_let_basic",
        "if_let_chain_same_scrutinee",
        "try_operator",
        "try_drops_frame",
        // MK7 §5's enum slice: the same `?`-frees-the-frame shape as the line
        // above, without the `fun` literal that this backend refuses.
        "try_frame_drops",
        // MK7 §5's enum slice, fixed after: the two wrong-width aggregate
        // loads (a one-eightbyte field out of a `Box` block, a one-eightbyte
        // element out of a `List` buffer) and the `?` whose success payload is
        // a one-eightbyte `datac`. Each one owns a buffer, so each is
        // leak-checked here as well as under valgrind.
        "heap_narrow_fields",
        "list_narrow_element_pop",
        "try_narrow_payload",
        "stdlib_option_result",
        // A-4: a `move T` parameter owns what it is handed, so each buffer is
        // freed exactly once by whichever function owns it at scope exit.
        "move_param",
        "move_param_return",
        // A-4: a function value for a `move` parameter hands a buffer to a
        // callee that frees it, and a passdown function value reads a global.
        "fn_value",
        // A-4: a `fun` literal whose `move` parameter owns and frees a buffer.
        "fn_literal",
        // A-4 phase D: `option`/`result`/`list` callbacks over owned payloads.
        "stdlib_combinators",
        "stdlib_list_callbacks",
        // A-5: `for` over a list of owned elements, plus a call iterable the
        // desugared block owns and frees once.
        "for_loop",
        // A-4 phase E: a closure owns its environment, and the fixture below
        // frees exactly one per capture; `closure_move` covers the three drop
        // points (block end, early `return`, one per loop iteration), and the
        // stdlib fixture hands environments to the shipped callbacks.
        "closure_basic",
        // Trap 1 and trap 8 of MK7 §5 item 6: a two-field environment of
        // differently aligned captures, and an indirect call whose callee
        // returns a `MEMORY`-class value while carrying an environment.
        "closure_env_wide_ret",
        "closure_move",
        "closure_two_captures",
        "closure_value_widening",
        "stdlib_closure_callbacks",
        // M6: the drop-plan defect's own two fixtures. A move only one path
        // takes used to remove the binding from every drop set, so the path
        // that kept the value leaked it; both fixtures are here as well as
        // under `memory_gate.rs`'s valgrind sweep because the leak was
        // *invisible* under AddressSanitizer on the C backend's `-O2` build
        // (clang deletes an allocation nothing reads), and valgrind is the
        // gate that saw it.
        "owned_conditional_move",
        "owned_move_in_loop",
        // BUG-n7Wq3: one spelling as two owned bindings, in one block and in a
        // nested one. Both drops name the binding that owns the buffer, so a
        // build that frees the newest binding twice, or frees only one of
        // them, fails here. Both fixtures read the earlier buffer before the
        // rebind or the shadow, so a missing drop also leaks at the level the
        // valgrind sweep runs, not only below -O2 where an optimizer can still
        // see that nothing reads the allocation.
        "owned_rebind",
        // BUG-tV9qH: a `move` parameter and the body's own owned local are
        // dropped at one block end, including when the local shadows the
        // parameter. One list written over the other leaked a buffer on both
        // backends.
        "move_param_and_body_local",
        "move_param_shadowed_by_a_local",
        // A-11: a partial move zeroes the field's slot, so the binding's own
        // drop frees the field that stayed and nothing where the moved value
        // now lives. A double free of the moved buffer fails here.
        "partial_move_field",
        "partial_move_conditional",
        "partial_move_reassign",
        "partial_move_move_param",
        // The enum-payload half of A-11, and the stdlib helpers that had to
        // take their receiver with `move` for it: each read moves an owned
        // payload out and clears the slot it came from.
        "partial_move_enum_payload_owned",
        "option_unwrap_owned",
        "result_map_owned",
        // MK9-S3-M1 stage A1: an `Atomic<T>` needs no drop, so this is the
        // "must free *nothing*" half of the list, like `array_basic`. Its cell
        // is a `pd val`, so a spurious drop would be a free of a global, and
        // the `compare_exchange` result is a `Result` an owned-value drop plan
        // could get wrong. ASAN is what sees either.
        "atomic_counter",
        "atomic_bool",
        // MISSING-z2bB9: a tuple is a derived record, so its positions own and
        // drop exactly as a `datac` field does. The first is the deep copy of
        // an owned element out of a `List` (the original is cleared first), the
        // second a `String` taken over by a `move` parameter, and the third a
        // `String` dropped as a `Result` payload. Each is a drop the tuple
        // record, rather than a declared type, decided.
        "tuple_derived_copy_sort",
        "tuple_param",
        "tuple_try",
        // MISSING-w9Kda: a fallible dial builds its reason text on the failure
        // path and frees the caller's previous buffer with it, so the error
        // arm owns exactly one allocation and the success arm still owns the
        // empty one it was given. Both arms are leak-checked here and under
        // valgrind. `ws_try_connect` is deliberately absent: it reads a port
        // from `argv`, so on this gate's no-argument run it would panic before
        // reaching either error arm, and the list holds only fixtures that
        // exit 0.
        "net_try_connect",
        "net_try_connect_retry",
        // roadmap open item 15, cases 2 and 3: an assignment through a
        // dereferenced borrow and to an owned field used to leak the value
        // they overwrote, because `DropPlan::at_assign` had no way to key a
        // drop to anything but a plain binding.
        "assign_through_deref",
        "assign_owned_field",
        // Kernel stage 9, MK9-S6-M1: a destructor type that owns a `String`,
        // so the generated `kelp_drop_<T>` must free the field exactly once
        // after the user body runs, and the container/`enum` drops must reach
        // the same place.
        "drop_owned_field",
        "drop_order",
        "drop_control_flow",
        "drop_container",
        "drop_enum",
        "drop_map",
        // A guard owned by a thread closure's heap environment: a worker that
        // never freed the environment would leave its box behind at exit.
        "drop_thread_closure",
        // B-5: the single-threaded mutex paths. The runtime record and its
        // value slot are malloc'd, the block's value is copied out, and
        // `into_inner` moves an owned `List` out and frees the record without
        // dropping the value, so ASAN sees a leak or a double free either way.
        "mutex_lock_value",
        "mutex_lock_return",
        "mutex_into_inner",
        // `std/tz`: a parsed zone owns five `List`s and a `String` table per
        // time type, the formatter builds a `String` per call, and the
        // synthetic fixture builds and drops a dozen `Bytes`. Under
        // AddressSanitizer a double free or a leak in any of them fails here.
        "stdlib_tz_synthetic",
        "stdlib_tz_zones",
        "owned_return_endless_loop",
    ];
    for name in fixtures {
        let source = format!("tests/fixtures/{name}.kp");
        // A distinct output path: the individually-run fixture tests build
        // the same names, and racing them for one file makes the binary
        // unreadable to whichever loses.
        let bin = out_path(&format!("{name}_asan_src"));
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

        let asan_bin = out_path(&format!("{name}_asan"));
        let cc = Command::new("cc")
            .arg("-fsanitize=address")
            .arg("-O1")
            .arg("-o")
            .arg(&asan_bin)
            .arg(bin.with_extension("c"))
            .output()
            .expect("failed to spawn cc");
        if !cc.status.success() {
            continue; // no AddressSanitizer here
        }

        let run = Command::new(&asan_bin)
            .output()
            .expect("failed to run the instrumented binary");
        assert!(
            run.status.success(),
            "AddressSanitizer flagged '{name}':\n{}",
            String::from_utf8_lossy(&run.stderr)
        );
    }
}

// ---------------------------------------------------------------------------
// Struct emission order. `structs` used to be collected from a `HashMap`, so
// a `datac` embedding another one compiled or failed at random. Declaration
// order is a valid topological order because a `datac` may only name datacs
// declared before it.
// ---------------------------------------------------------------------------

#[test]
fn a_datac_can_embed_an_earlier_datac() {
    assert_compiles_and_runs("tests/fixtures/datac_embeds_datac.kp", "8\n");
}

// MK7 §5 item 4, third slice: real SysV eightbyte/MEMORY classification for a
// `datac` over eight bytes.

#[test]
fn a_two_eightbyte_datac_passes_and_returns_in_a_register_pair() {
    assert_compiles_and_runs("tests/fixtures/datac_two_eightbytes.kp", "3\n4\n14\n11\n");
}

#[test]
fn a_memory_class_datac_returns_through_a_hidden_pointer() {
    assert_compiles_and_runs("tests/fixtures/datac_memory_class.kp", "1\n2\n3\n6\n33\n60\n");
}

// MK7 §5 item 4, fourth slice: a `MEMORY`-class `datac` passed and received by
// value, in the caller's outgoing argument area.

#[test]
fn a_memory_class_datac_passes_by_value_in_the_argument_area() {
    assert_compiles_and_runs(
        "tests/fixtures/datac_memory_class_by_value.kp",
        "6\n15\n24\n150\n579\n45\n11\n11\n11\n106\n1234\n13\n",
    );
}

#[test]
fn a_forward_reference_between_datacs_is_rejected() {
    assert_rejected("tests/fixtures/datac_forward_reference.kp");
}

// MK7 §5 item 4, the float-in-aggregate slice: a `datac` or `enum` payload
// holding an `f32`/`f64`, classified per eightbyte. The fixture's stdout covers
// the values; a consistently wrong register file is the failure a literal
// cannot see.

#[test]
fn a_float_field_datac_travels_in_the_vector_register_file() {
    assert_compiles_and_runs(
        "tests/fixtures/float_datac_boundary.kp",
        "3.000000\n4.500000\n4.000000\n3.750000\n2.500000\n7.500000\n2.500000\n7.500000\n\
         7.000000\n6.000000\n10.000000\n7.000000\n2.500000\n",
    );
}

#[test]
fn a_float_field_datac_over_sixteen_bytes_is_memory_class() {
    assert_compiles_and_runs(
        "tests/fixtures/float_datac_memory.kp",
        "6.500000\n5.000000\n2.500000\nkelp\n2.500000\n",
    );
}

#[test]
fn an_enum_payload_eightbyte_is_classified_through_the_union() {
    assert_compiles_and_runs(
        "tests/fixtures/float_enum_register_file.kp",
        "1.500000\n2.500000\n0.000000\n2.500000\n",
    );
}

#[test]
fn a_float_field_datac_crosses_the_hand_written_container_boundary() {
    assert_compiles_and_runs(
        "tests/fixtures/float_datac_containers.kp",
        "1.500000\n2.500000\n2.500000\n3.500000\n4.500000\n4.500000\n3.500000\n5.500000\n",
    );
}

#[test]
fn a_float_field_is_written_through_the_xmm_file() {
    assert_compiles_and_runs(
        "tests/fixtures/float_field_write.kp",
        "3\n2.000000\n3\n4.000000\n9\n4.250000\n",
    );
}

/// A field read of an *owned* temporary, whose temporary has to be dropped
/// once per read.
///
/// Both backends stage the temporary for the field's address and free *it*
/// after the read, so the fixture builds, runs and compares on either one.
/// (The C backend used to emit `&(rvalue).field` and reject the whole shape,
/// which is why this test used to be written the way its fixture was: the
/// refusal under every other backend is what `heap_field_of_owned_temporary`
/// no longer asserts.) This is the fixture the leak was found in: without a
/// test here under *either* backend, the loop below was committed
/// non-terminating and nothing said so. The only thing that noticed was
/// `memory_gate`'s 60 s timeout, which reads as a valgrind finding.
#[test]
fn a_field_of_an_owned_temporary_is_dropped_per_read() {
    assert_compiles_and_runs(
        "tests/fixtures/heap_field_of_owned_temporary.kp",
        "aa\nbb\n2\naa\n4\n3\n2\n2\n",
    );
}

// ---------------------------------------------------------------------------
// Collections: the builtin `List<T>`.
// ---------------------------------------------------------------------------

/// Builds a fixture, runs it, and asserts the run fails with `needle` on
/// stderr — the runtime-panic path (`List.pop` on an empty list).
fn assert_fixture_panics(name: &str, needle: &str) {
    assert_fixture_panics_all(name, &[needle]);
}

/// The same, for a panic whose diagnostic is more than its first line: every
/// needle must appear, in any order. The syscall-failure panics use this,
/// because `errno` and `strerror` are reported below the stable message.
fn assert_fixture_panics_all(name: &str, needles: &[&str]) {
    let source = format!("tests/fixtures/{name}.kp");
    let bin = out_path(name);
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
    let run = Command::new(&bin).output().expect("failed to run the binary");
    assert!(
        !run.status.success(),
        "expected '{name}' to panic at runtime, but it exited successfully"
    );
    let stderr = String::from_utf8_lossy(&run.stderr);
    for needle in needles {
        assert!(
            stderr.contains(needle),
            "expected the panic for '{name}' to mention '{needle}', got:\n{stderr}"
        );
    }
}

#[test]
fn a_list_sorts_reverses_searches_and_slices() {
    assert_compiles_and_runs(
        "tests/fixtures/stdlib_sort.kp",
        "1\n2\n3\n3\n1\ntrue\nfalse\n2\n3\n3\nfalse\ntrue\n0\n",
    );
}

#[test]
fn a_list_clones_without_sharing_its_elements() {
    // MISSING-7: `clone` is the whole-list case of `slice(0, len)`. Mutating
    // the original after the clone is what proves the copy is not a second
    // handle on the same buffer, and the fixture is in the AddressSanitizer
    // pass because a `List<String>` clone that shared an element would
    // double-free at scope end.
    assert_compiles_and_runs(
        "tests/fixtures/list_clone.kp",
        "2\n3\n1\n9\n2\nalpha\nchanged\n0\nalpha-beta\n",
    );
}

#[test]
fn sorting_and_slicing_a_list_of_owned_elements_moves_them_safely() {
    // The element shifts are moves, so this fixture runs under
    // AddressSanitizer: a double free or a leak here would be a real one.
    assert_compiles_and_runs(
        "tests/fixtures/list_slice_insert.kp",
        "apple\nfig\npear\ntrue\nfalse\n2\nfig\n4\nfig\napple\nfig\n3\npear\n",
    );
}

#[test]
fn a_list_sorts_and_searches_every_element_kind_it_can_hold() {
    // MK7 §5's sort/contains/slice slice: each element kind reaches a different
    // comparison (signed, unsigned, and a length-aware byte compare for the
    // owned buffers) and the two slice paths differ (a plain copy for a `Copy`
    // element, a clone for an owned one). A wrong element width or a
    // cloned-twice element is a memory bug rather than a wrong number, so this
    // fixture is also in the AddressSanitizer list and under valgrind.
    assert_compiles_and_runs(
        "tests/fixtures/list_sort_kinds.kp",
        "-7\n0\n3\ntrue\nfalse\n1\n-7\n3\n200\ntrue\nfalse\n\
         apple\nfig\npear\ntrue\nfalse\n2\nfig\n3\n",
    );
}

#[test]
fn list_sort_and_slice_reject_what_they_cannot_do() {
    // A `datac`/`enum` gets its order and its copy from its fields, so an
    // element that holds a `List` has neither and the diagnostic names that
    // rather than failing in codegen; a `slice` range past the end panics at
    // runtime.
    assert_rejected("tests/fixtures/list_unsortable_element.kp");
    assert_fixture_panics("list_slice_bounds", "List.slice: range out of bounds");
}

#[test]
fn list_sort_handles_introsorts_own_trouble_spots() {
    // `List#sort` moved from heapsort to introsort (quicksort with a
    // median-of-three pivot, an insertion-sort cutoff, and a depth-limited
    // heapsort fallback). The existing sort fixtures above were written
    // against heapsort and never stressed quicksort's known trouble spots:
    // an already-sorted run, a reverse-sorted run, an all-equal run, and
    // inputs sized just either side of the insertion-sort cutoff. None of
    // these should crash, hang, or blow the stack, and every one must come
    // out sorted; the depth-limit fallback to the existing heapsort is what
    // is supposed to catch the already-sorted/reverse-sorted/all-equal cases
    // before they could degrade toward O(n^2).
    assert_compiles_and_runs(
        "tests/fixtures/list_sort_introsort_stress.kp",
        "true\n5000\n\
         true\n5000\n\
         true\n5000\n\
         true\n15\n\
         true\n17\n\
         0\n\
         7\n",
    );
}

#[test]
fn a_datac_or_enum_element_gets_a_derived_copy_and_order() {
    // MISSING-jL9tE: `List<Digest>.clone()` and `List<Digest>.sort()` were both
    // "not defined" because a `datac` element had neither. The copy is deep
    // (the original is cleared first) and the order is field by field, with an
    // `enum` ordering by tag before payload.
    assert_compiles_and_runs(
        "tests/fixtures/list_datac_clone_sort.kp",
        "3\nb\na\n1\n3\nb\n2\na\nempty\na\nb\n3\n",
    );
}

#[test]
fn a_list_grows_indexes_and_shrinks() {
    assert_compiles_and_runs(
        "tests/fixtures/list_basic.kp",
        "0\ntrue\n3\nfalse\n1\n3\n1 20 3 \n3\n2\n0\n",
    );
}

#[test]
fn a_for_loop_walks_a_list_without_an_index() {
    // A-5: the parser desugars `for x in xs { }` to an index and a `while`, so
    // this pins the desugaring where a caller can see it — the element binding
    // (value for `Copy`, shared borrow for owned), a call iterable evaluated
    // once, the same once for a *field* of a call, nesting, and a list a
    // function only borrows.
    assert_compiles_and_runs(
        "tests/fixtures/for_loop.kp",
        "6\n3\n7\n8\n9\n4\n5\n1\na\nb\nc\n1 2 3 2 4 6 3 6 9 \n6\n",
    );
}

#[test]
fn a_for_loop_needs_its_in() {
    // `in` is not a keyword (syntax.md 2.3): it is read contextually, so the
    // diagnostic says what is missing instead of resolving a function `in`.
    assert_rejected_with("tests/fixtures/for_missing_in.kp", "'in'");
}

#[test]
fn a_must_use_result_cannot_be_dropped() {
    // MISSING-Cn2Ym: the marker is on `Sender.send` and the `fs.try_*`
    // mutators, and a user function can carry it too.
    assert_rejected_with("tests/fixtures/must_use_send_dropped.kp", "must be used");
    assert_rejected_with("tests/fixtures/must_use_user_dropped.kp", "'check' must be used");
    assert_compiles_and_runs("tests/fixtures/must_use_user_fun.kp", "positive\ntrue\n");
}

#[test]
fn a_for_loop_only_walks_a_list_or_an_iterator() {
    // A `List`/array keeps the index walk; anything else has to implement
    // `std.iter.Iterator`, and the refusal names the trait so the fix is one
    // `impl` away (`plan/iterators.md`). The span is the `for` line.
    assert_rejected_with("tests/fixtures/for_not_a_list.kp", "implements 'Iterator'");
    assert_rejected_with("tests/fixtures/for_iter_no_impl.kp", "implements 'Iterator'");
}

#[test]
fn a_for_loop_refuses_every_mutation_of_what_it_walks() {
    // `for x in xs { xs.push(x) }` was accepted for a `Copy` element (nothing
    // borrowed the list) and grew the list until memory ran out. Every mutation
    // form is now one refusal, naming the place the `for` walks.
    for fixture in [
        "for_mutates_iterated_push",
        "for_mutates_iterated_remove",
        "for_mutates_iterated_set",
        "for_mutates_iterated_assign",
        "for_mutates_iterated_field",
        "for_mutates_iterated_mutref",
        "for_mutates_iterated_mutarg",
        "for_mutates_iterated_capture",
        "for_mutates_iterated_move",
        "for_mutates_iterated_nested",
    ] {
        assert_rejected_with(
            &format!("tests/fixtures/{fixture}.kp"),
            "while iterating over it",
        );
    }
    // The message names the place: a local for a local, a field path for a
    // field.
    assert_rejected_with(
        "tests/fixtures/for_mutates_iterated_push.kp",
        "cannot mutate 'xs' while iterating over it",
    );
    assert_rejected_with(
        "tests/fixtures/for_mutates_iterated_field.kp",
        "cannot mutate 'self.items' while iterating over it",
    );
}

#[test]
fn a_for_loop_still_allows_reads_another_list_and_a_while() {
    // The rule covers mutation of the walked place only: reading it, changing a
    // different list, changing it after the loop, and the index-`while` that is
    // the supported way for a loop to change what it walks all stay legal.
    assert_compiles_and_runs(
        "tests/fixtures/for_iterated_place_is_read_only.kp",
        "2\n2\n5\n10\n20\n3\n3\n1\n",
    );
    assert_compiles_and_runs(
        "tests/fixtures/for_mutates_another_field.kp",
        "2\n7\n",
    );
}

#[test]
fn a_for_loop_over_an_iterator_walks_it_by_next() {
    // `plan/iterators.md`: a type implementing `std.iter.Iterator` walks through
    // `next()`, and `Range` is the shipped one. A step and a descending step are
    // the same loop with a different `step`.
    assert_compiles_and_runs(
        "tests/fixtures/for_iter_range.kp",
        "0\n1\n2\n3\n4\n0\n2\n4\n3\n2\n1\n",
    );
}

#[test]
fn a_user_iterator_loop_stops_on_break_and_skips_on_continue() {
    // The sequence is infinite, so `break` is the loop's exit; `continue`
    // reaches the next `next()` call.
    assert_compiles_and_runs(
        "tests/fixtures/for_iter_break.kp",
        "0\n1\n1\n2\n3\n5\n8\n13\n20\n",
    );
}

#[test]
fn a_defer_in_an_iterator_body_runs_once_per_iteration() {
    assert_compiles_and_runs("tests/fixtures/for_iter_defer.kp", "0 d1 d2 d\n");
}

#[test]
fn two_iterator_loops_nest_with_independent_cursors() {
    assert_compiles_and_runs(
        "tests/fixtures/for_iter_nested.kp",
        "0 1 2 10 11 12 20 21 22 \n",
    );
}

#[test]
fn the_iterator_helpers_count_sum_and_collect() {
    assert_compiles_and_runs(
        "tests/fixtures/for_iter_helpers.kp",
        "7\n10\n3\n0\n1\n2\n",
    );
}

#[test]
fn a_place_iterator_is_borrowed_by_its_loop() {
    // The loop holds `&mut f`, so naming `f` in the body is the ordinary
    // "already borrowed" error — the iterator mutability the design promises.
    assert_rejected_with(
        "tests/fixtures/for_iter_borrowed.kp",
        "already borrowed",
    );
}

#[test]
fn a_list_of_owned_and_datac_elements_moves_them() {
    assert_compiles_and_runs(
        "tests/fixtures/list_owned.kp",
        "2\nalpha beta \nbeta\n1\ntrue\n2\n3\n9\n3\n0\n7\n",
    );
}

#[test]
fn a_list_works_as_a_datac_field_and_as_an_owned_passdown() {
    assert_compiles_and_runs("tests/fixtures/list_field.kp", "bag\n7\n2\n2\n2\n");
}

#[test]
fn a_list_inserts_and_removes_by_shifting_owned_elements() {
    // The shifts are moves of a twenty-four-byte `String`, so this fixture
    // runs under AddressSanitizer: a shift that copied the wrong number of
    // bytes would leak a buffer or double-free one. It is kept apart from
    // `list_slice_insert.kp` because that one reaches `insert`/`remove` only
    // alongside `sort`/`contains`/`slice`.
    assert_compiles_and_runs(
        "tests/fixtures/list_insert_remove_shifts.kp",
        "3\na\nb\nc\nd\nb\n3\na\nc\nd\nd\n2\na\n1\ne\nc\ng\nf\nc\ng\nc\n0\n",
    );
}

#[test]
fn a_list_grows_past_several_doublings_and_reuses_its_buffer() {
    // Three hundred pushes is six `realloc`s, each one moving every element;
    // clearing a list keeps its buffer, so the refilled list lands twice the
    // checksum from the capacity it already had.
    assert_compiles_and_runs(
        "tests/fixtures/list_growth_and_rebuild.kp",
        "300\n44850\n0\n300\n89700\n300\nelement\n0\n598\n",
    );
}

#[test]
fn a_float_list_copies_and_compares_at_its_own_width() {
    // A plain `List<f64>` element: the copy width is eight bytes in a float
    // register, and `contains` compares with a float `==` rather than an
    // integer one. The differential fuzzer found the `llvm` backend emitting
    // an integer compare for the float element, which was invalid IR. The
    // narrow `f32` list below goes through the same path at four bytes.
    assert_compiles_and_runs(
        "tests/fixtures/list_float_element.kp",
        "4\n3.500000\n2.000000\ntrue\nfalse\n1.250000\n3.500000\n4\n2.000000\n\
         2.000000\n9.500000\n9.500000\n0.500000\n3.500000\n2.000000\n\
         2.500000\ntrue\nfalse\n",
    );
}

#[test]
fn a_list_copies_elements_at_their_own_width() {
    // A one-byte `bool`, an eight-byte `i64` and a twelve-byte `Trio` — the
    // last is not a power of two, so copying it is two instructions rather
    // than one, and every element this corpus had before was four or
    // thirty-two bytes wide.
    assert_compiles_and_runs(
        "tests/fixtures/list_element_widths.kp",
        "3\n5000000002\n15000000003\n7\n3\n2\nfalse\n3\n4\n9\n10\n7\n5\n2\n9\n0\n",
    );
}

#[test]
fn a_narrow_aggregate_field_is_read_at_its_own_width() {
    // The regression fixture for a wrong-width load in an aggregate field read:
    // the read loaded a whole `QWORD` whatever the field's own width was. A
    // padded stack slot absorbs the extra bytes, so the printed number came out
    // right and a stdout comparison could not see it, but a field read out of a
    // `Box` block, which is `sizeof(Outer)` bytes and nothing more, made the
    // load cross the allocation, which the valgrind sweep reports.
    // Field widths 1, 2, 3, 4, 5 and 7 are here (the ones the machine cannot
    // move in one instruction are 3, 5 and 7), each read whole and byte by
    // byte, plus an 8-byte field, for which the single load is right.
    assert_compiles_and_runs(
        "tests/fixtures/heap_narrow_fields.kp",
        "2\n2\n302\n23\n40302\n432\n5040302\n5\n605040302\n6\n8070605040302\n8\n42\n42\n80706\n8\n",
    );
}

#[test]
fn a_list_returns_a_narrow_element_at_its_own_width() {
    // The same defect in the second place a heap element's own address reaches
    // a one-eightbyte load: `List#pop`/`List#remove` hand the element address
    // to `emit_elem_return`, whose `One` arm loaded eight bytes — and whose
    // `Two` arm loaded a full second eightbyte — out of a buffer sized to the
    // element. The four pushes fill the initial capacity so both pops come off
    // the buffer's end, where the crossing load is a heap overread.
    assert_compiles_and_runs(
        "tests/fixtures/list_narrow_element_pop.kp",
        "4\n111\n987\n321\n1\n4\n98\n76\n2\n4\n10101\n90807\n30201\n1\n",
    );
}

#[test]
fn setting_an_owned_element_frees_the_one_it_replaces() {
    assert_compiles_and_runs(
        "tests/fixtures/list_owned_replacement.kp",
        "1\nsolo\nreplacement\nreplacement\nother\nb\n2\n0\n2\n0\n",
    );
}

#[test]
fn inserting_or_removing_out_of_bounds_panics() {
    // One past the end is a legal insertion point and index 0 is out of
    // bounds once the list is empty, so both messages are pinned from the
    // side the check is actually written against.
    assert_fixture_panics("list_insert_bounds", "List.insert: index out of bounds");
    assert_fixture_panics("list_remove_bounds", "List.remove: index out of bounds");
}

#[test]
fn pushing_an_owned_element_moves_it() {
    assert_rejected("tests/fixtures/list_moved_element.kp");
}

#[test]
fn a_borrow_cannot_be_a_list_element() {
    assert_rejected("tests/fixtures/list_borrow_element.kp");
}

#[test]
fn an_empty_list_needs_its_element_type() {
    assert_rejected("tests/fixtures/list_no_element_type.kp");
}

#[test]
fn an_empty_container_takes_its_element_from_the_first_use() {
    // The same declaration compiles once something hands the element in: a
    // `push`, a `push` in a nested block, a `push` under a `for`, a map's
    // `insert`, a box's `set`, and a position that expects a container (a
    // parameter, and a function's return type).
    assert_compiles_and_runs(
        "tests/fixtures/container_element_from_use.kp",
        "4 6\n2 8\n9\n0\n0\n",
    );
}

#[test]
fn popping_an_empty_list_panics() {
    assert_fixture_panics("list_pop_empty", "List.pop: the list is empty");
}

#[test]
fn a_list_may_hold_a_list_element() {
    // A-10 slice 1: an element lives in the outer list's own buffer, so a
    // `List<List<i32>>` is a list of growable buffers, each dropped once by the
    // outer `set`/`pop`/`clear`.
    assert_compiles_and_runs(
        "tests/fixtures/list_nested_list.kp",
        "2\n1\n2\n3\n9\n2\n1\n0\n",
    );
}

#[test]
fn a_nested_list_of_owned_elements_moves_borrows_and_drops() {
    // The `String`-owning nested list through push/get/set/pop/clear, plus a
    // nested list that drops at scope exit. Valgrind walks the nested drop
    // path (see `tests/memory_gate.rs`).
    assert_compiles_and_runs(
        "tests/fixtures/list_nested_owned.kp",
        "2\n2\nbeta\ngamma\nbeta\ntrue\n2\ndone\n",
    );
}

#[test]
fn a_list_may_hold_a_box_element() {
    // A-10 slice 1: an element is the box's own struct, so `Box.empty()` is a
    // `NULL` slot the list still drops exactly once, and `get` hands back a
    // borrow of the box.
    assert_compiles_and_runs(
        "tests/fixtures/list_box_element.kp",
        "2\n7\ntrue\n9\n9\nhi\ntrue\nthere\n0\n",
    );
}

#[test]
fn a_bare_list_settles_on_a_nested_element() {
    // H-6's open container meets A-10 slice 1: `xs.push(List.of(1))` settles
    // `xs` on `List<List<i32>>`, and a `Box` element settles it the same way.
    assert_compiles_and_runs("tests/fixtures/list_nested_open.kp", "2\n2\n5\n");
}

/// The open-container marker (`Unit` where an element type goes) is not an
/// element at any depth. `List.of` over a bare `List.empty()` would give the
/// outer list the marker as its element, and both backends then have no element
/// type to name: the C backend emits a `void` element and the `llvm` backend
/// refuses the instantiation as an internal error, so they disagree. The gate
/// refuses first, and says what to do about it.
#[test]
fn a_container_of_an_undecided_element_cannot_be_a_list_element() {
    assert_rejected_with(
        "tests/fixtures/list_of_open_list.kp",
        "'List<unit>' cannot be a 'List' element: a container whose element type is not decided yet",
    );
}

/// The same marker through `push`, where the open receiver is what would decide
/// the element: the slot would settle on `List<List<Unit>>`, so the element is
/// refused before the receiver is repaired.
#[test]
fn a_push_cannot_settle_an_open_list_on_an_open_element() {
    assert_rejected_with(
        "tests/fixtures/list_nested_open_marker.kp",
        "'List<unit>' cannot be a 'List' element: a container whose element type is not decided yet",
    );
}

/// `insert` names its element second, after the index, and the same gate sits
/// on both paths.
#[test]
fn an_insert_cannot_settle_an_open_list_on_an_open_element() {
    assert_rejected_with(
        "tests/fixtures/list_insert_open_list.kp",
        "'List<unit>' cannot be a 'List' element: a container whose element type is not decided yet",
    );
}

/// A box may hold another container, but not one whose element type is undecided:
/// `Box.of` over a bare `Box.empty()` would put the marker in the outer box.
#[test]
fn a_container_of_an_undecided_element_cannot_be_a_box_element() {
    assert_rejected_with(
        "tests/fixtures/box_of_open_box.kp",
        "'Box<unit>' cannot be a 'Box' element: a container whose element type is not decided yet",
    );
}

/// The `Box.set` half of the open-decides path, refused for the same reason.
#[test]
fn a_box_set_cannot_settle_an_open_box_on_an_open_element() {
    assert_rejected_with(
        "tests/fixtures/box_set_open_box.kp",
        "'Box<unit>' cannot be a 'Box' element: a container whose element type is not decided yet",
    );
}

#[test]
fn split_returns_a_list_of_owned_pieces_and_join_consumes_one() {
    assert_compiles_and_runs(
        "tests/fixtures/stdlib_list.kp",
        "4\na|b||c\n[a][b][][c]\n1\n1\n4\na-b--c\n1\nsolo\n",
    );
}

// ---------------------------------------------------------------------------
// M8: the builtin owning pointer `Box<T>`, recursive `datac`s, and the
// container emission order that makes them work.
// ---------------------------------------------------------------------------

#[test]
fn a_box_holds_a_value_or_nothing() {
    assert_compiles_and_runs(
        "tests/fixtures/box_basic.kp",
        "true\n7\nfalse\n9\ntrue\nhi\n5\n2\n1\n3\n",
    );
}

#[test]
fn a_datac_can_be_recursive_through_a_box() {
    // The linked list: `next` is an owned pointer, `Box.empty()` terminates
    // it, and the whole chain is freed recursively (also under ASAN).
    assert_compiles_and_runs(
        "tests/fixtures/box_recursive.kp",
        "6\n3\n3\n2\n1\n203\n19906\n198\ntrue\n198\n",
    );
}

#[test]
fn a_datac_can_be_recursive_through_a_list_and_mutually_recursive_through_boxes() {
    assert_compiles_and_runs("tests/fixtures/recursive_datac.kp", "113\n2\n100\nl1\nl1\nb1\n");
}

#[test]
fn using_a_field_after_it_was_moved_out_is_refused() {
    // The partial move takes the field, so the second statement reaches
    // through a field that is gone. The refusal is pinned so the fixture
    // cannot pass for the old "no partial moves" reason.
    assert_rejected_with(
        "tests/fixtures/owned_field_read.kp",
        "use of moved value 'bag.xs'",
    );
}

#[test]
fn a_partial_move_takes_an_owned_field_and_leaves_the_rest() {
    // The moved field's slot is zeroed at the move and the binding's own drop
    // frees the field that stayed, so both this fixture and
    // `partial_move_conditional` are in the AddressSanitizer and valgrind
    // gates: a double free or a leak fails there.
    assert_compiles_and_runs(
        "tests/fixtures/partial_move_field.kp",
        "kept\n1\n",
    );
}

#[test]
fn a_partial_move_on_one_branch_still_frees_once_on_both() {
    assert_compiles_and_runs(
        "tests/fixtures/partial_move_conditional.kp",
        "a\n1\nb\n2\n",
    );
}

#[test]
fn a_field_is_owned_again_after_a_fresh_value_is_assigned_to_it() {
    assert_compiles_and_runs(
        "tests/fixtures/partial_move_reassign.kp",
        "1\n2\n",
    );
}

#[test]
fn a_move_parameter_is_an_owned_local_for_a_partial_move() {
    // A `move` parameter owns what it was handed, so reading an owned field
    // out of it by value takes the field out the same way a `val` binding
    // does. The callee frees the fields that stayed, so a double free of the
    // moved buffer fails here and under valgrind.
    assert_compiles_and_runs(
        "tests/fixtures/partial_move_move_param.kp",
        "taken\n1\n",
    );
}

#[test]
fn a_partial_move_out_of_a_borrow_stays_refused() {
    assert_rejected_with(
        "tests/fixtures/partial_move_from_borrow.kp",
        "cannot move the field 'xs' out of 'b': the value is borrowed",
    );
}

#[test]
fn a_field_cannot_be_moved_out_twice() {
    assert_rejected_with(
        "tests/fixtures/partial_move_double.kp",
        "use of moved value 'bag.xs'",
    );
}

#[test]
fn a_partially_moved_binding_cannot_be_used_as_a_whole() {
    assert_rejected_with(
        "tests/fixtures/partial_move_use_base.kp",
        "cannot use 'bag' as a whole value: its field 'xs' was moved out",
    );
}

#[test]
fn a_partially_moved_binding_cannot_be_borrowed_as_a_whole() {
    // `&bag` is a whole-value read too, so the borrow is refused where the
    // binding is no longer whole. Two fields are gone and the refusal names
    // one, which pins the moved set as ordered: read out of a hash set, the
    // field named here would change from run to run.
    assert_rejected_with(
        "tests/fixtures/partial_move_borrow_base.kp",
        "cannot use 'bag' as a whole value: its field 'name' was moved out",
    );
}

#[test]
fn a_partial_move_takes_an_owned_enum_payload_and_leaves_the_rest() {
    // The enum-payload half of A-11. The moved payload's slot is zeroed at the
    // move and the enum's own drop frees the payloads the move left behind, so
    // this fixture is in the AddressSanitizer and valgrind gates: a double free
    // or a leak fails there. A payload that stayed is still readable, and the
    // first case moves it out of a `move` parameter.
    assert_compiles_and_runs(
        "tests/fixtures/partial_move_enum_payload_owned.kp",
        "moved\nkept\n2\n",
    );
}

#[test]
fn an_enum_payload_cannot_be_moved_out_twice() {
    // The first `when` took the value and cleared its slot, so the second read
    // of the same payload is a use of the moved value, named through the
    // projection.
    assert_rejected_with(
        "tests/fixtures/partial_move_enum_payload_double.kp",
        "use of moved value 'h.text'",
    );
}

#[test]
fn an_enum_payload_cannot_be_moved_out_of_a_borrow() {
    // A payload binds as a shared borrow when the scrutinee is one, so an
    // `if let` over a borrowed enum hands the arm a `&String` and cannot
    // produce the owned one. `enum_payload_move.kp` is the same refusal
    // through a `when`.
    assert_rejected_with(
        "tests/fixtures/partial_move_enum_payload_from_borrow.kp",
        "return type mismatch: expected 'String', found '&String'",
    );
}

#[test]
fn the_owned_option_payload_comes_out_with_unwrap() {
    // A-11 unblocks the accessors that hand a payload out by value: `unwrap`,
    // `expect` and `unwrap_or` take the option with `move`, so an owned payload
    // is read out of the enum by value. The payload's slot is cleared at that
    // read, so this is in the AddressSanitizer and valgrind gates.
    assert_compiles_and_runs(
        "tests/fixtures/option_unwrap_owned.kp",
        "hello\nnone\nexpected\nheld\nstaged\n",
    );
}

#[test]
fn a_result_combinator_hands_an_owned_payload_back() {
    // `result.map`/`map_err` take the result with `move`, so the side they
    // rebuild may be owned: `map` moves a `datac` error into the fresh `Err`
    // and `map_err` moves an owned `String` into the fresh `Ok`.
    assert_compiles_and_runs(
        "tests/fixtures/result_map_owned.kp",
        "5\ntrue\ntrue\ntrue\n",
    );
}

#[test]
fn an_owned_field_in_a_box_can_be_taken_and_set() {
    assert_compiles_and_runs("tests/fixtures/box_field_take.kp", "1\ntrue\n1\n2\n");
}

#[test]
fn box_owned_elements_are_freed_once_when_replaced_or_taken() {
    // A `set` on a full box frees the `String`/`Bytes` the slot held, and a
    // `take` moves the value out to a second owner without a second free.
    assert_compiles_and_runs(
        "tests/fixtures/box_owned_elements.kp",
        "false\nhello\nworld\ntrue\nworld\ntrue\nfalse\nalpha\nbeta\nbeta\ntrue\n3\n5\ndefgh\ntrue\ndefgh\n104\n4\n5\n9\ntrue\n5\n3\n6\n2\n103\n4956\n98\n195\ntrue\nfalse\n98\n103\n4956\n",
    );
}

#[test]
fn a_box_cannot_hold_a_borrow() {
    assert_rejected("tests/fixtures/box_borrow_element.kp");
}

#[test]
fn a_datac_cannot_hold_itself_by_value() {
    assert_rejected("tests/fixtures/datac_self_value.kp");
}

#[test]
fn a_map_cannot_be_a_list_element() {
    // A-10 slice 1 made a `List<T>` and a `Box<T>` list elements, but a map
    // value set is narrower, so a `Map` element stays refused.
    assert_rejected_with(
        "tests/fixtures/list_container_element.kp",
        "'Map<String, i32>' cannot be a 'List' element",
    );
}

#[test]
fn reaching_into_an_empty_box_panics() {
    assert_fixture_panics("box_empty_panic", "Box.get: the box is empty");
}

#[test]
fn a_datac_can_be_a_list_element() {
    assert_compiles_and_runs("tests/fixtures/list_datac_element.kp", "2\nspades\n2\nspades\n");
}

#[test]
fn a_map_stores_keys_and_returns_insertion_order() {
    assert_compiles_and_runs(
        "tests/fixtures/map_basic.kp",
        "0\ntrue\n3\ntrue\nfalse\n2\n99\n22\n1\n2\ntwo three \n2\n0\ntrue\n",
    );
}

#[test]
fn a_map_of_owned_values_borrows_clones_and_moves() {
    // The value is inline in the entry, the key is an owned `String`, and
    // `get_or`/`remove`/`values` all transfer or clone it; ASAN re-runs this
    // fixture (see `heap_fixtures_are_double_free_and_leak_clean`).
    assert_compiles_and_runs(
        "tests/fixtures/map_owned.kp",
        "2\ntwo\ntwo\nfallback\nTWO\none\n1\nbeta\nTWO\ntrue\n",
    );
}

#[test]
fn a_map_holds_datac_values_and_can_live_in_a_datac_field() {
    assert_compiles_and_runs("tests/fixtures/map_datac.kp", "30\n2\n3\n1\n");
}

#[test]
fn a_map_deep_copies_a_datac_value_out_through_get_or_and_values() {
    // MISSING-qM4Zx: `get_or` and `values` are gated on the same
    // declaration-graph deep-copy predicate `List.slice` uses, so a `datac`
    // whose fields all copy takes the same derived `kelp_clone_<T>` out of the
    // map that it takes out of a list. The value owns a `String`, so a shallow
    // copy would print the right text and then double-free the buffer; ASAN
    // re-runs this fixture (see `heap_fixtures_are_double_free_and_leak_clean`).
    assert_compiles_and_runs(
        "tests/fixtures/map_datac_value_clone.kp",
        "alpha\n10\nnone\n0\n2\nalpha\n10\nbeta\n20\nalpha\n2\n",
    );
}

#[test]
fn a_map_cannot_hold_a_container_or_be_created_without_a_value_type() {
    assert_rejected("tests/fixtures/map_container_value.kp");
    assert_rejected("tests/fixtures/map_no_value_type.kp");
}

#[test]
fn a_map_key_can_be_a_scalar_and_the_shorthand_still_means_string() {
    assert_compiles_and_runs(
        "tests/fixtures/map_key_types.kp",
        "2\n99\ntrue\nfalse\n0\n2\n1\n1\n10\n2\nyes\nno\n70\n2\n9\nfalse\n2\n6\nbb\n",
    );
}

#[test]
fn a_key_type_the_compiler_cannot_hash_or_compare_is_rejected() {
    for name in [
        // `nan != nan` would make an inserted key unfindable.
        "map_float_key",
        // A `datac` with one field that is not keyable is not a key either:
        // the keyability fixpoint over the declaration graph leaves it out.
        "map_unkeyable_datac_key",
        // A borrow and a container have no derived hash and equality.
        "map_borrow_key",
        "map_list_key",
        // One argument (`Map<V>`) or two (`Map<K, V>`), never three.
        "map_arity",
    ] {
        assert_rejected(&format!("tests/fixtures/{name}.kp"));
    }
}

#[test]
fn a_datac_whose_fields_are_keyable_is_a_map_key() {
    // A-6: the compiler derives the key's hash and equality for a `datac` the
    // same way it derives its order and its deep copy, so no comparator and no
    // trait is involved.
    assert_compiles_and_runs(
        "tests/fixtures/map_datac_key.kp",
        "1\n10\nfalse\nfalse\n",
    );
}

#[test]
fn a_datac_map_key_is_cloned_in_overwritten_and_removed() {
    assert_compiles_and_runs(
        "tests/fixtures/map_datac_key_ok.kp",
        "2\n99\n20\nfalse\ntrue\n2\n1\n2\n99\n1\nfalse\n31\n7\n29\nfalse\n",
    );
}

#[test]
fn a_datac_map_key_may_own_a_string_field() {
    // The `String` field is hashed byte-wise and compared length-aware, and the
    // entry owns a deep copy of the key, so ASAN re-runs this fixture (see
    // `heap_fixtures_are_double_free_and_leak_clean`).
    assert_compiles_and_runs(
        "tests/fixtures/map_nested_datac_key.kp",
        "2\n10\nfalse\n20\n10\n1\ntrue\n",
    );
}

#[test]
fn a_datac_that_declares_hash_and_eq_is_a_map_key() {
    // A-12: the declared pair is the escape hatch from the derived key set. A
    // float is never keyable (`nan != nan`), so `Sample` is a type the
    // keyability fixpoint refuses and the map reaches the two declared methods
    // instead of a derived hash and equality.
    assert_compiles_and_runs(
        "tests/fixtures/map_declared_key.kp",
        "2\n10\n20\nfalse\n2\n99\n2\nalpha\n1.500000\n20\n1\ntrue\nfalse\n",
    );
}

#[test]
fn an_enum_that_declares_hash_and_eq_is_a_map_key() {
    // A-12: the same rule for an `enum`. The declared `hash` folds only the
    // tag, so the two `Ratio` keys collide and `eq` is what the probe has to
    // call to tell them apart.
    assert_compiles_and_runs(
        "tests/fixtures/map_declared_key_enum.kp",
        "3\n20\nfalse\n3\n99\n30\n2\ntrue\n",
    );
}

#[test]
fn a_datac_that_declares_eq_gets_the_equality_operator() {
    // H-5: `a == b` lowers to `a.eq(&b)` and `a != b` to `!a.eq(&b)`, with
    // `eq` alone enough for the operator. The fixture also pins that each
    // operand is one evaluation, left to right, and that a borrow against an
    // owned value works.
    assert_compiles_and_runs(
        "tests/fixtures/eq_operator_datac.kp",
        "true\ntrue\nfalse\ntrue\ntrue\n3\n3\nfalse\n4\n4\nfalse\nguard\n3\n",
    );
}

#[test]
fn an_enum_that_declares_eq_gets_the_equality_operator() {
    // H-5: the rule is the same for an `enum`.
    assert_compiles_and_runs(
        "tests/fixtures/eq_operator_enum.kp",
        "true\ntrue\ntrue\nfalse\n",
    );
}

#[test]
fn a_view_compares_with_a_view_and_a_str() {
    // H-5: `View` against `View` and `View` against `str` lower to the builtin
    // `View.eq`, which compares the bytes and the lengths.
    assert_compiles_and_runs(
        "tests/fixtures/eq_operator_view.kp",
        "true\ntrue\ntrue\ntrue\ntrue\ntrue\nfalse\n",
    );
}

#[test]
fn a_lone_eq_declares_the_operator_and_is_not_a_map_key() {
    // A-12 + H-5: `eq` alone gives `==`/`!=` and leaves the type a *derived*
    // `Map` key (both halves are still what a declared key needs).
    assert_compiles_and_runs(
        "tests/fixtures/map_key_pair_only_eq.kp",
        "true\ntrue\ntrue\n2\n20\n",
    );
}

#[test]
fn the_equality_operator_is_refused_without_a_declared_eq() {
    // H-5: nothing is derived, so a `datac` without `eq` keeps the old
    // refusal, and `<` is refused even on a type that has `eq`.
    assert_rejected_with(
        "tests/fixtures/eq_operator_no_eq.kp",
        "a datac has no generated equality",
    );
    assert_rejected_with(
        "tests/fixtures/eq_operator_order.kp",
        "cannot compare 'P' values with '<'",
    );
    assert_rejected_with(
        "tests/fixtures/eq_operator_order_gt.kp",
        "cannot compare 'P' values with '>'",
    );
    assert_rejected_with(
        "tests/fixtures/eq_operator_bad_signature.kp",
        "'P.eq' takes exactly one parameter",
    );
    assert_rejected_with(
        "tests/fixtures/eq_operator_type_mismatch.kp",
        "cannot compare 'A' with 'B'",
    );
}

// ---------------------------------------------------------------------------
// Language-completeness 5: operator traits and `Display` (`plan/traits.md` 13).
// `std/ops` declares `Add`/`Sub`/`Mul`/`Div`/`Rem`/`Neg`/`Ord` and `std/fmt`
// declares `Display`, and the front end lowers each operator and each
// interpolation into the method it names, so both backends run the ordinary
// direct call.
// ---------------------------------------------------------------------------

#[test]
fn the_arithmetic_operators_lower_to_std_ops_methods() {
    assert_compiles_and_runs(
        "tests/fixtures/ops_arithmetic.kp",
        "4\n6\n-1\n0\n-1\n-2\n3\n8\n4\n0\n1\n3\n",
    );
}

#[test]
fn ord_gives_the_order_operators_and_a_hand_written_sort_works() {
    assert_compiles_and_runs(
        "tests/fixtures/ops_ord.kp",
        "true\nfalse\nfalse\ntrue\n1\n9\n2\n0\n2\n1\n",
    );
}

#[test]
fn display_lets_a_value_into_a_template_and_print_and_nests() {
    assert_compiles_and_runs(
        "tests/fixtures/ops_display.kp",
        "v = (3, 4)\n(3, 4)\n3\nline = (3, 4) -> (5, 6)\n(3, 4) -> (5, 6)\n(3, 4) -> (5, 6)\n",
    );
}

#[test]
fn compound_assignment_on_a_user_type_is_the_add_then_assign_desugar() {
    assert_compiles_and_runs(
        "tests/fixtures/ops_compound_assign.kp",
        "4\n6\n1\n2\n3\n8\n4\n5\n",
    );
}

#[test]
fn every_operand_of_an_overloaded_operator_is_evaluated_once_left_to_right() {
    assert_compiles_and_runs(
        "tests/fixtures/ops_operands_once.kp",
        "1\n2\n3\n3\n4\ntrue\n5\n6\n7\n4\n",
    );
}

#[test]
fn an_operator_resolves_on_a_bounded_generic_type_parameter() {
    // A generic body is checked once per instantiation, so `T` is the concrete
    // `Vec2` when `a + b` and `a < b` are lowered.
    assert_compiles_and_runs(
        "tests/fixtures/ops_generic.kp",
        "4\n7\ntrue\nfalse\n",
    );
}

#[test]
fn an_operator_needs_its_trait_and_the_order_trait_is_not_equality() {
    assert_rejected_with(
        "tests/fixtures/ops_arith_no_add.kp",
        "it does not implement 'Add' from 'std/ops'",
    );
    assert_rejected_with(
        "tests/fixtures/ops_ord_no_eq.kp",
        "'==' and '!=' come from a declared 'eq' method, and 'Ord' does not supply one",
    );
    assert_rejected_with(
        "tests/fixtures/ops_display_missing.kp",
        "it does not implement 'Display'",
    );
    assert_rejected_with(
        "tests/fixtures/ops_orphan_builtin.kp",
        "'i32' is a builtin type, which has no module",
    );
    assert_rejected_with(
        "tests/fixtures/ops_mixed_types.kp",
        "the operands must have the same type",
    );
}

// ---------------------------------------------------------------------------
// A-12 slice 2: traits with static dispatch. A trait is a named set of method
// signatures, an `impl` gives them to a `datac`/`enum`, and a generic function
// bounds its type parameter on one. Every call below is the concrete type's own
// method, so both backends emit the same direct call they would have emitted
// without the trait.

#[test]
fn a_trait_is_implemented_by_a_datac_and_an_enum_and_called_through_a_generic() {
    assert_compiles_and_runs(
        "tests/fixtures/traits_datac_and_enum.kp",
        "12.000000\n27.000000\n27.000000\ncircle\ntrue\n9.000000\n18.000000\n2\n9.000000\n0.000000\n",
    );
}

#[test]
fn a_generic_datac_method_calls_a_bounded_generic_with_its_type_parameter() {
    // The bound is resolved when the *method* is instantiated, so the call
    // inside `Pair<A, B>.total()` is checked once per pair of concrete
    // arguments, exactly as a free function's body is.
    assert_compiles_and_runs(
        "tests/fixtures/traits_generic_datac_method.kp",
        "7\n3\n",
    );
}

#[test]
fn a_bound_is_checked_at_every_call_and_transitively() {
    // A call whose type argument does not implement the bound, and a generic
    // function reaching a second bounded generic with its own parameter: the
    // inner call is refused where the outer one is instantiated with a type
    // that satisfies only the outer bound.
    assert_rejected_with(
        "tests/fixtures/traits_bound_unimplemented.kp",
        "needs 'Point' to implement the trait 'Shape'",
    );
    assert_rejected_with(
        "tests/fixtures/traits_bound_transitive.kp",
        "needs 'OnlyNamed' to implement the trait 'Doubled'",
    );
    // Two bounds on one parameter, only one of them satisfied.
    assert_rejected_with(
        "tests/fixtures/traits_bound_second.kp",
        "needs 'OnlyShape' to implement the trait 'Grow'",
    );
}

#[test]
fn an_impl_must_implement_exactly_the_traits_signatures() {
    assert_rejected_with(
        "tests/fixtures/traits_missing_method.kp",
        "does not implement it",
    );
    assert_rejected_with(
        "tests/fixtures/traits_wrong_signature.kp",
        "does not match trait 'Shape'",
    );
    assert_rejected_with(
        "tests/fixtures/traits_wrong_receiver.kp",
        "takes its receiver differently than trait 'Grow'",
    );
    assert_rejected_with(
        "tests/fixtures/traits_extra_method.kp",
        "'shrink' is not declared by trait 'Shape'",
    );
}

#[test]
fn a_trait_is_implemented_for_a_type_at_most_once() {
    assert_rejected_with(
        "tests/fixtures/traits_duplicate_impl.kp",
        "already has an impl for 'Circle'",
    );
}

#[test]
fn a_trait_method_and_an_explicit_method_cannot_share_a_name() {
    assert_rejected_with(
        "tests/fixtures/traits_inherent_collision.kp",
        "already has as an explicit method",
    );
}

#[test]
fn an_impl_naming_no_declared_trait_is_rejected() {
    assert_rejected_with(
        "tests/fixtures/traits_unknown.kp",
        "names no declared trait",
    );
}

#[test]
fn the_trait_shapes_this_slice_does_not_reach_are_refused_by_name() {
    assert_rejected_with(
        "tests/fixtures/traits_dyn.kp",
        "is not a type on its own",
    );
    assert_rejected_with(
        "tests/fixtures/traits_associated_type.kp",
        "default associated type is not in this slice",
    );
    // `impl Trait` in argument position was on this list and is now a shipped
    // sugar (A-20, 9.3);
    // `impl_trait_arguments_are_fresh_bounded_type_parameters` pins the
    // positive half, and the stored positions stay refused.
    // A `where` clause was on this list and is now the other spelling of a
    // bound (26.10.3); `a_trait_can_be_implemented_for_a_generic_type_under_bounds`
    // pins it.
    assert_compiles_and_runs("tests/fixtures/traits_where_clause.kp", "");
    assert_rejected_with(
        "tests/fixtures/traits_drop_declared.kp",
        "the trait 'Drop' is built in",
    );
}

/// A-12 slice 3: a trait method may carry a default body. An impl may take it
/// as written, override it, or omit every method of an all-default trait, and a
/// default may call the trait's other methods through `self`.
#[test]
fn a_default_method_is_inherited_overridden_and_may_call_the_traits_others() {
    assert_compiles_and_runs(
        "tests/fixtures/traits_default_body.kp",
        "9\n9\n18\n5\n",
    );
}

/// A-12 slice 3: `trait B: A` requires an `impl A for` the type, lets a
/// `<T: B>` call `A`'s methods, and lets a `T: B` satisfy a `T: A` bound.
#[test]
fn a_supertrait_is_required_and_its_methods_are_callable_through_the_subtrait() {
    assert_compiles_and_runs(
        "tests/fixtures/traits_supertrait.kp",
        "12\n21\n",
    );
}

/// A-12 slice 3: a trait implemented for a builtin type that already carries
/// methods (`i32`, `String`) is reached through a generic exactly as a
/// `datac`'s impl is.
#[test]
fn a_trait_implemented_for_a_builtin_is_called_through_a_generic() {
    assert_compiles_and_runs(
        "tests/fixtures/traits_impl_for_builtin.kp",
        "20\n36\na string\n",
    );
}

/// A-12 slice 3: a default body is resolved in the trait's module even when
/// the impl that inherits it lives in the implementing type's module, so it can
/// name the trait module's own types.
#[test]
fn a_default_body_resolves_in_the_traits_own_module() {
    assert_project_runs(
        "traits_default_cross",
        &project("traits_default_cross"),
        "10\n10\n",
    );
}

/// A-12 slice 3 rejections: a missing supertrait impl, a supertrait cycle, an
/// override that does not match the defaulted signature, a default body that
/// calls an undeclared method, and a builtin impl outside the trait's module.
#[test]
fn the_new_trait_shapes_are_refused_by_name() {
    assert_rejected_with(
        "tests/fixtures/traits_missing_supertrait.kp",
        "is missing its supertrait",
    );
    assert_rejected_with(
        "tests/fixtures/traits_supertrait_cycle.kp",
        "supertrait relation is cyclic",
    );
    assert_rejected_with(
        "tests/fixtures/traits_default_wrong_override.kp",
        "does not match trait 'Shape'",
    );
    assert_rejected_with(
        "tests/fixtures/traits_default_undeclared_method.kp",
        "has no method 'nope'",
    );
    assert_project_rejected(
        "traits_builtin_orphan",
        &project("traits_builtin_orphan"),
        "is a builtin type, which has no module",
    );
}

/// `Self` in a trait signature is the implementing type wherever it is written,
/// not only in the receiver: a parameter, a return type, and a `T` returned by a
/// bounded generic all resolve per instantiation.
#[test]
fn self_stands_for_the_implementing_type_everywhere_in_a_signature() {
    assert_compiles_and_runs(
        "tests/fixtures/traits_self_in_signature.kp",
        "1\n2\n",
    );
}

/// A-12 slice 4: two types implementing one trait, called through a `&dyn` in
/// a loop over borrowed bindings.
#[test]
fn two_types_are_dispatched_through_a_dyn_in_a_loop() {
    assert_compiles_and_runs(
        "tests/fixtures/traits_dyn_dispatch.kp",
        "circle\nsquare\n21.000000\n",
    );
}

/// A-12 slice 4: a default method and a supertrait method are both callable
/// through a `&dyn` of the subtrait.
#[test]
fn a_default_and_a_supertrait_method_are_callable_through_a_dyn() {
    assert_compiles_and_runs(
        "tests/fixtures/traits_dyn_default_super.kp",
        "circle\na shape\n16.000000\n",
    );
}

/// A-12 slice 4: `&mut dyn` mutates its target through the vtable.
#[test]
fn a_mut_dyn_mutates_its_target() {
    assert_compiles_and_runs("tests/fixtures/traits_dyn_mut.kp", "11\n18\n");
}

/// A-12 slice 4: a `&dyn` binding forwards on to another function.
#[test]
fn a_dyn_borrow_forwards_to_another_function() {
    assert_compiles_and_runs(
        "tests/fixtures/traits_dyn_forward.kp",
        "36.000000\n18.000000\n",
    );
}

/// A-12 slice 4: a trait implemented for a `Copy` builtin is reached through a
/// `&dyn`, so the thunk reads the scalar out of the erased pointer.
#[test]
fn a_builtin_impl_is_reached_through_a_dyn() {
    assert_compiles_and_runs("tests/fixtures/traits_dyn_builtin.kp", "84\n");
}

/// A-12 slice 4 rejections: an object-unsafe trait that returns `Self` or takes
/// another `Self`, a bare `dyn` that is neither borrowed nor boxed, a trait
/// object in a field, a returned `&dyn`, a type that does not implement the
/// trait, and an unknown trait.
#[test]
fn the_dyn_shapes_this_slice_does_not_reach_are_refused_by_name() {
    assert_rejected_with(
        "tests/fixtures/traits_dyn_unsafe_self.kp",
        "returns 'Self'",
    );
    assert_rejected_with(
        "tests/fixtures/traits_dyn_unsafe_param.kp",
        "takes a 'Self' parameter",
    );
    assert_rejected_with(
        "tests/fixtures/traits_dyn.kp",
        "is not a type on its own",
    );
    assert_rejected_with(
        "tests/fixtures/traits_dyn_field.kp",
        "is a borrowed type",
    );
    assert_rejected_with(
        "tests/fixtures/traits_dyn_return.kp",
        "cannot be a return type",
    );
    assert_rejected_with(
        "tests/fixtures/traits_dyn_not_impl.kp",
        "does not implement 'Shape'",
    );
    assert_rejected_with(
        "tests/fixtures/traits_dyn_unknown.kp",
        "names no declared trait",
    );
}

/// A-12 slice 5: a trait may declare an associated type, each impl binds it,
/// and `I::Item` is usable in a generic function's return type. Dispatch stays
/// static, so every call is the concrete type's own method.
#[test]
fn an_associated_type_is_bound_by_each_impl_and_projected_through_a_generic() {
    assert_compiles_and_runs(
        "tests/fixtures/traits_assoc_two_types.kp",
        "6\n12\n",
    );
}

/// A-12 slice 5: `type Entry: Named` is checked at the impl, and generic code
/// reaches the bound's method through the projection.
#[test]
fn a_bounded_associated_type_is_usable_inside_generic_code() {
    assert_compiles_and_runs("tests/fixtures/traits_assoc_bound.kp", "item\n5\n");
}

/// A-12 slice 5: `Self::Item` in a default method resolves through the impl's
/// binding when the concrete copy is synthesized.
#[test]
fn self_item_in_a_default_method_resolves_per_impl() {
    assert_compiles_and_runs(
        "tests/fixtures/traits_assoc_default_self.kp",
        "107\n107\n",
    );
}

/// A-12 slice 5: a `pub trait` with an associated type, implemented in another
/// module and used through a generic function whose return type names the
/// projection.
#[test]
fn an_associated_type_is_bound_across_modules() {
    assert_project_runs(
        "traits_assoc_cross",
        &project("traits_assoc_cross"),
        "1\n",
    );
}

/// A-12 slice 5 rejections: a missing binding, an extra one, a projection with
/// no bound to name it, an equality constraint, and a bound the impl's type
/// does not satisfy.
#[test]
fn the_associated_type_shapes_are_refused_by_name() {
    assert_rejected_with(
        "tests/fixtures/traits_assoc_missing.kp",
        "does not bind it",
    );
    assert_rejected_with(
        "tests/fixtures/traits_assoc_extra.kp",
        "is not declared by trait 'Src'",
    );
    assert_rejected_with(
        "tests/fixtures/traits_assoc_no_bound.kp",
        "is not a known associated type",
    );
    assert_rejected_with(
        "tests/fixtures/traits_assoc_equality.kp",
        "equality constraint on an associated type",
    );
    assert_rejected_with(
        "tests/fixtures/traits_assoc_bound_violated.kp",
        "does not satisfy the bound 'type Entry: Named'",
    );
    assert_rejected_with(
        "tests/fixtures/traits_assoc_const.kp",
        "associated consts are not in this slice",
    );
    assert_rejected_with(
        "tests/fixtures/traits_assoc_gat.kp",
        "generic associated type is not in this slice",
    );
}

/// A-12 slice 6: a trait may take type parameters, a receiverless method is a
/// static member, and `?` calls a declared `impl From<Source> for Target` when
/// the two failure payload types differ. Dispatch stays static, so both backends
/// emit the same direct calls.
#[test]
fn a_from_impl_converts_a_differing_failure_and_a_generic_trait_is_bounded() {
    assert_compiles_and_runs(
        "tests/fixtures/from_conversion.kp",
        "true\ntrue\n5\n9\n42\n5\n",
    );
}

/// A-12 slice 6: a static method written on the trait's own name
/// (`From.from(x)`), with the implementing type from the expected type.
#[test]
fn a_static_method_is_called_on_the_traits_own_name() {
    assert_compiles_and_runs("tests/fixtures/from_trait_name.kp", "5\n9\n");
}

/// A-12 slice 6: the `From` impl lives in the implementing type's module, and
/// the `?` that reaches it is in another module.
#[test]
fn a_from_impl_is_reached_across_modules() {
    assert_project_runs("convert_cross", &project("convert_cross"), "true\n");
}

/// A-12 slice 6 rejections: `?` with no conversion, a second impl of one trait
/// instance for one type, `&dyn` of a generic trait, and an impl that writes
/// the wrong number of trait arguments.
#[test]
fn the_generic_trait_shapes_this_slice_does_not_reach_are_refused_by_name() {
    assert_rejected_with(
        "tests/fixtures/from_no_impl.kp",
        "cannot convert a failure",
    );
    assert_rejected_with("tests/fixtures/from_no_impl.kp", "impl From<IoErr>");
    assert_rejected_with(
        "tests/fixtures/from_duplicate_impl.kp",
        "already has an impl for 'T'",
    );
    assert_rejected_with(
        "tests/fixtures/generic_trait_dyn.kp",
        "takes type parameters",
    );
    assert_rejected_with(
        "tests/fixtures/generic_trait_arity.kp",
        "takes 1 type argument(s)",
    );
}

/// A-12 slice 6: `std/convert` is pure language, so a bare-metal image uses
/// `From` and `?`'s conversion and boots with no undefined symbol.
#[test]
fn freestanding_convert_boots_with_a_converted_failure() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU) {
        println!(
            "skipping the freestanding convert boot: 'ld.lld' or 'qemu-system-riscv64' \
             is not on PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    boot_under_qemu(
        &RISCV64_QEMU,
        "freestanding_convert",
        &project("freestanding_convert"),
        &["converted 107", "done"],
    );
}

/// A-12 slice 4: a freestanding riscv64 image dispatches through a `&dyn` and a
/// `&mut dyn` over the UART, and the linked ELF references no undefined symbol,
/// which is to say the vtable is `.rodata`, the thunks are ordinary code, and
/// no `kelp_rt` or libc helper is reached.
#[test]
fn a_freestanding_image_dispatches_through_a_dyn_and_boots() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU) {
        println!(
            "skipping the freestanding dyn boot: 'ld.lld' or 'qemu-system-riscv64' is not on \
             PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = out_path("freestanding_dyn");
    let build = Command::new(kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .env_remove("KELPC_ORACLE")
        .arg("build")
        .arg(project("freestanding_dyn"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the freestanding dyn build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let path = bin.to_str().expect("the artifact path is utf-8");
    let symbols = tool("llvm-nm", &[path]);
    let undefined: Vec<&str> = symbols
        .lines()
        .filter(|line| line.split_whitespace().next() == Some("U"))
        .collect();
    assert!(
        undefined.is_empty(),
        "the freestanding dyn image has undefined symbols:\n{}",
        undefined.join("\n")
    );
    boot_under_qemu(
        &RISCV64_QEMU,
        "freestanding_dyn",
        &project("freestanding_dyn"),
        &["hello from dyn", "1", "bye from dyn", "2"],
    );
}

/// A-12 slice 7: a `List<Box<dyn Shape>>` of two implementors, each element a
/// fat owned pointer dispatched through its own vtable and freed through it when
/// the list drops.
#[test]
fn a_list_of_owned_trait_objects_is_dispatched_through_each_vtable() {
    assert_compiles_and_runs(
        "tests/fixtures/traits_boxdyn_list.kp",
        "circle\nsquare\n21.000000\n",
    );
}

/// A-12 slice 7: a factory function returns a `Box<dyn Shape>` chosen by a flag,
/// and a `&dyn` borrowed back out of a box goes on to a free function.
#[test]
fn a_factory_returns_an_owned_trait_object_and_the_trait_is_borrowed_back() {
    assert_compiles_and_runs(
        "tests/fixtures/traits_boxdyn_factory.kp",
        "circle\nsquare\n12.000000\n9.000000\n",
    );
}

/// A-12 slice 7: a `datac` holds a `Box<dyn Trait>` field, a `mut Self` method is
/// dispatched through the box's vtable, and each concrete type's user `drop`
/// runs through the vtable when its box dies.
#[test]
fn a_datac_field_holds_an_owned_trait_object_and_drops_it_through_the_vtable() {
    assert_compiles_and_runs(
        "tests/fixtures/traits_boxdyn_field.kp",
        "3.000000\n3.000000\n12.000000\nfalse\ndrop circle\ndrop circle\n",
    );
}

/// A-12 slice 7: an `enum` payload holds an owned trait object, and the live
/// variant's payload drops through the vtable when the binding that holds it
/// dies.
#[test]
fn an_enum_payload_holds_an_owned_trait_object_and_drops_it_through_the_vtable() {
    assert_compiles_and_runs(
        "tests/fixtures/traits_boxdyn_enum.kp",
        "empty\n12.000000\ndrop circle\ndone\n",
    );
}

/// A-12 slice 7: a box moves between lists with `pop`/`push`, `Box.empty()` is
/// the null owned trait object, and a `move Box<dyn Trait>` parameter owns one
/// by value.
#[test]
fn an_owned_trait_object_moves_between_lists_and_through_a_move_parameter() {
    assert_compiles_and_runs(
        "tests/fixtures/traits_boxdyn_move.kp",
        "true\n12.000000\n0\nfalse\n12.000000\n0\n",
    );
}

/// A-12 slice 7: an owned trait object crosses a thread boundary (an owned value
/// with no borrows is what the crossing rule admits), and the worker drops it
/// through the vtable when it returns.
#[test]
fn an_owned_trait_object_crosses_a_thread_boundary() {
    // The worker owns the box, so its drop runs when `area_of` returns, before
    // the join hands the result back to the main thread to print.
    assert_compiles_and_runs(
        "tests/fixtures/traits_boxdyn_thread.kp",
        "drop circle\n12.000000\n",
    );
}

/// A-12 slice 7 rejections: a coercion of a type that does not implement the
/// trait, an object-unsafe trait behind `Box<dyn>`, `Box<dyn A + B>`, a use
/// after a move, a partial move out of a `datac` field, a borrowed trait object
/// crossing a thread boundary, and `take`/`set` on an owned trait object.
#[test]
fn the_owned_trait_object_shapes_this_slice_refuses_are_refused_by_name() {
    assert_rejected_with(
        "tests/fixtures/traits_boxdyn_not_impl.kp",
        "does not implement 'Shape'",
    );
    assert_rejected_with(
        "tests/fixtures/traits_boxdyn_object_safe.kp",
        "'Box<dyn Maker>' is not allowed",
    );
    assert_rejected_with(
        "tests/fixtures/traits_boxdyn_plus.kp",
        "`dyn` names one trait",
    );
    assert_rejected_with(
        "tests/fixtures/traits_boxdyn_use_after_move.kp",
        "use of moved value 'b'",
    );
    assert_rejected_with(
        "tests/fixtures/traits_boxdyn_partial_move.kp",
        "use of moved value 'h.shape'",
    );
    assert_rejected_with(
        "tests/fixtures/traits_boxdyn_thread_borrow.kp",
        "cannot send '&dyn Shape'",
    );
    assert_rejected_with(
        "tests/fixtures/traits_boxdyn_take.kp",
        "'Box<dyn Shape>.take' is not allowed",
    );
    assert_rejected_with(
        "tests/fixtures/traits_boxdyn_generic_trait.kp",
        "takes type parameters",
    );
    assert_rejected_with(
        "tests/fixtures/traits_boxdyn_mut_while_borrowed.kp",
        "already borrowed as shared",
    );
}

/// A-12 slice 7 in a bare-metal image: two owned trait objects built through the
/// project's own allocator are dispatched through `.rodata` vtables and freed
/// through each vtable's drop entry, so the user `drop`s reach the UART in drop
/// order and the linked ELF references no undefined symbol.
#[test]
fn a_freestanding_image_owns_trait_objects_and_frees_them_through_the_vtable() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU) {
        println!(
            "skipping the freestanding boxdyn boot: 'ld.lld' or 'qemu-system-riscv64' is not on \
             PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = out_path("freestanding_boxdyn");
    let build = Command::new(kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .env_remove("KELPC_ORACLE")
        .arg("build")
        .arg(project("freestanding_boxdyn"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the freestanding boxdyn build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let path = bin.to_str().expect("the artifact path is utf-8");
    let symbols = tool("llvm-nm", &[path]);
    let undefined: Vec<&str> = symbols
        .lines()
        .filter(|line| line.split_whitespace().next() == Some("U"))
        .collect();
    assert!(
        undefined.is_empty(),
        "the freestanding boxdyn image has undefined symbols:\n{}",
        undefined.join("\n")
    );
    boot_under_qemu_in_order(
        &RISCV64_QEMU,
        "freestanding_boxdyn",
        &project("freestanding_boxdyn"),
        &[
            "hello from boxdyn",
            "1",
            "bye from boxdyn",
            "2",
            "hello from boxdyn",
            "drop hello",
            "drop bye",
            "drop hello",
            "boxdyn done",
        ],
    );
}

/// A trait declared in one module and implemented in another: the type's own
/// module implements the imported trait, and the trait's own module implements
/// it for a type from a third module that imports nothing. Both are at a
/// module boundary, so both need `pub` and the import that makes the name
/// reachable, and the call site in the entry module names neither trait.
#[test]
fn a_trait_crosses_a_module_boundary_on_both_allowed_sides() {
    assert_project_runs(
        "traits_cross",
        &project("traits_cross"),
        "12.000000\n27.000000\ncircle\nring\n12.000000\n",
    );
}

/// An impl in a third module, which owns neither the trait nor the type.
#[test]
fn an_impl_in_a_module_that_owns_neither_the_trait_nor_the_type_is_rejected() {
    assert_project_rejected(
        "traits_third_module",
        &project("traits_third_module"),
        "which owns neither",
    );
}

#[test]
fn a_declared_key_pair_that_is_half_or_misshaped_is_rejected() {
    // A-12: `hash` states that the type is a key, and a key needs both halves,
    // so a lone `hash`, or a signature the generated map code cannot call, is
    // refused at the declaration with the reason named rather than as a vague
    // "not a key" at a `Map` use site. A lone `eq` is *not* refused (H-5): it
    // declares the type's `==`/`!=` without making it a key, and
    // `map_key_pair_only_eq.kp` is an accepted fixture beside this one.
    assert_rejected_with(
        "tests/fixtures/map_key_pair_only_hash.kp",
        "'Half' declares 'hash' but not 'eq'",
    );
    assert_rejected_with(
        "tests/fixtures/map_key_pair_hash_ret.kp",
        "'Key.hash' must return 'u64', found 'i32'",
    );
    assert_rejected_with(
        "tests/fixtures/map_key_pair_eq_owned.kp",
        "'Key.eq' must take 'other: &Key'",
    );
    assert_rejected_with(
        "tests/fixtures/map_key_pair_eq_arity.kp",
        "'Key.eq' takes exactly one parameter",
    );
    assert_rejected_with(
        "tests/fixtures/map_key_pair_mut.kp",
        "'Key.hash' cannot take 'mut'",
    );
    assert_rejected_with(
        "tests/fixtures/map_key_pair_generic.kp",
        "'Pair.hash' cannot have type parameters",
    );
    // The pair is the escape hatch from the derived *key* set, not from the
    // deep copy the map's own entry needs.
    assert_rejected_with(
        "tests/fixtures/map_key_pair_no_deep_copy.kp",
        "'WithList' has no deep copy",
    );
}

#[test]
fn an_enum_whose_variant_payloads_are_keyable_is_a_map_key() {
    assert_compiles_and_runs(
        "tests/fixtures/map_enum_key.kp",
        "2\n10\n20\nfalse\nfalse\n2\n10\n1\n",
    );
}

#[test]
fn a_map_is_not_copy_so_assigning_one_moves_it() {
    assert_rejected("tests/fixtures/map_move.kp");
}

#[test]
fn a_missing_map_key_panics() {
    assert_fixture_panics("map_missing_key", "Map.get: key not found");
    assert_fixture_panics("map_remove_missing", "Map.remove: key not found");
}

#[test]
fn a_map_grows_by_rehashing_and_keeps_insertion_order() {
    // Twenty-three inserts cross two index capacities, and the six removes
    // before the twenty-fourth insert leave tombstones the rebuild has to
    // skip; `keys()` must still be insertion order with the removed keys gone.
    assert_compiles_and_runs(
        "tests/fixtures/map_growth_and_rehash.kp",
        "23\n220\n50\n60\n70\n80\n90\n100\n17\nfalse\n18\ntrue\n1000\n18\n0\n1\n2\n3\n4\n11\n12\n13\n14\n15\n16\n17\n18\n19\n20\n21\n22\n100\n",
    );
}

#[test]
fn a_map_sized_with_with_capacity_grows_and_behaves_like_a_new_one() {
    // Sixty-four entries of capacity for two hundred inserts crosses both
    // growth thresholds, so the hint has to leave the map in the same state
    // `Map.new()` would: same insertion order, same tombstones, same lookups.
    assert_compiles_and_runs(
        "tests/fixtures/map_with_capacity.kp",
        "200\n597\nfalse\n70\n1\n20\n10\nfalse\n30\n200\n0\n199\n99\n",
    );
    assert_rejected("tests/fixtures/map_with_capacity_arity.kp");
}

#[test]
fn a_list_sized_with_with_capacity_grows_like_an_empty_one() {
    assert_compiles_and_runs(
        "tests/fixtures/list_with_capacity.kp",
        "0\n100\n0\n198\ntrue\n7\n2\nb\n",
    );
    assert_rejected("tests/fixtures/list_with_capacity_arity.kp");
}

#[test]
fn a_map_probe_steps_over_a_tombstone_and_still_finds_the_live_key() {
    // "k0", "k8" and "k13" share one probe chain, so the tombstone `remove`
    // leaves at the chain's head is what a probe has to step past.
    assert_compiles_and_runs(
        "tests/fixtures/map_tombstones_and_reuse.kp",
        "2\n1\n1\nfalse\ntrue\n2\n2\ntrue\n3\n2\n3\ntrue\n9\n2\n3\n",
    );
}

#[test]
fn a_map_entry_keeps_the_value_at_the_key_width_offset() {
    assert_compiles_and_runs(
        "tests/fixtures/map_key_widths.kp",
        "2\n1000000007\n1000000009\n2\n2000000011\n2000000013\n2\n3000000017\n3000000019\n2\n4000000023\n4000000029\n2\n5000000031\n5000000037\n",
    );
}

#[test]
fn replacing_a_map_value_drops_the_old_one() {
    // The stdout is the same whether or not the old value is dropped; the
    // replacement path is a valgrind check (`memory_gate.rs` sweeps every
    // fixture), and this run only makes each replacement observable.
    assert_compiles_and_runs(
        "tests/fixtures/map_replace_drops_old_value.kp",
        "2\n2\nONE\n2\n1\nTWO\n2\nTWO\ntwo again\n2\n2\n1\ntwo again\n",
    );
}

#[test]
fn map_string_values_survive_the_whole_ownership_path() {
    assert_compiles_and_runs(
        "tests/fixtures/map_owned_round_trip.kp",
        "3\ntwo\nthree\nfallback\none\n2\n2\nbeta\ngamma\n2\ntwo\nthree\n0\ntrue\n",
    );
}

#[test]
fn a_cleared_map_is_empty_and_reusable() {
    assert_compiles_and_runs(
        "tests/fixtures/map_clear_then_reuse.kp",
        "10\n81\n0\ntrue\nfalse\n2\n33\n44\ntrue\n44\n1\nfalse\n1\n3\n",
    );
}

#[test]
fn a_shared_borrow_of_a_temporary_is_allowed_for_a_call() {
    assert_compiles_and_runs("tests/fixtures/borrow_temp_arg.kp", "3\n18\n");
}

#[test]
fn each_borrowed_temporary_gets_a_slot_of_its_own() {
    // MK7 §5 item 4, fourth slice: a temporary's address has to survive the
    // whole enclosing expression, including when a second temporary is built
    // before the first call reads it (`pair_sum(Pair(..), Pair(..))`), when the
    // temporary is itself a call's result (`sum(make(7, 8))`), and when it is
    // wider than one eightbyte (`quad_sum(Quad(..))`).
    assert_compiles_and_runs(
        "tests/fixtures/borrow_temp_slots.kp",
        "3\n18\n15\n6\n30\n6\n6\n",
    );
}

#[test]
fn a_borrow_from_a_temporary_bound_to_a_local_is_allowed() {
    assert_compiles_and_runs("tests/fixtures/borrow_temp_returned.kp", "x\n");
}

#[test]
fn a_binding_keeps_its_temporary_borrow_owner_alive() {
    assert_compiles_and_runs("tests/fixtures/borrow_temp_binding.kp", "hello\n");
}

#[test]
fn a_datac_can_own_the_text_its_view_field_borrows() {
    assert_compiles_and_runs("tests/fixtures/aggregate_text_borrow.kp", "label\n");
    assert_rejected_with(
        "tests/fixtures/refused_aggregate_text_borrow.kp",
        "not moved into an earlier String/Bytes field",
    );
}

/// The field spelling of the same refusal: `mkName().name` is a field of an
/// rvalue, so a borrow that outlives the call has no storage to live in. It
/// is checked by message, not just by rejection, because the shape *does*
/// build whenever the borrow is only read inside the call. The two have to be
/// told apart, and this is the one that must not quietly start compiling.
#[test]
fn a_borrow_from_a_temporary_field_bound_to_a_local_is_allowed() {
    assert_compiles_and_runs("tests/fixtures/borrow_field_of_temp_returned.kp", "x\n");
}

/// And the explicit-borrow half: `&<field of a temporary>` is not a place, so
/// binding one is refused in the compiler's own voice rather than by `cc`
/// objecting to `&(rvalue).field`.
#[test]
fn an_explicit_borrow_of_a_field_of_a_temporary_is_rejected() {
    assert_rejected_with(
        "tests/fixtures/borrow_field_of_temp.kp",
        "cannot borrow a temporary value",
    );
}

/// The other side of the same rule (`syntax.md` 10.2): a chain that steps
/// through a *borrow* has storage that outlives the statement, so an explicit
/// `&` on it is legal even though the call's own result is not a place. The
/// shallow AST question refused this until the check became the typed one.
#[test]
fn an_explicit_borrow_of_a_field_of_a_borrow_is_accepted() {
    assert_compiles_and_runs(
        "tests/fixtures/borrow_chain_through_a_borrow.kp",
        "42\nforty-two\n43\n7\n",
    );
}

#[test]
fn fs_list_returns_a_granted_directories_entries() {
    // A granted `stdlib/fs` module listing a directory it owns: the entries
    // come back as a `List<String>`, sorted, and a recursive clear built on
    // the same function leaves the directory gone.
    assert_project_runs(
        "caps_list",
        &caps_project("caps_list").join("main.kp"),
        "3\na.txt,b.txt,sub\na.txt b.txt sub \nfalse\n",
    );
}

#[test]
fn fs_breadth_creates_walks_stats_copies_opens_and_removes_under_one_grant() {
    // MK5-S5: `mkdir_all`, `read_lines`, `metadata`, `copy`, `open` (its mode
    // string is not a path), `touch`, `walk`, `temp_path`, `remove_all` — all
    // behind the one `fs` grant, all in a scratch directory the program
    // creates and removes itself.
    assert_project_runs(
        "caps_fs_breadth",
        &project("caps_fs_breadth").join("main.kp"),
        "2\nline one\nline two\n18\ntrue\nfalse\ntrue\nfalse\ntop\ntrue\n7\n/tmp/kelp_fs_breadth_test/a/b/c/note.txt\ntrue\ntrue\nfalse\n",
    );
}

#[test]
fn path_stem_extension_and_component_helpers() {
    // `stem` allocates (a prefix cannot be a view); `split` drops empty
    // components; `join_all` applies `join`'s absolute-part rule.
    assert_compiles_and_runs(
        "tests/fixtures/path_polish.kp",
        "archive.tar\nMakefile\n.bashrc\ntrue\nfalse\n2\na\nb\n/a/b/c\n/etc/x\n",
    );
}

#[test]
fn path_absolute_resolves_a_relative_argument_against_a_base() {
    // MISSING-hW5rB: `fs.cwd` is the base a command-line tool has, and the
    // resolved path is normalized so the capability check accepts it. The
    // absolute cases are compared to literals; the cwd-dependent ones assert
    // the property instead, since the test's own directory is not fixed.
    assert_compiles_and_runs(
        "tests/fixtures/path_absolute.kp",
        "/a/b/c.txt\n/a/c.txt\n/abs/x\ntrue\ntrue\n",
    );
}

/// MK7 §5 item 7 step 1's other half: a call whose wide return value is
/// discarded. A return wider than one eightbyte is written by the callee
/// through a hidden pointer, so the caller supplies the storage even when the
/// value is dead, which is the compiler temporary the C backend passes at the
/// same site. The gated fixtures needed the `unit` body's tail shape
/// (`fun main() { fs.read(p) }`); this fixture pins that plus the statement and
/// function-value shapes no fixture reached before.
#[test]
fn a_discarded_wide_return_still_builds() {
    assert_compiles_and_runs("tests/fixtures/discard_wide_return.kp", "discarded\n");
}

#[test]
fn a_narrow_fs_grant_still_reaches_cwd() {
    assert_project_runs(
        "caps_cwd",
        &caps_project("caps_cwd").join("main.kp"),
        "true\n",
    );
}

#[test]
fn a_symlink_is_seen_reported_and_left_alone_by_the_walk() {
    // MISSING-cK4hT: `is_symlink`/`read_link` classify a link, a broken link
    // still has a target text, `walk` reports a link without descending it (so
    // the link back to the root terminates the walk), and `remove_all` unlinks
    // a link instead of walking into it (BUG-a9Km4: the target's own file
    // survives).
    let _ = std::fs::remove_dir_all("/tmp/kelp_fs_symlink_kind");
    let _ = std::fs::remove_dir_all("/tmp/kelp_fs_symlink_kind_target");
    assert_compiles_and_runs(
        "tests/fixtures/fs_symlink_kind.kp",
        "true\ntrue\ntrue\nfalse\n/tmp/kelp_fs_symlink_kind/note.txt\ntrue\nfalse\n8\nfalse\ntrue\nfalse\n",
    );
    let _ = std::fs::remove_dir_all("/tmp/kelp_fs_symlink_kind");
    let _ = std::fs::remove_dir_all("/tmp/kelp_fs_symlink_kind_target");
}

// ---------------------------------------------------------------------------
// S4: the `std/fs` path family, called through its raw externs.
//
// The fixture calls the raw externs directly rather than through the gated
// wrappers, so each body is exercised on its own.
// ---------------------------------------------------------------------------

#[test]
fn the_fs_path_family_reads_writes_and_walks() {
    // One call per extern: `exists`/`is_file`/`is_dir`/`is_symlink`, `mkdir`,
    // `mkdir_all` (twice, so the `EEXIST` tolerance is pinned), `write_text`,
    // `write`, `read`, `symlink`, `read_link`, `list` and `remove`. The listing
    // is sorted by `alphasort`, and the link is reported without being
    // descended into.
    let _ = std::fs::remove_dir_all("/tmp/kelp_fs_path_family");
    assert_compiles_and_runs(
        "tests/fixtures/fs_path_family.kp",
        "false\ntrue\nfalse\n6\nhello\n\ntrue\nxyz\ntrue\ntrue\nfalse\n/tmp/kelp_fs_path_family/a.txt\nfalse\na.txt\nlink\nsub\nfalse\n",
    );
    let _ = std::fs::remove_dir_all("/tmp/kelp_fs_path_family");
}

#[test]
fn fs_set_mode_changes_the_bits_metadata_reports() {
    // 420 is octal 644 and 493 is octal 755.
    let _ = std::fs::remove_file("/tmp/kelp_fs_set_mode.txt");
    assert_compiles_and_runs("tests/fixtures/fs_set_mode.kp", "420\n493\n");
}

// ---------------------------------------------------------------------------
// S2: `io` (the standard descriptors as streams, buffered line reading) and
// `process` (argv, running a program, spawning one).
//
// `io` is ungated, so its fixtures are ordinary single files; `process` is
// gated, so its granted and denied cases live in projects, where the grant is
// written at the `impl` line.
// ---------------------------------------------------------------------------

#[test]
fn io_reads_stdin_as_lines_and_writes_the_standard_descriptors() {
    // One `io.read_line`, then `io.read_lines` for the rest, through the same
    // process-wide reader; a `File` over stdout writing a borrowed view; and
    // `eprint` on stderr, which the harness checks only for "did not fail".
    // The last line is deliberately not newline-terminated: the reader has to
    // return it at end of file rather than dropping it, which is the case a
    // `read_line` written as "read until newline" gets wrong.
    assert_fixture_runs_with_stdin(
        "io_stdio",
        "a\nb\nc\nd",
        "a\n3\nb\nc\nd\ndirect6\ntrue\ntrue\n",
    );
}

#[test]
fn io_reports_end_of_file_instead_of_a_line() {
    // The same program with nothing on stdin: `ok = false` on the first read,
    // no lines, and a clean exit.
    run_fixture_with_stdin(
        "io_stdio_eof",
        "io_stdio",
        "",
        "(no header)\n0\ndirect6\ntrue\ntrue\n",
    );
}

#[test]
fn a_held_buf_reader_reads_lines_and_consumes_the_rest() {
    // The buffered type is an owned `datac`, so this is the caller-threaded
    // form: one line out, the remainder of the stream, then `read_into` at end
    // of file (0 bytes, buffer untouched).
    assert_fixture_runs_with_stdin(
        "io_streams",
        "l1\nl2\n",
        "true\nl1\n3\nl2\n\ntrue\n0\n4\ntrue\ntrue\nfalse\n",
    );
}

#[test]
fn io_writes_bytes_text_and_a_borrowed_view() {
    // The three write bodies take their buffer as a borrow (`const kelp_Bytes*`
    // in the C signature), which a body that read the outgoing argument area
    // would miss and write nothing: each returns the byte count it wrote, and
    // the raw bytes reach stdout ahead of the buffered `print` because the body
    // flushes the matching stdio stream first.
    assert_compiles_and_runs(
        "tests/fixtures/io_write_bodies.kp",
        "bytes\n6\ntext\n5\nstr\n4\n",
    );
}

#[test]
fn process_runs_a_program_with_argv_and_spawns_one_with_pipes() {
    // `run` with a real argv array (the child prints its own `$1`/`$2`), an
    // exit code, separated stdout/stderr capture, then `spawn` with a write to
    // the child's stdin and a read of its stdout, `is_running`/`kill`/`wait`
    // on a child that is blocked, and a failed `exec` reported as code 127.
    // Also under AddressSanitizer, where the parent's buffers and pipes are
    // leak-checked.
    assert_compiles_and_runs(
        "tests/fixtures/process_spawn.kp",
        "0\none-two\n7\n0\n0\noops\n0\nhello\n\ntrue\n137\nfalse\n127\ntrue\n1\ntrue\ntrue\n",
    );
}

#[test]
fn a_granted_module_runs_a_program_and_reads_its_own_argv() {
    // The grant is `runner`'s, not `main`'s: `process.run` is checked against
    // the patterns of the module that named it, and `process.args` is behind
    // the same gate even though it names no resource.
    assert_project_runs_with_args(
        "process_run",
        &project("process_run").join("main.kp"),
        &["one", "two"],
        "0\nhi\n3\noops\none,two\n3\n",
    );
}

#[test]
fn a_signal_killed_child_is_told_apart_from_one_that_exited_with_that_code() {
    assert_project_runs(
        "process_signal",
        &project("process_signal").join("main.kp"),
        "143\n15\n143\n0\n9\n",
    );
}

#[test]
fn run_inherit_leaves_the_child_on_the_parents_descriptors() {
    // The child's `echo` lands between the parent's own prints, in order,
    // because the parent flushes before the fork and nothing is captured.
    assert_project_runs(
        "process_inherit",
        &project("process_inherit").join("main.kp"),
        "before\nfrom the child\nafter\n3\n0\n0\n15\n",
    );
}

#[test]
fn reading_argv_needs_the_gate_but_no_program_grant() {
    // An empty pattern list is a real grant: `runner` may name `std/process`
    // (so `process.args`/`process.exit` resolve) and may not execute anything.
    // It also pins the emitted allowlist for an empty grant, which used to be
    // skipped and left the generated C naming a symbol that did not exist.
    assert_project_runs_with_args(
        "process_argv_only",
        &project("process_argv_only").join("main.kp"),
        &["alpha", "beta"],
        "3\nalpha,beta\n",
    );
}

#[test]
fn a_granted_module_cannot_run_a_program_outside_its_grant() {
    // `runner` holds `process = ["/bin/true"]` and asks for `/bin/sh`; the
    // check runs before the fork, and the denial names the module, the call
    // site, and the grant.
    assert_project_denied_at_runtime(
        "process_denied",
        &project("process_denied").join("main.kp"),
        "is outside the process capability granted to module 'runner'",
    );
}

#[test]
fn a_child_gets_a_working_directory_and_a_fallible_run_reports_why() {
    // The marker files are the proof that `chdir` happened in the *child*: the
    // shell's relative redirection lands in the directory the parent named, and
    // the parent's own directory is untouched. `/tmp` as a working directory
    // (`dir_outside_grant`) pins the other half: a cwd is not a resource, so it
    // is not matched against the program allowlist.
    let dir = caps_scratch("process_in");
    std::fs::write(dir.join("not-exec"), "x").unwrap();
    assert_project_runs(
        "process_in",
        &caps_project("process_in").join("main.kp"),
        "0\ntrue\n0\n126\ntrue\nout-of-grant-cwd\ntrue\nthere\nnot-found\nnot-executable\ndir-not-found\nok\n",
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("marker.txt")).unwrap(),
        "hi",
        "`run_in` did not run the child in the directory it was given"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("spawned.txt")).unwrap(),
        "hi2",
        "`spawn_in` did not run the child in the directory it was given"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// S3: `net` — the capability vocabulary (`host`, `host:port`, `*.host`,
// `*:port`, `*`), sockets, and the one-process loopback test.
//
// Every `net` test is one process, loopback only, and never a fixed port: the
// listener binds 0 and `Listener.port` reports what the kernel assigned. The
// internet is never touched, so nothing here can flake on a network.
// ---------------------------------------------------------------------------

#[test]
fn the_entry_module_holds_net_as_the_whole_resource_wildcard() {
    // A single-file program, so `main` holds every domain with a `*` pattern:
    // `net` resolves with no `caps(...)` clause, and the runtime check is
    // the `*` the vocabulary gained in S3 (before it, `net` was a path domain
    // and every host would have been denied by the path normalizer).
    assert_compiles_and_runs("tests/fixtures/net_loopback.kp", "hello\n");
}

#[test]
fn a_granted_module_speaks_the_net_vocabulary_and_round_trips_bytes() {
    // The grant is `peer`'s, not `main`'s, and it is written in the vocabulary
    // open item 32 was about: `127.0.0.1:*` — a host and a wildcard port. The
    // program binds port 0, learns the port, connects, accepts, and exchanges
    // bytes in both directions, printing `send`'s byte count, `recv_into`'s,
    // both payloads, and whether the peer address is the requested loopback.
    assert_project_runs(
        "caps_net",
        &project("caps_net").join("main.kp"),
        "4 4 +more pong loopback bound\n",
    );
}

#[test]
fn the_net_check_matches_the_host_as_written() {
    // `util` holds `127.0.0.1:*` and connects to `localhost` — the same
    // machine under another name. The check runs before `getaddrinfo`, so this
    // is denied, and the denial prints the string the code wrote. The granted
    // pattern (not the host as written) is what the assertion reads, per the
    // determinism rule.
    assert_project_denied_at_runtime(
        "caps_net_denied",
        &project("caps_net_denied").join("main.kp"),
        "127.0.0.1:*",
    );
}

#[test]
fn a_socket_reports_readiness_without_blocking() {
    // `poll` on a connected socket: false on a bounded wait with nothing sent,
    // true once the peer writes, and true again at end of file after
    // `shutdown_write` — the three answers a single-threaded event loop needs.
    assert_compiles_and_runs("tests/fixtures/net_poll.kp", "false\ntrue\ntrue\nnow\ntrue\ntrue\n");
}

#[test]
fn resolving_a_name_returns_its_addresses() {
    // An address literal resolves to itself (deterministic); `localhost` is
    // asserted only as "resolved to something", since which addresses it maps
    // to is the machine's business. Never the internet.
    assert_compiles_and_runs(
        "tests/fixtures/net_resolve.kp",
        "1\n127.0.0.1\ntrue\n",
    );
}

#[test]
fn a_silent_peer_times_out_instead_of_hanging() {
    // `set_timeout` makes a blocking `recv` bounded: the panic names the
    // timeout rather than the program hanging until the harness gives up.
    assert_fixture_panics("net_timeout", "net.recv: timed out");
}

#[test]
fn resolving_a_host_outside_the_grant_is_denied() {
    // The `Host` check (no port) is enforced too, and the denial happens before
    // `getaddrinfo`, so this test never touches the network.
    assert_project_denied_at_runtime(
        "caps_net_resolve_denied",
        &project("caps_net_resolve_denied").join("main.kp"),
        "is outside the net capability granted to module 'util'",
    );
}

#[test]
fn a_net_pattern_that_is_not_a_host_is_rejected() {
    // The `net` vocabulary has its own validator now, so this is not the
    // "patterns are absolute paths" error a path domain gives: it names what a
    // host pattern looks like.
    assert_project_rejected(
        "caps_bad_net_pattern",
        &project("caps_bad_net_pattern").join("main.kp"),
        "is not a host",
    );
}

/// The denial's first field is `host:port` as the code wrote it, so the
/// port half is the *call's* own argument rather than the peer's address. That
/// string can only come out right when the host and the port were read from
/// their own argument slots, which makes this the behavioural pin for the
/// argument-position trap that `caps_net`'s admitted case cannot be (a swap
/// there would deny a call the grant covers, and the failure would read as a
/// missing grant rather than a misread argument).
#[test]
fn the_host_port_denial_names_the_host_as_written_and_its_own_port() {
    assert_project_denied_at_runtime(
        "caps_net_denied",
        &project("caps_net_denied").join("main.kp"),
        "'localhost:1' is outside the net capability granted to module 'util'",
    );
}

#[test]
fn a_refused_dial_is_a_value_and_not_the_end_of_the_program() {
    // `try_connect` against a port the program itself released: the bind, the
    // port the kernel assigned and the close are the whole preparation, so the
    // refusal is deterministic and no server is needed. The reason is the text
    // `net.connect` panics with, and the process exits 0 rather than dying.
    assert_compiles_and_runs(
        "tests/fixtures/net_try_connect.kp",
        "err net.connect: connection failed\n  errno 111: Connection refused\nsurvived\n",
    );
}

/// The `net` half of the fallible dial, denied: a fallible form must not be a
/// way around the grant the panicking one obeys. `util` holds `127.0.0.1:*` and
/// asks for `localhost`, so the check refuses before any resolution runs and no
/// `Err` is ever built.
#[test]
fn the_fallible_dial_is_checked_like_the_panicking_one() {
    assert_project_denied_at_runtime(
        "caps_net_try_denied",
        &project("caps_net_try_denied").join("main.kp"),
        "'localhost:1' is outside the net capability granted to module 'util'",
    );
}

#[test]
fn a_reconnect_loop_survives_three_refused_dials_and_gives_up() {
    // The fixture that could not exist before this: three dials at a port
    // nobody owns, one line per attempt, and a clean exit at the end instead
    // of a panic on the first one. The fpl discord library's gateway loop is
    // this shape, and `MISSING-w9Kda` is the dial that ended it.
    assert_compiles_and_runs(
        "tests/fixtures/net_try_connect_retry.kp",
        "attempt 1: net.connect: connection failed\n  errno 111: Connection refused\n\
         attempt 2: net.connect: connection failed\n  errno 111: Connection refused\n\
         attempt 3: net.connect: connection failed\n  errno 111: Connection refused\n\
         gave up\n",
    );
}

// ---------------------------------------------------------------------------
// S4: `std/http`. The client blocks inside `send` until the response arrives,
// so — unlike S3's `net` fixtures, which could be both ends in one process —
// an HTTP test needs a real server in another process (or, here, in another
// thread of the harness).
//
// The helper binds loopback, hands the port to the compiled program through
// argv, serves canned bytes, and counts accepted connections. That count is
// the point: it turns "the pool reused the socket" into an assertion instead of
// a timing inference, and it is the only way to see a connection that was made
// and not reused.
// ---------------------------------------------------------------------------

/// A canned 200 with a body and one extra header.
const BODY_200_HELLO: &[u8] =
    b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nX-Test: yes\r\n\r\nhello";

/// The same, with no `Content-Length`: the body ends at end of file, which is
/// the one framing that cannot be pooled.
const BODY_200_CLOSE: &[u8] = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\nbyebye";

/// `chunk!`, split into two chunks and terminated, with no trailers.
const CHUNKED_200: &[u8] =
    b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nchunk\r\n1\r\n!\r\n0\r\n\r\n";

const STATUS_404: &[u8] = b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n";

/// MISSING-p5Xtr: a `429` naming a delay and a spent per-route bucket, in the
/// shape Discord (the issue's worked example) sends it.
const STATUS_429_RATE_LIMITED: &[u8] = b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 2\r\nX-RateLimit-Limit: 10\r\nX-RateLimit-Remaining: 0\r\nX-RateLimit-Reset: 1719796939\r\nX-RateLimit-Reset-After: 0.500\r\nX-RateLimit-Bucket: abcd1234\r\nContent-Length: 0\r\n\r\n";

/// The answer the `ws.try_connect` gate serves: a peer that is not a WebSocket
/// server, which is the `Handshake` half of that call's error type.
const STATUS_403: &[u8] = b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n";

/// A relative redirect, so the follow-up keeps the same host (and therefore the
/// same pooled connection).
const REDIRECT_301: &[u8] =
    b"HTTP/1.1 301 Moved Permanently\r\nLocation: /a\r\nContent-Length: 0\r\n\r\n";

/// A redirect to a *different name* for the same machine. Port 80 is never
/// reached: the capability check denies the host string first, which is the
/// point of the test.
const REDIRECT_ELSEWHERE: &[u8] =
    b"HTTP/1.1 301 Moved Permanently\r\nLocation: http://localhost/a\r\nContent-Length: 0\r\n\r\n";

/// A 200 whose body echoes the request's, so a POST can assert what arrived.
fn echo_response(body: &str) -> Vec<u8> {
    format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\ngot:{body}", body.len() + 4)
        .into_bytes()
}

/// One connection carrying `responses`, in order.
fn one_connection(responses: &[&[u8]]) -> Vec<Vec<Vec<u8>>> {
    vec![responses.iter().map(|r| r.to_vec()).collect()]
}

/// `n` connections, one response each.
fn n_connections(response: &[u8], n: usize) -> Vec<Vec<Vec<u8>>> {
    (0..n).map(|_| vec![response.to_vec()]).collect()
}

/// A loopback server serving canned responses, and counting the connections it
/// accepted. It lives in a thread for the duration of one test.
struct CannedServer {
    port: u16,
    accepted: Arc<AtomicUsize>,
    handle: Option<thread::JoinHandle<()>>,
}

impl CannedServer {
    /// `connections[i]` is the list of responses to write on the `i`-th
    /// accepted connection, one per request read from it.
    fn start(connections: Vec<Vec<Vec<u8>>>) -> CannedServer {
        let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind loopback");
        let port = listener.local_addr().expect("local address").port();
        let accepted = Arc::new(AtomicUsize::new(0));
        let counter = accepted.clone();
        let handle = thread::spawn(move || serve(listener, connections, counter));
        CannedServer { port, accepted, handle: Some(handle) }
    }

    fn accepted(&self) -> usize {
        self.accepted.load(Ordering::SeqCst)
    }

    /// Waits for the serving thread, so the connection count is final.
    fn finish(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Reads one request: the head, plus the `Content-Length` bytes of body. Returns
/// false when the peer closed before a request arrived. Generic over `Read`
/// rather than `TcpStream` specifically, so the TLS server below
/// (`CannedTlsServer`) can run the identical request-framing logic over an
/// `SslStream<TcpStream>` instead of duplicating it.
fn read_http_request<S: Read>(stream: &mut S) -> bool {
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..pos]).to_string();
            let mut want = 0usize;
            for line in head.split("\r\n").skip(1) {
                if let Some((name, value)) = line.split_once(':') {
                    if name.eq_ignore_ascii_case("content-length") {
                        want = value.trim().parse().unwrap_or(0);
                    }
                }
            }
            let mut have = buf.len() - (pos + 4);
            while have < want {
                match stream.read(&mut chunk) {
                    Ok(0) => return false,
                    Ok(n) => {
                        buf.extend_from_slice(&chunk[..n]);
                        have += n;
                    }
                    Err(_) => return false,
                }
            }
            return true;
        }
        match stream.read(&mut chunk) {
            Ok(0) => return false,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(_) => return false,
        }
    }
}

fn serve(
    listener: TcpListener,
    connections: Vec<Vec<Vec<u8>>>,
    accepted: Arc<AtomicUsize>,
) {
    let _ = listener.set_nonblocking(true);
    // A bounded wait: if the client opens fewer connections than the test
    // canned (because it failed, or reused more than expected), the thread
    // gives up and the assertion on `accepted` reports the difference. Without
    // this, a wrong client would hang the suite instead of failing it.
    let deadline = Instant::now() + Duration::from_secs(20);
    for responses in connections {
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break Some(stream),
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() > deadline {
                        break None;
                    }
                    thread::sleep(Duration::from_millis(2));
                }
                Err(_) => break None,
            }
        };
        let Some(mut stream) = stream.take() else { return };
        // Non-blocking was only for the accept loop: an accepted socket
        // inherits the listener's mode on Linux, and a non-blocking read here
        // would report `WouldBlock` for a request that is still in flight.
        let _ = stream.set_nonblocking(false);
        accepted.fetch_add(1, Ordering::SeqCst);
        for response in &responses {
            if !read_http_request(&mut stream) {
                break;
            }
            if stream.write_all(response).is_err() {
                break;
            }
        }
        // Dropping the stream closes it, which is what a server that has said
        // everything it intends to say does. A client that tries to reuse a
        // connection the server has closed sees end of file, exactly as it
        // would against a real peer that timed the connection out.
    }
}

// ---------------------------------------------------------------------------
// MK5-S4b-M1 (open item 37): `std/tls`'s and `https`'s own test server. Same
// canned-connections shape as `CannedServer` above, except every accepted
// `TcpStream` goes through an `SslAcceptor`'s handshake first, using a
// throwaway self-signed certificate generated once per server (the `openssl`
// crate, not a CLI subprocess — see `Cargo.toml`'s `[dev-dependencies]`
// comment). The compiled `.kp` binary under test is started with
// `SSL_CERT_FILE` pointing at that certificate's own PEM file, which is what
// lets `std/tls`'s unconditional `SSL_CTX_set_default_verify_paths()`
// (there is no `insecure` flag in v1, on purpose) accept it: OpenSSL reads
// that variable as an override of the compiled-in trust store, so this needs
// no write to the machine's real one.
// ---------------------------------------------------------------------------

/// A throwaway self-signed certificate and key for one test server, PEM-encoded
/// on disk (`openssl::ssl::SslAcceptor::set_certificate_file` and the
/// `SSL_CERT_FILE` env var both want a path, not bytes in memory). Subject
/// and SAN are both `127.0.0.1`, since every fixture that uses this dials
/// that literal.
struct TestCert {
    cert_path: PathBuf,
    key_path: PathBuf,
    // Kept alive only so the temp directory is not removed while the paths
    // above are still in use; nothing reads it. Shared rather than owned, so
    // one certificate can back two servers (the `https` redirect case needs a
    // second local port that the client's one `SSL_CERT_FILE` also trusts).
    _dir: Arc<tempfile_dir::TempDir>,
}

impl Clone for TestCert {
    fn clone(&self) -> TestCert {
        TestCert {
            cert_path: self.cert_path.clone(),
            key_path: self.key_path.clone(),
            _dir: self._dir.clone(),
        }
    }
}

/// A tiny drop-cleaned temp directory, written by hand rather than pulling in
/// a `tempfile` dependency for one struct: `std::env::temp_dir()` plus the
/// test's own pid and an atomic counter (two TLS fixtures in one process
/// would otherwise collide on one directory), removed on `Drop`.
mod tempfile_dir {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    pub struct TempDir(PathBuf);

    impl TempDir {
        pub fn new(label: &str) -> TempDir {
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let dir = std::env::temp_dir()
                .join(format!("kelpc_{label}_{}_{n}", std::process::id()));
            std::fs::create_dir_all(&dir).expect("failed to create a scratch directory");
            TempDir(dir)
        }

        pub fn path(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

/// Which names the throwaway certificate carries. `LoopbackIp` is the server
/// every trusted fixture dials as `127.0.0.1`; `DnsLocalhost` is the same key
/// with only a DNS SAN, so dialing `127.0.0.1` is a name mismatch.
enum CertSubject {
    LoopbackIp,
    DnsLocalhost,
}

fn generate_test_cert() -> TestCert {
    generate_test_cert_with(CertSubject::LoopbackIp, true)
}

/// As `generate_test_cert`, with `notBefore`/`notAfter` in the past: the
/// certificate a machine's clock has run past, which verification must refuse.
fn generate_expired_test_cert() -> TestCert {
    generate_test_cert_with(CertSubject::LoopbackIp, false)
}

/// As `generate_test_cert`, with `localhost` as its only SAN.
fn generate_mismatched_test_cert() -> TestCert {
    generate_test_cert_with(CertSubject::DnsLocalhost, true)
}

fn generate_test_cert_with(subject: CertSubject, valid: bool) -> TestCert {
    use openssl::asn1::Asn1Time;
    use openssl::bn::{BigNum, MsbOption};
    use openssl::hash::MessageDigest;
    use openssl::pkey::PKey;
    use openssl::rsa::Rsa;
    use openssl::x509::extension::{BasicConstraints, SubjectAlternativeName};
    use openssl::x509::{X509, X509NameBuilder};

    let rsa = Rsa::generate(2048).expect("failed to generate an RSA key for the test cert");
    let key = PKey::from_rsa(rsa).expect("failed to wrap the RSA key");

    let mut name = X509NameBuilder::new().expect("failed to start a name builder");
    name.append_entry_by_text("CN", "127.0.0.1").expect("failed to set CN");
    let name = name.build();

    let mut serial = BigNum::new().expect("failed to allocate a serial number");
    serial
        .rand(64, MsbOption::MAYBE_ZERO, false)
        .expect("failed to randomize the serial number");

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs() as i64;
    let (not_before, not_after) = if valid {
        (
            Asn1Time::days_from_now(0).expect("not-before time"),
            Asn1Time::days_from_now(1).expect("not-after time"),
        )
    } else {
        (
            Asn1Time::from_unix(now - 3 * 86400).expect("past not-before time"),
            Asn1Time::from_unix(now - 2 * 86400).expect("past not-after time"),
        )
    };

    let mut builder = X509::builder().expect("failed to start a certificate builder");
    builder.set_version(2).expect("failed to set the certificate version");
    builder
        .set_serial_number(&serial.to_asn1_integer().expect("serial as an ASN.1 integer"))
        .expect("failed to set the serial number");
    builder.set_subject_name(&name).expect("failed to set the subject name");
    builder.set_issuer_name(&name).expect("failed to set the issuer name (self-signed)");
    builder.set_pubkey(&key).expect("failed to set the public key");
    builder
        .set_not_before(&not_before)
        .expect("failed to set not-before");
    builder
        .set_not_after(&not_after)
        .expect("failed to set not-after");
    builder
        .append_extension(BasicConstraints::new().critical().build().expect("basic constraints"))
        .expect("failed to append basic constraints");
    let mut san = SubjectAlternativeName::new();
    match subject {
        CertSubject::LoopbackIp => {
            san.ip("127.0.0.1");
        }
        CertSubject::DnsLocalhost => {
            san.dns("localhost");
        }
    }
    builder
        .append_extension(san.build(&builder.x509v3_context(None, None)).expect("subject alternative name"))
        .expect("failed to append the subject alternative name");
    builder
        .sign(&key, MessageDigest::sha256())
        .expect("failed to self-sign the test certificate");
    let cert = builder.build();

    let dir = tempfile_dir::TempDir::new("tls_test_cert");
    let cert_path = dir.path().join("cert.pem");
    let key_path = dir.path().join("key.pem");
    std::fs::write(&cert_path, cert.to_pem().expect("certificate as PEM"))
        .expect("failed to write the test certificate");
    std::fs::write(&key_path, key.private_key_to_pem_pkcs8().expect("private key as PEM"))
        .expect("failed to write the test private key");
    TestCert { cert_path, key_path, _dir: Arc::new(dir) }
}

/// As `CannedServer`, but every accepted connection is a TLS server socket
/// (`SslAcceptor::accept`) before the same canned request/response loop runs
/// over it.
struct CannedTlsServer {
    port: u16,
    accepted: Arc<AtomicUsize>,
    handle: Option<thread::JoinHandle<()>>,
    cert: TestCert,
}

impl CannedTlsServer {
    fn start(connections: Vec<Vec<Vec<u8>>>) -> CannedTlsServer {
        CannedTlsServer::start_with_cert(generate_test_cert(), connections)
    }

    /// The same server over a caller-supplied certificate (the expired and
    /// host-mismatch fixtures mint their own) and, for the TLS-1.1 case, a
    /// caller-supplied protocol window.
    fn start_with_cert(cert: TestCert, connections: Vec<Vec<Vec<u8>>>) -> CannedTlsServer {
        CannedTlsServer::start_with(cert, connections, None)
    }

    fn start_with(
        cert: TestCert,
        connections: Vec<Vec<Vec<u8>>>,
        version: Option<openssl::ssl::SslVersion>,
    ) -> CannedTlsServer {
        let acceptor = tls_acceptor(&cert, version);
        let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind loopback");
        let port = listener.local_addr().expect("local address").port();
        let accepted = Arc::new(AtomicUsize::new(0));
        let counter = accepted.clone();
        let handle = thread::spawn(move || {
            serve_tls(listener, acceptor, connections, counter)
        });
        CannedTlsServer { port, accepted, handle: Some(handle), cert }
    }

    fn accepted(&self) -> usize {
        self.accepted.load(Ordering::SeqCst)
    }

    fn finish(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// An `SslAcceptor` for `cert`, optionally pinned to one protocol version.
/// `Some(TLS1_1)` needs security level 0, because OpenSSL 3 refuses to
/// negotiate TLS 1.1 at the default level; the point of that server is the
/// protocol alert, not a working connection.
fn tls_acceptor(cert: &TestCert, version: Option<openssl::ssl::SslVersion>) -> openssl::ssl::SslAcceptor {
    use openssl::ssl::{SslAcceptor, SslFiletype, SslMethod};

    let mut builder = SslAcceptor::mozilla_intermediate_v5(SslMethod::tls_server())
        .expect("failed to start an SslAcceptor");
    builder
        .set_private_key_file(&cert.key_path, SslFiletype::PEM)
        .expect("failed to load the test private key");
    builder
        .set_certificate_file(&cert.cert_path, SslFiletype::PEM)
        .expect("failed to load the test certificate");
    if let Some(v) = version {
        builder.set_min_proto_version(Some(v)).expect("failed to set the minimum version");
        builder.set_max_proto_version(Some(v)).expect("failed to set the maximum version");
        builder.set_security_level(0);
    }
    builder.build()
}

fn serve_tls(
    listener: TcpListener,
    acceptor: openssl::ssl::SslAcceptor,
    connections: Vec<Vec<Vec<u8>>>,
    accepted: Arc<AtomicUsize>,
) {
    let _ = listener.set_nonblocking(true);
    let deadline = Instant::now() + Duration::from_secs(20);
    for responses in connections {
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break Some(stream),
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() > deadline {
                        break None;
                    }
                    thread::sleep(Duration::from_millis(2));
                }
                Err(_) => break None,
            }
        };
        let Some(stream) = stream.take() else { return };
        let _ = stream.set_nonblocking(false);
        let mut tls = match acceptor.accept(stream) {
            Ok(tls) => tls,
            // A client that never completes the handshake (the denial
            // fixtures: the capability check panics before dialing at all)
            // just means this connection is never accepted, which the
            // caller's `accepted()` assertion already covers.
            Err(_) => continue,
        };
        accepted.fetch_add(1, Ordering::SeqCst);
        for response in &responses {
            if !read_http_request(&mut tls) {
                break;
            }
            if tls.write_all(response).is_err() {
                break;
            }
        }
        let _ = tls.shutdown();
    }
}

/// As `assert_project_served`, but the server is `CannedTlsServer` and the
/// compiled binary is started with `SSL_CERT_FILE` pointing at the server's
/// own self-signed certificate, so `std/tls`'s ordinary (non-optional)
/// certificate verification accepts it.
fn assert_project_served_tls(
    name: &str,
    target: &Path,
    connections: Vec<Vec<Vec<u8>>>,
    expected_connections: usize,
    expected: &str,
) {
    let mut server = CannedTlsServer::start(connections);
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
        .arg(server.port.to_string())
        .env("SSL_CERT_FILE", &server.cert.cert_path)
        .output()
        .expect("failed to run the compiled binary");
    server.finish();
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
    assert_eq!(
        server.accepted(),
        expected_connections,
        "'{name}' made an unexpected number of TLS connections"
    );
}

/// As `assert_project_served_denied`, for the TLS server: the capability
/// check runs before any connection, TLS or otherwise, so this needs no
/// `SSL_CERT_FILE` — the denial happens before a handshake could use it.
fn assert_project_served_tls_denied(
    name: &str,
    target: &Path,
    connections: Vec<Vec<Vec<u8>>>,
    needle: &str,
) {
    let mut server = CannedTlsServer::start(connections);
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
        .arg(server.port.to_string())
        .env("SSL_CERT_FILE", &server.cert.cert_path)
        .output()
        .expect("failed to run the compiled binary");
    server.finish();
    assert!(
        !run.status.success(),
        "expected '{name}' to be denied at runtime, but it exited successfully"
    );
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.contains(needle),
        "expected the denial for '{name}' to mention '{needle}', got:\n{stderr}"
    );
}

// ---------------------------------------------------------------------------
// Release goal 1 (stable TLS): the local failure matrix. Every fixture dials
// `127.0.0.1` only. The certificate is minted per server, so no machine store
// is touched: a trusted case points `SSL_CERT_FILE` at the server's own PEM,
// an untrusted one removes that variable, and the private-CA case passes the
// PEM path as an argument and leaves the store alone.
// ---------------------------------------------------------------------------

/// What a one-connection TLS server does after `accept`.
enum TlsBehavior {
    /// Never answer the ClientHello, so the client's handshake times out.
    NeverHandshake,
    /// Complete the handshake, then close the raw socket with no
    /// `close_notify`: the truncation case.
    Truncate,
    /// Complete the handshake, then stay silent until the test says stop.
    Silent,
    /// Complete the handshake, write a partial response head and body, then
    /// write a raw record the client cannot authenticate and hold. The read of
    /// it has to be an error rather than a clean end of stream.
    CorruptRecord,
    /// Accept the TCP connection and close it without ever reading the
    /// ClientHello. That unread data turns the close into a RST, which is the
    /// reset a handshake has to survive as a typed error.
    ResetHandshake,
    /// Complete the handshake and close without reading the client's request,
    /// which is a RST for the same reason.
    ResetRead,
    /// Never answer the ClientHello, and close the socket after a fixed hold
    /// rather than waiting for the test to say stop, so a client with no
    /// deadline still ends.
    NeverHandshakeThenClose,
}

/// A one-connection server for the failure cases `CannedTlsServer`'s
/// request/response loop cannot express: a peer that never speaks, a peer that
/// speaks and then truncates, and a peer that speaks and then goes quiet.
struct RudeTlsServer {
    port: u16,
    accepted: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
    cert: TestCert,
}

impl RudeTlsServer {
    fn start(behavior: TlsBehavior) -> RudeTlsServer {
        let cert = generate_test_cert();
        // The mid-stream garbage is written as a raw record, which only a TLS 1.2
        // connection still reads without an AEAD record layer sealing it; the
        // point here is the client's failure to authenticate, not the version.
        let version = match behavior {
            TlsBehavior::CorruptRecord => Some(openssl::ssl::SslVersion::TLS1_2),
            _ => None,
        };
        let acceptor = tls_acceptor(&cert, version);
        let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind loopback");
        let port = listener.local_addr().expect("local address").port();
        let accepted = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let counter = accepted.clone();
        let stop_flag = stop.clone();
        let handle = thread::spawn(move || {
            let _ = listener.set_nonblocking(true);
            let deadline = Instant::now() + Duration::from_secs(20);
            let stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break Some(stream),
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        if Instant::now() > deadline {
                            break None;
                        }
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(_) => break None,
                }
            };
            let Some(stream) = stream else { return };
            let _ = stream.set_nonblocking(false);
            counter.fetch_add(1, Ordering::SeqCst);
            match behavior {
                TlsBehavior::NeverHandshake => {
                    while !stop_flag.load(Ordering::SeqCst) && Instant::now() < deadline {
                        thread::sleep(Duration::from_millis(5));
                    }
                }
                TlsBehavior::Truncate => {
                    if let Ok(tls) = acceptor.accept(stream) {
                        // Dropping the SslStream frees the SSL object but runs
                        // no SSL_shutdown, so the TcpStream closes with no
                        // close_notify: the truncation a client must detect.
                        drop(tls);
                    }
                }
                TlsBehavior::Silent => {
                    if let Ok(tls) = acceptor.accept(stream) {
                        while !stop_flag.load(Ordering::SeqCst) && Instant::now() < deadline {
                            thread::sleep(Duration::from_millis(5));
                        }
                        drop(tls);
                    }
                }
                TlsBehavior::CorruptRecord => {
                    if let Ok(mut tls) = acceptor.accept(stream) {
                        let _ = tls.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\npartial");
                        // A record that is not a valid sealed record of this
                        // session: content type 21, a length of 2 and two
                        // bytes that are not a real alert. It goes in through
                        // the underlying socket because the acceptor's own
                        // `SslStream` has no public call for an arbitrary
                        // post-handshake record.
                        let _ = tls.get_mut().write_all(&[21, 3, 3, 0, 2, 2, 80]);
                        while !stop_flag.load(Ordering::SeqCst) && Instant::now() < deadline {
                            thread::sleep(Duration::from_millis(5));
                        }
                        drop(tls);
                    }
                }
                TlsBehavior::ResetHandshake => {
                    // The ClientHello is on its way; give it time to land in the
                    // receive queue unread, then let `stream` drop at the end of
                    // this function, which is the RST.
                    thread::sleep(Duration::from_millis(150));
                }
                TlsBehavior::ResetRead => {
                    if let Ok(tls) = acceptor.accept(stream) {
                        // The request the client sends right after the handshake
                        // lands unread, so this close is a RST.
                        thread::sleep(Duration::from_millis(250));
                        drop(tls);
                    }
                }
                TlsBehavior::NeverHandshakeThenClose => {
                    thread::sleep(Duration::from_millis(1200));
                }
            }
        });
        RudeTlsServer { port, accepted, stop, handle: Some(handle), cert }
    }

    fn accepted(&self) -> usize {
        self.accepted.load(Ordering::SeqCst)
    }

    fn finish(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Builds `tests/fixtures/<name>.kp` and runs it with `args`, `SSL_CERT_FILE`
/// and `SSL_CERT_DIR` removed unless `env` sets them. The removal is what makes
/// the untrusted fixtures deterministic: a developer's own `SSL_CERT_FILE` must
/// not stand in for the test CA.
fn run_tls_fixture(name: &str, args: &[String], env: &[(&str, &str)]) -> std::process::Output {
    let bin = build_fixture_bin(name);
    run_tls_binary(&bin, args, env)
}

/// Builds `tests/fixtures/<name>.kp` and returns the binary, so a test that has to
/// time the *run* rather than the build can do the two separately.
fn build_fixture_bin(name: &str) -> PathBuf {
    let bin = out_path(name);
    let build = kelpc()
        .arg("build")
        .arg(fixture(&format!("tests/fixtures/{name}.kp")))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected '{name}' to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    bin
}

/// Runs an already-built fixture binary with `args`, `SSL_CERT_FILE` and
/// `SSL_CERT_DIR` removed unless `env` sets them.
fn run_tls_binary(bin: &Path, args: &[String], env: &[(&str, &str)]) -> std::process::Output {
    let mut cmd = Command::new(bin);
    for a in args {
        cmd.arg(a);
    }
    cmd.env_remove("SSL_CERT_FILE");
    cmd.env_remove("SSL_CERT_DIR");
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().expect("failed to run the compiled binary")
}

/// `run_tls_fixture` for a fixture that must exit zero, returning its stdout.
fn tls_fixture_stdout(name: &str, args: &[String], env: &[(&str, &str)]) -> String {
    let out = run_tls_fixture(name, args, env);
    assert!(
        out.status.success(),
        "expected '{name}' to exit successfully, got {:?}:\nstdout: {}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

#[test]
fn a_tls_handle_cannot_be_read_as_an_integer() {
    // Release goal 1: `TlsStream.handle` is the opaque `tls.TlsHandle`, so the
    // `usize` the module used to carry is gone and any integer use is a type
    // error. `std/tls` is compiler-shipped, so the fixture names the handle
    // with no `[extern.*]` section of its own.
    assert_rejected_with(
        "tests/fixtures/tls_handle_not_an_integer.kp",
        "cannot assign value of type 'std.tls.TlsHandle' to 'n' declared as 'usize'",
    );
}

#[test]
fn a_tls_handle_cannot_be_forged_from_an_integer() {
    // The other direction: no conversion from an integer to a handle exists,
    // so "no TLS" cannot be spelled `0` at a handle's type.
    assert_rejected_with(
        "tests/fixtures/tls_handle_not_forged.kp",
        "declared to return 'std.tls.TlsHandle' but body evaluates to 'i32'",
    );
}

#[test]
fn every_try_tls_operation_succeeds_against_a_local_server() {
    // The Ok path of `try_handshake`, `try_write`, `try_write_all`,
    // `try_read`, `try_read_into` and `try_shutdown`. `SSL_CERT_FILE` is the
    // server's own certificate, which is exactly the default store `std/tls`
    // reads on NixOS and every other machine.
    let mut server = CannedTlsServer::start(one_connection(&[BODY_200_HELLO, BODY_200_HELLO]));
    let ssl = server.cert.cert_path.to_str().expect("the cert path is utf-8").to_string();
    let stdout = tls_fixture_stdout(
        "tls_try_ok",
        &[server.port.to_string()],
        &[("SSL_CERT_FILE", ssl.as_str())],
    );
    server.finish();
    assert_eq!(stdout, "true\ntrue\ntrue\ntrue\ntrue\n");
    assert_eq!(server.accepted(), 1, "the client opened more than one connection");
}

#[test]
fn a_certificate_outside_the_store_is_untrusted() {
    // No `SSL_CERT_FILE`: the minted certificate is the only chain, and the
    // machine's store does not vouch for it.
    let mut server = CannedTlsServer::start(one_connection(&[BODY_200_HELLO]));
    let stdout = tls_fixture_stdout(
        "tls_error",
        &[server.port.to_string(), "127.0.0.1".to_string()],
        &[],
    );
    server.finish();
    assert_eq!(stdout, "untrusted\n");
}

#[test]
fn an_expired_certificate_is_reported_as_expired() {
    let mut server = CannedTlsServer::start_with_cert(
        generate_expired_test_cert(),
        one_connection(&[BODY_200_HELLO]),
    );
    let ssl = server.cert.cert_path.to_str().expect("the cert path is utf-8").to_string();
    let stdout = tls_fixture_stdout(
        "tls_error",
        &[server.port.to_string(), "127.0.0.1".to_string()],
        &[("SSL_CERT_FILE", ssl.as_str())],
    );
    server.finish();
    assert_eq!(stdout, "expired\n");
}

#[test]
fn a_certificate_that_names_another_host_is_refused() {
    // The certificate's only SAN is `localhost`; the client dials and verifies
    // against `127.0.0.1`, so the name check refuses it even though the store
    // trusts the certificate.
    let mut server = CannedTlsServer::start_with_cert(
        generate_mismatched_test_cert(),
        one_connection(&[BODY_200_HELLO]),
    );
    let ssl = server.cert.cert_path.to_str().expect("the cert path is utf-8").to_string();
    let stdout = tls_fixture_stdout(
        "tls_error",
        &[server.port.to_string(), "127.0.0.1".to_string()],
        &[("SSL_CERT_FILE", ssl.as_str())],
    );
    server.finish();
    assert_eq!(stdout, "host_mismatch\n");
}

#[test]
fn a_tls_1_1_only_server_is_reported_as_protocol_too_old() {
    use openssl::ssl::SslVersion;

    let mut server = CannedTlsServer::start_with(
        generate_test_cert(),
        one_connection(&[BODY_200_HELLO]),
        Some(SslVersion::TLS1_1),
    );
    let ssl = server.cert.cert_path.to_str().expect("the cert path is utf-8").to_string();
    let stdout = tls_fixture_stdout(
        "tls_error",
        &[server.port.to_string(), "127.0.0.1".to_string()],
        &[("SSL_CERT_FILE", ssl.as_str())],
    );
    server.finish();
    assert_eq!(stdout, "protocol_too_old\n");
}

#[test]
fn a_stream_that_ends_without_a_close_notify_is_a_truncation() {
    let mut server = RudeTlsServer::start(TlsBehavior::Truncate);
    let ssl = server.cert.cert_path.to_str().expect("the cert path is utf-8").to_string();
    let stdout = tls_fixture_stdout(
        "tls_truncated",
        &[server.port.to_string()],
        &[("SSL_CERT_FILE", ssl.as_str())],
    );
    server.finish();
    assert_eq!(stdout, "peer_closed_uncleanly\n");
    assert_eq!(server.accepted(), 1, "the client opened more than one connection");
}

#[test]
fn a_silent_peer_times_out_the_handshake() {
    let mut server = RudeTlsServer::start(TlsBehavior::NeverHandshake);
    let stdout = tls_fixture_stdout("tls_timeout_handshake", &[server.port.to_string()], &[]);
    server.finish();
    assert_eq!(stdout, "timeout\n");
}

#[test]
fn a_silent_peer_times_out_a_read() {
    // The handshake completes, and then the server says nothing: the read
    // deadline `set_timeout` applies is what turns that into `Timeout`.
    let mut server = RudeTlsServer::start(TlsBehavior::Silent);
    let ssl = server.cert.cert_path.to_str().expect("the cert path is utf-8").to_string();
    let stdout = tls_fixture_stdout(
        "tls_timeout_read",
        &[server.port.to_string()],
        &[("SSL_CERT_FILE", ssl.as_str())],
    );
    server.finish();
    assert_eq!(stdout, "timeout\n");
}

#[test]
fn a_private_ca_file_is_added_to_the_roots() {
    // No `SSL_CERT_FILE`: the certificate is trusted only because
    // `handshake_with`'s `TlsOptions.ca_file` names it.
    let mut server = CannedTlsServer::start(one_connection(&[BODY_200_HELLO]));
    let ca = server.cert.cert_path.to_str().expect("the cert path is utf-8").to_string();
    let stdout = tls_fixture_stdout("tls_ca_file", &[server.port.to_string(), ca], &[]);
    server.finish();
    assert_eq!(stdout, "ok\n");
    assert_eq!(server.accepted(), 1, "the client opened more than one connection");
}

// ---------------------------------------------------------------------------
// Release goal 1 (stable TLS), part 2: the hostile-conditions matrix. Every case
// dials 127.0.0.1 only and runs against a server started in this thread, with the
// same per-server throwaway certificate the failure matrix above uses. What these
// cases add are the conditions a happy path never reaches: a body far larger than
// one read, a peer that dribbles, a peer that resets, a peer that alerts
// mid-record, many clients at once, a silent peer with no deadline at all, the
// SNI name itself, an `https` redirect to a second local port, `Connection:
// close` against keep-alive, and a header block larger than the read buffer.
// ---------------------------------------------------------------------------

/// A digest as lowercase hex, the spelling `std/encoding`'s `hex_encode` gives, so
/// a fixture's own SHA-1 can be compared with this file's `test_sha1`.
fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// Byte `i` of the 1 MiB payload `tls_partial.kp` builds, matching that fixture's
/// `payload_byte`.
fn partial_payload_byte(i: usize) -> u8 {
    ((i * 31 + 7) % 256) as u8
}

fn partial_payload(n: usize) -> Vec<u8> {
    (0..n).map(partial_payload_byte).collect()
}

/// The 10 MiB body case (a) reads back. A position-dependent pattern rather than a
/// constant, so a chunk that duplicated or dropped a byte moves the SHA-1.
fn big_body(n: usize) -> Vec<u8> {
    (0..n).map(|i| ((i * 131 + 17) % 253) as u8).collect()
}

/// The body `SlowTlsServer` writes one byte at a time.
fn slow_reply(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i % 251) as u8).collect()
}

/// The server for case (b): drain the client's 1 MiB one byte at a time, record
/// what arrived, then answer with its own body one byte at a time. An `SSL_read`
/// of one byte still consumes a whole record underneath, so this drives the
/// partial-read and partial-write loops of one stream in both directions.
struct SlowTlsServer {
    port: u16,
    received: Arc<Mutex<Option<Vec<u8>>>>,
    handle: Option<thread::JoinHandle<()>>,
    cert: TestCert,
}

impl SlowTlsServer {
    fn start(message_len: usize, reply_len: usize) -> SlowTlsServer {
        let cert = generate_test_cert();
        let acceptor = tls_acceptor(&cert, None);
        let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind loopback");
        let port = listener.local_addr().expect("local address").port();
        let received = Arc::new(Mutex::new(None));
        let slot = received.clone();
        let handle = thread::spawn(move || {
            let _ = listener.set_nonblocking(true);
            let deadline = Instant::now() + Duration::from_secs(20);
            let stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break Some(stream),
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        if Instant::now() > deadline {
                            break None;
                        }
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(_) => break None,
                }
            };
            let Some(stream) = stream else { return };
            let _ = stream.set_nonblocking(false);
            let mut tls = acceptor.accept(stream).expect("the slow server's handshake");
            let mut one = [0u8; 1];
            let mut got = Vec::with_capacity(message_len);
            for _ in 0..message_len {
                if tls.read_exact(&mut one).is_err() {
                    break;
                }
                got.push(one[0]);
            }
            *slot.lock().expect("the received slot") = Some(got);
            for b in slow_reply(reply_len) {
                if tls.write_all(std::slice::from_ref(&b)).is_err() {
                    break;
                }
            }
            let _ = tls.shutdown();
        });
        SlowTlsServer { port, received, handle: Some(handle), cert }
    }

    fn finish(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }

    fn received(&self) -> Vec<u8> {
        self.received.lock().expect("the received slot").clone().unwrap_or_default()
    }
}

/// The server for case (e): one TLS connection per client, and each one echoes the
/// request head it read back byte for byte, then shuts down cleanly. A client
/// compares the echo against what it sent, so two streams that crossed produce a
/// different request, and the accept count pins that every client really got its
/// own connection.
struct EchoTlsServer {
    port: u16,
    accepted: Arc<AtomicUsize>,
    handle: Option<thread::JoinHandle<()>>,
    cert: TestCert,
}

impl EchoTlsServer {
    fn start(clients: usize) -> EchoTlsServer {
        let cert = generate_test_cert();
        let acceptor = Arc::new(tls_acceptor(&cert, None));
        let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind loopback");
        let port = listener.local_addr().expect("local address").port();
        let accepted = Arc::new(AtomicUsize::new(0));
        let counter = accepted.clone();
        let handle = thread::spawn(move || {
            let _ = listener.set_nonblocking(true);
            let deadline = Instant::now() + Duration::from_secs(60);
            let mut workers = Vec::new();
            for _ in 0..clients {
                let stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break Some(stream),
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            if Instant::now() > deadline {
                                break None;
                            }
                            thread::sleep(Duration::from_millis(1));
                        }
                        Err(_) => break None,
                    }
                };
                let Some(stream) = stream else { break };
                let _ = stream.set_nonblocking(false);
                let acceptor = acceptor.clone();
                let counter = counter.clone();
                workers.push(thread::spawn(move || {
                    let mut tls = acceptor.accept(stream).expect("the echo server's handshake");
                    counter.fetch_add(1, Ordering::SeqCst);
                    let mut head: Vec<u8> = Vec::new();
                    let mut one = [0u8; 1];
                    while tls.read(&mut one).unwrap_or(0) == 1 {
                        head.push(one[0]);
                        if head.ends_with(b"\r\n\r\n") {
                            break;
                        }
                    }
                    tls.write_all(&head).expect("the echo write");
                    let _ = tls.shutdown();
                }));
            }
            for worker in workers {
                let _ = worker.join();
            }
        });
        EchoTlsServer { port, accepted, handle: Some(handle), cert }
    }

    fn accepted(&self) -> usize {
        self.accepted.load(Ordering::SeqCst)
    }

    fn finish(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// The server for case (g): it records the server name the client sent through SNI
/// before it answers, so the test compares it with the host string the fixture
/// handed `handshake` rather than with whatever the socket resolved to. The
/// certificate carries only a `localhost` DNS name, which is what makes the
/// fixture's `handshake(s, "localhost")` verify even though it dialed 127.0.0.1.
struct SniTlsServer {
    port: u16,
    names: Arc<Mutex<Vec<String>>>,
    handle: Option<thread::JoinHandle<()>>,
    cert: TestCert,
}

impl SniTlsServer {
    fn start() -> SniTlsServer {
        let cert = generate_mismatched_test_cert();
        let acceptor = tls_acceptor(&cert, None);
        let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind loopback");
        let port = listener.local_addr().expect("local address").port();
        let names = Arc::new(Mutex::new(Vec::new()));
        let slot = names.clone();
        let handle = thread::spawn(move || {
            let _ = listener.set_nonblocking(true);
            let deadline = Instant::now() + Duration::from_secs(20);
            let stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break Some(stream),
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        if Instant::now() > deadline {
                            break None;
                        }
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(_) => break None,
                }
            };
            let Some(stream) = stream else { return };
            let _ = stream.set_nonblocking(false);
            let mut tls = acceptor.accept(stream).expect("the SNI server's handshake");
            let name = tls
                .ssl()
                .servername(openssl::ssl::NameType::HOST_NAME)
                .unwrap_or("")
                .to_string();
            slot.lock().expect("the SNI slot").push(name);
            if read_http_request(&mut tls) {
                let _ = tls.write_all(BODY_200_HELLO);
            }
            let _ = tls.shutdown();
        });
        SniTlsServer { port, names, handle: Some(handle), cert }
    }

    fn finish(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }

    fn names(&self) -> Vec<String> {
        self.names.lock().expect("the SNI slot").clone()
    }
}

#[test]
fn a_ten_mib_body_is_byte_for_byte_at_small_and_large_read_sizes() {
    // (a) The same 10 MiB body read in 1 KiB pieces and in 64 KiB pieces. Both
    // the length and the SHA-1 have to match the bytes the server actually sent,
    // so a chunk boundary that lost or duplicated a byte is caught.
    let body = big_body(10 * 1024 * 1024);
    // The payload goes straight onto the stream: this case is about the read
    // loop's chunking, so the body's end is a clean `close_notify` rather than a
    // header the fixture would have to parse. The `std/http` 10 MiB path is the
    // same framing `tls_partial` and the project tests already exercise.
    let expected = format!("{}\n{}\n", body.len(), hex_lower(&test_sha1(&body)));
    for mode in ["small", "large"] {
        let mut server = CannedTlsServer::start(one_connection(&[body.as_slice()]));
        let ssl = server.cert.cert_path.to_str().expect("the cert path is utf-8").to_string();
        let stdout = tls_fixture_stdout(
            "tls_big_body",
            &[server.port.to_string(), mode.to_string()],
            &[("SSL_CERT_FILE", ssl.as_str())],
        );
        server.finish();
        assert_eq!(stdout, expected, "the body read in {mode} pieces did not match");
        assert_eq!(server.accepted(), 1, "the client opened more than one connection");
    }
}

#[test]
fn one_mib_written_through_write_all_arrives_whole_against_a_dribbling_peer() {
    // (b) The client hands 1 MiB to `write_all` in one call while the server
    // drains one byte at a time, and the server answers one byte at a time while
    // the client reads 4 KiB at once. Both sides hash what they saw.
    const SENT: usize = 1048576;
    const REPLY: usize = 200_000;
    let mut server = SlowTlsServer::start(SENT, REPLY);
    let ssl = server.cert.cert_path.to_str().expect("the cert path is utf-8").to_string();
    let stdout = tls_fixture_stdout(
        "tls_partial",
        &[server.port.to_string(), REPLY.to_string()],
        &[("SSL_CERT_FILE", ssl.as_str())],
    );
    server.finish();
    let reply = slow_reply(REPLY);
    let expected = format!(
        "{SENT}\n{}\n{}\n",
        reply.len(),
        hex_lower(&test_sha1(&reply))
    );
    assert_eq!(stdout, expected);
    assert_eq!(
        hex_lower(&test_sha1(&server.received())),
        hex_lower(&test_sha1(&partial_payload(SENT))),
        "the server did not drain exactly what `write_all` sent"
    );
}

#[test]
fn a_record_that_fails_authentication_mid_stream_is_not_a_clean_end_of_stream() {
    // A partial response, then a record the client cannot authenticate. The read
    // must be an error: a peer that wrote garbage mid-body may never look like a
    // complete response. (`openssl`'s crate exposes no way to send a fatal alert
    // after the handshake, since every record is authenticated by then, so the
    // "peer that gives up mid-stream" signal is a corrupted record here and a
    // `close_notify` in `a_close_notify_mid_body_is_not_a_complete_response`.)
    let mut server = RudeTlsServer::start(TlsBehavior::CorruptRecord);
    let ssl = server.cert.cert_path.to_str().expect("the cert path is utf-8").to_string();
    let stdout = tls_fixture_stdout(
        "tls_bad_record",
        &[server.port.to_string()],
        &[("SSL_CERT_FILE", ssl.as_str())],
    );
    server.finish();
    let stdout = stdout.trim();
    assert_ne!(stdout, "clean", "a bad record was reported as a clean end of stream");
    assert!(
        stdout.starts_with("io ") || stdout == "peer_closed_uncleanly",
        "unexpected error for a bad record: {stdout}"
    );
}

#[test]
fn a_close_notify_mid_body_is_not_a_complete_response() {
    // (c) The alert a peer can actually send mid-stream: a clean `close_notify`
    // where a `Content-Length` still promised more. `std/tls` is right to report
    // a clean end of stream; `std/http` is what must refuse to call the short
    // body complete, and this projects the whole path through `http.get`.
    let response = b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\npartial";
    assert_project_served_tls_denied(
        "https_get",
        &project("https_get"),
        one_connection(&[response]),
        "the connection closed before the whole body arrived",
    );
}

#[test]
fn a_tcp_reset_during_the_handshake_is_a_typed_error() {
    // (d) The server accepts and closes with the ClientHello unread, which is a
    // RST. The handshake has to come back with an error at once.
    let mut server = RudeTlsServer::start(TlsBehavior::ResetHandshake);
    let ssl = server.cert.cert_path.to_str().expect("the cert path is utf-8").to_string();
    let stdout = tls_fixture_stdout(
        "tls_reset_handshake",
        &[server.port.to_string()],
        &[("SSL_CERT_FILE", ssl.as_str())],
    );
    server.finish();
    let stdout = stdout.trim();
    assert!(
        stdout.starts_with("io ") || stdout == "peer_closed_uncleanly",
        "unexpected error for a reset handshake: {stdout}"
    );
    assert_eq!(server.accepted(), 1, "the client opened more than one connection");
}

#[test]
fn a_tcp_reset_during_a_read_is_a_typed_error() {
    // (d) The handshake completes, the client sends a request, and the server
    // closes with that request unread, which is a RST. The write and the read are
    // both reported, and neither may look like a clean end of stream.
    let mut server = RudeTlsServer::start(TlsBehavior::ResetRead);
    let ssl = server.cert.cert_path.to_str().expect("the cert path is utf-8").to_string();
    let stdout = tls_fixture_stdout(
        "tls_reset_read",
        &[server.port.to_string()],
        &[("SSL_CERT_FILE", ssl.as_str())],
    );
    server.finish();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 2, "expected a write line and a read line, got:\n{stdout}");
    assert!(!stdout.contains("clean"), "a reset was reported as a clean read:\n{stdout}");
    assert!(!stdout.contains("data"), "a reset produced data:\n{stdout}");
    assert_eq!(server.accepted(), 1, "the client opened more than one connection");
}

#[test]
fn sixty_four_concurrent_clients_get_their_own_streams_and_leak_no_descriptors() {
    // (e) 64 workers, each with its own handshake, request, echo check and close.
    // The fixture prints how many read back their own request and the
    // `/proc/self/fd` count before and after, so a crossed stream and a leaked
    // descriptor are both failures here.
    const CLIENTS: usize = 64;
    let mut server = EchoTlsServer::start(CLIENTS);
    let ssl = server.cert.cert_path.to_str().expect("the cert path is utf-8").to_string();
    let stdout = tls_fixture_stdout(
        "tls_concurrent",
        &[server.port.to_string()],
        &[("SSL_CERT_FILE", ssl.as_str())],
    );
    server.finish();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 3, "expected the ok count and two fd counts, got:\n{stdout}");
    let ok: i64 = lines[0].parse().expect("the ok count is a number");
    let before: i64 = lines[1].parse().expect("the before count is a number");
    let after: i64 = lines[2].parse().expect("the after count is a number");
    assert_eq!(ok, CLIENTS as i64, "workers that did not read their own echo:\n{stdout}");
    assert_eq!(after, before, "leaked descriptors: {before} before, {after} after");
    assert_eq!(server.accepted(), CLIENTS, "the server accepted a different number of connections");
}

#[test]
fn a_silent_peer_respects_the_socket_deadline_and_has_none_by_default() {
    // (f) The same never-answering server for both halves, closing after a fixed
    // 1200 ms hold. With a 400 ms deadline the client gives up first and reports
    // `Timeout`; without one it waits for the close, so it cannot finish before
    // the hold and cannot report `Timeout`. That difference is the whole check.
    let hold = Duration::from_millis(900);

    // Built first, so the clock below measures the client's own wait and not
    // the compiler's.
    let no_deadline = build_fixture_bin("tls_handshake_no_timeout");
    let deadline = build_fixture_bin("tls_handshake_socket_timeout");

    let mut without = RudeTlsServer::start(TlsBehavior::NeverHandshakeThenClose);
    let started = Instant::now();
    let out = run_tls_binary(&no_deadline, &[without.port.to_string()], &[]);
    let waited = started.elapsed();
    without.finish();
    assert!(out.status.success(), "the no-timeout client failed:\n{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    assert!(
        !stdout.starts_with("timeout"),
        "a handshake with no deadline reported `{stdout}`"
    );
    assert!(
        waited >= hold,
        "the no-timeout client returned after {waited:?}, before the server's hold"
    );

    let mut with = RudeTlsServer::start(TlsBehavior::NeverHandshakeThenClose);
    let started = Instant::now();
    let out = run_tls_binary(&deadline, &[with.port.to_string()], &[]);
    let waited = started.elapsed();
    with.finish();
    assert!(out.status.success(), "the deadline client failed:\n{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "timeout\n");
    assert!(
        waited < hold,
        "the deadline client waited {waited:?}, past its own 400 ms deadline"
    );
}

#[test]
fn the_server_name_sent_is_the_host_given_to_handshake() {
    // (g) The client dials 127.0.0.1 but hands `handshake` the name `localhost`,
    // and the certificate names only `localhost`. The server records the SNI name
    // it was actually sent, so a handshake that passed the resolved address, or
    // sent no SNI at all, fails here even when verification passes.
    const HOST: &str = "localhost";
    let mut server = SniTlsServer::start();
    let ssl = server.cert.cert_path.to_str().expect("the cert path is utf-8").to_string();
    let stdout = tls_fixture_stdout(
        "tls_sni",
        &[server.port.to_string(), HOST.to_string()],
        &[("SSL_CERT_FILE", ssl.as_str())],
    );
    server.finish();
    assert_eq!(stdout, "ok\n");
    assert_eq!(
        server.names(),
        vec![HOST.to_string()],
        "the server received a different SNI name"
    );
}

#[test]
fn an_https_redirect_reaches_a_second_local_port() {
    // (h) Server B is up first so server A's `Location` can name its port. Both
    // use the same certificate, so the one `SSL_CERT_FILE` the client is given
    // trusts both hops; the body can only come from B.
    let cert = generate_test_cert();
    let mut second = CannedTlsServer::start_with_cert(cert.clone(), one_connection(&[BODY_200_HELLO]));
    let location = format!(
        "HTTP/1.1 301 Moved Permanently\r\nLocation: https://127.0.0.1:{}/final\r\nContent-Length: 0\r\n\r\n",
        second.port
    );
    let mut first = CannedTlsServer::start_with_cert(cert, one_connection(&[location.as_bytes()]));

    let bin = out_path("https_redirect");
    let build = kelpc()
        .arg("build")
        .arg(project("https_redirect"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected the https_redirect project to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let ssl = first.cert.cert_path.to_str().expect("the cert path is utf-8").to_string();
    let run = Command::new(&bin)
        .arg(first.port.to_string())
        .env_remove("SSL_CERT_FILE")
        .env_remove("SSL_CERT_DIR")
        .env("SSL_CERT_FILE", ssl)
        .output()
        .expect("failed to run the redirect client");
    first.finish();
    second.finish();

    assert!(
        run.status.success(),
        "expected the redirect client to exit successfully, got:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout), "200\nhello\ntrue\n");
    assert_eq!(first.accepted(), 1, "the redirecting server served a different number of connections");
    assert_eq!(second.accepted(), 1, "the redirect target served a different number of connections");
}

#[test]
fn connection_close_is_dropped_from_the_pool_and_keep_alive_is_kept() {
    // (i) The first connection answers `Connection: close`, so the pool has to
    // drop it and dial again for the second request; the third request then rides
    // the second connection. Two accepted connections is the whole assertion.
    const CLOSED: &[u8] =
        b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 5\r\n\r\nfirst";
    let connections = vec![
        vec![CLOSED.to_vec()],
        vec![BODY_200_HELLO.to_vec(), BODY_200_HELLO.to_vec()],
    ];
    assert_project_served(
        "http_close_vs_keepalive",
        &project("http_close_vs_keepalive"),
        connections,
        2,
        "200\nfalse\n0\n200\ntrue\n1\n200\ntrue\n1\n",
    );
}

#[test]
fn a_hundred_kib_header_block_survives_std_http() {
    // (j) The response head is 100 000 bytes of header, far more than the 4 KiB
    // the client reads at a time, so locating the blank line and copying the head
    // both span many refills. The body after it has to start at the right byte.
    let big = "a".repeat(100_000);
    let response =
        format!("HTTP/1.1 200 OK\r\nX-Big: {big}\r\nContent-Length: 4\r\n\r\ndone").into_bytes();
    assert_project_served_tls(
        "https_big_headers",
        &project("https_big_headers"),
        one_connection(&[response.as_slice()]),
        1,
        &format!("200\n{}\ndone\n", big.len()),
    );
}

/// The C backend's `kelpc`, whatever backend the enclosing run exercises: the
/// AddressSanitizer gate below needs the generated `.c` to instrument, and only
/// the C backend writes one.
fn kelpc_c() -> Command {
    let mut cmd = Command::new(kelpc_bin());
    cmd.env_remove("KELPC_BACKEND");
    cmd.env("KELPC_ORACLE", "c");
    cmd
}

/// Builds `tests/fixtures/<name>.kp` through the C backend, rebuilds its generated
/// `.c` with AddressSanitizer, runs it with `args` and `env`, and returns its
/// stdout. A report from AddressSanitizer or LeakSanitizer fails the test; a
/// non-zero exit that is the program's own typed-error path is fine.
fn asan_tls_fixture(name: &str, args: &[String], env: &[(&str, &str)]) -> String {
    let bin = out_path(&format!("{name}_asan_src"));
    let build = kelpc_c()
        .arg("build")
        .arg(fixture(&format!("tests/fixtures/{name}.kp")))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected '{name}' to compile through the C backend, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let asan_bin = out_path(&format!("{name}_asan"));
    let cc = Command::new("cc")
        .arg("-fsanitize=address")
        .arg("-O1")
        .arg("-o")
        .arg(&asan_bin)
        .arg(bin.with_extension("c"))
        .args(["-fwrapv", "-pthread", "-lm", "-lssl", "-lcrypto"])
        .output()
        .expect("failed to spawn cc");
    assert!(
        cc.status.success(),
        "the AddressSanitizer build of '{name}' failed:\n{}",
        String::from_utf8_lossy(&cc.stderr)
    );
    let mut cmd = Command::new(&asan_bin);
    for a in args {
        cmd.arg(a);
    }
    cmd.env_remove("SSL_CERT_FILE");
    cmd.env_remove("SSL_CERT_DIR");
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.env("ASAN_OPTIONS", "detect_leaks=1");
    let run = cmd.output().expect("failed to run the instrumented binary");
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        !stderr.contains("AddressSanitizer") && !stderr.contains("LeakSanitizer"),
        "AddressSanitizer flagged '{name}':\n{stderr}"
    );
    String::from_utf8_lossy(&run.stdout).to_string()
}

#[test]
fn tls_handshakes_free_their_openssl_objects_under_address_sanitizer() {
    // The leak gate for `SSL*`/`SSL_CTX*`: every handshake path, on the way out
    // and on the way down, is run under AddressSanitizer's leak checker. It is
    // the closest thing to `memory_gate.rs` this machine has (there is no
    // valgrind here), and it is the C backend's generated translation unit that
    // gets instrumented, which is the same C the LLVM backend compiles into its
    // per-program object.
    //
    // The success path first: every `try_` operation, one connection.
    {
        let mut server = CannedTlsServer::start(one_connection(&[BODY_200_HELLO, BODY_200_HELLO]));
        let ssl = server.cert.cert_path.to_str().expect("the cert path is utf-8").to_string();
        let stdout = asan_tls_fixture(
            "tls_try_ok",
            &[server.port.to_string()],
            &[("SSL_CERT_FILE", ssl.as_str())],
        );
        server.finish();
        assert_eq!(stdout, "true\ntrue\ntrue\ntrue\ntrue\n");
    }

    // The pooled path's own free: `Pool.close_all` ends each kept-alive
    // connection, which is the only thing that frees the `SSL*` a pooled
    // handshake allocated.
    {
        let mut server = CannedTlsServer::start(one_connection(&[BODY_200_HELLO, BODY_200_HELLO]));
        let ssl = server.cert.cert_path.to_str().expect("the cert path is utf-8").to_string();
        let stdout = asan_tls_fixture(
            "tls_pool_close",
            &[server.port.to_string()],
            &[("SSL_CERT_FILE", ssl.as_str())],
        );
        server.finish();
        assert_eq!(stdout, "200\ntrue\n1\n200\n1\n0\n");
        assert_eq!(server.accepted(), 1, "the pooled client opened more than one connection");
    }

    // A handshake that fails halfway: the certificate is not in the store, so
    // `SSL_connect` fails and the object it built has to be freed.
    {
        let mut server = CannedTlsServer::start(one_connection(&[BODY_200_HELLO]));
        let stdout = asan_tls_fixture(
            "tls_error",
            &[server.port.to_string(), "127.0.0.1".to_string()],
            &[],
        );
        server.finish();
        assert_eq!(stdout, "untrusted\n");
    }

    // A handshake that times out: the socket deadline fires inside `SSL_connect`.
    {
        let mut server = RudeTlsServer::start(TlsBehavior::NeverHandshake);
        let stdout = asan_tls_fixture("tls_timeout_handshake", &[server.port.to_string()], &[]);
        server.finish();
        assert_eq!(stdout, "timeout\n");
    }

    // A peer that resets during the handshake.
    {
        let mut server = RudeTlsServer::start(TlsBehavior::ResetHandshake);
        let ssl = server.cert.cert_path.to_str().expect("the cert path is utf-8").to_string();
        let stdout = asan_tls_fixture(
            "tls_reset_handshake",
            &[server.port.to_string()],
            &[("SSL_CERT_FILE", ssl.as_str())],
        );
        server.finish();
        assert!(!stdout.trim().is_empty(), "the reset handshake fixture printed nothing");
    }

    // A peer that truncates after a successful handshake, and one that alerts
    // mid-stream: the `SSL*` a successful handshake allocated is freed by
    // `try_shutdown` on the reader's way out.
    {
        let mut server = RudeTlsServer::start(TlsBehavior::Truncate);
        let ssl = server.cert.cert_path.to_str().expect("the cert path is utf-8").to_string();
        let stdout = asan_tls_fixture(
            "tls_truncated",
            &[server.port.to_string()],
            &[("SSL_CERT_FILE", ssl.as_str())],
        );
        server.finish();
        assert_eq!(stdout, "peer_closed_uncleanly\n");
    }
    {
        let mut server = RudeTlsServer::start(TlsBehavior::CorruptRecord);
        let ssl = server.cert.cert_path.to_str().expect("the cert path is utf-8").to_string();
        let stdout = asan_tls_fixture(
            "tls_bad_record",
            &[server.port.to_string()],
            &[("SSL_CERT_FILE", ssl.as_str())],
        );
        server.finish();
        assert!(!stdout.trim().is_empty(), "the bad-record fixture printed nothing");
    }
}

/// Compiles a project, runs it with the canned server's port as `argv[1]`,
/// asserts its stdout and how many connections the server accepted.
fn assert_project_served(
    name: &str,
    target: &Path,
    connections: Vec<Vec<Vec<u8>>>,
    expected_connections: usize,
    expected: &str,
) {
    let mut server = CannedServer::start(connections);
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
        .arg(server.port.to_string())
        .output()
        .expect("failed to run the compiled binary");
    server.finish();
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
    assert_eq!(
        server.accepted(),
        expected_connections,
        "'{name}' made an unexpected number of connections"
    );
}

/// As `assert_project_served`, for a project expected to be denied at runtime by
/// the capability check while the server is up.
fn assert_project_served_denied(
    name: &str,
    target: &Path,
    connections: Vec<Vec<Vec<u8>>>,
    needle: &str,
) {
    let mut server = CannedServer::start(connections);
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
        .arg(server.port.to_string())
        .output()
        .expect("failed to run the compiled binary");
    server.finish();
    assert!(
        !run.status.success(),
        "expected '{name}' to be denied at runtime, but it exited successfully"
    );
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.contains(needle),
        "expected the denial for '{name}' to mention '{needle}', got:\n{stderr}"
    );
}

#[test]
fn a_pooled_client_reuses_one_connection_for_five_requests() {
    // Five requests, one accepted connection: keep-alive and the pool are what
    // make the second, third, fourth and fifth requests cost nothing.
    assert_project_served(
        "http_get",
        &project("http_get"),
        one_connection(&[
            BODY_200_HELLO,
            BODY_200_HELLO,
            CHUNKED_200,
            STATUS_404,
            BODY_200_CLOSE,
        ]),
        1,
        "200\nhello\ntrue\ntrue\nfalse\nyes\n1\n200\n1\n200\nchunk!\ntrue\n404\nfalse\n200\nbyebye\nfalse\n0\n",
    );
}

#[test]
fn a_relative_redirect_is_followed_on_the_pooled_connection() {
    // Two hops on one connection (a relative `Location` keeps the host), then a
    // POST on a second: `http.post_text` goes through `Client.send`, which
    // never consults a pool.
    assert_project_served(
        "http_redirect",
        &project("http_redirect"),
        vec![
            vec![REDIRECT_301.to_vec(), BODY_200_HELLO.to_vec()],
            vec![echo_response("payload")],
        ],
        2,
        "200\nhello\n1\n200\ngot:payload\n",
    );
}

#[test]
fn rate_limit_headers_parse_and_absence_reads_as_none() {
    // MISSING-p5Xtr: a `429` naming `Retry-After` and the five
    // `X-RateLimit-*` bucket headers, then a plain `200` naming none of
    // them — the second response is the "absent, not a panic" half, and it
    // also exercises `remaining = 0` on the first response reading as
    // `Some(0)` rather than being confused with "absent". Neither response
    // asks to close, so both ride the one connection `BODY_200_HELLO`'s
    // sibling tests already rely on for keep-alive.
    assert_project_served(
        "http_rate_limit",
        &project("http_rate_limit"),
        one_connection(&[STATUS_429_RATE_LIMITED, BODY_200_HELLO]),
        1,
        "429\n2\n10\n0\n1719796939.000000\n0.500000\nabcd1234\n200\nnone\nnone\nnone\nnone\nnone\nnone\n",
    );
}

#[test]
fn the_pool_ceiling_comes_from_the_manifest() {
    // `http_pool = 0` in this project's `coral.toml` reaches `std/http` as the
    // compiled-in ceiling, so the same two requests now cost two connections.
    // This is the regression test for S4's only compiler-side change.
    assert_project_served(
        "http_pool_off",
        &project("http_pool_off"),
        n_connections(BODY_200_HELLO, 2),
        2,
        "0\n200\n200\n0\n",
    );
}

#[test]
fn a_redirect_to_a_host_the_grant_does_not_name_is_denied() {
    // The grant names `127.0.0.1`; the redirect names `localhost`. The check
    // matches the host as written, so the denial happens even though both names
    // are the same machine — and it happens before any connection to port 80.
    assert_project_served_denied(
        "http_redirect_denied",
        &project("http_redirect_denied"),
        one_connection(&[REDIRECT_ELSEWHERE]),
        "outside the net capability granted to module 'std/http'",
    );
}

#[test]
fn an_https_get_completes_the_tls_handshake_and_reads_the_body() {
    // MK5-S4b-M1 (open item 37): `parse_url` no longer panics on `https`, and
    // the same `Client.send` path that plain `http_get` exercises now goes
    // through `std/tls`'s handshake first. One connection, one request: the
    // point is the handshake and the certificate check succeeding against the
    // server's self-signed cert (via `SSL_CERT_FILE`), not keep-alive, which
    // `a_pooled_client_reuses_one_connection_for_five_requests` already covers
    // for plain `http`.
    assert_project_served_tls(
        "https_get",
        &project("https_get"),
        one_connection(&[BODY_200_HELLO]),
        1,
        "200\nhello\ntrue\n",
    );
}

const TRUNCATED_200: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nabc";
const LOOP_302: &[u8] = b"HTTP/1.1 302 Found\r\nLocation: /loop\r\nContent-Length: 0\r\n\r\n";

#[test]
fn a_fallible_send_reports_a_dial_an_early_close_and_a_redirect_loop_as_values() {
    // MISSING-p7Kdm. Three connections on the canned server, in the order the
    // program makes them: a fresh `try_get` whose body stops after 3 of 10
    // bytes, a pooled request that does the same (the pool must not keep that
    // half-read connection), and a pooled redirect loop followed by a good
    // response on the same kept connection (six `302`s to one `Location`
    // exhaust the client's five hops; the seventh response answers the request
    // after it). The refused half dials a port this test released.
    let mut loop_then_hello: Vec<Vec<u8>> = vec![LOOP_302.to_vec(); 6];
    loop_then_hello.push(BODY_200_HELLO.to_vec());
    let mut server = CannedServer::start(vec![
        vec![TRUNCATED_200.to_vec()],
        vec![TRUNCATED_200.to_vec()],
        loop_then_hello,
    ]);
    let dead = TcpListener::bind("127.0.0.1:0").expect("failed to bind loopback");
    let dead_port = dead.local_addr().expect("local address").port();
    drop(dead);

    let bin = out_path("http_try_send");
    let build = kelpc()
        .arg("build")
        .arg(fixture("tests/fixtures/http_try_send.kp"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected the http_try_send fixture to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&bin)
        .arg(dead_port.to_string())
        .arg(server.port.to_string())
        .output()
        .expect("failed to run the client");
    server.finish();
    assert!(
        run.status.success(),
        "expected the client to exit successfully, got:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let cut = "exchange http: the connection closed before the whole body arrived";
    let expected = format!(
        "refused dial\nfresh {cut}\npool-cut {cut}\npool kept 0\n\
         pool-loop redirects http: too many redirects\npool kept 1\n\
         pool-hello ok 200 hello\npool kept 1\nsurvived\n"
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        expected,
        "unexpected stdout for 'http_try_send'"
    );
    assert_eq!(server.accepted(), 3, "the client opened an unexpected number of connections");
}

#[test]
fn a_failed_tls_handshake_is_a_dial_error_value() {
    // MISSING-p7Kdm: the server presents a self-signed certificate and the
    // client is not told to trust it (no `SSL_CERT_FILE`), so the handshake
    // fails. That is `HttpErr.Dial` (nothing was sent), not a panic.
    let mut server = CannedTlsServer::start(one_connection(&[]));
    let bin = out_path("https_try_dial");
    let build = kelpc()
        .arg("build")
        .arg(fixture("tests/fixtures/https_try_dial.kp"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected the https_try_dial fixture to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    // Trust store pinned away from the test cert, so it cannot vouch for it.
    let run = Command::new(&bin)
        .arg(server.port.to_string())
        .env("SSL_CERT_FILE", "/dev/null")
        .env("SSL_CERT_DIR", "/dev/null")
        .output()
        .expect("failed to run the client");
    server.finish();
    assert!(
        run.status.success(),
        "expected the client to exit successfully, got:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "dial true\nsurvived\n",
        "unexpected stdout for 'https_try_dial'"
    );
}

#[test]
fn https_to_a_host_the_grant_does_not_name_is_denied() {
    // Same shape as `a_redirect_to_a_host_the_grant_does_not_name_is_denied`,
    // but for `https` directly: the grant names `127.0.0.1:80`\
    // (the `http` default port), the URL asks for `127.0.0.1:<TLS port>`, and
    // the capability check denies it before a socket is ever opened — so the
    // server here accepts nothing, and `SSL_CERT_FILE` is never consulted.
    assert_project_served_tls_denied(
        "https_get_denied",
        &project("https_get_denied"),
        one_connection(&[BODY_200_HELLO]),
        "outside the net capability granted to module 'std/http'",
    );
}

// ---------------------------------------------------------------------------
// MISSING-k4Nvz: `std/ws`, the WebSocket client. A client nobody has spoken to
// is not shipped, so this is a real server: it reads the handshake, checks the
// `Sec-WebSocket-Key` by computing the accept value the client must compare
// against (a Rust RFC 3174 SHA-1, the test's own copy), answers `101`, and
// then runs one scripted exchange so that every layer the module claims is
// exercised: a text frame, a message split across two continuation frames, a
// ping whose pong is read back, and a close handshake.
//
// Nothing here is canned bytes: the accept value is derived from the key the
// *client* sent, so a wrong `sha1` or a wrong base64 in `std/ws` fails the
// handshake instead of passing a fixture that hard-coded an answer. And every
// frame the server reads is checked for the mask bit and unmasked before its
// payload is compared, so a client that forgot to mask fails here.
// ---------------------------------------------------------------------------

/// The GUID of RFC 6455 1.3, beside the Rust SHA-1 below.
const WS_TEST_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// SHA-1 of `data` (RFC 3174), for the accept value. The test's own copy, so
/// the handshake check does not lean on the module it is checking.
fn test_sha1(data: &[u8]) -> [u8; 20] {
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    for block in msg.chunks(64) {
        let mut w = [0u32; 80];
        for (i, word) in w.iter_mut().take(16).enumerate() {
            *word = u32::from_be_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i / 20 {
                0 => ((b & c) | (!b & d), 0x5A827999u32),
                1 => (b ^ c ^ d, 0x6ED9EBA1),
                2 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }
    let mut out = [0u8; 20];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

/// Standard base64, padded: the accept value is 20 bytes, so this always ends
/// in one `=`.
fn test_base64(data: &[u8]) -> String {
    const ALPHABET: &[u8] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(triple >> 18) as usize & 63] as char);
        out.push(ALPHABET[(triple >> 12) as usize & 63] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[(triple >> 6) as usize & 63] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[triple as usize & 63] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// The client's handshake head, up to (not including) the blank line.
fn read_ws_head<S: Read>(stream: &mut S) -> String {
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            return String::from_utf8_lossy(&buf[..pos]).to_string();
        }
        match stream.read(&mut chunk) {
            Ok(0) => panic!("the client closed during the handshake"),
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) => panic!("failed to read the handshake: {e}"),
        }
    }
}

/// The value of `name` in a handshake head, case-insensitively, or the empty
/// string.
fn ws_header(head: &str, name: &str) -> String {
    head.lines()
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .find(|(n, _)| n.trim().eq_ignore_ascii_case(name))
        .map(|(_, v)| v.trim().to_string())
        .unwrap_or_default()
}

/// One unmasked server frame. Every frame this side sends has `FIN` set except
/// the first half of the fragmented message, which is the `fin` argument.
fn ws_frame(opcode: u8, fin: bool, payload: &[u8]) -> Vec<u8> {
    assert!(payload.len() <= 125, "this gate writes no extended lengths");
    let mut out = vec![(if fin { 0x80 } else { 0 }) | opcode, payload.len() as u8];
    out.extend_from_slice(payload);
    out
}

/// One frame from the client: its opcode and its unmasked payload. The mask
/// bit is required (RFC 6455 5.1), which is the half of the framing a client
/// can get wrong without the server noticing anything else.
fn read_ws_frame<S: Read>(stream: &mut S) -> (u8, Vec<u8>) {
    let mut header = [0u8; 2];
    stream
        .read_exact(&mut header)
        .expect("the client closed instead of sending a frame");
    let opcode = header[0] & 0x0f;
    assert!(
        header[1] & 0x80 != 0,
        "the client sent an unmasked frame (opcode {opcode:#x}), which RFC 6455 5.1 forbids"
    );
    let mut len = (header[1] & 0x7f) as usize;
    if len == 126 {
        let mut ext = [0u8; 2];
        stream.read_exact(&mut ext).expect("short extended length");
        len = u16::from_be_bytes(ext) as usize;
    } else if len == 127 {
        let mut ext = [0u8; 8];
        stream.read_exact(&mut ext).expect("short extended length");
        len = u64::from_be_bytes(ext) as usize;
    }
    let mut key = [0u8; 4];
    stream.read_exact(&mut key).expect("short masking key");
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload).expect("short frame payload");
    for (i, byte) in payload.iter_mut().enumerate() {
        *byte ^= key[i % 4];
    }
    (opcode, payload)
}

fn serve_ws(listener: TcpListener, accepted: Arc<AtomicUsize>) {
    let _ = listener.set_nonblocking(true);
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break Some(stream),
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() > deadline {
                    break None;
                }
                thread::sleep(Duration::from_millis(2));
            }
            Err(_) => break None,
        }
    };
    let Some(stream) = stream.take() else { return };
    // Non-blocking was only for the accept loop; an accepted socket inherits
    // the listener's mode on Linux. The timeouts are what stop a client that
    // stalls mid-handshake from hanging the suite.
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(20)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(20)));
    serve_ws_conn(stream, &accepted);
}

/// The `wss` gate: the same exchange, after the TLS handshake, so the client's
/// own `std/tls` path is what carries the WebSocket framing.
fn serve_ws_tls(
    listener: TcpListener,
    acceptor: openssl::ssl::SslAcceptor,
    accepted: Arc<AtomicUsize>,
) {
    let _ = listener.set_nonblocking(true);
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break Some(stream),
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() > deadline {
                    break None;
                }
                thread::sleep(Duration::from_millis(2));
            }
            Err(_) => break None,
        }
    };
    let Some(stream) = stream.take() else { return };
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(20)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(20)));
    if let Ok(tls) = acceptor.accept(stream) {
        serve_ws_conn(tls, &accepted);
    }
}

/// The scripted exchange, on one accepted connection. Every expectation here
/// is an assertion rather than a return value: a client that gets a step
/// wrong fails the test in this thread, and `WsServer::finish` turns that into
/// the test's own failure.
fn serve_ws_conn<S: Read + Write>(mut stream: S, accepted: &AtomicUsize) {
    accepted.fetch_add(1, Ordering::SeqCst);
    ws_gate_exchange(&mut stream);
}

/// The gate's scripted exchange over any byte stream: plain TCP for `ws`, a
/// TLS server stream for `wss`.
fn ws_gate_exchange<S: Read + Write>(stream: &mut S) {
    let head = read_ws_head(stream);
    assert!(
        head.starts_with("GET /gateway HTTP/1.1"),
        "the request line was not the gate's own path:\n{head}"
    );
    assert!(
        head.to_lowercase().contains("upgrade: websocket")
            && head.to_lowercase().contains("connection: upgrade"),
        "the handshake did not ask for an upgrade:\n{head}"
    );
    assert!(
        ws_header(&head, "sec-websocket-version") == "13",
        "the handshake did not name version 13:\n{head}"
    );
    let key = ws_header(&head, "sec-websocket-key");
    assert!(!key.is_empty(), "the handshake carried no Sec-WebSocket-Key:\n{head}");
    assert_eq!(
        key.len(),
        24,
        "a 16-byte key in base64 is 24 characters, and this was {key:?}"
    );
    let accept = test_base64(&test_sha1(format!("{key}{WS_TEST_GUID}").as_bytes()));
    let response = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n\
         Connection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
    );
    stream
        .write_all(response.as_bytes())
        .expect("failed to write the 101");

    stream.write_all(&ws_frame(0x1, true, b"hello")).expect("text frame");
    stream.write_all(&ws_frame(0x1, false, b"frag")).expect("first fragment");
    stream.write_all(&ws_frame(0x0, true, b"ment")).expect("continuation");
    stream.write_all(&ws_frame(0x9, true, b"ping?")).expect("ping frame");

    let (opcode, payload) = read_ws_frame(stream);
    assert_eq!(
        opcode, 0xA,
        "the client answered the ping with opcode {opcode:#x}, not a pong"
    );
    assert_eq!(payload, b"ping?", "the pong carried a different payload");

    let mut close = 1000u16.to_be_bytes().to_vec();
    close.extend_from_slice(b"done");
    stream.write_all(&ws_frame(0x8, true, &close)).expect("close frame");

    let (opcode, payload) = read_ws_frame(stream);
    assert_eq!(
        opcode, 0x8,
        "the client's closing frame was opcode {opcode:#x}, not a close"
    );
    assert_eq!(
        &payload[..2],
        &1000u16.to_be_bytes(),
        "the client's close code was not 1000"
    );
    assert_eq!(&payload[2..], b"bye", "the client's close reason was not 'bye'");
}

/// The `std/ws` gate: one connection, one scripted exchange. It lives in a
/// thread for the duration of one test, like `CannedServer`.
struct WsServer {
    port: u16,
    accepted: Arc<AtomicUsize>,
    handle: Option<thread::JoinHandle<()>>,
}

impl WsServer {
    fn start() -> WsServer {
        let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind loopback");
        let port = listener.local_addr().expect("local address").port();
        let accepted = Arc::new(AtomicUsize::new(0));
        let counter = accepted.clone();
        let handle = thread::spawn(move || serve_ws(listener, counter));
        WsServer { port, accepted, handle: Some(handle) }
    }

    fn accepted(&self) -> usize {
        self.accepted.load(Ordering::SeqCst)
    }

    /// Waits for the serving thread, so its assertions reach this test: a
    /// panic in that thread is a failure here, not a silent pass.
    fn finish(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.join().expect("the WebSocket gate thread failed; see its message");
        }
    }
}

/// The `wss` twin of `WsServer`: a TLS acceptor in front of the same scripted
/// exchange, so the client's own `std/tls` handshake and encrypted framing are
/// what the test exercises.
struct WsTlsServer {
    port: u16,
    accepted: Arc<AtomicUsize>,
    handle: Option<thread::JoinHandle<()>>,
    cert: TestCert,
}

impl WsTlsServer {
    fn start() -> WsTlsServer {
        let cert = generate_test_cert();
        let acceptor = tls_acceptor(&cert, None);
        let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind loopback");
        let port = listener.local_addr().expect("local address").port();
        let accepted = Arc::new(AtomicUsize::new(0));
        let counter = accepted.clone();
        let handle = thread::spawn(move || serve_ws_tls(listener, acceptor, counter));
        WsTlsServer { port, accepted, handle: Some(handle), cert }
    }

    fn accepted(&self) -> usize {
        self.accepted.load(Ordering::SeqCst)
    }

    fn finish(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.join().expect("the wss gate thread failed; see its message");
        }
    }
}

#[test]
fn a_websocket_client_speaks_the_handshake_and_every_frame_kind() {
    let mut server = WsServer::start();
    let bin = out_path("ws_gate");
    let build = kelpc()
        .arg("build")
        .arg(fixture("tests/fixtures/ws_gate.kp"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected the ws_gate fixture to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&bin)
        .arg(server.port.to_string())
        .output()
        .expect("failed to run the client");
    server.finish();
    assert!(
        run.status.success(),
        "expected the client to exit successfully, got:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let expected = format!(
        "url 127.0.0.1 {} /gateway\nconnected\ntext hello\nfrag fragment\n\
         ping ping?\nclose 1000 done\nclosed\n",
        server.port
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        expected,
        "unexpected stdout for 'ws_gate'"
    );
    assert_eq!(server.accepted(), 1, "the client opened more than one connection");
}

#[test]
fn a_wss_client_runs_the_websocket_framing_over_tls() {
    // The replacement for the old `wss_is_refused_by_name_because_v1_has_no_tls`
    // test: `wss` now parses, the handshake verifies the server's certificate
    // through the default store (`SSL_CERT_FILE`, which is what the harness
    // points at the server's own PEM), and the same scripted exchange the plain
    // `ws` gate runs happens over the TLS stream.
    let mut server = WsTlsServer::start();
    let bin = out_path("ws_wss");
    let build = kelpc()
        .arg("build")
        .arg(fixture("tests/fixtures/ws_wss.kp"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected the ws_wss fixture to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let ssl = server.cert.cert_path.to_str().expect("the cert path is utf-8").to_string();
    let run = Command::new(&bin)
        .arg(server.port.to_string())
        .env_remove("SSL_CERT_FILE")
        .env_remove("SSL_CERT_DIR")
        .env("SSL_CERT_FILE", ssl)
        .output()
        .expect("failed to run the client");
    server.finish();
    assert!(
        run.status.success(),
        "expected the wss client to exit successfully, got:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let expected = format!(
        "url 127.0.0.1 {} wss\nconnected\ntext hello\nfrag fragment\n\
         ping ping?\nclose 1000 done\nclosed\n",
        server.port
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        expected,
        "unexpected stdout for 'ws_wss'"
    );
    assert_eq!(server.accepted(), 1, "the client opened more than one connection");
}

#[test]
fn a_ws_try_connect_tells_a_refused_dial_from_a_non_101_answer() {
    // Two ports, two failures. The refused half dials a port this test
    // released (bind 0, learn the port, drop the listener), so nothing is
    // listening on it and the dial is refused without a server; the answered
    // half is the canned server, which answers `403`. The program branches on
    // the variant and exits 0 either way, which is the whole point: a
    // reconnect loop can tell "nothing is there" from "that is not a
    // WebSocket server". The `403` reason is the text `ws.connect` panics
    // with, byte for byte, because the panicking form is one `panic` over the
    // reason a `try_connect` produced.
    let mut server = CannedServer::start(one_connection(&[STATUS_403]));
    let dead = TcpListener::bind("127.0.0.1:0").expect("failed to bind loopback");
    let dead_port = dead.local_addr().expect("local address").port();
    drop(dead);

    let bin = out_path("ws_try_connect");
    let build = kelpc()
        .arg("build")
        .arg(fixture("tests/fixtures/ws_try_connect.kp"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected the ws_try_connect fixture to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&bin)
        .arg(dead_port.to_string())
        .arg(server.port.to_string())
        .output()
        .expect("failed to run the client");
    server.finish();
    assert!(
        run.status.success(),
        "expected the client to exit successfully, got:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let expected = "dial refused net.connect: connection failed\n  errno 111: Connection refused\n\
         handshake 403 ws.connect: the server answered '403' instead of '101 Switching Protocols'\n\
         survived\n";
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        expected,
        "unexpected stdout for 'ws_try_connect'"
    );
    assert_eq!(server.accepted(), 1, "the client opened more than one connection");
}

#[test]
fn a_wss_url_parses_with_its_scheme_and_default_port() {
    assert_fixture_runs_with_stdin(
        "ws_wss_parse",
        "",
        "gateway.discord.gg 443 /?v=10&encoding=json true\n8443 true\n80 false\n",
    );
}

#[test]
fn a_ws_host_outside_the_grant_is_denied_at_the_connect_call() {
    // The grant names `127.0.0.1` and every port on it; the program asks for
    // `example.com:80`. The check matches the host as written, before
    // `getaddrinfo` runs, and it runs inside `std/ws` at the handshake's own
    // `net.connect`, so no socket is opened and this needs no server.
    assert_project_denied_at_runtime(
        "ws_net_denied",
        &project("ws_net_denied").join("main.kp"),
        "'example.com:80' is outside the net capability granted to module 'std/ws'",
    );
}

#[test]
fn a_ws_host_outside_the_grant_is_denied_at_the_fallible_connect_too() {
    // The same denial the panicking form gets, at the same dial inside
    // `std/ws`: whichever of the two a caller reaches for, the grant holds.
    assert_project_denied_at_runtime(
        "ws_try_net_denied",
        &project("ws_try_net_denied").join("main.kp"),
        "'example.com:80' is outside the net capability granted to module 'std/ws'",
    );
}

#[test]
fn a_ws_program_that_does_not_grant_rand_does_not_build() {
    // `std/ws` draws the handshake key and every masking key from `std/rand`,
    // and a module holds exactly what its importer granted it, so `net` alone
    // is not a working grant. The refusal names what was not granted, at the
    // call inside the module.
    assert_project_rejected(
        "ws_rand_denied",
        &project("ws_rand_denied").join("main.kp"),
        "needs the 'rand' capability, which module 'std/ws' was not granted",
    );
}

// ---------------------------------------------------------------------------
// The two compiler fixes S4 forced. Both were found by the first real HTTP
// request: a request whose line ending is not CRLF is not a request.
// ---------------------------------------------------------------------------

#[test]
fn carriage_return_is_an_escape_like_any_other() {
    // `"\r"` used to decode to the letter `r`, because the lexer passed an
    // unknown escape through as the character itself. Fixing that exposed the
    // second half: `c_string_literal` did not escape CR either, so the
    // *generated C* had a raw carriage return inside a string literal, which
    // is not a string literal at all. Each escape is now pinned as a byte
    // value, so a change of meaning fails here rather than in a malformed
    // protocol on the wire.
    assert_compiles_and_runs("tests/fixtures/escapes.kp", "13\n2\n10\n9\n92\n34\n");
}

#[test]
fn a_block_comment_spans_and_nests_and_an_escape_writes_several_lines() {
    // `/* ... */` is new syntax (`plan/syntax.md` §2.1): it may span lines, it
    // nests, and it may sit inline before a statement. The fixture also pins
    // one `print` writing two lines through `\n`, and `print!` with no
    // terminating newline.
    assert_compiles_and_runs(
        "tests/fixtures/block_comments.kp",
        "first\nsecond\nno newline here\ndone\n",
    );
}

#[test]
fn a_documentation_comment_is_a_comment_to_the_compiler() {
    // `//!` starts a documentation block (`plan/syntax.md` §2.1). The compiler
    // reads it as a comment, so the fixture has to build and run like any
    // other program; what is refused is the trailing spelling, below.
    assert_compiles_and_runs("tests/fixtures/documentation_comment.kp", "7\n");
}

#[test]
fn source_location_is_the_kelp_call_site() {
    assert_compiles_and_runs("tests/fixtures/source_location.kp", "5\ntrue\n");
}

#[test]
fn trace_records_events_and_spans_on_stderr() {
    let bin = out_path("trace");
    let build = kelpc()
        .arg("build")
        .arg(fixture("tests/fixtures/trace.kp"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected trace fixture to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&bin)
        .output()
        .expect("failed to run trace fixture");
    assert!(
        run.status.success(),
        "trace fixture failed:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout), "done\n");
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.contains("trace\tevent\t"),
        "missing event record:\n{stderr}"
    );
    assert!(
        stderr.contains("trace\tbegin\t"),
        "missing begin record:\n{stderr}"
    );
    assert!(
        stderr.contains("trace\tend\t"),
        "missing end record:\n{stderr}"
    );
    assert!(
        stderr.contains("ready\\tset"),
        "text field was not escaped:\n{stderr}"
    );
}

#[test]
fn a_documentation_comment_after_code_is_rejected() {
    assert_rejected_with(
        "tests/fixtures/doc_comment_trailing.kp",
        "must start a line",
    );
}

#[test]
fn a_backslash_dollar_escape_writes_a_literal_dollar_sign() {
    assert_compiles_and_runs(
        "tests/fixtures/escape_dollar.kp",
        "36\n21\n$KELPC is unset, ${x}\n",
    );
}

#[test]
fn sys_reports_the_build_target() {
    assert_compiles_and_runs("tests/fixtures/sys_target.kp", "linux\ntrue\n");
}

#[test]
fn an_unknown_string_escape_is_rejected_by_name() {
    // The old fallback made `"\q"` mean `"q"`: a typo was silently a different
    // string. It is now a diagnostic that names the supported set.
    let bin = out_path("unknown_escape");
    let build = kelpc()
        .arg("build")
        .arg(fixture("tests/fixtures/unknown_escape.kp"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        !build.status.success(),
        "expected an unknown escape to be rejected, but it built successfully"
    );
    let stderr = String::from_utf8_lossy(&build.stderr);
    assert!(
        stderr.contains("unknown escape '\\q'"),
        "expected the diagnostic to name the escape, got:\n{stderr}"
    );
}

#[test]
fn the_download_example_writes_exactly_what_it_received() {
    // `examples/download.kp` is a program, not a snippet: it is compiled and run
    // against a canned server, and the file it writes is compared byte for
    // byte. The body deliberately holds a NUL and invalid utf-8, which is the
    // case a `String` could not carry — `fs.write` takes `Bytes` for exactly
    // this, and a download that went through text would corrupt it.
    let body: Vec<u8> = b"line\r\nnul:\0 hi:\xff\xfe".to_vec();
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(&body);

    let mut server = CannedServer::start(vec![vec![response]]);
    let bin = out_path("download_example");
    let build = kelpc()
        .arg("build")
        .arg(fixture("../examples/download.kp"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected examples/download.kp to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let out = out_path("download_example_out");
    let part = PathBuf::from(format!("{}.part", out.display()));
    let _ = std::fs::remove_file(&out);
    let _ = std::fs::remove_file(&part);
    let url = format!("http://127.0.0.1:{}/payload.bin", server.port);
    let run = Command::new(&bin)
        .arg(&url)
        .arg(&out)
        .output()
        .expect("failed to run the compiled binary");
    server.finish();

    assert!(
        run.status.success(),
        "expected the download to succeed, got:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        std::fs::read(&out).expect("the output file exists"),
        body,
        "the file on disk is not what the server sent"
    );
    assert!(
        !part.exists(),
        "the `.part` file should have been renamed into place"
    );
    let stdout = String::from_utf8_lossy(&run.stdout);
    let expected_size = format!("{}", body.len());
    assert!(
        stdout.starts_with(&format!("Content-Length: {expected_size}\n"))
            && stdout.ends_with(&format!(" bytes to {}\n", out.display())),
        "unexpected stdout:\n{stdout}"
    );
    let _ = std::fs::remove_file(&out);
}

/// MK6-S2-M1: `examples/download_pool.kp` fetches N paths from one host with
/// four worker threads, each holding its own keep-alive `Pool`. The canned
/// server accepts four connections and answers two requests on each, which is
/// the shape a worker's own pool produces (one handshake, then reuse): a
/// worker whose second request opened a *fifth* connection would mean its
/// pool was not being reused, and a run that hung would mean fewer than four
/// workers ever started.
#[test]
fn the_download_pool_example_uses_one_kept_alive_connection_per_worker() {
    let body = b"data".to_vec();
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    let mut per_connection = response.clone();
    per_connection.extend_from_slice(&body);
    let per_connection = per_connection;
    let connections: Vec<Vec<Vec<u8>>> = (0..4)
        .map(|_| vec![per_connection.clone(), per_connection.clone()])
        .collect();
    let mut server = CannedServer::start(connections);

    let bin = out_path("download_pool_example");
    let build = kelpc()
        .arg("build")
        .arg(fixture("../examples/download_pool.kp"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected examples/download_pool.kp to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let run = Command::new(&bin)
        .arg("127.0.0.1")
        .arg(server.port.to_string())
        .args(["/a", "/b", "/c", "/d", "/e", "/f", "/g", "/h"])
        .output()
        .expect("failed to run the compiled binary");
    server.finish();

    assert!(
        run.status.success(),
        "expected the pooled download to succeed, got:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(server.accepted(), 4, "expected exactly one connection per worker");
    let stdout = String::from_utf8_lossy(&run.stdout);
    for i in 0..4 {
        assert!(
            stdout.contains(&format!("worker {i}: 2 ok, 0 failed, 8 bytes\n")),
            "missing worker {i}'s report:\n{stdout}"
        );
    }
    assert!(
        stdout.ends_with("total: 8 ok, 0 failed, 32 bytes\n"),
        "unexpected stdout:\n{stdout}"
    );
}

// ---------------------------------------------------------------------------
// A-1: `enum` — tagged unions, variant construction, and pattern `when`.
// ---------------------------------------------------------------------------

#[test]
fn an_enum_matches_on_its_live_variant() {
    assert_compiles_and_runs(
        "tests/fixtures/enum_basic.kp",
        "12.000000\n12.000000\n0.000000\n1\n2\n2\n",
    );
}

#[test]
fn an_owned_payload_binds_as_a_borrow() {
    assert_compiles_and_runs(
        "tests/fixtures/enum_owned.kp",
        "5\n0\n0\n5\nfirst\n4\n0\n0\n",
    );
}

#[test]
fn a_recursive_enum_nests_through_a_box() {
    assert_compiles_and_runs("tests/fixtures/enum_recursive.kp", "0\n7\n6\n");
}

#[test]
fn an_enum_payload_is_laid_out_the_way_the_c_typedef_is() {
    // The three SysV classes a tagged union lands in, each with a payload that
    // only reads back correctly at the offset clang gives it: `Narrow`'s union
    // is at offset 4 because every payload is a `u8` (so the whole enum is 8
    // bytes, `AggClass::One`), `Eight`'s is at 8 (16 bytes, a register pair),
    // and `Wide`'s is at 8 with a 24-byte union (32 bytes, `MEMORY`). A wrong
    // payload offset and a wrong union size are both silent, which is what
    // makes this fixture worth its length.
    assert_compiles_and_runs(
        "tests/fixtures/enum_layout.kp",
        "5\n17\n-1\n41\n-2\n14\n15\n-3\n17\n1\n-1\n20\n-2\n404\n0\n202\n0\n116\n2\n5\n",
    );
}

#[test]
fn an_enum_is_a_list_element_a_map_value_and_a_box_element() {
    assert_compiles_and_runs(
        "tests/fixtures/enum_in_containers.kp",
        "2\n5\n0\n6\n0\n1\n2\n3\n5\nfalse\n5\nfalse\n1\n5\n0\n0\n6\n0\n0\n4\n0\n5\n9\n-1\n0\n",
    );
}

#[test]
fn the_enum_node_kinds_keep_two_live_locals_in_different_registers() {
    // The register allocator's liveness pass has twice handed two live locals
    // the same register because a node kind it never walked made one of them
    // look dead on declaration. This fixture keeps two scalars alive across
    // each of this slice's four node kinds — and, added with the slice's own
    // defect fix, across `if let` and across a call with an enum-typed local
    // live on the far side, the two shapes the slice's report claimed were
    // probed when the fixture held neither. A repeat of the mistake is a wrong
    // sum rather than a silent one.
    assert_compiles_and_runs(
        "tests/fixtures/enum_liveness.kp",
        "21\n143\n-13\n141\n42\n-1\n185\n86\n297\n-3\n226\n136\n",
    );
}

#[test]
fn nested_aggregate_literals_are_staged_in_slots_of_their_own() {
    // A pre-existing bug, not one this slice introduces: every staged aggregate
    // value used one shared frame slot, so a field initializer that staged
    // another one overwrote what the outer value had already written. Three of
    // these five prints were wrong before the fix (`7` for `6`, `10` for `9`,
    // `20` for `19`).
    assert_compiles_and_runs(
        "tests/fixtures/nested_literal_staging.kp",
        "6\n10\n9\n19\n13\n",
    );
}

#[test]
fn an_enum_is_a_container_element_and_a_container_drops_it() {
    assert_compiles_and_runs(
        "tests/fixtures/enum_container.kp",
        "12.000000\n0.000000\n27.000000\n0.000000\n4\n12\n0\n4\n",
    );
}

#[test]
fn when_arms_do_not_need_a_newline_inside_parentheses() {
    // A `when` is an expression, and inside `(` `)` the lexer emits no
    // newline tokens at all — so an arm separator cannot be required there.
    assert_compiles_and_runs("tests/fixtures/when_in_args.kp", "1\n2\n2\n");
}

#[test]
fn when_a_temporary_scrutinee_is_staged_into_a_local() {
    // A call's result is not a place, so it is bound to a local of its own and
    // the arms match that: a payload binds as a borrow, and the borrow needs
    // the temporary to outlive the arm.
    assert_compiles_and_runs("tests/fixtures/enum_temp_scrutinee.kp", "0.000000\n");
}

#[test]
fn when_a_field_of_a_temporary_is_staged_at_its_root() {
    // `mk().t` stages `mk()` and matches `#when0.t` rather than staging the
    // field itself, so the whole `Holder` is freed once, `extra` included.
    // The same fixture covers the `if let` desugar and its `elif let` chain.
    assert_compiles_and_runs(
        "tests/fixtures/scrutinee_field_of_temporary.kp",
        "4\n4\n4\n4\n8\n",
    );
}

#[test]
fn an_enum_method_is_called_on_a_binding() {
    // A method declared on an `enum` is called on a value of the enum type: a
    // binding reaches the method tables and resolves `fun Shape.label()`.
    assert_compiles_and_runs("tests/fixtures/enum_method_binding.kp", "empty\ncircle\n");
}

#[test]
fn an_enum_method_is_refused_on_a_variant_path() {
    // The receiver builds the variant, so it names a type rather than a value;
    // the method exists and the fix is still to bind the variant first, which
    // is what the message has to say.
    assert_rejected_with(
        "tests/fixtures/enum_method.kp",
        "an 'enum' method is called on a value, not on a variant path",
    );
}

#[test]
fn enum_misuse_is_rejected() {
    for name in [
        // Exhaustiveness, and arm/pattern shape.
        "enum_non_exhaustive",
        "enum_unknown_variant",
        "enum_duplicate_arm",
        "enum_bind_count",
        "enum_datac_scrutinee",
        // Construction and access.
        "enum_ctor_name",
        "enum_bare_variant",
        "enum_field_access",
        "enum_unknown_method",
        // Declaration rules, shared with `datac`.
        "enum_datac_clash",
        "enum_self_by_value",
        // A payload reached through a borrow binds as a shared borrow.
        "enum_payload_move",
    ] {
        assert_rejected(&format!("tests/fixtures/{name}.kp"));
    }
}

// ---------------------------------------------------------------------------
// MK5-S6-M1: `if let` — a pattern condition, desugared to a single-arm `when`.
// ---------------------------------------------------------------------------

#[test]
fn if_let_binds_one_variant_and_else_covers_the_rest() {
    assert_compiles_and_runs(
        "tests/fixtures/if_let_basic.kp",
        "2.000000\n0.000000\n0.000000\n4\n0\n3.000000\n-1.000000\n6.000000\n5.000000\n7.000000\n",
    );
}

#[test]
fn if_let_else_chain_reads_its_scrutinee_more_than_once() {
    assert_compiles_and_runs(
        "tests/fixtures/if_let_chain_same_scrutinee.kp",
        "12.566360\n2\n2.000000\n2\n",
    );
}

#[test]
fn if_let_misuse_is_rejected() {
    for name in ["if_let_unknown_variant", "if_let_bind_count"] {
        assert_rejected(&format!("tests/fixtures/{name}.kp"));
    }
}

#[test]
fn an_else_if_let_behind_a_plain_if_compiles() {
    // BUG-6: the checker's else-if arm assumed `Expr::If`, which only a plain
    // `else if` is (an `else if let` desugars to a block), so this shape was an
    // internal error rather than a program. The chain starts with a plain
    // `if`, a payload-free variant and a link whose pattern misses are all
    // here.
    assert_compiles_and_runs(
        "tests/fixtures/else_if_let.kp",
        "3\n0\n5\n0\n1\n3\n1\n7\n0\n",
    );
}

// ---------------------------------------------------------------------------
// MK5-S6-M1: `?` — propagate the failure of an `Ok`/`Err` or `Some`/`None` enum.
// ---------------------------------------------------------------------------

#[test]
fn try_propagates_a_failure_out_of_the_enclosing_function() {
    assert_compiles_and_runs(
        "tests/fixtures/try_operator.kp",
        "2\n1\n-3\n4\n-1\n3\n-3\n",
    );
}

#[test]
fn option_and_result_ship_as_stdlib_modules() {
    assert_compiles_and_runs(
        "tests/fixtures/stdlib_option_result.kp",
        "true\nfalse\n2\nfalse\n-1\ntrue\n1\n1\ntrue\n-1\n0\n2\nodd\ntrue\ntrue\n",
    );
}

#[test]
fn a_panicking_tail_satisfies_any_return_type() {
    // The body diverges, so no `return` is needed after the `panic` — and the
    // panic still fires when that path is taken.
    assert_fixture_panics("panic_tail", "always_fails: non-positive");
}

#[test]
fn try_frees_the_frame_it_returns_from() {
    // `?`'s early return is a `return`: a frame that owns something and
    // propagates a failure has to free what it owns first. The output pins the
    // success paths (freed exactly once) and the failure paths (freed at the
    // `?`); the AddressSanitizer gate below is what proves the second half,
    // because a leak prints nothing.
    assert_compiles_and_runs(
        "tests/fixtures/try_drops_frame.kp",
        "9\n-2\n6\n-2\n14\n-2\n4\n-1\n-2\n7\n9\n-2\n",
    );
}

#[test]
fn a_propagated_failure_frees_the_frame_it_leaves() {
    // `?`'s early return is a `return`, so the frame's owned locals die on it
    // too. `try_drops_frame.kp` pins the same shape, but its last function
    // wraps it in a `fun` literal, which this backend refuses by name — so this
    // fixture is the same mechanism without the closure. The printed values pin
    // the success arm; the valgrind sweep is what proves the failure arm frees,
    // because a leak prints nothing.
    assert_compiles_and_runs(
        "tests/fixtures/try_frame_drops.kp",
        "9\n-1\n6\n-1\n14\n-1\n-1\n7\n6\n-1\n",
    );
}

#[test]
fn a_one_eightbyte_try_payload_builds_without_a_destination() {
    // A `?` evaluated in a position that can hold one eightbyte (`expr`, which
    // is what a `let` of a one-eightbyte `datac` uses) refused *every*
    // aggregate payload for want of a destination, and said "over eight bytes"
    // about a three-byte `datac`. `Result<D3, u8>` is eight bytes in total, so
    // its success payload fits `rax` and the program must build — while a
    // genuinely wider payload keeps the refusal.
    assert_compiles_and_runs("tests/fixtures/try_narrow_payload.kp", "6\n9\n");
}

#[test]
fn try_misuse_is_rejected() {
    for name in [
        // `main` returns `Unit`, so there is nowhere to propagate to.
        "try_in_main",
        // The operand has to be an enum, of the right shape, held by value.
        "try_non_enum",
        "try_wrong_shape",
        "try_borrowed",
        // The failure payload types have to agree, and so do the shapes.
        "try_error_type",
        "try_shape_mismatch",
    ] {
        assert_rejected(&format!("tests/fixtures/{name}.kp"));
    }
}

// ---------------------------------------------------------------------------
// A-2: user-declarable generic types — `datac`/`enum` templates, monomorphized
// at each use site, with the type arguments inferred from the construction.
// ---------------------------------------------------------------------------

#[test]
fn a_generic_datac_and_enum_are_monomorphized_per_use_site() {
    assert_compiles_and_runs(
        "tests/fixtures/generic_type_basic.kp",
        "3\n3\nthree\n5\n9\n6\n7\n0\n19\n",
    );
}

#[test]
fn a_generic_instantiation_owns_and_drops_like_any_other_type() {
    assert_compiles_and_runs("tests/fixtures/generic_owned.kp", "5\n0\n4\n3\n2\n1\n");
}

#[test]
fn a_generic_type_can_be_recursive_through_a_box() {
    assert_compiles_and_runs("tests/fixtures/generic_recursive.kp", "6\none\ntwo\n0\n9\n");
}

#[test]
fn a_generic_function_is_instantiated_per_call_site() {
    assert_compiles_and_runs(
        "tests/fixtures/generic_function_basic.kp",
        "5\ntrue\n2.500000\ntext\n3\nonce\n1\n9\n",
    );
}

#[test]
fn a_generic_function_works_over_an_owned_type() {
    assert_compiles_and_runs(
        "tests/fixtures/generic_function_owned.kp",
        "true\nfalse\ntrue\ntrue\n5\n",
    );
}

#[test]
fn a_method_on_a_generic_type_is_instantiated_per_receiver() {
    assert_compiles_and_runs(
        "tests/fixtures/generic_method_basic.kp",
        "1\n3\n2\n3\n9\n7\n4\n",
    );
}

#[test]
fn explicit_type_arguments_are_written_at_the_call() {
    assert_compiles_and_runs(
        "tests/fixtures/generic_explicit_args.kp",
        "1\none\n7\n0\n5\ntext\ntrue\n",
    );
}

#[test]
fn nested_generic_constructions_infer_from_their_arguments() {
    assert_compiles_and_runs("tests/fixtures/generic_nested_inference.kp", "1\n2\n3\n");
}

#[test]
fn explicit_type_argument_misuse_is_rejected() {
    for name in [
        // The wrong number of arguments.
        "generic_explicit_arity",
        // A type argument list on something that is not generic.
        "generic_explicit_plain",
        // A method's parameters come from its receiver, so it takes none.
        "generic_explicit_method",
    ] {
        assert_rejected(&format!("tests/fixtures/{name}.kp"));
    }
}

#[test]
fn generic_method_misuse_is_rejected() {
    for name in [
        // The receiver omits its type arguments entirely.
        "generic_method",
        // ... or writes parameters the type does not have, in the wrong order.
        "generic_method_wrong_params",
        // A non-generic type has nothing to parameterize.
        "generic_method_on_plain",
    ] {
        assert_rejected(&format!("tests/fixtures/{name}.kp"));
    }
}

#[test]
fn generic_function_misuse_is_rejected() {
    for name in [
        "generic_function_cannot_infer",
        "generic_function_arity",
        "generic_main",
        "generic_function_clash",
        "generic_function_shadow",
    ] {
        assert_rejected(&format!("tests/fixtures/{name}.kp"));
    }
}

#[test]
fn generic_type_misuse_is_rejected() {
    for name in [
        "generic_needs_args",
        "generic_arity",
        "generic_cannot_infer",
        "generic_param_shadow",
        "generic_param_duplicate",
        "generic_clash",
        "generic_borrowed_field",
        "generic_self_value",
    ] {
        assert_rejected(&format!("tests/fixtures/{name}.kp"));
    }
}

// ---------------------------------------------------------------------------
// A-4 phase A: `move T` — a parameter that takes ownership of its argument.
// ---------------------------------------------------------------------------

#[test]
fn a_move_parameter_takes_ownership_of_its_argument() {
    // Also in the AddressSanitizer list: every buffer here is freed exactly
    // once, by whichever function owns it when the scope ends.
    assert_compiles_and_runs("tests/fixtures/move_param.kp", "5\n4\n7\n4\n4\n2\n");
}

#[test]
fn a_moved_parameter_is_dropped_on_every_path_out() {
    // The tail, an early `return`, and a `return` inside a loop. Also in the
    // AddressSanitizer list, so a missed drop is a leak.
    assert_compiles_and_runs(
        "tests/fixtures/move_param_return.kp",
        "4\ntrue\n3\nlong\n5\n",
    );
}

#[test]
fn a_move_parameter_and_the_body_local_beside_it_are_both_freed() {
    // BUG-tV9qH: the frame's owned parameter and the owned local the body
    // declared share one block end in the drop plan, and the parameter's list
    // was written over the block's own, so the body local stayed in no drop
    // set at all. valgrind reported its buffer as definitely lost, on both
    // backends; both buffers are freed now.
    assert_compiles_and_runs(
        "tests/fixtures/move_param_and_body_local.kp",
        "param\nbody\n",
    );
}

#[test]
fn a_body_local_shadowing_a_move_parameter_leaves_no_buffer_behind() {
    // The same one block end, with both bindings of one spelling: the
    // parameter's drop resolved by name to the newest binding and freed the
    // local's buffer while the local's own drop had been overwritten, so
    // exactly one of the two leaked whichever way it went. The parameter's drop
    // names the parameter and the local's names the local, so both are freed.
    assert_compiles_and_runs(
        "tests/fixtures/move_param_shadowed_by_a_local.kp",
        "param\nbody\n",
    );
}

#[test]
fn using_a_moved_argument_again_is_rejected() {
    assert_rejected_with(
        "tests/fixtures/move_param_use_after.kp",
        "use of moved value 'a'",
    );
    // The same rule one iteration later: the second turn of the loop has
    // nothing left to move.
    assert_rejected_with(
        "tests/fixtures/move_param_loop.kp",
        "use of moved value 'a'",
    );
}

#[test]
fn moving_out_of_a_borrow_is_rejected() {
    // The callee's own `&String` parameter cannot be handed to a `move
    // String` parameter: the borrow is not an owner.
    assert_rejected_with(
        "tests/fixtures/move_param_borrow_arg.kp",
        "this call moves its argument in, and a borrow cannot be moved out of",
    );
}

#[test]
fn move_parameter_misuse_is_rejected() {
    // `str` and `&T` are already borrowed, so there is nothing to own.
    assert_rejected_with(
        "tests/fixtures/move_param_str.kp",
        "'move' cannot be written on 'str'",
    );
    assert_rejected_with(
        "tests/fixtures/move_param_ref.kp",
        "'move' cannot be written on '&String'",
    );
    // One parameter cannot both borrow exclusively and take ownership.
    assert_rejected_with(
        "tests/fixtures/move_param_mut_conflict.kp",
        "'mut' and 'move' cannot be combined on one parameter",
    );
    // A receiver is always borrowed.
    assert_rejected_with(
        "tests/fixtures/move_param_receiver.kp",
        "'move' cannot be written on a receiver",
    );
}

// ---------------------------------------------------------------------------
// A-4 phase B: function values — a declared `fun` as a value of type
// `fun(T).return: U`, called through a local, a field, a passdown or a parameter.
// ---------------------------------------------------------------------------

#[test]
fn a_function_name_is_a_value_and_a_value_is_callable() {
    // Also in the AddressSanitizer list (the `move` callback frees a buffer).
    assert_compiles_and_runs(
        "tests/fixtures/fn_value.kp",
        "42\n10\n12\n12\n4\n2\n12\n18\n27\n22\n0\n2\n4\n30\n4.000000\n",
    );
}

#[test]
fn a_function_value_with_the_wrong_shape_is_rejected() {
    // The parameter types are part of the function type, so a callback of the
    // wrong shape is an ordinary argument-type mismatch — and a `fun(move
    // String)` callback cannot be handed a borrow.
    assert_rejected_with(
        "tests/fixtures/fn_value_mismatch.kp",
        "expected 'fun(f64).return: f64'",
    );
    assert_rejected_with(
        "tests/fixtures/fn_value_move_arg.kp",
        "this call moves its argument in, and a borrow cannot be moved out of",
    );
}

#[test]
fn a_function_value_that_needs_something_extra_is_rejected() {
    // Each of these names a *function* the compiler cannot turn into a plain
    // C function pointer, so each says which and why rather than "undefined
    // variable".
    assert_rejected_with(
        "tests/fixtures/fn_value_generic.kp",
        "'first' is a generic function",
    );
    assert_rejected_with(
        "tests/fixtures/fn_value_passdown.kp",
        "'uses_greeting' reads a passdown, so it cannot be a function value",
    );
    assert_rejected_with(
        "tests/fixtures/fn_value_main.kp",
        "cannot reference 'main': it is the program entry point",
    );
}

#[test]
fn function_type_misuse_is_rejected() {
    // A borrow of a function value (`mut` and `&` spellings), a container
    // element, a function type returning a function type, and `print`.
    assert_rejected_with(
        "tests/fixtures/fn_value_borrow.kp",
        "'mut' cannot be written on the function type",
    );
    assert_rejected_with(
        "tests/fixtures/fn_value_amp.kp",
        "'&' cannot be written on the function type",
    );
    assert_rejected_with(
        "tests/fixtures/fn_value_element.kp",
        "cannot be a 'List' element yet",
    );
    assert_rejected_with(
        "tests/fixtures/fn_value_nested_return.kp",
        "a function type cannot return another function type",
    );
    assert_rejected_with(
        "tests/fixtures/fn_value_print.kp",
        "cannot pass a value of type 'fun(i32).return: i32' to 'print'",
    );
}

#[test]
fn indirect_call_misuse_is_rejected() {
    assert_rejected_with(
        "tests/fixtures/fn_value_named_args.kp",
        "cannot use named arguments",
    );
    assert_rejected_with(
        "tests/fixtures/fn_value_arity.kp",
        "a call through 'f' takes 1 argument(s), found 2",
    );
    // A call's *result* is not itself callable: `pick()(4)` needs a binding.
    assert_rejected("tests/fixtures/fn_value_call_result.kp");
}

// ---------------------------------------------------------------------------
// A-4 phase C: anonymous `fun` literals — one synthesized function each,
// checked in a scope of their own.
// ---------------------------------------------------------------------------

#[test]
fn a_fun_literal_is_a_function_value() {
    // Also in the AddressSanitizer list: the `move` literal frees what it is
    // handed. Covers a literal passed directly, a block body, a nested
    // literal, a literal in a `datac` field, a literal returned from a
    // function, and `?` propagating against the literal's own `.return`.
    assert_compiles_and_runs(
        "tests/fixtures/fn_literal.kp",
        "42\n6\n7\n4\n31\n9\n12\ntrue\n4\n",
    );
}

#[test]
fn a_fun_literal_captures_rather_than_failing() {
    // Phase E replaced the "cannot capture yet" refusal: a name from the
    // enclosing scope is now the thing that makes the literal a closure.
    assert_compiles_and_runs("tests/fixtures/fn_literal_capture.kp", "10\n10\n");
}

#[test]
fn function_values_and_closures_need_no_stdlib() {
    // The closure slice's own fixture (MK7 S2E): the shapes of `fn_value` and
    // `closure_basic` with no `impl` that drags in a std module, because an
    // imported module's bodies are lowered too and `std/str` reaches
    // `std.mem.alloc_raw`, which is the runtime slice. A function value as a
    // local, a parameter, a return value and a `datac` field; a capturing
    // closure as a `Box` element, a `move` parameter and a field of its own.
    assert_compiles_and_runs(
        "tests/fixtures/fn_value_plain.kp",
        "42\n12\n18\n21\n20\n6\n7\n",
    );
}

#[test]
fn a_plain_function_value_is_widened_into_a_closure() {
    // Which of the two widening shapes is used depends on whether the compiler
    // knows which function the value is: a named one gets a shim and allocates
    // nothing, one chosen at the call gets a heap environment and a thunk.
    assert_compiles_and_runs(
        "tests/fixtures/closure_value_widening.kp",
        "6\n11\n9\n101\n",
    );
}

#[test]
fn a_two_field_environment_reads_and_drops_each_capture_at_its_own_offset() {
    // MK7 §5 item 6 trap 1: the only environment with more than one field in
    // the corpus. `a` is an `i32` at offset 0, `b` an `i64` C pads to 8 and `s`
    // an owned `String` at 16, so a packed field walk reads the wrong bytes,
    // and the printed number names all three fields.
    assert_compiles_and_runs("tests/fixtures/closure_two_captures.kp", "3711\n7\n11\n");
}

#[test]
fn an_indirect_call_numbers_the_environment_after_a_mem_class_return_pointer() {
    // MK7 §5 item 6 trap 8: the callee returns a 24-byte `MEMORY`-class `datac`
    // *and* carries an environment, so both take a register in the same
    // integer file, the hidden return pointer first. Numbering the environment
    // `ARG64[0]` would hand it the return pointer instead.
    assert_compiles_and_runs("tests/fixtures/closure_env_wide_ret.kp", "4\n5\n6\n");
}

#[test]
fn fun_literal_misuse_is_rejected() {
    // A passdown is a hidden argument a function value cannot carry, a borrow
    // return has no elision root, the tail is checked against `.return` like any
    // function's, and a body is required.
    assert_rejected_with(
        "tests/fixtures/fn_literal_passdown.kp",
        "a 'fun' literal cannot read passdown 'base'",
    );
    assert_rejected_with(
        "tests/fixtures/fn_literal_borrow_return.kp",
        "a 'fun' literal cannot return a borrow ('&str')",
    );
    assert_rejected_with(
        "tests/fixtures/fn_type_borrow_return.kp",
        "a function type cannot return a borrow ('&Point')",
    );
    assert_rejected_with(
        "tests/fixtures/fn_literal_tail_type.kp",
        "this 'fun' literal is declared to return 'i32' but its body evaluates to 'unit'",
    );
    assert_rejected_with(
        "tests/fixtures/fn_literal_no_body.kp",
        "a 'fun' literal needs a body",
    );
}

// ---------------------------------------------------------------------------
// A-4 phase D: the stdlib callbacks — what function values and a move-taking
// parameter were for.
// ---------------------------------------------------------------------------

#[test]
fn option_and_result_combinators_take_callbacks() {
    // Also in the AddressSanitizer list: `map` builds owned `String`s.
    assert_compiles_and_runs(
        "tests/fixtures/stdlib_combinators.kp",
        "5\nHELLO\n2\ntrue\n7\n7\n42\n105\n4\n11\n4\n",
    );
}

#[test]
fn list_callbacks_work_over_copy_and_owned_elements() {
    // Also in the AddressSanitizer list.
    assert_compiles_and_runs(
        "tests/fixtures/stdlib_list_callbacks.kp",
        "6\n2\n123\ntrue\ntrue\n2\n2\n4\n1\ntrue\n2\n",
    );
}

#[test]
fn a_combinator_needing_a_copy_payload_says_so() {
    // `list.filter` reads an accepted element out of a *borrowed* list, so its
    // element type must be `Copy`; the failure is inside the module, so the
    // message names the module and says why. The two helpers that only needed a
    // partial move now take their receiver with `move` instead (A-11): those
    // rejections moved to `option_unwrap_owned.kp` and `result_map_owned.kp`.
    assert_rejected_with(
        "tests/fixtures/stdlib_list_filter_owned.kp",
        "this is inside the shipped standard library ('std/list')",
    );
}

// ---------------------------------------------------------------------------
// A-4 phase E: captures — a `fun` literal that names a binding from the
// enclosing scope becomes a closure (`closure(T).return: U`), which owns the heap
// environment its captures were moved into.
// ---------------------------------------------------------------------------

#[test]
fn a_closure_carries_its_environment() {
    // Also in the AddressSanitizer list: every owned capture is freed exactly
    // once. Covers a `Copy` capture, an owned capture read through a borrow, a
    // capturing literal passed to a closure parameter and to a `move` one, a
    // closure returned out of the frame that made it, one in a `datac` field,
    // one in a `Box`, and an inner literal calling a closure the frame holds.
    assert_compiles_and_runs(
        "tests/fixtures/closure_basic.kp",
        "11\n10\n3\n30\n-6\n14\n15\n42\n13\n5\n11\ntrue\n",
    );
}

#[test]
fn a_closure_parameter_may_borrow_a_module_declared_type() {
    // BUG-2: the closure typedef and its coercion environment are emitted
    // before the `datac`s, so a `&Item` parameter has to be spelled with the
    // forward-declared struct tag there rather than with `Item`'s typedef
    // name. Naming the typedef name made `cc` reject any callback API over a
    // module's own type.
    assert_compiles_and_runs(
        "tests/fixtures/closure_datac_borrow_param.kp",
        "1\n",
    );
}

#[test]
fn a_library_may_take_a_closure_over_its_own_type() {
    // The other half of BUG-2, and the shape that motivated it: inside a
    // non-entry module the parameter type is module-qualified (`&items.Item`),
    // so the generated closure type is named after it. That name used to come
    // out of the mangling as `refitems.Item` — a dot in a C identifier — and
    // every generated use of the closure type failed to compile. Needs a
    // project because the module prefix is what makes the name qualified.
    assert_project_runs(
        "closure_datac_param",
        &project("closure_datac_param"),
        "1\n4\ntrue\n",
    );
}

#[test]
fn a_closure_is_dropped_where_its_binding_dies() {
    // Also in the AddressSanitizer list: a missed drop is a leak and a second
    // one a double free. Covers the three drop points — the end of a block, an
    // early `return`, and once per loop iteration.
    assert_compiles_and_runs(
        "tests/fixtures/closure_move.kp",
        "5\n7\n0\n10\n11\n12\n",
    );
}

#[test]
fn the_stdlib_callbacks_take_closures() {
    // Also in the AddressSanitizer list: every capturing callback here owns an
    // environment, and one of them captured an owned `String`. Covers a
    // capturing callback through `map`/`any`/`each`, a declared function and a
    // non-capturing literal (widened through a generated shim, so no
    // allocation), a function value whose identity is known only at the call
    // (widened into a one-field environment), and the same widening through
    // `option` and `result`.
    assert_compiles_and_runs(
        "tests/fixtures/stdlib_closure_callbacks.kp",
        "3\n9\ntrue\nv=1v=2v=3\n2\ntrue\n4\n107\n5\n12\n",
    );
}

#[test]
fn a_closure_capture_is_checked() {
    // A borrow has no owner to move into the environment, a receiver's field
    // would have to be read out of a borrow, a nested literal cannot move an
    // owned value out of the enclosing environment, and an owned capture can
    // only be borrowed inside the body.
    assert_rejected_with(
        "tests/fixtures/closure_capture_borrow.kp",
        "a 'fun' literal cannot capture 's': it is a borrow ('str')",
    );
    assert_rejected_with(
        "tests/fixtures/closure_capture_receiver.kp",
        "it is a field of 'Counter'",
    );
    assert_rejected_with(
        "tests/fixtures/closure_nested_owned.kp",
        "it belongs to the enclosing closure's environment",
    );
    assert_rejected_with(
        "tests/fixtures/closure_capture_by_value.kp",
        "cannot read a captured value out of the closure's environment by value",
    );
}

#[test]
fn the_closure_type_and_its_limits_are_checked() {
    // `&closure(...)` is not a type, a closure is not a container element, the
    // widening needs the same signature, a closure cannot go where a plain
    // function value is expected, and the one widening that exists (a `fun`
    // value where a closure is expected) does not blur the ownership rules:
    // capturing an owned binding still moves it, and a closure handed to a
    // `move` parameter is still gone afterwards.
    assert_rejected_with(
        "tests/fixtures/closure_type_amp.kp",
        "'&' cannot be written on the closure type",
    );
    assert_rejected_with(
        "tests/fixtures/closure_element.kp",
        "cannot be a 'List' element yet",
    );
    assert_rejected_with(
        "tests/fixtures/closure_signature_mismatch.kp",
        "declared as 'closure(i32).return: i32'",
    );
    assert_rejected_with(
        "tests/fixtures/closure_where_fn_expected.kp",
        "this is a closure: its body captured a binding",
    );
    assert_rejected_with(
        "tests/fixtures/closure_use_after_capture.kp",
        "use of moved value 'text'",
    );
    assert_rejected_with(
        "tests/fixtures/closure_use_after_move.kp",
        "use of moved value 'g'",
    );
}

// ---------------------------------------------------------------------------
// The CLI itself (MK8-S1-M3's release added `kelpc version`).
// ---------------------------------------------------------------------------

#[test]
fn the_cli_reports_its_version() {
    // The shape is what the scheme pins: `YY.MM.N`, with the month padded,
    // which is what the tag and the release asset are written as. The number
    // itself changes on every release, so asserting the shape keeps this test
    // from going stale on the next bump.
    let out = kelpc()
        .arg("version")
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        out.status.success(),
        "'kelpc version' should succeed, got:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let printed = String::from_utf8_lossy(&out.stdout).to_string();
    let version = printed
        .trim_end()
        .strip_prefix("kelpc ")
        .unwrap_or_else(|| panic!("'kelpc version' printed '{printed}'"));
    let parts: Vec<&str> = version.split('.').collect();
    assert_eq!(parts.len(), 3, "'{version}' is not YY.MM.N");
    assert_eq!(parts[0].len(), 2, "'{version}' has a one-digit year");
    assert_eq!(parts[1].len(), 2, "'{version}' has a one-digit month");
    assert!(
        parts[2].chars().all(|c| c.is_ascii_digit()) && !parts[2].is_empty(),
        "'{version}' has no release number"
    );
    assert!(out.stderr.is_empty(), "'kelpc version' should not warn");
}

#[test]
fn an_unknown_command_prints_the_usage_and_fails() {
    // Including `kelpc` with no arguments at all, which is the case a new user
    // hits first — and the usage line has to name every command that exists.
    for args in [vec![], vec!["bogus"], vec!["build"]] {
        let out = kelpc()
            .args(&args)
            .output()
            .expect("failed to spawn kelpc");
        assert!(
            !out.status.success(),
            "'kelpc {}' should fail",
            args.join(" ")
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("usage: kelpc build <file.kp>") && stderr.contains("kelpc version"),
            "unexpected usage text for 'kelpc {}':\n{stderr}",
            args.join(" ")
        );
    }
}

// ---------------------------------------------------------------------------
// The backend seam. The user-facing selector is gone (the MK7 record in
// `plan/done.md`), so what is asserted end-to-end here is the shape that replaced it: every
// build gets `llvm`, the retired flag and the retired environment variable are
// refused by name, and the project's one remaining switch points at the C
// oracle. These are end-to-end rather than only `backend.rs` unit tests because
// what matters is that the refusal and the switch reach the compiler.
// ---------------------------------------------------------------------------

/// Which code generator a build used is visible in the intermediate it writes
/// beside the artifact: the C backend keeps `<out>.c`, the LLVM backend its
/// textual IR as `<out>.ll` (MK7 §4.4). Both backends build this fixture and
/// print the same bytes, so the intermediate is the only place the choice
/// shows, and it is what these tests read.
fn build_is_llvm(bin: &Path) -> bool {
    bin.with_extension("ll").is_file()
}

/// A build that names no backend must use the compiler's default, and since
/// MK7 S7 that default is `llvm`. The artifact is not enough to assert this:
/// both backends print `55`, so the test also pins the backend by the
/// intermediate only `llvm` writes. This is the compiler's side of the
/// agreement `backend()` above keeps on the harness's side.
#[test]
fn the_default_backend_is_llvm() {
    let bin = out_path("backend_default");
    let _ = std::fs::remove_file(bin.with_extension("ll"));
    let _ = std::fs::remove_file(bin.with_extension("c"));
    let out = Command::new(kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .env_remove("KELPC_ORACLE")
        .arg("build")
        .arg(fixture("tests/fixtures/while_accumulator.kp"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        out.status.success(),
        "a build with no backend setting should succeed, got:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        build_is_llvm(&bin),
        "a build with no backend setting should have used the llvm default, \
         which writes <out>.ll beside the artifact"
    );
    let run = Command::new(&bin).output().expect("failed to run the binary");
    assert_eq!(String::from_utf8_lossy(&run.stdout), "55\n");
}

/// The C backend is what every other test in this file is written against, so
/// it has to stay reachable after the user-facing choice is removed: the
/// project's oracle switch still selects it. The `.c` intermediate is asserted
/// so a fallback to the default cannot pass by accident.
#[test]
fn the_oracle_switch_selects_the_c_backend() {
    let bin = out_path("backend_oracle");
    let _ = std::fs::remove_file(bin.with_extension("c"));
    let _ = std::fs::remove_file(bin.with_extension("ll"));
    let out = kelpc()
        .env("KELPC_ORACLE", "c")
        .args(["build"])
        .arg(fixture("tests/fixtures/while_accumulator.kp"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        out.status.success(),
        "the oracle switch should select the C backend, got:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        bin.with_extension("c").is_file(),
        "KELPC_ORACLE=c should have used the C backend, which writes <out>.c \
         beside the artifact"
    );
    let run = Command::new(&bin).output().expect("failed to run the binary");
    assert_eq!(String::from_utf8_lossy(&run.stdout), "55\n");
}

/// A build with no `-o` writes into `build/<stem>`, and creates that directory
/// if it is missing.
///
/// This is a behaviour change with a history: the default used to be a bare
/// `<stem>` **relative to the working directory**, so a build run from a
/// checkout put the binary in the checkout's root. Ten of them were committed
/// across six commits before anyone noticed, and the generated C beside each
/// one was invisible because `*.c` is ignored — which is exactly the shape a
/// test has to pin, because the output is only wrong somewhere no one looks.
///
/// Both halves matter: the artifact has to be *in* `build/`, and the working
/// directory has to be left with no new file in it.
#[test]
fn a_build_with_no_output_path_lands_in_build() {
    let cwd = out_path("default_out_dir");
    let _ = std::fs::remove_dir_all(&cwd);
    std::fs::create_dir_all(&cwd).expect("could not create the test's working directory");

    let out = kelpc()
        .args(["build"])
        .arg(fixture("tests/fixtures/while_accumulator.kp"))
        .current_dir(&cwd)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        out.status.success(),
        "a build with no -o should succeed, got:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let built = cwd.join("build").join("while_accumulator");
    assert!(
        built.is_file(),
        "expected the build to land at '{}', and it did not exist",
        built.display()
    );
    let mode = std::fs::metadata(&built)
        .expect("the built artifact should be stat-able")
        .permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert!(
            mode.mode() & 0o111 != 0,
            "the artifact should be executable, mode was {:o}",
            mode.mode()
        );
    }
    // The backend's own intermediate follows the artifact, so it is inside
    // `build/` too rather than beside it in the working directory: the C backend
    // keeps its `.c`, and the LLVM backend its textual IR. Asserting `.c` alone
    // made this the one test `BACKENDS=llvm ./kelpc/tests/run-all-backends.sh`
    // could not pass, so the suite could not be pointed at both backends.
    let intermediate = if backend() == "llvm" { "ll" } else { "c" };
    assert!(
        built.with_extension(intermediate).is_file(),
        "expected the generated .{intermediate} beside the artifact"
    );
    assert!(
        !cwd.join("while_accumulator").exists(),
        "the build must not write the artifact into the working directory"
    );
}

/// The two refusals that replace the selector, each carrying the exact text the
/// retired spellings are reported with. An exact comparison, not a search: a
/// message that drifted into "unknown flag" while a test only looked for the
/// word "backend" would keep passing while telling the caller the wrong thing.
const FLAG_GONE: &str = "--backend is gone: kelpc builds every program with the llvm backend, and the C backend is kept as the project's own oracle rather than as a user choice";
const ENV_GONE: &str = "KELPC_BACKEND is gone: kelpc builds every program with the llvm backend, and the C backend is kept as the project's own oracle rather than as a user choice";

/// A caller who still passes `--backend=c` is asking for a choice that no longer
/// exists. The refusal names the change and says which backend the build gets,
/// rather than reading as a flag that was never heard of.
#[test]
fn the_backend_flag_is_gone() {
    let out = kelpc()
        .args(["build", "--backend=c"])
        .arg(fixture("tests/fixtures/while_accumulator.kp"))
        .arg("-o")
        .arg(out_path("backend_flag_gone"))
        .output()
        .expect("failed to spawn kelpc");
    assert!(!out.status.success(), "--backend must be refused");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        stderr.trim_end(),
        format!("error: {FLAG_GONE}"),
        "the refusal for --backend=c should name the change"
    );
}

/// A shell that still exports `KELPC_BACKEND` is the case silence would get
/// wrong: its builds would use `llvm` while the export says the C backend. The
/// variable is refused by name whatever it holds, so the caller learns the
/// truth instead of measuring the wrong backend.
#[test]
fn the_backend_environment_variable_is_gone() {
    for value in ["c", "llvm"] {
        let out = kelpc()
            .args(["build"])
            .arg(fixture("tests/fixtures/while_accumulator.kp"))
            .arg("-o")
            .arg(out_path("backend_env_gone"))
            .env("KELPC_BACKEND", value)
            .output()
            .expect("failed to spawn kelpc");
        assert!(
            !out.status.success(),
            "KELPC_BACKEND={value} must be refused"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(
            stderr.trim_end(),
            format!("error: {ENV_GONE}"),
            "the refusal for KELPC_BACKEND={value} should name the change"
        );
    }
}

/// The oracle switch is not a choice: a value that is not the C backend's name
/// is refused by name, so a suite run cannot believe it measured the oracle and
/// measure the default instead.
#[test]
fn a_non_oracle_value_for_the_oracle_switch_is_refused() {
    for value in ["llvm", "own", "rust"] {
        let out = kelpc()
            .args(["build"])
            .arg(fixture("tests/fixtures/while_accumulator.kp"))
            .arg("-o")
            .arg(out_path("backend_oracle_bad"))
            .env("KELPC_ORACLE", value)
            .output()
            .expect("failed to spawn kelpc");
        assert!(
            !out.status.success(),
            "KELPC_ORACLE={value} must be refused"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains(&format!("KELPC_ORACLE names '{value}'"))
                && stderr.contains("accepts only 'c'"),
            "KELPC_ORACLE={value} should be refused with the one value it accepts, got:\n{stderr}"
        );
    }
}

/// An empty value is a mistake, not an instruction to use the default: a script
/// that computes the value and produces an empty string must fail rather than
/// silently measure the default backend.
#[test]
fn an_empty_oracle_value_is_reported_rather_than_defaulted() {
    let out = kelpc()
        .args(["build"])
        .arg(fixture("tests/fixtures/while_accumulator.kp"))
        .arg("-o")
        .arg(out_path("backend_oracle_empty"))
        .env("KELPC_ORACLE", "")
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        !out.status.success(),
        "an empty oracle value must be reported"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("KELPC_ORACLE is set but empty"), "{stderr}");
}

// ---------------------------------------------------------------------------
// A local binding: a second `val`/`var` of one name in a block is a shadow.
// ---------------------------------------------------------------------------

/// BUG-v6Qe0: the C backend turns each local into a C variable named after the
/// binding, so a rebinding in one block reached `cc` as a redefinition of
/// `kelp_a`. Every shape the report named is in the fixture: a scalar, a `str`,
/// a `datac` and a container, and each read after the shadow has to see the
/// newest binding. It also pins the read `val n = n + 1` makes: the initializer
/// names the binding the new one shadows, so it reads the one already in scope.
#[test]
fn a_local_rebinding_shadows_the_earlier_binding() {
    assert_compiles_and_runs(
        "tests/fixtures/shadowed_local_in_one_block.kp",
        "a=2\ns=two\np=3,4\ncards=1 7\ninner=2\nouter=1\n",
    );
}

/// BUG-mW3tZ: a `Unit` value may be bound. The local is zero-sized, so it gets
/// no storage and no drop, and the initializer's call still runs: the two
/// backends used to answer this with a compiler-defect message (LLVM) and with
/// `variable has incomplete type void` from `cc`. The annotated form of the
/// same binding is `val y: unit` (the language spells the type `unit`), and the
/// generic case below is the reason the binding is allowed at all.
#[test]
fn a_unit_valued_call_can_be_bound_to_a_local() {
    assert_compiles_and_runs("tests/fixtures/unit_val_binding.kp", "1\n");
}

/// A generic function instantiated at `unit`: the instantiation takes the value
/// as an argument and returns it, a plain function declares a `unit` parameter,
/// and a bound `unit` is passed on to it. None of the three crosses a value in
/// either backend's machine signature.
#[test]
fn a_generic_instantiated_at_unit_can_be_bound_and_passed_on() {
    assert_compiles_and_runs(
        "tests/fixtures/unit_val_generic_binding.kp",
        "1\n2\n",
    );
}

// ---------------------------------------------------------------------------
// MK9-S3-M1 stage A1: the atomic cell `Atomic<T>`, and the one ordering v1 has.
// ---------------------------------------------------------------------------

/// A counter incremented and read through an atomic matches a plain integer's
/// result, and a `compare_exchange` retry loop converges on the same total. The
/// atomic is a `pd val`, which is how a counter is shared once threads exist:
/// a passdown declared `val` hands a function a shared borrow, which is what an
/// atomic mutates through.
#[test]
fn an_atomic_counter_matches_a_plain_integer_and_a_compare_exchange_loop_converges() {
    assert_compiles_and_runs(
        "tests/fixtures/atomic_counter.kp",
        "100\n0\n100\n200\n200\n0\n",
    );
}

/// Every `fetch_*` over the widths an emitter can get wrong, each returning the
/// value that was there and leaving the new one behind.
#[test]
fn the_atomic_fetch_operations_report_the_value_that_was_there() {
    assert_compiles_and_runs(
        "tests/fixtures/atomic_int_fetch.kp",
        "200\n200\n210\n205\n192\n195\n194\n1\n40001\n-3\n-3\n-3\n-2\n9000000000\n8999999999\n0\n1000000\n1000001\n0\n-8\n1\n4294967295\n",
    );
}

/// `AtomicBool` has new/load/store/swap/compare_exchange and no arithmetic, and
/// a test-and-set through a `pd val` flag lets exactly one attempt see `false`.
#[test]
fn an_atomic_bool_has_no_arithmetic_and_a_test_and_set_wins_once() {
    assert_compiles_and_runs(
        "tests/fixtures/atomic_bool.kp",
        "true\ntrue\nfalse\nfalse\ntrue\ntrue\nfalse\ntrue\n",
    );
}

/// The `Err` half of `compare_exchange` carries the value that was actually
/// there, and a failed exchange leaves the cell alone.
#[test]
fn a_failed_compare_exchange_reports_the_value_that_was_there() {
    assert_compiles_and_runs(
        "tests/fixtures/atomic_cas_err.kp",
        "5\n5\n5\n8\n",
    );
}

/// The ordering slot is a real argument of a real enum, so naming an ordering
/// that does not exist yet is the ordinary unknown-variant error rather than a
/// special refusal: a second variant is a widening of `Ordering`.
#[test]
fn an_ordering_variant_that_does_not_exist_is_the_ordinary_unknown_variant_error() {
    assert_rejected("tests/fixtures/atomic_ordering_reserved.kp");
}

/// The element set is closed: `bool` and the integer types, so a float (which
/// has no atomic arithmetic to lower to) is refused by name.
#[test]
fn a_float_atomic_element_is_refused() {
    assert_rejected("tests/fixtures/atomic_element_refused.kp");
}

/// A-10 slice 1 closed MK9 9.3's open question: an atomic stays refused as a
/// container element, because a container's own routines read and write an
/// element as a plain location and every access to an atomic has to be one
/// atomic operation.
#[test]
fn an_atomic_cannot_be_a_list_element() {
    assert_rejected_with(
        "tests/fixtures/atomic_list_element.kp",
        "'Atomic<i32>' cannot be a 'List' element",
    );
}

/// The same refusal per container, so each phrasing of the one reason is
/// pinned: a map value is read and written as a plain location by the map's own
/// routines, a box's own routines read and write its value the same way, and an
/// array holds its elements inline.
#[test]
fn an_atomic_cannot_be_a_map_value_or_a_box_or_array_element() {
    assert_rejected_with(
        "tests/fixtures/atomic_map_value.kp",
        "'Atomic<i32>' cannot be a 'Map' value",
    );
    assert_rejected_with(
        "tests/fixtures/atomic_box_element.kp",
        "'Atomic<i32>' cannot be a 'Box' element",
    );
    assert_rejected_with(
        "tests/fixtures/atomic_array_element.kp",
        "'Atomic<i32>' cannot be an array element",
    );
}

// ---------------------------------------------------------------------------
// MK9-S3-M1 stage A2: `thread.fence`, the standalone fence.
// ---------------------------------------------------------------------------

/// `thread.fence(order)` compiles, runs, and changes no stdout: a fence orders
/// no location, so a single-threaded program has nothing for it to order. The
/// value is that the *call* is exercised at all, since a fence that lowered to
/// nothing would run exactly the same way; the shape test below is what pins the
/// instruction.
#[test]
fn a_thread_fence_compiles_runs_and_changes_no_stdout() {
    assert_compiles_and_runs("tests/fixtures/thread_fence.kp", "0\n0\n7\n0\n");
}

/// The instruction `thread.fence` lowers to, in the artifact the backend under
/// test wrote: `__atomic_thread_fence(__ATOMIC_SEQ_CST)` in the generated C and
/// `fence seq_cst` in the emitted IR (MK9 section 6, stage A2). Both files sit
/// beside the binary, which is the one place either backend's own output is
/// readable, so a program that merely printed the same stdout cannot pass this.
#[test]
fn a_thread_fence_lowers_to_the_fence_instruction_of_its_backend() {
    let source = fixture("tests/fixtures/thread_fence.kp");
    let bin = out_path("thread_fence");
    let build = kelpc()
        .arg("build")
        .arg(&source)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected 'thread_fence.kp' to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    // The C backend spells the fence as the C builtin at the call site; the
    // LLVM backend emits the instruction.
    let (artifact, want) = match backend().as_str() {
        "c" => (bin.with_extension("c"), "__atomic_thread_fence(__ATOMIC_SEQ_CST)"),
        _ => (bin.with_extension("ll"), "fence seq_cst"),
    };
    let generated = std::fs::read_to_string(&artifact).unwrap_or_else(|e| {
        panic!(
            "the {} backend writes no '{}': {e}",
            backend(),
            artifact.display()
        )
    });
    assert!(
        generated.contains(want),
        "the {} backend's artifact has no '{want}':\n{generated}",
        backend()
    );
}

// ---------------------------------------------------------------------------
// MK9-S3-M1 stages B1 and C1: the `thread.spawn` / `thread.join` surface, the
// argument rules of MK9 section 5, and the two gates section 7 asks for. Stage
// B1 landed the checker and every rule as a *rejection*; stage C1 lowers both
// calls, so the four rule probes are still refused by name and the accepted
// shapes now build and run.
// ---------------------------------------------------------------------------

/// Rule 1: an exclusive borrow never crosses to a worker, because the worker
/// outlives the frame that made the borrow and the borrow is the promise that
/// no other path reaches the value.
#[test]
fn a_mut_borrow_argument_to_spawn_is_refused() {
    assert_rejected_with(
        "tests/fixtures/thread_spawn_mut_borrow.kp",
        "'thread.spawn' cannot send '&mut Atomic<i64>'",
    );
}

/// Rule 2: a shared borrow may cross only when the place it borrows has process
/// lifetime, which in v1 means a `pd` global, so a borrow of a frame local is
/// refused.
#[test]
fn a_borrow_of_a_frame_local_is_refused() {
    assert_rejected_with(
        "tests/fixtures/thread_spawn_local_borrow.kp",
        "borrows a frame local that ends before the worker does",
    );
}

/// Rule 2's other shape: a `str` (and a `View`, the same thing with a length)
/// is borrowed text with no place of its own.
#[test]
fn a_str_argument_to_spawn_is_refused() {
    assert_rejected_with(
        "tests/fixtures/thread_spawn_str_arg.kp",
        "'thread.spawn' cannot send a 'str'",
    );
}

/// Rule 3: a shared borrow of a `pd` whose pointee has a reachable `var` field
/// is refused, because two threads could write it through the same borrow.
#[test]
fn a_shared_borrow_reaching_a_var_field_is_refused() {
    assert_rejected_with(
        "tests/fixtures/thread_spawn_var_field.kp",
        "the borrow reaches a 'var' field",
    );
}

/// Rule 4: a moved value that carries a borrow cannot be constructed in the
/// first place, so `thread.spawn` opens no hole there.
#[test]
fn a_moved_value_that_carries_a_borrow_is_still_refused() {
    assert_rejected_with(
        "tests/fixtures/thread_spawn_borrowed_value.kp",
        "construct it from a string literal or from an earlier String/Bytes field",
    );
}

/// Rules 2 and 3 in their accepted shape: a `&Atomic<T>` of a `pd` global is
/// the one borrow that crosses to a worker, and stage C1 lowers the call.
#[test]
fn a_shared_pd_atomic_is_the_one_borrow_that_crosses() {
    assert_compiles_and_runs("tests/fixtures/thread_spawn_pd_atomic.kp", "1\n");
}

/// The accepted shapes: a declared `fun`, a capturing closure, and a handle
/// typed as a parameter, a `datac` field and an `enum` payload. Stage C1 lowers
/// all of them, so each builds and runs.
#[test]
fn an_accepted_spawn_or_join_builds_and_runs() {
    assert_compiles_and_runs("tests/fixtures/thread_spawn_accepted.kp", "42\n");
    assert_compiles_and_runs("tests/fixtures/thread_spawn_closure.kp", "42\n");
    assert_compiles_and_runs("tests/fixtures/thread_join_accepted.kp", "1\n");
}

/// The stress gate (MK9 section 7): eight workers, each doing a thousand
/// `fetch_add`s of one shared `pd val Atomic<i64>`, every one of them joined
/// before the total is read. A lost update anywhere makes the total something
/// other than 8000, which is the one thing a single-threaded counter test
/// cannot catch.
#[test]
fn a_thread_stress_gate_produces_the_exact_total() {
    assert_compiles_and_runs("tests/fixtures/thread_stress.kp", "8000\n");
}

/// B-5's mutex stress gate: eight workers, each taking the lock a thousand
/// times to increment one shared `pd val Mutex<i64>`. A missing lock shows up
/// as a total other than 8000.
#[test]
fn a_mutex_stress_gate_produces_the_exact_total() {
    assert_compiles_and_runs("tests/fixtures/mutex_stress.kp", "8000\n");
}

/// B-5: a `Mutex<List<i32>>` shared across four workers, each pushing through
/// the lock. The element's own drop reaches the list through the runtime
/// record, so the whole list is dropped when the mutex dies.
#[test]
fn a_mutex_guards_a_list_pushed_from_several_workers() {
    assert_compiles_and_runs("tests/fixtures/mutex_list_push.kp", "40\n");
}

/// B-5: `m.lock { v -> ... }` evaluates to the block's tail, and `into_inner`
/// consumes the mutex and hands the guarded value out.
#[test]
fn a_mutex_block_value_is_returned_and_into_inner_consumes_the_mutex() {
    assert_compiles_and_runs("tests/fixtures/mutex_lock_value.kp", "8\n14\n7\n");
    assert_compiles_and_runs("tests/fixtures/mutex_into_inner.kp", "2\n41\n");
}

/// B-5: a `return` inside a lock block ends the block, the runtime releases
/// the lock on that exit, and a second `lock` afterwards succeeds. A `?`
/// inside the block is the same kind of exit, and a second call that would
/// deadlock if the first leaked the lock proves it.
#[test]
fn a_return_inside_a_lock_block_releases_the_mutex() {
    assert_compiles_and_runs("tests/fixtures/mutex_lock_return.kp", "2\n");
    assert_compiles_and_runs("tests/fixtures/mutex_lock_try.kp", "10\n");
}

/// B-5: a worker's panic poisons the mutex through the runtime's cleanup
/// handler, and the main thread's next `lock` panics with "mutex poisoned".
#[test]
fn a_panic_inside_a_lock_block_poisons_the_mutex() {
    assert_fixture_panics("mutex_poison", "mutex poisoned");
}

/// B-5: a `&Mutex<T>` of a `pd` global crosses to a worker, the `Sync` half of
/// the crossing rule. A plain `var` datac stays refused
/// (`thread_spawn_var_field.kp`).
#[test]
fn a_shared_pd_mutex_crosses_to_a_worker() {
    assert_compiles_and_runs("tests/fixtures/mutex_spawn_borrow.kp", "1\n");
}

/// B-5: the block's `v` cannot escape. Returning it is a type mismatch, and
/// capturing it into a closure's environment is refused by the same borrow
/// rule a scoped block follows.
#[test]
fn a_lock_blocks_mutable_borrow_cannot_escape() {
    assert_rejected_with(
        "tests/fixtures/mutex_lock_borrow_returned.kp",
        "expected 'unit', found 'mut i64'",
    );
    assert_rejected_with(
        "tests/fixtures/mutex_lock_borrow_stored.kp",
        "cannot capture 'v': it is a borrow",
    );
}

/// B-5: a freestanding build refuses `Mutex` and names the spin lock a kernel
/// would use instead. Freestanding is LLVM-only by design, so the refusal is
/// reached before either backend.
#[test]
fn a_freestanding_mutex_is_refused() {
    assert_rejected_with(
        "tests/projects/freestanding_mutex",
        "a freestanding program has no threads and no libc",
    );
}

/// `RwLock`: read/write blocks, `try_*` returning `Option`, and `into_inner`.
#[test]
fn an_rwlock_returns_block_values_and_takes_try_and_into_inner() {
    assert_compiles_and_runs("tests/fixtures/rwlock_basic.kp", "8\n10\n10\n");
    assert_compiles_and_runs("tests/fixtures/rwlock_try.kp", "1\ntrue\n1\ntrue\n5\n");
}

/// A `&RwLock<T>` of a `pd` global crosses to a worker, the `Sync` half of the
/// crossing rule.
#[test]
fn a_shared_pd_rwlock_crosses_to_a_worker() {
    assert_compiles_and_runs("tests/fixtures/rwlock_spawn_borrow.kp", "1\n");
}

/// Two readers hold the lock at once, and a writer already waiting goes before
/// a reader that arrives afterwards. The ordering is recorded with atomics, so
/// no sleep is used as proof.
#[test]
fn readers_overlap_and_a_waiting_writer_goes_first() {
    assert_compiles_and_runs("tests/fixtures/rwlock_concurrent.kp", "true\n1\n");
}

/// A worker's panic inside `write` poisons the rwlock; a later `write` panics.
#[test]
fn a_panic_inside_a_write_poisons_the_rwlock() {
    assert_fixture_panics("rwlock_poison", "rwlock poisoned");
}

/// A worker's panic inside `read` releases the read without poisoning, so a
/// later `read` succeeds and the value is unchanged.
#[test]
fn a_panic_inside_a_read_releases_without_poisoning() {
    assert_compiles_and_runs(
        "tests/fixtures/rwlock_read_releases.kp",
        "worker failed\n7\n",
    );
}

/// The rwlock block's borrow cannot escape: returning it is a type mismatch,
/// capturing it into a closure is refused by the borrow rule.
#[test]
fn an_rwlock_block_borrow_cannot_escape() {
    assert_rejected_with(
        "tests/fixtures/rwlock_read_borrow_returned.kp",
        "expected 'unit', found '&i64'",
    );
    assert_rejected_with(
        "tests/fixtures/rwlock_read_borrow_stored.kp",
        "cannot capture 'v': it is a borrow",
    );
}

/// A freestanding build refuses `RwLock` and names the spin lock a kernel would
/// use instead.
#[test]
fn a_freestanding_rwlock_is_refused() {
    assert_rejected_with(
        "tests/projects/freestanding_rwlock",
        "a freestanding program has no threads and no libc",
    );
}

/// `Condvar`: a handshake wakes a waiter, and the timed wait returns `None` at
/// the deadline and `Some` once the predicate is false.
#[test]
fn a_condvar_handshake_wakes_a_waiter() {
    assert_compiles_and_runs("tests/fixtures/condvar_handshake.kp", "42\n");
    assert_compiles_and_runs("tests/fixtures/condvar_timeout.kp", "true\n7\n");
}

/// A freestanding build refuses `Condvar`.
#[test]
fn a_freestanding_condvar_is_refused() {
    assert_rejected_with(
        "tests/projects/freestanding_condvar",
        "a freestanding program has no threads and no libc",
    );
}

/// `Mutex.share`/`Condvar.share` make another handle to the same record, and
/// `Mutex.swap` exchanges the guarded value, which is how an owned value gets
/// into a mutex from inside a scoped block.
#[test]
fn a_shared_mutex_handle_and_a_swap() {
    assert_compiles_and_runs("tests/fixtures/mutex_share_swap.kp", "6\n6\n40\na\n0\n");
}

/// Channels (`std/sync`): four producers feed one consumer through a bounded
/// channel, and the receiver's `for` ends by itself when the last sender drops.
#[test]
fn a_bounded_channel_carries_four_producers_to_one_consumer() {
    assert_compiles_and_runs("tests/fixtures/chan_bounded_mpsc.kp", "1000\n124500\n");
}

/// Closing in both directions: every sender gone drains then fails, a clone
/// keeps the channel open, and a send to a dropped receiver gets the value back.
#[test]
fn a_channel_closes_when_its_senders_or_its_receiver_drop() {
    assert_compiles_and_runs(
        "tests/fixtures/chan_close.kp",
        "6\n1\n2\n3\n-1\nopen\nclosed\nback\n",
    );
}

/// `recv_timeout` times out, takes a late value, and reports a closed channel.
#[test]
fn a_channel_recv_timeout_times_out_and_then_receives() {
    assert_compiles_and_runs("tests/fixtures/chan_recv_timeout.kp", "timeout\n99\nclosed\n");
}

/// A zero-capacity bounded channel is a named panic.
#[test]
fn a_zero_capacity_channel_panics() {
    assert_fixture_panics("chan_zero_capacity", "sync.bounded: the capacity must be at least 1");
}

/// A channel carries owned, movable values: a borrowed `str` and a lock are
/// both refused.
#[test]
fn a_channel_refuses_a_borrowed_or_a_lock_element() {
    assert_rejected_with("tests/fixtures/chan_non_sendable.kp", "'str' is a borrowed view");
    assert_rejected_with("tests/fixtures/chan_send_a_lock.kp", "'Mutex<i64>' cannot be a 'List' element");
}

/// A freestanding build refuses the channel module by name.
#[test]
fn a_freestanding_channel_is_refused() {
    assert_rejected_with(
        "tests/projects/freestanding_channel",
        "the module 'std/sync' is unavailable in a freestanding build",
    );
}

/// The thread pool (`std/pool`): 1000 jobs on four workers with a sum check.
#[test]
fn a_pool_runs_a_thousand_jobs() {
    assert_compiles_and_runs("tests/fixtures/pool_sum.kp", "499500\n0\n");
}

/// A panicking job is counted and its worker survives to run the rest.
#[test]
fn a_pool_counts_a_panicking_job_and_keeps_its_worker() {
    assert_compiles_and_runs("tests/fixtures/pool_panics.kp", "27\n4\n5\n");
}

/// Dropping a pool joins it, an idle or twice-joined pool shuts down, and jobs
/// own what they capture (here a channel's sender).
#[test]
fn a_pool_shuts_down_without_deadlock() {
    assert_compiles_and_runs("tests/fixtures/pool_drop_joins.kp", "50\nidle\n2870\n");
}

/// Queueing on a joined pool and an empty pool are named panics.
#[test]
fn a_pool_refuses_work_after_join_and_zero_workers() {
    assert_fixture_panics("pool_spawn_after_join", "Pool.spawn: the pool has been joined");
    assert_fixture_panics("pool_zero_workers", "pool.new: a pool needs at least one worker");
}

/// A freestanding build refuses the pool module by name.
#[test]
fn a_freestanding_pool_is_refused() {
    assert_rejected_with(
        "tests/projects/freestanding_pool",
        "the module 'std/pool' is unavailable in a freestanding build",
    );
}

/// The panic gate (MK9 section 7): a worker panics, its joiner sees `Err`
/// carrying the panic text, a second worker's work is intact, and the process
/// exits 0. A panic ends the thread it happened in; `main` still decides what to
/// do about it (MK9 section 2).
#[test]
fn a_panicking_worker_is_seen_as_an_error_by_its_joiner() {
    assert_compiles_and_runs(
        "tests/fixtures/thread_panic.kp",
        "worker exploded\n0\n42\n",
    );
}

/// `MISSING-r7Wqb`: a worker that returns `unit` can be spawned *and* joined.
/// `thread.join` on a `Thread<unit>` builds `Result<unit, ThreadErr>`, whose
/// `Ok` variant now lays out payload-less rather than refusing `unit`.
#[test]
fn a_worker_that_returns_unit_can_be_joined() {
    assert_compiles_and_runs("tests/fixtures/thread_join_unit.kp", "1\njoined\n");
}

/// `MISSING-r7Wqb`'s general case: a `Result<unit, E>` written by hand, with
/// no thread anywhere. `Ok`'s field is `unit`; constructing one still runs the
/// value expression for its side effect even though nothing is stored.
#[test]
fn a_hand_written_result_of_unit_lays_out_and_runs() {
    assert_compiles_and_runs("tests/fixtures/result_unit_payload.kp", "ok\n1\n7\n");
}

/// MK9-S3-M1 stage C2: `thread.sleep_ms` sleeps the calling thread (main or a
/// worker) and returns control to it, and `thread.current_id` is stable
/// across two calls on the same thread and distinct between the main thread
/// and a worker it spawned.
#[test]
fn thread_sleep_ms_and_current_id_work_on_both_threads() {
    assert_compiles_and_runs("tests/fixtures/thread_sleep.kp", "42\n");
    assert_compiles_and_runs("tests/fixtures/thread_current_id.kp", "true\nfalse\n");
}

/// perf-env-cache: `env.has`/`env.get`/`env.set`/`env.unset` from three
/// threads at once, each on a name none of the others touches. This is what
/// caught the real bug in the first version of the environment read cache:
/// an unguarded scan of `environ` (built once per lookup, twice per
/// `set`/`unset` here) racing a concurrent thread's own `setenv`/`unsetenv`
/// segfaulted reliably (glibc may `realloc` the whole `environ` array), which
/// a single-threaded `set`-then-`get` test has no way to see. The three
/// per-worker names keep the joined output deterministic no matter how the
/// threads interleave.
#[test]
fn env_reads_and_writes_from_several_threads_at_once_are_safe() {
    assert_compiles_and_runs("tests/fixtures/env_thread_stress.kp", "true\ntrue\ntrue\n");
}

/// BUG-hT3wK: `str.byte` bounds-checks with a scan of `index + 1` bytes instead
/// of a `strlen`; the answers at the edges, and the panics one past them, are
/// unchanged.
#[test]
fn str_byte_keeps_its_answers_and_bounds_at_the_edges() {
    assert_compiles_and_runs("tests/fixtures/str_byte_edges.kp", "104\n111\n5\n0\n");
    assert_fixture_panics("str_byte_at_len", "str.byte: index out of bounds");
    assert_fixture_panics("str_byte_empty", "str.byte: index out of bounds");
}

/// BUG-fY3nQ: rebuilds free the table they replace while readers keep probing.
/// The readers' answers do not depend on the interleaving (one name stays set,
/// one is never set); the leak half of the fix is gated by `memory_gate.rs`,
/// which runs this fixture under valgrind with every other one.
#[test]
fn env_table_rebuilds_do_not_disturb_concurrent_readers() {
    assert_compiles_and_runs("tests/fixtures/env_thread_churn.kp", "200000\n200000\n3000\n3000\n");
}

/// `thread.join` consumes a `Thread<T>` and nothing else: a handle is the one
/// thing `thread.spawn` makes.
#[test]
fn join_refuses_a_value_that_is_not_a_thread_handle() {
    assert_rejected_with(
        "tests/fixtures/thread_join_not_a_handle.kp",
        "takes a 'Thread<T>' handle",
    );
}

/// A `Thread<T>` is refused as a `List`/`Map`/`Box`/array element, the rule an
/// `Atomic<T>` already has: a handle names one running worker and is joined
/// once, while a container's own routines copy and drop an element like a plain
/// location.
#[test]
fn a_thread_handle_cannot_be_a_container_element() {
    for (fixture, container) in [
        ("thread_spawn_list_element", "a 'List' element"),
        ("thread_spawn_map_element", "a 'Map' value"),
        ("thread_spawn_box_element", "a 'Box' element"),
        ("thread_spawn_array_element", "an array element"),
    ] {
        assert_rejected_with(
            &format!("tests/fixtures/{fixture}.kp"),
            &format!("'Thread<i64>' cannot be {container}"),
        );
    }
}

// ---------------------------------------------------------------------------
// MISSING-z2bB9: tuple types and the destructuring binding that takes one
// apart (`plan/syntax.md` 3 and 4). A tuple is a record with positional
// fields, so it is a value like every other owned one: it owns its elements,
// it takes the derived deep copy and the derived order a `datac` takes in
// position order, it may appear in every position the element rules allow, and
// it is refused in the positions they refuse.
// ---------------------------------------------------------------------------

/// The item's own example: a function hands back two things as one tuple, and
/// the caller takes it apart, so no one-shot `datac` is needed per pair.
#[test]
fn a_function_hands_back_two_things_as_a_tuple() {
    assert_compiles_and_runs("tests/fixtures/tuple_min_max.kp", "1\n9\n8\n");
}

/// A tuple is a `datac` field, a parameter (a borrow) and a `move` parameter,
/// a `List` element, a `Map` key and value, a `Box` element, an array element,
/// an `enum` payload and a generic argument. Every position follows the element
/// rule of the position rather than a second set of rules of its own, so this
/// fixture is also what says a tuple key is keyable by derivation.
#[test]
fn a_tuple_is_a_value_in_every_position() {
    assert_compiles_and_runs("tests/fixtures/tuple_datac_field.kp", "4\n5\n14\n");
    assert_compiles_and_runs("tests/fixtures/tuple_param.kp", "3\nmoved\n7\n");
    assert_compiles_and_runs(
        "tests/fixtures/tuple_placements.kp",
        "30\n15\n3\n2\n4\ntrue\n19\n6\n",
    );
}

/// A tuple has two or more elements: `(T)` is the parenthesized type `T` and
/// `(e)` the parenthesized expression `e`. A tuple may hold another tuple, a
/// destructuring binding may nest, `_` ignores a position at any level, and
/// `var (a, b) = ...` makes every position a `var`.
#[test]
fn a_tuple_has_two_or_more_positions_and_a_pattern_takes_it_apart() {
    assert_compiles_and_runs("tests/fixtures/tuple_one_element.kp", "42\n42\n42\n");
    assert_compiles_and_runs("tests/fixtures/tuple_nested.kp", "1\n3\n11\n5\n3\n");
}

/// `?` reaches the `Result` a tuple *element* may be and never the tuple:
/// `mk(n)?` is the tuple the call succeeded with, and it destructures like any
/// other value.
#[test]
fn question_about_a_tuple_element_is_about_that_element() {
    assert_compiles_and_runs("tests/fixtures/tuple_try.kp", "5\n6\nnegative\n");
}

/// A tuple takes the derived deep copy and the derived order a `datac` takes,
/// position by position. The copy is deep (the original list is cleared before
/// the copy is read, so a shared buffer is a use-after-free) and the order is
/// the first position's, then the second's, which is what `copy.sort()` and the
/// reads after it pin.
#[test]
fn a_tuple_gets_the_derived_copy_and_order_a_datac_does() {
    assert_compiles_and_runs(
        "tests/fixtures/tuple_derived_copy_sort.kp",
        "3\na\n1\na\n3\nb\n2\n0\n7\nkept\n",
    );
}

/// The refusals, each naming the rule it breaks: the empty type, a borrowed
/// element, the absent value as an element, a pattern whose position count does
/// not match, a destructuring of something that is not a tuple, an element
/// naming a type declared later (open item 12, the by-value field rule), an
/// owned element read out of a borrow, the initializer used twice, and a tuple
/// handed to `print`.
#[test]
fn the_tuple_surface_is_refused_where_its_rules_refuse_it() {
    for (fixture, needle) in [
        ("tuple_empty_type", "'()' is not a type"),
        (
            "tuple_borrowed_element",
            "element 1 of the tuple is a borrowed type ('str')",
        ),
        ("tuple_unit_element", "element 2 of the tuple is 'unit'"),
        ("tuple_arity", "this pattern has 3 position(s)"),
        (
            "tuple_not_a_tuple",
            "a destructuring binding takes one tuple value apart",
        ),
        ("tuple_later_element", "'Later' is declared after this type"),
        (
            "tuple_element_borrow",
            "cannot read element 1 of the tuple out of a borrow by value",
        ),
        ("tuple_moved_twice", "use of moved value 'p'"),
        (
            "tuple_print",
            "cannot pass a value of type '(i32, i32)' to 'print'",
        ),
    ] {
        assert_rejected_with(&format!("tests/fixtures/{fixture}.kp"), needle);
    }
}

// Open item 75 prototype (`plan/trailing-closure.md`): a `{ ... }` on the same
// line as a call is that call's last argument, a zero-parameter Unit `fun`.
#[test]
fn a_trailing_block_is_the_calls_last_argument() {
    assert_compiles_and_runs("tests/fixtures/trailing_block.kp", "a\na\nb\nc\nshow\nshow\n");
}

#[test]
fn a_trailing_block_can_capture_into_a_closure_parameter() {
    assert_compiles_and_runs("tests/fixtures/trailing_block_capture.kp", "3\n3\n");
}

/// A program whose reader goes away must be ended by SIGPIPE, as every Unix tool
/// is. The TLS code blocks the signal only around the OpenSSL calls that can
/// write, never process-wide: a runtime that ignored it would leave this loop
/// writing to a closed pipe forever, and `producer | head` would never return.
#[test]
fn a_program_is_ended_by_sigpipe_when_its_reader_closes_the_pipe() {
    use std::io::{BufRead, BufReader};
    use std::os::unix::process::ExitStatusExt;
    let bin = out_path("pipe_producer");
    let build = kelpc()
        .arg("build")
        .arg(fixture("tests/hazards/pipe_producer.kp"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected 'pipe_producer' to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let mut child = Command::new(&bin)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("failed to run compiled binary");
    let mut reader = BufReader::new(child.stdout.take().expect("piped stdout"));
    let mut first = String::new();
    reader.read_line(&mut first).expect("the producer prints a first line");
    assert_eq!(first.trim(), "0");
    drop(reader);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let status = loop {
        if let Some(s) = child.try_wait().expect("wait on the producer") {
            break s;
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            panic!("the producer kept running after its reader closed the pipe: SIGPIPE is being ignored");
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    assert_eq!(status.signal(), Some(13), "expected death by SIGPIPE, got {status:?}");
}

// `plan/scoped-blocks.md`: a trailing block declares its parameters by name
// only and returns its tail where the expected type is non-unit, and a
// `scoped fun(...)` parameter accepts such a block (or a `fun` literal)
// written at the call site, capturing the enclosing frame by borrow.
#[test]
fn a_scoped_block_takes_its_parameters_and_return_from_the_parameter() {
    assert_compiles_and_runs(
        "tests/fixtures/scoped_block_params.kp",
        "4\n5\n6\n150\n101\n102\n12\n6\n",
    );
}

#[test]
fn a_scoped_block_can_read_a_borrowed_parameter() {
    assert_compiles_and_runs(
        "tests/fixtures/scoped_block_borrow.kp",
        "row\nrow\n203\nrow\n",
    );
}

#[test]
fn a_scoped_block_captures_owned_copy_and_borrowed_values() {
    assert_compiles_and_runs(
        "tests/fixtures/scoped_block_captures.kp",
        "108\n109\n7\n2030\n5\n",
    );
}

#[test]
fn a_scoped_block_in_a_loop_sees_the_current_iteration() {
    assert_compiles_and_runs(
        "tests/fixtures/scoped_borrow_in_loop.kp",
        "1\n2\n2\n3\n3\n4\n",
    );
}

#[test]
fn a_generic_scoped_parameter_types_its_block_from_the_expected_return() {
    assert_compiles_and_runs("tests/fixtures/scoped_block_generic.kp", "3\n");
}

#[test]
fn a_scoped_value_cannot_be_stored_in_a_datac() {
    assert_rejected_with(
        "tests/fixtures/scoped_param_stored.kp",
        "scoped function value",
    );
}

#[test]
fn a_scoped_value_cannot_be_returned() {
    assert_rejected_with(
        "tests/fixtures/scoped_param_returned.kp",
        "return type mismatch",
    );
}

#[test]
fn a_scoped_block_can_write_what_it_captures() {
    // `plan/exclusive-captures.md`: a write through a captured `var` makes the
    // capture exclusive for the call.
    assert_compiles_and_runs("tests/fixtures/excl_capture_field.kp", "13\n113\n");
    assert_compiles_and_runs("tests/fixtures/excl_capture_copy_var.kp", "1006\n3\n");
    assert_compiles_and_runs("tests/fixtures/excl_capture_list_push.kp", "0\n10\n20\n3\n");
    assert_compiles_and_runs("tests/fixtures/excl_capture_mut_param.kp", "103\n");
    assert_compiles_and_runs("tests/fixtures/excl_capture_two_owners.kp", "63\n2\n2\n");
    assert_compiles_and_runs("tests/fixtures/excl_capture_returns_value.kp", "40\n3\n");
    assert_compiles_and_runs("tests/fixtures/excl_capture_defer_loop.kp", "3\n30\n");
    assert_compiles_and_runs("tests/fixtures/excl_capture_drain.kp", "1\n6\n8\n8\n");
    assert_compiles_and_runs("tests/fixtures/excl_capture_view.kp", "2\n500\n");
}

#[test]
fn an_exclusive_capture_conflicts_with_the_rest_of_its_call() {
    assert_rejected_with(
        "tests/fixtures/excl_refused_same_call_arg.kp",
        "cannot borrow 's.count': already borrowed exclusively",
    );
    assert_rejected_with(
        "tests/fixtures/excl_refused_two_blocks.kp",
        "cannot borrow 's.count': already borrowed exclusively",
    );
    assert_rejected_with(
        "tests/fixtures/excl_refused_shared_arg.kp",
        "cannot borrow 's.count' as exclusive: already borrowed as shared",
    );
    assert_rejected_with(
        "tests/fixtures/excl_refused_receiver.kp",
        "cannot borrow 's.count' as exclusive: already borrowed exclusively",
    );
    // The receiver `s.bag` and the block's `s.count` are different fields of `s`.
    assert_compiles_and_runs("tests/fixtures/excl_capture_disjoint_receiver.kp", "1\n");
    assert_rejected_with(
        "tests/fixtures/excl_refused_mut_param_twice.kp",
        "needs the call's other arguments to leave it alone",
    );
    assert_rejected_with(
        "tests/fixtures/refused_mut_param_reborrow_twice.kp",
        "cannot borrow 's': already borrowed exclusively",
    );
    assert_rejected_with(
        "tests/fixtures/excl_refused_live_borrow.kp",
        "cannot borrow 's.items' as exclusive: already borrowed as shared",
    );
}

#[test]
fn a_mut_parameter_can_be_reborrowed_shared_twice_for_one_call() {
    // `borrowck::take_call_arguments` must use the *callee* parameter modes:
    // `show(s, s)` is two shared reborrows, while `touch(s, s)` is rejected.
    assert_compiles_and_runs("tests/fixtures/mut_param_shared_reborrow_twice.kp", "4\n4\n5\n");
}

#[test]
fn a_mut_parameter_cannot_go_to_a_mut_and_a_shared_parameter_of_one_call() {
    // The shared reborrow of a `mut` parameter is an `Ident`, not a `Ref`,
    // so only the exclusive reborrow used to be tracked: `f(s, s)` with one
    // `mut` and one `&` parameter was accepted in either order.
    assert_rejected_with(
        "tests/fixtures/refused_mut_param_with_shared_reborrow.kp",
        "cannot borrow 's': already borrowed exclusively",
    );
    assert_rejected_with(
        "tests/fixtures/refused_shared_then_mut_param_reborrow.kp",
        "cannot borrow 's': already borrowed exclusively",
    );
}

#[test]
fn a_tuple_has_positions_patterns_in_when_and_a_destructuring_for() {
    // `t.0`, `for (a, b) in xs`, and `when (a, b) { (Some(x), _) -> ... }`
    // (MISSING-Gt6Qc), on both backends.
    assert_compiles_and_runs("tests/fixtures/tuple_access_and_patterns.kp", "1\n2\n3\n2\n12\n8\n5\n40\n0\n3\n");
    assert_compiles_and_runs("tests/fixtures/tuple_when_owned_payloads.kp", "5\n300\n");
    assert_compiles_and_runs("tests/fixtures/tuple_index_nested.kp", "7\n9\n2\n");
    // A tuple as a `const` value.
    assert_compiles_and_runs("tests/fixtures/const_tuple.kp", "1\n3\n7\ntrue\n14\n2\n98\n");
    // Literal positions and nested tuple patterns in a `when`.
    assert_compiles_and_runs(
        "tests/fixtures/tuple_when_literals_and_nested.kp",
        "100\n7\n30\n-1\n7\n7\n5\n8\n11\n0\n1\n2\n3\n4\n",
    );
    // `for (k, v) in map` (the `Map` iteration of MISSING-Hd4Lr).
    assert_compiles_and_runs("tests/fixtures/for_over_a_map.kp", "1\n2\n3\n10\n20\n7\n5\n");
    assert_rejected_with(
        "tests/fixtures/refused_tuple_when_literals_without_fallback.kp",
        "needs an arm that matches anything there",
    );
    assert_rejected_with(
        "tests/fixtures/refused_tuple_when_not_exhaustive.kp",
        "this 'when' is not exhaustive: 'None' of enum",
    );
}

#[test]
fn assigning_to_something_that_is_not_a_place_is_an_error_not_a_crash() {
    // Found by the front-end fuzzer: `str.starts_with -= (...)` reached an
    // `unreachable!` in `check_target_writable`.
    assert_rejected_with(
        "tests/fixtures/refused_assign_to_a_function.kp",
        "cannot assign to this expression",
    );
}

#[test]
fn loop_and_labelled_break_and_continue_leave_nested_loops() {
    // MISSING-Lk9Xs: `loop { }`, `'outer: for ...`, `break 'outer` and
    // `continue 'outer`, on both backends.
    assert_compiles_and_runs("tests/fixtures/loop_and_labels.kp", "4\n4\n51\n");
    // Each exit frees the owned locals of every loop it leaves.
    assert_compiles_and_runs("tests/fixtures/loop_labels_free_owned_locals.kp", "9\n9\n7\n");
    // `loop` as a value and `break value` (the rest of MISSING-Lk9Xs).
    assert_compiles_and_runs("tests/fixtures/loop_value.kp", "9\n8\n4\n23\n");
    assert_rejected_with(
        "tests/fixtures/refused_break_value_in_statement_loop.kp",
        "only a 'loop' used as a value can 'break' with one",
    );
    assert_rejected_with(
        "tests/fixtures/refused_plain_break_in_value_loop.kp",
        "every 'break' out of it carries one",
    );
    assert_rejected_with(
        "tests/fixtures/refused_value_loop_without_break_value.kp",
        "needs a 'break value' to produce one",
    );
    assert_rejected_with(
        "tests/fixtures/refused_unknown_loop_label.kp",
        "no enclosing loop is labelled 'nope",
    );
    assert_rejected_with(
        "tests/fixtures/refused_label_across_a_closure.kp",
        "no enclosing loop is labelled 'o",
    );
}

#[test]
fn a_method_can_declare_type_parameters_of_its_own() {
    // MISSING-Pw3Fz: `fun Box2.map<U>(...)` on a plain type and
    // `fun Pair<A, B>.map_first<C>(...)` on a generic one, inferred at the
    // call, with a `mut` receiver and a trait bound.
    assert_compiles_and_runs("tests/fixtures/method_own_type_params.kp", "3\ntrue\ntrue\n2\n2\n7\n");
    assert_compiles_and_runs("tests/fixtures/method_own_type_params_bound.kp", "11\n");
}

#[test]
fn a_generic_enum_can_carry_methods_and_option_and_result_do() {
    // The receiver on a generic `enum`'s template used to be refused; now
    // `fun Opt<T>.map<U>(...)` is a method, and `std/option` / `std/result`
    // carry their borrowing combinators as methods too.
    assert_compiles_and_runs("tests/fixtures/enum_generic_method.kp", "true\n40\n");
    assert_compiles_and_runs("tests/fixtures/option_result_methods.kp", "true\nfalse\n5\n4\ntrue\nfalse\n");
}

#[test]
fn small_ergonomics_literal_float_math_f32_and_line_continuation() {
    // H-12: an integer literal where a float is expected is that float.
    assert_compiles_and_runs(
        "tests/fixtures/literal_adapts_to_float.kp",
        "5.000000\n3.000000\n5.000000\n3.000000\n2.500000\n-4.000000\n1\n",
    );
    // `f32.to_f64()` / `f64.to_f32()`, and the f32 overloads of `std/math`.
    assert_compiles_and_runs("tests/fixtures/float_width_conversion.kp", "2.500000\n4.500000\n");
    assert_compiles_and_runs(
        "tests/fixtures/math_f32_overloads.kp",
        "2.000000\n16.000000\n9.500000\n4.000000\n4.000000\n1.000000\nfalse\n",
    );
    // A line that ends in a binary operator, `,` or `.` goes on (open item 11).
    assert_compiles_and_runs("tests/fixtures/line_continuation_after_operator.kp", "16\n1\n1\n");
    // A `for (a, b)` over a temporary, whose hidden names must not meet the
    // loop desugar's own.
    assert_compiles_and_runs("tests/fixtures/for_pattern_over_a_temporary.kp", "3\n7\n");
}

#[test]
fn two_fields_of_one_owner_are_different_places_to_borrow() {
    // MISSING-Sw9Hd: roots are field paths now. Disjoint fields borrow
    // together (a call, two live borrows, a scoped block against a call's
    // other argument); anything that overlaps is still refused.
    assert_compiles_and_runs("tests/fixtures/borrow_disjoint_fields.kp", "2\n2\n12\n3\n4\n");
    assert_compiles_and_runs("tests/fixtures/excl_capture_disjoint_field.kp", "7\n");
    assert_rejected_with(
        "tests/fixtures/excl_refused_same_field.kp",
        "cannot borrow 's.items' as exclusive: already borrowed as shared",
    );
    assert_rejected_with("tests/fixtures/refused_field_and_whole.kp", "cannot borrow 's': already borrowed exclusively");
    assert_rejected_with(
        "tests/fixtures/refused_same_field_twice.kp",
        "cannot borrow 's.count': already borrowed exclusively",
    );
    assert_rejected_with(
        "tests/fixtures/refused_whole_while_field_borrowed.kp",
        "cannot move 's' while it is borrowed",
    );
    assert_rejected_with(
        "tests/fixtures/refused_field_borrow_then_whole_borrow.kp",
        "cannot borrow 's': already borrowed exclusively",
    );
    assert_rejected_with(
        "tests/fixtures/refused_field_shared_then_write.kp",
        "cannot borrow 's.items' as exclusive: already borrowed as shared",
    );
}

#[test]
fn iterator_adapters_chain_over_a_list_a_range_and_each_other() {
    // MISSING-Hd4Lr: `std/iter`'s `into_iter` and the adapters. A generic type
    // with an iterator-shaped `next` satisfies `I: Iterator` and names
    // `I::Item`, so the adapters nest without a generic trait impl.
    assert_compiles_and_runs("tests/fixtures/iter_list_take_skip.kp", "1\n2\n3\n4\n5\n7\n24\n");
    assert_compiles_and_runs(
        "tests/fixtures/iter_adapters.kp",
        "60\n80\n0\n10\n1\n11\n2\n12\n100\n102\n1\n2\n7\n8\n4\n14\n",
    );
    assert_compiles_and_runs("tests/fixtures/iter_adapters_owned.kp", "3\n5\n0\n1\n1\n2\n");
    // The consumers: any, all, find, last, fold.
    assert_compiles_and_runs("tests/fixtures/iter_consumers.kp", "true\nfalse\n10\n8\n7\n4\n");
}

#[test]
fn a_trait_can_be_implemented_for_a_generic_type_under_bounds() {
    // MISSING-Ve8Nc (the slice that is built): `impl<A: Describe, B: Describe>
    // Describe for Pair<A, B>`, an associated type over the parameters, and a
    // `where` clause as the other spelling of a bound.
    assert_compiles_and_runs("tests/fixtures/generic_impl_pair.kp", "12\n345\n");
    assert_compiles_and_runs("tests/fixtures/generic_impl_iterator.kp", "6\n6\n");
    assert_compiles_and_runs("tests/fixtures/where_clause.kp", "3\n");
    assert_rejected_with(
        "tests/fixtures/refused_generic_impl_bound.kp",
        "its impl needs 'bool' ('Pair' is parameter 'B') to implement 'Describe', and it does not",
    );
    assert_rejected_with(
        "tests/fixtures/refused_blanket_impl.kp",
        "needs the generic type it is for",
    );
}

#[test]
fn a_nested_owned_field_can_be_partially_moved_and_reinitialised() {
    assert_compiles_and_runs("tests/fixtures/nested_partial_move_field.kp", "Ada\n7\nLondon\n");
    assert_compiles_and_runs("tests/fixtures/nested_partial_move_reinit.kp", "old\nnew\n");
}

#[test]
fn a_parent_of_a_nested_partial_move_is_not_usable_as_a_whole_value() {
    assert_rejected_with(
        "tests/fixtures/refused_nested_partial_move_parent.kp",
        "use of moved value 'p.name'",
    );
}

#[test]
fn an_exclusive_capture_is_refused_where_it_cannot_be_sound() {
    assert_rejected_with(
        "tests/fixtures/excl_refused_val.kp",
        "it is not declared 'var' in the enclosing function",
    );
    assert_rejected_with(
        "tests/fixtures/excl_refused_shared_param.kp",
        "it is a shared borrow in the enclosing function",
    );
    assert_rejected_with(
        "tests/fixtures/excl_refused_returns_borrow.kp",
        "(an exclusive capture) and returns a borrow",
    );
    // A block in a block may capture the outer block's exclusive capture
    // (MISSING-Sw9Hd): the inner write is checked against the rest of the inner
    // call exactly as a local's would be.
    assert_compiles_and_runs("tests/fixtures/excl_capture_nested.kp", "2\n");
    assert_compiles_and_runs("tests/fixtures/excl_capture_nested_disjoint.kp", "2\n");
    assert_rejected_with(
        "tests/fixtures/excl_refused_nested_same_call.kp",
        "cannot borrow 'a captured value.count' as exclusive: this call also uses it",
    );
    assert_rejected_with(
        "tests/fixtures/excl_refused_stored.kp",
        "scoped function value",
    );
}

#[test]
fn a_scoped_value_cannot_reach_a_plain_function_parameter() {
    assert_rejected_with(
        "tests/fixtures/scoped_param_to_plain.kp",
        "scoped function value",
    );
}

#[test]
fn a_scoped_parameter_rejects_a_stored_value() {
    assert_rejected_with(
        "tests/fixtures/scoped_param_not_literal.kp",
        "never a stored value",
    );
}

#[test]
fn a_scoped_blocks_captured_borrow_cannot_alias_the_same_call() {
    assert_rejected_with(
        "tests/fixtures/scoped_escaping_borrow.kp",
        "already borrowed exclusively",
    );
}

#[test]
fn a_scoped_blocks_captured_borrow_cannot_outlive_a_move_in_the_same_call() {
    assert_rejected_with(
        "tests/fixtures/scoped_borrow_escapes_move.kp",
        "use of moved value",
    );
}

/// Whether `ld.lld` is on PATH. A freestanding build's last step is a bare
/// `ld.lld` link, so these tests are for the `dev-kernel.nix` shell (which
/// carries `llvmPackages_21.lld` and the llvm tools); under the ordinary
/// `dev.nix` shell the tool is absent and the test prints a skip rather than
/// failing, which is what keeps the normal suite green.
fn have_ld_lld() -> bool {
    Command::new("ld.lld")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// One bare-metal target's emulator: the QEMU binary and the machine arguments
/// the target needs. Kernel stage 6 added the riscv64 one; kernel stage 10, the
/// second architecture, added aarch64, so the boot harness is parameterized
/// instead of naming one emulator.
#[derive(Clone, Copy)]
struct Emulator {
    binary: &'static str,
    args: &'static [&'static str],
}

/// QEMU's riscv64 `virt` machine, the one kernel stage 6 boots. `-bios none`
/// makes QEMU start the image from 0x80000000 with no firmware in front of it.
const RISCV64_QEMU: Emulator = Emulator {
    binary: "qemu-system-riscv64",
    args: &[
        "-machine",
        "virt",
        "-bios",
        "none",
        "-nographic",
        "-no-reboot",
        "-serial",
        "mon:stdio",
    ],
};

/// The same riscv64 machine with a second hart (kernel stage 8). `-smp 2`
/// makes QEMU start both harts at the entry with no firmware to park the
/// second, which is what `examples/kernel0-percpu` proves the compiler's
/// `_start` stub handles.
const RISCV64_QEMU_SMP2: Emulator = Emulator {
    binary: "qemu-system-riscv64",
    args: &[
        "-machine",
        "virt",
        "-bios",
        "none",
        "-smp",
        "2",
        "-nographic",
        "-no-reboot",
        "-serial",
        "mon:stdio",
    ],
};

/// QEMU's aarch64 `virt` machine, the one kernel stage 10 boots. `-cpu
/// cortex-a57` is the core its target machine is built for; QEMU loads the
/// `-kernel` ELF at 0x40000000 and starts it at the ELF entry, so no `-bios`
/// argument is needed.
const AARCH64_QEMU: Emulator = Emulator {
    binary: "qemu-system-aarch64",
    args: &[
        "-M",
        "virt",
        "-cpu",
        "cortex-a57",
        "-nographic",
        "-serial",
        "mon:stdio",
    ],
};

/// QEMU's x86-64 `q35` machine, the one kernel stage 20 boots. `-kernel` loads
/// an ELF64 image through the Xen PVH note and SeaBIOS enters the 32-bit entry
/// it names, so the target machine's CPU is the one `-cpu qemu64` is: no AVX,
/// no AVX-512. `-no-reboot` makes the long-mode example's triple fault exit
/// QEMU instead of rebooting into the boot loader again.
const X86_64_QEMU: Emulator = Emulator {
    binary: "qemu-system-x86_64",
    args: &[
        "-M",
        "q35",
        "-cpu",
        "qemu64",
        "-nographic",
        "-serial",
        "mon:stdio",
        "-no-reboot",
    ],
};

/// Whether `emu`'s QEMU binary is on PATH. Kernel stage 6's milestone is a
/// real boot, so its acceptance test needs the emulator as well as `ld.lld`;
/// like the freestanding tests, a shell without it prints a skip.
fn have_qemu(emu: &Emulator) -> bool {
    Command::new(emu.binary)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Kernel stage 6's acceptance harness: build a freestanding project and boot
/// the ELF under `emu` until the last `expected` line has come out of the UART,
/// then kill QEMU and check every expected line against what was captured. The
/// kernel halts in a wait loop, so nothing but reading the marker and killing
/// the emulator keeps the test from waiting out the whole window.
///
/// The expected lines are compared whole, and they are QEMU's own stdout, not
/// anything the project says it will print.
fn boot_under_qemu(emu: &Emulator, name: &str, target: &Path, expected: &[&str]) {
    let marker = expected.last().expect("at least one expected line");
    let captured = boot_and_capture(emu, name, target, &|got| {
        got.lines().any(|line| line == *marker)
    });
    for line in expected {
        assert!(
            captured.lines().any(|got| got == *line),
            "the serial output of '{name}' has no line '{line}':\n{captured}"
        );
    }
}

/// The same boot, for an assertion that is a shape rather than a whole line.
/// Kernel 0's panic handler prints the compiler's own message, which the test
/// cannot spell out in full, so the wait and the check are a set of substrings:
/// `index out of bounds` is in the text and some line starts with `panic: `.
fn boot_under_qemu_containing(
    emu: &Emulator,
    name: &str,
    target: &Path,
    needle: &str,
    line_prefix: &str,
) {
    let captured = boot_and_capture(emu, name, target, &|got| {
        got.contains(needle) && got.lines().any(|line| line.starts_with(line_prefix))
    });
    assert!(
        captured.contains(needle),
        "the serial output of '{name}' does not contain '{needle}':\n{captured}"
    );
    assert!(
        captured.lines().any(|line| line.starts_with(line_prefix)),
        "the serial output of '{name}' has no line starting with '{line_prefix}':\n{captured}"
    );
}

/// The same boot, for a run whose interesting output is a sequence rather than
/// a set of whole lines. Kernel stage 22's exception reports carry a `rip` the
/// linker chose, so each `expected` entry is a substring that has to appear
/// after the one before it; SeaBIOS' "Booting from ROM.." is glued to the
/// first kernel line, which is the other reason the match is not anchored.
fn boot_under_qemu_in_order(emu: &Emulator, name: &str, target: &Path, expected: &[&str]) {
    let marker = expected.last().expect("at least one expected line");
    let captured = boot_and_capture(emu, name, target, &|got| got.contains(marker));
    let mut from = 0usize;
    for want in expected {
        match captured[from..].find(want) {
            Some(at) => from += at + want.len(),
            None => panic!(
                "the serial output of '{name}' has no '{want}' after the earlier lines:\n{captured}"
            ),
        }
    }
}

/// Builds a freestanding project, boots it under `emu`, and reads the serial
/// output until `done` says the interesting part has arrived (or the deadline
/// fires), then kills the emulator and returns everything captured.
fn boot_and_capture(
    emu: &Emulator,
    name: &str,
    target: &Path,
    done: &dyn Fn(&str) -> bool,
) -> String {
    let bin = out_path(name);
    let build = Command::new(kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .env_remove("KELPC_ORACLE")
        .arg("build")
        .arg(target)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the freestanding build of '{name}' failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let mut child = Command::new(emu.binary)
        .args(emu.args)
        .arg("-kernel")
        .arg(&bin)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap_or_else(|e| panic!("failed to spawn {}: {e}", emu.binary));

    // A reader thread turns the pipe into a channel so the wait below has a
    // deadline: without one a kernel that never prints would hang the suite.
    let stdout = child.stdout.take().expect("qemu stdout was piped");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            match line {
                Ok(line) => {
                    if tx.send(line).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let deadline = Instant::now() + Duration::from_secs(20);
    let mut captured = String::new();
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(remaining) {
            Ok(line) => {
                captured.push_str(&line);
                captured.push('\n');
                if done(&captured) {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    captured
}

/// Runs an external tool and returns its stdout, failing the test with the
/// tool's stderr when it exits nonzero.
fn tool(name: &str, args: &[&str]) -> String {
    let out = Command::new(name)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("could not run '{name}': {e}"));
    assert!(
        out.status.success(),
        "'{name}' failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Builds one freestanding project with the LLVM backend and returns its object
/// file (the `.o` the driver wrote beside the ELF), so a test can read the
/// layout the object writer produced rather than the linked image.
fn build_freestanding_object(name: &str, project_dir: &str) -> PathBuf {
    let bin = out_path(name);
    let build = Command::new(kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .env_remove("KELPC_ORACLE")
        .arg("build")
        .arg(project(project_dir))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the freestanding build of '{name}' failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    bin.with_extension("o")
}

/// One symbol's size from `llvm-nm -S` output (`addr size type name`, all hex
/// except undefined symbols, which have no address).
fn symbol_size(nm: &str, name: &str) -> u64 {
    for line in nm.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() == 4 && fields[3] == name {
            return u64::from_str_radix(fields[1], 16)
                .unwrap_or_else(|_| panic!("unreadable size in '{line}'"));
        }
    }
    panic!("symbol '{name}' is not in llvm-nm's output:\n{nm}");
}

/// The size column of one `llvm-readelf -S` line. The columns end
/// `... ES Flg Lk Inf Al`, so the size is six fields from the end.
fn percpu_section_size(line: &str) -> u64 {
    let fields: Vec<&str> = line.split_whitespace().collect();
    u64::from_str_radix(fields[fields.len() - 6], 16)
        .unwrap_or_else(|_| panic!("unreadable size in '{line}'"))
}

/// The last (`Al`) column of one `llvm-readelf -S` line, in decimal.
fn percpu_section_alignment(line: &str) -> u64 {
    let fields: Vec<&str> = line.split_whitespace().collect();
    fields[fields.len() - 1]
        .parse()
        .unwrap_or_else(|_| panic!("unreadable alignment in '{line}'"))
}

/// Kernel stage 4: `section("...")` on a `const` and on a `fun` changes where
/// the linker may place them, never what the program does. Both backends run
/// the same program to the same output.
#[test]
fn a_section_clause_places_code_and_data_without_changing_the_result() {
    assert_compiles_and_runs("tests/fixtures/section_attribute.kp", "41\n");
}

/// `print` is `printf`, and a freestanding build has no libc: the front end
/// refuses the construct by name. The check runs before either backend is
/// named, so the C oracle refuses for the same reason and with the same
/// message.
#[test]
fn print_is_rejected_in_a_freestanding_project() {
    assert_rejected_with(
        "tests/projects/freestanding_print",
        "freestanding build",
    );
}

/// The manifest's `[target] entry` must name a `fun` that exists in the entry
/// module: a missing one is named rather than left as an ELF whose `_start`
/// calls an undefined symbol.
#[test]
fn a_missing_freestanding_entry_is_rejected() {
    assert_rejected_with(
        "tests/projects/freestanding_no_entry",
        "no entry function 'kmain' found",
    );
}

/// The C backend's runtime preamble is libc, so it cannot express a
/// freestanding project at all. A build asked for the oracle anyway refuses
/// with a message that says which mode and which backend.
#[test]
fn the_c_backend_refuses_a_freestanding_project() {
    let bin = out_path("freestanding_oracle_refusal");
    let build = kelpc()
        .env("KELPC_ORACLE", "c")
        .arg("build")
        .arg(project("freestanding_min"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        !build.status.success(),
        "the C backend should not build a freestanding project"
    );
    let stderr = String::from_utf8_lossy(&build.stderr);
    assert!(stderr.contains("freestanding"), "{stderr}");
    assert!(stderr.contains("llvm"), "{stderr}");
}

/// `Ptr.alloc` and `Ptr.free` are the hosted `malloc`/`free`, so they are
/// refused even though `Ptr<T>` itself is allowed.
#[test]
fn ptr_alloc_is_rejected_in_a_freestanding_project() {
    assert_rejected_with(
        "tests/projects/freestanding_ptr_alloc",
        "'Ptr#alloc' is unavailable in a freestanding build",
    );
}

/// The host entry point has no runtime to call it in a freestanding image, so
/// a defined `main` is refused and the manifest's entry is the entry point.
#[test]
fn main_is_rejected_in_a_freestanding_project() {
    assert_rejected_with("tests/projects/freestanding_main", "'main' is unavailable");
}

/// The acceptance test for kernel stage 4: the `freestanding_min` project,
/// whose entry loops forever, builds through LLVM and `ld.lld` into an ELF64
/// RISC-V image based at 0x80000000 that references no undefined symbol, which
/// is to say no libc symbol and no `kelp_rt` helper.
///
/// It needs `ld.lld`, `llvm-readelf`, `llvm-objdump` and `llvm-nm`, all from
/// `dev-kernel.nix`; a shell without them prints a skip.
#[test]
fn freestanding_min_builds_a_riscv_elf_with_no_undefined_symbols() {
    if !have_ld_lld() {
        println!(
            "skipping the freestanding acceptance test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = out_path("freestanding_min");
    let build = Command::new(kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .env_remove("KELPC_ORACLE")
        .arg("build")
        .arg(project("freestanding_min"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the freestanding build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let path = bin.to_str().expect("the artifact path is utf-8");
    let header = tool("llvm-readelf", &["-h", path]);
    assert!(header.contains("ELF64"), "not an ELF64 image:\n{header}");
    assert!(header.contains("RISC-V"), "not a RISC-V image:\n{header}");
    let entry_line = header
        .lines()
        .find(|line| line.contains("Entry point address"))
        .unwrap_or_default();
    assert!(
        entry_line.contains("0x80000000"),
        "the entry point is not the linker script's 0x80000000:\n{header}"
    );

    // `llvm-objdump` is the disassembler half of the acceptance check: the
    // generated `_start` has to be real RISC-V code, not an empty section.
    let disassembly = tool("llvm-objdump", &["-d", path]);
    assert!(disassembly.contains("_start"), "no _start in the image:\n{disassembly}");
    assert!(disassembly.contains("wfi"), "the _start halt is missing:\n{disassembly}");

    // No symbol the linker had to leave for someone else: no `printf`, no
    // `malloc`, no `kelp_panic`.
    let symbols = tool("llvm-nm", &[path]);
    let undefined: Vec<&str> = symbols
        .lines()
        .filter(|line| line.split_whitespace().next() == Some("U"))
        .collect();
    assert!(
        undefined.is_empty(),
        "the freestanding image has undefined symbols:\n{}",
        undefined.join("\n")
    );
}

/// `plan/iterators.md`'s freestanding half: a kernel-style loop over
/// `iter.range` lowers to an ordinary `while` over `Range.next`, so the image
/// links with no undefined symbol. This is the same acceptance test as
/// `freestanding_min`, run on the project that walks a range, and it needs the
/// same `dev-kernel.nix` tools.
#[test]
fn a_freestanding_project_can_walk_a_range() {
    if !have_ld_lld() {
        println!(
            "skipping the freestanding iterator test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = out_path("freestanding_iter");
    let build = Command::new(kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .env_remove("KELPC_ORACLE")
        .arg("build")
        .arg(project("freestanding_iter"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the freestanding iterator build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let path = bin.to_str().expect("the artifact path is utf-8");
    let header = tool("llvm-readelf", &["-h", path]);
    assert!(header.contains("ELF64"), "not an ELF64 image:\n{header}");
    assert!(header.contains("RISC-V"), "not a RISC-V image:\n{header}");

    let symbols = tool("llvm-nm", &[path]);
    let undefined: Vec<&str> = symbols
        .lines()
        .filter(|line| line.split_whitespace().next() == Some("U"))
        .collect();
    assert!(
        undefined.is_empty(),
        "the freestanding iterator image has undefined symbols:\n{}",
        undefined.join("\n")
    );
}

/// Language-completeness 5's freestanding half: a `datac` with `Add` and `Ord`
/// from `std/ops`, used by `+` and `<`. The traits are pure language, so the
/// image links with no undefined symbol, exactly as the iterator one does.
/// Needs `dev-kernel.nix` tools.
#[test]
fn a_freestanding_project_can_use_the_operator_traits() {
    if !have_ld_lld() {
        println!(
            "skipping the freestanding operator-trait test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = out_path("freestanding_ops");
    let build = Command::new(kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .env_remove("KELPC_ORACLE")
        .arg("build")
        .arg(project("freestanding_ops"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the freestanding operator-trait build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let path = bin.to_str().expect("the artifact path is utf-8");
    let header = tool("llvm-readelf", &["-h", path]);
    assert!(header.contains("ELF64"), "not an ELF64 image:\n{header}");
    assert!(header.contains("RISC-V"), "not a RISC-V image:\n{header}");

    let symbols = tool("llvm-nm", &[path]);
    let undefined: Vec<&str> = symbols
        .lines()
        .filter(|line| line.split_whitespace().next() == Some("U"))
        .collect();
    assert!(
        undefined.is_empty(),
        "the freestanding operator-trait image has undefined symbols:\n{}",
        undefined.join("\n")
    );
}

// ---------------------------------------------------------------------------
// Kernel stage 5 (MK9-S7-M1/M3, roadmap B-9/B-16): inline `asm` and `naked
// fun`. The templates in these fixtures are x86-64 assembly in Intel syntax,
// which is the dialect the compiler selects for an x86 target on both backends
// (`plan/inline-asm.md`), so the fixtures run on both the default LLVM backend
// and the C oracle.
// ---------------------------------------------------------------------------

/// `inout` is read then written, and `in` is read: `add {0}, {1}` accumulates
/// the input into the inout place and the result is printed.
#[test]
fn inline_asm_inout_and_in_operands_compute_a_sum() {
    assert_project_runs("asm_inout", &project("asm_inout"), "42\n");
}

/// An `out` operand writes its place, and the anonymous form `out("r") _` is
/// the asm expression's own value.
#[test]
fn inline_asm_out_operand_writes_its_place_and_the_anonymous_form_is_a_value() {
    assert_project_runs("asm_out", &project("asm_out"), "1234\n7\n");
}

/// `clobber(...)` adds a register to the constraint string, which is what tells
/// the compiler not to keep a value live in one. The program's own result is
/// unchanged, so the value of the fixture is that the clobber builds and runs.
#[test]
fn inline_asm_clobber_builds_and_runs() {
    assert_project_runs("asm_clobber", &project("asm_clobber"), "4\n");
}

/// A `naked fun` is called like any other function: the caller follows the
/// System V return-register convention and the asm is what puts 42 in `rax`.
#[test]
fn a_naked_function_returns_a_constant_in_rax_and_is_called_normally() {
    assert_project_runs("asm_naked", &project("asm_naked"), "42\n");
}

/// `asm` is an unsafe operation, so it needs an `unsafe` context on top of the
/// module's `[unsafe]` listing.
#[test]
fn an_asm_outside_unsafe_is_rejected() {
    assert_project_rejected(
        "asm_outside_unsafe",
        &project("asm_outside_unsafe"),
        "'asm' is an unsafe operation",
    );
}

/// The operand kinds are `in`, `out`, `inout` and `clobber`; anything else is
/// named in the refusal. A parser-level rejection, so it is a single negative
/// fixture rather than a project.
#[test]
fn an_unknown_asm_operand_kind_is_rejected() {
    assert_rejected_with(
        "tests/fixtures/asm_unknown_operand.kp",
        "unknown asm operand kind 'sideways'",
    );
}

/// A `naked fun` body is exactly one `asm`: nothing else can be emitted around
/// it.
#[test]
fn a_naked_function_with_a_second_statement_is_rejected() {
    assert_project_rejected(
        "asm_naked_two_stmts",
        &project("asm_naked_two_stmts"),
        "a 'naked fun' body must be exactly one 'asm(...)' statement",
    );
}

/// `asm` is what puts a module in the audit list, exactly as an `unsafe` block
/// does: a module that writes one without the listing is refused at the `asm`.
#[test]
fn an_asm_in_a_module_without_the_manifest_listing_is_rejected() {
    assert_project_rejected(
        "asm_unlisted",
        &project("asm_unlisted"),
        "module 'main' is not listed in coral.toml [unsafe] modules",
    );
}

/// The riscv64 half of the stage: `csrr` and `wfi` have no Kelp spelling, so
/// the only way to write them is `asm`, and this is the object-only check that
/// the templates assemble. It needs `ld.lld` and `llvm-objdump` from
/// `dev-kernel.nix`; a shell without them prints a skip.
///
/// `csrr` is the decisive disassembly line: the generated `_start` stub already
/// contains a `wfi`, so only `csrr` could not have come from anywhere but this
/// fixture's asm.
#[test]
fn inline_asm_assembles_csrr_and_wfi_for_riscv64() {
    if !have_ld_lld() {
        println!(
            "skipping the riscv64 asm test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = out_path("asm_riscv");
    let build = Command::new(kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .env_remove("KELPC_ORACLE")
        .arg("build")
        .arg(project("asm_riscv"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the riscv64 asm build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let path = bin.to_str().expect("the artifact path is utf-8");
    let header = tool("llvm-readelf", &["-h", path]);
    assert!(header.contains("RISC-V"), "not a RISC-V image:\n{header}");
    let disassembly = tool("llvm-objdump", &["-d", path]);
    assert!(
        disassembly.contains("csrr"),
        "the asm's csrr is not in the image:\n{disassembly}"
    );
    assert!(
        disassembly.contains("wfi"),
        "the asm's wfi is not in the image:\n{disassembly}"
    );
}

/// Kernel stage 6's acceptance test: `examples/kernel0` is a complete Kelp
/// kernel that boots on QEMU's riscv64 `virt` machine and writes one line to
/// the NS16550 UART. The assertion is against what QEMU captured, so it proves
/// the boot, the linker script's stack and the driver's polling, not that the
/// source says `hello from kelp`.
///
/// It needs `ld.lld` and `qemu-system-riscv64` from `dev-kernel.nix`; a shell
/// without them prints a skip.
#[test]
fn kernel0_boots_and_prints_its_greeting_over_the_uart() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU) {
        println!(
            "skipping the kernel0 boot test: 'ld.lld' or 'qemu-system-riscv64' is not on \
             PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    boot_under_qemu(
        &RISCV64_QEMU,
        "kernel0",
        &fixture("../examples/kernel0"),
        &["hello from kelp"],
    );
}

/// The trap half of kernel stage 6: `mtvec` points at a naked function, an
/// `ecall` reaches it, the handler prints and `mret`s back, and `kmain`
/// finishes. Both lines have to come out of the UART in one boot.
#[test]
fn kernel0_handles_a_trap_and_continues_after_it() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU) {
        println!(
            "skipping the kernel0 trap test: 'ld.lld' or 'qemu-system-riscv64' is not on \
             PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    boot_under_qemu(
        &RISCV64_QEMU,
        "kernel0_trap",
        &fixture("../examples/kernel0-trap"),
        &["trap: ecall", "done"],
    );
}

/// Kernel stage 7's panic half: `examples/kernel0-panic` reads one element past
/// an array's end on purpose, so the compiler's own bounds check calls the
/// project's `[target] panic` handler with the static message the check was
/// written with. The handler prints `panic: ` and the message, which is only
/// possible now that a freestanding program has `str` operations to walk.
///
/// The assertion is the shape of the line rather than a whole line: the text is
/// the compiler's, and the test should not have to restate it verbatim to know
/// that the check fired.
#[test]
fn kernel0_panic_prints_the_checks_own_message_over_the_uart() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU) {
        println!(
            "skipping the kernel0 panic test: 'ld.lld' or 'qemu-system-riscv64' is not on \
             PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    boot_under_qemu_containing(
        &RISCV64_QEMU,
        "kernel0_panic",
        &fixture("../examples/kernel0-panic"),
        "index out of bounds",
        "panic: ",
    );
}

/// Kernel stage 15: both accepted panic-handler shapes pass the front end, and
/// the three-argument one is the example's. `kelpc check` is the front end
/// alone, so this needs no `ld.lld` and runs in the ordinary suite.
#[test]
fn a_freestanding_panic_handler_may_take_a_location() {
    for name in ["kernel0-panic", "kernel0-panic2"] {
        let target = fixture(&format!("../examples/{name}"));
        let check = Command::new(kelpc_bin())
            .env_remove("KELPC_BACKEND")
            .env_remove("KELPC_ORACLE")
            .arg("check")
            .arg(&target)
            .output()
            .expect("failed to spawn kelpc");
        assert!(
            check.status.success(),
            "'kelpc check' refused examples/{name}, whose panic handler is an accepted shape:\n{}",
            String::from_utf8_lossy(&check.stderr)
        );
    }
}

/// Kernel stage 15: a handler whose shape is neither `fun(msg: str)` nor
/// `fun(msg: str, file: str, line: u32)` is a compile error naming both. Two
/// projects cover the wrong arity and the wrong parameter type.
#[test]
fn a_freestanding_panic_handler_with_another_shape_is_rejected() {
    let needle = "must be a 'fun(msg: str)' or a 'fun(msg: str, file: str, line: u32)'";
    assert_project_rejected(
        "freestanding_panic_bad_arity",
        &project("freestanding_panic_bad_arity"),
        needle,
    );
    assert_project_rejected(
        "freestanding_panic_bad_type",
        &project("freestanding_panic_bad_type"),
        needle,
    );
}

/// Kernel stage 15's acceptance boot: `examples/kernel0-panic2` prints the
/// message, file and 1-based line of every check it exercises, and answers a
/// panic raised inside its own handler with the project's `kelp_double_panic`
/// rather than the compiler's weak wait-for-interrupt fallback. The example's
/// `run.sh` boots one phase per check and asserts the exact `panic: <message>
/// at <file>:<line>` line; this runs that script, which is where the serial
/// assertions live.
#[test]
fn kernel0_panic2_names_each_checks_message_and_location() {
    if !have_ld_lld() || !have_llvm_mc() || !have_qemu(&RISCV64_QEMU) {
        println!(
            "skipping the kernel0-panic2 boot test: 'ld.lld', 'llvm-mc' or \
             'qemu-system-riscv64' is not on PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let script = fixture("../examples/kernel0-panic2/run.sh");
    let run = Command::new("sh")
        .arg(&script)
        .env("KELPC", kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .env_remove("KELPC_ORACLE")
        .output()
        .expect("failed to spawn the kernel0-panic2 run script");
    assert!(
        run.status.success(),
        "examples/kernel0-panic2/run.sh failed:\n{}\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(
        stdout.contains("all six phases printed the lines the compiler named"),
        "the run script did not reach its final line:\n{stdout}"
    );
}

/// Kernel stage 15: the compiler always defines `kelp_double_panic`, and
/// defines it **weak**, so an image links with no undefined symbol and a
/// project's own strong definition wins. `kelp_panic_entry` is the internal
/// entry every check calls.
#[test]
fn a_freestanding_double_panic_is_a_weak_defined_symbol() {
    if !have_ld_lld() {
        println!(
            "skipping the double-panic symbol test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = build_freestanding(
        "kernel0_panic2_symbols",
        &fixture("../examples/kernel0-panic2"),
    );
    let path = bin.to_str().expect("the artifact path is utf-8");
    let symbols = tool("llvm-nm", &[path]);
    let double = symbols
        .lines()
        .find(|line| line.split_whitespace().last() == Some("kelp_double_panic"))
        .unwrap_or_else(|| panic!("the image has no kelp_double_panic:\n{symbols}"));
    assert_eq!(
        double.split_whitespace().nth(1),
        Some("W"),
        "kelp_double_panic is not weak, so a project cannot override it: {double}"
    );
    assert!(
        symbols
            .lines()
            .any(|line| line.split_whitespace().last() == Some("kelp_panic_entry")),
        "the image has no kelp_panic_entry:\n{symbols}"
    );
    assert!(
        tool("llvm-nm", &["-u", path]).trim().is_empty(),
        "the double-panic image has undefined symbols"
    );
}

/// Kernel stage 15's hosted half: the entry that carries a location is a
/// freestanding emission and changes nothing about a hosted panic. The runtime
/// still writes exactly `panic: <msg>` and exits non-zero, on both backends.
#[test]
fn a_hosted_panic_is_still_exactly_its_message() {
    let bin = out_path("hosted_panic_unchanged");
    let build = kelpc()
        .arg("build")
        .arg(fixture("tests/fixtures/panic_msg.kp"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the hosted panic fixture did not build:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&bin).output().expect("failed to run the binary");
    assert_eq!(run.status.code(), Some(1), "a hosted panic exits 1");
    assert_eq!(
        String::from_utf8_lossy(&run.stderr),
        "panic: deliberate failure\n",
        "the hosted panic format changed"
    );
}

/// Kernel stage 7's hosted half: the `str` operations a freestanding build
/// lowers itself (`len`, `byte`, `eq`, `starts_with` and `ptr`) behave exactly
/// as they did before the stage, on both backends. The suite runs this under
/// the default backend and under `KELPC_ORACLE=c`, so the two answers are
/// compared against one literal.
#[test]
fn the_hosted_str_core_operations_are_unchanged() {
    assert_compiles_and_runs(
        "tests/fixtures/str_core.kp",
        "0\n4\n107\n112\ntrue\nfalse\ntrue\ntrue\ntrue\nfalse\nfalse\n",
    );
}

/// Kernel stage 7's rejection half: `std/str` is only partly on the freestanding
/// allowed set, so its other functions are refused at the call, by name. The
/// check runs before either backend is named, so the C oracle refuses with the
/// same message `kelpc build` gives.
#[test]
fn a_std_str_function_outside_the_freestanding_five_is_refused() {
    assert_rejected_with(
        "tests/projects/freestanding_str_refused",
        "'std.str.char_count' is unavailable in a freestanding build",
    );
}

/// Kernel stage 7's acceptance test: the five `str` operations, called from a
/// freestanding entry, compile through LLVM and `ld.lld` into an image whose
/// symbols all resolve. The point of the test is the `llvm-nm` line: the
/// obvious lowering of a NUL scan is a loop LLVM's own optimizer rewrites into
/// a `strlen` call, and the module's allocating bodies would leave `malloc` and
/// friends behind, so an image that links at all is the stage's whole claim.
#[test]
fn freestanding_str_builds_the_five_operations_with_no_undefined_symbols() {
    if !have_ld_lld() {
        println!(
            "skipping the freestanding str test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = out_path("freestanding_str");
    let build = Command::new(kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .env_remove("KELPC_ORACLE")
        .arg("build")
        .arg(project("freestanding_str"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the freestanding str build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let path = bin.to_str().expect("the artifact path is utf-8");
    let symbols = tool("llvm-nm", &[path]);
    let undefined: Vec<&str> = symbols
        .lines()
        .filter(|line| line.split_whitespace().next() == Some("U"))
        .collect();
    assert!(
        undefined.is_empty(),
        "the freestanding str image has undefined symbols:\n{}",
        undefined.join("\n")
    );
    assert!(
        !symbols.contains("strlen"),
        "the NUL scan was rewritten into a libc strlen call:\n{symbols}"
    );
}

// ---------------------------------------------------------------------------
// Kernel stage 19 (release goal 3): the freestanding standard subset. Each
// admitted module has a project here that imports it and calls a
// representative function; every one is built for riscv64 and checked for zero
// undefined symbols, which is what "admitted" means. `plan/freestanding-std.md`
// is the table and how it was derived; `plan/freestanding.md` owns the mode.
// ---------------------------------------------------------------------------

/// The admitted set: one project per module the table admits, plus the boot.
/// Every one must link with no undefined symbol, so a module that reaches libc
/// or the allocator cannot be on the list by accident.
#[test]
fn every_admitted_std_module_builds_with_no_undefined_symbols() {
    if !have_ld_lld() {
        println!(
            "skipping the freestanding std tests: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    for dir in [
        "freestanding_std_option",
        "freestanding_std_result",
        "freestanding_std_atomic",
        "freestanding_std_arena",
        "freestanding_std_irq",
        "freestanding_std_frame",
        "freestanding_std_str",
        "freestanding_std_math",
        "freestanding_std_alloc",
        "freestanding_std_regex",
        "freestanding_std_kernel",
        "freestanding_view",
    ] {
        let bin = out_path(dir);
        let build = Command::new(kelpc_bin())
            .env_remove("KELPC_BACKEND")
            .env_remove("KELPC_ORACLE")
            .arg("build")
            .arg(project(dir))
            .arg("-o")
            .arg(&bin)
            .output()
            .expect("failed to spawn kelpc");
        assert!(
            build.status.success(),
            "the freestanding build of '{dir}' failed:\n{}",
            String::from_utf8_lossy(&build.stderr)
        );
        let path = bin.to_str().expect("the artifact path is utf-8");
        let undefined = tool("llvm-nm", &["-u", path]);
        assert!(
            undefined.trim().is_empty(),
            "'{dir}' links with undefined symbols:\n{undefined}"
        );
    }
}

/// Kernel stage 19's boot: `std/option`, `std/result` and `std/arena` in one
/// riscv64 image, printed over the real UART. `std/bits` is not shipped on this
/// branch, so those are the three the boot names.
#[test]
fn freestanding_std_kernel_boots_with_option_result_and_arena() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU) {
        println!(
            "skipping the freestanding std boot: 'ld.lld' or 'qemu-system-riscv64' \
             is not on PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    boot_under_qemu(
        &RISCV64_QEMU,
        "freestanding_std_kernel",
        &project("freestanding_std_kernel"),
        &[
            "some 1",
            "none 1",
            "unwrapped 14",
            "ok 1",
            "err 1",
            "fallback 9",
            "arena aligned 1",
            "arena byte 66",
            "arena used 4",
            "done",
        ],
    );
}

/// A whole std module that needs a hosted runtime is refused by name, and the
/// refusal lists the modules a freestanding project may implement.
#[test]
fn a_hosted_std_module_is_refused_by_name_with_the_allowed_list() {
    let allowed = "std/arena, std/atomic, std/convert, std/frame, std/irq, std/iter, std/list, \
                   std/math, std/ops, std/option, std/regex, std/result, std/str";
    for (dir, module) in [
        ("freestanding_std_refused_time", "std/time"),
        ("freestanding_std_refused_thread", "std/thread"),
        ("freestanding_std_refused_env", "std/env"),
        ("freestanding_std_refused_tz", "std/tz"),
    ] {
        let bin = out_path(dir);
        let build = Command::new(kelpc_bin())
            .env_remove("KELPC_BACKEND")
            .env_remove("KELPC_ORACLE")
            .arg("build")
            .arg(project(dir))
            .arg("-o")
            .arg(&bin)
            .output()
            .expect("failed to spawn kelpc");
        assert!(
            !build.status.success(),
            "expected '{dir}' to be rejected in a freestanding build"
        );
        let stderr = String::from_utf8_lossy(&build.stderr);
        for needle in [
            format!("the module '{module}' is unavailable in a freestanding build"),
            "needs a hosted runtime".to_string(),
            allowed.to_string(),
        ] {
            assert!(
                stderr.contains(&needle),
                "the refusal of '{dir}' does not mention '{needle}':\n{stderr}"
            );
        }
    }
}

/// `std/math` is admitted but its libm externs are not: a call to one is
/// refused by name inside the admitted module, rather than left for `ld.lld`.
#[test]
fn a_std_math_function_outside_the_integer_subset_is_refused() {
    assert_rejected_with(
        "tests/projects/freestanding_std_math_refused",
        "'std.math.sqrt' is unavailable in a freestanding build",
    );
    assert_rejected_with(
        "tests/projects/freestanding_std_math_refused",
        "needs the hosted runtime",
    );
}

/// A `std/option` callback combinator is admitted only with the allocator pair:
/// its closure environment is a heap box, and without one the call is refused by
/// name rather than left as an undefined `free`.
#[test]
fn a_std_option_callback_without_an_allocator_is_refused() {
    assert_rejected_with(
        "tests/projects/freestanding_std_option_no_alloc",
        "'std.option.map<i32, i32>' is unavailable in a freestanding build",
    );
    assert_rejected_with(
        "tests/projects/freestanding_std_option_no_alloc",
        "needs an allocator",
    );
}

// ---------------------------------------------------------------------------
// Kernel stage 7: global `static` data. A `static` is one object with a stable
// address, `static mut` is the audited mutable form, and an `Atomic<T>` static
// is the safe shared mutable. Every run test here passes on the default LLVM
// backend and on the C oracle, because the suite is run twice; the freestanding
// and kernel tests are LLVM-only by design and skip in a shell without the
// tools.
// ---------------------------------------------------------------------------

/// A `static mut` counter bumped in `unsafe` from more than one function, read
/// back, addressed with `Ptr.from_mut` and written through, beside an immutable
/// lookup table and a `datac` static read by field.
#[test]
fn static_mut_counter_and_a_lookup_table_run() {
    assert_project_runs(
        "static_globals",
        &project("static_globals"),
        "5\n50\n100\n1\n2\n100\n4\n8\n",
    );
}

/// Kernel stage 21: an array static's element may be a `datac`, and a nested
/// array is the same rule one level down. This is the shape a scheduler's table
/// of tasks needs (a saved frame and a stack per task), and it runs on both
/// backends because the project is an ordinary hosted program.
#[test]
fn a_static_can_hold_an_array_of_datac_values() {
    assert_project_runs(
        "static_datac_array",
        &project("static_datac_array"),
        "45\n2\n3\n9\n1\n",
    );
}

/// An `Atomic<i64>` static incremented from two `thread.spawn`ed workers and
/// read back after both joins: exactly 1000, with no `unsafe`, because an
/// atomic mutates through a shared borrow.
#[test]
fn a_static_atomic_counter_sums_across_two_threads() {
    assert_project_runs("static_atomic_threads", &project("static_atomic_threads"), "1000\n");
}

/// A `static mut` read outside `unsafe` is refused, the same way a `Ptr.read`
/// is: the gate names the object and says what would make it legal.
#[test]
fn a_static_mut_read_outside_unsafe_is_rejected() {
    assert_rejected_with(
        "tests/fixtures/static_mut_outside_unsafe.kp",
        "'static mut C' is an unsafe operation",
    );
}

/// A static's initializer must be a constant expression.
#[test]
fn a_non_constant_static_initializer_is_rejected() {
    assert_rejected_with(
        "tests/fixtures/static_not_constant.kp",
        "the initializer is not constant",
    );
}

/// A heap type cannot be a static's type.
#[test]
fn a_heap_type_static_is_rejected() {
    assert_rejected_with(
        "tests/fixtures/static_heap_type.kp",
        "'String' cannot be a static's type",
    );
}

/// Assigning to an immutable static is refused.
#[test]
fn assigning_to_an_immutable_static_is_rejected() {
    assert_rejected_with(
        "tests/fixtures/static_assign_immutable.kp",
        "cannot assign to 'static X': it is not declared 'static mut'",
    );
}

/// The freestanding placement test: a nonzero immutable table lands in
/// `.rodata`, a nonzero `static mut` in `.data`, and the zero cursor and the
/// 4096-byte zero buffer in `.bss`. The `NOBITS` `.bss` is what proves the big
/// zero array does not grow the image, which is the whole reason a static goes
/// there and a folded `const` cannot.
///
/// It needs `ld.lld`, `llvm-readelf` and `llvm-size` from `dev-kernel.nix`.
#[test]
fn freestanding_statics_land_in_data_rodata_and_bss() {
    if !have_ld_lld() {
        println!(
            "skipping the freestanding statics test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = out_path("freestanding_statics");
    let build = Command::new(kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .env_remove("KELPC_ORACLE")
        .arg("build")
        .arg(project("freestanding_statics"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the freestanding statics build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let path = bin.to_str().expect("the artifact path is utf-8");
    let sections = tool("llvm-readelf", &["-S", path]);
    let bss = sections
        .lines()
        .find(|line| line.contains("] .bss"))
        .unwrap_or_else(|| panic!("no .bss section:\n{sections}"));
    assert!(
        bss.contains("NOBITS"),
        "the zero statics should be in a NOBITS .bss:\n{bss}"
    );
    assert!(
        sections.lines().any(|line| line.contains("] .data") && line.contains("PROGBITS")),
        "the nonzero static mut should be in a PROGBITS .data:\n{sections}"
    );
    assert!(
        sections.lines().any(|line| line.contains("] .rodata") && line.contains("PROGBITS")),
        "the immutable table should be in a PROGBITS .rodata:\n{sections}"
    );

    let sizes = tool("llvm-size", &[path]);
    let total = sizes
        .lines()
        .find(|line| line.contains("fs.elf") || line.contains("freestanding_statics"))
        .or_else(|| sizes.lines().last())
        .unwrap_or_default();
    let numbers: Vec<u64> = total
        .split_whitespace()
        .take(4)
        .filter_map(|field| field.parse().ok())
        .collect();
    assert!(
        numbers.len() >= 3,
        "could not read llvm-size's text/data/bss columns:\n{sizes}"
    );
    assert_eq!(
        numbers[1], 8,
        "the nonzero static mut should be 8 bytes of .data:\n{sizes}"
    );
    assert!(
        numbers[2] >= 4096,
        "the zero statics should be at least 4096 bytes of .bss:\n{sizes}"
    );
}

/// Kernel stage 7's acceptance test: `examples/kernel0-statics` bumps a
/// `static mut` counter three times in `unsafe` and prints `counter 3` through
/// the NS16550 UART, then halts. The counter starts in `.bss`, which QEMU's ELF
/// loader zeroes, so a nonzero line proves the static round trip.
///
/// It needs `ld.lld` and `qemu-system-riscv64` from `dev-kernel.nix`.
#[test]
fn kernel0_statics_boots_and_prints_its_counter() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU) {
        println!(
            "skipping the kernel0-statics boot test: 'ld.lld' or 'qemu-system-riscv64' is not \
             on PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    boot_under_qemu(
        &RISCV64_QEMU,
        "kernel0_statics",
        &fixture("../examples/kernel0-statics"),
        &["counter 3"],
    );
}

/// Kernel stage 8 (MK9-S3-M3): `percpu` is a freestanding-only storage class,
/// so a hosted project that declares one is refused by the front end, before
/// either backend is named, which is why the oracle refuses it the same way.
#[test]
fn a_percpu_static_is_rejected_outside_freestanding() {
    assert_rejected_with(
        "tests/projects/percpu_hosted",
        "only legal in a freestanding build",
    );
}

/// A `percpu` static has to be `mut`: a per-CPU value nothing may write is an
/// ordinary `static` with extra address space.
#[test]
fn a_percpu_static_without_mut_is_rejected() {
    assert_rejected_with("tests/projects/percpu_no_mut", "must be 'mut'");
}

/// A `percpu` static is a `static mut` in every way, so naming it outside an
/// `unsafe` block is the ordinary `static mut` error.
#[test]
fn a_percpu_static_named_outside_unsafe_is_rejected() {
    assert_rejected_with("tests/projects/percpu_outside_unsafe", "'static mut SLOT'");
}

/// Its initializer is folded to a constant and replicated into every copy, so
/// anything that would need code at startup is refused.
#[test]
fn a_percpu_static_with_a_non_constant_initializer_is_rejected() {
    assert_rejected_with("tests/projects/percpu_initializer", "not constant");
}

/// `[target] cpus` is an integer in `1..=64`; both ends are refused by the
/// manifest reader naming the key.
#[test]
fn a_cpu_count_outside_one_to_sixty_four_is_rejected() {
    assert_rejected_with("tests/projects/percpu_cpus_zero", "'[target] cpus'");
    assert_rejected_with("tests/projects/percpu_cpus_65", "'[target] cpus'");
}

/// Kernel stage 8's object-layout test: a `percpu` static is `cpus` copies of
/// its type, each padded to a 64-byte cache line, in a 64-byte-aligned
/// `.percpu`. At `cpus = 1` a `usize` is one 64-byte copy.
///
/// It needs `ld.lld`, `llvm-nm` and `llvm-readelf` from `dev-kernel.nix`.
#[test]
fn freestanding_percpu_static_is_one_cache_line_at_cpus_1() {
    if !have_ld_lld() {
        println!(
            "skipping the freestanding per-CPU layout test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let obj = build_freestanding_object("freestanding_percpu_1", "freestanding_percpu_1");
    let path = obj.to_str().expect("the artifact path is utf-8");
    assert_eq!(
        symbol_size(&tool("llvm-nm", &["-S", path]), "kelp_SLOT"),
        64,
        "one usize copy should be padded to a 64-byte cache line"
    );
    let sections = tool("llvm-readelf", &["-S", path]);
    let percpu: Vec<&str> = sections
        .lines()
        .filter(|line| line.contains("] .percpu"))
        .collect();
    assert_eq!(percpu.len(), 1, "expected one .percpu section:\n{sections}");
    assert_eq!(
        percpu_section_size(percpu[0]),
        64,
        "the .percpu section should be one 64-byte copy:\n{}",
        percpu[0]
    );
    assert_eq!(
        percpu_section_alignment(percpu[0]),
        64,
        "the .percpu section should be 64-byte aligned:\n{}",
        percpu[0]
    );
}

/// The same at `cpus = 4`, with one copy whose size fills a cache line exactly
/// (`usize`, 8 bytes padded to 64) and one rounded up to the next multiple
/// (`[u8; 100]`, 100 bytes padded to 128), so the size column checks both.
#[test]
fn freestanding_percpu_statics_are_four_aligned_copies_at_cpus_4() {
    if !have_ld_lld() {
        println!(
            "skipping the freestanding per-CPU layout test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let obj = build_freestanding_object("freestanding_percpu_4", "freestanding_percpu_4");
    let path = obj.to_str().expect("the artifact path is utf-8");
    let syms = tool("llvm-nm", &["-S", path]);
    assert_eq!(
        symbol_size(&syms, "kelp_SLOT"),
        4 * 64,
        "four usize copies at 64 bytes each:\n{syms}"
    );
    assert_eq!(
        symbol_size(&syms, "kelp_TABLE"),
        4 * 128,
        "four [u8; 100] copies rounded up to 128 bytes each:\n{syms}"
    );
    let sections = tool("llvm-readelf", &["-S", path]);
    let percpu: Vec<&str> = sections
        .lines()
        .filter(|line| line.contains("] .percpu"))
        .collect();
    assert!(!percpu.is_empty(), "no .percpu section:\n{sections}");
    for line in &percpu {
        assert_eq!(
            percpu_section_alignment(line),
            64,
            "every .percpu section should be 64-byte aligned:\n{line}"
        );
    }
}

/// Kernel stage 8's acceptance boot: `examples/kernel0-percpu` runs on a
/// two-hart riscv64 `virt` with no firmware, so the compiler's `_start` stub has
/// to park the second hart before it touches memory. Hart 0 writes 4 into its
/// own copy of the `percpu` static and prints it, then sets `tp = 1` by hand and
/// reads the other copy, which still holds the initial 7; `cpu_index()` and
/// `cpu_count()` print 1 and 2.
///
/// It needs `ld.lld` and `qemu-system-riscv64` from `dev-kernel.nix`.
#[test]
fn kernel0_percpu_boots_with_two_harts_and_shows_the_other_copy() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU_SMP2) {
        println!(
            "skipping the kernel0-percpu boot test: 'ld.lld' or 'qemu-system-riscv64' is not \
             on PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    boot_under_qemu(
        &RISCV64_QEMU_SMP2,
        "kernel0_percpu",
        &fixture("../examples/kernel0-percpu"),
        &["cpu0 4", "cpu1 7", "idx 1", "count 2"],
    );
}

// ---------------------------------------------------------------------------
// Kernel stage 18 (MK10-S1's second half): IRQ lines and physical frames as
// capabilities. The hosted fixture below is a value test both backends run;
// the boot is freestanding riscv64 and skips in a shell without `ld.lld` and
// `qemu-system-riscv64`. The front-end refusals live in `tests/irq_frames.rs`.
// ---------------------------------------------------------------------------

/// The two capability values as plain hosted values: with no `[caps]` section
/// the entry holds the whole resource, so a constant claim is admitted and the
/// value it carries is the number the compiler checked. The C oracle computes
/// the same literal.
#[test]
fn an_irq_line_and_a_frame_are_values_a_hosted_program_can_claim() {
    assert_compiles_and_runs(
        "tests/fixtures/irq_frame_claim.kp",
        "10\n2148532224\n2148532240\n2148536319\n",
    );
}

/// Kernel stage 18's acceptance boot: `examples/kernel0-caps` holds exactly one
/// interrupt line and one physical frame in its `[caps]` section, claims both
/// with the gated constructors, and a driver that holds the same grants prints
/// `line 10`, `frame 0x80100000` and `offset 0x80100010` over the NS16550. The
/// ungranted module in the same `src/` is a compile-error fixture the driver
/// never imports, so it does not reach the image.
///
/// It needs `ld.lld` and `qemu-system-riscv64` from `dev-kernel.nix`.
#[test]
fn kernel0_caps_claims_one_line_and_one_frame_and_prints_them() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU) {
        println!(
            "skipping the kernel0-caps boot test: 'ld.lld' or 'qemu-system-riscv64' is not \
             on PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    boot_under_qemu(
        &RISCV64_QEMU,
        "kernel0_caps",
        &fixture("../examples/kernel0-caps"),
        &["line 10", "frame 0x80100000", "offset 0x80100010"],
    );
}

// ---------------------------------------------------------------------------
// Kernel stage 10 (MK9-S4-M8): Sv39 address spaces. The encoders below are
// pure arithmetic and run hosted on both backends; the boot and the two faults
// are freestanding riscv64 and skip in a shell without `ld.lld` and
// `qemu-system-riscv64`.
// ---------------------------------------------------------------------------

/// Kernel stage 10's hosted half: the Sv39 VPN split, the PTE encoder and the
/// `satp` encoder, copied out of `examples/kernel0-vm/src/vm.kp` into a project
/// both backends build and run. The suite's `KELPC_ORACLE=c` pass compares the
/// same literal, so the C oracle pins what the LLVM-default freestanding image
/// computes.
#[test]
fn the_sv39_vpn_split_and_pte_and_satp_encoders_are_the_specs() {
    assert_project_runs(
        "vm_pure",
        &project("vm_pure"),
        "255\n511\n511\n0\n4095\n536871119\n207\n2147483648\ntrue\ntrue\nfalse\ntrue\n8\n0\n524293\n",
    );
}

/// Build a freestanding project and assert the linked image names no undefined
/// symbol. Kernel stage 10's page tables reach only the frame-region symbols the
/// linker script defines, and the walker's `Ptr<u64>` accesses and `asm` must
/// leave no libc behind; a `malloc`, a `memcpy` or an unresolved frame bound
/// would show up here rather than at the boot.
fn assert_no_undefined_symbols(name: &str, target: &Path) {
    let bin = out_path(name);
    let build = Command::new(kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .env_remove("KELPC_ORACLE")
        .arg("build")
        .arg(target)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the freestanding build of '{name}' failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let path = bin.to_str().expect("the artifact path is utf-8");
    let symbols = tool("llvm-nm", &["-u", path]);
    assert!(
        symbols.trim().is_empty(),
        "the image for '{name}' has undefined symbols:\n{symbols}"
    );
}

/// Kernel stage 10's acceptance test: `examples/kernel0-vm` builds an Sv39
/// root table, identity-maps the image, its stack and the frame region, maps
/// the UART and one high page, drops from machine mode to supervisor mode,
/// turns the MMU on, and proves that the high virtual name and the
/// identity-mapped physical name reach the same frame. Every step is a line on
/// the UART.
///
/// It needs `ld.lld` and `qemu-system-riscv64` from `dev-kernel.nix`.
#[test]
fn kernel0_vm_boots_and_aliases_a_high_page() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU) {
        println!(
            "skipping the kernel0-vm boot test: 'ld.lld' or 'qemu-system-riscv64' is not on \
             PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    boot_under_qemu(
        &RISCV64_QEMU,
        "kernel0_vm",
        &fixture("../examples/kernel0-vm"),
        &[
            "vm: m-mode",
            "vm: root",
            "vm: identity",
            "vm: uart page",
            "vm: high page",
            "vm: s-mode",
            "vm: paging on",
            "vm: alias ok",
            "vm: unmap ok",
        ],
    );
}

/// Kernel stage 10's fault half: `examples/kernel0-vm-fault` takes a load page
/// fault (`scause` 13) on an unmapped address and a store page fault (15) on a
/// read-only leaf, and the S-mode trap handler prints both. The lines are the
/// handler's own hex rendering of `scause` and `stval`.
///
/// It needs `ld.lld` and `qemu-system-riscv64` from `dev-kernel.nix`.
#[test]
fn kernel0_vm_fault_reports_a_load_and_a_store_page_fault() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU) {
        println!(
            "skipping the kernel0-vm-fault boot test: 'ld.lld' or 'qemu-system-riscv64' is not \
             on PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    boot_under_qemu(
        &RISCV64_QEMU,
        "kernel0_vm_fault",
        &fixture("../examples/kernel0-vm-fault"),
        &[
            "fault: m-mode",
            "fault: paging on",
            "fault: scause 0xd stval 0x40000000",
            "fault: scause 0xf stval 0x3ffffff000",
        ],
    );
}

/// The two address-space images link with no undefined symbol: the page-table
/// walker, the frame allocator and the dropped-to-supervisor boot must each
/// resolve entirely inside the image.
///
/// It needs `ld.lld` and the llvm tools from `dev-kernel.nix`.
#[test]
fn kernel0_vm_images_link_with_no_undefined_symbols() {
    if !have_ld_lld() {
        println!(
            "skipping the kernel0-vm symbol test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    assert_no_undefined_symbols("kernel0_vm", &fixture("../examples/kernel0-vm"));
    assert_no_undefined_symbols("kernel0_vm_fault", &fixture("../examples/kernel0-vm-fault"));
}

/// Kernel stage 11's frame half: the trap frame is 264 bytes with `x1` at 0,
/// `x2` at 8, `x31` at 240, `sepc` at 248 and `sstatus` at 256. Those numbers
/// are what `trap_entry`'s asm hardcodes, so the project prints them and the
/// test pins them; the boot checks the same four against the same constants and
/// prints `user: frame ok` only when they agree. `trap_frame_layout` is a copy
/// of the example's declaration, and the layout queries fold in the front end,
/// so this runs on both backends with the same output.
#[test]
fn the_trap_frame_offsets_are_what_the_trap_entry_hardcodes() {
    assert_project_runs(
        "trap_frame_layout",
        &project("trap_frame_layout"),
        "264\n0\n8\n240\n248\n256\n",
    );
}

/// Kernel stage 21: the scheduler's task table is the second hardcoded layout.
/// The naked trap entry saves `x1..x31` at 0..240, `sepc` at 248 and `mstatus`
/// at 256, reads the scheduler's address from the word at 264, and the
/// scheduler reaches `state` at 272, `id` at 280 and the 4096-byte stack at 288
/// from the table's base. `sched.layout_ok` checks the same numbers at boot.
#[test]
fn the_scheduler_task_offsets_are_what_the_trap_entry_hardcodes() {
    assert_project_runs(
        "sched_task_layout",
        &project("sched_task_layout"),
        "264\n8\n248\n256\n4384\n0\n264\n272\n280\n288\n",
    );
}

/// Kernel stage 11's acceptance test: `examples/kernel0-user` drops to U-mode
/// and takes `ecall`s. The first program writes `hello from user`, earns
/// `-14` from a write whose pointer names a mapped kernel page, turns the errno
/// into a hex digit and writes `bad write -> E`, then exits with 7; the second
/// loads from an unmapped address, and the kernel prints the cause and survives
/// it. Every line is QEMU's own stdout.
///
/// It needs `ld.lld` and `qemu-system-riscv64` from `dev-kernel.nix`.
#[test]
fn kernel0_user_runs_a_syscall_abi_and_kills_a_faulting_program() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU) {
        println!(
            "skipping the kernel0-user boot test: 'ld.lld' or 'qemu-system-riscv64' is not \
             on PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    boot_under_qemu(
        &RISCV64_QEMU,
        "kernel0_user",
        &fixture("../examples/kernel0-user"),
        &[
            "user: s-mode",
            "user: paging on",
            "user: frame ok",
            "hello from user",
            "user: efault ok",
            "bad write -> E",
            "user exited 7",
            "user killed: scause 13",
            "kernel survived",
        ],
    );
}

/// The user-mode image links with no undefined symbol: the trap entry, the
/// dispatcher and the syscall copy reach only the image's own code and the
/// frame region's linker symbols, and the two user programs live in their own
/// sections rather than in a libc the image does not have.
///
/// It needs `ld.lld` and the llvm tools from `dev-kernel.nix`.
#[test]
fn kernel0_user_image_links_with_no_undefined_symbols() {
    if !have_ld_lld() {
        println!(
            "skipping the kernel0-user symbol test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    assert_no_undefined_symbols("kernel0_user", &fixture("../examples/kernel0-user"));
}

/// Kernel stage 21's acceptance boot: `examples/kernel0-sched` programs the
/// CLINT's `mtimecmp`, enables the machine timer interrupt and round-robins
/// three tasks that never yield, so the letters in the serial log can only come
/// from preemption. The example's `run.sh` boots it and asserts that all three
/// letters appear, that the letter changes at least three times and that no
/// line is torn; this runs that script, which is where the serial assertions
/// live.
///
/// It needs `ld.lld` and `qemu-system-riscv64` from `dev-kernel.nix`.
#[test]
fn kernel0_sched_preempts_three_tasks_on_the_machine_timer() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU) {
        println!(
            "skipping the kernel0-sched boot test: 'ld.lld' or 'qemu-system-riscv64' is not \
             on PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let script = fixture("../examples/kernel0-sched/run.sh");
    let run = Command::new("sh")
        .arg(&script)
        .env("KELPC", kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .env_remove("KELPC_ORACLE")
        .output()
        .expect("failed to spawn the kernel0-sched run script");
    assert!(
        run.status.success(),
        "examples/kernel0-sched/run.sh failed:\n{}\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(
        stdout.contains("no torn lines"),
        "the run script did not reach its final line:\n{stdout}"
    );
}

/// The scheduler image links with no undefined symbol: the trap entry, the
/// scheduler and the CLINT driver reach only the image's own code, the linker
/// script's stack symbols and the two device windows it was granted.
///
/// It needs `ld.lld` and the llvm tools from `dev-kernel.nix`.
#[test]
fn kernel0_sched_image_links_with_no_undefined_symbols() {
    if !have_ld_lld() {
        println!(
            "skipping the kernel0-sched symbol test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    assert_no_undefined_symbols("kernel0_sched", &fixture("../examples/kernel0-sched"));
}

/// A `pub static`/`pub static mut` is reached across modules exactly as a
/// `pub const` is (`state.COUNT`, `state.LIMIT`), and the mutable one is still
/// `unsafe` on the importing side.
#[test]
fn a_public_static_crosses_a_module_boundary() {
    assert_project_runs("static_module", &project("static_module"), "2\n3\n");
}

/// A static without `pub` is private to its module, which is the same
/// visibility gate a `const` goes through.
#[test]
fn a_private_static_is_not_reachable_from_another_module() {
    assert_project_rejected(
        "static_private",
        &project("static_private"),
        "'state.HIDDEN' is private",
    );
}

// ---------------------------------------------------------------------------
// Kernel stage 6 (MK9-S6-M1, roadmap B-8): `defer`. `defer.rs` lowers every
// deferred action to ordinary code at each exit of its block, so the fixtures
// below are ordinary programs by the time either backend sees them: LLVM is the
// default and these run there, and the suite's `KELPC_ORACLE=c` pass runs the
// same programs through the C oracle.
// ---------------------------------------------------------------------------

/// The deferred actions of one block run last-in-first-out, after the block's
/// own statements, and the block form (`defer { ... }`) is one action.
#[test]
fn deferred_actions_run_last_in_first_out() {
    assert_compiles_and_runs(
        "tests/fixtures/defer_lifo.kp",
        "body\nthird-a\nthird-b\nsecond\nfirst\n",
    );
}

/// A `return` nested in an `if` inside a `while` leaves the loop body and then
/// the function body, so the loop body's defer runs and then the function's.
#[test]
fn a_return_inside_a_loop_runs_both_blocks_defers() {
    assert_compiles_and_runs(
        "tests/fixtures/defer_return_in_loop.kp",
        "loop-out\nloop-out\nloop-out\nf-out\n20\n",
    );
}

/// Each block owns its own deferred actions, so an inner block's run when it is
/// left and the outer block's wait for the outer exit.
#[test]
fn a_nested_blocks_defers_run_at_its_own_exit() {
    assert_compiles_and_runs(
        "tests/fixtures/defer_nested_blocks.kp",
        "inner-body\ninner-end\nouter-body\nouter-end\n",
    );
}

/// A deferred action reads its locals at the block's exit, so a mutation after
/// the `defer` statement is visible to it.
#[test]
fn a_deferred_action_sees_the_final_value_of_a_local() {
    assert_compiles_and_runs("tests/fixtures/defer_sees_final_value.kp", "42\n");
}

/// The kernel pattern: a flag set at the top is released by a `defer`, and the
/// caller sees it clear after the function returns.
#[test]
fn a_defer_releases_an_atomic_lock_flag_on_the_way_out() {
    assert_compiles_and_runs(
        "tests/fixtures/defer_lock_flag.kp",
        "true\ntrue\nfalse\n",
    );
}

/// A function with a borrowed return type keeps its elided lifetime when its
/// body has a `defer`, and a `panic` tail stays a diverging tail (so the
/// deferred action does not run on it).
#[test]
fn a_defer_keeps_a_borrowed_return_and_a_panic_tail_working() {
    assert_compiles_and_runs(
        "tests/fixtures/defer_borrow_return.kp",
        "as_str-out\nhello\n",
    );
}

/// A `defer` written in an `if` block belongs to that block, so the branch's
/// action runs before the function body's.
#[test]
fn a_defer_in_a_branch_runs_before_the_function_body_defer() {
    assert_compiles_and_runs(
        "tests/fixtures/defer_then_branch.kp",
        "then\nout\n1\nout\n2\n",
    );
}

/// `for` desugars to a `while`, so a `defer` in the loop body runs once per
/// iteration.
#[test]
fn a_defer_in_a_for_body_runs_once_per_iteration() {
    assert_compiles_and_runs(
        "tests/fixtures/defer_for_loop.kp",
        "1\niter-end\n2\niter-end\n",
    );
}

/// A block used as a value keeps that value while its deferred actions run,
/// and a function whose body is a block expression is the same case.
#[test]
fn a_value_block_runs_its_defers_and_keeps_its_value() {
    assert_compiles_and_runs(
        "tests/fixtures/defer_value_block.kp",
        "block-out\ntext\nf-out\n5\n",
    );
}

/// The `?` refusal is about the block the `defer` is written in: a `?` inside a
/// `fun` literal belongs to the closure and leaves the outer `defer` alone.
#[test]
fn a_defer_tolerates_a_question_mark_inside_a_closure() {
    assert_compiles_and_runs("tests/fixtures/defer_closure_try.kp", "4\nout\n");
}

/// A panic never returns, so a deferred action does not run on the way out: the
/// action's line is absent from stdout and the process still reports the panic.
#[test]
fn a_panic_does_not_run_a_deferred_action() {
    let source = "tests/fixtures/defer_panic_skips_action.kp";
    let bin = out_path("defer_panic_skips_action");
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
    let run = Command::new(&bin).output().expect("failed to run the binary");
    assert!(
        !run.status.success(),
        "expected '{source}' to panic, but it exited successfully"
    );
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(stdout.contains("before"), "the program printed nothing before the panic:\n{stdout}");
    assert!(
        !stdout.contains("after"),
        "a panic ran the deferred action:\n{stdout}"
    );
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(stderr.contains("panic: stop"), "unexpected stderr:\n{stderr}");
}

/// `return f()` where `f` returns `unit` is legal: there is no value to carry,
/// so both backends emit the call for its side effect and a bare return.
#[test]
fn returning_a_unit_valued_call_is_legal_on_both_backends() {
    assert_compiles_and_runs("tests/fixtures/unit_return_value.kp", "said\n");
}

/// `?` leaves the enclosing function, which would skip the deferred action, so
/// a `defer` and a `?` in one block are refused by name.
#[test]
fn a_defer_with_a_question_mark_exit_is_refused_by_name() {
    assert_rejected_with(
        "tests/fixtures/defer_try_exit.kp",
        "defer with an early `?` exit is not supported yet",
    );
}

/// A deferred action has no caller to return to, so a `return` inside one is
/// refused rather than duplicated at every exit.
#[test]
fn a_return_inside_a_deferred_action_is_refused() {
    assert_rejected_with(
        "tests/fixtures/defer_return_inside.kp",
        "a deferred action cannot contain 'return'",
    );
}

/// A local moved out before the block's exit is a use of a moved value when the
/// deferred action reads it, reported at the `defer`.
#[test]
fn a_deferred_action_reading_a_moved_local_is_refused() {
    assert_rejected_with(
        "tests/fixtures/defer_use_after_move.kp",
        "use of moved value",
    );
}

/// A deferred action sees only the locals already in scope where the `defer` is
/// written, so naming one declared later in the block is refused.
#[test]
fn a_defer_naming_a_later_local_is_refused() {
    assert_rejected_with(
        "tests/fixtures/defer_later_local.kp",
        "declared later in the block",
    );
}

/// `break`/`continue` are ordinary loop control in a written `while`
/// (`plan/iterators.md`).
#[test]
fn break_and_continue_control_a_while_loop() {
    assert_compiles_and_runs("tests/fixtures/break_and_continue.kp", "1\n2\n4\n5\n");
}

/// A deferred action runs while its block is being left, so a `break` inside it
/// has no loop to leave.
#[test]
fn a_break_inside_a_deferred_action_is_refused() {
    assert_rejected_with(
        "tests/fixtures/defer_break_inside.kp",
        "a deferred action cannot contain 'break'",
    );
}

/// `break`/`continue` outside every loop is refused at the statement.
#[test]
fn break_outside_a_loop_is_refused() {
    assert_rejected_with(
        "tests/fixtures/for_iter_break_outside.kp",
        "no loop encloses it",
    );
}

/// Kernel stage 6's freestanding acceptance test: a project that releases a
/// lock flag with a `defer` builds through LLVM and `ld.lld` into an ELF64
/// RISC-V image that references no undefined symbol.
///
/// It needs `ld.lld`, `llvm-readelf`, `llvm-objdump` and `llvm-nm`, all from
/// `dev-kernel.nix`; a shell without them prints a skip.
#[test]
fn a_freestanding_project_can_release_a_lock_with_a_defer() {
    if !have_ld_lld() {
        println!(
            "skipping the freestanding defer test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = out_path("freestanding_defer");
    let build = Command::new(kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .env_remove("KELPC_ORACLE")
        .arg("build")
        .arg(project("freestanding_defer"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the freestanding defer build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let path = bin.to_str().expect("the artifact path is utf-8");
    let header = tool("llvm-readelf", &["-h", path]);
    assert!(header.contains("ELF64"), "not an ELF64 image:\n{header}");
    assert!(header.contains("RISC-V"), "not a RISC-V image:\n{header}");

    let disassembly = tool("llvm-objdump", &["-d", path]);
    assert!(disassembly.contains("_start"), "no _start in the image:\n{disassembly}");
    assert!(disassembly.contains("wfi"), "the _start halt is missing:\n{disassembly}");

    let symbols = tool("llvm-nm", &[path]);
    let undefined: Vec<&str> = symbols
        .lines()
        .filter(|line| line.split_whitespace().next() == Some("U"))
        .collect();
    assert!(
        undefined.is_empty(),
        "the freestanding defer image has undefined symbols:\n{}",
        undefined.join("\n")
    );
}

// ---------------------------------------------------------------------------
// Kernel stage 9 (MK9-S6-M1, roadmap B-8): the second half of stage 6, a user
// `Drop`. `fun mut T.drop()` is the destructor; the generated `kelp_drop_<T>`
// calls it first and drops the fields after, so every drop point the plan
// already has reaches it. The fixtures run on both backends: LLVM is the
// default and the suite's `KELPC_ORACLE=c` pass runs the same programs through
// the C oracle.
// ---------------------------------------------------------------------------

/// The user destructor runs before the fields, fields drop in declaration
/// order, and locals die last-in-first-out.
#[test]
fn a_user_destructor_runs_before_fields_in_declaration_order() {
    assert_compiles_and_runs(
        "tests/fixtures/drop_order.kp",
        "body\nleaf\n3\npair\nleaf\n1\nleaf\n2\n",
    );
}

/// Every drop point the plan already had reaches the destructor: an early
/// `return` frees the frame after reading the returned value, and a loop body
/// frees one value per iteration.
#[test]
fn a_destructor_runs_at_every_drop_point() {
    assert_compiles_and_runs(
        "tests/fixtures/drop_control_flow.kp",
        "leaf\n1\n9\nleaf\n2\nleaf\n1\n0\nloop\nleaf\n10\nloop\nleaf\n11\n",
    );
}

/// Reassigning an owned `var` of a destructor type runs the destructor on the
/// value it replaces.
#[test]
fn reassigning_an_owned_destructor_value_runs_the_destructor() {
    assert_compiles_and_runs(
        "tests/fixtures/drop_reassign.kp",
        "leaf\n1\nafter\nleaf\n2\n",
    );
}

/// A moved-out value is not dropped by the scope that declared it.
#[test]
fn a_moved_out_destructor_value_is_not_dropped() {
    assert_compiles_and_runs("tests/fixtures/drop_moved.kp", "moved\nleaf\n1\n");
}

/// A container drops each element through the element's destructor, including a
/// `Leaf` held by a `datac` held by a `List`.
#[test]
fn containers_drop_each_element_through_its_destructor() {
    assert_compiles_and_runs(
        "tests/fixtures/drop_container.kp",
        "pushed\nleaf\n1\nleaf\n2\ncleared\npopped\n3\nholder\nleaf\n4\nleaf\n3\n",
    );
}

/// An `enum` destructor runs for every variant, and the live variant's payload
/// drops after it.
#[test]
fn an_enum_destructor_runs_for_every_variant() {
    assert_compiles_and_runs("tests/fixtures/drop_enum.kp", "body\nmaybe\nmaybe\nleaf\n1\n");
}

/// A `Map` value of a destructor type: a removed value drops at the new owner,
/// and the values the map still holds drop when the map does.
#[test]
fn map_values_drop_through_their_destructor() {
    assert_compiles_and_runs(
        "tests/fixtures/drop_map.kp",
        "body\nremoved\ndrop\n1\ndrop\n2\n",
    );
}

/// The kernel pattern the stage exists for: the guard's destructor releases the
/// flag, so the caller sees it clear after the guarded call returns.
#[test]
fn a_lock_guard_releases_the_flag_through_its_destructor() {
    assert_compiles_and_runs("tests/fixtures/drop_guard_lock.kp", "true\nfalse\n");
}

/// A destructor type that owns a buffer: the user body runs first and can read
/// the field, and the field drop frees it after. It is in the AddressSanitizer
/// pass, where a missing drop is a leak and a second one a double free.
#[test]
fn a_destructor_type_owns_its_buffer_until_the_last_field_drop() {
    assert_compiles_and_runs("tests/fixtures/drop_owned_field.kp", "body\nnamed\n4\nnamed\n5\n");
}

/// A `Guard` moved into a closure that crosses a thread boundary is dropped on
/// the worker side when the closure's environment is freed, and the join sees
/// it.
#[test]
fn a_destructor_in_a_thread_closures_environment_runs_on_the_worker() {
    assert_compiles_and_runs("tests/fixtures/drop_thread_closure.kp", "9\n1\n");
}

/// A destructor already runs when the value dies, so naming it explicitly would
/// run it twice.
#[test]
fn an_explicit_destructor_call_is_refused_by_name() {
    assert_rejected_with(
        "tests/fixtures/explicit_destructor_call.kp",
        "explicit destructor call",
    );
}

/// A destructor's shape is checked where it is declared: a shared receiver is
/// not the mutable borrow the drop body needs.
#[test]
fn a_destructor_must_take_its_receiver_with_mut() {
    assert_rejected_with(
        "tests/fixtures/drop_wrong_signature.kp",
        "must take its receiver with 'mut'",
    );
}

/// One destructor per type: a second declaration shares the `T#drop` key.
#[test]
fn a_second_destructor_for_one_type_is_refused() {
    assert_rejected_with(
        "tests/fixtures/drop_two_impls.kp",
        "method 'Leaf.drop' is defined more than once",
    );
}

/// Only a user `datac`/`enum` can declare a destructor; a builtin type's
/// methods are the compiler's.
#[test]
fn a_destructor_on_a_builtin_type_is_refused() {
    assert_rejected_with(
        "tests/fixtures/drop_builtin_type.kp",
        "is a builtin type",
    );
}

/// A destructor type is not `Copy`, so the derived deep-copy fixpoint excludes
/// it: `List.clone` is refused rather than producing a second owner.
#[test]
fn a_destructor_type_cannot_be_copied() {
    assert_rejected_with(
        "tests/fixtures/drop_not_copy.kp",
        "a type with a destructor is not `Copy`",
    );
}

/// The map clones every key in, so a destructor type cannot declare a key
/// pair either.
#[test]
fn a_destructor_type_cannot_be_a_map_key() {
    assert_rejected_with(
        "tests/fixtures/drop_declared_key.kp",
        "a type with a destructor is not deep-copyable",
    );
}

/// A partial move of a destructor-typed field zeroes the slot it came from, so
/// the base's own drop would run the destructor a second time on the empty
/// slot; the move is refused instead.
#[test]
fn a_partial_move_of_a_destructor_field_is_refused() {
    assert_rejected_with(
        "tests/fixtures/drop_partial_move.kp",
        "it is a type with a destructor, and a partial move zeroes the slot",
    );
}

/// Kernel stage 9's freestanding acceptance test: a project whose guard
/// releases a static lock flag through its destructor builds through LLVM and
/// `ld.lld` into an ELF64 RISC-V image that references no undefined symbol, and
/// the generated drop and the user body are both in the image.
///
/// It needs `ld.lld`, `llvm-readelf`, `llvm-objdump` and `llvm-nm`, all from
/// `dev-kernel.nix`; a shell without them prints a skip.
#[test]
fn a_freestanding_project_can_release_a_lock_with_a_destructor() {
    if !have_ld_lld() {
        println!(
            "skipping the freestanding drop test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = out_path("freestanding_drop");
    let build = Command::new(kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .env_remove("KELPC_ORACLE")
        .arg("build")
        .arg(project("freestanding_drop"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the freestanding drop build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let path = bin.to_str().expect("the artifact path is utf-8");
    let header = tool("llvm-readelf", &["-h", path]);
    assert!(header.contains("ELF64"), "not an ELF64 image:\n{header}");
    assert!(header.contains("RISC-V"), "not a RISC-V image:\n{header}");

    let disassembly = tool("llvm-objdump", &["-d", path]);
    assert!(disassembly.contains("_start"), "no _start in the image:\n{disassembly}");
    // The generated drop and the user body it calls are both in the image, so
    // the destructor is linked in and not merely referenced.
    assert!(
        disassembly.contains("kelp_drop_LockGuard"),
        "the generated drop is missing:\n{disassembly}"
    );
    assert!(
        disassembly.contains("kelp_LockGuard__drop"),
        "the declared destructor is missing:\n{disassembly}"
    );

    let symbols = tool("llvm-nm", &[path]);
    let undefined: Vec<&str> = symbols
        .lines()
        .filter(|line| line.split_whitespace().next() == Some("U"))
        .collect();
    assert!(
        undefined.is_empty(),
        "the freestanding drop image has undefined symbols:\n{}",
        undefined.join("\n")
    );
}

// ---------------------------------------------------------------------------
// Kernel stage 7 (MK9-S5-M1, roadmap B-7): the global allocator hook. A
// freestanding manifest names `alloc`/`dealloc` (and optionally `realloc`), and
// `Box`/`List`/`Map` then allocate and free through them instead of libc. The
// rejection fixtures are front-end refusals, so they name the same construct on
// both backends; the builds and the boot are LLVM-only by design.
// ---------------------------------------------------------------------------

/// Kernel stage 7's acceptance build: a freestanding project that configures all
/// three allocator functions and instantiates `Box`, `List` and `Map`, linked
/// into an image with no undefined symbol. The IR beside the binary is checked
/// too: every allocation calls `@kelp_alloc`/`@kelp_realloc` and every free
/// `@kelp_dealloc`, with no `malloc`/`free` call at all.
///
/// It needs `ld.lld` and `llvm-nm` from `dev-kernel.nix`.
#[test]
fn freestanding_containers_allocate_through_the_project_allocator() {
    if !have_ld_lld() {
        println!(
            "skipping the freestanding allocator test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = out_path("freestanding_alloc");
    let build = Command::new(kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .env_remove("KELPC_ORACLE")
        .arg("build")
        .arg(project("freestanding_alloc"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the freestanding allocator build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let path = bin.to_str().expect("the artifact path is utf-8");
    let symbols = tool("llvm-nm", &[path]);
    let undefined: Vec<&str> = symbols
        .lines()
        .filter(|line| line.split_whitespace().next() == Some("U"))
        .collect();
    assert!(
        undefined.is_empty(),
        "the freestanding allocator image has undefined symbols:\n{}",
        undefined.join("\n")
    );

    let ir = std::fs::read_to_string(bin.with_extension("ll"))
        .expect("the LLVM backend writes textual IR beside the object");
    for symbol in [
        "@kelp_alloc(",
        "@kelp_dealloc(",
        "@kelp_realloc(",
        "@__kelp_freestanding_memset(",
    ] {
        assert!(
            ir.contains(symbol),
            "the container bodies should call '{symbol}'"
        );
    }
    for symbol in ["call ptr @malloc", "call ptr @realloc", "call void @free", "call ptr @memset", "call ptr @memcpy"] {
        assert!(
            !ir.contains(symbol),
            "a freestanding build must not call libc's '{symbol}'"
        );
    }
}

/// Kernel stage 7's acceptance boot: `examples/kernel0-heap` defines a bump
/// allocator over a `static mut` arena, builds a `List<i32>` of 1..=10 with no
/// `realloc` (the alloc + copy + dealloc fallback), sums it and prints
/// `heap sum 55`, then prints `balanced` once every allocation has reached a
/// matching `dealloc`.
///
/// It needs `ld.lld` and `qemu-system-riscv64` from `dev-kernel.nix`.
#[test]
fn kernel0_heap_boots_and_balances_its_allocations() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU) {
        println!(
            "skipping the kernel0-heap boot test: 'ld.lld' or 'qemu-system-riscv64' is not on \
             PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    boot_under_qemu(
        &RISCV64_QEMU,
        "kernel0_heap",
        &fixture("../examples/kernel0-heap"),
        &["heap sum 55", "balanced"],
    );
}

/// Kernel stage 12's acceptance build: `examples/kernel0-string` builds a
/// `String` from a literal and through `str.concat`/`str.from_u64`, clones and
/// grows it, and grows a `Bytes` past several doublings, all on the project's
/// allocator. The `llvm-nm` line is the claim: no `strlen`, no `memcpy`, no
/// `malloc`/`realloc`/`free` may appear as undefined, and the IR must call the
/// project's own allocator and no libc allocation.
///
/// It needs `ld.lld` and `llvm-nm` from `dev-kernel.nix`.
#[test]
fn kernel0_string_builds_on_the_project_allocator_with_no_undefined_symbols() {
    if !have_ld_lld() {
        println!(
            "skipping the kernel0-string build test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = out_path("kernel0_string");
    let build = Command::new(kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .env_remove("KELPC_ORACLE")
        .arg("build")
        .arg(fixture("../examples/kernel0-string"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the kernel0-string build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let path = bin.to_str().expect("the artifact path is utf-8");
    let symbols = tool("llvm-nm", &[path]);
    let undefined: Vec<&str> = symbols
        .lines()
        .filter(|line| line.split_whitespace().next() == Some("U"))
        .collect();
    assert!(
        undefined.is_empty(),
        "the kernel0-string image has undefined symbols:\n{}",
        undefined.join("\n")
    );

    let ir = std::fs::read_to_string(bin.with_extension("ll"))
        .expect("the LLVM backend writes textual IR beside the object");
    for symbol in ["@kelp_alloc(", "@kelp_realloc(", "@kelp_dealloc("] {
        assert!(
            ir.contains(symbol),
            "the buffer bodies should call the project allocator through '{symbol}'"
        );
    }
    for symbol in [
        "call ptr @malloc",
        "call ptr @realloc",
        "call void @free",
        "call i64 @strlen",
    ] {
        assert!(
            !ir.contains(symbol),
            "a freestanding build must not call libc's '{symbol}'"
        );
    }
}

/// Kernel stage 12's acceptance boot: `examples/kernel0-string` concatenates,
/// formats an integer and grows a `Bytes`, prints four lines through the NS16550
/// UART, and prints `balanced` only when every allocation reached a matching
/// free. The lines are QEMU's own stdout, not the source's.
///
/// It needs `ld.lld` and `qemu-system-riscv64` from `dev-kernel.nix`.
#[test]
fn kernel0_string_boots_and_prints_its_text_over_the_uart() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU) {
        println!(
            "skipping the kernel0-string boot test: 'ld.lld' or 'qemu-system-riscv64' is not on \
             PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    boot_under_qemu(
        &RISCV64_QEMU,
        "kernel0_string",
        &fixture("../examples/kernel0-string"),
        &["s: hello kelp!", "n: 12345", "b: 300", "balanced"],
    );
}

/// Kernel stage 23's acceptance build: `examples/kernel0-eq` compares a
/// `String`-field `datac` (a declared `eq` for `==`, and the derived key pair a
/// `Map<Pair, i32>` resolves through), slices a `str` view and copies the slice
/// with `View.to_string`, and runs a `std/regex` match. The `llvm-nm` line is
/// the claim that all of it is freestanding: `kelp_map_key_eq` and the two
/// `View` bodies are emitted into the module instead of being left for the
/// hosted runtime object, and no libc byte routine or allocator is called.
///
/// It needs `ld.lld` and `llvm-nm` from `dev-kernel.nix`.
#[test]
fn kernel0_eq_builds_with_no_undefined_symbols() {
    if !have_ld_lld() {
        println!(
            "skipping the kernel0-eq build test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = out_path("kernel0_eq");
    let build = Command::new(kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .env_remove("KELPC_ORACLE")
        .arg("build")
        .arg(fixture("../examples/kernel0-eq"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the kernel0-eq build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let symbols = tool("llvm-nm", &[bin.to_str().expect("the artifact path is utf-8")]);
    let undefined: Vec<&str> = symbols
        .lines()
        .filter(|line| line.split_whitespace().next() == Some("U"))
        .collect();
    assert!(
        undefined.is_empty(),
        "the kernel0-eq image has undefined symbols:\n{}",
        undefined.join("\n")
    );

    let ir = std::fs::read_to_string(bin.with_extension("ll"))
        .expect("the LLVM backend writes textual IR beside the object");
    for symbol in ["kelp_map_key_eq", "kelp_View__slice", "kelp_View__to_string"] {
        let defined = ir
            .lines()
            .any(|line| line.starts_with("define ") && line.contains(&format!("@{symbol}(")));
        assert!(
            defined,
            "a freestanding build should emit '@{symbol}' into the module"
        );
    }
    for symbol in [
        "call ptr @malloc",
        "call ptr @realloc",
        "call void @free",
        "call i64 @strlen",
    ] {
        assert!(
            !ir.contains(symbol),
            "a freestanding build must not call libc's '{symbol}'"
        );
    }
}

/// Kernel stage 23's acceptance boot: `examples/kernel0-eq` prints the
/// equality, `View` and `std/regex` results over the NS16550 UART, and prints
/// `balanced` only when every allocation reached a matching free. The lines are
/// QEMU's own stdout, not the source's.
///
/// It needs `ld.lld` and `qemu-system-riscv64` from `dev-kernel.nix`.
#[test]
fn kernel0_eq_boots_and_prints_the_equality_and_regex_lines() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU) {
        println!(
            "skipping the kernel0-eq boot test: 'ld.lld' or 'qemu-system-riscv64' is not on \
             PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    boot_under_qemu(
        &RISCV64_QEMU,
        "kernel0_eq",
        &fixture("../examples/kernel0-eq"),
        &[
            "eq same: yes",
            "eq differ: yes",
            "map keys: 2",
            "map hit: yes",
            "bytes eq: yes",
            "view slice: kelp",
            "view eq: yes",
            "regex match: hello42",
            "regex group: 42",
            "balanced",
        ],
    );
}

/// `List` with no `[target] alloc`/`dealloc` is refused exactly as before, with
/// a message that names the two keys whose absence is why.
#[test]
fn a_freestanding_list_without_an_allocator_is_refused() {
    assert_rejected_with(
        "tests/projects/freestanding_list_no_alloc",
        "'[target] alloc' and '[target] dealloc'",
    );
}

/// Without an allocator `String` stays refused, with the message that names the
/// two manifest keys whose absence is why (kernel stage 12).
#[test]
fn a_freestanding_string_without_an_allocator_is_refused() {
    assert_rejected_with(
        "tests/projects/freestanding_string_no_alloc",
        "'String', which is unavailable in a freestanding build",
    );
}

/// An allocator makes `String` legal, but a method with no libc-free lowering
/// is still refused by name at the call rather than left to the linker.
#[test]
fn a_freestanding_string_method_outside_the_libc_free_set_is_refused() {
    assert_rejected_with(
        "tests/projects/freestanding_alloc_string",
        "needs the hosted runtime",
    );
}

/// `str.from_f64`'s formatting is `snprintf`, so a float is refused by name
/// even when the allocator makes `String` legal.
#[test]
fn a_freestanding_float_format_is_refused_by_name() {
    assert_rejected_with(
        "tests/projects/freestanding_string_float",
        "needs the hosted runtime",
    );
}

/// The `View` surface is a borrowed `{ptr, len}` pair with no storage of its
/// own, so `len`/`get`/`slice`/`eq` build in a freestanding image with no
/// allocator; only `View.to_string` copies into an owned `String` and so needs
/// the pair.
#[test]
fn a_freestanding_view_slice_and_eq_build_without_an_allocator() {
    if !have_ld_lld() {
        println!(
            "skipping the freestanding view test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = out_path("freestanding_view");
    let build = Command::new(kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .env_remove("KELPC_ORACLE")
        .arg("build")
        .arg(project("freestanding_view"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the freestanding build of 'freestanding_view' failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let undefined = tool("llvm-nm", &["-u", bin.to_str().unwrap()]);
    assert!(
        undefined.trim().is_empty(),
        "the freestanding view image has undefined symbols:\n{undefined}"
    );
}

/// `alloc` and `dealloc` are required together: naming one alone is a manifest
/// error rather than an allocator that can never free.
#[test]
fn an_allocator_without_a_dealloc_is_refused() {
    assert_rejected_with(
        "tests/projects/freestanding_alloc_half",
        "'alloc' without 'dealloc'",
    );
}

/// A named allocator whose signature is not the `(size, align)` pair the
/// generated calls assume is refused with the expected signature.
#[test]
fn an_allocator_with_the_wrong_signature_is_refused() {
    assert_rejected_with(
        "tests/projects/freestanding_alloc_bad_sig",
        "wrong signature",
    );
}

/// The hosted regression: with no `[target]` allocator, containers still
/// allocate through libc. The program runs and prints its sum, and (on the LLVM
/// backend) the IR calls `@malloc`, never `@kelp_alloc`.
#[test]
fn hosted_containers_still_allocate_through_libc() {
    let bin = out_path("hosted_list_alloc");
    let build = kelpc()
        .arg("build")
        .arg(fixture("tests/fixtures/hosted_list_alloc.kp"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the hosted list fixture failed to build:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&bin).output().expect("failed to run the binary");
    assert!(run.status.success(), "the hosted list fixture failed to run");
    assert_eq!(String::from_utf8_lossy(&run.stdout), "6\n");

    if backend() == "llvm" {
        let ir = std::fs::read_to_string(bin.with_extension("ll"))
            .expect("the LLVM backend writes textual IR beside the object");
        // `List.reserve` grows through `realloc` (a null pointer on the first
        // call is libc's `malloc`), and the drop frees the buffer.
        assert!(
            ir.contains("call ptr @realloc") && ir.contains("call void @free"),
            "a hosted container should still allocate and free through libc"
        );
        assert!(
            !ir.contains("@kelp_alloc("),
            "a hosted build has no project allocator to call"
        );
    }
}

// ---------------------------------------------------------------------------
// Kernel gap 4: the freestanding runtime essentials. `_start` zeroes
// `[__bss_start, __bss_end)`, and the compiler defines the five `mem*`
// routines an aggregate copy calls, weakly and as loops. Freestanding is
// LLVM-only by design (the C backend's preamble is libc), so these tests build
// through the default backend with the oracle switch removed; they need
// `ld.lld` and the llvm tools from `dev-kernel.nix`.
// ---------------------------------------------------------------------------

/// Whether `llvm-mc` is on PATH. The weak-linkage test assembles the fixture
/// project's own riscv64 `memcpy` with it, because no C compiler on this host
/// targets that triple; the other kernel-gap-4 tests need only `ld.lld`.
fn have_llvm_mc() -> bool {
    Command::new("llvm-mc")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Builds a freestanding project through the default (LLVM) backend and returns
/// the ELF's path. The object and IR sit beside it (`<name>.o`, `<name>.ll`),
/// which is what the object-level checks read.
fn build_freestanding(name: &str, target: &Path) -> PathBuf {
    let bin = out_path(name);
    let build = Command::new(kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .env_remove("KELPC_ORACLE")
        .arg("build")
        .arg(target)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the freestanding build of '{name}' failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    bin
}

/// The address `llvm-nm` reports for `name`, as an integer.
fn nm_address(symbols: &str, name: &str) -> u64 {
    let line = symbols
        .lines()
        .find(|line| line.split_whitespace().last() == Some(name))
        .unwrap_or_else(|| panic!("the image has no symbol '{name}':\n{symbols}"));
    let address = line.split_whitespace().next().unwrap_or_default();
    u64::from_str_radix(address, 16)
        .unwrap_or_else(|_| panic!("the symbol '{name}' has no numeric address: {line}"))
}

/// The name of an `llvm-objdump` symbol header (`0000000080000034 <memcpy>:`).
fn disassembly_header(line: &str) -> Option<&str> {
    let (address, rest) = line.split_once(" <")?;
    if address.is_empty() || !address.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    rest.strip_suffix(">:")
}

/// The instruction lines of the function named `name`, from its `<_name>:`
/// header to the next header that names another function. `llvm-objdump` gives
/// each `la` in `_start` its own `.Lpcrel_hi<n>` label, so a header with that
/// prefix is part of the body and not a boundary.
fn disassembled_body(disassembly: &str, name: &str) -> Vec<String> {
    let mut body = Vec::new();
    let mut inside = false;
    for line in disassembly.lines() {
        let line = line.trim_end();
        if let Some(header) = disassembly_header(line) {
            if inside {
                if header != name && !header.starts_with(".Lpcrel_hi") {
                    break;
                }
            } else {
                inside = header == name;
            }
            continue;
        }
        if inside && !line.trim().is_empty() {
            body.push(line.to_string());
        }
    }
    assert!(
        !body.is_empty(),
        "the disassembly has no '{name}' body:\n{disassembly}"
    );
    body
}

/// One `llvm-objdump -d --no-show-raw-insn` instruction line, split into its
/// address, mnemonic and comma-stripped operands.
fn disassembled_instruction(line: &str) -> Option<(u64, String, Vec<String>)> {
    let (address, rest) = line.split_once(':')?;
    let address = u64::from_str_radix(address.trim(), 16).ok()?;
    let mut tokens = rest.split_whitespace().map(|token| token.trim_matches(',').to_string());
    let mnemonic = tokens.next()?;
    Some((address, mnemonic, tokens.collect()))
}

/// A signed decimal or `0x`-prefixed immediate, the two spellings
/// `llvm-objdump` prints.
fn parse_immediate(text: &str) -> i64 {
    let text = text.trim();
    if let Some(hex) = text.strip_prefix("-0x") {
        -i64::from_str_radix(hex, 16).unwrap_or_else(|_| panic!("bad immediate '{text}'"))
    } else if let Some(hex) = text.strip_prefix("0x") {
        i64::from_str_radix(hex, 16).unwrap_or_else(|_| panic!("bad immediate '{text}'"))
    } else {
        text.parse().unwrap_or_else(|_| panic!("bad immediate '{text}'"))
    }
}

/// The address a `la rd, symbol` sequence resolves to, read off the
/// disassembly: `auipc rd, hi` then `addi rd, rd, lo` is
/// `address(auipc) + (hi << 12) + lo`, and the linker resolved `symbol` into
/// exactly those two immediates.
fn la_target(body: &[(u64, String, Vec<String>)], register: &str) -> u64 {
    for pair in body.windows(2) {
        let (address, mnemonic, operands) = &pair[0];
        let (_, next_mnemonic, next_operands) = &pair[1];
        if mnemonic != "auipc" || operands.len() != 2 || operands[0] != register {
            continue;
        }
        if next_mnemonic != "addi"
            || next_operands.len() != 3
            || next_operands[0] != register
            || next_operands[1] != register
        {
            continue;
        }
        let high = parse_immediate(&operands[1]);
        let low = parse_immediate(&next_operands[2]);
        return (*address as i64).wrapping_add(high << 12).wrapping_add(low) as u64;
    }
    panic!("the body has no `la {register}, ...`: {body:?}");
}

/// Kernel gap 4's first acceptance test: a freestanding project whose entry
/// copies a 4 KiB `datac` (through its generated clone) and a `[u8; 4096]`
/// array. Both shapes lower to a `mem*` library call, which a bare-metal image
/// has no libc to define, so an image with no undefined symbol is the whole
/// claim. The `llvm-objdump -r` check pins the other half: the copy really does
/// reach a library routine the fallback has to supply.
#[test]
fn freestanding_memcpy_copies_a_large_aggregate_with_no_undefined_symbols() {
    if !have_ld_lld() {
        println!(
            "skipping the freestanding memcpy test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = build_freestanding("freestanding_memcpy", &project("freestanding_memcpy"));
    let path = bin.to_str().expect("the artifact path is utf-8");
    let symbols = tool("llvm-nm", &[path]);
    let undefined: Vec<&str> = symbols
        .lines()
        .filter(|line| line.split_whitespace().next() == Some("U"))
        .collect();
    assert!(
        undefined.is_empty(),
        "the freestanding image has undefined symbols:\n{}",
        undefined.join("\n")
    );

    let object = bin.with_extension("o");
    let relocations = tool(
        "llvm-objdump",
        &["-r", object.to_str().expect("the object path is utf-8")],
    );
    assert!(
        relocations.contains("memmove"),
        "the 4 KiB copies did not lower to a library call at all:\n{relocations}"
    );
}

/// Kernel gap 4's second acceptance test: the object defines all five `mem*`
/// routines as weak symbols, so an image that carries its own wins at the link,
/// and each body is a loop. The self-call check is the decisive one: LLVM's
/// loop-idiom pass rewrites a loop into a call to the library function of the
/// same name, which for `memcpy` is the function the loop is already in, and
/// `nobuiltin` plus the definition's own name are what stop it.
#[test]
fn the_freestanding_mem_routines_are_weak_and_do_not_call_themselves() {
    if !have_ld_lld() {
        println!(
            "skipping the freestanding mem routines test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = build_freestanding("freestanding_mem_weak", &project("freestanding_memcpy"));
    let path = bin.to_str().expect("the artifact path is utf-8");
    let symbols = tool("llvm-nm", &[path]);
    for name in ["memcpy", "memmove", "memset", "memcmp", "bcmp"] {
        let line = symbols
            .lines()
            .find(|line| line.split_whitespace().last() == Some(name))
            .unwrap_or_else(|| panic!("the image does not define '{name}':\n{symbols}"));
        assert_eq!(
            line.split_whitespace().nth(1),
            Some("W"),
            "'{name}' is not a weak definition: {line}"
        );
    }

    let disassembly = tool("llvm-objdump", &["-d", "--no-show-raw-insn", path]);
    for name in ["memcpy", "memmove", "memset", "memcmp", "bcmp"] {
        let body = disassembled_body(&disassembly, name);
        let self_call = body.iter().find(|line| {
            (line.contains("jal") || line.contains("call")) && line.contains(&format!("<{name}"))
        });
        assert!(
            self_call.is_none(),
            "'{name}' calls itself:\n{}",
            body.join("\n")
        );
    }
}

/// Kernel gap 4's third acceptance test: `_start` really clears the range the
/// linker script exported. The two registers the clear loop loads are decoded
/// back out of the disassembly and compared against the addresses `llvm-nm`
/// reports for `__bss_start` and `__bss_end`, so the loop cannot be iterating
/// some other range and passing by luck. `kernel0-bss` gives the range real
/// content; the stub is the same in every freestanding image.
#[test]
fn the_freestanding_entry_clears_between_the_bss_symbols() {
    if !have_ld_lld() {
        println!(
            "skipping the freestanding entry-clear test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = build_freestanding("kernel0_bss_entry", &fixture("../examples/kernel0-bss"));
    let path = bin.to_str().expect("the artifact path is utf-8");

    let symbols = tool("llvm-nm", &[path]);
    let bss_start = nm_address(&symbols, "__bss_start");
    let bss_end = nm_address(&symbols, "__bss_end");
    assert!(
        bss_end > bss_start,
        "the fixture's bss range is empty: {bss_start:#x}..{bss_end:#x}"
    );

    let disassembly = tool("llvm-objdump", &["-d", "--no-show-raw-insn", path]);
    let body = disassembled_body(&disassembly, "_start");
    let instructions: Vec<(u64, String, Vec<String>)> = body
        .iter()
        .filter_map(|line| disassembled_instruction(line))
        .collect();

    assert_eq!(
        la_target(&instructions, "t0"),
        bss_start,
        "_start does not load __bss_start into t0:\n{}",
        body.join("\n")
    );
    assert_eq!(
        la_target(&instructions, "t1"),
        bss_end,
        "_start does not load __bss_end into t1:\n{}",
        body.join("\n")
    );
    assert!(
        instructions
            .iter()
            .any(|(_, mnemonic, operands)| mnemonic == "sd"
                && operands.first().map(String::as_str) == Some("zero")),
        "the clear loop has no word store:\n{}",
        body.join("\n")
    );
    assert!(
        instructions.iter().any(|(_, mnemonic, _)| mnemonic == "bgeu"),
        "the clear loop has no compare:\n{}",
        body.join("\n")
    );
}

/// Kernel gap 4's weak-linkage test: a project whose own `memcpy` is a strong
/// definition links with that one, not the compiler's weak fallback. The
/// project ships `user_memcpy.s`; the test assembles it for the same triple
/// with `llvm-mc` (no C compiler here targets riscv64) and relinks the object
/// `kelpc` emitted beside it, then reads back which definition the link chose.
#[test]
fn a_project_that_defines_its_own_memcpy_links_with_that_one() {
    if !have_ld_lld() || !have_llvm_mc() {
        println!(
            "skipping the freestanding memcpy-override test: 'ld.lld' or 'llvm-mc' is not on \
             PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let target = project("freestanding_memcpy_user");
    let bin = build_freestanding("freestanding_memcpy_user", &target);

    let user_object = out_path("freestanding_memcpy_user_object");
    let assembled = Command::new("llvm-mc")
        .arg("-triple=riscv64-unknown-none-elf")
        .arg("-filetype=obj")
        .arg("-o")
        .arg(&user_object)
        .arg(target.join("user_memcpy.s"))
        .output()
        .expect("failed to spawn llvm-mc");
    assert!(
        assembled.status.success(),
        "llvm-mc failed on the project's own memcpy:\n{}",
        String::from_utf8_lossy(&assembled.stderr)
    );

    let relinked = out_path("freestanding_memcpy_user_relinked");
    let linked = Command::new("ld.lld")
        .arg("-T")
        .arg(target.join("kernel.ld"))
        .arg("-o")
        .arg(&relinked)
        .arg("--static")
        .arg(bin.with_extension("o"))
        .arg(&user_object)
        .output()
        .expect("failed to spawn ld.lld");
    assert!(
        linked.status.success(),
        "the relink with the project's own memcpy failed:\n{}",
        String::from_utf8_lossy(&linked.stderr)
    );

    let path = relinked.to_str().expect("the artifact path is utf-8");
    let symbols = tool("llvm-nm", &[path]);
    let memcpy_line = symbols
        .lines()
        .find(|line| line.split_whitespace().last() == Some("memcpy"))
        .unwrap_or_else(|| panic!("the relinked image has no memcpy:\n{symbols}"));
    assert_eq!(
        memcpy_line.split_whitespace().nth(1),
        Some("T"),
        "the compiler's weak memcpy won over the project's own: {memcpy_line}"
    );
    let memmove_line = symbols
        .lines()
        .find(|line| line.split_whitespace().last() == Some("memmove"))
        .unwrap_or_else(|| panic!("the relinked image has no memmove:\n{symbols}"));
    assert_eq!(
        memmove_line.split_whitespace().nth(1),
        Some("W"),
        "the untouched fallback is no longer weak: {memmove_line}"
    );

    let disassembly = tool("llvm-objdump", &["-d", "--no-show-raw-insn", path]);
    let body = disassembled_body(&disassembly, "memcpy").join("\n");
    assert!(
        body.contains("0x5eed"),
        "the linked memcpy is not the project's own body:\n{body}"
    );
}

/// Kernel gap 4's runtime proof. QEMU's RAM starts zeroed, so the example's
/// marker is a 64-byte pattern the image itself loads non-zero into a
/// `PROGBITS` section the script places inside the cleared range: `bss clear`
/// can only come out of the UART if `_start` really erased it. The ELF checks
/// pin the other half, that the pattern is in the image and covers exactly the
/// range the clear loop was pointed at, so the boot cannot pass vacuously.
#[test]
fn kernel0_bss_clears_an_image_loaded_marker_before_the_entry_runs() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU) {
        println!(
            "skipping the kernel0 bss test: 'ld.lld' or 'qemu-system-riscv64' is not on \
             PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let target = fixture("../examples/kernel0-bss");
    let bin = build_freestanding("kernel0_bss_image", &target);
    let path = bin.to_str().expect("the artifact path is utf-8");

    let sections = tool("llvm-readelf", &["-S", path]);
    let bss = sections
        .lines()
        .find(|line| line.split_whitespace().any(|token| token == ".bss"))
        .unwrap_or_else(|| panic!("the image has no .bss section:\n{sections}"));
    assert!(
        bss.contains("PROGBITS"),
        "the .bss is not image-loaded, so the boot would prove nothing:\n{bss}"
    );

    let symbols = tool("llvm-nm", &[path]);
    let bss_start = nm_address(&symbols, "__bss_start");
    let bss_end = nm_address(&symbols, "__bss_end");
    let marker = nm_address(&symbols, "kelp_bss_marker");
    // The marker is the first thing in `.bss`, which the project's script
    // orders `.bss.dirty` first. It is no longer the *whole* cleared range:
    // kernel stage 15 put the panic entry's `.bss` state (`kelp_panic_active`,
    // `kelp_panic_file`, `kelp_panic_line`) after it, so the assertion is that
    // the range starts at the marker and covers all 64 bytes of it.
    assert_eq!(
        bss_start, marker,
        "the marker is not the first byte of the cleared range: \
         {bss_start:#x}..{marker:#x}..{bss_end:#x}"
    );
    assert!(
        bss_end >= marker + 64,
        "the cleared range does not cover the whole marker: \
         {bss_start:#x}..{marker:#x}..{bss_end:#x}"
    );

    let contents = tool("llvm-objdump", &["-s", "-j", ".bss", path]);
    assert!(
        contents.contains("abababab"),
        "the image does not carry the marker pattern:\n{contents}"
    );

    boot_under_qemu(&RISCV64_QEMU, "kernel0_bss", &target, &["bss clear"]);
}

// ---------------------------------------------------------------------------
// Kernel stage 10: the second architecture. `[target] triple =
// "aarch64-unknown-none"` gets an AArch64 target machine and the aarch64
// `_start` stub (load `sp` from `__stack_top`, clear `[__bss_start,
// __bss_end)`, call the entry, spin on `wfe`), and `examples/kernel0-aarch64`
// is the PL011 driver that boots on QEMU's aarch64 `virt` machine. Freestanding
// is LLVM-only by design, so these build through the default backend; they need
// `ld.lld` and the llvm tools, and the boot needs `qemu-system-aarch64`.
// ---------------------------------------------------------------------------

/// The ELF half of the stage: the image is ELF64 AArch64 whose entry is the
/// linker script's 0x40000000, referencing no undefined symbol, and whose
/// `_start` is real AArch64 code that halts on `wfe`. A boot that worked by
/// accident on some other entry point could not pass these.
#[test]
fn kernel0_aarch64_builds_an_aarch64_elf_with_no_undefined_symbols() {
    if !have_ld_lld() {
        println!(
            "skipping the aarch64 elf test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = build_freestanding("kernel0_aarch64", &fixture("../examples/kernel0-aarch64"));
    let path = bin.to_str().expect("the artifact path is utf-8");

    let header = tool("llvm-readelf", &["-h", path]);
    assert!(header.contains("ELF64"), "not an ELF64 image:\n{header}");
    assert!(header.contains("AArch64"), "not an AArch64 image:\n{header}");
    let entry_line = header
        .lines()
        .find(|line| line.contains("Entry point address"))
        .unwrap_or_default();
    assert!(
        entry_line.contains("0x40000000"),
        "the entry point is not the linker script's 0x40000000:\n{header}"
    );

    // `llvm-objdump` is the disassembler half: the generated `_start` has to be
    // real AArch64 code, not an empty section.
    let disassembly = tool("llvm-objdump", &["-d", path]);
    assert!(
        disassembly.contains("_start"),
        "no _start in the image:\n{disassembly}"
    );
    assert!(
        disassembly.contains("wfe"),
        "the _start halt is missing:\n{disassembly}"
    );

    // `llvm-nm -u` prints what the linker had to leave for someone else: an
    // empty list is the whole claim that no libc symbol and no `kelp_rt` helper
    // is behind the image.
    let undefined = tool("llvm-nm", &["-u", path]);
    assert!(
        undefined.trim().is_empty(),
        "the aarch64 image has undefined symbols:\n{undefined}"
    );
}

/// The boot half: `examples/kernel0-aarch64` is a complete Kelp kernel that
/// boots on QEMU's aarch64 `virt` machine and writes one line to the PL011
/// UART. The assertion is against what QEMU captured, so it proves the boot,
/// the linker script's stack and the driver's polling, not that the source says
/// `hello from kelp (aarch64)`.
#[test]
fn kernel0_aarch64_boots_and_prints_its_greeting_over_the_uart() {
    if !have_ld_lld() || !have_qemu(&AARCH64_QEMU) {
        println!(
            "skipping the aarch64 boot test: 'ld.lld' or 'qemu-system-aarch64' is not on \
             PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    boot_under_qemu(
        &AARCH64_QEMU,
        "kernel0_aarch64_boot",
        &fixture("../examples/kernel0-aarch64"),
        &["hello from kelp (aarch64)"],
    );
}

// ---------------------------------------------------------------------------
// Kernel stage 20: the second ISA family. `[target] triple =
// "x86_64-unknown-none"` gets an X86 target machine and the x86-64 `_start`
// stub (the Xen PVH note, a GDT, a 1 GiB identity map of 2 MiB pages, PAE and
// `EFER.LME`, the far jump to `.code64`, the bss clear, SSE on, the entry
// call), and `examples/kernel0-x86_64` is the COM1 driver that boots on QEMU's
// `q35` machine. Freestanding is LLVM-only by design, so these build through
// the default backend; they need `ld.lld` and the llvm tools, and the boots
// need `qemu-system-x86_64`. `examples/kernel0-x86_64-lmode` is the second
// boot: it proves EFER.LMA and paging are on and then triple-faults on purpose.
// ---------------------------------------------------------------------------

/// The ELF half of the stage: the image is ELF64 x86-64 whose entry is the
/// linker script's 0x100000, carrying the `Xen` PVH note (type 18) whose
/// description is that same entry, referencing no undefined symbol, and whose
/// `_start` is real x86 code that halts. A boot that worked by accident on some
/// other entry point could not pass these.
#[test]
fn kernel0_x86_64_builds_an_x86_64_elf_with_the_pvh_note() {
    if !have_ld_lld() {
        println!(
            "skipping the x86-64 elf test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = build_freestanding("kernel0_x86_64", &fixture("../examples/kernel0-x86_64"));
    let path = bin.to_str().expect("the artifact path is utf-8");

    let header = tool("llvm-readelf", &["-h", path]);
    assert!(header.contains("ELF64"), "not an ELF64 image:\n{header}");
    assert!(
        header.contains("X86-64"),
        "not an x86-64 image:\n{header}"
    );
    let entry_line = header
        .lines()
        .find(|line| line.contains("Entry point address"))
        .unwrap_or_default();
    assert!(
        entry_line.contains("0x100000"),
        "the entry point is not the linker script's 0x100000:\n{header}"
    );

    // The PVH note: owner `Xen`, type 18 (printed in hex as 0x12), description
    // the 32-bit entry. Without it SeaBIOS has nothing to jump to.
    let notes = tool("llvm-readelf", &["-n", path]);
    assert!(notes.contains("Xen"), "no Xen PVH note in the image:\n{notes}");
    assert!(
        notes.contains("0x00000012"),
        "the Xen note is not XEN_ELFNOTE_PHYS32_ENTRY (type 18):\n{notes}"
    );
    assert!(
        notes.contains("00 00 10 00"),
        "the PVH note does not name the 0x100000 entry:\n{notes}"
    );

    // `llvm-objdump` is the disassembler half: the generated `_start` has to be
    // real x86 code, not an empty section.
    let disassembly = tool("llvm-objdump", &["-d", path]);
    assert!(
        disassembly.contains("_start"),
        "no _start in the image:\n{disassembly}"
    );
    assert!(
        disassembly.contains("hlt"),
        "the _start halt is missing:\n{disassembly}"
    );

    // `llvm-nm -u` prints what the linker had to leave for someone else: an
    // empty list is the whole claim that no libc symbol and no `kelp_rt` helper
    // is behind the image.
    let undefined = tool("llvm-nm", &["-u", path]);
    assert!(
        undefined.trim().is_empty(),
        "the x86-64 image has undefined symbols:\n{undefined}"
    );
}

/// The boot half: `examples/kernel0-x86_64` is a complete Kelp kernel that
/// boots on QEMU's `q35` machine through the Xen PVH path and writes one line
/// to the COM1 UART. The assertion is against what QEMU captured, so it proves
/// the boot, the linker script's stack and the driver's polling, not that the
/// source says `hello from kelp (x86_64)`.
#[test]
fn kernel0_x86_64_boots_and_prints_its_greeting_over_the_uart() {
    if !have_ld_lld() || !have_qemu(&X86_64_QEMU) {
        println!(
            "skipping the x86-64 boot test: 'ld.lld' or 'qemu-system-x86_64' is not on \
             PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    boot_under_qemu(
        &X86_64_QEMU,
        "kernel0_x86_64_boot",
        &fixture("../examples/kernel0-x86_64"),
        &["hello from kelp (x86_64)"],
    );
}

/// Kernel stage 23 on x86-64: the same `String`-field equality, `View` slice
/// and `std/regex` match as `examples/kernel0-eq`, built for
/// `x86_64-unknown-none` and booted through the Xen PVH note. Both halves of
/// the pair matter: the three bodies the compiler emits have to be
/// architecture-independent byte loops, not a riscv64-only lowering.
///
/// It needs `ld.lld` and `qemu-system-x86_64` from `dev-kernel.nix`.
#[test]
fn kernel0_eq_x86_64_builds_and_boots_the_same_lines() {
    if !have_ld_lld() {
        println!(
            "skipping the kernel0-eq x86-64 build test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = build_freestanding("kernel0_eq_x86_64", &fixture("../examples/kernel0-eq-x86_64"));
    let symbols = tool("llvm-nm", &[bin.to_str().unwrap()]);
    let undefined: Vec<&str> = symbols
        .lines()
        .filter(|line| line.split_whitespace().next() == Some("U"))
        .collect();
    assert!(
        undefined.is_empty(),
        "the kernel0-eq x86-64 image has undefined symbols:\n{}",
        undefined.join("\n")
    );

    if !have_qemu(&X86_64_QEMU) {
        println!(
            "skipping the kernel0-eq x86-64 boot: 'qemu-system-x86_64' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    boot_under_qemu(
        &X86_64_QEMU,
        "kernel0_eq_x86_64_boot",
        &fixture("../examples/kernel0-eq-x86_64"),
        &[
            "eq same: yes",
            "eq differ: yes",
            "map keys: 2",
            "map hit: yes",
            "bytes eq: yes",
            "view slice: kelp",
            "view eq: yes",
            "regex match: hello42",
            "regex group: 42",
            "balanced",
        ],
    );
}

/// The second boot: `examples/kernel0-x86_64-lmode` reads CR0/CR4/EFER through
/// `asm` and prints `long mode on` only when EFER.LMA, CR0.PG and CR4.PAE are
/// all set, so the greeting plus that line proves the stub really entered long
/// mode with paging. It then loads a null IDT and executes `ud2`; with no
/// descriptor the #UD is a triple fault, and `-no-reboot` turns QEMU's reset
/// into an exit the test reads. The assertion is still only the two serial
/// lines: the fault after them is the proof, not a line.
#[test]
fn kernel0_x86_64_lmode_proves_long_mode_then_triple_faults() {
    if !have_ld_lld() || !have_qemu(&X86_64_QEMU) {
        println!(
            "skipping the x86-64 long-mode boot test: 'ld.lld' or \
             'qemu-system-x86_64' is not on PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    boot_under_qemu(
        &X86_64_QEMU,
        "kernel0_x86_64_lmode_boot",
        &fixture("../examples/kernel0-x86_64-lmode"),
        &["hello from kelp (x86_64)", "long mode on"],
    );
}

// ---------------------------------------------------------------------------
// Kernel stage 22: the x86-64 IDT, the CPU exceptions and the PIC+PIT timer.
// `examples/kernel0-x86_64-idt` is the third x86-64 boot: it loads an IDT whose
// gates are naked functions, resumes from a `ud2` and a divide by zero, takes
// five timer interrupts, and ends in a store through an unmapped page whose
// `#PF` handler prints and halts. Freestanding is LLVM-only by design, so these
// build through the default backend; the boot needs `qemu-system-x86_64` and
// `ld.lld` from `dev-kernel.nix`.
// ---------------------------------------------------------------------------

/// The ELF half of the stage: the five gates are functions, each of them
/// 16-byte aligned (the `align(16)` clause, so an IDT entry never points into
/// the middle of another gate), and the image references no undefined symbol,
/// which is the whole claim that no libc helper sits behind the report or the
/// dispatcher.
#[test]
fn kernel0_x86_64_idt_builds_five_aligned_gates_with_no_undefined_symbols() {
    if !have_ld_lld() {
        println!(
            "skipping the x86-64 idt elf test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = build_freestanding(
        "kernel0_x86_64_idt",
        &fixture("../examples/kernel0-x86_64-idt"),
    );
    let path = bin.to_str().expect("the artifact path is utf-8");

    let symbols = tool("llvm-nm", &[path]);
    for gate in [
        "kelp_idt__gate_de",
        "kelp_idt__gate_ud",
        "kelp_idt__gate_gp",
        "kelp_idt__gate_pf",
        "kelp_idt__gate_timer",
    ] {
        let address = nm_address(&symbols, gate);
        assert_eq!(
            address % 16,
            0,
            "'{gate}' is at {address:#x}, which is not 16-byte aligned:\n{symbols}"
        );
    }

    let undefined = tool("llvm-nm", &["-u", path]);
    assert!(
        undefined.trim().is_empty(),
        "the x86-64 idt image has undefined symbols:\n{undefined}"
    );
}

/// The boot half: the serial lines have to carry, in this order, the `#UD` and
/// the divide by zero (both reported and both resumed, which the ticks after
/// them prove), the five timer ticks, the boot's own `x86: done`, and the
/// `#PF` on the unmapped store with no repair behind it, so it is the last
/// line. The `rip` in a report is the address the linker chose, so a report is
/// matched by shape; the error code and the `cr2` are pinned, because a
/// supervisor write to a not-present page is error code 2 and the address is
/// the one `main.kp` names.
#[test]
fn kernel0_x86_64_idt_takes_its_faults_and_ticks_the_timer_in_order() {
    if !have_ld_lld() || !have_qemu(&X86_64_QEMU) {
        println!(
            "skipping the x86-64 idt boot test: 'ld.lld' or 'qemu-system-x86_64' \
             is not on PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    boot_under_qemu_in_order(
        &X86_64_QEMU,
        "kernel0_x86_64_idt_boot",
        &fixture("../examples/kernel0-x86_64-idt"),
        &[
            "exception 6 err 0 rip 0x",
            "exception 0 err 0 rip 0x",
            "tick 1",
            "tick 2",
            "tick 3",
            "tick 4",
            "tick 5",
            "x86: done",
            "exception 14 err 2 rip 0x",
            "cr2 0x0000000200000000",
        ],
    );
}

/// Kernel stage 22's layout contract: the gates hardcode the `TrapFrame`
/// offsets and the offset split an IDT entry wants, and the compiler checks
/// neither end against the other. `tests/projects/x86_idt_layout` copies the
/// example's declarations and prints the numbers: the gate is 16 bytes with
/// its offset at 0/6/8, the `lidt` descriptor is ten bytes with the limit at 0
/// and the base at 2, and the frame is 160 bytes with the vector at 0, the
/// registers from 8 to 120, the error code at 128 and `RIP`/`CS`/`RFLAGS` at
/// 136/144/152.
#[test]
fn the_x86_idt_gate_and_frame_offsets_are_what_the_gates_hardcode() {
    assert_project_runs(
        "x86_idt_layout",
        &project("x86_idt_layout"),
        "16\n0\n2\n4\n5\n6\n8\n12\n10\n0\n2\n160\n0\n8\n120\n128\n136\n144\n152\n",
    );
}

/// Builds `source` with a `cc` shim first on `PATH` that records every
/// invocation's arguments, and returns the recorded lines.
fn build_recording_cc_lines(source: &str) -> Vec<String> {
    use std::os::unix::fs::PermissionsExt;

    let real_cc = String::from_utf8(
        Command::new("sh")
            .args(["-c", "command -v cc"])
            .output()
            .expect("failed to look up cc")
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    assert!(!real_cc.is_empty(), "no cc on PATH");

    let name = Path::new(source).file_stem().unwrap().to_string_lossy().to_string();
    let bin = out_path(&name);
    let dir = bin.with_extension("shim");
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("cc.log");
    let _ = std::fs::remove_file(&log);
    let shim = dir.join("cc");
    std::fs::write(
        &shim,
        format!(
            "#!/bin/sh\necho \"$@\" >> '{}'\nexec '{real_cc}' \"$@\"\n",
            log.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:{}", dir.display(), std::env::var("PATH").unwrap_or_default());

    let build = kelpc()
        .env("PATH", path)
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
    std::fs::read_to_string(&log)
        .expect("the cc shim was never called")
        .lines()
        .map(str::to_string)
        .collect()
}

/// BUG-x7Qmd: a program that does not reach `std/tls` neither links OpenSSL nor
/// includes its headers, so it builds on a host with no OpenSSL development
/// files. A program that does reach it links both libraries.
#[test]
fn only_a_tls_program_links_openssl() {
    let plain = build_recording_cc_lines("tests/fixtures/fs_path_family.kp");
    assert!(!plain.is_empty());
    for line in &plain {
        assert!(
            !line.contains("-lssl") && !line.contains("-lcrypto"),
            "a program without std/tls linked OpenSSL: {line}"
        );
    }
    let name = "fs_path_family";
    let generated = out_path(name);
    let source = std::fs::read_to_string(if backend() == "c" {
        generated.with_extension("c")
    } else {
        generated.with_extension("externs.c")
    })
    .expect("the generated C source");
    assert!(
        !source.contains("#include <openssl/"),
        "a program without std/tls includes OpenSSL's headers"
    );

    let tls = build_recording_cc_lines("tests/fixtures/https_try_dial.kp");
    assert!(
        tls.iter().any(|l| l.contains("-lssl") && l.contains("-lcrypto")),
        "a program that reaches std/tls did not link OpenSSL: {tls:?}"
    );
}

// ---------------------------------------------------------------------------
// Kernel stage 13 (MK9-S5-M3): the fallible `Ptr.try_alloc` and the shipped
// `std/arena`. The hosted halves run on both backends (each run of the suite is
// pointed at one); the freestanding halves are LLVM-only by design and need
// `ld.lld`, `llvm-nm` and `qemu-system-riscv64` from `dev-kernel.nix`.
// ---------------------------------------------------------------------------

/// The arena's hosted behaviour: the returned addresses are aligned (`base` is a
/// plain array with no alignment of its own, so the arena has to account for
/// where it starts), a request that does not fit and a size whose aligned end
/// would wrap both answer the null pointer, and `reset` makes the region
/// reusable. No `unsafe` is written in the fixture, so both backends build it.
#[test]
fn an_arena_bumps_with_alignment_and_reuses_after_reset() {
    assert_compiles_and_runs(
        "tests/fixtures/arena_bump.kp",
        "false\nfalse\nfalse\ntrue\ntrue\ntrue\ntrue\ntrue\ntrue\ntrue\ntrue\n0\ntrue\nfalse\n",
    );
}

/// A non-power-of-two `align` is refused at the call with the arena's own
/// message rather than handing back a misaligned pointer.
#[test]
fn an_arena_rejects_a_non_power_of_two_alignment() {
    assert_fixture_panics("arena_bad_align", "arena align must be a power of two");
}

/// The arena over real memory: both pointers are written and read back, and
/// `alloc_zeroed` clears a block the previous user wrote. `main` is on the
/// audit list because the writes go through a raw `Ptr<u8>`.
#[test]
fn an_arena_hands_out_writable_aligned_memory_and_zeroes_on_request() {
    assert_project_runs(
        "arena_memory",
        &project("arena_memory"),
        "170\n187\ntrue\ntrue\n0\n0\ntrue\n",
    );
}

/// `Ptr.try_alloc` is the non-panicking twin: a small request is non-null and
/// writable, and a count whose byte request no allocator can honour is the null
/// pointer instead of a call to the panic handler.
#[test]
fn try_alloc_is_fallible_where_alloc_would_panic() {
    assert_project_runs(
        "ptrs_try_alloc",
        &project("ptrs_try_alloc"),
        "false\n42\ntrue\nfalse\n",
    );
}

/// `Ptr.try_alloc` carries the same `unsafe` requirement as `Ptr.alloc`, so a
/// call outside any block is refused by name even in a module the manifest
/// lists.
#[test]
fn try_alloc_outside_unsafe_is_refused() {
    assert_project_rejected(
        "ptr_try_alloc_outside_unsafe",
        &project("ptr_try_alloc_outside_unsafe"),
        "'Ptr.try_alloc' is an unsafe operation",
    );
}

/// In a freestanding build the hook behind `Ptr.try_alloc` is the project's own
/// allocator, so without a `[target] alloc`/`dealloc` pair the call is refused
/// with the two keys that would make it available.
#[test]
fn try_alloc_without_a_freestanding_allocator_is_refused() {
    assert_project_rejected(
        "freestanding_try_alloc_no_alloc",
        &project("freestanding_try_alloc_no_alloc"),
        "'[target] alloc' and '[target] dealloc'",
    );
}

/// The freestanding arena's build claim: `std/arena` and its `unsafe` block
/// compile into a riscv64 image that references no undefined symbol, and the
/// project configures no allocator at all, which is what proves the module is
/// pure pointer arithmetic. Needs `ld.lld` and `llvm-nm`.
#[test]
fn freestanding_arena_builds_with_no_undefined_symbols() {
    if !have_ld_lld() {
        println!(
            "skipping the freestanding arena build test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = out_path("freestanding_arena");
    let build = Command::new(kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .env_remove("KELPC_ORACLE")
        .arg("build")
        .arg(project("freestanding_arena"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the freestanding arena build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let path = bin.to_str().expect("the artifact path is utf-8");
    let symbols = tool("llvm-nm", &[path]);
    let undefined: Vec<&str> = symbols
        .lines()
        .filter(|line| line.split_whitespace().next() == Some("U"))
        .collect();
    assert!(
        undefined.is_empty(),
        "the freestanding arena image has undefined symbols:\n{}",
        undefined.join("\n")
    );
}

/// The freestanding arena's boot: a `static mut [u8; 4096]` region carved into
/// aligned allocations, written and read through the returned pointers, with
/// the exhaustion and overflow answers, all read from QEMU's own serial output.
#[test]
fn freestanding_arena_boots_and_prints_its_lines() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU) {
        println!(
            "skipping the freestanding arena boot test: 'ld.lld' or 'qemu-system-riscv64' is \
             not on PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    boot_under_qemu(
        &RISCV64_QEMU,
        "freestanding_arena",
        &project("freestanding_arena"),
        &[
            "p0 aligned 1",
            "p1 aligned 1",
            "ordered 1",
            "used seen 1",
            "remain seen 1",
            "read 170",
            "exhaust 1",
            "wrap 1",
            "reset 0",
            "zero 0",
        ],
    );
}

/// The freestanding `try_alloc`'s build claim: with `[target] alloc`/`dealloc`
/// naming the project's own bump allocator, no libc `malloc` is left behind.
#[test]
fn freestanding_try_alloc_builds_with_no_undefined_symbols() {
    if !have_ld_lld() {
        println!(
            "skipping the freestanding try_alloc build test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = out_path("freestanding_try_alloc");
    let build = Command::new(kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .env_remove("KELPC_ORACLE")
        .arg("build")
        .arg(project("freestanding_try_alloc"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the freestanding try_alloc build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let path = bin.to_str().expect("the artifact path is utf-8");
    let symbols = tool("llvm-nm", &[path]);
    let undefined: Vec<&str> = symbols
        .lines()
        .filter(|line| line.split_whitespace().next() == Some("U"))
        .collect();
    assert!(
        undefined.is_empty(),
        "the freestanding try_alloc image has undefined symbols:\n{}",
        undefined.join("\n")
    );
}

/// The freestanding `try_alloc`'s boot: a normal request comes back non-null,
/// aligned and writable through the project allocator, and an absurd count comes
/// back null. QEMU's serial output is the assertion, so this proves the hook was
/// actually called.
#[test]
fn freestanding_try_alloc_boots_and_prints_its_lines() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU) {
        println!(
            "skipping the freestanding try_alloc boot test: 'ld.lld' or 'qemu-system-riscv64' \
             is not on PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    boot_under_qemu(
        &RISCV64_QEMU,
        "freestanding_try_alloc",
        &project("freestanding_try_alloc"),
        &["ptr 0", "align 0", "val 1234", "huge 1"],
    );
}

// ---------------------------------------------------------------------------
// Kernel stage 14 (MK9-S7-M2), the stack half: the freestanding `probe-stack`
// attribute and the guard page. The first test is the object-only claim that a
// frame larger than a page carries the inline probe; the second is the boot
// that overflows a small frame and a big one into the same unmapped page.
// ---------------------------------------------------------------------------

/// Parses a `0x`-prefixed hex word.
fn parse_hex_word(text: &str) -> Option<usize> {
    usize::from_str_radix(text.strip_prefix("0x")?, 16).ok()
}

/// Kernel stage 14's object-only test: `examples/kernel0-stack`'s
/// `recurse_big` holds a `[u8; 16384]` local, and the freestanding build's
/// `"probe-stack"="inline-asm"` attribute has to lower its prologue to a page
/// probe rather than one `sub sp, sp, 16400`. The assertion is on
/// `llvm-objdump`: at least three probe stores, each preceded by a
/// page-sized subtraction, which is what lets the guard page catch the frame
/// instead of the frame stepping over it.
///
/// It needs `ld.lld` and `llvm-objdump` from `dev-kernel.nix`.
#[test]
fn freestanding_big_frames_probe_every_page() {
    if !have_ld_lld() {
        println!(
            "skipping the freestanding probe test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = out_path("kernel0_stack_probe");
    let build = Command::new(kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .env_remove("KELPC_ORACLE")
        .arg("build")
        .arg(fixture("../examples/kernel0-stack"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the kernel0-stack build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let path = bin.to_str().expect("the artifact path is utf-8");

    // The lowering carries the attribute, so the check has two halves: the IR
    // says the function was marked, and the object shows the probe it lowers to.
    let ir = std::fs::read_to_string(bin.with_extension("ll")).expect("the .ll artifact");
    assert!(
        ir.contains("\"probe-stack\"=\"inline-asm\""),
        "the freestanding IR carries no probe-stack attribute"
    );

    let disassembly = tool(
        "llvm-objdump",
        &["-d", "--disassemble-symbols=kelp_recurse_big", path],
    );
    // `llvm-objdump` prints the displacement as `0(sp)` or `0x0(sp)` depending
    // on the build, so the probe is matched by its operands rather than one
    // exact spelling.
    let is_probe = |line: &str| {
        line.contains("\tsd\t") && line.contains("zero, 0") && line.contains("sp)")
    };
    let probes = disassembly.lines().filter(|line| is_probe(line)).count();
    assert!(
        probes >= 3,
        "kelp_recurse_big's probe is missing: its disassembly has {probes} probe stores:\n{disassembly}"
    );
    // A single `sub sp, sp, 16400` with no probe store would mean the 16 KiB
    // frame stepped over the guard in one instruction.
    assert!(
        disassembly.lines().filter(|line| line.contains("sub\tsp, sp,")).count() >= 3,
        "kelp_recurse_big builds its frame in fewer than three page steps:\n{disassembly}"
    );
}

/// Kernel stage 14's acceptance boot: `examples/kernel0-stack` runs two phases
/// over one trap vector. Phase a recurses with a 16-byte frame until the frame
/// that crosses `__stack_bottom` stores into the guard page; phase b is entered
/// by the handler, which restored `sp` to the top of the kernel stack, and its
/// one 16 KiB frame probes down into the same page. The handler prints the
/// `scause` and `stval` of each fault and says so, so the test can assert both
/// addresses are inside the guard page the boot printed.
///
/// It needs `ld.lld` and `qemu-system-riscv64` from `dev-kernel.nix`.
#[test]
fn kernel0_stack_overflows_into_the_guard_page() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU) {
        println!(
            "skipping the kernel0-stack boot test: 'ld.lld' or 'qemu-system-riscv64' is not \
             on PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let captured = boot_and_capture(
        &RISCV64_QEMU,
        "kernel0_stack",
        &fixture("../examples/kernel0-stack"),
        &|got| got.contains("stack: halted"),
    );
    for line in [
        "stack: m-mode",
        "stack: paging on",
        "stack: phase a",
        "stack: stval in guard",
        "stack: phase b",
        "stack: stval in guard",
        "stack: halted",
    ] {
        assert!(
            captured.lines().any(|got| got == line),
            "the serial output of kernel0-stack has no line '{line}':\n{captured}"
        );
    }

    // The kernel's own `in guard` line is one check; this is the independent
    // one: parse the guard bounds the boot printed and the two `stval`s, and
    // require each fault address to lie in the hole.
    let bounds: Vec<usize> = captured
        .lines()
        .find(|line| line.starts_with("stack: guard "))
        .and_then(|line| {
            let words: Vec<&str> = line.split_whitespace().collect();
            Some(vec![parse_hex_word(words.get(2)?)?, parse_hex_word(words.get(3)?)?])
        })
        .expect("the boot printed no `stack: guard <lo> <hi>` line");
    let (lo, hi) = (bounds[0], bounds[1]);
    let faults: Vec<usize> = captured
        .lines()
        .filter(|line| line.starts_with("stack overflow: scause "))
        .filter_map(|line| {
            let words: Vec<&str> = line.split_whitespace().collect();
            let at = words.iter().position(|w| *w == "stval")?;
            parse_hex_word(words.get(at + 1)?)
        })
        .collect();
    assert_eq!(
        faults.len(),
        2,
        "expected one fault per phase, got {}:\n{captured}",
        faults.len()
    );
    for fault in &faults {
        assert!(
            *fault >= lo && *fault < hi,
            "a fault at {fault:#x} is outside the guard page [{lo:#x}, {hi:#x}):\n{captured}"
        );
    }
}

/// Kernel stage 14's other half of "hosted builds are unchanged": a hosted
/// function carries no `probe-stack` attribute, so the probe is a freestanding
/// lowering and not a change to every build. The IR is the LLVM backend's
/// artifact, so this check is LLVM-only.
#[test]
fn hosted_functions_carry_no_stack_probe() {
    if backend() == "c" {
        println!("skipping the hosted probe check: the C backend writes no IR artifact");
        return;
    }
    let bin = out_path("hosted_no_probe");
    let build = Command::new(kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .env_remove("KELPC_ORACLE")
        .arg("build")
        .arg(fixture("tests/fixtures/array_basic.kp"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the hosted build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let ir = std::fs::read_to_string(bin.with_extension("ll")).expect("the .ll artifact");
    assert!(
        !ir.contains("probe-stack"),
        "a hosted build carries the freestanding probe-stack attribute"
    );
}

// ---------------------------------------------------------------------------
// Roadmap A-14 slice 1: separate compilation (`--incremental`)
// ---------------------------------------------------------------------------
//
// With the mode off the backend emits one LLVM module named `kelp`, which is why
// nothing else in this file mentions it: the whole suite is the mode-off
// assertion. These tests are the other half. The same programs built with the
// per-unit split must behave identically, the split must be reproducible, and
// the two spellings that turn it on (a flag and a manifest key) must agree.

/// Builds `target` twice, once whole-program and once with `--incremental`, and
/// reports the first difference (empty when the two agree). `KELPC_JOBS` is left
/// at its default, which is the mode an ordinary incremental build uses.
///
/// A target that builds is run both ways and its stdout and exit status are
/// compared; a target the front end *rejects* has to be rejected identically,
/// down to the diagnostic, because the split is a backend change and the mode
/// may not change what the compiler says about a program it refuses.
fn incremental_matches_whole_program(tag: &str, target: &Path) -> Result<(), String> {
    incremental_matches_whole_program_at_jobs(tag, target, &[None])
}

/// [`incremental_matches_whole_program`] with an explicit job setting per
/// incremental build: `None` leaves `KELPC_JOBS` unset (the default), `Some(n)`
/// runs that build with `n` workers. The whole-program build is done once and
/// compared against each incremental build, so testing the parallel scheduler
/// does not rebuild the whole-program side for every job setting.
fn incremental_matches_whole_program_at_jobs(
    tag: &str,
    target: &Path,
    jobs: &[Option<&str>],
) -> Result<(), String> {
    let whole = out_path(&format!("whole_{tag}"));
    let whole_build = kelpc()
        .arg("build")
        .arg(target)
        .arg("-o")
        .arg(&whole)
        .output()
        .map_err(|e| format!("could not spawn kelpc: {e}"))?;
    for (index, job) in jobs.iter().enumerate() {
        let split = out_path(&format!("inc_{tag}_{index}"));
        let mut command = kelpc();
        command
            .env_remove("KELPC_JOBS")
            // The cache is off so each incremental build really lowers and
            // compiles its units: with it on, the second job setting would
            // only replay the first one's objects.
            .env("KELPC_CACHE", "off")
            .arg("build")
            .arg(target)
            .arg("-o")
            .arg(&split)
            .arg("--incremental");
        if let Some(count) = job {
            command.env("KELPC_JOBS", count);
        }
        let split_build = command
            .output()
            .map_err(|e| format!("could not spawn kelpc: {e}"))?;
        if !whole_build.status.success() || !split_build.status.success() {
            if whole_build.status.success() != split_build.status.success() {
                return Err(format!(
                    "one build succeeded and the other did not:\n--incremental stderr:\n{}",
                    String::from_utf8_lossy(&split_build.stderr)
                ));
            }
            if whole_build.stderr != split_build.stderr {
                return Err(format!(
                    "the two rejections differ:\n  whole-program: {}\n  --incremental: {}",
                    String::from_utf8_lossy(&whole_build.stderr),
                    String::from_utf8_lossy(&split_build.stderr)
                ));
            }
            continue;
        }
        let run = |bin: &Path| -> Result<(Option<i32>, Vec<u8>), String> {
            let out = Command::new(bin)
                .output()
                .map_err(|e| format!("could not run '{}': {e}", bin.display()))?;
            Ok((out.status.code(), out.stdout))
        };
        let (whole_status, whole_stdout) = run(&whole)?;
        let (split_status, split_stdout) = run(&split)?;
        if whole_status != split_status {
            return Err(format!(
                "exit status {whole_status:?} whole-program vs {split_status:?} --incremental"
            ));
        }
        if whole_stdout != split_stdout {
            return Err(format!(
                "stdout differs:\n  whole-program: {:?}\n  --incremental: {:?}",
                String::from_utf8_lossy(&whole_stdout),
                String::from_utf8_lossy(&split_stdout)
            ));
        }
    }
    Ok(())
}

/// A project target: the directory when it has a manifest, and its entry file
/// when it does not, which is the two shapes `kelpc build` takes.
fn incremental_project_target(name: &str) -> PathBuf {
    let dir = project(name);
    if dir.join("coral.toml").exists() {
        dir
    } else {
        dir.join("main.kp")
    }
}

/// A fixture that exercises one feature the split has to keep working: generics,
/// traits, closures, threads, the containers, an owning `Drop`, a passdown, a
/// global. Every one of them is built and run both ways by the test below.
const INCREMENTAL_FIXTURES: &[&str] = &[
    // generics: instances of a module's generic function belong to its unit
    "generic_explicit_args",
    "generic_function_basic",
    "generic_function_owned",
    "generic_method_basic",
    "generic_nested_inference",
    "generic_owned",
    "generic_recursive",
    "generic_type_basic",
    "scoped_block_generic",
    // traits: static dispatch, default bodies, supertraits, `&dyn` vtables
    "traits_datac_and_enum",
    "traits_default_body",
    "traits_dyn_builtin",
    "traits_dyn_default_super",
    "traits_dyn_dispatch",
    "traits_dyn_forward",
    "traits_dyn_mut",
    "traits_generic_datac_method",
    "traits_impl_for_builtin",
    "traits_self_in_signature",
    "traits_supertrait",
    // closures and function values: the `_support` shims and thunks
    "closure_basic",
    "closure_datac_borrow_param",
    "closure_env_wide_ret",
    "closure_move",
    "closure_two_captures",
    "closure_value_widening",
    "fn_literal",
    "fn_literal_capture",
    "fn_value",
    "fn_value_plain",
    "scoped_block_captures",
    "scoped_block_params",
    "trailing_block_capture",
    // threads and mutexes: the per-signature spawn body and the runtime records
    "mutex_into_inner",
    "mutex_list_push",
    "mutex_lock_value",
    "thread_current_id",
    "thread_fence",
    "thread_join_accepted",
    "thread_join_unit",
    "thread_spawn_accepted",
    "thread_spawn_closure",
    // the containers and the tagged union
    "box_basic",
    "box_owned_elements",
    "box_recursive",
    "enum_basic",
    "enum_in_containers",
    "enum_liveness",
    "enum_owned",
    "enum_recursive",
    "if_let_basic",
    "list_basic",
    "list_growth_and_rebuild",
    "list_nested_list",
    "list_owned",
    "list_slice_insert",
    "map_basic",
    "map_datac_key",
    "map_datac_value_clone",
    "map_enum_key",
    "map_growth_and_rehash",
    "map_nested_datac_key",
    "map_tombstones_and_reuse",
    "when_expr",
    // owning `Drop`s: the generated per-type drop in `_support`
    "drop_container",
    "drop_control_flow",
    "drop_declared_key",
    "drop_enum",
    "drop_map",
    "drop_order",
    // passdowns, atomics and the rest of the scalar surface
    "atomic_counter",
    "bitwise",
    "consts",
    "datac_basic",
    "endian_scalars",
    "for_loop",
    "passdowns",
    "pd_owned",
    "section_attribute",
    "string_build",
    "string_views",
    "stdlib_fmt",
    "stdlib_sort",
    "view",
    "while_accumulator",
];

/// The multi-module projects the split has to keep working: a cross-module call,
/// a cross-module `pub static` read, a cross-module type, an `impl`/alias graph,
/// a user `extern fun`, an `unsafe` module and a `section("...")` placement.
const INCREMENTAL_PROJECTS: &[&str] = &[
    "alias_cross_file",
    "alias_kpconf",
    "closure_datac_param",
    "incremental_units",
    "namespace",
    "ptrs_try_alloc",
    "same_name",
    "self_call",
    "shapes",
    "static_datac_array",
    "static_globals",
    "static_module",
    "static_private",
    "traits_cross",
    "traits_default_cross",
    "traits_third_module",
    "unsafe_ptr",
    "unsafe_unaligned",
    "visibility_import",
];

/// Equivalence with the parallel scheduler (roadmap A-14 slice 3): the same
/// corpus the serial test uses is built with `KELPC_JOBS=1` and `KELPC_JOBS=4`
/// and each split build is compared with the whole-program build, so thread
/// count cannot change stdout, exit status or a refusal. The existing
/// `incremental_builds_match_the_whole_program_build` below covers the default
/// worker count.
#[test]
fn parallel_and_serial_builds_match_the_whole_program_build() {
    let mut failures: Vec<String> = Vec::new();
    let jobs = [Some("1"), Some("4")];
    for name in INCREMENTAL_FIXTURES {
        let source = fixture(&format!("tests/fixtures/{name}.kp"));
        if let Err(why) = incremental_matches_whole_program_at_jobs(name, &source, &jobs) {
            failures.push(format!("{name}: {why}"));
        }
    }
    for name in INCREMENTAL_PROJECTS {
        let target = incremental_project_target(name);
        if let Err(why) = incremental_matches_whole_program_at_jobs(name, &target, &jobs) {
            failures.push(format!("project {name}: {why}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} targets differ with --incremental at jobs 1 or 4:\n{}",
        failures.len(),
        INCREMENTAL_FIXTURES.len() + INCREMENTAL_PROJECTS.len(),
        failures.join("\n")
    );
}

/// Equivalence (roadmap A-14 slice 1): a program built with the per-unit split
/// prints what the whole-program build prints and exits the same way. The list
/// is the features the split can break, one fixture each plus the multi-module
/// projects, and it runs on whatever backend the suite is pointed at: under the
/// C oracle the flag is ignored, which this test then proves by construction.
#[test]
fn incremental_builds_match_the_whole_program_build() {
    let mut failures: Vec<String> = Vec::new();
    for name in INCREMENTAL_FIXTURES {
        let source = fixture(&format!("tests/fixtures/{name}.kp"));
        if let Err(why) = incremental_matches_whole_program(name, &source) {
            failures.push(format!("{name}: {why}"));
        }
    }
    for name in INCREMENTAL_PROJECTS {
        let target = incremental_project_target(name);
        if let Err(why) = incremental_matches_whole_program(name, &target) {
            failures.push(format!("project {name}: {why}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} targets differ with --incremental:\n{}",
        failures.len(),
        INCREMENTAL_FIXTURES.len() + INCREMENTAL_PROJECTS.len(),
        failures.join("\n")
    );
}

/// Unit assignment (roadmap A-14 slice 1): a project that sets
/// `[build] incremental = true` is split into one unit per source module plus
/// `_support`, and the cross-unit call and `pub static` read in it still work.
/// The manifest is the turn-on here; `--incremental` is the other spelling and
/// the equivalence test covers it on every other target.
#[test]
fn a_manifest_turns_the_per_unit_split_on() {
    if backend() == "c" {
        println!("skipping: the C oracle keeps its one translation unit");
        return;
    }
    let bin = out_path("inc_manifest_units");
    let build = kelpc()
        .arg("build")
        .arg(project("incremental_units"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the manifest-enabled split failed to build:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    // One unit per module id, plus the one unit that owns the generated helpers.
    let units = bin.with_extension("units");
    for unit in ["main", "state", "_support"] {
        let ir = units.join(format!("{unit}.ll"));
        let object = units.join(format!("{unit}.o"));
        assert!(ir.exists(), "'{}' was not written", ir.display());
        assert!(object.exists(), "'{}' was not written", object.display());
    }
    // `main` is the entry module's unit, `state` owns the other module's
    // function, static and datac, and the two are linked into one binary.
    let main_ir = std::fs::read_to_string(units.join("main.ll")).expect("main.ll");
    assert!(
        main_ir.contains("define i32 @main("),
        "the entry module's unit does not define main"
    );
    let state_ir = std::fs::read_to_string(units.join("state.ll")).expect("state.ll");
    assert!(
        state_ir.contains("@kelp_state__LIMIT = constant"),
        "the owning unit does not define the static"
    );
    assert!(
        state_ir.contains("define i64 @kelp_state__double("),
        "the owning unit does not define the cross-unit callee"
    );
    assert!(
        main_ir.contains("@kelp_state__LIMIT = external") && !main_ir.contains("@kelp_state__LIMIT = constant"),
        "the user unit should only declare the static"
    );
    let run = Command::new(&bin).output().expect("failed to run the split binary");
    assert!(run.status.success(), "the split binary failed");
    assert_eq!(String::from_utf8_lossy(&run.stdout), "42\n3\n7\n");
}

/// Determinism (roadmap A-14 slice 1): the same sources produce byte-identical
/// unit IR, which is what a later slice caches objects by. `KELPC_DUMP_UNITS`
/// writes each unit's unoptimized IR to `<dir>/<unit>.ll`.
#[test]
fn two_incremental_builds_produce_identical_unit_ir() {
    if backend() == "c" {
        println!("skipping: the C oracle writes no unit IR");
        return;
    }
    let base = out_path("inc_determinism");
    let mut dumps = Vec::new();
    for pass in ["a", "b"] {
        let dir = base.with_extension(pass);
        let _ = std::fs::remove_dir_all(&dir);
        let bin = base.with_extension(format!("bin_{pass}"));
        let build = kelpc()
            .env("KELPC_DUMP_UNITS", &dir)
            .arg("build")
            .arg(project("shapes"))
            .arg("-o")
            .arg(&bin)
            .arg("--incremental")
            .output()
            .expect("failed to spawn kelpc");
        assert!(
            build.status.success(),
            "the determinism build failed:\n{}",
            String::from_utf8_lossy(&build.stderr)
        );
        dumps.push(dir);
    }
    let read_dir = |dir: &Path| -> std::collections::BTreeMap<String, Vec<u8>> {
        std::fs::read_dir(dir)
            .unwrap_or_else(|e| panic!("could not read '{}': {e}", dir.display()))
            .map(|entry| {
                let entry = entry.expect("a readable directory entry");
                (
                    entry.file_name().to_string_lossy().into_owned(),
                    std::fs::read(entry.path()).expect("a readable unit IR file"),
                )
            })
            .collect()
    };
    let (a, b) = (read_dir(&dumps[0]), read_dir(&dumps[1]));
    assert!(
        !a.is_empty(),
        "no unit IR was dumped into '{}'",
        dumps[0].display()
    );
    assert_eq!(
        a.keys().collect::<Vec<_>>(),
        b.keys().collect::<Vec<_>>(),
        "the two builds wrote different unit sets"
    );
    for (name, first) in &a {
        assert_eq!(
            Some(first),
            b.get(name),
            "unit '{name}' differs between two builds of one project"
        );
    }
}

/// The freestanding refusal (roadmap A-14 slice 1): the mode is not implemented
/// for a bare-metal image yet, and it is refused by name rather than miscompiled
/// or silently ignored.
#[test]
fn incremental_is_refused_for_a_freestanding_project() {
    if backend() == "c" {
        println!("skipping: the C oracle refuses a freestanding project already");
        return;
    }
    let bin = out_path("inc_freestanding");
    let build = kelpc()
        .arg("build")
        .arg(project("freestanding_min"))
        .arg("-o")
        .arg(&bin)
        .arg("--incremental")
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        !build.status.success(),
        "expected the freestanding split to be refused"
    );
    let stderr = String::from_utf8_lossy(&build.stderr);
    assert!(
        stderr.contains("'--incremental' is not implemented for a freestanding project"),
        "the refusal does not name the mode: {stderr}"
    );
}

/// The C oracle ignores the flag (roadmap A-14 slice 1): it is the suite's other
/// answer to the same question, so it stays one translation unit, and a build
/// with the flag writes the same `.c` artifact it always did.
#[test]
fn the_c_oracle_ignores_the_incremental_flag() {
    let bin = out_path("inc_oracle");
    let build = Command::new(kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .env("KELPC_ORACLE", "c")
        .arg("build")
        .arg(fixture("tests/fixtures/datac_basic.kp"))
        .arg("-o")
        .arg(&bin)
        .arg("--incremental")
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the oracle build with --incremental failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    assert!(
        bin.with_extension("c").exists(),
        "the oracle did not write its one generated C file"
    );
    assert!(
        !bin.with_extension("units").exists(),
        "the oracle split its single translation unit"
    );
    let run = Command::new(&bin).output().expect("failed to run the oracle binary");
    assert!(run.status.success());
    assert_eq!(String::from_utf8_lossy(&run.stdout), "3\n4\n25\n11\n");
}

// Roadmap A-14 slice 2: the unit object cache
// ---------------------------------------------------------------------------
//
// The cache stores a unit's object under the hash of its IR text and the
// machine settings it was compiled for (`plan/incremental-compilation.md`).
// These tests drive the real compiler and read its stderr: `KELPC_CACHE_STATS=1`
// prints one hit/miss/store line per build, and `--timings` prints one
// `hit`/`miss` line per unit. The C oracle ignores `--incremental`, so every
// test here is llvm-only and skips under the oracle.

/// One cache-enabled build's result, with the cache lines parsed out of stderr.
struct CacheBuild {
    ok: bool,
    stderr: String,
    hits: u64,
    misses: u64,
    stored: u64,
    units: Vec<(String, bool)>,
}

impl CacheBuild {
    /// Whether `unit` was a hit (`None` when the build listed no such unit).
    fn verdict(&self, unit: &str) -> Option<bool> {
        self.units
            .iter()
            .find(|(name, _)| name == unit)
            .map(|(_, hit)| *hit)
    }
}

/// The three counters on the `kelpc cache: H hits, M misses, S stored` line.
fn parse_cache_stats(stderr: &str) -> (u64, u64, u64) {
    for line in stderr.lines() {
        let Some(rest) = line.strip_prefix("kelpc cache: ") else {
            continue;
        };
        let numbers: Vec<u64> = rest
            .split_whitespace()
            .filter_map(|word| word.parse::<u64>().ok())
            .collect();
        assert_eq!(numbers.len(), 3, "unexpected cache line: {line}");
        return (numbers[0], numbers[1], numbers[2]);
    }
    panic!("no 'kelpc cache:' line in stderr:\n{stderr}");
}

/// The `unit NAME: N.N ms hit|miss` lines of a `--timings` build.
fn parse_unit_verdicts(stderr: &str) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    for line in stderr.lines() {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        if tokens.len() == 5 && tokens[0] == "unit" && tokens[3] == "ms" {
            let name = tokens[1].trim_end_matches(':').to_string();
            out.push((name, tokens[4] == "hit"));
        }
    }
    out
}

/// The three-module project the cache tests edit: `main` calls `alpha`, `alpha`
/// calls `beta`, so every unit is a real compilation unit and the generated
/// `_support` helper unit sits beside them.
fn cache_project(tag: &str) -> PathBuf {
    let dir = out_path(&format!("cacheproj_{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).expect("create the project directory");
    std::fs::write(
        dir.join("coral.toml"),
        "name = \"cacheproj\"\n\n[build]\nincremental = true\n",
    )
    .expect("write the manifest");
    std::fs::write(
        dir.join("src/main.kp"),
        "impl \"alpha\"\n\nfun main() {\n    print(alpha.greet())\n}\n",
    )
    .expect("write main");
    std::fs::write(
        dir.join("src/alpha.kp"),
        "impl \"beta\"\n\npub fun greet().return: str = beta.message()\n",
    )
    .expect("write alpha");
    std::fs::write(
        dir.join("src/beta.kp"),
        "pub fun message().return: str = \"hello\"\n",
    )
    .expect("write beta");
    dir
}

/// Rewrites module B's function body, which is the unit the tests make miss.
fn set_beta_message(dir: &Path, body: &str) {
    std::fs::write(
        dir.join("src/beta.kp"),
        format!("pub fun message().return: str = {body}\n"),
    )
    .expect("rewrite beta");
}

/// One `kelpc build --incremental --timings` against `cache`, with `flags` on
/// the command line and `envs` in the environment.
fn cache_build(
    target: &Path,
    bin: &Path,
    cache: &Path,
    flags: &[&str],
    envs: &[(&str, &str)],
) -> CacheBuild {
    let mut cmd = kelpc();
    cmd.env_remove("KELPC_CACHE")
        .env_remove("KELPC_CACHE_STATS")
        .env_remove("KELPC_CACHE_MAX_MB")
        .env_remove("KELPC_DUMP_UNITS")
        .arg("build")
        .arg(target)
        .arg("-o")
        .arg(bin)
        .arg("--incremental")
        .arg("--timings")
        .env("KELPC_CACHE_DIR", cache)
        .env("KELPC_CACHE_STATS", "1");
    for flag in flags {
        cmd.arg(flag);
    }
    for (key, value) in envs {
        cmd.env(key, value);
    }
    let out = cmd.output().expect("failed to spawn kelpc");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let (hits, misses, stored) = parse_cache_stats(&stderr);
    let units = parse_unit_verdicts(&stderr);
    CacheBuild {
        ok: out.status.success(),
        stderr,
        hits,
        misses,
        stored,
        units,
    }
}

/// Runs a built binary and returns its exit code and stdout.
fn run_cached_binary(bin: &Path) -> (Option<i32>, Vec<u8>) {
    let out = Command::new(bin)
        .output()
        .unwrap_or_else(|e| panic!("could not run '{}': {e}", bin.display()));
    (out.status.code(), out.stdout)
}

/// The `<key>.o` entries in a cache directory, sorted by name.
fn cache_entries(cache: &Path) -> Vec<PathBuf> {
    let Ok(read) = std::fs::read_dir(cache) else {
        return Vec::new();
    };
    let mut names: Vec<String> = read
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".o"))
        .collect();
    names.sort();
    names
        .into_iter()
        .map(|name| cache.join(name))
        .collect()
}

/// The generated compiler-owned module contracts in a cache directory.
fn interface_entries(cache: &Path) -> Vec<PathBuf> {
    let Ok(read) = std::fs::read_dir(cache) else {
        return Vec::new();
    };
    let mut names: Vec<String> = read
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".kpi"))
        .collect();
    names.sort();
    names.into_iter().map(|name| cache.join(name)).collect()
}

/// Whether `path` holds a complete ELF object, the property a partial cache
/// entry fails. A concurrent build must leave only such files behind.
fn is_complete_elf(path: &Path) -> bool {
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    if bytes.len() < 64 || &bytes[..4] != b"\x7fELF" || bytes[4] != 2 {
        return false;
    }
    let word = |at: usize| {
        u64::from_le_bytes([
            bytes[at],
            bytes[at + 1],
            bytes[at + 2],
            bytes[at + 3],
            bytes[at + 4],
            bytes[at + 5],
            bytes[at + 6],
            bytes[at + 7],
        ])
    };
    let half = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]) as u64;
    let shoff = word(0x28);
    let shentsize = half(0x3a);
    let shnum = half(0x3c);
    shoff != 0 && shentsize != 0 && shnum != 0 && bytes.len() as u64 >= shoff + shentsize * shnum
}

#[test]
fn incremental_builds_write_generated_module_interfaces() {
    if backend() == "c" {
        println!("skipping: the C oracle ignores --incremental");
        return;
    }
    let dir = cache_project("interfaces");
    let cache = out_path("cache_interfaces");
    let bin = out_path("cache_interfaces_bin");
    let build = cache_build(&dir, &bin, &cache, &[], &[]);
    assert!(build.ok, "the incremental build failed:\n{}", build.stderr);
    let entries = interface_entries(&cache);
    assert!(!entries.is_empty(), "the build wrote no interface artifacts");
    for entry in entries {
        let text = std::fs::read_to_string(&entry).expect("read interface artifact");
        assert!(text.starts_with("kelpc-front-end-interface-1\nmodule="), "bad interface artifact '{}': {text}", entry.display());
    }
}

#[test]
fn timings_report_front_end_interface_hits_and_misses() {
    if backend() == "c" {
        println!("skipping: the C oracle ignores --incremental");
        return;
    }
    let dir = cache_project("iface_timings");
    let cache = out_path("cache_iface_timings");
    let _ = std::fs::remove_dir_all(&cache);
    let bin = out_path("cache_iface_timings_bin");
    let line = |stderr: &str| {
        stderr
            .lines()
            .find(|l| l.contains("front-end interfaces:"))
            .map(|l| l.trim().to_string())
            .unwrap_or_default()
    };
    let first = cache_build(&dir, &bin, &cache, &[], &[]);
    assert!(first.ok, "{}", first.stderr);
    assert!(line(&first.stderr).contains(" 0 hit,"), "{}", first.stderr);
    let second = cache_build(&dir, &bin, &cache, &[], &[]);
    assert!(second.ok, "{}", second.stderr);
    assert!(line(&second.stderr).contains(" 0 miss"), "{}", second.stderr);
}

#[test]
fn an_unchanged_project_is_all_hits_on_the_second_build() {
    if backend() == "c" {
        println!("skipping: the C oracle ignores --incremental");
        return;
    }
    let dir = cache_project("unchanged");
    let cache = out_path("cache_unchanged");
    let _ = std::fs::remove_dir_all(&cache);
    let first_bin = out_path("cache_unchanged_1");
    let second_bin = out_path("cache_unchanged_2");
    let first = cache_build(&dir, &first_bin, &cache, &[], &[]);
    assert!(first.ok, "the first build failed:\n{}", first.stderr);
    assert_eq!(first.hits, 0);
    assert_eq!(first.misses, first.units.len() as u64);
    assert_eq!(first.stored, first.misses);
    let second = cache_build(&dir, &second_bin, &cache, &[], &[]);
    assert!(second.ok, "the second build failed:\n{}", second.stderr);
    assert_eq!(
        (second.hits, second.misses, second.stored),
        (first.units.len() as u64, 0, 0),
        "an unchanged project did not build from the cache alone"
    );
    assert!(
        second.units.iter().all(|(_, hit)| *hit),
        "not every unit was a hit: {:?}",
        second.units
    );
    let (code1, stdout1) = run_cached_binary(&first_bin);
    let (code2, stdout2) = run_cached_binary(&second_bin);
    assert_eq!(code1, code2, "the cached build exited differently");
    assert_eq!(stdout1, stdout2, "the cached build printed differently");
    assert_eq!(String::from_utf8_lossy(&stdout2), "hello\n");
    incremental_matches_whole_program("cache_unchanged", &dir).unwrap();
}

#[test]
fn changing_one_body_misses_only_its_unit() {
    if backend() == "c" {
        println!("skipping: the C oracle ignores --incremental");
        return;
    }
    let dir = cache_project("onebody");
    let cache = out_path("cache_onebody");
    let _ = std::fs::remove_dir_all(&cache);
    let dump_before = out_path("cache_onebody_ir1");
    let dump_after = out_path("cache_onebody_ir2");
    let _ = std::fs::remove_dir_all(&dump_before);
    let _ = std::fs::remove_dir_all(&dump_after);
    let first = cache_build(
        &dir,
        &out_path("cache_onebody_1"),
        &cache,
        &[],
        &[("KELPC_DUMP_UNITS", dump_before.to_str().unwrap())],
    );
    assert!(first.ok, "the first build failed:\n{}", first.stderr);
    set_beta_message(&dir, "\"hello, cache\"");
    let second_bin = out_path("cache_onebody_2");
    let second = cache_build(
        &dir,
        &second_bin,
        &cache,
        &[],
        &[("KELPC_DUMP_UNITS", dump_after.to_str().unwrap())],
    );
    assert!(second.ok, "the second build failed:\n{}", second.stderr);
    assert_eq!(second.verdict("beta"), Some(false), "beta had to miss");
    assert_eq!(second.verdict("alpha"), Some(true), "alpha had to hit");
    assert_eq!(second.verdict("main"), Some(true), "main had to hit");
    // `_support` holds generated helpers and may or may not embed the changed
    // literal; its verdict has to follow its IR, which the dump records.
    let support_changed = std::fs::read(dump_before.join("_support.ll")).unwrap()
        != std::fs::read(dump_after.join("_support.ll")).unwrap();
    assert_eq!(
        second.verdict("_support"),
        Some(!support_changed),
        "the _support verdict did not follow its IR"
    );
    assert_eq!(
        second.misses,
        1 + u64::from(support_changed),
        "exactly the changed IR had to miss"
    );
    assert_eq!(
        second.hits,
        first.units.len() as u64 - second.misses,
        "the other units had to hit"
    );
    let (_, stdout) = run_cached_binary(&second_bin);
    assert_eq!(String::from_utf8_lossy(&stdout), "hello, cache\n");
    incremental_matches_whole_program("cache_onebody", &dir).unwrap();
}

#[test]
fn changing_a_callee_signature_misses_the_caller_too() {
    if backend() == "c" {
        println!("skipping: the C oracle ignores --incremental");
        return;
    }
    let dir = out_path("cache_signature");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).expect("create the project directory");
    std::fs::write(
        dir.join("coral.toml"),
        "name = \"cachesig\"\n\n[build]\nincremental = true\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.kp"),
        "impl \"alpha\"\n\nfun main() {\n    print(alpha.greet())\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/alpha.kp"),
        "impl \"beta\"\n\npub fun greet().return: i64 = beta.value()\n",
    )
    .unwrap();
    std::fs::write(dir.join("src/beta.kp"), "pub fun value().return: i64 = 41\n").unwrap();
    let cache = out_path("cache_signature_cache");
    let _ = std::fs::remove_dir_all(&cache);
    let first = cache_build(&dir, &out_path("cache_signature_1"), &cache, &[], &[]);
    assert!(first.ok, "the first build failed:\n{}", first.stderr);
    // The signature changes and B's body with it; A's source is rewritten only
    // because the call now takes the argument, and its own unit misses because
    // every unit declares every program item.
    std::fs::write(
        dir.join("src/alpha.kp"),
        "impl \"beta\"\n\npub fun greet().return: i64 = beta.value(0)\n",
    )
    .unwrap();
    std::fs::write(dir.join("src/beta.kp"), "pub fun value(x: i64).return: i64 = x + 41\n").unwrap();
    let second_bin = out_path("cache_signature_2");
    let second = cache_build(&dir, &second_bin, &cache, &[], &[]);
    assert!(second.ok, "the second build failed:\n{}", second.stderr);
    assert_eq!(second.verdict("beta"), Some(false), "beta had to miss");
    assert_eq!(
        second.verdict("alpha"),
        Some(false),
        "alpha had to miss with the callee's declaration"
    );
    // `main` is not asserted: every unit declares every program item, so a
    // callee's signature change is visible in each one's IR, which is the
    // documented cost of having no interface files.
    let (_, stdout) = run_cached_binary(&second_bin);
    assert_eq!(String::from_utf8_lossy(&stdout), "41\n");
    incremental_matches_whole_program("cache_signature", &dir).unwrap();
}

#[test]
fn different_optimization_levels_never_share_entries() {
    if backend() == "c" {
        println!("skipping: the C oracle ignores --incremental");
        return;
    }
    let dir = cache_project("optlevel");
    let cache = out_path("cache_optlevel");
    let _ = std::fs::remove_dir_all(&cache);
    let at_o0 = cache_build(&dir, &out_path("cache_opt_0"), &cache, &["-O0"], &[]);
    assert!(at_o0.ok, "the -O0 build failed:\n{}", at_o0.stderr);
    assert_eq!(at_o0.stored, at_o0.units.len() as u64);
    let at_o2 = cache_build(&dir, &out_path("cache_opt_2"), &cache, &["-O2"], &[]);
    assert!(at_o2.ok, "the -O2 build failed:\n{}", at_o2.stderr);
    assert_eq!(
        at_o2.hits, 0,
        "an -O2 build reused an -O0 entry:\n{}",
        at_o2.stderr
    );
    assert_eq!(at_o2.misses, at_o0.units.len() as u64);
    // The two levels' entries coexist, and each build's program is the same.
    let (_, o0_out) = run_cached_binary(&out_path("cache_opt_0"));
    let (_, o2_out) = run_cached_binary(&out_path("cache_opt_2"));
    assert_eq!(o0_out, o2_out);
    incremental_matches_whole_program("cache_optlevel", &dir).unwrap();
}

#[test]
fn a_truncated_entry_is_a_miss_and_is_rewritten() {
    if backend() == "c" {
        println!("skipping: the C oracle ignores --incremental");
        return;
    }
    let dir = cache_project("truncate");
    let cache = out_path("cache_truncate");
    let _ = std::fs::remove_dir_all(&cache);
    let first = cache_build(&dir, &out_path("cache_truncate_1"), &cache, &[], &[]);
    assert!(first.ok, "the first build failed:\n{}", first.stderr);
    let entries = cache_entries(&cache);
    assert!(!entries.is_empty(), "the first build stored nothing");
    let victim = entries[0].clone();
    let full_len = std::fs::metadata(&victim).unwrap().len();
    assert!(full_len > 64);
    // Truncate to the 64-byte ELF header: the file is non-empty and still
    // starts with the magic, so only the section-header bounds check can see
    // that it is incomplete.
    let bytes = std::fs::read(&victim).unwrap();
    std::fs::write(&victim, &bytes[..64]).unwrap();
    let second = cache_build(&dir, &out_path("cache_truncate_2"), &cache, &[], &[]);
    assert!(second.ok, "the recovery build failed:\n{}", second.stderr);
    assert_eq!(second.misses, 1, "the truncated entry was not a miss");
    assert_eq!(second.stored, 1, "the miss was not stored again");
    assert!(
        std::fs::metadata(&victim).unwrap().len() >= full_len,
        "the truncated entry was not rewritten"
    );
    incremental_matches_whole_program("cache_truncate", &dir).unwrap();
}

#[test]
fn concurrent_builds_share_a_cache_without_partial_files() {
    if backend() == "c" {
        println!("skipping: the C oracle ignores --incremental");
        return;
    }
    let dir = cache_project("concurrent");
    let cache = out_path("cache_concurrent");
    let _ = std::fs::remove_dir_all(&cache);
    let spawn = |bin: &Path| {
        kelpc()
            .env_remove("KELPC_CACHE")
            .env_remove("KELPC_CACHE_STATS")
            .env("KELPC_CACHE_DIR", &cache)
            .arg("build")
            .arg(&dir)
            .arg("-o")
            .arg(bin)
            .arg("--incremental")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn kelpc")
    };
    let bin_a = out_path("cache_concurrent_a");
    let bin_b = out_path("cache_concurrent_b");
    let child_a = spawn(&bin_a);
    let child_b = spawn(&bin_b);
    let out_a = child_a.wait_with_output().expect("wait for the first build");
    let out_b = child_b.wait_with_output().expect("wait for the second build");
    assert!(
        out_a.status.success(),
        "the first concurrent build failed:\n{}",
        String::from_utf8_lossy(&out_a.stderr)
    );
    assert!(
        out_b.status.success(),
        "the second concurrent build failed:\n{}",
        String::from_utf8_lossy(&out_b.stderr)
    );
    let (_, stdout_a) = run_cached_binary(&bin_a);
    let (_, stdout_b) = run_cached_binary(&bin_b);
    assert_eq!(stdout_a, stdout_b);
    assert_eq!(String::from_utf8_lossy(&stdout_a), "hello\n");
    // Atomic writes mean the cache holds only complete entries and no
    // temporary file from either build.
    for entry in cache_entries(&cache) {
        let name = entry.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            name.ends_with(".o"),
            "'{name}' is not a cache entry name"
        );
        assert!(
            is_complete_elf(&entry),
            "'{name}' is not a complete object file"
        );
    }
    let leftovers: Vec<String> = std::fs::read_dir(&cache)
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with('.'))
        .collect();
    assert!(
        leftovers.is_empty(),
        "a concurrent build left temporary files: {leftovers:?}"
    );
}

#[test]
fn the_size_cap_evicts_the_oldest_entry() {
    if backend() == "c" {
        println!("skipping: the C oracle ignores --incremental");
        return;
    }
    let dir = cache_project("cap");
    let cache = out_path("cache_cap");
    let _ = std::fs::remove_dir_all(&cache);
    let first = cache_build(&dir, &out_path("cache_cap_1"), &cache, &[], &[]);
    assert!(first.ok, "the first build failed:\n{}", first.stderr);
    let entries = cache_entries(&cache);
    assert!(entries.len() >= 2, "the cap test needs more than one entry");
    // Give the entries distinct mtimes, earliest first, so the eviction order
    // is a fact rather than a tie the filesystem breaks differently.
    let base = std::time::SystemTime::now();
    let mut sizes = Vec::new();
    for (i, entry) in entries.iter().enumerate() {
        sizes.push(std::fs::metadata(entry).unwrap().len());
        std::fs::File::options()
            .write(true)
            .open(entry)
            .unwrap()
            .set_modified(base + Duration::from_secs(i as u64))
            .unwrap();
    }
    let total: u64 = sizes.iter().sum();
    let cap = total - sizes[0];
    let cap_mb = format!("{}", cap as f64 / (1024.0 * 1024.0));
    let second = cache_build(
        &dir,
        &out_path("cache_cap_2"),
        &cache,
        &[],
        &[("KELPC_CACHE_MAX_MB", cap_mb.as_str())],
    );
    assert!(second.ok, "the capped build failed:\n{}", second.stderr);
    assert_eq!(second.misses, 0, "the capped build recompiled a unit");
    assert!(
        !entries[0].exists(),
        "the oldest entry survived the size cap"
    );
    for entry in &entries[1..] {
        assert!(entry.exists(), "'{}' was evicted before the oldest", entry.display());
    }
    let remaining: u64 = entries[1..]
        .iter()
        .map(|entry| std::fs::metadata(entry).unwrap().len())
        .sum();
    assert!(remaining <= cap, "the cap was not enforced");
    incremental_matches_whole_program("cache_cap", &dir).unwrap();
}

#[test]
fn cache_off_writes_nothing() {
    if backend() == "c" {
        println!("skipping: the C oracle ignores --incremental");
        return;
    }
    let dir = cache_project("off");
    let cache = out_path("cache_off");
    let _ = std::fs::remove_dir_all(&cache);
    let bin = out_path("cache_off_bin");
    let build = cache_build(&dir, &bin, &cache, &[], &[("KELPC_CACHE", "off")]);
    assert!(build.ok, "the KELPC_CACHE=off build failed:\n{}", build.stderr);
    assert!(
        !cache.exists(),
        "KELPC_CACHE=off created '{}'",
        cache.display()
    );
    assert_eq!(
        (build.hits, build.misses, build.stored),
        (0, 0, 0),
        "KELPC_CACHE=off still consulted the cache"
    );
    let (_, stdout) = run_cached_binary(&bin);
    assert_eq!(String::from_utf8_lossy(&stdout), "hello\n");
    incremental_matches_whole_program("cache_off", &dir).unwrap();
}

#[test]
fn the_cache_command_reports_and_clears() {
    if backend() == "c" {
        println!("skipping: the C oracle ignores --incremental");
        return;
    }
    let dir = cache_project("command");
    let cache = out_path("cache_command");
    let _ = std::fs::remove_dir_all(&cache);
    let first = cache_build(&dir, &out_path("cache_command_1"), &cache, &[], &[]);
    assert!(first.ok, "the first build failed:\n{}", first.stderr);
    let entries = cache_entries(&cache);
    assert!(!entries.is_empty());
    let stats = kelpc()
        .arg("cache")
        .arg("stats")
        .arg("--dir")
        .arg(&cache)
        .output()
        .expect("failed to spawn kelpc cache stats");
    assert!(stats.status.success());
    let stats_text = String::from_utf8_lossy(&stats.stdout);
    // The cache holds unit objects and generated interface artifacts.
    let total = entries.len() + interface_entries(&cache).len();
    assert!(
        stats_text.contains(&format!("{total} entries")),
        "kelpc cache stats did not report {total} entries: {stats_text}",
    );
    let clear = kelpc()
        .arg("cache")
        .arg("clear")
        .arg("--dir")
        .arg(&cache)
        .output()
        .expect("failed to spawn kelpc cache clear");
    assert!(clear.status.success());
    assert!(
        cache_entries(&cache).is_empty() && interface_entries(&cache).is_empty(),
        "kelpc cache clear left entries behind"
    );
    let second = cache_build(&dir, &out_path("cache_command_2"), &cache, &[], &[]);
    assert!(second.ok, "the rebuild after a clear failed:\n{}", second.stderr);
    assert_eq!(
        second.misses,
        first.units.len() as u64,
        "the cleared cache produced a hit"
    );
    assert_eq!(second.hits, 0);
}

// Roadmap A-14 slice 3: parallel unit compilation
// ---------------------------------------------------------------------------
//
// Cache misses are compiled on worker threads, one LLVM context per unit, and
// the results are linked in unit order. The tests here pin the three things
// that make that safe to ship: the scheduler does not change an object, a unit
// that fails names itself and does not discard the other units' cache entries,
// and the compile-time job-count parsing.

/// The files of a build's `<out>.units/` directory: every unit's IR and object,
/// keyed by file name so two builds can be compared entry by entry.
fn unit_dir_files(dir: &Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("could not read '{}': {e}", dir.display()))
        .map(|entry| {
            let entry = entry.expect("a readable directory entry");
            (
                entry.file_name().to_string_lossy().into_owned(),
                std::fs::read(entry.path()).expect("a readable unit artifact"),
            )
        })
        .collect()
}

/// Determinism (roadmap A-14 slice 3): the same sources built with one worker
/// and with four workers write byte-identical unit IR *and* byte-identical
/// objects, and the two linked binaries print the same thing. The cache is off
/// so both builds really lower and emit every unit rather than copying a stored
/// object.
#[test]
fn serial_and_parallel_builds_write_byte_identical_units() {
    if backend() == "c" {
        println!("skipping: the C oracle ignores --incremental");
        return;
    }
    let target = project("shapes");
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut bins: Vec<PathBuf> = Vec::new();
    let mut orders: Vec<Vec<String>> = Vec::new();
    for jobs in ["1", "4"] {
        let bin = out_path(&format!("inc_units_j{jobs}"));
        let build = kelpc()
            .env("KELPC_JOBS", jobs)
            .env("KELPC_CACHE", "off")
            .arg("build")
            .arg(&target)
            .arg("-o")
            .arg(&bin)
            .arg("--incremental")
            .arg("--timings")
            .output()
            .expect("failed to spawn kelpc");
        assert!(
            build.status.success(),
            "the jobs={jobs} build failed:\n{}",
            String::from_utf8_lossy(&build.stderr)
        );
        // One unit line per unit, in unit order, whatever the worker count:
        // the `--timings` readout is a property of the program, not of the
        // schedule.
        orders.push(
            parse_unit_verdicts(&String::from_utf8_lossy(&build.stderr))
                .into_iter()
                .map(|(name, _)| name)
                .collect(),
        );
        assert!(
            orders.last().map_or(false, |order| !order.is_empty()),
            "the jobs={jobs} build listed no units"
        );
        dirs.push(bin.with_extension("units"));
        bins.push(bin);
    }
    let (serial, parallel) = (unit_dir_files(&dirs[0]), unit_dir_files(&dirs[1]));
    assert!(!serial.is_empty(), "the build wrote no unit artifacts");
    assert_eq!(
        serial.keys().collect::<Vec<_>>(),
        parallel.keys().collect::<Vec<_>>(),
        "the two builds wrote different unit artifact sets"
    );
    for (name, first) in &serial {
        assert_eq!(
            Some(first),
            parallel.get(name),
            "'{name}' differs between jobs 1 and jobs 4"
        );
    }
    assert_eq!(orders[0], orders[1], "the two builds listed units in different orders");
    let run = |bin: &Path| Command::new(bin).output().expect("failed to run a split binary");
    let (one, four) = (run(&bins[0]), run(&bins[1]));
    assert_eq!(one.status.code(), four.status.code());
    assert_eq!(one.stdout, four.stdout);
    assert_eq!(String::from_utf8_lossy(&one.stdout), "12.566360\n6.000000\n50.265440\n28.274310\n");
}

/// A failing unit (roadmap A-14 slice 3): `KELPC_TEST_CORRUPT_UNIT` appends an
/// unterminated basic block to one unit's module, so LLVM's verifier rejects it
/// with a real LLVM-level error. The build has to fail naming that unit, and the
/// units that did compile have to keep their cache entries: a second build
/// without the hook misses only the failed unit and hits the rest.
#[test]
fn a_failed_unit_names_itself_and_leaves_the_others_cached() {
    if backend() == "c" {
        println!("skipping: the C oracle ignores --incremental");
        return;
    }
    let dir = cache_project("failedunit");
    let cache = out_path("cache_failedunit");
    let _ = std::fs::remove_dir_all(&cache);
    let failed_bin = out_path("cache_failedunit_bin");
    let failed = kelpc()
        .env("KELPC_JOBS", "4")
        .env("KELPC_CACHE_DIR", &cache)
        .env("KELPC_CACHE_STATS", "1")
        .env("KELPC_TEST_CORRUPT_UNIT", "alpha")
        .arg("build")
        .arg(&dir)
        .arg("-o")
        .arg(&failed_bin)
        .arg("--incremental")
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        !failed.status.success(),
        "the corrupted unit built successfully"
    );
    assert_eq!(failed.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&failed.stderr);
    assert!(
        stderr.contains("compilation unit 'alpha' failed"),
        "the failure did not name the corrupt unit: {stderr}"
    );
    // A real LLVM-level error, not a front-end refusal: the verifier's own
    // message for the unterminated block is in the diagnostic.
    assert!(
        stderr.contains("does not have terminator"),
        "the failure is not the LLVM verifier's: {stderr}"
    );
    // The other three units stored their entries even though the build failed.
    let (_, misses, stored) = parse_cache_stats(&stderr);
    assert_eq!(stored, misses, "the surviving units did not store their entries");
    assert_eq!(misses, 3, "three of the four units should have compiled");
    // The failed unit has no entry: the next build misses it and hits the rest,
    // which is the partial-progress resume the design promises.
    let second = cache_build(
        &dir,
        &out_path("cache_failedunit_2"),
        &cache,
        &[],
        &[("KELPC_JOBS", "4")],
    );
    assert!(second.ok, "the resume build failed:\n{}", second.stderr);
    assert_eq!(second.verdict("alpha"), Some(false), "alpha had to miss");
    assert_eq!(second.verdict("beta"), Some(true), "beta had to hit");
    assert_eq!(second.verdict("main"), Some(true), "main had to hit");
    assert_eq!(second.verdict("_support"), Some(true), "_support had to hit");
    let (_, stdout) = run_cached_binary(&out_path("cache_failedunit_2"));
    assert_eq!(String::from_utf8_lossy(&stdout), "hello\n");
}

// ---------------------------------------------------------------------------
// st2: the HTTP server (`plan/http-server.md`).
//
// The Rust test is the *client*. The server is a compiled Kelp program from
// `tests/hazards/` (it waits for clients, so it is not a fixture: the sweeps run
// every fixture with no timeout), started on port 0 with the port read from its
// first stdout line. Every program ends by itself after the number of requests
// it was told to answer, runs under `ulimit -v`/`ulimit -t` and `timeout`, and
// every socket here has a read timeout, so a hang is a failed assertion, not a
// stuck suite. The suite is backend-agnostic like the rest of this file: the
// program is built by `kelpc()`, so `KELPC_ORACLE=c` runs the same cases on the
// C oracle.
// ---------------------------------------------------------------------------

/// The hazard program `name`, built once per test process (the builds of two
/// tests that need the same program would otherwise race for one output file).
fn http_hazard_bin(name: &str) -> PathBuf {
    static BUILT: Mutex<Vec<(String, PathBuf)>> = Mutex::new(Vec::new());
    let mut built = BUILT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, p)) = built.iter().find(|(n, _)| n == name) {
        return p.clone();
    }
    let bin = out_path(&format!("hazard_{name}"));
    let build = kelpc()
        .arg("build")
        .arg(fixture(&format!("tests/hazards/{name}.kp")))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected hazard '{name}' to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    built.push((name.to_string(), bin.clone()));
    bin
}

/// A running server program: its port, and what it printed when it ended.
struct HttpServer {
    child: std::process::Child,
    port: u16,
    out: BufReader<std::process::ChildStdout>,
}

impl HttpServer {
    /// Starts hazard `name` with `args`, bounded in memory, CPU and wall time.
    fn start(name: &str, args: &[&str]) -> HttpServer {
        HttpServer::start_bin(&http_hazard_bin(name), args, true)
    }

    /// As `start` for an already-built program. `cap_memory` is false only for
    /// an AddressSanitizer build, which reserves far more address space than
    /// `ulimit -v` allows.
    fn start_bin(bin: &Path, args: &[&str], cap_memory: bool) -> HttpServer {
        let script = if cap_memory {
            "ulimit -v 1048576; ulimit -t 30; exec timeout 40 \"$0\" \"$@\""
        } else {
            "ulimit -t 30; exec timeout 40 \"$0\" \"$@\""
        };
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(script)
            .arg(&bin)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to start the server program");
        let mut out = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        out.read_line(&mut line).expect("the server printed no port");
        let port: u16 = line
            .trim()
            .parse()
            .unwrap_or_else(|_| panic!("the server's first line is not a port: {line:?}"));
        HttpServer { child, port, out }
    }

    /// The default limits with the request count and the knobs a case changes:
    /// `max_requests`, `max_body`, head/body/idle timeouts in ms, per connection.
    fn start_with(max_requests: u32, max_body: u32, head_ms: u32, body_ms: u32, idle_ms: u32, per_conn: u32) -> HttpServer {
        let args = [max_requests, max_body, head_ms, body_ms, idle_ms, per_conn].map(|n| n.to_string());
        let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        HttpServer::start("http_server", &refs)
    }

    fn start_default(max_requests: u32) -> HttpServer {
        HttpServer::start_with(max_requests, 1 << 20, 10_000, 30_000, 5_000, 100)
    }

    fn client(&self) -> HttpClient {
        HttpClient::connect(self.port)
    }

    /// Waits for the program to end (it ends by itself), checks it exited zero,
    /// and returns `(requests it answered, descriptors before, descriptors
    /// after)` from its last line. Equal descriptor counts are the leak and
    /// double-close gate: every connection and the listener were closed once.
    fn finish(mut self) -> (u64, u64, u64) {
        let deadline = Instant::now() + Duration::from_secs(30);
        let status = loop {
            match self.child.try_wait().expect("wait on the server") {
                Some(s) => break s,
                None => {
                    assert!(Instant::now() < deadline, "the server program did not end");
                    thread::sleep(Duration::from_millis(20));
                }
            }
        };
        let mut rest = String::new();
        self.out.read_to_string(&mut rest).unwrap();
        let mut err = String::new();
        if let Some(mut e) = self.child.stderr.take() {
            let _ = e.read_to_string(&mut err);
        }
        assert!(
            !err.contains("AddressSanitizer") && !err.contains("LeakSanitizer"),
            "AddressSanitizer flagged the server:\n{err}"
        );
        assert!(status.success(), "the server exited with {status:?}\nstdout: {rest}\nstderr: {err}");
        let last = rest.lines().last().unwrap_or_default().to_string();
        let words: Vec<&str> = last.split_whitespace().collect();
        assert!(
            words.len() == 5 && words[0] == "served" && words[2] == "fds",
            "unexpected last line {last:?}\nstderr: {err}"
        );
        let n = |i: usize| words[i].parse::<u64>().unwrap();
        let (served, before, after) = (n(1), n(3), n(4));
        assert_eq!(before, after, "the server leaked or double-closed a descriptor: {last}");
        (served, before, after)
    }
}

impl Drop for HttpServer {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

#[derive(Debug)]
struct HttpReply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl HttpReply {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// A loopback client with a read timeout on every read.
struct HttpClient {
    stream: TcpStream,
    buf: Vec<u8>,
}

use std::net::TcpStream;

impl HttpClient {
    fn connect(port: u16) -> HttpClient {
        let stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to the server");
        stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        stream.set_write_timeout(Some(Duration::from_secs(10))).unwrap();
        HttpClient { stream, buf: Vec::new() }
    }

    /// Writes `bytes`, ignoring a failure: a server that has already refused the
    /// request may have closed, and the answer is still waiting to be read.
    fn send(&mut self, bytes: &[u8]) {
        let _ = self.stream.write_all(bytes);
    }

    /// Reads more into the buffer; false at end of stream or on a reset.
    fn fill(&mut self) -> bool {
        let mut chunk = [0u8; 65536];
        match self.stream.read(&mut chunk) {
            Ok(0) => false,
            Ok(n) => {
                self.buf.extend_from_slice(&chunk[..n]);
                true
            }
            Err(e) => {
                assert!(
                    e.kind() != std::io::ErrorKind::WouldBlock && e.kind() != std::io::ErrorKind::TimedOut,
                    "the server did not answer in time (buffered {:?})",
                    String::from_utf8_lossy(&self.buf)
                );
                false
            }
        }
    }

    /// The next response. `head_only` is for the answer to `HEAD`, whose
    /// `Content-Length` describes a body that is not sent.
    fn read_response(&mut self, head_only: bool) -> HttpReply {
        let end = loop {
            if let Some(i) = self.buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break i + 4;
            }
            assert!(
                self.fill(),
                "the connection ended before a response head arrived (buffered {:?})",
                String::from_utf8_lossy(&self.buf)
            );
        };
        let head = String::from_utf8_lossy(&self.buf[..end - 4]).to_string();
        self.buf.drain(..end);
        let mut lines = head.split("\r\n");
        let status_line = lines.next().unwrap();
        let status: u16 = status_line.split(' ').nth(1).unwrap().parse().unwrap();
        let headers: Vec<(String, String)> = lines
            .map(|l| {
                let (k, v) = l.split_once(':').unwrap();
                (k.to_string(), v.trim().to_string())
            })
            .collect();
        let mut reply = HttpReply { status, headers, body: Vec::new() };
        let bodyless = head_only || status < 200 || status == 204 || status == 304;
        if !bodyless {
            let len: usize = reply
                .header("content-length")
                .expect("a response with a body carries Content-Length")
                .parse()
                .unwrap();
            while self.buf.len() < len {
                assert!(self.fill(), "the connection ended inside a response body");
            }
            reply.body = self.buf.drain(..len).collect();
        }
        reply
    }

    /// True when the server has closed this connection and sent nothing more.
    fn at_eof(&mut self) -> bool {
        if !self.buf.is_empty() {
            return false;
        }
        !self.fill() && self.buf.is_empty()
    }
}

#[test]
fn http_server_answers_a_get_and_echoes_a_post() {
    let server = HttpServer::start_default(2);
    let mut c = server.client();
    c.send(b"GET / HTTP/1.1\r\nHost: t\r\n\r\n");
    let r = c.read_response(false);
    assert_eq!(r.status, 200);
    assert_eq!(r.text(), "hello");
    assert_eq!(r.header("content-type"), Some("text/plain; charset=utf-8"));
    assert_eq!(r.header("content-length"), Some("5"));
    assert_eq!(r.header("connection"), None);
    c.send(b"POST /echo HTTP/1.1\r\nHost: t\r\nContent-Length: 11\r\nConnection: close\r\n\r\nbinary\x00\xff\x01\x7f!");
    let r = c.read_response(false);
    assert_eq!(r.status, 200);
    assert_eq!(r.body, b"binary\x00\xff\x01\x7f!");
    assert_eq!(r.header("connection"), Some("close"));
    assert!(c.at_eof());
    assert_eq!(server.finish().0, 2);
}

#[test]
fn http_server_routes_on_path_segments_and_builds_responses() {
    let server = HttpServer::start_default(8);
    let mut c = server.client();
    let get = |c: &mut HttpClient, target: &str| {
        c.send(format!("GET {target} HTTP/1.1\r\nHost: t\r\n\r\n").as_bytes());
        c.read_response(false)
    };
    let r = get(&mut c, "/users/42");
    assert_eq!((r.status, r.text().as_str()), (200, "{\"id\":\"42\"}"));
    assert_eq!(r.header("content-type"), Some("application/json"));
    let r = get(&mut c, "/seg//a%2Fb/%7Ec/");
    assert_eq!(r.text(), "seg|a/b|~c");
    let r = get(&mut c, "/users");
    assert_eq!(r.status, 404);
    let r = get(&mut c, "/nope");
    assert_eq!((r.status, r.text().as_str()), (404, "not found\n"));
    // An empty response carries no body: a 204 has no Content-Length, a 418 has
    // a zero one.
    c.send(b"GET /nocontent HTTP/1.1\r\nHost: t\r\n\r\n");
    let r = c.read_response(false);
    assert_eq!(r.status, 204);
    assert_eq!(r.header("content-length"), None);
    let r = get(&mut c, "/teapot");
    assert_eq!((r.status, r.header("content-length")), (418, Some("0")));
    let r = get(&mut c, "/bad");
    assert_eq!((r.status, r.text().as_str()), (400, "nope"));
    // The request's own accessors, and a handler-added header.
    c.send(b"GET /hdr HTTP/1.1\r\nHost: t\r\nx-TEST:   padded value  \r\n\r\n");
    let r = c.read_response(false);
    assert_eq!(r.text(), "padded value|has|absent");
    assert_eq!(r.header("x-reply"), Some("yes"));
    assert_eq!(server.finish().0, 8);
}

#[test]
fn http_server_decodes_the_query_and_splits_the_target() {
    let server = HttpServer::start_default(2);
    let mut c = server.client();
    c.send(b"GET /q?name=J%C3%BCn%20e+x&n=7&n=8 HTTP/1.1\r\nHost: t\r\n\r\n");
    let r = c.read_response(false);
    assert_eq!(r.text(), "J\u{fc}n e x,7,<none>");
    assert_eq!(r.header("content-type"), Some("text/html; charset=utf-8"));
    c.send(b"GET /method/a?x=1?y=2 HTTP/1.1\r\nHost: t\r\n\r\n");
    let r = c.read_response(false);
    assert_eq!(r.text(), "GET /method/a?x=1?y=2 /method/a x=1?y=2");
    assert_eq!(server.finish().0, 2);
}

#[test]
fn http_server_keeps_a_connection_alive_for_three_requests() {
    let server = HttpServer::start_default(3);
    let mut c = server.client();
    for i in 0..3 {
        c.send(format!("POST /echo HTTP/1.1\r\nHost: t\r\nContent-Length: 1\r\n\r\n{i}").as_bytes());
        let r = c.read_response(false);
        assert_eq!(r.text(), i.to_string());
        // The server marks the last answer it will give (its request budget).
        assert_eq!(r.header("connection"), if i == 2 { Some("close") } else { None });
    }
    assert!(c.at_eof());
    assert_eq!(server.finish().0, 3);
}

#[test]
fn http_server_answers_pipelined_requests_in_order() {
    let server = HttpServer::start_default(4);
    let mut c = server.client();
    // Two requests in one write, the first with a body, then a second write
    // that carries two more: the read-ahead of one is the start of the next.
    c.send(b"POST /echo HTTP/1.1\r\nHost: t\r\nContent-Length: 5\r\n\r\nfirstGET /method/second HTTP/1.1\r\nHost: t\r\n\r\n");
    assert_eq!(c.read_response(false).text(), "first");
    assert_eq!(c.read_response(false).text(), "GET /method/second /method/second ");
    c.send(b"GET /seg/3 HTTP/1.1\r\nHost: t\r\n\r\nGET /seg/4 HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n");
    assert_eq!(c.read_response(false).text(), "seg|3");
    assert_eq!(c.read_response(false).text(), "seg|4");
    assert!(c.at_eof());
    assert_eq!(server.finish().0, 4);
}

#[test]
fn http_server_reads_a_chunked_request_body_and_discards_trailers() {
    let server = HttpServer::start_default(3);
    let mut c = server.client();
    c.send(b"POST /echo HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: Chunked\r\n\r\n4\r\nWiki\r\n5;ext=1\r\npedia\r\nE\r\n in\r\n\r\nchunks.\r\n0\r\nX-Trailer: a\r\nX-More: b\r\n\r\n");
    assert_eq!(c.read_response(false).text(), "Wikipedia in\r\n\r\nchunks.");
    // The trailers were consumed: the next request on the connection parses.
    c.send(b"POST /len HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n");
    assert_eq!(c.read_response(false).text(), "0");
    // A body sent one byte at a time still frames.
    let wire = b"POST /echo HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n3\r\nabc\r\n0\r\n\r\n";
    for b in wire.iter() {
        c.send(&[*b]);
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(c.read_response(false).text(), "abc");
    assert_eq!(server.finish().0, 3);
}

#[test]
fn http_server_sends_headers_only_for_head() {
    let server = HttpServer::start_default(3);
    let mut c = server.client();
    c.send(b"HEAD / HTTP/1.1\r\nHost: t\r\n\r\n");
    let r = c.read_response(true);
    assert_eq!((r.status, r.header("content-length")), (200, Some("5")));
    assert!(r.body.is_empty());
    // No body bytes followed: the next answer is the next request's.
    c.send(b"GET /method/x HTTP/1.1\r\nHost: t\r\n\r\n");
    assert_eq!(c.read_response(false).text(), "GET /method/x /method/x ");
    c.send(b"HEAD /nope HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n");
    let r = c.read_response(true);
    assert_eq!(r.status, 404);
    assert!(c.at_eof());
    assert_eq!(server.finish().0, 3);
}

#[test]
fn http_server_answers_expect_100_continue_before_the_body() {
    let server = HttpServer::start_default(2);
    let mut c = server.client();
    c.send(b"POST /echo HTTP/1.1\r\nHost: t\r\nContent-Length: 4\r\nExpect: 100-continue\r\n\r\n");
    // The interim answer arrives before any body byte is sent.
    let interim = c.read_response(false);
    assert_eq!(interim.status, 100);
    c.send(b"body");
    let r = c.read_response(false);
    assert_eq!((r.status, r.text().as_str()), (200, "body"));
    // A request with no body does not get one.
    c.send(b"GET / HTTP/1.1\r\nHost: t\r\nExpect: 100-continue\r\nConnection: close\r\n\r\n");
    assert_eq!(c.read_response(false).status, 200);
    assert_eq!(server.finish().0, 2);
}

#[test]
fn http_server_honours_connection_headers_and_http_1_0() {
    let server = HttpServer::start_default(5);
    // HTTP/1.0 closes unless it asks to keep the connection.
    let mut c = server.client();
    c.send(b"GET / HTTP/1.0\r\n\r\n");
    let r = c.read_response(false);
    assert_eq!((r.status, r.header("connection")), (200, Some("close")));
    assert!(c.at_eof());
    let mut c = server.client();
    c.send(b"GET / HTTP/1.0\r\nConnection: keep-alive\r\n\r\n");
    let r = c.read_response(false);
    assert_eq!(r.header("connection"), Some("keep-alive"));
    c.send(b"GET /seg/b HTTP/1.0\r\nConnection: Keep-Alive\r\n\r\n");
    assert_eq!(c.read_response(false).text(), "seg|b");
    c.send(b"GET / HTTP/1.0\r\n\r\n");
    assert_eq!(c.read_response(false).header("connection"), Some("close"));
    assert!(c.at_eof());
    // HTTP/1.1 with `Connection: close` closes; so does a handler that asks.
    let mut c = server.client();
    c.send(b"GET / HTTP/1.1\r\nHost: t\r\nConnection: keep-alive, Close\r\n\r\n");
    assert_eq!(c.read_response(false).header("connection"), Some("close"));
    assert!(c.at_eof());
    assert_eq!(server.finish().0, 5);
}

#[test]
fn http_server_closes_when_the_handler_asks_and_after_the_per_connection_cap() {
    // `/close` sets `Connection: close` itself.
    let server = HttpServer::start_default(3);
    let mut c = server.client();
    c.send(b"GET /close HTTP/1.1\r\nHost: t\r\n\r\n");
    let r = c.read_response(false);
    assert_eq!((r.text().as_str(), r.header("connection")), ("bye", Some("close")));
    assert!(c.at_eof());
    // A cap of 2 requests per connection: the second answer says close.
    let mut c = server.client();
    c.send(b"GET / HTTP/1.1\r\nHost: t\r\n\r\n");
    assert_eq!(c.read_response(false).header("connection"), None);
    c.send(b"GET / HTTP/1.1\r\nHost: t\r\n\r\n");
    assert_eq!(c.read_response(false).header("connection"), Some("close"));
    assert!(c.at_eof());
    assert_eq!(server.finish().0, 3);
    let server = HttpServer::start_with(4, 1 << 20, 10_000, 30_000, 5_000, 2);
    let mut c = server.client();
    for i in 0..2 {
        c.send(b"GET / HTTP/1.1\r\nHost: t\r\n\r\n");
        let r = c.read_response(false);
        assert_eq!(r.header("connection"), if i == 1 { Some("close") } else { None });
    }
    assert!(c.at_eof());
    let mut c = server.client();
    for _ in 0..2 {
        c.send(b"GET / HTTP/1.1\r\nHost: t\r\n\r\n");
        assert_eq!(c.read_response(false).status, 200);
    }
    assert!(c.at_eof());
    assert_eq!(server.finish().0, 4);
}

/// One request that breaks a limit or a framing rule, against a fresh server
/// with a 100-byte body limit and a 300 ms head timeout: the status it is
/// answered with, `Connection: close`, and a closed connection.
fn http_refused(request: &[u8], status: u16) {
    let server = HttpServer::start_with(1, 100, 300, 300, 300, 100);
    let mut c = server.client();
    c.send(request);
    let r = c.read_response(false);
    assert_eq!(
        r.status,
        status,
        "wrong status for {:?}: {r:?}",
        String::from_utf8_lossy(&request[..request.len().min(60)])
    );
    assert_eq!(r.header("connection"), Some("close"), "an error response closes");
    assert!(!r.body.is_empty(), "an error response explains itself");
    assert!(c.at_eof(), "the connection must close after a {status}");
    assert_eq!(server.finish().0, 1);
}

#[test]
fn http_server_refuses_oversized_requests_with_the_right_status() {
    // 431: one header over 16 KiB of headers, and more than 100 headers.
    let mut big = b"GET / HTTP/1.1\r\nHost: t\r\nX-Big: ".to_vec();
    big.extend(std::iter::repeat(b'a').take(17_000));
    big.extend_from_slice(b"\r\n\r\n");
    http_refused(&big, 431);
    let mut many = b"GET / HTTP/1.1\r\nHost: t\r\n".to_vec();
    for i in 0..101 {
        many.extend_from_slice(format!("X-{i}: v\r\n").as_bytes());
    }
    many.extend_from_slice(b"\r\n");
    http_refused(&many, 431);
    // 414: a target past the 8 KiB request-line limit.
    let mut long = b"GET /".to_vec();
    long.extend(std::iter::repeat(b'a').take(9_000));
    long.extend_from_slice(b" HTTP/1.1\r\nHost: t\r\n\r\n");
    http_refused(&long, 414);
    // 413: a Content-Length over the limit (the body is sent, the server never
    // reads it), a chunk over it, and chunks that add up past it.
    let mut body = b"POST /echo HTTP/1.1\r\nHost: t\r\nContent-Length: 101\r\n\r\n".to_vec();
    body.extend(std::iter::repeat(b'x').take(101));
    http_refused(&body, 413);
    http_refused(b"POST /echo HTTP/1.1\r\nHost: t\r\nContent-Length: 99999999999999999999999\r\n\r\n", 413);
    http_refused(b"POST /echo HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n65\r\n", 413);
    let mut sum = b"POST /echo HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
    for _ in 0..3 {
        sum.extend_from_slice(b"3C\r\n");
        sum.extend(std::iter::repeat(b'y').take(60));
        sum.extend_from_slice(b"\r\n");
    }
    http_refused(&sum, 413);
}

#[test]
fn http_server_refuses_malformed_heads_with_400() {
    http_refused(b"GARBAGE\r\n\r\n", 400);
    http_refused(b"GET /\r\n\r\n", 400);
    http_refused(b"GET  / HTTP/1.1\r\nHost: t\r\n\r\n", 400);
    http_refused(b"GET x HTTP/1.1\r\nHost: t\r\n\r\n", 400);
    http_refused(b"G(T / HTTP/1.1\r\nHost: t\r\n\r\n", 400);
    http_refused(b"GET / HTTP/x.y\r\nHost: t\r\n\r\n", 400);
    http_refused(b"\r\nGET / HTTP/1.1\r\nHost: t\r\n\r\n", 400);
    // A bare LF, a NUL, a byte above 127: heads are CRLF ASCII.
    http_refused(b"GET / HTTP/1.1\nHost: t\n\n", 400);
    http_refused(b"GET / HTTP/1.1\r\nHost: t\x00\r\n\r\n", 400);
    http_refused(b"GET / HTTP/1.1\r\nHost: t\xff\r\n\r\n", 400);
    // A header with no colon, a space before the colon, an empty name.
    http_refused(b"GET / HTTP/1.1\r\nHost t\r\n\r\n", 400);
    http_refused(b"GET / HTTP/1.1\r\nHost : t\r\n\r\n", 400);
    http_refused(b"GET / HTTP/1.1\r\n: t\r\n\r\n", 400);
    // Folded headers are refused, with a space and with a tab.
    http_refused(b"GET / HTTP/1.1\r\nHost: t\r\nX-A: one\r\n two\r\n\r\n", 400);
    http_refused(b"GET / HTTP/1.1\r\nHost: t\r\nX-A: one\r\n\ttwo\r\n\r\n", 400);
    // A chunk size that is not hexadecimal, and a chunk not followed by CRLF.
    http_refused(b"POST /echo HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n", 400);
    http_refused(b"POST /echo HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabcXY0\r\n\r\n", 400);
}

#[test]
fn http_server_refuses_request_smuggling_shapes() {
    // Content-Length twice with different values, a list, a sign, a non-number.
    http_refused(b"POST /echo HTTP/1.1\r\nHost: t\r\nContent-Length: 3\r\nContent-Length: 4\r\n\r\nabc", 400);
    http_refused(b"POST /echo HTTP/1.1\r\nHost: t\r\nContent-Length: 3, 3\r\n\r\nabc", 400);
    http_refused(b"POST /echo HTTP/1.1\r\nHost: t\r\nContent-Length: +3\r\n\r\nabc", 400);
    http_refused(b"POST /echo HTTP/1.1\r\nHost: t\r\nContent-Length: \r\n\r\n", 400);
    // Content-Length together with Transfer-Encoding, either order.
    http_refused(b"POST /echo HTTP/1.1\r\nHost: t\r\nContent-Length: 3\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n", 400);
    http_refused(b"POST /echo HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\nContent-Length: 3\r\n\r\n0\r\n\r\n", 400);
    // Two Transfer-Encoding headers; a chunked framing in HTTP/1.0.
    http_refused(b"POST /echo HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n", 400);
    http_refused(b"POST /echo HTTP/1.0\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n", 400);
    // An unknown transfer coding is 501, whether alone or before `chunked`.
    http_refused(b"POST /echo HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: gzip\r\n\r\n", 501);
    http_refused(b"POST /echo HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: gzip, chunked\r\n\r\n", 501);
}

#[test]
fn http_server_refuses_versions_expectations_and_missing_lengths() {
    http_refused(b"GET / HTTP/2.0\r\nHost: t\r\n\r\n", 505);
    http_refused(b"GET / HTTP/0.9\r\nHost: t\r\n\r\n", 505);
    http_refused(b"GET / HTTP/1.2\r\nHost: t\r\n\r\n", 505);
    http_refused(b"POST /echo HTTP/1.1\r\nHost: t\r\n\r\n", 411);
    http_refused(b"PUT /echo HTTP/1.1\r\nHost: t\r\n\r\n", 411);
    http_refused(b"GET / HTTP/1.1\r\nHost: t\r\nExpect: nonsense\r\n\r\n", 417);
}

#[test]
fn http_server_times_out_a_request_that_stalls() {
    // 408: the head stops halfway, and the body stops halfway (300 ms limits).
    let started = Instant::now();
    http_refused(b"GET / HTTP/1.1\r\nHo", 408);
    http_refused(b"POST /echo HTTP/1.1\r\nHost: t\r\nContent-Length: 10\r\n\r\nabc", 408);
    http_refused(b"POST /echo HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nab", 408);
    assert!(started.elapsed() < Duration::from_secs(20));
}

#[test]
fn http_server_cuts_off_a_slowloris_client_at_the_head_deadline() {
    // The head deadline is 500 ms. The client sends a valid request one byte
    // every 100 ms, which would take 4 s: every `recv` succeeds well inside any
    // per-read timeout, so only a deadline over the whole head ends this.
    let server = HttpServer::start_with(1, 1 << 20, 500, 500, 500, 100);
    let mut c = server.client();
    let mut writer = c.stream.try_clone().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    let wire = b"GET / HTTP/1.1\r\nHost: slow\r\nX-Pad: aaaaaaaaaaaaaaaaaaaa\r\n\r\n".to_vec();
    let trickler = thread::spawn(move || {
        for b in &wire[..wire.len() - 1] {
            if stop2.load(Ordering::SeqCst) || writer.write_all(&[*b]).is_err() {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
    });
    let started = Instant::now();
    let r = c.read_response(false);
    let took = started.elapsed();
    stop.store(true, Ordering::SeqCst);
    trickler.join().unwrap();
    assert_eq!(r.status, 408, "{r:?}");
    assert_eq!(r.header("connection"), Some("close"));
    assert!(took >= Duration::from_millis(400), "cut off before the deadline: {took:?}");
    assert!(took < Duration::from_millis(3000), "not cut off by the deadline: {took:?}");
    assert!(c.at_eof());
    assert_eq!(server.finish().0, 1);
}

#[test]
fn http_server_closes_an_idle_keep_alive_connection_without_an_answer() {
    // The idle limit is 300 ms: the connection is closed with no 408 (nothing
    // was being requested), and the server goes on to the next client.
    let server = HttpServer::start_with(2, 1 << 20, 10_000, 10_000, 300, 100);
    let mut c = server.client();
    c.send(b"GET / HTTP/1.1\r\nHost: t\r\n\r\n");
    assert_eq!(c.read_response(false).status, 200);
    let started = Instant::now();
    assert!(c.at_eof(), "the idle connection was not closed");
    assert!(started.elapsed() < Duration::from_secs(5));
    let mut c2 = server.client();
    c2.send(b"GET / HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n");
    assert_eq!(c2.read_response(false).status, 200);
    assert_eq!(server.finish().0, 2);
}

#[test]
fn http_server_survives_a_client_that_disconnects_mid_body() {
    let server = HttpServer::start_default(1);
    {
        let mut c = server.client();
        c.send(b"POST /echo HTTP/1.1\r\nHost: t\r\nContent-Length: 100\r\n\r\nonly ten b");
        // Dropped here: the server reads an end of stream inside the body.
    }
    {
        let mut c = server.client();
        c.send(b"POST /echo HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n9\r\nabc");
    }
    {
        let mut c = server.client();
        c.send(b"GET / HTTP/1.1\r\nHost: t\r\nX-Cut:");
    }
    let mut c = server.client();
    c.send(b"GET / HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n");
    assert_eq!(c.read_response(false).text(), "hello");
    assert_eq!(server.finish().0, 1);
}

#[test]
fn http_server_survives_a_client_that_resets_mid_response() {
    // `/big` is a 4 MiB body. The client reads a little and closes with the rest
    // unread, which the kernel turns into a reset: the server's write fails, it
    // closes its end and serves the next client. The reset request counts (its
    // handler ran), which is why the budget is 2.
    let server = HttpServer::start_default(2);
    {
        let mut c = server.client();
        c.send(b"GET /big HTTP/1.1\r\nHost: t\r\n\r\n");
        assert!(c.fill());
    }
    let mut c = server.client();
    c.send(b"GET / HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n");
    assert_eq!(c.read_response(false).text(), "hello");
    assert_eq!(server.finish().0, 2);
}

#[test]
fn http_server_serves_eight_concurrent_clients_on_four_workers() {
    // 4 worker threads accept on one listener. `/id` sleeps 200 ms and answers
    // with the serving thread's id, so a sequential server would show one id
    // and take 1.6 s.
    let server = HttpServer::start("http_server_threaded", &["4", "1500"]);
    let port = server.port;
    let started = Instant::now();
    let clients: Vec<_> = (0..8)
        .map(|_| {
            thread::spawn(move || {
                let mut c = HttpClient::connect(port);
                c.send(b"GET /id HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n");
                let r = c.read_response(false);
                assert_eq!(r.status, 200);
                r.text()
            })
        })
        .collect();
    let ids: Vec<String> = clients.into_iter().map(|h| h.join().unwrap()).collect();
    let took = started.elapsed();
    let mut distinct = ids.clone();
    distinct.sort();
    distinct.dedup();
    assert!(distinct.len() >= 3, "the clients were not served by separate workers: {ids:?}");
    assert!(distinct.len() <= 4, "more thread ids than workers: {ids:?}");
    assert!(took < Duration::from_millis(1500), "the clients were served one at a time: {took:?}");
    assert_eq!(server.finish().0, 8);
}

#[test]
fn http_listen_follows_the_net_grant_port() {
    // `http.listen` is `net.listen`: a grant that names other ports refuses it
    // before anything is bound, and a grant that covers the port lets a server
    // bind port 0 and report it.
    assert_project_denied_at_runtime(
        "caps_http_listen_denied",
        &project("caps_http_listen_denied").join("main.kp"),
        "127.0.0.1:1",
    );
    assert_project_runs(
        "caps_http_listen",
        &project("caps_http_listen").join("main.kp"),
        "listening\ntrue\n",
    );
}

/// The hazard program `name` built through the C backend and rebuilt from its
/// generated `.c` with AddressSanitizer (as `asan_tls_fixture` does for a
/// fixture).
fn http_hazard_asan_bin(name: &str) -> PathBuf {
    let bin = out_path(&format!("hazard_{name}_asan_src"));
    let build = kelpc_c()
        .arg("build")
        .arg(fixture(&format!("tests/hazards/{name}.kp")))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected hazard '{name}' to compile through the C backend:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let asan_bin = out_path(&format!("hazard_{name}_asan"));
    let cc = Command::new("cc")
        .arg("-fsanitize=address")
        .arg("-O1")
        .arg("-o")
        .arg(&asan_bin)
        .arg(bin.with_extension("c"))
        .args(["-fwrapv", "-pthread", "-lm", "-lssl", "-lcrypto"])
        .output()
        .expect("failed to spawn cc");
    assert!(
        cc.status.success(),
        "the AddressSanitizer build of '{name}' failed:\n{}",
        String::from_utf8_lossy(&cc.stderr)
    );
    asan_bin
}

#[test]
fn http_server_frees_everything_under_address_sanitizer() {
    // The leak and double-close gate for the server: every kind of connection
    // end (a keep-alive run, a refused request, a chunked body, `HEAD`, a client
    // that vanishes mid-body, one that resets mid-response, an idle timeout)
    // runs under AddressSanitizer's leak checker, and the server's own
    // descriptor count before and after is compared as well.
    let bin = http_hazard_asan_bin("http_server");
    let args = ["7", "1000", "10000", "10000", "300", "100"];
    let server = HttpServer::start_bin(&bin, &args, false);
    let mut c = server.client();
    c.send(b"GET / HTTP/1.1\r\nHost: t\r\n\r\n");
    assert_eq!(c.read_response(false).text(), "hello");
    c.send(b"POST /echo HTTP/1.1\r\nHost: t\r\nContent-Length: 3\r\n\r\nabc");
    assert_eq!(c.read_response(false).text(), "abc");
    c.send(b"HEAD /seg/x HTTP/1.1\r\nHost: t\r\n\r\n");
    assert_eq!(c.read_response(true).status, 200);
    c.send(b"POST /echo HTTP/1.1\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nhi\r\n0\r\nT: 1\r\n\r\n");
    assert_eq!(c.read_response(false).text(), "hi");
    // Idle: the server closes this connection by itself (no answer, no count).
    assert!(c.at_eof());
    {
        let mut c = server.client();
        c.send(b"POST /echo HTTP/1.1\r\nHost: t\r\nContent-Length: 50\r\n\r\nshort");
    }
    let mut c = server.client();
    c.send(b"POST /echo HTTP/1.1\r\nHost: t\r\nContent-Length: 5000\r\n\r\n");
    assert_eq!(c.read_response(false).status, 413);
    assert!(c.at_eof());
    {
        let mut c = server.client();
        c.send(b"GET /big HTTP/1.1\r\nHost: t\r\n\r\n");
        assert!(c.fill());
    }
    let mut c = server.client();
    c.send(b"GET /nope HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n");
    assert_eq!(c.read_response(false).status, 404);
    assert_eq!(server.finish().0, 7);
    // The worker threads too: each one's connections and the shared listener.
    let bin = http_hazard_asan_bin("http_server_threaded");
    let server = HttpServer::start_bin(&bin, &["3", "1000"], false);
    let port = server.port;
    let clients: Vec<_> = (0..5)
        .map(|_| {
            thread::spawn(move || {
                let mut c = HttpClient::connect(port);
                c.send(b"GET /id HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n");
                c.read_response(false).status
            })
        })
        .collect();
    for h in clients {
        assert_eq!(h.join().unwrap(), 200);
    }
    assert_eq!(server.finish().0, 5);
}

#[test]
fn http_server_low_level_api_and_accessors_work_in_one_process() {
    // The defaults, the request accessors (`path_segments`, `query_param` with
    // its percent- and plus-decoding, case-insensitive headers) and the
    // low-level accept / read_request / write_response / write_error / close
    // loop, with the client and the server in one process over loopback.
    assert_compiles_and_runs(
        "tests/fixtures/http_server_lowlevel.kp",
        "8192\n16384\n100\n1048576\n10000\n30000\n5000\n100\n3\nb c\nd\n1\nA b\n\n\n%zz\n<none>\n1\ntrue\nfalse\nGET\n/p\nq=1\nPOST\n2\n0\nfalse\nHTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: 3\r\n\r\noneHTTP/1.1 200 OK\r\nContent-Type: application/json\r\nX-Seen: yes\r\nContent-Length: 2\r\n\r\n{}\n400\nHTTP/1.1 400 Bad Request\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: 35\r\nConnection: close\r\n\r\nconflicting Content-Length headers\n\n",
    );
}

/// Polyp v3 stage 1: the pure diff and the whole todo program on the headless backend.
///
/// `examples/polyp-core-test` has no `[extern.*]` section at all, so this runs in the plain
/// development shell with no X server and no GTK, on the backend under test. The first half is the
/// diff: identical trees produce nothing, a text change one Update, a keyed reorder two Moves and
/// no Create or Remove, an insert two Creates (the keyed row and its Text child), a remove one
/// patch for the whole subtree, a kind change a replace, and a duplicate key a panic naming it.
///
/// The second half runs the todo example's own program (`examples/polyp-todo/src/app.kp`, linked in
/// as `deps/todo`) on the headless backend with the same scripted input `examples/polyp-todo/smoke.sh`
/// arms the GTK backend with, and prints the same summary. The counters here must be the ones that
/// smoke asserts: created 22, updated 10, removed 5, live 17, focus present and surviving, one
/// entry built, and the entry still showing `gamma` while the model's draft is empty.
#[test]
fn polyp_core_diffs_and_runs_the_todo_program_with_no_display() {
    let bin = out_path("polyp_core_test");
    let build = kelpc()
        .arg("build")
        .arg(fixture("../examples/polyp-core-test"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected the polyp core test project to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let run = Command::new(&bin).output().expect("failed to run the core test binary");
    assert!(
        run.status.success(),
        "expected the core test to exit successfully, got:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        EXPECTED,
        "unexpected output from the polyp core test"
    );
}

const EXPECTED: &str = "identical\n0\n0\n0\n0\ntext\n0\n1\n0\n0\nreorder\n0\n0\n0\n2\ninsert\n2\n0\n0\n0\nremove\n0\n0\n1\n0\nkind\n1\n0\n1\n0\n-- todo --\n2\nalpha\nfalse\nbeta\ntrue\n7\n12\n17\n22\n22\n22\n22\n10\n5\n17\ntrue\ntrue\n1\ngamma\n\nend\n-- tree --\ncolumn\n  row\n    entry id=0 \"gamma\"\n    button id=1 \"add\"\n  separator\n  scrolled\n    row #1\n      checkbox id=2 unchecked\n      text \"alpha\"\n      button id=3 \"^\"\n      button id=4 \"x\"\n    row #2\n      checkbox id=5 checked\n      text \"beta\"\n      button id=6 \"^\"\n      button id=7 \"x\"\n  progress 0.500\n\n";

/// The duplicate-key half, in its own run because a `panic` ends the process.
#[test]
fn polyp_core_panics_on_a_duplicate_key_and_names_it() {
    let bin = out_path("polyp_core_dupkey");
    let build = kelpc()
        .arg("build")
        .arg(fixture("../examples/polyp-core-test"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected the polyp core test project to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let run = Command::new(&bin)
        .env("POLYP_TEST", "dupkey")
        .output()
        .expect("failed to run the core test binary");
    assert!(
        !run.status.success(),
        "expected a duplicate key to panic, but the program exited successfully"
    );
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.contains("duplicate key 'dup' among the children of one container"),
        "expected the panic to name the key, got:\n{stderr}"
    );
}


/// Polyp v3 stage 2: the software rasterizer's primitives, font, damage list and checksums.
///
/// `examples/polyp-soft-test` carries the tests in Kelp: each primitive runs against an
/// independent per-pixel reference written in the project (full-precision integers, its own
/// geometry), on fixed-seed LCG inputs with negative and zero sizes and clips on every edge.
/// The project also prints golden FNV checksums for three scenes. Three panic contracts
/// (`POLYP_SOFT_TEST=scale`, `clip_depth`, `clip_empty`) and the hosted PPM dump
/// (`POLYP_SOFT_TEST=dump`) are exercised here, because a panic ends the process and the dump
/// writes a file.
///
/// Every run goes through `run_bounded`: `ulimit -v 1048576` (1 GiB), `ulimit -t 10` and
/// `timeout 10`, so a bug cannot take the host down the way an earlier unbounded 6.7 GB run did.
#[test]
fn polyp_soft_raster_matches_its_references_and_golden_checksums() {
    let bin = out_path("polyp_soft_test");
    let build = kelpc()
        .arg("build")
        .arg(fixture("../examples/polyp-soft-test"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected the polyp soft test project to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let run = run_bounded(&bin, &[]);
    assert!(
        run.status.success(),
        "expected the soft test to exit successfully, got:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        POLYP_SOFT_EXPECTED,
        "unexpected output from the soft test"
    );

    for (mode, needle) in [
        ("scale", "text scale must be in 1..=4"),
        ("clip_depth", "clip stack depth 8 exceeded"),
        ("clip_empty", "clip stack is empty"),
    ] {
        let run = run_bounded(&bin, &[("POLYP_SOFT_TEST", mode)]);
        assert!(
            !run.status.success(),
            "expected POLYP_SOFT_TEST={mode} to panic, but the program exited successfully"
        );
        let stderr = String::from_utf8_lossy(&run.stderr);
        assert!(
            stderr.contains(needle),
            "expected the {mode} panic to mention '{needle}', got:\n{stderr}"
        );
    }

    let ppm = out_path("polyp_soft_frame").with_extension("ppm");
    let run = run_bounded(
        &bin,
        &[
            ("POLYP_SOFT_TEST", "dump"),
            ("POLYP_SOFT_PPM", ppm.to_str().unwrap()),
        ],
    );
    assert!(run.status.success(), "the dump run failed:\n{}", String::from_utf8_lossy(&run.stderr));
    assert_eq!(String::from_utf8_lossy(&run.stdout), "dumped\n");
    let bytes = std::fs::read(&ppm).expect("the PPM the dump run wrote");
    assert_eq!(&bytes[..13], b"P6\n40 28\n255\n", "the PPM header");
    assert_eq!(bytes.len(), 13 + 40 * 28 * 3, "the PPM body length");
    // Pixel (0, 0) of the dumped blend scene: `0x80FF0000` source-over the `0xFF000000`
    // background is `(128 * 255 + 127) / 255 == 128` red, zero green, zero blue.
    assert_eq!(&bytes[13..16], &[128u8, 0, 0], "the dumped pixel at (0, 0)");
    let _ = std::fs::remove_file(&ppm);
}

/// Runs `bin` with `envs` under a 1 GiB address-space limit, a 10 s CPU limit and a 10 s wall
/// timeout, and returns its output. Every compiled program this file runs on behalf of the
/// raster tests goes through here.
fn run_bounded(bin: &Path, envs: &[(&str, &str)]) -> std::process::Output {
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg("( ulimit -v 1048576; ulimit -t 10; timeout 10 \"$1\" )")
        .arg("sh")
        .arg(bin);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.output().expect("failed to run the soft test binary")
}

const POLYP_SOFT_EXPECTED: &str = "fill_rect
60
0
stroke_rect
60
0
fill_round_rect
60
0
stroke_round_rect
60
0
line
60
0
blend
40
0
blit_mask8
30
0
draw_text
40
0
clip_stack
40
0
edges
13
0
damage
200
8
0
cap
16
true
static_fb
5870808570317741093
scene shapes
14814842095218757869
scene text
15225207939037777876
scene blend
3376705936528111333
failures
0
";

/// Polyp v3 stage 3: the software backend's hit testing, focus order, entry editing, damage and
/// layout skip.
///
/// `examples/polyp-ui-test` runs on `HeadlessPlatform` in the plain development shell. The five
/// sections are deterministic: the hit section clicks each interactive widget by its own centre;
/// the order section Tabs through the tree and wraps; the edit section types, moves the caret,
/// deletes and backspaces; the damage section changes one label's text and asserts the exact
/// rectangle and that a FNV digest of every pixel outside it is unchanged, then hovers and
/// asserts an idle frame presents nothing; the layout section counts re-measures and re-arranges
/// for a same-width text change (one measure, no arrange) and a wider one (both).
#[test]
fn polyp_soft_ui_hits_focuses_edits_and_damages() {
    let bin = out_path("polyp_ui_test");
    let build = kelpc()
        .arg("build")
        .arg(fixture("../examples/polyp-ui-test"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected the polyp ui test project to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = run_bounded(&bin, &[]);
    assert!(
        run.status.success(),
        "expected the ui test to exit successfully, got:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        POLYP_UI_EXPECTED,
        "unexpected output from the polyp ui test"
    );
}

const POLYP_UI_EXPECTED: &str = "hit\n0\n1\n2\n3\n-1\norder\n0\n1\n2\n3\n0\n1\n2\n3\n0\n3\nedit\nabcd\n0\nXabcd\n1\nXaYbcd\n3\n2\naYbcd\n0\n5\naYbc\n4\ndamage\n1\n0\n0\n24\n7\noutside-same\n1\n0\n0\nhover\n1\n22\n7\n22\n20\n1\n22\n22\ncheckbox\ntrue\n1\nscroll\nscrolled\n1\n0\nlayout\n1\n0\n2\n3\n";

/// Polyp v3 stage 3: the todo program on the soft backend, at both golden sizes.
///
/// `examples/polyp-todo-soft` links `examples/polyp-todo/src/app.kp` (the program) and
/// `examples/polyp-todo/src/scenario.kp` (the driver the framebuffer kernels share with it), so its
/// final model and the library's counters are the ones `examples/polyp-todo/smoke.sh` and
/// `examples/polyp-core-test` produce. It prints one `step <n> checksum <hex> dirty <count>` line
/// per frame and then the report, and every line here is the golden value; `POLYP_SOFT_SIZE=big`
/// reruns the same binary at 800x600 for the second golden set, and `POLYP_SOFT_SIZE=kernel` at the
/// 320x200 the framebuffer kernels use, which is the expected output the two kernels below compare
/// against.
#[test]
fn polyp_todo_runs_on_the_soft_backend_with_the_same_counters() {
    let bin = out_path("polyp_todo_soft");
    let build = kelpc()
        .arg("build")
        .arg(fixture("../examples/polyp-todo-soft"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected the polyp todo soft project to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = run_bounded(&bin, &[]);
    assert!(
        run.status.success(),
        "expected the 400x300 run to exit successfully, got:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        POLYP_TODO_SOFT_400,
        "unexpected output from the 400x300 todo-soft run"
    );
    let run = run_bounded(&bin, &[("POLYP_SOFT_SIZE", "big")]);
    assert!(
        run.status.success(),
        "expected the 800x600 run to exit successfully, got:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        POLYP_TODO_SOFT_800,
        "unexpected output from the 800x600 todo-soft run"
    );
}

const POLYP_TODO_SOFT_400: &str = "step 0 checksum 0x3fe9bf4f4c216435 dirty 1
step 1 checksum 0x0b75e67efe727df5 dirty 1
step 2 checksum 0x30a73f0b7686ae98 dirty 1
step 3 checksum 0x5ea79458670ced5c dirty 2
step 4 checksum 0xd765bd454cf40665 dirty 1
step 5 checksum 0x6e014703e561504c dirty 2
step 6 checksum 0x133d1ca6e8e276a5 dirty 1
step 7 checksum 0x8e17675a724b10fd dirty 2
step 8 checksum 0xfa3d5e46fb8b2113 dirty 2
step 9 checksum 0x89b8f7d6bd68e19b dirty 2
step 10 checksum 0x12d8cf35c1606b27 dirty 3
step 11 checksum 0x12d8cf35c1606b27 dirty 0
display 0xc33211c98befabd7
-- todo --
2
alpha
false
beta
true
7
12
17
22
22
22
22
10
5
17
true
true
1
gamma

end
";

const POLYP_TODO_SOFT_800: &str = "step 0 checksum 0xf0430e4ebe066935 dirty 1
step 1 checksum 0xf4497c1d3a354775 dirty 1
step 2 checksum 0xf93c79c54dcb06f8 dirty 1
step 3 checksum 0x3f9118f05cc2f7fc dirty 2
step 4 checksum 0xd539e0e8a372d425 dirty 1
step 5 checksum 0x8f9684b6e753a46c dirty 2
step 6 checksum 0xebde2f6e1f6fe7e5 dirty 1
step 7 checksum 0x5d3fbf9923562efd dirty 2
step 8 checksum 0xe451f4fb536873bf dirty 2
step 9 checksum 0xe4bf0c25d129d687 dirty 2
step 10 checksum 0xf4b409b661bcc047 dirty 3
step 11 checksum 0xf4b409b661bcc047 dirty 0
display 0x33651316579eafb7
-- todo --
2
alpha
false
beta
true
7
12
17
22
22
22
22
10
5
17
true
true
1
gamma

end
";

/// Polyp v3 stage 4: the hosted run at the framebuffer kernels' resolution, which is the expected
/// output for both kernels.
///
/// It builds `examples/polyp-todo-soft` and runs it with `POLYP_SOFT_SIZE=kernel` (320x200, the
/// kernels' `static mut` surface size), through the bounded runner every compiled program in this
/// file goes through.
fn hosted_fb_run() -> Vec<String> {
    let bin = out_path("polyp_fb_hosted");
    let build = kelpc()
        .arg("build")
        .arg(fixture("../examples/polyp-todo-soft"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected the hosted soft project to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = run_bounded(&bin, &[("POLYP_SOFT_SIZE", "kernel")]);
    assert!(
        run.status.success(),
        "expected the hosted 320x200 run to exit successfully, got:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    run_slice(&String::from_utf8_lossy(&run.stdout))
}

/// The lines that make up one run of the shared scenario: from the first `step ` line to the first
/// `end` line, which is the report's last line. The hosted entry prints a `-- todo --` separator
/// before the report and the kernels do not, so it is dropped; everything else has to be equal.
/// The same function reads a kernel's serial output, so the two sides are sliced identically and
/// only the run itself can differ.
fn run_slice(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines
        .iter()
        .position(|line| line.starts_with("step "))
        .unwrap_or_else(|| panic!("no step line in the run's output:\n{text}"));
    let end = lines
        .iter()
        .position(|line| *line == "end")
        .unwrap_or_else(|| panic!("no `end` line in the run's output:\n{text}"));
    let out: Vec<String> = lines[start..=end]
        .iter()
        .filter(|line| **line != "-- todo --")
        .map(|line| line.to_string())
        .collect();
    // Twelve `step` lines, one per frame, the `display` line the run ends with, and the report's
    // twenty-one. A run whose shape changed would otherwise compare equal only by accident.
    assert_eq!(out.len(), 12 + 1 + 21, "the run's shape changed:\n{out:?}");
    out
}

/// `llvm-nm -u`'s output for one linked image, which is the proof that nothing in the Polyp run
/// needed libc or a soft-float helper: a freestanding image must name no undefined symbol at all.
fn undefined_symbols(image: &Path) -> String {
    tool("llvm-nm", &["-u", image.to_str().expect("a utf-8 image path")])
}

/// Polyp v3 stage 4: the riscv64 framebuffer kernel draws the hosted run's own pixels.
///
/// `examples/polyp-fb-riscv64` links the Polyp core, the stage-2 raster, the soft backend and the
/// shared todo scenario into a bare-metal riscv64 image with no libc: its own
/// `static mut [u32; 320 * 200]` surface, a bump arena behind `[target] alloc`/`dealloc`, the
/// scripted platform and the NS16550 driver. The expected output is the hosted soft backend's: this
/// test runs `examples/polyp-todo-soft` at `POLYP_SOFT_SIZE=kernel` and requires the kernel's
/// serial lines, both the `step <n> checksum ... dirty ...` lines and the report, to be identical.
/// One pixel difference anywhere fails it. `llvm-nm -u` on the image is the other half: the image
/// names no undefined symbol, so nothing reached libc or a soft-float helper. Both tests skip where
/// `ld.lld` or the emulator is absent (`dev.nix`), which is why they run in `dev-kernel.nix`.
#[test]
fn polyp_fb_riscv64_draws_the_hosted_runs_pixels() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU) {
        eprintln!(
            "skipping the riscv64 framebuffer kernel: 'ld.lld' or 'qemu-system-riscv64' is not on \
             PATH (run this test in dev-kernel.nix)"
        );
        return;
    }
    let expected = hosted_fb_run();
    let name = "polyp_fb_riscv64";
    let captured = boot_and_capture(&RISCV64_QEMU, name, &fixture("../examples/polyp-fb-riscv64"), &|got| {
        got.lines().any(|line| line == "end")
    });
    let undefined = undefined_symbols(&out_path(name));
    assert!(
        undefined.trim().is_empty(),
        "the riscv64 Polyp image names undefined symbols, so something reached libc or soft float:\n{undefined}"
    );
    assert_eq!(
        run_slice(&captured),
        expected,
        "the riscv64 kernel's serial output differs from the hosted 320x200 run; captured:\n{captured}"
    );
}

/// Polyp v3 stage 4: the x86-64 framebuffer kernel draws the same pixels as the hosted run and as
/// the riscv64 kernel.
///
/// `examples/polyp-fb-x86_64` is the same program with the COM1 driver in place of the NS16550 and
/// the same `fb_platform.kp` source file (the two projects share it through a symlink). Born from
/// SeaBIOS' Xen PVH loader, it goes through the compiler's 32-bit stub into long mode before any
/// Kelp runs, so this test also covers the whole renderer on the second ISA family.
#[test]
fn polyp_fb_x86_64_draws_the_hosted_runs_pixels() {
    if !have_ld_lld() || !have_qemu(&X86_64_QEMU) {
        eprintln!(
            "skipping the x86-64 framebuffer kernel: 'ld.lld' or 'qemu-system-x86_64' is not on \
             PATH (run this test in dev-kernel.nix)"
        );
        return;
    }
    let expected = hosted_fb_run();
    let name = "polyp_fb_x86_64";
    let captured = boot_and_capture(&X86_64_QEMU, name, &fixture("../examples/polyp-fb-x86_64"), &|got| {
        got.lines().any(|line| line == "end")
    });
    let undefined = undefined_symbols(&out_path(name));
    assert!(
        undefined.trim().is_empty(),
        "the x86-64 Polyp image names undefined symbols, so something reached libc or soft float:\n{undefined}"
    );
    assert_eq!(
        run_slice(&captured),
        expected,
        "the x86-64 kernel's serial output differs from the hosted 320x200 run; captured:\n{captured}"
    );
}

/// Polyp v3 stage 4's stretch: QEMU's `ramfb` device shows the kernel's framebuffer.
///
/// The riscv64 project registers its `static mut` surface with the `ramfb` device through fw_cfg (a
/// DMA write of `etc/ramfb`: address, `XR24`, flags, width, height, stride, all big-endian, which is
/// what `src/ramfb.kp` implements). This test boots the image with `-device ramfb` and a monitor on
/// a pipe, waits for the run's last line, asks the monitor for a `screendump`, and checks the PPM it
/// wrote against the kernel's own `display` line: FNV-1a 64 over each pixel's blue, green and red
/// bytes and a constant alpha, because a display shows only the colour (the framebuffer's alpha byte
/// carries glyph coverage, so `checksum` would not match a screen). The same function is what the
/// raster's `checksum_rgb` computes, so a single differing pixel fails it. Skips where `ld.lld` or
/// the emulator is absent.
#[test]
fn polyp_fb_riscv64_shows_its_framebuffer_on_a_real_display() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU) {
        eprintln!(
            "skipping the riscv64 ramfb boot: 'ld.lld' or 'qemu-system-riscv64' is not on PATH \
             (run this test in dev-kernel.nix)"
        );
        return;
    }
    let name = "polyp_fb_ramfb";
    let bin = out_path(name);
    let build = kelpc()
        .env_remove("KELPC_BACKEND")
        .env_remove("KELPC_ORACLE")
        .arg("build")
        .arg(fixture("../examples/polyp-fb-riscv64"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the freestanding ramfb build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let serial = out_path("polyp_fb_ramfb_serial").with_extension("txt");
    let shot = out_path("polyp_fb_ramfb_shot").with_extension("ppm");
    let _ = std::fs::remove_file(&serial);
    let _ = std::fs::remove_file(&shot);

    // `-display none` still creates the console `ramfb` attaches to, which is what makes a
    // `screendump` of the device possible in a headless test.
    let mut child = Command::new("qemu-system-riscv64")
        .args([
            "-machine",
            "virt",
            "-bios",
            "none",
            "-no-reboot",
            "-display",
            "none",
            "-device",
            "ramfb",
        ])
        .arg("-serial")
        .arg(format!("file:{}", serial.display()))
        .args(["-monitor", "stdio"])
        .arg("-kernel")
        .arg(&bin)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn qemu-system-riscv64");

    // Wait for the run to finish, then ask the monitor for the screen. Both waits are bounded by
    // the same 20 s the other boot tests use.
    let deadline = Instant::now() + Duration::from_secs(20);
    let serial_text = wait_for_file_text(&serial, &deadline, |text| text.contains("\nend\n"));
    let mut stdin = child.stdin.take().expect("qemu's monitor pipe");
    writeln!(stdin, "screendump {}", shot.display()).expect("failed to ask for a screendump");
    stdin.flush().expect("failed to flush the monitor pipe");
    let _ = wait_for_file_text(&shot, &deadline, |_| true);
    writeln!(stdin, "quit").expect("failed to ask qemu to quit");
    let _ = stdin.flush();
    let _ = child.kill();
    let _ = child.wait();

    let ppm = std::fs::read(&shot).expect("the PPM the monitor wrote");
    let (width, height, body) = parse_ppm(&ppm);
    assert_eq!((width, height), (320, 200), "the screen the ramfb device reported");
    assert_eq!(body.len(), 320 * 200 * 3, "the PPM body length");
    let shown = ppm_rgb_checksum(width, height, body);

    // The kernel's own line, from its serial output: `display 0x<16 hex digits>`.
    let line = serial_text
        .lines()
        .find(|line| line.starts_with("display "))
        .unwrap_or_else(|| panic!("no `display` line in the kernel's serial output:\n{serial_text}"));
    let expected = u64::from_str_radix(line.trim_start_matches("display 0x"), 16)
        .unwrap_or_else(|_| panic!("unreadable display line '{line}'"));
    assert_eq!(
        shown, expected,
        "the display device's pixels differ from the kernel's framebuffer ({line})"
    );
}

/// Reads `path` until `done` accepts its text, or the deadline passes, and answers the text it last
/// saw (empty when the file never appeared). The bounded wait is what keeps a kernel that never
/// prints, or a monitor that never writes, from hanging the suite.
fn wait_for_file_text(path: &Path, deadline: &Instant, done: impl Fn(&str) -> bool) -> String {
    let mut last = String::new();
    while Instant::now() < *deadline {
        if let Ok(text) = std::fs::read_to_string(path) {
            if done(&text) {
                return text;
            }
            last = text;
        }
        thread::sleep(Duration::from_millis(50));
    }
    last
}

/// A binary P6 PPM as `(width, height, body)`. The three header lines are `P6`, the size and the
/// maximum value, each terminated by a newline, which is what QEMU's `screendump` and
/// `soft/debug.dump_ppm` both write.
fn parse_ppm(bytes: &[u8]) -> (usize, usize, &[u8]) {
    let mut parts = bytes.splitn(4, |b| *b == b'\n');
    assert_eq!(parts.next(), Some(b"P6".as_slice()), "the PPM magic");
    let size = parts.next().expect("the PPM size line");
    let mut numbers = size
        .split(|b| *b == b' ')
        .map(|n| String::from_utf8_lossy(n).parse::<usize>().expect("a PPM dimension"));
    let width = numbers.next().expect("the PPM width");
    let height = numbers.next().expect("the PPM height");
    assert_eq!(parts.next(), Some(b"255".as_slice()), "the PPM maximum value");
    (width, height, parts.next().expect("the PPM body"))
}

/// The raster's `checksum_rgb` over a P6 body: FNV-1a 64 with each pixel's blue, green and red bytes
/// and a constant `0xFF` for the fourth. The kernel prints the same number on its `display` line, so
/// a display device that shows the framebuffer produces the same value.
fn ppm_rgb_checksum(width: usize, height: usize, body: &[u8]) -> u64 {
    let mut h: u64 = 14695981039346656037;
    for i in 0..width * height {
        let (r, g, b) = (body[3 * i], body[3 * i + 1], body[3 * i + 2]);
        for byte in [b, g, r, 255u8] {
            h ^= u64::from(byte);
            h = h.wrapping_mul(1099511628211);
        }
    }
    h
}

/// Polyp v3 stage 3: `PpmPlatform` writes each presented frame when `POLYP_DUMP_DIR` is set.
#[test]
fn polyp_soft_ppm_platform_writes_a_frame() {
    let bin = out_path("polyp_ui_test_ppm");
    let build = kelpc()
        .arg("build")
        .arg(fixture("../examples/polyp-ui-test"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected the polyp ui test project to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let dir = out_path("polyp_ui_ppm");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("the ppm dump directory");
    let run = run_bounded(
        &bin,
        &[
            ("POLYP_UI_TEST", "ppm"),
            ("POLYP_DUMP_DIR", dir.to_str().unwrap()),
        ],
    );
    assert!(
        run.status.success(),
        "expected the ppm run to exit successfully, got:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout), "ppm\n1\n1\n");
    let ppm = dir.join("frame_0.ppm");
    let bytes = std::fs::read(&ppm).expect("the PPM the platform wrote");
    assert_eq!(&bytes[..13], b"P6\n64 48\n255\n", "the PPM header");
    assert_eq!(bytes.len(), 13 + 64 * 48 * 3, "the PPM body length");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Polyp v3 stage 5: the soft backend's work budgets, pinned as exact machine-independent counts.
///
/// `examples/polyp-budget-test` drives the backend directly and prints one number per line:
///
/// - `idle`: an idle frame writes 0 pixels, presents 0 rectangles and allocates nothing.
/// - `text`: a same-width label change damages one rectangle (area 168, the 24x7 label) and, after
///   the patch buffer has seen that shape, accounts for 0 allocations.
/// - `hover`/`focus`: a hover or focus change damages exactly the affected widget(s), and allocates
///   nothing. The primitive pixel count is higher than the rectangle area because a repaint fills
///   the rectangle with the background and then draws the widget over it (the overdraw `repaint`
///   measures).
/// - `type`: typing one character damages at most the entry's rectangle (area 1800) and accounts
///   for the one allocation the new text needs.
/// - `checkbox`, `progress`, `caret`, `scroll`: a checkbox flip, a progress change, a caret move
///   and a wheel step each damage exactly one rectangle (areas 504, 720, 1800 and 600000) and
///   account for 0 allocations. The caret move leaves the text alone and the caret at byte 4.
/// - `remove`: dropping one row of 200 re-measures one node, re-arranges the tree, and damages only
///   rectangles at or below the removed row (`70` is the smallest damage `y`); nothing above it is
///   touched.
/// - `append`: adding a row at the end damages the new (off-screen) row and nothing else.
/// - `repaint`: a full 800x600 repaint overdraws 1.969x, under the factor 2 the budget allows.
/// - `deadline`: a 2500 us budget and a clock that charges 1000 us per read stop the first frame
///   after three rectangles (`presented 3`, `damage 2`) and leave the last two for the next frame
///   (`presented 2`, `damage 0`); the second frame is never deferred.
/// - `allocs`: an idle frame and a steady-state patch frame both account for 0 allocations; the
///   patch buffer's high-water is 1 patch.
#[test]
fn polyp_soft_work_budgets_hold() {
    let bin = out_path("polyp_budget_test");
    let build = kelpc()
        .arg("build")
        .arg(fixture("../examples/polyp-budget-test"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected the polyp budget test project to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = run_bounded(&bin, &[]);
    assert!(
        run.status.success(),
        "expected the budget test to exit successfully, got:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        POLYP_BUDGET_EXPECTED,
        "unexpected output from the budget test"
    );
}

const POLYP_BUDGET_EXPECTED: &str = r"idle
0
0
0
0
text
1
168
212
0
1
0
hover
1
440
988
0
focus
1
440
1064
0
type
1
1800
4114
1
Xabcd
remove
16
3712864
832351
1
1002
70
row
append
1
15680
0
6
1012
checkbox
1
504
591
0
progress
1
720
1764
0
caret
1
1800
4101
0
4
scroll
1
600000
967616
0
repaint
1
945223
480000
1969
deadline
deferred
3
2
done
2
0
allocs
0
0
1
";

/// Polyp v3 stage 5: the benchmark builds and prints a table.
///
/// `examples/polyp-bench` runs the five scenarios in `plan/polyp-v3.md`'s table. The test sets
/// `POLYP_BENCH_QUICK`, which shortens every scenario to one frame per run, so the whole run fits
/// in the bounded runner on the slower C oracle; the counts it pins are the same numbers the full
/// run's table carries. The wall-clock columns are the host's and are only checked to be numbers.
#[test]
fn polyp_bench_prints_a_bounded_table() {
    let bin = out_path("polyp_bench");
    let build = kelpc()
        .arg("build")
        .arg(fixture("../examples/polyp-bench"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected the polyp bench project to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = run_bounded(&bin, &[("POLYP_BENCH_QUICK", "1")]);
    assert!(
        run.status.success(),
        "expected the benchmark to exit successfully, got:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let text = String::from_utf8_lossy(&run.stdout);
    let lines: Vec<&str> = text.lines().collect();
    // Five scenarios, seven lines each: name, frames, pixels, rects, allocs, median, worst.
    assert_eq!(lines.len(), 35, "the benchmark's table shape changed:\n{text}");
    let expected = [
        ("scroll-200", 5, 4393660, 5, 0),
        ("type-100", 5, 163730, 5, 5),
        ("toggle-50", 5, 231965, 10, 0),
        ("resize", 5, 7775980, 80, 0),
        ("repaint", 5, 4726115, 5, 0),
    ];
    for (i, (name, frames, pixels, rects, allocs)) in expected.iter().enumerate() {
        let base = i * 7;
        assert_eq!(lines[base], *name, "scenario {i}'s name");
        assert_eq!(lines[base + 1], frames.to_string(), "{name} frames");
        assert_eq!(lines[base + 2], pixels.to_string(), "{name} pixels");
        assert_eq!(lines[base + 3], rects.to_string(), "{name} rects");
        assert_eq!(lines[base + 4], allocs.to_string(), "{name} allocations");
        for k in 5..7 {
            let us: i64 = lines[base + k].parse().unwrap_or_else(|_| {
                panic!("{name}'s time column is not a number: {}", lines[base + k])
            });
            assert!(us >= 0, "{name}'s time column is negative");
        }
    }
}

/// Build one Polyp v4 example project, run it bounded and return its stdout. Every v4 test below is
/// a project in `examples/` that prints a deterministic transcript; the file under
/// `tests/polyp_v4/` is the pinned copy, so a changed number is a visible diff.
fn polyp_v4_transcript(project: &str, tag: &str, envs: &[(&str, &str)]) -> String {
    let bin = out_path(tag);
    let build = kelpc()
        .arg("build")
        .arg(fixture(&format!("../examples/{project}")))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected {project} to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = run_bounded(&bin, envs);
    assert!(
        run.status.success(),
        "expected {project} to exit successfully, got:\n{}\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    String::from_utf8_lossy(&run.stdout).into_owned()
}

/// Polyp v4 stage 1: timers and animation on the headless soft backend, driven by the test clock.
///
/// `examples/polyp-anim-test` pins the integer easings, a progress bar animating 0..1000 permille over
/// 500 ms (exact values at exact ticks, damage only over the bar, `wants_frames` true while it moves
/// and false after), a restart on a changed `Anim`, a spacer animation that re-lays-out what is below
/// it, opacity and offset animations, a repeating timer that skips the periods it missed, a one-shot
/// that fires once and re-arms when the state asks again, and a UI at rest.
#[test]
fn polyp_v4_timers_and_animation_are_exact_on_the_test_clock() {
    let got = polyp_v4_transcript("polyp-anim-test", "polyp_anim_test", &[]);
    assert_eq!(got, include_str!("polyp_v4/anim.expected"), "unexpected output from the animation test");
}

/// Polyp v4 stage 2: UTF-8 text in the soft backend.
///
/// `examples/polyp-text-test` pins the decoder (well-formed 1- to 4-byte sequences, stray and
/// truncated bytes, overlong forms, surrogates, values above U+10FFFF: each malformed run is one
/// unit), measure by code point ("héllo" is 5 cells), which blocks have drawings, that a missing
/// glyph and a malformed run draw the same single box and a truncated tail reads nothing out of
/// bounds, and entry editing by code point (caret moves, backspace on `é`, delete on a 3-byte
/// scalar, a malformed byte, a caret stranded inside a sequence, an invalid scalar).
#[test]
fn polyp_v4_text_is_utf8_by_code_point() {
    let got = polyp_v4_transcript("polyp-text-test", "polyp_text_test", &[]);
    assert_eq!(got, include_str!("polyp_v4/text.expected"), "unexpected output from the text test");
}

/// Polyp v4 stage 3: the accessibility tree.
///
/// `examples/polyp-a11y-test` pins the todo app's tree dump, that pressing "add" by node id ends in the
/// same model and focus as a click at its bounds, focus by id, `set_value` on an entry, a checkbox
/// toggled by id (named after the label beside it), and that a disabled button and entry reject every
/// action, ignore clicks and are skipped by Tab.
#[test]
fn polyp_v4_a11y_tree_drives_the_ui() {
    let got = polyp_v4_transcript("polyp-a11y-test", "polyp_a11y_test", &[]);
    assert_eq!(got, include_str!("polyp_v4/a11y.expected"), "unexpected output from the a11y test");
}

/// Polyp v4 stage 4: memoised subtrees.
///
/// `examples/polyp-memo-test` runs a scrolled list of memoised rows beside the same list built with
/// plain keyed rows and requires the same framebuffer checksum after every change; it pins the
/// counters: 200 rows are 200 misses the first time, one changed row is 1 view call and 199 hits,
/// no change is 200 hits and nothing painted, a deps change rebuilds exactly that row, a reorder
/// keeps every memo a hit, a removed row's record is dropped and its key built again when it comes
/// back, a nested memo hits when only the outer deps changed, a press inside a memo row reaches the
/// model, and the plain loop with no store draws the same screen.
#[test]
fn polyp_v4_memo_skips_unchanged_subtrees_and_never_changes_a_pixel() {
    let got = polyp_v4_transcript("polyp-memo-test", "polyp_memo_test", &[]);
    assert_eq!(got, include_str!("polyp_v4/memo.expected"), "unexpected output from the memo test");
}

/// Polyp v4 stage 4: the memoisation benchmark builds and its counters are exact. The times
/// (columns 7 to 9) are the host's; `POLYP_BENCH_QUICK` shortens each run to 3 frames.
#[test]
fn polyp_v4_memo_bench_counts_view_calls_and_hits() {
    let got = polyp_v4_transcript("polyp-memo-bench", "polyp_memo_bench", &[("POLYP_BENCH_QUICK", "1")]);
    let lines: Vec<&str> = got.lines().collect();
    // Four runs, nine lines each: label, rows, frames, view calls, hits, pixels, median, view, diff.
    assert_eq!(lines.len(), 36, "the memo benchmark's shape changed:\n{got}");
    let expect = [("plain", 200, 0, 0), ("memo", 200, 3, 3 * 199), ("plain", 1000, 0, 0), ("memo", 1000, 3, 3 * 999)];
    for (i, (label, rows, calls, hits)) in expect.iter().enumerate() {
        let b = i * 9;
        assert_eq!(lines[b], *label);
        assert_eq!(lines[b + 1], rows.to_string(), "{label} rows");
        assert_eq!(lines[b + 2], "3", "{label} frames");
        assert_eq!(lines[b + 3], calls.to_string(), "{label} view calls");
        assert_eq!(lines[b + 4], hits.to_string(), "{label} hits");
        for k in 6..9 {
            lines[b + k].parse::<i64>().unwrap_or_else(|_| panic!("{label}: column {k} is not a number"));
        }
    }
    // Memo and plain write the same pixels.
    assert_eq!(lines[5], lines[9 + 5], "200 rows: pixels written");
    assert_eq!(lines[18 + 5], lines[27 + 5], "1000 rows: pixels written");
}

/// The lines of one Polyp v4 demo run: from the first `step ` line to the first `end` line, without
/// the hosted entry's `-- demo --` separator. Used on the hosted transcript and on a kernel's serial
/// output alike, so the two are sliced identically and only the run itself can differ.
fn v4_run_slice(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines
        .iter()
        .position(|line| line.starts_with("step "))
        .unwrap_or_else(|| panic!("no step line in the run's output:\n{text}"));
    let end = lines
        .iter()
        .position(|line| *line == "end")
        .unwrap_or_else(|| panic!("no `end` line in the run's output:\n{text}"));
    lines[start..=end]
        .iter()
        .filter(|line| **line != "-- demo --")
        .map(|line| line.to_string())
        .collect()
}

/// Polyp v4: the demo program on the riscv64 kernel prints the hosted run's own transcript.
///
/// `examples/polyp-v4-riscv64` is the same `app.kp`/`scenario.kp` as `examples/polyp-v4-demo`, with a
/// platform clock the kernel advances and a `Tick` event it queues for it: every step line (clock,
/// framebuffer checksum, rectangles presented, `wants_frames`, timer count) and the report must equal
/// the hosted line. Skipped under `dev.nix`; run it in `dev-kernel.nix`.
#[test]
fn polyp_v4_riscv64_prints_the_hosted_transcript() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU) {
        eprintln!("skipping the riscv64 Polyp v4 kernel: 'ld.lld' or 'qemu-system-riscv64' is not on PATH");
        return;
    }
    let expected = v4_run_slice(&polyp_v4_transcript("polyp-v4-demo", "polyp_v4_demo_hosted_rv", &[]));
    let name = "polyp_v4_riscv64";
    let captured = boot_and_capture(&RISCV64_QEMU, name, &fixture("../examples/polyp-v4-riscv64"), &|got| {
        got.lines().any(|line| line == "end")
    });
    let undefined = undefined_symbols(&out_path(name));
    assert!(
        undefined.trim().is_empty(),
        "the riscv64 Polyp v4 image names undefined symbols:\n{undefined}"
    );
    assert_eq!(v4_run_slice(&captured), expected, "the riscv64 kernel differs from the hosted run; captured:\n{captured}");
}

/// The same on the x86-64 kernel (`examples/polyp-v4-x86_64`).
#[test]
fn polyp_v4_x86_64_prints_the_hosted_transcript() {
    if !have_ld_lld() || !have_qemu(&X86_64_QEMU) {
        eprintln!("skipping the x86-64 Polyp v4 kernel: 'ld.lld' or 'qemu-system-x86_64' is not on PATH");
        return;
    }
    let expected = v4_run_slice(&polyp_v4_transcript("polyp-v4-demo", "polyp_v4_demo_hosted_x86", &[]));
    let name = "polyp_v4_x86_64";
    let captured = boot_and_capture(&X86_64_QEMU, name, &fixture("../examples/polyp-v4-x86_64"), &|got| {
        got.lines().any(|line| line == "end")
    });
    let undefined = undefined_symbols(&out_path(name));
    assert!(
        undefined.trim().is_empty(),
        "the x86-64 Polyp v4 image names undefined symbols:\n{undefined}"
    );
    assert_eq!(v4_run_slice(&captured), expected, "the x86-64 kernel differs from the hosted run; captured:\n{captured}");
}

/// Polyp v4: the hosted demo transcript is pinned, so a changed number is a visible diff.
#[test]
fn polyp_v4_demo_prints_its_pinned_transcript() {
    let got = polyp_v4_transcript("polyp-v4-demo", "polyp_v4_demo", &[]);
    assert_eq!(got, include_str!("polyp_v4/demo.expected"), "unexpected output from the v4 demo");
}

#[test]
fn a_while_true_loop_that_breaks_continues_after_the_loop() {
    assert_compiles_and_runs("tests/fixtures/while_true_break.kp", "4\n5\n30\nxxx\n");
}

// Language-completeness 8: the interaction matrix of the traits, iterators, const
// generics, operator and conversion features (`plan/traits.md` 14). Each cell is a
// fixture; a refused cell pins its message, an accepted one its output.
#[test]
fn matrix_associated_types_cannot_be_boxed_as_a_trait_object() {
    assert_rejected_with(
        "tests/fixtures/matrix_assoc_boxdyn.kp",
        "'Box<dyn Source>' is not allowed: the trait declares the associated type 'Item'",
    );
}

#[test]
fn matrix_an_operator_trait_takes_no_type_argument() {
    assert_rejected_with(
        "tests/fixtures/matrix_generic_trait_operator.kp",
        "takes no type arguments",
    );
}

#[test]
fn matrix_a_user_iterator_runs_defers_per_iteration_and_on_return() {
    assert_compiles_and_runs(
        "tests/fixtures/matrix_iter_defer_return.kp",
        "iter\niter\nleave\n4\niter\nleave\n-1\n",
    );
}

#[test]
fn matrix_a_const_generic_type_is_not_an_iterator_but_a_cursor_walks_it() {
    assert_rejected_with(
        "tests/fixtures/matrix_iter_const_generic.kp",
        "implement the trait for a non-generic wrapper",
    );
    assert_compiles_and_runs(
        "tests/fixtures/matrix_iter_const_generic_ok.kp",
        "15\n30\n4\n5\n6\n",
    );
}

#[test]
fn matrix_question_mark_inside_next_must_match_the_option_shape() {
    assert_rejected_with(
        "tests/fixtures/matrix_from_try_in_next.kp",
        "match the value instead",
    );
    assert_compiles_and_runs("tests/fixtures/matrix_from_try_ok.kp", "11\n21\n");
}

#[test]
fn matrix_display_works_through_a_generic_bound_and_a_template() {
    assert_compiles_and_runs(
        "tests/fixtures/matrix_display_generic.kp",
        "<1,2>\n#9\n<1,2> <1,2>\n",
    );
}

#[test]
fn matrix_ord_never_decides_map_key_identity() {
    assert_compiles_and_runs("tests/fixtures/matrix_ord_map_key.kp", "2\n11\ntrue\n");
    assert_compiles_and_runs(
        "tests/fixtures/matrix_ord_only_key.kp",
        "2\nfalse\ntrue\n",
    );
}

/// Builds `fixture` with the named backend (`"c"` for the oracle, anything
/// else for the default) and returns the build and the run, so a test can pin
/// both backends in one go regardless of which one the suite is pointed at.
fn build_and_run_on(fixture_name: &str, oracle: bool) -> (std::process::Output, Option<std::process::Output>) {
    let tag = if oracle { "c" } else { "llvm" };
    let bin = out_path(&format!("{fixture_name}_{tag}"));
    let mut cmd = Command::new(kelpc_bin());
    cmd.env_remove("KELPC_BACKEND");
    if oracle {
        cmd.env("KELPC_ORACLE", "c");
    } else {
        cmd.env_remove("KELPC_ORACLE");
    }
    let build = cmd
        .arg("build")
        .arg(fixture(&format!("tests/fixtures/{fixture_name}.kp")))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    if !build.status.success() {
        return (build, None);
    }
    let run = Command::new(&bin).output().expect("failed to run the binary");
    (build, Some(run))
}

/// Shifts (`tl7`): an in-range shift and the `wrapping_shl`/`wrapping_shr`
/// escapes print the same on both backends.
#[test]
fn shifts_in_range_and_wrapping_shifts_agree_on_both_backends() {
    let expected = "128\n1\n240\n32768\n1\n2147483648\n1\n-2147483648\n-1\n-4\n9223372036854775808\n1\n4611686018427387904\n-1\n48\n5\n2\n1\n2147483648\n268435456\n2\n2\n-8\n-2147483648\n";
    for oracle in [false, true] {
        let (build, run) = build_and_run_on("shift_checked", oracle);
        let run = run.unwrap_or_else(|| {
            panic!("shift_checked failed to build:\n{}", String::from_utf8_lossy(&build.stderr))
        });
        assert!(run.status.success(), "shift_checked (oracle={oracle}) failed:\n{}", String::from_utf8_lossy(&run.stderr));
        assert_eq!(String::from_utf8_lossy(&run.stdout), expected, "oracle={oracle}");
    }
}

/// Shifts (`tl7`): a shift by the width or more, or a negative amount, panics
/// with the same message on both backends, at every width, and the legal shift
/// printed before it still appears.
#[test]
fn a_shift_by_the_width_or_more_panics_on_both_backends() {
    for (name, first) in [
        ("shift_panic_u32", "2147483648\n"),
        ("shift_panic_u8_right", "1\n"),
        ("shift_panic_negative", "-8\n"),
        ("shift_panic_u64", "9223372036854775808\n"),
    ] {
        for oracle in [false, true] {
            let (build, run) = build_and_run_on(name, oracle);
            let run = run.unwrap_or_else(|| {
                panic!("{name} failed to build:\n{}", String::from_utf8_lossy(&build.stderr))
            });
            assert!(!run.status.success(), "{name} (oracle={oracle}) should panic");
            assert_eq!(String::from_utf8_lossy(&run.stdout), first, "{name} oracle={oracle}");
            let stderr = String::from_utf8_lossy(&run.stderr);
            assert!(
                stderr.contains("shift amount out of range"),
                "{name} (oracle={oracle}) panicked with:\n{stderr}"
            );
        }
    }
}

/// Shifts (`tl7`): a shift by a constant amount at or past the width is a
/// compile error that names the type and the amount, on both backends.
#[test]
fn a_shift_by_a_constant_at_or_past_the_width_is_a_compile_error() {
    for oracle in [false, true] {
        for (name, needle) in [
            ("shift_const_too_wide", "a shift by 33 is not defined for a 'u32'"),
            ("shift_const_named_too_wide", "a shift by 8 is not defined for a 'u8'"),
        ] {
            let (build, run) = build_and_run_on(name, oracle);
            assert!(run.is_none(), "{name} (oracle={oracle}) should be rejected");
            let stderr = String::from_utf8_lossy(&build.stderr);
            assert!(stderr.contains(needle), "{name}: expected '{needle}', got:\n{stderr}");
        }
    }
}

/// Shifts (`tl7`): the freestanding half. The check reaches the project's own
/// panic handler, the image has no undefined symbol, and it boots to the
/// message. Needs the `dev-kernel.nix` tools; a shell without them skips.
#[test]
fn a_freestanding_shift_out_of_range_reaches_the_projects_panic_handler() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU) {
        println!(
            "skipping the freestanding shift boot: 'ld.lld' or 'qemu-system-riscv64' is not on \
             PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = out_path("freestanding_shift");
    let build = Command::new(kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .env_remove("KELPC_ORACLE")
        .arg("build")
        .arg(project("freestanding_shift"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the freestanding shift build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let symbols = tool("llvm-nm", &[bin.to_str().expect("the artifact path is utf-8")]);
    let undefined: Vec<&str> = symbols
        .lines()
        .filter(|line| line.split_whitespace().next() == Some("U"))
        .collect();
    assert!(undefined.is_empty(), "undefined symbols:\n{}", undefined.join("\n"));
    boot_under_qemu(
        &RISCV64_QEMU,
        "freestanding_shift",
        &project("freestanding_shift"),
        &["8", "panic: shift amount out of range"],
    );
}

// ---------------------------------------------------------------------------
// Evaluation order: every call argument, template interpolation, operator
// operand and aggregate field is evaluated strictly left to right on both
// backends (`plan/syntax.md`, section 5). The fixtures record the call order
// into a passdown and print it, so a C compiler's own order shows up as a diff.
// ---------------------------------------------------------------------------

#[test]
fn eval_order_template_interpolations_run_left_to_right() {
    assert_compiles_and_runs("tests/fixtures/eval_order_template.kp", "removed 10, 1 left\n");
}

#[test]
fn ordered_binary_conditions_are_c_expressions() {
    // The C backend stages the left operand of an effectful binary operation.
    // In a condition the resulting GNU statement expression needs its own
    // parentheses, rather than borrowing `if` or `while`'s opening parenthesis.
    assert_compiles_and_runs("tests/fixtures/eval_order_condition.kp", "12\n1234\n");
}

#[test]
fn eval_order_basic_transcript_is_left_to_right() {
    assert_compiles_and_runs("tests/fixtures/eval_order_basic.kp", "x 1 2 3\ntmpl: 123\n6\ncall: 123\nbinop: 123\nctor: 123\narray: 123\npush: 12\nand: 1\nor: 1\nset: 12\ncompound: 7\n195\nnested: 12345\n7 4\narith: 1234\nrecv: 123\n6\nrecv2: 123\n");
}

#[test]
fn eval_order_aggregates_operators_and_indirect_calls_are_left_to_right() {
    assert_compiles_and_runs("tests/fixtures/eval_order_more.kp", "enum: 12\nctor_swapped: 21\nindirect: 12\nindirect_plus: 345\ncmp: 12\nchain: 1234\n2 1 2\nreads: 12\nread_then_call: 5\nif: 12\narr_expr: 1234\nclosure: 1828\n");
}

// ---------------------------------------------------------------------------
// Several lock-typed `pd val` globals in one program. The LLVM module
// initialiser used to restart every global in the entry block, leaving the
// first global's `alloc.ok` block without a terminator.
// ---------------------------------------------------------------------------

#[test]
fn two_mutex_a_rwlock_and_a_condvar_global_initialise_together() {
    assert_compiles_and_runs("tests/fixtures/pd_lock_globals_pair.kp", "10\n21\n32\n");
}

#[test]
fn six_lock_globals_are_used_from_several_threads() {
    assert_compiles_and_runs(
        "tests/fixtures/pd_lock_globals_many.kp",
        "20\n21\n25\n27\n20\n",
    );
}
