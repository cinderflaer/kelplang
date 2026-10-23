use super::*;

impl<'a> Lexer<'a> {
    /// Whether the `'` at the cursor starts a loop label: a name right after
    /// it with no closing `'` (`'outer`), not a character literal (`'o'`,
    /// `'\n'`).
    pub(crate) fn label_ahead(&self) -> bool {
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

    /// Refuses a digit separator (`_`) in the digit run `run_start..run_end`
    /// that is not between two digits. A separator is allowed only between
    /// two digits: `1_000`, `0x1000_0000`, `0b1_0`. A separator first (`0x_1`),
    /// last (`1_`) or doubled (`1__0`) is refused at its own position.
    pub(crate) fn check_digit_separators(
        &self,
        run_start: usize,
        run_end: usize,
    ) -> Result<(), LexError> {
        let run = &self.bytes[run_start..run_end];
        for (i, &c) in run.iter().enumerate() {
            if c != b'_' {
                continue;
            }
            let at = run_start + i;
            let refuse = |message: &str| LexError {
                message: format!("a digit separator '_' {message}"),
                span: Span::new(at, at + 1),
            };
            if i == 0 {
                return Err(refuse(
                    "must come between two digits, not before the first digit",
                ));
            }
            if run[i - 1] == b'_' {
                return Err(refuse(
                    "takes one at a time: put one '_' between two digits, not two in a row",
                ));
            }
            if i + 1 == run.len() {
                return Err(refuse(
                    "must come between two digits, not after the last digit",
                ));
            }
        }
        Ok(())
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
    pub(crate) fn lex_char(&mut self, start: usize) -> Result<u32, LexError> {
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
    pub(crate) fn lex_byte_literal(&mut self, start: usize) -> Result<u8, LexError> {
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
    pub(crate) fn lex_hex_escape(&mut self, start: usize) -> Result<u8, LexError> {
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
    pub(crate) fn lex_string(&mut self, start: usize) -> Result<(), LexError> {
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

    /// Lexes a multi-line string block, having consumed the opening `"""`;
    /// `start` is the offset of its first quote.
    ///
    /// The opening `"""` is followed by spaces or tabs and then a line break,
    /// which is not part of the value. The block ends at the first line that
    /// holds `"""` after its indentation (the closing line). Its indentation
    /// is removed from every other line: a non-blank line must start with it
    /// (else refused), and a blank line that is shorter than it becomes empty.
    /// Each line then gets a newline, so the value always ends with one, and
    /// the string escapes apply to the dedented text. A line that would begin
    /// with `"""` is written `\"""`, so it stays content. A `$` is
    /// text: a block is never a template. A CR just before a line break is
    /// part of that break, so CRLF files give LF values.
    pub(crate) fn lex_block_string(&mut self, start: usize) -> Result<(), LexError> {
        let line_no = |offset: usize| self.src[..offset].matches('\n').count() + 1;

        while matches!(self.peek(), Some(b' ') | Some(b'\t')) {
            self.pos += 1;
        }
        let opened = match self.peek() {
            Some(b'\n') => 1,
            Some(b'\r') if self.peek_at(1) == Some(b'\n') => 2,
            _ => {
                return Err(LexError {
                    message: "a multi-line string opens with \"\"\" and then a line break: write the text on the lines below it".to_string(),
                    span: Span::new(start, self.pos),
                })
            }
        };
        self.pos += opened;

        // Find the closing line and collect the content lines with their offsets.
        let mut lines: Vec<(usize, &str)> = Vec::new();
        let mut line_start = self.pos;
        let (indent, close_at) = loop {
            // The last line of the file may have no line break, and it may be
            // the closing line.
            let line_end = self.src[line_start..].find('\n').map_or(self.src.len(), |i| line_start + i);
            let raw = &self.src[line_start..line_end];
            let line = raw.strip_suffix('\r').unwrap_or(raw);
            let trimmed = line.trim_start_matches([' ', '\t']);
            if trimmed.starts_with("\"\"\"") {
                let indent = &line[..line.len() - trimmed.len()];
                break (indent, line_start + indent.len() + 3);
            }
            if line_end == self.src.len() {
                return Err(LexError {
                    message: format!(
                        "unterminated multi-line string literal: it opened at line {} and needs a line holding only \"\"\" to close it",
                        line_no(start)
                    ),
                    span: Span::new(start, self.src.len()),
                });
            }
            lines.push((line_start, line));
            line_start = line_end + 1;
        };

        let mut value = String::new();
        for (offset, line) in lines {
            let body = if line.bytes().all(|b| b == b' ' || b == b'\t') {
                // A blank line: keep what is past the indentation, or nothing.
                line.strip_prefix(indent).unwrap_or("")
            } else {
                match line.strip_prefix(indent) {
                    Some(rest) => rest,
                    None => {
                        return Err(LexError {
                            message: format!(
                                "line {} of the multi-line string is less indented than its closing \"\"\": every line must start with the same indentation as the closing line",
                                line_no(offset)
                            ),
                            span: Span::new(offset, offset + line.len()),
                        })
                    }
                }
            };
            let body_start = offset + (line.len() - body.len());
            decode_block_line(body, body_start, &mut value)?;
            value.push('\n');
        }

        self.pos = close_at;
        self.pending.push_back(Token {
            kind: TokenKind::StringLit(value),
            span: Span::new(start, self.pos),
        });
        Ok(())
    }

    /// True right after a `$` that starts an interpolation: `$name` or
    /// `${expr}`. A `$` followed by anything else (a space, a digit, end of
    /// string) stays literal text, so `"cost $5"` needs no escaping.
    pub(crate) fn at_interpolation(&self) -> bool {
        match self.peek() {
            Some(b'{') => true,
            Some(c) => c == b'_' || c.is_ascii_alphabetic(),
            None => false,
        }
    }
}

/// Decodes one dedented line of a multi-line string block into `out`, with the
/// string escapes (`\n \t \r \" \\ \$ \xNN`). `offset` is where `line` starts in
/// the source, so an error names the escape's own bytes.
fn decode_block_line(line: &str, offset: usize, out: &mut String) -> Result<(), LexError> {
    let mut chars = line.char_indices();
    while let Some((i, c)) = chars.next() {
        let at = offset + i;
        match c {
            '\0' => {
                return Err(LexError {
                    message: "a 'str' is NUL-terminated, so a raw NUL byte cannot be in a string literal".to_string(),
                    span: Span::new(at, at + 1),
                })
            }
            '\\' => match chars.next() {
                Some((_, 'n')) => out.push('\n'),
                Some((_, 't')) => out.push('\t'),
                Some((_, 'r')) => out.push('\r'),
                Some((_, '"')) => out.push('"'),
                Some((_, '\\')) => out.push('\\'),
                Some((_, '$')) => out.push('$'),
                Some((_, 'x')) => {
                    let hi = chars.next().and_then(|(_, h)| h.to_digit(16));
                    let lo = chars.next().and_then(|(_, l)| l.to_digit(16));
                    let (Some(hi), Some(lo)) = (hi, lo) else {
                        return Err(LexError {
                            message: "a hex escape is written '\\xNN', with exactly two hex digits".to_string(),
                            span: Span::new(at, offset + line.len()),
                        });
                    };
                    let byte = (hi * 16 + lo) as u8;
                    if byte == 0 {
                        return Err(LexError {
                            message: "a 'str' is NUL-terminated, so '\\x00' cannot be in a string literal".to_string(),
                            span: Span::new(at, at + 4),
                        });
                    }
                    if !byte.is_ascii() {
                        return Err(LexError {
                            message: format!(
                                "a string literal is UTF-8: '\\x{byte:02X}' is not a text character. Write the character itself, or build the bytes with 'Bytes'"
                            ),
                            span: Span::new(at, at + 4),
                        });
                    }
                    out.push(byte as char);
                }
                Some((j, other)) => {
                    return Err(LexError {
                        message: format!(
                            "unknown escape '\\{other}' in a multi-line string literal: the escapes are \\n, \\t, \\r, \\\", \\\\, \\$ and \\xNN"
                        ),
                        span: Span::new(offset + j - 1, offset + j + other.len_utf8()),
                    })
                }
                None => {
                    return Err(LexError {
                        message: "a backslash at the end of a line is not a line continuation in a multi-line string: write \\n, or the rest of the text on this line".to_string(),
                        span: Span::new(at, at + 1),
                    })
                }
            },
            c => out.push(c),
        }
    }
    Ok(())
}
