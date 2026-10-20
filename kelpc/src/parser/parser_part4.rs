use super::*;

impl Parser {
    /// `( name: T, ... )` - one parameter list, shared by a declaration and a
    /// `fun` literal (A-4), since the two take exactly the same parameters.
    /// The flag is `true` for a declaration whose last parameter is the `...`
    /// variadic marker (gap 7); a `fun` literal refuses one at its own call
    /// site, because only a user `extern fun` can be variadic.
    pub(crate) fn parse_params(&mut self) -> PResult<(Vec<Param>, bool)> {
        self.expect(TokenKind::LParen, "'('")?;
        let mut params = Vec::new();
        let mut variadic = false;
        while !self.check(&TokenKind::RParen) {
            if self.check(&TokenKind::DotDotDot) {
                self.advance();
                if !self.check(&TokenKind::RParen) {
                    return Err(ParseError {
                        help: None,
                        message: "'...' must be the last parameter of an 'extern fun': nothing may follow it".to_string(),
                        span: self.peek().span,
                    });
                }
                variadic = true;
                break;
            }
            let (pname, _) = self.expect_ident()?;
            self.expect(TokenKind::Colon, "':'")?;
            // `mut T` requests an exclusive borrow and `move T` takes
            // ownership (A-4); this is the *only* spelling for either in
            // parameter position - `&mut T` is rejected here (the leading
            // `&` in `parse_type` expects an identifier next, not the `mut`
            // keyword token), and `&`/`mut` stay expression-only.
            let mode = self.parse_param_mode()?;
            let ty = self.parse_type()?;
            params.push(Param {
                name: pname,
                ty,
                mode,
            });
            if self.check(&TokenKind::Comma) {
                self.advance();
            } else {
                break;
            }
        }
        self.expect(TokenKind::RParen, "')'")?;
        Ok((params, variadic))
    }

    /// `fun(x: i32) => i32 = x * 2` in expression position - an anonymous
    /// function (A-4). A body is required (there is no `extern` spelling for
    /// a literal), and everything else about the header is a declaration's:
    /// the same parameter list, the same optional `=> T`.
    pub(crate) fn parse_fn_literal(&mut self) -> PResult<Expr> {
        let outer = std::mem::take(&mut self.loop_stack);
        let r = self.parse_fn_literal_inner();
        self.loop_stack = outer;
        r
    }

    pub(crate) fn parse_fn_literal_inner(&mut self) -> PResult<Expr> {
        let start = self.expect(TokenKind::Fun, "'fun'")?.span;
        let (params, variadic) = self.parse_params()?;
        if variadic {
            return Err(ParseError {
                help: None,
                message: "a 'fun' literal cannot be variadic: '...' is only legal on a user 'extern fun'"
                    .to_string(),
                span: start,
            });
        }
        let return_type = self.parse_return_type()?;
        let (body, end) = if self.check(&TokenKind::Eq) {
            self.advance();
            let e = self.parse_expr()?;
            let end = e.span();
            (FunctionBody::Expr(e), end)
        } else if self.check(&TokenKind::LBrace) {
            let b = self.parse_block()?;
            let end = b.span;
            (FunctionBody::Block(b), end)
        } else {
            return Err(ParseError {
                help: None,
                message: "a 'fun' literal needs a body: '= expr' or '{ ... }'".to_string(),
                span: self.peek().span,
            });
        };
        Ok(Expr::FnLit {
            params,
            infer_params: false,
            return_type,
            body: Box::new(body),
            span: start.to(end),
        })
    }

    /// The `mut`/`move` qualifier before a parameter's type, in either
    /// parameter position (`name: mut T` in a declaration, `fun(mut T)` in a
    /// function type). Reading at most one is what makes the two mutually
    /// exclusive: `mut` says "I write through your value", `move` says "I own
    /// it", and one parameter cannot say both.
    pub(crate) fn parse_param_mode(&mut self) -> PResult<ParamMode> {
        let mode = if self.check(&TokenKind::Mut) {
            self.advance();
            ParamMode::Mut
        } else if self.check(&TokenKind::Move) {
            self.advance();
            ParamMode::Move
        } else {
            return Ok(ParamMode::Borrow);
        };
        if self.check(&TokenKind::Mut) || self.check(&TokenKind::Move) {
            return Err(ParseError {
                help: None,
                message: "'mut' and 'move' cannot be combined on one parameter - 'mut T' is an exclusive borrow, 'move T' takes ownership".to_string(),
                span: self.peek().span,
            });
        }
        Ok(mode)
    }

    /// The rest of `fun(i32, String) => bool`, with `fun` already consumed.
    /// A parameter of a function type takes the same `mut`/`move` qualifiers
    /// a declaration's parameter takes, because the two have to agree: a
    /// written `fun(String)` is a function whose parameter is a borrow, and
    /// `fun(move String)` is one that takes ownership, exactly as
    /// `fun f(s: String)` and `fun f(s: move String)` do.
    pub(crate) fn parse_fn_type(
        &mut self,
        start: Span,
        amp_start: Option<Span>,
        name: &str,
        scoped: bool,
    ) -> PResult<TypeExpr> {
        self.expect(TokenKind::LParen, "'('")?;
        let mut params = Vec::new();
        while !self.check(&TokenKind::RParen) {
            let mode = self.parse_param_mode()?;
            let ty = self.parse_type()?;
            params.push(FnTypeParam { mode, ty });
            if self.check(&TokenKind::Comma) {
                self.advance();
            } else {
                break;
            }
        }
        let close = self.expect(TokenKind::RParen, "')'")?;
        let mut span = start.to(close.span);
        let ret = self.parse_return_type()?.map(|ty| {
            span = span.to(ty.span);
            Box::new(ty)
        });
        Ok(TypeExpr {
            name: name.to_string(),
            tuple: None,
            args: Vec::new(),
            array_elem: None,
            assoc: None,
            array_len: None,
            amp: amp_start.is_some(),
            amp_mut: false,
            dyn_trait: None,
            scoped,
            fn_params: Some(params),
            fn_ret: ret,
            const_expr: None,
            impl_trait: None,
            bound_eq: Vec::new(),
            span: amp_start.map(|a| a.to(span)).unwrap_or(span),
        })
    }

    /// `"<" type ("," type)* ">"`, with the `<` not yet consumed. Returns the
    /// arguments and the closing `>`'s span, which the expression-level
    /// speculative reader needs in order to check adjacency.
    pub(crate) fn parse_type_arg_list(&mut self) -> PResult<(Vec<TypeExpr>, Span)> {
        self.expect(TokenKind::Lt, "'<'")?;
        let mut args = Vec::new();
        loop {
            args.push(self.parse_type_arg()?);
            if self.check(&TokenKind::Comma) {
                self.advance();
                continue;
            }
            break;
        }
        let close = self.expect(TokenKind::Gt, "'>' to close the type argument list")?;
        Ok((args, close.span))
    }

    /// One argument of a `<...>` list. A type argument is either a type or,
    /// for a `const` parameter, a compile-time expression written the same way
    /// (`Buf<16>`, `Buf<SIZE * 2>`, A-20). The two are told apart by lookahead
    /// here and settled for good where the declaration's parameter list says
    /// which one it takes.
    pub(crate) fn parse_type_arg(&mut self) -> PResult<TypeExpr> {
        self.nest()?;
        let r = self.parse_type_arg_inner();
        self.depth -= 1;
        r
    }

    pub(crate) fn parse_type_arg_inner(&mut self) -> PResult<TypeExpr> {
        if self.starts_const_arg() {
            let start = self.peek().span;
            let e = self.parse_const_arg_expr()?;
            let end = e.span();
            return Ok(TypeExpr::const_arg(e, start.to(end)));
        }
        // A bare or dotted name is ambiguous: it may be a type argument or a
        // named `const` written as a const argument (`Buf<SIZE>`). It is kept
        // as a type here and reconstructed as an expression by
        // `named_const_expr` where a `const` parameter is the one being bound.
        self.parse_type()
    }

    /// Whether the tokens at the cursor begin a const argument rather than a
    /// type: an integer, boolean or character literal, a leading `-`, or a name that is
    /// called or used in an arithmetic expression (`SIZE * 2`, `size()`).
    pub(crate) fn starts_const_arg(&self) -> bool {
        match self.peek_kind() {
            TokenKind::IntLit(_)
            | TokenKind::CharLit(_)
            | TokenKind::True
            | TokenKind::False
            | TokenKind::Minus => true,
            TokenKind::Ident(_) => {
                let mut i = self.pos + 1;
                while matches!(self.tokens.get(i).map(|t| &t.kind), Some(TokenKind::Dot))
                    && matches!(
                        self.tokens.get(i + 1).map(|t| &t.kind),
                        Some(TokenKind::Ident(_))
                    )
                {
                    i += 2;
                }
                matches!(
                    self.tokens.get(i).map(|t| &t.kind),
                    Some(TokenKind::LParen)
                        | Some(TokenKind::Plus)
                        | Some(TokenKind::Minus)
                        | Some(TokenKind::Star)
                        | Some(TokenKind::Slash)
                        | Some(TokenKind::Percent)
                        | Some(TokenKind::Amp)
                        | Some(TokenKind::Pipe)
                        | Some(TokenKind::Caret)
                )
            }
            _ => false,
        }
    }

    /// `<T, U>` in an *expression*, immediately followed by `(` - the explicit
    /// instantiation of a generic construction or call, `Pair<i32, String>(...)`
    /// or `Option.Some<i32>(v = 1)` (A-2).
    ///
    /// This is the one place the spelling is ambiguous: `a < b > (c)` is a
    /// valid comparison chain, and `a < b, c > (d)` is not (a comma cannot
    /// appear in an expression there), so a lookahead that simply tries is not
    /// safe. The rule is adjacency - the `<` must touch the name and the `(`
    /// must touch the `>` - so comparison code, which is written with spaces,
    /// can never be misread. Returns `None` and restores the token position
    /// when the form does not apply.
    pub(crate) fn try_explicit_type_args(&mut self) -> PResult<Option<Vec<TypeExpr>>> {
        if !self.check(&TokenKind::Lt) {
            return Ok(None);
        }
        let prev_end = if self.pos == 0 {
            0
        } else {
            self.tokens[self.pos - 1].span.end
        };
        if self.peek().span.start != prev_end {
            return Ok(None);
        }
        let save = self.pos;
        if let Ok((args, close)) = self.parse_type_arg_list() {
            if self.check(&TokenKind::LParen) && self.peek().span.start == close.end {
                return Ok(Some(args));
            }
        }
        self.pos = save;
        Ok(None)
    }

    pub(crate) fn parse_block(&mut self) -> PResult<Block> {
        let start = self.expect(TokenKind::LBrace, "'{'")?.span;
        self.parse_block_body(start)
    }

    /// The statements and closing `}` of a block whose `{` (and, for a block
    /// with parameters, whose `{ x, y ->` header) the caller has consumed.
    pub(crate) fn parse_block_body(&mut self, start: Span) -> PResult<Block> {
        self.nest()?;
        let r = self.parse_block_body_inner(start);
        self.depth -= 1;
        r
    }

    pub(crate) fn parse_block_body_inner(&mut self, start: Span) -> PResult<Block> {
        let saved = std::mem::replace(&mut self.no_trailing_block, false);
        self.skip_newlines();
        let stmts = self.parse_stmts_to_rbrace()?;
        let end = self.expect(TokenKind::RBrace, "'}'")?.span;
        self.no_trailing_block = saved;
        Ok(Block {
            stmts,
            span: start.to(end),
        })
    }

    /// The help line for "expected a newline or '}' after a statement": the
    /// foreign word at the stray token or the one just before it (`let x = 5`
    /// stops at `x`, after `let`), else the statement rule.
    pub(crate) fn statement_end_hint(&self) -> Option<String> {
        self.foreign_hint_at(self.pos)
            .or_else(|| self.pos.checked_sub(1).and_then(|prev| self.foreign_hint_at(prev)))
            .or_else(|| Some("Kelp has no ';' between statements. Put the next statement on its own line.".to_string()))
    }

    /// The statements of a block up to, not including, its closing `}`.
    ///
    /// A `let PATTERN = EXPR else { BLOCK }` takes every statement after it
    /// into its pattern arm, so the rest of the block is parsed here, by a
    /// recursive call, and the statement becomes a `when` whose pattern arm
    /// is that rest and whose `else` arm is the else block (`design/syntax.md (kelp wiki)`
    /// 4.1). The `when` is the last statement of the block, so its value is
    /// the rest's value, and every existing rule for `when` arms, drops and
    /// divergence applies to the let-else unchanged.
    pub(crate) fn parse_stmts_to_rbrace(&mut self) -> PResult<Vec<Stmt>> {
        let mut stmts = Vec::new();
        while !self.check(&TokenKind::RBrace) {
            if self.at_let_else() {
                let (pattern, init, else_block, span) = self.parse_let_else_head()?;
                if !self.check(&TokenKind::RBrace) {
                    if !self.check(&TokenKind::Newline) {
                        return Err(ParseError {
                            help: self.statement_end_hint(),
                            message: format!(
                                "expected a newline or '}}' after a statement, found {}",
                                self.peek_kind().describe()
                            ),
                            span: self.peek().span,
                        });
                    }
                    self.skip_newlines();
                }
                let rest = self.parse_stmts_to_rbrace()?;
                let else_span = else_block.span;
                stmts.push(Stmt::Expr {
                    expr: Expr::When {
                        scrutinee: Box::new(init),
                        arms: vec![
                            WhenArm {
                                pattern: Some(pattern),
                                body: Expr::Block(Block { stmts: rest, span }),
                                span,
                            },
                            WhenArm {
                                pattern: None,
                                body: Expr::Block(else_block),
                                span: else_span,
                            },
                        ],
                        span,
                        let_else: true,
                    },
                    span,
                });
                return Ok(stmts);
            }
            let stmt = self.parse_stmt()?;
            stmts.push(stmt);
            if !self.check(&TokenKind::RBrace) {
                if !self.check(&TokenKind::Newline) {
                    return Err(ParseError {
                        help: self.statement_end_hint(),
                        message: format!(
                            "expected a newline or '}}' after a statement, found {}",
                            self.peek_kind().describe()
                        ),
                        span: self.peek().span,
                    });
                }
                self.skip_newlines();
            }
        }
        Ok(stmts)
    }

    pub(crate) fn parse_stmt(&mut self) -> PResult<Stmt> {
        match self.peek_kind() {
            TokenKind::Val | TokenKind::Var => self.parse_let_stmt(),
            TokenKind::While | TokenKind::For => self.parse_loop_construct(None),
            // `loop { ... }`: an endless loop, `while true` under another name.
            // Contextual, like `break`: `loop` followed by `{` at the start of
            // a statement.
            TokenKind::Ident(name) if name == "loop" && matches!(self.peek2_kind(), TokenKind::LBrace) => {
                self.parse_loop_construct(None)
            }
            // `'outer: while ...` / `'outer: for ...` / `'outer: loop { ... }`.
            TokenKind::Label(name) => {
                let label = name.clone();
                self.advance();
                self.expect(TokenKind::Colon, "':' after the loop label")?;
                let loop_ahead = matches!(self.peek_kind(), TokenKind::While | TokenKind::For)
                    || matches!(self.peek_kind(), TokenKind::Ident(n) if n == "loop");
                if !loop_ahead {
                    return Err(ParseError {
                        help: None,
                        message: format!("the label '{label} has to be followed by a 'while', 'for' or 'loop'"),
                        span: self.peek().span,
                    });
                }
                self.parse_loop_construct(Some(label))
            }
            TokenKind::Defer => self.parse_defer_stmt(),
            TokenKind::Errdefer => self.parse_errdefer_stmt(),
            TokenKind::Return => self.parse_return_stmt(),
            // `break` and `continue` are read as ordinary names everywhere
            // else, so a bare one here (the statement ends after it) is the
            // loop control statement (`kelp wiki page language/iterators.md`); `break + 1` and
            // `break()` stay expressions.
            TokenKind::Ident(name)
                if (name == "break" || name == "continue")
                    && (self.statement_ends_here()
                        || matches!(self.peek2_kind(), TokenKind::Label(_))
                        || (name == "break" && self.break_value_at(1))) =>
            {
                let is_break = name == "break";
                let span = self.advance().span;
                let label = if let TokenKind::Label(label) = self.peek_kind().clone() {
                    let lspan = self.advance().span;
                    Some((label, span.to(lspan)))
                } else {
                    None
                };
                // `break value`: the value of the `loop` it leaves.
                let value = if is_break && self.break_value_at(0) {
                    Some(self.parse_expr()?)
                } else {
                    None
                };
                if let Some((label, lspan)) = label {
                    return self.parse_labelled_exit(is_break, &label, lspan, value);
                }
                self.parse_exit(is_break, span, value)
            }
            // `pub` leads a declaration, and a declaration is not a statement,
            // so the message names the forms it can lead (A-13) rather than
            // reading the keyword as a start-of-expression.
            TokenKind::Pub => Err(ParseError {
                help: None,
                message: "'pub' applies to a top-level 'fun', 'extern fun', 'datac', 'enum', 'const' or 'static', and only at the top level of a file".to_string(),
                span: self.peek().span,
            }),
            _ => {
                let expr = self.parse_expr()?;
                let span = expr.span();
                Ok(Stmt::Expr { expr, span })
            }
        }
    }

    pub(crate) fn parse_let_stmt(&mut self) -> PResult<Stmt> {
        let mutable = self.check(&TokenKind::Var);
        let start = if mutable {
            self.expect(TokenKind::Var, "'var'")?.span
        } else {
            self.expect(TokenKind::Val, "'val'")?.span
        };
        // `val (lo, hi) = min_max(xs)`: a destructuring binding takes one
        // tuple value apart (`design/syntax.md (kelp wiki)` 4). The pattern is a comma-
        // separated list in parentheses, so the one thing that tells it from
        // the ordinary `val name = ...` is the `(` right after the keyword.
        if self.check(&TokenKind::LParen) {
            let pattern = self.parse_binding_pattern()?;
            if self.check(&TokenKind::Colon) {
                return Err(ParseError {
                    help: None,
                    message: "a destructuring binding takes no written type: every position's type comes from the initializer, in order; bind the whole value first ('val p = ...') and write the type there"
                        .to_string(),
                    span: self.peek().span,
                });
            }
            self.expect(TokenKind::Eq, "'='")?;
            let init = self.parse_expr()?;
            let span = start.to(init.span());
            return Ok(Stmt::LetTuple {
                pattern,
                mutable,
                init,
                span,
            });
        }
        let (name, _) = self.expect_ident()?;
        let ty = if self.check(&TokenKind::Colon) {
            self.advance();
            Some(self.parse_type()?)
        } else {
            None
        };
        self.expect(TokenKind::Eq, "'='")?;
        let init = self.parse_expr()?;
        let span = start.to(init.span());
        Ok(Stmt::Let {
            name,
            mutable,
            ty,
            init,
            span,
        })
    }

    /// `(lo, hi)` / `((a, b), c)`: the pattern a destructuring binding takes
    /// its value apart with (`design/syntax.md (kelp wiki)` 4). A position is a name, `_` to
    /// ignore it, or a nested pattern, and there are two or more of them:
    /// `(x)` is not a pattern but the parenthesized name `x`, exactly as `(T)`
    /// is not a tuple type.
    pub(crate) fn parse_binding_pattern(&mut self) -> PResult<BindingPattern> {
        self.nest()?;
        let r = self.parse_binding_pattern_inner();
        self.depth -= 1;
        r
    }

    pub(crate) fn parse_binding_pattern_inner(&mut self) -> PResult<BindingPattern> {
        let open = self
            .expect(TokenKind::LParen, "'(' to open a destructuring pattern")?
            .span;
        let mut positions = Vec::new();
        loop {
            if self.check(&TokenKind::LParen) {
                positions.push(self.parse_binding_pattern()?);
            } else {
                let (name, _) = self.expect_ident()?;
                positions.push(if name == "_" {
                    BindingPattern::Ignore
                } else {
                    BindingPattern::Name(name)
                });
            }
            if self.check(&TokenKind::Comma) {
                self.advance();
                continue;
            }
            break;
        }
        let close = self
            .expect(TokenKind::RParen, "')' to close a destructuring pattern")?
            .span;
        if positions.len() < 2 {
            return Err(ParseError {
                help: None,
                message: "a destructuring pattern needs two or more positions ('val (lo, hi) = ...'); a single name is an ordinary 'val', and '_' ignores a position rather than binding one"
                    .to_string(),
                span: open.to(close),
            });
        }
        Ok(BindingPattern::Tuple(positions))
    }

    /// `loop { ... }`: `while true { ... }` (the loop control statements and the
    /// missing-return analysis already treat that as an endless loop).
    pub(crate) fn parse_loop_stmt(&mut self) -> PResult<Stmt> {
        let start = self.advance().span;
        let body = self.parse_block()?;
        let span = start.to(body.span);
        Ok(Stmt::While {
            cond: Expr::BoolLit(true, start),
            body,
            iterating: None,
            span,
        })
    }

    /// One loop (`while`, `for` or `loop`), optionally labelled, with the
    /// bookkeeping a labelled exit needs. A `break 'outer` from inside nested
    /// loops sets a hidden flag and breaks the innermost loop; each loop
    /// between it and the target then re-checks the flag right after it ends
    /// and breaks in turn, and the target either breaks or continues. Every
    /// step is an ordinary `break`, `continue`, `if` and `var`, so borrow
    /// checking, drops and both backends see nothing new.
    pub(crate) fn parse_loop_construct(&mut self, label: Option<String>) -> PResult<Stmt> {
        self.parse_loop_construct_with(label, None).map(|(stmt, _)| stmt)
    }

    /// `parse_loop_construct`, optionally for a `loop` used as a value, whose
    /// `break value`s push to `value_slot`. Also returns how many valued
    /// breaks the loop had.
    pub(crate) fn parse_loop_construct_with(
        &mut self,
        label: Option<String>,
        value_slot: Option<String>,
    ) -> PResult<(Stmt, usize)> {
        self.loop_stack.push(LoopCtx { label, flag: None, exits: Vec::new(), value_slot, valued_breaks: 0 });
        let parsed = match self.peek_kind() {
            TokenKind::While => self.parse_while_stmt(),
            TokenKind::For => self.parse_for_stmt(),
            _ => self.parse_loop_stmt(),
        };
        let ctx = self.loop_stack.pop().expect("the loop pushed above");
        let stmt = parsed?;
        let valued = ctx.valued_breaks;
        if ctx.flag.is_none() && ctx.exits.is_empty() {
            return Ok((stmt, valued));
        }
        let span = match &stmt {
            Stmt::While { span, .. } | Stmt::For { span, .. } => *span,
            _ => unreachable!("a loop construct is a while or a for"),
        };
        let mut stmts: Vec<Stmt> = Vec::new();
        if let Some(flag) = &ctx.flag {
            stmts.push(Stmt::Let {
                name: flag.clone(),
                mutable: true,
                ty: None,
                init: Expr::IntLit(0, span),
                span,
            });
        }
        stmts.push(stmt);
        let parent = self.loop_stack.len();
        let ident = |name: &str| Expr::Ident(name.to_string(), span);
        let test = |name: &str, op: BinOp, n: u64| Expr::Binary {
            op,
            lhs: Box::new(Expr::Ident(name.to_string(), span)),
            rhs: Box::new(Expr::IntLit(n, span)),
            span,
        };
        let when = |cond: Expr, body: Vec<Stmt>| Stmt::Expr {
            expr: Expr::If {
                cond: Box::new(cond),
                then_block: Block { stmts: body, span },
                else_block: None,
                span,
            },
            span,
        };
        for target in ctx.exits {
            let flag = self.loop_stack[target].flag.clone().expect("a target with an exit has a flag");
            if target + 1 == parent {
                // The target is the loop this one sits directly in.
                stmts.push(when(test(&flag, BinOp::Eq, 1), vec![Stmt::Break { span }]));
                stmts.push(when(
                    test(&flag, BinOp::Eq, 2),
                    vec![
                        Stmt::Expr {
                            expr: Expr::Assign {
                                target: Box::new(ident(&flag)),
                                value: Box::new(Expr::IntLit(0, span)),
                                span,
                            },
                            span,
                        },
                        Stmt::Continue { span },
                    ],
                ));
            } else {
                stmts.push(when(test(&flag, BinOp::NotEq, 0), vec![Stmt::Break { span }]));
            }
        }
        Ok((Stmt::Expr { expr: Expr::Block(Block { stmts, span }), span }, valued))
    }

    /// `loop { ... break value ... }` used as a value: `val x = loop { ... }`.
    /// The value travels in a hidden one-element list the loop's `break value`s
    /// push to (a bare `List.empty()` takes its element type from the first
    /// push), and the loop's value is what is popped out of it afterwards, so
    /// nothing below the parser knows about it.
    pub(crate) fn parse_loop_value(&mut self, label: Option<String>) -> PResult<Expr> {
        let start = self.peek().span;
        self.label_flags += 1;
        let slot = format!("#loopres{}", self.label_flags - 1);
        let (stmt, valued) = self.parse_loop_construct_with(label, Some(slot.clone()))?;
        let span = start.to(match &stmt {
            Stmt::Expr { span, .. } | Stmt::While { span, .. } | Stmt::For { span, .. } => *span,
            _ => start,
        });
        if valued == 0 {
            return Err(ParseError {
                help: None,
                message: "a 'loop' used as a value needs a 'break value' to produce one".to_string(),
                span,
            });
        }
        let ident = |name: &str| Expr::Ident(name.to_string(), span);
        let call = |receiver: &str, method: &str, args: Vec<CallArg>| Expr::MethodCall {
            receiver: Box::new(ident(receiver)),
            method: method.to_string(),
            type_args: Vec::new(),
            args,
            span,
        };
        Ok(Expr::Block(Block {
            stmts: vec![
                Stmt::Let {
                    name: slot.clone(),
                    mutable: true,
                    ty: None,
                    init: call("List", "empty", Vec::new()),
                    span,
                },
                stmt,
                Stmt::Expr { expr: call(&slot, "pop", Vec::new()), span },
            ],
            span,
        }))
    }

    /// `break 'outer` / `continue 'outer`. The innermost loop is a plain
    /// `break`/`continue`; an outer one sets the target's flag (1 for break,
    /// 2 for continue) and breaks, and `parse_loop_construct` adds the
    /// re-checks after each loop in between.
    pub(crate) fn parse_labelled_exit(&mut self, is_break: bool, label: &str, span: Span, value: Option<Expr>) -> PResult<Stmt> {
        let Some(target) = self.loop_stack.iter().rposition(|l| l.label.as_deref() == Some(label)) else {
            return Err(ParseError {
                help: None,
                message: format!("no enclosing loop is labelled '{label}; a label is written before a loop ('{label}: for ...') and does not cross a function or a block passed to a call"),
                span,
            });
        };
        let top = self.loop_stack.len() - 1;
        let push = self.valued_exit(target, is_break, span, value)?;
        if target == top {
            return Ok(Self::with_push(push, if is_break { Stmt::Break { span } } else { Stmt::Continue { span } }, span));
        }
        let flag = match self.loop_stack[target].flag.clone() {
            Some(f) => f,
            None => {
                self.label_flags += 1;
                let f = format!("{}{}", LABEL_FLAG_PREFIX, self.label_flags - 1);
                self.loop_stack[target].flag = Some(f.clone());
                f
            }
        };
        for between in (target + 1)..=top {
            if !self.loop_stack[between].exits.contains(&target) {
                self.loop_stack[between].exits.push(target);
            }
        }
        let mut stmts = Vec::new();
        stmts.extend(push);
        stmts.push(Stmt::Expr {
            expr: Expr::Assign {
                target: Box::new(Expr::Ident(flag, span)),
                value: Box::new(Expr::IntLit(if is_break { 1 } else { 2 }, span)),
                span,
            },
            span,
        });
        stmts.push(Stmt::Break { span });
        Ok(Stmt::Expr { expr: Expr::Block(Block { stmts, span }), span })
    }

    /// An unlabelled `break`/`continue` (with `break value`'s value, if any).
    pub(crate) fn parse_exit(&mut self, is_break: bool, span: Span, value: Option<Expr>) -> PResult<Stmt> {
        let Some(target) = self.loop_stack.len().checked_sub(1) else {
            // No loop encloses it: the type checker says so, with its own wording.
            return Ok(if is_break { Stmt::Break { span } } else { Stmt::Continue { span } });
        };
        let push = self.valued_exit(target, is_break, span, value)?;
        Ok(Self::with_push(push, if is_break { Stmt::Break { span } } else { Stmt::Continue { span } }, span))
    }

    /// The `slot.push(value)` of a `break value` out of loop `target`, after
    /// checking that the loop is one that produces a value and the exit is
    /// one that gives it: `Ok(None)` for an exit that carries no value.
    pub(crate) fn valued_exit(&mut self, target: usize, is_break: bool, span: Span, value: Option<Expr>) -> PResult<Option<Stmt>> {
        let slot = self.loop_stack[target].value_slot.clone();
        match (slot, value) {
            (Some(slot), Some(v)) => {
                self.loop_stack[target].valued_breaks += 1;
                Ok(Some(Stmt::Expr {
                    expr: Expr::MethodCall {
                        receiver: Box::new(Expr::Ident(slot, span)),
                        method: "push".to_string(),
                        type_args: Vec::new(),
                        args: vec![CallArg { name: None, value: v }],
                        span,
                    },
                    span,
                }))
            }
            (None, Some(_)) => Err(ParseError {
                help: None,
                message: "only a 'loop' used as a value can 'break' with one: write 'val x = loop { ... break value ... }'".to_string(),
                span,
            }),
            (Some(_), None) if is_break => Err(ParseError {
                help: None,
                message: "this 'loop' produces a value, so every 'break' out of it carries one: 'break value'".to_string(),
                span,
            }),
            _ => Ok(None),
        }
    }

    /// Whether the token `offset` past the current one starts a `break` value:
    /// see `starts_break_value`, plus a negative number (`break -1`).
    pub(crate) fn break_value_at(&self, offset: usize) -> bool {
        let kind = |i: usize| self.tokens.get(self.pos + i).map(|t| &t.kind);
        match kind(offset) {
            Some(TokenKind::Minus) => {
                matches!(kind(offset + 1), Some(TokenKind::IntLit(_)) | Some(TokenKind::FloatLit(_)))
            }
            Some(k) => starts_break_value(k),
            None => false,
        }
    }
}
