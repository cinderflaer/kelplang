use super::*;

// Digit separators (syntax issue 259): `1_000_000` reads as `1000000`. The
// lexer tests pin the token values; these pin that a program using separated
// literals compiles and computes the same thing, and that a misplaced
// separator is refused with its message.

#[test]
fn digit_separators_read_the_same_value_as_the_plain_literal() {
    assert_fixture_runs("tests/fixtures/digit_separators.kp");
}

#[test]
fn a_digit_separator_after_the_last_digit_is_refused() {
    assert_rejected_with(
        "tests/fixtures/digit_separator_trailing.kp",
        "not after the last digit",
    );
}

#[test]
fn two_digit_separators_in_a_row_are_refused() {
    assert_rejected_with(
        "tests/fixtures/digit_separator_doubled.kp",
        "one at a time",
    );
}
