use std::collections::VecDeque;


impl<'a> Lexer<'a> {
    pub fn new(src: &'a str) -> Self {
        Lexer {
            src,
            bytes: src.as_bytes(),
            pos: 0,
            paren_depth: 0,
            brace_parens: Vec::new(),
            pending: VecDeque::new(),
            keep_comments: false,
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

    /// Skips spaces, tabs and carriage returns - the trivia a token never
    /// starts with. A newline is *not* skipped: whether it is significant
    /// depends on `paren_depth`, so the caller handles it.
    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ') | Some(b'\t') | Some(b'\r')) {
            self.pos += 1;
        }
    }

    /// Consumes `//...` up to (not including) the line terminator. A trailing
    /// CR is part of the terminator, not the comment, so a CRLF file does not
    /// carry a stray CR into the comment's text or span.
    fn consume_comment(&mut self) {
        while let Some(b) = self.peek() {
            if b == b'\n' || b == b'\r' {
                break;
            }
            self.pos += 1;
        }
    }

    /// Consumes a `/* ... */` comment, delimiters included.
    ///
    /// **Block comments nest**, like Kotlin's and Rust's rather than like C's:
    /// an inner `/*` needs its own `*/`, so commenting out a region that
    /// already contains a block comment works. The alternative - C's rule -
    /// ends the comment at the *first* `*/`, so the rest of the region becomes
    /// code and the failure is a parse error somewhere else entirely. The cost
    /// of nesting is that an extra `*/` no longer "closes twice" silently: an
    /// unbalanced `/*` is an unterminated comment, reported at its own start.
    ///
    /// A `//` inside a block comment is just text, and a `/*` inside a line
    /// comment is too - the caller decides which delimiters apply.
    fn consume_block_comment(&mut self) -> Result<(), LexError> {
        let start = self.pos;
        self.pos += 2; // the opening `/*`, already checked by the caller
        let mut depth = 1usize;
        while let Some(b) = self.peek() {
            if b == b'/' && self.peek_at(1) == Some(b'*') {
                depth += 1;
                self.pos += 2;
            } else if b == b'*' && self.peek_at(1) == Some(b'/') {
                depth -= 1;
                self.pos += 2;
                if depth == 0 {
                    return Ok(());
                }
            } else {
                self.pos += 1;
            }
        }
        Err(LexError {
            message: "unterminated block comment: this '/*' has no matching '*/' ".to_string()
                + "(block comments nest, so every inner '/*' needs its own '*/')",
            span: Span::new(start, self.pos),
        })
    }

    /// Consumes a comment in whichever form starts here, or does nothing when
    /// none does. Returns `true` when one was consumed.
    fn consume_any_comment(&mut self) -> Result<bool, LexError> {
        if self.peek() != Some(b'/') {
            return Ok(false);
        }
        match self.peek_at(1) {
            Some(b'/') => {
                let start = self.pos;
                self.consume_comment();
                self.refuse_a_trailing_doc_comment(start)?;
                Ok(true)
            }
            Some(b'*') => {
                self.consume_block_comment()?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// A `//!` opens a documentation block, so it has to start a line: the
    /// block form is what makes a run of them one foldable unit above the
    /// declaration it documents. A `//!` with code before it on the same line
    /// is inside a statement, so it documents nothing and is reliably a
    /// mistake; `//` is the spelling for a note beside code.
    fn refuse_a_trailing_doc_comment(&self, start: usize) -> Result<(), LexError> {
        if !self.src[start..].starts_with("//!") {
            return Ok(());
        }
        let line_start = self.src[..start].rfind('\n').map_or(0, |i| i + 1);
        if self.src[line_start..start]
            .bytes()
            .all(|b| matches!(b, b' ' | b'\t' | b'\r'))
        {
            return Ok(());
        }
        Err(LexError {
            message: "'//!' is a documentation comment and must start a line: a '//!' after \
                      code on the same line documents nothing and is refused; write '//' for a \
                      note beside the code"
                .to_string(),
            span: Span::new(start, self.pos),
        })
    }

    /// Skips spaces/tabs/CRs and comments (never newlines: those are handled
    /// by the caller, since whether they're significant depends on
    /// `paren_depth`).
    fn skip_trivia(&mut self) -> Result<(), LexError> {
        loop {
            self.skip_ws();
            if !self.consume_any_comment()? {
                break;
            }
        }
        Ok(())
    }

    /// Lexes the whole source. Comments are skipped, exactly as they always
    /// have been - the parser's token stream is this one, and it does not
    /// change.
    pub fn tokenize(mut self) -> Result<Vec<Token>, LexError> {
        self.keep_comments = false;
        self.run()
    }

    /// `tokenize`, but comments are returned as `TokenKind::Comment`
    /// tokens instead of being skipped, and a run of newlines stops at one.
    /// Everything else - token text, spans, the collapse of a blank-line run
    /// into a single `Newline` - is identical, which is what lets the
    /// formatter reconstruct the source while nothing else in the compiler
    /// changes.
    pub fn tokenize_with_trivia(mut self) -> Result<Vec<Token>, LexError> {
        self.keep_comments = true;
        self.run()
    }

    fn run(&mut self) -> Result<Vec<Token>, LexError> {
        let mut tokens: Vec<Token> = Vec::new();
        loop {
            match self.next_token()? {
                // A line that ends in an operator that needs a right-hand side
                // (or in `,` or `.`) goes on to the next line: the newline is
                // not a statement end. The formatter's token stream keeps the
                // newline, so it can reproduce the source as written.
                Some(t)
                    if !self.keep_comments
                        && t.kind == TokenKind::Newline
                        && tokens.last().is_some_and(|p| continues_line(&p.kind)) => {}
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
            if self.keep_comments {
                self.skip_ws();
                // One comment token per comment, delimiters included, so the
                // formatter can copy the text verbatim. A block comment's
                // newlines are part of that text, which is why the newline run
                // below cannot see them.
                let opens_comment = self.peek() == Some(b'/')
                    && matches!(self.peek_at(1), Some(b'/') | Some(b'*'));
                if opens_comment {
                    let start = self.pos;
                    self.consume_any_comment()?;
                    return Ok(Some(Token {
                        kind: TokenKind::Comment(self.src[start..self.pos].to_string()),
                        span: Span::new(start, self.pos),
                    }));
                }
            } else {
                self.skip_trivia()?;
            }
            let start = self.pos;

            if self.peek() == Some(b'\n') {
                let nl_start = self.pos;
                while self.peek() == Some(b'\n') {
                    self.pos += 1;
                    if self.keep_comments {
                        self.skip_ws();
                    } else {
                        self.skip_trivia()?;
                    }
                }
                // Significant at the top level, and inside a block whose own
                // braces were opened inside parens - otherwise parens win.
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
                b';' => {
                    self.advance();
                    TokenKind::Semi
                }
                b':' => {
                    self.advance();
                    if self.peek() == Some(b':') {
                        self.advance();
                        TokenKind::ColonColon
                    } else {
                        TokenKind::Colon
                    }
                }
                b'.' => {
                    self.advance();
                    // `...` is one token: the variadic marker in a user
                    // `extern fun`'s parameter list. A lone `.` (a field or
                    // method access) and the `..` a `caps` pattern hides
                    // inside a string literal are unchanged, so the three
                    // dots are read together and one or two are left alone.
                    if self.peek() == Some(b'.') && self.peek_at(1) == Some(b'.') {
                        self.advance();
                        self.advance();
                        TokenKind::DotDotDot
                    } else {
                        TokenKind::Dot
                    }
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
                    } else if self.peek() == Some(b'>') {
                        self.advance();
                        TokenKind::Arrow
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
                b'^' => {
                    self.advance();
                    TokenKind::Caret
                }
                b'~' => {
                    self.advance();
                    TokenKind::Tilde
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
                        TokenKind::Pipe
                    }
                }
                b'"' => {
                    self.advance();
                    self.lex_string(start)?;
                    continue;
                }
                // A `'` starts a character literal. It used to be an error
                // ("unexpected character '\''"), which is why every
                // byte-level algorithm in the shipped stdlib is written with
                // decimal codes (`str.char_at(c, i) == 34`) and why the JSON
                // reader spells its punctuation as hex.
                b'\'' if self.label_ahead() => {
                    self.advance();
                    let name_start = self.pos;
                    while matches!(self.peek(), Some(c) if c == b'_' || c.is_ascii_alphanumeric()) {
                        self.pos += 1;
                    }
                    TokenKind::Label(self.src[name_start..self.pos].to_string())
                }
                b'\'' => {
                    self.advance();
                    let cp = self.lex_char(start)?;
                    TokenKind::CharLit(cp)
                }
                b'0'..=b'9' => {
                    // A radix prefix is read first, at the leading `0`, because
                    // `0` on its own is decimal: it has to branch before the
                    // decimal digit run consumes the zero and stops, which is
                    // what left `0xff` lexing as `0` and then an identifier
                    // `xff` (A-9, the half left after the bitwise operators).
                    let radix = if b == b'0' {
                        match self.peek_at(1) {
                            Some(b'x') | Some(b'X') => 16,
                            Some(b'b') | Some(b'B') => 2,
                            _ => 10,
                        }
                    } else {
                        10
                    };
                    if radix != 10 {
                        self.pos += 2; // the `0` and the prefix letter
                        let name = if radix == 16 { "hexadecimal" } else { "binary" };
                        let digit = |c: u8| {
                            if radix == 16 {
                                c.is_ascii_hexdigit()
                            } else {
                                c == b'0' || c == b'1'
                            }
                        };
                        let digits_start = self.pos;
                        while matches!(self.peek(), Some(c) if digit(c)) {
                            self.pos += 1;
                        }
                        if self.pos == digits_start {
                            let example = if radix == 16 { "0x1f" } else { "0b1010" };
                            return Err(LexError {
                                message: format!(
                                    "a {name} literal needs at least one {} digit, for example '{example}'",
                                    if radix == 16 { "hex" } else { "binary" }
                                ),
                                span: Span::new(start, self.pos),
                            });
                        }
                        // A digit the radix does not have, or a separator, is
                        // named here rather than left to the parser: `0b12`
                        // would otherwise be `0b1` and then `2`, and the parse
                        // error would point at the wrong thing.
                        if let Some(c) = self.peek() {
                            if c == b'_' {
                                return Err(LexError {
                                    message: "an integer literal takes no digit separators"
                                        .to_string(),
                                    span: Span::new(start, self.pos + 1),
                                });
                            }
                            if c.is_ascii_alphanumeric() {
                                return Err(LexError {
                                    message: format!(
                                        "invalid digit '{}' in a {name} literal",
                                        c as char
                                    ),
                                    span: Span::new(start, self.pos + 1),
                                });
                            }
                        }
                        let digits = &self.src[digits_start..self.pos];
                        let text = &self.src[start..self.pos];
                        TokenKind::IntLit(u64::from_str_radix(digits, radix).map_err(|_| {
                            LexError {
                                message: format!(
                                    "'{text}' is larger than the largest integer literal, {}",
                                    u64::MAX
                                ),
                                span: Span::new(start, self.pos),
                            }
                        })?)
                    } else {
                        while matches!(self.peek(), Some(b'0'..=b'9')) {
                            self.pos += 1;
                        }
                        let mut is_float = false;
                        // A number right after a single `.` is a tuple
                        // position (`t.0.1`), never the start of a float.
                        let after_dot = start > 0
                            && self.bytes[start - 1] == b'.'
                            && !(start > 1 && self.bytes[start - 2] == b'.');
                        if !after_dot
                            && self.peek() == Some(b'.')
                            && matches!(self.peek_at(1), Some(b'0'..=b'9'))
                        {
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
                            // A trailing identifier character is a suffix or a
                            // separator, and a literal in this language has
                            // neither: its width comes from its context (2.4).
                            // Saying so beats the parser's "found 'u8'".
                            if let Some(c) = self.peek() {
                                if c == b'_' {
                                    return Err(LexError {
                                        message: "an integer literal takes no digit separators"
                                            .to_string(),
                                        span: Span::new(start, self.pos + 1),
                                    });
                                }
                                if c.is_ascii_alphabetic() {
                                    return Err(LexError {
                                        message: format!(
                                            "a literal takes no suffix: the width of '{text}' comes from where it sits"
                                        ),
                                        span: Span::new(start, self.pos + 1),
                                    });
                                }
                            }
                            // The digit run is decimal digits already, so the
                            // only way the parse fails is a value wider than
                            // the widest integer type, and that is named as the
                            // limit it passed rather than as a malformed run.
                            TokenKind::IntLit(text.parse().map_err(|_| LexError {
                                message: format!(
                                    "'{text}' is larger than the largest integer literal, {}",
                                    u64::MAX
                                ),
                                span: Span::new(start, self.pos),
                            })?)
                        }
                    }
                }
                c if c.is_ascii_alphabetic() || c == b'_' => {
                    while matches!(self.peek(), Some(c) if c.is_ascii_alphanumeric() || c == b'_')
                    {
                        self.pos += 1;
                    }
                    // Fold the specific adjacent shape `ident!(` into one
                    // identifier token (covers `print!`), rather than
                    // merging `!` into identifiers generally - that would
                    // wrongly swallow `!=` when written without a space
                    // (`x!=y`).
                    if self.peek() == Some(b'!') && self.peek_at(1) == Some(b'(') {
                        self.pos += 1;
                    }
                    // `b'x'` is a byte literal, not the identifier `b` beside
                    // a character literal (H-11). The test is on the exact
                    // spelling `b` followed by a quote, so `bx'x'` is still an
                    // identifier and a literal.
                    if self.src[start..self.pos] == *"b" && self.peek() == Some(b'\'') {
                        self.advance(); // the opening quote
                        let byte = self.lex_byte_literal(start)?;
                        return Ok(Some(Token {
                            kind: TokenKind::ByteLit(byte),
                            span: Span::new(start, self.pos),
                        }));
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
                        "elif" => TokenKind::Elif,
                        "while" => TokenKind::While,
                        "for" => TokenKind::For,
                        "when" => TokenKind::When,
                        "defer" => TokenKind::Defer,
                        "errdefer" => TokenKind::Errdefer,
                        "return" => TokenKind::Return,
                        "true" => TokenKind::True,
                        "false" => TokenKind::False,
                        "datac" => TokenKind::Datac,
                        "enum" => TokenKind::Enum,
                        "pd" => TokenKind::Pd,
                        "tl" => TokenKind::Tl,
                        "impl" => TokenKind::Impl,
                        "trait" => TokenKind::Trait,
                        "as" => TokenKind::As,
                        "const" => TokenKind::Const,
                        "static" => TokenKind::Static,
                        "extern" => TokenKind::Extern,
                        "pub" => TokenKind::Pub,
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

    /// Lexes a character literal, having consumed the opening `'`; `start`
    /// is that quote's offset (open item 53).
    ///
    /// A literal holds exactly one Unicode scalar value, so a non-ASCII
    /// character is decoded here rather than left as its first byte: `'é'`
    /// and `'中'` are one character each. The code point is returned, not a
    /// byte, which is what lets `infer.rs` decide whether it fits the type
    /// its context asks for (and refuse it when it does not).
    ///
    /// Escapes are the string escapes minus what only makes sense in a
    /// longer literal, plus `\0` and `\'`: a character literal is not
    /// NUL-terminated and `'` has to be escapable somewhere.
    /// Whether the `'` at the cursor starts a loop label: a name right after
    /// it with no closing `'` (`'outer`), not a character literal (`'o'`,
    /// `'\n'`).
    fn label_ahead(&self) -> bool {
        let first = self.peek_at(1);
        if !matches!(first, Some(c) if c == b'_' || c.is_ascii_alphabetic()) {
            return false;
        }
        let mut i = self.pos + 1;
        while i < self.bytes.len() && (self.bytes[i] == b'_' || self.bytes[i].is_ascii_alphanumeric()) {
            i += 1;
        }
        self.bytes.get(i) != Some(&b'\'')
    }

    fn lex_char(&mut self, start: usize) -> Result<u32, LexError> {
        // A macro rather than a closure: every diagnostic needs the *current*
        // `self.pos`, and a closure that read it would hold a borrow across
        // the `self.advance()` calls below.
        macro_rules! err {
            ($($arg:tt)*) => {
                LexError {
                    message: format!($($arg)*),
                    span: Span::new(start, self.pos),
                }
            };
        }
        // `''` is not an empty character; say so rather than treating the
        // second quote as the character.
        if self.peek() == Some(b'\'') {
            self.advance();
            return Err(err!("a character literal cannot be empty"));
        }
        let cp = match self.advance() {
            None => return Err(err!("unterminated character literal")),
            Some(b'\n') => {
                return Err(err!(
                    "a character literal cannot span lines - write '\\n' for a newline"
                ))
            }
            Some(b'\\') => match self.advance() {
                Some(b'n') => '\n' as u32,
                Some(b't') => '\t' as u32,
                Some(b'r') => '\r' as u32,
                Some(b'0') => 0,
                Some(b'\\') => '\\' as u32,
                Some(b'\'') => '\'' as u32,
                Some(b'"') => '"' as u32,
                // `\xNN` is the code point NN in a character literal, the
                // same value a character with that code point has: `'\x41'`
                // is `'A'`. It is the byte NN in a byte literal. Two hex
                // digits exactly (open item 64).
                Some(b'x') => self.lex_hex_escape(start)? as u32,
                // `\u{4e2d}` - the code point in hex, braced so the escape has
                // an unambiguous end and an arbitrary number of digits.
                Some(b'u') => {
                    if self.advance() != Some(b'{') {
                        return Err(err!(
                            "a unicode escape is written '\\u{{...}}' - for example '\\u{{4e2d}}'"
                        ));
                    }
                    let hex_start = self.pos;
                    while matches!(self.peek(), Some(c) if c.is_ascii_hexdigit()) {
                        self.pos += 1;
                    }
                    if self.pos == hex_start {
                        return Err(err!("a unicode escape needs at least one hex digit"));
                    }
                    let digits = self.src[hex_start..self.pos].to_string();
                    if self.advance() != Some(b'}') {
                        return Err(err!(
                            "a unicode escape is closed with '}}' - for example '\\u{{4e2d}}'"
                        ));
                    }
                    let v = u32::from_str_radix(&digits, 16)
                        .map_err(|_| err!("invalid unicode escape '\\u{{{}}}'", digits))?;
                    if v > 0x10FFFF || (0xD800..=0xDFFF).contains(&v) {
                        return Err(err!("'\\u{{{}}}' is not a Unicode scalar value", digits));
                    }
                    v
                }
                Some(c) => {
                    return Err(err!(
                        "unknown escape '\\{}' in a character literal: the escapes are \\n, \\t, \\r, \\0, \\', \\\", \\\\, \\xNN and \\u{{...}}",
                        c as char
                    ))
                }
                None => return Err(err!("unterminated character literal")),
            },
            Some(_) => {
                // The byte just consumed is only the first byte of a
                // non-ASCII character, so re-decode the whole thing from the
                // source, which is already valid UTF-8.
                self.pos -= 1;
                let c = self.src[self.pos..]
                    .chars()
                    .next()
                    .expect("not at the end of the source");
                self.pos += c.len_utf8();
                c as u32
            }
        };
        match self.advance() {
            Some(b'\'') => Ok(cp),
            Some(_) => Err(err!("a character literal holds exactly one character")),
            None => Err(err!("unterminated character literal")),
        }
    }

    /// Lexes a byte literal, having consumed the opening `'`; `start` is that
    /// quote's offset (H-11).
    ///
    /// The result is a `u8` by construction, which is the point: `val nl =
    /// b'\n'` needs no type, and it is refused where a wider type is asked
    /// for rather than widened into one. `infer.rs` gives it `u8` and nothing
    /// else, so unlike a character literal it does not take the type its
    /// context asks for.
    ///
    /// It holds one byte, written as one ASCII character or as a `\xNN`
    /// escape: `b'é'` is a diagnostic (a byte literal is not a code point, and
    /// `'é'` belongs in a character literal), while `b'\xe9'` is the byte 233,
    /// which is the value a character literal with that code point has in a
    /// `u8` position. `b'\u{4e2d}'` is not its escape either.
    fn lex_byte_literal(&mut self, start: usize) -> Result<u8, LexError> {
        macro_rules! err {
            ($($arg:tt)*) => {
                LexError {
                    message: format!($($arg)*),
                    span: Span::new(start, self.pos),
                }
            };
        }
        if self.peek() == Some(b'\'') {
            self.advance();
            return Err(err!("a byte literal cannot be empty"));
        }
        let byte = match self.advance() {
            None => return Err(err!("unterminated byte literal")),
            Some(b'\n') => {
                return Err(err!(
                    "a byte literal cannot span lines, write '\\n' for a newline"
                ))
            }
            Some(b'\\') => match self.advance() {
                Some(b'n') => b'\n',
                Some(b't') => b'\t',
                Some(b'r') => b'\r',
                Some(b'0') => 0,
                Some(b'\\') => b'\\',
                Some(b'\'') => b'\'',
                Some(b'"') => b'"',
                Some(b'x') => self.lex_hex_escape(start)?,
                Some(b'u') => {
                    return Err(err!(
                        "a byte literal holds one byte, so '\\u{{...}}' is a character literal escape"
                    ))
                }
                Some(c) => {
                    return Err(err!(
                        "unknown escape '\\{}' in a byte literal: the escapes are \\n, \\t, \\r, \\0, \\', \\\", \\\\ and \\xNN",
                        c as char
                    ))
                }
                None => return Err(err!("unterminated byte literal")),
            },
            // The byte just consumed is only the first byte of a non-ASCII
            // character, so the message names the character rather than a byte
            // of its UTF-8: a byte literal is one ASCII character or a `\xNN`
            // escape (the escape is how any other byte is written), so a
            // non-ASCII character belongs in a character literal.
            Some(c) if !c.is_ascii() => {
                let byte_start = self.pos - 1;
                let ch = self.src[byte_start..]
                    .chars()
                    .next()
                    .expect("not at the end of the source");
                return Err(LexError {
                    message: format!(
                        "a byte literal is one ASCII character or a '\\xNN' escape, and '{ch}' is not one: a non-ASCII character is a code point, so it belongs in a character literal"
                    ),
                    span: Span::new(start, byte_start + ch.len_utf8()),
                });
            }
            Some(c) => c,
        };
        match self.advance() {
            Some(b'\'') => Ok(byte),
            Some(_) => Err(err!("a byte literal holds exactly one byte")),
            None => Err(err!("unterminated byte literal")),
        }
    }

    /// Reads the two hex digits of a `\xNN` escape, the `x` already consumed,
    /// and returns the value.
    ///
    /// Exactly two digits, never a run of them: C's hex escape swallows every
    /// hex digit after it, so `"\x0d" + "a"` is one escape there and its value
    /// overflows the byte (open item 64). A fixed width is the whole
    /// discipline, and it is why a third hex digit is just the next character
    /// of the text: `"\x41a"` is `"Aa"`.
    fn lex_hex_escape(&mut self, start: usize) -> Result<u8, LexError> {
        let mut value: u32 = 0;
        for _ in 0..2 {
            let digit = match self.advance() {
                Some(c) if c.is_ascii_hexdigit() => {
                    (c as char).to_digit(16).expect("an ASCII hex digit")
                }
                _ => {
                    return Err(LexError {
                        message: "a hex escape is written '\\xNN', with two hex digits".to_string(),
                        span: Span::new(start, self.pos),
                    })
                }
            };
            value = value * 16 + digit;
        }
        Ok(value as u8)
    }

    /// Lexes a string literal, having consumed the opening quote; `start` is
    /// that quote's offset.
    ///
    /// Without interpolation this queues the single `StringLit` token it
    /// always did. With interpolation it queues the flat template run
    /// (`TemplateStart StrChunk ... TemplateEnd`), lexing each `${...}`'s
    /// tokens through `next_token` - which is what makes nested strings,
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
                    // `\$` is a literal dollar sign, the way to write one in
                    // front of a name or a brace without starting a template.
                    Some(b'$') => text.push('$'),
                    // `\xNN`, exactly two hex digits (open item 64). A `str`
                    // is NUL-terminated UTF-8, so only the ASCII range and
                    // everything but NUL can be written this way; anything
                    // above it is a byte buffer's business (`Bytes`), not
                    // text's.
                    Some(b'x') => {
                        let byte = self.lex_hex_escape(start)?;
                        if byte == 0 {
                            return Err(LexError {
                                message: "a 'str' is NUL-terminated, so '\\x00' cannot be in a string literal".to_string(),
                                span: Span::new(start, self.pos),
                            });
                        }
                        if !byte.is_ascii() {
                            return Err(LexError {
                                message: format!(
                                    "a string literal is UTF-8: '\\x{byte:02X}' is not a text character. Write the character itself, or build the bytes with 'Bytes'"
                                ),
                                span: Span::new(start, self.pos),
                            });
                        }
                        text.push(byte as char);
                    }
                    // Anything else is an error rather than the bare character:
                    // passing it through made `"\q"` mean `"q"`, so a typo was
                    // silently a different string. `\0` is rejected for a second
                    // reason - a `str` is NUL-terminated and could not hold one.
                    Some(c) => {
                        return Err(LexError {
                            message: format!(
                                "unknown escape '\\{}' in a string literal: the escapes are \\n, \\t, \\r, \\\", \\\\, \\$ and \\xNN",
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
                        // `$name` - an identifier, nothing more (use
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
                // A raw NUL byte is refused where the `\x00` escape is, and for
                // the same reason: a `str` is NUL-terminated, so carrying the
                // byte would truncate the literal in every backend instead of
                // failing.
                Some(b'\0') => {
                    return Err(LexError {
                        message: "a 'str' is NUL-terminated, so a raw NUL byte cannot be in a string literal".to_string(),
                        span: Span::new(start, self.pos),
                    })
                }
                Some(c) if c < 0x80 => text.push(c as char),
                Some(c) => {
                    // A non-ASCII byte starts a multi-byte UTF-8 sequence.
                    // `src` is UTF-8, so reassemble the character instead of
                    // pushing each byte as its own Latin-1 scalar - the
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
#[path = "lexer/tests.rs"]
mod tests;

#[path = "lexer/token.rs"]
mod token;
pub(crate) use token::*;
