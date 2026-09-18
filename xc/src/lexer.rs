#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    StringLit(String),
    Ident(String),

    // keywords
    Fun,
    Val,
    Var,
    Mut,
    If,
    Else,
    While,
    When,
    Return,
    True,
    False,
    Datac,

    // punctuation
    LParen,
    RParen,
    LBrace,
    RBrace,
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
            self.skip_trivia();
            let start = self.pos;

            if self.peek() == Some(b'\n') {
                let nl_start = self.pos;
                while self.peek() == Some(b'\n') {
                    self.pos += 1;
                    self.skip_trivia();
                }
                if self.paren_depth == 0 {
                    tokens.push(Token {
                        kind: TokenKind::Newline,
                        span: Span::new(nl_start, self.pos),
                    });
                }
                continue;
            }

            let Some(b) = self.peek() else {
                tokens.push(Token {
                    kind: TokenKind::Eof,
                    span: Span::new(start, start),
                });
                break;
            };

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
                    TokenKind::LBrace
                }
                b'}' => {
                    self.advance();
                    TokenKind::RBrace
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
                    let mut s = String::new();
                    loop {
                        match self.advance() {
                            Some(b'"') => break,
                            Some(b'\\') => match self.advance() {
                                Some(b'n') => s.push('\n'),
                                Some(b't') => s.push('\t'),
                                Some(b'"') => s.push('"'),
                                Some(b'\\') => s.push('\\'),
                                Some(c) => s.push(c as char),
                                None => {
                                    return Err(LexError {
                                        message: "unterminated string literal".to_string(),
                                        span: Span::new(start, self.pos),
                                    })
                                }
                            },
                            Some(c) => s.push(c as char),
                            None => {
                                return Err(LexError {
                                    message: "unterminated string literal".to_string(),
                                    span: Span::new(start, self.pos),
                                })
                            }
                        }
                    }
                    TokenKind::StringLit(s)
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
                        "val" => TokenKind::Val,
                        "var" => TokenKind::Var,
                        "mut" => TokenKind::Mut,
                        "if" => TokenKind::If,
                        "else" => TokenKind::Else,
                        "while" => TokenKind::While,
                        "when" => TokenKind::When,
                        "return" => TokenKind::Return,
                        "true" => TokenKind::True,
                        "false" => TokenKind::False,
                        "datac" => TokenKind::Datac,
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

            tokens.push(Token {
                kind,
                span: Span::new(start, self.pos),
            });
        }
        Ok(tokens)
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
}
