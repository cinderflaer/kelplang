use super::*;

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

pub(super) fn adopt_signatures(scan: &mut Scan, other: &Scan) {
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

pub(super) struct DocLine {
    start: usize,
    text: String,
}

pub(super) struct Scanner<'a> {
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
    /// True while the items of a `pub { ... }` group are read (issue #290): each
    /// item in the group is exported as if `pub` were written on it.
    pub_group: bool,
    eof: Token,
}

pub(super) fn strip_doc(text: &str, marker: &str) -> String {
    let rest = &text[marker.len()..];
    rest.strip_prefix(' ').unwrap_or(rest).trim_end().to_string()
}

impl<'a> Scanner<'a> {
    pub(super) fn new(src: &'a str, trivia: &[Token]) -> Self {
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
            pub_group: false,
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

    pub(super) fn line_of(&self, offset: usize) -> usize {
        self.line_starts.partition_point(|&s| s <= offset) - 1
    }

    pub(super) fn kind(&self, i: usize) -> &TokenKind {
        self.toks.get(i).map(|t| &t.kind).unwrap_or(&self.eof.kind)
    }

    pub(super) fn start(&self, i: usize) -> usize {
        self.toks.get(i).map(|t| t.span.start).unwrap_or(self.src.len())
    }

    pub(super) fn end(&self, i: usize) -> usize {
        self.toks.get(i).map(|t| t.span.end).unwrap_or(self.src.len())
    }

    pub(super) fn is_ident(&self, i: usize, word: &str) -> bool {
        matches!(self.kind(i), TokenKind::Ident(n) if n == word)
    }

    pub(super) fn ident_at(&self, i: usize) -> Option<String> {
        match self.kind(i) {
            TokenKind::Ident(n) => Some(n.clone()),
            _ => None,
        }
    }

    /// The doc comment block directly above the token at `i`, which it marks
    /// consumed.
    pub(super) fn doc_above(&mut self, i: usize) -> Option<String> {
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
    pub(super) fn skip_balanced(&self, i: usize) -> usize {
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
    pub(super) fn skip_angles(&self, i: usize) -> usize {
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
    pub(super) fn header_end(&self, from: usize) -> usize {
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
    pub(super) fn skip_body(&self, j: usize) -> usize {
        match self.kind(j) {
            TokenKind::LBrace => self.skip_balanced(j),
            TokenKind::Eq => self.skip_to_line_end(j + 1),
            _ => j,
        }
    }

    /// Index of the `Newline` (or closing brace at depth 0) that ends the
    /// expression or declaration starting at `from`.
    pub(super) fn skip_to_line_end(&self, from: usize) -> usize {
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
    pub(super) fn continues(&self, j: usize) -> bool {
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
    pub(super) fn text(&self, from: usize, to: usize) -> String {
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

    pub(super) fn line1(&self, i: usize) -> usize {
        self.line_of(self.start(i)) + 1
    }

    /// Byte offsets of the token at `i`.
    pub(super) fn tok_span(&self, i: usize) -> (usize, usize) {
        (self.start(i), self.end(i))
    }

    /// Byte offset where the last real token before `end` (exclusive) ends.
    pub(super) fn last_end(&self, from: usize, end: usize) -> usize {
        let mut e = end.min(self.toks.len());
        while e > from
            && matches!(self.kind(e - 1), TokenKind::Newline | TokenKind::Comma | TokenKind::Eof)
        {
            e -= 1;
        }
        if e <= from {
            return self.start(from);
        }
        self.end(e - 1)
    }

    /// The 1-based line the last real token before `end` (exclusive) ends on,
    /// skipping the newlines, commas and end of file that trail it. `from` is
    /// the declaration's first token, the answer when nothing precedes `end`.
    pub(super) fn end_line_before(&self, from: usize, end: usize) -> usize {
        let mut e = end.min(self.toks.len());
        while e > from
            && matches!(self.kind(e - 1), TokenKind::Newline | TokenKind::Comma | TokenKind::Eof)
        {
            e -= 1;
        }
        if e <= from {
            return self.line1(from);
        }
        self.line_of(self.end(e - 1).saturating_sub(1)) + 1
    }

    pub(super) fn run(mut self) -> Scan {
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
            if self.pub_group {
                if matches!(self.kind(i), TokenKind::RBrace) {
                    self.pub_group = false;
                    i += 1;
                    continue;
                }
            } else if matches!(self.kind(i), TokenKind::Pub) && matches!(self.kind(i + 1), TokenKind::LBrace) {
                self.pub_group = true;
                i += 2;
                continue;
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
    pub(super) fn module_doc(&mut self, scan: &mut Scan) {
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

    pub(super) fn top_level(
        &mut self,
        i: usize,
        scan: &mut Scan,
        aliases: &mut BTreeMap<String, String>,
    ) -> usize {
        let first = i;
        let mut j = i;
        let mut is_pub = self.pub_group;
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
                    if matches!(n.as_str(), "unsafe" | "naked" | "interrupt" | "percpu" | "must_use")
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
                    end_line: self.end_line_before(first, f.next),
                    name_span: self.tok_span(f.name_at),
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
                let span = self.tok_span(j + 1);
                let end_line = self.end_line_before(first, close);
                scan.items.push(simple_item(Kind::Datac, name, sig, doc, is_pub, members, line, span, end_line));
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
                let span = self.tok_span(j + 1);
                let end_line = self.end_line_before(first, close);
                scan.items.push(simple_item(Kind::Enum, name, sig, doc, is_pub, members, line, span, end_line));
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
                let span = self.tok_span(j + 1);
                let end_line = self.end_line_before(first, close);
                scan.items.push(simple_item(Kind::Trait, name, sig, doc, is_pub, members, line, span, end_line));
                close
            }
            TokenKind::Extern if self.is_ident(j + 1, "type") => {
                let Some(name) = self.ident_at(j + 2) else {
                    return self.skip_to_line_end(j);
                };
                let end = self.skip_to_line_end(j);
                let sig = self.text(first, end);
                let doc = self.doc_above(first);
                let span = self.tok_span(j + 2);
                let end_line = self.end_line_before(first, end);
                scan.items.push(simple_item(
                    Kind::ExternType,
                    name,
                    sig,
                    doc,
                    is_pub,
                    Vec::new(),
                    line,
                    span,
                    end_line,
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
                let span = self.tok_span(k);
                let end_line = self.end_line_before(first, end);
                scan.items.push(simple_item(kind, name, sig, doc, is_pub, Vec::new(), line, span, end_line));
                end
            }
            _ => self.skip_to_line_end(j),
        }
    }

    /// The `{` that opens a declaration's body, on its own header line.
    pub(super) fn find_open_brace(&self, from: usize) -> Option<usize> {
        let mut j = from;
        loop {
            match self.kind(j) {
                TokenKind::LBrace => return Some(j),
                TokenKind::Newline | TokenKind::Eof | TokenKind::RBrace => return None,
                _ => j += 1,
            }
        }
    }

    pub(super) fn impl_item(&mut self, j: usize, scan: &mut Scan, aliases: &mut BTreeMap<String, String>) -> usize {
        if let TokenKind::StringLit(_) = self.kind(j + 1).clone() {
            let end = self.skip_to_line_end(j);
            let line = self.text(j, end);
            // `impl "a", "b"` (#299): one import per path. Each entry is listed
            // as its own `impl "path"` line, and its span runs from the keyword
            // to that path.
            let listed = matches!(self.kind(j + 2), TokenKind::Comma);
            let mut k = j + 1;
            while let TokenKind::StringLit(path) = self.kind(k).clone() {
                let alias = path.rsplit('/').next().unwrap_or(&path).to_string();
                if let Some(d) = domain_of_module(&path) {
                    aliases.insert(alias.clone(), d.to_string());
                }
                let (text, span) = if listed {
                    (format!("impl \"{path}\""), (self.start(j), self.last_end(j, k + 1)))
                } else {
                    (line.clone(), (self.start(j), self.last_end(j, end)))
                };
                scan.imports.push(Import {
                    text,
                    alias: crate::resolve::default_alias(&path),
                    path,
                    line: self.line1(j),
                    span,
                });
                if !matches!(self.kind(k + 1), TokenKind::Comma) {
                    break;
                }
                k += 2;
            }
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
                line: self.line1(j),
                end_line: self.end_line_before(j, close),
            });
        }
        close
    }

    /// The members of a `trait` or `impl` body (`from..to` is the inside of the
    /// braces).
    pub(super) fn block_members(&mut self, from: usize, to: usize, in_trait: bool) -> Vec<Member> {
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
                    end_line: self.end_line_before(first, end),
                    name_span: self.tok_span(j + 1),
                    is_pub: false,
                });
                j = end;
                continue;
            }
            let mut k = j;
            while matches!(self.kind(k), TokenKind::Pub | TokenKind::Extern)
                || (matches!(self.kind(k), TokenKind::Ident(n) if n == "unsafe" || n == "naked" || n == "must_use" || n == "interrupt")
                    && matches!(self.kind(k + 1), TokenKind::Fun | TokenKind::Ident(_)))
            {
                k += 1;
            }
            if matches!(self.kind(k), TokenKind::Fun) {
                if let Some(f) = self.function(first, k) {
                    let doc = self.doc_above(first);
                    let is_pub = (first..k).any(|m| matches!(self.kind(m), TokenKind::Pub));
                    let next = f.next.min(to).max(first + 1);
                    out.push(Member {
                        kind: MemberKind::Method,
                        name: f.name,
                        sig: f.sig,
                        doc,
                        line: self.line1(first),
                        end_line: self.end_line_before(first, next),
                        name_span: self.tok_span(f.name_at),
                        is_pub,
                    });
                    j = next;
                    continue;
                }
            }
            let _ = in_trait;
            j = self.skip_to_line_end(j).min(to).max(first + 1);
        }
        out
    }

    /// The fields of a `datac` (`from..to` is the inside of the parentheses).
    pub(super) fn fields(&mut self, from: usize, to: usize) -> Vec<Member> {
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
                end_line: self.end_line_before(first, j),
                name_span: self.tok_span(k),
                is_pub: false,
            });
        }
        out
    }

    /// The variants of an `enum` (`from..to` is the inside of the braces).
    pub(super) fn variants(&mut self, from: usize, to: usize) -> Vec<Member> {
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
                end_line: self.end_line_before(first, j),
                name_span: self.tok_span(first),
                is_pub: false,
            });
        }
        out
    }

    /// A `fun` declaration whose `fun` keyword is at `fun_at` and whose first
    /// token (a modifier, or the `fun` itself) is at `first`.
    pub(super) fn function(&self, first: usize, fun_at: usize) -> Option<ParsedFun> {
        let mut k = fun_at + 1;
        if matches!(self.kind(k), TokenKind::Mut) {
            k += 1;
        }
        let a = self.ident_at(k)?;
        let mut name_at = k;
        k += 1;
        if matches!(self.kind(k), TokenKind::Lt) {
            k = self.skip_angles(k);
        }
        let (name, receiver) = if matches!(self.kind(k), TokenKind::Dot) {
            name_at = k + 1;
            (self.ident_at(k + 1)?, Some(a))
        } else {
            (a, None)
        };
        let header_end = self.header_end(fun_at + 1);
        let next = self.skip_body(header_end);
        let unsafe_ = (first..fun_at).any(|m| self.is_ident(m, "unsafe"));
        Some(ParsedFun {
            name,
            name_at,
            receiver,
            sig: self.text(first, header_end),
            header_end,
            next,
            unsafe_,
        })
    }
}
