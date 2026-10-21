use super::*;

/// Whether a concrete type's associated type meets a bound (MISSING-Ve8Nc
/// step 5): the type binds `assoc`, and that type implements `key`.
pub(crate) enum ProjectionCheck {
    Holds,
    NoItem,
    Unmet(Ty),
}

pub(crate) fn projection_check(env: &TypeEnv, arg: &Ty, assoc: &str, key: &str) -> ProjectionCheck {
    let Some(item) = assoc_type_of(env, arg, assoc) else {
        return ProjectionCheck::NoItem;
    };
    let name = struct_name_of(&item)
        .map(|s| s.to_string())
        .or_else(|| builtin_type_name(&item).map(|s| s.to_string()));
    match name {
        Some(n) if implements_trait(env, key, &n) => ProjectionCheck::Holds,
        _ => ProjectionCheck::Unmet(item),
    }
}

/// Checks one `where P::Assoc: Bound` clause of a function at a call: the bound
/// is on the concrete associated type, so `I::Item` is checked where `I` is
/// bound (MISSING-Ve8Nc step 5).
pub(crate) fn check_projection_bound(
    ctx: &Ctx,
    tmpl: &FuncTemplate,
    type_args: &[Ty],
    pb: &ast::ProjectionBound,
    span: Span,
) -> Result<()> {
    let callee = display_callee(&tmpl.key);
    let written = format!("{}::{}", pb.param, pb.assoc);
    let Some(pi) = tmpl.params.iter().position(|p| *p == pb.param) else {
        return Ok(());
    };
    let Some(arg) = type_args.get(pi) else {
        return Ok(());
    };
    if !pb.bound.args.is_empty() {
        return Err(TypeError {
            help: None,
            message: format!(
                "'where {written}: {}' bounds a projection with trait arguments, which is not in this slice",
                pb.bound.display()
            ),
            span,
        });
    }
    let Some(key) = qualify_written_name(&pb.bound.name, &tmpl.prefix, &tmpl.aliases) else {
        return Err(TypeError {
            help: None,
            message: format!(
                "'{}' bounds '{written}' of '{callee}', but its first segment is not a module the declaration imports",
                pb.bound.name
            ),
            span,
        });
    };
    if !ctx.env.traits.contains_key(&key) {
        return Err(TypeError {
            help: None,
            message: format!(
                "'{}' is not a trait, so it cannot bound '{written}' of '{callee}': a bound names a declared trait ('trait {} {{ ... }}')",
                pb.bound.name, pb.bound.name
            ),
            span,
        });
    }
    match projection_check(&ctx.env, arg, &pb.assoc, &key) {
        ProjectionCheck::Holds => Ok(()),
        ProjectionCheck::NoItem => Err(TypeError {
            help: None,
            message: format!(
                "the call to '{callee}' bounds '{written}' with '{}', but '{}' binds no associated type '{}': a projection bound needs a type that binds it, such as an iterator",
                pb.bound.display(),
                arg.display(),
                pb.assoc
            ),
            span,
        }),
        ProjectionCheck::Unmet(item) => Err(TypeError {
            help: None,
            message: format!(
                "the call to '{callee}' needs '{written}', which is '{}' for '{}', to implement the trait '{}', and it does not: write 'impl {} for {} {{ ... }}' in the trait's module or in the type's",
                item.display(),
                arg.display(),
                pb.bound.display(),
                pb.bound.display(),
                item.display()
            ),
            span,
        }),
    }
}

/// The refusal for a `dyn` coercion of a type whose trait comes from a blanket
/// impl: a blanket impl has no vtable of its own yet, so the method is reached
/// through a generic function instead.
pub(crate) fn blanket_dyn_refusal(
    want: &Ty,
    value_ty: &Ty,
    inner: &Ty,
    trait_name: &str,
    span: Span,
) -> TypeError {
    TypeError {
        help: None,
        message: format!(
            "expected '{}', found '{}': '{}' implements '{}' through a blanket impl, and a blanket impl has no 'dyn' form yet (MISSING-Ve8Nc step 4 does not include it): call the method through a generic function ('fun f<T: {}>(x: &T)') instead",
            want.display(),
            value_ty.display(),
            inner.display(),
            trait_name,
            trait_name
        ),
        span,
    }
}

/// Whether a builtin receiver carries type arguments its impl table cannot see
/// (`List<i32>` answers to the name `List` for every element type).
pub(crate) fn builtin_arguments_unchecked(ty: &Ty) -> bool {
    matches!(
        ty,
        Ty::List(_) | Ty::Box(_) | Ty::Array(..) | Ty::Map(..) | Ty::Atomic(_) | Ty::Mutex(_) | Ty::RwLock(_)
    )
}

/// The template of a blanket impl's method `method` that applies to the receiver
/// `ty`, if one does. `None` when no blanket impl writes the method, or none of
/// them applies to `ty`. `Some(Err)` when two blanket impls of different traits
/// both apply (refused by name), or when the receiver has no name to check the
/// bound against (a reference or a function type has no impl table).
pub(crate) fn blanket_method_template(ctx: &Ctx, ty: &Ty, method: &str, span: Span) -> Option<Result<String>> {
    let candidates: Vec<&BlanketImpl> = ctx
        .env
        .blanket_impls
        .iter()
        .filter(|b| b.methods.iter().any(|m| m == method))
        .collect();
    if candidates.is_empty() {
        return None;
    }
    let Some(name) = struct_name_of(ty)
        .map(|s| s.to_string())
        .or_else(|| builtin_type_name(ty).map(|s| s.to_string()))
    else {
        let b = candidates[0];
        let bound = b.bounds.first().map(|k| k.as_str()).unwrap_or(b.trait_name.as_str());
        return Some(Err(TypeError {
            help: None,
            message: format!(
                "the method '{method}' comes from a blanket impl of '{}', whose bound on the receiver ('{bound}') cannot be checked on '{}': a reference or a function type has no impl table to check a bound against, so call the method on the value it names",
                b.trait_name,
                ty.display()
            ),
            span,
        }));
    };
    let holding: Vec<&BlanketImpl> = candidates
        .iter()
        .copied()
        .filter(|b| b.bounds.iter().all(|k| implements_trait(&ctx.env, k, &name)))
        .collect();
    // A builtin container's impl table is keyed by its bare name (`List`), so a
    // bound on its element cannot be answered for `List<i32>`. A bound that
    // fails by name there is refused, not read as "does not apply".
    if holding.is_empty() && builtin_arguments_unchecked(ty) {
        let b = candidates[0];
        let bound = b.bounds.first().map(|k| k.as_str()).unwrap_or(b.trait_name.as_str());
        return Some(Err(TypeError {
            help: None,
            message: format!(
                "the method '{method}' comes from a blanket impl of '{}', whose bound '{bound}' on the receiver cannot be checked on '{}': a builtin container's impl table has no entry per element type, so the bound on its element is not visible to the check",
                b.trait_name,
                ty.display()
            ),
            span,
        }));
    }
    match holding.as_slice() {
        [] => None,
        [one] => Some(Ok(format!("{}#{method}", crate::types::blanket_receiver_name(&one.trait_key)))),
        [a, b, ..] => Some(Err(TypeError {
            help: None,
            message: format!(
                "'{}.{method}()' is ambiguous: the blanket impls of '{}' and '{}' both provide '{method}' for it, and a method is not chosen between traits by name",
                name, a.trait_name, b.trait_name
            ),
            span,
        })),
    }
}

/// The template key of a method declared on a generic type (A-2): `owner` is
/// the receiver's *instance* key (`Pair<i32, String>`), and the template is
/// declared under the type template's own name (`Pair#first`).
pub(crate) fn method_template_key(ctx: &Ctx, owner: &str, method: &str) -> Option<String> {
    // A method with type parameters of its own on a plain type is a template
    // under the type's own name.
    let plain = format!("{owner}#{method}");
    if ctx.fns.templates.contains_key(&plain) {
        return Some(plain);
    }
    let type_tmpl = ctx
        .env
        .instances
        .borrow()
        .origin
        .get(owner)
        .map(|(t, _)| t.clone())?;
    let key = format!("{type_tmpl}#{method}");
    ctx.fns.templates.contains_key(&key).then_some(key)
}

/// The associated-type projections a generic instantiation makes available
/// (A-12 slice 5): for each type parameter and each trait it is bounded by, the
/// concrete type the argument's impl bound for every associated type the trait
/// declares. The key is the written projection (`I::Item`), which is exactly
/// what `TypeScope::resolve` looks up when it meets one.
pub(crate) fn assoc_projection_bindings(
    env: &TypeEnv,
    params: &[String],
    bounds: &[Vec<ast::TypeExpr>],
    prefix: &str,
    aliases: &HashMap<String, String>,
    type_args: &[Ty],
) -> HashMap<String, Ty> {
    let mut out: HashMap<String, Ty> = HashMap::new();
    for (i, bs) in bounds.iter().enumerate() {
        let (Some(param), Some(arg)) = (params.get(i), type_args.get(i)) else {
            continue;
        };
        let Some(arg_name) = struct_name_of(arg)
            .map(|s| s.to_string())
            .or_else(|| builtin_type_name(arg).map(|s| s.to_string()))
        else {
            continue;
        };
        for b in bs {
            let Some(key) = qualify_written_name(&b.name, prefix, aliases) else {
                continue;
            };
            for tk in trait_keys_with_supertraits(env, &key) {
                if let Some(bindings) = env.assoc.get(&(tk.clone(), arg_name.clone())) {
                    for (item, ty) in bindings {
                        out.entry(format!("{param}::{item}"))
                            .or_insert_with(|| ty.clone());
                    }
                } else if let Some((gi, targs)) = generic_impl_applies(env, &tk, &arg_name) {
                    for (item, te) in &gi.assoc_types {
                        if let Some(ty) = resolve_impl_assoc(te, &gi.params, &targs, env) {
                            out.entry(format!("{param}::{item}")).or_insert(ty);
                        }
                    }
                } else if tk == ITERATOR_TRAIT_KEY {
                    if let Some(ty) = shaped_iterator_item(env, &arg_name) {
                        out.entry(format!("{param}::Item")).or_insert(ty);
                    }
                }
            }
        }
    }
    out
}

/// Instantiates a generic function for one call site (A-2), returning the
/// instance's key and its concrete signature. The type arguments come from
/// three places, in this order: the receiver's own instantiation (a method),
/// the expected type, and the argument types. The signature is registered
/// *before* the body is queued, so a recursive call inside the body resolves
/// to the same instance instead of instantiating again.
pub(crate) fn instantiate_function(
    ctx: &mut Ctx,
    tmpl_key: &str,
    explicit: &[Ty],
    args: &[ast::CallArg],
    span: Span,
    expected: Option<&Ty>,
    receiver_ty: Option<&Ty>,
) -> Result<(String, FnSig)> {
    let tmpl = ctx.fns.templates[tmpl_key].clone();
    let mut bindings: HashMap<String, Ty> = HashMap::new();
    // A method's type parameters are the receiver type's, which the receiver
    // expression already resolved.
    let blanket = tmpl
        .receiver
        .as_ref()
        .is_some_and(|(name, _)| crate::types::is_blanket_receiver(name));
    if tmpl.receiver.is_some() {
        if blanket {
            // A blanket impl's parameter is the receiver type itself, whatever
            // it is (a builtin included), not an instance's argument.
            if let (Some(rt), Some(p)) = (receiver_ty, tmpl.params.first()) {
                bindings.insert(p.clone(), rt.clone());
            }
        } else if let Some(rk) = receiver_ty.and_then(struct_name_of) {
            if let Some((_, targs)) = ctx.env.instances.borrow().origin.get(rk).cloned() {
                for (p, a) in tmpl.params.iter().zip(targs.iter()) {
                    bindings.insert(p.clone(), a.clone());
                }
            }
        }
    }
    let what = format!("function '{}'", display_callee(&tmpl.key));
    // With an explicit instantiation there is nothing to infer - the arguments
    // are still matched, but by `check_call`, against the signature this
    // produces. Otherwise the expected return type and the argument types
    // supply the parameters.
    if explicit.is_empty() {
        if let (Some(written), Some(exp)) = (&tmpl.ret, expected) {
            bind_type_params(&ctx.env, &tmpl.prefix, &tmpl.aliases, written, exp, &tmpl.params, &mut bindings);
        }
        if tmpl.declared.is_empty() {
            if !args.is_empty() {
                return Err(TypeError {
                    help: None,
                    message: format!("{what} expects 0 argument(s), found {}", args.len()),
                    span,
                });
            }
        } else {
            let names: Vec<String> = tmpl.declared.iter().map(|p| p.name.clone()).collect();
            let slots = arg_slots(&what, &names, args, span)?;
            for (i, p) in tmpl.declared.iter().enumerate() {
                let hint = field_hint(ctx, &tmpl.prefix, &tmpl.aliases, &p.ty, &bindings);
                let probed = check_expr(&args[slots[i]].value, ctx, hint.as_ref())?;
                bind_type_params(&ctx.env, &tmpl.prefix, &tmpl.aliases, &p.ty, &probed.ty, &tmpl.params, &mut bindings);
                ctx.keep_probe(&args[slots[i]].value, probed);
            }
        }
    }
    let mut type_args = if explicit.is_empty() {
        finish_type_args(&tmpl.key, &tmpl.params, &bindings, span)?
    } else {
        check_arity(&tmpl.key, &tmpl.params, explicit, span)?
    };
    // A-20: re-type every const argument to its parameter's declared type and
    // range-check it, exactly as a type instantiation does.
    bind_const_args(&tmpl.params, &tmpl.const_params, &mut type_args, span)?;
    // A-12 slice 2: the bound is checked before the instance is registered, and
    // before the early return below, so every call/instantiation of the
    // template answers it. The body is then checked per concrete type exactly
    // as it was before bounds existed, which is what keeps dispatch static.
    check_trait_bounds(ctx, &tmpl, &type_args, span)?;
    let key = instance_key(&tmpl.key, &type_args);
    if let Some(sig) = ctx.sig_of(&key) {
        return Ok((key, sig));
    }
    // The parameter and return types, resolved from the *final* arguments -
    // whichever way they were determined, so an explicit instantiation needs
    // no inference bindings at all. A failure here is a problem in the
    // template's own signature, written in the template's file, so the
    // message names the instance and the span points at the use site, which
    // is the module the caller will attribute the error to.
    let mut resolved: HashMap<String, Ty> = tmpl
        .params
        .iter()
        .cloned()
        .zip(type_args.iter().cloned())
        .collect();
    resolved.extend(assoc_projection_bindings(
        &ctx.env,
        &tmpl.params,
        &tmpl.bounds,
        &tmpl.prefix,
        &tmpl.aliases,
        &type_args,
    ));
    let scope = TypeScope {
        structs: &ctx.structs,
        enums: &ctx.enums,
        env: ctx.env.clone(),
        params: &resolved,
        prefix: &tmpl.prefix,
        aliases: &tmpl.aliases,
        later_datacs: None,
        in_progress: None,
        consts: Some(&ctx.consts),
        const_fns: Some(&ctx.const_fns),
        handles: &ctx.env.handles,
    };
    let mut params = Vec::new();
    if let Some((_, receiver_mutable)) = &tmpl.receiver {
        let self_ty = if blanket {
            receiver_ty
                .cloned()
                .expect("a blanket method is only instantiated for a receiver")
        } else {
            let rk = receiver_ty
                .and_then(struct_name_of)
                .expect("a receiver method is only instantiated for a struct receiver");
            Ty::Struct(rk.to_string())
        };
        params.push(("self".to_string(), implicit_borrow_wrap(self_ty, *receiver_mutable)));
    }
    for p in &tmpl.declared {
        let ty = scope.resolve_param(p).map_err(|e| TypeError {
            help: None,
            message: format!("in '{key}': {}", e.message),
            span,
        })?;
        params.push((p.name.clone(), ty));
    }
    let ret = match &tmpl.ret {
        Some(t) => scope.resolve_return(t).map_err(|e| TypeError {
            help: None,
            message: format!("in '{key}': {}", e.message),
            span,
        })?,
        None => Ty::Unit,
    };
    let sig = FnSig { params, ret };
    let mut st = ctx.fns.instances.borrow_mut();
    if st.sigs.len() >= MAX_FUNCTION_INSTANTIATIONS {
        return Err(TypeError {
            help: None,
            message: format!(
                "generic instantiation did not terminate - more than {MAX_FUNCTION_INSTANTIATIONS} function instances were created; a generic function that calls itself with a different type argument grows without bound"
            ),
            span,
        });
    }
    st.sigs.insert(key.clone(), sig.clone());
    st.origin.insert(key.clone(), tmpl.key.clone());
    st.pending.push_back((key.clone(), type_args));
    Ok((key, sig))
}

/// Argument-type overloading (M6.3, stdlib decision 4). Picks the single
/// declaration whose parameter types match the first argument's type. Every
/// parameter of one overload has the same type, so which argument is probed
/// does not matter; and the rule exists only for compiler-shipped modules,
/// so user code has no overload set to resolve against in the first place.
pub(crate) fn resolve_overload(
    callee: &str,
    owned_key: &str,
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
) -> Result<Option<(String, FnSig)>> {
    if args.is_empty() {
        return Ok(None);
    }
    // The calling module's own spelling first, for the same reason
    // `check_call` tries it first.
    let bases = [owned_key, callee];
    if !bases.iter().any(|b| ctx.overloads.contains_key(*b)) {
        return Ok(None);
    }
    // A type-only probe: `check_expr` has no persistent side effects beyond
    // reading `ctx`, the same assumption `check_method_call` makes.
    let probe = check_expr(&args[0].value, ctx, None)?;
    let mut hits: Vec<String> = Vec::new();
    for base in bases {
        if let Some(set) = ctx.overloads.get(base) {
            for (ptys, key) in set {
                if ptys.iter().all(|t| *t == probe.ty) {
                    hits.push(key.clone());
                }
            }
        }
    }
    hits.sort();
    hits.dedup();
    match hits.len() {
        0 => Err(TypeError {
            help: None,
            message: format!(
                "no overload of '{}' takes '{}'{}",
                display_callee(callee),
                probe.ty.display(),
                overload_options(callee, owned_key, ctx)
            ),
            span,
        }),
        1 => {
            let key = hits.remove(0);
            let sig = ctx
                .sigs
                .get(&key)
                .cloned()
                .expect("every overload key is in sigs");
            Ok(Some((key, sig)))
        }
        _ => Err(TypeError {
            help: None,
            message: format!(
                "call to '{}' is ambiguous for argument type '{}'",
                display_callee(callee),
                probe.ty.display()
            ),
            span,
        }),
    }
}

/// " - it is declared for i32, i64, f64", for the message above.
pub(crate) fn overload_options(callee: &str, owned_key: &str, ctx: &Ctx) -> String {
    for base in [callee, owned_key] {
        if let Some(set) = ctx.overloads.get(base) {
            let mut names: Vec<String> = set
                .iter()
                .filter_map(|(ptys, _)| ptys.first().map(|t| t.display()))
                .collect();
            names.sort();
            names.dedup();
            if !names.is_empty() {
                return format!(" - it is declared for {}", names.join(", "));
            }
        }
    }
    String::new()
}

/// The hidden passdown arguments a call to `callee` has to pass, in the
/// order the callee's signature declares them (M5.2). Each one is a borrow of
/// the passdown's storage: the generated global's address, or - inside a
/// function that received that passdown as a hidden parameter - the parameter
/// itself, which *is* that same borrow. A caller that only calls a function
/// reading a passdown never receives one of its own (the wiring is per
/// function's own references), which is why the global's address is the
/// general case rather than the `main`-only case it used to be: emitting the
/// bare identifier there named the global whose type is the passdown's value,
/// so the call passed an integer where the callee's signature wanted a
/// pointer and `cc` refused the file.
pub(crate) fn hidden_args(callee: &str, span: Span, ctx: &Ctx) -> Result<Vec<tir::Expr>> {
    let Some(pds) = ctx.wired_for(callee) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::with_capacity(pds.len());
    for pd in pds {
        if !ctx.pd_initialized(&pd.hidden) {
            return Err(TypeError {
                help: None,
                message: format!(
                    "'{callee}' reads passdown '{}', which is not initialized yet here - a 'pd' initializer can only call functions whose passdowns are already set up",
                    pd.qualified
                ),
                span,
            });
        }
        let own_parameter = ctx.own_hidden.iter().any(|h| h == &pd.hidden);
        // The borrow handed over is the callee's (`PdWiring::exclusive`), not
        // the caller's: a caller that only forwards a read-only callee's
        // parameter never needed an exclusive borrow of its own, and one that
        // does hold the parameter exclusively can pass it on either way
        // (the closure in `close_pd_modes` is what makes the two agree).
        let exclusive = if own_parameter {
            ctx.pd_exclusive(&pd)
        } else {
            let callee_key = ctx.wiring_key_of(callee);
            ctx.pd_exclusive_for(&callee_key, &pd)
        };
        out.push(pd.arg(span, !own_parameter, exclusive));
    }
    Ok(out)
}

/// Checks a string template's interpolated pieces and lowers it to one
/// `tir::ExprKind::Template`. Interpolated values must be printable under
/// exactly the rule `print` already applies (numeric, `bool`, `char`,
/// `str`). Printed, the template writes its text; outside `print` the caller
/// sets the type to `String` and the text is built into an owned buffer.
pub(crate) fn check_template(
    parts: &[ast::TemplatePart],
    newline: bool,
    span: Span,
    ctx: &mut Ctx,
) -> Result<tir::Expr> {
    let mut pieces = Vec::with_capacity(parts.len());
    for part in parts {
        match part {
            ast::TemplatePart::Text(text) => {
                pieces.push(tir::TemplatePiece::Text(text.clone()))
            }
            ast::TemplatePart::Expr(e) => {
                let value = check_expr(e, ctx, None)?;
                // The element type decides whether a container is printable at
                // all, and an open container has not decided it yet: report
                // that rather than spell the marker (`List<unit>`).
                refuse_open_arg(&value.ty, e)?;
                let value = borrow_for_print(value, "a string template", ctx)?;
                if let Ty::Handle(name) = &value.ty {
                    return Err(TypeError {
                        help: None,
                        message: format!(
                            "cannot interpolate a value of type '{name}' into a string template: an 'extern type' is an opaque native handle with no printable value"
                        ),
                        span: e.span(),
                    });
                }
                let printable = value.ty.is_numeric()
                    || matches!(value.ty, Ty::Bool | Ty::Char)
                    || is_str_like(&value.ty)
                    || is_printable_buffer(&value.ty);
                if !printable {
                    // A `datac`/`enum` that implements `Display` interpolates
                    // through `x.to_text()` (`design/traits.md (kelp wiki)` 13.4).
                    if let Some(text) = lower_display(&value, e.span(), ctx)? {
                        pieces.push(tir::TemplatePiece::Value(text));
                        continue;
                    }
                    return Err(TypeError {
                        help: None,
                        message: match &value.ty {
                            Ty::Struct(_) => format!(
                                "cannot interpolate a value of type '{}' into a string template: it does not implement 'Display' (write 'impl fmt.Display for {} {{ fun Self.to_text() => String }}', importing 'std/fmt')",
                                value.ty.display(),
                                value.ty.display()
                            ),
                            other => format!(
                                "cannot interpolate a value of type '{}' into a string template",
                                other.display()
                            ),
                        },
                        span: e.span(),
                    });
                }
                pieces.push(tir::TemplatePiece::Value(value));
            }
        }
    }
    Ok(tir::Expr {
        kind: tir::ExprKind::Template { newline, pieces },
        ty: Ty::Unit,
        span,
    })
}

/// Whether this is a call that can say what an open container holds: the
/// element `List`/`Box` hand in, or the value (and the key beside it) a map
/// `insert` hands in. `List.insert` and `List.set` name theirs second, after
/// the index.
pub(crate) fn decides_container_element(ty: &Ty, method: &str) -> bool {
    match ty {
        Ty::List(_) => matches!(method, "push" | "insert" | "set"),
        Ty::Box(_) => method == "set",
        Ty::Map(..) => method == "insert",
        _ => false,
    }
}

/// Checks one deciding argument for the type it contributes to an open
/// container. A `str` literal is checked again as the `String` a container
/// element is: `xs.push("a")` builds a list of text, exactly as
/// `List<String>.push("a")` does. A `str` *value* still has no element to be,
/// and comes back the borrowed view it is for the element check to refuse.
pub(crate) fn check_deciding_element(arg: &ast::Expr, ctx: &mut Ctx) -> Result<tir::Expr> {
    let checked = check_expr(arg, ctx, None)?;
    if matches!(checked.ty, Ty::Str) {
        return check_expr(arg, ctx, Some(&Ty::String));
    }
    Ok(checked)
}

/// The container an open container's deciding call names, or `None` when the
/// call is not one that decides anything.
///
/// Every deciding argument is checked here, once, to learn its type. It is the
/// same probe the receiver already goes through, and safe for the same reason:
/// `check_expr` reads the context and reports, and the call's own argument
/// check runs again below with the signature this produced.
pub(crate) fn open_container_from_call(
    open: &Ty,
    method: &str,
    args: &[ast::CallArg],
    ctx: &mut Ctx,
) -> Result<Option<Ty>> {
    match open {
        Ty::List(_) => {
            let arg = match method {
                "push" => args.first(),
                "insert" | "set" => args.get(1),
                _ => return Ok(None),
            };
            let Some(arg) = arg else { return Ok(None) };
            let value = check_deciding_element(&arg.value, ctx)?;
            note_heap_use(&ctx.env, managed::HeapSlot::ListElement, &value.ty, arg.value.span());
            if !crate::types::is_list_element(&value.ty) {
                return Err(TypeError {
                    help: None,
                    message: list_element_error(&value.ty),
                    span: arg.value.span(),
                });
            }
            Ok(crate::types::fill_open_container(
                open,
                &Ty::List(Box::new(value.ty)),
                &ctx.keyable_set(),
            ))
        }
        Ty::Box(_) => {
            if method != "set" {
                return Ok(None);
            }
            let Some(arg) = args.first() else { return Ok(None) };
            let value = check_deciding_element(&arg.value, ctx)?;
            note_heap_use(&ctx.env, managed::HeapSlot::BoxPayload, &value.ty, arg.value.span());
            if !crate::types::is_box_element(&value.ty) {
                return Err(TypeError {
                    help: None,
                    message: box_element_error(&value.ty),
                    span: arg.value.span(),
                });
            }
            Ok(crate::types::fill_open_container(
                open,
                &Ty::Box(Box::new(value.ty)),
                &ctx.keyable_set(),
            ))
        }
        Ty::Map(..) => {
            if method != "insert" {
                return Ok(None);
            }
            let (Some(key_arg), Some(value_arg)) = (args.first(), args.get(1)) else {
                return Ok(None);
            };
            let value = check_deciding_element(&value_arg.value, ctx)?;
            note_heap_use(&ctx.env, managed::HeapSlot::MapValue, &value.ty, value_arg.value.span());
            if !crate::types::is_map_element(&value.ty) || ctx.is_enum_ty(&value.ty) {
                return Err(TypeError {
                    help: None,
                    message: if ctx.is_enum_ty(&value.ty) {
                        map_enum_value_error(&value.ty)
                    } else {
                        map_element_error(&value.ty)
                    },
                    span: value_arg.value.span(),
                });
            }
            // The key is read through a `str` view for a `String` key, which
            // is the one key type a call site cannot spell from the argument
            // alone: a `String` key takes `"a"`, and `"a"` is a `str`.
            let key = check_expr(&key_arg.value, ctx, None)?;
            note_heap_use(&ctx.env, managed::HeapSlot::MapKey, &key.ty, key_arg.value.span());
            let key_ty = if is_str_like(&key.ty) {
                Ty::String
            } else if crate::types::is_map_key_with(&key.ty, &ctx.keyable_set()) {
                key.ty.clone()
            } else {
                return Err(TypeError {
                    help: None,
                    message: map_key_error(&key.ty),
                    span: key_arg.value.span(),
                });
            };
            Ok(crate::types::fill_open_container(
                open,
                &Ty::Map(Box::new(key_ty), Box::new(value.ty)),
                &ctx.keyable_set(),
            ))
        }
        _ => Ok(None),
    }
}

/// The diagnostic an open container gets at a use that neither decides what it
/// holds nor is one of the two associated constructors that never knew: the
/// element type is still missing, and saying so names the call that could have
/// supplied it instead of the local that was declared without one.
pub(crate) fn open_container_use_error(ty: &Ty, receiver: &ast::Expr, span: Span) -> TypeError {
    let (binding, constructor, decider, example) = match ty {
        Ty::List(_) => ("xs", "List.empty()", "xs.push(v)", "var xs: List<i32> = List.empty()"),
        Ty::Box(_) => ("b", "Box.empty()", "b.set(v)", "var b: Box<i32> = Box.empty()"),
        _ => ("m", "Map.new()", "m.insert(k, v)", "var m: Map<String, i32> = Map.new()"),
    };
    let named = match receiver {
        ast::Expr::Ident(name, _) => format!("'{name}'"),
        _ => format!("this container (the one bound as '{binding}')"),
    };
    TypeError {
        help: None,
        message: format!(
            "the element type of {named} is still open: nothing said what '{constructor}' holds, and '{decider}' is what decides it. Write it on the binding instead ('{example}')"
        ),
        span,
    }
}

/// An open container handed to a call's argument has nobody left to decide it:
/// a parameter that names a container element takes it (`fun take(xs:
/// List<i32>)` repairs the binding on the way in), while a parameter that takes
/// the container's own type (`fun show<T>(x: T)`) would carry the marker into
/// the emitted program, where the element type is what every routine is named
/// after.
pub(crate) fn refuse_open_arg(ty: &Ty, arg: &ast::Expr) -> Result<()> {
    if crate::types::is_open_container(ty) {
        return Err(open_container_use_error(ty, arg, arg.span()));
    }
    Ok(())
}

/// `E.A.tag()` where `E` is an `enum`: the receiver *builds* a variant, so it
/// names a type rather than a value, while a method an `enum` declares (`fun
/// E.tag()`) is called on a value of the enum type. Refused before the
/// receiver is checked as a value, so the diagnostic is about the spelling
/// rather than about a variant of the wrong arity; a value of enum type
/// reached any other way (`val e = E.A` then `e.tag()`) reaches the method
/// tables like any other receiver.
pub(crate) fn check_method_call_enum_guard(
    receiver: &ast::Expr,
    method: &str,
    span: Span,
    ctx: &Ctx,
) -> Result<()> {
    // `E.A.to_int()` is a read of the variant's value, so it is reached on the
    // variant path itself; only a *declared* method needs a binding first.
    if method == "to_int" {
        return Ok(());
    }
    if let ast::Expr::FieldAccess { base, .. } = receiver {
        if enum_path(base, ctx).is_some() {
            return Err(TypeError {
                help: None,
                message: "an 'enum' method is called on a value, not on a variant path; bind the variant first, as in: val e = E.A  then  e.tag()".to_string(),
                span,
            });
        }
    }
    Ok(())
}
