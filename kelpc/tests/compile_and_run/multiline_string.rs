use super::*;

// Multi-line string literal (syntax issue 260): a `"""` block is the same
// string as the text written with `\n` escapes. The lexer tests pin the token
// values; these pin that a program using a block compiles and computes the
// same thing, and that a less-indented line is refused with its message.

#[test]
fn a_block_equals_the_same_text_written_with_escapes() {
    assert_fixture_runs("tests/fixtures/multiline_string.kp");
}

#[test]
fn a_block_line_less_indented_than_its_closing_line_is_refused() {
    assert_rejected_with(
        "tests/fixtures/multiline_string_less_indented.kp",
        "less indented",
    );
}
