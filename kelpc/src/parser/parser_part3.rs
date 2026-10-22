use super::*;

impl Parser {
    /// The `where` clause after a generic declaration's header (MISSING-Ve8Nc
    /// step 5), shared by a function and an impl: `where T: A + B` bounds a
    /// parameter, and `where T::Assoc: A` bounds an associated type projected
    /// from one. `params` names the declaration's own type parameters; a name
    /// outside them is refused here, where the declaration is in hand.
    pub(crate) fn parse_where_clause(
        &mut self,
        params: &[String],
        bounds: &mut [Vec<TypeExpr>],
        projections: &mut Vec<ProjectionBound>,
        owner: &str,
    ) -> PResult<()> {
        if !matches!(self.peek_kind(), TokenKind::Ident(n) if n == "where") {
            return Ok(());
        }
        self.advance();
        loop {
            let (pname, pspan) = self.expect_ident()?;
            let Some(at) = params.iter().position(|p| *p == pname) else {
                return Err(ParseError {
                    help: None,
                    message: format!(
                        "'where {pname}: ...' names no type parameter of this {owner}: a 'where' clause bounds a parameter written in the '<...>' list, so declare '{pname}' there first"
                    ),
                    span: pspan,
                });
            };
            if self.check(&TokenKind::ColonColon) {
                self.advance();
                let (assoc, _) = self.expect_ident()?;
                self.expect(TokenKind::Colon, "':' after the associated type in a 'where' clause")?;
                let more = self.parse_bounds()?;
                for bound in more {
                    projections.push(ProjectionBound {
                        param: pname.clone(),
                        assoc: assoc.clone(),
                        bound,
                    });
                }
            } else {
                self.expect(TokenKind::Colon, "':' after the parameter in a 'where' clause")?;
                let more = self.parse_bounds()?;
                bounds[at].extend(more);
            }
            if self.check(&TokenKind::Comma) {
                self.advance();
                continue;
            }
            break;
        }
        Ok(())
    }

    /// A `datac`/`enum` parameter list, with a bound refused (A-12 slice 2): a
    /// type declaration's parameters are bound at the use site (a function's
    /// `fun f<T: Name>`), not on the type itself. A `const` parameter keeps
    /// its integer type and is split out into `const_params` (A-20).
    pub(crate) fn no_type_param_bounds(
        params: Vec<TypeParamParsed>,
    ) -> PResult<(Vec<String>, Vec<ConstParamDecl>)> {
        if let Some(p) = params.iter().find(|p| !p.bounds.is_empty()) {
            return Err(ParseError {
                help: None,
                message: format!(
                    "a bound cannot be written on a type declaration's parameter '{}': a bound is written where the parameter is used ('fun f<T: Name>'), so a type declaration lists its parameters alone",
                    p.name
                ),
                span: p.bounds[0].span,
            });
        }
        let names: Vec<String> = params.iter().map(|p| p.name.clone()).collect();
        let consts: Vec<ConstParamDecl> = params
            .into_iter()
            .filter_map(|p| {
                p.const_ty.map(|ty| ConstParamDecl {
                    name: p.name,
                    ty,
                    span: p.span,
                })
            })
            .collect();
        Ok((names, consts))
    }

    /// Whether the `impl` at the cursor opens an **import** (`impl "path"`,
    /// `design/syntax.md (kelp wiki)` 13.1) rather than an `impl Trait for Type { ... }`
    /// block (A-12 slice 2): the two are told apart by the token after the
    /// keyword, a string literal for the import and an identifier for the
    /// block.
    pub(crate) fn at_module_impl(&self) -> bool {
        self.check(&TokenKind::Impl) && matches!(self.peek2_kind(), TokenKind::StringLit(_))
    }

    /// A dotted name: an identifier, then `.` and another identifier, repeated
    /// (`Shape`, `shape.Shape`). Used where only a name is legal (a bound, an
    /// `impl` line's trait, and its `for` type, A-12 slice 2), so a generic
    /// argument or a `&` is refused rather than silently accepted.
    pub(crate) fn parse_dotted_name(&mut self) -> PResult<(String, Span)> {
        let (first, start) = self.expect_ident()?;
        let mut name = first;
        let mut end = start;
        while self.check(&TokenKind::Dot) {
            self.advance();
            let (part, pspan) = self.expect_ident()?;
            name.push('.');
            name.push_str(&part);
            end = pspan;
        }
        Ok((name, start.to(end)))
    }

    /// The payload of a variant, or a `datac`'s field list, with the opening
    /// `(` already consumed and the closing `)` left for the caller. Shared
    /// so an `enum` variant and a `datac` field obey exactly the same
    /// `val`/`var name: Type` rules.
    pub(crate) fn parse_field_list(&mut self) -> PResult<Vec<FieldDecl>> {
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
    /// - variants are newline-separated, and a variant's payload list is the
    /// same one a `datac` uses (`design/syntax.md (kelp wiki)` §9.2).
    pub(crate) fn parse_enum_decl(&mut self, is_pub: bool, repr: Option<Repr>) -> PResult<EnumDecl> {
        let start = self.expect(TokenKind::Enum, "'enum'")?.span;
        let (name, _) = self.expect_ident()?;
        let (type_params, const_params) =
            Parser::no_type_param_bounds(self.parse_type_params()?)?;
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
            // `Off = 0`: an explicit discriminant on a field-less variant.
            // Whether the enum has any payload at all is checked in
            // `infer.rs`, where the whole declaration is in view.
            let discriminant = if self.check(&TokenKind::Eq) {
                self.advance();
                let negative = if self.check(&TokenKind::Minus) {
                    self.advance();
                    true
                } else {
                    false
                };
                match self.peek_kind().clone() {
                    TokenKind::IntLit(n) => {
                        self.advance();
                        let value = i128::from(n);
                        Some(if negative { -value } else { value })
                    }
                    other => {
                        return Err(ParseError {
                            help: None,
                            message: format!(
                                "expected an integer literal discriminant after '=', found {}", other.describe()
                            ),
                            span: self.peek().span,
                        })
                    }
                }
            } else {
                None
            };
            variants.push(VariantDecl {
                name: vname,
                fields,
                discriminant,
                span: vspan,
            });
            if !self.check(&TokenKind::RBrace) {
                if !self.check(&TokenKind::Newline) {
                    return Err(ParseError {
                        help: None,
                        message: format!(
                            "expected a newline or '}}' after an enum variant, found {}",
                            self.peek_kind().describe()
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
                help: None,
                message: "an 'enum' needs at least one variant".to_string(),
                span: start.to(end),
            });
        }
        Ok(EnumDecl {
            name,
            is_pub,
            type_params,
            const_params,
            variants,
            repr,
            order: 0,
            module: String::new(),
            span: start.to(end),
        })
    }

    /// One function declaration. `signature` is set only for a method written
    /// inside a `trait` body (A-12 slices 2 and 3): its receiver must be
    /// `Self`, and a body is a default method. Every other spelling of the
    /// declaration is shared, so a trait method, a default and an `impl`
    /// method are parsed by the same code and cannot drift apart.
    pub(crate) fn parse_function(&mut self, is_pub: bool, unsafe_: bool, naked: bool, section: Option<String>, align: Option<u32>, signature: bool) -> PResult<FunctionDecl> {
        let outer = std::mem::take(&mut self.loop_stack);
        let r = self.parse_function_inner(is_pub, unsafe_, naked, section, align, signature);
        self.loop_stack = outer;
        r
    }

    pub(crate) fn parse_function_inner(&mut self, is_pub: bool, unsafe_: bool, naked: bool, section: Option<String>, align: Option<u32>, signature: bool) -> PResult<FunctionDecl> {
        // `extern fun ...` - a declaration whose body the compiler supplies
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
                help: None,
                message: "'move' cannot be written on a receiver - a method always borrows its receiver; a move-taking parameter is written 'name: move T'".to_string(),
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
        let (receiver, name, type_params, type_param_bounds, const_params) =
            if self.check(&TokenKind::Dot) {
            self.advance();
            let (method, method_span) = self.expect_ident()?;
            // `fun Pair<A, B>.map_first<C>(...)`: the method's own type
            // parameters, after its name. They are separate from the
            // receiver's (which are the type's) and inferred at a call.
            let own_params = self.parse_type_params()?;
            if let Some(p) = own_params.iter().find(|p| p.const_ty.is_some()) {
                return Err(ParseError {
                    help: None,
                    message: format!("a method cannot take the const parameter '{}' of its own yet: only type parameters", p.name),
                    span: method_span,
                });
            }
            // A receiver's parameters are its type's, so there is nowhere for a
            // bound to go (A-12 slice 2): the type's own parameters are bound
            // by whoever declares the generic type.
            if let Some(p) = name_params.iter().find(|p| !p.bounds.is_empty()) {
                return Err(ParseError {
                    help: None,
                    message: format!(
                        "a bound cannot be written on a receiver's type parameter '{}': those parameters come from the receiver's type, so bind them there, or write the bound on a function's own parameter list ('fun f<T: Name>')",
                        p.name
                    ),
                    span: first_span,
                });
            }
            let type_args: Vec<String> = name_params.iter().map(|p| p.name.clone()).collect();
            let const_params: Vec<ConstParamDecl> = name_params
                .into_iter()
                .filter_map(|p| {
                    p.const_ty.map(|ty| ConstParamDecl {
                        name: p.name,
                        ty,
                        span: p.span,
                    })
                })
                .collect();
            (
                Some(Receiver {
                    type_name: first,
                    type_args,
                    const_params,
                    mutable: receiver_mutable,
                    span: first_span,
                }),
                method,
                own_params.iter().map(|p| p.name.clone()).collect(),
                own_params.iter().map(|p| p.bounds.clone()).collect(),
                Vec::new(),
            )
        } else {
            if receiver_mutable {
                return Err(ParseError {
                    help: None,
                    message: "'mut' before a function name is only valid on a receiver ('fun mut Type.method(...)')".to_string(),
                    span: first_span,
                });
            }
            let type_params: Vec<String> = name_params.iter().map(|p| p.name.clone()).collect();
            let bounds: Vec<Vec<TypeExpr>> =
                name_params.iter().map(|p| p.bounds.clone()).collect();
            let const_params: Vec<ConstParamDecl> = name_params
                .into_iter()
                .filter_map(|p| {
                    p.const_ty.map(|ty| ConstParamDecl {
                        name: p.name,
                        ty,
                        span: p.span,
                    })
                })
                .collect();
            (None, first, type_params, bounds, const_params)
        };
        // A trait's method signatures are the trait's interface: they take no
        // type parameters of their own, and their receiver is `Self` (A-12
        // slice 2). Both are refused here, where the receiver and the
        // parameter list are already in hand.
        if signature {
            if extern_ {
                return Err(ParseError {
                    help: None,
                    message: "a trait method cannot be 'extern': a trait declares the method an impl or the default body provides".to_string(),
                    span: start,
                });
            }
            match &receiver {
                Some(r) if r.type_name == "Self" => {
                    if !type_params.is_empty() {
                        return Err(ParseError {
                            help: None,
                            message: "a trait method cannot take type parameters: the signature names 'Self' and the trait's own types".to_string(),
                            span: first_span,
                        });
                    }
                }
                Some(r) => {
                    return Err(ParseError {
                        help: None,
                        message: format!(
                            "a trait method's receiver is written 'Self', not '{}': 'Self' stands for the implementing type ('fun Self.{name}(...)')",
                            r.type_name
                        ),
                        span: first_span,
                    })
                }
                // A receiverless trait method is a **static** method (A-12
                // slice 6): `fun from(value: T) => Self`. It may not take type
                // parameters of its own (a generic method stays refused), which
                // keeps it exactly as object-unsafe-free as a receiver method.
                None => {
                    if !type_params.is_empty() {
                        return Err(ParseError {
                            help: None,
                            message: "a trait method cannot take type parameters: the signature names 'Self', the trait's own types and no others"
                                .to_string(),
                            span: first_span,
                        });
                    }
                }
            }
        }

        // A method is reached through a value of its type, never through a
        // module alias, so `pub` on one has nothing to govern: the type's own
        // `pub` is what exports the method with it (A-13).
        if is_pub && receiver.is_some() {
            return Err(ParseError {
                help: None,
                message: "'pub' cannot be written on a method: a method's reachability follows its declaring type, so mark the type 'pub' instead".to_string(),
                span: start,
            });
        }

        let (params, variadic) = self.parse_params()?;
        // A-20: `impl Trait` in argument position is sugar for a fresh bounded
        // type parameter, one per `impl` occurrence. The rewrite happens here,
        // at the declaration, so every later pass sees an ordinary generic
        // function. A receiver method takes no type parameters of its own, and
        // a trait signature declares none either, so both refuse it.
        let (params, type_params, mut type_param_bounds) = desugar_impl_trait_params(
            params,
            type_params,
            type_param_bounds,
            receiver.is_some(),
            signature,
            first_span,
        )?;

        if extern_ && receiver.is_some() {
            return Err(ParseError {
                help: None,
                message: "'extern' cannot be used on a receiver method - declare it as a free function"
                    .to_string(),
                span: start,
            });
        }
        if extern_ && section.is_some() {
            return Err(ParseError {
                help: None,
                message: "'section(\"...\")' cannot be written on an 'extern fun': the declaration has no code of its own to place"
                    .to_string(),
                span: start,
            });
        }
        if extern_ && align.is_some() {
            return Err(ParseError {
                help: None,
                message: "'align(N)' cannot be written on an 'extern fun': the declaration has no code of its own to align"
                    .to_string(),
                span: start,
            });
        }

        let return_type = self.parse_return_type()?;
        if let Some(ty) = &return_type {
            if let Some(span) = impl_trait_anywhere(ty) {
                return Err(ParseError {
                    help: None,
                    message: "'impl Trait' is not allowed in return position: write a named type parameter and bound it ('fun f<T: Trait>(...) => T') when the caller needs a concrete type, or return the owning value ('Box<dyn Trait>')".to_string(),
                    span,
                });
            }
        }

        // `where T: Name` is the other spelling of a bound (A-12 slice 2), and
        // `where T::Item: Name` bounds an associated type (MISSING-Ve8Nc step 5).
        let mut projection_bounds: Vec<ProjectionBound> = Vec::new();
        self.parse_where_clause(&type_params, &mut type_param_bounds, &mut projection_bounds, "function")?;

        let body = if signature {
            // A trait method with no body is a signature; one written with `=`
            // or `{ ... }` is a default method (A-12 slice 3).
            if self.check(&TokenKind::Eq) {
                let eq_span = self.advance().span;
                if return_type.is_none() {
                    return Err(ParseError {
                        help: None,
                        message: "expression-bodied function requires a '=> T' return type"
                            .to_string(),
                        span: eq_span,
                    });
                }
                FunctionBody::Expr(self.parse_expr()?)
            } else if self.check(&TokenKind::LBrace) {
                FunctionBody::Block(self.parse_block()?)
            } else {
                FunctionBody::Signature
            }
        } else if extern_ {
            // No body and no `=`: the C is supplied by the compiler. A
            // written body would be dead code, so say so rather than ignore
            // it.
            if self.check(&TokenKind::Eq) || self.check(&TokenKind::LBrace) {
                return Err(ParseError {
                    help: None,
                    message: "an 'extern fun' declaration has no body - the compiler supplies it"
                        .to_string(),
                    span: self.peek().span,
                });
            }
            FunctionBody::Extern
        } else if self.check(&TokenKind::Eq) {
            let eq_span = self.advance().span;
            if return_type.is_none() {
                return Err(ParseError {
                    help: None,
                    message: "expression-bodied function requires a '=> T' return type"
                        .to_string(),
                    span: eq_span,
                });
            }
            FunctionBody::Expr(self.parse_expr()?)
        } else if self.check(&TokenKind::LBrace) {
            FunctionBody::Block(self.parse_block()?)
        } else {
            return Err(ParseError {
                help: self.foreign_hint_at(self.pos),
                message: format!("expected '=' or '{{', found {}", self.peek_kind().describe()),
                span: self.peek().span,
            });
        };

        let end = match &body {
            FunctionBody::Block(b) => b.span,
            FunctionBody::Expr(e) => e.span(),
            // A declaration with no body ends where its signature does: at
            // the return type if one was written, otherwise at the last
            // parameter's type.
            FunctionBody::Extern | FunctionBody::Signature => return_type
                .as_ref()
                .map(|t| t.span)
                .or_else(|| params.last().map(|p| p.ty.span))
                .unwrap_or(start),
        };
        Ok(FunctionDecl {
            name,
            is_pub,
            type_params,
            type_param_bounds,
            projection_bounds,
            receiver_bounds: Vec::new(),
            const_params,
            trait_impl: None,
            extern_,
            unsafe_,
            must_use: false,
            variadic,
            naked,
            interrupt: false,
            receiver,
            params,
            return_type,
            body,
            module: String::new(),
            body_module: None,
            section,
            align,
            span: start.to(end),
        })
    }

    pub(crate) fn parse_type(&mut self) -> PResult<TypeExpr> {
        self.nest()?;
        let r = self.parse_type_inner();
        self.depth -= 1;
        r
    }

    pub(crate) fn parse_type_inner(&mut self) -> PResult<TypeExpr> {
        // A trait object is `&dyn Name` (A-12 slice 4) or `Box<dyn Name>`
        // (slice 7). `dyn` is not a keyword, so a bare `dyn Name` is recognized
        // here, where two names in a row can only mean this, and *marked* on
        // the `TypeExpr`: `infer.rs` accepts the mark only as the argument of a
        // `Box` and refuses a bare `dyn Name` everywhere else, which is what
        // makes the message name the position. `impl Name` in a type position
        // is the other spelling of the bound, refused for the same reason.
        if matches!(self.peek_kind(), TokenKind::Ident(n) if n == "dyn")
            && matches!(self.peek2_kind(), TokenKind::Ident(_))
        {
            let start = self.advance().span;
            let (first, first_span) = self.expect_ident()?;
            let mut name = first;
            let mut span = start.to(first_span);
            while self.check(&TokenKind::Dot) {
                self.advance();
                let (part, part_span) = self.expect_ident()?;
                name.push('.');
                name.push_str(&part);
                span = span.to(part_span);
            }
            // `Box<dyn A + B>` is refused where the `+` is: a vtable names one
            // trait, and the trait that combines two is a declared trait with
            // `A` and `B` as supertraits.
            if self.check(&TokenKind::Plus) {
                return Err(ParseError {
                    help: None,
                    message: format!(
                        "`dyn` names one trait: write `dyn {name}`, and make a trait that combines them (`trait Both: A + B {{ ... }}`) when both sets of methods are wanted"
                    ),
                    span: self.peek().span,
                });
            }
            return Ok(TypeExpr {
                name: String::new(),
                tuple: None,
                args: Vec::new(),
                array_elem: None,
                assoc: None,
                array_len: None,
                amp: false,
                amp_mut: false,
                dyn_trait: Some(name),
                scoped: false,
                fn_params: None,
                fn_ret: None,
                const_expr: None,
                impl_trait: None,
                bound_eq: Vec::new(),
                span,
            });
        }
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
        // `&dyn Name` / `&mut dyn Name`: the one place `&mut` may be written in
        // a type position (A-12 slice 4). Everywhere else an exclusive borrow
        // is `mut T` on a parameter, and `&mut T` keeps its refusal.
        if let Some(amp) = amp_start {
            let amp_mut = if self.check(&TokenKind::Mut) {
                self.advance();
                true
            } else {
                false
            };
            if matches!(self.peek_kind(), TokenKind::Ident(n) if n == "dyn") {
                self.advance();
                let (first, first_span) = self.expect_ident()?;
                let mut name = first;
                let mut span = amp.to(first_span);
                while self.check(&TokenKind::Dot) {
                    self.advance();
                    let (part, part_span) = self.expect_ident()?;
                    name.push('.');
                    name.push_str(&part);
                    span = span.to(part_span);
                }
                return Ok(TypeExpr {
                    name: String::new(),
                    tuple: None,
                    args: Vec::new(),
                    array_elem: None,
                    assoc: None,
                    array_len: None,
                    amp: true,
                    amp_mut,
                    dyn_trait: Some(name),
                    scoped: false,
                    fn_params: None,
                    fn_ret: None,
                    const_expr: None,
                    impl_trait: None,
                    bound_eq: Vec::new(),
                    span,
                });
            }
            if amp_mut {
                return Err(ParseError {
                    help: None,
                    message: "'&mut' is only written before 'dyn' in a type: a parameter writes 'mut T' for an exclusive borrow".to_string(),
                    span: self.peek().span,
                });
            }
        }
        // `impl Name` in a parameter position (A-20): the sugar for a fresh
        // bounded type parameter. The parser records it and the declaration
        // desugars it, so it never reaches type resolution from a `fun`; any
        // other position (`let`, a field, a return) is refused in `infer.rs`.
        // A leading `&` is kept, since `&impl Name` is the borrowed spelling
        // of the same parameter.
        if self.check(&TokenKind::Impl) {
            let start = self.advance().span;
            let (trait_name, tspan) = self.parse_dotted_name()?;
            if self.check(&TokenKind::Lt) {
                return Err(ParseError {
                    help: None,
                    message: format!(
                        "'impl {trait_name}' cannot take the trait's own arguments here: write a named type parameter bounded by the trait ('fun f<T: {trait_name}<Arg>>(x: T)')"
                    ),
                    span: self.peek().span,
                });
            }
            let span = amp_start.map(|a| a.to(tspan)).unwrap_or_else(|| start.to(tspan));
            return Ok(TypeExpr {
                name: String::new(),
                tuple: None,
                args: Vec::new(),
                array_elem: None,
                assoc: None,
                array_len: None,
                amp: amp_start.is_some(),
                amp_mut: false,
                dyn_trait: None,
                scoped: false,
                fn_params: None,
                fn_ret: None,
                const_expr: None,
                impl_trait: Some(trait_name),
                bound_eq: Vec::new(),
                span,
            });
        }
        // A function type (A-4): `fun(i32, String) => bool`. `fun` is a
        // keyword, so this can never be an ordinary type name. A closure type
        // (A-4 phase E) is the same shape under the `closure` keyword:
        // `closure(i32) => i32` is a function value that owns an environment.
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
                assoc: None,
                array_len: Some(Box::new(len)),
                amp: amp_start.is_some(),
                amp_mut: false,
                dyn_trait: None,
                scoped: false,
                fn_params: None,
                fn_ret: None,
                const_expr: None,
                impl_trait: None,
                bound_eq: Vec::new(),
                span: amp_start.map(|a| a.to(close)).unwrap_or_else(|| open.to(close)),
            });
        }
        // A tuple type (MISSING-z2bB9): `(A, B)`, two or more element types.
        // A tuple is a record with positional fields (`design/syntax.md (kelp wiki)` 3), so
        // this is a *type* like any other. `(T)` is not a tuple: it is the
        // parenthesized type `T`. `()` is the type of the absent value, the
        // same type as `unit` (Gitea #223), so `Result<(), String>` and
        // `Result<unit, String>` are one type; it is spelled as `unit` here.
        if self.check(&TokenKind::LParen) {
            let open = self.advance().span;
            if self.check(&TokenKind::RParen) {
                let close = self.advance().span;
                return Ok(TypeExpr {
                    name: "unit".to_string(),
                    tuple: None,
                    args: Vec::new(),
                    array_elem: None,
                    assoc: None,
                    array_len: None,
                    amp: amp_start.is_some(),
                    amp_mut: false,
                    dyn_trait: None,
                    scoped: false,
                    fn_params: None,
                    fn_ret: None,
                    const_expr: None,
                    impl_trait: None,
                    bound_eq: Vec::new(),
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
                assoc: None,
                array_len: None,
                amp: amp_start.is_some(),
                amp_mut: false,
                dyn_trait: None,
                scoped: false,
                fn_params: None,
                fn_ret: None,
                const_expr: None,
                impl_trait: None,
                bound_eq: Vec::new(),
                span,
            });
        }
        if self.check(&TokenKind::Fun) {
            let start = self.advance().span;
            return self.parse_fn_type(start, amp_start, "fun", false);
        }
        // `scoped fun(T) => U` - a *contextual* modifier. `scoped` is read as
        // one only when the next token is `fun`, so an ordinary type or
        // binding named `scoped` is untouched, and the modifier never has a
        // keyword that could break an existing program.
        if matches!(self.peek_kind(), TokenKind::Ident(n) if n == "scoped")
            && matches!(self.peek2_kind(), TokenKind::Fun)
        {
            let start = self.advance().span;
            self.advance();
            return self.parse_fn_type(start, amp_start, "fun", true);
        }
        if self.check(&TokenKind::Closure) {
            let start = self.advance().span;
            return self.parse_fn_type(start, amp_start, "closure", false);
        }
        let (first, first_span) = self.expect_ident()?;
        // A dotted name is a module-qualified reference to an imported item
        // (`circle.Circle`) - see `resolve.rs`, which rewrites it to the
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
        // `I::Item` / `Self::Item` (A-12 slice 5): an associated type
        // projected from the name just read. A generic associated type is
        // refused here, where the `::` says this is the shape.
        let assoc = if self.check(&TokenKind::ColonColon) {
            self.advance();
            let (item, ispan) = self.expect_ident()?;
            span = span.to(ispan);
            if self.check(&TokenKind::Lt) {
                return Err(ParseError {
                    help: None,
                    message: "a generic associated type is not in this slice: 'type Item' declares a type, not a parameterized one, so write the type the impl binds without an argument list"
                        .to_string(),
                    span: self.peek().span,
                });
            }
            Some(item)
        } else {
            None
        };
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
                assoc,
                array_len: None,
                amp: amp_start.is_some(),
                amp_mut: false,
                dyn_trait: None,
                scoped: false,
                fn_params: None,
                fn_ret: None,
                const_expr: None,
                impl_trait: None,
                bound_eq: Vec::new(),
                span: amp_start.map(|a| a.to(span)).unwrap_or(span),
            });
        }
        Ok(TypeExpr {
            name,
            tuple: None,
            args: Vec::new(),
            array_elem: None,
            assoc,
            array_len: None,
            amp: amp_start.is_some(),
            amp_mut: false,
            dyn_trait: None,
            scoped: false,
            fn_params: None,
            fn_ret: None,
            const_expr: None,
            impl_trait: None,
            bound_eq: Vec::new(),
            span: amp_start.map(|a| a.to(span)).unwrap_or(span),
        })
    }
}
