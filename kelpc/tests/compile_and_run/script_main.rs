//! A file of top-level statements with no `fun main` is its own main (issue #288).

use super::*;

/// Statements, declarations before, between and after them, a method call and a
/// loop run in source order from the start of the program.
#[test]
pub(crate) fn a_file_of_top_level_statements_runs_as_main() {
    assert_compiles_and_runs("tests/fixtures/script_main.kp", "start\n42\nhello, Kelp\n2\n0\n1\n2\n");
}

/// A top-level `let ... else` takes the statements after it as its rest.
#[test]
pub(crate) fn a_top_level_let_else_runs_the_rest_of_the_script() {
    assert_compiles_and_runs("tests/fixtures/script_let_else.kp", "4\n5\n");
}

/// Statements and a `fun main` in one file are refused, and the refusal names both.
#[test]
pub(crate) fn statements_and_a_fun_main_in_one_file_are_refused() {
    assert_rejected_with(
        "tests/fixtures/script_main_with_main_refused.kp",
        "cannot also declare 'main'",
    );
}

/// Statements in a module other than the entry file are refused.
#[test]
pub(crate) fn statements_outside_the_entry_file_are_refused() {
    assert_project_rejected(
        "script_in_module",
        &project("script_in_module"),
        "allowed only in the entry file",
    );
}

/// `?` is refused in a script, as it is in `fun main()`.
#[test]
pub(crate) fn a_question_mark_in_a_script_is_refused() {
    assert_rejected_with("tests/fixtures/script_question_refused.kp", "it returns 'unit'");
}

/// A declaration after a top-level `let ... else` is refused.
#[test]
pub(crate) fn a_declaration_after_a_top_level_let_else_is_refused() {
    assert_rejected_with(
        "tests/fixtures/script_let_else_then_decl_refused.kp",
        "cannot follow a 'let ... else' at the top level",
    );
}
