use super::*;

impl Parser {
    /// `[a, b, c]` and `[x; N]`: the two array literals (A-18). The list form
    /// fixes the length by how many elements were written; the repeat form
    /// takes it from a compile-time constant, which is what makes a zeroed
    /// buffer (`[0; 4096]`) one expression rather than 4096 of them.
    ///
    /// `[]` is refused here rather than later: with no elements there is no
    /// element type to infer and no length, so there is nothing a later pass
    /// could do with it.
    pub(crate) fn parse_array_literal(&mut self, start: Span) -> PResult<Expr> {
        self.expect(TokenKind::LBracket, "'['")?;
        if self.check(&TokenKind::RBracket) {
            return Err(ParseError {
                help: None,
                message: "an array literal needs at least one element ('[a, b, c]') or a repeat ('[x; N]')"
                    .to_string(),
                span: start.to(self.peek().span),
            });
        }
        let first = self.parse_expr()?;
        if self.check(&TokenKind::Semi) {
            self.advance();
            let len = self.parse_expr()?;
            let end = self.expect(TokenKind::RBracket, "']' to close the array literal")?.span;
            return Ok(Expr::ArrayRepeat {
                value: Box::new(first),
                len: Box::new(len),
                span: start.to(end),
            });
        }
        let mut elems = vec![first];
        while self.check(&TokenKind::Comma) {
            self.advance();
            if self.check(&TokenKind::RBracket) {
                break;
            }
            elems.push(self.parse_expr()?);
        }
        let end = self.expect(TokenKind::RBracket, "']' to close the array literal")?.span;
        Ok(Expr::ArrayLit {
            elems,
            span: start.to(end),
        })
    }

    /// Parses a full string template, starting at its `TemplateStart` (the
    /// lexer has already split the literal into chunk/interpolation tokens).
    pub(crate) fn parse_template(&mut self) -> PResult<Expr> {
        let start = self
            .expect(TokenKind::TemplateStart, "start of a string template")?
            .span;
        let mut parts = Vec::new();
        loop {
            match self.peek_kind().clone() {
                TokenKind::StrChunk(text) => {
                    self.advance();
                    if !text.is_empty() {
                        parts.push(TemplatePart::Text(text));
                    }
                }
                TokenKind::TemplateExprStart => {
                    self.advance();
                    let value = self.parse_expr()?;
                    self.expect(TokenKind::TemplateExprEnd, "'} ' ending the '${...}' interpolation")?;
                    parts.push(TemplatePart::Expr(value));
                }
                TokenKind::TemplateEnd => {
                    let end = self.advance().span;
                    return Ok(Expr::Template {
                        parts,
                        span: start.to(end),
                    });
                }
                other => {
                    return Err(ParseError {
                        help: None,
                        message: format!(
                            "malformed string template: expected literal text or '${{expr}}', found {}", other.describe()
                        ),
                        span: self.peek().span,
                    })
                }
            }
        }
    }

    pub(crate) fn parse_if(&mut self) -> PResult<Expr> {
        let start = self.expect(TokenKind::If, "'if'")?.span;
        self.parse_if_link(start)
    }

    /// `elif cond { ... }`: the `else if` link written as one keyword
    /// (`design/syntax.md (kelp wiki)` 6.1). The token is consumed and the rest of the link
    /// goes through `parse_if_link`, exactly as `else if` does, so the two
    /// spellings cannot mean two different things.
    pub(crate) fn parse_elif(&mut self) -> PResult<Expr> {
        let start = self.expect(TokenKind::Elif, "'elif'")?.span;
        self.parse_if_link(start)
    }

    /// One link of an `if` chain: the optional `let` pattern, the condition,
    /// the block, and whatever follows (nothing, `else`, `else if`, or
    /// `elif`). Shared by `if` and `elif`, which is why a chain may mix them.
    pub(crate) fn parse_if_link(&mut self, start: Span) -> PResult<Expr> {
        // `if let PATTERN = EXPR { ... }` - a pattern condition rather than a
        // boolean one. `let` is only an identifier everywhere else in the
        // language, so this is recognized by the token pair rather than by
        // reserving the word; the one thing it costs is that a binding
        // *named* `let` cannot be the condition of an `if` without
        // parentheses.
        if matches!(self.peek_kind(), TokenKind::Ident(word) if word == "let") {
            return self.parse_if_let(start);
        }
        let cond = self.parse_head_expr()?;
        let then_block = self.parse_block()?;
        let mut span = start.to(then_block.span);
        let else_block = if self.check(&TokenKind::Else) {
            self.advance();
            if self.check(&TokenKind::If) {
                let elif = self.parse_if()?;
                span = span.to(elif.span());
                Some(Box::new(ElseBranch::If(elif)))
            } else {
                let block = self.parse_block()?;
                span = span.to(block.span);
                Some(Box::new(ElseBranch::Block(block)))
            }
        } else if self.check(&TokenKind::Elif) {
            let link = self.parse_elif()?;
            span = span.to(link.span());
            Some(Box::new(ElseBranch::If(link)))
        } else {
            None
        };
        Ok(Expr::If {
            cond: Box::new(cond),
            then_block,
            else_block,
            span,
        })
    }

    /// `if let PATTERN = EXPR { THEN } [else ...]`, desugared here into a
    /// block that binds the scrutinee to a fresh local and matches it with the
    /// pattern form of `when`:
    ///
    /// ```text
    /// { val #iflet0 = EXPR  when #iflet0 { PATTERN -> { THEN } else -> { ELSE } } }
    /// ```
    ///
    /// Nothing downstream needs a new case - and binding the scrutinee first is
    /// what makes a temporary legal here, since the pattern form of `when`
    /// requires a *place* (an owned payload binds as a borrow of it, and a
    /// borrow of a temporary would not outlive the arm).
    /// The payload binds of `Variant(a, _, c)` after the variant name has been
    /// read: the parenthesised list is optional, so `Empty` binds nothing.
    pub(crate) fn parse_variant_binds(&mut self, variant: String, vspan: Span) -> PResult<VariantPattern> {
        let mut binds = Vec::new();
        if self.check(&TokenKind::LParen) {
            self.advance();
            while !self.check(&TokenKind::RParen) {
                let (bind, _) = self.expect_ident()?;
                binds.push(if bind == "_" { None } else { Some(bind) });
                if self.check(&TokenKind::Comma) {
                    self.advance();
                } else {
                    break;
                }
            }
            self.expect(TokenKind::RParen, "')'")?;
        }
        Ok(VariantPattern {
            variant,
            binds,
            span: vspan,
        })
    }

    /// `let PATTERN = EXPR else { BLOCK }` (`design/syntax.md (kelp wiki)` 4.1), read up to
    /// and including the else block. The statement is only ever a `let` that
    /// starts a statement and is followed by a name or a parenthesis.
    pub(crate) fn at_let_else(&self) -> bool {
        matches!(self.peek_kind(), TokenKind::Ident(name) if name == "let")
            && matches!(self.peek2_kind(), TokenKind::Ident(_) | TokenKind::LParen)
    }

    pub(crate) fn parse_let_else_head(&mut self) -> PResult<(VariantPattern, Expr, Block, Span)> {
        let start = self.advance().span; // `let`
        if self.check(&TokenKind::LParen) {
            return Err(ParseError {
                help: None,
                message: "let-else: a tuple pattern always matches, so the else block could never run. Fix: write 'val (a, b) = ...' with no else, or match an enum variant such as 'Some(x)' here".to_string(),
                span: self.peek().span,
            });
        }
        let (name, nspan) = self.expect_ident()?;
        if !self.check(&TokenKind::LParen) && self.check(&TokenKind::Eq) {
            return Err(ParseError {
                help: foreign::hint("let"),
                message: format!(
                    "let-else: '{name}' is a plain name, which always matches, so the else block could never run. Fix: write 'val {name} = ...' with no else, or match an enum variant such as 'Some({name})' here"
                ),
                span: nspan,
            });
        }
        let pattern = self.parse_variant_binds(name, nspan)?;
        self.expect(TokenKind::Eq, "'=' after the pattern")?;
        let init = self.parse_head_expr()?;
        if !self.check(&TokenKind::Else) {
            return Err(ParseError {
                help: None,
                message: "let-else: a 'let' with a pattern needs an 'else' block that leaves the enclosing code. Fix: add 'else { return ... }' (or break, continue, panic), or use 'if let' when the no-match case needs no exit".to_string(),
                span: self.peek().span,
            });
        }
        self.advance();
        let block = self.parse_block()?;
        let span = start.to(block.span);
        Ok((pattern, init, block, span))
    }

    pub(crate) fn parse_if_let(&mut self, start: Span) -> PResult<Expr> {
        self.advance(); // `let`
        let (variant, vspan) = self.expect_ident()?;
        let pattern = self.parse_variant_binds(variant, vspan)?;
        self.expect(TokenKind::Eq, "'=' after the pattern")?;
        let scrutinee = self.parse_head_expr()?;
        let scrutinee_span = scrutinee.span();
        let then_block = self.parse_block()?;
        let mut span = start.to(then_block.span);
        let else_expr = if self.check(&TokenKind::Else) {
            self.advance();
            if self.check(&TokenKind::If) {
                let elif = self.parse_if()?;
                span = span.to(elif.span());
                elif
            } else {
                let block = self.parse_block()?;
                span = span.to(block.span);
                Expr::Block(block)
            }
        } else if self.check(&TokenKind::Elif) {
            // `elif let Foo(x) = e { ... }` - the chained pattern link, in the
            // spelling 6.1 prefers. Same link, same function.
            let link = self.parse_elif()?;
            span = span.to(link.span());
            link
        } else {
            // No `else`, so the whole thing produces `Unit` - exactly what an
            // `if` without one does. An empty block is the `Unit` arm.
            Expr::Block(Block {
                stmts: Vec::new(),
                span: then_block.span,
            })
        };
        // See the long comment below for why a place gets no synthesized
        // binding at all, and anything else still does. A field chain over a
        // temporary stages its *root* (`mk().t` binds `mk()` and matches
        // `#iflet0.t`), so the temporary is freed once, at the end of the
        // desugared block, and the field is borrowed from a place that
        // outlives the arm.
        let (scrutinee_stmt, scrutinee_ref): (Option<Stmt>, Expr) =
            match temporary_root(&scrutinee).cloned() {
                Some(root) => {
                    self.if_lets += 1;
                    let tmp = format!("{}{}", IF_LET_PREFIX, self.if_lets - 1);
                    let root_span = root.span();
                    let stmt = Stmt::Let {
                        name: tmp.clone(),
                        mutable: false,
                        ty: None,
                        init: root,
                        span: root_span,
                    };
                    let rebased = rebase_root(&scrutinee, &Expr::Ident(tmp, scrutinee_span));
                    (Some(stmt), rebased)
                }
                None => (None, scrutinee),
            };
        let when_expr = Stmt::Expr {
            expr: Expr::When {
                scrutinee: Box::new(scrutinee_ref),
                arms: vec![
                    WhenArm {
                        pattern: Some(pattern),
                        body: Expr::Block(then_block),
                        span,
                    },
                    WhenArm {
                        pattern: None,
                        body: else_expr,
                        span,
                    },
                ],
                span,
                let_else: false,
            },
            span,
        };
        // The `when` is the block's last statement, which is what makes it the
        // block's value (statements are positional in this language - there is
        // no `tail` field to set).
        //
        // A scrutinee that is already a *place* (`if let Ok(v) = r`) is used
        // directly, with no synthesized binding at all - exactly what a bare
        // `when r { ... }` already does (6.1's own note: "a bare `when` needs
        // a place, since a payload borrow has to outlive the arm", and
        // matching a place never moves it, only a field of it). This is what
        // makes an `else if let PAT2 = r { ... }` chained after `if let PAT1
        // = r { ... }` legal: each link only *reads* `r` to test its tag, so
        // reading it twice is fine, the same way `when`'s own multi-arm form
        // reads its one scrutinee once and tests it repeatedly. Binding it to
        // a fresh `val #iflet0 = r` first - every earlier version of this
        // desugaring did that unconditionally - makes the *first* `if let`
        // look like it moves `r` into the temporary, and the next `else if
        // let`'s own re-evaluation of the same source expression then reads
        // an already-moved place. A scrutinee that is *not* a place (a call's
        // result, a literal) still needs the temporary, because `when`
        // requires one and there is nothing else to borrow through.
        match scrutinee_stmt {
            Some(stmt) => Ok(Expr::Block(Block {
                stmts: vec![stmt, when_expr],
                span,
            })),
            None => Ok(Expr::Block(Block {
                stmts: vec![when_expr],
                span,
            })),
        }
    }

    /// Parses both forms of `when`:
    ///
    /// - `when { cond -> value, ..., else -> value }` - no scrutinee, so it
    ///   is desugared here and now to a nested `if`/`else` chain; `infer.rs`
    ///   and the removed C backend never see this form at all.
    /// - `when expr { Variant(bind) -> value, ..., else -> value }`  -  a
    ///   pattern match (A-1), kept as `Expr::When` because only `infer.rs`
    ///   knows the scrutinee's type, and therefore which variants exist.
    ///
    /// The two never collide: the boolean form has no scrutinee.
    pub(crate) fn parse_when(&mut self) -> PResult<Expr> {
        let start = self.expect(TokenKind::When, "'when'")?.span;
        if !self.check(&TokenKind::LBrace) {
            return self.parse_when_patterns(start);
        }
        self.expect(TokenKind::LBrace, "'{'")?;
        self.skip_newlines();

        let mut arms: Vec<(Option<Expr>, Expr)> = Vec::new();
        while !self.check(&TokenKind::RBrace) {
            let cond = if self.check(&TokenKind::Else) {
                self.advance();
                None
            } else {
                Some(self.parse_expr()?)
            };
            self.expect(TokenKind::Arrow, "'->' or '=>'")?;
            let body = self.parse_expr()?;
            arms.push((cond, body));

            // A newline separates arms, but it is not *required* to: inside
            // `(` `)` the lexer does not emit newline tokens at all (see
            // `Lexer::paren_depth`), and a `when` is allowed to be a call
            // argument. An arm head is unambiguous either way - every arm is
            // `head -> value` (or `head => value`), and the arrow appears
            // nowhere else in the language.
            if !self.check(&TokenKind::RBrace) {
                self.skip_newlines();
            }
        }
        let end = self.expect(TokenKind::RBrace, "'}'")?.span;
        let span = start.to(end);

        if arms.is_empty() {
            return Err(ParseError {
                help: None,
                message: "'when' needs at least one arm".to_string(),
                span,
            });
        }

        // Fold right-to-left: the trailing `else` (if any) becomes the base
        // else-block; every preceding arm wraps the accumulator in another
        // `if cond { body } else { <accumulator> }`.
        let mut acc: Option<ElseBranch> = None;
        for (cond, body) in arms.into_iter().rev() {
            let body_span = body.span();
            let body_block = Block {
                stmts: vec![Stmt::Expr {
                    expr: body,
                    span: body_span,
                }],
                span: body_span,
            };
            match cond {
                None => acc = Some(ElseBranch::Block(body_block)),
                Some(c) => {
                    let if_span = c.span().to(body_span);
                    let if_expr = Expr::If {
                        cond: Box::new(c),
                        then_block: body_block,
                        else_block: acc.take().map(Box::new),
                        span: if_span,
                    };
                    acc = Some(ElseBranch::If(if_expr));
                }
            }
        }

        match acc.unwrap() {
            ElseBranch::If(e) => Ok(e),
            ElseBranch::Block(b) => Ok(Expr::Block(b)),
        }
    }

    /// `when s { Circle(r) -> ... else -> ... }` - the pattern form. Arm
    /// heads are *shapes*, not expressions: a variant name with an optional
    /// `(bind, ...)` list, where each bind is an identifier or `_`.
    pub(crate) fn parse_when_patterns(&mut self, start: Span) -> PResult<Expr> {
        let scrutinee = self.parse_head_expr()?;
        self.expect(TokenKind::LBrace, "'{'")?;
        self.skip_newlines();

        if self.check(&TokenKind::LParen) {
            return self.parse_when_tuple_arms(start, scrutinee);
        }
        let mut arms: Vec<WhenArm> = Vec::new();
        while !self.check(&TokenKind::RBrace) {
            let pattern = if self.check(&TokenKind::Else) {
                self.advance();
                None
            } else {
                let (variant, vspan) = self.expect_ident()?;
                let mut binds = Vec::new();
                if self.check(&TokenKind::LParen) {
                    self.advance();
                    while !self.check(&TokenKind::RParen) {
                        let (bind, _) = self.expect_ident()?;
                        // `_` is the ignore-a-payload binding; it lexes as an
                        // identifier because an identifier may start with `_`.
                        binds.push(if bind == "_" { None } else { Some(bind) });
                        if self.check(&TokenKind::Comma) {
                            self.advance();
                        } else {
                            break;
                        }
                    }
                    self.expect(TokenKind::RParen, "')'")?;
                }
                Some(VariantPattern {
                    variant,
                    binds,
                    span: vspan,
                })
            };
            self.expect(TokenKind::Arrow, "'->' or '=>'")?;
            let body = self.parse_expr()?;
            let arm_span = pattern
                .as_ref()
                .map(|p| p.span)
                .unwrap_or_else(|| body.span())
                .to(body.span());
            arms.push(WhenArm {
                pattern,
                body,
                span: arm_span,
            });

            // A newline separates arms, but it is not *required* to: inside
            // `(` `)` the lexer does not emit newline tokens at all (see
            // `Lexer::paren_depth`), and a `when` is allowed to be a call
            // argument. An arm head is unambiguous either way - every arm is
            // `head -> value` (or `head => value`), and the arrow appears
            // nowhere else in the language.
            if !self.check(&TokenKind::RBrace) {
                self.skip_newlines();
            }
        }
        let end = self.expect(TokenKind::RBrace, "'}'")?.span;
        if arms.is_empty() {
            return Err(ParseError {
                help: None,
                message: "'when' needs at least one arm".to_string(),
                span: start.to(end),
            });
        }
        Ok(Expr::When {
            scrutinee: Box::new(scrutinee),
            arms,
            span: start.to(end),
            let_else: false,
        })
    }

    /// `when (a, b) { (Some(x), _) -> ... (n, None) -> ... else -> ... }`: arms
    /// whose heads are tuple patterns (`design/syntax.md (kelp wiki)` 4). A position is a
    /// name, `_`, or a variant pattern (a capitalized name, with a bind list in
    /// parentheses when it has a payload). Nothing below the parser learns
    /// about it: the scrutinee is bound once and the arms are rewritten into
    /// nested single-column `when`s, one level per refutable position, so the
    /// existing checks (every variant covered, the payload arity, the moves)
    /// apply to each level. A row is tried in order and a position that
    /// matches everything is bound by name wherever it is reached.
    pub(crate) fn parse_when_tuple_arms(&mut self, start: Span, scrutinee: Expr) -> PResult<Expr> {
        let mut rows: Vec<TupleRow> = Vec::new();
        let mut width: Option<usize> = None;
        while !self.check(&TokenKind::RBrace) {
            let mut pats: Vec<TuplePat> = Vec::new();
            let head = self.peek().span;
            if self.check(&TokenKind::Else) {
                self.advance();
            } else {
                if !self.check(&TokenKind::LParen) {
                    return Err(ParseError {
                        help: None,
                        message: "every arm of a 'when' over a tuple is a tuple pattern '(a, b)' or 'else'".to_string(),
                        span: head,
                    });
                }
                self.advance();
                loop {
                    pats.push(self.parse_tuple_position()?);
                    if self.check(&TokenKind::Comma) {
                        self.advance();
                        continue;
                    }
                    break;
                }
                self.expect(TokenKind::RParen, "')' to close the tuple pattern")?;
                if pats.len() < 2 {
                    return Err(ParseError {
                        help: None,
                        message: "a tuple pattern needs two or more positions".to_string(),
                        span: head,
                    });
                }
                match width {
                    None => width = Some(pats.len()),
                    Some(w) if w != pats.len() => {
                        return Err(ParseError {
                            help: None,
                            message: format!("this tuple pattern has {} positions and the first arm has {w}", pats.len()),
                            span: head,
                        })
                    }
                    Some(_) => {}
                }
            }
            self.expect(TokenKind::Arrow, "'->' or '=>'")?;
            let body = self.parse_expr()?;
            rows.push(TupleRow { pats, binds: Vec::new(), body, span: head });
            if !self.check(&TokenKind::RBrace) {
                self.skip_newlines();
            }
        }
        let end = self.expect(TokenKind::RBrace, "'}'")?.span;
        let span = start.to(end);
        let Some(width) = width else {
            return Err(ParseError {
                help: None,
                message: "a 'when' over a tuple needs at least one tuple pattern arm".to_string(),
                span,
            });
        };
        for row in rows.iter_mut() {
            if row.pats.is_empty() {
                row.pats = vec![TuplePat::Any(None); width];
            }
        }
        // A scrutinee written as a tuple literal is never built: its elements
        // are the positions, so a borrowed element (a parameter, a field) is
        // matched where it lives, which a tuple value could not hold. Anything
        // else that is not already a place is bound once first.
        let mut stmts: Vec<Stmt> = Vec::new();
        let stage = |this: &mut Parser, e: Expr, stmts: &mut Vec<Stmt>| -> Expr {
            if is_place_chain(&e) {
                return e;
            }
            this.when_temps += 1;
            let name = format!("{}{}", WHEN_TUPLE_PREFIX, this.when_temps - 1);
            let espan = e.span();
            stmts.push(Stmt::Let { name: name.clone(), mutable: false, ty: None, init: e, span: espan });
            Expr::Ident(name, espan)
        };
        let cols: Vec<Expr> = match scrutinee {
            Expr::TupleLit { elems, .. } if elems.len() == width => {
                let mut cols = Vec::new();
                for e in elems {
                    cols.push(stage(self, e, &mut stmts));
                }
                cols
            }
            other => {
                let whole = stage(self, other, &mut stmts);
                (0..width)
                    .map(|i| Expr::FieldAccess {
                        base: Box::new(whole.clone()),
                        field: crate::types::tuple_field_name(i),
                        span,
                    })
                    .collect()
            }
        };
        let inner = self.compile_tuple_rows(rows, &cols, span)?;
        stmts.push(Stmt::Expr { expr: inner, span });
        Ok(Expr::Block(Block { stmts, span }))
    }

    /// One position of a tuple pattern: `_`, a lowercase name, or a variant
    /// (`None`, `Some(x)`, `Rect(w, _)`).
    pub(crate) fn parse_tuple_position(&mut self) -> PResult<TuplePat> {
        self.nest()?;
        let r = self.parse_tuple_position_inner();
        self.depth -= 1;
        r
    }

    pub(crate) fn parse_tuple_position_inner(&mut self) -> PResult<TuplePat> {
        // A nested tuple pattern.
        if self.check(&TokenKind::LParen) {
            let open = self.advance().span;
            let mut elems = Vec::new();
            loop {
                elems.push(self.parse_tuple_position()?);
                if self.check(&TokenKind::Comma) {
                    self.advance();
                    continue;
                }
                break;
            }
            let close = self.expect(TokenKind::RParen, "')' to close the tuple pattern")?.span;
            if elems.len() < 2 {
                return Err(ParseError {
                    help: None,
                    message: "a tuple pattern needs two or more positions".to_string(),
                    span: open.to(close),
                });
            }
            return Ok(TuplePat::Tuple(elems, open.to(close)));
        }
        // A literal position.
        let lit_span = self.peek().span;
        match self.peek_kind().clone() {
            TokenKind::IntLit(v) => {
                self.advance();
                return Ok(TuplePat::Lit { key: format!("i{v}"), expr: Expr::IntLit(v, lit_span) });
            }
            TokenKind::Minus if matches!(self.peek2_kind(), TokenKind::IntLit(_)) => {
                self.advance();
                let TokenKind::IntLit(v) = self.peek_kind().clone() else { unreachable!() };
                let end = self.advance().span;
                return Ok(TuplePat::Lit {
                    key: format!("-i{v}"),
                    expr: Expr::Unary {
                        op: UnOp::Neg,
                        expr: Box::new(Expr::IntLit(v, end)),
                        span: lit_span.to(end),
                    },
                });
            }
            TokenKind::CharLit(c) => {
                self.advance();
                return Ok(TuplePat::Lit { key: format!("c{c}"), expr: Expr::CharLit(c, lit_span) });
            }
            TokenKind::True | TokenKind::False => {
                let value = matches!(self.peek_kind(), TokenKind::True);
                self.advance();
                return Ok(TuplePat::Lit { key: format!("b{value}"), expr: Expr::BoolLit(value, lit_span) });
            }
            _ => {}
        }
        let (name, span) = self.expect_ident()?;
        if name == "_" {
            return Ok(TuplePat::Any(None));
        }
        let variant = name.chars().next().is_some_and(|c| c.is_ascii_uppercase());
        if !variant {
            return Ok(TuplePat::Any(Some(name)));
        }
        let mut binds = Vec::new();
        if self.check(&TokenKind::LParen) {
            self.advance();
            while !self.check(&TokenKind::RParen) {
                let (bind, _) = self.expect_ident()?;
                binds.push(if bind == "_" { None } else { Some(bind) });
                if self.check(&TokenKind::Comma) {
                    self.advance();
                } else {
                    break;
                }
            }
            self.expect(TokenKind::RParen, "')'")?;
        }
        Ok(TuplePat::Variant { name, binds, span })
    }

    /// The rows left to try, as one expression, over the tuple positions
    /// `cols`. The first row decides the shape: when it has no variant pattern
    /// left it wins (its names are bound, then its body runs); otherwise the
    /// first variant position is matched with a `when` of its own, one arm per
    /// variant the rows name, each arm holding the rows that can still match
    /// there, and an `else` arm holding the rows that match anything.
    pub(crate) fn compile_tuple_rows(&mut self, rows: Vec<TupleRow>, cols: &[Expr], span: Span) -> PResult<Expr> {
        self.nest()?;
        let r = self.compile_tuple_rows_inner(rows, cols, span);
        self.depth -= 1;
        r
    }

    pub(crate) fn compile_tuple_rows_inner(&mut self, rows: Vec<TupleRow>, cols: &[Expr], span: Span) -> PResult<Expr> {
        // A nested tuple pattern splits its position into one position per
        // element before anything else is decided, so the rest only ever sees
        // flat columns.
        if let Some(c) = rows[0].pats.iter().position(|p| matches!(p, TuplePat::Tuple(..))) {
            return self.expand_tuple_column(rows, cols, c, span);
        }
        let first = &rows[0];
        // The first position the first row has to test: a variant or a literal.
        let Some(c) = first
            .pats
            .iter()
            .position(|p| matches!(p, TuplePat::Variant { .. } | TuplePat::Lit { .. }))
        else {
            let mut stmts = Vec::new();
            for (name, value) in &first.binds {
                stmts.push(Stmt::Let { name: name.clone(), mutable: false, ty: None, init: value.clone(), span });
            }
            for (i, p) in first.pats.iter().enumerate() {
                if let TuplePat::Any(Some(name)) = p {
                    stmts.push(Stmt::Let { name: name.clone(), mutable: false, ty: None, init: cols[i].clone(), span });
                }
            }
            let body = first.body.clone();
            let body_span = body.span();
            stmts.push(Stmt::Expr { expr: body, span: body_span });
            return Ok(Expr::Block(Block { stmts, span: first.span }));
        };
        if matches!(first.pats[c], TuplePat::Lit { .. }) {
            return self.compile_literal_column(rows, cols, c, span);
        }
        // The variants named in column `c`, in order of first appearance.
        let mut variants: Vec<(String, usize, Span)> = Vec::new();
        for row in &rows {
            if let TuplePat::Variant { name, binds, span } = &row.pats[c] {
                match variants.iter().find(|(n, _, _)| n == name) {
                    None => variants.push((name.clone(), binds.len(), *span)),
                    Some((_, arity, _)) if *arity != binds.len() => {
                        return Err(ParseError {
                            help: None,
                            message: format!("'{name}' is bound with {} name(s) here and {arity} in an earlier arm", binds.len()),
                            span: *span,
                        })
                    }
                    Some(_) => {}
                }
            }
        }
        let mut arms: Vec<WhenArm> = Vec::new();
        for (vname, arity, vspan) in &variants {
            self.when_temps += 1;
            let payload: Vec<String> = (0..*arity).map(|j| format!("{}{}_{j}", WHEN_TUPLE_PREFIX, self.when_temps - 1)).collect();
            let mut group: Vec<TupleRow> = Vec::new();
            for row in &rows {
                let mut next = row.clone();
                match &row.pats[c] {
                    TuplePat::Variant { name, binds, .. } if name == vname => {
                        for (j, b) in binds.iter().enumerate() {
                            if let Some(b) = b {
                                next.binds.push((b.clone(), Expr::Ident(payload[j].clone(), *vspan)));
                            }
                        }
                    }
                    TuplePat::Variant { .. } => continue,
                    TuplePat::Any(name) => {
                        if let Some(n) = name {
                            next.binds.push((n.clone(), cols[c].clone()));
                        }
                    }
                    TuplePat::Lit { expr, .. } => {
                        return Err(ParseError {
                            help: None,
                            message: "a position cannot be matched by a variant in one arm and a literal in another".to_string(),
                            span: expr.span(),
                        })
                    }
                    TuplePat::Tuple(_, tspan) => {
                        return Err(ParseError {
                            help: None,
                            message: "a position cannot be matched by a variant in one arm and a tuple in another".to_string(),
                            span: *tspan,
                        })
                    }
                }
                next.pats[c] = TuplePat::Any(None);
                group.push(next);
            }
            let body = self.compile_tuple_rows(group, cols, span)?;
            let bspan = body.span();
            arms.push(WhenArm {
                pattern: Some(VariantPattern {
                    variant: vname.clone(),
                    binds: payload.into_iter().map(Some).collect(),
                    span: *vspan,
                }),
                body,
                span: vspan.to(bspan),
            });
        }
        let mut rest: Vec<TupleRow> = Vec::new();
        for row in &rows {
            if let TuplePat::Any(name) = &row.pats[c] {
                let mut next = row.clone();
                if let Some(n) = name {
                    next.binds.push((n.clone(), cols[c].clone()));
                }
                next.pats[c] = TuplePat::Any(None);
                rest.push(next);
            }
        }
        if !rest.is_empty() {
            // The rows that match any variant are the `else` arm. When they
            // test a literal with nothing to catch the other values, the arm
            // is dropped instead of refused: if the variants above already
            // cover the enum it was never reachable, and if they do not the
            // ordinary "not exhaustive" error names the missing variant.
            match self.compile_tuple_rows(rest, cols, span) {
                Ok(body) => {
                    let bspan = body.span();
                    arms.push(WhenArm { pattern: None, body, span: bspan });
                }
                Err(e) if e.message.starts_with(NO_LITERAL_FALLBACK) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(Expr::When {
            scrutinee: Box::new(cols[c].clone()),
            arms,
            span,
            let_else: false,
        })
    }
}
