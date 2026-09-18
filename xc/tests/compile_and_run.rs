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
fn stdlib_import_is_rejected_until_m6() {
    assert_project_rejected(
        "stdlib_import",
        &project("stdlib_import").join("main.x"),
        "standard library is not implemented yet",
    );
}

#[test]
fn caps_clause_is_rejected_until_m7() {
    assert_project_rejected(
        "caps_clause",
        &project("caps_clause").join("main.x"),
        "'with caps(...)' grants are not implemented yet",
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
fn writing_a_passdown_inside_a_function_is_rejected() {
    assert_rejected("tests/fixtures/pd_var_write.x");
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
