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
         50 4b 01 02 de ad be ef 90 1f\ntrue\n",
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
    // `util.kp:23:31` is `fs.read_text("/etc/passwd")` in the control, as the
    // compiler was invoked with the file. (`pub` on the control's own line is
    // what moved the column from 27; A-13.)
    let expected = format!(
        "{}:23:31: panic: '/etc/passwd' is outside the fs capability granted to module 'util'\n  \
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
        "{}:20:30: panic: '/etc/passwd' is outside the fs capability granted to module 'util'\n  \
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
fn a_declared_key_pair_that_is_half_or_misshaped_is_rejected() {
    // A-12: either method name states that the type is a key, so half a pair,
    // or a signature the generated map code cannot call, is refused at the
    // declaration with the reason named rather than as a vague "not a key" at
    // a `Map` use site.
    assert_rejected_with(
        "tests/fixtures/map_key_pair_only_hash.kp",
        "'Half' declares 'hash' but not 'eq'",
    );
    assert_rejected_with(
        "tests/fixtures/map_key_pair_only_eq.kp",
        "'Half' declares 'eq' but not 'hash'",
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
fn a_returned_borrow_cannot_come_from_a_temporary() {
    assert_rejected("tests/fixtures/borrow_temp_returned.kp");
}

/// The field spelling of the same refusal: `mkName().name` is a field of an
/// rvalue, so a borrow that outlives the call has no storage to live in. It
/// is checked by message, not just by rejection, because the shape *does*
/// build whenever the borrow is only read inside the call. The two have to be
/// told apart, and this is the one that must not quietly start compiling.
#[test]
fn a_returned_borrow_cannot_come_from_a_field_of_a_temporary() {
    assert_rejected_with(
        "tests/fixtures/borrow_field_of_temp_returned.kp",
        "this call returns a borrow of its parameter 'self'",
    );
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
    // above are still in use; nothing reads it.
    _dir: tempfile_dir::TempDir,
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

fn generate_test_cert() -> TestCert {
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

    let mut builder = X509::builder().expect("failed to start a certificate builder");
    builder.set_version(2).expect("failed to set the certificate version");
    builder
        .set_serial_number(&serial.to_asn1_integer().expect("serial as an ASN.1 integer"))
        .expect("failed to set the serial number");
    builder.set_subject_name(&name).expect("failed to set the subject name");
    builder.set_issuer_name(&name).expect("failed to set the issuer name (self-signed)");
    builder.set_pubkey(&key).expect("failed to set the public key");
    builder
        .set_not_before(&Asn1Time::days_from_now(0).expect("not-before time"))
        .expect("failed to set not-before");
    builder
        .set_not_after(&Asn1Time::days_from_now(1).expect("not-after time"))
        .expect("failed to set not-after");
    builder
        .append_extension(BasicConstraints::new().critical().build().expect("basic constraints"))
        .expect("failed to append basic constraints");
    builder
        .append_extension(
            SubjectAlternativeName::new()
                .ip("127.0.0.1")
                .build(&builder.x509v3_context(None, None))
                .expect("subject alternative name"),
        )
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
    TestCert { cert_path, key_path, _dir: dir }
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
        use openssl::ssl::{SslAcceptor, SslFiletype, SslMethod};

        let cert = generate_test_cert();
        let mut acceptor_builder =
            SslAcceptor::mozilla_intermediate_v5(SslMethod::tls_server())
                .expect("failed to start an SslAcceptor");
        acceptor_builder
            .set_private_key_file(&cert.key_path, SslFiletype::PEM)
            .expect("failed to load the test private key");
        acceptor_builder
            .set_certificate_file(&cert.cert_path, SslFiletype::PEM)
            .expect("failed to load the test certificate");
        let acceptor = acceptor_builder.build();

        let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind loopback");
        let port = listener.local_addr().expect("local address").port();
        let accepted = Arc::new(AtomicUsize::new(0));
        let counter = accepted.clone();
        let handle = thread::spawn(move || serve_tls(listener, acceptor, connections, counter));
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
fn read_ws_head(stream: &mut TcpStream) -> String {
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
fn read_ws_frame(stream: &mut TcpStream) -> (u8, Vec<u8>) {
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

/// The scripted exchange, on one accepted connection. Every expectation here
/// is an assertion rather than a return value: a client that gets a step
/// wrong fails the test in this thread, and `WsServer::finish` turns that into
/// the test's own failure.
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
    let Some(mut stream) = stream.take() else { return };
    // Non-blocking was only for the accept loop; an accepted socket inherits
    // the listener's mode on Linux. The timeouts are what stop a client that
    // stalls mid-handshake from hanging the suite.
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(20)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(20)));
    accepted.fetch_add(1, Ordering::SeqCst);

    let head = read_ws_head(&mut stream);
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

    let (opcode, payload) = read_ws_frame(&mut stream);
    assert_eq!(
        opcode, 0xA,
        "the client answered the ping with opcode {opcode:#x}, not a pong"
    );
    assert_eq!(payload, b"ping?", "the pong carried a different payload");

    let mut close = 1000u16.to_be_bytes().to_vec();
    close.extend_from_slice(b"done");
    stream.write_all(&ws_frame(0x8, true, &close)).expect("close frame");

    let (opcode, payload) = read_ws_frame(&mut stream);
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
fn wss_is_refused_by_name_because_v1_has_no_tls() {
    // The scheme is the whole refusal: the URL is otherwise well formed and
    // the grant covers the host it names.
    assert_fixture_panics("ws_wss_refused", "v1 has no TLS");
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
fn a_documentation_comment_after_code_is_rejected() {
    assert_rejected_with(
        "tests/fixtures/doc_comment_trailing.kp",
        "must start a line",
    );
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
        // A method cannot be declared on a generic `enum`'s template.
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
        "field 'text' is a borrowed type ('View')",
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
