//! #300: the pure std modules (`str`, `fmt`, `math`, `option`, `result`,
//! `convert`, `iter`) are in scope in a project module without an `impl` line,
//! a bare `Option` and `Result` name the std types, and an explicit `impl`
//! that names one of them keeps its meaning for its own file.

use super::*;

#[test]
pub(crate) fn pure_std_modules_and_bare_option_result_need_no_impl_line() {
    // Every one of the seven names is used with no `impl` line, and `Option.Some`,
    // `Result.Err` and the annotations `Option<T>`, `Result<T, E>` name the std
    // types. The prefix form still runs too.
    assert_project_runs(
        "std_default_names",
        &project("std_default_names"),
        "4\ntrue\ntrue\n42\n-1\ntrue\ntrue\nKELP\n5\n4\nPoint(3, 4)\n5\ntrue\n",
    );
}

#[test]
pub(crate) fn an_explicit_impl_naming_a_std_name_keeps_it_for_its_file() {
    // `impl "str"` is the user's `str` in main.kp; report.kp names nothing and
    // reaches the std `str`.
    assert_project_runs("std_name_clash", &project("std_name_clash"), "7\n42\n");
}

#[test]
pub(crate) fn a_local_binding_or_own_type_named_like_a_std_name_wins() {
    // `val str` shadows the module for `str.len()`, and a datac named `Result`
    // is the file's own type.
    assert_project_runs("std_local_names", &project("std_local_names"), "4\n3\n");
}

#[test]
pub(crate) fn derive_text_refuses_a_file_whose_fmt_is_another_module() {
    assert_project_rejected(
        "derive_text_clash",
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("derive_text_clash")
            .join("src")
            .join("main.kp"),
        "is not std/fmt",
    );
}
