//! `kelpc lint` (open item 23): the warnings that are not the compiler's job.
//!
//! A lint never fails a build and never rewrites a file, so the command's whole
//! contract is "read the sources, print warnings, exit zero". What it needs
//! from the compiler is the token stream *with* comments, which the formatter
//! already paid for: [`Lexer::tokenize_with_trivia`] returns every comment as a
//! token, so a rule about comments can be stated over tokens rather than over
//! raw text, and a `//` inside a string or inside a block comment cannot be
//! mistaken for one. Every rule here is a rule about tokens, for the same
//! reason: `lint` lexes a file and reads nothing else, so a file that does not
//! parse, or that names an alias only its project can resolve, is still linted.
//!
//! There are five rules:
//!
//! - the **comment run** (roadmap item G-5): two or more consecutive `//`
//!   lines with no blank line between them.
//! - the **naming case** (roadmap item 23, `syntax.md` 19): the project's case,
//!   `camelCase` (the default) or `snake_case`, read from `coral.toml`'s
//!   `naming_case`. Only `camelCase` is checked (#368 phase 3).
//! - the **redundant borrow** on a parameter of an owned type or `str` (`&String`,
//!   `&List<T>`, `&Point` for a `datac`, `&str`, ...): a warning. See
//!   [`redundant_borrow_params_in`].
//! - the **redundant comparison** (`== true`, and `.len() == 0` where
//!   `.is_empty()` says it): a warning, never a failure.
//! - the **unused pattern binding** (`Err(e) -> 0`, where `e` is never read in
//!   its arm): a warning that names the fix, `_`. See [`unused_bindings_in`].
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
//!
//! ## The naming case
//!
//! The rule reads the names a file *declares* (see [`declared_names`]) and
//! asks whether each one is written in the project's case. A name the project
//! did not write is not its business, and that is the whole of the standard
//! library's exception: `fs.read_text` at a call site is a use, not a
//! declaration, so a project that sets `camelCase` is never warned about the
//! stdlib's `snake_case` and a project that sets `snake_case` is never warned
//! about its own call into `str.is_empty`.
//!
//! Only `camelCase` is checked. A project that sets `snake_case` gets no naming
//! finding at all, because the lint warns about a snake_case name only where
//! the project asks for camelCase; a camelCase name in a snake project is the
//! project's own choice.
//!
//! The setting is a project-wide convention and so it lives in the project's
//! manifest rather than in a per-invocation flag. A file with no manifest to
//! read gets the option's default (`camelCase`, `0`), and a file under the
//! project's `src/deps/` gets no case rule at all: a fetched dependency is not
//! this project's code and its names are not this project's to fix.

use std::collections::HashSet;

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

/// The comment-run rule over a whole `src`, the entry the unit tests below
/// use; `findings` runs the same rule over tokens it already has.
#[cfg(test)]
pub fn comment_runs(src: &str) -> Result<Vec<CommentRun>, LexError> {
    let tokens = Lexer::new(src).tokenize_with_trivia()?;
    Ok(comment_runs_in(&tokens, src))
}

/// The comment-run rule over an already-lexed file.
fn comment_runs_in(tokens: &[Token], src: &str) -> Vec<CommentRun> {
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
    runs
}

/// One lint finding: where it is, and the warning to print there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub span: Span,
    pub message: String,
}

/// Every finding in `src`, in source order.
///
/// `naming` is `None` for a file the naming rule does not apply to (a fetched
/// dependency under `src/deps/`, see [`Naming`]); every other file gets a
/// setting, which is the project's case or the option's default.
pub fn findings(src: &str, naming: Option<&Naming>) -> Result<Vec<Finding>, LexError> {
    let tokens = Lexer::new(src).tokenize_with_trivia()?;
    let mut all: Vec<Finding> = comment_runs_in(&tokens, src)
        .into_iter()
        .map(|run| Finding {
            span: run.span,
            message: run.message(),
        })
        .collect();
    all.extend(redundant_comparisons_in(&tokens));
    all.extend(redundant_borrow_params_in(&tokens));
    all.extend(unused_bindings_in(&tokens));
    all.extend(crate::doc::orphan_findings(src, &tokens));
    if let Some(naming) = naming {
        all.extend(naming_findings_in(&tokens, naming));
    }
    // A rule reads the tokens in order, but they do not all start at the same
    // place (a `fun` declaration's name is found from the `fun`), so the
    // findings are sorted by where they are before anyone prints them.
    all.sort_by_key(|f| f.span.start);
    Ok(all)
}

/// A `&` on a parameter whose type already borrows its argument. A plain
/// parameter of an owned, non-`Copy` type is a shared borrow, so `s: &T` and
/// `s: T` take the same arguments for `String`, `Bytes`, `List`, `Map`, `Box`,
/// a `datac` and `str` (`kelpc/tests/fixtures/param_borrow_spellings.kp`).
/// The `&` is kept where it means something else: `&dyn`, a scalar or a `Copy`
/// type (passed by value without it), a fixed-size array (by value), a generic
/// `T` (its `Copy`-ness depends on the instantiation), a `Box<dyn ...>`, and
/// `fun X.eq(other: &X)`, which the checker requires. A `datac` counts only when
/// this file declares it, so a type from another file is not reported. Warning
/// only.
fn redundant_borrow_params_in(tokens: &[Token]) -> Vec<Finding> {
    let datacs: std::collections::HashSet<&str> = tokens
        .windows(2)
        .filter_map(|pair| match (&pair[0].kind, &pair[1].kind) {
            (TokenKind::Datac, TokenKind::Ident(name)) => Some(name.as_str()),
            _ => None,
        })
        .collect();
    let mut out = Vec::new();
    for (i, token) in tokens.iter().enumerate() {
        if !matches!(token.kind, TokenKind::Fun) {
            continue;
        }
        // `fun X.eq(other: &X)`: the checker requires the borrow here.
        if matches!(tokens.get(i + 2).map(|t| &t.kind), Some(TokenKind::Dot))
            && matches!(tokens.get(i + 3).map(|t| &t.kind), Some(TokenKind::Ident(name)) if name == "eq")
        {
            continue;
        }
        let mut depth = 0usize;
        let mut k = i + 1;
        while k < tokens.len() {
            match &tokens[k].kind {
                TokenKind::LParen => depth += 1,
                TokenKind::RParen => depth = depth.saturating_sub(1),
                TokenKind::LBrace | TokenKind::Newline | TokenKind::Eof => break,
                TokenKind::Eq if depth == 0 => break,
                _ => {}
            }
            if depth >= 1 {
                if let Some(name) = borrowed_owner_param(tokens, k, &datacs) {
                    let amp = &tokens[k + 2];
                    let ty = &tokens[k + 3];
                    out.push(Finding {
                        span: Span::new(amp.span.start, ty.span.end),
                        message: format!(
                            "redundant '&' on a parameter: a parameter of type '{name}' already borrows its argument, so write it without '&'"
                        ),
                    });
                }
            }
            k += 1;
        }
    }
    out
}

/// The type name of a `name: &Type` parameter that starts at token `k`, when
/// the `&` on it is redundant (see [`redundant_borrow_params_in`]).
fn borrowed_owner_param<'a>(
    tokens: &'a [Token],
    k: usize,
    datacs: &std::collections::HashSet<&str>,
) -> Option<&'a str> {
    if !matches!(tokens.get(k)?.kind, TokenKind::Ident(_)) {
        return None;
    }
    if !matches!(tokens.get(k + 1)?.kind, TokenKind::Colon) || !matches!(tokens.get(k + 2)?.kind, TokenKind::Amp) {
        return None;
    }
    // A name followed by `.` is a path into another module (`str.ParseErr` is
    // an enum from `std/str`), not the builtin or the `datac` of that name.
    if matches!(tokens.get(k + 4).map(|t| &t.kind), Some(TokenKind::Dot)) {
        return None;
    }
    match &tokens.get(k + 3)?.kind {
        TokenKind::Ident(name) if owns_by_name(name, datacs) && !is_box_dyn(tokens, k + 3) => Some(name.as_str()),
        _ => None,
    }
}

/// Whether a type named `name` owns memory or is text that a plain parameter
/// borrows: the builtin owners, `str`, and a `datac` this file declares.
fn owns_by_name(name: &str, datacs: &std::collections::HashSet<&str>) -> bool {
    matches!(name, "String" | "Bytes" | "List" | "Map" | "Box" | "str") || datacs.contains(name)
}

/// Whether the type at `at` is `Box<dyn ...>`, an owned trait object that the
/// `&` cannot be dropped from.
fn is_box_dyn(tokens: &[Token], at: usize) -> bool {
    matches!(&tokens[at].kind, TokenKind::Ident(name) if name == "Box")
        && matches!(tokens.get(at + 1).map(|t| &t.kind), Some(TokenKind::Lt))
        && matches!(tokens.get(at + 2).map(|t| &t.kind), Some(TokenKind::Ident(name)) if name == "dyn")
}

/// The comparisons a plain condition or `.is_empty()` says better. Warnings
/// only: the comparison compiles and means the same thing.
fn redundant_comparisons_in(tokens: &[Token]) -> Vec<Finding> {
    let mut out = Vec::new();
    for (i, token) in tokens.iter().enumerate() {
        if !matches!(token.kind, TokenKind::EqEq) {
            continue;
        }
        // `done == true` is `done`.
        if matches!(tokens.get(i + 1).map(|t| &t.kind), Some(TokenKind::True)) {
            out.push(Finding {
                span: token.span,
                message: "'== true' is redundant: write the condition alone".to_string(),
            });
            continue;
        }
        // `xs.len() == 0` is `xs.is_empty()`. Before the operator come the
        // receiver, `.`, `len`, `(` and `)`, so the span starts at the receiver.
        if i >= 5
            && matches!(tokens[i - 4].kind, TokenKind::Dot)
            && matches!(&tokens[i - 3].kind, TokenKind::Ident(name) if name == "len")
            && matches!(tokens[i - 2].kind, TokenKind::LParen)
            && matches!(tokens[i - 1].kind, TokenKind::RParen)
            && matches!(tokens.get(i + 1).map(|t| &t.kind), Some(TokenKind::IntLit(0)))
        {
            out.push(Finding {
                span: Span::new(tokens[i - 5].span.start, tokens[i + 1].span.end),
                message: "'.len() == 0' reads better as '.is_empty()'".to_string(),
            });
        }
    }
    out
}

/// A name a `when` arm or an `if let` binds, and that its arm (or its block)
/// never reads. The fix is `_`, which the language spells for "I do not read
/// this" (`Err(_) -> 0`), so the finding names it.
///
/// The scope is the arm's body or the `if let`'s then-block, and a name counts
/// as read when it appears there as a plain identifier (a string interpolation
/// `"$e"` is one, see the lexer's template tokens). Two choices keep the rule
/// on the safe side:
///
/// - a name that starts with `_` is never reported, and neither is a name that
///   is not lowercase (`Some` and `Err` are variants, not bindings);
/// - the scope ends where the next arm starts, so a name read by a *later*
///   arm does not count for this one. A body continued on the next line
///   (`Err(e) -> 0 +` then `e`) is one arm, because the next line only ends the
///   arm when it begins an arm head.
///
/// Shadowing can only hide a finding (a nested pattern that reuses the name
/// counts as a read), never invent one. `val`, `var` and `for` bindings are
/// not checked here. Warning only.
fn unused_bindings_in(tokens: &[Token]) -> Vec<Finding> {
    let mut out = Vec::new();
    for open in 0..tokens.len() {
        if !matches!(tokens[open].kind, TokenKind::LParen) {
            continue;
        }
        // `pattern_binds` is the one place that says which parenthesised lists
        // are patterns, so an empty answer means "not a pattern, or no names".
        if pattern_binds(tokens, open).is_empty() {
            continue;
        }
        // A parameter list (`fun f<T>(text: &str) => ...`, a `fun` type) names
        // types and parameters, not bindings, and its `=>` looks like an arm.
        if is_parameter_list(tokens, open) {
            continue;
        }
        // A pattern's head is a variant, and variants are written in PascalCase.
        // A lowercase head is a call, so `send(tx, x) -> 1` (a `select` case)
        // is not a pattern: its names are read, not bound. A tuple arm has no
        // head and is left alone.
        let head_is_variant = open > 0
            && matches!(&tokens[open - 1].kind, TokenKind::Ident(h) if h.starts_with(char::is_uppercase));
        if !head_is_variant {
            continue;
        }
        let Some(close) = matching_paren(tokens, open) else {
            continue;
        };
        let scope = match tokens.get(close + 1).map(|t| &t.kind) {
            Some(TokenKind::Arrow) => {
                let start = close + 2;
                (start, arm_end(tokens, start))
            }
            Some(TokenKind::Eq) => match if_let_block(tokens, close + 2) {
                Some(block) => block,
                None => continue,
            },
            _ => continue,
        };
        for k in open + 1..close {
            let TokenKind::Ident(name) = &tokens[k].kind else {
                continue;
            };
            if !is_unused_candidate(tokens, k, name) || read_in(tokens, scope, name) {
                continue;
            }
            out.push(Finding {
                span: tokens[k].span,
                message: format!(
                    "unused binding '{name}' in a pattern: write '_' to ignore the value"
                ),
            });
        }
    }
    out
}

/// Whether the `(` at `open` opens a `fun` or `closure` parameter list. A
/// header can hold a receiver's `Type.`, type arguments with bounds
/// (`fun map<I: Iterator, U>(`) and nested `fun` types, so the walk back goes
/// over everything up to the start of the statement: a line break, a brace,
/// an `=`, an `=>` or a `)`. Reaching a `fun` or `closure` keyword first means
/// the list is a parameter list. A pattern after a `{`, a newline or `=` is
/// never mistaken for one.
fn is_parameter_list(tokens: &[Token], open: usize) -> bool {
    let mut i = open;
    while i > 0 {
        i -= 1;
        match tokens[i].kind {
            TokenKind::Fun | TokenKind::Closure => return true,
            TokenKind::Newline
            | TokenKind::LBrace
            | TokenKind::RBrace
            | TokenKind::RParen
            | TokenKind::Eq
            | TokenKind::Arrow
            | TokenKind::Eof => return false,
            _ => {}
        }
    }
    false
}

/// Whether the identifier at `k`, a name inside a pattern's parentheses, is a
/// binding the unused rule may report: a lowercase name with no leading
/// underscore, that is not a path part (`.`) and not a call (`(`).
fn is_unused_candidate(tokens: &[Token], k: usize, name: &str) -> bool {
    let starts_lower = name.starts_with(char::is_lowercase);
    let after_dot = k > 0 && matches!(tokens[k - 1].kind, TokenKind::Dot);
    let called = matches!(tokens.get(k + 1).map(|t| &t.kind), Some(TokenKind::LParen | TokenKind::Dot));
    starts_lower && !after_dot && !called
}

/// Whether `name` is read as a plain identifier inside the token range
/// `scope` (`start` inclusive, `end` exclusive). A field path after a `.` is
/// not a read of a local of that name.
fn read_in(tokens: &[Token], (start, end): (usize, usize), name: &str) -> bool {
    (start..end.min(tokens.len())).any(|k| {
        matches!(&tokens[k].kind, TokenKind::Ident(n) if n == name)
            && !(k > 0 && matches!(tokens[k - 1].kind, TokenKind::Dot))
    })
}

/// The index just past the body of the `when` arm whose body starts at
/// `start`. The body ends at a comma or a closing bracket at its own depth, at
/// the next line that begins an arm head, or at the end of the file.
fn arm_end(tokens: &[Token], start: usize) -> usize {
    let mut depth = 0usize;
    let mut k = start;
    while k < tokens.len() {
        match tokens[k].kind {
            TokenKind::LParen | TokenKind::LBrace | TokenKind::LBracket => depth += 1,
            TokenKind::RParen | TokenKind::RBrace | TokenKind::RBracket => {
                if depth == 0 {
                    return k;
                }
                depth -= 1;
            }
            TokenKind::Comma if depth == 0 => return k,
            TokenKind::Newline if depth == 0 && starts_an_arm_head(tokens, k + 1) => return k,
            TokenKind::Eof => return k,
            _ => {}
        }
        k += 1;
    }
    tokens.len()
}

/// Whether the tokens from `at` (after any newlines) open an arm: `else ->`, or
/// a variant or name followed by `->`, or by a payload list and then `->`.
fn starts_an_arm_head(tokens: &[Token], at: usize) -> bool {
    let mut j = at;
    while matches!(tokens.get(j).map(|t| &t.kind), Some(TokenKind::Newline)) {
        j += 1;
    }
    let names_an_arm = matches!(
        tokens.get(j).map(|t| &t.kind),
        Some(TokenKind::Ident(_) | TokenKind::Else)
    );
    if !names_an_arm {
        return false;
    }
    match tokens.get(j + 1).map(|t| &t.kind) {
        Some(TokenKind::Arrow) => true,
        Some(TokenKind::LParen) => matches!(
            matching_paren(tokens, j + 1).and_then(|c| tokens.get(c + 1)).map(|t| &t.kind),
            Some(TokenKind::Arrow)
        ),
        _ => false,
    }
}

/// The then-block of an `if let` whose scrutinee starts at `from`, as the
/// token range from its `{` to just past its matching `}`. A brace inside the
/// scrutinee's parentheses does not start the block.
fn if_let_block(tokens: &[Token], from: usize) -> Option<(usize, usize)> {
    let mut parens = 0usize;
    for k in from..tokens.len() {
        match tokens[k].kind {
            TokenKind::LParen | TokenKind::LBracket => parens += 1,
            TokenKind::RParen | TokenKind::RBracket => parens = parens.saturating_sub(1),
            TokenKind::LBrace if parens == 0 => {
                // `let Some(x) = e else { ... }` is a let-else: its block is the
                // failure branch, which does not see the bindings, and the
                // bindings are read later in the enclosing block. Not checked.
                if k > 0 && matches!(tokens[k - 1].kind, TokenKind::Else) {
                    return None;
                }
                let mut depth = 0usize;
                for (j, token) in tokens.iter().enumerate().skip(k) {
                    match token.kind {
                        TokenKind::LBrace => depth += 1,
                        TokenKind::RBrace => {
                            depth -= 1;
                            if depth == 0 {
                                return Some((k, j + 1));
                            }
                        }
                        _ => {}
                    }
                }
                return None;
            }
            _ => {}
        }
    }
    None
}

/// The naming case a project asks for, as `coral.toml`'s `naming_case`
/// numbers it (`syntax.md` 19, roadmap item 23).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamingCase {
    /// `0`, the default: `readText`.
    Camel,
    /// `1`: `read_text`.
    Snake,
}

impl NamingCase {
    /// The option number, as the manifest writes it.
    pub fn option(self) -> usize {
        match self {
            NamingCase::Camel => 0,
            NamingCase::Snake => 1,
        }
    }

    /// The case's own name, for the warning.
    pub fn name(self) -> &'static str {
        match self {
            NamingCase::Camel => "camelCase",
            NamingCase::Snake => "snake_case",
        }
    }

    /// The case an option number names, or `None` when the number is not one
    /// the option defines. The manifest parser refuses it in that case, so
    /// this only has to be total here.
    pub fn from_option(n: usize) -> Option<Self> {
        match n {
            0 => Some(NamingCase::Camel),
            1 => Some(NamingCase::Snake),
            _ => None,
        }
    }
}

/// Whether `name` is written in camelCase, the one case the rule checks. A
/// leading underscore is not part of either case: `_unused` and `_unused_name`
/// are as ordinary a `camelCase` pair as `unused` and `unusedName`, because the
/// underscore in front marks a name the author deliberately does not read.
fn is_camel_case(name: &str) -> bool {
    let bare = name.trim_start_matches('_');
    !bare.contains('_') && !bare.starts_with(char::is_uppercase)
}

/// The naming rule's setting for one file, and where it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Naming {
    pub case: NamingCase,
    /// `true` when `coral.toml` set `naming_case` itself, `false` when the
    /// option's default applies (no manifest, or a manifest that says
    /// nothing about it). The warning says which, because "the project asks
    /// for" and "the default is" are different answers to "why am I being
    /// told this?"
    pub from_manifest: bool,
}

impl Naming {
    /// What a file with no manifest to read gets.
    pub fn default_without_manifest() -> Self {
        Naming {
            case: NamingCase::Camel,
            from_manifest: false,
        }
    }
}

/// The naming rule over an already-lexed file.
fn naming_findings_in(tokens: &[Token], naming: &Naming) -> Vec<Finding> {
    let mut out = Vec::new();
    // Only the default case is checked (#368 phase 3): a snake_case project
    // gets no naming finding, camelCase names included.
    if naming.case != NamingCase::Camel {
        return out;
    }
    // One name can be recognized by two shapes at once (`datac P(val x: i32)`
    // is both a `val` binding and a field, and they are the same identifier),
    // so a name found twice is reported once.
    let mut seen: HashSet<usize> = HashSet::new();
    for decl in declared_names(tokens) {
        let name = decl.name.as_str();
        if name == "_" || !seen.insert(decl.span.start) || is_camel_case(name) {
            continue;
        }
        let kind = match decl.kind {
            NameKind::Function => "function",
            NameKind::Binding => "binding",
        };
        // What the name *is* written in, which is not always the case the
        // option offers: `Read_Text` is neither, and a name that starts with
        // a capital (`Foo`) is PascalCase, and the warning says so.
        let found = if name.contains('_') {
            if name.chars().any(|c| c.is_uppercase()) {
                "neither camelCase nor snake_case"
            } else {
                "snake_case"
            }
        } else {
            "PascalCase"
        };
        let asks = if naming.from_manifest {
            format!(
                "this project asks for {} (coral.toml 'naming_case = {}')",
                naming.case.name(),
                naming.case.option()
            )
        } else {
            format!(
                "the default is {} ('naming_case = {}', no manifest sets it)",
                naming.case.name(),
                naming.case.option()
            )
        };
        let rewrite = to_camel(name);
        out.push(Finding {
            span: decl.span,
            message: format!("{kind} '{name}' is {found}, and {asks}; write '{rewrite}'"),
        });
    }
    out
}

/// Which declaration a name is, for the warning's wording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NameKind {
    Function,
    Binding,
}

/// One declared name, with the span of the name itself.
#[derive(Debug, Clone)]
struct Declaration {
    name: String,
    span: Span,
    kind: NameKind,
}

/// `read_text` -> `readText`, for the warning's suggestion.
///
/// Leading underscores are kept (they mark a name the author does not read),
/// and the ones between words are dropped. `read_HTTP` is `readHttp`: the
/// suggestion is a starting point the author edits, not a rename `lint`
/// performs, so lowercasing the tail of a word is the honest default rather
/// than a guess about acronyms.
fn to_camel(name: &str) -> String {
    let rest = name.trim_start_matches('_');
    let mut out = name[..name.len() - rest.len()].to_string();
    for (i, word) in rest.split('_').filter(|w| !w.is_empty()).enumerate() {
        if i == 0 {
            out.push_str(&word.to_lowercase());
            continue;
        }
        let word = word.to_lowercase();
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    out
}

/// The identifier at `i`, if there is one.
fn ident_at(tokens: &[Token], i: usize) -> Option<(String, Span)> {
    match &tokens.get(i)?.kind {
        TokenKind::Ident(name) => Some((name.clone(), tokens[i].span)),
        _ => None,
    }
}

/// Every name `src` declares, in source order, with the span of the name
/// itself and which kind of declaration it is.
///
/// This reads the token stream rather than the parsed program, and that is the
/// point: `kelpc lint` lexes a file and reads nothing else, so a file that does
/// not parse, or that names an alias only its project can resolve, still gets
/// its naming findings. The shapes a declaration can take are each local to
/// their own tokens:
///
/// - a `val`, `var` or `for` is a keyword, so the name is the identifier after
///   one. That covers a local binding, a passdown, a `datac` field, an enum
///   payload field and a loop binding, and it is why none of them needs a
///   second case below. A `val`/`var` whose next token is `(` is a
///   destructuring binding (a tuple taken apart), and every identifier in that
///   parenthesized pattern is a binding it declares; the synthesized local the
///   lowering binds the whole tuple to is not a name the file wrote.
/// - a `fun` declaration's name is the identifier after `fun` (past `mut`),
///   unless a receiver's `Type.` follows it, in which case the name is the
///   identifier after that `.`.
/// - a parameter or a field is an identifier that a `(` or a `,` introduces
///   and a `:` follows: `fun f(a: i32)`, `datac P(val x: i32)`,
///   `fun(mut Counter)`. A function *type*'s `=> i64` cannot be mistaken for
///   one, because its `R` follows a `.`.
/// - a pattern binding is an identifier in a variant's payload list: the list
///   after the variant name of an `if let`, or the list an arm head's arrow
///   closes. Nothing else puts identifiers in those two places.
///
/// Nothing else in the language puts an identifier in any of those positions.
/// A call's named argument is `name = value`; a `const` name is introduced by
/// `const` and is written `UPPER_SNAKE` by convention, so it is not this
/// rule's business; a type name is `PascalCase` whatever the option says; and
/// an `impl` path is a string, not an identifier.
fn declared_names(tokens: &[Token]) -> Vec<Declaration> {
    let mut out = Vec::new();
    for (i, token) in tokens.iter().enumerate() {
        match &token.kind {
            TokenKind::Val | TokenKind::Var => {
                if let Some((name, span)) = ident_at(tokens, i + 1) {
                    out.push(Declaration {
                        name,
                        span,
                        kind: NameKind::Binding,
                    });
                } else if matches!(tokens.get(i + 1).map(|t| &t.kind), Some(TokenKind::LParen)) {
                    for (name, span) in tuple_pattern_binds(tokens, i + 1) {
                        out.push(Declaration {
                            name,
                            span,
                            kind: NameKind::Binding,
                        });
                    }
                }
            }
            // `for x in xs { ... }` binds `x`, and `impl Name for Type { ... }`
            // (A-12 slice 2) writes the same keyword and binds nothing: the
            // contextual `in` after the name is what tells the two apart.
            TokenKind::For => {
                let is_loop = matches!(
                    tokens.get(i + 2).map(|t| &t.kind),
                    Some(TokenKind::Ident(word)) if word == "in"
                );
                if is_loop {
                    if let Some((name, span)) = ident_at(tokens, i + 1) {
                        out.push(Declaration {
                            name,
                            span,
                            kind: NameKind::Binding,
                        });
                    }
                }
                // `for (lo, hi) in pairs { ... }` takes a tuple apart, and every
                // name in the pattern is a binding, as in `val (lo, hi) = ...`.
                let is_tuple_loop = matches!(tokens.get(i + 1).map(|t| &t.kind), Some(TokenKind::LParen))
                    && matching_paren(tokens, i + 1).is_some_and(|close| {
                        matches!(
                            tokens.get(close + 1).map(|t| &t.kind),
                            Some(TokenKind::Ident(word)) if word == "in"
                        )
                    });
                if is_tuple_loop {
                    for (name, span) in tuple_pattern_binds(tokens, i + 1) {
                        out.push(Declaration {
                            name,
                            span,
                            kind: NameKind::Binding,
                        });
                    }
                }
            }
            TokenKind::Fun => {
                if let Some((name, span)) = fun_name(tokens, i) {
                    out.push(Declaration {
                        name,
                        span,
                        kind: NameKind::Function,
                    });
                }
            }
            TokenKind::Ident(name) => {
                // `block name: T { ... }` (manual memory block) declares `name`:
                // `name.box()` reads it. The contextual `block` is followed by
                // an identifier and a `:` only in that form.
                if name == "block" {
                    if let Some((block_name, span)) = ident_at(tokens, i + 1) {
                        if matches!(tokens.get(i + 2).map(|t| &t.kind), Some(TokenKind::Colon)) {
                            out.push(Declaration {
                                name: block_name,
                                span,
                                kind: NameKind::Binding,
                            });
                        }
                    }
                }
                let introduced = i > 0
                    && matches!(tokens[i - 1].kind, TokenKind::LParen | TokenKind::Comma);
                let typed = matches!(
                    tokens.get(i + 1).map(|t| &t.kind),
                    Some(TokenKind::Colon)
                );
                if introduced && typed {
                    out.push(Declaration {
                        name: name.clone(),
                        span: token.span,
                        kind: NameKind::Binding,
                    });
                }
            }
            TokenKind::LParen => {
                // The payload list of a pattern is `(a, b)` and nothing else,
                // so every identifier in it is a binding.
                for (name, span) in pattern_binds(tokens, i) {
                    out.push(Declaration {
                        name,
                        span,
                        kind: NameKind::Binding,
                    });
                }
            }
            _ => {}
        }
    }
    out
}

/// A `fun` declaration's name: the identifier after `fun` (past `mut`), or,
/// when a receiver's `Type.` follows it, the identifier after that `.`.
///
/// The scan stops at the parameter list's `(`, so a dotted type *inside* the
/// parameters (`fun f(p: Point.x)`) cannot be read as a receiver.
fn fun_name(tokens: &[Token], fun: usize) -> Option<(String, Span)> {
    let mut i = fun + 1;
    if matches!(tokens.get(i)?.kind, TokenKind::Mut) {
        i += 1;
    }
    // No identifier at all is a `fun` literal (`val f = fun(x: i32) = x`),
    // which declares no name.
    let mut name = ident_at(tokens, i)?;
    let mut j = i + 1;
    while let Some(token) = tokens.get(j) {
        match token.kind {
            TokenKind::Dot => {
                if let Some(found) = ident_at(tokens, j + 1) {
                    name = found;
                }
            }
            TokenKind::Ident(_) | TokenKind::Lt | TokenKind::Gt | TokenKind::Comma => {}
            _ => break,
        }
        j += 1;
    }
    Some(name)
}

/// The bindings of the pattern a `(` opens, or nothing when that `(` is not a
/// pattern's payload list.
///
/// Two shapes are a pattern: `if let Variant(a, b) = e`, where the identifier
/// before the variant is the contextual `let`, and `when e { Variant(a, b) ->
/// .. }`, where the `->` (or `=>`) of the arm head follows the list. The
/// arrow appears nowhere else in the language, and `let` directly before a
/// variant name is the `if let` grammar, so neither test can be met by a call.
fn pattern_binds(tokens: &[Token], open: usize) -> Vec<(String, Span)> {
    let Some(close) = matching_paren(tokens, open) else {
        return Vec::new();
    };
    let is_if_let = open >= 2
        && matches!(tokens[open - 1].kind, TokenKind::Ident(_))
        && matches!(&tokens[open - 2].kind, TokenKind::Ident(word) if word == "let");
    // A parameter list followed by `=>` is a return type, not a when arm, when
    // the list belongs to a `fun` header or a `fun`/`closure` type.
    let is_fun_header = {
        let mut i = open;
        let mut found = false;
        while i > 0 {
            i -= 1;
            match tokens[i].kind {
                TokenKind::Ident(_) | TokenKind::Dot | TokenKind::Mut => continue,
                TokenKind::Fun | TokenKind::Closure => found = true,
                _ => {}
            }
            break;
        }
        found
    };
    let is_arm_head = !is_fun_header
        && matches!(
            tokens.get(close + 1).map(|t| &t.kind),
            Some(TokenKind::Arrow)
        );
    if !is_if_let && !is_arm_head {
        return Vec::new();
    }
    let mut out = Vec::new();
    for token in &tokens[open + 1..close] {
        if let TokenKind::Ident(name) = &token.kind {
            out.push((name.clone(), token.span));
        }
    }
    out
}

/// The bindings of the tuple pattern the `(` at `open` opens: `val (lo, hi) =
/// ...`, including a nested position (`val ((a, b), c) = ...`). Every
/// identifier between the parentheses is a position the binding names, because
/// a pattern has only names, `_` and further parentheses in it; `_` ignores a
/// position and declares nothing.
fn tuple_pattern_binds(tokens: &[Token], open: usize) -> Vec<(String, Span)> {
    let Some(close) = matching_paren(tokens, open) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for token in &tokens[open + 1..close] {
        if let TokenKind::Ident(name) = &token.kind {
            if name != "_" {
                out.push((name.clone(), token.span));
            }
        }
    }
    out
}

/// The index of the `)` that closes the `(` at `open`.
fn matching_paren(tokens: &[Token], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (i, token) in tokens.iter().enumerate().skip(open) {
        match token.kind {
            TokenKind::LParen => depth += 1,
            TokenKind::RParen => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
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
#[path = "lint/tests.rs"]
mod tests;
