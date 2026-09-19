//! End-to-end regression suite: compiles `.kp` fixtures with the real `kelpc`
//! binary, then either runs the resulting binary and checks its stdout, or
//! (for fixtures that should be rejected) checks that `kelpc build` fails.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
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
/// `KELPC_BACKEND` is the only input that varies it. Reading the default from
/// the environment rather than hard-coding `c` is what lets a second backend
/// be validated by this whole suite with no edits here — see
/// `plan/mk7-own-backend.md` stage 1.
fn backend() -> String {
    std::env::var("KELPC_BACKEND").unwrap_or_else(|_| "c".to_string())
}

/// A `kelpc` invocation pinned to the backend under test.
///
/// The variable is passed explicitly rather than inherited so that the suite's
/// child processes cannot disagree with `backend()` about which backend is
/// being exercised — an inherited value that differs from this one would make
/// a failure impossible to attribute.
fn kelpc() -> Command {
    let mut cmd = Command::new(kelpc_bin());
    cmd.env("KELPC_BACKEND", backend());
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
    p.push(format!("kelpc_test_{}_{name}", backend()));
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
fn moving_a_borrowed_value_is_rejected() {
    assert_rejected("tests/fixtures/move_while_borrowed.kp");
}

#[test]
fn receiver_methods_resolve_and_mutate_through_self() {
    assert_compiles_and_runs("tests/fixtures/receiver_methods.kp", "12.566360\n28.274310\n");
}

#[test]
fn mut_receiver_method_on_val_binding_is_rejected() {
    assert_rejected("tests/fixtures/receiver_mut_on_val.kp");
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
fn import_alias_reference_runs() {
    // `impl "util" as u` plus a qualified type in a signature (`u.Tally`).
    assert_project_runs("alias", &project("alias"), "7\n4\n");
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
fn unshipped_stdlib_module_is_rejected() {
    // `std/tls` is the next module in the roadmap (S4b, behind open item 37)
    // and is deliberately absent; the error names what *is* shipped so the
    // reservation stays useful. `std/http` shipped in S4 and is no longer the
    // example here.
    assert_project_rejected(
        "stdlib_import",
        &project("stdlib_import").join("main.kp"),
        "stdlib module 'tls' is not implemented yet (shipped today: std/mem",
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
    assert_rejected("tests/fixtures/template_not_printed.kp");
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
fn a_bytes_parameter_is_a_borrow_not_a_move() {
    assert_compiles_and_runs(
        "tests/fixtures/heap_param_borrow.kp",
        "2\nhi\nXi\nfalse\n",
    );
}

#[test]
fn a_borrowed_datac_field_is_rejected() {
    assert_rejected("tests/fixtures/datac_str_field.kp");
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
fn stdlib_text_builds_numbers_replaces_and_parses() {
    // Concatenation, replacement, and both live edges of the decimal
    // conversion: `u64`'s maximum and `i64`'s minimum, round-tripped through
    // the parsers because no literal can spell either one.
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
        "0\n-1234\n4294967295\n3.142\n1.5\n0\nff\nbeef\n007\nlong\n7..\n61 62\n\n007!\n",
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
fn owned_datac_fields_are_dropped_with_the_struct() {
    assert_compiles_and_runs(
        "tests/fixtures/string_field.kp",
        "first\n7\nafter scope\nsecond\n7\ndone\n",
    );
}

#[test]
fn an_owned_passdown_is_freed_at_exit() {
    assert_compiles_and_runs("tests/fixtures/pd_owned.kp", "== x ==\n== x ==\n");
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

/// A scratch directory the capability fixtures point at. Their patterns are
/// absolute (a grant must be), so the path has to exist before the program
/// runs and is removed afterwards.
fn caps_scratch(name: &str) -> PathBuf {
    // `/tmp` literally, not `std::env::temp_dir()`: the capability patterns in
    // the fixtures are source text and say `/tmp/kelp_caps_<name>/**`, so a
    // `TMPDIR` that points somewhere else (a sandbox, a CI runner) would leave
    // the harness writing one directory while the program reads another.
    let dir = PathBuf::from("/tmp").join(format!("kelp_caps_{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("could not create the capability scratch directory");
    dir
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
        &project("caps_granted").join("main.kp"),
        "hello caps\ntrue\ntrue\n",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_ungranted_module_cannot_name_the_gated_namespace() {
    assert_project_rejected(
        "caps_ungranted",
        &project("caps_ungranted").join("main.kp"),
        "needs the 'fs' capability, which module 'util' was not granted",
    );
}

#[test]
fn a_module_cannot_delegate_more_than_it_holds() {
    assert_project_rejected(
        "caps_escalation",
        &project("caps_escalation").join("main.kp"),
        "can delegate less than it has, never more",
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

#[test]
fn a_granted_module_can_write_rename_size_and_remove_inside_its_grant() {
    let dir = caps_scratch("write");
    assert_project_runs(
        "caps_write",
        &project("caps_write").join("main.kp"),
        "true\n5\nfalse\nhello\nfalse\ntrue\n3\n",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_path_outside_the_grant_is_denied_at_runtime() {
    assert_project_denied_at_runtime(
        "caps_denied",
        &project("caps_denied").join("main.kp"),
        "outside the fs capability granted to module 'util'",
    );
}

#[test]
fn a_symlink_inside_the_grant_pointing_outside_is_denied() {
    let dir = caps_scratch("symlink");
    std::os::unix::fs::symlink("/etc/passwd", dir.join("escape")).unwrap();
    assert_project_denied_at_runtime(
        "caps_symlink",
        &project("caps_symlink").join("main.kp"),
        "/etc/passwd",
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
        &project("caps_link").join("main.kp"),
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
        &project("caps_link_denied").join("main.kp"),
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

#[test]
fn a_view_carries_its_length_and_slices_without_allocating() {
    assert_compiles_and_runs(
        "tests/fixtures/view.kp",
        "the quick brown fox\n19\nfalse\ntrue\ntrue\nquick\nthe\nhe quick brown fox\ntrue\ntrue\nfalse\ntrue\nthe\nhe quick brown fox\n3\ntrue\n0\ntrue\nquick\nthe\nowned text\ntext\nquick brown\n11\n",
    );
}

#[test]
fn a_view_cannot_be_stored_in_a_datac() {
    assert_rejected("tests/fixtures/view_field.kp");
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
fn a_view_of_a_temporary_is_rejected() {
    assert_rejected("tests/fixtures/view_of_temporary.kp");
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

#[test]
fn character_literals_take_the_type_their_context_asks_for() {
    assert_compiles_and_runs(
        "tests/fixtures/char_literals.kp",
        "34\n48\n53\n10\n9\n92\n39\ntrue\ntrue\ntrue\ntrue\nfalse\ntrue\nfalse\ntrue\ntrue\nfalse\n122\n",
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
/// It is therefore also the one test that is **C-backend-only**: it recompiles
/// `kelpc`'s generated `.c` with `cc -fsanitize=address`, so under another
/// backend there is no such file and nothing to instrument. That is not a gap
/// this test can close by being cleverer — an own backend cannot borrow C's
/// sanitizer to verify its own code, which is exactly why `plan/mk7-own-backend.md`
/// records the transfer of the memory-safety argument as Tier 3's obligation
/// rather than this suite's. Under a second backend the test reports that and
/// passes, so the suite stays green while the obligation stays visible.
#[test]
fn heap_fixtures_are_double_free_and_leak_clean() {
    if backend() != "c" {
        eprintln!(
            "note: the AddressSanitizer gate instruments the generated C, so it is \
             C-backend-only; backend '{}' is not checked by it",
            backend()
        );
        return;
    }
    let fixtures = [
        "heap_bytes",
        "heap_move_then_rebind",
        "heap_returned",
        "heap_scope_drop",
        "heap_drops_in_control_flow",
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
        "pd_owned",
        // A passdown forwarded through a caller that does not name it: the
        // hidden argument is the global's address there, while an owned
        // passdown still has to be freed at exit by exactly one handler.
        "pd_argument_forwarding",
        "stdlib_mem",
        "stdlib_str",
        "stdlib_path",
        "stdlib_math",
        "string_views",
        "path_parts",
        "view_released_at_scope_end",
        "heap_temporary_print",
        "list_basic",
        "list_owned",
        "list_field",
        "stdlib_list",
        "borrow_temp_arg",
        "list_datac_element",
        "stdlib_sort",
        "list_slice_insert",
        "path_polish",
        "box_basic",
        "box_recursive",
        "recursive_datac",
        "box_field_take",
        "map_basic",
        "map_owned",
        "map_datac",
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
        "try_operator",
        "try_drops_frame",
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
        "closure_move",
        "stdlib_closure_callbacks",
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

#[test]
fn a_forward_reference_between_datacs_is_rejected() {
    assert_rejected("tests/fixtures/datac_forward_reference.kp");
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
fn sorting_and_slicing_a_list_of_owned_elements_moves_them_safely() {
    // The element shifts are moves, so this fixture runs under
    // AddressSanitizer: a double free or a leak here would be a real one.
    assert_compiles_and_runs(
        "tests/fixtures/list_slice_insert.kp",
        "apple\nfig\npear\ntrue\nfalse\n2\nfig\n4\nfig\napple\nfig\n3\npear\n",
    );
}

#[test]
fn list_sort_and_slice_reject_what_they_cannot_do() {
    // A `datac` element has no order, so `sort` names the reason rather than
    // failing in codegen; a `slice` range past the end panics at runtime.
    assert_rejected("tests/fixtures/list_unsortable_element.kp");
    assert_fixture_panics("list_slice_bounds", "List.slice: range out of bounds");
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
    // once, nesting, and a list a function only borrows.
    assert_compiles_and_runs(
        "tests/fixtures/for_loop.kp",
        "6\n3\n7\n8\n9\na\nb\nc\n1 2 3 2 4 6 3 6 9 \n6\n",
    );
}

#[test]
fn a_for_loop_needs_its_in() {
    // `in` is not a keyword (syntax.md 2.3): it is read contextually, so the
    // diagnostic says what is missing instead of resolving a function `in`.
    assert_rejected_with("tests/fixtures/for_missing_in.kp", "'in'");
}

#[test]
fn a_for_loop_only_walks_a_list() {
    // The desugar is `.len()`/`.get()`, so a non-list fails on `.len()` and the
    // span is the iterable — `for x in 5` points at `5`, not at the `for`.
    assert_rejected_with("tests/fixtures/for_not_a_list.kp", "has no method 'len'");
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
fn popping_an_empty_list_panics() {
    assert_fixture_panics("list_pop_empty", "List.pop: the list is empty");
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
fn an_owned_field_cannot_be_read_by_value() {
    // The copy would be a second owner of the same buffer; the field has to
    // be borrowed, or held in a `Box` and taken.
    assert_rejected("tests/fixtures/owned_field_read.kp");
}

#[test]
fn an_owned_field_in_a_box_can_be_taken_and_set() {
    assert_compiles_and_runs("tests/fixtures/box_field_take.kp", "1\ntrue\n1\n2\n");
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
fn a_container_cannot_be_a_list_element() {
    assert_rejected("tests/fixtures/list_container_element.kp");
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
    // The value is behind a pointer, the key is an owned `String`, and
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
fn a_map_cannot_hold_a_container_or_be_created_without_a_value_type() {
    assert_rejected("tests/fixtures/map_container_value.kp");
    assert_rejected("tests/fixtures/map_no_value_type.kp");
}

#[test]
fn a_map_key_can_be_a_scalar_and_the_shorthand_still_means_string() {
    assert_compiles_and_runs(
        "tests/fixtures/map_key_types.kp",
        "2\n99\ntrue\nfalse\n0\n2\n1\n1\n10\n2\nyes\nno\n70\n2\n6\nbb\n",
    );
}

#[test]
fn a_key_type_the_compiler_cannot_hash_or_compare_is_rejected() {
    for name in [
        // `nan != nan` would make an inserted key unfindable.
        "map_float_key",
        // A `datac` has no equality in v1.
        "map_datac_key",
        // One argument (`Map<V>`) or two (`Map<K, V>`), never three.
        "map_arity",
    ] {
        assert_rejected(&format!("tests/fixtures/{name}.kp"));
    }
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
fn a_shared_borrow_of_a_temporary_is_allowed_for_a_call() {
    assert_compiles_and_runs("tests/fixtures/borrow_temp_arg.kp", "3\n18\n");
}

#[test]
fn a_returned_borrow_cannot_come_from_a_temporary() {
    assert_rejected("tests/fixtures/borrow_temp_returned.kp");
}

#[test]
fn fs_list_returns_a_granted_directories_entries() {
    // A granted `stdlib/fs` module listing a directory it owns: the entries
    // come back as a `List<String>`, sorted, and a recursive clear built on
    // the same function leaves the directory gone.
    assert_project_runs(
        "caps_list",
        &project("caps_list").join("main.kp"),
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
        &project("process_in").join("main.kp"),
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
    // `net` resolves with no `with caps(...)` clause, and the runtime check is
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
/// false when the peer closed before a request arrived.
fn read_http_request(stream: &mut TcpStream) -> bool {
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
fn enum_misuse_is_rejected() {
    for name in [
        // Exhaustiveness, and arm/pattern shape.
        "enum_non_exhaustive",
        "enum_unknown_variant",
        "enum_duplicate_arm",
        "enum_bind_count",
        "enum_temp_scrutinee",
        "enum_datac_scrutinee",
        // Construction and access.
        "enum_ctor_name",
        "enum_bare_variant",
        "enum_field_access",
        "enum_method",
        // Declaration rules, shared with `datac`.
        "enum_datac_clash",
        "enum_self_by_value",
        // No partial moves: a payload binds by borrow, never by move.
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
fn if_let_misuse_is_rejected() {
    for name in ["if_let_unknown_variant", "if_let_bind_count"] {
        assert_rejected(&format!("tests/fixtures/{name}.kp"));
    }
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
        // An enum carries no methods in v1, generic or not.
        "generic_enum_method",
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
// `fun(T).R: U`, called through a local, a field, a passdown or a parameter.
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
        "expected 'fun(f64).R: f64'",
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
        "cannot pass a value of type 'fun(i32).R: i32' to 'print'",
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
    // function, and `?` propagating against the literal's own `.R`.
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
fn fun_literal_misuse_is_rejected() {
    // A passdown is a hidden argument a function value cannot carry, a borrow
    // return has no elision root, the tail is checked against `.R` like any
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
    // `result.map` rebuilds `Err`, `option.unwrap` hands the payload out, and
    // `list.filter` reads an accepted element out — each needs a `Copy`
    // payload or element, and each fails inside the module, so the message
    // names the module and says why.
    assert_rejected_with(
        "tests/fixtures/stdlib_combinators_owned_error.kp",
        "this is inside the shipped standard library ('std/result')",
    );
    assert_rejected_with(
        "tests/fixtures/stdlib_option_unwrap_owned.kp",
        "this is inside the shipped standard library ('std/option')",
    );
    assert_rejected_with(
        "tests/fixtures/stdlib_list_filter_owned.kp",
        "this is inside the shipped standard library ('std/list')",
    );
}

// ---------------------------------------------------------------------------
// A-4 phase E: captures — a `fun` literal that names a binding from the
// enclosing scope becomes a closure (`closure(T).R: U`), which owns the heap
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
        "declared as 'closure(i32).R: i32'",
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
    // `env!("CARGO_PKG_VERSION")` here is the same `[package] version` the
    // compiler takes its own string from, so this cannot pass by the two
    // drifting apart.
    let out = kelpc()
        .arg("version")
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        out.status.success(),
        "'kelpc version' should succeed, got:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        format!("kelpc {}\n", env!("CARGO_PKG_VERSION"))
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
// The backend seam (MK7 stage 1). `plan/mk7-own-backend.md` makes the selector
// the precondition for every later stage, so it is asserted end-to-end here
// rather than only in `backend.rs`'s unit tests: what matters is that the flag
// and the environment variable reach the compiler, not that a parser works.
// ---------------------------------------------------------------------------

/// The C backend is what every other test in this file is written against, so
/// it has to be reachable by the flag, by the environment, and by default.
/// Each case strips the other input, so a case passing because *both* were set
/// is not possible.
#[test]
fn the_c_backend_is_selectable_by_flag_environment_and_default() {
    let bin = out_path("backend_selectable");
    for case in ["default", "flag", "environment"] {
        let mut cmd = Command::new(kelpc_bin());
        cmd.env_remove("KELPC_BACKEND");
        cmd.arg("build");
        match case {
            "flag" => {
                cmd.arg("--backend=c");
            }
            "environment" => {
                cmd.env("KELPC_BACKEND", "c");
            }
            _ => {}
        }
        let out = cmd
            .arg(fixture("tests/fixtures/while_accumulator.kp"))
            .arg("-o")
            .arg(&bin)
            .output()
            .expect("failed to spawn kelpc");
        assert!(
            out.status.success(),
            "the backend should be selectable by {case}, got:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let run = Command::new(&bin).output().expect("failed to run the binary");
        assert_eq!(String::from_utf8_lossy(&run.stdout), "55\n");
    }
}

/// The differential gate, at the level the whole MK7 argument rests on: the
/// same fixture built by both backends must print the same bytes.
///
/// This is the one test that would catch a miscompile the fixture assertions
/// cannot, because those compare against a *literal* an author wrote — a wrong
/// literal and a wrong backend agree with each other. Comparing the two
/// backends against each other is what makes the C backend an oracle rather
/// than a second opinion, and it is why `codegen_c.rs` is never deleted
/// (`plan/mk7-own-backend.md` §1.1).
///
/// The fixture list is deliberately scalar programs: it grows as the asm
/// backend covers more, and a fixture that the asm backend still refuses makes
/// this test fail, which is the intended signal — the list is a coverage
/// assertion, not a wish list.
#[test]
fn the_two_backends_agree_on_the_scalar_fixtures() {
    for name in [
        "while_accumulator",
        "if_expr",
        "named_args",
        "bitwise",
        "asm_float_scalars",
    ] {
        let source = format!("tests/fixtures/{name}.kp");
        let mut outputs = Vec::new();
        for backend in ["c", "asm"] {
            let bin = out_path(&format!("differential_{backend}_{name}"));
            let build = Command::new(kelpc_bin())
                .args(["build", &format!("--backend={backend}")])
                .arg(fixture(&source))
                .arg("-o")
                .arg(&bin)
                .output()
                .expect("failed to spawn kelpc");
            assert!(
                build.status.success(),
                "the '{backend}' backend could not build '{source}':\n{}",
                String::from_utf8_lossy(&build.stderr)
            );
            let run = Command::new(&bin).output().expect("failed to run the binary");
            outputs.push(String::from_utf8_lossy(&run.stdout).to_string());
        }
        assert_eq!(
            outputs[0], outputs[1],
            "the 'c' and 'asm' backends disagree on '{source}'"
        );
    }
}

/// An explicit `--backend` must win, or a one-off override of a run whose
/// environment already names another backend would be impossible.
#[test]
fn an_explicit_backend_flag_wins_over_the_environment() {
    let out = kelpc()
        .args(["build", "--backend=c"])
        .arg(fixture("tests/fixtures/while_accumulator.kp"))
        .arg("-o")
        .arg(out_path("backend_flag_wins"))
        // A value that is not selectable yet: if the environment won, this
        // build would fail, so success is the assertion.
        .env("KELPC_BACKEND", "own")
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        out.status.success(),
        "the flag should have won over KELPC_BACKEND=own, got:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A typo and a planned-but-unbuilt backend are different mistakes, and the
/// messages must not be interchangeable: one is the caller's error, the other
/// is the roadmap's.
#[test]
fn an_unknown_backend_is_rejected_and_a_planned_one_says_so() {
    let build_with = |value: &str| {
        kelpc()
            .args(["build", "--backend", value])
            .arg(fixture("tests/fixtures/while_accumulator.kp"))
            .arg("-o")
            .arg(out_path("backend_rejected"))
            .output()
            .expect("failed to spawn kelpc")
    };

    let unknown = build_with("llvm");
    assert!(!unknown.status.success(), "an unknown backend must be rejected");
    let stderr = String::from_utf8_lossy(&unknown.stderr);
    assert!(stderr.contains("unknown backend 'llvm'"), "{stderr}");
    // The suggestion must not name a backend that cannot be selected. The names
    // are quoted so that "unknown" cannot satisfy the check by containing
    // "own".
    assert!(stderr.contains("expected one of:"), "{stderr}");
    assert!(!stderr.contains("'own'"), "{stderr}");

    // Only the tiers that are genuinely still ahead are reserved now that
    // `asm` is selectable: a reserved name is a roadmap item, and reporting an
    // implemented one that way would be as wrong as calling a typo planned.
    for planned in ["own"] {
        let out = build_with(planned);
        assert!(!out.status.success(), "'{planned}' is not implemented yet");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains(&format!("backend '{planned}'"))
                && stderr.contains("planned but not implemented"),
            "'{planned}' should be reported as planned, got:\n{stderr}"
        );
    }
}

/// An empty value is a mistake, not an instruction to use the default: a
/// script that computes the backend name and produces an empty string must
/// fail rather than silently build with the wrong one.
#[test]
fn an_empty_backend_value_is_reported_rather_than_defaulted() {
    let out = kelpc()
        .args(["build"])
        .arg(fixture("tests/fixtures/while_accumulator.kp"))
        .arg("-o")
        .arg(out_path("backend_empty"))
        .env("KELPC_BACKEND", "")
        .output()
        .expect("failed to spawn kelpc");
    assert!(!out.status.success(), "an empty backend must be reported");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("KELPC_BACKEND is set but empty"), "{stderr}");
}
