/// One rule the formatter enforces, with the corpus pairs that pin it.
///
/// The module header's bullet list is these statements verbatim and in this
/// order, and the tests at the end of this file compare the two, so the prose a
/// reader is given here cannot claim more than [`RULES`] states.
#[allow(dead_code)]
pub(crate) struct Rule {
    /// Short name, used in the header and in the tests.
    pub(crate) name: &'static str,
    /// What the rule does, in one line.
    pub(crate) statement: &'static str,
    /// Stems of the `tests/fmt_corpus/<stem>.kp` pairs that declare this rule -
    /// the pair's own first line names the rule, and the tests check that name
    /// against this list, not that the pair exercises the rule.
    pub(crate) pins: &'static [&'static str],
}

/// Every rule the formatter enforces. A rule with no declared corpus pair is a
/// test failure, and so is a corpus pair no rule here claims, so a rule cannot
/// be added, and a pair cannot go unclaimed, silently.
#[allow(dead_code)]
pub(crate) const RULES: &[Rule] = &[
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
                    rewritten or dropped - a block comment's own newlines and inner \
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
        statement: "No space goes before `,` `:` `.` `::` `)` `]` or `?`.",
        pins: &["spacing_return_arrow"],
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
                    follows `(` `[` `.` or `::` with a space.",
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
        statement: "An inline `alias _n = …` declaration is one atom copied to the end \
                    of its line, and a `_name` reference is an ordinary identifier.",
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
        pins: &[
            "default_one_space",
            "extern_type",
            "extern_type_parent",
            "scoped_blocks",
            "defer",
            "traits_syntax",
            "traits_generic",
            "const_generics",
        ],
    },
];

/// One reason the formatter declines a file, with whether `kelpc build` accepts
/// the same file.
#[allow(dead_code)]
pub(crate) struct Refusal {
    /// The closed reason this entry documents.
    pub(crate) kind: RefusalKind,
    /// The condition that fires, in one line. The header's bullet for this
    /// reason is this text verbatim.
    pub(crate) reason: &'static str,
    /// Whether `kelpc build` accepts the same file.
    pub(crate) build_accepts: bool,
    /// Where the refusal comes from.
    pub(crate) raised_by: &'static str,
}

/// The reasons `fmt` declines a file by naming a variant, one entry per
/// [`RefusalKind`] variant on the `next` chain [`RefusalKind::all`] walks, in the
/// enumeration's order. A unit test compares [`RefusalKind::all`] against this
/// list, so a variant on that chain the data omits, or an entry for no chained
/// variant, is a test failure; a variant off the chain is in neither.
#[allow(dead_code)]
pub(crate) const REFUSALS: &[Refusal] = &[
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
        reason: "an alias that cannot be expanded: a duplicate or invalid definition, a \
                 cycle, or replacement text that does not lex",
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
/// compares [`RefusalKind::all`] - the chain `next` walks - against [`REFUSALS`],
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
    pub(crate) fn next(self) -> Option<RefusalKind> {
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
    pub(crate) fn all() -> Vec<RefusalKind> {
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
