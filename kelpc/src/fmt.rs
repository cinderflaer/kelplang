//! `kelpc fmt` - a whitespace and comment-preserving source reformatter.
//!
//! The formatter is token-level, and that is the whole design. It lexes the
//! raw source with comments kept as tokens ([`Lexer::tokenize_with_trivia`])
//! and re-emits every token's own source slice, changing only the whitespace
//! *between* tokens and the indentation at the start of a line. No token's
//! text is ever rewritten, so a literal's spelling, a `_alias`'s macro
//! text and a template's contents survive byte-for-byte by construction.
//!
//! Why not an AST printer: the information a formatter needs is information
//! the AST drops.
//!
//! - Comments are discarded by the lexer before the parser runs, and the AST
//!   has no comment node, so an AST printer cannot reproduce them at all.
//! - A run of blank lines is collapsed by the lexer into one `Newline` token,
//!   so only a formatter that reads raw spans can see the run and rewrite it.
//! - Literals are stored decoded (`StringLit(String)`, `FloatLit(f64)`), so a
//!   printer has to re-encode them; copying the source slice cannot lose a
//!   byte.
//!
//! Two lists in this file - [`RULES`] and [`REFUSALS`] - are the single source
//! of truth for what the formatter enforces and for when it refuses a file. The
//! prose that describes them is checked against them: the bullet list under
//! *Rules* is the rule list's own statements verbatim, the bullet list under
//! *Reasons a file is refused* is the refusal list's own conditions verbatim,
//! and every rule names the `tests/fmt_corpus/<name>.kp` pair whose own first
//! line names that rule back. The formatter declines a file by naming a variant
//! of [`RefusalKind`], and a unit test
//! (`the_refusal_enumeration_and_the_refusal_data_agree`) compares
//! [`RefusalKind::all`] - the chain `next()` walks from the first variant -
//! against [`REFUSALS`], same names, same order, same count. A variant `next()`
//! leaves off that chain is in neither list, and the comparison reads only the
//! variants it reaches. A rule with no
//! bullet, a bullet with no rule, a rule with no declared corpus pair, a pair
//! claimed by two rules, and a corpus pin whose first line names a rule that
//! does not claim it are all test failures.
//!
//! What that does *not* cover, and no test here can close: prose outside the two
//! enumerated sections is unconstrained, so a paragraph may claim anything the
//! lists do not contradict; a rule whose statement is wrong in the same way as
//! its pinning pair survives, because the tests compare the data to the prose
//! and to the corpus and never to what the emitter does with a pair; and a
//! refusal raised with a raw message, or a variant `next` leaves off the chain
//! `all` walks, is covered by no check, because the comparison reads only the
//! variants on that chain and the command's own argument errors print strings
//! that name no variant.
//!
//! ## Rules this formatter enforces
//!
//! [`RULES`] is the list: each rule has a short name, a one-line statement of
//! exactly what it does, and the corpus pairs that declare it (the pair's own
//! first line names the rule back; that the pair exercises the rule is not
//! tested). The bullets below are its statements verbatim, in its order. One
//! qualifier applies to every rule:
//! a space a rule removes is put back where removing it would change the token
//! sequence ([`space-suppression-never-relexes`]). The 100-column limit, the part
//! of the canonical declaration order the parser does not already reject, and
//! the naming/case rules are deliberately not touched - see the design report for
//! why each is a lint, not a format.
//!
//! - **indent-four-spaces** - A line the formatter re-indents is written with four spaces per brace level, recomputed from the source's own brace depth.
//! - **no-tabs-in-emitted-indentation** - Every indentation the formatter writes is spaces: a tab in a line's leading whitespace never survives (the line is re-indented, or a tab on a continuation line becomes four spaces), while a tab inside a comment or a literal is that token's own byte and is copied verbatim.
//! - **continuation-line-keeps-author-indent** - A line inside an open `(` or `[` keeps the author's own leading whitespace instead of the brace depth's, spaces as written and each tab as four spaces.
//! - **blank-lines-at-most-one** - At most one blank line is kept between top-level declarations, a run inside a block keeps the author's grouping, and a blank line the author did not write is never inserted.
//! - **trailing-whitespace-outside-comments** - Trailing whitespace is removed from a line of code, while whitespace inside a comment is the comment's own text and is kept.
//! - **crlf-line-endings-become-lf** - A CRLF line ending is written as LF, because the `\r` before a newline is whitespace to the lexer and never reaches the output.
//! - **single-trailing-newline** - The output ends with exactly one newline, and a source whose bytes are all whitespace is emitted empty.
//! - **comments-verbatim-bytes** - Comment text is copied byte-for-byte, a `///` doc comment and a `/* … */` block comment included, so no byte inside a comment is rewritten or dropped - a block comment's own newlines and inner indentation among them.
//! - **comment-keeps-its-line** - A comment stays on the line the author wrote it on, with the gap around it normalised like any other gap.
//! - **one-space-around-binary-and-assignment** - One space goes before and after a binary operator and an `=`.
//! - **one-space-after-comma** - One space goes after a `,` and none goes before it.
//! - **no-space-before-suffix-token** - No space goes before `,` `:` `.` `::` `)` `]` or `?`.
//! - **brace-spacing** - `{` takes one space before it, except after a token that takes no space after itself (as `(` does in `foo({ 1 })`) and between the two braces of `{}`.
//! - **no-space-after-open-paren** - `(` and `[` attach to a token that ends an expression (`f(x)`, `a[i]`) and take a space after one that does not (`= (a + b)`), and nothing follows `(` `[` `.` or `::` with a space.
//! - **prefix-operator-binds** - A prefix operator (`!`, `~`, and `-` `+` `*` `&` with nothing that ends an expression before them) binds to its operand with no space.
//! - **space-suppression-never-relexes** - A space is dropped only where the two atoms re-lex to exactly their own tokens, so `& &` keeps its space instead of becoming `&&`.
//! - **alias-declaration-and-reference-atomic** - An inline `alias _n = …` declaration is one atom copied to the end of its line, and a `_name` reference is an ordinary identifier.
//! - **template-run-atomic** - A string template run is one atom, copied verbatim, so its `StrChunk` text and the layout inside it are never re-spaced.
//! - **angle-brackets-keep-author-spacing** - `<` `>` `<=` and `>=` keep the author's spacing, attached in `List<T>` and spaced in `a > b`, because a token-level formatter cannot tell a generic from a comparison.
//! - **default-one-space-between-atoms** - Two atoms that share a line get one space between them unless another rule here removes it, so no pair outside these rules is ever run together.
//!
//! ## What is checked, and what is refused
//!
//! Nothing is formatted that does not parse. The alias gate is exact for the
//! aliases *the file actually uses*, and no check here loads a module graph, so
//! a file that references no unresolvable alias is formatted even when the
//! surrounding project is broken. Two checks stand behind that:
//!
//! - [`format`], this module's own gate, parses the file after expanding the
//!   `alias _n = ...` declarations *the file itself* holds. It decides every
//!   file that declares what it uses, and its verdict is the only one many
//!   files need.
//! - the compiler's resolver, which `main.rs` puts to a file that gate refuses
//!   (`fmt_text`). It loads the project and expands the file against the
//!   project's whole alias table, the `[alias]` section of `coral.toml` included, which is what
//!   `kelpc build` uses for that file. A file it accepts is formatted; a file it
//!   rejects is refused with its diagnostic, which is the text and position
//!   `kelpc build` prints.
//!
//! That boundary is narrower than "the alias table a file is parsed under is
//! the one `kelpc build` uses": the resolver runs only for a file whose own gate
//! refuses, and it is asked only about that file. So for a file whose own gate
//! *accepts* it - one that references no unresolvable alias - the resolver
//! never runs, and `fmt` neither loads the project's alias configuration nor
//! diagnoses it: a cyclic or malformed `[alias]` section, or an imported module
//! with a syntax error, is something `kelpc build` exits 1 on and reports,
//! while such a file is formatted. A file the gate refuses does reach
//! `[alias]`, through the resolver, and can be refused with its
//! diagnostic.
//!
//! An undefined alias, and an alias another module declares whose expansion
//! does not parse, are therefore both refusals and not whitespace edits: no
//! placeholder is ever substituted for a reference, because a placeholder would
//! format a file the compiler rejects.
//!
//! ## Reasons a file is refused
//!
//! [`REFUSALS`] is the list of reasons `fmt` declines a file by naming a
//! variant: the condition that fires, and whether `kelpc build` accepts the same
//! file. The bullet list below is its conditions verbatim, in its order, and a
//! unit test compares the two, so a bullet cannot say anything the data does not.
//! The formatter names a variant of [`RefusalKind`] when it declines a file, and
//! the same unit test checks [`RefusalKind::all`] against [`REFUSALS`] - same
//! names, same order, same count - over the variants that chain reaches; a
//! refusal raised outside it, or a variant the chain omits, is not compared.
//!
//! Only `lone-carriage-return` declines a file whose *program* `kelpc build`
//! accepts. The other four that `kelpc build` does not fail on are each a verdict
//! on something other than the file's text: `not-a-regular-file` is about the
//! node, `write-failed` about the destination filesystem,
//! `check-found-unformatted` is the `--check` verdict (it writes nothing and
//! exits nonzero because the file is not already formatted), and
//! `token-fidelity-internal-error` can only fire on an emitter bug.
//! `path-names-nothing`, `dangling-symlink` and `unreadable-file` are paths with
//! no readable file behind them, which `kelpc build` fails on too; a symbolic
//! link that names a *directory* is `not-a-regular-file`, because the file the
//! link names is a directory.
//!
//! - **`not-a-regular-file`** - the path, or the file a link names, is a FIFO, a socket, a device or a directory
//! - **`path-names-nothing`** - the path cannot be read at all (`stat` on it fails), so there is no file to format
//! - **`dangling-symlink`** - the path is a symbolic link whose target cannot be read, so there is no file to format
//! - **`unreadable-file`** - the bytes cannot be read, or are not UTF-8 text
//! - **`lex-error`** - the source does not lex
//! - **`syntax-error`** - the preprocessed source does not parse, by this module's own gate or by the resolver that `main.rs` consults after that gate refuses
//! - **`unresolvable-alias`** - an alias that cannot be expanded: a duplicate or invalid definition, a cycle, or replacement text that does not lex
//! - **`lone-carriage-return`** - a carriage return in the whitespace between tokens that is not part of a CRLF pair
//! - **`token-fidelity-internal-error`** - the emitted text's significant tokens differ from the input's
//! - **`write-failed`** - the temporary could not be written, or the rename over the target failed
//! - **`check-found-unformatted`** - `--check` found the file differs from its formatted form
//!
//! ## Five limits that remain
//!
//! None is a correctness limit:
//!
//! - **No canonical form for an author-wrapped continuation line inside an
//!   open `(` or `[`.** Such a line keeps the author's own leading whitespace
//!   when that whitespace is spaces; a tab there becomes four spaces
//!   ([`push_indent`] states the rule, and why the line is otherwise left
//!   alone: it is the aligned continuation the §2.2 example shows, and
//!   re-indenting it would move arguments off their own columns). Wrapping such
//!   a line is the author's choice, not the formatter's. The same is true of
//!   `a < b` against `List<T>`: they are one token apart (`needs_space`), so the
//!   author's spacing is kept there too. Both are design choices, not defects.
//! - **`lone-carriage-return` refuses a file whose line break is a carriage
//!   return.** A `\r` in the whitespace *between* tokens is blank trivia to the
//!   lexer (`Lexer::skip_ws`), never a line break, so the formatter cannot see a
//!   line ending there. Where the byte was standing in for a line break, the two
//!   sides of it are laid out on one line; anywhere else - leading whitespace,
//!   trailing whitespace, or whitespace inside a line - it separates nothing and
//!   is dropped like any other whitespace, and nothing is joined. The refusal
//!   states which of the two happened for the byte it names. This is the only
//!   input shape in which `fmt` declines a program `kelpc build` accepts: the
//!   lexer skips the `\r`, so the file parses and builds, and only a reformat
//!   would change it. (`not-a-regular-file` declines a node rather than a
//!   program; a `\r` *inside* a string or character literal is that literal's
//!   own byte, is copied verbatim like every other token byte, and does not
//!   trigger this refusal.) A lexer limit, not a formatting rule - CRLF files
//!   are ordinary input and are formatted normally (their line endings become
//!   LF, which `tests/fmt_corpus/crlf.*.kp` pins).
//! - **The rewrite replaces the file's inode.** `-w` writes a temporary beside
//!   the target and renames it into place, so a reader sees the old bytes or the
//!   new ones and never a mixture (`write_atomic` in `main.rs`) - and the file
//!   that lands is a *new inode*. A hard link to the formatted file therefore
//!   keeps the old bytes, and so does a file descriptor another process already
//!   holds. The temporary is created by the writing process, and `write_atomic`
//!   copies the target's *mode bits* onto it and nothing else. Of the four things
//!   a replacement file could carry, two are measured: the permission bits and
//!   the group are both asserted by `tests/fmt.rs`, whose inode test moves the
//!   file to a secondary group and requires the rewrite not to carry it (the
//!   group was checked by hand in an earlier round, before that assertion
//!   existed); the
//!   owner cannot be varied without privilege, so it rests on reading
//!   `write_atomic`; ACLs and extended attributes rest on the same reading -
//!   `set_permissions` with mode bits has nowhere to put either, and no test
//!   measures them. The new file's
//!   modification time is the write's own. Preserving the inode would mean
//!   writing in place, which is the half-written file this design exists to
//!   avoid; a file that must keep its inode has to be formatted another way
//!   (`kelpc fmt FILE > tmp` and then a copy back). The hard-link outcome and the
//!   absent temp both have tests.
//! - **An abrupt death of the process can leave a temporary behind.** The write
//!   is atomic and a failure removes its own `.kelpc-fmt-<pid>-<seq>.tmp`, but
//!   `SIGKILL` (or `SIGXFSZ` from a file-size limit) runs no in-process
//!   cleanup, so that temporary stays in the source directory for the reader to
//!   remove.
//! - **The cleanup evidence is weaker where user namespaces are unavailable.**
//!   The filesystem-failure tests (`tests/fmt.rs`) build their conditions inside
//!   an unprivileged user and mount namespace; on a machine that cannot make
//!   one they print `SKIPPED` and exercise nothing, so the cleanup path is
//!   verified only where `unshare -Urm` works.
//!
//! Two constructs are copied verbatim as atoms, because re-spacing them would
//! corrupt text that is never type-checked (the rules
//! `alias-declaration-and-reference-atomic` and `template-run-atomic`):
//!
//! - an inline `alias _name = <raw text to end of line>` declaration, whose
//!   right-hand side is macro text spliced in later (`preprocess.rs`);
//! - a `_name` reference, which is an ordinary identifier: the formatter
//!   needs no special case for it, and `preprocess` replaces it only when a
//!   declared alias has that name.
//!
//! A string template run is copied verbatim for the same reason: its inner
//! `StrChunk` text is decoded, so it cannot be safely re-emitted from tokens.

use crate::lexer::{Lexer, Span, Token, TokenKind};
use crate::parser::Parser;
use crate::preprocess::AliasTable;

mod rules;
pub(crate) use rules::*;

/// Module id the gate's expanded copy is attributed to. It is never a real
/// module - the gate only asks whether the copy parses - so it cannot collide
/// with one.
const FMT_MODULE: &str = "<fmt>";

/// A failure that stops the formatter. The span is in the source that was
/// handed in, so the caller can render `file:line:col`.
#[derive(Debug)]
pub struct FmtError {
    /// The reason, from [`RefusalKind`]. Every error the formatter builds names
    /// one.
    #[allow(dead_code)]
    pub kind: RefusalKind,
    pub message: String,
    pub span: Span,
}

impl FmtError {
    /// Builds an error for a reason from [`RefusalKind`].
    fn refusal(kind: RefusalKind, message: String, span: Span) -> FmtError {
        FmtError { kind, message, span }
    }
}

/// Formats `src`, checking this file's own declarations before touching it.
/// On success the result is the formatted source, ended by exactly one newline
/// (unless the source held no tokens at all). On failure nothing is formatted:
/// a lex error, an unresolvable alias or a parse error is returned as it is.
///
/// This is the gate for a caller that holds one file's text and nothing else
/// - the unit tests, and the first check the CLI applies to every file. An
/// alias a *different* module declares is not visible here, so a file that
/// uses one is refused even though it is valid Kelp; `format_after_resolve` is
/// the entry point for a file the compiler's resolver has accepted.
pub fn format(src: &str) -> Result<String, FmtError> {
    parse_gate(src)?;
    format_after_resolve(src)
}

/// Formats `src` without the parse gate, for a caller that has already had the
/// file accepted by `kelpc build`'s own resolver. That acceptance is the
/// stronger check: the resolver expands the file against the whole project's
/// alias table and parses the result, where this module's gate can only see
/// the declarations the file itself holds.
///
/// Emission is unchanged either way - every token is copied from its own source
/// slice - and the token-fidelity post-condition below still runs, so this can
/// only ever skip a *refusal*, never a check that the output is the same
/// program.
pub fn format_after_resolve(src: &str) -> Result<String, FmtError> {
    let tokens = Lexer::new(src)
        .tokenize_with_trivia()
        .map_err(|e| FmtError::refusal(RefusalKind::LexError, e.message, e.span))?;
    reject_lone_carriage_returns(src, &tokens)?;
    let out = emit(src, &tokens);
    check_token_fidelity(src, &out)?;
    Ok(out)
}

/// Refuses a file that uses a lone carriage return as a line terminator.
///
/// The rule is about the whitespace *between* tokens, which is the only place a
/// `\r` can be doing that job: the lexer skips it there (`Lexer::skip_ws`)
/// rather than ending a line on it, so no line break it was written to make is
/// visible to the formatter. The bytes inside a token are not the formatter's to
/// reinterpret: a `\r` inside a string or character literal is that literal's own
/// byte, is copied verbatim like every other token byte, and is not this case.
/// `\r\n` is an ordinary line ending (the lexer consumes the pair) and is not
/// this case either.
///
/// The refusal itself says what would happen to the byte: text on either side of
/// it on its own line is laid out on one line, and a byte with nothing on one
/// side is dropped like any other whitespace. Both are computed here, because a
/// byte at the end of the file or in a line's leading whitespace joins nothing,
/// and saying it would join would be false of it.
///
/// `tokens` are the spans the check needs; the caller has them from the lex it
/// already does, and a source that does not lex has a lex error to report
/// instead of this one.
///
/// Reading the spans has one visible consequence: a `\r` inside the whitespace
/// run that a `Newline` token already covers - the bytes right after a real
/// line break - is not flagged, because it is not ending a line that has not
/// already ended, and the formatter drops it with the rest of that gap.
fn reject_lone_carriage_returns(src: &str, tokens: &[Token]) -> Result<(), FmtError> {
    let bytes = src.as_bytes();
    // Token spans are neither always in source order nor always disjoint (a
    // template's `StrChunk` and `ExprStart` overlap by design), so they are
    // merged first: what the check wants is the set of bytes they cover, and
    // the gaps between them are the whitespace between tokens.
    let mut spans: Vec<(usize, usize)> = tokens
        .iter()
        .map(|t| (t.span.start.min(src.len()), t.span.end.min(src.len())))
        .collect();
    spans.sort_unstable();
    let mut gaps: Vec<(usize, usize)> = Vec::new();
    let mut covered = 0usize;
    for (start, end) in spans {
        if start > covered {
            gaps.push((covered, start));
        }
        covered = covered.max(end);
    }
    gaps.push((covered, src.len()));

    for (from, to) in gaps {
        for i in from..to {
            // The `\n` that pairs a `\r` may be the first byte of the next
            // token - a `Newline` token starts *at* the `\n`, after the `\r`
            // has been skipped as whitespace - so the pair is read from the
            // whole source, not from the gap.
            if bytes[i] == b'\r' && bytes.get(i + 1) != Some(&b'\n') {
                return Err(lone_cr_error(src, i));
            }
        }
    }
    Ok(())
}

/// The refusal for a lone `\r` at byte `at`.
///
/// It names the condition that fired - a carriage return in whitespace that is
/// not part of a token and is not part of a CRLF pair, the whitespace the lexer
/// skips between tokens (and before the first one and after the last) - and then
/// only the consequence that really applies: the byte stands between text on both
/// sides of it on its own line, so that text is laid out on one line; or it has
/// nothing on one side of it (leading whitespace, trailing whitespace, or a file
/// whose only byte it is), so it is dropped like any other whitespace and
/// nothing is joined.
fn lone_cr_error(src: &str, at: usize) -> FmtError {
    let bytes = src.as_bytes();
    let line_start = src[..at].rfind('\n').map(|p| p + 1).unwrap_or(0);
    let before = bytes[line_start..at]
        .iter()
        .any(|b| !b.is_ascii_whitespace());
    let line_end = src[at + 1..]
        .find('\n')
        .map(|p| at + 1 + p)
        .unwrap_or(src.len());
    let after = bytes[at + 1..line_end]
        .iter()
        .any(|b| !b.is_ascii_whitespace());
    let consequence = if before && after {
        "it stands between text on both sides of it on its line, so that text is laid out on \
         one line"
    } else {
        "it has nothing on one side of it, so it is dropped like any other whitespace and \
         nothing is joined"
    };
    FmtError::refusal(
        RefusalKind::LoneCarriageReturn,
        format!(
            "a carriage return sits in the whitespace between tokens - or before the first \
             one, or after the last - and is not part of a CRLF pair: the Kelp lexer reads \
             such a byte as whitespace, never as a line ending, so no line break it was \
             written to make is visible to the formatter, and {consequence}. Convert the \
             file's line endings to LF (or CRLF) and format the file again"
        ),
        Span::new(at, at + 1),
    )
}

/// Refuses to format a file that does not parse, so a syntax error is always
/// reported as a syntax error and never turned into a whitespace edit. This
/// runs for every file, whether it uses aliases or not.
///
/// The parser reads a *preprocessed* copy, because `preprocess` strips every
/// inline `alias _n = ...` declaration and expands every `_n` reference
/// before a real compile parses; the raw stream is never what the parser sees.
/// Gating on a preprocessed copy is what makes an alias-using file obey the
/// same rule as every other file instead of bypassing the gate.
///
/// The table built here holds this file's inline declarations and nothing
/// else. A `_name` identifier no declaration names is an ordinary name and is
/// formatted like any other. A string that is exactly one undeclared `_name`
/// (a module path, as in `impl "_util"`) is refused here, so `fmt_text` in
/// `main.rs` puts the file to the resolver, which has the project's table.
fn parse_gate(src: &str) -> Result<(), FmtError> {
    let trivia = Lexer::new(src)
        .tokenize_with_trivia()
        .map_err(|e| FmtError::refusal(RefusalKind::LexError, e.message, e.span))?;
    reject_lone_carriage_returns(src, &trivia)?;
    let raw = Lexer::new(src)
        .tokenize()
        .map_err(|e| FmtError::refusal(RefusalKind::LexError, e.message, e.span))?;
    let expanded = expand_own_aliases(src, &raw)?;
    Parser::new(expanded)
        .parse_program()
        .map(|_| ())
        .map_err(|e| FmtError::refusal(RefusalKind::SyntaxError, e.message, e.span))
}

/// Expands the aliases one file declares, using `preprocess` - the same scanner
/// and the same expansion `kelpc build` runs. Every failure is the gate's
/// refusal: an unresolvable reference, an alias cycle, alias text that does not
/// lex, or a declaration with no replacement text.
fn expand_own_aliases(src: &str, raw: &[Token]) -> Result<Vec<Token>, FmtError> {
    let as_fmt_error = |e: crate::diag::Diagnostic| {
        FmtError::refusal(RefusalKind::UnresolvableAlias, e.message, e.span)
    };
    let defs = crate::preprocess::scan_inline_aliases(raw, src, FMT_MODULE).map_err(as_fmt_error)?;
    let mut table = AliasTable::new();
    for def in defs {
        table.define(def).map_err(as_fmt_error)?;
    }
    let expanded = table.expand_file(raw, FMT_MODULE).map_err(as_fmt_error)?;
    // A `_name` this file does not declare may be one another module declares.
    // The gate cannot see that module, so it hands the file to the resolver
    // (see `fmt_text` in `main.rs`) rather than accept it as a plain name.
    // Only a string that is exactly one undeclared `_name`, as in
    // `impl "_util"`, is handed to the resolver: that is a module path an
    // alias from another module may supply. An ordinary `_name` identifier is
    // a plain name here (it may be a local), so it is formatted like any other.
    // The expanded stream is checked, so an alias declaration's own text is
    // never looked at.
    for token in &expanded {
        let undeclared = match &token.kind {
            TokenKind::StringLit(name)
                if crate::preprocess::is_alias_name(name) && !table.declares(name) =>
            {
                Some(name.clone())
            }
            _ => None,
        };
        if let Some(name) = undeclared {
            return Err(FmtError::refusal(
                RefusalKind::UnresolvableAlias,
                format!("'{name}' is not declared in this file"),
                token.span,
            ));
        }
    }
    Ok(expanded)
}

/// The whole-output post-condition. A formatter that can change the token
/// sequence must never be able to do so silently, so after emitting, the
/// significant token sequence (kind and exact source slice, trivia excluded)
/// of the result is compared with the input's. Any difference is an internal
/// error that stops the write - the formatter's contract is that it never
/// changes the program, and this is what makes that true rather than claimed.
fn check_token_fidelity(src: &str, out: &str) -> Result<(), FmtError> {
    let before = significant_tokens(src)
        .map_err(|e| FmtError::refusal(RefusalKind::LexError, e.message, e.span))?;
    let after = significant_tokens(out).map_err(|e| {
        FmtError::refusal(
            RefusalKind::TokenFidelityInternalError,
            format!(
                "internal error: kelpc fmt produced text that does not lex: {}",
                e.message
            ),
            Span::new(0, 0),
        )
    })?;
    for i in 0..before.len().max(after.len()) {
        let b = before.get(i);
        let a = after.get(i);
        let same = match (b, a) {
            (Some(b), Some(a)) => {
                b.kind == a.kind && src[b.span.start..b.span.end] == out[a.span.start..a.span.end]
            }
            _ => false,
        };
        if !same {
            let span = b.map(|b| b.span).unwrap_or(Span::new(src.len(), src.len()));
            return Err(FmtError::refusal(
                RefusalKind::TokenFidelityInternalError,
                format!(
                    "internal error: kelpc fmt would change token {i}: input {}, output {}",
                    token_text(b, src),
                    token_text(a, out),
                ),
                span,
            ));
        }
    }
    Ok(())
}

/// The tokens of `src` with trivia removed: `Newline` is dropped because
/// blank-line runs are the formatter's to normalize, and `Eof` carries no
/// text. Everything else keeps its kind and its exact source span.
fn significant_tokens(src: &str) -> Result<Vec<Token>, crate::lexer::LexError> {
    Ok(Lexer::new(src)
        .tokenize()?
        .into_iter()
        .filter(|t| !matches!(t.kind, TokenKind::Newline | TokenKind::Eof))
        .collect())
}

/// A one-token account for a fidelity-error message.
fn token_text(token: Option<&Token>, src: &str) -> String {
    match token {
        Some(t) => format!("{:?} {:?}", t.kind, &src[t.span.start..t.span.end]),
        None => "nothing".to_string(),
    }
}

/// How an emitted atom behaves for spacing. A template run and a `_name`
mod emit;
pub(crate) use emit::*;

#[cfg(test)]
#[path = "fmt/tests.rs"]
mod tests;
