use super::*;

/// A pattern `when` whose scrutinee is a temporary stages it into a local of
/// its own and matches the rebased place: `when mk().t { ... }` becomes
/// `{ val #when0 = mk()  when #when0.t { ... } }`. The arms read their payloads
/// through a borrow, so the scrutinee has to be a place that outlives them, and
/// binding the *root* is what frees the whole temporary once, at the end of the
/// block and after the last arm, rather than leaking every field the match
/// did not read (`heap_field_of_owned_temporary.kp`).
pub(crate) fn check_when_scrutinee(
    scrutinee: &ast::Expr,
    arms: &[ast::WhenArm],
    span: Span,
    let_else: bool,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
) -> Result<tir::Expr> {
    let Some(root) = ast::temporary_root(scrutinee).cloned() else {
        return check_when(scrutinee, arms, span, let_else, ctx, expected);
    };
    ctx.when_temps += 1;
    let tmp = format!("{}{}", WHEN_TEMP_PREFIX, ctx.when_temps - 1);
    let init = check_expr(&root, ctx, None)?;
    let bind_ty = init.ty.clone();
    let rebased = ast::rebase_root(scrutinee, &ast::Expr::Ident(tmp.clone(), span));
    // The binding is visible to the arms and to nothing outside them: a
    // `#when0` cannot collide with a written name, and popping the scope is
    // what keeps a nested `when` from resolving its own temporary to this one.
    ctx.scopes.push(HashMap::new());
    ctx.define(&tmp, bind_ty.clone(), false);
    let inner = check_when(&rebased, arms, span, let_else, ctx, expected);
    ctx.scopes.pop();
    let inner = inner?;
    let ty = inner.ty.clone();
    Ok(tir::Expr {
        kind: tir::ExprKind::Block(tir::Block {
            stmts: vec![tir::Stmt::Let {
                name: tmp,
                ty: bind_ty,
                init,
            }],
            tail: Some(Box::new(inner)),
        }),
        ty,
        span,
    })
}

/// The dotted path an expression names, if it is one: `Shape`, `shapes.Shape`,
/// `sh.Shape`. Used to recognize the enum half of `Enum.Variant(...)` before
/// the receiver is checked as a value (A-1).
pub(crate) fn expr_path(expr: &ast::Expr) -> Option<String> {
    match expr {
        ast::Expr::Ident(name, _) => Some(name.clone()),
        ast::Expr::FieldAccess { base, field, .. } => {
            Some(format!("{}.{}", expr_path(base)?, field))
        }
        _ => None,
    }
}

/// Whether `expr` names an `enum` (A-1), resolving the same way a type name
/// does: a bare name belongs to this module, and an alias-qualified one to the
/// module it was imported from.
pub(crate) fn enum_path(expr: &ast::Expr, ctx: &Ctx) -> Option<String> {
    // A local, parameter or passdown of the same name shadows the type, the
    // same way it shadows a module alias.
    if let ast::Expr::Ident(name, _) = expr {
        if ctx.lookup(name).is_some() || ctx.pds.contains_key(name) {
            return None;
        }
    }
    let path = expr_path(expr)?;
    if ctx.is_enum(&path) {
        return Some(path);
    }
    // The built-in `BlockError` is named with no import (decision 139).
    if path == "BlockError" {
        return ctx.is_enum(&block_error_name()).then(block_error_name);
    }
    let qualified = match path.split_once('.') {
        Some((alias, item)) => format!("{}{}", ctx.aliases.get(alias)?, item),
        None => ctx.qualify_own(&path),
    };
    ctx.is_enum(&qualified).then_some(qualified)
}

/// The concrete `enum`s declared in the calling module that declare a variant
/// named `name`, by their short names, sorted (#281). A bare variant name is
/// looked up only here: an imported enum keeps its qualifier. The prelude
/// names keep their meaning, so they never belong to this set.
pub(crate) fn bare_variant_owners(name: &str, ctx: &Ctx) -> Vec<String> {
    if matches!(name, "None" | "Some" | "Ok" | "Err") {
        return Vec::new();
    }
    let mut owners: Vec<String> = ctx
        .enums
        .iter()
        .filter_map(|(key, def)| {
            let short = key.strip_prefix(ctx.prefix.as_str())?;
            (!short.contains('.') && def.variant(name).is_some()).then(|| short.to_string())
        })
        .collect();
    owners.sort();
    owners
}

/// `Circle(r = 1.0)` or `Empty` with no qualifier (#281): the variant of the one
/// module enum that declares it. Two or more owners is a clash, and the message
/// names the qualifier to write. A payload-carrying variant named without its
/// arguments gets the refusal `Shape.Circle` gets.
pub(crate) fn check_bare_variant(
    name: &str,
    args: Option<&[ast::CallArg]>,
    span: Span,
    ctx: &mut Ctx,
) -> Result<tir::Expr> {
    let owners = bare_variant_owners(name, ctx);
    match owners.as_slice() {
        [] => Err(TypeError {
            help: None,
            message: format!("undefined variable '{name}'"),
            span,
        }),
        [one] => {
            let ename = ctx.qualify_own(one);
            let edef = ctx.enum_def(&ename).expect("bare_variant_owners only returns known enums");
            let (_, vdef) = edef.variant(name).expect("the owner declares the variant");
            if args.is_none() && !vdef.fields.is_empty() {
                let fields: Vec<String> = vdef.fields.iter().map(|f| f.name.clone()).collect();
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "variant '{one}.{name}' carries payload ({}) - construct it as '{one}.{name}({})'",
                        fields.join(", "),
                        fields.join(" = ..., ")
                    ),
                    span,
                });
            }
            check_enum_construction(&ename, name, args.unwrap_or(&[]), span, ctx)
        }
        many => {
            let written: Vec<String> = many.iter().map(|o| format!("'{o}.{name}'")).collect();
            let (last, rest) = written.split_last().expect("at least two owners");
            Err(TypeError {
                help: None,
                message: format!(
                    "'{name}' is a variant of more than one enum in this module, so it must be written with its enum: {} or {last}",
                    rest.join(", ")
                ),
                span,
            })
        }
    }
}

/// The generic-template counterpart of `enum_path` (A-2): the qualified name of
/// the template `expr` names, when it names one. `Option.Some(v = 1)` and
/// `Option.None` both arrive here, because a generic enum has no concrete
/// entry in the enum table for `enum_path` to find.
pub(crate) fn generic_type_path(expr: &ast::Expr, ctx: &Ctx) -> Option<String> {
    if let ast::Expr::Ident(name, _) = expr {
        if ctx.lookup(name).is_some() || ctx.pds.contains_key(name) {
            return None;
        }
    }
    let path = expr_path(expr)?;
    if ctx.env.generics.contains_key(&path) {
        return Some(path);
    }
    let qualified = match path.split_once('.') {
        Some((alias, item)) => format!("{}{}", ctx.aliases.get(alias)?, item),
        None => {
            // A bare `Option` or `Result` names the std type (#300), unless the
            // file declares a type of that name itself.
            let own = ctx.qualify_own(&path);
            let declared_here = ctx.env.generics.contains_key(&own)
                || ctx.structs.contains_key(&own)
                || ctx.enums.contains_key(&own);
            crate::stdlib::bare_default_type(&path, declared_here, &ctx.aliases).unwrap_or(own)
        }
    };
    ctx.env.generics.contains_key(&qualified).then_some(qualified)
}

/// The qualified name of the generic template `name` names, if it names one
/// (A-2).
pub(crate) fn generic_template_name(name: &str, ctx: &Ctx) -> Option<String> {
    if ctx.env.generics.contains_key(name) {
        return Some(name.to_string());
    }
    let qualified = ctx.qualify_own(name);
    ctx.env
        .generics
        .contains_key(&qualified)
        .then_some(qualified)
}

/// Pairs every parameter with the index of the argument that fills it,
/// honouring named arguments exactly as `match_args` does. Factored out so
/// type-argument inference (A-2) can see the pairing before the parameter
/// types are known - `match_args` needs the types, and the types need the
/// pairing.
pub(crate) fn arg_slots(what: &str, names: &[String], args: &[ast::CallArg], span: Span) -> Result<Vec<usize>> {
    if args.len() != names.len() {
        return Err(TypeError {
            help: None,
            message: format!("{what} expects {} argument(s), found {}", names.len(), args.len()),
            span,
        });
    }
    let mut named: HashMap<&str, usize> = HashMap::new();
    for (i, a) in args.iter().enumerate() {
        if let Some(n) = &a.name {
            if named.insert(n.as_str(), i).is_some() {
                return Err(TypeError {
                    help: None,
                    message: format!("argument '{n}' specified more than once"),
                    span,
                });
            }
        }
    }
    let mut positional = args.iter().enumerate().filter(|(_, a)| a.name.is_none());
    let mut slots = Vec::with_capacity(names.len());
    for name in names {
        let idx = match named.remove(name.as_str()) {
            Some(i) => i,
            None => positional
                .next()
                .map(|(i, _)| i)
                .ok_or_else(|| TypeError {
                    help: None,
                    message: format!("{what} is missing argument '{name}'"),
                    span,
                })?,
        };
        slots.push(idx);
    }
    if let Some((&leftover, _)) = named.iter().next() {
        return Err(TypeError {
            help: None,
            message: format!("{what} has no parameter named '{leftover}'"),
            span,
        });
    }
    Ok(slots)
}

/// Resolves an expression's explicit type arguments (`Pair<i32, String>(...)`,
/// `Option.Some<i32>(...)`), which are written types exactly like a
/// signature's (A-2).
pub(crate) fn resolve_explicit_args(type_args: &[ast::TypeExpr], ctx: &Ctx) -> Result<Vec<Ty>> {
    if type_args.is_empty() {
        return Ok(Vec::new());
    }
    let scope = ctx.type_scope();
    let mut out = Vec::with_capacity(type_args.len());
    for t in type_args {
        // A const argument (A-20): a literal or an arithmetic expression the
        // parser already recognized, folded with no declared type here and
        // re-typed by `bind_const_args` once the template is known.
        if let Some(expr) = &t.const_expr {
            out.push(scope.fold_const_arg_any(expr)?);
            continue;
        }
        match scope.resolve(t, true) {
            Ok(ty) => out.push(ty),
            // A bare or dotted name that is not a type may be a named `const`
            // (`sum<SIZE>`): fold it, and let the template's parameter list
            // decide which one it is. A name that is neither keeps its type
            // error.
            Err(type_err) => match named_const_expr(t) {
                Some(expr) if scope.fold_const_arg_any(&expr).is_ok() => {
                    out.push(scope.fold_const_arg_any(&expr)?)
                }
                _ => return Err(type_err),
            },
        }
    }
    Ok(out)
}

/// Confirms the argument count of an explicit instantiation (`Pair<i32, String>`,
/// A-2) and returns it as the type arguments.
pub(crate) fn check_arity(name: &str, params: &[String], explicit: &[Ty], span: Span) -> Result<Vec<Ty>> {
    if explicit.len() != params.len() {
        return Err(TypeError {
            help: None,
            message: format!(
                "'{name}' takes {} type argument(s), found {}",
                params.len(),
                explicit.len()
            ),
            span,
        });
    }
    Ok(explicit.to_vec())
}

/// Binds a template's type parameters from one written field type and the
/// type the corresponding argument actually has (A-2). Only the shapes that
/// can carry a parameter are inspected: a bare parameter name, or a parameter
/// under a `List`/`Box`/`Map`. A field type that mentions no parameter is
/// ignored here - its argument is checked against it later, by `match_args`,
/// which is also where a mismatch is reported.
/// A written type name as its canonical key, resolved against the module the
/// *declaration* was written in rather than the module the call site is in -
/// `Result<T, E>` inside `std/result.kp` is `std.result.Result<T, E>` no matter
/// who calls it.
pub(crate) fn qualify_written(name: &str, prefix: &str, aliases: &HashMap<String, String>) -> String {
    match name.split_once('.') {
        Some((alias, item)) => match aliases.get(alias) {
            Some(p) => format!("{p}{item}"),
            None => name.to_string(),
        },
        None => format!("{prefix}{name}"),
    }
}

pub(crate) fn bind_type_params(
    env: &TypeEnv,
    prefix: &str,
    aliases: &HashMap<String, String>,
    written: &ast::TypeExpr,
    actual: &Ty,
    params: &[String],
    out: &mut HashMap<String, Ty>,
) {
    // A parameter that is borrowed at the call site (`fun is_some<T>(o: &Option<T>)`
    // called with an `Option<i32>`) still determines the parameter, and a
    // parameter that is *already* a borrow determines it from its pointee.
    let actual = match actual {
        Ty::Ref { inner, .. } => inner.as_ref(),
        other => other,
    };
    // A function type (A-4) pairs a written type against a function value
    // structurally: the parameters in order, then the return. This is what
    // lets `map<T, U>(o: &Option<T>, f: fun(&T) => U)` take both `T` and `U`
    // from its arguments, and it is the only place a function type is bound.
    // A closure type (A-4 phase E) pairs the same way, which is what lets a
    // callback written as `f: closure fun(&T) => U` infer `U` from a literal.
    if let Some(written_params) = &written.fn_params {
        let implied = match actual {
            Ty::Fn { params, ret }
            | Ty::Closure { params, ret }
            | Ty::ScopedFn { params, ret } => Some((params, ret)),
            _ => None,
        };
        if let Some((actual_params, actual_ret)) = implied {
            if written_params.len() == actual_params.len() {
                for (w, a) in written_params.iter().zip(actual_params.iter()) {
                    bind_type_params(env, prefix, aliases, &w.ty, a, params, out);
                }
                if let Some(written_ret) = &written.fn_ret {
                    bind_type_params(env, prefix, aliases, written_ret, actual_ret, params, out);
                }
            }
        }
        return;
    }
    // A projection (`I::Item`, A-12 slice 5) determines no type parameter: the
    // concrete type is the impl's, so nothing about `I` can be read off an
    // argument of it. An explicit type argument (or another parameter) is what
    // must determine `I`.
    if written.assoc.is_some() {
        return;
    }
    // A fixed-size array (A-18) binds its element parameters and, when the
    // length is a bare parameter name, the const parameter the length stands
    // for (A-20): `sum(&arr)` infers `N` from `arr`'s own length.
    if let Some(written_elem) = &written.array_elem {
        if let Ty::Array(actual_elem, len) = actual {
            bind_type_params(env, prefix, aliases, written_elem, actual_elem, params, out);
            if let Some(len_expr) = written.array_len.as_deref() {
                if let ast::Expr::Ident(name, _) = len_expr {
                    if params.iter().any(|p| p == name) {
                        out.entry(name.clone()).or_insert(Ty::ConstInt {
                            value: *len as i128,
                            ty: Box::new(Ty::Usize),
                        });
                    }
                }
            }
        }
        return;
    }
    if written.args.is_empty() {
        if params.iter().any(|p| p == &written.name) {
            out.entry(written.name.clone())
                .or_insert_with(|| actual.clone());
        }
        return;
    }
    match (written.name.as_str(), actual) {
        ("List", Ty::List(e)) | ("Box", Ty::Box(e)) => {
            bind_type_params(env, prefix, aliases, &written.args[0], e, params, out)
        }
        // `Map<V>` writes the value alone (the key is `String`), so only a
        // two-argument `Map<K, V>` has a key to bind.
        ("Map", Ty::Map(k, v)) => {
            if written.args.len() == 2 {
                bind_type_params(env, prefix, aliases, &written.args[0], k, params, out);
            }
            bind_type_params(
                env,
                prefix,
                aliases,
                &written.args[written.args.len() - 1],
                v,
                params,
                out,
            );
        }
        // A user generic type (`Pair<T, T>` against `Pair<i32, i32>`): the
        // instantiation records the template it came from and the arguments it
        // was built with, which is exactly the pairing wanted here.
        _ => {
            let mine = qualify_written(&written.name, prefix, aliases);
            if let Ty::Struct(key) = actual {
                let origin = env.instances.borrow().origin.get(key).cloned();
                if let Some((tname, targs)) = origin {
                    if tname == mine {
                        for (w, a) in written.args.iter().zip(targs.iter()) {
                            bind_type_params(env, prefix, aliases, w, a, params, out);
                        }
                    }
                }
            }
        }
    }
}

/// Completes a partial set of type-argument bindings from the expected type:
/// `val x: Option<i32> = Option.None` has nothing else to go on (A-2).
pub(crate) fn bind_from_expected(
    tmpl: &GenericTemplate,
    expected: Option<&Ty>,
    ctx: &Ctx,
    out: &mut HashMap<String, Ty>,
) {
    let Some(Ty::Struct(key)) = expected else {
        return;
    };
    let st = ctx.env.instances.borrow();
    let Some((name, args)) = st.origin.get(key) else {
        return;
    };
    if name != &tmpl.name {
        return;
    }
    for (p, a) in tmpl.params.iter().zip(args.iter()) {
        out.entry(p.clone()).or_insert_with(|| a.clone());
    }
}

/// The complete argument list for one instantiation, or an error naming the
/// parameter that could not be inferred.
pub(crate) fn finish_type_args(
    name: &str,
    params: &[String],
    bindings: &HashMap<String, Ty>,
    span: Span,
) -> Result<Vec<Ty>> {
    let mut args = Vec::with_capacity(params.len());
    for p in params {
        match bindings.get(p) {
            Some(t) => args.push(t.clone()),
            None => {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "cannot infer type parameter '{p}' of '{name}' here - annotate the binding (e.g. 'val x: {name}<{}> = ...') or pass an argument that determines it",
                        params.join(", ")
                    ),
                    span,
                })
            }
        }
    }
    Ok(args)
}

/// The type a constructor argument is checked *against*, as far as the
/// bindings so far allow it (A-2). `Node(value = 1, next = Box.empty())`
/// needs the second argument to know it is a `Box<Node<i32>>`, and the only
/// place that says so is the field's written type - which can be resolved
/// once an earlier argument has pinned the parameters. `None` means "not
/// enough is known yet", and the argument is probed unconstrained.
pub(crate) fn field_hint(
    ctx: &Ctx,
    prefix: &str,
    aliases: &HashMap<String, String>,
    written: &ast::TypeExpr,
    bindings: &HashMap<String, Ty>,
) -> Option<Ty> {
    let scope = TypeScope {
        structs: &ctx.structs,
        enums: &ctx.enums,
        env: ctx.env.clone(),
        params: bindings,
        prefix,
        aliases,
        later_datacs: None,
        in_progress: None,
        consts: Some(&ctx.consts),
        const_fns: Some(&ctx.const_fns),
        handles: &ctx.env.handles,
    };
    // A generic declaration's `scoped fun(...)` parameter is still a top-level
    // parameter of a declaration, so the scoped type is legal here; the hint
    // is what lets a trailing block header take its parameter and return types
    // from it. `resolve` refuses the modifier everywhere because everywhere
    // else the value could be stored, so the hint goes through the same
    // parameter path a non-generic declaration's signature uses.
    if written.scoped {
        return scope.resolve_scoped_param(written, ast::ParamMode::Borrow).ok();
    }
    scope.resolve(written, false).ok()
}

/// `Pair(first = 1, second = "x")` - construction of a generic `datac` (A-2).
/// The type arguments are inferred from the argument types, falling back to
/// the expected type, so a generic value is built with the same syntax as an
/// ordinary one.
pub(crate) fn check_generic_datac_construction(
    tmpl_name: &str,
    explicit: &[Ty],
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
) -> Result<tir::Expr> {
    let tmpl = ctx.env.generics[tmpl_name].clone();
    let GenericKind::Record(fields) = &tmpl.kind else {
        let variants: Vec<String> = match &tmpl.kind {
            GenericKind::Sum(vs) => vs.iter().map(|v| format!("'{}.{}'", tmpl.name, v.name)).collect(),
            GenericKind::Record(_) => unreachable!(),
        };
        return Err(TypeError {
            help: None,
            message: format!(
                "'{}' is an enum - construct one of its variants: {}",
                tmpl.name,
                variants.join(", ")
            ),
            span,
        });
    };
    let names: Vec<String> = fields.iter().map(|f| f.name.clone()).collect();
    let slots = arg_slots(&format!("datac '{}'", tmpl.name), &names, args, span)?;
    // With an explicit instantiation there is nothing to infer; otherwise the
    // expected type is consulted first, because it is the only thing that can
    // pin a parameter *before* an argument is probed - which is what makes
    // `xs.push(Option.Some(value = 7))` infer `Option<i64>` from the list's
    // element type rather than defaulting the literal to `int`.
    let type_args = if explicit.is_empty() {
        let mut bindings = HashMap::new();
        bind_from_expected(&tmpl, expected, ctx, &mut bindings);
        for (i, f) in fields.iter().enumerate() {
            let hint = field_hint(ctx, &tmpl.prefix, &tmpl.aliases, &f.ty, &bindings);
            let probed = check_expr(&args[slots[i]].value, ctx, hint.as_ref())?;
            bind_type_params(&ctx.env, &tmpl.prefix, &tmpl.aliases, &f.ty, &probed.ty, &tmpl.params, &mut bindings);
            ctx.keep_probe(&args[slots[i]].value, probed);
        }
        finish_type_args(&tmpl.name, &tmpl.params, &bindings, span)?
    } else {
        check_arity(&tmpl.name, &tmpl.params, explicit, span)?
    };
    let ty = ctx.instantiate_type(tmpl_name, &type_args, span)?;
    let key = struct_name_of(&ty)
        .expect("an instantiation is a struct type")
        .to_string();
    let sdef = ctx.struct_def(&key).expect("just instantiated");
    let params: Vec<(String, Ty)> = sdef
        .fields
        .iter()
        .map(|f| (f.name.clone(), f.ty.clone()))
        .collect();
    let what = format!("datac '{}'", key);
    let shorthand = field_shorthand(&what, &params, args, ctx)?;
    let checked = match_args(&what, &params, &shorthand, span, ctx)?;
    Ok(tir::Expr {
        kind: tir::ExprKind::StructLit {
            name: key,
            fields: sdef
                .fields
                .iter()
                .map(|f| f.name.clone())
                .zip(checked)
                .collect(),
            borrowed_fields: Vec::new(),
        },
        ty: Ty::Struct(sdef.name.clone()),
        span,
    })
}

/// `Option.Some(v = 1)` - construction of one variant of a generic `enum`
/// (A-2), the enum counterpart of `check_generic_datac_construction`.
pub(crate) fn check_generic_enum_construction(
    tmpl_name: &str,
    variant: &str,
    explicit: &[Ty],
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
) -> Result<tir::Expr> {
    let tmpl = ctx.env.generics[tmpl_name].clone();
    let GenericKind::Sum(variants) = &tmpl.kind else {
        return Err(TypeError {
            help: None,
            message: format!(
                "'{}' is a generic datac - construct it as '{}(...)'",
                tmpl.name, tmpl.name
            ),
            span,
        });
    };
    let Some(vdef) = variants.iter().find(|v| v.name == variant) else {
        let names: Vec<String> = variants.iter().map(|v| format!("'{}'", v.name)).collect();
        return Err(TypeError {
            help: None,
            message: format!(
                "'{}.{}' is not a variant - the variants of '{}' are {}",
                tmpl.name,
                variant,
                tmpl.name,
                names.join(", ")
            ),
            span,
        });
    };
    let names: Vec<String> = vdef.fields.iter().map(|f| f.name.clone()).collect();
    let slots = arg_slots(
        &format!("variant '{}.{}'", tmpl.name, variant),
        &names,
        args,
        span,
    )?;
    let type_args = if explicit.is_empty() {
        let mut bindings = HashMap::new();
        bind_from_expected(&tmpl, expected, ctx, &mut bindings);
        for (i, f) in vdef.fields.iter().enumerate() {
            let hint = field_hint(ctx, &tmpl.prefix, &tmpl.aliases, &f.ty, &bindings);
            let probed = check_expr(&args[slots[i]].value, ctx, hint.as_ref())?;
            bind_type_params(&ctx.env, &tmpl.prefix, &tmpl.aliases, &f.ty, &probed.ty, &tmpl.params, &mut bindings);
            ctx.keep_probe(&args[slots[i]].value, probed);
        }
        finish_type_args(&tmpl.name, &tmpl.params, &bindings, span)?
    } else {
        check_arity(&tmpl.name, &tmpl.params, explicit, span)?
    };
    let ty = ctx.instantiate_type(tmpl_name, &type_args, span)?;
    let key = struct_name_of(&ty)
        .expect("an instantiation is a struct type")
        .to_string();
    let edef = ctx.enum_def(&key).expect("just instantiated");
    let Some((vidx, vdef)) = edef.variant(variant) else {
        unreachable!("the instance keeps the template's variants")
    };
    let params: Vec<(String, Ty)> = vdef
        .fields
        .iter()
        .map(|f| (f.name.clone(), f.ty.clone()))
        .collect();
    let what = format!("variant '{}.{}'", key, variant);
    let shorthand = field_shorthand(&what, &params, args, ctx)?;
    let checked = match_args(&what, &params, &shorthand, span, ctx)?;
    Ok(tir::Expr {
        kind: tir::ExprKind::EnumLit {
            enum_name: key.clone(),
            variant: vidx,
            fields: checked,
        },
        ty: Ty::Struct(key),
        span,
    })
}

/// `Option.None` - a payload-less variant of a generic `enum`, written without
/// a call (A-2). Nothing in the expression determines the type arguments, so
/// this form only checks when the expected type does.
pub(crate) fn check_generic_bare_variant(
    tmpl_name: &str,
    variant: &str,
    span: Span,
    ctx: &mut Ctx,
    expected: Option<&Ty>,
) -> Result<tir::Expr> {
    let tmpl = ctx.env.generics[tmpl_name].clone();
    let GenericKind::Sum(variants) = &tmpl.kind else {
        return Err(TypeError {
            help: None,
            message: format!(
                "'{}' is a generic datac - read its fields with '.' after constructing it",
                tmpl.name
            ),
            span,
        });
    };
    let Some(vdef) = variants.iter().find(|v| v.name == variant) else {
        let names: Vec<String> = variants.iter().map(|v| format!("'{}'", v.name)).collect();
        return Err(TypeError {
            help: None,
            message: format!(
                "'{}.{}' is not a variant - the variants of '{}' are {}",
                tmpl.name,
                variant,
                tmpl.name,
                names.join(", ")
            ),
            span,
        });
    };
    if !vdef.fields.is_empty() {
        let fields: Vec<String> = vdef.fields.iter().map(|f| f.name.clone()).collect();
        return Err(TypeError {
            help: None,
            message: format!(
                "variant '{}.{}' carries payload ({}) - construct it as '{}.{}({})'",
                tmpl.name,
                variant,
                fields.join(", "),
                tmpl.name,
                variant,
                fields.join(" = ..., ")
            ),
            span,
        });
    }
    let mut bindings = HashMap::new();
    bind_from_expected(&tmpl, expected, ctx, &mut bindings);
    let type_args = finish_type_args(&tmpl.name, &tmpl.params, &bindings, span)?;
    let ty = ctx.instantiate_type(tmpl_name, &type_args, span)?;
    let key = struct_name_of(&ty)
        .expect("an instantiation is a struct type")
        .to_string();
    let edef = ctx.enum_def(&key).expect("just instantiated");
    let Some((vidx, _)) = edef.variant(variant) else {
        unreachable!("the instance keeps the template's variants")
    };
    Ok(tir::Expr {
        kind: tir::ExprKind::EnumLit {
            enum_name: key.clone(),
            variant: vidx,
            fields: Vec::new(),
        },
        ty: Ty::Struct(key),
        span,
    })
}

/// The two-variant shapes `?` understands (MK5-S6-M1): `Ok`/`Err`, where the
/// failure carries a payload, and `Some`/`None`, where it does not. Recognized
/// by *variant name and arity*, never by the enum's own name, so a program's
/// own `Result`/`Option` works with no import and no blessed name - the shape
/// is the contract.
pub(crate) struct TryShape {
    pub(crate) ok: usize,
    pub(crate) err: usize,
    pub(crate) err_has_payload: bool,
}

pub(crate) fn try_shape(edef: &EnumDef) -> Option<TryShape> {
    if edef.variants.len() != 2 {
        return None;
    }
    let find = |n: &str| edef.variants.iter().position(|v| v.name == n);
    if let (Some(ok), Some(err)) = (find("Ok"), find("Err")) {
        if edef.variants[ok].fields.len() == 1 && edef.variants[err].fields.len() == 1 {
            return Some(TryShape {
                ok,
                err,
                err_has_payload: true,
            });
        }
        return None;
    }
    if let (Some(some), Some(none)) = (find("Some"), find("None")) {
        if edef.variants[some].fields.len() == 1 && edef.variants[none].fields.is_empty() {
            return Some(TryShape {
                ok: some,
                err: none,
                err_has_payload: false,
            });
        }
    }
    None
}

/// `expr?` (MK5-S6-M1). The operand has to be an `enum` shaped `Ok`/`Err` or
/// `Some`/`None`, held by value (a borrow cannot give its payload away), and
/// the enclosing function has to return an enum of the same shape - with the
/// *same* failure payload type for `Ok`/`Err`, because v1 has no error
/// conversion. What the operator evaluates to is the success payload.
/// The canonical name of the built-in `BlockError` enum.
pub fn block_error_name() -> String {
    format!("{}BlockError", crate::stdlib::key_prefix(crate::stdlib::BLOCK_MODULE))
}

/// The type of a block's failure half: `BlockError`.
pub fn block_error_ty() -> Ty {
    Ty::Struct(block_error_name())
}

/// The key of the stdlib `Result` that a block's type is an instance of. A
/// program that never imports `std/result` is told to.
pub(crate) fn block_result_key(ctx: &Ctx, span: Span) -> Result<String> {
    let key = format!("{}Result", crate::stdlib::key_prefix(crate::stdlib::RESULT_MODULE));
    if !ctx.env.generics.contains_key(&key) {
        return Err(TypeError {
            help: None,
            message: format!(
                "a block's result is a 'Result<T, BlockError>', which '{}' declares: add `impl \"{}\"` to the file that writes the block",
                key,
                crate::stdlib::RESULT_MODULE
            ),
            span,
        });
    }
    Ok(key)
}

/// Whether `ty` may be a block's element (decision 138 C, slice 1): a scalar,
/// or a `datac` whose fields are all scalars. A container or a box is refused.
pub(crate) fn block_elem_ok(ty: &Ty, ctx: &Ctx) -> bool {
    match ty {
        Ty::I32 | Ty::I64 | Ty::Int | Ty::U32 | Ty::U64 | Ty::Uint | Ty::F32 | Ty::F64 | Ty::Float | Ty::Bool | Ty::Char => true,
        Ty::U8 | Ty::U16 | Ty::Usize | Ty::Isize => true,
        Ty::Struct(name) => {
            !ctx.enums.contains_key(name)
                && ctx.structs.get(name).is_some_and(|def| {
                    !def.fields.is_empty() && def.fields.iter().all(|f| block_elem_ok(&f.ty, ctx))
                })
        }
        _ => false,
    }
}

/// Where a bare block-box name is about to go (decision 139, step 4). It only
/// words the refusal: each sink has its own text in the design (section 14.3).
#[derive(Clone, Debug)]
pub(crate) enum BoxSink {
    /// A value taken out of the block: a `let`, or an argument, named here.
    Move(String),
    /// A store into a place declared outside the block, named here.
    Store(String),
    /// A store into a shared container (a `List`, `Map` or `Box` payload),
    /// named by its receiver: the container outlives the block.
    Shared(String),
    /// The value of the block itself.
    Tail,
}

/// The refusal for a block box used outside its field reads. `sink` says where
/// the bare name was going, when the checker knew.
pub(crate) fn box_move_error(name: &str, block: &str, sink: Option<&BoxSink>, span: Span) -> TypeError {
    match sink {
        Some(BoxSink::Store(target)) => TypeError {
            help: Some(format!("'{target}' lives longer than '{block}'; store the number you need from '{name}', or declare '{target}' inside '{block}'")),
            message: format!("block value '{name}' cannot be stored in '{target}', which is declared outside block '{block}'"),
            span,
        },
        Some(BoxSink::Shared(target)) => TypeError {
            help: Some(format!("store the number you need from '{name}', or keep '{target}' inside '{block}'")),
            message: format!("block value '{name}' cannot be stored in '{target}', because '{target}' is shared-pool memory that can outlive block '{block}'"),
            span,
        },
        Some(BoxSink::Tail) => TypeError {
            help: Some(format!("'{block}' ends at its closing brace; return a number or a bool from '{name}', or do the work that needs '{name}' inside '{block}'")),
            message: format!("block value '{name}' cannot leave block '{block}' as the value of its block"),
            span,
        },
        Some(BoxSink::Move(dest)) => box_move_into(name, block, dest, span),
        None => box_move_into(name, block, &format!("a place outside '{block}'"), span),
    }
}

pub(crate) fn box_move_into(name: &str, block: &str, dest: &str, span: Span) -> TypeError {
    TypeError {
        help: Some(format!("copy the number you need, or borrow '{name}' for a call whose result holds no reference")),
        message: format!("block value '{name}' cannot be moved out of block '{block}' into {dest}"),
        span,
    }
}

/// The refusal for a block box that a `fun` literal names: the literal would
/// keep the value in its environment, which is shared-pool memory (`fun`), a
/// stack frame that must not outlive the block (`scoped fun`), or a thread
/// that can run after the block ends (a spawn).
pub(crate) fn box_capture_error(name: &str, block: &str, ctx: &Ctx, span: Span) -> TypeError {
    if ctx.spawn_entry {
        TypeError {
            help: Some(format!("a thread can run after '{block}' ends; create the value inside the thread, or pass a number")),
            message: format!("block value '{name}' cannot be passed to a spawned thread"),
            span,
        }
    } else if ctx.borrowed_captures {
        TypeError {
            help: Some(format!("move the scoped block inside '{block}', or pass the number you need into it")),
            message: format!("scoped block captures block value '{name}', which would leave block '{block}'"),
            span,
        }
    } else {
        TypeError {
            help: Some(format!("pass '{name}' to the closure as a parameter, or create the closure outside '{block}'")),
            message: format!("this closure captures block value '{name}', so its environment would be shared-pool memory"),
            span,
        }
    }
}
