use super::*;

/// The name a container type is written with, when the type is one of the
/// shared-pool containers that a block may not make (section 6, rule 4).
pub(crate) fn shared_container_kind(ty: &Ty) -> Option<&'static str> {
    match ty {
        Ty::List(_) => Some("List"),
        Ty::Map(_, _) => Some("Map"),
        Ty::String => Some("String"),
        Ty::Bytes => Some("Bytes"),
        _ => None,
    }
}

/// The name at the root of an assignment target: `keep` for `keep`, and `best`
/// for `best.x`.
pub(crate) fn place_root_name(target: &ast::Expr) -> Option<String> {
    match target {
        ast::Expr::Ident(name, _) => Some(name.clone()),
        ast::Expr::FieldAccess { base, .. } => place_root_name(base),
        _ => None,
    }
}

/// `name.box()` with no arguments, when `name` is a block of this function:
/// the block's name, or `None` for any other call.
pub(crate) fn block_box_receiver(receiver: &ast::Expr, method: &str, args: &[ast::CallArg], ctx: &Ctx) -> Option<String> {
    match receiver {
        ast::Expr::Ident(name, _) if method == "box" && args.is_empty() && ctx.block_names.contains(name) => {
            Some(name.clone())
        }
        _ => None,
    }
}

/// `name.box()`: one zeroed element of the block's type, which the region
/// owns. The result is `Result<Box<T>, BlockError>`.
pub(crate) fn check_block_box(bname: &str, span: Span, ctx: &mut Ctx) -> Result<tir::Expr> {
    let Some((_, elem)) = ctx.active_blocks.iter().rev().find(|(n, _)| n == bname).cloned() else {
        return Err(TypeError {
            help: Some(format!("make the box inside `block {bname}: T {{ ... }}`")),
            message: format!("'{bname}.box()' is valid only inside the block '{bname}'"),
            span,
        });
    };
    let boxes = ctx.block_box_count.entry(bname.to_string()).or_insert(0);
    *boxes += 1;
    if *boxes > 1 {
        return Err(TypeError {
            help: Some("a block holds one element: make the second box in another block".to_string()),
            message: format!("the block '{bname}' already has a box: a second '{bname}.box()' would need a second slot (slice 1)"),
            span,
        });
    }
    let key = block_result_key(ctx, span)?;
    let ty = ctx.instantiate_type(&key, &[Ty::Box(Box::new(elem.clone())), block_error_ty()], span)?;
    Ok(tir::Expr {
        kind: tir::ExprKind::BlockBox {
            name: bname.to_string(),
            elem,
        },
        ty,
        span,
    })
}

/// `block name: T { ... }` (decision 139): a named region whose value is the
/// body's `T`, wrapped as `Result<T, BlockError>`. The name is unique in its
/// function, and the block's result must be used with `?` or matched.
pub(crate) fn check_manual_block(
    name: &str,
    ty: &ast::TypeExpr,
    body: &ast::Block,
    span: Span,
    ctx: &mut Ctx,
) -> Result<tir::Expr> {
    if !std::mem::take(&mut ctx.block_result_used) {
        return Err(TypeError {
            help: Some("add '?' after the closing brace in a function that returns a Result, or match the result with 'when'".to_string()),
            message: format!("block '{name}' can fail to start, and its result is not used"),
            span,
        });
    }
    if ctx.block_names.contains(name) {
        return Err(TypeError {
            help: Some("give the second block another name: a block's name is unique in its function".to_string()),
            message: format!("the block '{name}' is already declared in this function"),
            span,
        });
    }
    if ctx.let_names.contains(name) {
        return Err(TypeError {
            help: Some("give the block another name, or the 'let' another one".to_string()),
            message: format!("the block '{name}' shares its name with a 'let' in this function"),
            span,
        });
    }
    ctx.block_names.insert(name.to_string());
    let elem = ctx.type_scope().resolve(ty, false)?;
    if !block_elem_ok(&elem, ctx) {
        return Err(TypeError {
            help: Some("a block holds one datac of scalar fields (slice 1)".to_string()),
            message: format!("the block '{name}' cannot hold '{}': its element must be a scalar or a datac of scalars", elem.display()),
            span,
        });
    }
    let key = block_result_key(ctx, span)?;
    ctx.active_blocks.push((name.to_string(), elem.clone()));
    let checked = check_block(body, ctx, Some(&elem));
    ctx.active_blocks.pop();
    let checked = checked?;
    let value_ty = checked.tail.as_ref().map(|t| t.ty.clone()).unwrap_or(Ty::Unit);
    if value_ty != elem {
        return Err(TypeError {
            help: Some(format!("end the block with a value of type '{}'", elem.display())),
            message: format!("the block '{name}' ends with '{}', not its element type '{}'", value_ty.display(), elem.display()),
            span,
        });
    }
    let result = ctx.instantiate_type(&key, &[elem.clone(), block_error_ty()], span)?;
    Ok(tir::Expr {
        kind: tir::ExprKind::Region {
            name: name.to_string(),
            elem,
            body: checked,
        },
        ty: result,
        span,
    })
}

pub(crate) fn check_try(sub: &ast::Expr, span: Span, ctx: &mut Ctx) -> Result<tir::Expr> {
    // A block's failure has to go to the caller. In a function that returns
    // `Unit` there is nowhere for it to go (decision 139, R31).
    if matches!(sub, ast::Expr::ManualBlock { .. }) && matches!(ctx.ret_ty, Ty::Unit) {
        return Err(TypeError {
            help: Some(format!("match the block's result with 'when' here, or make '{}' return a Result", ctx.fn_name)),
            message: format!("'?' has nowhere to send a failure, because '{}' returns Unit", ctx.fn_name),
            span,
        });
    }
    let inner = if let ast::Expr::MethodCall { receiver, method, args, .. } = sub {
        match block_box_receiver(receiver, method, args, ctx) {
            Some(bname) => {
                if !std::mem::take(&mut ctx.box_let_ok) {
                    return Err(TypeError {
                        help: Some(format!("write `let x = {bname}.box()?` inside the block, so the box stays in its block")),
                        message: format!("a block box from '{bname}.box()?' must be bound by a 'let' in its block"),
                        span,
                    });
                }
                check_block_box(&bname, span, ctx)?
            }
            None => check_expr(sub, ctx, None)?,
        }
    } else {
        if matches!(sub, ast::Expr::ManualBlock { .. }) {
            ctx.block_result_used = true;
        }
        check_expr(sub, ctx, None)?
    };
    // The operand is read as a whole value and its payload is moved out, so a
    // borrow is not usable here - `when`/`if let` is the form for that.
    let source = match &inner.ty {
        Ty::Struct(name) => name.clone(),
        other => {
            return Err(TypeError {
                help: None,
                message: format!(
                    "'?' needs a value shaped 'Ok'/'Err' or 'Some'/'None' held by value, not '{}' - match it with 'when' or 'if let' instead",
                    other.display()
                ),
                span,
            })
        }
    };
    let Some(edef) = ctx.enum_def(&source) else {
        return Err(TypeError {
            help: None,
            message: format!("'?' needs an enum, but '{}' is a datac", inner.ty.display()),
            span,
        });
    };
    let Some(shape) = try_shape(&edef) else {
        let names: Vec<String> = edef.variants.iter().map(|v| format!("'{}'", v.name)).collect();
        return Err(TypeError {
            help: None,
            message: format!(
                "'?' needs an enum with an 'Ok' and an 'Err' variant (or a 'Some' and a 'None'), but '{}' has {}",
                inner.ty.display(),
                names.join(", ")
            ),
            span,
        });
    };
    // The enclosing function's return type is where a failure goes.
    let ret = ctx.ret_ty.clone();
    let ret_name = match &ret {
        Ty::Struct(name) if ctx.enum_def(name).is_some() => name.clone(),
        other => {
            return Err(TypeError {
                help: None,
                message: format!(
                    "'?' propagates a failure out of the enclosing function, so that function has to return an enum shaped 'Ok'/'Err' or 'Some'/'None' - it returns '{}'",
                    other.display()
                ),
                span,
            })
        }
    };
    let rdef = ctx
        .enum_def(&ret_name)
        .expect("checked just above that the return type is an enum");
    let Some(rshape) = try_shape(&rdef) else {
        let names: Vec<String> = rdef.variants.iter().map(|v| format!("'{}'", v.name)).collect();
        return Err(TypeError {
            help: None,
            message: format!(
                "'?' propagates a failure out of the enclosing function, so that function has to return an enum shaped 'Ok'/'Err' or 'Some'/'None' - it returns '{}', with {}",
                ret.display(),
                names.join(", ")
            ),
            span,
        });
    };
    if shape.err_has_payload != rshape.err_has_payload {
        return Err(TypeError {
            help: None,
            message: format!(
                "'?' here propagates {} failure, but the enclosing function returns '{}' - the two have to be the same shape; match the value instead (`when r {{ Ok(v) -> ..., Err(e) -> return <your failure> }}`), or make the function return the same kind of enum",
                if shape.err_has_payload { "a carrying" } else { "a payload-less" },
                ret.display()
            ),
            span,
        });
    }
    // A-12 slice 6: when the two failure payload types differ, `?` converts
    // through a declared `impl From<Source> for Target`, calling
    // `Target.from(source)`. When they agree, the payload moves across with no
    // call, exactly as before.
    let mut convert: Option<(String, bool)> = None;
    if shape.err_has_payload {
        let from = edef.variants[shape.err].fields[0].ty.clone();
        let to = rdef.variants[rshape.err].fields[0].ty.clone();
        if from != to {
            match find_from_conversion(ctx, &from, &to)? {
                Some(key) => convert = Some(key),
                None => {
                    return Err(TypeError {
                        help: None,
                        message: format!(
                            "'?' cannot convert a failure: '{}' carries '{}' and the enclosing function returns '{}', whose failure carries '{}' - write 'impl From<{}> for {} {{ fun from(value: {}) => {} {{ ... }} }}' so the operator has a conversion to call",
                            inner.ty.display(),
                            from.display(),
                            ret.display(),
                            to.display(),
                            from.display(),
                            to.display(),
                            from.display(),
                            to.display()
                        ),
                        span,
                    })
                }
            }
        }
    }
    let ok_ty = edef.variants[shape.ok].fields[0].ty.clone();
    Ok(tir::Expr {
        kind: tir::ExprKind::Try {
            expr: Box::new(inner),
            source,
            target: ret_name,
            ok_variant: shape.ok,
            err_variant: shape.err,
            target_err_variant: rshape.err,
            err_has_payload: shape.err_has_payload,
            convert,
        },
        ty: ok_ty,
        span,
    })
}

/// The static `From` method key that converts one failure payload into another
/// (A-12 slice 6), or `None` when no `impl From<from> for to` exists. The
/// implementing type is the *target* payload type itself, because that is what
/// the `for` clause names (`impl From<IoErr> for AppErr`).
pub(crate) fn find_from_conversion(ctx: &Ctx, from_ty: &Ty, to_ty: &Ty) -> Result<Option<(String, bool)>> {
    let Some(to_name) = struct_name_of(to_ty)
        .map(|s| s.to_string())
        .or_else(|| builtin_type_name(to_ty).map(|s| s.to_string()))
    else {
        return Ok(None);
    };
    for imp in ctx.env.trait_impls.iter() {
        if imp.type_name != to_name {
            continue;
        }
        let Some(info) = ctx.env.traits.get(&imp.trait_name) else {
            continue;
        };
        // The conversion is the declared `From` trait's `from`, whatever
        // module ships it; the operator names `From` in its refusal for the
        // same reason.
        if !(info.name == "From" || info.name.ends_with(".From")) {
            continue;
        }
        if info.type_params.len() != 1 {
            continue;
        }
        let trait_args = trait_arg_tys(&imp.trait_arg_names);
        for m in &info.methods {
            if m.receiver.is_some() || m.name != "from" || m.params.len() != 1 {
                continue;
            }
            let sig = resolve_trait_method_sig(
                info,
                m,
                to_ty,
                &trait_args,
                &ctx.structs,
                &ctx.enums,
                &ctx.env,
                &ctx.consts,
                &ctx.const_fns,
            )?;
            if sig.params.len() != 1 || sig.ret != *to_ty {
                continue;
            }
            // A `datac` parameter is a shared borrow of the payload; a `Copy`
            // scalar is taken by value. The caller passes whichever the
            // method declares.
            let by_ref = matches!(&sig.params[0].1, Ty::Ref { inner, .. } if **inner == *from_ty);
            if sig.params[0].1 == *from_ty || by_ref {
                return Ok(Some((impl_method_key(imp, m), by_ref)));
            }
        }
    }
    Ok(None)
}

/// `Shape.Circle(r = 1.0)` - one variant's worth of payload, checked against
/// the payload fields exactly as a `datac` construction is checked against its
/// fields (A-1).
pub(crate) fn check_enum_construction(
    ename: &str,
    variant: &str,
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
) -> Result<tir::Expr> {
    let edef = ctx
        .enums
        .get(ename)
        .cloned()
        .expect("enum_path only returns known enums");
    // `Mode.from_int(n)` shares the `Type.member(...)` spelling with a variant
    // construction but is the enum's own associated conversion (kernel stage
    // 2), so it is resolved before the variant lookup refuses it as one. A
    // variant actually named `from_int` still wins.
    if variant == "from_int" && edef.variant("from_int").is_none() {
        return lower_enum_from_int(ename, args, span, ctx);
    }
    let Some((vidx, vdef)) = edef.variant(variant) else {
        let names: Vec<String> = edef
            .variants
            .iter()
            .map(|v| format!("'{}'", v.name))
            .collect();
        return Err(TypeError {
            help: None,
            message: format!(
                "'{ename}.{variant}' is not a variant - the variants of '{ename}' are {}",
                names.join(", ")
            ),
            span,
        });
    };
    let params: Vec<(String, Ty)> = vdef
        .fields
        .iter()
        .map(|f| (f.name.clone(), f.ty.clone()))
        .collect();
    let what = format!("variant '{}.{}'", ename, variant);
    let shorthand = field_shorthand(&what, &params, args, ctx)?;
    let fields = match_args(&what, &params, &shorthand, span, ctx)?;
    Ok(tir::Expr {
        kind: tir::ExprKind::EnumLit {
            enum_name: ename.to_string(),
            variant: vidx,
            fields,
        },
        ty: Ty::Struct(ename.to_string()),
        span,
    })
}

/// `when s { Circle(r) -> ... else -> ... }` (A-1) - a pattern match over an
/// `enum`, lowered here to a chain of `if <tag test> { <bindings> <body> }
/// else ...`: the same TIR shape the boolean `when` already lowers to in the
/// parser, plus two node kinds (`EnumTag`, `EnumField`) that codegen emits as
/// `.tag ==` and `.v.<Variant>.<field>`. No new statement machinery and no new
/// drop plan are needed as a result.
///
/// `scrutinee` is always a place by the time this runs: `check_when_scrutinee`
/// stages a temporary into a local of its own first. That is what makes the
/// payload borrows below outlive their arms, and what makes an owned payload of
/// a temporary scrutinee bind by value out of that staged local.
/// Whether a block's last statement leaves the enclosing code on every path:
/// `return`, `break`, `continue`, or `panic(...)`. A let-else leaves when its
/// pattern arm (the rest of the block) and its else block both leave.
pub(crate) fn ast_block_leaves(block: &ast::Block) -> bool {
    match block.stmts.last() {
        Some(ast::Stmt::Return { .. } | ast::Stmt::Break { .. } | ast::Stmt::Continue { .. }) => true,
        Some(ast::Stmt::Expr { expr: ast::Expr::Call { callee, .. }, .. }) => callee == "panic",
        Some(ast::Stmt::Expr { expr, .. }) => let_else_leaves(expr),
        _ => false,
    }
}

/// Whether an expression is a let-else whose both arms leave (see
/// `ast_block_leaves`). Such a statement is no tail: it has no value, and the
/// function it ends does not fall off its end.
pub(crate) fn let_else_leaves(expr: &ast::Expr) -> bool {
    match expr {
        ast::Expr::When { let_else: true, arms, .. } => arms.iter().all(|a| match &a.body {
            ast::Expr::Block(b) => ast_block_leaves(b),
            _ => false,
        }),
        _ => false,
    }
}

/// The let-else rules (`design/syntax.md (kelp wiki)` 4.1) that are checked before the
/// desugared `when` is: the else block must end in a diverging statement, and
/// the pattern's enum must be the expression's type. The variant lookup is
/// left to `check_when`, which names the let-else rule in its own refusal.
pub(crate) fn check_let_else_form(scrut: &tir::Expr, arms: &[ast::WhenArm]) -> Result<()> {
    if let Some(ast::WhenArm { body: ast::Expr::Block(block), span, pattern: None }) = arms.get(1) {
        if !ast_block_leaves(block) {
            return Err(TypeError {
                help: None,
                message: "let-else: the else block falls through. Its last statement must leave the enclosing code (return, break, continue, or panic(...)), because the code after the let runs only when the pattern matched".to_string(),
                span: *span,
            });
        }
    }
    if struct_name_of(&scrut.ty).is_none() {
        let pattern = match arms.first().and_then(|a| a.pattern.as_ref()) {
            Some(p) => p.variant.clone(),
            None => String::new(),
        };
        return Err(TypeError {
            help: None,
            message: format!(
                "let-else: the pattern '{pattern}' must match the expression's type, and '{}' is not an enum, so no variant can match it. Fix: give the expression the enum type the pattern names",
                scrut.ty.display()
            ),
            span: scrut.span,
        });
    }
    Ok(())
}

pub(crate) fn check_when(
    scrutinee: &ast::Expr,
    arms: &[ast::WhenArm],
    span: Span,
    let_else: bool,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
) -> Result<tir::Expr> {
    let scrut = check_expr(scrutinee, ctx, None)?;
    if let_else {
        check_let_else_form(&scrut, arms)?;
    }
    let Some(ename) = struct_name_of(&scrut.ty).map(|s| s.to_string()) else {
        return Err(TypeError {
            help: None,
            message: format!(
                "'when' with a scrutinee matches an 'enum', but '{}' is not one - for a chain of conditions write 'when {{ cond -> value }}' with no scrutinee",
                scrut.ty.display()
            ),
            span: scrut.span,
        });
    };
    let Some(edef) = ctx.enum_def(&ename) else {
        return Err(TypeError {
            help: None,
            message: format!(
                "'when' with a scrutinee matches an 'enum', but '{ename}' is a datac - read its fields with '.' instead"
            ),
            span: scrut.span,
        });
    };

    // An owned payload binds by value only when the scrutinee is a place this
    // frame owns outright: a `val`/`var` local or a `move` parameter, which is
    // the base `borrowck.rs` plans a partial move from. A borrow scrutinee, a
    // field path and a dereference all bind it as a shared borrow instead, so
    // the value stays where it is; the staged `#when0` a temporary scrutinee
    // becomes is an owned local like any other, which is what lets the payload
    // of `when f() { ... }` be handed out.
    let owns_scrutinee =
        matches!(scrut.kind, tir::ExprKind::Ident(_)) && !matches!(scrut.ty, Ty::Ref { .. });
    let mut covered: Vec<Option<usize>> = vec![None; edef.variants.len()];
    let mut else_seen = false;
    let mut checked: Vec<(Option<usize>, tir::Block)> = Vec::new();
    let mut arm_ty: Option<Ty> = None;

    for (ai, arm) in arms.iter().enumerate() {
        if else_seen {
            return Err(TypeError {
                help: None,
                message: "an 'else' arm must be the last arm of a 'when'".to_string(),
                span: arm.span,
            });
        }
        let variant = match &arm.pattern {
            None => {
                else_seen = true;
                None
            }
            Some(p) => {
                let Some((vidx, vdef)) = edef.variant(&p.variant) else {
                    let names: Vec<String> = edef
                        .variants
                        .iter()
                        .map(|v| format!("'{}'", v.name))
                        .collect();
                    if let_else {
                        return Err(TypeError {
                            help: None,
                            message: format!(
                                "let-else: the pattern '{}' must match the expression's type, and '{ename}' has no such variant (the variants are {}). Fix: name a variant of the expression's enum, or change the expression to the enum the pattern names",
                                p.variant,
                                names.join(", ")
                            ),
                            span: p.span,
                        });
                    }
                    return Err(TypeError {
                        help: None,
                        message: format!(
                            "'{}' is not a variant of enum '{ename}' - the variants are {}",
                            p.variant,
                            names.join(", ")
                        ),
                        span: p.span,
                    });
                };
                if covered[vidx].is_some() {
                    return Err(TypeError {
                        help: None,
                        message: format!("variant '{}' is matched more than once", p.variant),
                        span: p.span,
                    });
                }
                if p.binds.len() != vdef.fields.len() {
                    let fields: Vec<String> =
                        vdef.fields.iter().map(|f| f.name.clone()).collect();
                    return Err(TypeError {
                        help: None,
                        message: format!(
                            "pattern '{}({})' has {} binding(s) but variant '{}' carries {} payload field(s) ({})",
                            p.variant,
                            p.binds
                                .iter()
                                .map(|b| b.as_deref().unwrap_or("_").to_string())
                                .collect::<Vec<_>>()
                                .join(", "),
                            p.binds.len(),
                            p.variant,
                            vdef.fields.len(),
                            fields.join(", ")
                        ),
                        span: p.span,
                    });
                }
                covered[vidx] = Some(ai);
                Some(vidx)
            }
        };

        // Payload bindings live in a scope of their own, so the same name may
        // be used by two arms and shadows nothing outside them.
        let mut bind_stmts: Vec<tir::Stmt> = Vec::new();
        ctx.scopes.push(HashMap::new());
        if let (Some(vidx), Some(p)) = (variant, &arm.pattern) {
            for (fi, f) in edef.variants[vidx].fields.iter().enumerate() {
                let Some(bind) = &p.binds[fi] else { continue };
                let field = tir::Expr {
                    kind: tir::ExprKind::EnumField {
                        base: Box::new(scrut.clone()),
                        variant: vidx,
                        field: fi,
                    },
                    ty: f.ty.clone(),
                    span: p.span,
                };
                let (ty, init) = if f.ty.is_copy() || owns_scrutinee {
                    (f.ty.clone(), field)
                } else {
                    // An owned payload of a scrutinee that is not a local this
                    // frame owns directly binds as a shared borrow: a move out
                    // by value is a partial move, and one only takes a
                    // projection out of a `val`/`var`/`move` local
                    // (`borrowck.rs::partial_move_base`), so a borrow scrutinee,
                    // a field path and a dereference keep the value where it is.
                    let ty = Ty::Ref {
                        mutable: false,
                        inner: Box::new(f.ty.clone()),
                    };
                    (
                        ty.clone(),
                        tir::Expr {
                            kind: tir::ExprKind::Ref {
                                mutable: false,
                                expr: Box::new(field),
                            },
                            ty,
                            span: p.span,
                        },
                    )
                };
                ctx.define(bind, ty.clone(), false);
                bind_stmts.push(tir::Stmt::Let {
                    name: bind.clone(),
                    ty,
                    init,
                });
            }
        }
        let body = check_expr(&arm.body, ctx, expected.or(arm_ty.as_ref()))?;
        ctx.scopes.pop();
        match arm_ty.clone() {
            // `str` and `&str` arms agree, and join to the borrow (see
            // `join_value_type`); anything else has to match exactly.
            Some(t) => match join_value_type(&t, &body.ty) {
                Some(joined) => arm_ty = Some(joined),
                None => {
                    return Err(TypeError {
                        help: None,
                        message: format!(
                            "'when' arms have different types: '{}' and '{}'",
                            t.display(),
                            body.ty.display()
                        ),
                        span: arm.span,
                    })
                }
            },
            None => arm_ty = Some(body.ty.clone()),
        }
        checked.push((
            variant,
            tir::Block {
                stmts: bind_stmts,
                tail: Some(Box::new(body)),
            },
        ));
    }

    if !else_seen {
        let missing: Vec<String> = edef
            .variants
            .iter()
            .enumerate()
            .filter(|(i, _)| covered[*i].is_none())
            .map(|(_, v)| format!("'{}'", v.name))
            .collect();
        if !missing.is_empty() {
            return Err(TypeError {
                help: None,
                message: format!(
                    "this 'when' is not exhaustive: {} of enum '{ename}' has no arm - add an arm, or an 'else' arm",
                    missing.join(" and ")
                ),
                span,
            });
        }
    }

    // Fold right-to-left. The last arm becomes the base `else` and gets no
    // tag test of its own: after every other arm's test has failed, only its
    // variant can be live (and when it is an `else` arm, that is its meaning).
    // Doing it this way also means the chain always has an `else`, so the
    // temporary an `if`-as-a-value lowers to is always assigned on both
    // paths - no uninitialized read, and nothing for a C warning to flag.
    let (_, base_block) = checked
        .pop()
        .expect("a 'when' always parses with at least one arm");
    let mut acc: tir::Block = base_block;
    for (variant, block) in checked.into_iter().rev() {
        let vidx = variant.expect("only the last arm may be an 'else' arm");
        let cond = tir::Expr {
            kind: tir::ExprKind::EnumTag {
                expr: Box::new(scrut.clone()),
                variant: vidx,
            },
            ty: Ty::Bool,
            span,
        };
        let block_ty = block.ty();
        let if_expr = tir::Expr {
            kind: tir::ExprKind::If {
                cond: Box::new(cond),
                then_block: block,
                else_block: Some(acc),
            },
            ty: block_ty,
            span,
        };
        acc = tir::Block {
            stmts: Vec::new(),
            tail: Some(Box::new(if_expr)),
        };
    }
    let ty = acc.ty();
    Ok(tir::Expr {
        kind: tir::ExprKind::Block(acc),
        ty,
        span,
    })
}

/// Whether a bare identifier names an `impl` alias rather than a value. A
/// local, parameter, receiver field, or passdown shadows it, exactly as it
/// would shadow any other name.
/// A note appended to an error only a stdlib *instantiation* can produce: a
/// helper written for a `Copy` payload or element fails inside the module, and
/// the span points into a file the caller never opened. Empty everywhere else.
pub(crate) fn stdlib_instantiation_note(ctx: &Ctx) -> String {
    if is_shipped_module(&ctx.module) {
        format!(
            " - this is inside the shipped standard library ('{}'), so the instantiation you asked for is one it does not offer: a helper that hands a value out is written for a 'Copy' payload or element, and the module's header says which",
            ctx.module
        )
    } else {
        String::new()
    }
}

pub(crate) fn is_module_alias(name: &str, ctx: &Ctx) -> bool {
    ctx.lookup(name).is_none()
        && !ctx.pds.contains_key(name)
        && ctx.aliases.contains_key(name)
}

/// A method call whose receiver is a local binding that shares its name with a
/// module alias in scope (`MISSING-9tRe4`). The missing-method message names the
/// local's type and never the module the call was meant to reach, which sends
/// the reader to look for a method on a type that was never going to have one.
///
/// The note is only added when the shadowed module really declares that method:
/// the sentence is a claim about where the call could have gone, so it has to be
/// true of that module, not merely of the name.
pub(crate) fn shadowed_module_alias_note(receiver: &ast::Expr, method: &str, ctx: &Ctx) -> String {
    let ast::Expr::Ident(name, _) = receiver else {
        return String::new();
    };
    if ctx.lookup(name).is_none() {
        return String::new();
    }
    let Some(prefix) = ctx.aliases.get(name) else {
        return String::new();
    };
    let key = format!("{prefix}{method}");
    // The sentence is a claim about where the call could have gone, so the
    // item has to be one the module actually exports (A-13).
    if ctx.sig_of(&key).is_none() || !exported(&key, &ctx.env) {
        return String::new();
    }
    format!(
        " '{name}' is a local binding here, so '{name}.{method}' is not the module '{name}'; rename the binding"
    )
}

/// The passdown whose storage a place expression is, if it is one: the hidden
/// parameter (`pd#<name>`, already a borrow) directly, or behind the explicit
/// `&` that a `main` or `pd` initializer takes of the generated global. Any
/// other place - a local, a field of one, a dereferenced borrow - is `None`.
/// The two shapes are why this looks through a `Ref` rather than matching the
/// identifier alone (M6.4).
pub(crate) fn pd_place_hidden(expr: &tir::Expr) -> Option<&str> {
    match &expr.kind {
        tir::ExprKind::Ident(name) if name.starts_with("pd#") => Some(name),
        tir::ExprKind::Ref { expr, .. } => pd_place_hidden(expr),
        _ => None,
    }
}

/// Assignment targets: `Ident`, a `.field` chain rooted in one, or a
/// dereferenced borrow (`*p`, `(*p).field`) - anything `is_ast_place`
/// accepts.
/// The two rules a written place obeys: it must be writable at all (`var`, an
/// exclusive borrow, ...), and a field of a `datac` must be declared `var`.
/// Shared by `=` and by an `asm` operand's `out`/`inout` place, so the two
/// cannot drift.
pub(crate) fn check_written_place(target: &tir::Expr, ctx: &Ctx) -> Result<()> {
    check_target_writable(target, ctx)?;
    // Only the *immediate* field being written needs to be `var`; fields merely
    // read through on the way there do not.
    if let tir::ExprKind::FieldAccess { base, field } = &target.kind {
        let sname = struct_name_of(&base.ty)
            .expect("field access base is always a struct type, guaranteed by check_expr");
        let sdef = ctx
            .struct_def(sname)
            .expect("struct types are only produced for known structs");
        let fdef = sdef.field(field).expect("field already resolved by check_expr");
        if !fdef.mutable {
            return Err(TypeError {
                help: None,
                message: format!("cannot write to '{field}': declared 'val'"),
                span: target.span,
            });
        }
    }
    Ok(())
}
