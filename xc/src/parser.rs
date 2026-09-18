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

    pub fn parse_program(&mut self) -> PResult<Program> {
        let mut functions = Vec::new();
        while !matches!(self.peek_kind(), TokenKind::Eof) {
            functions.push(self.parse_function()?);
        }
        Ok(Program { functions })
    }

    fn parse_function(&mut self) -> PResult<FunctionDecl> {
        let start = self.expect(TokenKind::Fn, "'fn'")?.span;
        let (name, _) = self.expect_ident()?;
        self.expect(TokenKind::LParen, "'('")?;
        let mut params = Vec::new();
        while !self.check(&TokenKind::RParen) {
            let (pname, pspan) = self.expect_ident()?;
            self.expect(TokenKind::Colon, "':'")?;
            let ty = self.parse_type()?;
            params.push(Param {
                name: pname,
                ty,
                span: pspan,
            });
            if self.check(&TokenKind::Comma) {
                self.advance();
            } else {
                break;
            }
        }
        self.expect(TokenKind::RParen, "')'")?;

        let return_type = if self.check(&TokenKind::Arrow) {
            self.advance();
            Some(self.parse_type()?)
        } else {
            None
        };

        let body = self.parse_block()?;
        let span = start.to(body.span);
        Ok(FunctionDecl {
            name,
            params,
            return_type,
            body,
            span,
        })
    }

    fn parse_type(&mut self) -> PResult<TypeExpr> {
        let (name, span) = self.expect_ident()?;
        Ok(TypeExpr { name, span })
    }

    fn parse_block(&mut self) -> PResult<Block> {
        let start = self.expect(TokenKind::LBrace, "'{'")?.span;
        let mut stmts = Vec::new();
        while !self.check(&TokenKind::RBrace) {
            stmts.push(self.parse_stmt()?);
        }
        let end = self.expect(TokenKind::RBrace, "'}'")?.span;
        Ok(Block {
            stmts,
            span: start.to(end),
        })
    }

    fn parse_stmt(&mut self) -> PResult<Stmt> {
        match self.peek_kind() {
            TokenKind::Let => self.parse_let_stmt(),
            TokenKind::Return => self.parse_return_stmt(),
            _ => {
                let expr = self.parse_expr()?;
                let block_like = matches!(expr, Expr::If { .. } | Expr::Block(_));
                if self.check(&TokenKind::Semi) {
                    let semi = self.advance();
                    let span = expr.span().to(semi.span);
                    Ok(Stmt::Expr { expr, span })
                } else if block_like || self.check(&TokenKind::RBrace) {
                    // block-like statement, or a tail expression ending a block
                    let span = expr.span();
                    Ok(Stmt::Expr { expr, span })
                } else {
                    Err(ParseError {
                        message: format!("expected ';', found {:?}", self.peek_kind()),
                        span: self.peek().span,
                    })
                }
            }
        }
    }

    fn parse_let_stmt(&mut self) -> PResult<Stmt> {
        let start = self.expect(TokenKind::Let, "'let'")?.span;
        let mutable = if self.check(&TokenKind::Mut) {
            self.advance();
            true
        } else {
            false
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
        let end = self.expect(TokenKind::Semi, "';'")?.span;
        Ok(Stmt::Let {
            name,
            mutable,
            ty,
            init,
            span: start.to(end),
        })
    }

    fn parse_return_stmt(&mut self) -> PResult<Stmt> {
        let start = self.expect(TokenKind::Return, "'return'")?.span;
        let value = if self.check(&TokenKind::Semi) {
            None
        } else {
            Some(self.parse_expr()?)
        };
        let end = self.expect(TokenKind::Semi, "';'")?.span;
        Ok(Stmt::Return {
            value,
            span: start.to(end),
        })
    }

    // ---- expressions (Pratt parsing) ----

    fn parse_expr(&mut self) -> PResult<Expr> {
        self.parse_binary(0)
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
            _ => self.parse_postfix(),
        }
    }

    fn parse_postfix(&mut self) -> PResult<Expr> {
        self.parse_primary()
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
                    self.advance();
                    let mut args = Vec::new();
                    while !self.check(&TokenKind::RParen) {
                        args.push(self.parse_expr()?);
                        if self.check(&TokenKind::Comma) {
                            self.advance();
                        } else {
                            break;
                        }
                    }
                    let end = self.expect(TokenKind::RParen, "')'")?.span;
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;

    fn parse(src: &str) -> Program {
        let tokens = Lexer::new(src).tokenize().expect("lex failed");
        Parser::new(tokens).parse_program().expect("parse failed")
    }

    #[test]
    fn parses_empty_function() {
        let p = parse("fn main() {}");
        assert_eq!(p.functions.len(), 1);
        assert_eq!(p.functions[0].name, "main");
        assert!(p.functions[0].params.is_empty());
        assert!(p.functions[0].return_type.is_none());
    }

    #[test]
    fn parses_params_and_return_type() {
        let p = parse("fn add(a: i32, b: i32) -> i32 { return a + b; }");
        let f = &p.functions[0];
        assert_eq!(f.params.len(), 2);
        assert_eq!(f.params[0].name, "a");
        assert_eq!(f.params[0].ty.name, "i32");
        assert_eq!(f.return_type.as_ref().unwrap().name, "i32");
        assert_eq!(f.body.stmts.len(), 1);
        match &f.body.stmts[0] {
            Stmt::Return { value: Some(e), .. } => match e {
                Expr::Binary { op: BinOp::Add, .. } => {}
                other => panic!("expected addition, got {other:?}"),
            },
            other => panic!("expected return stmt, got {other:?}"),
        }
    }

    #[test]
    fn parses_let_with_and_without_type() {
        let p = parse("fn main() { let x = 1; let mut y: i32 = 2; }");
        let stmts = &p.functions[0].body.stmts;
        assert_eq!(stmts.len(), 2);
        match &stmts[0] {
            Stmt::Let {
                name,
                mutable,
                ty,
                init,
                ..
            } => {
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
    fn respects_operator_precedence() {
        let p = parse("fn main() { let x = 1 + 2 * 3; }");
        match &p.functions[0].body.stmts[0] {
            Stmt::Let { init, .. } => match init {
                Expr::Binary {
                    op: BinOp::Add,
                    lhs,
                    rhs,
                    ..
                } => {
                    assert!(matches!(**lhs, Expr::IntLit(1, _)));
                    assert!(matches!(**rhs, Expr::Binary { op: BinOp::Mul, .. }));
                }
                other => panic!("expected top-level addition, got {other:?}"),
            },
            other => panic!("expected let stmt, got {other:?}"),
        }
    }

    #[test]
    fn parses_comparison_and_logical_ops() {
        let p = parse("fn main() { let x = 1 < 2 && 3 >= 4; }");
        match &p.functions[0].body.stmts[0] {
            Stmt::Let { init, .. } => {
                assert!(matches!(init, Expr::Binary { op: BinOp::And, .. }));
            }
            other => panic!("expected let stmt, got {other:?}"),
        }
    }

    #[test]
    fn parses_if_else_as_expression_and_statement() {
        let p = parse(
            "fn main() { if 1 < 2 { println(1); } else { println(2); } let x = if true { 1 } else { 2 }; }",
        );
        let stmts = &p.functions[0].body.stmts;
        assert_eq!(stmts.len(), 2);
        assert!(matches!(stmts[0], Stmt::Expr { expr: Expr::If { .. }, .. }));
        match &stmts[1] {
            Stmt::Let { init, .. } => assert!(matches!(init, Expr::If { .. })),
            other => panic!("expected let stmt, got {other:?}"),
        }
    }

    #[test]
    fn parses_function_calls() {
        let p = parse("fn main() { println(fib(10)); }");
        match &p.functions[0].body.stmts[0] {
            Stmt::Expr {
                expr: Expr::Call { callee, args, .. },
                ..
            } => {
                assert_eq!(callee, "println");
                assert_eq!(args.len(), 1);
                assert!(matches!(args[0], Expr::Call { .. }));
            }
            other => panic!("expected call stmt, got {other:?}"),
        }
    }

    #[test]
    fn rejects_malformed_input() {
        let tokens = Lexer::new("fn main( {}").tokenize().expect("lex failed");
        let result = Parser::new(tokens).parse_program();
        assert!(result.is_err());
    }

    #[test]
    fn parses_unary_operators() {
        let p = parse("fn main() { let x = -1; let y = !true; }");
        let stmts = &p.functions[0].body.stmts;
        match &stmts[0] {
            Stmt::Let { init, .. } => {
                assert!(matches!(init, Expr::Unary { op: UnOp::Neg, .. }));
            }
            other => panic!("expected let stmt, got {other:?}"),
        }
        match &stmts[1] {
            Stmt::Let { init, .. } => {
                assert!(matches!(init, Expr::Unary { op: UnOp::Not, .. }));
            }
            other => panic!("expected let stmt, got {other:?}"),
        }
    }
}
