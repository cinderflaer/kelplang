//! `kelpc fmt` — a whitespace and comment-preserving source reformatter.
//!
//! The formatter is token-level, and that is the whole design. It lexes the
//! raw source with comments kept as tokens ([`Lexer::tokenize_with_trivia`])
//! and re-emits every token's own source slice, changing only the whitespace
//! *between* tokens and the indentation at the start of a line. No token's
//! text is ever rewritten, so a literal's spelling, a `%*alias*%`'s macro
//! text and a template's contents survive byte-for-byte by construction.
//!
//! Why not an AST printer (`pretty.rs` is one, and is dead code): the
//! information a formatter needs is information the AST drops.
//!
//! - Comments are discarded by the lexer before the parser runs, and the AST
//!   has no comment node, so an AST printer cannot reproduce them at all.
//! - A run of blank lines is collapsed by the lexer into one `Newline` token,
//!   so only a formatter that reads raw spans can see the run and rewrite it.
//! - Literals are stored decoded (`StringLit(String)`, `FloatLit(f64)`), so a
//!   printer has to re-encode them; copying the source slice cannot lose a
//!   byte.
//!
//! Two lists in this file — [`RULES`] and [`REFUSALS`] — are the single source
//! of truth for what the formatter enforces and for when it refuses a file. The
//! prose that describes them is checked against them: the bullet list under
//! *Rules* is the rule list's own statements verbatim, the bullet list under
//! *Reasons a file is refused* is the refusal list's own conditions verbatim,
//! and every rule names the `tests/fmt_corpus/<name>.kp` pair whose own first
//! line names that rule back. The formatter declines a file by naming a variant
//! of [`RefusalKind`], and a unit test
//! (`the_refusal_enumeration_and_the_refusal_data_agree`) compares
//! [`RefusalKind::all`] — the chain `next()` walks from the first variant —
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
//! the naming/case rules are deliberately not touched — see the design report for
//! why each is a lint, not a format.
//!
//! - **indent-four-spaces** — A line the formatter re-indents is written with four spaces per brace level, recomputed from the source's own brace depth.
//! - **no-tabs-in-emitted-indentation** — Every indentation the formatter writes is spaces: a tab in a line's leading whitespace never survives (the line is re-indented, or a tab on a continuation line becomes four spaces), while a tab inside a comment or a literal is that token's own byte and is copied verbatim.
//! - **continuation-line-keeps-author-indent** — A line inside an open `(` or `[` keeps the author's own leading whitespace instead of the brace depth's, spaces as written and each tab as four spaces.
//! - **blank-lines-at-most-one** — At most one blank line is kept between top-level declarations, a run inside a block keeps the author's grouping, and a blank line the author did not write is never inserted.
//! - **trailing-whitespace-outside-comments** — Trailing whitespace is removed from a line of code, while whitespace inside a comment is the comment's own text and is kept.
//! - **crlf-line-endings-become-lf** — A CRLF line ending is written as LF, because the `\r` before a newline is whitespace to the lexer and never reaches the output.
//! - **single-trailing-newline** — The output ends with exactly one newline, and a source whose bytes are all whitespace is emitted empty.
//! - **comments-verbatim-bytes** — Comment text is copied byte-for-byte, a `///` doc comment and a `/* … */` block comment included, so no byte inside a comment is rewritten or dropped — a block comment's own newlines and inner indentation among them.
//! - **comment-keeps-its-line** — A comment stays on the line the author wrote it on, with the gap around it normalised like any other gap.
//! - **one-space-around-binary-and-assignment** — One space goes before and after a binary operator and an `=`.
//! - **one-space-after-comma** — One space goes after a `,` and none goes before it.
//! - **no-space-before-suffix-token** — No space goes before `,` `:` `.` `)` `]` or `?`.
//! - **brace-spacing** — `{` takes one space before it, except after a token that takes no space after itself (as `(` does in `foo({ 1 })`) and between the two braces of `{}`.
//! - **no-space-after-open-paren** — `(` and `[` attach to a token that ends an expression (`f(x)`, `a[i]`) and take a space after one that does not (`= (a + b)`), and nothing follows `(` `[` or `.` with a space.
//! - **prefix-operator-binds** — A prefix operator (`!`, `~`, and `-` `+` `*` `&` with nothing that ends an expression before them) binds to its operand with no space.
//! - **space-suppression-never-relexes** — A space is dropped only where the two atoms re-lex to exactly their own tokens, so `& &` keeps its space instead of becoming `&&`.
//! - **alias-declaration-and-reference-atomic** — An inline `alias %*n*% = …` declaration is one atom copied to the end of its line, and a `%*name*%` reference is one atom whose five tokens stay adjacent.
//! - **template-run-atomic** — A string template run is one atom, copied verbatim, so its `StrChunk` text and the layout inside it are never re-spaced.
//! - **angle-brackets-keep-author-spacing** — `<` `>` `<=` and `>=` keep the author's spacing, attached in `List<T>` and spaced in `a > b`, because a token-level formatter cannot tell a generic from a comparison.
//! - **default-one-space-between-atoms** — Two atoms that share a line get one space between them unless another rule here removes it, so no pair outside these rules is ever run together.
//!
//! ## What is checked, and what is refused
//!
//! Nothing is formatted that does not parse. The alias gate is exact for the
//! aliases *the file actually uses*, and no check here loads a module graph, so
//! a file that references no unresolvable alias is formatted even when the
//! surrounding project is broken. Two checks stand behind that:
//!
//! - [`format`], this module's own gate, parses the file after expanding the
//!   `alias %*n*% = ...` declarations *the file itself* holds. It decides every
//!   file that declares what it uses, and its verdict is the only one many
//!   files need.
//! - the compiler's resolver, which `main.rs` puts to a file that gate refuses
//!   (`fmt_text`). It loads the project and expands the file against the
//!   project's whole alias table — `alias.kpconf` included — which is what
//!   `kelpc build` uses for that file. A file it accepts is formatted; a file it
//!   rejects is refused with its diagnostic, which is the text and position
//!   `kelpc build` prints.
//!
//! That boundary is narrower than "the alias table a file is parsed under is
//! the one `kelpc build` uses": the resolver runs only for a file whose own gate
//! refuses, and it is asked only about that file. So for a file whose own gate
//! *accepts* it — one that references no unresolvable alias — the resolver
//! never runs, and `fmt` neither loads the project's alias configuration nor
//! diagnoses it: a cyclic or malformed `alias.kpconf`, or an imported module
//! with a syntax error, is something `kelpc build` exits 1 on and reports,
//! while such a file is formatted. A file the gate refuses does reach
//! `alias.kpconf`, through the resolver, and can be refused with its
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
//! the same unit test checks [`RefusalKind::all`] against [`REFUSALS`] — same
//! names, same order, same count — over the variants that chain reaches; a
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
//! - **`not-a-regular-file`** — the path, or the file a link names, is a FIFO, a socket, a device or a directory
//! - **`path-names-nothing`** — the path cannot be read at all (`stat` on it fails), so there is no file to format
//! - **`dangling-symlink`** — the path is a symbolic link whose target cannot be read, so there is no file to format
//! - **`unreadable-file`** — the bytes cannot be read, or are not UTF-8 text
//! - **`lex-error`** — the source does not lex
//! - **`syntax-error`** — the preprocessed source does not parse, by this module's own gate or by the resolver that `main.rs` consults after that gate refuses
//! - **`unresolvable-alias`** — a `%*name*%` reference that neither the file's own declarations nor the project's alias table defines
//! - **`lone-carriage-return`** — a carriage return in the whitespace between tokens that is not part of a CRLF pair
//! - **`token-fidelity-internal-error`** — the emitted text's significant tokens differ from the input's
//! - **`write-failed`** — the temporary could not be written, or the rename over the target failed
//! - **`check-found-unformatted`** — `--check` found the file differs from its formatted form
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
//!   sides of it are laid out on one line; anywhere else — leading whitespace,
//!   trailing whitespace, or whitespace inside a line — it separates nothing and
//!   is dropped like any other whitespace, and nothing is joined. The refusal
//!   states which of the two happened for the byte it names. This is the only
//!   input shape in which `fmt` declines a program `kelpc build` accepts: the
//!   lexer skips the `\r`, so the file parses and builds, and only a reformat
//!   would change it. (`not-a-regular-file` declines a node rather than a
//!   program; a `\r` *inside* a string or character literal is that literal's
//!   own byte, is copied verbatim like every other token byte, and does not
//!   trigger this refusal.) A lexer limit, not a formatting rule — CRLF files
//!   are ordinary input and are formatted normally (their line endings become
//!   LF, which `tests/fmt_corpus/crlf.*.kp` pins).
//! - **The rewrite replaces the file's inode.** `-w` writes a temporary beside
//!   the target and renames it into place, so a reader sees the old bytes or the
//!   new ones and never a mixture (`write_atomic` in `main.rs`) — and the file
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
//!   `write_atomic`; ACLs and extended attributes rest on the same reading —
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
//! - an inline `alias %*name*% = <raw text to end of line>` declaration, whose
//!   right-hand side is macro text spliced in later (`preprocess.rs`);
//! - a `%*name*%` reference, whose five tokens must stay adjacent or
//!   `preprocess` stops reading it as a reference.
//!
//! A string template run is copied verbatim for the same reason: its inner
//! `StrChunk` text is decoded, so it cannot be safely re-emitted from tokens.

use crate::lexer::{Lexer, Span, Token, TokenKind};
use crate::parser::Parser;
use crate::preprocess::AliasTable;

/// One rule the formatter enforces, with the corpus pairs that pin it.
///
/// The module header's bullet list is these statements verbatim and in this
/// order, and the tests at the end of this file compare the two, so the prose a
/// reader is given here cannot claim more than [`RULES`] states.
#[allow(dead_code)]
struct Rule {
    /// Short name, used in the header and in the tests.
    name: &'static str,
    /// What the rule does, in one line.
    statement: &'static str,
    /// Stems of the `tests/fmt_corpus/<stem>.kp` pairs that declare this rule —
    /// the pair's own first line names the rule, and the tests check that name
    /// against this list, not that the pair exercises the rule.
    pins: &'static [&'static str],
}

/// Every rule the formatter enforces. A rule with no declared corpus pair is a
/// test failure, and so is a corpus pair no rule here claims, so a rule cannot
/// be added, and a pair cannot go unclaimed, silently.
#[allow(dead_code)]
const RULES: &[Rule] = &[
    Rule {
        name: "indent-four-spaces",
        statement: "A line the formatter re-indents is written with four spaces per \
                    brace level, recomputed from the source's own brace depth.",
        pins: &["indent_depth", "deeply_nested"],
    },
    Rule {
        name: "no-tabs-in-emitted-indentation",
        statement: "Every indentation the formatter writes is spaces: a tab in a line's \
                    leading whitespace never survives (the line is re-indented, or a tab \
                    on a continuation line becomes four spaces), while a tab inside a \
                    comment or a literal is that token's own byte and is copied \
                    verbatim.",
        pins: &["tabs"],
    },
    Rule {
        name: "continuation-line-keeps-author-indent",
        statement: "A line inside an open `(` or `[` keeps the author's own leading \
                    whitespace instead of the brace depth's, spaces as written and each \
                    tab as four spaces.",
        pins: &["continuation_indent"],
    },
    Rule {
        name: "blank-lines-at-most-one",
        statement: "At most one blank line is kept between top-level declarations, a run \
                    inside a block keeps the author's grouping, and a blank line the \
                    author did not write is never inserted.",
        pins: &["blank_lines_collapse", "consecutive_declarations"],
    },
    Rule {
        name: "trailing-whitespace-outside-comments",
        statement: "Trailing whitespace is removed from a line of code, while whitespace \
                    inside a comment is the comment's own text and is kept.",
        pins: &["comment_trailing_whitespace"],
    },
    Rule {
        name: "crlf-line-endings-become-lf",
        statement: "A CRLF line ending is written as LF, because the `\\r` before a \
                    newline is whitespace to the lexer and never reaches the output.",
        pins: &["crlf"],
    },
    Rule {
        name: "single-trailing-newline",
        statement: "The output ends with exactly one newline, and a source whose bytes \
                    are all whitespace is emitted empty.",
        pins: &["many_trailing_newlines", "no_trailing_newline"],
    },
    Rule {
        name: "comments-verbatim-bytes",
        statement: "Comment text is copied byte-for-byte, a `///` doc comment and a \
                    `/* … */` block comment included, so no byte inside a comment is \
                    rewritten or dropped — a block comment's own newlines and inner \
                    indentation among them.",
        pins: &["doc_comment", "block_comment"],
    },
    Rule {
        name: "comment-keeps-its-line",
        statement: "A comment stays on the line the author wrote it on, with the gap \
                    around it normalised like any other gap.",
        pins: &[
            "comment_at_eof",
            "comment_between_statements",
            "line_only_comment",
            "comment_in_arg_list",
            "comment_after_trailing_comma",
        ],
    },
    Rule {
        name: "one-space-around-binary-and-assignment",
        statement: "One space goes before and after a binary operator and an `=`.",
        pins: &["spacing_binary_ops"],
    },
    Rule {
        name: "one-space-after-comma",
        statement: "One space goes after a `,` and none goes before it.",
        pins: &["spacing_named_args"],
    },
    Rule {
        name: "no-space-before-suffix-token",
        statement: "No space goes before `,` `:` `.` `)` `]` or `?`.",
        pins: &["spacing_dot_return"],
    },
    Rule {
        name: "brace-spacing",
        statement: "`{` takes one space before it, except after a token that takes no \
                    space after itself (as `(` does in `foo({ 1 })`) and between the two \
                    braces of `{}`.",
        pins: &["spacing_paren_brace"],
    },
    Rule {
        name: "no-space-after-open-paren",
        statement: "`(` and `[` attach to a token that ends an expression (`f(x)`, `a[i]`) \
                    and take a space after one that does not (`= (a + b)`), and nothing \
                    follows `(` `[` or `.` with a space.",
        pins: &["spacing_brace_after_paren"],
    },
    Rule {
        name: "prefix-operator-binds",
        statement: "A prefix operator (`!`, `~`, and `-` `+` `*` `&` with nothing that ends \
                    an expression before them) binds to its operand with no space.",
        pins: &["spacing_unary"],
    },
    Rule {
        name: "space-suppression-never-relexes",
        statement: "A space is dropped only where the two atoms re-lex to exactly their own \
                    tokens, so `& &` keeps its space instead of becoming `&&`.",
        pins: &["prefix_operator_adjacency"],
    },
    Rule {
        name: "alias-declaration-and-reference-atomic",
        statement: "An inline `alias %*n*% = …` declaration is one atom copied to the end \
                    of its line, and a `%*name*%` reference is one atom whose five tokens \
                    stay adjacent.",
        pins: &["alias_atomic"],
    },
    Rule {
        name: "template-run-atomic",
        statement: "A string template run is one atom, copied verbatim, so its `StrChunk` \
                    text and the layout inside it are never re-spaced.",
        pins: &["template_atomic"],
    },
    Rule {
        name: "angle-brackets-keep-author-spacing",
        statement: "`<` `>` `<=` and `>=` keep the author's spacing, attached in `List<T>` \
                    and spaced in `a > b`, because a token-level formatter cannot tell a \
                    generic from a comparison.",
        pins: &["angle_bracket_spacing"],
    },
    Rule {
        name: "default-one-space-between-atoms",
        statement: "Two atoms that share a line get one space between them unless another \
                    rule here removes it, so no pair outside these rules is ever run \
                    together.",
        pins: &["default_one_space", "extern_type", "extern_type_parent"],
    },
];

/// One reason the formatter declines a file, with whether `kelpc build` accepts
/// the same file.
#[allow(dead_code)]
struct Refusal {
    /// The closed reason this entry documents.
    kind: RefusalKind,
    /// The condition that fires, in one line. The header's bullet for this
    /// reason is this text verbatim.
    reason: &'static str,
    /// Whether `kelpc build` accepts the same file.
    build_accepts: bool,
    /// Where the refusal comes from.
    raised_by: &'static str,
}

/// The reasons `fmt` declines a file by naming a variant, one entry per
/// [`RefusalKind`] variant on the `next` chain [`RefusalKind::all`] walks, in the
/// enumeration's order. A unit test compares [`RefusalKind::all`] against this
/// list, so a variant on that chain the data omits, or an entry for no chained
/// variant, is a test failure; a variant off the chain is in neither.
#[allow(dead_code)]
const REFUSALS: &[Refusal] = &[
    Refusal {
        kind: RefusalKind::NotARegularFile,
        reason: "the path, or the file a link names, is a FIFO, a socket, a device or a \
                 directory",
        build_accepts: true,
        raised_by: "`fmt_file`/`require_regular`/`collect_kp` in `main.rs`, before any read",
    },
    Refusal {
        kind: RefusalKind::PathNamesNothing,
        reason: "the path cannot be read at all (`stat` on it fails), so there is no file \
                 to format",
        build_accepts: false,
        raised_by: "`fmt_file`/`collect_kp` in `main.rs`",
    },
    Refusal {
        kind: RefusalKind::DanglingSymlink,
        reason: "the path is a symbolic link whose target cannot be read, so there is no \
                 file to format",
        build_accepts: false,
        raised_by: "`fmt_file` in `main.rs`",
    },
    Refusal {
        kind: RefusalKind::UnreadableFile,
        reason: "the bytes cannot be read, or are not UTF-8 text",
        build_accepts: false,
        raised_by: "`read_source` in `main.rs`",
    },
    Refusal {
        kind: RefusalKind::LexError,
        reason: "the source does not lex",
        build_accepts: false,
        raised_by: "`Lexer::tokenize_with_trivia` in `format_after_resolve`",
    },
    Refusal {
        kind: RefusalKind::SyntaxError,
        reason: "the preprocessed source does not parse, by this module's own gate or by \
                 the resolver that `main.rs` consults after that gate refuses",
        build_accepts: false,
        raised_by: "`parse_gate`, or the resolver `fmt_text` consults",
    },
    Refusal {
        kind: RefusalKind::UnresolvableAlias,
        reason: "a `%*name*%` reference that neither the file's own declarations nor the \
                 project's alias table defines",
        build_accepts: false,
        raised_by: "`expand_own_aliases`, then the resolver",
    },
    Refusal {
        kind: RefusalKind::LoneCarriageReturn,
        reason: "a carriage return in the whitespace between tokens that is not part of a \
                 CRLF pair",
        build_accepts: true,
        raised_by: "`reject_lone_carriage_returns`",
    },
    Refusal {
        kind: RefusalKind::TokenFidelityInternalError,
        reason: "the emitted text's significant tokens differ from the input's",
        build_accepts: true,
        raised_by: "`check_token_fidelity`",
    },
    Refusal {
        kind: RefusalKind::WriteFailed,
        reason: "the temporary could not be written, or the rename over the target failed",
        build_accepts: true,
        raised_by: "`write_atomic` in `main.rs`",
    },
    Refusal {
        kind: RefusalKind::CheckFoundUnformatted,
        reason: "`--check` found the file differs from its formatted form",
        build_accepts: true,
        raised_by: "`check_one` in `main.rs`",
    },
];

/// One reason `fmt` declines a file, as an enumeration.
///
/// The formatter names one of these variants when it declines a file. The
/// matches below have no wildcard, so a new variant stops the compiler until
/// `name` and `next` name it; the unit test
/// `the_refusal_enumeration_and_the_refusal_data_agree`
/// compares [`RefusalKind::all`] — the chain `next` walks — against [`REFUSALS`],
/// so an entry `REFUSALS` omits for a variant on that chain, or an entry for no
/// such variant, is a test failure. A variant `next` leaves off the chain is not
/// compared, and a refusal the command prints as a raw string names no variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefusalKind {
    NotARegularFile,
    PathNamesNothing,
    DanglingSymlink,
    UnreadableFile,
    LexError,
    SyntaxError,
    UnresolvableAlias,
    LoneCarriageReturn,
    TokenFidelityInternalError,
    WriteFailed,
    CheckFoundUnformatted,
}

impl RefusalKind {
    /// The refusal's name, as the header spells it.
    pub fn name(self) -> &'static str {
        match self {
            RefusalKind::NotARegularFile => "not-a-regular-file",
            RefusalKind::PathNamesNothing => "path-names-nothing",
            RefusalKind::DanglingSymlink => "dangling-symlink",
            RefusalKind::UnreadableFile => "unreadable-file",
            RefusalKind::LexError => "lex-error",
            RefusalKind::SyntaxError => "syntax-error",
            RefusalKind::UnresolvableAlias => "unresolvable-alias",
            RefusalKind::LoneCarriageReturn => "lone-carriage-return",
            RefusalKind::TokenFidelityInternalError => "token-fidelity-internal-error",
            RefusalKind::WriteFailed => "write-failed",
            RefusalKind::CheckFoundUnformatted => "check-found-unformatted",
        }
    }

    /// The variant after this one in `REFUSALS` order, or `None` for the last.
    /// The match has no wildcard, so a new variant is a compile error here until
    /// it is placed in the chain. This match is the chain [`RefusalKind::all`]
    /// walks, written out by hand: a variant whose `next` returns `None` and
    /// that no other arm points to is absent from `all` and from the comparison
    /// against `REFUSALS`.
    #[allow(dead_code)]
    fn next(self) -> Option<RefusalKind> {
        match self {
            RefusalKind::NotARegularFile => Some(RefusalKind::PathNamesNothing),
            RefusalKind::PathNamesNothing => Some(RefusalKind::DanglingSymlink),
            RefusalKind::DanglingSymlink => Some(RefusalKind::UnreadableFile),
            RefusalKind::UnreadableFile => Some(RefusalKind::LexError),
            RefusalKind::LexError => Some(RefusalKind::SyntaxError),
            RefusalKind::SyntaxError => Some(RefusalKind::UnresolvableAlias),
            RefusalKind::UnresolvableAlias => Some(RefusalKind::LoneCarriageReturn),
            RefusalKind::LoneCarriageReturn => Some(RefusalKind::TokenFidelityInternalError),
            RefusalKind::TokenFidelityInternalError => Some(RefusalKind::WriteFailed),
            RefusalKind::WriteFailed => Some(RefusalKind::CheckFoundUnformatted),
            RefusalKind::CheckFoundUnformatted => None,
        }
    }

    /// The variants the `next` chain reaches, in `REFUSALS` order, built by
    /// walking [`RefusalKind::next`] from the first variant.
    #[allow(dead_code)]
    fn all() -> Vec<RefusalKind> {
        let mut kinds = vec![RefusalKind::NotARegularFile];
        while let Some(next) = kinds.last().and_then(|kind| kind.next()) {
            kinds.push(next);
        }
        kinds
    }

    /// Pairs this kind with the text a refusal of it prints.
    pub fn message(self, message: impl Into<String>) -> String {
        debug_assert!(
            REFUSALS.iter().any(|refusal| refusal.kind == self),
            "refusal kind {} is not in REFUSALS",
            self.name()
        );
        message.into()
    }
}

/// Module id the gate's expanded copy is attributed to. It is never a real
/// module — the gate only asks whether the copy parses — so it cannot collide
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
/// — the unit tests, and the first check the CLI applies to every file. An
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
/// Emission is unchanged either way — every token is copied from its own source
/// slice — and the token-fidelity post-condition below still runs, so this can
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
/// run that a `Newline` token already covers — the bytes right after a real
/// line break — is not flagged, because it is not ending a line that has not
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
            // token — a `Newline` token starts *at* the `\n`, after the `\r`
            // has been skipped as whitespace — so the pair is read from the
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
/// It names the condition that fired — a carriage return in whitespace that is
/// not part of a token and is not part of a CRLF pair, the whitespace the lexer
/// skips between tokens (and before the first one and after the last) — and then
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
            "a carriage return sits in the whitespace between tokens — or before the first \
             one, or after the last — and is not part of a CRLF pair: the Kelp lexer reads \
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
/// inline `alias %*n*% = ...` declaration and expands every `%*n*%` reference
/// before a real compile parses; the raw stream is never what the parser sees.
/// Gating on a preprocessed copy is what makes an alias-using file obey the
/// same rule as every other file instead of bypassing the gate.
///
/// The table built here holds this file's inline declarations and nothing
/// else, so a reference they cannot resolve is a refusal — the diagnostic is
/// `preprocess`'s own `unknown alias '%*n*%'`, the one `kelpc build` prints —
/// and never a substitution of placeholder text. That substitution is what used
/// to let two files through: one whose alias was undefined, and one whose alias
/// another module declares with an expansion that does not parse. In both, the
/// layout around the reference was checked and the reference itself was not; the
/// resolver is what decides the second case now (see `fmt_text` in `main.rs`).
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

/// Expands the aliases one file declares, using `preprocess` — the same scanner
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
    table.expand_file(raw, FMT_MODULE).map_err(as_fmt_error)
}

/// The whole-output post-condition. A formatter that can change the token
/// sequence must never be able to do so silently, so after emitting, the
/// significant token sequence (kind and exact source slice, trivia excluded)
/// of the result is compared with the input's. Any difference is an internal
/// error that stops the write — the formatter's contract is that it never
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

/// How an emitted atom behaves for spacing. A template run and a `%*name*%`
/// reference behave like a primary expression (`Value`); the other two are
/// themselves.
enum Lead {
    Value,
    Decl,
    Comment,
    Tok(TokenKind),
}

impl Lead {
    /// Does this atom end an expression? It is what decides whether a `-`, `+`,
    /// `*` or `&` after it is a binary operator or a prefix one, and whether a
    /// following `(` calls it or opens a group.
    fn ends_expr(&self) -> bool {
        match self {
            Lead::Value => true,
            Lead::Comment | Lead::Decl => false,
            Lead::Tok(k) => matches!(
                k,
                TokenKind::IntLit(_)
                    | TokenKind::FloatLit(_)
                    | TokenKind::StringLit(_)
                    | TokenKind::CharLit(_)
                    | TokenKind::ByteLit(_)
                    | TokenKind::Ident(_)
                    | TokenKind::True
                    | TokenKind::False
                    | TokenKind::RParen
                    | TokenKind::RBracket
                    | TokenKind::RBrace
                    | TokenKind::Question
                    // `closure(&T)` — the keyword is a type constructor, always
                    // followed by its parenthesized argument list, so it
                    // attaches like a name.
                    | TokenKind::Closure
            ),
        }
    }
}

/// One unit the emitter copies as a whole and spaces against its neighbours.
struct Atom<'a> {
    text: &'a str,
    lead: Lead,
    /// Index of the first token of this atom.
    first: usize,
    /// Index of the first token after this atom.
    next: usize,
    /// Source offset the next atom's gap starts at.
    end: usize,
}

fn atom<'a>(src: &'a str, tokens: &[Token], i: usize) -> Atom<'a> {
    let tok = &tokens[i];

    // A template run is one atom; its endpoint is the `TemplateEnd` that
    // closes the `TemplateStart`.
    if matches!(tok.kind, TokenKind::TemplateStart) {
        let end_tok = {
            let mut depth = 0i32;
            let mut j = i;
            loop {
                match tokens[j].kind {
                    TokenKind::TemplateStart => depth += 1,
                    TokenKind::TemplateEnd => {
                        depth -= 1;
                        if depth == 0 {
                            break j;
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
        };
        let start = tokens[i].span.start;
        let end = tokens[end_tok].span.end;
        return Atom {
            text: &src[start..end],
            lead: Lead::Value,
            first: i,
            next: end_tok + 1,
            end,
        };
    }

    // `%*name*%` — copied verbatim, so its five tokens stay adjacent.
    if let Some(len) = alias_ref_len(tokens, i) {
        let start = tokens[i].span.start;
        let end = tokens[i + len - 1].span.end;
        return Atom {
            text: &src[start..end],
            lead: Lead::Value,
            first: i,
            next: i + len,
            end,
        };
    }

    // An inline `alias ...` declaration — the whole line, verbatim.
    if is_alias_decl(tokens, i) {
        let start = tok.span.start;
        let term = alias_decl_end(tokens, i);
        let line_end = tokens[term].span.start;
        return Atom {
            text: src[start..line_end].trim_end(),
            lead: Lead::Decl,
            first: i,
            next: term,
            end: line_end,
        };
    }

    if let TokenKind::Comment(_) = tok.kind {
        return Atom {
            text: &src[tok.span.start..tok.span.end],
            lead: Lead::Comment,
            first: i,
            next: i + 1,
            end: tok.span.end,
        };
    }

    Atom {
        text: &src[tok.span.start..tok.span.end],
        lead: Lead::Tok(tok.kind.clone()),
        first: i,
        next: i + 1,
        end: tok.span.end,
    }
}

fn emit<'a>(src: &'a str, tokens: &[Token]) -> String {
    let mut out = String::with_capacity(src.len() + 1);
    let mut prev: Option<(Atom<'a>, bool)> = None; // previous atom, was-prefix
    let mut depth = 0usize;
    let mut paren_depth = 0usize;
    let mut i = 0usize;

    while i < tokens.len() {
        match tokens[i].kind {
            TokenKind::Newline | TokenKind::Eof => {
                i += 1;
                continue;
            }
            _ => {}
        }
        let a = atom(src, &tokens, i);
        let start = tokens[i].span.start;
        let kind = match &a.lead {
            Lead::Tok(k) => Some(k.clone()),
            _ => None,
        };

        // A closing brace is indented one level out, so its depth is reduced
        // before the line's indentation is computed.
        if matches!(kind, Some(TokenKind::RBrace)) {
            depth = depth.saturating_sub(1);
        }

        let prev_ends = prev
            .as_ref()
            .map(|(atom, _)| atom.lead.ends_expr())
            .unwrap_or(false);
        let cur_prefix = is_prefix(&a.lead, prev_ends);

        match &prev {
            None => {
                push_indent(&mut out, "", depth, paren_depth, false);
                out.push_str(a.text);
            }
            Some((patom, pprefix)) => {
                let gap = &src[patom.end..start];
                let newlines = gap.bytes().filter(|&b| b == b'\n').count();
                if newlines == 0 {
                    let gap_ws = gap.bytes().any(|b| matches!(b, b' ' | b'\t' | b'\r'));
                    let mut space = needs_space(&patom.lead, *pprefix, &a.lead, gap_ws);
                    // Never suppress a space when the two atoms' text would
                    // not re-lex to exactly those two atoms' own tokens.
                    if !space && !joins_cleanly(patom, &a, tokens) {
                        space = true;
                    }
                    if space {
                        out.push(' ');
                    }
                    out.push_str(a.text);
                } else {
                    let mut blanks = newlines - 1;
                    // At most one blank line between top-level declarations,
                    // and never a blank line the author did not write
                    // (`blank-lines-at-most-one`); a run inside a block keeps
                    // the author's grouping.
                    if depth == 0 && paren_depth == 0 && blanks > 1 {
                        blanks = 1;
                    }
                    for _ in 0..=blanks {
                        out.push('\n');
                    }
                    push_indent(&mut out, gap, depth, paren_depth, true);
                    out.push_str(a.text);
                }
            }
        }

        if matches!(kind, Some(TokenKind::LBrace)) {
            depth += 1;
        }
        match kind {
            Some(TokenKind::LParen) | Some(TokenKind::LBracket) => paren_depth += 1,
            Some(TokenKind::RParen) | Some(TokenKind::RBracket) => {
                paren_depth = paren_depth.saturating_sub(1)
            }
            _ => {}
        }

        let next = a.next;
        prev = Some((a, cur_prefix));
        i = next;
    }

    if !out.is_empty() {
        out.push('\n');
    }
    out
}

/// Appends the indentation for a line. A continuation line inside an open `(`
/// or `[` keeps the author's own leading whitespace (the aligned continuation
/// the §2.2 example shows), so the formatter has no canonical form for such a
/// line: spaces are preserved as written, and a tab there becomes four spaces.
/// Anywhere else the indentation is recomputed as four spaces per brace level,
/// which is what makes `indent-four-spaces` and
/// `no-tabs-in-emitted-indentation` hold for every line it re-indents, while a
/// comment's or literal's own bytes are copied as they are. `gap` is the source
/// between the previous atom and this one when there is one.
fn push_indent(out: &mut String, gap: &str, depth: usize, paren_depth: usize, from_gap: bool) {
    if from_gap && paren_depth > 0 {
        let ws = match gap.rfind('\n') {
            Some(p) => &gap[p + 1..],
            None => gap,
        };
        for c in ws.chars() {
            match c {
                '\t' => out.push_str("    "),
                '\r' | '\n' => {}
                other => out.push(other),
            }
        }
    } else {
        for _ in 0..4 * depth {
            out.push(' ');
        }
    }
}

/// Whether an operator atom is a prefix one (`-x`) rather than a binary one
/// (`a - b`). `!` and `~` are always prefix; `- + * &` are prefix exactly when
/// nothing that can end an expression precedes them.
fn is_prefix(lead: &Lead, prev_ends_expr: bool) -> bool {
    match lead {
        Lead::Tok(TokenKind::Bang) | Lead::Tok(TokenKind::Tilde) => true,
        Lead::Tok(
            TokenKind::Minus | TokenKind::Plus | TokenKind::Star | TokenKind::Amp,
        ) => !prev_ends_expr,
        _ => false,
    }
}

/// Whether one space goes between two atoms that share a line. Joining rules
/// (returning `false`) are what remove space; the default is one space, so a
/// pair the table does not name is never run together.
fn needs_space(prev: &Lead, prev_prefix: bool, cur: &Lead, gap_ws: bool) -> bool {
    // `{}` — an empty block, never `{ }`.
    if matches!(prev, Lead::Tok(TokenKind::LBrace)) && matches!(cur, Lead::Tok(TokenKind::RBrace)) {
        return false;
    }
    // No space before a token that attaches to what precedes it. `;` is on
    // the list for the one place it can appear: the separator in an array's
    // `[T; N]`, which is written tight on its left and spaced on its right.
    if matches!(
        cur,
        Lead::Tok(
            TokenKind::Comma
                | TokenKind::Semi
                | TokenKind::RParen
                | TokenKind::RBracket
                | TokenKind::Dot
                | TokenKind::Colon
                | TokenKind::Question
        )
    ) {
        return false;
    }
    // No space after `(`, `[` or `.`.
    if matches!(
        prev,
        Lead::Tok(TokenKind::LParen | TokenKind::LBracket | TokenKind::Dot)
    ) {
        return false;
    }
    // `>` of `map<T, U>(...)` abuts the parameter list; `a > (b)` does not.
    // The angle brackets are the one pair a token-level formatter cannot tell
    // a generic from a comparison in, so the author's choice is kept.
    if matches!(
        cur,
        Lead::Tok(TokenKind::LParen | TokenKind::LBracket)
    ) && matches!(
        prev,
        Lead::Tok(TokenKind::Lt | TokenKind::Gt | TokenKind::LtEq | TokenKind::GtEq)
    ) {
        return gap_ws;
    }
    // A prefix operator binds to its operand (`!x`, `&x`, `-(a + b)`). This
    // precedes the call rule so `!(...)` does not read like a call.
    if prev_prefix {
        return false;
    }
    // `(`/`[` attach to a callee or index base, but open a group — with a
    // space — after an operator, `=`, `return`, or anything else that is not
    // the end of an expression.
    if matches!(
        cur,
        Lead::Tok(TokenKind::LParen | TokenKind::LBracket)
    ) {
        return !prev.ends_expr();
    }
    // `<`, `>`, `<=`, `>=` keep the author's spacing: attached in a type
    // (`List<T>`), spaced in a comparison (`a > b`).
    if matches!(
        prev,
        Lead::Tok(TokenKind::Lt | TokenKind::Gt | TokenKind::LtEq | TokenKind::GtEq)
    ) || matches!(
        cur,
        Lead::Tok(TokenKind::Lt | TokenKind::Gt | TokenKind::LtEq | TokenKind::GtEq)
    ) {
        return gap_ws;
    }
    true
}

/// Length in tokens of a contiguous `%*name*%` starting at `i`, or `None`.
/// Adjacency is by span, so `a % *b * %` is arithmetic and stays arithmetic.
fn alias_ref_len(tokens: &[Token], i: usize) -> Option<usize> {
    let t = |k: usize| tokens.get(i + k);
    let (pct, star, name, star2, pct2) = (t(0)?, t(1)?, t(2)?, t(3)?, t(4)?);
    if !matches!(pct.kind, TokenKind::Percent) || !matches!(star.kind, TokenKind::Star) {
        return None;
    }
    let TokenKind::Ident(n) = &name.kind else {
        return None;
    };
    if n.is_empty() {
        return None;
    }
    if !matches!(star2.kind, TokenKind::Star) || !matches!(pct2.kind, TokenKind::Percent) {
        return None;
    }
    let adjacent = pct.span.end == star.span.start
        && star.span.end == name.span.start
        && name.span.end == star2.span.start
        && star2.span.end == pct2.span.start;
    adjacent.then_some(5)
}

/// True when `tokens[i]` begins an inline `alias %*name*% = ...` declaration,
/// mirroring `preprocess::alias_decl_shape`: the keyword `alias` at the start
/// of a line, a contiguous `%*name*%`, then `=`.
fn is_alias_decl(tokens: &[Token], i: usize) -> bool {
    if !matches!(&tokens[i].kind, TokenKind::Ident(n) if n == "alias") {
        return false;
    }
    if i > 0
        && !matches!(
            tokens[i - 1].kind,
            TokenKind::Newline | TokenKind::LBrace | TokenKind::RBrace
        )
    {
        return false;
    }
    let Some(len) = alias_ref_len(tokens, i + 1) else {
        return false;
    };
    matches!(tokens.get(i + 1 + len).map(|t| &t.kind), Some(TokenKind::Eq))
}

/// Index of the token that ends an inline alias declaration's right-hand side,
/// mirroring `preprocess::alias_decl_shape`: a newline, a `}`, or EOF. The
/// declaration's text runs from its `alias` up to this token.
fn alias_decl_end(tokens: &[Token], i: usize) -> usize {
    let len = alias_ref_len(tokens, i + 1).expect("alias declaration without a name");
    let mut j = i + 1 + len + 1;
    while j < tokens.len()
        && !matches!(
            tokens[j].kind,
            TokenKind::Newline | TokenKind::Eof | TokenKind::RBrace
        )
    {
        j += 1;
    }
    j
}

/// Whether concatenating two atoms' own source text re-lexes to exactly the
/// two atoms' tokens, in order. The emitter drops a space only when this
/// holds, so no suppression can merge `&` and `&` into `&&`, turn `/ /` into a
/// comment, or otherwise change the token sequence. The expected tokens are
/// read from the input stream, not from lexing each atom in isolation — the
/// lexer folds `print!(` into one identifier, so an atom's text can lex
/// differently on its own than it does in context. Stating the rule over the
/// exact tokens is what makes it need no list of operators and miss no pair.
fn joins_cleanly(prev: &Atom, cur: &Atom, tokens: &[Token]) -> bool {
    let Ok(lexed) = Lexer::new(&format!("{}{}", prev.text, cur.text)).tokenize() else {
        return false;
    };
    let expected: Vec<TokenKind> = tokens[prev.first..prev.next]
        .iter()
        .chain(&tokens[cur.first..cur.next])
        .map(|t| t.kind.clone())
        .collect();
    let actual: Vec<TokenKind> = lexed
        .into_iter()
        .map(|t| t.kind)
        .filter(|k| !matches!(k, TokenKind::Eof))
        .collect();
    expected == actual
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    /// Every directory the formatter is expected to cover, minus the ones
    /// another workflow owns (`kelpc/tests/fixtures`, `kelpc/tests/fmt_corpus`).
    fn corpus_dirs() -> Vec<PathBuf> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        vec![
            root.join("std"),
            root.join("tests/projects"),
            root.join("../examples"),
            root.join("../bench"),
        ]
    }

    fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect(&path, out);
            } else if path.extension().and_then(|s| s.to_str()) == Some("kp") {
                out.push(path);
            }
        }
    }

    fn corpus() -> Vec<PathBuf> {
        let mut files = Vec::new();
        for dir in corpus_dirs() {
            collect(&dir, &mut files);
        }
        files.sort();
        files
    }

    /// The token stream with trivia removed: kind plus the exact source text
    /// the token came from. `Newline` is dropped because blank-line runs are
    /// deliberately normalized; comparing it would reject the reformat itself.
    fn code_tokens(src: &str) -> Vec<(String, String)> {
        Lexer::new(src)
            .tokenize_with_trivia()
            .expect("lex failed")
            .into_iter()
            .filter(|t| {
                !matches!(
                    t.kind,
                    TokenKind::Newline | TokenKind::Comment(_) | TokenKind::Eof
                )
            })
            .map(|t| {
                (
                    format!("{:?}", t.kind),
                    src[t.span.start..t.span.end].to_string(),
                )
            })
            .collect()
    }

    fn comments(src: &str) -> Vec<String> {
        Lexer::new(src)
            .tokenize_with_trivia()
            .expect("lex failed")
            .into_iter()
            .filter_map(|t| match t.kind {
                TokenKind::Comment(_) => Some(src[t.span.start..t.span.end].to_string()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn every_repository_file_is_idempotent_token_faithful_and_comment_preserving() {
        let files = corpus();
        assert!(files.len() >= 40, "only found {} files", files.len());
        for file in &files {
            let src = std::fs::read_to_string(file).unwrap();
            let once = match format(&src) {
                Ok(text) => text,
                // A file the gate refuses is exercised by the refusal tests
                // instead: a deliberate error fixture in one of these dirs, or
                // a file whose `%*alias*%` another module declares — this gate
                // reads one file's text, so it cannot see that module and is
                // right to refuse rather than substitute a placeholder.
                Err(_) => continue,
            };
            let twice = format(&once).expect("formatted output must format");
            assert_eq!(once, twice, "not idempotent: {}", file.display());
            assert_eq!(
                code_tokens(&src),
                code_tokens(&once),
                "token changed: {}",
                file.display()
            );
            assert_eq!(
                comments(&src),
                comments(&once),
                "comment changed: {}",
                file.display()
            );
        }
    }

    #[test]
    fn an_unsafe_block_and_an_unsafe_fun_round_trip() {
        // Stage n (MK9-S2): `unsafe` is a contextual word, so the formatter
        // treats it as one and `unsafe {` keeps its single space.
        let src = "unsafe fun poke(p: Ptr<i32>, v: i32) {\n    p.write(v)\n}\n\nfun main() {\n    unsafe {\n        p.write(1)\n    }\n    val q = unsafe { p.read() }\n}\n";
        let out = format(src).expect("the new syntax should format");
        assert_eq!(out, src);
    }

    #[test]
    fn a_parse_error_is_refused_and_nothing_is_formatted() {
        let err = format("fun main( {\n    print(\"x\")\n}\n").unwrap_err();
        assert!(!err.message.is_empty());
        assert_eq!(err.kind, RefusalKind::SyntaxError);
    }

    #[test]
    fn an_alias_declaration_and_reference_are_atomic() {
        // The declaration's right-hand side is macro text and is copied
        // verbatim (the double space survives); the five tokens of the
        // reference stay adjacent, so `preprocess` still reads it.
        let src = "alias %*a*% = 1 +  2\n\nfun main() {\n    print( %*a*% )\n}\n";
        let out = format(src).unwrap();
        assert_eq!(
            out,
            "alias %*a*% = 1 +  2\n\nfun main() {\n    print(%*a*%)\n}\n"
        );
    }

    #[test]
    fn a_prefix_operator_never_merges_with_the_token_after_it() {
        // `& &` must keep its space: `&&` is one token, so joining the two
        // borrows changes the program. The reported defect.
        let src = "fun main() {\n    val x = 5\n    val b = & & x\n}\n";
        let out = format(src).unwrap();
        assert!(out.contains("val b = & &x"), "{out}");
        assert_eq!(code_tokens(src), code_tokens(&out));
        assert_eq!(format(&out).unwrap(), out, "not idempotent");
    }

    #[test]
    fn the_token_fidelity_post_condition_rejects_a_changed_sequence() {
        // The post-condition is a real comparison, not a no-op: the same text
        // passes and a merged operator fails with an internal error.
        assert!(check_token_fidelity("a & & b", "a & & b").is_ok());
        let err = check_token_fidelity("a & & b", "a && b").unwrap_err();
        assert!(err.message.contains("internal error"), "{}", err.message);
    }

    #[test]
    fn an_alias_file_with_a_syntax_error_is_refused() {
        // The preprocessed copy is what the parser sees, so the alias does not
        // turn the syntax gate off.
        let src = "alias %*greeting*% = \"hi\"\n\nfun main() {\n    print(%*greeting*%)\n    print(\"oops\"\n}\n";
        let err = format(src).unwrap_err();
        assert!(err.message.contains("expected ')'"), "{}", err.message);
    }

    #[test]
    fn an_unresolvable_alias_is_refused_rather_than_papered_over() {
        // The gate has no table beyond this file's own declarations, so a
        // reference they cannot resolve is a refusal that names it. This used
        // to be a placeholder identifier, which formatted a file `kelpc build`
        // rejects; the resolver decides the project case instead (see
        // `fmt_text` in `main.rs`).
        let src = "fun main() {\n    print(%*undefined*%)\n}\n";
        let err = format(src).unwrap_err();
        assert_eq!(err.kind, RefusalKind::UnresolvableAlias);
        assert!(
            err.message.contains("unknown alias '%*undefined*%'"),
            "{}",
            err.message
        );
        // `format_after_resolve` is the emission half the CLI uses once the
        // resolver has accepted the file, and it does not re-run the gate.
        assert_eq!(format_after_resolve(src).unwrap(), src);
    }

    #[test]
    fn a_carriage_return_between_tokens_is_refused() {
        // The lexer reads a `\r` in the whitespace between tokens as trivia, so
        // a CR-terminated file is one line to it: no line break the byte was
        // written to make is visible, and the formatter refuses rather than
        // laying the text on either side of it out on one line. The message
        // says which of the two consequences applies (see
        // `a_carriage_return_that_joins_nothing_is_described_honestly`).
        let err = format("fun main() {\r    print(\"hi\")\r}\r").unwrap_err();
        assert_eq!(err.kind, RefusalKind::LoneCarriageReturn);
        assert!(err.message.contains("carriage return"), "{}", err.message);
        assert!(err.message.contains("between tokens"), "{}", err.message);
        // CRLF is ordinary input and is not affected: the pair is a line break.
        assert!(format("fun main() {\r\n    print(\"hi\")\r\n}\r\n").is_ok());
    }

    #[test]
    fn a_carriage_return_inside_a_literal_is_not_a_line_terminator() {
        // Defect 11: the refusal fired on any `\r` anywhere in the bytes, and
        // its diagnostic claimed the file "would be formatted as a single
        // line" — false on both counts for these inputs, which are multi-line,
        // are accepted by `kelpc build`, and format safely. The rule is about a
        // `\r` used as a line ending, and a literal's own bytes are not that.
        let string_lit = "fun main() {\n    print(\"a\rb\")\n}\n";
        assert_eq!(format(string_lit).unwrap(), string_lit);
        let char_lit = "fun main() {\n    val c = '\r'\n}\n";
        assert_eq!(format(char_lit).unwrap(), char_lit);
    }

    #[test]
    fn a_template_run_is_copied_verbatim() {
        let src = "fun main() {\n    val n = 1\n    print(\"sum=${n +  1}\")\n}\n";
        let out = format(src).unwrap();
        assert!(out.contains("\"sum=${n +  1}\""), "{out}");
    }

    /// The old refusal claimed the formatter "would join the text on either side
    /// of it onto one line", which is false of a `\r` with nothing on one side
    /// of it: leading whitespace, trailing whitespace at the end of the file, or
    /// the only byte in the file. The message now states the consequence that
    /// applies to the byte it names, and this pins both branches.
    #[test]
    fn a_carriage_return_that_joins_nothing_is_described_honestly() {
        // Text on both sides of the byte on its own line: the joining case.
        let err = format("fun main() {\r    print(\"x\")\r}\r").unwrap_err();
        assert!(
            err.message.contains("laid out on one line"),
            "the joining case was not described as one: {}",
            err.message
        );

        // Nothing on one side of the byte: the file's only byte, and a trailing
        // one after the last token. Neither joins anything, so the message must
        // not claim a join for either.
        for src in ["\r", "fun main() {}\r"] {
            let err = format(src).unwrap_err();
            assert!(
                err.message.contains("carriage return")
                    && err.message.contains("between tokens"),
                "{src:?} was refused without naming the condition: {}",
                err.message
            );
            assert!(
                err.message.contains("nothing is joined"),
                "{src:?} joins nothing and the message does not say so: {}",
                err.message
            );
            assert!(
                !err.message.contains("laid out on one line"),
                "{src:?} joins nothing and the message claims it does: {}",
                err.message
            );
        }
    }

    // -----------------------------------------------------------------------
    // the prose is checked against the data, not against another copy of itself
    // -----------------------------------------------------------------------

    const RULES_HEADING: &str = "## Rules this formatter enforces";
    const CHECKS_HEADING: &str = "## What is checked, and what is refused";
    const REFUSALS_HEADING: &str = "## Reasons a file is refused";
    const LIMITS_HEADING: &str = "## Five limits that remain";

    /// The module header: the doc comment at the top of this file, `//!` removed.
    /// The rules and refusals above it are the data it has to agree with, so the
    /// tests read the prose back out of the source file instead of trusting a
    /// copy of it held somewhere else.
    fn module_header() -> String {
        let mut out = String::new();
        for line in include_str!("fmt.rs").lines() {
            let Some(rest) = line.strip_prefix("//!") else {
                break;
            };
            out.push_str(rest);
            out.push('\n');
        }
        assert!(!out.is_empty(), "the module header was not found");
        out
    }

    /// The header from `heading` onwards.
    fn prose_from<'a>(header: &'a str, heading: &str) -> &'a str {
        let start = header
            .find(heading)
            .unwrap_or_else(|| panic!("the header has no '{heading}' section"));
        &header[start..]
    }

    /// The header from `from` up to the next section, `to`.
    fn prose_between<'a>(header: &'a str, from: &str, to: &str) -> &'a str {
        let section = prose_from(header, from);
        let end = section
            .find(to)
            .unwrap_or_else(|| panic!("the header has no '{to}' section after '{from}'"));
        &section[..end]
    }

    /// The `- **name** — statement` bullets of a prose section, in order.
    fn bullet_list(section: &str) -> Vec<(&str, &str)> {
        section
            .lines()
            .filter_map(|line| {
                let rest = line.trim_start().strip_prefix("- **")?;
                let (name, statement) = rest.split_once("** — ")?;
                Some((name, statement.trim_end()))
            })
            .collect()
    }

    /// The stems of the `tests/fmt_corpus/<stem>.kp` pairs on disk.
    fn corpus_pair_stems() -> Vec<String> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fmt_corpus");
        let mut stems = Vec::new();
        for entry in std::fs::read_dir(&dir).expect("reading tests/fmt_corpus") {
            let name = entry.expect("a directory entry").file_name();
            let name = name.to_string_lossy().to_string();
            if let Some(stem) = name.strip_suffix(".kp") {
                if !stem.ends_with(".fmt") {
                    stems.push(stem.to_string());
                }
            }
        }
        stems.sort();
        stems
    }

    /// The header's rule bullets are [`RULES`] itself: same names, same
    /// statements, same order. A bullet that is not a rule, a rule with no
    /// bullet, or a statement the two sides word differently all fail here.
    #[test]
    fn the_header_rule_bullets_are_the_rule_list_verbatim() {
        let header = module_header();
        let prose = prose_between(&header, RULES_HEADING, CHECKS_HEADING);
        let from_prose = bullet_list(prose);
        let from_data: Vec<(&str, &str)> = RULES
            .iter()
            .map(|rule| (rule.name, rule.statement))
            .collect();
        assert_eq!(
            from_prose, from_data,
            "the header's rule bullets are not the rule list, in its order"
        );
    }

    /// Every rule has at least one declared corpus pair, every pair a rule names
    /// exists, and every pair on disk is claimed by exactly one rule. What this
    /// proves is existence and a declared assignment; it does not prove that a
    /// pair exercises its rule (see the module header). A rule with no declared
    /// pair, and a pair no rule claims, are both failures.
    #[test]
    fn every_rule_is_pinned_by_a_corpus_pair_and_every_pair_is_claimed_once() {
        let pairs = corpus_pair_stems();
        assert!(
            pairs.len() >= 20,
            "only {} corpus pairs on disk",
            pairs.len()
        );
        let mut failures = Vec::new();
        for rule in RULES {
            if rule.pins.is_empty() {
                failures.push(format!("rule '{}' names no pinning pair", rule.name));
            }
            for pin in rule.pins {
                if rule.pins.iter().filter(|p| *p == pin).count() > 1 {
                    failures.push(format!("rule '{}' names '{pin}.kp' twice", rule.name));
                }
                if !pairs.iter().any(|pair| pair == pin) {
                    failures.push(format!(
                        "rule '{}' names '{pin}.kp', which is not in tests/fmt_corpus",
                        rule.name
                    ));
                }
            }
        }
        for pair in &pairs {
            let claims = RULES
                .iter()
                .filter(|rule| rule.pins.contains(&pair.as_str()))
                .count();
            if claims != 1 {
                failures.push(format!(
                    "tests/fmt_corpus/{pair}.kp is claimed by {claims} rules, not 1"
                ));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    /// Each corpus input's first line declares the rule it pins, and it must name
    /// exactly the rule [`RULES`] claims it for. A pair whose first line names a
    /// different rule, or a rule that names a pair whose line does not name it
    /// back, fails here. This proves the declared assignment, not that the pair
    /// exercises the rule.
    #[test]
    fn each_corpus_pair_declares_the_rule_that_claims_it() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fmt_corpus");
        let mut failures = Vec::new();
        for rule in RULES {
            for pin in rule.pins {
                let path = dir.join(format!("{pin}.kp"));
                let src = match std::fs::read_to_string(&path) {
                    Ok(src) => src,
                    Err(e) => {
                        failures.push(format!("reading {}: {e}", path.display()));
                        continue;
                    }
                };
                let first = src.lines().next().unwrap_or("");
                let Some(rest) = first.strip_prefix("// pins ") else {
                    failures.push(format!("{pin}.kp does not start with '// pins '"));
                    continue;
                };
                let declared = rest.split(':').next().unwrap_or("").trim();
                if declared != rule.name {
                    failures.push(format!(
                        "{pin}.kp declares '// pins {declared}', but rule '{}' claims it",
                        rule.name
                    ));
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    /// The refusal enumeration and the refusal data agree in name, order and
    /// count, over the variants that chain reaches. [`RefusalKind::all`] is built
    /// by walking [`RefusalKind::next`] (a wildcard-free match), so the comparison
    /// reads only the variants on that chain. Every entry of [`REFUSALS`] carries
    /// a kind, and no kind appears twice.
    #[test]
    fn the_refusal_enumeration_and_the_refusal_data_agree() {
        let from_enum: Vec<&str> = RefusalKind::all().iter().map(|kind| kind.name()).collect();
        let from_data: Vec<&str> = REFUSALS.iter().map(|refusal| refusal.kind.name()).collect();
        assert_eq!(
            from_enum, from_data,
            "the enumeration and REFUSALS disagree in name, order or count"
        );
        let mut kinds: Vec<&str> = REFUSALS.iter().map(|refusal| refusal.kind.name()).collect();
        kinds.sort_unstable();
        kinds.dedup();
        assert_eq!(
            kinds.len(),
            REFUSALS.len(),
            "REFUSALS carries the same kind twice"
        );
    }

    /// The header's refusal bullets are [`REFUSALS`] itself: same names, same
    /// conditions, same order. A bullet whose text the data does not carry — a
    /// false statement kept alive by its backticked name — fails here, which is
    /// what the older name-only check could not catch.
    #[test]
    fn the_header_refusal_bullets_are_the_refusal_list_verbatim() {
        let header = module_header();
        let prose = prose_between(&header, REFUSALS_HEADING, LIMITS_HEADING);
        let from_prose: Vec<(String, String)> = bullet_list(prose)
            .into_iter()
            .map(|(name, statement)| (name.to_string(), statement.to_string()))
            .collect();
        let from_data: Vec<(String, String)> = REFUSALS
            .iter()
            .map(|refusal| {
                (
                    format!("`{}`", refusal.kind.name()),
                    refusal.reason.to_string(),
                )
            })
            .collect();
        assert_eq!(
            from_prose, from_data,
            "the header's refusal bullets are not the refusal list, in its order"
        );
    }
}

