use super::*;

impl Parser {
    /// `push` (if any) followed by the exit itself, as one statement.
    pub(crate) fn with_push(push: Option<Stmt>, exit: Stmt, span: Span) -> Stmt {
        match push {
            None => exit,
            Some(p) => Stmt::Expr { expr: Expr::Block(Block { stmts: vec![p, exit], span }), span },
        }
    }

    pub(crate) fn parse_while_stmt(&mut self) -> PResult<Stmt> {
        let start = self.expect(TokenKind::While, "'while'")?.span;
        let cond = self.parse_head_expr()?;
        let body = self.parse_block()?;
        let span = start.to(body.span);
        Ok(Stmt::While { cond, body, iterating: None, span })
    }

    /// `for x in xs { BODY }`, desugared here into an indexed walk over the
    /// `List` - the same trick the boolean `when` and `if let` already use, so
    /// nothing downstream learns a new form (A-5, `design/syntax.md (kelp wiki)` §6.2):
    ///
    /// ```text
    /// {
    ///     var #for0_i: usize = 0
    ///     while #for0_i < SEQ.len() {
    ///         { val x = SEQ.get(#for0_i)  BODY }
    ///         #for0_i = #for0_i + 1
    ///     }
    /// }
    /// ```
    ///
    /// `SEQ` is the iterable as written when it is a place, and a synthesized
    /// `val #for0 = EXPR` otherwise (so a call is evaluated once and freed at
    /// the end of the block). Binding a place by value would move the list, and
    /// a parameter *is* a borrow - `for x in xs` inside a function would move
    /// out of a borrow, which is the common case.
    ///
    /// The element binding is exactly what `List.get` gives: a copy for a
    /// `Copy` element, a shared borrow for an owned one. It lives in a block
    /// of its own so the borrow ends before the increment, and so `BODY`'s own
    /// tail expression stays the tail of a block.
    pub(crate) fn parse_for_stmt(&mut self) -> PResult<Stmt> {
        let start = self.expect(TokenKind::For, "'for'")?.span;
        // `for (k, v) in xs`: the element is bound to a hidden name and the
        // body starts with the ordinary destructuring `val (k, v) = <hidden>`
        // (`design/syntax.md (kelp wiki)` 4), so a pattern costs no pass anything.
        let pattern = if self.check(&TokenKind::LParen) {
            Some((self.peek().span, self.parse_binding_pattern()?))
        } else {
            None
        };
        let (binding, binding_span) = match &pattern {
            Some((span, _)) => {
                self.for_patterns += 1;
                (format!("{}{}", FOR_PATTERN_PREFIX, self.for_patterns - 1), *span)
            }
            None => self.expect_ident()?,
        };
        // `in` is an ordinary identifier (2.3): it is the separator only in
        // this position, the way `let` is a pattern only after `if` (6.1).
        let (word, word_span) = self.expect_ident()?;
        if word != "in" {
            return Err(ParseError {
                help: None,
                message: format!(
                    "expected 'in' between the 'for' binding and what it iterates, found '{word}'"
                ),
                span: word_span,
            });
        }
        let iterable = self.parse_head_expr()?;
        let mut body = self.parse_block()?;
        let span = start.to(body.span);
        if let Some((pspan, pattern)) = pattern {
            body.stmts.insert(
                0,
                Stmt::LetTuple {
                    pattern,
                    mutable: false,
                    init: Expr::Ident(binding.clone(), pspan),
                    span: pspan,
                },
            );
        }
        // The type of the iterable decides the desugar (`kelp wiki page language/iterators.md`),
        // and only `infer.rs` knows it, so the statement is kept as written.
        Ok(Stmt::For {
            binding,
            binding_span,
            iterable,
            body,
            span,
        })
    }

    /// Whether the token after the current one ends the statement: a newline,
    /// the block's `}`, or the end of the file. Used to tell a bare `break` or
    /// `continue` statement from a name used in an expression.
    pub(crate) fn statement_ends_here(&self) -> bool {
        matches!(
            self.peek2_kind(),
            TokenKind::Newline | TokenKind::RBrace | TokenKind::Eof
        )
    }

    pub(crate) fn parse_defer_stmt(&mut self) -> PResult<Stmt> {
        let start = self.expect(TokenKind::Defer, "'defer'")?.span;
        let expr = self.parse_head_expr()?;
        let span = start.to(expr.span());
        Ok(Stmt::Defer { expr, span })
    }

    pub(crate) fn parse_errdefer_stmt(&mut self) -> PResult<Stmt> {
        let start = self.expect(TokenKind::Errdefer, "'errdefer'")?.span;
        let expr = self.parse_head_expr()?;
        let span = start.to(expr.span());
        Ok(Stmt::Errdefer { expr, span })
    }

    pub(crate) fn parse_return_stmt(&mut self) -> PResult<Stmt> {
        let start = self.expect(TokenKind::Return, "'return'")?.span;
        let value = if self.at_stmt_end() {
            None
        } else {
            Some(self.parse_expr()?)
        };
        let span = match &value {
            Some(v) => start.to(v.span()),
            None => start,
        };
        Ok(Stmt::Return { value, span })
    }

    // ---- expressions (Pratt parsing) ----

    pub(crate) fn parse_expr(&mut self) -> PResult<Expr> {
        self.nest()?;
        let r = self.parse_expr_inner();
        self.depth -= 1;
        r
    }

    pub(crate) fn parse_expr_inner(&mut self) -> PResult<Expr> {
        self.parse_assignment()
    }

    pub(crate) fn parse_assignment(&mut self) -> PResult<Expr> {
        let target = self.parse_binary(0)?;

        let compound_op = match self.peek_kind() {
            TokenKind::PlusEq => Some(BinOp::Add),
            TokenKind::MinusEq => Some(BinOp::Sub),
            TokenKind::StarEq => Some(BinOp::Mul),
            TokenKind::SlashEq => Some(BinOp::Div),
            TokenKind::PercentEq => Some(BinOp::Mod),
            _ => None,
        };
        if let Some(op) = compound_op {
            self.advance();
            let rhs = self.parse_assignment()?;
            let span = target.span().to(rhs.span());
            // Desugar `target OP= rhs` to `target = target OP rhs`.
            let combined = Expr::Binary {
                op,
                lhs: Box::new(target.clone()),
                rhs: Box::new(rhs),
                span,
            };
            return Ok(Expr::Assign {
                target: Box::new(target),
                value: Box::new(combined),
                span,
            });
        }

        if self.check(&TokenKind::Eq) {
            self.advance();
            let value = self.parse_assignment()?;
            let span = target.span().to(value.span());
            return Ok(Expr::Assign {
                target: Box::new(target),
                value: Box::new(value),
                span,
            });
        }
        Ok(target)
    }

    pub(crate) fn binding_power(op: &TokenKind) -> Option<(u8, BinOp)> {
        use TokenKind::*;
        Some(match op {
            // C's precedence *except* for the bitwise operators, which sit
            // above the comparisons rather than below them. This is Rust's
            // reading, and it is deliberate: in C, `flags & MASK == 0` means
            // `flags & (MASK == 0)`, which is a bug that has been written a
            // million times. Here it means `(flags & MASK) == 0`. `|` and `^`
            // keep their relative order (`|` loosest, then `^`, then `&`), so
            // `a | b & c` is still `a | (b & c)`.
            OrOr => (1, BinOp::Or),
            AndAnd => (2, BinOp::And),
            EqEq => (3, BinOp::Eq),
            NotEq => (3, BinOp::NotEq),
            Lt => (4, BinOp::Lt),
            LtEq => (4, BinOp::LtEq),
            Gt => (4, BinOp::Gt),
            GtEq => (4, BinOp::GtEq),
            Pipe => (5, BinOp::BitOr),
            Caret => (6, BinOp::BitXor),
            // `&` in *infix* position is bitwise-and. A prefix `&` never
            // reaches here: `parse_unary` consumes it first.
            Amp => (7, BinOp::BitAnd),
            // `a << b` and `a >> b` arrive as two adjacent `<`/`>` tokens
            // (`parse_binary` folds them before consulting this table), so
            // the shift operators are not listed here.
            Plus => (9, BinOp::Add),
            Minus => (9, BinOp::Sub),
            Star => (10, BinOp::Mul),
            Slash => (10, BinOp::Div),
            Percent => (10, BinOp::Mod),
            _ => return None,
        })
    }

    /// `<<` and `>>`, recognized as two *adjacent* `<`/`>` tokens (A-9).
    ///
    /// The lexer deliberately does not produce a single `Shl`/`Shr` token:
    /// `List<List<i32>>` closes a nested type-argument list with two `>`
    /// characters, and a token that swallowed both would leave the type
    /// parser with nothing to close the inner list with. Adjacency is what
    /// separates `a << b` from a comparison followed by another one, and the
    /// two spellings being visually distinct is why the shift reads as one
    /// operator anyway.
    pub(crate) fn shift_op(&self) -> Option<BinOp> {
        let first = &self.tokens[self.pos];
        let second = self.tokens.get(self.pos + 1)?;
        if first.span.end != second.span.start {
            return None;
        }
        match (&first.kind, &second.kind) {
            (TokenKind::Lt, TokenKind::Lt) => Some(BinOp::Shl),
            (TokenKind::Gt, TokenKind::Gt) => Some(BinOp::Shr),
            _ => None,
        }
    }

    pub(crate) fn parse_binary(&mut self, min_bp: u8) -> PResult<Expr> {
        self.parse_binary_filtered(min_bp, true)
    }

    /// A binary expression that stops before every comparison operator. Used
    /// for a const argument (`Buf<SIZE * 2>`, A-20), where `<` and `>` are the
    /// delimiters of the argument list rather than operators.
    pub(crate) fn parse_const_arg_expr(&mut self) -> PResult<Expr> {
        self.parse_binary_filtered(0, false)
    }

    pub(crate) fn parse_binary_filtered(&mut self, min_bp: u8, comparisons: bool) -> PResult<Expr> {
        let mut lhs = self.parse_unary()?;
        let mut chain = 0usize;
        loop {
            // Shifts first: they are two tokens wide, so the table above
            // cannot describe them.
            let (bp, op, width) = if let Some(op) = self.shift_op() {
                (8u8, op, 2usize)
            } else {
                match Self::binding_power(self.peek_kind()) {
                    Some((bp, op)) => (bp, op, 1),
                    None => break,
                }
            };
            if !comparisons
                && matches!(
                    op,
                    BinOp::Lt
                        | BinOp::LtEq
                        | BinOp::Gt
                        | BinOp::GtEq
                        | BinOp::Eq
                        | BinOp::NotEq
                        | BinOp::Or
                        | BinOp::And
                )
            {
                break;
            }
            if bp < min_bp {
                break;
            }
            chain += 1;
            self.chain_limit(chain)?;
            for _ in 0..width {
                self.advance();
            }
            let rhs = self.parse_binary_filtered(bp + 1, comparisons)?;
            let span = lhs.span().to(rhs.span());
            lhs = Expr::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                span,
            };
        }
        Ok(lhs)
    }

    pub(crate) fn parse_unary(&mut self) -> PResult<Expr> {
        self.nest()?;
        let r = self.parse_unary_inner();
        self.depth -= 1;
        r
    }

    pub(crate) fn parse_unary_inner(&mut self) -> PResult<Expr> {
        match self.peek_kind() {
            TokenKind::Minus => {
                let start = self.advance().span;
                let expr = self.parse_unary()?;
                let span = start.to(expr.span());
                Ok(Expr::Unary {
                    op: UnOp::Neg,
                    expr: Box::new(expr),
                    span,
                })
            }
            TokenKind::Bang => {
                let start = self.advance().span;
                let expr = self.parse_unary()?;
                let span = start.to(expr.span());
                Ok(Expr::Unary {
                    op: UnOp::Not,
                    expr: Box::new(expr),
                    span,
                })
            }
            TokenKind::Tilde => {
                let start = self.advance().span;
                let expr = self.parse_unary()?;
                let span = start.to(expr.span());
                Ok(Expr::Unary {
                    op: UnOp::BitNot,
                    expr: Box::new(expr),
                    span,
                })
            }
            TokenKind::Amp => {
                let start = self.advance().span;
                let mutable = if self.check(&TokenKind::Mut) {
                    self.advance();
                    true
                } else {
                    false
                };
                let expr = self.parse_unary()?;
                let span = start.to(expr.span());
                Ok(Expr::Ref {
                    mutable,
                    expr: Box::new(expr),
                    span,
                })
            }
            // A leading `*` at the start of a unary-parse position is
            // unambiguously prefix-deref: multiplication's `Star` is only
            // ever consumed by `parse_binary`'s own operator loop, which
            // runs *after* this function returns, never by `parse_unary`
            // itself.
            TokenKind::Star => {
                let start = self.advance().span;
                let expr = self.parse_unary()?;
                let span = start.to(expr.span());
                Ok(Expr::Deref {
                    expr: Box::new(expr),
                    span,
                })
            }
            _ => self.parse_postfix(),
        }
    }

    pub(crate) fn parse_postfix(&mut self) -> PResult<Expr> {
        let mut expr = self.parse_primary()?;
        let mut chain = 0usize;
        loop {
            chain += 1;
            self.chain_limit(chain)?;
            // `expr?` - the propagation operator (MK5-S6-M1). It is postfix,
            // so it binds tighter than any binary operator and chains with the
            // accesses below (`f()?.x`).
            if self.check(&TokenKind::Question) {
                let q = self.advance().span;
                let start = expr.span();
                expr = Expr::Try {
                    expr: Box::new(expr),
                    span: start.to(q),
                };
                continue;
            }
            if self.check(&TokenKind::Dot) {
                self.advance();
                // `t.0`: a tuple position, the field a derived record names
                // `f0` (`design/syntax.md (kelp wiki)` 4).
                if let TokenKind::IntLit(n) = self.peek_kind().clone() {
                    let nspan = self.advance().span;
                    let span = expr.span().to(nspan);
                    expr = Expr::FieldAccess {
                        base: Box::new(expr),
                        field: crate::types::tuple_field_name(n as usize),
                        span,
                    };
                    continue;
                }
                let (name, nspan) = self.expect_ident()?;
                // `Option.Some<i32>(v = 1)` - an explicit instantiation on a
                // generic `enum`'s variant (A-2). Recognized only in the
                // adjacent form, so `a.b < c > (d)` stays a comparison.
                let type_args = self.try_explicit_type_args()?.unwrap_or_default();
                if self.check(&TokenKind::LParen)
                    || (!self.no_trailing_block && self.check(&TokenKind::LBrace))
                {
                    let (mut args, mut end) = if self.check(&TokenKind::LParen) {
                        self.parse_call_args()?
                    } else {
                        (Vec::new(), nspan)
                    };
                    self.parse_trailing_block(&mut args, &mut end)?;
                    let span = expr.span().to(end);
                    expr = Expr::MethodCall {
                        receiver: Box::new(expr),
                        method: name,
                        type_args,
                        args,
                        span,
                    };
                } else {
                    let span = expr.span().to(nspan);
                    expr = Expr::FieldAccess {
                        base: Box::new(expr),
                        field: name,
                        span,
                    };
                }
            } else {
                break;
            }
        }
        Ok(expr)
    }

    /// Parses a parenthesized, comma-separated argument list (each
    /// optionally named, `f(x = 1)`), starting at the `(`. Shared between
    /// plain calls (`f(...)`) and method calls (`recv.m(...)`).
    pub(crate) fn parse_call_args(&mut self) -> PResult<(Vec<CallArg>, Span)> {
        self.expect(TokenKind::LParen, "'('")?;
        let saved = std::mem::replace(&mut self.no_trailing_block, false);
        let mut args = Vec::new();
        while !self.check(&TokenKind::RParen) {
            // A named argument is exactly `ident =` (not `==`); parens
            // already make newlines insignificant here.
            let name = if matches!(self.peek_kind(), TokenKind::Ident(_)) && matches!(self.peek2_kind(), TokenKind::Eq) {
                let (n, _) = self.expect_ident()?;
                self.advance(); // '='
                Some(n)
            } else {
                None
            };
            let value = self.parse_expr()?;
            args.push(CallArg { name, value });
            if self.check(&TokenKind::Comma) {
                self.advance();
            } else {
                break;
            }
        }
        let end = self.expect(TokenKind::RParen, "')'")?.span;
        self.no_trailing_block = saved;
        Ok((args, end))
    }

    /// `asm("<template>", operands...)` (kernel stage 5). The template's
    /// placeholders are `{0}`, `{1}`, ... in operand order; each operand is
    /// `in("<constraint>") expr`, `out("<constraint>") place` (or `_` for the
    /// anonymous output that is the expression's value), `inout(...) place`,
    /// or `clobber("<register>")`, and the option words are `volatile`,
    /// `nomem` and `nostack`. `kelp wiki page language/unsafe.md` is the design.
    pub(crate) fn parse_asm_expr(&mut self) -> PResult<Expr> {
        let start = self.advance().span;
        self.expect(TokenKind::LParen, "'(' after 'asm'")?;
        let (template, _) = match self.peek_kind().clone() {
            TokenKind::StringLit(s) => {
                self.advance();
                (s, self.tokens[self.pos - 1].span)
            }
            other => {
                return Err(ParseError {
                    help: None,
                    message: format!(
                        "expected a quoted instruction template in 'asm(\"...\", ...)', found {}", other.describe()
                    ),
                    span: self.peek().span,
                })
            }
        };
        let saved = std::mem::replace(&mut self.no_trailing_block, true);
        let mut operands: Vec<AsmOperand> = Vec::new();
        let mut clobbers: Vec<String> = Vec::new();
        let mut nomem = false;
        let mut nostack = false;
        while self.check(&TokenKind::Comma) {
            self.advance();
            let (word, word_span) = match self.peek_kind().clone() {
                TokenKind::Ident(n) => {
                    self.advance();
                    (n, self.tokens[self.pos - 1].span)
                }
                other => {
                    return Err(ParseError {
                        help: None,
                        message: format!(
                            "expected an asm operand ('in', 'out', 'inout', 'clobber') or an option word ('volatile', 'nomem', 'nostack'), found {}", other.describe()
                        ),
                        span: self.peek().span,
                    })
                }
            };
            match word.as_str() {
                "volatile" | "nomem" | "nostack"
                    if !matches!(self.peek_kind(), TokenKind::LParen) =>
                {
                    match word.as_str() {
                        "volatile" => {}
                        "nomem" => nomem = true,
                        _ => nostack = true,
                    }
                }
                "clobber" if matches!(self.peek_kind(), TokenKind::LParen) => {
                    let (name, _) = self.asm_constraint("clobber")?;
                    if name.is_empty() {
                        return Err(ParseError {
                            help: None,
                            message: "'clobber(\"\")' needs a register name, for example 'clobber(\"memory\")' or 'clobber(\"rax\")'"
                                .to_string(),
                            span: word_span,
                        });
                    }
                    clobbers.push(name);
                }
                "in" | "out" | "inout" if matches!(self.peek_kind(), TokenKind::LParen) => {
                    let kind = match word.as_str() {
                        "in" => AsmOperandKind::In,
                        "out" => AsmOperandKind::Out,
                        _ => AsmOperandKind::InOut,
                    };
                    let (constraint, _) = self.asm_constraint(&word)?;
                    let operand = if matches!(self.peek_kind(), TokenKind::Ident(n) if n == "_") {
                        self.advance();
                        None
                    } else {
                        Some(self.parse_expr()?)
                    };
                    if operand.is_none() && kind != AsmOperandKind::Out {
                        return Err(ParseError {
                            help: None,
                            message: format!(
                                "'{}' needs a place: only 'out' may write an anonymous output ('out(\"...\") _')",
                                word
                            ),
                            span: word_span,
                        });
                    }
                    let end = operand
                        .as_ref()
                        .map(|e| e.span())
                        .unwrap_or(word_span);
                    operands.push(AsmOperand {
                        kind,
                        constraint,
                        operand,
                        span: word_span.to(end),
                    });
                }
                other => {
                    return Err(ParseError {
                        help: None,
                        message: format!(
                            "unknown asm operand kind '{other}': expected 'in', 'out', 'inout' or 'clobber'"
                        ),
                        span: word_span,
                    })
                }
            }
        }
        self.no_trailing_block = saved;
        let end = self.expect(TokenKind::RParen, "')' to close 'asm(...)'")?.span;
        Ok(Expr::Asm {
            template,
            operands,
            clobbers,
            nomem,
            nostack,
            span: start.to(end),
        })
    }

    /// `("<constraint>")` after an asm operand kind: the LLVM constraint or
    /// clobber name, passed to the backend unchanged. The compiler checks that
    /// it is a plain word, never that the register exists.
    pub(crate) fn asm_constraint(&mut self, kind: &str) -> PResult<(String, Span)> {
        self.expect(TokenKind::LParen, "'(' after the operand kind")?;
        let (name, span) = match self.peek_kind().clone() {
            TokenKind::StringLit(s) => {
                let span = self.peek().span;
                self.advance();
                (s, span)
            }
            other => {
                return Err(ParseError {
                    help: None,
                    message: format!(
                        "expected a quoted constraint in '{kind}(\"...\")', found {}", other.describe()
                    ),
                    span: self.peek().span,
                })
            }
        };
        if name.is_empty() {
            return Err(ParseError {
                help: None,
                message: format!(
                    "'{kind}(\"\")' needs a constraint name, for example 'in(\"r\")' or 'out(\"rax\")'"
                ),
                span,
            });
        }
        if name
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '{' | '}' | '~' | ',' | '$'))
        {
            return Err(ParseError {
                help: None,
                message: format!(
                    "'{name}' is not a constraint name: a constraint is a register class or register name, with no spaces, '{{', '}}', '~', ',' or '$'"
                ),
                span,
            });
        }
        self.expect(TokenKind::RParen, "')' to close the constraint")?;
        Ok((name, span))
    }

    pub(crate) fn parse_primary(&mut self) -> PResult<Expr> {
        let tok = self.peek().clone();
        match tok.kind {
            TokenKind::IntLit(v) => {
                self.advance();
                Ok(Expr::IntLit(v, tok.span))
            }
            TokenKind::CharLit(v) => {
                self.advance();
                Ok(Expr::CharLit(v, tok.span))
            }
            TokenKind::ByteLit(v) => {
                self.advance();
                Ok(Expr::ByteLit(v, tok.span))
            }
            TokenKind::FloatLit(v) => {
                self.advance();
                Ok(Expr::FloatLit(v, tok.span))
            }
            TokenKind::StringLit(ref s) => {
                let s = s.clone();
                self.advance();
                Ok(Expr::StringLit(s, tok.span))
            }
            TokenKind::TemplateStart => self.parse_template(),
            // `loop { ... break value ... }` as a value (contextual: `loop`
            // followed by `{`), optionally labelled (`'a: loop { ... }`).
            TokenKind::Ident(ref name) if name == "loop" && matches!(self.peek2_kind(), TokenKind::LBrace) => {
                self.parse_loop_value(None)
            }
            TokenKind::Label(ref label)
                if matches!(self.peek2_kind(), TokenKind::Colon)
                    && matches!(self.tokens.get(self.pos + 2).map(|t| &t.kind), Some(TokenKind::Ident(n)) if n == "loop") =>
            {
                let label = label.clone();
                self.advance();
                self.advance();
                self.parse_loop_value(Some(label))
            }
            TokenKind::True => {
                self.advance();
                Ok(Expr::BoolLit(true, tok.span))
            }
            TokenKind::False => {
                self.advance();
                Ok(Expr::BoolLit(false, tok.span))
            }
            TokenKind::Ident(ref name) => {
                let name = name.clone();
                // `asm("...", ...)` (kernel stage 5): `asm` is a contextual
                // word, so it only counts when a `(` touches it, exactly as
                // `unsafe` counts only before `{`.
                if name == "asm" && matches!(self.peek2_kind(), TokenKind::LParen) {
                    return self.parse_asm_expr();
                }
                // `unsafe { ... }` (MK9-S2 stage 1): the contextual word is a
                // block introducer exactly when the next token is `{`, so
                // `unsafe` stays usable as an ordinary name everywhere else.
                // `select { ... }` (issue 216, plan 236 step d): contextual, only before `{`.
                if name == "select" && matches!(self.peek2_kind(), TokenKind::LBrace) {
                    return self.parse_select();
                }
                if name == "unsafe" && matches!(self.peek2_kind(), TokenKind::LBrace) {
                    self.advance();
                    let block = self.parse_block()?;
                    let span = tok.span.to(block.span);
                    return Ok(Expr::Unsafe { block, span });
                }
                // `block name: T { ... }` (manual memory blocks, slice 1): the
                // contextual word is a block introducer only when an identifier
                // and then `:` follow it, so `block` stays an ordinary name
                // (`val block: i32`, `block = 5`, `block[0]`, `block.reset()`).
                if name == "block"
                    && matches!(self.peek2_kind(), TokenKind::Ident(_))
                    && matches!(self.peek3_kind(), TokenKind::Colon)
                {
                    self.advance();
                    let name_tok = self.advance();
                    let block_name = match name_tok.kind {
                        TokenKind::Ident(n) => n,
                        _ => unreachable!("peek2_kind said an identifier follows"),
                    };
                    self.expect(TokenKind::Colon, "':' after the block name")?;
                    let ty = self.parse_type()?;
                    let body = self.parse_block()?;
                    let span = tok.span.to(body.span);
                    return Ok(Expr::ManualBlock { name: block_name, ty, body, span });
                }
                self.advance();
                let type_args = self.try_explicit_type_args()?.unwrap_or_default();
                if self.check(&TokenKind::LParen) {
                    let (mut args, mut end) = self.parse_call_args()?;
                    self.parse_trailing_block(&mut args, &mut end)?;
                    Ok(Expr::Call {
                        callee: name,
                        type_args,
                        args,
                        span: tok.span.to(end),
                    })
                } else if !self.no_trailing_block && self.check(&TokenKind::LBrace) {
                    // `name { ... }`: a call whose only argument is the block.
                    let (mut args, mut end) = (Vec::new(), tok.span);
                    self.parse_trailing_block(&mut args, &mut end)?;
                    Ok(Expr::Call {
                        callee: name,
                        type_args,
                        args,
                        span: tok.span.to(end),
                    })
                } else {
                    Ok(Expr::Ident(name, tok.span))
                }
            }
            TokenKind::LParen => {
                // A tuple literal `(a, b)` is the parenthesized form with a
                // comma in it, so the first element decides which one this is:
                // `(e)` stays the parenthesized expression it always was, and
                // a tuple has two or more elements (`design/syntax.md (kelp wiki)` 3).
                self.advance();
                if self.check(&TokenKind::RParen) {
                    // `()` is the absent value, of type `unit` (`design/syntax.md (kelp wiki)`
                    // 3, 6.4). It is an empty block: a block with no tail
                    // evaluates to `unit`, so `Ok(value = ())` builds a
                    // `Result<unit, E>` whose payload occupies no storage.
                    let close = self.advance().span;
                    return Ok(Expr::Block(Block {
                        stmts: Vec::new(),
                        span: tok.span.to(close),
                    }));
                }
                let expr = self.parse_expr()?;
                if !self.check(&TokenKind::Comma) {
                    self.expect(TokenKind::RParen, "')'")?;
                    return Ok(expr);
                }
                let mut elems = vec![expr];
                while self.check(&TokenKind::Comma) {
                    self.advance();
                    elems.push(self.parse_expr()?);
                }
                let end = self.expect(TokenKind::RParen, "')' to close the tuple")?.span;
                Ok(Expr::TupleLit {
                    elems,
                    span: tok.span.to(end),
                })
            }
            TokenKind::LBrace => {
                let block = self.parse_block()?;
                Ok(Expr::Block(block))
            }
            TokenKind::LBracket => self.parse_array_literal(tok.span),
            TokenKind::Fun => self.parse_fn_literal(),
            TokenKind::If => self.parse_if(),
            TokenKind::When => self.parse_when(),
            other => Err(ParseError {
                help: foreign_hint_for(&other),
                message: format!("unexpected token {other:?}"),
                span: tok.span,
            }),
        }
    }
}
