use super::*;

/// reference behave like a primary expression (`Value`); the other two are
/// themselves.
pub(crate) enum Lead {
    Value,
    Decl,
    Comment,
    Tok(TokenKind),
}

impl Lead {
    /// Does this atom end an expression? It is what decides whether a `-`, `+`,
    /// `*` or `&` after it is a binary operator or a prefix one, and whether a
    /// following `(` calls it or opens a group.
    pub(crate) fn ends_expr(&self) -> bool {
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
            ),
        }
    }
}

/// One unit the emitter copies as a whole and spaces against its neighbours.
pub(crate) struct Atom<'a> {
    pub(crate) text: &'a str,
    pub(crate) lead: Lead,
    /// Index of the first token of this atom.
    pub(crate) first: usize,
    /// Index of the first token after this atom.
    pub(crate) next: usize,
    /// Source offset the next atom's gap starts at.
    pub(crate) end: usize,
}

pub(crate) fn atom<'a>(src: &'a str, tokens: &[Token], i: usize) -> Atom<'a> {
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

    // An inline `alias ...` declaration - the whole line, verbatim.
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

pub(crate) fn emit<'a>(src: &'a str, tokens: &[Token]) -> String {
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
pub(crate) fn push_indent(out: &mut String, gap: &str, depth: usize, paren_depth: usize, from_gap: bool) {
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
pub(crate) fn is_prefix(lead: &Lead, prev_ends_expr: bool) -> bool {
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
pub(crate) fn needs_space(prev: &Lead, prev_prefix: bool, cur: &Lead, gap_ws: bool) -> bool {
    // `{}` - an empty block, never `{ }`.
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
                | TokenKind::ColonColon
                | TokenKind::Question
        )
    ) {
        return false;
    }
    // No space after `(`, `[`, `.` or `::`.
    if matches!(
        prev,
        Lead::Tok(TokenKind::LParen | TokenKind::LBracket | TokenKind::Dot | TokenKind::ColonColon)
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
    // `(`/`[` attach to a callee or index base, but open a group - with a
    // space - after an operator, `=`, `return`, or anything else that is not
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

/// True when `tokens[i]` begins an inline `alias _name = ...` declaration,
/// mirroring `preprocess::alias_decl_shape`: the keyword `alias` at the start
/// of a line, a contiguous `_name`, then `=`.
pub(crate) fn is_alias_decl(tokens: &[Token], i: usize) -> bool {
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
    let Some(TokenKind::Ident(name)) = tokens.get(i + 1).map(|t| &t.kind) else {
        return false;
    };
    if !crate::preprocess::is_alias_name(name) {
        return false;
    }
    matches!(tokens.get(i + 2).map(|t| &t.kind), Some(TokenKind::Eq))
}

/// Index of the token that ends an inline alias declaration's right-hand side,
/// mirroring `preprocess::alias_decl_shape`: a newline, a `}`, or EOF. The
/// declaration's text runs from its `alias` up to this token.
pub(crate) fn alias_decl_end(tokens: &[Token], i: usize) -> usize {
    let mut j = i + 3;
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
/// read from the input stream, not from lexing each atom in isolation - the
/// lexer folds `print!(` into one identifier, so an atom's text can lex
/// differently on its own than it does in context. Stating the rule over the
/// exact tokens is what makes it need no list of operators and miss no pair.
pub(crate) fn joins_cleanly(prev: &Atom, cur: &Atom, tokens: &[Token]) -> bool {
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
