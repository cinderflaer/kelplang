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
    Fn,
    Let,
    Mut,
    If,
    Else,
    While,
    Return,
    True,
    False,
    Struct,

    // punctuation
    LParen,
    RParen,
    LBrace,
    RBrace,
    Comma,
    Colon,
    Semi,
    Arrow, // ->
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
    AndAnd,
    OrOr,
    Bang,
    Amp,
    Dot,

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

    fn skip_trivia(&mut self) {
        loop {
            match self.peek() {
                Some(b' ') | Some(b'\t') | Some(b'\r') | Some(b'\n') => {
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
                    TokenKind::LParen
                }
                b')' => {
                    self.advance();
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
                b';' => {
                    self.advance();
                    TokenKind::Semi
                }
                b'.' => {
                    self.advance();
                    TokenKind::Dot
                }
                b'+' => {
                    self.advance();
                    TokenKind::Plus
                }
                b'*' => {
                    self.advance();
                    TokenKind::Star
                }
                b'/' => {
                    self.advance();
                    TokenKind::Slash
                }
                b'%' => {
                    self.advance();
                    TokenKind::Percent
                }
                b'-' => {
                    self.advance();
                    if self.peek() == Some(b'>') {
                        self.advance();
                        TokenKind::Arrow
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
                    let text = &self.src[start..self.pos];
                    match text {
                        "fn" => TokenKind::Fn,
                        "let" => TokenKind::Let,
                        "mut" => TokenKind::Mut,
                        "if" => TokenKind::If,
                        "else" => TokenKind::Else,
                        "while" => TokenKind::While,
                        "return" => TokenKind::Return,
                        "true" => TokenKind::True,
                        "false" => TokenKind::False,
                        "struct" => TokenKind::Struct,
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
            kinds("fn let mut if else while return true false struct foo"),
            vec![
                TokenKind::Fn,
                TokenKind::Let,
                TokenKind::Mut,
                TokenKind::If,
                TokenKind::Else,
                TokenKind::While,
                TokenKind::Return,
                TokenKind::True,
                TokenKind::False,
                TokenKind::Struct,
                TokenKind::Ident("foo".to_string()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn lexes_operators() {
        assert_eq!(
            kinds("-> == != <= >= && || + - * / % < > = ! &"),
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
            vec![TokenKind::IntLit(1), TokenKind::IntLit(2), TokenKind::Eof]
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
