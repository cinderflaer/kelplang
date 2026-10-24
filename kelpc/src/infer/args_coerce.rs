use super::*;

/// Matches call-site arguments (each optionally named, `f(x = 1)`) against
/// a callee's declared parameter list, by name first and positionally for
/// the rest - shared between plain function calls and `datac` construction,
/// since both look identical at a call site.
pub(crate) fn match_args(
    what: &str,
    params: &[(String, Ty)],
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
) -> Result<Vec<tir::Expr>> {
    let (fixed, extra) = match_args_split(what, params, args, span, ctx, false)?;
    debug_assert!(extra.is_empty(), "a non-variadic call cannot have extra arguments");
    Ok(fixed)
}

/// Field shorthand (#258): in a constructor's argument list (`datac` or enum
/// variant), a bare name `host` is `host = host`. It needs a field of that name
/// and a local of that name in scope; anything else is refused here, with the
/// name in the message. Named and positional arguments are left as they are.
/// Only constructors call this: a bare name in a function call stays positional.
pub(crate) fn field_shorthand<'a>(
    what: &str,
    params: &[(String, Ty)],
    args: &'a [ast::CallArg],
    ctx: &Ctx,
) -> Result<std::borrow::Cow<'a, [ast::CallArg]>> {
    // The common call has no bare name: borrow the list, so a deep nest of
    // constructions never copies its arguments.
    if !args.iter().any(|a| a.name.is_none() && matches!(a.value, ast::Expr::Ident(..))) {
        return Ok(std::borrow::Cow::Borrowed(args));
    }
    let mut out = Vec::with_capacity(args.len());
    for a in args {
        match (&a.name, &a.value) {
            (None, ast::Expr::Ident(name, at)) => {
                if !params.iter().any(|(p, _)| p == name) {
                    return Err(TypeError {
                        help: Some(format!("write the field's own name, '<field> = {name}', or pass the value positionally")),
                        message: format!(
                            "'{name}' is shorthand for '{name} = {name}', but {what} has no field named '{name}'"
                        ),
                        span: *at,
                    });
                }
                if ctx.lookup(name).is_none() {
                    return Err(TypeError {
                        help: Some(format!("declare a local named '{name}' before the call, or write '{name} = <value>'")),
                        message: format!(
                            "'{name}' is shorthand for '{name} = {name}', but no local named '{name}' is in scope"
                        ),
                        span: *at,
                    });
                }
                out.push(ast::CallArg { name: Some(name.clone()), value: a.value.clone() });
            }
            _ => out.push(a.clone()),
        }
    }
    Ok(std::borrow::Cow::Owned(out))
}

/// `match_args`'s body. When `variadic` is set, trailing *positional*
/// arguments after the declared parameters are returned unchecked instead of
/// being refused: the caller of a variadic user `extern fun` (gap 7) checks
/// and appends them itself. A named argument that matches no parameter is
/// still refused, whatever `variadic` says.
pub(crate) fn match_args_split<'a>(
    what: &str,
    params: &[(String, Ty)],
    args: &'a [ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
    variadic: bool,
) -> Result<(Vec<tir::Expr>, Vec<&'a ast::CallArg>)> {
    let site = ctx.str_arg_site.take();
    let store = ctx.box_store_into.take();
    if args.len() != params.len() && !(variadic && args.len() > params.len()) {
        return Err(TypeError {
            help: None,
            message: format!("{what} expects {} argument(s), found {}", params.len(), args.len()),
            span,
        });
    }

    let mut named: HashMap<&str, &ast::CallArg> = HashMap::new();
    for a in args {
        if let Some(n) = &a.name {
            if named.insert(n.as_str(), a).is_some() {
                return Err(TypeError {
                    help: None,
                    message: format!("argument '{n}' specified more than once"),
                    span,
                });
            }
        }
    }

    let mut positional = args.iter().filter(|a| a.name.is_none());
    let mut result = Vec::with_capacity(params.len());
    for (pname, pty) in params {
        let arg = match named.remove(pname.as_str()) {
            Some(a) => a,
            None => positional.next().ok_or_else(|| TypeError {
                help: None,
                message: format!("{what} is missing argument '{pname}'"),
                span,
            })?,
        };
        // The label the diagnostics use: a declared parameter is quoted by
        // name, which is what a call site can see.
        result.push(check_call_arg_in(&format!("'{pname}'"), pty, &arg.value, ctx, site.as_ref(), store.as_deref())?);
    }

    if let Some((&leftover, _)) = named.iter().next() {
        return Err(TypeError {
            help: None,
            message: format!("{what} has no parameter named '{leftover}'"),
            span,
        });
    }

    // A variadic call's extra arguments have no name to be matched by, so
    // anything left is by definition positional and in order.
    let extra: Vec<&ast::CallArg> = positional.collect();
    Ok((result, extra))
}

/// Checks one call-site argument against its matched parameter type. When
/// the parameter is `Ty::Ref`, the caller never writes `&` - a plain place
/// argument (`p`, `p.x`) is auto-borrowed here (the callee's signature is
/// what decides shared vs exclusive), matching `design/syntax.md (kelp wiki)`'s "shared
/// borrow is implicit - write nothing." An argument that's already a borrow
/// (an explicit `&expr`, or forwarding an existing `Ref`-typed parameter)
/// type-matches directly instead of double-wrapping; a shared parameter
/// accepts an exclusive argument (implicit downgrade), never the reverse.
/// A call whose result *borrows from an argument* (the elision rule) cannot
/// borrow from a temporary: the temporary is freed at the end of the
/// enclosing statement, and codegen would have to drop it before the borrow
/// could be used. Argument matching itself allows a temporary for a shared
/// borrow - that is what makes `str.join(str.split(s, sep), sep)` legal - so
/// the one case where the lifetime actually matters is checked here, where
/// the callee's return type and its elided root are both known.
///
/// Only a *freshly auto-borrowed temporary* is the problem. An argument that
/// is already a borrow value (a `&String` from `l.get(i)`, a `&str` from
/// `s.as_str()`) is a pointer whose own chain `borrowck.rs` walks to its
/// root, so it is left alone.
pub(crate) fn require_place_for_borrowed_arg(
    params: &[(String, Ty)],
    ret: &Ty,
    args: &[tir::Expr],
    span: Span,
) -> Result<()> {
    if !is_borrowed_value(ret) {
        return Ok(());
    }
    let Some(idx) = borrow_root_param(params) else {
        return Ok(());
    };
    let Some(arg) = args.get(idx) else {
        return Ok(());
    };
    let tir::ExprKind::Ref { expr: inner, .. } = &arg.kind else {
        return Ok(());
    };
    // `is_durable_place`, not `is_tir_place`: a field of a temporary has an
    // address (codegen stages one) but not one that outlives the statement the
    // borrow is taken in, which is the whole question here.
    if !is_durable_place(inner) {
        return Err(TypeError {
            help: None,
            message: format!(
                "this call returns a borrow of its parameter '{}', so that argument has to be a variable or a field - a temporary would be freed before the borrow could be used; bind it to a local first (`val tmp = <the expression>`) and pass `tmp` instead",
                params[idx].0
            ),
            span,
        });
    }
    Ok(())
}

/// `check_expr(arg, Some(expected))`, except that a receiver a method call
/// already checked with no expectation is reused when its type is exactly the
/// expected one (the expectation then changes nothing about the result).
pub(crate) fn check_expr_or_memo(
    memo: Option<tir::Expr>,
    arg_expr: &ast::Expr,
    ctx: &mut Ctx,
    expected: &Ty,
) -> Result<tir::Expr> {
    match memo {
        Some(done) if done.ty == *expected => Ok(done),
        _ => check_expr(arg_expr, ctx, Some(expected)),
    }
}

/// `pname` is how the argument is named in a diagnostic - `'b'` for a
/// declared parameter, `1` for the position in a call through a function value
/// (a function *type* has no parameter names).
pub(crate) fn check_call_arg(pname: &str, pty: &Ty, arg_expr: &ast::Expr, ctx: &mut Ctx) -> Result<tir::Expr> {
    check_call_arg_in(pname, pty, arg_expr, ctx, None, None)
}

/// What a `String` argument for a `str` parameter does at one call. `Implicit`
/// borrows it exactly as `.as_str()` would (design/syntax.md (kelp wiki) 11.2); `Refused`
/// names the callee that takes no implicit borrow (a generic function, or a
/// trait method) and asks for the explicit `.as_str()`.
#[derive(Clone)]
pub(crate) enum StrArgSite {
    Implicit,
    Refused(String),
}

/// `check_call_arg` with the call's `String`-to-`str` policy. `None` keeps the
/// old rule: a `String` against a `str` parameter is a type mismatch.
pub(crate) fn check_call_arg_in(
    pname: &str,
    pty: &Ty,
    arg_expr: &ast::Expr,
    ctx: &mut Ctx,
    site: Option<&StrArgSite>,
    store: Option<&str>,
) -> Result<tir::Expr> {
    let memo = ctx.take_probe(arg_expr);
    if matches!(arg_expr, ast::Expr::Ident(..)) {
        ctx.box_sink_ok = Some(match store {
            Some(container) => BoxSink::Shared(container.to_string()),
            None => BoxSink::Move(format!("the parameter {pname}")),
        });
    }
    // A `scoped fun(...)` parameter accepts only a `fun` literal or a trailing
    // block written at this call site (`kelp wiki page language/functions.md`), never a
    // stored value. The one exception is forwarding a scoped value that is
    // already in hand - a `scoped fun(...)` parameter of this frame, or a
    // `val` bound to one - which is still inside the call.
    if matches!(pty, Ty::ScopedFn { .. }) && !matches!(arg_expr, ast::Expr::FnLit { .. }) {
        let checked = check_expr(arg_expr, ctx, None)?;
        if !matches!(checked.ty, Ty::ScopedFn { .. }) {
            return Err(TypeError {
                help: None,
                message: format!(
                    "argument {pname} has type '{}', expected '{}': a scoped parameter accepts only a function literal or a trailing block written at this call site, never a stored value",
                    checked.ty.display(),
                    pty.display()
                ),
                span: arg_expr.span(),
            });
        }
        if checked.ty != *pty {
            return Err(TypeError {
                help: None,
                message: format!(
                    "argument {pname} has type '{}', expected '{}'",
                    checked.ty.display(),
                    pty.display()
                ),
                span: arg_expr.span(),
            });
        }
        return Ok(checked);
    }
    let Ty::Ref { mutable, inner } = pty else {
        let checked = check_expr_or_memo(memo, arg_expr, ctx, pty)?;
        refuse_open_arg(&checked.ty, arg_expr)?;
        if *pty == Ty::Str && checked.ty == Ty::String {
            match site {
                Some(StrArgSite::Implicit) => {
                    // The same expression `arg.as_str()` would type to: the
                    // receiver is checked again, as a method receiver always is.
                    return check_method_call(arg_expr, "as_str", &[], &[], arg_expr.span(), ctx, None);
                }
                Some(StrArgSite::Refused(callee)) => {
                    return Err(TypeError {
                        help: None,
                        message: format!(
                            "argument {pname} has type 'String', expected 'str': {callee} takes no implicit borrow of a 'String' for a 'str' parameter; write `.as_str()` on the argument"
                        ),
                        span: arg_expr.span(),
                    })
                }
                None => {}
            }
        }
        // A `&str` view and a `str` are the same value (both are the flat
        // borrowed view type), so a view satisfies a `str` parameter
        // directly - that is what makes `str.len(s.as_str())` work (M6.3).
        if is_str_like(pty) && is_str_like(&checked.ty) {
            return Ok(checked);
        }
        // The parameter is bare (by value), so this call *moves* its argument
        // in - which a `move T` parameter and every container's
        // `push`/`set`-shaped builtin both do. A borrow cannot be moved out
        // of, so name that instead of leaving a plain type mismatch
        // (`'&String'` against `'String'`) to imply the caller forgot an `&`
        // (A-4).
        if matches!(checked.ty, Ty::Ref { .. }) && !pty.is_copy() {
            return Err(TypeError {
                help: None,
                message: format!(
                    "argument {pname} has type '{}', but this call moves its argument in, and a borrow cannot be moved out of - pass an owned value (a 'Box' holding one moves it out with 'take()'){}",
                    checked.ty.display(),
                    stdlib_instantiation_note(ctx)
                ),
                span: arg_expr.span(),
            });
        }
        if checked.ty != *pty {
            if let Some(why) = closure_kind_mismatch(pty, &checked.ty) {
                return Err(TypeError { message: why, span: arg_expr.span(), help: None });
            }
            return Err(TypeError {
                help: None,
                message: format!(
                    "argument {pname} has type '{}', expected '{}'{}{}",
                    checked.ty.display(),
                    pty.display(),
                    char_integer_hint(&checked.ty, pty),
                    str_view_hint(&checked.ty, pty)
                ),
                span: arg_expr.span(),
            });
        }
        return Ok(checked);
    };

    let checked = check_expr_or_memo(memo, arg_expr, ctx, inner)?;
    refuse_open_arg(&checked.ty, arg_expr)?;
    match &checked.ty {
        t if t == inner.as_ref() => {
            // A *shared* borrow may be taken of a temporary (`str.join(parts
            // = str.split(s, sep), sep = "-")`): the temporary lives for the
            // whole enclosing expression, which is exactly how long a
            // call-scoped borrow lasts, and the removed C backend hoists it into a C
            // temporary to take its address. An *exclusive* borrow still
            // needs a place to write into, and `is_durable_place` is the
            // question: a temp staged for the call would swallow the write,
            // and a field of one would take the write down with it.
            if *mutable {
                if !is_durable_place(&checked) {
                    return Err(TypeError {
                        help: None,
                        message: format!("argument {pname} must be a variable or field, not a temporary value"),
                        span: arg_expr.span(),
                    });
                }
                ctx.note_self_write(&checked)?;
                check_target_writable(&checked, ctx)?;
            }
            let span = checked.span;
            Ok(tir::Expr {
                kind: tir::ExprKind::Ref { mutable: *mutable, expr: Box::new(checked) },
                ty: pty.clone(),
                span,
            })
        }
        Ty::Ref { mutable: a_mut, inner: a_inner } if a_inner == inner => {
            if *mutable && !a_mut {
                note_excl_need(&checked, ctx);
                return Err(TypeError {
                    help: None,
                    message: format!("argument {pname} requires an exclusive borrow, found a shared one"),
                    span: arg_expr.span(),
                });
            }
            if *mutable {
                ctx.note_self_write(&checked)?;
            }
            Ok(checked)
        }
        other => Err(TypeError {
            help: None,
            message: closure_kind_mismatch(pty, other).unwrap_or_else(|| {
                format!(
                    "argument {pname} has type '{}', expected '{}'{}{}",
                    other.display(),
                    pty.display(),
                    char_integer_hint(other, pty),
                    str_view_hint(other, inner)
                )
            }),
            span: arg_expr.span(),
        }),
    }
}

/// Whether some trait declares a method of this name. A `str` parameter of
/// such a method takes no implicit borrow (`StrArgSite::Refused`).
pub(crate) fn trait_declares_method(ctx: &Ctx, method: &str) -> bool {
    ctx.env.traits.values().any(|t| t.methods.iter().any(|m| m.name == method))
}

/// The one widening a function value gets (A-4 phase E): a plain `fun` value
/// used where a closure is expected is wrapped, so the two are separate types
/// but one *value* can be written either way. A signature mismatch is left
/// alone - the caller's own type comparison reports it, and a coerced value
/// whose signature does not fit would hide which half was wrong.
pub(crate) fn coerce_fn_to_closure(expected: Ty, value: tir::Expr) -> tir::Expr {
    let Ty::Closure { params, ret } = &expected else {
        return value;
    };
    let Ty::Fn { params: actual_params, ret: actual_ret } = &value.ty else {
        return value;
    };
    if actual_params != params || actual_ret != ret {
        return value;
    }
    let span = value.span;
    tir::Expr {
        kind: tir::ExprKind::FnToClosure {
            func: Box::new(value),
        },
        ty: expected.clone(),
        span,
    }
}

/// The same widening for a scoped function value: a literal that captured
/// nothing is a plain `fun` pointer, and the `scoped fun(...)` parameter takes
/// the fat pointer, so it is wrapped with `env = NULL` (the shim path) exactly
/// as a plain function value widened into a closure is. Every other scoped
/// value already has the fat-pointer type, because `check_fn_literal` builds a
/// `ScopedClosureLit` for one that captured.
pub(crate) fn coerce_fn_to_scoped(expected: Ty, value: tir::Expr) -> tir::Expr {
    let Ty::ScopedFn { params, ret } = &expected else {
        return value;
    };
    let Ty::Fn { params: actual_params, ret: actual_ret } = &value.ty else {
        return value;
    };
    if actual_params != params || actual_ret != ret {
        return value;
    }
    let span = value.span;
    tir::Expr {
        kind: tir::ExprKind::FnToClosure {
            func: Box::new(value),
        },
        ty: expected.clone(),
        span,
    }
}

/// The name a `Ty` answers to in the trait-impl table (A-12 slice 4): a
/// `datac`/`enum`'s canonical key, or the written name of a builtin
/// (`i32`, `String`, `AtomicI64`). `None` for a type no trait can be
/// implemented for, which is what makes a coercion from one an error rather
/// than a vtable that names nothing.
pub(crate) fn dyn_target_name(ty: &Ty) -> Option<String> {
    match ty {
        Ty::Struct(name) => Some(name.clone()),
        Ty::Atomic(elem) => crate::types::atomic_alias_name(elem).map(str::to_string),
        other => builtin_type_name(other).map(str::to_string),
    }
}

/// The one coercion this slice adds (A-12 slice 4, `design/traits.md (kelp wiki)` 9.3): a
/// `&T` where `T` implements `Trait` becomes a `&dyn Trait`, and a `&mut T`
/// becomes either spelling. The value is a fat pair, and the coercion records
/// the `(trait, type)` pair so the vtable list knows to emit one.
pub(crate) fn coerce_to_dyn(want: Ty, value: tir::Expr, span: Span, ctx: &Ctx) -> Result<tir::Expr> {
    let Ty::Dyn { mutable: want_mut, trait_name } = &want else {
        return Ok(value);
    };
    if value.ty == want {
        return Ok(value);
    }
    // A `&dyn Name` value retypes to another spelling of the same trait (so a
    // `&mut dyn` forwards to a `&dyn` parameter), but never to a different
    // trait: there is no upcast in this slice.
    if let Ty::Dyn { mutable: src_mut, trait_name: src_trait } = &value.ty {
        if src_trait != trait_name {
            return Err(TypeError {
                help: None,
                message: format!(
                    "expected '{}', found '{}': a trait object does not convert to another trait's ('{}' to '{}'); take a bound on a type parameter, or coerce from the concrete value",
                    want.display(),
                    value.ty.display(),
                    src_trait,
                    trait_name
                ),
                span,
            });
        }
        if *want_mut && !*src_mut {
            return Err(TypeError {
                help: None,
                message: format!(
                    "expected '{}', found '{}': a shared borrow cannot become an exclusive trait object",
                    want.display(),
                    value.ty.display()
                ),
                span,
            });
        }
        let mut retyped = value;
        retyped.ty = want;
        return Ok(retyped);
    }
    let Ty::Ref { mutable: src_mut, inner } = &value.ty else {
        return Err(TypeError {
            help: None,
            message: format!(
                "expected '{}', found '{}': a trait object is built from a borrow, so write '&value' (or '&mut value' for '{want}')",
                want.display(),
                value.ty.display(),
                want = want.display()
            ),
            span,
        });
    };
    if *want_mut && !*src_mut {
        return Err(TypeError {
            help: None,
            message: format!(
                "expected '{}', found '{}': a shared borrow cannot become an exclusive trait object",
                want.display(),
                value.ty.display()
            ),
            span,
        });
    }
    // An `&mut` operand needs a place to write into, exactly as an `&mut`
    // argument of a declared parameter does: a temporary staged for the call
    // would swallow the write, and a field of one would take the write down
    // with it. A value that is already a borrow (a `mut T` parameter read, or
    // a call's result) is a pointer in hand and needs no such place.
    if *src_mut {
        if let tir::ExprKind::Ref { expr: place, .. } = &value.kind {
            if !is_durable_place(place) {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "expected '{}', found '{}': an exclusive borrow needs a variable or field to write through, not a temporary value",
                        want.display(),
                        value.ty.display()
                    ),
                    span,
                });
            }
            check_target_writable(place, ctx)?;
        }
    }
    let Some(type_name) = dyn_target_name(inner) else {
        return Err(TypeError {
            help: None,
            message: format!(
                "expected '{}', found '{}': '{}' cannot be the value behind a trait object",
                want.display(),
                value.ty.display(),
                inner.display()
            ),
            span,
        });
    };
    let implements = ctx
        .env
        .impls
        .get(trait_name)
        .is_some_and(|set| set.contains(&type_name));
    if !implements {
        if blanket_holds(&ctx.env, &trait_name.to_string(), &type_name.to_string()) {
            return Err(blanket_dyn_refusal(&want, &value.ty, &inner, trait_name, span));
        }
        return Err(TypeError {
            help: None,
            message: format!(
                "expected '{}', found '{}': '{}' does not implement '{}', so write 'impl {} for {}' first",
                want.display(),
                value.ty.display(),
                inner.display(),
                trait_name,
                trait_name,
                inner.display()
            ),
            span,
        });
    }
    ctx.env
        .dyn_pairs
        .borrow_mut()
        .insert((trait_name.clone(), type_name.clone()));
    Ok(tir::Expr {
        kind: tir::ExprKind::DynCoerce {
            trait_name: trait_name.clone(),
            type_name,
            expr: Box::new(value),
        },
        ty: want,
        span,
    })
}

/// The owned half of the coercion (A-12 slice 7): a `Box<T>` where `T`
/// implements `Trait` becomes a `Box<dyn Trait>`. The box's allocation becomes
/// the trait object's, so this is a *move* of the operand (its binding is moved
/// out exactly as any other read of an owned value is) and no second allocation
/// happens. The `(trait, type)` pair is recorded, so the vtable and its drop
/// entry are emitted for the coercion.
pub(crate) fn coerce_box_to_dyn(
    want: Ty,
    value: tir::Expr,
    span: Span,
    ctx: &mut Ctx,
) -> Result<tir::Expr> {
    let Ty::BoxDyn { trait_name } = &want else {
        return Ok(value);
    };
    if value.ty == want {
        return Ok(value);
    }
    // A `Box<dyn Name>` retypes to the same type and never to another trait's:
    // there is no upcast in this slice, exactly as for a `&dyn`.
    if let Ty::BoxDyn { trait_name: src_trait } = &value.ty {
        return Err(TypeError {
            help: None,
            message: format!(
                "expected '{}', found '{}': an owned trait object does not convert to another trait's ('{}' to '{}'); take a bound on a type parameter, or coerce from the concrete value",
                want.display(),
                value.ty.display(),
                src_trait,
                trait_name
            ),
            span,
        });
    }
    let Ty::Box(inner) = &value.ty else {
        return Err(TypeError {
            help: None,
            message: format!(
                "expected '{}', found '{}': an owned trait object is built from a box of a value that implements the trait ('Box.of(v)'), and this is not a box",
                want.display(),
                value.ty.display()
            ),
            span,
        });
    };
    let Some(type_name) = dyn_target_name(inner) else {
        return Err(TypeError {
            help: None,
            message: format!(
                "expected '{}', found '{}': '{}' cannot be the value behind an owned trait object",
                want.display(),
                value.ty.display(),
                inner.display()
            ),
            span,
        });
    };
    let implements = ctx
        .env
        .impls
        .get(trait_name)
        .is_some_and(|set| set.contains(&type_name));
    if !implements {
        if blanket_holds(&ctx.env, &trait_name.to_string(), &type_name.to_string()) {
            return Err(blanket_dyn_refusal(&want, &value.ty, &inner, trait_name, span));
        }
        return Err(TypeError {
            help: None,
            message: format!(
                "expected '{}', found '{}': '{}' does not implement '{}', so write 'impl {} for {}' first",
                want.display(),
                value.ty.display(),
                inner.display(),
                trait_name,
                trait_name,
                inner.display()
            ),
            span,
        });
    }
    ctx.env
        .dyn_pairs
        .borrow_mut()
        .insert((trait_name.clone(), type_name.clone()));
    Ok(tir::Expr {
        kind: tir::ExprKind::BoxDynCoerce {
            trait_name: trait_name.clone(),
            type_name,
            expr: Box::new(value),
        },
        ty: want,
        span,
    })
}

/// The one sentence that explains why a plain function value and a closure are
/// different types, when one is passed where the other is expected (A-4 phase
/// E). The two are the same shape of value and are called identically, so a
/// bare `expected 'closure(i32) => i32', found 'fun(i32) => i32'` would leave
/// the reader to guess which half is missing. The expected side is looked
/// through a borrow, because a parameter of a closure type is one.
pub(crate) fn closure_kind_mismatch(expected: &Ty, actual: &Ty) -> Option<String> {
    let expected = match expected {
        Ty::Ref { inner, .. } => inner.as_ref(),
        other => other,
    };
    match (expected, actual) {
        (Ty::Closure { .. }, Ty::Fn { .. }) => Some(
            "this is a plain function value, not a closure: its body captured nothing, so it carries no environment - a `fun` literal becomes a closure when it names a binding from the enclosing scope, and a declared `fun` is always a plain function value".to_string(),
        ),
        (Ty::Fn { .. }, Ty::Closure { .. }) => Some(
            "this is a closure: its body captured a binding from the enclosing scope, so it carries an environment a plain function value has no room for - write the expected type as 'closure(...)' (the same parameter list and '=> '), or pass a declared function instead".to_string(),
        ),
        (Ty::Fn { .. } | Ty::Closure { .. }, Ty::ScopedFn { .. }) => Some(
            "this is a scoped function value: it may only be passed to another 'scoped fun(...)' parameter, because it may not outlive the call it was written in - a plain function or closure parameter could store it or return it".to_string(),
        ),
        (Ty::ScopedFn { .. }, Ty::Fn { .. } | Ty::Closure { .. }) => Some(
            "this parameter takes a plain function or closure value, and what was passed is a scoped one: a scoped literal may only be written at the call site of a 'scoped fun(...)' parameter".to_string(),
        ),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// MK9-S3-M1 stages B1 and C1: `thread.spawn` / `thread.join`. B1 landed the
// checker surface and the rules of MK9 section 5; C1 lowers both, so the two
// calls below build the `tir` node each backend translates into a real worker.
// ---------------------------------------------------------------------------

/// One of the two thread calls, dispatched on the key `check_call` matched.
pub(crate) fn check_thread_call(
    callee: &str,
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
) -> Result<tir::Expr> {
    if callee == crate::stdlib::SPAWN_FN {
        check_thread_spawn(args, span, ctx, expected)
    } else {
        check_thread_join(args, span, ctx, expected)
    }
}

/// `thread.spawn(entry, arg)` (MK9-S3-M1 stages B1 and C1). `entry` is a
/// declared `fun`, a `fun` literal, or a capturing closure, taking exactly one
/// parameter; `arg` is the value moved into the worker as that parameter, and
/// the handle's element is `entry`'s **return** type, which is the value the
/// worker hands its joiner (MK9 section 4).
///
/// The refusals are MK9 section 5's rules 1 to 4, each naming its own reason.
/// The accepted path is a `tir::Call` under this module's own key, which is what
/// both backends recognize to emit the worker and the trampoline that calls it.
pub(crate) fn check_thread_spawn(
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
    _expected: Option<&Ty>,
) -> Result<tir::Expr> {
    let slots = arg_slots(
        "function 'thread.spawn'",
        &["entry".to_string(), "arg".to_string()],
        args,
        span,
    )?;
    let entry_arg = &args[slots[0]];
    let value_arg = &args[slots[1]];

    // A `fun` literal written directly as the entry may capture resources
    // (decision 127). Anything else that is the entry keeps the closure rule.
    let literal_entry = matches!(entry_arg.value, ast::Expr::FnLit { .. });
    ctx.spawn_entry_next = literal_entry;
    let entry = check_expr(&entry_arg.value, ctx, None);
    ctx.spawn_entry_next = false;
    let entry = entry?;
    let (params, ret) = match &entry.ty {
        Ty::Fn { params, ret } | Ty::Closure { params, ret } => (params.clone(), (**ret).clone()),
        other => {
            return Err(TypeError {
                help: None,
                message: format!(
                    "'thread.spawn' takes a function or closure value as its entry point, found '{}': a declared 'fun', a 'fun' literal and a capturing closure all work, because a function value is widened where a closure is expected",
                    other.display()
                ),
                span: entry_arg.value.span(),
            })
        }
    };
    // One parameter, because the worker is handed exactly one value: the `arg`
    // moved in as that parameter (MK9 section 5 rule 1).
    if params.len() != 1 {
        return Err(TypeError {
            help: None,
            message: format!(
                "'thread.spawn''s entry point has to take exactly one parameter, the value moved into the worker, and this one takes {}",
                params.len()
            ),
            span: entry_arg.value.span(),
        });
    }
    let moved = params[0].clone();
    // A plain function value is widened into a closure here rather than at each
    // backend, so `entry` has one shape downstream: the ordinary A-4 phase E
    // widening, a `fun` value wrapped in the one-field environment a closure's
    // code pointer is called with.
    let entry = coerce_fn_to_closure(
        Ty::Closure {
            params: params.clone(),
            ret: Box::new(ret.clone()),
        },
        entry,
    );

    let value = check_expr(&value_arg.value, ctx, Some(&moved))?;
    check_thread_arg(&value_arg.value, &value, ctx)?;
    note_heap_use(&ctx.env, managed::HeapSlot::ThreadArgument, &value.ty, value_arg.value.span());
    note_heap_use(&ctx.env, managed::HeapSlot::ThreadResult, &ret, span);
    if value.ty != moved {
        return Err(TypeError {
            help: None,
            message: format!(
                "'thread.spawn''s argument has type '{}', and its entry point takes '{}' by move: the worker's parameter is the value moved in, so the two have to be the same type",
                value.ty.display(),
                moved.display()
            ),
            span: value_arg.value.span(),
        });
    }

    Ok(tir::Expr {
        kind: tir::ExprKind::Call {
            callee: crate::stdlib::SPAWN_FN.to_string(),
            args: vec![entry, value],
        },
        ty: Ty::Thread(Box::new(ret)),
        span,
    })
}

/// `thread.join(handle)` (MK9-S3-M1 stages B1 and C1). The handle is a
/// `Thread<T>`, the joiner consumes it, and the result is
/// `Result<T, ThreadErr>`: `Ok` the worker's value, `Err` why it ended without
/// one (MK9 section 2).
pub(crate) fn check_thread_join(
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
    _expected: Option<&Ty>,
) -> Result<tir::Expr> {
    let slots = arg_slots(
        "function 'thread.join'",
        &["handle".to_string()],
        args,
        span,
    )?;
    let handle_arg = &args[slots[0]];
    let handle = check_expr(&handle_arg.value, ctx, None)?;
    let elem = match &handle.ty {
        Ty::Thread(elem) => (**elem).clone(),
        other => {
            return Err(TypeError {
                help: None,
                message: format!(
                    "'thread.join' takes a 'Thread<T>' handle, found '{}': a handle comes from 'thread.spawn', which is the only call that makes one",
                    other.display()
                ),
                span: handle_arg.value.span(),
            })
        }
    };

    // The result names the shipped `ThreadErr` through this module's key
    // exactly as the signature states, and a program that never imported
    // `std/result` is told to rather than given an internal error.
    let result_key = format!(
        "{}Result",
        crate::stdlib::key_prefix(crate::stdlib::RESULT_MODULE)
    );
    if !ctx.env.generics.contains_key(&result_key) {
        return Err(TypeError {
            help: None,
            message: format!(
                "'thread.join' hands back 'Result<T, ThreadErr>', which '{}' declares: add `impl \"{}\"` to the file that writes the call",
                result_key,
                crate::stdlib::RESULT_MODULE
            ),
            span,
        });
    }
    let joined = ctx.instantiate_type(&result_key, &[elem, thread_err_ty()], span)?;

    Ok(tir::Expr {
        kind: tir::ExprKind::Call {
            callee: crate::stdlib::JOIN_FN.to_string(),
            args: vec![handle],
        },
        ty: joined,
        span,
    })
}
