use crate::ast::*;
use crate::lexer::{Span, Token, TokenKind};

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    /// How many `if let`s have been desugared, so each one's synthesized
    /// scrutinee binding has a name of its own (`parser::IF_LET_PREFIX`).
    if_lets: usize,
    /// How many `for` loops have been desugared, for the same reason
    /// (`parser::FOR_PREFIX`).
    fors: usize,
}

#[derive(Debug)]
pub struct ParseError {
    pub message: String,
    pub span: Span,
}

type PResult<T> = Result<T, ParseError>;

/// The name prefix of the local an `if let` binds its scrutinee to. `#` cannot
/// appear in a lexed identifier, so a synthesized name can never collide with
/// one the user wrote — the same trick `infer.rs` uses for a hidden passdown
/// parameter.
const IF_LET_PREFIX: &str = "#iflet";

/// The name prefix of the two locals a `for` loop synthesizes: the sequence
/// (`#for0`) and its index (`#for0_i`). Same `#` trick as `IF_LET_PREFIX`, so
/// a synthesized name can never collide with one the user wrote.
const FOR_PREFIX: &str = "#for";

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Parser {
            tokens,
            pos: 0,
            if_lets: 0,
            fors: 0,
        }
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
        let mut consts = Vec::new();
        let mut pds = Vec::new();
        let mut datacs = Vec::new();
        let mut enums = Vec::new();
        let mut functions = Vec::new();
        // One counter across both type declarations: it is what `resolve.rs`
        // uses to rebuild a single merged declaration order from the two
        // lists (A-1).
        let mut type_order = 0usize;
        while !matches!(self.peek_kind(), TokenKind::Eof) {
            // `pub` is an optional leading token on the item forms that can
            // carry one (A-13). It is read here, before the dispatch below,
            // because a public item is otherwise the same declaration.
            let is_pub = self.check(&TokenKind::Pub);
            if is_pub {
                let pub_span = self.advance().span;
                self.check_pub_target(pub_span)?;
            }
            if self.check(&TokenKind::Impl) {
                if !consts.is_empty() || !pds.is_empty() || !datacs.is_empty() || !enums.is_empty() || !functions.is_empty() {
                    return Err(ParseError {
                        message: "'impl' lines must come first (canonical order: impl, const/pd, datac/enum/fun)".to_string(),
                        span: self.peek().span,
                    });
                }
                impls.push(self.parse_impl_decl()?);
            } else if self.check(&TokenKind::Const) {
                if !pds.is_empty() || !datacs.is_empty() || !enums.is_empty() || !functions.is_empty() {
                    return Err(ParseError {
                        message: "'const' lines must come before any 'pd', 'datac', 'enum' or 'fun' (canonical order: impl, const/pd, datac/enum/fun)".to_string(),
                        span: self.peek().span,
                    });
                }
                consts.push(self.parse_const_decl(is_pub)?);
            } else if self.check(&TokenKind::Pd) {
                if !datacs.is_empty() || !enums.is_empty() || !functions.is_empty() {
                    return Err(ParseError {
                        message: "'pd' lines must come before any 'datac', 'enum' or 'fun' (canonical order: impl, const/pd, datac/enum/fun)".to_string(),
                        span: self.peek().span,
                    });
                }
                pds.push(self.parse_pd_decl()?);
            } else if self.check(&TokenKind::Datac) {
                let mut d = self.parse_datac_decl(is_pub)?;
                d.order = type_order;
                type_order += 1;
                datacs.push(d);
            } else if self.check(&TokenKind::Enum) {
                let mut e = self.parse_enum_decl(is_pub)?;
                e.order = type_order;
                type_order += 1;
                enums.push(e);
            } else {
                functions.push(self.parse_function(is_pub)?);
            }
            self.skip_newlines();
        }
        Ok(Program {
            impls,
            consts,
            pds,
            datacs,
            enums,
            functions,
        })
    }

    /// The item a leading `pub` must be followed by (A-13). `pub` on anything
    /// else is refused where it is written, naming what it applies to, rather
    /// than read as a no-op or as something else. A `pd` gets its own message
    /// because it is not a missing feature: a passdown is module-scoped by
    /// design (`plan/roadmap.md` item 10), so `pub` on one can never be true.
    fn check_pub_target(&self, pub_span: Span) -> PResult<()> {
        match self.peek_kind() {
            TokenKind::Fun
            | TokenKind::Extern
            | TokenKind::Datac
            | TokenKind::Enum
            | TokenKind::Const => Ok(()),
            TokenKind::Pd => Err(ParseError {
                message: "a 'pd' cannot be 'pub': a passdown is module-scoped by design, so another module cannot reach it; a 'fun' that reads it can be exported instead".to_string(),
                span: pub_span.to(self.peek().span),
            }),
            _ => Err(ParseError {
                message: format!(
                    "'pub' applies to a top-level 'fun', 'extern fun', 'datac', 'enum' or 'const', found {:?}",
                    self.peek_kind()
                ),
                span: pub_span.to(self.peek().span),
            }),
        }
    }

    /// `const NAME: T = expr` — the type is mandatory, since a `const` is a
    /// declared contract rather than an inferred binding.
    fn parse_const_decl(&mut self, is_pub: bool) -> PResult<ConstDecl> {
        let start = self.expect(TokenKind::Const, "'const'")?.span;
        let (name, _) = self.expect_ident()?;
        self.expect(TokenKind::Colon, "':' (a 'const' must declare its type)")?;
        let ty = self.parse_type()?;
        self.expect(TokenKind::Eq, "'='")?;
        let init = self.parse_expr()?;
        let span = start.to(init.span());
        Ok(ConstDecl {
            name,
            is_pub,
            ty,
            init,
            module: String::new(),
            span,
        })
    }

    /// `pd val name = expr` / `pd var name = expr` / `pd val name: T = expr`.
    fn parse_pd_decl(&mut self) -> PResult<PdDecl> {
        let start = self.expect(TokenKind::Pd, "'pd'")?.span;
        let mutable = if self.check(&TokenKind::Var) {
            self.advance();
            true
        } else {
            self.expect(TokenKind::Val, "'val' or 'var'")?;
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
        let span = start.to(init.span());
        Ok(PdDecl {
            name,
            mutable,
            ty,
            init,
            module: String::new(),
            span,
        })
    }

    /// `impl "shapes/circle"` and the capability grant
    /// `impl "std/fs" caps(fs = ["/var/log/myapp/*"])` (M7).
    /// `with caps(...)` is refused by name, the way `as name` is: both were
    /// deprecated by `26.09.0` and removed in the cycle that followed.
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
        // `as name` was removed once a module's own name became its namespace
        // (`syntax.md` 13.1). The token is still a keyword, so the old clause is
        // refused here, where the path is in hand, rather than falling through
        // to the generic "expected an item" error further up.
        if self.check(&TokenKind::As) {
            return Err(ParseError {
                message: "`impl \"...\" as name` is gone: reach the module by its own name \
                          (`std/path` is `path`, `shapes/circle` is `circle`, and a dependency's \
                          entry module `deps/argparse/lib` is `argparse`)"
                    .to_string(),
                span: self.peek().span,
            });
        }
        // `with caps(...)` was the keyword in front of the grant, and it was
        // removed with the entry that scheduled it (`deprecated.rs`, `syntax.md`
        // 23): `impl` already means "this module declares what it imports", so
        // the `with` states nothing the clause does not. It is refused here,
        // where the path is in hand and the surviving spelling is known, rather
        // than falling through to the generic "expected an item" error above the
        // grants this clause still accepts.
        if matches!(self.peek_kind(), TokenKind::Ident(n) if n == "with") {
            return Err(ParseError {
                message: "`impl \"...\" with caps(...)` is gone: drop the `with`, so the grant is \
                          `impl \"std/fs\" caps(fs = [\"/var/log/**\"])`, and the patterns, the \
                          domains and the no-escalation rule are exactly the same"
                    .to_string(),
                span: self.peek().span,
            });
        }
        // `caps(domain = ["pattern", ...], ...)` — the capability grant
        // (M7). Domains and patterns are validated in `resolve.rs`, where the
        // module graph (and therefore the importer's own set) is known.
        let has_caps_clause = if matches!(self.peek_kind(), TokenKind::Ident(n) if n == "caps") {
            self.advance();
            true
        } else {
            false
        };
        let caps = if has_caps_clause {
            self.expect(TokenKind::LParen, "'(' after 'caps'")?;
            let mut grants: Vec<CapGrant> = Vec::new();
            while !self.check(&TokenKind::RParen) {
                let (domain, dspan) = self.expect_ident()?;
                self.expect(TokenKind::Eq, "'=' after the capability domain")?;
                self.expect(TokenKind::LBracket, "'[' before the pattern list")?;
                let mut patterns = Vec::new();
                while !self.check(&TokenKind::RBracket) {
                    match self.peek_kind().clone() {
                        TokenKind::StringLit(p) => {
                            self.advance();
                            patterns.push(p);
                        }
                        other => {
                            return Err(ParseError {
                                message: format!(
                                    "expected a quoted path or host pattern, found {other:?}"
                                ),
                                span: self.peek().span,
                            })
                        }
                    }
                    if self.check(&TokenKind::Comma) {
                        self.advance();
                    } else {
                        break;
                    }
                }
                self.expect(TokenKind::RBracket, "']'")?;
                grants.push(CapGrant {
                    domain,
                    patterns,
                    span: dspan,
                });
                if self.check(&TokenKind::Comma) {
                    self.advance();
                } else {
                    break;
                }
            }
            self.expect(TokenKind::RParen, "')'")?;
            grants
        } else {
            Vec::new()
        };
        Ok(ImplDecl {
            path,
            caps,
            span: start.to(path_span),
        })
    }

    fn parse_datac_decl(&mut self, is_pub: bool) -> PResult<DatacDecl> {
        let start = self.expect(TokenKind::Datac, "'datac'")?.span;
        let (name, _) = self.expect_ident()?;
        let type_params = self.parse_type_params()?;
        self.expect(TokenKind::LParen, "'('")?;
        let fields = self.parse_field_list()?;
        let end = self.expect(TokenKind::RParen, "')'")?.span;
        Ok(DatacDecl {
            name,
            is_pub,
            type_params,
            fields,
            order: 0,
            module: String::new(),
            span: start.to(end),
        })
    }

    /// `<T, U>` after a declared type name (A-2). Returns an empty vector when
    /// no `<` was written, so this is a no-op for every ordinary declaration.
    fn parse_type_params(&mut self) -> PResult<Vec<String>> {
        if !self.check(&TokenKind::Lt) {
            return Ok(Vec::new());
        }
        let open = self.advance().span;
        if self.check(&TokenKind::Gt) {
            return Err(ParseError {
                message: "a type parameter list needs at least one parameter ('<T>')".to_string(),
                span: open.to(self.peek().span),
            });
        }
        let mut params = Vec::new();
        loop {
            let (param, pspan) = self.expect_ident()?;
            if params.contains(&param) {
                return Err(ParseError {
                    message: format!("type parameter '{param}' is declared more than once"),
                    span: pspan,
                });
            }
            params.push(param);
            if self.check(&TokenKind::Comma) {
                self.advance();
                continue;
            }
            break;
        }
        self.expect(TokenKind::Gt, "'>' to close the type parameter list")?;
        Ok(params)
    }

    /// The payload of a variant, or a `datac`'s field list, with the opening
    /// `(` already consumed and the closing `)` left for the caller. Shared
    /// so an `enum` variant and a `datac` field obey exactly the same
    /// `val`/`var name: Type` rules.
    fn parse_field_list(&mut self) -> PResult<Vec<FieldDecl>> {
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
        Ok(fields)
    }

    /// `enum Shape { Circle(val r: f64) Rect(val w: f64, val h: f64) Empty }`
    /// — variants are newline-separated, and a variant's payload list is the
    /// same one a `datac` uses (`plan/syntax.md` §9.2).
    fn parse_enum_decl(&mut self, is_pub: bool) -> PResult<EnumDecl> {
        let start = self.expect(TokenKind::Enum, "'enum'")?.span;
        let (name, _) = self.expect_ident()?;
        let type_params = self.parse_type_params()?;
        self.expect(TokenKind::LBrace, "'{'")?;
        self.skip_newlines();
        let mut variants = Vec::new();
        while !self.check(&TokenKind::RBrace) {
            let (vname, vspan) = self.expect_ident()?;
            let mut fields = Vec::new();
            if self.check(&TokenKind::LParen) {
                self.advance();
                fields = self.parse_field_list()?;
                self.expect(TokenKind::RParen, "')'")?;
            }
            variants.push(VariantDecl {
                name: vname,
                fields,
                span: vspan,
            });
            if !self.check(&TokenKind::RBrace) {
                if !self.check(&TokenKind::Newline) {
                    return Err(ParseError {
                        message: format!(
                            "expected a newline or '}}' after an enum variant, found {:?}",
                            self.peek_kind()
                        ),
                        span: self.peek().span,
                    });
                }
                self.skip_newlines();
            }
        }
        let end = self.expect(TokenKind::RBrace, "'}'")?.span;
        if variants.is_empty() {
            return Err(ParseError {
                message: "an 'enum' needs at least one variant".to_string(),
                span: start.to(end),
            });
        }
        Ok(EnumDecl {
            name,
            is_pub,
            type_params,
            variants,
            order: 0,
            module: String::new(),
            span: start.to(end),
        })
    }

    fn parse_function(&mut self, is_pub: bool) -> PResult<FunctionDecl> {
        // `extern fun ...` — a declaration whose body the compiler supplies
        // (M6.2). It has no body at all, and cannot be a receiver method
        // (there is nothing to give it a body with).
        let extern_ = if self.check(&TokenKind::Extern) {
            self.advance();
            true
        } else {
            false
        };
        let start = self.expect(TokenKind::Fun, "'fun'")?.span;

        // A receiver-form decl is `fun [mut] Type.name(...)`; a plain one
        // is `fun name(...)`. `mut` here can only ever start a receiver
        // (free functions never write `fun mut name(...)`), so seeing it
        // commits to that form. `move` never belongs here: a method that
        // took its receiver by value would destroy the caller's value at a
        // call that looks like a read (`c.total()`), so the spelling is
        // refused with the parameter form named instead (A-4).
        if self.check(&TokenKind::Move) {
            return Err(ParseError {
                message: "'move' cannot be written on a receiver — a method always borrows its receiver; a move-taking parameter is written 'name: move T'".to_string(),
                span: self.peek().span,
            });
        }
        let receiver_mutable = if self.check(&TokenKind::Mut) {
            self.advance();
            true
        } else {
            false
        };
        let (first, first_span) = self.expect_ident()?;
        // `<T>` may follow either the function's own name (`fun f<T>(...)`) or
        // the receiver's type name (`fun Pair<A, B>.first()`); which one it is
        // is settled by whether a `.` comes next (A-2).
        let name_params = self.parse_type_params()?;
        let (receiver, name, type_params) = if self.check(&TokenKind::Dot) {
            self.advance();
            let (method, _) = self.expect_ident()?;
            (
                Some(Receiver {
                    type_name: first,
                    type_args: name_params,
                    mutable: receiver_mutable,
                    span: first_span,
                }),
                method,
                Vec::new(),
            )
        } else {
            if receiver_mutable {
                return Err(ParseError {
                    message: "'mut' before a function name is only valid on a receiver ('fun mut Type.method(...)')".to_string(),
                    span: first_span,
                });
            }
            (None, first, name_params)
        };

        // A method is reached through a value of its type, never through a
        // module alias, so `pub` on one has nothing to govern: the type's own
        // `pub` is what exports the method with it (A-13).
        if is_pub && receiver.is_some() {
            return Err(ParseError {
                message: "'pub' cannot be written on a method: a method's reachability follows its declaring type, so mark the type 'pub' instead".to_string(),
                span: start,
            });
        }

        let params = self.parse_params()?;

        if extern_ && receiver.is_some() {
            return Err(ParseError {
                message: "'extern' cannot be used on a receiver method — declare it as a free function"
                    .to_string(),
                span: start,
            });
        }

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

        let body = if extern_ {
            // No body and no `=`: the C is supplied by the compiler. A
            // written body would be dead code, so say so rather than ignore
            // it.
            if self.check(&TokenKind::Eq) || self.check(&TokenKind::LBrace) {
                return Err(ParseError {
                    message: "an 'extern fun' declaration has no body — the compiler supplies it"
                        .to_string(),
                    span: self.peek().span,
                });
            }
            FunctionBody::Extern
        } else if self.check(&TokenKind::Eq) {
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
            FunctionBody::Extern => return_type.as_ref().map(|t| t.span).unwrap_or(start),
        };
        Ok(FunctionDecl {
            name,
            is_pub,
            type_params,
            extern_,
            receiver,
            params,
            return_type,
            body,
            module: String::new(),
            span: start.to(end),
        })
    }

    fn parse_type(&mut self) -> PResult<TypeExpr> {
        // A leading `&` is recorded but not applied here: in a parameter
        // position `&str`/`&T` is exactly what the bare name already means
        // (a borrowed view passed by value), while in a *return* position it
        // is the elided-borrow form `infer.rs` turns into a `Ty::Ref`
        // (M6.3). Only the resolver knows which position it is looking at.
        let amp_start = if self.check(&TokenKind::Amp) {
            Some(self.advance().span)
        } else {
            None
        };
        // A function type (A-4): `fun(i32, String).R: bool`. `fun` is a
        // keyword, so this can never be an ordinary type name. A closure type
        // (A-4 phase E) is the same shape under the `closure` keyword:
        // `closure(i32).R: i32` is a function value that owns an environment.
        // A fixed-size array type (A-18): `[T; N]`. Its length is a
        // compile-time constant expression (A-19), kept as an expression here
        // and folded by `infer.rs` when the type is resolved. So
        // `[Entry; PAGE_SIZE / ENTRY_SIZE]` is one type, and the `;` between
        // the two halves is the token that says so.
        if self.check(&TokenKind::LBracket) {
            let open = self.advance().span;
            let elem = self.parse_type()?;
            self.expect(TokenKind::Semi, "';' between an array's element type and its length")?;
            let len = self.parse_expr()?;
            let close = self.expect(TokenKind::RBracket, "']' to close the array type")?.span;
            return Ok(TypeExpr {
                name: String::new(),
                tuple: None,
                args: Vec::new(),
                array_elem: Some(Box::new(elem)),
                array_len: Some(Box::new(len)),
                amp: amp_start.is_some(),
                fn_params: None,
                fn_ret: None,
                span: amp_start.map(|a| a.to(close)).unwrap_or_else(|| open.to(close)),
            });
        }
        // A tuple type (MISSING-z2bB9): `(A, B)`, two or more element types.
        // A tuple is a record with positional fields (`plan/syntax.md` 3), so
        // this is a *type* like any other. `(T)` is not a tuple: it is the
        // parenthesized type `T`, and `()` is refused by name because the
        // absent value's type is spelled `unit`.
        if self.check(&TokenKind::LParen) {
            let open = self.advance().span;
            if self.check(&TokenKind::RParen) {
                let close = self.advance().span;
                return Err(ParseError {
                    message: "'()' is not a type: a tuple has two or more element types ('(i32, i32)'), and the absent value's type is 'unit'"
                        .to_string(),
                    span: amp_start.map(|a| a.to(close)).unwrap_or_else(|| open.to(close)),
                });
            }
            let first = self.parse_type()?;
            if !self.check(&TokenKind::Comma) {
                let close = self.expect(TokenKind::RParen, "')' to close the tuple type")?.span;
                let mut inner = first;
                if let Some(amp) = amp_start {
                    inner.amp = true;
                    inner.span = amp.to(close);
                }
                return Ok(inner);
            }
            let mut elems = vec![first];
            while self.check(&TokenKind::Comma) {
                self.advance();
                elems.push(self.parse_type()?);
            }
            let close = self.expect(TokenKind::RParen, "')' to close the tuple type")?.span;
            let span = amp_start.map(|a| a.to(close)).unwrap_or_else(|| open.to(close));
            return Ok(TypeExpr {
                name: String::new(),
                tuple: Some(elems),
                args: Vec::new(),
                array_elem: None,
                array_len: None,
                amp: amp_start.is_some(),
                fn_params: None,
                fn_ret: None,
                span,
            });
        }
        if self.check(&TokenKind::Fun) {
            let start = self.advance().span;
            return self.parse_fn_type(start, amp_start, "fun");
        }
        if self.check(&TokenKind::Closure) {
            let start = self.advance().span;
            return self.parse_fn_type(start, amp_start, "closure");
        }
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
        // Type arguments (collections milestone): `List<T>`. Unambiguous in
        // a type position, and v1 knows exactly one generic name, so a
        // comma-separated list is parsed here and rejected in `infer.rs`
        // with a message naming which type does not take arguments.
        if self.check(&TokenKind::Lt) {
            let (args, close) = self.parse_type_arg_list()?;
            span = span.to(close);
            return Ok(TypeExpr {
                name,
                tuple: None,
                args,
                array_elem: None,
                array_len: None,
                amp: amp_start.is_some(),
                fn_params: None,
                fn_ret: None,
                span: amp_start.map(|a| a.to(span)).unwrap_or(span),
            });
        }
        Ok(TypeExpr {
            name,
            tuple: None,
            args: Vec::new(),
            array_elem: None,
            array_len: None,
            amp: amp_start.is_some(),
            fn_params: None,
            fn_ret: None,
            span: amp_start.map(|a| a.to(span)).unwrap_or(span),
        })
    }

    /// `( name: T, ... )` — one parameter list, shared by a declaration and a
    /// `fun` literal (A-4), since the two take exactly the same parameters.
    fn parse_params(&mut self) -> PResult<Vec<Param>> {
        self.expect(TokenKind::LParen, "'('")?;
        let mut params = Vec::new();
        while !self.check(&TokenKind::RParen) {
            let (pname, pspan) = self.expect_ident()?;
            self.expect(TokenKind::Colon, "':'")?;
            // `mut T` requests an exclusive borrow and `move T` takes
            // ownership (A-4); this is the *only* spelling for either in
            // parameter position — `&mut T` is rejected here (the leading
            // `&` in `parse_type` expects an identifier next, not the `mut`
            // keyword token), and `&`/`mut` stay expression-only.
            let mode = self.parse_param_mode()?;
            let ty = self.parse_type()?;
            params.push(Param {
                name: pname,
                ty,
                mode,
                span: pspan,
            });
            if self.check(&TokenKind::Comma) {
                self.advance();
            } else {
                break;
            }
        }
        self.expect(TokenKind::RParen, "')'")?;
        Ok(params)
    }

    /// `fun(x: i32).R: i32 = x * 2` in expression position — an anonymous
    /// function (A-4). A body is required (there is no `extern` spelling for
    /// a literal), and everything else about the header is a declaration's:
    /// the same parameter list, the same optional `.R: T`.
    fn parse_fn_literal(&mut self) -> PResult<Expr> {
        let start = self.expect(TokenKind::Fun, "'fun'")?.span;
        let params = self.parse_params()?;
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
                message: "a 'fun' literal needs a body: '= expr' or '{ ... }'".to_string(),
                span: self.peek().span,
            });
        };
        Ok(Expr::FnLit {
            params,
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
    fn parse_param_mode(&mut self) -> PResult<ParamMode> {
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
                message: "'mut' and 'move' cannot be combined on one parameter — 'mut T' is an exclusive borrow, 'move T' takes ownership".to_string(),
                span: self.peek().span,
            });
        }
        Ok(mode)
    }

    /// The rest of `fun(i32, String).R: bool`, with `fun` already consumed.
    /// A parameter of a function type takes the same `mut`/`move` qualifiers
    /// a declaration's parameter takes, because the two have to agree: a
    /// written `fun(String)` is a function whose parameter is a borrow, and
    /// `fun(move String)` is one that takes ownership, exactly as
    /// `fun f(s: String)` and `fun f(s: move String)` do.
    fn parse_fn_type(
        &mut self,
        start: Span,
        amp_start: Option<Span>,
        name: &str,
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
        let ret = if self.check(&TokenKind::Dot) {
            self.advance();
            let (marker, mspan) = self.expect_ident()?;
            if marker != "R" {
                return Err(ParseError {
                    message: format!("expected 'R' after '.', found '{marker}'"),
                    span: mspan,
                });
            }
            self.expect(TokenKind::Colon, "':'")?;
            let ty = self.parse_type()?;
            span = span.to(ty.span);
            Some(Box::new(ty))
        } else {
            None
        };
        Ok(TypeExpr {
            name: name.to_string(),
            tuple: None,
            args: Vec::new(),
            array_elem: None,
            array_len: None,
            amp: amp_start.is_some(),
            fn_params: Some(params),
            fn_ret: ret,
            span: amp_start.map(|a| a.to(span)).unwrap_or(span),
        })
    }

    /// `"<" type ("," type)* ">"`, with the `<` not yet consumed. Returns the
    /// arguments and the closing `>`'s span, which the expression-level
    /// speculative reader needs in order to check adjacency.
    fn parse_type_arg_list(&mut self) -> PResult<(Vec<TypeExpr>, Span)> {
        self.expect(TokenKind::Lt, "'<'")?;
        let mut args = Vec::new();
        loop {
            args.push(self.parse_type()?);
            if self.check(&TokenKind::Comma) {
                self.advance();
                continue;
            }
            break;
        }
        let close = self.expect(TokenKind::Gt, "'>' to close the type argument list")?;
        Ok((args, close.span))
    }

    /// `<T, U>` in an *expression*, immediately followed by `(` — the explicit
    /// instantiation of a generic construction or call, `Pair<i32, String>(...)`
    /// or `Option.Some<i32>(v = 1)` (A-2).
    ///
    /// This is the one place the spelling is ambiguous: `a < b > (c)` is a
    /// valid comparison chain, and `a < b, c > (d)` is not (a comma cannot
    /// appear in an expression there), so a lookahead that simply tries is not
    /// safe. The rule is adjacency — the `<` must touch the name and the `(`
    /// must touch the `>` — so comparison code, which is written with spaces,
    /// can never be misread. Returns `None` and restores the token position
    /// when the form does not apply.
    fn try_explicit_type_args(&mut self) -> PResult<Option<Vec<TypeExpr>>> {
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
            TokenKind::For => self.parse_for_stmt(),
            TokenKind::Return => self.parse_return_stmt(),
            // `pub` leads a declaration, and a declaration is not a statement,
            // so the message names the forms it can lead (A-13) rather than
            // reading the keyword as a start-of-expression.
            TokenKind::Pub => Err(ParseError {
                message: "'pub' applies to a top-level 'fun', 'extern fun', 'datac', 'enum' or 'const', and only at the top level of a file".to_string(),
                span: self.peek().span,
            }),
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
        // `val (lo, hi) = min_max(xs)`: a destructuring binding takes one
        // tuple value apart (`plan/syntax.md` 4). The pattern is a comma-
        // separated list in parentheses, so the one thing that tells it from
        // the ordinary `val name = ...` is the `(` right after the keyword.
        if self.check(&TokenKind::LParen) {
            let pattern = self.parse_binding_pattern()?;
            if self.check(&TokenKind::Colon) {
                return Err(ParseError {
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
    /// its value apart with (`plan/syntax.md` 4). A position is a name, `_` to
    /// ignore it, or a nested pattern, and there are two or more of them:
    /// `(x)` is not a pattern but the parenthesized name `x`, exactly as `(T)`
    /// is not a tuple type.
    fn parse_binding_pattern(&mut self) -> PResult<BindingPattern> {
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
                message: "a destructuring pattern needs two or more positions ('val (lo, hi) = ...'); a single name is an ordinary 'val', and '_' ignores a position rather than binding one"
                    .to_string(),
                span: open.to(close),
            });
        }
        Ok(BindingPattern::Tuple(positions))
    }

    fn parse_while_stmt(&mut self) -> PResult<Stmt> {
        let start = self.expect(TokenKind::While, "'while'")?.span;
        let cond = self.parse_expr()?;
        let body = self.parse_block()?;
        let span = start.to(body.span);
        Ok(Stmt::While { cond, body, span })
    }

    /// `for x in xs { BODY }`, desugared here into an indexed walk over the
    /// `List` — the same trick the boolean `when` and `if let` already use, so
    /// nothing downstream learns a new form (A-5, `plan/syntax.md` §6.2):
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
    /// a parameter *is* a borrow — `for x in xs` inside a function would move
    /// out of a borrow, which is the common case.
    ///
    /// The element binding is exactly what `List.get` gives: a copy for a
    /// `Copy` element, a shared borrow for an owned one. It lives in a block
    /// of its own so the borrow ends before the increment, and so `BODY`'s own
    /// tail expression stays the tail of a block.
    fn parse_for_stmt(&mut self) -> PResult<Stmt> {
        let start = self.expect(TokenKind::For, "'for'")?.span;
        // The loop binds one name, and a destructuring pattern is not one: a
        // `val`/`var` takes a tuple apart (4), and a loop body that wants the
        // positions writes that binding itself. Saying so beats the
        // "expected identifier" a `(` would get.
        if self.check(&TokenKind::LParen) {
            return Err(ParseError {
                message: "a 'for' binding is one name ('for x in xs'), not a destructuring pattern; bind the element and take it apart in the body ('val (a, b) = x')"
                    .to_string(),
                span: self.peek().span,
            });
        }
        let (binding, binding_span) = self.expect_ident()?;
        // `in` is an ordinary identifier (2.3): it is the separator only in
        // this position, the way `let` is a pattern only after `if` (6.1).
        let (word, word_span) = self.expect_ident()?;
        if word != "in" {
            return Err(ParseError {
                message: format!(
                    "expected 'in' between the 'for' binding and what it iterates, found '{word}'"
                ),
                span: word_span,
            });
        }
        let iterable = self.parse_expr()?;
        let body = self.parse_block()?;
        let span = start.to(body.span);

        self.fors += 1;
        let seq_name = format!("{}{}", FOR_PREFIX, self.fors - 1);
        let idx_name = format!("{seq_name}_i");
        // Every synthesized node is spelled with the `for`'s own span: a
        // diagnostic about a desugared `.len()`/`.get()` should point at the
        // loop, not at nothing.
        let idx_span = word_span;

        let mut stmts = Vec::new();
        let iterable = match temporary_root(&iterable).cloned() {
            // A place is re-read as written on every iteration. It costs
            // nothing, and binding one by value would *move* it: a parameter
            // is a borrow, so the common `for x in xs` inside a function would
            // move out of a borrow.
            None => iterable,
            // Anything else is evaluated once and bound first, so the loop
            // walks the sequence the `for` line saw. A field chain over a
            // temporary stages its *root*: `for x in mk().items` binds `mk()`
            // once instead of calling it again for the condition and for every
            // element.
            Some(root) => {
                let init_span = root.span();
                stmts.push(Stmt::Let {
                    name: seq_name.clone(),
                    mutable: false,
                    ty: None,
                    init: root,
                    span: init_span,
                });
                rebase_root(&iterable, &Expr::Ident(seq_name.clone(), init_span))
            }
        };
        let seq_span = iterable.span();

        let cond = Expr::Binary {
            op: BinOp::Lt,
            lhs: Box::new(Expr::Ident(idx_name.clone(), idx_span)),
            rhs: Box::new(Expr::MethodCall {
                receiver: Box::new(iterable.clone()),
                method: "len".to_string(),
                type_args: Vec::new(),
                args: Vec::new(),
                span: seq_span,
            }),
            span,
        };
        let element = Stmt::Let {
            name: binding,
            mutable: false,
            ty: None,
            init: Expr::MethodCall {
                receiver: Box::new(iterable),
                method: "get".to_string(),
                type_args: Vec::new(),
                args: vec![CallArg {
                    name: None,
                    value: Expr::Ident(idx_name.clone(), idx_span),
                }],
                span: binding_span,
            },
            span: binding_span,
        };
        let mut element_block_stmts = vec![element];
        element_block_stmts.extend(body.stmts);

        let increment = Stmt::Expr {
            expr: Expr::Assign {
                target: Box::new(Expr::Ident(idx_name.clone(), idx_span)),
                value: Box::new(Expr::Binary {
                    op: BinOp::Add,
                    lhs: Box::new(Expr::Ident(idx_name.clone(), idx_span)),
                    rhs: Box::new(Expr::IntLit(1, idx_span)),
                    span: idx_span,
                }),
                span: idx_span,
            },
            span: idx_span,
        };

        stmts.push(Stmt::Let {
            name: idx_name,
            mutable: true,
            ty: Some(TypeExpr::named("usize", idx_span)),
            init: Expr::IntLit(0, idx_span),
            span: idx_span,
        });
        stmts.push(Stmt::While {
            cond,
            body: Block {
                stmts: vec![
                    Stmt::Expr {
                        expr: Expr::Block(Block {
                            stmts: element_block_stmts,
                            span: body.span,
                        }),
                        span: body.span,
                    },
                    increment,
                ],
                span: body.span,
            },
            span,
        });

        Ok(Stmt::Expr {
            expr: Expr::Block(Block { stmts, span }),
            span,
        })
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
    fn shift_op(&self) -> Option<BinOp> {
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

    fn parse_binary(&mut self, min_bp: u8) -> PResult<Expr> {
        let mut lhs = self.parse_unary()?;
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
            if bp < min_bp {
                break;
            }
            for _ in 0..width {
                self.advance();
            }
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

    fn parse_postfix(&mut self) -> PResult<Expr> {
        let mut expr = self.parse_primary()?;
        loop {
            // `expr?` — the propagation operator (MK5-S6-M1). It is postfix,
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
                let (name, nspan) = self.expect_ident()?;
                // `Option.Some<i32>(v = 1)` — an explicit instantiation on a
                // generic `enum`'s variant (A-2). Recognized only in the
                // adjacent form, so `a.b < c > (d)` stays a comparison.
                let type_args = self.try_explicit_type_args()?.unwrap_or_default();
                if self.check(&TokenKind::LParen) {
                    let (args, end) = self.parse_call_args()?;
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
                let type_args = self.try_explicit_type_args()?.unwrap_or_default();
                if self.check(&TokenKind::LParen) {
                    let (args, end) = self.parse_call_args()?;
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
                // a tuple has two or more elements (`plan/syntax.md` 3).
                self.advance();
                if self.check(&TokenKind::RParen) {
                    return Err(ParseError {
                        message: "'()' is not a value: a tuple has two or more elements, and the absent value is 'unit', which no expression writes"
                            .to_string(),
                        span: tok.span,
                    });
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
                message: format!("unexpected token {other:?}"),
                span: tok.span,
            }),
        }
    }

    /// `[a, b, c]` and `[x; N]`: the two array literals (A-18). The list form
    /// fixes the length by how many elements were written; the repeat form
    /// takes it from a compile-time constant, which is what makes a zeroed
    /// buffer (`[0; 4096]`) one expression rather than 4096 of them.
    ///
    /// `[]` is refused here rather than later: with no elements there is no
    /// element type to infer and no length, so there is nothing a later pass
    /// could do with it.
    fn parse_array_literal(&mut self, start: Span) -> PResult<Expr> {
        self.expect(TokenKind::LBracket, "'['")?;
        if self.check(&TokenKind::RBracket) {
            return Err(ParseError {
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
    fn parse_template(&mut self) -> PResult<Expr> {
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
                        message: format!(
                            "malformed string template: expected literal text or '${{expr}}', found {other:?}"
                        ),
                        span: self.peek().span,
                    })
                }
            }
        }
    }

    fn parse_if(&mut self) -> PResult<Expr> {
        let start = self.expect(TokenKind::If, "'if'")?.span;
        self.parse_if_link(start)
    }

    /// `elif cond { ... }`: the `else if` link written as one keyword
    /// (`plan/syntax.md` 6.1). The token is consumed and the rest of the link
    /// goes through `parse_if_link`, exactly as `else if` does, so the two
    /// spellings cannot mean two different things.
    fn parse_elif(&mut self) -> PResult<Expr> {
        let start = self.expect(TokenKind::Elif, "'elif'")?.span;
        self.parse_if_link(start)
    }

    /// One link of an `if` chain: the optional `let` pattern, the condition,
    /// the block, and whatever follows (nothing, `else`, `else if`, or
    /// `elif`). Shared by `if` and `elif`, which is why a chain may mix them.
    fn parse_if_link(&mut self, start: Span) -> PResult<Expr> {
        // `if let PATTERN = EXPR { ... }` — a pattern condition rather than a
        // boolean one. `let` is only an identifier everywhere else in the
        // language, so this is recognized by the token pair rather than by
        // reserving the word; the one thing it costs is that a binding
        // *named* `let` cannot be the condition of an `if` without
        // parentheses.
        if matches!(self.peek_kind(), TokenKind::Ident(word) if word == "let") {
            return self.parse_if_let(start);
        }
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
    /// Nothing downstream needs a new case — and binding the scrutinee first is
    /// what makes a temporary legal here, since the pattern form of `when`
    /// requires a *place* (an owned payload binds as a borrow of it, and a
    /// borrow of a temporary would not outlive the arm).
    fn parse_if_let(&mut self, start: Span) -> PResult<Expr> {
        self.advance(); // `let`
        let (variant, vspan) = self.expect_ident()?;
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
        let pattern = VariantPattern {
            variant,
            binds,
            span: vspan,
        };
        self.expect(TokenKind::Eq, "'=' after the pattern")?;
        let scrutinee = self.parse_expr()?;
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
            // `elif let Foo(x) = e { ... }` — the chained pattern link, in the
            // spelling 6.1 prefers. Same link, same function.
            let link = self.parse_elif()?;
            span = span.to(link.span());
            link
        } else {
            // No `else`, so the whole thing produces `Unit` — exactly what an
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
            },
            span,
        };
        // The `when` is the block's last statement, which is what makes it the
        // block's value (statements are positional in this language — there is
        // no `tail` field to set).
        //
        // A scrutinee that is already a *place* (`if let Ok(v) = r`) is used
        // directly, with no synthesized binding at all — exactly what a bare
        // `when r { ... }` already does (6.1's own note: "a bare `when` needs
        // a place, since a payload borrow has to outlive the arm", and
        // matching a place never moves it, only a field of it). This is what
        // makes an `else if let PAT2 = r { ... }` chained after `if let PAT1
        // = r { ... }` legal: each link only *reads* `r` to test its tag, so
        // reading it twice is fine, the same way `when`'s own multi-arm form
        // reads its one scrutinee once and tests it repeatedly. Binding it to
        // a fresh `val #iflet0 = r` first — every earlier version of this
        // desugaring did that unconditionally — makes the *first* `if let`
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
    /// - `when { cond -> value, ..., else -> value }` — no scrutinee, so it
    ///   is desugared here and now to a nested `if`/`else` chain; `infer.rs`
    ///   and `codegen_c.rs` never see this form at all.
    /// - `when expr { Variant(bind) -> value, ..., else -> value }` — a
    ///   pattern match (A-1), kept as `Expr::When` because only `infer.rs`
    ///   knows the scrutinee's type, and therefore which variants exist.
    ///
    /// The two never collide: the boolean form has no scrutinee.
    fn parse_when(&mut self) -> PResult<Expr> {
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
            self.expect(TokenKind::Arrow, "'->'")?;
            let body = self.parse_expr()?;
            arms.push((cond, body));

            // A newline separates arms, but it is not *required* to: inside
            // `(` `)` the lexer does not emit newline tokens at all (see
            // `Lexer::paren_depth`), and a `when` is allowed to be a call
            // argument. An arm head is unambiguous either way — every arm is
            // `head -> value`, and `->` appears nowhere else in the language.
            if !self.check(&TokenKind::RBrace) {
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

    /// `when s { Circle(r) -> ... else -> ... }` — the pattern form. Arm
    /// heads are *shapes*, not expressions: a variant name with an optional
    /// `(bind, ...)` list, where each bind is an identifier or `_`.
    fn parse_when_patterns(&mut self, start: Span) -> PResult<Expr> {
        let scrutinee = self.parse_expr()?;
        self.expect(TokenKind::LBrace, "'{'")?;
        self.skip_newlines();

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
            self.expect(TokenKind::Arrow, "'->'")?;
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
            // argument. An arm head is unambiguous either way — every arm is
            // `head -> value`, and `->` appears nowhere else in the language.
            if !self.check(&TokenKind::RBrace) {
                self.skip_newlines();
            }
        }
        let end = self.expect(TokenKind::RBrace, "'}'")?.span;
        if arms.is_empty() {
            return Err(ParseError {
                message: "'when' needs at least one arm".to_string(),
                span: start.to(end),
            });
        }
        Ok(Expr::When {
            scrutinee: Box::new(scrutinee),
            arms,
            span: start.to(end),
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
    fn a_tuple_type_is_its_element_list() {
        // MISSING-z2bB9: `(A, B)` is a type, and `(A)` is not a tuple but the
        // parenthesized type `A` (`plan/syntax.md` 3).
        let p = parse("fun f(p: (i32, String)).R: (i32, i32) = (1, 2)");
        let param = &p.functions[0].params[0].ty;
        assert_eq!(param.display(), "(i32, String)");
        assert_eq!(param.tuple.as_ref().unwrap().len(), 2);
        let ret = p.functions[0].return_type.as_ref().unwrap();
        assert_eq!(ret.display(), "(i32, i32)");

        let paren = parse("fun f().R: (i32) = 1");
        let ret = paren.functions[0].return_type.as_ref().unwrap();
        assert!(ret.tuple.is_none(), "(T) is the parenthesized type T");
        assert_eq!(ret.display(), "i32");

        let nested = parse("fun f(p: ((i32, i32), i32)) {}");
        assert_eq!(
            nested.functions[0].params[0].ty.display(),
            "((i32, i32), i32)"
        );
    }

    #[test]
    fn an_empty_tuple_type_is_refused_by_name() {
        let err = parse_err("fun f(p: ()) {}");
        assert!(err.message.contains("'()' is not a type"), "{}", err.message);
    }

    #[test]
    fn a_tuple_literal_needs_two_elements() {
        // `(e)` is the parenthesized expression it always was, and `()` has no
        // value at all.
        let p = parse("fun f().R: i32 {\n    val a = (1)\n    return a\n}");
        match &p.functions[0].body {
            FunctionBody::Block(b) => match &b.stmts[0] {
                Stmt::Let { init, .. } => {
                    assert!(matches!(init, Expr::IntLit(1, _)), "got {init:?}");
                }
                other => panic!("expected a let, got {other:?}"),
            },
            other => panic!("expected a block body, got {other:?}"),
        }

        let p = parse("fun f() {\n    val p = (1, 2)\n}");
        match &p.functions[0].body {
            FunctionBody::Block(b) => match &b.stmts[0] {
                Stmt::Let { init, .. } => match init {
                    Expr::TupleLit { elems, .. } => assert_eq!(elems.len(), 2),
                    other => panic!("expected a tuple literal, got {other:?}"),
                },
                other => panic!("expected a let, got {other:?}"),
            },
            other => panic!("expected a block body, got {other:?}"),
        }

        let err = parse_err("fun f() {\n    val p = ()\n}");
        assert!(err.message.contains("'()' is not a value"), "{}", err.message);
    }

    #[test]
    fn a_destructuring_binding_binds_every_position() {
        // MISSING-z2bB9: `val (lo, _) = p`, with `_` ignoring a position and a
        // nested pattern allowed (`plan/syntax.md` 4).
        let p = parse("fun f(p: (i32, i32)) {\n    val (lo, _) = p\n    var ((a, b), c) = ((1, 2), 3)\n}");
        let stmts = match &p.functions[0].body {
            FunctionBody::Block(b) => &b.stmts,
            other => panic!("expected a block body, got {other:?}"),
        };
        assert_eq!(stmts.len(), 2);
        let Stmt::LetTuple {
            pattern, mutable, ..
        } = &stmts[0]
        else {
            panic!("expected a destructuring binding, got {:?}", stmts[0]);
        };
        assert!(!*mutable);
        let BindingPattern::Tuple(positions) = pattern else {
            panic!("expected a tuple pattern, got {pattern:?}");
        };
        assert!(matches!(positions[0], BindingPattern::Name(ref n) if n == "lo"));
        assert!(matches!(positions[1], BindingPattern::Ignore));

        let Stmt::LetTuple {
            pattern, mutable, ..
        } = &stmts[1]
        else {
            panic!("expected a destructuring binding, got {:?}", stmts[1]);
        };
        assert!(*mutable, "`var (a, b) = ...` makes every position a `var`");
        let mut names = Vec::new();
        pattern.binds(&mut Vec::new(), &mut names);
        assert_eq!(
            names.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
            vec!["a", "b", "c"]
        );
        assert_eq!(names[1].1, vec![0, 1], "`b` is element 2 of element 1");
    }

    #[test]
    fn a_destructuring_binding_needs_two_positions_and_no_written_type() {
        let err = parse_err("fun f() {\n    val (a) = 1\n}");
        assert!(
            err.message.contains("needs two or more positions"),
            "{}",
            err.message
        );
        let err = parse_err("fun f() {\n    val (a, b): (i32, i32) = (1, 2)\n}");
        assert!(
            err.message.contains("takes no written type"),
            "{}",
            err.message
        );
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

    /// The number of links in an `if` chain: one per `if`/`elif`, plus one for
    /// a trailing `else` block. Both spellings have to give the same number,
    /// which is the whole claim `elif` makes (`plan/syntax.md` 6.1).
    fn links(branch: &ElseBranch) -> usize {
        match branch {
            ElseBranch::If(e) => match e {
                Expr::If { else_block, .. } => {
                    1 + else_block.as_deref().map(links).unwrap_or(0)
                }
                other => panic!("expected an if, got {other:?}"),
            },
            ElseBranch::Block(_) => 1,
        }
    }

    /// The chain of the first statement of the first function in `src`.
    fn chain(src: &str) -> usize {
        let p = parse(src);
        match &p.functions[0].body {
            FunctionBody::Block(b) => match &b.stmts[0] {
                Stmt::Expr { expr, .. } => match expr {
                    Expr::If { else_block, .. } => {
                        1 + else_block.as_deref().map(links).unwrap_or(0)
                    }
                    other => panic!("expected an if, got {other:?}"),
                },
                other => panic!("expected an expression statement, got {other:?}"),
            },
            other => panic!("expected a block body, got {other:?}"),
        }
    }

    #[test]
    fn elif_is_the_same_link_as_else_if() {
        let spelled_out = "fun f(n: i32).R: i32 {\n    if n > 2 { return 1 } else if n > 1 { return 2 } else { return 3 }\n}";
        let spelled_elif = "fun f(n: i32).R: i32 {\n    if n > 2 { return 1 } elif n > 1 { return 2 } else { return 3 }\n}";
        assert_eq!(chain(spelled_out), 3);
        assert_eq!(chain(spelled_elif), chain(spelled_out));
    }

    #[test]
    fn an_if_chain_may_mix_both_spellings() {
        let mixed = "fun f(n: i32).R: i32 {\n    if n > 3 { return 1 } else if n > 2 { return 2 } elif n > 1 { return 3 } else { return 4 }\n}";
        assert_eq!(chain(mixed), 4);
    }

    #[test]
    fn elif_needs_no_else_after_it() {
        let src = "fun f(n: i32).R: i32 {\n    if n > 1 { return 1 } elif n > 0 { return 2 }\n}";
        assert_eq!(chain(src), 2);
    }

    #[test]
    fn elif_let_is_the_chained_pattern_link() {
        // `elif let PAT = e { ... }` is `else if let PAT = e { ... }` in the
        // preferred spelling, so it goes through the same desugaring: a block
        // whose `when` carries the link.
        let after_a_pattern = "enum Answer {\n    Yes(val n: i32)\n    No\n}\n\nfun f(m: Answer).R: i32 {\n    if let Yes(n) = m {\n        return n\n    } elif let No = m {\n        return 0\n    }\n    return 1\n}";
        assert_eq!(parse(after_a_pattern).functions.len(), 1);
        // And a chain may leave the pattern form for a boolean one, either
        // way round.
        let leaving_the_pattern = "enum Answer {\n    Yes(val n: i32)\n    No\n}\n\nfun f(m: Answer).R: i32 {\n    if let Yes(n) = m {\n        return n\n    } elif n > 1 {\n        return 2\n    }\n    return 1\n}";
        assert_eq!(parse(leaving_the_pattern).functions.len(), 1);
    }

    #[test]
    fn an_elif_that_follows_nothing_is_an_error() {
        parse_err("fun f(n: i32) {\n    elif n > 1 { }\n}");
    }

    #[test]
    fn elsif_is_not_a_spelling() {
        parse_err("fun f(n: i32) {\n    if n > 1 { } elsif n > 0 { }\n}");
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
    fn parses_an_enum_and_a_pattern_when() {
        let p = parse(
            "enum Shape {\n    Circle(val r: f64)\n    Rect(val w: f64, val h: f64)\n    Empty\n}\n\ndatac Point(val x: i32)\n\nfun area(s: &Shape).R: f64 = when s {\n    Circle(r) -> r\n    Rect(w, h) -> w * h\n    Empty -> 0.0\n}",
        );
        assert_eq!(p.enums.len(), 1);
        let e = &p.enums[0];
        assert_eq!(e.name, "Shape");
        assert_eq!(e.variants.len(), 3);
        assert_eq!(e.variants[0].fields.len(), 1);
        assert_eq!(e.variants[0].fields[0].name, "r");
        assert_eq!(e.variants[1].fields.len(), 2);
        assert!(e.variants[2].fields.is_empty());
        // `datac` and `enum` share one declaration order, which is what
        // `resolve.rs` merges and codegen emits in.
        assert_eq!(e.order, 0);
        assert_eq!(p.datacs[0].order, 1);

        match &p.functions[0].body {
            FunctionBody::Expr(Expr::When { arms, .. }) => {
                assert_eq!(arms.len(), 3);
                let first = arms[0].pattern.as_ref().unwrap();
                assert_eq!(first.variant, "Circle");
                assert_eq!(first.binds, vec![Some("r".to_string())]);
                assert!(arms[2].pattern.as_ref().unwrap().binds.is_empty());
                assert!(matches!(arms[2].body, Expr::FloatLit(..)));
            }
            other => panic!("expected a pattern `when`, got {other:?}"),
        }
    }

    #[test]
    fn an_else_arm_in_a_pattern_when_parses_as_a_catch_all() {
        let p = parse(
            "enum Flag {\n    On\n    Off\n}\n\nfun f(x: &Flag).R: i32 = when x {\n    On -> 1\n    else -> 2\n}",
        );
        match &p.functions[0].body {
            FunctionBody::Expr(Expr::When { arms, .. }) => {
                assert!(arms[0].pattern.is_some());
                assert!(arms[1].pattern.is_none());
            }
            other => panic!("expected a pattern `when`, got {other:?}"),
        }
    }

    #[test]
    fn if_let_desugars_to_a_bound_scrutinee_and_a_single_arm_when() {
        // MK5-S6-M1: `if let P = e { A } else { B }` becomes
        // `{ val #iflet0 = e  when #iflet0 { P -> { A } else -> { B } } }`.
        let p = parse(
            "fun main() {\n    if let Circle(r) = Shape.Circle(r = 1.0) {\n        print(r)\n    } else {\n        print(0.0)\n    }\n}",
        );
        let stmts = match &p.functions[0].body {
            FunctionBody::Block(b) => &b.stmts,
            _ => panic!("expected block body"),
        };
        let inner = match &stmts[0] {
            Stmt::Expr { expr: Expr::Block(b), .. } => b,
            other => panic!("expected a desugared block, got {other:?}"),
        };
        match &inner.stmts[0] {
            Stmt::Let { name, init, .. } => {
                assert_eq!(name, "#iflet0");
                assert!(matches!(init, Expr::MethodCall { .. }));
            }
            other => panic!("expected the scrutinee binding, got {other:?}"),
        }
        match &inner.stmts[1] {
            Stmt::Expr { expr: Expr::When { scrutinee, arms, .. }, .. } => {
                assert!(matches!(**scrutinee, Expr::Ident(ref n, _) if n == "#iflet0"));
                assert_eq!(arms.len(), 2);
                assert!(arms[0].pattern.is_some());
                assert!(arms[1].pattern.is_none());
            }
            other => panic!("expected the `when`, got {other:?}"),
        }
    }

    #[test]
    fn for_loop_desugars_to_a_synthesized_index_and_a_while() {
        // A-5: `for x in xs { BODY }` becomes
        // `{ var #for0_i: usize = 0  while #for0_i < xs.len() {
        //      { val x = xs.get(#for0_i) BODY }  #for0_i = #for0_i + 1 } }`.
        // A place iterable is used as written, so there is no `#for0` binding.
        let p = parse(
            "fun main() {\n    for x in xs {\n        print(x)\n    }\n}",
        );
        let stmts = match &p.functions[0].body {
            FunctionBody::Block(b) => &b.stmts,
            _ => panic!("expected block body"),
        };
        let outer = match &stmts[0] {
            Stmt::Expr { expr: Expr::Block(b), .. } => b,
            other => panic!("expected a desugared block, got {other:?}"),
        };
        assert_eq!(outer.stmts.len(), 2);
        match &outer.stmts[0] {
            Stmt::Let { name, mutable, ty, .. } => {
                assert_eq!(name, "#for0_i");
                assert!(*mutable);
                assert_eq!(ty.as_ref().map(|t| t.name.as_str()), Some("usize"));
            }
            other => panic!("expected the index binding, got {other:?}"),
        }
        let while_body = match &outer.stmts[1] {
            Stmt::While { cond, body, .. } => {
                let lhs = match cond {
                    Expr::Binary { op: BinOp::Lt, lhs, .. } => lhs,
                    other => panic!("expected an index comparison, got {other:?}"),
                };
                assert!(matches!(**lhs, Expr::Ident(ref n, _) if n == "#for0_i"));
                body
            }
            other => panic!("expected the while, got {other:?}"),
        };
        assert_eq!(while_body.stmts.len(), 2);
        let inner = match &while_body.stmts[0] {
            Stmt::Expr { expr: Expr::Block(b), .. } => b,
            other => panic!("expected the element block, got {other:?}"),
        };
        match &inner.stmts[0] {
            Stmt::Let { name, init, .. } => {
                assert_eq!(name, "x");
                match init {
                    Expr::MethodCall { method, args, .. } if method == "get" => {
                        assert!(matches!(
                            args[0].value,
                            Expr::Ident(ref n, _) if n == "#for0_i"
                        ));
                    }
                    other => panic!("expected the element read, got {other:?}"),
                }
            }
            other => panic!("expected the element binding, got {other:?}"),
        }
        // The body's statement survives, and the increment is last.
        assert!(matches!(&inner.stmts[1], Stmt::Expr { .. }));
        assert!(matches!(&while_body.stmts[1], Stmt::Expr { expr: Expr::Assign { .. }, .. }));
    }

    #[test]
    fn a_non_place_iterable_is_bound_to_a_temporary_first() {
        // A call result is evaluated once, into `#for0`, which the desugared
        // block owns and frees — the reason `for x in make()` is not a call
        // per iteration.
        let p = parse("fun main() {\n    for x in make() {\n        print(x)\n    }\n}");
        let stmts = match &p.functions[0].body {
            FunctionBody::Block(b) => &b.stmts,
            _ => panic!("expected block body"),
        };
        let outer = match &stmts[0] {
            Stmt::Expr { expr: Expr::Block(b), .. } => b,
            other => panic!("expected a desugared block, got {other:?}"),
        };
        assert_eq!(outer.stmts.len(), 3);
        match &outer.stmts[0] {
            Stmt::Let { name, init, .. } => {
                assert_eq!(name, "#for0");
                assert!(matches!(init, Expr::Call { .. }));
            }
            other => panic!("expected the sequence binding, got {other:?}"),
        }
        match &outer.stmts[2] {
            Stmt::While { cond, .. } => match cond {
                Expr::Binary { rhs, .. } => match &**rhs {
                    Expr::MethodCall { receiver, method, .. } => {
                        assert_eq!(method, "len");
                        assert!(matches!(**receiver, Expr::Ident(ref n, _) if n == "#for0"));
                    }
                    other => panic!("expected the length call, got {other:?}"),
                },
                other => panic!("expected an index comparison, got {other:?}"),
            },
            other => panic!("expected the while, got {other:?}"),
        }
    }

    #[test]
    fn parses_datac_decl_with_val_and_var_fields() {
        let p = parse("datac Point(val x: i32, var y: i32)\nfun main() {}");
        assert_eq!(p.datacs.len(), 1);
        let d = &p.datacs[0];
        assert_eq!(d.name, "Point");
        assert!(d.type_params.is_empty());
        assert_eq!(d.fields.len(), 2);
        assert!(!d.fields[0].mutable);
        assert!(d.fields[1].mutable);
    }

    #[test]
    fn parses_type_parameters_on_a_datac_and_an_enum() {
        // A-2: the parameter list sits between the declared name and the
        // declaration's own `(` or `{`, and a use site writes the arguments.
        let p = parse("datac Pair<A, B>(val first: A, val second: B)\nfun main() {}");
        assert_eq!(p.datacs[0].type_params, vec!["A".to_string(), "B".to_string()]);

        let p = parse("enum Option<T> {\n    Some(val value: T)\n    None\n}\nfun main() {}");
        assert_eq!(p.enums[0].type_params, vec!["T".to_string()]);

        let p = parse("fun f(p: Pair<i32, String>).R: Option<i32> = 1\nfun main() {}");
        let ty = &p.functions[0].params[0].ty;
        assert_eq!(ty.name, "Pair");
        assert_eq!(ty.args.len(), 2);
        assert_eq!(ty.args[0].name, "i32");
        assert_eq!(ty.args[1].name, "String");
        assert_eq!(p.functions[0].return_type.as_ref().unwrap().args.len(), 1);
    }

    #[test]
    fn generic_declaration_errors_are_parse_errors() {
        parse_err("datac Pair<T, T>(val a: T, val b: T)\nfun main() {}");
        parse_err("datac Pair<>(val a: i32)\nfun main() {}");
    }

    #[test]
    fn parses_type_parameters_on_a_function() {
        // A-2: a free function declares its own parameters after its name, and
        // a receiver method's come from the receiver's argument list.
        let p = parse("fun first<T>(xs: List<T>).R: T = xs.get(i = 0)\nfun main() {}");
        assert_eq!(p.functions[0].type_params, vec!["T".to_string()]);
        assert!(p.functions[0].receiver.is_none());

        let p = parse("fun Pair<A, B>.first().R: A = a\nfun main() {}");
        let f = &p.functions[0];
        assert!(f.type_params.is_empty());
        let r = f.receiver.as_ref().expect("a receiver");
        assert_eq!(r.type_name, "Pair");
        assert_eq!(r.type_args, vec!["A".to_string(), "B".to_string()]);
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
    fn explicit_type_arguments_are_recognized_only_when_adjacent() {
        // A-2: `Pair<i32, String>(...)` — the `<` touches the name and the
        // `(` touches the `>`, which is what keeps an ordinary comparison
        // chain (`a < b > (c)`) from being read as an instantiation.
        let p = parse("fun main() {\n    val x = Pair<i32, String>(a = 1)\n}");
        let stmts = match &p.functions[0].body {
            FunctionBody::Block(b) => &b.stmts,
            _ => panic!("expected block body"),
        };
        match &stmts[0] {
            Stmt::Let { init: Expr::Call { type_args, .. }, .. } => {
                assert_eq!(type_args.len(), 2);
                assert_eq!(type_args[0].name, "i32");
                assert_eq!(type_args[1].name, "String");
            }
            other => panic!("expected a call with type arguments, got {other:?}"),
        }

        let p = parse("fun main() {\n    val y = a < b > (d)\n}");
        let stmts = match &p.functions[0].body {
            FunctionBody::Block(b) => &b.stmts,
            _ => panic!("expected block body"),
        };
        match &stmts[0] {
            Stmt::Let { init: Expr::Binary { op: BinOp::Gt, .. }, .. } => {}
            other => panic!("expected a comparison, got {other:?}"),
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
    fn parses_a_move_parameter() {
        let p = parse("fun consume(s: move String).R: usize = s.len()");
        let param = &p.functions[0].params[0];
        assert!(param.mode.moves());
        assert!(!param.mode.mutable());
        assert_eq!(param.ty.name, "String");
    }

    #[test]
    fn parses_a_function_type_in_parameter_and_return_position() {
        let p = parse("fun apply(f: fun(i32).R: i32, x: i32).R: fun(move String) = f");
        let f = &p.functions[0];
        let param = &f.params[0];
        assert!(param.ty.is_fn());
        let params = param.ty.fn_params.as_ref().unwrap();
        assert_eq!(params.len(), 1);
        assert!(!params[0].mode.mutable());
        assert_eq!(params[0].ty.name, "i32");
        assert_eq!(param.ty.fn_ret.as_ref().unwrap().name, "i32");

        // The return type is a function type with no `.R`, so it returns
        // `Unit` — and its parameter is a `move`, which a function type
        // spells the same way a declaration does.
        let ret = f.return_type.as_ref().unwrap();
        assert!(ret.is_fn());
        assert!(ret.fn_ret.is_none());
        assert!(ret.fn_params.as_ref().unwrap()[0].mode.moves());
        assert_eq!(ret.display(), "fun(move String)");
    }

    #[test]
    fn parses_a_closure_type_in_parameter_and_return_position() {
        // A closure type is a function type under the `closure` keyword — the
        // same parameter list, the same optional `.R:` (A-4 phase E).
        let p = parse("fun use(f: closure(i32).R: i32, g: move closure()) {}");
        let f = &p.functions[0];

        let f_param = &f.params[0];
        assert!(f_param.ty.is_closure());
        assert_eq!(f_param.ty.display(), "closure(i32).R: i32");
        assert!(!f_param.mode.moves());

        // A bare closure parameter borrows the environment (an owned type's
        // parameter is a borrow); `move closure(...)` takes it over.
        let g_param = &f.params[1];
        assert!(g_param.ty.is_closure());
        assert!(g_param.mode.moves());
        assert_eq!(g_param.ty.display(), "closure()");
    }

    #[test]
    fn parses_a_fun_literal_in_expression_position() {
        let p = parse("fun main() {\n    val f = fun(x: move String).R: usize = x.len()\n}");
        let stmts = match &p.functions[0].body {
            FunctionBody::Block(b) => &b.stmts,
            _ => panic!("expected block body"),
        };
        match &stmts[0] {
            Stmt::Let {
                init: Expr::FnLit { params, return_type, .. },
                ..
            } => {
                assert_eq!(params.len(), 1);
                assert!(params[0].mode.moves());
                assert_eq!(return_type.as_ref().unwrap().name, "usize");
            }
            other => panic!("expected a fun literal, got {other:?}"),
        }
    }

    #[test]
    fn a_fun_literal_needs_a_body() {
        parse_err("fun main() {\n    val f = fun(x: i32).R: i32\n}");
    }

    #[test]
    fn rejects_move_combined_with_mut_or_a_receiver() {
        // One parameter cannot both borrow exclusively and take ownership.
        parse_err("fun f(s: mut move String) {}");
        // `move` is a parameter qualifier, never a receiver one.
        parse_err("fun move Counter.bump() {}");
    }

    #[test]
    fn rejects_malformed_input() {
        parse_err("fun main( {}");
    }

    #[test]
    fn parses_ref_deref_and_mut_param() {
        let p = parse("fun bump(c: mut Counter) {\n    val r = &c\n    val m = &mut c\n    val v = *r\n}");
        assert!(p.functions[0].params[0].mode.mutable());
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
        let p = parse("impl \"shapes/circle\"\nimpl \"std/time\" caps(time = [\"/7d\"])\n\nfun main() {}");
        assert_eq!(p.impls.len(), 2);
        assert_eq!(p.impls[0].path, "shapes/circle");
        assert_eq!(p.impls[1].path, "std/time");
        assert_eq!(p.impls[1].caps[0].domain, "time");
        assert_eq!(p.functions.len(), 1);
    }

    #[test]
    fn the_impl_as_clause_is_refused_by_name() {
        let err = parse_err("impl \"time\" as t\n\nfun main() {}");
        assert!(
            err.message.contains("is gone: reach the module by its own name"),
            "{}",
            err.message
        );
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
        assert!(e.message.contains("must come first"), "got: {}", e.message);
    }

    #[test]
    fn parses_a_caps_clause_with_several_grants() {
        let p = parse("impl \"util\" caps(fs = [\"/tmp/**\", \"/var/log/*\"], env = [\"HOME\"])");
        assert_eq!(p.impls.len(), 1);
        let caps = &p.impls[0].caps;
        assert_eq!(caps.len(), 2);
        assert_eq!(caps[0].domain, "fs");
        assert_eq!(caps[0].patterns, vec!["/tmp/**", "/var/log/*"]);
        assert_eq!(caps[1].domain, "env");
        assert_eq!(caps[1].patterns, vec!["HOME"]);
    }

    #[test]
    fn the_with_caps_spelling_is_refused_by_name() {
        // Removed, not outdated: the clause is the same one under the surviving
        // spelling, and the refusal names the keyword to delete.
        let err = parse_err("impl \"util\" with caps(fs = [\"/tmp/**\"])");
        assert!(
            err.message.contains("is gone: drop the `with`"),
            "{}",
            err.message
        );
        let p = parse("impl \"util\" caps(fs = [\"/tmp/**\"])");
        assert_eq!(p.impls[0].caps[0].domain, "fs");
    }

    #[test]
    fn a_caps_clause_requires_bracketed_string_patterns() {
        parse_err("impl \"util\" caps(fs = [tmp])");
    }

    #[test]
    fn an_impl_without_caps_has_no_grants() {
        let p = parse("impl \"std/fs\"");
        assert!(p.impls[0].caps.is_empty());
    }

    #[test]
    fn impl_requires_a_string_path() {
        parse_err("impl util");
    }

    #[test]
    fn parses_passdown_declarations() {
        let p = parse("pd val app_name = \"shapes\"\npd var count = 0\npd val label: &str = app_name\n\nfun main() {}");
        assert_eq!(p.pds.len(), 3);
        assert_eq!(p.pds[0].name, "app_name");
        assert!(!p.pds[0].mutable);
        assert!(p.pds[0].ty.is_none());
        assert!(p.pds[1].mutable);
        assert_eq!(p.pds[2].ty.as_ref().unwrap().name, "str");
        assert!(matches!(p.pds[2].init, Expr::Ident(..)));
    }

    #[test]
    fn pd_after_a_declaration_is_rejected() {
        let e = parse_err("fun main() {}\npd val x = 1");
        assert!(e.message.contains("must come before"), "got: {}", e.message);
    }

    #[test]
    fn pd_requires_val_or_var() {
        parse_err("pd x = 1");
    }

    #[test]
    fn parses_a_template_into_text_and_expression_parts() {
        let p = parse("fun main() {\n    print(\"a $b ${c.d()}\")\n}");
        let stmts = match &p.functions[0].body {
            FunctionBody::Block(b) => &b.stmts,
            _ => panic!("expected block body"),
        };
        let call = match &stmts[0] {
            Stmt::Expr { expr: Expr::Call { args, .. }, .. } => &args[0].value,
            other => panic!("expected a call, got {other:?}"),
        };
        let parts = match call {
            Expr::Template { parts, .. } => parts,
            other => panic!("expected a template, got {other:?}"),
        };
        assert_eq!(parts.len(), 4);
        assert!(matches!(&parts[0], TemplatePart::Text(t) if t == "a "));
        assert!(matches!(&parts[1], TemplatePart::Expr(Expr::Ident(n, _)) if n == "b"));
        assert!(matches!(&parts[2], TemplatePart::Text(t) if t == " "));
        assert!(matches!(&parts[3], TemplatePart::Expr(Expr::MethodCall { method, .. }) if method == "d"));
    }

    #[test]
    fn pub_is_read_on_the_five_item_forms_that_can_carry_it() {
        let p = parse(
            "pub const LIMIT: i32 = 1\npub datac Point(val x: i32)\npub enum Shape { Empty }\npub fun f().R: i32 = 1\npub extern fun g().R: i32",
        );
        assert!(p.consts[0].is_pub);
        assert!(p.datacs[0].is_pub);
        assert!(p.enums[0].is_pub);
        assert!(p.functions[0].is_pub);
        assert!(p.functions[1].is_pub);
        assert!(p.functions[1].extern_);
    }

    #[test]
    fn an_item_without_pub_is_private() {
        let p = parse("fun f().R: i32 = 1");
        assert!(!p.functions[0].is_pub);
    }

    #[test]
    fn pub_pd_is_refused_by_name() {
        let e = parse_err("pub pd val x = 1\nfun main() {}");
        assert!(e.message.contains("a 'pd' cannot be 'pub'"), "got: {}", e.message);
    }

    #[test]
    fn pub_on_a_receiver_method_names_the_type() {
        let e = parse_err("pub fun Circle.area().R: f64 = 1.0");
        assert!(
            e.message.contains("mark the type 'pub' instead"),
            "got: {}",
            e.message
        );
    }

    #[test]
    fn pub_on_anything_else_names_what_it_applies_to() {
        let e = parse_err("pub impl \"util\"");
        assert!(
            e.message.contains("'pub' applies to a top-level 'fun'"),
            "got: {}",
            e.message
        );
    }

    #[test]
    fn pub_inside_a_block_names_the_top_level_forms() {
        let e = parse_err("fun main() {\n    pub val x = 1\n}");
        assert!(
            e.message.contains("and only at the top level of a file"),
            "got: {}",
            e.message
        );
    }
}
