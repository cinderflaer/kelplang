use super::*;

impl<'a> ConstEvaluator<'a> {
    pub(crate) fn new(
        const_fns: &'a ConstFns,
        structs: &'a HashMap<String, StructDef>,
        enums: &'a HashMap<String, EnumDef>,
        env: &'a TypeEnv,
        consts: &'a HashMap<String, ConstInfo>,
    ) -> ConstEvaluator<'a> {
        ConstEvaluator {
            const_fns,
            structs,
            enums,
            env,
            consts,
            steps: std::cell::Cell::new(0),
            depth: std::cell::Cell::new(0),
        }
    }

    /// The type-namespace view of one module, so a parameter's or a return
    /// value's written type resolves where it was written.
    pub(crate) fn type_scope<'s>(
        &'s self,
        prefix: &'s str,
        aliases: &'s HashMap<String, String>,
    ) -> TypeScope<'s> {
        TypeScope {
            structs: self.structs,
            enums: self.enums,
            env: self.env.clone(),
            params: &EMPTY_PARAMS,
            prefix,
            aliases,
            later_datacs: None,
            in_progress: None,
            consts: Some(self.consts),
            const_fns: Some(self.const_fns),
            handles: &self.env.handles,
        }
    }

    /// Charges one step against the budget.
    pub(crate) fn tick(&self, span: Span) -> Result<()> {
        self.steps.set(self.steps.get() + 1);
        if self.steps.get() > CONST_EVAL_STEPS {
            return Err(TypeError {
                help: None,
                message: format!(
                    "this constant expression did not finish within {CONST_EVAL_STEPS} steps: a 'const' is folded while the compiler runs, so a 'fun' it calls has to terminate on the values it is given"
                ),
                span,
            });
        }
        Ok(())
    }

    /// Folds a `const` declaration's initializer into the literal a use site
    /// substitutes.
    pub(crate) fn fold_const(
        &self,
        name: &str,
        expr: &ast::Expr,
        declared: &Ty,
        prefix: &str,
        aliases: &HashMap<String, String>,
    ) -> Result<tir::Expr> {
        let mut scope = ConstScope::new(prefix.to_string(), aliases.clone());
        let (value, found) = self.eval(expr, Some(declared), &mut scope)?;
        literal_for(&value, declared).map_err(|_| TypeError {
            help: None,
            message: format!(
                "const '{name}' is declared as '{}' but its initializer is a '{}'",
                declared.display(),
                found.display()
            ),
            span: expr.span(),
        })
    }

    /// Folds the `N` of an array type or of a repeat literal (A-18): the one
    /// number a type or a literal needs, evaluated as a `usize` because a
    /// length is a count.
    pub(crate) fn array_len(
        &self,
        expr: &ast::Expr,
        prefix: &str,
        aliases: &HashMap<String, String>,
    ) -> Result<u64> {
        let mut scope = ConstScope::new(prefix.to_string(), aliases.clone());
        let (value, ty) = self.eval(expr, Some(&Ty::Usize), &mut scope)?;
        match value {
            ConstValue::Int(n) if n > 0 => Ok(n as u64),
            ConstValue::Int(n) => Err(TypeError {
                help: None,
                message: format!(
                    "an array length has to be at least 1, and this one is {n}: v1 has no zero-length form ('[T; 0]' would be a C array with no elements, which is not standard C)"
                ),
                span: expr.span(),
            }),
            _ => Err(TypeError {
                help: None,
                message: format!(
                    "an array length has to be an integer constant, and this one is a '{}' value",
                    ty.display()
                ),
                span: expr.span(),
            }),
        }
    }

    /// Folds one written expression to a value of `expected`'s type, or to its
    /// own default type when the context names none.
    pub(crate) fn eval(
        &self,
        expr: &ast::Expr,
        expected: Option<&Ty>,
        scope: &mut ConstScope,
    ) -> Result<ConstResult> {
        self.tick(expr.span())?;
        if scope.halted() {
            return Ok((ConstValue::Unit, Ty::Unit));
        }
        match expr {
            ast::Expr::IntLit(v, span) => Ok(match expected {
                Some(t) if t.is_float() => (ConstValue::Float(*v as f64), t.clone()),
                Some(t) if t.is_integer() => {
                    check_int_literal(*v as i128, t, false, *span)?;
                    (ConstValue::Int(*v as i128), t.clone())
                }
                Some(Ty::Char) => (ConstValue::Int(*v as i128), Ty::Char),
                _ => {
                    check_int_literal(*v as i128, &Ty::Int, true, *span)?;
                    (ConstValue::Int(*v as i128), Ty::Int)
                }
            }),
            ast::Expr::CharLit(v, span) => Ok(match expected {
                Some(t) if t.is_integer() => {
                    if !char_fits(*v, t) {
                        return Err(TypeError {
                            help: None,
                            message: format!(
                                "the character '{}' (U+{:04X}) does not fit in '{}'",
                                printable_codepoint(*v),
                                v,
                                t.display()
                            ),
                            span: *span,
                        });
                    }
                    (ConstValue::Int(*v as i128), t.clone())
                }
                _ => (ConstValue::Char(*v), Ty::Char),
            }),
            ast::Expr::ByteLit(v, span) => {
                if let Some(t) = expected {
                    if !matches!(t, Ty::U8) {
                        return Err(TypeError {
                            help: None,
                            message: format!(
                                "a byte literal is a 'u8', not a '{}': write it as a character literal when it should take this type",
                                t.display()
                            ),
                            span: *span,
                        });
                    }
                }
                Ok((ConstValue::Int(*v as i128), Ty::U8))
            }
            ast::Expr::FloatLit(v, _) => Ok(match expected {
                Some(t) if t.is_float() => (ConstValue::Float(*v), t.clone()),
                _ => (ConstValue::Float(*v), Ty::F64),
            }),
            ast::Expr::BoolLit(b, _) => Ok((ConstValue::Bool(*b), Ty::Bool)),
            ast::Expr::StringLit(s, _) => Ok((ConstValue::Str(s.clone()), Ty::Str)),
            ast::Expr::Ident(name, span) => {
                if let Some(v) = scope.get(name) {
                    return Ok(v.clone());
                }
                let key = format!("{}{}", scope.prefix, name);
                if let Some(c) = self.consts.get(&key).and_then(|c| value_of(&c.value)) {
                    return Ok(c);
                }
                if self.const_fns.fns.contains_key(&key) {
                    return Err(TypeError {
                        help: None,
                        message: format!(
                            "'{name}' is a function, not a value: call it, and a constant expression can call it too when its body is one a constant can evaluate"
                        ),
                        span: *span,
                    });
                }
                Err(TypeError {
                    help: None,
                    message: format!(
                        "'{name}' is not a constant here: a compile-time expression (an array length, or a 'const' initializer) can only use literals, 'const's declared before it, and the locals of the 'fun' it is evaluating; a 'val' from the enclosing function is not one"
                    ),
                    span: *span,
                })
            }
            // `math.PI`: another module's constant, reached through an alias.
            ast::Expr::FieldAccess { base, field, span } => {
                let ast::Expr::Ident(alias, _) = base.as_ref() else {
                    return Err(self.unsupported(expr.span(), "a field of a value"));
                };
                let Some(prefix) = scope.aliases.get(alias).cloned() else {
                    return Err(TypeError {
                        help: None,
                        message: format!(
                            "'{alias}' is not a module alias, so '{alias}.{field}' is a field of a value; a constant expression has no values to read fields from"
                        ),
                        span: *span,
                    });
                };
                let key = format!("{prefix}{field}");
                if self.consts.contains_key(&key) && !exported(&key, self.env) {
                    return Err(private_item_error(alias, field, *span));
                }
                match self.consts.get(&key).and_then(|c| value_of(&c.value)) {
                    Some(c) => Ok(c),
                    None => Err(TypeError {
                        help: None,
                        message: format!(
                            "module '{alias}' has no constant '{field}' declared before this one"
                        ),
                        span: *span,
                    }),
                }
            }
            ast::Expr::Unary { op, expr: sub, span } => {
                // The sign-and-literal pair `check_expr_inner` handles the same
                // way: under `-`, the value is the negated literal, not the
                // literal the lexer read.
                if *op == ast::UnOp::Neg {
                    if let ast::Expr::IntLit(v, lit_span) = &**sub {
                        if expected.map_or(true, |t| t.is_integer()) {
                            self.tick(*lit_span)?;
                            let (ty, defaulted) = int_lit_ty(expected);
                            check_int_literal(-(*v as i128), &ty, defaulted, *lit_span)?;
                            return self.fold_unary(*op, ConstValue::Int(*v as i128), &ty, *span);
                        }
                    }
                }
                let (v, ty) = self.eval(sub, expected, scope)?;
                if scope.halted() {
                    return Ok((ConstValue::Unit, Ty::Unit));
                }
                self.fold_unary(*op, v, &ty, *span)
            }
            ast::Expr::Binary { op, lhs, rhs, span } => self.fold_binary(*op, lhs, rhs, *span, expected, scope),
            ast::Expr::If {
                cond,
                then_block,
                else_block,
                span: _,
            } => {
                let (value, ty) = self.eval(cond, Some(&Ty::Bool), scope)?;
                if scope.halted() {
                    return Ok((ConstValue::Unit, Ty::Unit));
                }
                let ConstValue::Bool(take) = value else {
                    return Err(TypeError {
                        help: None,
                        message: format!(
                            "the condition of an 'if' has to be a 'bool', and this one is a '{}'",
                            ty.display()
                        ),
                        span: cond.span(),
                    });
                };
                if take {
                    return match self.eval_block(then_block, expected, scope)? {
                        ConstFlow::Fell(v) | ConstFlow::Returned(v) => match v {
                            Some(v) => Ok(v),
                            None => Ok((ConstValue::Unit, Ty::Unit)),
                        },
                    };
                }
                match else_block.as_deref() {
                    Some(ast::ElseBranch::Block(b)) => match self.eval_block(b, expected, scope)? {
                        ConstFlow::Fell(v) | ConstFlow::Returned(v) => match v {
                            Some(v) => Ok(v),
                            None => Ok((ConstValue::Unit, Ty::Unit)),
                        },
                    },
                    Some(ast::ElseBranch::If(e)) => self.eval(e, expected, scope),
                    None => Ok((ConstValue::Unit, Ty::Unit)),
                }
            }
            ast::Expr::Block(block) => match self.eval_block(block, expected, scope)? {
                ConstFlow::Fell(v) | ConstFlow::Returned(v) => match v {
                    Some(v) => Ok(v),
                    None => Ok((ConstValue::Unit, Ty::Unit)),
                },
            },
            ast::Expr::Assign { target, value, span } => {
                let ast::Expr::Ident(name, nspan) = target.as_ref() else {
                    return Err(self.unsupported(expr.span(), "an assignment to anything but a local"));
                };
                // The declared type is the binding's, so `x = 1` in an `i64`
                // local is an `i64` assignment and not an `i32` one.
                let declared = scope.get(name).map(|(_, t)| t.clone());
                let v = self.eval(value, declared.as_ref(), scope)?;
                if scope.halted() {
                    return Ok((ConstValue::Unit, Ty::Unit));
                }
                if let Some((_, ty)) = scope.get(name) {
                    if v.1 != *ty {
                        return Err(TypeError {
                            help: None,
                            message: format!(
                                "cannot assign a '{}' to '{name}', which is a '{}'",
                                v.1.display(),
                                ty.display()
                            ),
                            span: *span,
                        });
                    }
                }
                match scope.get_mut(name) {
                    Some(b) if b.mutable => {
                        b.value = v.clone();
                        Ok((ConstValue::Unit, Ty::Unit))
                    }
                    Some(_) => Err(TypeError {
                        help: Some(format!("write var {name} = ... instead if the value has to change.")),
                        message: format!("cannot assign to '{name}': it was declared with 'val'"),
                        span: *nspan,
                    }),
                    None => Err(TypeError {
                        help: Some("declare it with val or var before the assignment.".to_string()),
                        message: format!("undefined local '{name}'"),
                        span: *nspan,
                    }),
                }
            }
            ast::Expr::Call {
                callee,
                type_args,
                args,
                span,
            } => {
                // A layout query is foldable wherever it is written, including
                // an array length and a `const` that names a builtin
                // (kernel stage 2). It is resolved here as well as in
                // `check_layout_query`, because those two positions fold
                // through this evaluator rather than through the expression
                // checker.
                if !type_args.is_empty() {
                    if matches!(callee.as_str(), "size_of" | "align_of" | "offset_of")
                        && !callee.contains('.')
                    {
                        return self.layout_query(callee, type_args, args, *span, scope);
                    }
                    return Err(self.unsupported(expr.span(), "a call with explicit type arguments"));
                }
                let key = format!("{}{}", scope.prefix, callee);
                self.call_fn(&key, callee, args, *span, scope)
            }
            // `math.sqrt(2.0)` reaches here as a method call on a module
            // alias, exactly as `math.PI` does; anything else is either an
            // integer conversion (`n.to_u8()`) or out of reach.
            ast::Expr::MethodCall {
                receiver,
                method,
                args,
                type_args: _,
                span,
            } => {
                if let ast::Expr::Ident(alias, _) = receiver.as_ref() {
                    if let Some(prefix) = scope.aliases.get(alias).cloned() {
                        let key = format!("{prefix}{method}");
                        if self.const_fns.fns.contains_key(&key) && !exported(&key, self.env) {
                            return Err(private_item_error(alias, method, *span));
                        }
                        return self.call_fn(&key, &format!("{alias}.{method}"), args, *span, scope);
                    }
                }
                if let Some(to) = method.strip_prefix("to_") {
                    if !args.is_empty() {
                        return Err(self.unsupported(expr.span(), "a conversion call with arguments"));
                    }
                    let (value, from) = self.eval(receiver, None, scope)?;
                    if scope.halted() {
                        return Ok((ConstValue::Unit, Ty::Unit));
                    }
                    return self.convert(&value, &from, to, method, *span);
                }
                Err(self.unsupported(expr.span(), "a method call"))
            }
            ast::Expr::ArrayLit { .. } | ast::Expr::ArrayRepeat { .. } => {
                Err(self.unsupported(expr.span(), "an array value"))
            }
            ast::Expr::TupleLit { elems, .. } => {
                // The declared tuple type, when there is one, gives each
                // position the type a bare literal should take.
                let hinted: Option<Vec<Ty>> = match expected {
                    Some(Ty::Struct(name)) => self
                        .env
                        .instances
                        .borrow()
                        .records
                        .get(name)
                        .map(|def| def.fields.iter().map(|f| f.ty.clone()).collect()),
                    _ => None,
                };
                if hinted.as_ref().is_some_and(|h| h.len() != elems.len()) {
                    return Err(TypeError {
                        help: None,
                        message: format!(
                            "a tuple of {} element(s) is expected here, and this one has {}",
                            hinted.as_ref().map_or(0, |h| h.len()),
                            elems.len()
                        ),
                        span: expr.span(),
                    });
                }
                let mut values = Vec::with_capacity(elems.len());
                for (i, e) in elems.iter().enumerate() {
                    let hint = hinted.as_ref().map(|h| &h[i]);
                    let (v, t) = self.eval(e, hint, scope)?;
                    if scope.halted() {
                        return Ok((ConstValue::Unit, Ty::Unit));
                    }
                    values.push((v, t));
                }
                let ty = tuple_ty(self.env, values.iter().map(|(_, t)| t.clone()).collect());
                Ok((ConstValue::Tuple(values), ty))
            }
            ast::Expr::Template { .. } => Err(self.unsupported(expr.span(), "a string template")),
            ast::Expr::FnLit { .. } => Err(self.unsupported(expr.span(), "a 'fun' literal")),
            ast::Expr::When { .. } => Err(self.unsupported(expr.span(), "a 'when' pattern match")),
            ast::Expr::Try { .. } => Err(self.unsupported(expr.span(), "the '?' operator")),
            ast::Expr::Deref { .. } => Err(self.unsupported(expr.span(), "a dereference")),
            ast::Expr::Ref { .. } => Err(self.unsupported(expr.span(), "a borrow")),
            ast::Expr::Unsafe { .. } => Err(self.unsupported(expr.span(), "an 'unsafe' block")),
            ast::Expr::ManualBlock { .. } => Err(self.unsupported(expr.span(), "a block")),
            ast::Expr::Asm { .. } => Err(self.unsupported(expr.span(), "an 'asm' expression")),
        }
    }

    /// Folds `size_of<T>()`, `align_of<T>()` or `offset_of<T>("field")` while
    /// a `const` initializer or an array length is being evaluated. The type
    /// tables are the same ones the expression checker reads, so the two
    /// answer identically whenever both can answer at all; a `const` is folded
    /// before any `datac` exists, so a query there can only name a builtin.
    pub(crate) fn layout_query(
        &self,
        callee: &str,
        type_args: &[ast::TypeExpr],
        args: &[ast::CallArg],
        span: Span,
        scope: &ConstScope,
    ) -> Result<(ConstValue, Ty)> {
        self.tick(span)?;
        if type_args.len() != 1 {
            return Err(TypeError {
                help: None,
                message: format!(
                    "'{callee}' takes exactly one type argument, written '{callee}<T>()'"
                ),
                span,
            });
        }
        let ty = self
            .type_scope(&scope.prefix, &scope.aliases)
            .resolve(&type_args[0], true)?;
        let field = if callee == "offset_of" {
            if args.len() != 1 {
                return Err(TypeError {
                    help: None,
                    message: "'offset_of' takes exactly one field name, written 'offset_of<T>(\"field\")'"
                        .to_string(),
                    span,
                });
            }
            match &args[0].value {
                ast::Expr::StringLit(field, _) if args[0].name.is_none() => Some(field.clone()),
                _ => {
                    return Err(TypeError {
                        help: None,
                        message: "'offset_of' takes the field name as a string literal, written 'offset_of<T>(\"field\")'"
                            .to_string(),
                        span,
                    })
                }
            }
        } else {
            if !args.is_empty() {
                return Err(TypeError {
                    help: None,
                    message: format!("'{callee}' takes no arguments, found {}", args.len()),
                    span,
                });
            }
            None
        };
        let value = layout_query_value(
            callee,
            &ty,
            field.as_deref(),
            self.structs,
            self.enums,
            span,
        )?;
        Ok((ConstValue::Int(value as i128), Ty::Usize))
    }

    /// The message every out-of-reach construct shares: it names the construct
    /// and says what a constant expression is allowed to do.
    pub(crate) fn unsupported(&self, span: Span, what: &str) -> TypeError {
        TypeError {
            help: None,
            message: format!(
                "a 'const' initializer cannot use {what}: what it can evaluate is literals, other 'const's, the arithmetic, comparison and bitwise operators, 'if', and a call to a plain 'fun' whose own body stays inside that"
            ),
            span,
        }
    }

    pub(crate) fn fold_unary(
        &self,
        op: ast::UnOp,
        v: ConstValue,
        ty: &Ty,
        span: Span,
    ) -> Result<ConstResult> {
        use ConstValue::*;
        let kind_mismatch = || TypeError {
            help: None,
            message: format!(
                "operator does not apply to '{}'",
                ty.display()
            ),
            span,
        };
        Ok(match (op, v) {
            (ast::UnOp::Neg, Int(i)) => (Int(wrap_int(-i, ty)), ty.clone()),
            (ast::UnOp::Neg, Float(f)) => (Float(-f), ty.clone()),
            (ast::UnOp::Not, Bool(b)) => (Bool(!b), Ty::Bool),
            (ast::UnOp::BitNot, Int(i)) => (Int(wrap_int(!i, ty)), ty.clone()),
            _ => return Err(kind_mismatch()),
        })
    }

    pub(crate) fn fold_binary(
        &self,
        op: ast::BinOp,
        lhs: &ast::Expr,
        rhs: &ast::Expr,
        span: Span,
        expected: Option<&Ty>,
        scope: &mut ConstScope,
    ) -> Result<ConstResult> {
        use ast::BinOp::*;
        let hint = match op {
            Add | Sub | Mul | Div | Mod => expected.filter(|t| t.is_numeric()),
            BitAnd | BitOr | BitXor | Shl | Shr => expected.filter(|t| t.is_integer()),
            _ => None,
        };
        let (l, r) = if hint.is_none() && is_untyped_literal(lhs) && !is_untyped_literal(rhs) {
            let right = self.eval(rhs, None, scope)?;
            let ty = right.1.clone();
            let left = self.eval(lhs, Some(&ty), scope)?;
            (left, right)
        } else {
            let left = self.eval(lhs, hint, scope)?;
            let ty = left.1.clone();
            let right = self.eval(rhs, Some(&ty), scope)?;
            (left, right)
        };
        // A `return` in either operand means the whole expression is skipped:
        // its value belongs to the function that returned, not to this
        // operator.
        if scope.halted() {
            return Ok((ConstValue::Unit, Ty::Unit));
        }
        let (lv, lt) = l;
        let (rv, rt) = r;
        if lt != rt {
            return Err(TypeError {
                help: None,
                message: format!(
                    "a constant expression cannot mix '{}' and '{}': every operator requires matching operands, exactly as it does at runtime",
                    lt.display(),
                    rt.display()
                ),
                span,
            });
        }
        let no = |what: &str, ty: &Ty| TypeError {
            help: None,
            message: format!(
                "a 'const' expression cannot use {what} on '{}' values",
                ty.display()
            ),
            span,
        };
        let zero = || TypeError {
            help: None,
            message: "division by zero in a constant expression".to_string(),
            span,
        };
        use ConstValue::*;
        Ok(match (lv, rv) {
            (Int(a), Int(b)) => {
                let bits = int_bits(&lt).unwrap_or(64);
                match op {
                    Add => (Int(wrap_int(a + b, &lt)), lt),
                    Sub => (Int(wrap_int(a - b, &lt)), lt),
                    Mul => (Int(wrap_int(a * b, &lt)), lt),
                    Div => {
                        if b == 0 {
                            return Err(zero());
                        }
                        (Int(wrap_int(a / b, &lt)), lt)
                    }
                    Mod => {
                        if b == 0 {
                            return Err(zero());
                        }
                        (Int(wrap_int(a % b, &lt)), lt)
                    }
                    BitAnd => (Int(wrap_int(a & b, &lt)), lt),
                    BitOr => (Int(wrap_int(a | b, &lt)), lt),
                    BitXor => (Int(wrap_int(a ^ b, &lt)), lt),
                    // A shift by the width or more is undefined in C, so it is
                    // refused here rather than folded to whatever this machine
                    // happens to do; `>>` is arithmetic on a signed type,
                    // which is what the generated code does.
                    Shl | Shr if b < 0 || b >= bits as i128 => {
                        return Err(TypeError {
                            help: None,
                            message: format!(
                                "a shift by {b} is not defined for a '{}', whose width is {bits} bits",
                                lt.display()
                            ),
                            span,
                        })
                    }
                    Shl => (Int(wrap_int(a << (b as u32), &lt)), lt),
                    // `>>` on the `i128` that holds a signed value is already
                    // arithmetic, and an unsigned value is never negative in
                    // that representation, so one shift covers both.
                    Shr => (Int(wrap_int(a >> (b as u32), &lt)), lt),
                    Eq => (Bool(a == b), Ty::Bool),
                    NotEq => (Bool(a != b), Ty::Bool),
                    Lt => (Bool(a < b), Ty::Bool),
                    LtEq => (Bool(a <= b), Ty::Bool),
                    Gt => (Bool(a > b), Ty::Bool),
                    GtEq => (Bool(a >= b), Ty::Bool),
                    And | Or => return Err(no(if op == And { "&&" } else { "||" }, &lt)),
                }
            }
            (Float(a), Float(b)) => match op {
                Add => (Float(a + b), lt),
                Sub => (Float(a - b), lt),
                Mul => (Float(a * b), lt),
                Div => (Float(a / b), lt),
                Mod => (Float(a % b), lt),
                Eq => (Bool(a == b), Ty::Bool),
                NotEq => (Bool(a != b), Ty::Bool),
                Lt => (Bool(a < b), Ty::Bool),
                LtEq => (Bool(a <= b), Ty::Bool),
                Gt => (Bool(a > b), Ty::Bool),
                GtEq => (Bool(a >= b), Ty::Bool),
                _ => return Err(no("a bitwise operator", &lt)),
            },
            (Bool(a), Bool(b)) => match op {
                And => (Bool(a && b), Ty::Bool),
                Or => (Bool(a || b), Ty::Bool),
                Eq => (Bool(a == b), Ty::Bool),
                NotEq => (Bool(a != b), Ty::Bool),
                _ => return Err(no("an arithmetic operator", &lt)),
            },
            _ => {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "a constant expression cannot compare or combine '{}' values",
                        lt.display()
                    ),
                    span,
                })
            }
        })
    }

    /// An integer conversion in constant position (`n.to_usize()`): the same
    /// checked narrowing the runtime performs, reported as a diagnostic
    /// instead of a panic because nothing is running.
    pub(crate) fn convert(
        &self,
        value: &ConstValue,
        from: &Ty,
        to: &str,
        call: &str,
        span: Span,
    ) -> Result<ConstResult> {
        let Some(target) = Ty::from_name(to) else {
            return Err(TypeError {
                help: None,
                message: format!("'{}' is not a type to convert to", to),
                span,
            });
        };
        let (ConstValue::Int(n), true) = (value, from.is_integer()) else {
            return Err(TypeError {
                help: None,
                message: format!(
                    "only an integer conversion is available in a constant expression, and this one reads a '{}'",
                    from.display()
                ),
                span,
            });
        };
        if target.is_float() {
            return Ok((
                ConstValue::Float(if matches!(target, Ty::F32) { *n as f32 as f64 } else { *n as f64 }),
                target,
            ));
        }
        if !target.is_integer() {
            return Err(TypeError {
                help: None,
                message: format!("'{call}' is not a conversion a constant expression can perform"),
                span,
            });
        }
        let (lo, hi) = Ty::int_bounds(&target).expect("an integer target has bounds");
        // `hi` is a `u128` so `u64`/`usize` can state theirs, but every bound
        // it holds fits an `i128`, which is what the folded value is.
        let fits = *n >= lo && *n <= hi as i128;
        if !fits {
            return Err(TypeError {
                help: None,
                message: format!("{call}: value out of range"),
                span,
            });
        }
        Ok((ConstValue::Int(wrap_int(*n, &target)), target))
    }

    /// A `fun` call in constant position: bind the arguments to the declared
    /// parameters, then walk the body (A-19).
    pub(crate) fn call_fn(
        &self,
        key: &str,
        display: &str,
        args: &[ast::CallArg],
        span: Span,
        scope: &mut ConstScope,
    ) -> Result<ConstResult> {
        let Some(f) = self.const_fns.fns.get(key) else {
            return Err(TypeError {
                help: None,
                message: format!(
                    "'{display}' cannot be called in a constant expression: only a plain 'fun' whose own body a constant can evaluate is in reach, and this is not one (a builtin, an 'extern', a method and a generic function are all out of reach)"
                ),
                span,
            });
        };
        if self.depth.get() >= CONST_EVAL_DEPTH {
            return Err(TypeError {
                help: None,
                message: format!(
                    "evaluating '{display}' recursed more than {CONST_EVAL_DEPTH} calls deep: a constant expression has to terminate on the values it is given"
                ),
                span,
            });
        }
        let ts = self.type_scope(&f.prefix, &f.aliases);
        let mut callee = ConstScope::new(f.prefix.clone(), f.aliases.clone());
        let mut slots: Vec<Option<ConstResult>> = Vec::with_capacity(f.params.len());
        for _ in 0..f.params.len() {
            slots.push(None);
        }
        let mut next_positional = 0usize;
        for arg in args {
            let index = match &arg.name {
                Some(name) => match f.params.iter().position(|(p, _)| p == name) {
                    Some(i) => i,
                    None => {
                        return Err(TypeError {
                            help: None,
                            message: format!("'{display}' has no parameter named '{name}'"),
                            span: arg.value.span(),
                        })
                    }
                },
                None => {
                    let i = next_positional;
                    next_positional += 1;
                    i
                }
            };
            if index >= f.params.len() {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "'{display}' takes {} argument(s), and this call has more",
                        f.params.len()
                    ),
                    span: arg.value.span(),
                });
            }
            if slots[index].is_some() {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "parameter '{}' of '{display}' is given twice",
                        f.params[index].0
                    ),
                    span: arg.value.span(),
                });
            }
            let pty = ts
                .resolve(&f.params[index].1, false)
                .map_err(|e| e.at(arg.value.span()))?;
            // The argument is evaluated in the *caller's* scope: `f(n)` inside
            // `f`'s own body reads the caller's `n`, which is the binding the
            // call frame is about to replace.
            slots[index] = Some(self.eval(&arg.value, Some(&pty), scope)?);
            if scope.halted() {
                return Ok((ConstValue::Unit, Ty::Unit));
            }
        }
        let ret_ty = match &f.ret {
            Some(t) => Some(ts.resolve(t, false).map_err(|e| e.at(span))?),
            None => None,
        };
        for (i, slot) in slots.into_iter().enumerate() {
            match slot {
                Some(v) => {
                    let declared = ts
                        .resolve(&f.params[i].1, false)
                        .map_err(|e| e.at(span))?;
                    if v.1 != declared {
                        return Err(TypeError {
                            help: None,
                            message: format!(
                                "parameter '{}' of '{display}' is a '{}', and this argument is a '{}'",
                                f.params[i].0,
                                declared.display(),
                                v.1.display()
                            ),
                            span,
                        });
                    }
                    callee.frames[0].insert(
                        f.params[i].0.clone(),
                        ConstBinding {
                            value: v,
                            mutable: false,
                        },
                    );
                }
                None => {
                    return Err(TypeError {
                        help: None,
                        message: format!(
                            "'{display}' is missing its argument for parameter '{}'",
                            f.params[i].0
                        ),
                        span,
                    })
                }
            }
        }
        callee.ret_ty = ret_ty.clone();
        self.depth.set(self.depth.get() + 1);
        let flow = self.eval_block(&f.body, ret_ty.as_ref(), &mut callee);
        self.depth.set(self.depth.get() - 1);
        let value = match flow? {
            ConstFlow::Fell(v) | ConstFlow::Returned(v) => v,
        };
        match (value, ret_ty) {
            (Some(v), Some(t)) => {
                if v.1 != t {
                    return Err(TypeError {
                        help: None,
                        message: format!(
                            "'{display}' declares '{}' but this call produced a '{}'",
                            t.display(),
                            v.1.display()
                        ),
                        span,
                    });
                }
                Ok(v)
            }
            (Some(_), None) => Err(TypeError {
                help: None,
                message: format!("'{display}' returns a value but declares no return type"),
                span,
            }),
            (None, None) => Ok((ConstValue::Unit, Ty::Unit)),
            (None, Some(t)) => Err(TypeError {
                help: None,
                message: format!(
                    "'{display}' has to produce a '{}' value, and it fell off its end",
                    t.display()
                ),
                span,
            }),
        }
    }
}
