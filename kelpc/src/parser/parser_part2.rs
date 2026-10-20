use super::*;

impl Parser {
    /// `trait Name { fun Self.m(...) => T ... }` (A-12 slice 2). The body is
    /// methods only: an associated item is refused here, where the reason can
    /// name the follow-up. A method with a body is a default (A-12 slice 3).
    pub(crate) fn parse_trait_decl(&mut self, is_pub: bool) -> PResult<TraitDecl> {
        let start = self.expect(TokenKind::Trait, "'trait'")?.span;
        let (name, name_span) = self.expect_ident()?;
        if name == "Drop" {
            return Err(ParseError {
                help: None,
                message: "the trait 'Drop' is built in: a type's destructor is written 'fun mut T.drop()', not through a trait"
                    .to_string(),
                span: name_span,
            });
        }
        if name == "Self" {
            return Err(ParseError {
                help: None,
                message: "'Self' is the implementing type inside a 'trait' or an 'impl', so it cannot be a trait's own name"
                    .to_string(),
                span: name_span,
            });
        }
        if crate::types::Ty::from_name(&name).is_some() {
            return Err(ParseError {
                help: None,
                message: format!(
                    "'{name}' is a builtin type, so it cannot be a trait's name"
                ),
                span: name_span,
            });
        }
        let name_params = self.parse_type_params()?;
        if let Some(p) = name_params.iter().find(|p| p.const_ty.is_some()) {
            return Err(ParseError {
                help: None,
                message: format!(
                    "a trait cannot take a const parameter ('const {}'): a const parameter is a value on a type or a function (A-20), and a trait is implemented for a type",
                    p.name
                ),
                span: p.span,
            });
        }
        let type_params: Vec<String> = name_params.iter().map(|p| p.name.clone()).collect();
        let type_param_bounds: Vec<Vec<TypeExpr>> =
            name_params.into_iter().map(|p| p.bounds).collect();
        // `trait B: A + C` (A-12 slice 3): the supertraits, resolved in the
        // trait's own module exactly as a bound on a function's type parameter
        // is. A cycle is refused by `infer.rs`, where the whole trait table is
        // in hand.
        let supertraits = if self.check(&TokenKind::Colon) {
            self.advance();
            self.parse_plain_bounds("a supertrait")?
        } else {
            Vec::new()
        };
        self.expect(TokenKind::LBrace, "'{' to open the trait's body")?;
        let mut methods = Vec::new();
        let mut assoc_types: Vec<AssocTypeDecl> = Vec::new();
        self.skip_newlines();
        while !self.check(&TokenKind::RBrace) {
            // `type Item` / `type Item: Named` (A-12 slice 5). `type` is not a
            // keyword, so it is an identifier here and the next token being a
            // name is what makes this the declaration.
            if matches!(self.peek_kind(), TokenKind::Ident(n) if n == "type")
                && matches!(self.peek2_kind(), TokenKind::Ident(_))
            {
                self.advance();
                let (name, name_span) = self.expect_ident()?;
                if self.check(&TokenKind::Lt) {
                    return Err(ParseError {
                        help: None,
                        message: "a generic associated type is not in this slice: 'type Item' declares a plain type, so parameters wait for a later slice".to_string(),
                        span: self.peek().span,
                    });
                }
                let bounds = if self.check(&TokenKind::Colon) {
                    self.advance();
                    self.parse_plain_bounds("an associated type's bound")?
                } else {
                    Vec::new()
                };
                if self.check(&TokenKind::Eq) {
                    return Err(ParseError {
                        help: None,
                        message: "a default associated type is not in this slice: write 'type Item' in the trait and bind it in every 'impl' ('type Item = i32'), because a default type would let an impl inherit a type it never names".to_string(),
                        span: self.peek().span,
                    });
                }
                if assoc_types.iter().any(|a| a.name == name) {
                    return Err(ParseError {
                        help: None,
                        message: format!("trait declares 'type {name}' more than once"),
                        span: name_span,
                    });
                }
                assoc_types.push(AssocTypeDecl {
                    name,
                    bounds,
                });
                self.skip_newlines();
                continue;
            }
            if !self.check(&TokenKind::Fun) {
                // Any other item form: a trait body is method signatures and
                // associated types only.
                let what = match self.peek_kind() {
                    TokenKind::Ident(n) => format!(
                        "'{n}' is not part of a trait: a trait body declares methods ('fun Self.m(...)') and associated types ('type Item')"
                    ),
                    TokenKind::Const => "associated consts are not in this slice: a trait declares methods and associated types only"
                        .to_string(),
                    other => format!(
                        "a trait body declares methods and associated types, so 'fun' or 'type' is expected here, found {}", other.describe()
                    ),
                };
                return Err(ParseError {
                    help: None,
                    message: what,
                    span: self.peek().span,
                });
            }
            methods.push(self.parse_function(false, false, false, None, None, true)?);
            self.skip_newlines();
        }
        let end = self.expect(TokenKind::RBrace, "'}' to close the trait's body")?.span;
        Ok(TraitDecl {
            name,
            is_pub,
            type_params,
            type_param_bounds,
            supertraits,
            methods,
            assoc_types,
            module: String::new(),
            span: start.to(end),
        })
    }

    /// `impl Name for Type { fun Self.m(...) { ... } ... }` (A-12 slice 2).
    /// Returns the pairing (what `infer.rs` checks coherence and the exact
    /// signatures against) and the methods, which are ordinary receiver
    /// methods from here on: `resolve.rs` substitutes `Self` with the `for`
    /// type before any later pass sees them.
    pub(crate) fn parse_trait_impl_decl(&mut self) -> PResult<(TraitImplDecl, Vec<FunctionDecl>)> {
        let impl_span = self.expect(TokenKind::Impl, "'impl'")?.span;
        // `impl<T: Bound> Trait for Type<T>`: the impl's own type parameters.
        let impl_params = self.parse_type_params()?;
        if let Some(p) = impl_params.iter().find(|p| p.const_ty.is_some()) {
            return Err(ParseError {
                help: None,
                message: format!("an impl cannot take the const parameter '{}' yet: only type parameters", p.name),
                span: p.span,
            });
        }
        let (trait_name, trait_span) = self.parse_dotted_name()?;
        // `impl From<IoErr> for AppErr` (A-12 slice 6): the trait's arguments,
        // which name the trait instance this block implements.
        let trait_args: Vec<TypeExpr> = if self.check(&TokenKind::Lt) {
            self.parse_type_arg_list()?.0
        } else {
            Vec::new()
        };
        if trait_name == "Drop" || trait_name.ends_with(".Drop") {
            return Err(ParseError {
                help: None,
                message: "the trait 'Drop' is built in, so it cannot be implemented: a type's destructor is written 'fun mut T.drop()' inside its own module"
                    .to_string(),
                span: trait_span,
            });
        }
        self.expect(TokenKind::For, "'for' after the trait name ('impl Name for Type') and not 'impl \"path\"', which imports a module")?;
        let (type_name, type_span) = self.parse_dotted_name()?;
        if type_name == "Self" || type_name.ends_with(".Self") {
            return Err(ParseError {
                help: None,
                message: "the 'for' type names the implementing type, not 'Self': 'Self' is what the methods of this block call it"
                    .to_string(),
                span: type_span,
            });
        }
        let mut impl_bounds: Vec<Vec<TypeExpr>> = impl_params.iter().map(|p| p.bounds.clone()).collect();
        // `impl<T: Bound> Trait for T`: a blanket impl, whose type is the impl's
        // own parameter. It applies to every type that meets the bound.
        let blanket = impl_params.iter().any(|p| p.name == type_name);
        if blanket {
            if self.check(&TokenKind::Lt) {
                return Err(ParseError {
                    help: None,
                    message: format!("a blanket impl names its parameter as the type it implements for: 'impl<{type_name}: Bound> Trait for {type_name}', with no '<...>' after '{type_name}'"),
                    span: type_span,
                });
            }
            if impl_params.len() != 1 {
                return Err(ParseError {
                    help: None,
                    message: format!("a blanket impl has exactly one type parameter, the type it implements for: 'impl<{type_name}: Bound> Trait for {type_name}'"),
                    span: type_span,
                });
            }
            if !trait_args.is_empty() {
                return Err(ParseError {
                    help: None,
                    message: format!("a blanket impl of a trait with type arguments is not supported yet: 'impl<{type_name}: Bound> Trait<...> for {type_name}' has no argument to bind the instance by, so write the trait without its arguments"),
                    span: type_span,
                });
            }
        } else if self.check(&TokenKind::Lt) {
            // `Type<T, U>`: the generic type with the impl's own parameters, in
            // order. Anything else (`Pair<i32>`, `Pair<List<T>>`) would be an
            // impl for one instantiation, which a template cannot express.
            let (args, args_span) = self.parse_type_arg_list()?;
            let same = args.len() == impl_params.len()
                && args.iter().zip(impl_params.iter()).all(|(a, p)| {
                    a.name == p.name && a.args.is_empty() && a.tuple.is_none() && !a.amp && a.array_elem.is_none()
                        && a.fn_params.is_none()
                });
            if impl_params.is_empty() || !same {
                return Err(ParseError {
                    help: None,
                    message: "an impl for a generic type writes the type's parameters, in order, as its own: 'impl<T: Bound> Trait for Type<T>'; an impl for one instantiation ('Type<i32>') is not supported - implement the trait for a non-generic wrapper, or give the type an inherent method".to_string(),
                    span: args_span,
                });
            }
        } else if !impl_params.is_empty() {
            return Err(ParseError {
                help: None,
                message: "'impl<...>' needs the generic type it is for: 'impl<T: Bound> Trait for Type<T>', or a blanket impl 'impl<T: Bound> Trait for T' with the parameter as the type".to_string(),
                span: type_span,
            });
        }
        // `where T: A + B` after the header: more bounds on the impl's parameters,
        // and `where T::Item: B` on an associated type projected from one.
        let impl_param_names: Vec<String> = impl_params.iter().map(|p| p.name.clone()).collect();
        let mut impl_projections: Vec<ProjectionBound> = Vec::new();
        self.parse_where_clause(&impl_param_names, &mut impl_bounds, &mut impl_projections, "impl")?;
        self.expect(TokenKind::LBrace, "'{' to open the impl's body")?;
        let mut methods = Vec::new();
        let mut names = Vec::new();
        let mut assoc_types: Vec<AssocTypeBinding> = Vec::new();
        self.skip_newlines();
        while !self.check(&TokenKind::RBrace) {
            // `type Item = i32` (A-12 slice 5). `type` is not a keyword, so it
            // is an identifier and the name after it is what makes this the
            // binding.
            if matches!(self.peek_kind(), TokenKind::Ident(n) if n == "type")
                && matches!(self.peek2_kind(), TokenKind::Ident(_))
            {
                let start = self.advance().span;
                let (name, name_span) = self.expect_ident()?;
                if self.check(&TokenKind::Colon) {
                    return Err(ParseError {
                        help: None,
                        message: format!(
                            "a bound on 'type {name}' is written in the trait's declaration, not in the impl: write 'type {name}: Named' in the 'trait' body, and bind it here with 'type {name} = <concrete type>'"
                        ),
                        span: self.peek().span,
                    });
                }
                if !self.check(&TokenKind::Eq) {
                    return Err(ParseError {
                        help: None,
                        message: format!(
                            "'type {name}' needs the type the impl binds: write 'type {name} = <concrete type>'"
                        ),
                        span: self.peek().span,
                    });
                }
                self.advance();
                let ty = self.parse_type()?;
                let end = ty.span;
                if assoc_types.iter().any(|a| a.name == name) {
                    return Err(ParseError {
                        help: None,
                        message: format!("'type {name}' is bound more than once in this impl"),
                        span: name_span,
                    });
                }
                assoc_types.push(AssocTypeBinding {
                    name,
                    ty,
                    span: start.to(end),
                });
                self.skip_newlines();
                continue;
            }
            if !self.check(&TokenKind::Fun) {
                return Err(ParseError {
                    help: None,
                    message: "an 'impl' block declares methods ('fun Self.name(...)') and associated types ('type Name = Type'): move anything else to the top level"
                        .to_string(),
                    span: self.peek().span,
                });
            }
            let mut f = self.parse_function(false, false, false, None, None, false)?;
            match &f.receiver {
                Some(r) if r.type_name == "Self" && !r.type_args.is_empty() => {
                    return Err(ParseError {
                        help: None,
                        message: format!(
                            "an 'impl' method cannot take type parameters: the block already names the type it implements for, so write 'fun Self.{}(...)'",
                            f.name
                        ),
                        span: r.span,
                    })
                }
                Some(r) if r.type_name == "Self" => {
                    // A generic impl's methods are template methods of the type.
                    if !impl_params.is_empty() {
                        if let Some(r) = &mut f.receiver {
                            r.type_args = impl_params.iter().map(|p| p.name.clone()).collect();
                        }
                        f.receiver_bounds = impl_bounds.clone();
                    }
                }
                Some(r) => {
                    return Err(ParseError {
                        help: None,
                        message: format!(
                            "an 'impl' method's receiver is written 'Self', not '{}': 'Self' stands for the type the block implements for ('fun Self.{}(...)')",
                            r.type_name, f.name
                        ),
                        span: r.span,
                    })
                }
                None if blanket => {
                    return Err(ParseError {
                        help: None,
                        message: format!(
                            "a blanket impl has no static member yet: '{}' has no receiver to take the type from, so write it as a method ('fun Self.{}(...)')",
                            f.name, f.name
                        ),
                        span: f.span,
                    })
                }
                None => {}
            }
            f.trait_impl = Some(TraitImplSite {
                trait_name: trait_name.clone(),
                trait_args: trait_args.clone(),
                trait_instance: String::new(),
                self_type: type_name.clone(),
            });
            names.push(f.name.clone());
            methods.push(f);
            self.skip_newlines();
        }
        self.expect(TokenKind::RBrace, "'}' to close the impl's body")?;
        Ok((
            TraitImplDecl {
                trait_name,
                trait_args,
                trait_arg_names: Vec::new(),
                trait_instance: String::new(),
                type_name,
                trait_span,
                type_span,
                methods: names,
                assoc_types,
                module: String::new(),
                impl_span,
                type_params: impl_params.iter().map(|p| p.name.clone()).collect(),
                type_param_bounds: impl_bounds,
                projection_bounds: impl_projections,
            },
            methods,
        ))
    }

    pub(crate) fn parse_datac_decl(&mut self, is_pub: bool, repr: Option<Repr>) -> PResult<DatacDecl> {
        let start = self.expect(TokenKind::Datac, "'datac'")?.span;
        let (name, _) = self.expect_ident()?;
        let (type_params, const_params) =
            Parser::no_type_param_bounds(self.parse_type_params()?)?;
        self.expect(TokenKind::LParen, "'('")?;
        let fields = self.parse_field_list()?;
        let end = self.expect(TokenKind::RParen, "')'")?.span;
        Ok(DatacDecl {
            name,
            is_pub,
            type_params,
            const_params,
            fields,
            repr,
            order: 0,
            module: String::new(),
            span: start.to(end),
        })
    }

    /// `derive(eq, hash)` (lang-derive-eq): the names of a derive clause, or
    /// `None` when the next tokens are not one. Only `eq` and `hash` are names;
    /// any other name is refused by name.
    pub(crate) fn parse_derive_clause(&mut self) -> PResult<Option<crate::derive::DeriveRequest>> {
        if !matches!(self.peek_kind(), TokenKind::Ident(n) if n == "derive")
            || !matches!(self.peek2_kind(), TokenKind::LParen)
        {
            return Ok(None);
        }
        let start = self.advance().span;
        self.expect(TokenKind::LParen, "'(' after 'derive'")?;
        let mut names = Vec::new();
        while !self.check(&TokenKind::RParen) {
            let (name, span) = self.expect_ident()?;
            if name != "eq" && name != "hash" {
                return Err(ParseError {
                    help: None,
                    message: format!(
                        "'derive' can derive 'eq' and 'hash', not '{name}': a 'datac' or 'enum' gets '==' and a 'Map' key from those two methods"
                    ),
                    span,
                });
            }
            names.push((name, span));
            if !self.check(&TokenKind::RParen) {
                self.expect(TokenKind::Comma, "',' between the derive names")?;
            }
        }
        let end = self.expect(TokenKind::RParen, "')' after the derive names")?.span;
        if names.is_empty() {
            return Err(ParseError {
                help: None,
                message: "'derive()' names nothing: write 'derive(eq, hash)', or one of the two".to_string(),
                span: start,
            });
        }
        Ok(Some(crate::derive::DeriveRequest {
            type_name: String::new(),
            names,
            span: Span::new(start.start, end.end),
        }))
    }

    /// A `repr(c)` / `repr(packed)` / `repr(align(N))` / `repr(u8)` clause, or
    /// `None` when the cursor is not at one (kernel stage 2). `repr` is a
    /// contextual word, so it only counts when a `(` touches it, exactly as
    /// `unsafe` only counts when `fun` follows.
    pub(crate) fn parse_repr_clause(&mut self) -> PResult<Option<Repr>> {
        if !matches!(self.peek_kind(), TokenKind::Ident(n) if n == "repr")
            || !matches!(self.peek2_kind(), TokenKind::LParen)
        {
            return Ok(None);
        }
        self.advance();
        self.expect(TokenKind::LParen, "'(' after 'repr'")?;
        let (word, word_span) = self.expect_ident()?;
        let repr = match word.as_str() {
            "c" => Repr::C,
            "packed" => Repr::Packed,
            "align" => {
                self.expect(TokenKind::LParen, "'(' after 'repr(align'")?;
                let (n, n_span) = match self.peek_kind().clone() {
                    TokenKind::IntLit(n) => {
                        self.advance();
                        (n, self.tokens[self.pos - 1].span)
                    }
                    other => {
                        return Err(ParseError {
                            help: None,
                            message: format!(
                                "expected an integer literal alignment in 'repr(align(N))', found {}", other.describe()
                            ),
                            span: self.peek().span,
                        })
                    }
                };
                self.expect(TokenKind::RParen, "')' after 'repr(align(N)'")?;
                if !n.is_power_of_two() {
                    return Err(ParseError {
                        help: None,
                        message: format!(
                            "'repr(align({n}))' needs a power of two: 1, 2, 4, 8, 16, ... are the alignments a type can have"
                        ),
                        span: n_span,
                    });
                }
                // The bound is the C compilers': GCC and Clang refuse an
                // `aligned` above 2^28, and the LLVM alignment anchor is a
                // vector of `N` bytes, so a value that does not fit is a
                // refusal here rather than a nonsense type later.
                if n > 1 << 28 {
                    return Err(ParseError {
                        help: None,
                        message: format!(
                            "'repr(align({n}))' is too large: the largest alignment is 268435456"
                        ),
                        span: n_span,
                    });
                }
                Repr::Align(n)
            }
            // `repr(u8)` and its siblings: an integer storage type for an
            // enum tag. Any other word is not a repr this language has.
            other => {
                let is_int = matches!(
                    other,
                    "u8" | "u16" | "u32" | "u64" | "i8" | "i16" | "i32" | "i64"
                );
                if !is_int {
                    return Err(ParseError {
                        help: None,
                        message: format!(
                            "unknown 'repr({other})': the clause is 'c', 'packed', 'align(N)' or an integer type ('u8', 'i8', 'u16', 'i16', 'u32', 'i32', 'u64', 'i64')"
                        ),
                        span: word_span,
                    });
                }
                Repr::Int(other.to_string())
            }
        };
        self.expect(TokenKind::RParen, "')' to close 'repr(...)'")?;
        Ok(Some(repr))
    }

    /// `section("<name>")`, an optional leading clause on a top-level `const`
    /// or `fun` (kernel stage 4), or `None` when the cursor is not at one.
    /// `section` is a contextual word, so it only counts when a `(` touches it,
    /// exactly as `repr` and `unsafe` do.
    pub(crate) fn parse_section_clause(&mut self) -> PResult<Option<String>> {
        if !matches!(self.peek_kind(), TokenKind::Ident(n) if n == "section")
            || !matches!(self.peek2_kind(), TokenKind::LParen)
        {
            return Ok(None);
        }
        self.advance();
        self.expect(TokenKind::LParen, "'(' after 'section'")?;
        let (name, name_span) = match self.peek_kind().clone() {
            TokenKind::StringLit(s) => {
                let span = self.peek().span;
                self.advance();
                (s, span)
            }
            other => {
                return Err(ParseError {
                    help: None,
                    message: format!(
                        "expected a quoted section name in 'section(\"...\")', found {}", other.describe()
                    ),
                    span: self.peek().span,
                })
            }
        };
        self.expect(TokenKind::RParen, "')' to close 'section(...)'")?;
        if name.is_empty() {
            return Err(ParseError {
                help: None,
                message: "'section(\"\")' needs a section name, for example 'section(\".text.boot\")'"
                    .to_string(),
                span: name_span,
            });
        }
        Ok(Some(name))
    }

    /// `section("...")` and `align(N)` are the two clauses of one family on a
    /// `fun` (kernel stages 4 and gap 2), and a program may write either or
    /// both, in either order. Each is consumed at most once; a second one is
    /// the caller's duplicate error.
    pub(crate) fn parse_section_and_align_clauses(&mut self) -> PResult<(Option<String>, Option<u32>)> {
        let mut section = None;
        let mut align = None;
        loop {
            if section.is_none() {
                if let Some(s) = self.parse_section_clause()? {
                    section = Some(s);
                    continue;
                }
            }
            if align.is_none() {
                if let Some(a) = self.parse_align_clause()? {
                    align = Some(a);
                    continue;
                }
            }
            break;
        }
        Ok((section, align))
    }

    /// `align(N)`, an optional leading clause on a top-level `fun` (kernel gap
    /// 2), or `None` when the cursor is not at one. `align` is a contextual
    /// word like `section`, so it only counts when a `(` touches it. `N` is a
    /// power of two up to 4096: an alignment above a page is not one a trap
    /// vector can use, and the bound keeps the LLVM and C attributes in the
    /// range both accept.
    pub(crate) fn parse_align_clause(&mut self) -> PResult<Option<u32>> {
        if !matches!(self.peek_kind(), TokenKind::Ident(n) if n == "align")
            || !matches!(self.peek2_kind(), TokenKind::LParen)
        {
            return Ok(None);
        }
        self.advance();
        self.expect(TokenKind::LParen, "'(' after 'align'")?;
        let (n, n_span) = match self.peek_kind().clone() {
            TokenKind::IntLit(n) => {
                self.advance();
                (n, self.tokens[self.pos - 1].span)
            }
            other => {
                return Err(ParseError {
                    help: None,
                    message: format!(
                        "expected an integer literal alignment in 'align(N)', found {}", other.describe()
                    ),
                    span: self.peek().span,
                })
            }
        };
        self.expect(TokenKind::RParen, "')' to close 'align(...)'")?;
        if !n.is_power_of_two() || n > 4096 {
            return Err(ParseError {
                help: None,
                message: format!(
                    "'align({n})' needs a power of two up to 4096: 1, 2, 4, 8, ... 4096 are the alignments a function can have"
                ),
                span: n_span,
            });
        }
        Ok(Some(n as u32))
    }

    /// Whether the cursor is at a function declaration: `fun`, `extern fun` or
    /// `unsafe fun`. Used to decide whether a `section(...)` clause has an item
    /// it applies to.
    pub(crate) fn at_function_decl(&self) -> bool {
        matches!(self.peek_kind(), TokenKind::Fun)
            || (matches!(self.peek_kind(), TokenKind::Extern)
                && matches!(self.peek2_kind(), TokenKind::Fun))
            || (matches!(self.peek_kind(), TokenKind::Ident(n) if n == "unsafe" || n == "naked" || n == "must_use")
                && matches!(self.peek2_kind(), TokenKind::Fun))
    }

    /// `extern type Widget` or `extern type Window: Widget`: an opaque native
    /// handle (gap 1). `type` is not a lexer keyword, so the caller has already
    /// seen the two tokens and this reads the name and an optional parent.
    pub(crate) fn parse_extern_type_decl(&mut self, is_pub: bool) -> PResult<ExternTypeDecl> {
        let start = self.expect(TokenKind::Extern, "'extern'")?.span;
        self.advance();
        let (name, name_span) = self.expect_ident()?;
        let mut span = start.to(name_span);
        let parent = if self.check(&TokenKind::Colon) {
            self.advance();
            let (first, first_span) = self.expect_ident()?;
            let mut parent = first;
            let mut pend = first_span;
            while self.check(&TokenKind::Dot) {
                self.advance();
                let (part, part_span) = self.expect_ident()?;
                parent.push('.');
                parent.push_str(&part);
                pend = part_span;
            }
            span = span.to(pend);
            Some(parent)
        } else {
            None
        };
        Ok(ExternTypeDecl {
            name,
            parent,
            is_pub,
            module: String::new(),
            span,
        })
    }

    /// `<T, U>` after a declared type name (A-2). Returns an empty vector when
    /// no `<` was written, so this is a no-op for every ordinary declaration.
    /// A `<...>` parameter list, one `(name, bounds)` pair per parameter
    /// (A-2, with the bound half from A-12 slice 2): `fun f<T: A + B>` gives
    /// `("T", [A, B])`, and a parameter with no bound gets an empty list, so
    /// the two halves stay in step by construction.
    pub(crate) fn parse_type_params(&mut self) -> PResult<Vec<TypeParamParsed>> {
        if !self.check(&TokenKind::Lt) {
            return Ok(Vec::new());
        }
        let open = self.advance().span;
        if self.check(&TokenKind::Gt) {
            return Err(ParseError {
                help: None,
                message: "a type parameter list needs at least one parameter ('<T>')".to_string(),
                span: open.to(self.peek().span),
            });
        }
        let mut params: Vec<TypeParamParsed> = Vec::new();
        loop {
            // `const N: usize` (A-20): a compile-time value parameter rather
            // than a type. `const` is a keyword, so the two forms never
            // collide.
            if self.check(&TokenKind::Const) {
                self.advance();
                let (param, pspan) = self.expect_ident()?;
                if params.iter().any(|p| p.name == param) {
                    return Err(ParseError {
                        help: None,
                        message: format!("parameter '{param}' is declared more than once"),
                        span: pspan,
                    });
                }
                self.expect(TokenKind::Colon, "': then the integer type of the const parameter, as in 'const N: usize'")?;
                let (ty, _) = self.expect_ident()?;
                if self.check(&TokenKind::Lt) || self.check(&TokenKind::Colon) {
                    return Err(ParseError {
                        help: None,
                        message: format!(
                            "a const parameter's type is one of 'usize', 'u8'..'u64', 'i32', 'i64', 'bool' or 'char', so '{ty}' cannot take arguments or bounds"
                        ),
                        span: self.peek().span,
                    });
                }
                params.push(TypeParamParsed {
                    name: param,
                    bounds: Vec::new(),
                    const_ty: Some(ty),
                    span: pspan,
                });
            } else {
                let (param, pspan) = self.expect_ident()?;
                if params.iter().any(|p| p.name == param) {
                    return Err(ParseError {
                        help: None,
                        message: format!("type parameter '{param}' is declared more than once"),
                        span: pspan,
                    });
                }
                // `T: Name`: the traits the parameter must implement. A parameter
                // with no `:` has an empty list, which is every parameter written
                // before bounds existed.
                let bounds = if self.check(&TokenKind::Colon) {
                    self.advance();
                    self.parse_bounds()?
                } else {
                    Vec::new()
                };
                params.push(TypeParamParsed {
                    name: param,
                    bounds,
                    const_ty: None,
                    span: pspan,
                });
            }
            if self.check(&TokenKind::Comma) {
                self.advance();
                continue;
            }
            break;
        }
        self.expect(TokenKind::Gt, "'>' to close the type parameter list")?;
        Ok(params)
    }

    /// One or more trait names separated by `+` (A-12 slice 2): `Name`,
    /// `alias.Name`, `A + B`. A bound names a trait and nothing else, so no
    /// argument list and no `&` is accepted.
    pub(crate) fn parse_bounds(&mut self) -> PResult<Vec<TypeExpr>> {
        self.parse_bounds_at(None)
    }

    /// `parse_bounds` for a site that takes plain trait names only (a
    /// supertrait, an associated type's bound): an equality constraint there is
    /// refused where it is written, naming the site.
    pub(crate) fn parse_plain_bounds(&mut self, site: &str) -> PResult<Vec<TypeExpr>> {
        self.parse_bounds_at(Some(site))
    }

    /// One or more bounds separated by `+`. `Name<Arg, Assoc = Type>` (MISSING-
    /// Ve8Nc step 5) is a trait instance whose arguments are types and whose
    /// `Assoc = Type` items are equality constraints on its associated types;
    /// `site` refuses the constraints where a plain name is required.
    pub(crate) fn parse_bounds_at(&mut self, site: Option<&str>) -> PResult<Vec<TypeExpr>> {
        let mut out = Vec::new();
        loop {
            // A negative bound (`T: !Name`) is excluded from this design
            // (`kelp issue #243` 6): refused by name where it starts.
            if self.check(&TokenKind::Bang) {
                return Err(ParseError {
                    help: None,
                    message: "a negative bound ('!Name') is excluded from generic trait impls (plan/generic-trait-impls.md section 6): a bound says a type implements a trait, and no bound says that it does not".to_string(),
                    span: self.peek().span,
                });
            }
            let (name, span) = self.parse_dotted_name()?;
            // `Name<Arg>` (A-12 slice 6): a bound on a trait instance. The
            // trait's own arguments are a comma-separated type list, exactly
            // as an impl line writes them.
            let mut bounds_span = span;
            let mut eqs: Vec<(String, TypeExpr)> = Vec::new();
            let args = if self.check(&TokenKind::Lt) {
                let open = self.advance().span;
                let mut args = Vec::new();
                loop {
                    if matches!(self.peek_kind(), TokenKind::Ident(_))
                        && matches!(self.peek2_kind(), TokenKind::Eq)
                    {
                        let (item, item_span) = self.expect_ident()?;
                        if let Some(site) = site {
                            return Err(ParseError {
                                help: None,
                                message: format!(
                                    "an equality constraint ('{name}<{item} = ...>') is not in this slice on {site}: an equality constraint is written on a function's or an impl's type parameter ('fun f<I: {name}<{item} = i64>>')"
                                ),
                                span: item_span,
                            });
                        }
                        self.expect(TokenKind::Eq, "'=' after the associated type in an equality constraint")?;
                        let ty = self.parse_type()?;
                        eqs.push((item, ty));
                    } else {
                        args.push(self.parse_type()?);
                    }
                    if self.check(&TokenKind::Comma) {
                        self.advance();
                        continue;
                    }
                    break;
                }
                let close = self.expect(TokenKind::Gt, "'>' to close the trait argument list")?.span;
                bounds_span = open.to(close);
                args
            } else {
                Vec::new()
            };
            let mut bound = TypeExpr::named(name, bounds_span);
            bound.args = args;
            bound.bound_eq = eqs;
            bound.span = bounds_span;
            out.push(bound);
            if self.check(&TokenKind::Plus) {
                self.advance();
                continue;
            }
            break;
        }
        Ok(out)
    }
}
