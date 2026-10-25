use super::*;

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Parser {
            tokens,
            pos: 0,
            if_lets: 0,
            for_patterns: 0,
            loop_stack: Vec::new(),
            label_flags: 0,
            when_temps: 0,
            no_trailing_block: false,
            depth: 0,
            datac_owner: None,
            bare_top_fn: false,
            pending_externs: Vec::new(),
        }
    }

    /// Refuses a left-leaning chain (`a < b < c ...`, `x.f.g.h ...`, `e???`)
    /// longer than `MAX_NESTING`: the loop that builds it does not recurse, but
    /// the tree it leaves is as deep as the chain is long.
    pub(crate) fn chain_limit(&self, links: usize) -> PResult<()> {
        if links > MAX_NESTING * 4 {
            return Err(ParseError {
                help: None,
                message: format!(
                    "this chain of operators or accesses is longer than {} links; split it into smaller pieces",
                    MAX_NESTING * 4
                ),
                span: self.peek().span,
            });
        }
        Ok(())
    }

    /// Enters one nesting level, or reports the depth limit. The caller undoes
    /// it with `self.depth -= 1` on every path out (the wrappers below do).
    pub(crate) fn nest(&mut self) -> PResult<()> {
        if self.depth >= MAX_NESTING {
            return Err(ParseError {
                help: None,
                message: format!(
                    "this nests more than {MAX_NESTING} levels deep (expressions, blocks, types and patterns inside one another); split it into smaller pieces"
                ),
                span: self.peek().span,
            });
        }
        self.depth += 1;
        Ok(())
    }

    /// Parses the expression in an `if`/`while`/`for`/`when` head, where a
    /// following `{` is the body and never a trailing block.
    pub(crate) fn parse_head_expr(&mut self) -> PResult<Expr> {
        let saved = std::mem::replace(&mut self.no_trailing_block, true);
        let e = self.parse_expr();
        self.no_trailing_block = saved;
        e
    }

    /// A `{ ... }` on the same line as the end of a call is that call's last
    /// argument, exactly what `f(fun() { ... })` writes. Nothing else follows
    /// a call on the same line with a `{`, so this only ever accepts text that
    /// was a parse error.
    ///
    /// A `{ x, y -> ... }` header declares the block's parameters by *name*
    /// only; their types come from the expected function type of the parameter
    /// the block is passed to (`kelp wiki page language/functions.md` part 1), so the literal
    /// is marked `infer_params`. The header is read only when the first tokens
    /// are identifiers, comma separated, followed by `->` or `=>` (one token),
    /// which appears nowhere else at the start of a block.
    pub(crate) fn parse_trailing_block(&mut self, args: &mut Vec<CallArg>, end: &mut Span) -> PResult<()> {
        if self.no_trailing_block || !self.check(&TokenKind::LBrace) {
            return Ok(());
        }
        let start = self.advance().span;
        let mut params = Vec::new();
        if self.block_header_starts_params() {
            loop {
                let (name, span) = self.expect_ident()?;
                params.push(Param {
                    name,
                    ty: TypeExpr::named("", span),
                    mode: ParamMode::Borrow,
                    default: None,
                });
                if self.check(&TokenKind::Comma) {
                    self.advance();
                    continue;
                }
                break;
            }
            self.expect(TokenKind::Arrow, "'->' or '=>' after the block's parameters")?;
        }
        let outer = std::mem::take(&mut self.loop_stack);
        let block = self.parse_block_body(start);
        self.loop_stack = outer;
        let block = block?;
        let span = block.span;
        *end = span;
        args.push(CallArg {
            name: None,
            value: Expr::FnLit {
                params,
                infer_params: true,
                return_type: None,
                body: Box::new(FunctionBody::Block(block)),
                span,
            },
        });
        Ok(())
    }

    /// Whether the `{` the parser has already consumed opens a block whose
    /// first tokens are a parameter header: one or more identifiers, comma
    /// separated, then `->` or `=>`. A lookahead only - nothing is consumed.
    pub(crate) fn block_header_starts_params(&self) -> bool {
        let mut i = self.pos;
        if !matches!(self.tokens.get(i).map(|t| &t.kind), Some(TokenKind::Ident(_))) {
            return false;
        }
        loop {
            i += 1;
            match self.tokens.get(i).map(|t| &t.kind) {
                Some(TokenKind::Arrow) => return true,
                Some(TokenKind::Comma) => {
                    i += 1;
                    if !matches!(self.tokens.get(i).map(|t| &t.kind), Some(TokenKind::Ident(_))) {
                        return false;
                    }
                }
                _ => return false,
            }
        }
    }

    pub(crate) fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    pub(crate) fn peek_kind(&self) -> &TokenKind {
        &self.tokens[self.pos].kind
    }

    /// The foreign-word hint for the token at `idx`, when that token is a word
    /// another language spells differently (`fn`, `null`, `as`). Only error
    /// sites read it, so it never changes what compiles.
    pub(crate) fn foreign_hint_at(&self, idx: usize) -> Option<String> {
        self.tokens.get(idx).and_then(|tok| foreign_hint_for(&tok.kind))
    }

    /// The kind of the token one past the current one (`Eof` past the end).
    pub(crate) fn peek2_kind(&self) -> TokenKind {
        self.tokens
            .get(self.pos + 1)
            .map(|t| t.kind.clone())
            .unwrap_or(TokenKind::Eof)
    }

    /// The kind of the token two past the current one (`Eof` past the end).
    pub(crate) fn peek3_kind(&self) -> TokenKind {
        self.tokens
            .get(self.pos + 2)
            .map(|t| t.kind.clone())
            .unwrap_or(TokenKind::Eof)
    }

    pub(crate) fn advance(&mut self) -> Token {
        let tok = self.tokens[self.pos].clone();
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        tok
    }

    pub(crate) fn check(&self, kind: &TokenKind) -> bool {
        std::mem::discriminant(self.peek_kind()) == std::mem::discriminant(kind)
    }

    pub(crate) fn expect(&mut self, kind: TokenKind, what: &str) -> PResult<Token> {
        if self.check(&kind) {
            Ok(self.advance())
        } else {
            Err(ParseError {
                help: self.foreign_hint_at(self.pos),
                message: format!("expected {what}, found {}", self.peek_kind().describe()),
                span: self.peek().span,
            })
        }
    }

    pub(crate) fn expect_ident(&mut self) -> PResult<(String, Span)> {
        match self.peek_kind().clone() {
            TokenKind::Ident(name) => {
                let span = self.peek().span;
                self.advance();
                Ok((name, span))
            }
            other => Err(ParseError {
                help: self.foreign_hint_at(self.pos),
                message: format!("expected identifier, found {}", other.describe()),
                span: self.peek().span,
            }),
        }
    }

    /// Consumes zero or more `Newline` tokens (blank lines already collapse
    /// to a single token in the lexer).
    pub(crate) fn skip_newlines(&mut self) {
        while self.check(&TokenKind::Newline) {
            self.advance();
        }
    }

    /// True at a point where a statement (or `when`-arm) may legally end:
    /// a newline, the block's closing `}`, or end of input.
    pub(crate) fn at_stmt_end(&self) -> bool {
        matches!(
            self.peek_kind(),
            TokenKind::Newline | TokenKind::RBrace | TokenKind::Eof
        )
    }

    pub fn parse_program(&mut self) -> PResult<Program> {
        self.skip_newlines();
        let mut impls = Vec::new();
        let mut consts = Vec::new();
        let mut statics = Vec::new();
        let mut pds = Vec::new();
        let mut datacs = Vec::new();
        let mut enums = Vec::new();
        let mut extern_types = Vec::new();
        let mut traits = Vec::new();
        let mut trait_impls = Vec::new();
        let mut functions = Vec::new();
        // One counter across both type declarations: it is what `resolve.rs`
        // uses to rebuild a single merged declaration order from the two
        // lists (A-1).
        let mut type_order = 0usize;
        // `derive(eq, hash)` (lang-derive-eq) is a line on its own above the
        // `datac` or `enum` it applies to. It is read here and attached to the
        // type when that type is parsed; the methods are made after the file.
        let mut derives: Vec<crate::derive::DeriveRequest> = Vec::new();
        let mut pending_derive: Option<crate::derive::DeriveRequest> = None;
        // The statements of a script (issue #288), in source order across the
        // declarations between them. They become the body of `main` at the end.
        let mut script: Vec<Stmt> = Vec::new();
        let mut script_span: Option<Span> = None;
        // The `pub {` that opened a group (issue #290), while its items are read.
        // Each item in the group is read as if `pub` were written in front of it.
        let mut pub_group: Option<Span> = None;
        while !matches!(self.peek_kind(), TokenKind::Eof) {
            if pub_group.is_some() && self.check(&TokenKind::RBrace) {
                self.advance();
                pub_group = None;
                self.skip_newlines();
                continue;
            }
            if pub_group.is_none() && self.check(&TokenKind::Pub) && matches!(self.peek2_kind(), TokenKind::LBrace) {
                let open = self.advance().span;
                self.advance();
                self.skip_newlines();
                if self.check(&TokenKind::RBrace) {
                    return Err(ParseError {
                        help: None,
                        message: "a 'pub' group needs at least one declaration between its braces".to_string(),
                        span: open.to(self.peek().span),
                    });
                }
                pub_group = Some(open);
                continue;
            }
            if let Some(req) = self.parse_derive_clause()? {
                if let Some(earlier) = &pending_derive {
                    return Err(ParseError {
                        help: None,
                        message: "a 'derive(...)' line applies to the 'datac' or 'enum' right under it, and this one follows another with nothing between".to_string(),
                        span: earlier.span,
                    });
                }
                pending_derive = Some(req);
                self.skip_newlines();
                continue;
            }
            // `repr(...)` is an optional leading clause on a `datac`/`enum`
            // (kernel stage 2), `section("...")` one on a `const`/`fun`
            // (kernel stage 4), and `pub` is an optional leading token on the
            // item forms that can carry one (A-13). They are read here, before
            // the dispatch below, in either order round `pub`, because a
            // modified item is otherwise the same declaration.
            let (leading_section, leading_align) = self.parse_section_and_align_clauses()?;
            let leading_repr = self.parse_repr_clause()?;
            // Inside a group every item is `pub` already, so a `pub` written on
            // it (before or after its clauses) is refused, and so is a nested group.
            if pub_group.is_some() && self.check(&TokenKind::Pub) {
                let at = self.advance().span;
                let message = if matches!(self.peek_kind(), TokenKind::LBrace) {
                    "a 'pub' group cannot be nested in another: the outer group already marks each item 'pub'".to_string()
                } else {
                    "a 'pub' group already marks each item 'pub', so drop the 'pub' written on this item".to_string()
                };
                return Err(ParseError { help: None, message, span: at });
            }
            let is_pub = self.check(&TokenKind::Pub) || pub_group.is_some();
            if is_pub {
                let pub_span = match pub_group {
                    Some(open) => open,
                    None => self.advance().span,
                };
                if pub_group.is_some() && self.at_script_statement() {
                    return Err(ParseError {
                        help: None,
                        message: "a 'pub' group holds declarations only, and this line is a statement: move it out of the group".to_string(),
                        span: self.peek().span,
                    });
                }
                self.check_pub_target(pub_span)?;
            }
            let repr = match (leading_repr, self.parse_repr_clause()?) {
                (None, after_pub) => after_pub,
                (before_pub, None) => before_pub,
                (Some(_), Some(_)) => {
                    return Err(ParseError {
                        help: None,
                        message: "a 'repr(...)' clause is written once, before or after 'pub', not both".to_string(),
                        span: self.peek().span,
                    })
                }
            };
            let (after_section, after_align) = self.parse_section_and_align_clauses()?;
            let section = match (leading_section, after_section) {
                (None, after_pub) => after_pub,
                (before_pub, None) => before_pub,
                (Some(_), Some(_)) => {
                    return Err(ParseError {
                        help: None,
                        message: "a 'section(...)' clause is written once, before or after 'pub', not both".to_string(),
                        span: self.peek().span,
                    })
                }
            };
            let align = match (leading_align, after_align) {
                (None, after_pub) => after_pub,
                (before_pub, None) => before_pub,
                (Some(_), Some(_)) => {
                    return Err(ParseError {
                        help: None,
                        message: "an 'align(N)' clause is written once, before or after 'pub', not both".to_string(),
                        span: self.peek().span,
                    })
                }
            };
            // `percpu static ...` (kernel stage 8): `percpu` is a contextual
            // word that modifies the `static` it precedes. It is read after
            // the `section(...)`/`align(N)` clauses so a modified item stays
            // the same declaration wherever the clauses are written.
            let percpu = self.check_percpu_marker();
            if !percpu && matches!(self.peek_kind(), TokenKind::Ident(n) if n == "percpu") {
                return Err(ParseError {
                    help: None,
                    message: "'percpu' applies to a 'static', written 'percpu static mut NAME: T = INIT'"
                        .to_string(),
                    span: self.peek().span,
                });
            }
            if repr.is_some() && !self.check(&TokenKind::Datac) && !self.check(&TokenKind::Enum) {
                return Err(ParseError {
                    help: None,
                    message: format!(
                        "'repr(...)' applies to a 'datac' or an 'enum', found {}",
                        self.peek_kind().describe()
                    ),
                    span: self.peek().span,
                });
            }
            if let Some(req) = &pending_derive {
                if !self.check(&TokenKind::Datac) && !self.check(&TokenKind::Enum) {
                    return Err(ParseError {
                        help: None,
                        message: "'derive(...)' applies to the 'datac' or 'enum' on the line under it".to_string(),
                        span: req.span,
                    });
                }
            }
            if section.is_some() && !self.check(&TokenKind::Const) && !self.check(&TokenKind::Static) && !self.at_function_decl() {
                return Err(ParseError {
                    help: None,
                    message: format!(
                        "'section(\"...\")' applies to a top-level 'const', 'static' or 'fun', found {}",
                        self.peek_kind().describe()
                    ),
                    span: self.peek().span,
                });
            }
            if align.is_some() && !self.at_function_decl() {
                return Err(ParseError {
                    help: None,
                    message: format!(
                        "'align(N)' applies to a top-level 'fun', found {}",
                        self.peek_kind().describe()
                    ),
                    span: self.peek().span,
                });
            }
            if self.check(&TokenKind::Impl) && !self.at_module_impl() {
                // `impl Name for Type { ... }` (A-12 slice 2). It declares
                // methods, so it belongs to the last group with `datac`,
                // `enum`, `fun` and `trait`, and may sit anywhere among them.
                let (decl, methods) = self.parse_trait_impl_decl()?;
                trait_impls.push(decl);
                functions.extend(methods);
            } else if self.check(&TokenKind::Trait) {
                traits.push(self.parse_trait_decl(is_pub)?);
            } else if self.check(&TokenKind::Impl) {
                if !consts.is_empty() || !statics.is_empty() || !pds.is_empty() || !datacs.is_empty() || !enums.is_empty() || !traits.is_empty() || !functions.is_empty() {
                    return Err(ParseError {
                        help: None,
                        message: "'impl' lines must come first (canonical order: impl, const/pd, datac/enum/trait/fun)".to_string(),
                        span: self.peek().span,
                    });
                }
                impls.push(self.parse_impl_decl()?);
            } else if self.check(&TokenKind::Const) {
                if !pds.is_empty() || !datacs.is_empty() || !enums.is_empty() || !traits.is_empty() || !functions.is_empty() {
                    return Err(ParseError {
                        help: None,
                        message: "'const' lines must come before any 'pd', 'datac', 'enum', 'trait' or 'fun' (canonical order: impl, const/pd, datac/enum/trait/fun)".to_string(),
                        span: self.peek().span,
                    });
                }
                consts.push(self.parse_const_decl(is_pub, section)?);
            } else if self.check(&TokenKind::Static) {
                if !pds.is_empty() || !datacs.is_empty() || !enums.is_empty() || !traits.is_empty() || !functions.is_empty() {
                    return Err(ParseError {
                        help: None,
                        message: "'static' lines must come before any 'pd', 'datac', 'enum', 'trait' or 'fun' (canonical order: impl, const/static/pd, datac/enum/trait/fun)".to_string(),
                        span: self.peek().span,
                    });
                }
                statics.push(self.parse_static_decl(is_pub, section, percpu)?);
            } else if self.check(&TokenKind::Pd) {
                if !datacs.is_empty() || !enums.is_empty() || !traits.is_empty() || !functions.is_empty() {
                    return Err(ParseError {
                        help: None,
                        message: "'pd' lines must come before any 'datac', 'enum', 'trait' or 'fun' (canonical order: impl, const/pd, datac/enum/trait/fun)".to_string(),
                        span: self.peek().span,
                    });
                }
                pds.push(self.parse_pd_decl()?);
            } else if self.check(&TokenKind::Tl) {
                if !datacs.is_empty() || !enums.is_empty() || !traits.is_empty() || !functions.is_empty() {
                    return Err(ParseError {
                        help: None,
                        message: "'tl' lines must come before any 'datac', 'enum', 'trait' or 'fun' (canonical order: impl, const/pd/tl, datac/enum/trait/fun)".to_string(),
                        span: self.peek().span,
                    });
                }
                pds.push(self.parse_tl_decl()?);
            } else if self.check(&TokenKind::Datac) {
                let (mut d, methods) = self.parse_datac_decl(is_pub, repr)?;
                d.order = type_order;
                type_order += 1;
                if let Some(mut req) = pending_derive.take() {
                    req.type_name = d.name.clone();
                    derives.push(req);
                }
                datacs.push(d);
                // Methods written in the datac's block (issue #285) are the
                // methods the same lines after the type would be.
                functions.extend(methods);
            } else if self.check(&TokenKind::Enum) {
                let mut e = self.parse_enum_decl(is_pub, repr)?;
                e.order = type_order;
                type_order += 1;
                if let Some(mut req) = pending_derive.take() {
                    req.type_name = e.name.clone();
                    derives.push(req);
                }
                enums.push(e);
            } else if self.check(&TokenKind::Extern)
                && matches!(self.peek2_kind(), TokenKind::Ident(ref n) if n == "type")
            {
                extern_types.push(self.parse_extern_type_decl(is_pub)?);
            } else if self.at_script_statement() {
                let first = self.peek().span;
                let stmts = self.parse_script_run(false)?;
                let span = first.to(self.tokens[self.pos.saturating_sub(1)].span);
                script_span = Some(script_span.map_or(span, |s| s.to(span)));
                script.extend(stmts);
            } else {
                // `unsafe fun ...` / `naked fun ...` (MK9-S2 stage 1, kernel
                // stage 5). Both are contextual words everywhere, so they are
                // identifier tokens and the lookahead is what makes them
                // declaration markers here.
                let (unsafe_, naked, must_use, interrupt) = self.check_function_markers();
                // `fun` may be left out of a plain top-level function (issue #287).
                self.bare_top_fn = !is_pub && self.at_bare_fn_decl();
                let mut f = self.parse_function(is_pub, unsafe_, naked, section, align, false)?;
                f.must_use = must_use;
                f.interrupt = interrupt;
                // `Self` is a receiver only inside a `trait` or an `impl`
                // (A-12 slice 2): at the top level it names nothing.
                if let Some(r) = &f.receiver {
                    if r.type_name == "Self" {
                        return Err(ParseError {
                            help: None,
                            message: "'Self' can only be written inside a 'trait' or an 'impl': it names the implementing type, so a method of a named type writes that type ('fun Circle.area()')".to_string(),
                            span: r.span,
                        });
                    }
                }
                functions.append(&mut self.pending_externs);
                functions.push(f);
            }
            self.skip_newlines();
        }
        if let Some(open) = pub_group {
            return Err(ParseError {
                help: Some("add '}' after the last declaration of the group".to_string()),
                message: "a 'pub' group is not closed before the end of the file".to_string(),
                span: open,
            });
        }
        if let Some(req) = pending_derive {
            return Err(ParseError {
                help: None,
                message: "'derive(...)' is written on a line of its own above a 'datac' or 'enum', and nothing follows it".to_string(),
                span: req.span,
            });
        }
        if let Some(span) = script_span {
            // The statements are the file's `main`, so the file cannot also
            // declare one (issue #288).
            if let Some(main) = functions.iter().find(|f| f.name == "main" && f.receiver.is_none()) {
                return Err(ParseError {
                    help: Some("a file of top-level statements runs them as its main: delete the 'fun main' or move the statements into it".to_string()),
                    message: "a file with top-level statements cannot also declare 'main'".to_string(),
                    span: main.span,
                });
            }
            functions.push(FunctionDecl {
                name: "main".to_string(),
                is_pub: false,
                type_params: Vec::new(),
                type_param_bounds: Vec::new(),
                projection_bounds: Vec::new(),
                receiver_bounds: Vec::new(),
                const_params: Vec::new(),
                trait_impl: None,
                extern_: false,
                unsafe_: false,
                must_use: false,
                variadic: false,
                naked: false,
                interrupt: false,
                receiver: None,
                params: Vec::new(),
                return_type: None,
                body: FunctionBody::Block(Block { stmts: script, span }),
                module: String::new(),
                body_module: None,
                section: None,
                align: None,
                span,
            });
        }
        let derived = crate::derive::expand(&derives, &datacs, &enums, &functions, &impls)?;
        functions.extend(derived.functions);
        trait_impls.extend(derived.impls);
        Ok(Program {
            script: script_span,
            impls,
            consts,
            statics,
            pds,
            datacs,
            enums,
            extern_types,
            traits,
            trait_impls,
            functions,
        })
    }

    /// The `return` after `.`, which is the retired `.return:` spelling. It is
    /// refused by name: the return type is written `=> T` after the parameters
    /// (`parse_return_type`).
    pub(crate) fn expect_return_marker(&mut self) -> PResult<()> {
        if self.check(&TokenKind::Return) {
            let span = self.peek().span;
            return Err(ParseError {
                help: None,
                message: "'.return:' is no longer accepted: write the return type after the parameter list with '=>', as in 'fun f(a: i32) => i32'.".to_string(),
                span,
            });
        }
        let (marker, mspan) = self.expect_ident()?;
        Err(ParseError {
            help: None,
            message: format!(
                "expected 'return' after '.', found '{marker}': the return type is written '=> T' after the parameters"
            ),
            span: mspan,
        })
    }

    /// The return type written after a parameter list, or `None` when there is
    /// none. `=> T` is the spelling; `-> T` is an alias of it. The retired
    /// `=>: T` still parses until it is refused by name.
    pub(crate) fn parse_return_type(&mut self) -> PResult<Option<TypeExpr>> {
        if self.check(&TokenKind::Arrow) {
            self.advance();
            return Ok(Some(self.parse_type()?));
        }
        if self.check(&TokenKind::Dot) {
            self.advance();
            self.expect_return_marker()?;
            self.expect(TokenKind::Colon, "':'")?;
            return Ok(Some(self.parse_type()?));
        }
        Ok(None)
    }

    /// The `percpu` marker before a top-level `static` (kernel stage 8). Like
    /// `unsafe`/`naked`, it is a contextual word: it is only a marker when
    /// `static` is what follows it, so a program can still use `percpu` as an
    /// ordinary name. Nothing is consumed otherwise.
    pub(crate) fn check_percpu_marker(&mut self) -> bool {
        if matches!(self.peek_kind(), TokenKind::Ident(n) if n == "percpu")
            && matches!(self.peek2_kind(), TokenKind::Static)
        {
            self.advance();
            true
        } else {
            false
        }
    }

    /// The declaration markers before `fun`: `unsafe` and `naked` (MK9-S2
    /// stage 1, kernel stage 5). Neither is a keyword, so each is only a marker
    /// when `fun` is what follows the run of them, and nothing is consumed
    /// otherwise.
    pub(crate) fn check_function_markers(&mut self) -> (bool, bool, bool, bool) {
        let mut unsafe_ = false;
        let mut naked = false;
        let mut must_use = false;
        let mut interrupt = false;
        let mut i = 0;
        loop {
            match self.tokens.get(self.pos + i).map(|t| &t.kind) {
                Some(TokenKind::Ident(n)) if n == "unsafe" => unsafe_ = true,
                Some(TokenKind::Ident(n)) if n == "naked" => naked = true,
                Some(TokenKind::Ident(n)) if n == "must_use" => must_use = true,
                Some(TokenKind::Ident(n)) if n == "interrupt" => interrupt = true,
                _ => break,
            }
            i += 1;
        }
        if !matches!(
            self.tokens.get(self.pos + i).map(|t| &t.kind),
            Some(TokenKind::Fun)
        ) {
            return (false, false, false, false);
        }
        for _ in 0..i {
            self.advance();
        }
        (unsafe_, naked, must_use, interrupt)
    }

    /// The item a leading `pub` must be followed by (A-13). `pub` on anything
    /// else is refused where it is written, naming what it applies to, rather
    /// than read as a no-op or as something else. A `pd` gets its own message
    /// because it is not a missing feature: a passdown is module-scoped by
    /// design (`kelp wiki page meta/roadmap.md` item 10), so `pub` on one can never be true.
    /// Whether the cursor is at a top-level function written without `fun`
    /// (issue #287): a name, or `mut` and a type name, then `Name` or `Type.name`,
    /// then `(` or `<`. Only a line of that shape is a declaration, so an
    /// assignment or any other top-level line is never read as one.
    pub(crate) fn at_bare_fn_decl(&self) -> bool {
        let kind = |i: usize| self.tokens.get(self.pos + i).map(|t| &t.kind);
        let has_mut = matches!(kind(0), Some(TokenKind::Mut));
        let mut i = usize::from(has_mut);
        if !matches!(kind(i), Some(TokenKind::Ident(_))) {
            return false;
        }
        i += 1;
        if matches!(kind(i), Some(TokenKind::Dot)) {
            if !matches!(kind(i + 1), Some(TokenKind::Ident(_))) {
                return false;
            }
            i += 2;
        } else if has_mut {
            return false;
        }
        if has_mut {
            return matches!(kind(i), Some(TokenKind::LParen) | Some(TokenKind::Lt));
        }
        // A line that starts like a call (`print("hi")`, `c.bump()`) is a
        // declaration only when a body follows the parameter list: the `{`, or
        // the `=>` of an expression body. Without that it is a statement of a
        // script (issue #288).
        let mut open = i;
        if matches!(kind(i), Some(TokenKind::Lt)) {
            loop {
                open += 1;
                match kind(open) {
                    Some(TokenKind::LParen) => break,
                    Some(TokenKind::Newline) | Some(TokenKind::Eof) | None => return false,
                    _ => {}
                }
            }
        }
        if !matches!(kind(open), Some(TokenKind::LParen)) {
            return false;
        }
        let mut depth = 0usize;
        let mut k = open;
        loop {
            match kind(k) {
                Some(TokenKind::LParen) => depth += 1,
                Some(TokenKind::RParen) => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                Some(TokenKind::Eof) | None => return false,
                _ => {}
            }
            k += 1;
        }
        matches!(kind(k + 1), Some(TokenKind::LBrace) | Some(TokenKind::Arrow))
    }

    /// Whether the cursor is at a top-level line that is a statement of a
    /// script rather than a declaration (issue #288). A statement starts a line
    /// and is none of the declaration forms: a `fun` or modifier run, a receiver
    /// `mut`, a bare function header, a declaration keyword or clause, or two
    /// names in a row (which keeps its refusal, so `def main()` still names the
    /// foreign word).
    pub(crate) fn at_script_statement(&self) -> bool {
        // A statement starts a line: `extern type Widget {}` is not a script.
        let line_start = self.pos == 0 || matches!(self.tokens[self.pos - 1].kind, TokenKind::Newline);
        if !line_start || self.check(&TokenKind::Mut) || self.at_bare_fn_decl() {
            return false;
        }
        let kind = |i: usize| self.tokens.get(self.pos + i).map(|t| &t.kind);
        // A run of function markers before `fun` (`unsafe naked fun`) is a declaration.
        let mut i = 0;
        while matches!(kind(i), Some(TokenKind::Ident(n)) if matches!(n.as_str(), "unsafe" | "naked" | "must_use" | "interrupt")) {
            i += 1;
        }
        if matches!(kind(i), Some(TokenKind::Fun) | Some(TokenKind::Extern)) {
            return false;
        }
        match kind(0) {
            // The keywords of the declarations, and the clauses that come before one.
            Some(TokenKind::Pub | TokenKind::Impl | TokenKind::Trait | TokenKind::Const | TokenKind::Static | TokenKind::Pd | TokenKind::Tl | TokenKind::Datac | TokenKind::Enum | TokenKind::Fun) => false,
            Some(TokenKind::Ident(n)) if matches!(n.as_str(), "derive" | "repr" | "section" | "align") && matches!(kind(1), Some(TokenKind::LParen)) => false,
            Some(TokenKind::Ident(n)) if n == "percpu" && matches!(kind(1), Some(TokenKind::Static)) => false,
            // Two names in a row (`def main()`, `struct P`) is never a statement: it keeps
            // the refusal that names the foreign word. `let` is the one contextual word that
            // starts a statement before a name (`let Some(n) = ... else`).
            Some(TokenKind::Ident(n)) if n != "let" && matches!(kind(1), Some(TokenKind::Ident(_))) => false,
            _ => true,
        }
    }

    /// The statements of a script from the cursor up to the next declaration or
    /// the end of the file (issue #288). A top-level `let ... else` takes the
    /// rest of the file as its rest (`rest` is true inside it), so a declaration
    /// after one is refused: the rest would have no end.
    pub(crate) fn parse_script_run(&mut self, rest: bool) -> PResult<Vec<Stmt>> {
        let mut stmts = Vec::new();
        loop {
            self.skip_newlines();
            if self.check(&TokenKind::Eof) {
                break;
            }
            if !self.at_script_statement() {
                if rest {
                    return Err(ParseError {
                        help: Some("the statements after a top-level 'let ... else' are its rest, so a declaration goes above the 'let'".to_string()),
                        message: "a declaration cannot follow a 'let ... else' at the top level".to_string(),
                        span: self.peek().span,
                    });
                }
                break;
            }
            if self.at_let_else() {
                let (pattern, init, else_block, span) = self.parse_let_else_head()?;
                if !self.check(&TokenKind::Eof) {
                    if !self.check(&TokenKind::Newline) {
                        return Err(ParseError {
                            help: self.statement_end_hint(),
                            message: format!(
                                "expected a newline after a statement, found {}",
                                self.peek_kind().describe()
                            ),
                            span: self.peek().span,
                        });
                    }
                }
                let rest_stmts = self.parse_script_run(true)?;
                let else_span = else_block.span;
                stmts.push(Stmt::Expr {
                    expr: Expr::When {
                        scrutinee: Box::new(init),
                        arms: vec![
                            WhenArm {
                                pattern: Some(pattern),
                                body: Expr::Block(Block { stmts: rest_stmts, span }),
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
                break;
            }
            let stmt = self.parse_stmt()?;
            stmts.push(stmt);
            if !self.check(&TokenKind::Newline) && !self.check(&TokenKind::Eof) {
                return Err(ParseError {
                    help: self.statement_end_hint(),
                    message: format!(
                        "expected a newline after a statement, found {}",
                        self.peek_kind().describe()
                    ),
                    span: self.peek().span,
                });
            }
        }
        Ok(stmts)
    }

    pub(crate) fn check_pub_target(&self, pub_span: Span) -> PResult<()> {
        match self.peek_kind() {
            TokenKind::Fun
            | TokenKind::Extern
            | TokenKind::Datac
            | TokenKind::Enum
            | TokenKind::Trait
            | TokenKind::Const
            | TokenKind::Static => Ok(()),
            // `pub unsafe fun ...` / `pub naked fun ...`, in either order: both
            // are contextual words, so each is only a marker when `fun` follows
            // the run of them.
            TokenKind::Ident(n)
                if (n == "unsafe" || n == "naked" || n == "must_use" || n == "interrupt")
                    && (matches!(self.peek2_kind(), TokenKind::Fun)
                        || matches!(self.peek2_kind(), TokenKind::Ident(m) if m == "unsafe" || m == "naked" || m == "must_use" || m == "interrupt")) =>
            {
                Ok(())
            }
            // `pub repr(c) datac ...`: a `repr(...)` clause may sit between
            // `pub` and the type declaration; the dispatch after it is what
            // requires a 'datac' or an 'enum'.
            TokenKind::Ident(n)
                if n == "repr" && matches!(self.peek2_kind(), TokenKind::LParen) =>
            {
                Ok(())
            }
            // `pub section(".x") fun f() ...`: a `section(...)` clause may sit
            // between `pub` and the `const`/`fun` it modifies (kernel stage 4).
            TokenKind::Ident(n)
                if n == "section" && matches!(self.peek2_kind(), TokenKind::LParen) =>
            {
                Ok(())
            }
            // `pub percpu static ...`: `percpu` may sit between `pub` and the
            // `static` it modifies (kernel stage 8).
            TokenKind::Ident(n) if n == "percpu" && matches!(self.peek2_kind(), TokenKind::Static) => {
                Ok(())
            }
            // `pub align(4) fun f() ...`: an `align(N)` clause may sit between
            // `pub` and the `fun` it modifies (kernel gap 2).
            TokenKind::Ident(n)
                if n == "align" && matches!(self.peek2_kind(), TokenKind::LParen) =>
            {
                Ok(())
            }
            // `pub half(...)`: a function written without `fun` cannot be `pub`
            // (issue #287). The refusal writes the spelling that works.
            TokenKind::Ident(n) if self.at_bare_fn_decl() => {
                let head = match self.peek2_kind() {
                    TokenKind::Dot => match self.tokens.get(self.pos + 2).map(|t| &t.kind) {
                        Some(TokenKind::Ident(m)) => format!("{n}.{m}"),
                        _ => n.clone(),
                    },
                    _ => n.clone(),
                };
                Err(ParseError {
                    help: None,
                    message: format!(
                        "a function written without 'fun' cannot be 'pub': write 'pub fun {head}(...)'"
                    ),
                    span: pub_span.to(self.peek().span),
                })
            }
            TokenKind::Pd => Err(ParseError {
                help: None,
                message: "a 'pd' cannot be 'pub': a passdown is module-scoped by design, so another module cannot reach it; a 'fun' that reads it can be exported instead".to_string(),
                span: pub_span.to(self.peek().span),
            }),
            TokenKind::Tl => Err(ParseError {
                help: None,
                message: "a 'tl' cannot be 'pub': thread-local storage is module-scoped, the same as a 'pd'; a 'fun' that reads it can be exported instead".to_string(),
                span: pub_span.to(self.peek().span),
            }),
            _ => Err(ParseError {
                help: None,
                message: format!(
                    "'pub' applies to a top-level 'fun', 'extern fun', 'extern type', 'datac', 'enum', 'const' or 'static', found {}",
                    self.peek_kind().describe()
                ),
                span: pub_span.to(self.peek().span),
            }),
        }
    }

    /// `const NAME: T = expr` - the type is mandatory, since a `const` is a
    /// declared contract rather than an inferred binding.
    pub(crate) fn parse_const_decl(&mut self, is_pub: bool, section: Option<String>) -> PResult<ConstDecl> {
        let start = self.expect(TokenKind::Const, "'const'")?.span;
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
        Ok(ConstDecl {
            name,
            is_pub,
            ty,
            init,
            module: String::new(),
            section,
            span,
        })
    }

    /// `static NAME: T = expr` / `static mut NAME: T = expr` - a top-level
    /// global object (kernel stage 7), optionally with a leading `percpu`
    /// (kernel stage 8). The type is mandatory, as it is on a `const`, and the
    /// initializer must fold at compile time.
    pub(crate) fn parse_static_decl(
        &mut self,
        is_pub: bool,
        section: Option<String>,
        percpu: bool,
    ) -> PResult<StaticDecl> {
        let start = self.expect(TokenKind::Static, "'static'")?.span;
        let mutable = if self.check(&TokenKind::Mut) {
            self.advance();
            true
        } else {
            false
        };
        let (name, _) = self.expect_ident()?;
        self.expect(TokenKind::Colon, "':' (a 'static' must declare its type)")?;
        let ty = self.parse_type()?;
        self.expect(TokenKind::Eq, "'='")?;
        let init = self.parse_expr()?;
        let span = start.to(init.span());
        Ok(StaticDecl {
            name,
            is_pub,
            mutable,
            percpu,
            ty,
            init,
            module: String::new(),
            section,
            span,
        })
    }

    /// `tl val name = literal` / `tl var name = literal`: thread-local storage.
    pub(crate) fn parse_tl_decl(&mut self) -> PResult<PdDecl> {
        let start = self.expect(TokenKind::Tl, "'tl'")?.span;
        self.parse_decl_after_keyword(start, true)
    }

    /// `pd val name = expr` / `pd var name = expr` / `pd val name: T = expr`.
    pub(crate) fn parse_pd_decl(&mut self) -> PResult<PdDecl> {
        let start = self.expect(TokenKind::Pd, "'pd'")?.span;
        self.parse_decl_after_keyword(start, false)
    }

    pub(crate) fn parse_decl_after_keyword(&mut self, start: Span, thread_local: bool) -> PResult<PdDecl> {
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
            thread_local,
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
    pub(crate) fn parse_impl_decl(&mut self) -> PResult<ImplDecl> {
        let start = self.expect(TokenKind::Impl, "'impl'")?.span;
        let (path, path_span) = match self.peek_kind().clone() {
            TokenKind::StringLit(s) => {
                let span = self.peek().span;
                self.advance();
                (s, span)
            }
            other => {
                return Err(ParseError {
                    help: None,
                    message: format!("expected a quoted module path after 'impl', found {}", other.describe()),
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
                help: None,
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
                help: None,
                message: "`impl \"...\" with caps(...)` is gone: drop the `with`, so the grant is \
                          `impl \"std/fs\" caps(fs = [\"/var/log/**\"])`, and the patterns, the \
                          domains and the no-escalation rule are exactly the same"
                    .to_string(),
                span: self.peek().span,
            });
        }
        // `caps(domain = ["pattern", ...], ...)` - the capability grant
        // (M7). Domains and patterns are validated in `resolve.rs`, where the
        // module graph (and therefore the importer's own set) is known.
        // `caps(*)` is the one alternative spelling: it delegates the whole
        // resource of every domain, so it stands alone and is refused here
        // when a named domain stands beside it.
        let has_caps_clause = if matches!(self.peek_kind(), TokenKind::Ident(n) if n == "caps") {
            self.advance();
            true
        } else {
            false
        };
        let mut caps_all = false;
        let caps = if has_caps_clause {
            self.expect(TokenKind::LParen, "'(' after 'caps'")?;
            let mut grants: Vec<CapGrant> = Vec::new();
            if self.check(&TokenKind::Star) {
                let star_span = self.peek().span;
                self.advance();
                caps_all = true;
                if !self.check(&TokenKind::RParen) {
                    return Err(mixing_wildcard_error(star_span));
                }
            }
            while !self.check(&TokenKind::RParen) {
                if self.check(&TokenKind::Star) {
                    return Err(mixing_wildcard_error(self.peek().span));
                }
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
                                help: None,
                                message: format!(
                                    "expected a quoted path or host pattern, found {}", other.describe()
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
            caps_all,
            span: start.to(path_span),
        })
    }
}
