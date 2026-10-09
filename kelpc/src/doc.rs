//! `kelpc doc`: the documentation generator for Kelp programs and the std
//! library (`plan/doc-tool.md`).
//!
//! The syntax is two comment forms the lexer already keeps as trivia, so the
//! parser and the AST are untouched:
//!
//! - a run of `///` lines directly above a `fun`, `datac`, `enum`, `trait`,
//!   `pd`, `const`, `static`, `extern type`, an enum variant, a `datac` field
//!   or a trait method documents that declaration;
//! - a run of `//!` lines before the first declaration of a file (only `impl
//!   "..."` imports may come first) documents the module.
//!
//! Everything else stays an ordinary comment. A `///` or `//!` that is attached
//! to nothing is a lint warning (`orphan_findings`), never an error.
//!
//! The scanner works on the token stream, not on the AST: the AST has no
//! comments, and a file with project aliases cannot be parsed alone with spans
//! that still point into its text. It walks the top level, reads the header of
//! each declaration, and steps over bodies by balancing braces, which is all a
//! documentation page needs. Signatures are taken from the formatter's output
//! (`fmt::format_after_resolve`) so a page shows a declaration the way
//! `kelpc fmt` writes it, then collapsed to one line.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::lexer::{LexError, Lexer, Token, TokenKind};
use crate::lint::Finding;

/// What a documented top-level declaration is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Fun,
    Datac,
    Enum,
    Trait,
    ExternType,
    Const,
    Static,
    Pd,
}

impl Kind {
    pub fn word(self) -> &'static str {
        match self {
            Kind::Fun => "fun",
            Kind::Datac => "datac",
            Kind::Enum => "enum",
            Kind::Trait => "trait",
            Kind::ExternType => "extern type",
            Kind::Const => "const",
            Kind::Static => "static",
            Kind::Pd => "pd",
        }
    }

    fn is_type(self) -> bool {
        matches!(self, Kind::Datac | Kind::Enum | Kind::Trait | Kind::ExternType)
    }
}

/// What a member of a declaration is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MemberKind {
    Field,
    Variant,
    Method,
    AssocType,
}

impl MemberKind {
    fn word(self) -> &'static str {
        match self {
            MemberKind::Field => "field",
            MemberKind::Variant => "variant",
            MemberKind::Method => "method",
            MemberKind::AssocType => "associated type",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Member {
    pub kind: MemberKind,
    pub name: String,
    pub sig: String,
    pub doc: Option<String>,
    /// 1-based line of the member in the file as written.
    pub line: usize,
}

#[derive(Clone, Debug)]
pub struct Item {
    pub kind: Kind,
    pub name: String,
    pub sig: String,
    pub doc: Option<String>,
    pub is_pub: bool,
    pub unsafe_: bool,
    pub extern_: bool,
    /// `Some(type)` for a receiver method, `fun Circle.area()`.
    pub receiver: Option<String>,
    pub members: Vec<Member>,
    /// The capability domains the body names through an imported gated module.
    pub body_domains: BTreeSet<String>,
    pub line: usize,
}

#[derive(Clone, Debug)]
pub struct Import {
    /// The line as written, `impl "std/fs" caps(fs = ["/tmp/**"])`.
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct TraitImpl {
    /// `Shape`, `From<IoErr>`.
    pub trait_name: String,
    /// The implementing type, as written.
    pub ty: String,
    pub members: Vec<Member>,
}

#[derive(Debug, Default)]
pub struct Scan {
    pub module_doc: Option<String>,
    pub imports: Vec<Import>,
    pub items: Vec<Item>,
    pub impls: Vec<TraitImpl>,
    /// Doc comments attached to nothing, as lint findings.
    pub orphans: Vec<Finding>,
}

/// The module's own capability domain, when it is a gated one. Mirrors what
/// `stdlib::domain_of` gates, plus the two wrappers that reach a domain through
/// what they import and are granted on their own `impl` line.
pub fn domain_of_module(id: &str) -> Option<&'static str> {
    if let Some(d) = crate::stdlib::domain_of(id) {
        return Some(d);
    }
    match id {
        "std/http" => Some("net"),
        "std/tlsfile" => Some("fs"),
        _ => None,
    }
}

/// Lexes and scans `src`. `Err` is a lex error, reported like any other.
pub fn scan(src: &str) -> Result<Scan, LexError> {
    let trivia = Lexer::new(src).tokenize_with_trivia()?;
    let mut scan = Scanner::new(src, &trivia).run();
    // Signatures come from the formatted text when it has the same shape.
    if let Ok(formatted) = crate::fmt::format_after_resolve(src) {
        if formatted != src {
            if let Ok(tokens) = Lexer::new(&formatted).tokenize_with_trivia() {
                let other = Scanner::new(&formatted, &tokens).run();
                adopt_signatures(&mut scan, &other);
            }
        }
    }
    Ok(scan)
}

/// The orphan doc comments of an already-lexed file, for `kelpc lint`.
pub fn orphan_findings(src: &str, trivia: &[Token]) -> Vec<Finding> {
    Scanner::new(src, trivia).run().orphans
}

fn adopt_signatures(scan: &mut Scan, other: &Scan) {
    let same_shape = scan.items.len() == other.items.len()
        && scan.impls.len() == other.impls.len()
        && scan
            .items
            .iter()
            .zip(&other.items)
            .all(|(a, b)| a.name == b.name && a.kind == b.kind && a.members.len() == b.members.len())
        && scan
            .impls
            .iter()
            .zip(&other.impls)
            .all(|(a, b)| a.members.len() == b.members.len());
    if !same_shape {
        return;
    }
    for (a, b) in scan.items.iter_mut().zip(&other.items) {
        a.sig = b.sig.clone();
        for (ma, mb) in a.members.iter_mut().zip(&b.members) {
            ma.sig = mb.sig.clone();
        }
    }
    for (a, b) in scan.impls.iter_mut().zip(&other.impls) {
        for (ma, mb) in a.members.iter_mut().zip(&b.members) {
            ma.sig = mb.sig.clone();
        }
    }
}

struct DocLine {
    start: usize,
    text: String,
}

struct Scanner<'a> {
    src: &'a str,
    /// The tokens with comments removed.
    toks: Vec<Token>,
    comments: Vec<(usize, usize, &'a str)>,
    line_starts: Vec<usize>,
    /// Whole-line `///` comments by 0-based line.
    docs: HashMap<usize, DocLine>,
    /// Whole-line `//!` comments by 0-based line.
    inner: BTreeMap<usize, DocLine>,
    consumed: HashSet<usize>,
    eof: Token,
}

fn strip_doc(text: &str, marker: &str) -> String {
    let rest = &text[marker.len()..];
    rest.strip_prefix(' ').unwrap_or(rest).trim_end().to_string()
}

impl<'a> Scanner<'a> {
    fn new(src: &'a str, trivia: &[Token]) -> Self {
        let mut line_starts = vec![0];
        for (i, b) in src.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        let mut s = Scanner {
            src,
            toks: Vec::new(),
            comments: Vec::new(),
            line_starts,
            docs: HashMap::new(),
            inner: BTreeMap::new(),
            consumed: HashSet::new(),
            eof: Token {
                kind: TokenKind::Eof,
                span: crate::lexer::Span::new(src.len(), src.len()),
            },
        };
        for t in trivia {
            match &t.kind {
                TokenKind::Comment(_) => {
                    let text = &src[t.span.start..t.span.end];
                    s.comments.push((t.span.start, t.span.end, text));
                    let line = s.line_of(t.span.start);
                    let whole = src[s.line_starts[line]..t.span.start].trim().is_empty();
                    if whole && text.starts_with("///") && !text.starts_with("////") {
                        s.docs.insert(
                            line,
                            DocLine {
                                start: t.span.start,
                                text: strip_doc(text, "///"),
                            },
                        );
                    } else if whole && text.starts_with("//!") {
                        s.inner.insert(
                            line,
                            DocLine {
                                start: t.span.start,
                                text: strip_doc(text, "//!"),
                            },
                        );
                    }
                }
                _ => s.toks.push(t.clone()),
            }
        }
        if !matches!(s.toks.last().map(|t| &t.kind), Some(TokenKind::Eof)) {
            s.toks.push(s.eof.clone());
        }
        s
    }

    fn line_of(&self, offset: usize) -> usize {
        self.line_starts.partition_point(|&s| s <= offset) - 1
    }

    fn kind(&self, i: usize) -> &TokenKind {
        self.toks.get(i).map(|t| &t.kind).unwrap_or(&self.eof.kind)
    }

    fn start(&self, i: usize) -> usize {
        self.toks.get(i).map(|t| t.span.start).unwrap_or(self.src.len())
    }

    fn end(&self, i: usize) -> usize {
        self.toks.get(i).map(|t| t.span.end).unwrap_or(self.src.len())
    }

    fn is_ident(&self, i: usize, word: &str) -> bool {
        matches!(self.kind(i), TokenKind::Ident(n) if n == word)
    }

    fn ident_at(&self, i: usize) -> Option<String> {
        match self.kind(i) {
            TokenKind::Ident(n) => Some(n.clone()),
            _ => None,
        }
    }

    /// The doc comment block directly above the token at `i`, which it marks
    /// consumed.
    fn doc_above(&mut self, i: usize) -> Option<String> {
        let mut line = self.line_of(self.start(i));
        let mut lines = Vec::new();
        while line > 0 {
            let Some(d) = self.docs.get(&(line - 1)) else {
                break;
            };
            lines.push(d.text.clone());
            self.consumed.insert(d.start);
            line -= 1;
        }
        if lines.is_empty() {
            return None;
        }
        lines.reverse();
        Some(lines.join("\n"))
    }

    /// Index just past the token closing the bracket opened at `i`.
    fn skip_balanced(&self, i: usize) -> usize {
        let (open, close) = match self.kind(i) {
            TokenKind::LParen => (TokenKind::LParen, TokenKind::RParen),
            TokenKind::LBrace => (TokenKind::LBrace, TokenKind::RBrace),
            TokenKind::LBracket => (TokenKind::LBracket, TokenKind::RBracket),
            _ => return i + 1,
        };
        let mut depth = 0usize;
        let mut j = i;
        loop {
            let k = self.kind(j);
            if *k == open {
                depth += 1;
            } else if *k == close {
                depth -= 1;
                if depth == 0 {
                    return j + 1;
                }
            } else if *k == TokenKind::Eof {
                return j;
            }
            j += 1;
        }
    }

    /// Index just past the `<...>` opened at `i` (which must be a `Lt`).
    fn skip_angles(&self, i: usize) -> usize {
        let mut depth = 0usize;
        let mut j = i;
        loop {
            match self.kind(j) {
                TokenKind::Lt => depth += 1,
                TokenKind::Gt => {
                    depth -= 1;
                    if depth == 0 {
                        return j + 1;
                    }
                }
                TokenKind::Eof | TokenKind::Newline | TokenKind::LBrace => return j,
                _ => {}
            }
            j += 1;
        }
    }

    /// The index where a function's header ends: its `{`, its `=`, the end of
    /// its line (a signature with no body), or a closing brace.
    fn header_end(&self, from: usize) -> usize {
        let mut depth = 0usize;
        let mut j = from;
        loop {
            match self.kind(j) {
                TokenKind::LParen | TokenKind::LBracket => depth += 1,
                TokenKind::RParen | TokenKind::RBracket => depth = depth.saturating_sub(1),
                TokenKind::LBrace | TokenKind::Eq if depth == 0 => return j,
                TokenKind::Newline | TokenKind::RBrace if depth == 0 => return j,
                TokenKind::Eof => return j,
                _ => {}
            }
            j += 1;
        }
    }

    /// Steps over a function's body: a block, an `= expression`, or nothing.
    fn skip_body(&self, j: usize) -> usize {
        match self.kind(j) {
            TokenKind::LBrace => self.skip_balanced(j),
            TokenKind::Eq => self.skip_to_line_end(j + 1),
            _ => j,
        }
    }

    /// Index of the `Newline` (or closing brace at depth 0) that ends the
    /// expression or declaration starting at `from`.
    fn skip_to_line_end(&self, from: usize) -> usize {
        let mut depth = 0usize;
        let mut j = from;
        loop {
            match self.kind(j) {
                TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => depth += 1,
                TokenKind::RParen | TokenKind::RBracket => depth = depth.saturating_sub(1),
                TokenKind::RBrace => {
                    if depth == 0 {
                        return j;
                    }
                    depth -= 1;
                }
                TokenKind::Newline if depth == 0 => {
                    if !self.continues(j) {
                        return j;
                    }
                }
                TokenKind::Eof => return j,
                _ => {}
            }
            j += 1;
        }
    }

    /// Whether the line ending at the `Newline` at `j` carries on: it ends in
    /// an operator or a comma.
    fn continues(&self, j: usize) -> bool {
        j > 0
            && matches!(
                self.kind(j - 1),
                TokenKind::AndAnd
                    | TokenKind::OrOr
                    | TokenKind::Plus
                    | TokenKind::Minus
                    | TokenKind::Star
                    | TokenKind::Slash
                    | TokenKind::Percent
                    | TokenKind::Comma
                    | TokenKind::Dot
                    | TokenKind::Eq
                    | TokenKind::Pipe
                    | TokenKind::Amp
            )
    }

    /// The tokens `from..to` as one line of source text: gaps that held a
    /// newline or a comment become one space (none inside brackets), and a
    /// trailing comma before a closing bracket is dropped.
    fn text(&self, from: usize, to: usize) -> String {
        let mut out = String::new();
        let mut prev: Option<usize> = None;
        for j in from..to.min(self.toks.len()) {
            let k = self.kind(j);
            if matches!(k, TokenKind::Newline | TokenKind::Eof) {
                continue;
            }
            if matches!(k, TokenKind::Comma) {
                let mut n = j + 1;
                while matches!(self.kind(n), TokenKind::Newline) {
                    n += 1;
                }
                if n < to && matches!(self.kind(n), TokenKind::RParen | TokenKind::RBracket) {
                    continue;
                }
            }
            if let Some(p) = prev {
                let gap = &self.src[self.end(p)..self.start(j)];
                let broken = gap.contains('\n') || gap.contains("//") || gap.contains("/*");
                if broken {
                    let after_open = matches!(
                        self.kind(p),
                        TokenKind::LParen | TokenKind::LBracket
                    );
                    let before_close = matches!(
                        k,
                        TokenKind::RParen | TokenKind::RBracket | TokenKind::Comma
                    );
                    if !after_open && !before_close {
                        out.push(' ');
                    }
                } else {
                    out.push_str(gap);
                }
            }
            out.push_str(&self.src[self.start(j)..self.end(j)]);
            prev = Some(j);
        }
        out
    }

    fn line1(&self, i: usize) -> usize {
        self.line_of(self.start(i)) + 1
    }

    fn run(mut self) -> Scan {
        let mut scan = Scan::default();
        self.module_doc(&mut scan);
        let mut aliases: BTreeMap<String, String> = BTreeMap::new();
        let mut i = 0;
        loop {
            while matches!(self.kind(i), TokenKind::Newline) {
                i += 1;
            }
            if matches!(self.kind(i), TokenKind::Eof) {
                break;
            }
            i = self.top_level(i, &mut scan, &mut aliases);
        }
        // Everything unconsumed is attached to nothing.
        for &(start, end, text) in &self.comments {
            if self.consumed.contains(&start) {
                continue;
            }
            let message = if text.starts_with("///") && !text.starts_with("////") {
                "this '///' documents nothing: a doc comment must sit directly above a fun, \
                 datac, enum, trait, pd, const, static, extern type, enum variant, field or \
                 trait method"
            } else if text.starts_with("//!") {
                "this '//!' documents nothing: a module's '//!' block must be the first thing in \
                 the file, before any declaration other than an 'impl \"...\"' import"
            } else {
                continue;
            };
            scan.orphans.push(Finding {
                span: crate::lexer::Span::new(start, end),
                message: message.to_string(),
            });
        }
        scan
    }

    /// The `//!` block before the first declaration.
    fn module_doc(&mut self, scan: &mut Scan) {
        // The first token that is neither a newline nor part of an import.
        let mut i = 0;
        loop {
            while matches!(self.kind(i), TokenKind::Newline) {
                i += 1;
            }
            if matches!(self.kind(i), TokenKind::Impl)
                && matches!(self.kind(i + 1), TokenKind::StringLit(_))
            {
                i = self.skip_to_line_end(i);
                continue;
            }
            break;
        }
        let limit = self.start(i);
        let mut run: Vec<usize> = Vec::new();
        for (&line, d) in &self.inner {
            if d.start >= limit {
                break;
            }
            match run.last() {
                Some(&last) if last + 1 != line => break,
                _ => run.push(line),
            }
        }
        if run.is_empty() {
            return;
        }
        let mut lines = Vec::new();
        for line in run {
            let d = &self.inner[&line];
            self.consumed.insert(d.start);
            lines.push(d.text.clone());
        }
        scan.module_doc = Some(lines.join("\n"));
    }

    fn top_level(
        &mut self,
        i: usize,
        scan: &mut Scan,
        aliases: &mut BTreeMap<String, String>,
    ) -> usize {
        let first = i;
        let mut j = i;
        let mut is_pub = false;
        let mut unsafe_ = false;
        let mut extern_ = false;
        loop {
            match self.kind(j) {
                TokenKind::Pub => {
                    is_pub = true;
                    j += 1;
                }
                TokenKind::Ident(n)
                    if matches!(n.as_str(), "repr" | "section" | "align")
                        && matches!(self.kind(j + 1), TokenKind::LParen) =>
                {
                    j = self.skip_balanced(j + 1);
                }
                TokenKind::Ident(n)
                    if matches!(n.as_str(), "unsafe" | "naked" | "percpu" | "must_use")
                        && matches!(
                            self.kind(j + 1),
                            TokenKind::Fun | TokenKind::Static | TokenKind::Ident(_)
                        ) =>
                {
                    if n == "unsafe" {
                        unsafe_ = true;
                    }
                    j += 1;
                }
                TokenKind::Extern if matches!(self.kind(j + 1), TokenKind::Fun) => {
                    extern_ = true;
                    j += 1;
                }
                _ => break,
            }
        }
        let line = self.line1(first);
        match self.kind(j).clone() {
            TokenKind::Impl => self.impl_item(j, scan, aliases),
            TokenKind::Fun => {
                let Some(f) = self.function(first, j) else {
                    return self.skip_to_line_end(j);
                };
                let doc = self.doc_above(first);
                let mut body_domains = BTreeSet::new();
                for k in f.header_end..f.next {
                    if let (Some(n), TokenKind::Dot) = (self.ident_at(k), self.kind(k + 1)) {
                        if let Some(d) = aliases.get(&n) {
                            body_domains.insert(d.clone());
                        }
                    }
                }
                scan.items.push(Item {
                    kind: Kind::Fun,
                    name: f.name,
                    sig: f.sig,
                    doc,
                    is_pub,
                    unsafe_: unsafe_ || f.unsafe_,
                    extern_,
                    receiver: f.receiver,
                    members: Vec::new(),
                    body_domains,
                    line,
                });
                f.next
            }
            TokenKind::Datac => {
                let Some(name) = self.ident_at(j + 1) else {
                    return self.skip_to_line_end(j);
                };
                let mut k = j + 2;
                if matches!(self.kind(k), TokenKind::Lt) {
                    k = self.skip_angles(k);
                }
                if !matches!(self.kind(k), TokenKind::LParen) {
                    return self.skip_to_line_end(j);
                }
                let close = self.skip_balanced(k);
                let sig = self.text(first, close);
                let doc = self.doc_above(first);
                let members = self.fields(k + 1, close.saturating_sub(1));
                scan.items.push(simple_item(Kind::Datac, name, sig, doc, is_pub, members, line));
                close
            }
            TokenKind::Enum => {
                let Some(name) = self.ident_at(j + 1) else {
                    return self.skip_to_line_end(j);
                };
                let Some(open) = self.find_open_brace(j + 2) else {
                    return self.skip_to_line_end(j);
                };
                let close = self.skip_balanced(open);
                let sig = self.text(first, open);
                let doc = self.doc_above(first);
                let members = self.variants(open + 1, close.saturating_sub(1));
                scan.items.push(simple_item(Kind::Enum, name, sig, doc, is_pub, members, line));
                close
            }
            TokenKind::Trait => {
                let Some(name) = self.ident_at(j + 1) else {
                    return self.skip_to_line_end(j);
                };
                let Some(open) = self.find_open_brace(j + 2) else {
                    return self.skip_to_line_end(j);
                };
                let close = self.skip_balanced(open);
                let sig = self.text(first, open);
                let doc = self.doc_above(first);
                let members = self.block_members(open + 1, close.saturating_sub(1), true);
                scan.items.push(simple_item(Kind::Trait, name, sig, doc, is_pub, members, line));
                close
            }
            TokenKind::Extern if self.is_ident(j + 1, "type") => {
                let Some(name) = self.ident_at(j + 2) else {
                    return self.skip_to_line_end(j);
                };
                let end = self.skip_to_line_end(j);
                let sig = self.text(first, end);
                let doc = self.doc_above(first);
                scan.items.push(simple_item(
                    Kind::ExternType,
                    name,
                    sig,
                    doc,
                    is_pub,
                    Vec::new(),
                    line,
                ));
                end
            }
            TokenKind::Const | TokenKind::Static | TokenKind::Pd => {
                let kind = match self.kind(j) {
                    TokenKind::Const => Kind::Const,
                    TokenKind::Static => Kind::Static,
                    _ => Kind::Pd,
                };
                let mut k = j + 1;
                while matches!(self.kind(k), TokenKind::Mut | TokenKind::Val | TokenKind::Var) {
                    k += 1;
                }
                let Some(name) = self.ident_at(k) else {
                    return self.skip_to_line_end(j);
                };
                let end = self.skip_to_line_end(j);
                let mut sig = self.text(first, end);
                if sig.chars().count() > 100 {
                    let eq = (j..end).find(|&e| matches!(self.kind(e), TokenKind::Eq));
                    if let Some(eq) = eq {
                        sig = format!("{} = ...", self.text(first, eq));
                    }
                }
                let doc = self.doc_above(first);
                scan.items.push(simple_item(kind, name, sig, doc, is_pub, Vec::new(), line));
                end
            }
            _ => self.skip_to_line_end(j),
        }
    }

    /// The `{` that opens a declaration's body, on its own header line.
    fn find_open_brace(&self, from: usize) -> Option<usize> {
        let mut j = from;
        loop {
            match self.kind(j) {
                TokenKind::LBrace => return Some(j),
                TokenKind::Newline | TokenKind::Eof | TokenKind::RBrace => return None,
                _ => j += 1,
            }
        }
    }

    fn impl_item(&mut self, j: usize, scan: &mut Scan, aliases: &mut BTreeMap<String, String>) -> usize {
        if let TokenKind::StringLit(path) = self.kind(j + 1).clone() {
            let end = self.skip_to_line_end(j);
            let text = self.text(j, end);
            let alias = path.rsplit('/').next().unwrap_or(&path).to_string();
            if let Some(d) = domain_of_module(&path) {
                aliases.insert(alias, d.to_string());
            }
            scan.imports.push(Import { text });
            return end;
        }
        let Some(open) = self.find_open_brace(j + 1) else {
            return self.skip_to_line_end(j);
        };
        let close = self.skip_balanced(open);
        let for_at = (j + 1..open).find(|&k| matches!(self.kind(k), TokenKind::For));
        if let Some(f) = for_at {
            let members = self.block_members(open + 1, close.saturating_sub(1), false);
            scan.impls.push(TraitImpl {
                trait_name: self.text(j + 1, f),
                ty: self.text(f + 1, open),
                members,
            });
        }
        close
    }

    /// The members of a `trait` or `impl` body (`from..to` is the inside of the
    /// braces).
    fn block_members(&mut self, from: usize, to: usize, in_trait: bool) -> Vec<Member> {
        let mut out = Vec::new();
        let mut j = from;
        while j < to {
            while j < to && matches!(self.kind(j), TokenKind::Newline) {
                j += 1;
            }
            if j >= to {
                break;
            }
            let first = j;
            if self.is_ident(j, "type") && matches!(self.kind(j + 1), TokenKind::Ident(_)) {
                let name = self.ident_at(j + 1).unwrap_or_default();
                let end = self.skip_to_line_end(j).min(to);
                let doc = self.doc_above(first);
                out.push(Member {
                    kind: MemberKind::AssocType,
                    name,
                    sig: self.text(first, end),
                    doc,
                    line: self.line1(first),
                });
                j = end;
                continue;
            }
            let mut k = j;
            while matches!(self.kind(k), TokenKind::Pub | TokenKind::Extern)
                || (matches!(self.kind(k), TokenKind::Ident(n) if n == "unsafe" || n == "naked" || n == "must_use")
                    && matches!(self.kind(k + 1), TokenKind::Fun | TokenKind::Ident(_)))
            {
                k += 1;
            }
            if matches!(self.kind(k), TokenKind::Fun) {
                if let Some(f) = self.function(first, k) {
                    let doc = self.doc_above(first);
                    out.push(Member {
                        kind: MemberKind::Method,
                        name: f.name,
                        sig: f.sig,
                        doc,
                        line: self.line1(first),
                    });
                    j = f.next.min(to).max(first + 1);
                    continue;
                }
            }
            let _ = in_trait;
            j = self.skip_to_line_end(j).min(to).max(first + 1);
        }
        out
    }

    /// The fields of a `datac` (`from..to` is the inside of the parentheses).
    fn fields(&mut self, from: usize, to: usize) -> Vec<Member> {
        let mut out = Vec::new();
        let mut j = from;
        while j < to {
            while j < to && matches!(self.kind(j), TokenKind::Newline | TokenKind::Comma) {
                j += 1;
            }
            if j >= to {
                break;
            }
            let first = j;
            let mut depth = 0usize;
            while j < to {
                match self.kind(j) {
                    TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => depth += 1,
                    TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace => {
                        depth = depth.saturating_sub(1)
                    }
                    TokenKind::Comma if depth == 0 => break,
                    _ => {}
                }
                j += 1;
            }
            let mut k = first;
            while matches!(self.kind(k), TokenKind::Val | TokenKind::Var | TokenKind::Mut) {
                k += 1;
            }
            let name = self.ident_at(k).unwrap_or_default();
            // A field written on the same line as the token before it (the
            // `(` of `datac P(var x: i32, ...`, or an earlier field) has no
            // line of its own to carry a doc comment, and the comment above
            // that line belongs to the `datac`.
            let mut prev = first;
            while prev > 0 && matches!(self.kind(prev - 1), TokenKind::Newline) {
                prev -= 1;
            }
            let own_line = prev < first
                || prev == 0
                || self.line_of(self.start(prev - 1)) != self.line_of(self.start(first));
            let doc = if own_line { self.doc_above(first) } else { None };
            out.push(Member {
                kind: MemberKind::Field,
                name,
                sig: self.text(first, j),
                doc,
                line: self.line1(first),
            });
        }
        out
    }

    /// The variants of an `enum` (`from..to` is the inside of the braces).
    fn variants(&mut self, from: usize, to: usize) -> Vec<Member> {
        let mut out = Vec::new();
        let mut j = from;
        while j < to {
            while j < to && matches!(self.kind(j), TokenKind::Newline | TokenKind::Comma) {
                j += 1;
            }
            if j >= to {
                break;
            }
            let first = j;
            let mut depth = 0usize;
            while j < to {
                match self.kind(j) {
                    TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => depth += 1,
                    TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace => {
                        depth = depth.saturating_sub(1)
                    }
                    TokenKind::Comma | TokenKind::Newline if depth == 0 => break,
                    _ => {}
                }
                j += 1;
            }
            let name = self.ident_at(first).unwrap_or_default();
            let doc = self.doc_above(first);
            out.push(Member {
                kind: MemberKind::Variant,
                name,
                sig: self.text(first, j),
                doc,
                line: self.line1(first),
            });
        }
        out
    }

    /// A `fun` declaration whose `fun` keyword is at `fun_at` and whose first
    /// token (a modifier, or the `fun` itself) is at `first`.
    fn function(&self, first: usize, fun_at: usize) -> Option<ParsedFun> {
        let mut k = fun_at + 1;
        if matches!(self.kind(k), TokenKind::Mut) {
            k += 1;
        }
        let a = self.ident_at(k)?;
        k += 1;
        if matches!(self.kind(k), TokenKind::Lt) {
            k = self.skip_angles(k);
        }
        let (name, receiver) = if matches!(self.kind(k), TokenKind::Dot) {
            (self.ident_at(k + 1)?, Some(a))
        } else {
            (a, None)
        };
        let header_end = self.header_end(fun_at + 1);
        let next = self.skip_body(header_end);
        let unsafe_ = (first..fun_at).any(|m| self.is_ident(m, "unsafe"));
        Some(ParsedFun {
            name,
            receiver,
            sig: self.text(first, header_end),
            header_end,
            next,
            unsafe_,
        })
    }
}

struct ParsedFun {
    name: String,
    receiver: Option<String>,
    sig: String,
    header_end: usize,
    next: usize,
    unsafe_: bool,
}

fn simple_item(
    kind: Kind,
    name: String,
    sig: String,
    doc: Option<String>,
    is_pub: bool,
    members: Vec<Member>,
    line: usize,
) -> Item {
    Item {
        kind,
        name,
        sig,
        doc,
        is_pub,
        unsafe_: false,
        extern_: false,
        receiver: None,
        members,
        body_domains: BTreeSet::new(),
        line,
    }
}

// ---------------------------------------------------------------------------
// Pages
// ---------------------------------------------------------------------------

/// One module to document.
pub struct Module {
    /// `std/option`, `shapes/circle`, `main`.
    pub id: String,
    /// The file as the user named it, for `--check` messages.
    pub path: String,
    pub scan: Scan,
    /// Every item is public (the shipped std modules, which have no `pub`).
    pub all_public: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Markdown,
    Html,
}

impl Format {
    pub fn ext(self) -> &'static str {
        match self {
            Format::Markdown => "md",
            Format::Html => "html",
        }
    }
}

pub struct Options {
    pub format: Format,
    pub private: bool,
}

struct Entry<'a> {
    item: &'a Item,
    anchor: String,
    methods: Vec<(&'a Item, String)>,
    impls: Vec<&'a TraitImpl>,
}

struct Section<'a> {
    title: String,
    entries: Vec<Entry<'a>>,
}

struct Page<'a> {
    module: &'a Module,
    file: String,
    sections: Vec<Section<'a>>,
    other_impls: Vec<&'a TraitImpl>,
}

fn page_file(id: &str, format: Format) -> String {
    format!("{}.{}", id.replace('/', "."), format.ext())
}

/// GitHub's heading anchor for `text`, which both formats use so a link into a
/// page means the same in either.
fn anchor_of(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if c.is_alphanumeric() || c == '_' || c == '-' {
            out.extend(c.to_lowercase());
        } else if c == ' ' {
            out.push('-');
        }
    }
    out
}

struct Anchors(HashMap<String, usize>);

impl Anchors {
    fn take(&mut self, text: &str) -> String {
        let base = anchor_of(text);
        let n = self.0.entry(base.clone()).or_insert(0);
        let out = if *n == 0 { base } else { format!("{base}-{n}") };
        *n += 1;
        out
    }
}

fn base_name(path: &str) -> &str {
    let head = path.split('<').next().unwrap_or(path);
    head.rsplit('.').next().unwrap_or(head).trim()
}

impl Module {
    fn is_public(&self, item: &Item) -> bool {
        item.is_pub || self.all_public
    }

    /// Whether an item gets a page entry.
    fn shown(&self, item: &Item, opts: &Options) -> bool {
        if opts.private {
            return true;
        }
        if item.extern_ && self.all_public {
            return false;
        }
        if item.kind == Kind::Pd {
            return false;
        }
        if item.receiver.is_some() {
            return self.method_public(item);
        }
        self.is_public(item)
    }

    /// A receiver method is public with its type: the type is a public local
    /// one, or the module is a shipped one.
    fn method_public(&self, item: &Item) -> bool {
        if self.all_public {
            return !item.extern_;
        }
        let Some(recv) = &item.receiver else {
            return false;
        };
        self.scan
            .items
            .iter()
            .any(|t| t.kind.is_type() && t.name == *recv && t.is_pub)
    }

    fn domains(&self, item: &Item) -> BTreeSet<String> {
        let mut out = item.body_domains.clone();
        if let Some(d) = domain_of_module(&self.id) {
            out.insert(d.to_string());
        }
        out
    }
}

fn build_page<'a>(module: &'a Module, opts: &Options) -> Page<'a> {
    let mut anchors = Anchors(HashMap::new());
    let shown: Vec<&Item> = module
        .scan
        .items
        .iter()
        .filter(|i| module.shown(i, opts))
        .collect();
    let mut free: Vec<&Item> = shown.iter().copied().filter(|i| i.receiver.is_none()).collect();
    free.sort_by(|a, b| a.name.cmp(&b.name).then(a.line.cmp(&b.line)));
    let methods: Vec<&Item> = shown.iter().copied().filter(|i| i.receiver.is_some()).collect();
    let local_types: BTreeSet<&str> = free
        .iter()
        .filter(|i| i.kind.is_type())
        .map(|i| i.name.as_str())
        .collect();

    let order = [
        ("Types", &[Kind::Datac, Kind::Enum, Kind::ExternType][..]),
        ("Traits", &[Kind::Trait][..]),
        ("Functions", &[Kind::Fun][..]),
        ("Constants", &[Kind::Const][..]),
        ("Statics", &[Kind::Static][..]),
        ("Passdowns", &[Kind::Pd][..]),
    ];
    let mut sections = Vec::new();
    let mut impl_used: HashSet<usize> = HashSet::new();
    for (title, kinds) in order {
        let mut entries = Vec::new();
        for item in free.iter().filter(|i| kinds.contains(&i.kind)) {
            let anchor = anchors.take(&item.name);
            let mut ms: Vec<&Item> = methods
                .iter()
                .copied()
                .filter(|m| m.receiver.as_deref() == Some(item.name.as_str()))
                .collect();
            ms.sort_by(|a, b| a.name.cmp(&b.name).then(a.line.cmp(&b.line)));
            let methods: Vec<(&Item, String)> = ms
                .into_iter()
                .map(|m| {
                    let a = anchors.take(&format!("{}.{}", item.name, m.name));
                    (m, a)
                })
                .collect();
            let mut impls: Vec<&TraitImpl> = Vec::new();
            if item.kind.is_type() {
                for (n, im) in module.scan.impls.iter().enumerate() {
                    if base_name(&im.ty) == item.name {
                        impls.push(im);
                        impl_used.insert(n);
                    }
                }
                impls.sort_by(|a, b| a.trait_name.cmp(&b.trait_name));
            }
            entries.push(Entry { item, anchor, methods, impls });
        }
        if !entries.is_empty() {
            sections.push(Section { title: title.to_string(), entries });
        }
    }
    // Methods on a type this module does not declare, grouped by that type.
    let mut foreign: BTreeMap<&str, Vec<&Item>> = BTreeMap::new();
    for m in &methods {
        let r = m.receiver.as_deref().unwrap_or("");
        if !local_types.contains(r) {
            foreign.entry(r).or_default().push(m);
        }
    }
    if !foreign.is_empty() {
        let mut entries = Vec::new();
        for (recv, mut ms) in foreign {
            ms.sort_by(|a, b| a.name.cmp(&b.name).then(a.line.cmp(&b.line)));
            for m in ms {
                let anchor = anchors.take(&format!("{recv}.{}", m.name));
                entries.push(Entry { item: m, anchor, methods: Vec::new(), impls: Vec::new() });
            }
        }
        sections.push(Section { title: "Methods on other types".to_string(), entries });
    }
    let mut other_impls: Vec<&TraitImpl> = module
        .scan
        .impls
        .iter()
        .enumerate()
        .filter(|(n, im)| {
            !impl_used.contains(n)
                && (opts.private || module.all_public || !local_types.contains(base_name(&im.ty)))
        })
        .map(|(_, im)| im)
        .collect();
    other_impls.sort_by(|a, b| a.ty.cmp(&b.ty).then(a.trait_name.cmp(&b.trait_name)));
    Page { module, file: page_file(&module.id, opts.format), sections, other_impls }
}

/// Name -> the one place it is declared, for the types and traits that are
/// unambiguous across every page. A name declared twice has no entry.
type LinkIndex = HashMap<String, (String, String)>;

fn link_index(pages: &[Page]) -> LinkIndex {
    let mut seen: HashMap<String, Vec<(String, String)>> = HashMap::new();
    for p in pages {
        for s in &p.sections {
            for e in &s.entries {
                if e.item.kind.is_type() {
                    seen.entry(e.item.name.clone())
                        .or_default()
                        .push((p.file.clone(), e.anchor.clone()));
                }
            }
        }
    }
    seen.into_iter()
        .filter(|(_, v)| v.len() == 1)
        .map(|(k, mut v)| (k, v.remove(0)))
        .collect()
}

fn target(index: &LinkIndex, from: &str, name: &str) -> Option<String> {
    let (file, anchor) = index.get(name)?;
    Some(if file == from {
        format!("#{anchor}")
    } else {
        format!("{file}#{anchor}")
    })
}

fn is_ident_text(s: &str) -> bool {
    !s.is_empty()
        && s.chars().all(|c| c.is_alphanumeric() || c == '_')
        && !s.chars().next().is_some_and(|c| c.is_ascii_digit())
}

/// Doc text with each `` `Name` `` that names a unique type or trait linked.
fn link_markdown(text: &str, index: &LinkIndex, from: &str) -> String {
    let mut out = Vec::new();
    let mut fenced = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            out.push(line.to_string());
            continue;
        }
        if fenced {
            out.push(line.to_string());
            continue;
        }
        let mut rest = line;
        let mut buf = String::new();
        while let Some(a) = rest.find('`') {
            buf.push_str(&rest[..a]);
            let after = &rest[a + 1..];
            let Some(b) = after.find('`') else {
                buf.push_str(&rest[a..]);
                rest = "";
                break;
            };
            let word = &after[..b];
            match target(index, from, word).filter(|_| is_ident_text(word)) {
                Some(t) => buf.push_str(&format!("[`{word}`]({t})")),
                None => buf.push_str(&format!("`{word}`")),
            }
            rest = &after[b + 1..];
        }
        buf.push_str(rest);
        out.push(buf);
    }
    out.join("\n")
}

fn first_paragraph(doc: &str) -> String {
    let mut words = Vec::new();
    for line in doc.lines() {
        if line.trim().is_empty() {
            if words.is_empty() {
                continue;
            }
            break;
        }
        words.push(line.trim());
    }
    words.join(" ")
}

fn badges(module: &Module, item: &Item) -> Vec<String> {
    let mut out = Vec::new();
    if item.unsafe_ {
        out.push("unsafe".to_string());
    }
    let d = module.domains(item);
    if !d.is_empty() && item.kind == Kind::Fun {
        let list: Vec<String> = d.iter().map(|x| format!("`{x}`")).collect();
        out.push(format!("requires capability {}", list.join(", ")));
    }
    out
}

fn impl_line(im: &TraitImpl, index: &LinkIndex, from: &str) -> String {
    let t = base_name(&im.trait_name);
    match target(index, from, t) {
        Some(link) => {
            let args = &im.trait_name[t.len().min(im.trait_name.len())..];
            let head = im.trait_name.rfind(t).map(|p| &im.trait_name[..p]).unwrap_or("");
            format!("`impl` [`{head}{t}`]({link})`{args} for {}`", im.ty)
        }
        None => format!("`impl {} for {}`", im.trait_name, im.ty),
    }
}

/// The Markdown page for one module.
fn render_md(page: &Page, index: &LinkIndex) -> String {
    let m = page.module;
    let mut o = String::new();
    o.push_str(&format!("# Module `{}`\n\n", m.id));
    if let Some(doc) = &m.scan.module_doc {
        o.push_str(&link_markdown(doc, index, &page.file));
        o.push_str("\n\n");
    }
    if !m.scan.imports.is_empty() {
        o.push_str("## Imports\n\n");
        for im in &m.scan.imports {
            o.push_str(&format!("- `{}`\n", im.text));
        }
        o.push('\n');
    }
    for s in &page.sections {
        o.push_str(&format!("## {}\n\n", s.title));
        for e in &s.entries {
            let heading = match &e.item.receiver {
                Some(r) => format!("{r}.{}", e.item.name),
                None => e.item.name.clone(),
            };
            o.push_str(&format!("### {}\n\n", md_heading(&heading)));
            md_item(&mut o, page, e.item, index);
            if !e.item.members.is_empty() {
                let title = match e.item.kind {
                    Kind::Enum => "Variants",
                    Kind::Trait => "Required and provided items",
                    _ => "Fields",
                };
                o.push_str(&format!("**{title}**\n\n"));
                for mem in &e.item.members {
                    o.push_str(&format!("- `{}`", mem.sig));
                    if let Some(d) = &mem.doc {
                        let d = first_paragraph(d);
                        o.push_str(&format!(": {}", link_markdown(&d, index, &page.file)));
                    }
                    o.push('\n');
                }
                o.push('\n');
            }
            for (meth, anchor) in &e.methods {
                let _ = anchor;
                o.push_str(&format!(
                    "#### {}\n\n",
                    md_heading(&format!("{}.{}", e.item.name, meth.name))
                ));
                md_item(&mut o, page, meth, index);
            }
            if !e.impls.is_empty() {
                o.push_str("**Trait impls**\n\n");
                for im in &e.impls {
                    o.push_str(&format!("- {}\n", impl_line(im, index, &page.file)));
                }
                o.push('\n');
            }
        }
    }
    if !page.other_impls.is_empty() {
        o.push_str("## Trait impls for other types\n\n");
        for im in &page.other_impls {
            o.push_str(&format!("- {}\n", impl_line(im, index, &page.file)));
        }
        o.push('\n');
    }
    while o.ends_with("\n\n") {
        o.pop();
    }
    o
}

/// A heading's text with its code spelling, so the heading's GitHub anchor is
/// the one `anchor_of` computed.
fn md_heading(text: &str) -> String {
    format!("`{text}`")
}

fn md_item(o: &mut String, page: &Page, item: &Item, index: &LinkIndex) {
    o.push_str("```kelp\n");
    o.push_str(&item.sig);
    o.push_str("\n```\n\n");
    let b = badges(page.module, item);
    if !b.is_empty() {
        o.push_str(&format!("*{}*\n\n", b.join("; ")));
    }
    if let Some(d) = &item.doc {
        o.push_str(&link_markdown(d, index, &page.file));
        o.push_str("\n\n");
    }
}

fn md_index(pages: &[Page]) -> String {
    let mut o = String::from("# Modules\n\n");
    for p in pages {
        o.push_str(&format!("- [`{}`]({})", p.module.id, p.file));
        if let Some(d) = &p.module.scan.module_doc {
            let s = first_paragraph(d);
            if !s.is_empty() {
                o.push_str(&format!(" - {s}"));
            }
        }
        o.push('\n');
    }
    o
}

// -- HTML -------------------------------------------------------------------

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            _ => o.push(c),
        }
    }
    o
}

fn html_inline(text: &str, index: &LinkIndex, from: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(a) = rest.find('`') {
        out.push_str(&esc(&rest[..a]));
        let after = &rest[a + 1..];
        let Some(b) = after.find('`') else {
            out.push_str(&esc(&rest[a..]));
            return out;
        };
        let word = &after[..b];
        match target(index, from, word).filter(|_| is_ident_text(word)) {
            Some(t) => out.push_str(&format!("<a href=\"{}\"><code>{}</code></a>", esc(&t), esc(word))),
            None => out.push_str(&format!("<code>{}</code>", esc(word))),
        }
        rest = &after[b + 1..];
    }
    out.push_str(&esc(rest));
    out
}

/// Doc text as HTML: paragraphs, `- ` lists and fenced code blocks.
fn html_doc(text: &str, index: &LinkIndex, from: &str) -> String {
    let mut o = String::new();
    let mut para: Vec<&str> = Vec::new();
    let mut list: Vec<&str> = Vec::new();
    let flush = |o: &mut String, para: &mut Vec<&str>, list: &mut Vec<&str>| {
        if !para.is_empty() {
            o.push_str(&format!("<p>{}</p>\n", html_inline(&para.join(" "), index, from)));
            para.clear();
        }
        if !list.is_empty() {
            o.push_str("<ul>\n");
            for l in list.iter() {
                o.push_str(&format!("<li>{}</li>\n", html_inline(l, index, from)));
            }
            o.push_str("</ul>\n");
            list.clear();
        }
    };
    let mut fenced = false;
    let mut code = String::new();
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            if fenced {
                o.push_str(&format!("<pre><code>{}</code></pre>\n", esc(code.trim_end_matches('\n'))));
                code.clear();
            } else {
                flush(&mut o, &mut para, &mut list);
            }
            fenced = !fenced;
            continue;
        }
        if fenced {
            code.push_str(line);
            code.push('\n');
        } else if line.trim().is_empty() {
            flush(&mut o, &mut para, &mut list);
        } else if let Some(item) = line.trim_start().strip_prefix("- ") {
            if !para.is_empty() {
                flush(&mut o, &mut para, &mut list);
            }
            list.push(item);
        } else {
            if !list.is_empty() {
                flush(&mut o, &mut para, &mut list);
            }
            para.push(line.trim());
        }
    }
    if fenced {
        o.push_str(&format!("<pre><code>{}</code></pre>\n", esc(code.trim_end_matches('\n'))));
    }
    flush(&mut o, &mut para, &mut list);
    o
}

const CSS: &str = "body{font-family:system-ui,sans-serif;max-width:52rem;margin:2rem auto;padding:0 1rem;line-height:1.5;color:#1b1b1b;background:#fff}\
pre{background:#f4f4f4;padding:.6rem .8rem;overflow-x:auto;border-radius:4px}\
code{font-family:ui-monospace,monospace}\
h2{border-bottom:1px solid #ddd;padding-bottom:.2rem;margin-top:2rem}\
.badges{font-style:italic;color:#555}\
@media (prefers-color-scheme:dark){body{background:#161616;color:#e6e6e6}pre{background:#252525}h2{border-color:#444}a{color:#8ab4f8}.badges{color:#aaa}}";

fn html_shell(title: &str, body: &str) -> String {
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<title>{}</title>\n<style>{}</style>\n</head>\n<body>\n{}</body>\n</html>\n",
        esc(title),
        CSS,
        body
    )
}

fn html_item(o: &mut String, page: &Page, item: &Item, index: &LinkIndex) {
    o.push_str(&format!("<pre><code>{}</code></pre>\n", esc(&item.sig)));
    let b = badges(page.module, item);
    if !b.is_empty() {
        o.push_str(&format!(
            "<p class=\"badges\">{}</p>\n",
            html_inline(&b.join("; "), index, &page.file)
        ));
    }
    if let Some(d) = &item.doc {
        o.push_str(&html_doc(d, index, &page.file));
    }
}

fn render_html(page: &Page, index: &LinkIndex) -> String {
    let m = page.module;
    let mut o = String::new();
    o.push_str(&format!(
        "<p><a href=\"index.html\">All modules</a></p>\n<h1>Module <code>{}</code></h1>\n",
        esc(&m.id)
    ));
    if let Some(doc) = &m.scan.module_doc {
        o.push_str(&html_doc(doc, index, &page.file));
    }
    if !m.scan.imports.is_empty() {
        o.push_str("<h2>Imports</h2>\n<ul>\n");
        for im in &m.scan.imports {
            o.push_str(&format!("<li><code>{}</code></li>\n", esc(&im.text)));
        }
        o.push_str("</ul>\n");
    }
    for s in &page.sections {
        o.push_str(&format!("<h2>{}</h2>\n", esc(&s.title)));
        for e in &s.entries {
            let heading = match &e.item.receiver {
                Some(r) => format!("{r}.{}", e.item.name),
                None => e.item.name.clone(),
            };
            o.push_str(&format!("<h3 id=\"{}\"><code>{}</code></h3>\n", esc(&e.anchor), esc(&heading)));
            html_item(&mut o, page, e.item, index);
            if !e.item.members.is_empty() {
                let title = match e.item.kind {
                    Kind::Enum => "Variants",
                    Kind::Trait => "Required and provided items",
                    _ => "Fields",
                };
                o.push_str(&format!("<p><strong>{title}</strong></p>\n<ul>\n"));
                for mem in &e.item.members {
                    o.push_str(&format!("<li><code>{}</code>", esc(&mem.sig)));
                    if let Some(d) = &mem.doc {
                        o.push_str(&format!(
                            ": {}",
                            html_inline(&first_paragraph(d), index, &page.file)
                        ));
                    }
                    o.push_str("</li>\n");
                }
                o.push_str("</ul>\n");
            }
            for (meth, anchor) in &e.methods {
                o.push_str(&format!(
                    "<h4 id=\"{}\"><code>{}.{}</code></h4>\n",
                    esc(anchor),
                    esc(&e.item.name),
                    esc(&meth.name)
                ));
                html_item(&mut o, page, meth, index);
            }
            if !e.impls.is_empty() {
                o.push_str("<p><strong>Trait impls</strong></p>\n<ul>\n");
                for im in &e.impls {
                    o.push_str(&format!("<li>{}</li>\n", html_impl(im, index, &page.file)));
                }
                o.push_str("</ul>\n");
            }
        }
    }
    if !page.other_impls.is_empty() {
        o.push_str("<h2>Trait impls for other types</h2>\n<ul>\n");
        for im in &page.other_impls {
            o.push_str(&format!("<li>{}</li>\n", html_impl(im, index, &page.file)));
        }
        o.push_str("</ul>\n");
    }
    html_shell(&format!("Module {}", m.id), &o)
}

fn html_impl(im: &TraitImpl, index: &LinkIndex, from: &str) -> String {
    let t = base_name(&im.trait_name);
    let plain = format!("impl {} for {}", im.trait_name, im.ty);
    match target(index, from, t) {
        Some(link) => format!(
            "<code>impl</code> <a href=\"{}\"><code>{}</code></a><code> for {}</code>",
            esc(&link),
            esc(&im.trait_name),
            esc(&im.ty)
        ),
        None => format!("<code>{}</code>", esc(&plain)),
    }
}

fn html_index(pages: &[Page]) -> String {
    let mut o = String::from("<h1>Modules</h1>\n<ul>\n");
    for p in pages {
        o.push_str(&format!(
            "<li><a href=\"{}\"><code>{}</code></a>",
            esc(&p.file),
            esc(&p.module.id)
        ));
        if let Some(d) = &p.module.scan.module_doc {
            let s = first_paragraph(d);
            if !s.is_empty() {
                o.push_str(&format!(" - {}", esc(&s)));
            }
        }
        o.push_str("</li>\n");
    }
    o.push_str("</ul>\n");
    html_shell("Modules", &o)
}

/// Every page of the documentation set as `(file name, text)`, index first and
/// the rest sorted by module id. No timestamps, no host paths: the same input
/// is the same bytes.
pub fn render(modules: &[Module], opts: &Options) -> Vec<(String, String)> {
    let mut sorted: Vec<&Module> = modules.iter().collect();
    sorted.sort_by(|a, b| a.id.cmp(&b.id));
    let pages: Vec<Page> = sorted.iter().map(|m| build_page(m, opts)).collect();
    let index = link_index(&pages);
    let mut out = Vec::new();
    match opts.format {
        Format::Markdown => {
            out.push(("index.md".to_string(), md_index(&pages)));
            for p in &pages {
                out.push((p.file.clone(), render_md(p, &index)));
            }
        }
        Format::Html => {
            out.push(("index.html".to_string(), html_index(&pages)));
            for p in &pages {
                out.push((p.file.clone(), render_html(p, &index)));
            }
        }
    }
    out.into_iter().map(|(name, text)| (name, no_dashes(&text))).collect()
}

/// Replaces every en dash and em dash in a generated page with a hyphen, and
/// the HTML entities that would display one. This runs over the whole text of
/// every page, so a dash cannot reach a page from any source: the doc comments
/// copied from `.kp` files, a signature, or the generator's own prose. The
/// generator's own sentences were reworded to read correctly on their own; the
/// hyphen is the fallback for text it copies from source.
pub fn no_dashes(text: &str) -> String {
    text.replace("&mdash;", "-")
        .replace("&ndash;", "-")
        .replace("&#8212;", "-")
        .replace("&#8211;", "-")
        .replace("&#x2014;", "-")
        .replace("&#x2013;", "-")
        .replace(['\u{2014}', '\u{2013}'], "-")
}

/// The public items without a doc comment, as `(file, line, message)`.
pub fn missing_docs(modules: &[Module], opts: &Options) -> Vec<(String, usize, String)> {
    let mut out = Vec::new();
    let mut sorted: Vec<&Module> = modules.iter().collect();
    sorted.sort_by(|a, b| a.id.cmp(&b.id));
    for m in sorted {
        for item in &m.scan.items {
            if !m.shown(item, opts) {
                continue;
            }
            let name = match &item.receiver {
                Some(r) => format!("{r}.{}", item.name),
                None => item.name.clone(),
            };
            if item.doc.is_none() {
                out.push((
                    m.path.clone(),
                    item.line,
                    format!("public {} `{name}` has no doc comment", item.kind.word()),
                ));
            }
            for mem in &item.members {
                if mem.doc.is_none() && mem.kind != MemberKind::AssocType {
                    out.push((
                        m.path.clone(),
                        mem.line,
                        format!(
                            "public {} `{}.{}` has no doc comment",
                            mem.kind.word(),
                            item.name,
                            mem.name
                        ),
                    ));
                }
            }
        }
    }
    out
}
