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
//! There are four rules:
//!
//! - the **comment run** (roadmap item G-5): two or more consecutive `//`
//!   lines with no blank line between them.
//! - the **naming case** (roadmap item 23, `syntax.md` 19): the project's case,
//!   `camelCase` (the default) or `snake_case`, read from `coral.toml`'s
//!   `naming_case`.
//! - the **redundant borrow** on a `List` or `str` parameter (`&List<T>` and
//!   `&str` take the same arguments as `List<T>` and `str`): a warning.
//! - the **redundant comparison** (`== true`, and `.len() == 0` where
//!   `.is_empty()` says it): a warning, never a failure.
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

/// A `&` on a `List` or `str` parameter of a `fun` header. Such a parameter
/// already borrows its argument, and `List<T>` and `str` take the same
/// arguments as `&List<T>` and `&str` (`kelpc/tests/fixtures/param_borrow_spellings.kp`).
/// Warning only.
fn redundant_borrow_params_in(tokens: &[Token]) -> Vec<Finding> {
    let mut out = Vec::new();
    for (i, token) in tokens.iter().enumerate() {
        if !matches!(token.kind, TokenKind::Fun) {
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
            if depth >= 1
                && matches!(tokens[k].kind, TokenKind::Ident(_))
                && matches!(tokens.get(k + 1).map(|t| &t.kind), Some(TokenKind::Colon))
                && matches!(tokens.get(k + 2).map(|t| &t.kind), Some(TokenKind::Amp))
                && matches!(tokens.get(k + 3).map(|t| &t.kind), Some(TokenKind::Ident(name)) if name == "List" || name == "str")
            {
                let amp = &tokens[k + 2];
                let ty = &tokens[k + 3];
                out.push(Finding {
                    span: Span::new(amp.span.start, ty.span.end),
                    message: "redundant '&' on a parameter: a 'List' or 'str' parameter already borrows its argument, so write it without '&'".to_string(),
                });
            }
            k += 1;
        }
    }
    out
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

    /// Whether `name` is written in this case. A leading underscore is not
    /// part of either case: `_unused` and `_unused_name` are as ordinary a
    /// `camelCase` pair as `unused` and `unusedName`, because the underscore
    /// in front marks a name the author deliberately does not read.
    fn keeps(self, name: &str) -> bool {
        let bare = name.trim_start_matches('_');
        match self {
            NamingCase::Camel => {
                !bare.contains('_') && !bare.starts_with(char::is_uppercase)
            }
            NamingCase::Snake => !bare.chars().any(|c| c.is_uppercase()),
        }
    }
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
    // One name can be recognized by two shapes at once (`datac P(val x: i32)`
    // is both a `val` binding and a field, and they are the same identifier),
    // so a name found twice is reported once.
    let mut seen: HashSet<usize> = HashSet::new();
    for decl in declared_names(tokens) {
        let name = decl.name.as_str();
        if name == "_" || !seen.insert(decl.span.start) || naming.case.keeps(name) {
            continue;
        }
        let kind = match decl.kind {
            NameKind::Function => "function",
            NameKind::Binding => "binding",
        };
        // What the name *is* written in, which is not always the case the
        // option offers: `Read_Text` is neither, and the warning says so
        // rather than calling it one of the two.
        let found = if name.chars().any(|c| c.is_uppercase()) {
            if name.contains('_') {
                "neither camelCase nor snake_case"
            } else {
                "camelCase"
            }
        } else if name.contains('_') {
            "snake_case"
        } else {
            "camelCase"
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
        let rewrite = match naming.case {
            NamingCase::Camel => to_camel(name),
            NamingCase::Snake => to_snake(name),
        };
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

/// `readText` -> `read_text`, for the warning's suggestion.
///
/// A capital starts a new word when the character before it is not a capital
/// (`readText`) or when it is the last capital of a run (`readHTTPServer`),
/// which is the usual rule and the one that keeps an acronym in one piece.
fn to_snake(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let mut out = String::new();
    for (i, c) in chars.iter().enumerate() {
        if !c.is_uppercase() {
            out.push(*c);
            continue;
        }
        let prev = i.checked_sub(1).map(|j| chars[j]);
        let next = chars.get(i + 1).copied();
        let starts_a_word = matches!(prev, Some(p) if p.is_lowercase() || p.is_ascii_digit())
            || (matches!(prev, Some(p) if p.is_uppercase())
                && matches!(next, Some(n) if n.is_lowercase()));
        if starts_a_word && !out.ends_with('_') {
            out.push('_');
        }
        out.extend(c.to_lowercase());
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
