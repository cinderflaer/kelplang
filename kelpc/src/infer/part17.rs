use super::*;

pub(crate) fn aggregate_borrowed_fields(
    fields: &[FieldDef],
    values: &[tir::Expr],
    span: Span,
) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for (i, (field, value)) in fields.iter().zip(values).enumerate() {
        if !matches!(field.ty, Ty::Str | Ty::View) {
            continue;
        }
        if matches!(value.kind, tir::ExprKind::StringLit(_)) {
            continue;
        }
        let Some(source) = aggregate_borrow_source(value) else {
            return Err(TypeError { message: format!("field '{}' stores a borrowed '{}', so construct it from a string literal or from an earlier String/Bytes field in this datac", field.name, field.ty.display()), span, help: None });
        };
        let owns_source = fields[..i].iter().zip(&values[..i]).any(|(owner, value)| {
            matches!(owner.ty, Ty::String | Ty::Bytes)
                && matches!(&value.kind, tir::ExprKind::Ident(name) if name == source)
        });
        if !owns_source {
            return Err(TypeError { message: format!("field '{}' borrows '{source}', but that value is not moved into an earlier String/Bytes field of this datac", field.name), span, help: None });
        }
        out.push(field.name.clone());
    }
    Ok(out)
}

pub(crate) fn aggregate_borrow_source(value: &tir::Expr) -> Option<&str> {
    let tir::ExprKind::Call { args, .. } = &value.kind else { return None; };
    let tir::ExprKind::Ref { expr, .. } = &args.first()?.kind else { return None; };
    let tir::ExprKind::Ident(name) = &expr.kind else { return None; };
    Some(name)
}

/// The gated resource constructors (`irq.claim`, `frame.claim`, kernel stage
/// 18): the call is the capability's gate, so the argument is checked here,
/// where both backends and both the hosted and the freestanding modes pass.
/// The domain gate already ran (`check_capability`), so the module holds the
/// domain; what is left is the value.
///
/// A constant argument has to fall inside a granted range, and a `frame`
/// address has to be 4 KiB-aligned whatever the grant. A non-constant argument
/// is admitted only under the whole-resource `["*"]`, because the compiler
/// cannot prove a runtime value lands in a range; the same rule the `mmio`
/// address check applies (`kelp wiki page language/capabilities.md`, `kelp wiki page stdlib/irq.md`).
pub(crate) fn check_claim_argument(
    key: &str,
    args: &[tir::Expr],
    span: Span,
    ctx: &Ctx,
) -> Result<()> {
    let Some(domain) = crate::stdlib::claim_domain(key) else {
        return Ok(());
    };
    let patterns = ctx.caps.get(domain).cloned().unwrap_or_default();
    let value = args.first().and_then(crate::stdlib::const_int);
    // The alignment rule holds for a constant whatever the grant, including
    // the wildcard: a misaligned frame is not a frame.
    if domain == "frames" {
        if let Some(v) = value {
            if v % crate::stdlib::FRAME_SIZE != 0 {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "'frame.claim' would take the address 0x{v:X}, which is not {}-aligned: a frame is a {}-byte-aligned physical address",
                        crate::stdlib::FRAME_SIZE, crate::stdlib::FRAME_SIZE
                    ),
                    span,
                });
            }
        }
    }
    if patterns.iter().any(|p| p == crate::stdlib::RANGE_ANY) {
        return Ok(());
    }
    let name = if domain == "frames" { "frame.claim" } else { "irq.claim" };
    let what = if domain == "frames" { "address" } else { "line" };
    let Some(value) = value else {
        return Err(TypeError {
            help: None,
            message: format!(
                "'{name}' takes a {what} the compiler cannot fold, and module '{}' holds only the granted {what}(s) {}; a computed {what} needs the whole-resource grant `caps({domain} = [\"*\"])`",
                ctx.module,
                patterns.join(", "),
            ),
            span,
        });
    };
    if crate::stdlib::range_covers(domain, &patterns, value) {
        return Ok(());
    }
    if domain == "frames" {
        return Err(TypeError {
            help: None,
            message: format!(
                "'frame.claim' would take the address 0x{value:X}, which no 'frames' grant of module '{}' covers (granted: {})",
                ctx.module,
                patterns.join(", ")
            ),
            span,
        });
    }
    Err(TypeError {
        help: None,
        message: format!(
            "'irq.claim' would take the line {value}, which no 'irq' grant of module '{}' covers (granted: {})",
            ctx.module,
            patterns.join(", ")
        ),
        span,
    })
}

/// Records the fixed-arity wrapper one variadic call site needs (gap 7), and
/// returns its name. Idempotent: the wrapper is created the first time a call
/// passes a given list of extra-argument types, and every later call with the
/// same list names the same one. The wrapper's own parameters are the extern's
/// declared parameters followed by one per extra argument, so
/// the removed C backend's `write_user_extern` forwards all of them to the native symbol
/// unchanged  -  the C compiler then applies the default argument promotions a
/// real variadic call gets.
pub(crate) fn register_variadic_wrapper(
    ctx: &mut Ctx,
    extern_key: &str,
    sig: &FnSig,
    extra_tys: &[Ty],
    span: Span,
) -> String {
    let name = crate::types::variadic_wrapper_name(extern_key, extra_tys);
    let module = ctx
        .variadic_externs
        .get(extern_key)
        .cloned()
        .unwrap_or_else(|| ctx.module.clone());
    let mut state = ctx.fns.instances.borrow_mut();
    if state.variadic_wrappers_seen.insert(name.clone()) {
        let mut params = sig.params.clone();
        for (i, ty) in extra_tys.iter().enumerate() {
            params.push((format!("#va{i}"), ty.clone()));
        }
        state.variadic_wrappers.push((
            extern_key.to_string(),
            tir::Function {
                name: name.clone(),
                params,
                ret: sig.ret.clone(),
                body: tir::Block {
                    stmts: Vec::new(),
                    tail: None,
                },
                span,
                module,
                extern_: true,
                env: None,
                section: None,
                naked: false,
                align: None,
            },
        ));
    }
    name
}

/// The key of the generic *function* template `name` or `owned` names, if
/// either does (A-2). The lookup on `name` as written carries the same rule
/// the signature lookup in `check_call` does: it is the canonical spelling,
/// so it has to name a module. A bare name is the calling module's own, and
/// the entry module is the only one whose keys are bare (A-13).
pub(crate) fn func_template_named(ctx: &Ctx, name: &str, owned: &str) -> Option<String> {
    if ctx.fns.templates.contains_key(owned) {
        return Some(owned.to_string());
    }
    (name.contains('.') && ctx.fns.templates.contains_key(name)).then(|| name.to_string())
}

/// The generic types that declare an iterator-shaped `next` (`TypeEnv::iter_nexts`).
pub(crate) fn collect_iter_nexts(program: &ast::Program) -> HashMap<String, IterNext> {
    let mut out = HashMap::new();
    for f in &program.functions {
        let (Some(r), "next") = (&f.receiver, f.name.as_str()) else { continue };
        if r.type_args.is_empty() || !r.mutable || !f.params.is_empty() {
            continue;
        }
        let Some(ret) = &f.return_type else { continue };
        let is_option = ret.name == "Option" || ret.name.ends_with(".Option");
        if !is_option || ret.args.len() != 1 {
            continue;
        }
        out.insert(
            r.type_name.clone(),
            IterNext { params: r.type_args.clone(), item: ret.args[0].clone() },
        );
    }
    out
}

/// A written type that mentions only the template's parameters, builtin scalars
/// and tuples of those, resolved against an instance's arguments.
pub(crate) fn resolve_item_type(te: &ast::TypeExpr, params: &[String], args: &[Ty], env: &TypeEnv) -> Option<Ty> {
    if let Some(elems) = &te.tuple {
        let tys: Option<Vec<Ty>> = elems.iter().map(|e| resolve_item_type(e, params, args, env)).collect();
        return Some(tuple_ty(env, tys?));
    }
    if !te.args.is_empty() || te.amp || te.array_elem.is_some() || te.fn_params.is_some() {
        return None;
    }
    if let Some(i) = params.iter().position(|p| *p == te.name) {
        return args.get(i).cloned();
    }
    Ty::from_name(&te.name)
}

/// The `Item` of a generic type that is an iterator by shape, from the instance's
/// own arguments.
pub(crate) fn shaped_iterator_item(env: &TypeEnv, instance: &str) -> Option<Ty> {
    let (template, args) = env.instances.borrow().origin.get(instance).cloned()?;
    let next = env.iter_nexts.get(&template)?;
    resolve_item_type(&next.item, &next.params, &args, env)
}

/// The equality constraints and the `where` projection bounds of a generic
/// impl (MISSING-Ve8Nc step 5), checked as declared: each constraint names an
/// associated type its trait declares, and a projection bound names a plain
/// trait.
pub(crate) type ImplConstraints = (Vec<Vec<(String, ast::TypeExpr)>>, Vec<(usize, String, String)>);

pub(crate) fn impl_constraints(
    imp: &TraitImplInfo,
    traits: &HashMap<String, TraitInfo>,
    project: &Project,
) -> std::result::Result<ImplConstraints, Diagnostic> {
    let prefix = project.prefix_of(&imp.module);
    let aliases = project.alias_prefixes_of(&imp.module);
    let mut eqs: Vec<Vec<(String, ast::TypeExpr)>> = Vec::new();
    for bs in &imp.type_param_bounds {
        let mut list = Vec::new();
        for b in bs {
            if b.bound_eq.is_empty() {
                continue;
            }
            let key = qualify_written_name(&b.name, prefix, &aliases).unwrap_or_default();
            for (assoc, rhs) in &b.bound_eq {
                let declared = traits
                    .get(&key)
                    .is_some_and(|info| info.assoc_types.iter().any(|a| a.name == *assoc));
                if !declared {
                    return Err(Diagnostic::new(
                        format!(
                            "'{}' declares no associated type '{assoc}', so the equality constraint '{assoc} = {}' on a parameter of this impl names nothing",
                            b.name,
                            rhs.display()
                        ),
                        imp.impl_span,
                        imp.module.clone(),
                    ));
                }
                list.push((assoc.clone(), rhs.clone()));
            }
        }
        eqs.push(list);
    }
    let mut projections = Vec::new();
    for pb in &imp.projection_bounds {
        let Some(idx) = imp.type_params.iter().position(|p| *p == pb.param) else {
            continue;
        };
        if !pb.bound.args.is_empty() {
            return Err(Diagnostic::new(
                format!("'where {}::{}: {}' bounds a projection with trait arguments, which is not in this slice", pb.param, pb.assoc, pb.bound.display()),
                imp.impl_span,
                imp.module.clone(),
            ));
        }
        let Some(key) = qualify_written_name(&pb.bound.name, prefix, &aliases) else {
            return Err(Diagnostic::new(
                format!("'{}' bounds '{}::{}' of this impl, but its first segment is not a module it imports", pb.bound.name, pb.param, pb.assoc),
                imp.impl_span,
                imp.module.clone(),
            ));
        };
        if !traits.contains_key(&key) {
            return Err(Diagnostic::new(
                format!("'{}' is not a trait, so it cannot bound '{}::{}' of this impl", pb.bound.name, pb.param, pb.assoc),
                imp.impl_span,
                imp.module.clone(),
            ));
        }
        projections.push((idx, pb.assoc.clone(), key));
    }
    Ok((eqs, projections))
}

/// The trait key each parameter of a generic or blanket impl must implement, one
/// list per parameter, in the impl's own module's spelling.
pub(crate) fn impl_param_bounds(
    imp: &TraitImplInfo,
    traits: &HashMap<String, TraitInfo>,
    project: &Project,
) -> std::result::Result<Vec<Vec<String>>, Diagnostic> {
    let prefix = project.prefix_of(&imp.module);
    let aliases = project.alias_prefixes_of(&imp.module);
    let mut bounds: Vec<Vec<String>> = Vec::new();
    for bs in &imp.type_param_bounds {
        let mut keys = Vec::new();
        for b in bs {
            if !b.args.is_empty() {
                return Err(Diagnostic::new(
                    format!("the bound '{}' on an impl parameter takes trait arguments, which a generic impl does not support yet", b.display()),
                    imp.impl_span,
                    imp.module.clone(),
                ));
            }
            let Some(key) = qualify_written_name(&b.name, prefix, &aliases) else {
                return Err(Diagnostic::new(
                    format!("'{}' bounds a parameter of this impl, but its first segment is not a module it imports", b.name),
                    imp.impl_span,
                    imp.module.clone(),
                ));
            };
            if !traits.contains_key(&key) {
                return Err(Diagnostic::new(
                    format!("'{}' is not a trait, so it cannot bound a parameter of this impl", b.name),
                    imp.impl_span,
                    imp.module.clone(),
                ));
            }
            keys.push(key);
        }
        bounds.push(keys);
    }
    Ok(bounds)
}

/// Refuses blanket impls whose bounds lead back to themselves (`impl<T: A> B for
/// T` with `impl<T: B> A for T`, or a blanket impl bounded on its own trait):
/// checking one such instance would need the same check again, forever.
pub(crate) fn check_blanket_cycles(blankets: &[BlanketImpl]) -> std::result::Result<(), Diagnostic> {
    for b in blankets {
        let mut stack: Vec<String> = b.bounds.clone();
        let mut seen = std::collections::HashSet::new();
        while let Some(key) = stack.pop() {
            if key == b.trait_key {
                return Err(Diagnostic::new(
                    format!(
                        "the blanket impl of '{}' bounds its parameter on '{}' itself, so checking any type against it would need the same check again: a blanket impl's bound has to be a different trait",
                        b.trait_name, b.trait_name
                    ),
                    b.impl_span,
                    b.module.clone(),
                ));
            }
            if !seen.insert(key.clone()) {
                continue;
            }
            for other in blankets.iter().filter(|o| o.trait_key == key) {
                stack.extend(other.bounds.iter().cloned());
            }
        }
    }
    Ok(())
}

/// The blanket impl of `trait_key` whose bounds `name` meets, if any: a type
/// that meets them implements the trait (`TypeEnv::blanket_impls`).
pub(crate) fn blanket_holds(env: &TypeEnv, trait_key: &str, name: &str) -> bool {
    env.blanket_impls
        .iter()
        .any(|b| b.trait_key == trait_key && b.bounds.iter().all(|k| implements_trait(env, k, name)))
}

/// The generic impl of `trait_key` that applies to the instance `ty_name`, if
/// there is one: the instance's template has an impl and every bound on the
/// impl's parameters holds for the instance's arguments.
pub(crate) fn generic_impl_applies(env: &TypeEnv, trait_key: &str, ty_name: &str) -> Option<(GenericImpl, Vec<Ty>)> {
    let (template, args) = env.instances.borrow().origin.get(ty_name).cloned()?;
    for gi in env.generic_impls.get(trait_key)? {
        if gi.template != template {
            continue;
        }
        let holds = gi.bounds.iter().enumerate().all(|(i, keys)| {
            keys.iter().all(|key| {
                args.get(i)
                    .and_then(|a| {
                        struct_name_of(a)
                            .map(|s| s.to_string())
                            .or_else(|| builtin_type_name(a).map(|s| s.to_string()))
                    })
                    .is_some_and(|n| implements_trait(env, key, &n))
            })
        });
        if holds && generic_impl_constraints_hold(env, gi, &args) {
            return Some((gi.clone(), args));
        }
    }
    None
}

/// The equality constraints and projection bounds of a generic impl hold for
/// the instance's arguments (MISSING-Ve8Nc step 5).
pub(crate) fn generic_impl_constraints_hold(env: &TypeEnv, gi: &GenericImpl, args: &[Ty]) -> bool {
    let eqs_hold = gi.eqs.iter().enumerate().all(|(i, list)| {
        list.iter().all(|(assoc, rhs)| {
            args.get(i)
                .and_then(|a| assoc_type_of(env, a, assoc))
                .is_some_and(|got| eq_rhs_matches(rhs, &gi.params, args, &gi.prefix, &gi.aliases, &got))
        })
    });
    eqs_hold
        && gi.projections.iter().all(|(i, assoc, key)| {
            args.get(*i)
                .is_some_and(|a| matches!(projection_check(env, a, assoc, key), ProjectionCheck::Holds))
        })
}

/// Why a type with a generic impl of `trait_key` does not implement it: the
/// first argument whose bound fails, spelled out. `None` when the type has no
/// generic impl of the trait at all.
pub(crate) fn generic_impl_shortfall(env: &TypeEnv, trait_key: &str, ty: &Ty) -> Option<String> {
    let name = struct_name_of(ty)?.to_string();
    let (template, args) = env.instances.borrow().origin.get(&name).cloned()?;
    let gi = env.generic_impls.get(trait_key)?.iter().find(|g| g.template == template)?;
    for (i, keys) in gi.bounds.iter().enumerate() {
        let arg = args.get(i)?;
        let arg_name = struct_name_of(arg)
            .map(|s| s.to_string())
            .or_else(|| builtin_type_name(arg).map(|s| s.to_string()))
            .unwrap_or_else(|| arg.display());
        for key in keys {
            if !implements_trait(env, key, &arg_name) {
                return Some(format!(
                    "its impl needs '{}' ('{}' is parameter '{}') to implement '{}', and it does not",
                    arg.display(),
                    gi.template,
                    gi.params.get(i).map(String::as_str).unwrap_or("?"),
                    key
                ));
            }
        }
        for (assoc, rhs) in gi.eqs.get(i).into_iter().flatten() {
            let got = assoc_type_of(env, arg, assoc).map(|t| t.display()).unwrap_or_else(|| "nothing".to_string());
            if assoc_type_of(env, arg, assoc).is_none_or(|got| !eq_rhs_matches(rhs, &gi.params, &args, &gi.prefix, &gi.aliases, &got)) {
                return Some(format!(
                    "its impl needs '{}' ('{}' is parameter '{}') to bind '{assoc}' to '{}', and it binds '{got}'",
                    arg.display(),
                    gi.template,
                    gi.params.get(i).map(String::as_str).unwrap_or("?"),
                    rhs.display()
                ));
            }
        }
    }
    for (i, assoc, key) in &gi.projections {
        let Some(a) = args.get(*i) else { continue };
        if !matches!(projection_check(env, a, assoc, key), ProjectionCheck::Holds) {
            return Some(format!(
                "its impl needs '{}::{assoc}' to implement '{key}', and it does not",
                gi.params.get(*i).map(String::as_str).unwrap_or("?")
            ));
        }
    }
    None
}

/// The type a trait's associated type `item` is for the instance `ty_name`:
/// bound by a concrete impl, by a generic impl (resolved over the instance's
/// arguments), or the item of an iterator by shape.
pub(crate) fn assoc_type_of(env: &TypeEnv, ty: &Ty, item: &str) -> Option<Ty> {
    let name = struct_name_of(ty)
        .map(|s| s.to_string())
        .or_else(|| builtin_type_name(ty).map(|s| s.to_string()))?;
    for ((_, tn), bindings) in env.assoc.iter() {
        if *tn == name {
            if let Some(t) = bindings.get(item) {
                return Some(t.clone());
            }
        }
    }
    for trait_key in env.generic_impls.keys() {
        if let Some((gi, args)) = generic_impl_applies(env, trait_key, &name) {
            if let Some((_, te)) = gi.assoc_types.iter().find(|(n, _)| n == item) {
                return resolve_impl_assoc(te, &gi.params, &args, env);
            }
        }
    }
    if item == "Item" {
        return shaped_iterator_item(env, &name);
    }
    None
}

/// A generic impl's `type Item = ...`, resolved over an instance's arguments:
/// a parameter, a builtin, a tuple of those, or a projection `P::Name` of a
/// parameter's argument.
pub(crate) fn resolve_impl_assoc(te: &ast::TypeExpr, params: &[String], args: &[Ty], env: &TypeEnv) -> Option<Ty> {
    if let Some(item) = &te.assoc {
        let i = params.iter().position(|p| *p == te.name)?;
        return assoc_type_of(env, args.get(i)?, item);
    }
    if let Some(elems) = &te.tuple {
        let tys: Option<Vec<Ty>> = elems.iter().map(|e| resolve_impl_assoc(e, params, args, env)).collect();
        return Some(tuple_ty(env, tys?));
    }
    resolve_item_type(te, params, args, env)
}

/// Whether `ty_name` implements `trait_key`, directly or through a supertrait
/// (A-12 slice 3): a type with `impl B for` satisfies a bound on `A` whenever
/// `B: A`, because implementing `B` required an `impl A for` the type.
pub(crate) fn implements_trait(env: &TypeEnv, trait_key: &str, ty_name: &str) -> bool {
    if env
        .impls
        .get(trait_key)
        .map(|types| types.contains(ty_name))
        .unwrap_or(false)
    {
        return true;
    }
    // A generic type whose template declares an iterator-shaped `next` is an
    // `Iterator` by shape (`TypeEnv::iter_nexts`).
    // An explicit generic `impl Iterator` of the type governs it: its bounds
    // (`where I::Item: Display`, `I: Iterator<Item = i64>`) must hold, so the
    // shape alone does not satisfy the trait then (MISSING-Ve8Nc step 5).
    if trait_key == ITERATOR_TRAIT_KEY
        && shaped_iterator_item(env, ty_name).is_some()
        && !has_generic_impl_of(env, trait_key, ty_name)
    {
        return true;
    }
    // `impl<T: Bound> Trait for Type<T>`: an instance implements the trait when
    // each of its arguments meets the bounds the impl puts on its parameter.
    if generic_impl_applies(env, trait_key, ty_name).is_some() {
        return true;
    }
    // `impl<T: Bound> Trait for T`: every type that meets the bound implements it.
    if blanket_holds(env, trait_key, ty_name) {
        return true;
    }
    env.impls.iter().any(|(other, types)| {
        other != trait_key && types.contains(ty_name) && trait_requires(env, other, trait_key)
    })
}

/// Whether the generic type `ty_name` (an instance) has an explicit generic impl
/// of `trait_key` written for its template, whatever the impl's bounds are.
pub(crate) fn has_generic_impl_of(env: &TypeEnv, trait_key: &str, ty_name: &str) -> bool {
    let Some((template, _)) = env.instances.borrow().origin.get(ty_name).cloned() else {
        return false;
    };
    env.generic_impls
        .get(trait_key)
        .is_some_and(|impls| impls.iter().any(|g| g.template == template))
}

/// Whether trait `sub` inherits, directly or transitively, from `sup`.
pub(crate) fn trait_requires(env: &TypeEnv, sub: &str, sup: &str) -> bool {
    let mut stack = vec![sub.to_string()];
    let mut seen = std::collections::HashSet::new();
    while let Some(t) = stack.pop() {
        if !seen.insert(t.clone()) {
            continue;
        }
        if t == sup {
            return true;
        }
        if let Some(info) = env.traits.get(&t) {
            for s in &info.supertraits {
                stack.push(s.clone());
            }
        }
    }
    false
}

/// Checks the trait bounds one instantiation's type arguments must satisfy
/// (A-12 slice 2). `tmpl` is the declaration and `type_args` the concrete
/// arguments, so this answers about the type a call actually binds; `span` is
/// the call site, which is where an unsatisfied bound is reported.
///
/// The bound is written in the callee's module, so its name is qualified with
/// the *template's* prefix and alias table, exactly as its parameter and return
/// types are.
pub(crate) fn check_trait_bounds(
    ctx: &Ctx,
    tmpl: &FuncTemplate,
    type_args: &[Ty],
    span: Span,
) -> Result<()> {
    for (i, bounds) in tmpl.bounds.iter().enumerate() {
        if bounds.is_empty() {
            continue;
        }
        let param = tmpl.params.get(i).map(String::as_str).unwrap_or("T");
        let Some(arg) = type_args.get(i) else {
            continue;
        };
        for b in bounds {
            // The alias may not be in the callee's table: the name is read
            // where the declaration was written, and the call site cannot tell
            // the two apart, so the error points at the call.
            let key = match qualify_written_name(&b.name, &tmpl.prefix, &tmpl.aliases) {
                Some(key) => key,
                None => {
                    return Err(TypeError {
                        help: None,
                        message: format!(
                            "'{}' bounds '{}' of '{}', but its first segment is not a module the declaration imports",
                            b.name,
                            param,
                            display_callee(&tmpl.key)
                        ),
                        span,
                    })
                }
            };
            let Some(info) = ctx.env.traits.get(&key) else {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "'{}' is not a trait, so it cannot bound '{}' of '{}': a bound names a declared trait ('trait {} {{ ... }}')",
                        b.name,
                        param,
                        display_callee(&tmpl.key),
                        b.name
                    ),
                    span,
                });
            };
            // A bound written through another module's alias needs that module
            // to export the trait, exactly as naming any other item there does.
            if let Some((alias, item)) = b.name.split_once('.') {
                if !exported(&key, &ctx.env) {
                    return Err(private_item_error(alias, item, span));
                }
            }
            // A-12 slice 6: the bound names a trait *instance*, so its
            // arguments are resolved here, where the callee's parameters are
            // already bound to the call's types.
            let mut arg_names = Vec::new();
            for a in &b.args {
                let simple = a.tuple.is_none()
                    && a.array_elem.is_none()
                    && a.fn_params.is_none()
                    && a.dyn_trait.is_none()
                    && a.assoc.is_none()
                    && a.args.is_empty()
                    && !a.amp
                    && !a.name.is_empty();
                if !simple {
                    return Err(TypeError {
                        help: None,
                        message: format!(
                            "'{}' bounds '{}' of '{}' with the argument '{}', which is not a named type or a builtin: a container, tuple, array, borrow, function type or projection as a bound argument is not in this slice",
                            b.display(),
                            param,
                            display_callee(&tmpl.key),
                            a.display()
                        ),
                        span,
                    });
                }
                let resolved = if let Some(pi) = tmpl.params.iter().position(|p| *p == a.name) {
                    type_args.get(pi).and_then(|at| {
                        struct_name_of(at)
                            .map(|s| s.to_string())
                            .or_else(|| builtin_type_name(at).map(|s| s.to_string()))
                    })
                } else if crate::types::Ty::from_name(&a.name).is_some() {
                    Some(a.name.clone())
                } else {
                    qualify_written_name(&a.name, &tmpl.prefix, &tmpl.aliases)
                };
                let Some(resolved) = resolved else {
                    return Err(TypeError {
                        help: None,
                        message: format!(
                            "'{}' bounds '{}' of '{}' with the argument '{}', which names no type",
                            b.display(),
                            param,
                            display_callee(&tmpl.key),
                            a.display()
                        ),
                        span,
                    });
                };
                arg_names.push(resolved);
            }
            if arg_names.len() != info.type_params.len() {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "'{}' bounds '{}' of '{}', and trait '{}' takes {} type argument(s), found {}",
                        b.display(),
                        param,
                        display_callee(&tmpl.key),
                        b.name,
                        info.type_params.len(),
                        arg_names.len()
                    ),
                    span,
                });
            }
            for (bi, pbounds) in info.type_param_bounds.iter().enumerate() {
                let Some(aname) = arg_names.get(bi) else { continue };
                for (written, pkey) in pbounds {
                    if !implements_trait(&ctx.env, pkey, aname) {
                        return Err(TypeError {
                            help: None,
                            message: format!(
                                "the call to '{}' instantiates '{}', and '{}' does not satisfy the bound '{written}' on its parameter '{}': write 'impl {written} for {aname} {{ ... }}'",
                                display_callee(&tmpl.key),
                                b.display(),
                                aname,
                                info.type_params[bi]
                            ),
                            span,
                        });
                    }
                }
            }
            let instance = crate::types::trait_instance_key(&key, &arg_names);
            let name = struct_name_of(arg)
                .map(|s| s.to_string())
                .or_else(|| builtin_type_name(arg).map(|s| s.to_string()));
            let implements = name
                .map(|n| implements_trait(&ctx.env, &instance, &n))
                .unwrap_or(false);
            if !implements {
                // A type with a generic impl of the trait fails on one of its
                // arguments; name which, rather than suggesting a concrete impl.
                if let Some(why) = generic_impl_shortfall(&ctx.env, &instance, arg) {
                    return Err(TypeError {
                        help: None,
                        message: format!(
                            "the call to '{}' needs '{}' to implement the trait '{}': {why}",
                            display_callee(&tmpl.key),
                            arg.display(),
                            b.display(),
                        ),
                        span,
                    });
                }
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "the call to '{}' needs '{}' to implement the trait '{}', and it does not: write 'impl {} for {} {{ ... }}' in the trait's module or in the type's",
                        display_callee(&tmpl.key),
                        arg.display(),
                        b.display(),
                        b.display(),
                        arg.display()
                    ),
                    span,
                });
            }
            check_bound_equalities(ctx, tmpl, type_args, arg, b, &key, param, span)?;
        }
    }
    for pb in &tmpl.projection_bounds {
        check_projection_bound(ctx, tmpl, type_args, pb, span)?;
    }
    Ok(())
}

/// Whether `got` is the type an equality constraint's right-hand side names,
/// read in the spelling of the declaration that wrote it: a parameter of that
/// declaration (bound to `args`), a builtin, or a named type of its module.
pub(crate) fn eq_rhs_matches(
    te: &ast::TypeExpr,
    params: &[String],
    args: &[Ty],
    prefix: &str,
    aliases: &HashMap<String, String>,
    got: &Ty,
) -> bool {
    if !te.args.is_empty() || te.tuple.is_some() || te.array_elem.is_some() || te.amp || te.fn_params.is_some() {
        return false;
    }
    if let Some(i) = params.iter().position(|p| *p == te.name) {
        return args.get(i).is_some_and(|a| a == got);
    }
    if let Some(t) = Ty::from_name(&te.name) {
        return &t == got;
    }
    if builtin_type_name(got).is_some_and(|b| b == te.name) {
        return true;
    }
    qualify_written_name(&te.name, prefix, aliases)
        .is_some_and(|key| struct_name_of(got).is_some_and(|n| n == key))
}

/// Checks the equality constraints (`I: Iterator<Item = i64>`) one bound of a
/// call's parameter writes: the trait declares each associated type, and the
/// argument's type binds it to the named type (MISSING-Ve8Nc step 5).
#[allow(clippy::too_many_arguments)]
pub(crate) fn check_bound_equalities(
    ctx: &Ctx,
    tmpl: &FuncTemplate,
    type_args: &[Ty],
    arg: &Ty,
    b: &ast::TypeExpr,
    key: &str,
    param: &str,
    span: Span,
) -> Result<()> {
    let callee = display_callee(&tmpl.key);
    for (assoc, rhs) in &b.bound_eq {
        let declared = ctx
            .env
            .traits
            .get(key)
            .is_some_and(|info| info.assoc_types.iter().any(|a| a.name == *assoc));
        if !declared {
            return Err(TypeError {
                help: None,
                message: format!(
                    "'{}' declares no associated type '{assoc}', so the equality constraint '{assoc} = {}' on '{param}' of '{callee}' names nothing",
                    b.name,
                    rhs.display()
                ),
                span,
            });
        }
        let Some(got) = assoc_type_of(&ctx.env, arg, assoc) else {
            return Err(TypeError {
                help: None,
                message: format!(
                    "the call to '{callee}' instantiates '{param}: {}' with '{}', which binds no '{assoc}', so the equality constraint '{assoc} = {}' cannot hold",
                    b.display(),
                    arg.display(),
                    rhs.display()
                ),
                span,
            });
        };
        if !eq_rhs_matches(rhs, &tmpl.params, type_args, &tmpl.prefix, &tmpl.aliases, &got) {
            return Err(TypeError {
                help: None,
                message: format!(
                    "the call to '{callee}' instantiates '{param}: {}' with '{}', whose '{assoc}' is '{}', not '{}': the equality constraint '{assoc} = {}' fails",
                    b.display(),
                    arg.display(),
                    got.display(),
                    rhs.display(),
                    rhs.display()
                ),
                span,
            });
        }
    }
    Ok(())
}
