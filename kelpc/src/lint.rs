//! `kelpc lint` (open item 23): the warnings that are not the compiler's job.
//!
//! A lint never fails a build and never rewrites a file, so the command's whole
//! contract is "read the sources, print warnings, exit zero". What it needs
//! from the compiler is the token stream *with* comments, which the formatter
//! already paid for: [`Lexer::tokenize_with_trivia`] returns every comment as a
//! token, so a rule about comments can be stated over tokens rather than over
//! raw text, and a `//` inside a string or inside a block comment cannot be
//! mistaken for one.
//!
//! There is one rule so far. The configurable naming case (a *second* want in
//! the same open item) needs a decision this file does not make: where the
//! setting lives, `coral.toml` or a flag, and whether the stdlib's locked
//! `snake_case` convention stays an exception to it.
//!
//! ## The comment run
//!
//! Two or more consecutive `//` lines with no blank line and no code between
//! them suggest one `/* ... */` comment:
//!
//! ```kelp
//! // Kelp has no string builder, so this walks the list and formats each
//! // element into a buffer, then joins the pieces with a comma.
//! ```
//!
//! That is a block comment written as a run of line comments, and the language
//! has the spelling for it. What the lint does **not** do is decide intent: the
//! author may have meant two lines, so the finding is a warning and the exit
//! stays zero.
//!
//! What breaks a run, each for a reason:
//!
//! - **A blank line.** `// a`, an empty line, `// b` is two thoughts, which is
//!   how the author separated them.
//! - **Any code.** A run is comments and their line terminators and nothing
//!   else. `// a` then `val x = 1` then `// b` is not a run, and neither is a
//!   trailing comment after code followed by a full-line one: a run has to be
//!   *lines* of comments, so its first comment must be the first token on its
//!   line.
//! - **A block comment.** `// a`, `/* b */`, `// c` has a block comment in the
//!   middle, so it is not a run of line comments.
//! - **A doc comment.** `///` and `//!` are documentation the tooling reads as
//!   documentation, and a doc comment has its own home (`///` on a declaration,
//!   `//!` at the top of a module), so a run of doc lines is not the shape this
//!   rule is about. `////` is excluded with them: it starts with `///`.

use crate::lexer::{LexError, Lexer, Span, Token, TokenKind};

/// One run of consecutive `//` lines: `span` is the *first* comment's own span
/// (that is the location a warning names) and `lines` is how many comments the
/// run holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommentRun {
    pub span: Span,
    pub lines: usize,
}

impl CommentRun {
    /// The warning this run earns. The wording names what was found (how many
    /// lines) and what it suggests (one block comment), which is all a reader
    /// needs to agree or disagree.
    pub fn message(&self) -> String {
        format!(
            "{} consecutive '//' lines suggest one '/* ... */' comment",
            self.lines
        )
    }
}

/// Every comment run in `src`, in source order.
///
/// A lexing failure is returned rather than reported: the caller has the file
/// and the diagnostic machinery, so it decides how to print it. `kelpc lint`
/// prints it the way `kelpc build` prints the same failure for the same file.
pub fn comment_runs(src: &str) -> Result<Vec<CommentRun>, LexError> {
    let tokens = Lexer::new(src).tokenize_with_trivia()?;
    let mut runs = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        if !starts_a_run(&tokens, i) {
            i += 1;
            continue;
        }
        // Walk forward while each line is one more `//` comment of the run. The
        // index always ends up past everything the run consumed, so no comment
        // is counted twice and no run is reported twice.
        let mut lines = 1;
        let mut end = i + 1;
        while let Some(next) = next_line_comment(&tokens, end, src) {
            lines += 1;
            end = next + 1;
        }
        if lines >= 2 {
            runs.push(CommentRun {
                span: tokens[i].span,
                lines,
            });
        }
        i = end;
    }
    Ok(runs)
}

/// Whether the token at `i` is a `//` comment that begins a run: the token is a
/// non-doc line comment, and it is the first token on its line (either the file
/// starts with it, or the token before it is a line terminator).
///
/// The "first token on its line" half is what keeps `val x = 1 // why` from
/// pairing with the comment on the next line, and it is why a *trailing*
/// comment is not the start of a run even when the next line is also a comment.
fn starts_a_run(tokens: &[Token], i: usize) -> bool {
    is_line_comment(&tokens[i]) && (i == 0 || matches!(tokens[i - 1].kind, TokenKind::Newline))
}

/// The index of the next line comment in a run that starts after `i`, or `None`
/// when the run ends there.
///
/// Between the two comments there has to be exactly one line terminator, and
/// counting the terminators that are *in* the newline token is the way to ask:
/// the lexer's newline token spans the line ending and then the indentation of
/// the next line (one `Newline` token per run of blank lines, whichever
/// indentation follows it), so its own length says nothing about how many lines
/// were skipped. Two newlines in that span is a blank line, which ends the run.
/// A `\r\n` file is unaffected: the `\r` is whitespace the lexer skips, and the
/// span still holds one `\n`.
fn next_line_comment(tokens: &[Token], i: usize, src: &str) -> Option<usize> {
    let newline = tokens.get(i)?;
    if !matches!(newline.kind, TokenKind::Newline)
        || src[newline.span.start..newline.span.end]
            .bytes()
            .filter(|b| *b == b'\n')
            .count()
            != 1
    {
        return None;
    }
    let next = i + 1;
    if is_line_comment(tokens.get(next)?) {
        Some(next)
    } else {
        None
    }
}

/// A `//` comment, as opposed to a `/* ... */` one, and not a doc comment.
fn is_line_comment(token: &Token) -> bool {
    match &token.kind {
        TokenKind::Comment(text) => text.starts_with("//") && !is_doc_comment(text),
        _ => false,
    }
}

/// `///`, `////`, `//!`: a comment the tooling reads as documentation rather
/// than as prose a reader skims, so the block-comment suggestion does not apply.
fn is_doc_comment(text: &str) -> bool {
    text.starts_with("///") || text.starts_with("//!")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The runs as `(byte offset of the first comment, number of lines)`, which
    /// is all a caller of a run needs.
    fn runs(src: &str) -> Vec<(usize, usize)> {
        comment_runs(src)
            .expect("the fixture lexes")
            .into_iter()
            .map(|r| (r.span.start, r.lines))
            .collect()
    }

    /// The offset of `needle` in a fixture, so a test says *which* comment a run
    /// starts at instead of counting bytes by hand.
    fn at(src: &str, needle: &str) -> usize {
        src.find(needle).expect("the needle is in the fixture")
    }

    #[test]
    fn two_consecutive_line_comments_are_one_run() {
        let src = "// one\n// two\nfun main() {}\n";
        assert_eq!(runs(src), vec![(at(src, "// one"), 2)]);
    }

    #[test]
    fn a_run_counts_every_line_it_holds() {
        let src = "// one\n// two\n// three\n// four\n";
        assert_eq!(runs(src), vec![(at(src, "// one"), 4)]);
    }

    #[test]
    fn a_single_line_comment_is_not_a_run() {
        assert_eq!(runs("// only one\nfun main() {}\n"), vec![]);
        assert_eq!(runs("fun main() {}\n// trailing\n"), vec![]);
    }

    #[test]
    fn a_blank_line_between_two_comments_ends_the_run() {
        let src = "// one\n\n// two\n// three\n";
        assert_eq!(runs(src), vec![(at(src, "// two"), 2)]);
    }

    #[test]
    fn code_between_two_comments_ends_the_run() {
        let src = "// one\nfun main() {}\n// two\n// three\n";
        assert_eq!(runs(src), vec![(at(src, "// two"), 2)]);
    }

    #[test]
    fn a_comment_after_code_is_not_the_start_of_a_run() {
        // The first comment is a trailing one: its line holds code, so the two
        // lines are not a run of comment lines.
        let src = "val x = 1 // why\n// and another\n";
        assert_eq!(runs(src), vec![]);
    }

    #[test]
    fn a_block_comment_between_line_comments_ends_the_run() {
        let src = "// one\n/* two */\n// three\n// four\n";
        assert_eq!(runs(src), vec![(at(src, "// three"), 2)]);
    }

    #[test]
    fn a_doc_comment_run_is_not_a_run() {
        assert_eq!(runs("/// one\n/// two\nfun main() {}\n"), vec![]);
        assert_eq!(runs("//! one\n//! two\nfun main() {}\n"), vec![]);
    }

    #[test]
    fn a_doc_comment_ends_a_run_of_ordinary_comments() {
        let src = "// one\n/// two\n/// three\n";
        assert_eq!(runs(src), vec![]);
    }

    #[test]
    fn a_line_comment_inside_a_string_is_not_a_comment() {
        let src = "val s = \"// not a comment\"\nval t = \"// also not\"\n";
        assert_eq!(runs(src), vec![]);
    }

    #[test]
    fn indented_comments_are_still_a_run() {
        // Indentation varies inside a run, and that is exactly what a block
        // comment would make uniform; the run is found anyway. The lexer's
        // newline token carries the next line's indentation, which is why the
        // rule counts terminators instead of measuring that span.
        let src = "fun main() {\n    // one\n        // two\n}\n";
        assert_eq!(runs(src), vec![(at(src, "// one"), 2)]);
    }

    #[test]
    fn two_runs_are_reported_separately() {
        let src = "// one\n// two\n\nfun main() {}\n\n// three\n// four\n";
        assert_eq!(
            runs(src),
            vec![(at(src, "// one"), 2), (at(src, "// three"), 2)]
        );
    }

    #[test]
    fn the_message_names_the_count_and_the_suggestion() {
        let src = "// one\n// two\n// three\n";
        let found = comment_runs(src).expect("lexes");
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].message(),
            "3 consecutive '//' lines suggest one '/* ... */' comment"
        );
    }

    #[test]
    fn a_lex_error_is_returned_rather_than_swallowed() {
        assert!(comment_runs("// one\n/* unterminated\n").is_err());
    }

    #[test]
    fn the_carriage_return_of_a_crlf_file_does_not_break_a_run() {
        let src = "// one\r\n// two\r\n";
        assert_eq!(runs(src), vec![(0, 2)]);
    }
}
