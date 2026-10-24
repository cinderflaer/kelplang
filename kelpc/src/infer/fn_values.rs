use super::*;

/// One user-declared key-pair half, kept while the signatures are built so a
/// type that declares only one of the two can be named with the span of the
/// half it did declare.
pub(crate) struct KeyPairHalf {
    pub(crate) method: &'static str,
    pub(crate) span: Span,
    pub(crate) module: String,
}

/// Whether one half of a declared `Map` key pair is a shape the map can call
/// (A-12), with `params` the resolved parameter list (the receiver first) and
/// `ret` the resolved return type.
///
/// The borrow is checked as *written* as well as resolved: an owned `other: T`
/// and a `other: &T` resolve to the same `Ty::Ref` (a `datac` parameter is
/// borrowed either way), and the map passes the caller's key by reference, so a
/// parameter that reads as an ownership transfer is refused rather than
/// silently accepted.
pub(crate) fn check_key_pair_method(
    f: &ast::FunctionDecl,
    params: &[(String, Ty)],
    ret: &Ty,
) -> std::result::Result<(), Diagnostic> {
    let receiver = f
        .receiver
        .as_ref()
        .expect("only a receiver method is a key-pair half");
    let owner = &receiver.type_name;
    let err = |message: String| Diagnostic::new(message, f.span, f.module.clone());
    if f.name == crate::types::KEY_PAIR_HASH_METHOD {
        if !f.params.is_empty() {
            return Err(err(format!(
                "'{owner}.hash' takes no parameters: a key's hash is computed from the key alone, so write 'fun {owner}.hash() => u64'"
            )));
        }
        if *ret != Ty::U64 {
            return Err(err(format!(
                "'{owner}.hash' must return 'u64', found '{}': the `Map` probe folds the hash of every key through 'kelp_map_combine'",
                ret.display()
            )));
        }
        return Ok(());
    }
    if f.params.len() != 1 {
        return Err(err(format!(
            "'{owner}.eq' takes exactly one parameter, a shared borrow of '{owner}', so write 'fun {owner}.eq(other: &{owner}) => bool'"
        )));
    }
    let written = &f.params[0];
    if !written.ty.amp || written.mode != ast::ParamMode::Borrow {
        return Err(err(format!(
            "'{owner}.eq' must take 'other: &{owner}': a lookup compares the caller's key against an entry the caller does not own, so the argument cannot be moved"
        )));
    }
    let same = matches!(
        params.get(1).map(|(_, ty)| ty),
        Some(Ty::Ref { mutable: false, inner }) if **inner == Ty::Struct(owner.clone())
    );
    if !same {
        return Err(err(format!(
            "'{owner}.eq' takes a shared borrow of '{owner}', found '{}'",
            params[1].1.display()
        )));
    }
    if *ret != Ty::Bool {
        return Err(err(format!(
            "'{owner}.eq' must return 'bool', found '{}'",
            ret.display()
        )));
    }
    Ok(())
}

/// The `Ty::Fn` a declared signature has (A-4): its parameter types, in order,
/// and its return type. A function *type* spells exactly this, so a written
/// `fun(String) => usize` and the signature of `fun f(s: String) => usize` are
/// the same type with no conversion anywhere.
pub(crate) fn fn_value_ty(sig: &FnSig) -> Ty {
    Ty::Fn {
        params: sig.params.iter().map(|(_, t)| t.clone()).collect(),
        ret: Box::new(sig.ret.clone()),
    }
}

/// The canonical key and function type of a *bare* name used as a value
/// (A-4), or `None` when the name is not a function at all (so the caller can
/// report whatever else it might have been).
///
/// The lookup order is a call's: the calling module's own spelling first, then
/// the name as written. Three names that *are* functions are refused rather
/// than silently dropped, because "undefined variable" would be a lie:
/// a generic template (its signature is not concrete until a call
/// instantiates it), `main` (the entry point cannot be called), and a function
/// that reads a passdown (the hidden argument it receives is not part of a C
/// function pointer).
pub(crate) fn function_value(name: &str, ctx: &Ctx, span: Span) -> Result<Option<(String, Ty)>> {
    let owned = ctx.qualify_own(name);
    // A generic template has no concrete signature yet, so it is never in
    // `sigs`; its refusal is `function_value_at`'s, and the key it looks under
    // is still the calling module's own.
    if ctx.sig_of(&owned).is_some() || ctx.fns.templates.contains_key(&owned) {
        return function_value_at(&owned, name, ctx, span);
    }
    // Same rule as the signature lookup in `check_call`: only a name that
    // already carries its module prefix may fall back to the name as written,
    // because that is the only spelling of another module's key. A bare name
    // is the calling module's own, and the entry module is the one whose keys
    // are bare (A-13).
    if !name.contains('.') {
        return Ok(None);
    }
    function_value_at(name, name, ctx, span)
}

/// `function_value` for an already-canonical key, with `written` as the name
/// to show in a diagnostic (a bare name, or `math.sqrt`).
pub(crate) fn function_value_at(
    key: &str,
    written: &str,
    ctx: &Ctx,
    span: Span,
) -> Result<Option<(String, Ty)>> {
    if let Some(sig) = ctx.sig_of(key) {
        if key == "main" {
            return Err(TypeError {
                help: None,
                message: "cannot reference 'main': it is the program entry point".to_string(),
                span,
            });
        }
        if ctx.wired_for(key).is_some_and(|w| !w.is_empty()) {
            return Err(TypeError {
                help: None,
                message: format!(
                    "'{written}' reads a passdown, so it cannot be a function value - a passdown is passed to the callee as a hidden argument, and a function value is a plain C function pointer; call it directly instead"
                ),
                span,
            });
        }
        // A gated item named as a value is a *name* like any other, so the
        // domain half of the gate applies here exactly as it does at a call
        // site: without the capability the name does not resolve at all.
        ctx.check_capability(key, span)?;
        // Holding the domain is not enough. The pattern half of the gate is
        // emitted from the *call's* own callee key (the removed C backend's `cap_plan`),
        // and a call through a function value names the pointer, not the
        // gated item: the check would be skipped and the pointer would reach
        // the resource on the domain gate's say-so alone, which makes a
        // narrowed grant advisory for this one shape. A value cannot carry
        // the naming module's patterns, and the resource argument position is
        // a property of the callee's signature, so no check can follow the
        // pointer. Refuse the value, the way the domain half refuses the
        // ungranted name.
        if let Some(domain) = ctx.gated.get(key) {
            return Err(TypeError {
                help: None,
                message: format!(
                    "'{written}' is a gated function ('{domain}' capability), so it cannot be a function value: a call through the pointer is not a call site, and the '{domain}' check on its resource argument cannot be emitted there; call it directly instead"
                ),
                span,
            });
        }
        return Ok(Some((key.to_string(), fn_value_ty(&sig))));
    }
    if ctx.fns.templates.contains_key(key) {
        return Err(TypeError {
            help: None,
            message: format!(
                "'{written}' is a generic function - a function value needs one concrete signature, so wrap the call in a small non-generic function and pass that"
            ),
            span,
        });
    }
    Ok(None)
}

/// The callee *expression* and type of a name that holds a function value
/// (A-4): a local, or a passdown. `Ok(None)` when the name holds no function
/// value at all, so the caller can report it as an undefined function.
pub(crate) fn indirect_callee(
    name: &str,
    span: Span,
    ctx: &mut Ctx,
) -> Result<Option<(tir::Expr, Ty)>> {
    // A parameter of a closure type is a *borrow* of one (the parameter rule
    // borrows an owned type), so the callee's type may be `&closure fun(..)` -
    // codegen dereferences it. A `fun` is `Copy`, so it is never borrowed.
    let callable = |ty: &Ty| -> Option<Ty> {
        match ty {
            Ty::Fn { .. } => Some(ty.clone()),
            Ty::Closure { .. } => Some(ty.clone()),
            // A scoped function value is `Copy`, so a parameter of one is the
            // fat pointer itself (no borrow is added by the parameter rule).
            Ty::ScopedFn { .. } => Some(ty.clone()),
            Ty::Ref { inner, .. } if matches!(**inner, Ty::Closure { .. }) => Some(ty.clone()),
            _ => None,
        }
    };
    if let Some(info) = ctx.lookup(name) {
        if let Some(ty) = callable(&info.ty) {
            return Ok(Some((
                tir::Expr {
                    kind: tir::ExprKind::Ident(name.to_string()),
                    ty: ty.clone(),
                    span,
                },
                ty,
            )));
        }
        return Ok(None);
    }
    if ctx.in_fn_literal {
        // A closure the enclosing frame holds, called from inside this
        // literal (A-4 phase E): the literal captures it like any other
        // binding, and the call goes through the capture. The capture is
        // registered here, which is why this takes `&mut Ctx`.
        if let Some(ty) = ctx.note_capture(name, span) {
            // A scoped literal captures an owned closure by shared borrow, so
            // a callable capture may arrive as `&closure fun(...)` as well.
            let callable = matches!(ty, Ty::Fn { .. } | Ty::Closure { .. } | Ty::ScopedFn { .. })
                || matches!(&ty, Ty::Ref { inner, .. } if matches!(**inner, Ty::Fn { .. } | Ty::Closure { .. } | Ty::ScopedFn { .. }));
            if callable {
                let index = ctx
                    .captures
                    .iter()
                    .position(|(n, _)| n == name)
                    .expect("note_capture records the name");
                return Ok(Some((
                    tir::Expr {
                        kind: tir::ExprKind::Capture { index },
                        ty: ty.clone(),
                        span,
                    },
                    ty,
                )));
            }
        }
    }
    // A passdown holding a function value is the same thing one module up:
    // its storage is the generated global (or the hidden parameter), so the
    // callee expression is whatever a read of that passdown already is.
    if let Some(pd) = ctx.pds.get(name) {
        let Some(ty) = callable(&pd.value_ty) else {
            return Ok(None);
        };
        if !ctx.pd_initialized(&pd.hidden) {
            return Err(TypeError {
                help: None,
                message: format!(
                    "passdown '{}' is read before it is initialized - a 'pd' initializer can only use passdowns declared before it",
                    pd.qualified
                ),
                span,
            });
        }
        return Ok(Some((
            pd.value(span, ctx.pd_global_borrow, ctx.pd_exclusive(&pd)),
            ty,
        )));
    }
    Ok(None)
}

/// `f(x)` where `f` holds a function value (A-4): a local, a passdown, or a
/// `datac` field (`cfg.on_event(x)`). A function *type* has no parameter
/// names, so arguments are positional and counted against the callee's own
/// list; everything else - the borrow an owned parameter implies, the move a
/// `move` parameter implies - is the rule `check_call_arg` already applies to
/// a declared call, because the parameter types are the same types.
pub(crate) fn check_indirect_call_expr(
    callee: tir::Expr,
    fn_ty: &Ty,
    args: &[ast::CallArg],
    span: Span,
    ctx: &mut Ctx,
    what: &str,
) -> Result<tir::Expr> {
    // A closure parameter is a borrow of a closure, and a borrow is not a
    // value: look through it for the signature (A-4 phase E). The callee
    // expression keeps its borrowed type, which is what codegen uses to pick
    // `->` over `.`.
    let ty = match fn_ty {
        Ty::Ref { inner, .. } => inner.as_ref(),
        other => other,
    };
    let (params, ret) = match ty {
        Ty::Fn { params, ret } => (params, ret),
        Ty::Closure { params, ret } => (params, ret),
        // A scoped function value is called exactly as a closure is: the
        // fat pointer's code member with the environment first.
        Ty::ScopedFn { params, ret } => (params, ret),
        other => unreachable!(
            "an indirect call is only reached with a function type, found '{}'",
            other.display()
        ),
    };
    if args.len() != params.len() {
        return Err(TypeError {
            help: None,
            message: format!(
                "a call through {what} takes {} argument(s), found {}",
                params.len(),
                args.len()
            ),
            span,
        });
    }
    for a in args {
        if a.name.is_some() {
            return Err(TypeError {
                help: None,
                message: format!(
                    "a call through {what} cannot use named arguments - a function type has no parameter names, so every argument is positional"
                ),
                span: a.value.span(),
            });
        }
    }
    let mut checked = Vec::with_capacity(args.len());
    for (i, (pty, a)) in params.iter().zip(args).enumerate() {
        checked.push(check_call_arg(&(i + 1).to_string(), pty, &a.value, ctx)?);
    }
    Ok(tir::Expr {
        kind: tir::ExprKind::IndirectCall {
            callee: Box::new(callee),
            args: checked,
        },
        ty: (**ret).clone(),
        span,
    })
}

/// The type of a `datac` field read through a (possibly borrowed) receiver -
/// `None` when there is no such field. Used only to find a field holding a
/// function value, which is callable with method syntax.
pub(crate) fn fn_field_ty(ty: &Ty, name: &str, ctx: &Ctx) -> Option<Ty> {
    let owner = struct_name_of(ty)?.to_string();
    let def = ctx.struct_def(&owner)?;
    Some(def.field(name)?.ty.clone())
}

/// A function's lookup key in `sigs`/`tir::Function.name`: a receiver-form
/// method is keyed `Type#name` (a receiver method and a free function can
/// share a bare name with no collision, and `#` can't appear in a real
/// identifier so it can't collide with a hand-written name either) - a
/// plain function is keyed by its bare name, unchanged from before
/// receivers existed.
pub(crate) fn sig_key(f: &ast::FunctionDecl) -> String {
    match &f.receiver {
        Some(r) => format!("{}#{}", r.type_name, f.name),
        // A static member of a trait `impl` (A-12 slice 6) carries its trait
        // instance, so `From<IoErr>`'s `from` and `From<String>`'s `from` for
        // the same type are two functions rather than a collision.
        None => match &f.trait_impl {
            Some(site) if !site.trait_instance.is_empty() => {
                format!("{}#{}@{}", site.self_type, f.name, site.trait_instance)
            }
            _ => f.name.clone(),
        },
    }
}

pub(crate) const PRINTABLE_BUILTINS: &[&str] = &["print", "print!"];

/// Function names that may be declared more than once, once per parameter
/// type, in a compiler-shipped module (M6.3, stdlib decision 4). The call
/// site picks by argument type; user code cannot define overloads at all,
/// and cannot see the mechanism except as `math.abs` working on whatever
/// number it is given.
/// The overloaded `std/math` names that stayed usable as a function value when
/// they gained an `f32` overload: `abs`, `min`, `max` and `clamp` were never one.
pub(crate) const FLOAT_MATH_VALUES: &[&str] = &[
    "sqrt", "pow", "floor", "ceil", "round", "trunc", "exp", "ln", "log2", "log10", "sin", "cos", "tan", "atan2",
    "is_nan", "is_finite", "sign",
];

pub(crate) const OVERLOADABLE: &[&str] = &[
    "abs", "min", "max", "clamp", "sqrt", "pow", "floor", "ceil", "round", "trunc", "exp", "ln", "log2", "log10", "sin",
    "cos", "tan", "atan2", "is_nan", "is_finite", "sign",
];

/// Whether a module id is one the compiler ships (M6.3, stdlib decision 4).
/// The `std/` prefix is deliberately *not* the test: a project file at
/// `src/std/x.kp` is loaded from disk and prefixed like any other module, so
/// an id that only looks shipped would take the exemptions the compiler's own
/// text earns: the export-everything rule among them, which would silently
/// switch privacy off there. `stdlib::source` is the table of shipped text,
/// and `std/rt` (the injected runtime) is in it for the same reason.
pub(crate) fn is_shipped_module(module: &str) -> bool {
    crate::stdlib::source(module).is_some()
}

/// The canonical keys of the items each module exports (A-13, `design/syntax.md (kelp wiki)`
/// 13.4). Private by default, so a key is present only for a declaration
/// written `pub`. A key is a declaration's own lookup name, which is exactly
/// what a cross-module lookup is keyed by.
///
/// A compiler-shipped module exports everything. That is the transitional
/// rule, not a convenience: shipped modules import each other (`std/fmt` ->
/// `std/str`, `std/net` -> `std/io` + `std/str`), so without it the standard
/// library stops building, and marking every shipped declaration `pub` is a
/// sweep this slice does not need.
///
/// The names are read from the *merged* program, where `resolve.rs` has
/// already prefixed every non-entry declaration with its module path, so a
/// declaration's own `name` is its canonical key as it stands.
pub(crate) fn collect_exports(program: &ast::Program) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    let mut export = |module: &str, name: &str, is_pub: bool| {
        if is_pub || is_shipped_module(module) {
            out.insert(name.to_string());
        }
    };
    for d in &program.datacs {
        export(&d.module, &d.name, d.is_pub);
    }
    for e in &program.enums {
        export(&e.module, &e.name, e.is_pub);
    }
    for t in &program.extern_types {
        export(&t.module, &t.name, t.is_pub);
    }
    for t in &program.traits {
        export(&t.module, &t.name, t.is_pub);
    }
    for c in &program.consts {
        export(&c.module, &c.name, c.is_pub);
    }
    for s in &program.statics {
        export(&s.module, &s.name, s.is_pub);
    }
    for f in &program.functions {
        // A method's reachability follows its declaring type, and the parser
        // refuses `pub` on one, so only a free function contributes a key: a
        // method is reached through a value, never through a module alias.
        if f.receiver.is_none() {
            export(&f.module, &f.name, f.is_pub);
        }
    }
    out
}

/// Whether the module that owns `key` exports it (A-13).
pub(crate) fn exported(key: &str, env: &TypeEnv) -> bool {
    env.exported.contains(key)
}

/// The refusal a cross-module reference to an existing but unexported item
/// gets (A-13). It is deliberately apart from the "has no item" message: the
/// item exists, and its own module is the reason it is out of reach.
pub(crate) fn private_item_error(alias: &str, item: &str, span: Span) -> TypeError {
    TypeError {
        help: None,
        message: format!(
            "'{alias}.{item}' is private: '{item}' is declared in module '{alias}' without 'pub', so only that module can name it; add 'pub' to its declaration to export it"
        ),
        span,
    }
}

/// Whether a canonical lookup key names some declared item in the merged
/// program. This is the existence half of the visibility gate, so a reference
/// to an item that does not exist still reads as "has no item" rather than
/// as "private".
pub(crate) fn declares(key: &str, ctx: &Ctx) -> bool {
    ctx.structs.contains_key(key)
        || ctx.enums.contains_key(key)
        || ctx.env.handles.contains(key)
        || ctx.consts.contains_key(key)
        || ctx.statics.contains_key(key)
        || ctx.sigs.contains_key(key)
        || ctx.env.generics.contains_key(key)
        || ctx.fns.templates.contains_key(key)
        || ctx.overloads.contains_key(key)
}

/// The visibility gate for a type name written through an alias
/// (`alias.Type`, A-13). `Ok(())` when the name is not alias-qualified, when
/// the alias is unknown, or when the name is no type at all, so the ordinary
/// "unknown type" path still reports those.
pub(crate) fn check_type_exported(
    name: &str,
    structs: &HashMap<String, StructDef>,
    enums: &HashMap<String, EnumDef>,
    aliases: &HashMap<String, String>,
    env: &TypeEnv,
    span: Span,
) -> Result<()> {
    let Some((alias, item)) = name.split_once('.') else {
        return Ok(());
    };
    let Some(prefix) = aliases.get(alias) else {
        return Ok(());
    };
    let key = format!("{prefix}{item}");
    let known = structs.contains_key(&key)
        || enums.contains_key(&key)
        || env.generics.contains_key(&key)
        || env.handles.contains(&key);
    if known && !exported(&key, env) {
        return Err(private_item_error(alias, item, span));
    }
    Ok(())
}

/// The names an `extern type` may take (gap 1). A handle is a type name like
/// any other, so it must be unique in the program and must not collide with a
/// `datac`, an `enum` or a builtin: `Ty::Handle` and `Ty::Struct` are separate
/// variants, and a collision would make one of the two silently unreachable
/// rather than a diagnostic.
pub(crate) fn check_extern_type_names(program: &ast::Program) -> std::result::Result<(), Diagnostic> {
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for t in &program.extern_types {
        if !seen.insert(t.name.as_str()) {
            return Err(Diagnostic::new(
                format!("extern type '{}' is declared more than once", t.name),
                t.span,
                t.module.clone(),
            ));
        }
        if program.datacs.iter().any(|d| d.name == t.name)
            || program.enums.iter().any(|e| e.name == t.name)
        {
            return Err(Diagnostic::new(
                format!(
                    "'{}' is used for both an 'extern type' and a datac or enum",
                    t.name
                ),
                t.span,
                t.module.clone(),
            ));
        }
        if Ty::from_name(&t.name).is_some() {
            return Err(Diagnostic::new(
                format!(
                    "'{}' is a builtin type and cannot be redeclared as an 'extern type'",
                    t.name
                ),
                t.span,
                t.module.clone(),
            ));
        }
    }
    Ok(())
}

/// The parent relation of the program's `extern type`s (gap 1): a canonical
/// child name -> its canonical parent name. The written parent is qualified
/// with its declaring module's prefix and alias table, exactly as a field's
/// type name is, so `extern type Window: Widget` in one module and
/// `extern type Window: widgets.Widget` in another name the same type.
///
/// A parent must be another `extern type`, and the chain must be acyclic: a
/// value converts implicitly to *any* ancestor, and a cycle would make the
/// relation meaningless rather than merely deep. `Ok` on a program with no
/// parented handles, so the map is empty for every program that predates it.
pub(crate) fn build_handle_parents(
    program: &ast::Program,
    project: &Project,
) -> std::result::Result<HashMap<String, String>, Diagnostic> {
    let handles: std::collections::HashSet<&str> =
        program.extern_types.iter().map(|t| t.name.as_str()).collect();
    let mut parents: HashMap<String, String> = HashMap::new();
    for t in &program.extern_types {
        let Some(written) = &t.parent else {
            continue;
        };
        let prefix = project.prefix_of(&t.module);
        let aliases = project.alias_prefixes_of(&t.module);
        let key = qualify_written(written, prefix, &aliases);
        if !handles.contains(key.as_str()) {
            return Err(Diagnostic::new(
                format!(
                    "'extern type {}' names parent '{written}', which is not an 'extern type': a handle's parent must be another handle declared earlier in this module or imported",
                    bare_name(&t.name, prefix)
                ),
                t.span,
                t.module.clone(),
            ));
        }
        parents.insert(t.name.clone(), key);
    }
    for t in &program.extern_types {
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let mut cur = t.name.as_str();
        loop {
            if !seen.insert(cur) {
                let prefix = project.prefix_of(&t.module);
                return Err(Diagnostic::new(
                    format!(
                        "'extern type {}' has a cycle in its parent chain; a handle may only convert up to an ancestor",
                        bare_name(&t.name, prefix)
                    ),
                    t.span,
                    t.module.clone(),
                ));
            }
            match parents.get(cur) {
                Some(p) => cur = p.as_str(),
                None => break,
            }
        }
    }
    Ok(parents)
}

/// Whether `ancestor` is `descendant` itself or one of its parent-chain
/// ancestors. This is the direction the implicit conversion runs: a child
/// converts to an ancestor, never the other way.
pub(crate) fn handle_is_same_or_ancestor(
    parents: &HashMap<String, String>,
    ancestor: &str,
    descendant: &str,
) -> bool {
    let mut cur = descendant;
    for _ in 0..=parents.len() {
        if cur == ancestor {
            return true;
        }
        match parents.get(cur) {
            Some(p) => cur = p.as_str(),
            None => return false,
        }
    }
    false
}
