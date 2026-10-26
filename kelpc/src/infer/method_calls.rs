use super::*;

/// `Buffer.null()` and `Window.cast(w)` (gap 1): the two operations written on
/// a handle's *type name*. `null` is `NULL` (C) / `ptr null` (LLVM) in the
/// handle's own type; `cast` is a pointer reinterpretation with no runtime
/// check, so it is the explicit downcast the implicit upcast direction does not
/// give. Both are legal only where an `[extern.*]` section exists.
pub(crate) fn check_handle_associated(
    handle: &str,
    method: &str,
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
) -> Result<tir::Expr> {
    match method {
        "null" => {
            ctx.require_extern_section(&format!("{handle}.null()"), span)?;
            if !args.is_empty() {
                return Err(TypeError {
                    help: None,
                    message: format!("'{handle}.null()' takes no arguments, found {}", args.len()),
                    span,
                });
            }
            Ok(tir::Expr {
                kind: tir::ExprKind::Call {
                    callee: crate::types::HANDLE_NULL_KEY.to_string(),
                    args: Vec::new(),
                },
                ty: Ty::Handle(handle.to_string()),
                span,
            })
        }
        "cast" => {
            ctx.require_extern_section(&format!("{handle}.cast(w)"), span)?;
            if args.len() != 1 {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "'{handle}.cast(w)' takes exactly 1 argument, found {}",
                        args.len()
                    ),
                    span,
                });
            }
            if args[0].name.is_some() {
                return Err(TypeError {
                    help: None,
                    message: format!("'{handle}.cast' does not take named arguments"),
                    span,
                });
            }
            let value = check_expr(&args[0].value, ctx, None)?;
            let Ty::Handle(from) = &value.ty else {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "'{handle}.cast(w)' requires 'w' to be a handle, found '{}'",
                        value.ty.display()
                    ),
                    span: args[0].value.span(),
                });
            };
            if !handle_is_same_or_ancestor(&ctx.env.handle_parents, from, handle) {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "'{handle}.cast(w)' requires 'w' to be '{handle}' or an ancestor of it, found '{from}'; a child converts up to its parent implicitly, and 'cast' never checks at run time"
                    ),
                    span: args[0].value.span(),
                });
            }
            Ok(tir::Expr {
                ty: Ty::Handle(handle.to_string()),
                ..value
            })
        }
        other => Err(TypeError {
            help: None,
            message: format!(
                "'{handle}.{other}' is not a handle operation: write '{handle}.null()' for the null handle, or call 'is_null()' on a value ('w.is_null()')"
            ),
            span,
        }),
    }
}

/// `receiver.method(args)`: resolves `method` against the receiver
/// expression's `datac` type, then lowers to a plain `tir::ExprKind::Call`
/// - the receiver becomes the first argument, auto-borrowed through
/// `check_call_arg` exactly like any other `Ty::Ref` parameter (so a
/// `fun mut T.m()` method reuses M4's aliasing checks with no special
/// casing). No new `tir`/codegen shape is needed for method calls at all.
///
/// A `List<T>` method is the same thing with the element type read off the
/// receiver (`list_sig`), and `expected` is consulted only for the two
/// associated constructors that cannot name their own element type:
/// `List.empty()` gets it from the binding's written type
/// (`val xs: List<String> = List.empty()`), and `List.of(x)` from `x` or
/// from `expected` when both are available.
/// Whether a static method's parameter accepts an argument of this type: an
/// exact match, or a shared borrow of the argument's type (a `datac` parameter
/// is borrowed, so its resolved type is a `Ty::Ref` over the written type).
pub(crate) fn static_param_accepts(param: &Ty, arg: &Ty) -> bool {
    if param == arg {
        return true;
    }
    matches!(param, Ty::Ref { inner, .. } if **inner == *arg)
}

/// Resolves a static method written on a **trait's** own name (A-12 slice 6):
/// `From.from(x)`. The implementing type has to be inferable from the expected
/// type, because the trait name alone does not name one; the call then resolves
/// against that type's impls exactly as `AppErr.from(x)` does. `None` when the
/// trait declares no such static method, so the ordinary method path can
/// report it.
pub(crate) fn check_trait_static_call(
    trait_key: &str,
    method: &str,
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
) -> Result<Option<tir::Expr>> {
    let Some(info) = ctx.env.traits.get(trait_key) else {
        return Ok(None);
    };
    if !info
        .methods
        .iter()
        .any(|m| m.receiver.is_none() && m.name == method)
    {
        return Ok(None);
    }
    let Some(exp) = expected else {
        return Err(TypeError {
            help: None,
            message: format!(
                "'{trait_key}.{method}' is a static method, so the implementing type has to be known here: annotate the binding with its type, or call it on the type itself ('<Type>.{method}(...)'), which names it outright"
            ),
            span,
        });
    };
    let Some(owner) = struct_name_of(exp)
        .map(|s| s.to_string())
        .or_else(|| builtin_type_name(exp).map(|s| s.to_string()))
    else {
        return Err(TypeError {
            help: None,
            message: format!(
                "'{trait_key}.{method}' needs a concrete implementing type, and the expected type '{}' is not one: call it on the type itself ('<Type>.{method}(...)'), which names it outright",
                exp.display()
            ),
            span,
        });
    };
    check_static_method_call(&owner, method, args, span, ctx, expected)
}

/// The canonical type name a `Type.member(...)` receiver written as a bare
/// identifier stands for (A-12 slice 6): a declared `datac`/`enum`, a builtin
/// that carries methods, or the concrete type a bounded type parameter is
/// instantiated with (`X.from(x)` in a generic body). `None` when the name is a
/// local, a module alias, or no type at all.
pub(crate) fn static_owner_name(name: &str, ctx: &Ctx) -> Option<String> {
    if let Some(ty) = ctx.type_args.get(name) {
        return struct_name_of(ty)
            .map(|s| s.to_string())
            .or_else(|| builtin_type_name(ty).map(|s| s.to_string()));
    }
    if ctx.structs.contains_key(name) || ctx.is_enum(name) {
        return Some(name.to_string());
    }
    if crate::types::Ty::from_name(name).is_some() {
        return Some(name.to_string());
    }
    let qualified = ctx.qualify_own(name);
    if ctx.structs.contains_key(&qualified) || ctx.is_enum(&qualified) {
        return Some(qualified);
    }
    None
}

/// Resolves a **static** method call written on a type name (A-12 slice 6):
/// `AppErr.from(x)`, or `X.from(x)` where `X` is a bounded type parameter
/// instantiated with a concrete type. Returns `None` when the type has no
/// static member of that name, so the caller can fall back to the ordinary
/// method path and its diagnostic.
pub(crate) fn check_static_method_call(
    owner: &str,
    method: &str,
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
) -> Result<Option<tir::Expr>> {
    let _ = expected;
    let owner_ty = crate::types::Ty::from_name(owner)
        .unwrap_or_else(|| Ty::Struct(owner.to_string()));
    // Every `impl` of any trait for this type whose trait declares a static
    // method of this name is a candidate; the argument types then pick one.
    let mut candidates: Vec<(String, FnSig)> = Vec::new();
    for imp in ctx.env.trait_impls.iter() {
        if imp.type_name != owner {
            continue;
        }
        let Some(info) = ctx.env.traits.get(&imp.trait_name) else {
            continue;
        };
        let trait_args = trait_arg_tys(&imp.trait_arg_names);
        for m in &info.methods {
            if m.receiver.is_some() || m.name != method {
                continue;
            }
            let sig = resolve_trait_method_sig(
                info,
                m,
                &owner_ty,
                &trait_args,
                &ctx.structs,
                &ctx.enums,
                &ctx.env,
                &ctx.consts,
                &ctx.const_fns,
            )?;
            candidates.push((impl_method_key(imp, m), sig));
        }
    }
    if candidates.is_empty() {
        return Ok(None);
    }
    candidates.retain(|(_, s)| s.params.len() == args.len());
    if candidates.is_empty() {
        return Err(TypeError {
            help: None,
            message: format!(
                "'{owner}.{method}' is a static method, and no overload takes {} argument(s)",
                args.len()
            ),
            span,
        });
    }
    let (key, sig) = if candidates.len() == 1 {
        candidates.remove(0)
    } else {
        // Several instantiations of one trait for one type (`From<A>` and
        // `From<B>`): the argument types pick the one meant. A type-only
        // probe, exactly as overload resolution does.
        let mut arg_tys = Vec::with_capacity(args.len());
        for a in args {
            arg_tys.push(check_expr(&a.value, ctx, None)?.ty);
        }
        let mut hits: Vec<(String, FnSig)> = candidates
            .into_iter()
            .filter(|(_, s)| {
                s.params
                    .iter()
                    .zip(arg_tys.iter())
                    .all(|((_, p), a)| static_param_accepts(p, a))
            })
            .collect();
        match hits.len() {
            1 => hits.remove(0),
            0 => {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "no static method '{owner}.{method}' matches the argument type(s) {}",
                        arg_tys
                            .iter()
                            .map(|t| format!("'{}'", t.display()))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    span,
                })
            }
            _ => {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "call to static method '{owner}.{method}' is ambiguous: several trait impls match these argument types"
                    ),
                    span,
                })
            }
        }
    };
    ctx.check_capability(&key, span)?;
    let checked = match_args(
        &format!("static method '{owner}.{method}'"),
        &sig.params,
        args,
        span,
        ctx,
    )?;
    Ok(Some(tir::Expr {
        kind: tir::ExprKind::Call {
            callee: key,
            args: checked,
        },
        ty: sig.ret,
        span,
    }))
}

pub(crate) fn check_method_call(
    receiver: &ast::Expr,
    method: &str,
    type_args: &[ast::TypeExpr],
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
) -> Result<tir::Expr> {
    // `Bytes.empty()` / `Bytes.from_str(s)` / `List.of(x)`: an *associated*
    // builtin, written on the type name, so it takes no receiver argument and
    // is resolved before the receiver is checked as a value (it isn't one). A
    // local or an `impl` alias of the same name shadows the type name,
    // exactly as it would anywhere else - which is why those cases fall
    // through to the ordinary method path below.
    if let ast::Expr::Ident(name, _) = receiver {
        if ctx.lookup(name).is_none() && !is_module_alias(name, ctx) {
            if name == "List" || name == "Box" || name == "Map" {
                return check_container_associated(name, method, args, span, ctx, expected);
            }
            // `Atomic.new(x)` and `AtomicI64.new(0)`, the two spellings of the
            // one constructor of the builtin atomic cell (MK9-S3-M1 stage A1).
            if name == "Atomic" || crate::types::atomic_alias_elem(name).is_some() {
                return check_atomic_associated(name, method, args, span, ctx, expected);
            }
            // `Mutex.new(x)`, the one constructor of the builtin mutex
            // (B-5, `std/sync`). Like `Atomic.new`, the element type comes
            // from the value or from the binding's written type.
            if name == "Mutex" {
                return check_mutex_associated(name, method, args, span, ctx, expected);
            }
            // `RwLock.new(x)` and `Condvar.new()`, the constructors of the
            // builtin rwlock and condition variable (`std/sync`).
            if name == "RwLock" {
                return check_rwlock_associated(name, method, args, span, ctx, expected);
            }
            if name == "Condvar" {
                return check_condvar_associated(name, method, args, span, ctx, expected);
            }
            // `Widget.null()` and `Window.cast(w)` (gap 1): the two operations
            // written on a handle's type name rather than on a value.
            if let Some(handle) = ctx.handle_type_named(name) {
                return check_handle_associated(&handle, method, args, span, ctx);
            }
            // `Ptr.null()`, `Ptr.from_addr(a)`, `Ptr.from_ref(&x)`,
            // `Ptr.from_mut(x)` and `Ptr.alloc<T>(count)` (MK9-S2 stage 1).
            if name == "Ptr" {
                return check_ptr_associated(method, type_args, args, span, ctx, expected);
            }
            // A-12 slice 6: a static member written on the trait's own name
            // (`From.from(x)`), when the expected type names the implementing
            // type. Ambiguity is refused by name rather than guessed.
            let trait_key = ctx.qualify_own(name);
            if ctx.env.traits.contains_key(&trait_key) {
                if let Some(e) =
                    check_trait_static_call(&trait_key, method, args, span, ctx, expected)?
                {
                    return Ok(e);
                }
            }
            // A-12 slice 6: a static member of a trait impl written on the
            // type (`AppErr.from(x)`), or on a bounded type parameter
            // instantiated with a concrete type (`X.from(x)`).
            if let Some(owner) = static_owner_name(name, ctx) {
                if let Some(e) =
                    check_static_method_call(&owner, method, args, span, ctx, expected)?
                {
                    return Ok(e);
                }
            }
            let key = format!("{name}#{method}");
            if let Some(assoc) = BUILTINS.get(&key).filter(|b| b.associated) {
                let params = assoc.params.clone();
                let ret = assoc.ret.clone();
                let checked = match_args(&format!("'{name}.{method}'"), &params, args, span, ctx)?;
                return Ok(tir::Expr {
                    kind: tir::ExprKind::Call { callee: key, args: checked },
                    ty: ret,
                    span,
                });
            }
        }
    }
    // The same two operations written through a module alias:
    // `mem.Buffer.null()` and `mem.Buffer.cast(w)`. The receiver is a dotted
    // type path, not a value, which is why it is read here before the probe.
    if !matches!(receiver, ast::Expr::Ident(..)) {
        if let Some(path) = expr_path(receiver) {
            if !is_module_alias(&path, ctx) {
                if let Some(handle) = ctx.handle_type_named(&path) {
                    return check_handle_associated(&handle, method, args, span, ctx);
                }
            }
        }
    }

    // Type-only probe to learn the receiver's owner name; re-checked for
    // real (with its actual borrow/move effects) via `check_call_arg`
    // below. `check_expr` has no persistent side effects beyond reading
    // `ctx`, so checking the same expression twice here is safe.
    let probe = check_expr(receiver, ctx, None)?;
    // `n.to_byte()` and `n.try_to_byte()` are the everyday spellings of the
    // `u8` conversions (issue #279): from here on the call is the builtin's.
    let method = numeric_alias(&probe.ty, method).unwrap_or(method);
    // `n.toText()` on any number is the text of the value (issue #262).
    if method == "toText" && probe.ty.is_numeric() {
        return check_number_to_text(receiver, &probe.ty, args, span, ctx);
    }
    // A method call on a borrowed trait object (A-12 slice 4) goes through the
    // vtable: the trait's flattened method list gives the slot, the receiver's
    // type gives the mutability, and the trait's own signature gives the
    // argument types.
    if let Ty::Dyn { mutable, trait_name } = &probe.ty {
        let (mutable, trait_name) = (*mutable, trait_name.clone());
        return check_dyn_method_call(probe, &trait_name, mutable, method, args, span, ctx);
    }
    // A method call on an owned trait object (A-12 slice 7): `is_empty` and
    // `get` are the box's own two methods, and every other name goes through the
    // trait's vtable exactly as it does on a `&dyn`. A *borrow* of the box (a
    // `mut Box<dyn Trait>` parameter's `&Box<dyn Trait>`, or a `List.get` of an
    // element) is seen through to the box, because a box holds a pointer and the
    // borrow is what reaches it.
    let box_dyn_trait = match &probe.ty {
        Ty::BoxDyn { trait_name } => Some(trait_name.clone()),
        Ty::Ref { inner, .. } => match &**inner {
            Ty::BoxDyn { trait_name } => Some(trait_name.clone()),
            _ => None,
        },
        _ => None,
    };
    if let Some(trait_name) = box_dyn_trait {
        return check_box_dyn_method_call(probe, &trait_name, method, args, span, ctx);
    }
    // `w.is_null()` (gap 1): the one method an opaque handle has, comparing
    // the pointer against the type's null value. Any other name is a method
    // the handle does not have, which is a better diagnostic than the
    // generic "non-struct type" below.
    if let Ty::Handle(handle) = &probe.ty {
        if method == "is_null" {
            ctx.require_extern_section(&format!("{handle}.is_null()"), span)?;
            if !args.is_empty() {
                return Err(TypeError {
                    help: None,
                    message: format!("'is_null' takes no arguments, found {}", args.len()),
                    span,
                });
            }
            return Ok(tir::Expr {
                kind: tir::ExprKind::Call {
                    callee: crate::types::HANDLE_IS_NULL_KEY.to_string(),
                    args: vec![probe],
                },
                ty: Ty::Bool,
                span,
            });
        }
        return Err(TypeError {
            help: None,
            message: format!(
                "'{handle}' is an opaque native handle with no readable contents: 'is_null()' is its only method"
            ),
            span,
        });
    }
    // A method on a `Ptr<T>` (MK9-S2 stage 1). Every signature depends on the
    // element type, and `cast<U>()` also takes a written target, so none of
    // them can live in the static builtin table. The receiver is `Copy`, so it
    // goes through `check_call_arg` as a plain value rather than a borrow.
    if crate::types::ptr_elem_of(&probe.ty).is_some() {
        return check_ptr_method(probe, receiver, method, type_args, args, span, ctx);
    }
    let owner = struct_name_of(&probe.ty)
        .map(|s| s.to_string())
        .or_else(|| builtin_type_name(&probe.ty).map(|s| s.to_string()));
    let Some(owner) = owner else {
        return Err(TypeError {
            help: None,
            message: format!("cannot call method '{method}' on non-struct type '{}'", probe.ty.display()),
            span,
        });
    };
    // Kernel stage 9, MK9-S6-M1: a destructor is not callable. Naming it would
    // run it on top of the drop the value already owes, so the explicit call is
    // refused with the reason.
    if method == crate::types::DROP_METHOD
        && ctx.sig_of(&crate::types::drop_method_key(&owner)).is_some()
    {
        return Err(TypeError {
            help: None,
            message: format!(
                "'{owner}.drop' cannot be called: an explicit destructor call is an error, because the value's own drop already runs when its owner dies"
            ),
            span,
        });
    }
    // A method on an `Atomic<T>` (MK9-S3-M1 stage A1). Its signature depends on
    // the element type, and `compare_exchange`'s `Result<T, T>` is an instance
    // of the stdlib enum rather than a fixed type, so neither can live in the
    // static builtin table. The receiver is a place, so it goes through the same
    // `check_call_arg` a declared method's does, which is what makes `store`
    // and the `fetch_*` require a `mut` binding, since they write the location.
    // `m.to_int()` (kernel stage 2): a field-less enum's compile-time
    // discriminant, read as the repr integer type. Resolved before the method
    // tables because an enum has no declaration to search and no method of
    // that name in the builtin table either; a method the enum declares under
    // that name still wins.
    if method == "to_int"
        && ctx.is_enum(&owner)
        && ctx.sig_of(&format!("{owner}#to_int")).is_none()
    {
        return lower_enum_to_int(probe.ty.clone(), receiver, span, ctx);
    }
    if let Some(elem) = crate::types::mutex_elem_of(&probe.ty).cloned() {
        return check_mutex_method(probe, receiver, &elem, method, args, span, ctx, expected);
    }
    if let Some(elem) = crate::types::rwlock_elem_of(&probe.ty).cloned() {
        return check_rwlock_method(probe, receiver, &elem, method, args, span, ctx, expected);
    }
    if matches!(probe.ty, Ty::Condvar) {
        return check_condvar_method(probe, receiver, method, args, span, ctx);
    }
    if let Some(elem) = crate::types::atomic_elem_of(&probe.ty).cloned() {
        let key = format!("Atomic#{method}");
        if let Some(sig) = atomic_sig(&key, &elem, ctx, span)? {
            let mut checked = vec![check_call_arg("'self'", &sig.params[0].1, receiver, ctx)?];
            checked.extend(match_args(
                &format!("method 'Atomic.{method}'"),
                &sig.params[1..],
                args,
                span,
                ctx,
            )?);
            return Ok(tir::Expr {
                kind: tir::ExprKind::Call { callee: key, args: checked },
                ty: sig.ret,
                span,
            });
        }
    }
    // `n.try_to_u8()` (MISSING-RbhdW): the fallible twin of `n.to_u8()`. A
    // builtin has no declaration to resolve its return type at, so the call is
    // lowered here into a shape both backends already lower: the receiver
    // bound once, the range test, and one arm per `Option` variant. It is
    // resolved before the method tables because an integer has no table to
    // search and no declaration ever names this method.
    // `x.wrapping_shl(n)` / `x.wrapping_shr(n)`: the shift with the amount
    // masked to the width (`n & (bits - 1)`), the escape from the run-time
    // range check a plain `<<`/`>>` makes. Lowered here to the ordinary
    // shift, so both backends and a freestanding build need nothing new.
    if matches!(method, "wrapping_shl" | "wrapping_shr") {
        if let Some(bits) = int_bits(&probe.ty) {
            let ty = probe.ty.clone();
            let value = check_call_arg("'self'", &ty, receiver, ctx)?;
            let amount = match_args(
                &format!("method '{}.{method}'", ty.display()),
                &[("n".to_string(), ty.clone())],
                args,
                span,
                ctx,
            )?
            .remove(0);
            let mask = (bits - 1) as i128;
            let masked = match &amount.kind {
                tir::ExprKind::IntLit(v) => tir::Expr {
                    kind: tir::ExprKind::IntLit(*v & mask),
                    ty: ty.clone(),
                    span,
                },
                _ => tir::Expr {
                    kind: tir::ExprKind::Binary {
                        op: ast::BinOp::BitAnd,
                        lhs: Box::new(amount),
                        rhs: Box::new(tir::Expr {
                            kind: tir::ExprKind::IntLit(mask),
                            ty: ty.clone(),
                            span,
                        }),
                    },
                    ty: ty.clone(),
                    span,
                },
            };
            let op = if method == "wrapping_shl" { ast::BinOp::Shl } else { ast::BinOp::Shr };
            return Ok(tir::Expr {
                kind: tir::ExprKind::Binary {
                    op,
                    lhs: Box::new(value),
                    rhs: Box::new(masked),
                },
                ty,
                span,
            });
        }
    }
    if let Some((src, dst)) = try_to_pair(&probe.ty, method) {
        let value = check_call_arg("'self'", &src, receiver, ctx)?;
        return lower_try_to(&src, &dst, value, span, ctx);
    }
    // `cfg.on_event(x)` where the receiver has a *field* of function type
    // (A-4): the call syntax is a method's, the callee is a field, and the
    // field is a place holding a function value. A declared method of the
    // same name wins - the declaration is the more specific statement of what
    // the type does - so this is only consulted when there is no such method.
    if ctx.sig_of(&format!("{owner}#{method}")).is_none() {
        if let Some(fn_ty) = fn_field_ty(&probe.ty, method, ctx) {
            // A field holding a closure is callable the same way (A-4 phase
            // E): the call goes through the pair the field holds.
            if matches!(fn_ty, Ty::Fn { .. } | Ty::Closure { .. } | Ty::ScopedFn { .. }) {
                let callee = tir::Expr {
                    kind: tir::ExprKind::FieldAccess {
                        base: Box::new(probe),
                        field: method.to_string(),
                    },
                    ty: fn_ty.clone(),
                    span,
                };
                return check_indirect_call_expr(
                    callee,
                    &fn_ty,
                    args,
                    span,
                    ctx,
                    &format!("field '{method}'"),
                );
            }
        }
    }
    let key = format!("{owner}#{method}");
    // A container a bare `List.empty()`/`Box.empty()`/`Map.new()` left with no
    // element type (`types::is_open_container`) is decided by the first call
    // that has to know it. `push`, `insert` and `set` hand the element in, so
    // the binding is repaired in place here and every later statement sees the
    // real type. A call that does not decide it is refused: monomorphizing a
    // `List<Unit>` routine names a routine nothing builds.
    let decided = if crate::types::is_open_container(&probe.ty) {
        match open_container_from_call(&probe.ty, method, args, ctx)? {
            Some(filled) => {
                if let ast::Expr::Ident(name, _) = receiver {
                    if ctx.lookup(name).is_some() {
                        ctx.set_binding_ty(name, filled.clone());
                    }
                }
                filled
            }
            // A deciding method whose argument list is not the shape it needs:
            // the ordinary arity check says more than this one could.
            None if decides_container_element(&probe.ty, method) => probe.ty.clone(),
            None => return Err(open_container_use_error(&probe.ty, receiver, span)),
        }
    } else {
        probe.ty.clone()
    };
    let container_elem = crate::types::list_elem_of(&decided)
        .or_else(|| crate::types::box_elem_of(&decided))
        .or_else(|| crate::types::map_elem_of(&decided))
        .or_else(|| crate::types::array_elem_of(&decided))
        .cloned();
    let container_key = crate::types::map_key_of(&decided)
        .cloned()
        .unwrap_or(Ty::Unit);
    // A blanket impl's method (`impl<T: Bound> Trait for T`) is the last source
    // a method name is tried against: an inherent method, a concrete or generic
    // impl and a builtin all come first. Its result is taken only in the arm
    // below, and only when nothing else matched.
    let mut blanket_tmpl = blanket_method_template(ctx, &probe.ty, method, span);
    let (key, sig) = match ctx.sig_of(&key) {
        Some(s) => (key, s),
        // A-2: a method declared on a generic type (`fun Pair<A, B>.first()`).
        // Its type arguments are the receiver's own instantiation, which the
        // probe already resolved, so the template key is the *type template*
        // plus the method name - and the instance's key is what the call names.
        None => match method_template_key(ctx, &owner, method) {
            Some(tmpl_key) => {
                instantiate_function(ctx, &tmpl_key, &[], args, span, expected, Some(&probe.ty))?
            }
            None => match container_elem.as_ref().and_then(|e| {
                let elem_sortable = ctx.value_sortable(e);
                let elem_clonable = ctx.value_clonable(e);
                container_sig(
                    &key,
                    e,
                    &container_key,
                    crate::types::array_len_of(&probe.ty),
                    elem_sortable,
                    elem_clonable,
                )
            }) {
                Some(s) => (key, s),
                None => match builtin_sig(&key) {
                    Some(s) => (key, s),
                    None if blanket_tmpl.is_some() => {
                        let found = blanket_tmpl.take().expect("the guard saw a blanket method");
                        let tmpl_key = found?;
                        instantiate_function(ctx, &tmpl_key, &[], args, span, expected, Some(&probe.ty))?
                    }
                    None => {
                        let base = match (&container_elem, owner.as_str()) {
                // A destructor type is not `Copy` (kernel stage 9,
                // MK9-S6-M1): the derived copy fixpoint excludes it, so this
                // names the destructor rather than reporting a field that
                // cannot be copied.
                (Some(Ty::Struct(n)), "List")
                    if method == "clone" && ctx.env.declared_drops.contains(n) =>
                {
                    format!(
                        "List<{n}>.clone is not defined: '{n}' declares 'fun mut {n}.drop()', and a type with a destructor is not `Copy`, so a second owner of the same resource would be freed twice"
                    )
                }
                // Three List methods are defined only for an element type that
                // can be ordered, compared, or copied, so the generic "no
                // method" text would be misleading for exactly these three.
                (Some(e), "List")
                    if matches!(e, Ty::Struct(_))
                        && matches!(method, "sort" | "contains" | "slice" | "clone") =>
                {
                    format!(
                        "List<{}>.{method} is not defined: '{}' has no {}, and a 'datac'/'enum' takes one from its fields, so every field it holds has to have it too - use an element whose fields all do",
                        e.display(),
                        e.display(),
                        match method {
                            "sort" => "order",
                            "contains" => "equality",
                            _ => "copy",
                        }
                    )
                }
                (Some(e), "List") => format!(
                    "List<{}> has no method '{method}' - the List surface is len, is_empty, push, get, set, pop, clear, sort, reverse, contains, slice, clone, insert, remove",
                    e.display()
                ),
                (Some(e), "Map") => format!(
                    "Map<{}> has no method '{method}' - the Map surface is len, is_empty, has, get, get_or, insert, remove, clear, keys, values",
                    e.display()
                ),
                (Some(e), "Array") => format!(
                    "[{}; N] has no method '{method}': the array surface is len, is_empty, get, set",
                    e.display()
                ),
                (Some(e), _) => format!(
                    "Box<{}> has no method '{method}' - the Box surface is is_empty, get, set, take",
                    e.display()
                ),
                // An `Atomic<T>` with no such method: `bool` has the five
                // conversions' worth of surface and nothing more, so the
                // generic "datac 'Atomic'" text would name a datac that does
                // not exist.
                (None, "Atomic") => {
                    let elem = crate::types::atomic_elem_of(&probe.ty)
                        .expect("an Atomic receiver names its element");
                    if elem.is_integer() {
                        format!(
                            "Atomic<{}> has no method '{method}': the atomic surface is new, load, store, swap, compare_exchange, fetch_add, fetch_sub, fetch_and, fetch_or, fetch_xor",
                            elem.display()
                        )
                    } else {
                        format!(
                            "Atomic<{}> has no method '{method}': the atomic surface is new, load, store, swap and compare_exchange, and 'bool' has no atomic arithmetic, so there is no fetch_add/fetch_sub/fetch_and/fetch_or/fetch_xor for it",
                            elem.display()
                        )
                    }
                }
                (None, _) if probe.ty.is_integer() => format!(
                    "type '{}' has no method '{method}': an integer has to_i32, to_i64, to_u32, to_u64, to_byte, to_u16, to_usize, to_isize, to_f64, to_f32, swap_bytes, to_be and to_le, wrapping_shl and wrapping_shr, plus a try_to_<T> twin of every conversion that can lose a value (which hands back 'Option<T>'), and no other type converts",
                    probe.ty.display()
                ),
                // An `enum` and a `datac` miss the same way, and an enum
                // binding reaches this arm since its methods are resolved
                // like any other type's, so the text names the kind it is.
                (None, _) if ctx.is_enum(&owner) => {
                    format!("enum '{owner}' has no method '{method}'")
                }
                (None, _) => format!("datac '{owner}' has no method '{method}'"),
                        };
                        return Err(TypeError {
                            help: None,
                            message: format!(
                                "{base}{}",
                                shadowed_module_alias_note(receiver, method, ctx)
                            ),
                            span,
                        });
                    }
                },
            },
        },
    };

    // The capability gate applies to a *method* of a gated module exactly as
    // it does to a free function of one: the key is `Type#method` with the
    // receiver type's module prefix, and the gate table holds it. A method of
    // a gated module reaches the resource without naming any of the module's
    // free functions (`Child.kill` is `kill_raw` plus an error check), so a
    // call that skipped this check reached the syscall with no grant at all.
    ctx.check_capability(&key, span)?;
    // An `unsafe fun` receiver method (MK9-S2 stage 1) is callable only from
    // an `unsafe` context, exactly as a free one is.
    if ctx.unsafe_fns.contains(&key) && ctx.unsafe_depth == 0 {
        return Err(TypeError {
            help: None,
            message: format!(
                "'{owner}.{method}' is an 'unsafe fun': it can only be called inside 'unsafe {{ ... }}' or from another 'unsafe fun'"
            ),
            span,
        });
    }
    let mut checked_args = Vec::with_capacity(sig.params.len());
    ctx.keep_probe(receiver, probe);
    // A shared borrow cannot call a method that writes its receiver (issue #286):
    // the refusal names the rule, whichever way the mode was written.
    checked_args.push(check_call_arg("'self'", &sig.params[0].1, receiver, ctx).map_err(|mut e| {
        if e.message.contains("requires an exclusive borrow") {
            e.message = format!(
                "'{owner}.{method}' writes its receiver, so a call needs an exclusive borrow, and a shared borrow cannot make it: a method's receiver is exclusive when its body writes self, or when it is written 'fun mut'"
            );
        }
        e
    })?);
    ctx.str_arg_site = Some(if key.contains('<') || trait_declares_method(ctx, method) {
        StrArgSite::Refused(format!("method '{owner}.{method}'"))
    } else {
        StrArgSite::Implicit
    });
    // A store into a shared container names the container in the refusal of a
    // block value (golden G1). A receiver with no place name keeps the move wording.
    ctx.box_store_into = match key.as_str() {
        "List#push" | "List#insert" | "List#set" | "Map#insert" | "Box#set" => place_root_name(receiver),
        _ => None,
    };
    checked_args.extend(match_args(&format!("method '{owner}.{method}'"), &sig.params[1..], args, span, ctx)?);
    // A method whose result borrows from its receiver (`s.as_str()`) has the
    // same rule as a free function.
    require_place_for_borrowed_arg(&sig.params, &sig.ret, &checked_args, span)?;
    // Receiver methods can reference passdowns too; the call site passes the
    // same hidden arguments a free-function call would.
    checked_args.extend(hidden_args(&key, span, ctx)?);

    Ok(tir::Expr {
        kind: tir::ExprKind::Call { callee: key, args: checked_args },
        ty: sig.ret,
        span,
    })
}
