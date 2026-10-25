//! Methods written inside the braces of their datac (issue #285).

use super::*;

/// A method inside a datac block is the method the `fun T.name` line after the
/// type is: `mut` writes the receiver, the expression body and the block body
/// both work, a generic datac's methods take its parameters, and the two forms
/// mix for one type.
#[test]
pub(crate) fn a_method_written_inside_its_datac_block_runs_like_one_written_after_it() {
    assert_fixture_runs("tests/fixtures/datac_block_methods.kp");
}

/// The `fun` keyword is refused inside the block, with the form without it named.
#[test]
pub(crate) fn a_fun_keyword_inside_a_datac_block_is_refused() {
    assert_rejected_with(
        "tests/fixtures/datac_block_method_fun.kp",
        "a method inside a 'datac' block is written without 'fun'",
    );
}

/// A method inside a block cannot be `pub`: the type's `pub` exports it.
#[test]
pub(crate) fn a_pub_method_inside_a_datac_block_is_refused() {
    assert_rejected_with(
        "tests/fixtures/datac_block_method_pub.kp",
        "a method inside a 'datac' block cannot be 'pub'",
    );
}

/// A datac block holds methods only: a field line in it is refused.
#[test]
pub(crate) fn a_datac_block_line_that_is_not_a_method_is_refused() {
    assert_rejected_with(
        "tests/fixtures/datac_block_bad_line.kp",
        "a 'datac' block declares methods",
    );
}
