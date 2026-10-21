use super::*;

/// One arm of a `select`, as written.
enum SelectArm {
    /// `recv(rx) as v -> e`: the case receives from `rx`; `bind` names the value in `e`.
    Recv { chan: Expr, bind: Option<String>, body: Expr, span: Span },
    /// `send(tx, x) -> e`: the case sends `value` on `tx`.
    Send { chan: Expr, value: Expr, body: Expr, span: Span },
    /// `timeout(ms) -> e`: the select gives up after `ms` milliseconds.
    Timeout { millis: Expr, body: Expr, span: Span },
    /// `closed -> e`: a receive case found its channel closed and empty.
    Closed { body: Expr, span: Span },
}

impl SelectArm {
    fn span(&self) -> Span {
        match self {
            SelectArm::Recv { span, .. }
            | SelectArm::Send { span, .. }
            | SelectArm::Timeout { span, .. }
            | SelectArm::Closed { span, .. } => *span,
        }
    }

    fn body(&self) -> &Expr {
        match self {
            SelectArm::Recv { body, .. }
            | SelectArm::Send { body, .. }
            | SelectArm::Timeout { body, .. }
            | SelectArm::Closed { body, .. } => body,
        }
    }
}

/// A channel named by a select case is read again at every poll, so it must be
/// a place (`rx`, `self.rx`), not an expression that would run again each time.
fn is_select_place(e: &Expr) -> bool {
    match e {
        Expr::Ident(..) => true,
        Expr::FieldAccess { base, .. } => is_select_place(base),
        _ => false,
    }
}

fn ident(name: &str, span: Span) -> Expr {
    Expr::Ident(name.to_string(), span)
}

fn borrow(expr: Expr, span: Span) -> Expr {
    Expr::Ref {
        mutable: false,
        expr: Box::new(expr),
        span,
    }
}

fn arg(value: Expr) -> CallArg {
    CallArg { name: None, value }
}

/// `sync.<name>(args)`, spelled the way a program spells a call into an imported
/// module (a method call on the module alias), so inference resolves it the same way.
fn call_std(name: &str, args: Vec<Expr>, span: Span) -> Expr {
    method(ident("sync", span), name, args, span)
}

fn method(receiver: Expr, name: &str, args: Vec<Expr>, span: Span) -> Expr {
    Expr::MethodCall {
        receiver: Box::new(receiver),
        method: name.to_string(),
        type_args: Vec::new(),
        args: args.into_iter().map(arg).collect(),
        span,
    }
}

fn stmt_expr(expr: Expr, span: Span) -> Stmt {
    Stmt::Expr { expr, span }
}

fn let_stmt(name: String, mutable: bool, init: Expr, span: Span) -> Stmt {
    Stmt::Let {
        name,
        mutable,
        ty: None,
        init,
        span,
    }
}

fn assign(name: &str, value: Expr, span: Span) -> Stmt {
    stmt_expr(
        Expr::Assign {
            target: Box::new(ident(name, span)),
            value: Box::new(value),
            span,
        },
        span,
    )
}

fn block_of(stmts: Vec<Stmt>, span: Span) -> Block {
    Block { stmts, span }
}

impl Parser {
    /// `select { ... }`: a choice between channel cases, as a value (the chosen
    /// arm's value). Contextual: `select` starts the form only when a `{` follows,
    /// so it stays an ordinary name everywhere else.
    ///
    /// Arms (`recv(rx) as v -> e`, `send(tx, x) -> e`, `timeout(ms) -> e`,
    /// `closed -> e`) are separated by newlines or commas. The form is lowered here
    /// onto `sync.select_begin`, `sync.recv_case`, `sync.send_case` and the
    /// `poll`, `park` and `wait` calls of `std/sync`, so borrowck and inference
    /// check it as ordinary code. The first arm that is ready in source order wins
    /// a tie; a select that cannot proceed blocks in the runtime, not in a loop.
    pub(crate) fn parse_select(&mut self) -> PResult<Expr> {
        let start = self.advance().span;
        self.expect(TokenKind::LBrace, "'{' after 'select'")?;
        self.skip_newlines();
        let mut arms = Vec::new();
        while !self.check(&TokenKind::RBrace) {
            arms.push(self.parse_select_arm()?);
            if self.check(&TokenKind::Comma) {
                self.advance();
            }
            self.skip_newlines();
        }
        let end = self.expect(TokenKind::RBrace, "'}' to close the select")?.span;
        let span = start.to(end);
        self.lower_select(arms, span)
    }

    fn parse_select_arm(&mut self) -> PResult<SelectArm> {
        let (head, head_span) = self.expect_ident()?;
        let arm = match head.as_str() {
            "recv" => {
                self.expect(TokenKind::LParen, "'(' after 'recv'")?;
                let chan = self.parse_expr()?;
                let close = self.expect(TokenKind::RParen, "')' to close the receive case")?.span;
                let bind = if self.check(&TokenKind::As) {
                    self.advance();
                    Some(self.expect_ident()?.0)
                } else {
                    None
                };
                self.expect(TokenKind::Arrow, "'->' after the select case")?;
                let body = self.parse_expr()?;
                let span = head_span.to(close).to(body.span());
                SelectArm::Recv { chan, bind, body, span }
            }
            "send" => {
                self.expect(TokenKind::LParen, "'(' after 'send'")?;
                let chan = self.parse_expr()?;
                self.expect(TokenKind::Comma, "',' between the channel and the value")?;
                let value = self.parse_expr()?;
                let close = self.expect(TokenKind::RParen, "')' to close the send case")?.span;
                self.expect(TokenKind::Arrow, "'->' after the select case")?;
                let body = self.parse_expr()?;
                let span = head_span.to(close).to(body.span());
                SelectArm::Send { chan, value, body, span }
            }
            "timeout" => {
                self.expect(TokenKind::LParen, "'(' after 'timeout'")?;
                let millis = self.parse_expr()?;
                let close = self.expect(TokenKind::RParen, "')' to close the timeout")?.span;
                self.expect(TokenKind::Arrow, "'->' after the timeout")?;
                let body = self.parse_expr()?;
                let span = head_span.to(close).to(body.span());
                SelectArm::Timeout { millis, body, span }
            }
            "closed" => {
                self.expect(TokenKind::Arrow, "'->' after 'closed'")?;
                let body = self.parse_expr()?;
                let span = head_span.to(body.span());
                SelectArm::Closed { body, span }
            }
            other => {
                return Err(ParseError {
                    help: Some("a select arm is recv(rx) as v -> e, send(tx, x) -> e, timeout(ms) -> e or closed -> e".to_string()),
                    message: format!("'{other}' is not a select arm"),
                    span: head_span,
                });
            }
        };
        Ok(arm)
    }

    /// Checks the arms and builds the lowering. Every error is raised here, before
    /// any statement exists, so a bad select never reaches inference.
    fn lower_select(&mut self, arms: Vec<SelectArm>, span: Span) -> PResult<Expr> {
        let err = |message: String, at: Span| ParseError { help: None, message, span: at };
        if arms.is_empty() {
            return Err(err("'select' needs at least one arm".to_string(), span));
        }
        let mut recvs = 0usize;
        let mut closeds = 0usize;
        let mut timeouts = 0usize;
        let mut sent: Vec<(String, Span)> = Vec::new();
        for arm in &arms {
            match arm {
                SelectArm::Recv { chan, .. } => {
                    recvs += 1;
                    if !is_select_place(chan) {
                        return Err(err(
                            "a select case names its channel as a variable or a field (`rx`, `self.rx`), because the case reads it again at every poll".to_string(),
                            chan.span(),
                        ));
                    }
                }
                SelectArm::Send { chan, value, .. } => {
                    if !is_select_place(chan) {
                        return Err(err(
                            "a select case names its channel as a variable or a field (`tx`, `self.tx`), because the case reads it again at every poll".to_string(),
                            chan.span(),
                        ));
                    }
                    if let Expr::Ident(name, at) = value {
                        if sent.iter().any(|(n, _)| n == name) {
                            return Err(err(
                                format!("the value '{name}' is sent in two cases of this select: a value can be sent in one case only, so send a copy or a second value in the other case"),
                                *at,
                            ));
                        }
                        sent.push((name.clone(), *at));
                    }
                }
                SelectArm::Timeout { .. } => timeouts += 1,
                SelectArm::Closed { .. } => closeds += 1,
            }
        }
        if timeouts > 1 {
            return Err(err("a 'select' takes one timeout arm".to_string(), span));
        }
        if closeds > 1 {
            return Err(err("a 'select' takes one closed arm".to_string(), span));
        }
        if recvs > 0 && closeds == 0 {
            return Err(err(
                "a 'select' with a receive case needs a 'closed -> e' arm: it runs when the chosen receive case is closed and empty".to_string(),
                span,
            ));
        }
        if recvs == 0 && closeds > 0 {
            return Err(err("a 'closed' arm needs a receive case to be closed".to_string(), span));
        }

        // Statement names are unique per select: the select's own start offset.
        let base = format!("#sel{}", span.start);
        let sel = base.clone();
        let pick = format!("{base}_pick");
        let closed_flag = format!("{base}_closed");
        let expired = format!("{base}_expired");
        let (timed, millis) = match arms.iter().find(|a| matches!(a, SelectArm::Timeout { .. })) {
            Some(SelectArm::Timeout { millis, .. }) => (true, millis.clone()),
            _ => (false, Expr::IntLit(0, span)),
        };

        let mut stmts = Vec::new();
        stmts.push(let_stmt(
            sel.clone(),
            false,
            call_std("select_begin", vec![Expr::BoolLit(timed, span), millis], span),
            span,
        ));
        // One slot per case, named by arm position.
        let mut slot_of: Vec<Option<String>> = Vec::new();
        for (i, arm) in arms.iter().enumerate() {
            let slot = format!("{base}_s{i}");
            match arm {
                SelectArm::Recv { chan, .. } => {
                    let init = call_std("recv_case", vec![borrow(chan.clone(), span), borrow(ident(&sel, span), span)], span);
                    stmts.push(let_stmt(slot.clone(), false, init, arm.span()));
                    slot_of.push(Some(slot));
                }
                SelectArm::Send { chan, value, .. } => {
                    let init = call_std(
                        "send_case",
                        vec![borrow(chan.clone(), span), borrow(ident(&sel, span), span), value.clone()],
                        arm.span(),
                    );
                    stmts.push(let_stmt(slot.clone(), false, init, arm.span()));
                    slot_of.push(Some(slot));
                }
                SelectArm::Timeout { .. } | SelectArm::Closed { .. } => slot_of.push(None),
            }
        }

        stmts.push(let_stmt(pick.clone(), true, Expr::IntLit(0, span), span));
        if recvs > 0 {
            stmts.push(let_stmt(closed_flag.clone(), true, Expr::BoolLit(false, span), span));
        }
        if timeouts > 0 {
            stmts.push(let_stmt(expired.clone(), true, Expr::BoolLit(false, span), span));
        }

        // The wait loop: poll every case in source order, give up at the deadline
        // once, then park every receive case, block, and unpark.
        let mut loop_body = Vec::new();
        for (i, arm) in arms.iter().enumerate() {
            let Some(slot) = slot_of[i].clone() else {
                continue;
            };
            let ready_stmts = match arm {
                SelectArm::Recv { chan, .. } => {
                    let mut ready = vec![assign(&pick, Expr::IntLit(i as u64, span), span)];
                    if closeds > 0 {
                        ready.push(assign(
                            &closed_flag,
                            method(ident(&slot, span), "is_closed", Vec::new(), span),
                            span,
                        ));
                    }
                    ready.push(Stmt::Break { span });
                    (method(chan.clone(), "select_poll", vec![borrow(ident(&slot, span), span)], span), ready)
                }
                SelectArm::Send { chan, .. } => {
                    let ready = vec![assign(&pick, Expr::IntLit(i as u64, span), span), Stmt::Break { span }];
                    (method(chan.clone(), "select_send", vec![borrow(ident(&slot, span), span)], span), ready)
                }
                _ => continue,
            };
            loop_body.push(stmt_expr(
                Expr::If {
                    cond: Box::new(ready_stmts.0),
                    then_block: block_of(ready_stmts.1, span),
                    else_block: None,
                    span,
                },
                span,
            ));
        }
        if let Some(i) = arms.iter().position(|a| matches!(a, SelectArm::Timeout { .. })) {
            loop_body.push(stmt_expr(
                Expr::If {
                    cond: Box::new(ident(&expired, span)),
                    then_block: block_of(
                        vec![assign(&pick, Expr::IntLit(i as u64, span), span), Stmt::Break { span }],
                        span,
                    ),
                    else_block: None,
                    span,
                },
                span,
            ));
        }
        let parked: Vec<String> = slot_of.iter().flatten().cloned().collect();
        for slot in &parked {
            loop_body.push(stmt_expr(method(ident(slot, span), "park", vec![borrow(ident(&sel, span), span)], span), span));
        }
        let wait = method(ident(&sel, span), "wait", Vec::new(), span);
        if timeouts > 0 {
            loop_body.push(assign(&expired, Expr::Unary { op: UnOp::Not, expr: Box::new(wait), span }, span));
        } else {
            loop_body.push(stmt_expr(wait, span));
        }
        for slot in &parked {
            loop_body.push(stmt_expr(method(ident(slot, span), "unpark", Vec::new(), span), span));
        }
        stmts.push(Stmt::While {
            cond: Expr::BoolLit(true, span),
            body: block_of(loop_body, span),
            iterating: None,
            span,
        });

        // The result: each value arm runs when its index was picked, and the
        // closed arm runs when the chosen receive case was closed. With a closed
        // arm, the closed arm is the final else and every value arm is tested
        // (a receive arm also needs the case not to be closed); without one, the
        // last value arm is the final else.
        let closed_body = arms.iter().find_map(|a| match a {
            SelectArm::Closed { body, span } => Some((body.clone(), *span)),
            _ => None,
        });
        let value_arms: Vec<(usize, &SelectArm)> = arms
            .iter()
            .enumerate()
            .filter(|(_, a)| !matches!(a, SelectArm::Closed { .. }))
            .collect();
        let last = value_arms.len().saturating_sub(1);
        let mut items: Vec<(Option<Expr>, Block)> = Vec::new();
        for (k, (i, arm)) in value_arms.iter().enumerate() {
            let mut body_stmts = Vec::new();
            if let SelectArm::Recv { bind: Some(name), .. } = arm {
                let slot = slot_of[*i].clone().unwrap_or_default();
                body_stmts.push(let_stmt(name.clone(), false, method(ident(&slot, span), "take", Vec::new(), span), arm.span()));
            }
            body_stmts.push(stmt_expr(arm.body().clone(), arm.span()));
            let is_final = closed_body.is_none() && k == last;
            let cond = if is_final {
                None
            } else {
                let picked = Expr::Binary {
                    op: BinOp::Eq,
                    lhs: Box::new(ident(&pick, span)),
                    rhs: Box::new(Expr::IntLit(*i as u64, span)),
                    span,
                };
                let guarded = match (arm, closed_body.is_some()) {
                    (SelectArm::Recv { .. }, true) => Expr::Binary {
                        op: BinOp::And,
                        lhs: Box::new(picked),
                        rhs: Box::new(Expr::Unary {
                            op: UnOp::Not,
                            expr: Box::new(ident(&closed_flag, span)),
                            span,
                        }),
                        span,
                    },
                    _ => picked,
                };
                Some(guarded)
            };
            items.push((cond, block_of(body_stmts, arm.span())));
        }
        if let Some((closed_body, closed_span)) = closed_body {
            items.push((None, block_of(vec![stmt_expr(closed_body, closed_span)], closed_span)));
        }

        let mut tail: Option<ElseBranch> = None;
        for (cond, block) in items.into_iter().rev() {
            match cond {
                None => tail = Some(ElseBranch::Block(block)),
                Some(cond) => {
                    let node = Expr::If {
                        cond: Box::new(cond),
                        then_block: block,
                        else_block: tail.take().map(Box::new),
                        span,
                    };
                    tail = Some(ElseBranch::If(node));
                }
            }
        }
        let result = match tail {
            Some(ElseBranch::If(node)) => node,
            Some(ElseBranch::Block(block)) => Expr::Block(block),
            None => Expr::Block(block_of(Vec::new(), span)),
        };
        stmts.push(stmt_expr(result, span));
        Ok(Expr::Block(block_of(stmts, span)))
    }
}
