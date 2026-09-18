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

// ---------------------------------------------------------------------------
// M5: multi-file projects (`impl` imports, `alias.xconf`-free form).
//
// A project lives in `tests/projects/<name>/`. `xc build` accepts either the
// project directory (using `x.toml`'s entry point) or any `.x` file inside
// it; a directory with no manifest is supported too, in which case the file's
// own directory is the module root.
// ---------------------------------------------------------------------------

fn project(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("projects")
        .join(name)
}

/// Builds a project target (directory or `.x` file), runs the binary, and
/// asserts its stdout.
fn assert_project_runs(name: &str, target: &Path, expected: &str) {
    let bin = out_path(name);
    let build = Command::new(xc_bin())
        .arg("build")
        .arg(target)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn xc");
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
    let build = Command::new(xc_bin())
        .arg("build")
        .arg(target)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn xc");
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
    // `main.x` imports `shapes/circle` and `geom`; `shapes/circle.x` itself
    // imports `geom`, exercising a nested import with its own alias table.
    assert_project_runs(
        "shapes",
        &project("shapes"),
        "12.566360\n6.000000\n50.265440\n28.274310\n",
    );
}

#[test]
fn manifest_less_sibling_import_runs() {
    // No `x.toml`: the entry file's own directory is the module root.
    assert_project_runs(
        "loose",
        &project("loose").join("main.x"),
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
        &project("same_name").join("main.x"),
        "6\n9\n4\n13\n",
    );
}

#[test]
fn import_cycle_is_rejected() {
    assert_project_rejected(
        "cycle",
        &project("cycle").join("a.x"),
        "import cycle: a -> b -> a",
    );
}

#[test]
fn missing_module_is_rejected() {
    assert_project_rejected(
        "unknown_module",
        &project("unknown_module").join("main.x"),
        "cannot find module 'nope'",
    );
}

#[test]
fn duplicate_import_alias_is_rejected() {
    assert_project_rejected(
        "duplicate_alias",
        &project("duplicate_alias").join("main.x"),
        "alias 'circle' is already used by another import",
    );
}

#[test]
fn parent_directory_import_is_rejected() {
    assert_project_rejected(
        "dotdot",
        &project("dotdot").join("main.x"),
        "must not contain '.' or '..' segments",
    );
}

#[test]
fn non_identifier_module_path_is_rejected() {
    assert_project_rejected(
        "bad_path",
        &project("bad_path").join("main.x"),
        "is not a valid module name",
    );
}

#[test]
fn unknown_module_member_is_rejected() {
    assert_project_rejected(
        "unknown_member",
        &project("unknown_member").join("main.x"),
        "module 'prov' has no item 'Nope'",
    );
}

#[test]
fn unqualified_imported_item_is_rejected() {
    // Strict module-qualified namespace: a bare `Thing(...)` never resolves
    // to another module's `Thing`, and the error suggests the qualified form.
    assert_project_rejected(
        "unqualified_import",
        &project("unqualified_import").join("main.x"),
        "did you mean 'prov.Thing'?",
    );
}

#[test]
fn unshipped_stdlib_module_is_rejected() {
    // `std/net` is not shipped yet (M7 gates the domain but the module has no
    // items), and the error names what *is* shipped so the reservation stays
    // useful.
    assert_project_rejected(
        "stdlib_import",
        &project("stdlib_import").join("main.x"),
        "stdlib module 'net' is not implemented yet (shipped today: std/mem",
    );
}

#[test]
fn a_grant_pattern_that_is_not_a_path_is_rejected() {
    assert_project_rejected(
        "caps_bad_path_pattern",
        &project("caps_bad_path_pattern").join("main.x"),
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
        "tests/fixtures/passdowns.x",
        "shapes\nshapes\n[shapes] hits = 0\n42\n",
    );
}

#[test]
fn a_datac_passdown_is_read_through_a_shared_borrow() {
    assert_compiles_and_runs("tests/fixtures/passdown_datac.x", "(3, 4) sum=7\n7\n");
}

#[test]
fn a_local_can_shadow_a_passdown() {
    assert_compiles_and_runs("tests/fixtures/pd_shadowed.x", "local=inner\npd=outer\n");
}

#[test]
fn a_pd_var_is_writable_from_a_function_and_from_main() {
    // M6.4 replaced M5.2's "read-only inside a function": a `pd var` is
    // reached through an exclusive borrow, so writes work and stay checked.
    assert_compiles_and_runs(
        "tests/fixtures/pd_var_write.x",
        "count = 5\ncount = 100\n",
    );
}

#[test]
fn writing_a_pd_val_inside_a_function_is_rejected() {
    assert_rejected("tests/fixtures/pd_val_write.x");
}

#[test]
fn a_pd_var_cannot_be_borrowed_twice_in_one_call() {
    // The documented cost of M6.4's design, pinned so it reads as a rule.
    let bin = out_path("pd_var_double_borrow");
    let build = Command::new(xc_bin())
        .arg("build")
        .arg(fixture("tests/fixtures/pd_var_double_borrow.x"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn xc");
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
        "tests/fixtures/pd_var_copy_read_twice.x",
        "2\n20\n",
    );
}

#[test]
fn reading_a_passdown_before_it_is_declared_is_rejected() {
    assert_rejected("tests/fixtures/pd_forward_read.x");
}

#[test]
fn templates_print_literals_and_values() {
    assert_compiles_and_runs(
        "tests/fixtures/templates.x",
        "100% of {nothing} is 50%\narea = 12.566360\nr=2.000000 doubled=4.000000\nno newline here!\n",
    );
}

#[test]
fn a_template_outside_print_is_rejected() {
    assert_rejected("tests/fixtures/template_not_printed.x");
}

#[test]
fn a_passdown_is_not_visible_from_another_module() {
    assert_project_rejected(
        "pd_module_scope",
        &project("pd_module_scope").join("main.x"),
        "is a passdown of module 'util'",
    );
}

// ---------------------------------------------------------------------------
// M5.3: `%*alias*%` text substitution (`alias.xconf` + inline `alias`).
// ---------------------------------------------------------------------------

#[test]
fn aliases_substitute_text_in_expressions_and_strings() {
    // Also pins the pure-text semantics: `%*doubled*%` is `40 + 2 * 2`.
    assert_compiles_and_runs(
        "tests/fixtures/aliases.x",
        "42\n44\nsay aliasword\n",
    );
}

#[test]
fn an_alias_xconf_entry_can_be_an_impl_path() {
    assert_project_runs("alias_xconf", &project("alias_xconf"), "42\n");
}

#[test]
fn an_alias_from_an_imported_module_is_visible() {
    // The table is flat and global: `%*greeting*%` is declared in `util.x`
    // and used in `main.x`.
    assert_project_runs(
        "alias_cross_file",
        &project("alias_cross_file"),
        "hello from an imported alias\n",
    );
}

#[test]
fn an_alias_cycle_is_rejected_even_when_unused() {
    assert_rejected("tests/fixtures/alias_cycle.x");
}

#[test]
fn an_unknown_alias_is_rejected() {
    assert_rejected("tests/fixtures/alias_unknown.x");
}

#[test]
fn an_alias_cycle_across_two_files_names_both_sites() {
    // `a` is defined in `alias.xconf`, `b` in `main.x`; the diagnostic points
    // at the reference that closed the cycle and notes the definition, so
    // both files appear.
    let bin = out_path("alias_xconf_cycle");
    let build = Command::new(xc_bin())
        .arg("build")
        .arg(project("alias_xconf_cycle"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn xc");
    assert!(!build.status.success(), "expected a rejection");
    let stderr = String::from_utf8_lossy(&build.stderr);
    for needle in ["alias cycle: a -> b -> a", "alias.xconf:", "main.x:"] {
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
        "tests/fixtures/heap_bytes.x",
        "5\n104\nHello\nHello\nfalse\n0\n",
    );
}

#[test]
fn a_bytes_value_moves() {
    // `Bytes` is non-Copy, so binding it to a second name moves it and the
    // first name is dead (M3's rule, now with a type that makes it matter).
    assert_rejected("tests/fixtures/heap_use_after_move.x");
}

#[test]
fn a_moved_from_binding_can_be_reassigned() {
    assert_compiles_and_runs("tests/fixtures/heap_move_then_rebind.x", "2\n1\n");
}

#[test]
fn an_owned_value_can_be_returned_by_a_function() {
    // The callee must not free it; the caller must.
    assert_compiles_and_runs("tests/fixtures/heap_returned.x", "5\nhello\n");
}

#[test]
fn an_owned_local_dies_at_the_end_of_its_block() {
    assert_compiles_and_runs("tests/fixtures/heap_scope_drop.x", "first\nafter\n");
}

#[test]
fn drops_follow_control_flow() {
    // A loop body, a `return` from inside a branch with another owned local
    // still live, and a block used as a value.
    assert_compiles_and_runs(
        "tests/fixtures/heap_drops_in_control_flow.x",
        "loop\nloop\nextra\nkeep\n5\n",
    );
}

#[test]
fn reassigning_an_owned_binding_frees_the_old_value() {
    assert_compiles_and_runs("tests/fixtures/heap_reassign.x", "three\nagain\n");
}

#[test]
fn ownership_through_branches_frees_exactly_once() {
    assert_compiles_and_runs(
        "tests/fixtures/heap_branches.x",
        "second\nelse\nfirst\n",
    );
}

#[test]
fn a_bytes_parameter_is_a_borrow_not_a_move() {
    assert_compiles_and_runs(
        "tests/fixtures/heap_param_borrow.x",
        "2\nhi\nXi\nfalse\n",
    );
}

#[test]
fn a_borrowed_datac_field_is_rejected() {
    assert_rejected("tests/fixtures/datac_str_field.x");
}

#[test]
fn printing_a_temporary_owned_buffer_works() {
    // Until M6.3 this was a diagnostic telling the author to bind a `val`
    // first; now codegen hoists the temporary and drops it after the printf,
    // and the fixture runs under AddressSanitizer to prove it is freed.
    assert_compiles_and_runs(
        "tests/fixtures/heap_temporary_print.x",
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
        "tests/fixtures/string_basic.x",
        "6\n5\nfalse\nhéllo\nhéllo\n6\nhéllo\ntrue\n0\n",
    );
}

#[test]
fn owned_datac_fields_are_dropped_with_the_struct() {
    assert_compiles_and_runs(
        "tests/fixtures/string_field.x",
        "first\n7\nafter scope\nsecond\n7\ndone\n",
    );
}

#[test]
fn an_owned_passdown_is_freed_at_exit() {
    assert_compiles_and_runs("tests/fixtures/pd_owned.x", "== x ==\n== x ==\n");
}

#[test]
fn a_compiler_shipped_stdlib_module_can_be_imported() {
    // `impl "std/mem"` — the module's X body calls a compiler-supplied
    // `extern` for the allocation itself.
    assert_compiles_and_runs("tests/fixtures/stdlib_mem.x", "3\nhi\n0\n");
}

#[test]
fn the_str_stdlib_module_runs_from_x_source() {
    // `std/str` is written in X; its only compiler-supplied pieces are the
    // two byte reads it wraps.
    assert_compiles_and_runs(
        "tests/fixtures/stdlib_str.x",
        "HI THERE\n8\n8\ntrue\nfalse\nhello\n98\nababab\n6\n",
    );
}

#[test]
fn the_path_stdlib_module_runs_from_x_source() {
    // `std/path` imports `std/str`, so this also exercises a stdlib module
    // importing another one.
    assert_compiles_and_runs(
        "tests/fixtures/stdlib_path.x",
        "/tmp/x.txt\n/tmp/x.txt\n/abs\ntrue\nfalse\n",
    );
}

#[test]
fn an_extern_without_a_compiler_supplied_body_is_rejected() {
    assert_rejected("tests/fixtures/extern_no_body.x");
}

#[test]
fn an_extern_receiver_declaration_is_rejected() {
    assert_rejected("tests/fixtures/extern_receiver.x");
}

#[test]
fn comparing_owned_values_is_rejected() {
    // Without this rule the comparison reached `cc` as `x_a == x_b` on two
    // `x_String` structs, which is where the author would have learned what
    // happened instead of at the type checker.
    assert_rejected("tests/fixtures/compare_owned.x");
}

#[test]
fn an_owned_value_in_statement_position_is_rejected() {
    assert_rejected("tests/fixtures/owned_statement.x");
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
    let dir = std::env::temp_dir().join(format!("xc_caps_{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("could not create the capability scratch directory");
    dir
}

/// Builds a caps project, runs it, and asserts the run fails with `needle` on
/// stderr — the runtime-denial path, which only a *granted* module can reach.
fn assert_project_denied_at_runtime(name: &str, target: &Path, needle: &str) {
    let bin = out_path(name);
    let build = Command::new(xc_bin())
        .arg("build")
        .arg(target)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn xc");
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
        &project("caps_granted").join("main.x"),
        "hello caps\ntrue\ntrue\n",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_ungranted_module_cannot_name_the_gated_namespace() {
    assert_project_rejected(
        "caps_ungranted",
        &project("caps_ungranted").join("main.x"),
        "needs the 'fs' capability, which module 'util' was not granted",
    );
}

#[test]
fn a_module_cannot_delegate_more_than_it_holds() {
    assert_project_rejected(
        "caps_escalation",
        &project("caps_escalation").join("main.x"),
        "can delegate less than it has, never more",
    );
}

#[test]
fn a_pattern_containing_dotdot_is_rejected() {
    assert_project_rejected(
        "caps_bad_pattern",
        &project("caps_bad_pattern").join("main.x"),
        "contains a '..' segment",
    );
}

#[test]
fn an_unknown_capability_domain_is_rejected() {
    assert_project_rejected(
        "caps_bad_domain",
        &project("caps_bad_domain").join("main.x"),
        "'disk' is not a capability domain",
    );
}

#[test]
fn a_granted_module_can_write_rename_size_and_remove_inside_its_grant() {
    let dir = caps_scratch("write");
    assert_project_runs(
        "caps_write",
        &project("caps_write").join("main.x"),
        "true\n5\nfalse\nhello\nfalse\ntrue\n3\n",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_path_outside_the_grant_is_denied_at_runtime() {
    assert_project_denied_at_runtime(
        "caps_denied",
        &project("caps_denied").join("main.x"),
        "outside the fs capability granted to module 'util'",
    );
}

#[test]
fn a_symlink_inside_the_grant_pointing_outside_is_denied() {
    let dir = caps_scratch("symlink");
    std::os::unix::fs::symlink("/etc/passwd", dir.join("escape")).unwrap();
    assert_project_denied_at_runtime(
        "caps_symlink",
        &project("caps_symlink").join("main.x"),
        "/etc/passwd",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_granted_env_reference_passes_the_name_check_and_reaches_the_stub() {
    // The domain, the grant and the variable-name check are real; the body is
    // the gated stub, which panics on its own terms.
    assert_project_denied_at_runtime(
        "caps_env",
        &project("caps_env").join("main.x"),
        "gated but not implemented yet",
    );
}

#[test]
fn time_and_rand_are_gated_domains_with_no_named_resource() {
    assert_compiles_and_runs(
        "tests/fixtures/caps_time_rand.x",
        "true\ntrue\ntrue\n4\nslept\n",
    );
}

#[test]
fn path_normalize_is_the_function_the_capability_checker_uses() {
    assert_compiles_and_runs(
        "tests/fixtures/path_normalize.x",
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
        "tests/fixtures/consts.x",
        "42\n1.500000\n84\nx\ntrue\n",
    );
}

#[test]
fn std_math_overloads_resolve_on_argument_type() {
    assert_compiles_and_runs(
        "tests/fixtures/stdlib_math.x",
        "3.141593\n6.283185\n7\n7.500000\n3\n9\n10\n1.414214\n2.000000\n-1.000000\ntrue\nfalse\n",
    );
}

#[test]
fn str_views_borrow_and_owned_truncations_allocate() {
    assert_compiles_and_runs(
        "tests/fixtures/string_views.x",
        "hello\n5\nhello  \n7\nello\ntrue\ntrue\ntrue\ntrue\nfalse\ntrue\n2\nraw\n5\nown\n",
    );
}

#[test]
fn path_components_split_a_path() {
    assert_compiles_and_runs(
        "tests/fixtures/path_parts.x",
        "notes.txt\ntxt\n/tmp\nbare.txt\ntxt\n\n/var/log\ntrue\n",
    );
}

#[test]
fn returning_a_borrow_of_the_wrong_parameter_is_rejected() {
    assert_rejected("tests/fixtures/elision_wrong_param.x");
}

#[test]
fn returning_a_view_of_a_local_is_rejected() {
    assert_rejected("tests/fixtures/elision_not_from_param.x");
}

#[test]
fn a_live_view_keeps_its_owner_pinned() {
    assert_rejected("tests/fixtures/view_pins_owner.x");
}

#[test]
fn a_view_releases_its_owner_at_scope_end() {
    assert_compiles_and_runs(
        "tests/fixtures/view_released_at_scope_end.x",
        "hi\nhi\n",
    );
}

#[test]
fn comparing_str_values_is_rejected() {
    assert_rejected("tests/fixtures/str_comparison.x");
}

#[test]
fn a_const_that_does_not_fold_is_rejected() {
    assert_rejected("tests/fixtures/const_not_constant.x");
}

#[test]
fn a_modules_own_items_are_reachable_bare_inside_it() {
    // `util.x` calls `doubled` and constructs `Pair` without qualifying
    // either, even though `resolve.rs` stores both prefixed.
    assert_project_runs(
        "self_call",
        &project("self_call").join("main.x"),
        "12\n3\n",
    );
}

/// The M6 gate, run rather than argued: every `Bytes` fixture is rebuilt with
/// AddressSanitizer and executed, so a double free, a use-after-free, or a
/// leak fails the suite. If `cc` has no AddressSanitizer the check is skipped
/// — the one place in this suite that depends on a toolchain feature.
#[test]
fn heap_fixtures_are_double_free_and_leak_clean() {
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
        "string_field",
        "pd_owned",
        "stdlib_mem",
        "stdlib_str",
        "stdlib_path",
        "stdlib_math",
        "string_views",
        "path_parts",
        "view_released_at_scope_end",
        "heap_temporary_print",
    ];
    for name in fixtures {
        let source = format!("tests/fixtures/{name}.x");
        // A distinct output path: the individually-run fixture tests build
        // the same names, and racing them for one file makes the binary
        // unreadable to whichever loses.
        let bin = out_path(&format!("{name}_asan_src"));
        let build = Command::new(xc_bin())
            .arg("build")
            .arg(fixture(&source))
            .arg("-o")
            .arg(&bin)
            .output()
            .expect("failed to spawn xc");
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
    assert_compiles_and_runs("tests/fixtures/datac_embeds_datac.x", "8\n");
}

#[test]
fn a_forward_reference_between_datacs_is_rejected() {
    assert_rejected("tests/fixtures/datac_forward_reference.x");
}
