//! The `fun` keyword is optional at the top level (issue #287).

use super::*;

/// A top-level function, a method and a receiver method written without `fun`
/// run the same as the written form, and the two forms mix in one file.
#[test]
pub(crate) fn a_top_level_function_may_leave_out_fun() {
    assert_compiles_and_runs("tests/fixtures/fun_optional.kp", "2\n4\n4\n8\n0\n5\n");
}

/// A `pub` top-level function keeps `fun`, and the refusal names the spelling.
#[test]
pub(crate) fn a_pub_function_written_without_fun_is_refused_with_the_fix_named() {
    assert_rejected_with(
        "tests/fixtures/fun_optional_pub_refused.kp",
        "write 'pub fun half(...)'",
    );
}

/// A top-level line that is not a declaration keeps its refusal.
#[test]
pub(crate) fn a_top_level_assignment_is_still_refused() {
    assert_rejected_with("tests/fixtures/fun_optional_not_decl_refused.kp", "expected 'fun'");
}
