use super::*;

fn kinds(src: &str) -> Vec<TokenKind> {
    Lexer::new(src)
        .tokenize()
        .expect("lex failed")
        .into_iter()
        .map(|t| t.kind)
        .collect()
}

#[test]
fn lexes_keywords_and_idents() {
    assert_eq!(
        kinds("fun val var mut if else elif while for when return true false datac pub trait foo"),
        vec![
            TokenKind::Fun,
            TokenKind::Val,
            TokenKind::Var,
            TokenKind::Mut,
            TokenKind::If,
            TokenKind::Else,
            TokenKind::Elif,
            TokenKind::While,
            TokenKind::For,
            TokenKind::When,
            TokenKind::Return,
            TokenKind::True,
            TokenKind::False,
            TokenKind::Datac,
            TokenKind::Pub,
            TokenKind::Trait,
            TokenKind::Ident("foo".to_string()),
            TokenKind::Eof,
        ]
    );
}

#[test]
fn lexes_an_associated_type_projection() {
    // `::` is one token, so the formatter can glue both sides the way it
    // glues a `.`, and two colons written apart stay two `Colon`s.
    assert_eq!(
        kinds("Self::Item"),
        vec![
            TokenKind::Ident("Self".to_string()),
            TokenKind::ColonColon,
            TokenKind::Ident("Item".to_string()),
            TokenKind::Eof,
        ]
    );
    assert_eq!(
        kinds("a : : b"),
        vec![
            TokenKind::Ident("a".to_string()),
            TokenKind::Colon,
            TokenKind::Colon,
            TokenKind::Ident("b".to_string()),
            TokenKind::Eof,
        ]
    );
}

#[test]
fn lexes_a_trait_supertrait_list_and_default_body() {
    // The new syntax adds no token: a supertrait list is `:` and `+`, and a
    // default body is a brace, exactly what the lexer already emits.
    assert_eq!(
        kinds("trait Named: Shape + Other"),
        vec![
            TokenKind::Trait,
            TokenKind::Ident("Named".to_string()),
            TokenKind::Colon,
            TokenKind::Ident("Shape".to_string()),
            TokenKind::Plus,
            TokenKind::Ident("Other".to_string()),
            TokenKind::Eof,
        ]
    );
}

#[test]
fn fat_arrow_is_the_arrow_token_and_leaves_the_others_alone() {
    assert_eq!(
        kinds("=> -> = == >= <= ==> >= ="),
        vec![
            TokenKind::Arrow,
            TokenKind::Arrow,
            TokenKind::Eq,
            TokenKind::EqEq,
            TokenKind::GtEq,
            TokenKind::LtEq,
            TokenKind::EqEq,
            TokenKind::Gt,
            TokenKind::GtEq,
            TokenKind::Eq,
            TokenKind::Eof,
        ]
    );
}

#[test]
fn lexes_operators_including_compound_assign() {
    assert_eq!(
        kinds("-> == != <= >= && || + - * / % < > = ! & += -= *= /= %="),
        vec![
            TokenKind::Arrow,
            TokenKind::EqEq,
            TokenKind::NotEq,
            TokenKind::LtEq,
            TokenKind::GtEq,
            TokenKind::AndAnd,
            TokenKind::OrOr,
            TokenKind::Plus,
            TokenKind::Minus,
            TokenKind::Star,
            TokenKind::Slash,
            TokenKind::Percent,
            TokenKind::Lt,
            TokenKind::Gt,
            TokenKind::Eq,
            TokenKind::Bang,
            TokenKind::Amp,
            TokenKind::PlusEq,
            TokenKind::MinusEq,
            TokenKind::StarEq,
            TokenKind::SlashEq,
            TokenKind::PercentEq,
            TokenKind::Eof,
        ]
    );
}

#[test]
fn lexes_literals() {
    assert_eq!(
        kinds(r#"42 3.14 "hi\n""#),
        vec![
            TokenKind::IntLit(42),
            TokenKind::FloatLit(3.14),
            TokenKind::StringLit("hi\n".to_string()),
            TokenKind::Eof,
        ]
    );
}

/// `0x` and `0b` literals (A-9). The prefix has to be read at the leading
/// `0`, before the decimal digit run stops there: `0xff` used to lex as
/// `0` and then an identifier `xff`, so `val mask: i32 = 0xff` was a
/// parse error and every mask in the standard library was decimal.
#[test]
fn lexes_hexadecimal_and_binary_literals() {
    assert_eq!(
        kinds("0xff 0xFF 0b1010 0x0 0b0 0x7fffffff"),
        vec![
            TokenKind::IntLit(255),
            TokenKind::IntLit(255),
            TokenKind::IntLit(10),
            TokenKind::IntLit(0),
            TokenKind::IntLit(0),
            TokenKind::IntLit(0x7fff_ffff),
            TokenKind::Eof,
        ]
    );
    // A prefix is not a radix for the next token: `0xff` stops at the
    // first non-hex character, and `.` follows as its own token.
    assert_eq!(
        kinds("0xf.a"),
        vec![
            TokenKind::IntLit(15),
            TokenKind::Dot,
            TokenKind::Ident("a".to_string()),
            TokenKind::Eof,
        ]
    );
}

/// A digit separator (`_` between two digits) reads as the same value, in
/// every radix and in a float. `1_000_000` is `1000000`, not a name.
#[test]
fn a_digit_separator_between_digits_reads_the_same_value() {
    assert_eq!(kinds("1_000_000"), kinds("1000000"));
    assert_eq!(kinds("0x1000_0000"), kinds("0x10000000"));
    assert_eq!(kinds("0b1010_1010"), kinds("0b10101010"));
    assert_eq!(
        kinds("1_000_000 0x1000_0000 0xFF_ff 0b1_0 1_000.5"),
        vec![
            TokenKind::IntLit(1_000_000),
            TokenKind::IntLit(0x1000_0000),
            TokenKind::IntLit(0xFFFF),
            TokenKind::IntLit(2),
            TokenKind::FloatLit(1000.5),
            TokenKind::Eof,
        ]
    );
    // A separator in the fraction of a float is read the same way.
    assert_eq!(kinds("1.2_5"), kinds("1.25"));
    // A tuple position is not a number literal, so no separator is read there.
    assert_eq!(
        kinds("t.1_0"),
        vec![
            TokenKind::Ident("t".to_string()),
            TokenKind::Dot,
            TokenKind::IntLit(1),
            TokenKind::Ident("_0".to_string()),
            TokenKind::Eof,
        ]
    );
}

/// `...` is one token (the variadic marker, gap 7), while a lone `.` stays
/// the field-access token and `..` stays two of them: only three dots in a
/// row are read together.
#[test]
fn lexes_three_dots_as_one_variadic_token() {
    assert_eq!(
        kinds("... a.b .."),
        vec![
            TokenKind::DotDotDot,
            TokenKind::Ident("a".to_string()),
            TokenKind::Dot,
            TokenKind::Ident("b".to_string()),
            TokenKind::Dot,
            TokenKind::Dot,
            TokenKind::Eof,
        ]
    );
}

/// The whole `u64` range is lexable (MISSING-x8Wq3): the payload is the
/// magnitude the source wrote, so the top half of `u64` reaches the checker
/// instead of being refused before any type is known. Which of those values
/// is legal is still the checker's decision, from the literal's context.
#[test]
fn lexes_the_top_half_of_the_u64_range() {
    assert_eq!(
        kinds("0xFFFFFFFFFFFFFFFF 18446744073709551615 9223372036854775808"),
        vec![
            TokenKind::IntLit(u64::MAX),
            TokenKind::IntLit(u64::MAX),
            TokenKind::IntLit(1u64 << 63),
            TokenKind::Eof,
        ]
    );
}

/// Each malformed shape is named at its own span, rather than splitting
/// into two tokens and failing later in the parser: `0b12` used to be
/// `0b1` then `2`, and `1_000` was `1` then an identifier `_000`.
#[test]
fn a_malformed_radix_literal_names_its_own_problem() {
    let cases = [
        ("0x", "needs at least one hex digit"),
        ("0b", "needs at least one binary digit"),
        ("0b12", "invalid digit '2' in a binary literal"),
        ("0x1g", "invalid digit 'g' in a hexadecimal literal"),
        ("0x_ff", "not before the first digit"),
        ("0b_1", "not before the first digit"),
        ("0x1_", "not after the last digit"),
        ("1_", "not after the last digit"),
        ("1_.5", "not after the last digit"),
        ("1.5_", "not after the last digit"),
        ("1__000", "one at a time"),
        ("0x1__0", "one at a time"),
        ("1_u8", "takes no suffix"),
        ("5u8", "takes no suffix"),
        (
            "0x10000000000000000",
            "larger than the largest integer literal",
        ),
        (
            "18446744073709551616",
            "larger than the largest integer literal, 18446744073709551615",
        ),
    ];
    for (src, needle) in cases {
        let err = Lexer::new(src).tokenize().unwrap_err();
        assert!(
            err.message.contains(needle),
            "for {src:?} expected a message containing {needle:?}, got: {}",
            err.message
        );
    }
}

/// `b'x'` is one token, not the identifier `b` beside a character literal
/// (H-11), and `\xNN` is exactly two hex digits in a byte literal, a
/// character literal and a string.
#[test]
fn lexes_byte_literals_and_hex_escapes() {
    assert_eq!(
        kinds(r#"b'x' b'\n' b'\x41' b'\xff' '\x41' "\x41""#),
        vec![
            TokenKind::ByteLit(b'x'),
            TokenKind::ByteLit(b'\n'),
            TokenKind::ByteLit(0x41),
            TokenKind::ByteLit(0xff),
            TokenKind::CharLit(0x41),
            TokenKind::StringLit("A".to_string()),
            TokenKind::Eof,
        ]
    );
    // `b` is an ordinary identifier when it is not a byte literal, and a
    // hex escape does not run on into the hex-looking text after it.
    assert_eq!(
        kinds(r#"bx b "\x41a""#),
        vec![
            TokenKind::Ident("bx".to_string()),
            TokenKind::Ident("b".to_string()),
            TokenKind::StringLit("Aa".to_string()),
            TokenKind::Eof,
        ]
    );
}

/// Each malformed shape is named at its own span: a byte literal is one
/// ASCII character or a `\xNN` escape, and the escape is two hex digits in
/// every literal form, with a string literal stopping at the ASCII range.
/// A raw NUL byte is refused like `\x00`, since a `str` cannot hold one.
#[test]
fn a_malformed_byte_literal_or_hex_escape_names_its_own_problem() {
    let cases = [
        ("b''", "cannot be empty"),
        ("b'ab'", "exactly one byte"),
        ("b'\u{e9}'", "one ASCII character or a '\\xNN' escape"),
        ("b'\\u{4e2d}'", "character literal escape"),
        ("b'\\x4'", "two hex digits"),
        ("\"\\xff\"", "not a text character"),
        ("\"\\x00\"", "NUL-terminated"),
        // A raw NUL byte, written as `\0` in this Rust source, is the same
        // defect as the escape above and must be refused the same way.
        ("\"a\0b\"", "NUL-terminated"),
    ];
    for (src, needle) in cases {
        let err = Lexer::new(src).tokenize().unwrap_err();
        assert!(
            err.message.contains(needle),
            "for {src:?} expected a message containing {needle:?}, got: {}",
            err.message
        );
    }
}

#[test]
fn decodes_multibyte_utf8_in_string_literals() {
    // The lexer is byte-oriented, so a literal used to push each byte of a
    // multi-byte character as its own Latin-1 scalar - `"héllo"` came out
    // as `"hÃ©llo"`, and `String.len` then reported 8 bytes for 6. The
    // character has to survive verbatim.
    assert_eq!(
        kinds("\"héllo\" \"→\""),
        vec![
            TokenKind::StringLit("héllo".to_string()),
            TokenKind::StringLit("→".to_string()),
            TokenKind::Eof,
        ]
    );
}

#[test]
fn skips_line_comments() {
    assert_eq!(
        kinds("1 // this is a comment\n2"),
        vec![
            TokenKind::IntLit(1),
            TokenKind::Newline,
            TokenKind::IntLit(2),
            TokenKind::Eof
        ]
    );
}

/// The formatter needs the comments the parser never sees. The default
/// stream is unchanged - this is the whole point of keeping the change
/// additive - and the trivia stream interleaves the comment in source
/// order.
#[test]
fn trivia_retention_keeps_comments_without_changing_the_default_stream() {
    let src = "1 // c\n2";
    assert_eq!(
        kinds(src),
        vec![
            TokenKind::IntLit(1),
            TokenKind::Newline,
            TokenKind::IntLit(2),
            TokenKind::Eof
        ]
    );
    let trivia: Vec<TokenKind> = Lexer::new(src)
        .tokenize_with_trivia()
        .unwrap()
        .into_iter()
        .map(|t| t.kind)
        .collect();
    assert_eq!(
        trivia,
        vec![
            TokenKind::IntLit(1),
            TokenKind::Comment("// c".to_string()),
            TokenKind::Newline,
            TokenKind::IntLit(2),
            TokenKind::Eof
        ]
    );
}

/// A comment must not change how a blank-line run collapses in the default
/// stream: `1\n\n// c\n2` still has a single `Newline` token.
#[test]
fn a_comment_between_newlines_leaves_the_default_stream_alone() {
    assert_eq!(
        kinds("1\n\n// c\n2"),
        vec![
            TokenKind::IntLit(1),
            TokenKind::Newline,
            TokenKind::IntLit(2),
            TokenKind::Eof
        ]
    );
}

/// CR is a line terminator, not part of the comment, so a CRLF file does
/// not carry a stray CR into the comment's text.
#[test]
fn a_carriage_return_is_not_part_of_a_comment() {
    let tokens = Lexer::new("// x\r\ny").tokenize_with_trivia().unwrap();
    assert_eq!(tokens[0].kind, TokenKind::Comment("// x".to_string()));
    assert_eq!(tokens[0].span, Span::new(0, 4));
}

#[test]
fn skips_block_comments_without_changing_the_tokens_around_them() {
    assert_eq!(
        kinds("1 /* two\nlines */ 2"),
        vec![
            TokenKind::IntLit(1),
            TokenKind::IntLit(2),
            TokenKind::Eof
        ]
    );
}

/// Nesting, not C's first-`*/`-wins: commenting out a region that already
/// contains a block comment is the case that decides it, and it is the case
/// a reader hits first in a real file.
#[test]
fn block_comments_nest() {
    assert_eq!(
        kinds("1 /* /* inner */ still */ 2"),
        vec![
            TokenKind::IntLit(1),
            TokenKind::IntLit(2),
            TokenKind::Eof
        ]
    );
}

#[test]
fn an_unterminated_block_comment_is_an_error_at_its_own_start() {
    let err = Lexer::new("1 /* open").tokenize().unwrap_err();
    assert!(
        err.message.contains("unterminated block comment"),
        "unexpected message: {}",
        err.message
    );
    assert_eq!(err.span, Span::new(2, 9));
}

/// The delimiters are text inside a string, and a `//` or a `/*` inside a
/// block comment is text too, since only the caller's own delimiters apply.
#[test]
fn comment_delimiters_inside_a_string_or_a_comment_are_text() {
    assert_eq!(
        kinds("\"a/*b*/c\" /* // */ 1"),
        vec![
            TokenKind::StringLit("a/*b*/c".to_string()),
            TokenKind::IntLit(1),
            TokenKind::Eof
        ]
    );
}

/// `//!` starts a documentation block, so a `//!` that starts its line is
/// an ordinary comment: the token stream it produces is the one an
/// ordinary `//` produces, indentation included.
#[test]
fn a_doc_comment_that_starts_a_line_is_a_comment() {
    for src in ["//! the module header", "    //! indented, and still first on its line"] {
        let doc = Lexer::new(&format!("{src}\n1")).tokenize_with_trivia().unwrap();
        assert_eq!(doc[0].kind, TokenKind::Comment(src.trim_start().to_string()));
        assert_eq!(
            kinds(&format!("{src}\n1")),
            kinds(&format!("{}\n1", src.replacen("//!", "//", 1)))
        );
    }
}

/// A `//!` with code before it on the same line documents nothing, so it
/// is refused rather than skipped. `//` beside code stays legal.
#[test]
fn a_doc_comment_after_code_is_refused() {
    let err = Lexer::new("fun main() { print(1) } //! not documentation")
        .tokenize()
        .unwrap_err();
    assert!(
        err.message.contains("must start a line"),
        "unexpected message: {}",
        err.message
    );
    assert_eq!(err.span, Span::new(24, 45));
    assert!(Lexer::new("fun main() { print(1) } // a note").tokenize().is_ok());
}

/// The formatter copies a block comment verbatim, so the token has to hold
/// the delimiters and the newlines exactly as written.
#[test]
fn trivia_retention_keeps_a_block_comment_whole() {
    let src = "1 /* a\nb */ 2";
    let trivia: Vec<TokenKind> = Lexer::new(src)
        .tokenize_with_trivia()
        .unwrap()
        .into_iter()
        .map(|t| t.kind)
        .collect();
    assert_eq!(
        trivia,
        vec![
            TokenKind::IntLit(1),
            TokenKind::Comment("/* a\nb */".to_string()),
            TokenKind::IntLit(2),
            TokenKind::Eof
        ]
    );
}

#[test]
fn newline_is_significant_at_top_level() {
    assert_eq!(
        kinds("1\n2"),
        vec![
            TokenKind::IntLit(1),
            TokenKind::Newline,
            TokenKind::IntLit(2),
            TokenKind::Eof
        ]
    );
}

#[test]
fn consecutive_newlines_collapse_to_one_token() {
    assert_eq!(
        kinds("1\n\n\n2"),
        vec![
            TokenKind::IntLit(1),
            TokenKind::Newline,
            TokenKind::IntLit(2),
            TokenKind::Eof
        ]
    );
}

#[test]
fn newline_is_insignificant_inside_parens() {
    assert_eq!(
        kinds("f(1,\n2)"),
        vec![
            TokenKind::Ident("f".to_string()),
            TokenKind::LParen,
            TokenKind::IntLit(1),
            TokenKind::Comma,
            TokenKind::IntLit(2),
            TokenKind::RParen,
            TokenKind::Eof
        ]
    );
}

#[test]
fn folds_bang_paren_into_macro_like_ident() {
    assert_eq!(
        kinds("print!(1)"),
        vec![
            TokenKind::Ident("print!".to_string()),
            TokenKind::LParen,
            TokenKind::IntLit(1),
            TokenKind::RParen,
            TokenKind::Eof,
        ]
    );
}

#[test]
fn does_not_fold_not_equal_without_spaces() {
    assert_eq!(
        kinds("x!=y"),
        vec![
            TokenKind::Ident("x".to_string()),
            TokenKind::NotEq,
            TokenKind::Ident("y".to_string()),
            TokenKind::Eof,
        ]
    );
}

#[test]
fn tracks_spans() {
    let tokens = Lexer::new("abc").tokenize().unwrap();
    assert_eq!(tokens[0].span, Span::new(0, 3));
}

#[test]
fn rejects_unterminated_string() {
    assert!(Lexer::new("\"abc").tokenize().is_err());
}

#[test]
fn rejects_unknown_character() {
    assert!(Lexer::new("$").tokenize().is_err());
}

#[test]
fn plain_strings_are_unchanged_by_templates_existing() {
    // No `$`: the single `StringLit` token every earlier milestone relied
    // on, same span covering both quotes.
    let tokens = Lexer::new("\"cost $5\"").tokenize().unwrap();
    assert_eq!(tokens[0].kind, TokenKind::StringLit("cost $5".to_string()));
    assert_eq!(tokens[0].span, Span::new(0, 9));
}

#[test]
fn lexes_a_name_interpolation() {
    assert_eq!(
        kinds("\"hi $name!\""),
        vec![
            TokenKind::TemplateStart,
            TokenKind::StrChunk("hi ".to_string()),
            TokenKind::TemplateExprStart,
            TokenKind::Ident("name".to_string()),
            TokenKind::TemplateExprEnd,
            TokenKind::StrChunk("!".to_string()),
            TokenKind::TemplateEnd,
            TokenKind::Eof,
        ]
    );
}

#[test]
fn lexes_an_expression_interpolation() {
    assert_eq!(
        kinds("\"${c.area()}\""),
        vec![
            TokenKind::TemplateStart,
            TokenKind::StrChunk(String::new()),
            TokenKind::TemplateExprStart,
            TokenKind::Ident("c".to_string()),
            TokenKind::Dot,
            TokenKind::Ident("area".to_string()),
            TokenKind::LParen,
            TokenKind::RParen,
            TokenKind::TemplateExprEnd,
            TokenKind::StrChunk(String::new()),
            TokenKind::TemplateEnd,
            TokenKind::Eof,
        ]
    );
}

#[test]
fn a_dollar_that_starts_nothing_stays_literal() {
    assert_eq!(
        kinds("\"$5 and $ \""),
        vec![TokenKind::StringLit("$5 and $ ".to_string()), TokenKind::Eof]
    );
}

#[test]
fn braces_inside_an_interpolation_do_not_end_it() {
    assert_eq!(
        kinds("\"${ if x { 1 } else { 2 } }\""),
        vec![
            TokenKind::TemplateStart,
            TokenKind::StrChunk(String::new()),
            TokenKind::TemplateExprStart,
            TokenKind::If,
            TokenKind::Ident("x".to_string()),
            TokenKind::LBrace,
            TokenKind::IntLit(1),
            TokenKind::RBrace,
            TokenKind::Else,
            TokenKind::LBrace,
            TokenKind::IntLit(2),
            TokenKind::RBrace,
            TokenKind::TemplateExprEnd,
            TokenKind::StrChunk(String::new()),
            TokenKind::TemplateEnd,
            TokenKind::Eof,
        ]
    );
}

#[test]
fn a_template_can_nest_inside_its_own_interpolation() {
    assert_eq!(
        kinds("\"[${\"in $x\"}]\""),
        vec![
            TokenKind::TemplateStart,
            TokenKind::StrChunk("[".to_string()),
            TokenKind::TemplateExprStart,
            TokenKind::TemplateStart,
            TokenKind::StrChunk("in ".to_string()),
            TokenKind::TemplateExprStart,
            TokenKind::Ident("x".to_string()),
            TokenKind::TemplateExprEnd,
            TokenKind::StrChunk(String::new()),
            TokenKind::TemplateEnd,
            TokenKind::TemplateExprEnd,
            TokenKind::StrChunk("]".to_string()),
            TokenKind::TemplateEnd,
            TokenKind::Eof,
        ]
    );
}

#[test]
fn template_sub_expression_spans_are_absolute() {
    // `"ab $xy"`: the `xy` ident must carry its own offsets in the
    // original source (5..7), not offsets inside the literal text.
    let tokens = Lexer::new("\"ab $xy\"").tokenize().unwrap();
    assert_eq!(tokens[3].kind, TokenKind::Ident("xy".to_string()));
    assert_eq!(tokens[3].span, Span::new(5, 7));
}

#[test]
fn rejects_an_unterminated_interpolation() {
    assert!(Lexer::new("\"${x\"").tokenize().is_err());
}

#[test]
fn describes_a_token_as_its_source_text() {
    assert_eq!(TokenKind::Semi.describe(), "';'");
    assert_eq!(TokenKind::LBracket.describe(), "'['");
    assert_eq!(TokenKind::Arrow.describe(), "'=>'");
    assert_eq!(TokenKind::Fun.describe(), "'fun'");
    assert_eq!(TokenKind::Newline.describe(), "end of line");
    assert_eq!(TokenKind::Eof.describe(), "end of file");
    assert_eq!(TokenKind::StringLit("a".to_string()).describe(), "\"a\"");
    assert_eq!(TokenKind::Ident("count".to_string()).describe(), "'count'");
    assert_eq!(TokenKind::IntLit(7).describe(), "7");
}

// Multi-line string blocks: `"""` opens one, a line holding only `"""` (after
// its indentation) closes it. The value is the lines between, with the closing
// line's indentation removed, a final newline, and the string escapes.

fn block_value(src: &str) -> String {
    let tokens = Lexer::new(src).tokenize().expect("lex failed");
    tokens
        .into_iter()
        .find_map(|t| match t.kind {
            TokenKind::StringLit(s) => Some(s),
            _ => None,
        })
        .expect("no string literal")
}

fn block_error(src: &str) -> LexError {
    Lexer::new(src).tokenize().expect_err("lex should fail")
}

#[test]
fn a_block_keeps_its_lines_and_drops_the_closing_indentation() {
    let src = "val s = \"\"\"\n    first\n      second\n\n    end\n    \"\"\"\n";
    // The same text written with escapes.
    assert_eq!(block_value(src), "first\n  second\n\nend\n");
}

#[test]
fn a_block_ends_with_a_newline_and_the_opening_line_break_is_not_part_of_it() {
    assert_eq!(block_value("\"\"\"\n    one\n    \"\"\""), "one\n");
    assert_eq!(block_value("\"\"\"\n    one\n    two\n    \"\"\""), "one\ntwo\n");
}

#[test]
fn an_empty_block_is_the_empty_string() {
    assert_eq!(block_value("\"\"\"\n\"\"\""), "");
}

#[test]
fn a_block_with_no_indentation_on_its_closing_line_keeps_every_line_whole() {
    assert_eq!(block_value("\"\"\"\n  a\nb\n\"\"\""), "  a\nb\n");
}

#[test]
fn a_blank_line_is_empty_and_a_whitespace_line_keeps_what_is_past_the_indentation() {
    // Closing indentation is 4 spaces. A 2-space blank line is shorter than
    // that, so it becomes empty; a 6-space line keeps its last 2 spaces.
    let src = "\"\"\"\n    a\n  \n      b\n    \"\"\"";
    assert_eq!(block_value(src), "a\n\n  b\n");
}

#[test]
fn a_literal_quote_needs_no_escape_inside_a_block() {
    let src = "\"\"\"\n    say \"\"\"hi\"\"\" and \"x\"\n    \"\"\"";
    assert_eq!(block_value(src), "say \"\"\"hi\"\"\" and \"x\"\n");
}

#[test]
fn a_line_that_starts_with_three_quotes_is_escaped_to_stay_content() {
    // `\"""` is the content line `"""`; it does not close the block.
    let src = "\"\"\"\n    \\\"\"\"\n    \"\"\"";
    assert_eq!(block_value(src), "\"\"\"\n");
}

#[test]
fn a_block_takes_the_string_escapes_after_the_indentation_is_removed() {
    let src = "\"\"\"\n    tab\\there\\\\ \\x41 \\\"q\\\" \\$5\n    \"\"\"";
    assert_eq!(block_value(src), "tab\there\\ A \"q\" $5\n");
}

#[test]
fn a_block_with_crlf_line_endings_has_lf_in_its_value() {
    let src = "\"\"\"\r\n    a\r\n    b\r\n    \"\"\"\r\n";
    assert_eq!(block_value(src), "a\nb\n");
}

#[test]
fn a_block_keeps_non_ascii_text() {
    assert_eq!(block_value("\"\"\"\n    héllo 中\n    \"\"\""), "héllo 中\n");
}

#[test]
fn a_block_can_be_followed_by_more_tokens_on_its_closing_line() {
    let tokens = kinds("f(\"\"\"\n    a\n    \"\"\")");
    assert_eq!(
        tokens,
        vec![
            TokenKind::Ident("f".to_string()),
            TokenKind::LParen,
            TokenKind::StringLit("a\n".to_string()),
            TokenKind::RParen,
            TokenKind::Eof,
        ]
    );
}

#[test]
fn an_unterminated_block_is_refused_at_its_opening_with_a_clear_message() {
    let err = block_error("val s = \"\"\"\n    never closed\n    more\n");
    assert!(
        err.message.contains("unterminated multi-line string literal"),
        "message was: {}",
        err.message
    );
    assert!(err.message.contains("line 1"), "message was: {}", err.message);
    assert_eq!(err.span.start, 8);
}

#[test]
fn a_block_line_less_indented_than_its_closing_line_is_refused() {
    let err = block_error("\"\"\"\n  short\n    \"\"\"");
    assert!(err.message.contains("less indented"), "message was: {}", err.message);
    assert!(err.message.contains("line 2"), "message was: {}", err.message);
}

#[test]
fn text_after_the_opening_delimiter_on_its_line_is_refused() {
    let err = block_error("\"\"\"abc\"\"\"");
    assert!(err.message.contains("opens with"), "message was: {}", err.message);
}

#[test]
fn an_unknown_escape_inside_a_block_is_refused() {
    let err = block_error("\"\"\"\n    \\q\n    \"\"\"");
    assert!(err.message.contains("unknown escape '\\q'"), "message was: {}", err.message);
}

#[test]
fn a_nul_escape_inside_a_block_is_refused() {
    let err = block_error("\"\"\"\n    \\x00\n    \"\"\"");
    assert!(err.message.contains("NUL-terminated"), "message was: {}", err.message);
}
