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
//! There are three rules:
//!
//! - the **comment run** (roadmap item G-5): two or more consecutive `//`
//!   lines with no blank line between them.
//! - the **`else if` link** (`syntax.md` 6.1, roadmap item G-3): `elif` is the
//!   preferred spelling, and the longer one is what this warns about.
//! - the **naming case** (roadmap item 23, `syntax.md` 19): the project's case,
//!   `camelCase` (the default) or `snake_case`, read from `coral.toml`'s
//!   `naming_case`.
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
//! ## The `else if` link
//!
//! `syntax.md` 6.1 decides that `elif` and `else if` are the same link, that
//! `elif` is the preferred spelling, and that the longer one gets a cosmetic
//! warning. Cosmetic means cosmetic: `else if` stays legal, compiles, and is
//! never rewritten by `kelpc fmt`, which is token-faithful.
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
    all.extend(else_if_links_in(&tokens));
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

/// The `else if` rule over an already-lexed file.
fn else_if_links_in(tokens: &[Token]) -> Vec<Finding> {
    let mut out = Vec::new();
    for pair in tokens.windows(2) {
        if matches!(pair[0].kind, TokenKind::Else) && matches!(pair[1].kind, TokenKind::If) {
            out.push(Finding {
                span: pair[0].span,
                message: "'else if' compiles, but 'elif' is the preferred spelling".to_string(),
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
///   `fun(mut Counter)`. A function *type*'s `.return: i64` cannot be mistaken for
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
    let is_arm_head = matches!(
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

    /// The findings of a file linted under a case, as `(offset, message)`.
    fn case_findings(src: &str, case: NamingCase) -> Vec<(usize, String)> {
        findings(
            src,
            Some(&Naming {
                case,
                from_manifest: true,
            }),
        )
        .expect("the fixture lexes")
        .into_iter()
        .map(|f| (f.span.start, f.message))
        .collect()
    }

    /// Just the names the naming rule reports, in source order.
    fn named(src: &str, case: NamingCase) -> Vec<String> {
        case_findings(src, case)
            .into_iter()
            .map(|(_, message)| {
                let rest = message.split('\'').nth(1).unwrap_or_default();
                rest.to_string()
            })
            .collect()
    }

    #[test]
    fn an_else_if_link_is_reported_at_the_else() {
        let src = "fun f(n: i32) {\n    if n > 1 { } else if n > 0 { }\n}\n";
        let found = findings(src, None).expect("lexes");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].span.start, at(src, "else if"));
        assert_eq!(
            found[0].message,
            "'else if' compiles, but 'elif' is the preferred spelling"
        );
    }

    #[test]
    fn an_else_if_let_link_is_reported_too() {
        let src = "fun f(m: Answer) {\n    if let Yes(n) = m { } else if let No = m { }\n}\n";
        let found = findings(src, None).expect("lexes");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].span.start, at(src, "else if let"));
    }

    #[test]
    fn elif_is_not_reported() {
        let src = "fun f(n: i32) {\n    if n > 1 { } elif n > 0 { } else { }\n}\n";
        assert!(findings(src, None).expect("lexes").is_empty());
    }

    #[test]
    fn an_else_and_an_if_on_separate_lines_are_not_a_link() {
        // The parser reads the two tokens as a link only when they are
        // adjacent, so a separated pair is not the spelling this warns about.
        let src = "fun f(n: i32) {\n    if n > 1 { } else\n        if n > 0 { }\n}\n";
        assert!(findings(src, None).expect("lexes").is_empty());
    }

    #[test]
    fn the_two_spellings_are_not_confused_inside_a_string() {
        let src = "fun f() {\n    print(\"if a { } else if b { }\")\n}\n";
        assert!(findings(src, None).expect("lexes").is_empty());
    }

    /// The names as owned strings, so an expected list reads as names.
    fn strings(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn camel_case_reports_every_kind_of_declaration() {
        let src = "datac Point(val x_axis: i32)\n\nfun read_text(first_arg: i32).return: i32 {\n    val line_count = 1\n    val f = fun(inner_arg: i32) = inner_arg\n    for each_item in 0 {\n        print(each_item)\n    }\n    return f(first_arg) + line_count + x_axis\n}\n";
        assert_eq!(
            named(src, NamingCase::Camel),
            strings(&[
                "x_axis",
                "read_text",
                "first_arg",
                "line_count",
                "inner_arg",
                "each_item",
            ])
        );
    }

    #[test]
    fn a_field_named_by_two_shapes_is_reported_once() {
        // `datac P(val x_axis: i32)` is both a `val` binding and a field, and
        // they are the same identifier: one finding, not two.
        let src = "datac Point(val x_axis: i32, val y_axis: i32)\n";
        let found = case_findings(src, NamingCase::Camel);
        assert_eq!(found.len(), 2, "one finding per field, got {found:?}");
    }

    #[test]
    fn a_destructuring_binding_declares_every_position_it_names() {
        // `val (lo_index, _) = ...` declares `lo_index` and ignores the other
        // position; a nested pattern declares its own names too, so the rule
        // reaches every name a tuple taken apart binds.
        let src = "fun f(p: (i32, i32)).return: i32 {\n    val (lo_index, _) = p\n    val ((first_val, second_val), last_val) = ((1, 2), 3)\n    return lo_index + first_val + second_val + last_val\n}\n";
        assert_eq!(
            named(src, NamingCase::Camel),
            strings(&["lo_index", "first_val", "second_val", "last_val"])
        );
    }

    #[test]
    fn a_tuple_type_is_not_a_declaration() {
        // `(i32, i32)` puts identifiers where a parameter list would, and a
        // type name is `PascalCase` whatever the option says, so no position
        // of a tuple type is this rule's business.
        let src = "fun f(lo_axis: (i32, DeviceInfo)).return: (i32, i32) = (1, 2)\n";
        assert_eq!(named(src, NamingCase::Camel), strings(&["lo_axis"]));
    }

    #[test]
    fn a_pattern_binding_is_checked_in_both_of_its_homes() {
        let src = "enum Shape { Circle(val r: f64) Empty }\n\nfun f(s: Shape).return: f64 {\n    if let Circle(one_r) = s {\n        return one_r\n    }\n    when s {\n        Circle(two_r) -> return two_r\n        Empty -> return 0.0\n    }\n}\n";
        assert_eq!(
            named(src, NamingCase::Camel),
            strings(&["one_r", "two_r"])
        );
    }

    #[test]
    fn a_call_is_not_a_declaration() {
        // The stdlib's locked `snake_case` is the reason this matters: a
        // project that asks for `camelCase` calls `str.is_empty` and is not
        // warned about a name it did not write and cannot change.
        let src = "fun f(s: &str).return: bool {\n    val empty = str.is_empty(s)\n    return empty\n}\n";
        assert!(case_findings(src, NamingCase::Camel).is_empty());
    }

    #[test]
    fn a_function_type_return_clause_is_not_a_binding() {
        // `fun(i32).return: i32` puts an identifier right before a `:`. It is the
        // return clause of a function *type*, not a parameter, and the `.` in
        // front of the `R` is what says so.
        let src = "fun apply(f: fun(i32).return: i32).return: i32 = f(1)\n";
        assert!(case_findings(src, NamingCase::Camel).is_empty());
    }

    #[test]
    fn a_const_name_is_not_this_rules_business() {
        // `const LIMIT` is `UPPER_SNAKE` by convention (`syntax.md` 19), which
        // is neither of the option's two cases.
        let src = "const LIMIT: i32 = 10\n\nfun f().return: i32 = LIMIT\n";
        assert!(case_findings(src, NamingCase::Camel).is_empty());
        assert!(case_findings(src, NamingCase::Snake).is_empty());
    }

    #[test]
    fn a_receivers_declaration_names_the_name_after_the_dot() {
        // The receiver's type name is `PascalCase` whatever the option says,
        // and the name after the `.` is a function name like any other; the
        // parameter is a binding.
        let src = "fun Circle.surface_area(scale_factor: f64).return: f64 = scale_factor\n";
        assert_eq!(
            named(src, NamingCase::Camel),
            strings(&["surface_area", "scale_factor"])
        );
        assert!(case_findings(src, NamingCase::Snake).is_empty());
    }

    #[test]
    fn a_function_literal_names_the_parameters_it_declares() {
        // A `fun` literal declares no name of its own, and its parameters are
        // bindings like any other.
        let src = "fun f().return: i32 {\n    val g = fun(an_arg: i32) = an_arg\n    return g(1)\n}\n";
        assert_eq!(named(src, NamingCase::Camel), strings(&["an_arg"]));
    }

    #[test]
    fn a_leading_underscore_is_not_part_of_either_case() {
        let src = "fun f().return: i32 {\n    val _unused = 1\n    return _unused\n}\n";
        assert!(case_findings(src, NamingCase::Camel).is_empty());
        assert!(case_findings(src, NamingCase::Snake).is_empty());
        // The underscore is kept in the suggestion, and the words after it
        // are what changes.
        let src = "fun f().return: i32 {\n    val _unused_name = 1\n    return _unused_name\n}\n";
        assert!(case_findings(src, NamingCase::Camel)[0]
            .1
            .contains("write '_unusedName'"));
    }

    #[test]
    fn snake_case_asks_for_the_mirror_image() {
        let src = "fun readText().return: i32 {\n    val lineCount = 1\n    return lineCount\n}\n";
        assert_eq!(
            named(src, NamingCase::Snake),
            strings(&["readText", "lineCount"])
        );
        assert!(case_findings(src, NamingCase::Snake)[0]
            .1
            .contains("write 'read_text'"));
    }

    #[test]
    fn a_name_in_the_projects_case_is_left_alone() {
        let camel = "fun readText(lineCount: i32).return: i32 = lineCount\n";
        assert!(case_findings(camel, NamingCase::Camel).is_empty());
        let snake = "fun read_text(line_count: i32).return: i32 = line_count\n";
        assert!(case_findings(snake, NamingCase::Snake).is_empty());
    }

    #[test]
    fn a_warning_that_came_from_a_manifest_says_so() {
        let src = "fun read_text().return: i32 = 1\n";
        let from_manifest = case_findings(src, NamingCase::Camel)[0].1.clone();
        assert!(from_manifest.contains("coral.toml 'naming_case = 0'"), "{from_manifest}");
        let default = findings(src, Some(&Naming::default_without_manifest()))
            .expect("lexes")[0]
            .message
            .clone();
        assert!(default.contains("no manifest sets it"), "{default}");
    }

    #[test]
    fn the_suggestions_are_the_case_rewrites() {
        assert_eq!(to_camel("read_text"), "readText");
        assert_eq!(to_camel("read__text"), "readText");
        assert_eq!(to_camel("_read_text"), "_readText");
        assert_eq!(to_snake("readText"), "read_text");
        assert_eq!(to_snake("read_Text"), "read_text");
        assert_eq!(to_snake("readHTTPServer"), "read_http_server");
    }

    #[test]
    fn a_file_with_no_naming_setting_gets_no_naming_findings() {
        // What `src/deps/` gets: the comment rule still applies, the case rule
        // does not.
        let src = "// one\n// two\n\nfun read_text().return: i32 = 1\n";
        let found = findings(src, None).expect("lexes");
        assert_eq!(found.len(), 1);
        assert!(found[0].message.contains("consecutive '//' lines"));
    }

    #[test]
    fn an_impl_lines_for_is_not_a_loop_binding() {
        // `impl Name for Type { ... }` (A-12 slice 2) writes the same keyword a
        // `for` loop does, and only the loop binds a name: the contextual `in`
        // is what tells them apart, so a PascalCase `for` type is not read as a
        // mis-cased loop variable.
        let src = "trait Shape {\n    fun Self.area().return: i32\n}\n\ndatac Circle(val r: i32)\n\nimpl Shape for Circle {\n    fun Self.area().return: i32 = r\n}\n";
        let found = findings(
            src,
            Some(&Naming {
                case: NamingCase::Camel,
                from_manifest: false,
            }),
        )
        .expect("lexes");
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn the_findings_of_one_file_come_out_in_source_order() {
        let src = "fun read_text().return: i32 {\n    // one\n    // two\n    if read_text() > 1 {\n        return 1\n    } else if read_text() > 0 {\n        return 2\n    }\n    return 3\n}\n";
        let found = findings(
            src,
            Some(&Naming {
                case: NamingCase::Camel,
                from_manifest: false,
            }),
        )
        .expect("lexes");
        assert_eq!(found.len(), 3, "{found:?}");
        let mut starts: Vec<usize> = found.iter().map(|f| f.span.start).collect();
        let sorted = starts.clone();
        starts.sort_unstable();
        assert_eq!(starts, sorted);
        assert_eq!(found[0].span.start, at(src, "read_text"));
    }
}
