use crate::ast::*;
use crate::lexer::{Span, Token, TokenKind};

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

#[derive(Debug)]
pub struct ParseError {
    pub message: String,
    pub span: Span,
}

type PResult<T> = Result<T, ParseError>;

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Parser { tokens, pos: 0 }
    }

    fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn peek_kind(&self) -> &TokenKind {
        &self.tokens[self.pos].kind
    }

    /// The kind of the token one past the current one (`Eof` past the end).
    fn peek2_kind(&self) -> TokenKind {
        self.tokens
            .get(self.pos + 1)
            .map(|t| t.kind.clone())
            .unwrap_or(TokenKind::Eof)
    }

    fn advance(&mut self) -> Token {
        let tok = self.tokens[self.pos].clone();
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        tok
    }

    fn check(&self, kind: &TokenKind) -> bool {
        std::mem::discriminant(self.peek_kind()) == std::mem::discriminant(kind)
    }

    fn expect(&mut self, kind: TokenKind, what: &str) -> PResult<Token> {
        if self.check(&kind) {
            Ok(self.advance())
        } else {
            Err(ParseError {
                message: format!("expected {what}, found {:?}", self.peek_kind()),
                span: self.peek().span,
            })
        }
    }

    fn expect_ident(&mut self) -> PResult<(String, Span)> {
        match self.peek_kind().clone() {
            TokenKind::Ident(name) => {
                let span = self.peek().span;
                self.advance();
                Ok((name, span))
            }
            other => Err(ParseError {
                message: format!("expected identifier, found {other:?}"),
                span: self.peek().span,
            }),
        }
    }

    /// Consumes zero or more `Newline` tokens (blank lines already collapse
    /// to a single token in the lexer).
    fn skip_newlines(&mut self) {
        while self.check(&TokenKind::Newline) {
            self.advance();
        }
    }

    /// True at a point where a statement (or `when`-arm) may legally end:
    /// a newline, the block's closing `}`, or end of input.
    fn at_stmt_end(&self) -> bool {
        matches!(
            self.peek_kind(),
            TokenKind::Newline | TokenKind::RBrace | TokenKind::Eof
        )
    }

    pub fn parse_program(&mut self) -> PResult<Program> {
        self.skip_newlines();
        let mut impls = Vec::new();
        let mut datacs = Vec::new();
        let mut functions = Vec::new();
        while !matches!(self.peek_kind(), TokenKind::Eof) {
            if self.check(&TokenKind::Impl) {
                if !datacs.is_empty() || !functions.is_empty() {
                    return Err(ParseError {
                        message: "'impl' lines must come before any 'datac' or 'fun' (canonical order: impl, pd, datac/fun)".to_string(),
                        span: self.peek().span,
                    });
                }
                impls.push(self.parse_impl_decl()?);
            } else if self.check(&TokenKind::Datac) {
                datacs.push(self.parse_datac_decl()?);
            } else {
                functions.push(self.parse_function()?);
            }
            self.skip_newlines();
        }
        Ok(Program {
            impls,
            datacs,
            functions,
        })
    }

    /// `impl "shapes/circle"` / `impl "time" as t`. `with caps(...)` is
    /// milestone M7 (capability-based security); a file that writes it gets
    /// a clear "not yet" error rather than a confusing parse error.
    fn parse_impl_decl(&mut self) -> PResult<ImplDecl> {
        let start = self.expect(TokenKind::Impl, "'impl'")?.span;
        let (path, path_span) = match self.peek_kind().clone() {
            TokenKind::StringLit(s) => {
                let span = self.peek().span;
                self.advance();
                (s, span)
            }
            other => {
                return Err(ParseError {
                    message: format!("expected a quoted module path after 'impl', found {other:?}"),
                    span: self.peek().span,
                })
            }
        };
        let (alias, end) = if self.check(&TokenKind::As) {
            self.advance();
            let (name, span) = self.expect_ident()?;
            (Some(name), span)
        } else {
            (None, path_span)
        };
        if matches!(self.peek_kind(), TokenKind::Ident(n) if n == "with") {
            return Err(ParseError {
                message: "'with caps(...)' grants are not implemented yet (milestone M7)".to_string(),
                span: self.peek().span,
            });
        }
        Ok(ImplDecl {
            path,
            alias,
            span: start.to(end),
        })
    }

    fn parse_datac_decl(&mut self) -> PResult<DatacDecl> {
        let start = self.expect(TokenKind::Datac, "'datac'")?.span;
        let (name, _) = self.expect_ident()?;
        self.expect(TokenKind::LParen, "'('")?;
        let mut fields = Vec::new();
        while !self.check(&TokenKind::RParen) {
            let mutable = if self.check(&TokenKind::Var) {
                self.advance();
                true
            } else {
                self.expect(TokenKind::Val, "'val' or 'var'")?;
                false
            };
            let (fname, fspan) = self.expect_ident()?;
            self.expect(TokenKind::Colon, "':'")?;
            let ty = self.parse_type()?;
            fields.push(FieldDecl {
                name: fname,
                mutable,
                ty,
                span: fspan,
            });
            if self.check(&TokenKind::Comma) {
                self.advance();
            } else {
                break;
            }
        }
        let end = self.expect(TokenKind::RParen, "')'")?.span;
        Ok(DatacDecl {
            name,
            fields,
            module: String::new(),
            span: start.to(end),
        })
    }

    fn parse_function(&mut self) -> PResult<FunctionDecl> {
        let start = self.expect(TokenKind::Fun, "'fun'")?.span;

        // A receiver-form decl is `fun [mut] Type.name(...)`; a plain one
        // is `fun name(...)`. `mut` here can only ever start a receiver
        // (free functions never write `fun mut name(...)`), so seeing it
        // commits to that form.
        let receiver_mutable = if self.check(&TokenKind::Mut) {
            self.advance();
            true
        } else {
            false
        };
        let (first, first_span) = self.expect_ident()?;
        let (receiver, name) = if self.check(&TokenKind::Dot) {
            self.advance();
            let (method, _) = self.expect_ident()?;
            (
                Some(Receiver {
                    type_name: first,
                    mutable: receiver_mutable,
                    span: first_span,
                }),
                method,
            )
        } else {
            if receiver_mutable {
                return Err(ParseError {
                    message: "'mut' before a function name is only valid on a receiver ('fun mut Type.method(...)')".to_string(),
                    span: first_span,
                });
            }
            (None, first)
        };

        self.expect(TokenKind::LParen, "'('")?;
        let mut params = Vec::new();
        while !self.check(&TokenKind::RParen) {
            let (pname, pspan) = self.expect_ident()?;
            self.expect(TokenKind::Colon, "':'")?;
            // `mut T` requests an exclusive borrow; this is the *only*
            // spelling in parameter position — `&mut T` is rejected here
            // (the leading `&` in `parse_type` expects an identifier next,
            // not the `mut` keyword token), `&mut` stays expression-only.
            let mutable = if self.check(&TokenKind::Mut) {
                self.advance();
                true
            } else {
                false
            };
            let ty = self.parse_type()?;
            params.push(Param {
                name: pname,
                ty,
                mutable,
                span: pspan,
            });
            if self.check(&TokenKind::Comma) {
                self.advance();
            } else {
                break;
            }
        }
        self.expect(TokenKind::RParen, "')'")?;

        let return_type = if self.check(&TokenKind::Dot) {
            self.advance();
            let (marker, mspan) = self.expect_ident()?;
            if marker != "R" {
                return Err(ParseError {
                    message: format!("expected 'R' after '.', found '{marker}'"),
                    span: mspan,
                });
            }
            self.expect(TokenKind::Colon, "':'")?;
            Some(self.parse_type()?)
        } else {
            None
        };

        let body = if self.check(&TokenKind::Eq) {
            let eq_span = self.advance().span;
            if return_type.is_none() {
                return Err(ParseError {
                    message: "expression-bodied function requires a '.R: T' return type"
                        .to_string(),
                    span: eq_span,
                });
            }
            FunctionBody::Expr(self.parse_expr()?)
        } else if self.check(&TokenKind::LBrace) {
            FunctionBody::Block(self.parse_block()?)
        } else {
            return Err(ParseError {
                message: format!("expected '=' or '{{', found {:?}", self.peek_kind()),
                span: self.peek().span,
            });
        };

        let end = match &body {
            FunctionBody::Block(b) => b.span,
            FunctionBody::Expr(e) => e.span(),
        };
        Ok(FunctionDecl {
            name,
            receiver,
            params,
            return_type,
            body,
            module: String::new(),
            span: start.to(end),
        })
    }

    fn parse_type(&mut self) -> PResult<TypeExpr> {
        // A leading `&` is accepted and discarded: v1 has no real
        // reference/borrow type yet (that's borrow-checker territory,
        // deferred), but the only string type is spelled `&str` in
        // examples, so the annotation needs to at least parse.
        let amp_start = if self.check(&TokenKind::Amp) {
            Some(self.advance().span)
        } else {
            None
        };
        let (first, first_span) = self.expect_ident()?;
        // A dotted name is a module-qualified reference to an imported item
        // (`circle.Circle`) — see `resolve.rs`, which rewrites it to the
        // item's canonical, module-prefixed name.
        let mut name = first;
        let mut span = first_span;
        while self.check(&TokenKind::Dot) {
            self.advance();
            let (part, part_span) = self.expect_ident()?;
            name.push('.');
            name.push_str(&part);
            span = span.to(part_span);
        }
        Ok(TypeExpr {
            name,
            span: amp_start.map(|a| a.to(span)).unwrap_or(span),
        })
    }

    fn parse_block(&mut self) -> PResult<Block> {
        let start = self.expect(TokenKind::LBrace, "'{'")?.span;
        self.skip_newlines();
        let mut stmts = Vec::new();
        while !self.check(&TokenKind::RBrace) {
            let stmt = self.parse_stmt()?;
            stmts.push(stmt);
            if !self.check(&TokenKind::RBrace) {
                if !self.check(&TokenKind::Newline) {
                    return Err(ParseError {
                        message: format!(
                            "expected a newline or '}}' after a statement, found {:?}",
                            self.peek_kind()
                        ),
                        span: self.peek().span,
                    });
                }
                self.skip_newlines();
            }
        }
        let end = self.expect(TokenKind::RBrace, "'}'")?.span;
        Ok(Block {
            stmts,
            span: start.to(end),
        })
    }

    fn parse_stmt(&mut self) -> PResult<Stmt> {
        match self.peek_kind() {
            TokenKind::Val | TokenKind::Var => self.parse_let_stmt(),
            TokenKind::While => self.parse_while_stmt(),
            TokenKind::Return => self.parse_return_stmt(),
            _ => {
                let expr = self.parse_expr()?;
                let span = expr.span();
                Ok(Stmt::Expr { expr, span })
            }
        }
    }

    fn parse_let_stmt(&mut self) -> PResult<Stmt> {
        let mutable = self.check(&TokenKind::Var);
        let start = if mutable {
            self.expect(TokenKind::Var, "'var'")?.span
        } else {
            self.expect(TokenKind::Val, "'val'")?.span
        };
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

    fn parse_while_stmt(&mut self) -> PResult<Stmt> {
        let start = self.expect(TokenKind::While, "'while'")?.span;
        let cond = self.parse_expr()?;
        let body = self.parse_block()?;
        let span = start.to(body.span);
        Ok(Stmt::While { cond, body, span })
    }

    fn parse_return_stmt(&mut self) -> PResult<Stmt> {
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

    fn parse_expr(&mut self) -> PResult<Expr> {
        self.parse_assignment()
    }

    fn parse_assignment(&mut self) -> PResult<Expr> {
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

    fn binding_power(op: &TokenKind) -> Option<(u8, BinOp)> {
        use TokenKind::*;
        Some(match op {
            OrOr => (1, BinOp::Or),
            AndAnd => (2, BinOp::And),
            EqEq => (3, BinOp::Eq),
            NotEq => (3, BinOp::NotEq),
            Lt => (4, BinOp::Lt),
            LtEq => (4, BinOp::LtEq),
            Gt => (4, BinOp::Gt),
            GtEq => (4, BinOp::GtEq),
            Plus => (5, BinOp::Add),
            Minus => (5, BinOp::Sub),
            Star => (6, BinOp::Mul),
            Slash => (6, BinOp::Div),
            Percent => (6, BinOp::Mod),
            _ => return None,
        })
    }

    fn parse_binary(&mut self, min_bp: u8) -> PResult<Expr> {
        let mut lhs = self.parse_unary()?;
        loop {
            let Some((bp, op)) = Self::binding_power(self.peek_kind()) else {
                break;
            };
            if bp < min_bp {
                break;
            }
            self.advance();
            let rhs = self.parse_binary(bp + 1)?;
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

    fn parse_unary(&mut self) -> PResult<Expr> {
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

    fn parse_postfix(&mut self) -> PResult<Expr> {
        let mut expr = self.parse_primary()?;
        loop {
            if self.check(&TokenKind::Dot) {
                self.advance();
                let (name, nspan) = self.expect_ident()?;
                if self.check(&TokenKind::LParen) {
                    let (args, end) = self.parse_call_args()?;
                    let span = expr.span().to(end);
                    expr = Expr::MethodCall {
                        receiver: Box::new(expr),
                        method: name,
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
    fn parse_call_args(&mut self) -> PResult<(Vec<CallArg>, Span)> {
        self.expect(TokenKind::LParen, "'('")?;
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
        Ok((args, end))
    }

    fn parse_primary(&mut self) -> PResult<Expr> {
        let tok = self.peek().clone();
        match tok.kind {
            TokenKind::IntLit(v) => {
                self.advance();
                Ok(Expr::IntLit(v, tok.span))
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
                self.advance();
                if self.check(&TokenKind::LParen) {
                    let (args, end) = self.parse_call_args()?;
                    Ok(Expr::Call {
                        callee: name,
                        args,
                        span: tok.span.to(end),
                    })
                } else {
                    Ok(Expr::Ident(name, tok.span))
                }
            }
            TokenKind::LParen => {
                self.advance();
                let expr = self.parse_expr()?;
                self.expect(TokenKind::RParen, "')'")?;
                Ok(expr)
            }
            TokenKind::LBrace => {
                let block = self.parse_block()?;
                Ok(Expr::Block(block))
            }
            TokenKind::If => self.parse_if(),
            TokenKind::When => self.parse_when(),
            other => Err(ParseError {
                message: format!("unexpected token {other:?}"),
                span: tok.span,
            }),
        }
    }

    fn parse_if(&mut self) -> PResult<Expr> {
        let start = self.expect(TokenKind::If, "'if'")?.span;
        let cond = self.parse_expr()?;
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

    /// Parses `when { cond -> expr, ..., else -> expr }` and immediately
    /// desugars it to a nested `if`/`else if`/`else` chain — `infer.rs` and
    /// `codegen_c.rs` never see a `when` at all.
    fn parse_when(&mut self) -> PResult<Expr> {
        let start = self.expect(TokenKind::When, "'when'")?.span;
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
            self.expect(TokenKind::Arrow, "'->'")?;
            let body = self.parse_expr()?;
            arms.push((cond, body));

            if !self.check(&TokenKind::RBrace) {
                if !self.check(&TokenKind::Newline) {
                    return Err(ParseError {
                        message: format!(
                            "expected a newline or '}}' after a 'when' arm, found {:?}",
                            self.peek_kind()
                        ),
                        span: self.peek().span,
                    });
                }
                self.skip_newlines();
            }
        }
        let end = self.expect(TokenKind::RBrace, "'}'")?.span;
        let span = start.to(end);

        if arms.is_empty() {
            return Err(ParseError {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;

    fn parse(src: &str) -> Program {
        let tokens = Lexer::new(src).tokenize().expect("lex failed");
        Parser::new(tokens).parse_program().expect("parse failed")
    }

    fn parse_err(src: &str) -> ParseError {
        let tokens = Lexer::new(src).tokenize().expect("lex failed");
        Parser::new(tokens).parse_program().expect_err("expected a parse error")
    }

    #[test]
    fn parses_empty_block_bodied_function() {
        let p = parse("fun main() {}");
        assert_eq!(p.functions.len(), 1);
        assert_eq!(p.functions[0].name, "main");
        assert!(p.functions[0].params.is_empty());
        assert!(p.functions[0].return_type.is_none());
        assert!(matches!(p.functions[0].body, FunctionBody::Block(_)));
    }

    #[test]
    fn parses_params_and_return_clause() {
        let p = parse("fun add(a: i32, b: i32).R: i32 {\n    return a + b\n}");
        let f = &p.functions[0];
        assert_eq!(f.params.len(), 2);
        assert_eq!(f.params[0].name, "a");
        assert_eq!(f.params[0].ty.name, "i32");
        assert_eq!(f.return_type.as_ref().unwrap().name, "i32");
        match &f.body {
            FunctionBody::Block(b) => {
                assert_eq!(b.stmts.len(), 1);
                match &b.stmts[0] {
                    Stmt::Return { value: Some(e), .. } => {
                        assert!(matches!(e, Expr::Binary { op: BinOp::Add, .. }));
                    }
                    other => panic!("expected return stmt, got {other:?}"),
                }
            }
            other => panic!("expected block body, got {other:?}"),
        }
    }

    #[test]
    fn parses_expression_bodied_function() {
        let p = parse("fun add(a: i32, b: i32).R: i32 = a + b");
        match &p.functions[0].body {
            FunctionBody::Expr(e) => assert!(matches!(e, Expr::Binary { op: BinOp::Add, .. })),
            other => panic!("expected expr body, got {other:?}"),
        }
    }

    #[test]
    fn expression_body_without_return_type_is_an_error() {
        parse_err("fun add(a: i32) = a");
    }

    #[test]
    fn parses_val_and_var() {
        let p = parse("fun main() {\n    val x = 1\n    var y: i32 = 2\n}");
        let stmts = match &p.functions[0].body {
            FunctionBody::Block(b) => &b.stmts,
            _ => panic!("expected block body"),
        };
        assert_eq!(stmts.len(), 2);
        match &stmts[0] {
            Stmt::Let { name, mutable, ty, init, .. } => {
                assert_eq!(name, "x");
                assert!(!mutable);
                assert!(ty.is_none());
                assert!(matches!(init, Expr::IntLit(1, _)));
            }
            other => panic!("expected let stmt, got {other:?}"),
        }
        match &stmts[1] {
            Stmt::Let { name, mutable, ty, .. } => {
                assert_eq!(name, "y");
                assert!(*mutable);
                assert_eq!(ty.as_ref().unwrap().name, "i32");
            }
            other => panic!("expected let stmt, got {other:?}"),
        }
    }

    #[test]
    fn newline_terminates_statements_without_semicolons() {
        let p = parse("fun main() {\n    val x = 1\n    val y = 2\n}");
        let stmts = match &p.functions[0].body {
            FunctionBody::Block(b) => &b.stmts,
            _ => panic!("expected block body"),
        };
        assert_eq!(stmts.len(), 2);
    }

    #[test]
    fn two_statements_on_one_line_without_separator_is_an_error() {
        parse_err("fun main() {\n    val x = 1 val y = 2\n}");
    }

    #[test]
    fn respects_operator_precedence() {
        let p = parse("fun main() {\n    val x = 1 + 2 * 3\n}");
        let stmts = match &p.functions[0].body {
            FunctionBody::Block(b) => &b.stmts,
            _ => panic!("expected block body"),
        };
        match &stmts[0] {
            Stmt::Let { init, .. } => match init {
                Expr::Binary { op: BinOp::Add, lhs, rhs, .. } => {
                    assert!(matches!(**lhs, Expr::IntLit(1, _)));
                    assert!(matches!(**rhs, Expr::Binary { op: BinOp::Mul, .. }));
                }
                other => panic!("expected top-level addition, got {other:?}"),
            },
            other => panic!("expected let stmt, got {other:?}"),
        }
    }

    #[test]
    fn parses_if_else_as_expression_and_statement() {
        let p = parse(
            "fun main() {\n    if 1 < 2 {\n        print(1)\n    } else {\n        print(2)\n    }\n    val x = if true { 1 } else { 2 }\n}",
        );
        let stmts = match &p.functions[0].body {
            FunctionBody::Block(b) => &b.stmts,
            _ => panic!("expected block body"),
        };
        assert_eq!(stmts.len(), 2);
        assert!(matches!(stmts[0], Stmt::Expr { expr: Expr::If { .. }, .. }));
        match &stmts[1] {
            Stmt::Let { init, .. } => assert!(matches!(init, Expr::If { .. })),
            other => panic!("expected let stmt, got {other:?}"),
        }
    }

    #[test]
    fn parses_while_with_compound_assignment() {
        let p = parse("fun main() {\n    var i = 0\n    while i < 10 {\n        i += 1\n    }\n}");
        let stmts = match &p.functions[0].body {
            FunctionBody::Block(b) => &b.stmts,
            _ => panic!("expected block body"),
        };
        match &stmts[1] {
            Stmt::While { body, .. } => {
                assert_eq!(body.stmts.len(), 1);
                match &body.stmts[0] {
                    Stmt::Expr { expr: Expr::Assign { value, .. }, .. } => {
                        assert!(matches!(**value, Expr::Binary { op: BinOp::Add, .. }));
                    }
                    other => panic!("expected assign stmt, got {other:?}"),
                }
            }
            other => panic!("expected while stmt, got {other:?}"),
        }
    }

    #[test]
    fn parses_datac_decl_with_val_and_var_fields() {
        let p = parse("datac Point(val x: i32, var y: i32)\nfun main() {}");
        assert_eq!(p.datacs.len(), 1);
        let d = &p.datacs[0];
        assert_eq!(d.name, "Point");
        assert_eq!(d.fields.len(), 2);
        assert!(!d.fields[0].mutable);
        assert!(d.fields[1].mutable);
    }

    #[test]
    fn parses_construction_and_field_access_via_call_syntax() {
        let p = parse("fun main() {\n    val p = Point(x = 1, y = 2)\n    val z = p.x.y\n}");
        let stmts = match &p.functions[0].body {
            FunctionBody::Block(b) => &b.stmts,
            _ => panic!("expected block body"),
        };
        match &stmts[0] {
            Stmt::Let { init: Expr::Call { callee, args, .. }, .. } => {
                assert_eq!(callee, "Point");
                assert_eq!(args.len(), 2);
                assert_eq!(args[0].name.as_deref(), Some("x"));
                assert_eq!(args[1].name.as_deref(), Some("y"));
            }
            other => panic!("expected call, got {other:?}"),
        }
        match &stmts[1] {
            Stmt::Let { init: Expr::FieldAccess { field, base, .. }, .. } => {
                assert_eq!(field, "y");
                assert!(matches!(**base, Expr::FieldAccess { .. }));
            }
            other => panic!("expected field access, got {other:?}"),
        }
    }

    #[test]
    fn parses_mixed_named_and_positional_call_args() {
        let p = parse("fun main() {\n    print(f(1, y = 2))\n}");
        let stmts = match &p.functions[0].body {
            FunctionBody::Block(b) => &b.stmts,
            _ => panic!("expected block body"),
        };
        match &stmts[0] {
            Stmt::Expr { expr: Expr::Call { args, .. }, .. } => {
                let inner = &args[0].value;
                match inner {
                    Expr::Call { args, .. } => {
                        assert!(args[0].name.is_none());
                        assert_eq!(args[1].name.as_deref(), Some("y"));
                    }
                    other => panic!("expected inner call, got {other:?}"),
                }
            }
            other => panic!("expected call stmt, got {other:?}"),
        }
    }

    #[test]
    fn does_not_confuse_equality_with_named_arg() {
        let p = parse("fun main() {\n    val x = f(a == 1)\n}");
        let stmts = match &p.functions[0].body {
            FunctionBody::Block(b) => &b.stmts,
            _ => panic!("expected block body"),
        };
        match &stmts[0] {
            Stmt::Let { init: Expr::Call { args, .. }, .. } => {
                assert!(args[0].name.is_none());
                assert!(matches!(args[0].value, Expr::Binary { op: BinOp::Eq, .. }));
            }
            other => panic!("expected call, got {other:?}"),
        }
    }

    #[test]
    fn parses_when_and_desugars_to_nested_if() {
        let p = parse(
            "fun classify(n: i32).R: i32 = when {\n    n > 0 -> 1\n    n < 0 -> -1\n    else -> 0\n}",
        );
        match &p.functions[0].body {
            FunctionBody::Expr(Expr::If { then_block, else_block, .. }) => {
                assert_eq!(then_block.stmts.len(), 1);
                match else_block.as_deref() {
                    Some(ElseBranch::If(Expr::If { else_block, .. })) => {
                        assert!(matches!(else_block.as_deref(), Some(ElseBranch::Block(_))));
                    }
                    other => panic!("expected nested else-if, got {other:?}"),
                }
            }
            other => panic!("expected desugared if, got {other:?}"),
        }
    }

    #[test]
    fn parses_compound_assignment_operators() {
        let p = parse("fun main() {\n    var x = 1\n    x += 2\n    x -= 1\n    x *= 3\n    x /= 2\n    x %= 2\n}");
        let stmts = match &p.functions[0].body {
            FunctionBody::Block(b) => &b.stmts,
            _ => panic!("expected block body"),
        };
        for (stmt, op) in stmts[1..].iter().zip([BinOp::Add, BinOp::Sub, BinOp::Mul, BinOp::Div, BinOp::Mod]) {
            match stmt {
                Stmt::Expr { expr: Expr::Assign { value, .. }, .. } => {
                    assert!(matches!(&**value, Expr::Binary { op: o, .. } if *o == op));
                }
                other => panic!("expected compound-assign desugared stmt, got {other:?}"),
            }
        }
    }

    #[test]
    fn accepts_ampersand_str_type() {
        let p = parse("fun f(s: &str).R: &str = s");
        assert_eq!(p.functions[0].params[0].ty.name, "str");
        assert_eq!(p.functions[0].return_type.as_ref().unwrap().name, "str");
    }

    #[test]
    fn rejects_malformed_input() {
        parse_err("fun main( {}");
    }

    #[test]
    fn parses_ref_deref_and_mut_param() {
        let p = parse("fun bump(c: mut Counter) {\n    val r = &c\n    val m = &mut c\n    val v = *r\n}");
        assert!(p.functions[0].params[0].mutable);
        let stmts = match &p.functions[0].body {
            FunctionBody::Block(b) => &b.stmts,
            _ => panic!("expected block body"),
        };
        match &stmts[0] {
            Stmt::Let { init: Expr::Ref { mutable: false, .. }, .. } => {}
            other => panic!("expected shared ref, got {other:?}"),
        }
        match &stmts[1] {
            Stmt::Let { init: Expr::Ref { mutable: true, .. }, .. } => {}
            other => panic!("expected exclusive ref, got {other:?}"),
        }
        match &stmts[2] {
            Stmt::Let { init: Expr::Deref { .. }, .. } => {}
            other => panic!("expected deref, got {other:?}"),
        }
    }

    #[test]
    fn ampersand_mut_is_rejected_in_param_position() {
        parse_err("fun bump(c: &mut Counter) {}");
    }

    #[test]
    fn parses_unary_operators() {
        let p = parse("fun main() {\n    val x = -1\n    val y = !true\n}");
        let stmts = match &p.functions[0].body {
            FunctionBody::Block(b) => &b.stmts,
            _ => panic!("expected block body"),
        };
        match &stmts[0] {
            Stmt::Let { init, .. } => assert!(matches!(init, Expr::Unary { op: UnOp::Neg, .. })),
            other => panic!("expected let stmt, got {other:?}"),
        }
        match &stmts[1] {
            Stmt::Let { init, .. } => assert!(matches!(init, Expr::Unary { op: UnOp::Not, .. })),
            other => panic!("expected let stmt, got {other:?}"),
        }
    }

    #[test]
    fn parses_impl_lines_before_decls() {
        let p = parse("impl \"shapes/circle\"\nimpl \"time\" as t\n\nfun main() {}");
        assert_eq!(p.impls.len(), 2);
        assert_eq!(p.impls[0].path, "shapes/circle");
        assert_eq!(p.impls[0].alias, None);
        assert_eq!(p.impls[1].path, "time");
        assert_eq!(p.impls[1].alias.as_deref(), Some("t"));
        assert_eq!(p.functions.len(), 1);
    }

    #[test]
    fn parses_module_qualified_type_names() {
        let p = parse("fun f(c: circle.Circle).R: circle.Circle = c");
        assert_eq!(p.functions[0].params[0].ty.name, "circle.Circle");
        assert_eq!(p.functions[0].return_type.as_ref().unwrap().name, "circle.Circle");
    }

    #[test]
    fn impl_after_a_declaration_is_rejected() {
        // Canonical order is impl, pd, then datac/fun.
        let e = parse_err("fun main() {}\nimpl \"util\"");
        assert!(e.message.contains("must come before"), "got: {}", e.message);
    }

    #[test]
    fn caps_clause_is_a_clear_not_yet_error() {
        let e = parse_err("impl \"util\" with caps(fs = [\"/tmp/*\"])");
        assert!(e.message.contains("M7"), "got: {}", e.message);
    }

    #[test]
    fn impl_requires_a_string_path() {
        parse_err("impl util");
    }
}
