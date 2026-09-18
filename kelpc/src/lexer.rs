use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        Span { start, end }
    }

    pub fn to(self, other: Span) -> Span {
        Span::new(self.start, other.end)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // literals
    IntLit(i64),
    FloatLit(f64),
    /// A string with no `$` interpolation in it, stored decoded (escapes
    /// already processed). A string *with* interpolation becomes the
    /// `TemplateStart` ... `TemplateEnd` sequence below instead, so every
    /// existing string in every existing test keeps this exact token.
    StringLit(String),
    Ident(String),

    // string-template pieces (`"a$b${c}"`), all emitted in one flat run:
    // `TemplateStart StrChunk("a") ExprStart Ident(b) ExprEnd
    //  StrChunk("") ExprStart <expr tokens> ExprEnd StrChunk("") TemplateEnd`.
    // Splitting templates in the lexer is what keeps the sub-expressions'
    // spans *absolute* (no offset arithmetic to get `file:line:col` right)
    // and lets a template nest inside its own `${...}`.
    TemplateStart,
    /// Literal text between interpolations. Always present, possibly empty,
    /// and always one before `TemplateEnd`, so the shape is regular.
    StrChunk(String),
    TemplateExprStart,
    TemplateExprEnd,
    TemplateEnd,

    // keywords
    Fun,
    /// `closure(i32).R: i32` — the type of a `fun` literal that captures
    /// (A-4, phase E): a function *value* that carries an environment, so it
    /// is owned (not Copy) and drops that environment. A keyword rather than
    /// a contextual name, for the reason `fun` is one: the spelling is a type
    /// constructor, and a program that named a variable `closure` would
    /// shadow a type name.
    Closure,
    Val,
    Var,
    Mut,
    /// `fun consume(s: move String)` — a parameter that takes ownership of
    /// its argument (A-4), the counterpart of `mut T`'s exclusive borrow.
    Move,
    If,
    Else,
    While,
    When,
    Return,
    True,
    False,
    Datac,
    /// `enum Shape { Circle(val r: f64) Empty }` — a tagged union
    /// (`plan/syntax.md` §9.2): exactly one variant is live, and `when` on a
    /// scrutinee matches it (A-1).
    Enum,
    /// `pd val name = expr` / `pd var name = expr` — a passdown
    /// (`plan/syntax.md` §9).
    Pd,
    /// `impl "path/to/module"` — the import keyword (`plan/syntax.md` §8;
    /// `impl` method blocks do not exist, receivers replaced them).
    Impl,
    /// `impl "path" as alias`.
    As,
    /// `const NAME: T = expr` — a top-level compile-time constant (M6.3).
    Const,
    /// `extern fun name(...).R: T` — a declaration whose body the compiler
    /// supplies (M6.2). Only compiler-shipped stdlib modules may use it: the
    /// whole point is to confine the unportable primitive (a syscall, a
    /// `malloc`) to a body that is not `.kp` source.
    Extern,

    // punctuation
    LParen,
    RParen,
    LBrace,
    RBrace,
    // `[`/`]` are not used by any expression yet — they exist so M7's
    // `with caps(fs = ["..."])` grant lists lex (and so the parser can
    // reject them with a real message instead of a lex error).
    LBracket,
    RBracket,
    Comma,
    Colon,
    Dot,
    Arrow, // -> (reserved for `when` arms)
    Newline,

    // operators
    Eq,
    EqEq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    PercentEq,
    AndAnd,
    OrOr,
    Bang,
    Amp,
    /// `expr?` — propagate a failure out of the enclosing function
    /// (MK5-S6-M1). Postfix, so it is only ever read after an expression.
    Question,

    Eof,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

pub struct Lexer<'a> {
    src: &'a str,
    bytes: &'a [u8],
    pos: usize,
    /// Depth of unclosed `(`. Newlines are insignificant (not tokenized)
    /// while this is > 0 — this is what lets a `datac`'s field-list parens,
    /// or any parenthesized sub-expression, span multiple lines.
    paren_depth: usize,
    /// The `paren_depth` in force when each unclosed `{` was opened. A block
    /// *inside* parens is where the rule above stops: `{` opens a statement
    /// block, and statements are separated by newlines, so those are tokenized
    /// again however deep the parens are — which is what lets a `fun` literal
    /// with a multi-statement body be written as a call argument
    /// (`f(fun(x: i32) { a\n b\n })`). A function body's own braces are at
    /// paren depth 0, so a wrapped argument list inside it still ignores
    /// newlines (A-4 phase E).
    brace_parens: Vec<usize>,
    /// Tokens produced by a single source construct but consumed one at a
    /// time — only string templates use this: one `"..."` literal expands to
    /// a whole token run, and `next_token` drains the run before reading
    /// more source.
    pending: VecDeque<Token>,
}

#[derive(Debug)]
pub struct LexError {
    pub message: String,
    pub span: Span,
}

impl<'a> Lexer<'a> {
    pub fn new(src: &'a str) -> Self {
        Lexer {
            src,
            bytes: src.as_bytes(),
            pos: 0,
            paren_depth: 0,
            brace_parens: Vec::new(),
            pending: VecDeque::new(),
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<u8> {
        self.bytes.get(self.pos + offset).copied()
    }

    fn advance(&mut self) -> Option<u8> {
        let b = self.peek()?;
        self.pos += 1;
        Some(b)
    }

    /// Skips spaces/tabs/comments (never newlines — those are handled by
    /// the caller, since whether they're significant depends on
    /// `paren_depth`).
    fn skip_trivia(&mut self) {
        loop {
            match self.peek() {
                Some(b' ') | Some(b'\t') | Some(b'\r') => {
                    self.pos += 1;
                }
                Some(b'/') if self.peek_at(1) == Some(b'/') => {
                    while let Some(b) = self.peek() {
                        if b == b'\n' {
                            break;
                        }
                        self.pos += 1;
                    }
                }
                _ => break,
            }
        }
    }

    pub fn tokenize(mut self) -> Result<Vec<Token>, LexError> {
        let mut tokens = Vec::new();
        loop {
            match self.next_token()? {
                Some(t) => tokens.push(t),
                None => {
                    tokens.push(Token {
                        kind: TokenKind::Eof,
                        span: Span::new(self.pos, self.pos),
                    });
                    break;
                }
            }
        }
        Ok(tokens)
    }

    /// The next token, or `None` at end of input. Newline tokens are
    /// *returned* (at `paren_depth == 0`) rather than skipped, since
    /// statement termination depends on them.
    fn next_token(&mut self) -> Result<Option<Token>, LexError> {
        loop {
            // A single string literal can expand to a whole run of tokens
            // (a template); those queue up here and drain first.
            if let Some(t) = self.pending.pop_front() {
                return Ok(Some(t));
            }
            self.skip_trivia();
            let start = self.pos;

            if self.peek() == Some(b'\n') {
                let nl_start = self.pos;
                while self.peek() == Some(b'\n') {
                    self.pos += 1;
                    self.skip_trivia();
                }
                // Significant at the top level, and inside a block whose own
                // braces were opened inside parens — otherwise parens win.
                let in_paren_block = self
                    .brace_parens
                    .last()
                    .is_some_and(|&depth| depth > 0);
                if self.paren_depth == 0 || in_paren_block {
                    return Ok(Some(Token {
                        kind: TokenKind::Newline,
                        span: Span::new(nl_start, self.pos),
                    }));
                }
                continue;
            }

            let Some(b) = self.peek() else {
                return Ok(None);
            };

            if b == b'"' {
                self.advance();
                self.lex_string(start)?;
                continue; // the tokens (one, or a template's run) are queued
            }

            let kind = match b {
                b'(' => {
                    self.advance();
                    self.paren_depth += 1;
                    TokenKind::LParen
                }
                b')' => {
                    self.advance();
                    self.paren_depth = self.paren_depth.saturating_sub(1);
                    TokenKind::RParen
                }
                b'{' => {
                    self.advance();
                    self.brace_parens.push(self.paren_depth);
                    TokenKind::LBrace
                }
                b'}' => {
                    self.advance();
                    self.brace_parens.pop();
                    TokenKind::RBrace
                }
                b'[' => {
                    self.advance();
                    TokenKind::LBracket
                }
                b']' => {
                    self.advance();
                    TokenKind::RBracket
                }
                b',' => {
                    self.advance();
                    TokenKind::Comma
                }
                b':' => {
                    self.advance();
                    TokenKind::Colon
                }
                b'.' => {
                    self.advance();
                    TokenKind::Dot
                }
                b'+' => {
                    self.advance();
                    if self.peek() == Some(b'=') {
                        self.advance();
                        TokenKind::PlusEq
                    } else {
                        TokenKind::Plus
                    }
                }
                b'*' => {
                    self.advance();
                    if self.peek() == Some(b'=') {
                        self.advance();
                        TokenKind::StarEq
                    } else {
                        TokenKind::Star
                    }
                }
                b'/' => {
                    self.advance();
                    if self.peek() == Some(b'=') {
                        self.advance();
                        TokenKind::SlashEq
                    } else {
                        TokenKind::Slash
                    }
                }
                b'%' => {
                    self.advance();
                    if self.peek() == Some(b'=') {
                        self.advance();
                        TokenKind::PercentEq
                    } else {
                        TokenKind::Percent
                    }
                }
                b'-' => {
                    self.advance();
                    if self.peek() == Some(b'>') {
                        self.advance();
                        TokenKind::Arrow
                    } else if self.peek() == Some(b'=') {
                        self.advance();
                        TokenKind::MinusEq
                    } else {
                        TokenKind::Minus
                    }
                }
                b'=' => {
                    self.advance();
                    if self.peek() == Some(b'=') {
                        self.advance();
                        TokenKind::EqEq
                    } else {
                        TokenKind::Eq
                    }
                }
                b'!' => {
                    self.advance();
                    if self.peek() == Some(b'=') {
                        self.advance();
                        TokenKind::NotEq
                    } else {
                        TokenKind::Bang
                    }
                }
                b'<' => {
                    self.advance();
                    if self.peek() == Some(b'=') {
                        self.advance();
                        TokenKind::LtEq
                    } else {
                        TokenKind::Lt
                    }
                }
                b'>' => {
                    self.advance();
                    if self.peek() == Some(b'=') {
                        self.advance();
                        TokenKind::GtEq
                    } else {
                        TokenKind::Gt
                    }
                }
                b'&' => {
                    self.advance();
                    if self.peek() == Some(b'&') {
                        self.advance();
                        TokenKind::AndAnd
                    } else {
                        TokenKind::Amp
                    }
                }
                b'?' => {
                    self.advance();
                    TokenKind::Question
                }
                b'|' => {
                    self.advance();
                    if self.peek() == Some(b'|') {
                        self.advance();
                        TokenKind::OrOr
                    } else {
                        return Err(LexError {
                            message: "unexpected character '|'".to_string(),
                            span: Span::new(start, self.pos),
                        });
                    }
                }
                b'"' => {
                    self.advance();
                    self.lex_string(start)?;
                    continue;
                }
                b'0'..=b'9' => {
                    while matches!(self.peek(), Some(b'0'..=b'9')) {
                        self.pos += 1;
                    }
                    let mut is_float = false;
                    if self.peek() == Some(b'.') && matches!(self.peek_at(1), Some(b'0'..=b'9')) {
                        is_float = true;
                        self.pos += 1;
                        while matches!(self.peek(), Some(b'0'..=b'9')) {
                            self.pos += 1;
                        }
                    }
                    let text = &self.src[start..self.pos];
                    if is_float {
                        TokenKind::FloatLit(text.parse().map_err(|_| LexError {
                            message: format!("invalid float literal '{text}'"),
                            span: Span::new(start, self.pos),
                        })?)
                    } else {
                        TokenKind::IntLit(text.parse().map_err(|_| LexError {
                            message: format!("invalid integer literal '{text}'"),
                            span: Span::new(start, self.pos),
                        })?)
                    }
                }
                c if c.is_ascii_alphabetic() || c == b'_' => {
                    while matches!(self.peek(), Some(c) if c.is_ascii_alphanumeric() || c == b'_')
                    {
                        self.pos += 1;
                    }
                    // Fold the specific adjacent shape `ident!(` into one
                    // identifier token (covers `print!`), rather than
                    // merging `!` into identifiers generally — that would
                    // wrongly swallow `!=` when written without a space
                    // (`x!=y`).
                    if self.peek() == Some(b'!') && self.peek_at(1) == Some(b'(') {
                        self.pos += 1;
                    }
                    let text = &self.src[start..self.pos];
                    match text {
                        "fun" => TokenKind::Fun,
                        "closure" => TokenKind::Closure,
                        "val" => TokenKind::Val,
                        "var" => TokenKind::Var,
                        "mut" => TokenKind::Mut,
                        "move" => TokenKind::Move,
                        "if" => TokenKind::If,
                        "else" => TokenKind::Else,
                        "while" => TokenKind::While,
                        "when" => TokenKind::When,
                        "return" => TokenKind::Return,
                        "true" => TokenKind::True,
                        "false" => TokenKind::False,
                        "datac" => TokenKind::Datac,
                        "enum" => TokenKind::Enum,
                        "pd" => TokenKind::Pd,
                        "impl" => TokenKind::Impl,
                        "as" => TokenKind::As,
                        "const" => TokenKind::Const,
                        "extern" => TokenKind::Extern,
                        _ => TokenKind::Ident(text.to_string()),
                    }
                }
                _ => {
                    return Err(LexError {
                        message: format!("unexpected character '{}'", b as char),
                        span: Span::new(start, self.pos + 1),
                    });
                }
            };

            return Ok(Some(Token {
                kind,
                span: Span::new(start, self.pos),
            }));
        }
    }

    /// Lexes a string literal, having consumed the opening quote; `start` is
    /// that quote's offset.
    ///
    /// Without interpolation this queues the single `StringLit` token it
    /// always did. With interpolation it queues the flat template run
    /// (`TemplateStart StrChunk ... TemplateEnd`), lexing each `${...}`'s
    /// tokens through `next_token` — which is what makes nested strings,
    /// nested templates, and absolute spans work with no offset math.
    fn lex_string(&mut self, start: usize) -> Result<(), LexError> {
        let mut text = String::new();
        let mut chunk_start = self.pos;
        let mut parts: Vec<Token> = Vec::new();
        let mut is_template = false;

        loop {
            match self.advance() {
                Some(b'"') => break,
                Some(b'\\') => match self.advance() {
                    Some(b'n') => text.push('\n'),
                    Some(b't') => text.push('\t'),
                    // CR is not decoration: every text protocol ends its lines
                    // with CRLF, and before this arm existed `"\r\n"` decoded to
                    // the letter `r` followed by a newline (the fallback below),
                    // so an HTTP request came out malformed with no diagnostic.
                    Some(b'r') => text.push('\r'),
                    Some(b'"') => text.push('"'),
                    Some(b'\\') => text.push('\\'),
                    // Anything else is an error rather than the bare character:
                    // passing it through made `"\q"` mean `"q"`, so a typo was
                    // silently a different string. `\0` is rejected for a second
                    // reason — a `str` is NUL-terminated and could not hold one.
                    Some(c) => {
                        return Err(LexError {
                            message: format!(
                                "unknown escape '\\{}' in a string literal — the escapes are \\n, \\t, \\r, \\\" and \\\\",
                                c as char
                            ),
                            span: Span::new(start, self.pos),
                        })
                    }
                    None => {
                        return Err(LexError {
                            message: "unterminated string literal".to_string(),
                            span: Span::new(start, self.pos),
                        })
                    }
                },
                Some(b'$') if self.at_interpolation() => {
                    if !is_template {
                        is_template = true;
                        parts.push(Token {
                            kind: TokenKind::TemplateStart,
                            span: Span::new(start, start + 1),
                        });
                    }
                    parts.push(Token {
                        kind: TokenKind::StrChunk(std::mem::take(&mut text)),
                        span: Span::new(chunk_start, self.pos - 1),
                    });
                    if self.peek() == Some(b'{') {
                        let brace = self.pos;
                        self.advance();
                        parts.push(Token {
                            kind: TokenKind::TemplateExprStart,
                            span: Span::new(brace, brace + 2),
                        });
                        let mut depth = 0usize;
                        loop {
                            let Some(tok) = self.next_token()? else {
                                return Err(LexError {
                                    message: "unterminated '${' in string template".to_string(),
                                    span: Span::new(brace, self.pos),
                                });
                            };
                            match &tok.kind {
                                // Newlines inside `${...}` are insignificant:
                                // a template may wrap a multi-line expression.
                                TokenKind::Newline => continue,
                                TokenKind::LBrace => depth += 1,
                                TokenKind::RBrace if depth == 0 => break,
                                TokenKind::RBrace => depth -= 1,
                                _ => {}
                            }
                            parts.push(tok);
                        }
                        let close = self.pos - 1;
                        parts.push(Token {
                            kind: TokenKind::TemplateExprEnd,
                            span: Span::new(close, close + 1),
                        });
                    } else {
                        // `$name` — an identifier, nothing more (use
                        // `${name.field}` for anything compound). Wrapped in
                        // the same start/end pair as `${...}` so the parser
                        // has one uniform shape to read.
                        let ident_start = self.pos;
                        while matches!(self.peek(), Some(c) if c == b'_' || c.is_ascii_alphanumeric()) {
                            self.pos += 1;
                        }
                        let name = &self.src[ident_start..self.pos];
                        parts.push(Token {
                            kind: TokenKind::TemplateExprStart,
                            span: Span::new(ident_start - 1, ident_start),
                        });
                        parts.push(Token {
                            kind: TokenKind::Ident(name.to_string()),
                            span: Span::new(ident_start, self.pos),
                        });
                        parts.push(Token {
                            kind: TokenKind::TemplateExprEnd,
                            span: Span::new(self.pos, self.pos),
                        });
                    }
                    chunk_start = self.pos;
                }
                Some(c) if c < 0x80 => text.push(c as char),
                Some(c) => {
                    // A non-ASCII byte starts a multi-byte UTF-8 sequence.
                    // `src` is UTF-8, so reassemble the character instead of
                    // pushing each byte as its own Latin-1 scalar — the
                    // latter re-encodes as mojibake and inflates the
                    // literal's byte length, which `String.len` would then
                    // report.
                    let first = self.pos - 1;
                    let width = match c {
                        0xC2..=0xDF => 2,
                        0xE0..=0xEF => 3,
                        0xF0..=0xF4 => 4,
                        _ => {
                            return Err(LexError {
                                message: "string literal is not valid utf-8".to_string(),
                                span: Span::new(first, self.pos),
                            })
                        }
                    };
                    while self.pos < first + width {
                        match self.peek() {
                            Some(b) if (b & 0xC0) == 0x80 => self.pos += 1,
                            _ => {
                                return Err(LexError {
                                    message: "string literal is not valid utf-8".to_string(),
                                    span: Span::new(first, self.pos),
                                })
                            }
                        }
                    }
                    match self.src.get(first..first + width) {
                        Some(s) => text.push_str(s),
                        None => {
                            return Err(LexError {
                                message: "string literal is not valid utf-8".to_string(),
                                span: Span::new(first, self.pos),
                            })
                        }
                    }
                }
                None => {
                    return Err(LexError {
                        message: "unterminated string literal".to_string(),
                        span: Span::new(start, self.pos),
                    })
                }
            }
        }

        if is_template {
            parts.push(Token {
                kind: TokenKind::StrChunk(text),
                span: Span::new(chunk_start, self.pos - 1),
            });
            parts.push(Token {
                kind: TokenKind::TemplateEnd,
                span: Span::new(self.pos - 1, self.pos),
            });
            self.pending.extend(parts);
        } else {
            self.pending.push_back(Token {
                kind: TokenKind::StringLit(text),
                span: Span::new(start, self.pos),
            });
        }
        Ok(())
    }

    /// True right after a `$` that starts an interpolation: `$name` or
    /// `${expr}`. A `$` followed by anything else (a space, a digit, end of
    /// string) stays literal text, so `"cost $5"` needs no escaping.
    fn at_interpolation(&self) -> bool {
        match self.peek() {
            Some(b'{') => true,
            Some(c) => c == b'_' || c.is_ascii_alphabetic(),
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<TokenKind> {
        Lexer::new(src)
            .tokenize()
            .expect("lex failed")
            .into_iter()
            .map(|t| t.kind)
            .collect()
    }

    #[test]
    fn lexes_keywords_and_idents() {
        assert_eq!(
            kinds("fun val var mut if else while when return true false datac foo"),
            vec![
                TokenKind::Fun,
                TokenKind::Val,
                TokenKind::Var,
                TokenKind::Mut,
                TokenKind::If,
                TokenKind::Else,
                TokenKind::While,
                TokenKind::When,
                TokenKind::Return,
                TokenKind::True,
                TokenKind::False,
                TokenKind::Datac,
                TokenKind::Ident("foo".to_string()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn lexes_operators_including_compound_assign() {
        assert_eq!(
            kinds("-> == != <= >= && || + - * / % < > = ! & += -= *= /= %="),
            vec![
                TokenKind::Arrow,
                TokenKind::EqEq,
                TokenKind::NotEq,
                TokenKind::LtEq,
                TokenKind::GtEq,
                TokenKind::AndAnd,
                TokenKind::OrOr,
                TokenKind::Plus,
                TokenKind::Minus,
                TokenKind::Star,
                TokenKind::Slash,
                TokenKind::Percent,
                TokenKind::Lt,
                TokenKind::Gt,
                TokenKind::Eq,
                TokenKind::Bang,
                TokenKind::Amp,
                TokenKind::PlusEq,
                TokenKind::MinusEq,
                TokenKind::StarEq,
                TokenKind::SlashEq,
                TokenKind::PercentEq,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn lexes_literals() {
        assert_eq!(
            kinds(r#"42 3.14 "hi\n""#),
            vec![
                TokenKind::IntLit(42),
                TokenKind::FloatLit(3.14),
                TokenKind::StringLit("hi\n".to_string()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn decodes_multibyte_utf8_in_string_literals() {
        // The lexer is byte-oriented, so a literal used to push each byte of a
        // multi-byte character as its own Latin-1 scalar — `"héllo"` came out
        // as `"hÃ©llo"`, and `String.len` then reported 8 bytes for 6. The
        // character has to survive verbatim.
        assert_eq!(
            kinds("\"héllo\" \"→\""),
            vec![
                TokenKind::StringLit("héllo".to_string()),
                TokenKind::StringLit("→".to_string()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn skips_line_comments() {
        assert_eq!(
            kinds("1 // this is a comment\n2"),
            vec![
                TokenKind::IntLit(1),
                TokenKind::Newline,
                TokenKind::IntLit(2),
                TokenKind::Eof
            ]
        );
    }

    #[test]
    fn newline_is_significant_at_top_level() {
        assert_eq!(
            kinds("1\n2"),
            vec![
                TokenKind::IntLit(1),
                TokenKind::Newline,
                TokenKind::IntLit(2),
                TokenKind::Eof
            ]
        );
    }

    #[test]
    fn consecutive_newlines_collapse_to_one_token() {
        assert_eq!(
            kinds("1\n\n\n2"),
            vec![
                TokenKind::IntLit(1),
                TokenKind::Newline,
                TokenKind::IntLit(2),
                TokenKind::Eof
            ]
        );
    }

    #[test]
    fn newline_is_insignificant_inside_parens() {
        assert_eq!(
            kinds("f(1,\n2)"),
            vec![
                TokenKind::Ident("f".to_string()),
                TokenKind::LParen,
                TokenKind::IntLit(1),
                TokenKind::Comma,
                TokenKind::IntLit(2),
                TokenKind::RParen,
                TokenKind::Eof
            ]
        );
    }

    #[test]
    fn folds_bang_paren_into_macro_like_ident() {
        assert_eq!(
            kinds("print!(1)"),
            vec![
                TokenKind::Ident("print!".to_string()),
                TokenKind::LParen,
                TokenKind::IntLit(1),
                TokenKind::RParen,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn does_not_fold_not_equal_without_spaces() {
        assert_eq!(
            kinds("x!=y"),
            vec![
                TokenKind::Ident("x".to_string()),
                TokenKind::NotEq,
                TokenKind::Ident("y".to_string()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn tracks_spans() {
        let tokens = Lexer::new("abc").tokenize().unwrap();
        assert_eq!(tokens[0].span, Span::new(0, 3));
    }

    #[test]
    fn rejects_unterminated_string() {
        assert!(Lexer::new("\"abc").tokenize().is_err());
    }

    #[test]
    fn rejects_unknown_character() {
        assert!(Lexer::new("$").tokenize().is_err());
    }

    #[test]
    fn plain_strings_are_unchanged_by_templates_existing() {
        // No `$`: the single `StringLit` token every earlier milestone relied
        // on, same span covering both quotes.
        let tokens = Lexer::new("\"cost $5\"").tokenize().unwrap();
        assert_eq!(tokens[0].kind, TokenKind::StringLit("cost $5".to_string()));
        assert_eq!(tokens[0].span, Span::new(0, 9));
    }

    #[test]
    fn lexes_a_name_interpolation() {
        assert_eq!(
            kinds("\"hi $name!\""),
            vec![
                TokenKind::TemplateStart,
                TokenKind::StrChunk("hi ".to_string()),
                TokenKind::TemplateExprStart,
                TokenKind::Ident("name".to_string()),
                TokenKind::TemplateExprEnd,
                TokenKind::StrChunk("!".to_string()),
                TokenKind::TemplateEnd,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn lexes_an_expression_interpolation() {
        assert_eq!(
            kinds("\"${c.area()}\""),
            vec![
                TokenKind::TemplateStart,
                TokenKind::StrChunk(String::new()),
                TokenKind::TemplateExprStart,
                TokenKind::Ident("c".to_string()),
                TokenKind::Dot,
                TokenKind::Ident("area".to_string()),
                TokenKind::LParen,
                TokenKind::RParen,
                TokenKind::TemplateExprEnd,
                TokenKind::StrChunk(String::new()),
                TokenKind::TemplateEnd,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn a_dollar_that_starts_nothing_stays_literal() {
        assert_eq!(
            kinds("\"$5 and $ \""),
            vec![TokenKind::StringLit("$5 and $ ".to_string()), TokenKind::Eof]
        );
    }

    #[test]
    fn braces_inside_an_interpolation_do_not_end_it() {
        assert_eq!(
            kinds("\"${ if x { 1 } else { 2 } }\""),
            vec![
                TokenKind::TemplateStart,
                TokenKind::StrChunk(String::new()),
                TokenKind::TemplateExprStart,
                TokenKind::If,
                TokenKind::Ident("x".to_string()),
                TokenKind::LBrace,
                TokenKind::IntLit(1),
                TokenKind::RBrace,
                TokenKind::Else,
                TokenKind::LBrace,
                TokenKind::IntLit(2),
                TokenKind::RBrace,
                TokenKind::TemplateExprEnd,
                TokenKind::StrChunk(String::new()),
                TokenKind::TemplateEnd,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn a_template_can_nest_inside_its_own_interpolation() {
        assert_eq!(
            kinds("\"[${\"in $x\"}]\""),
            vec![
                TokenKind::TemplateStart,
                TokenKind::StrChunk("[".to_string()),
                TokenKind::TemplateExprStart,
                TokenKind::TemplateStart,
                TokenKind::StrChunk("in ".to_string()),
                TokenKind::TemplateExprStart,
                TokenKind::Ident("x".to_string()),
                TokenKind::TemplateExprEnd,
                TokenKind::StrChunk(String::new()),
                TokenKind::TemplateEnd,
                TokenKind::TemplateExprEnd,
                TokenKind::StrChunk("]".to_string()),
                TokenKind::TemplateEnd,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn template_sub_expression_spans_are_absolute() {
        // `"ab $xy"`: the `xy` ident must carry its own offsets in the
        // original source (5..7), not offsets inside the literal text.
        let tokens = Lexer::new("\"ab $xy\"").tokenize().unwrap();
        assert_eq!(tokens[3].kind, TokenKind::Ident("xy".to_string()));
        assert_eq!(tokens[3].span, Span::new(5, 7));
    }

    #[test]
    fn rejects_an_unterminated_interpolation() {
        assert!(Lexer::new("\"${x\"").tokenize().is_err());
    }
}
