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
    // borrows an owned type), so the callee's type may be `&closure(..)` -
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
            // a callable capture may arrive as `&closure(...)` as well.
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

/// The private-in-public rule (A-13, `design/syntax.md (kelp wiki)` 13.4): a declaration
/// another module can reach must not name, anywhere in its signature, a type
/// its module does not export. A private declaration may name anything,
/// because nothing outside its module can name it either.
///
/// The check runs once over the *merged* program with the whole export set
/// already in hand (`collect_exports`), rather than per item while the
/// declarations are collected, so a private type declared after the item that
/// names it is caught the same way as one declared before: the order of two
/// declarations in one file does not decide the answer.
pub(crate) fn check_pub_signatures(
    program: &ast::Program,
    project: &Project,
) -> std::result::Result<(), Diagnostic> {
    let exported = collect_exports(program);
    let declared: std::collections::HashSet<&str> = program
        .datacs
        .iter()
        .map(|d| d.name.as_str())
        .chain(program.enums.iter().map(|e| e.name.as_str()))
        .chain(program.extern_types.iter().map(|t| t.name.as_str()))
        .collect();
    for d in &program.datacs {
        if !exported.contains(&d.name) {
            continue;
        }
        SignatureScope::new(project, &d.module, &d.name, &d.name, &d.type_params, &exported, &declared)
            .check_fields(&d.fields)
            .map_err(|e| e.in_module(&d.module))?;
    }
    for e in &program.enums {
        if !exported.contains(&e.name) {
            continue;
        }
        let scope =
            SignatureScope::new(project, &e.module, &e.name, &e.name, &e.type_params, &exported, &declared);
        for v in &e.variants {
            scope.check_fields(&v.fields).map_err(|err| err.in_module(&e.module))?;
        }
    }
    for c in &program.consts {
        if !exported.contains(&c.name) {
            continue;
        }
        SignatureScope::new(project, &c.module, &c.name, &c.name, &[], &exported, &declared)
            .check_type(&c.ty)
            .map_err(|e| e.in_module(&c.module))?;
    }
    for s in &program.statics {
        if !exported.contains(&s.name) {
            continue;
        }
        SignatureScope::new(project, &s.module, &s.name, &s.name, &[], &exported, &declared)
            .check_type(&s.ty)
            .map_err(|e| e.in_module(&s.module))?;
    }
    for f in &program.functions {
        // A method is reached through a value of its declaring type, never
        // through a module alias (the parser refuses `pub` on one), so it is
        // as reachable as that type is, and the type is the declaration a
        // reader would make private instead.
        let (reachable, item, fix, type_params) = match &f.receiver {
            Some(r) => (
                exported.contains(&r.type_name),
                format!("{}.{}", r.type_name, f.name),
                r.type_name.clone(),
                &r.type_args,
            ),
            None => (
                exported.contains(&f.name),
                f.name.clone(),
                f.name.clone(),
                &f.type_params,
            ),
        };
        if !reachable {
            continue;
        }
        SignatureScope::new(project, &f.module, &item, &fix, type_params, &exported, &declared)
            .check_function(f)
            .map_err(|e| e.in_module(&f.module))?;
    }
    Ok(())
}

/// One reachable declaration's signature, checked against the export set. The
/// written type names are canonicalized exactly as resolution canonicalizes
/// them (`qualify_written`), so a bare name belongs to the declaring module
/// and an `alias.Item` spelling to the alias target, one naming scheme and
/// no second.
pub(crate) struct SignatureScope<'a> {
    pub(crate) project: &'a Project,
    pub(crate) module: &'a str,
    pub(crate) prefix: &'a str,
    pub(crate) aliases: HashMap<String, String>,
    /// The public item, as a reader writes it, for the refusal.
    pub(crate) item: &'a str,
    /// The declaration to make private instead: the item itself, or a
    /// method's declaring type.
    pub(crate) fix: &'a str,
    /// The type parameters in scope. A parameter is not a declared type, so
    /// naming one is never a violation, exactly as naming a builtin is not.
    pub(crate) type_params: Vec<&'a str>,
    pub(crate) exported: &'a std::collections::HashSet<String>,
    pub(crate) declared: &'a std::collections::HashSet<&'a str>,
}

/// The name a callee key shows in a diagnostic: overload keys carry their
/// parameter types (`std.math.abs@f64`), which read as noise in an error
/// message about arguments.
pub(crate) fn display_callee(key: &str) -> &str {
    key.split('@').next().unwrap_or(key)
}

/// A function's signature key. An overloadable name in a stdlib module
/// carries its parameter types, which is what lets several declarations of
/// `abs` coexist in `std/math.kp` and be told apart at a call site.
pub(crate) fn sig_key_for(f: &ast::FunctionDecl, prefix: &str, params: &[(String, Ty)]) -> String {
    let plain = sig_key(f);
    if f.receiver.is_none()
        && is_shipped_module(&f.module)
        && OVERLOADABLE.contains(&bare_name(&f.name, prefix))
    {
        let types: Vec<String> = params.iter().map(|(_, t)| t.display()).collect();
        format!("{plain}@{}", types.join(","))
    } else {
        plain
    }
}

/// A compiler-provided function (M6). These live in `sigs`' namespace (keyed
/// `Type#name`, exactly like a receiver method) but their bodies are emitted
/// by the removed C backend from the fixed runtime preamble, so nothing about call
/// resolution, argument matching, or passdown wiring needs to know they are
/// special - only codegen, and only in that the C function it names already
/// exists.
pub(crate) struct Builtin {
    pub(crate) params: Vec<(String, Ty)>,
    pub(crate) ret: Ty,
    /// `Bytes.empty()` - an *associated* function written on the type name,
    /// with no receiver argument - versus `b.len()`, which takes one.
    pub(crate) associated: bool,
}

/// The `Bytes` surface (M6). Kept deliberately small: construct, measure,
/// index, mutate, copy. Everything else about `Bytes` needs either `String`
/// or growable buffers, which are later milestones.
///
/// `List<T>` (collections milestone) is deliberately *not* in this static
/// table: its signatures depend on the element type, so they are built on
/// demand by `list_sig` below, keyed by the same `List#name` spelling.
pub(crate) static BUILTINS: std::sync::LazyLock<HashMap<String, Builtin>> =
    std::sync::LazyLock::new(|| {
        let bytes = Ty::Bytes;
        let shared = Ty::Ref { mutable: false, inner: Box::new(bytes.clone()) };
        let exclusive = Ty::Ref { mutable: true, inner: Box::new(bytes.clone()) };
        let mut m = HashMap::new();
        let mut add = |key: &str, params: Vec<(String, Ty)>, ret: Ty, associated: bool| {
            m.insert(key.to_string(), Builtin { params, ret, associated });
        };
        add("Bytes#empty", vec![], bytes.clone(), true);
        add("Bytes#from_str", vec![("s".to_string(), Ty::Str)], bytes.clone(), true);
        add("Bytes#len", vec![("self".to_string(), shared.clone())], Ty::Usize, false);
        add("Bytes#is_empty", vec![("self".to_string(), shared.clone())], Ty::Bool, false);
        add(
            "Bytes#get",
            vec![("self".to_string(), shared.clone()), ("i".to_string(), Ty::Usize)],
            Ty::U8,
            false,
        );
        add(
            "Bytes#set",
            vec![
                ("self".to_string(), exclusive),
                ("i".to_string(), Ty::Usize),
                ("b".to_string(), Ty::U8),
            ],
            Ty::Unit,
            false,
        );
        add("Bytes#clone", vec![("self".to_string(), shared)], bytes, false);

        // Growing a `Bytes` in place (S1). `push`/`append` need an exclusive
        // borrow, so the borrow checker refuses to grow a buffer while a
        // `str`/`&Bytes` view of it is alive - which is what makes the
        // reallocation sound without a lifetime syntax (see the bump of
        // `cap` in the runtime preamble).
        let bytes_exclusive = Ty::Ref { mutable: true, inner: Box::new(Ty::Bytes) };
        add(
            "Bytes#push",
            vec![("self".to_string(), bytes_exclusive.clone()), ("b".to_string(), Ty::U8)],
            Ty::Unit,
            false,
        );
        add(
            "Bytes#append",
            vec![
                ("self".to_string(), bytes_exclusive.clone()),
                ("other".to_string(), Ty::Ref { mutable: false, inner: Box::new(Ty::Bytes) }),
            ],
            Ty::Unit,
            false,
        );
        add(
            "Bytes#reserve",
            vec![("self".to_string(), bytes_exclusive.clone()), ("extra".to_string(), Ty::Usize)],
            Ty::Unit,
            false,
        );
        add("Bytes#clear", vec![("self".to_string(), bytes_exclusive)], Ty::Unit, false);

        // Byte-order reads and writes on a byte buffer (kernel stage 3,
        // MK9-S8): `b.read_u16_le(offset)` and its siblings, one name per
        // width and byte order, each bounds-checked and with no alignment
        // requirement. The receiver is `&Bytes` for a read and `mut Bytes` for
        // a write, so a write goes through the same exclusive borrow `push`
        // takes; every offset is a `usize`. `u8` needs no reader (`Bytes.get`
        // is one), `i16` does not exist, and `usize`/`isize` are absent because
        // a wire format fixes its field widths rather than the host's pointer
        // width.
        let bytes_shared = Ty::Ref { mutable: false, inner: Box::new(Ty::Bytes) };
        let bytes_mut = Ty::Ref { mutable: true, inner: Box::new(Ty::Bytes) };
        for value in ["u16", "u32", "u64", "i32", "i64"] {
            let ty = Ty::from_name(value).expect("integer type");
            for order in ["le", "be"] {
                add(
                    &format!("Bytes#read_{value}_{order}"),
                    vec![
                        ("self".to_string(), bytes_shared.clone()),
                        ("offset".to_string(), Ty::Usize),
                    ],
                    ty.clone(),
                    false,
                );
                add(
                    &format!("Bytes#write_{value}_{order}"),
                    vec![
                        ("self".to_string(), bytes_mut.clone()),
                        ("offset".to_string(), Ty::Usize),
                        ("value".to_string(), ty.clone()),
                    ],
                    Ty::Unit,
                    false,
                );
            }
        }

        // The `String` surface (M6.2): construct, measure, copy, and convert
        // to/from `Bytes` with an ownership transfer. `as_str`/`as_bytes`
        // return a borrow of the receiver, which needs the `&str`
        // return-elision rule (M6.3) before it can be typed at all.
        let string = Ty::String;
        let string_shared = Ty::Ref { mutable: false, inner: Box::new(string.clone()) };
        add("String#empty", vec![], string.clone(), true);
        add("String#from_str", vec![("s".to_string(), Ty::Str)], string.clone(), true);
        // By value: `from_bytes` takes ownership of the buffer, so the
        // receiver local is moved into this call rather than borrowed.
        add("String#from_bytes", vec![("b".to_string(), Ty::Bytes)], string.clone(), true);
        add("String#len", vec![("self".to_string(), string_shared.clone())], Ty::Usize, false);
        add(
            "String#char_count",
            vec![("self".to_string(), string_shared.clone())],
            Ty::Usize,
            false,
        );
        add("String#is_empty", vec![("self".to_string(), string_shared.clone())], Ty::Bool, false);
        add("String#clone", vec![("self".to_string(), string_shared.clone())], string.clone(), false);
        add(
            "String#into_bytes",
            vec![("self".to_string(), string.clone())],
            Ty::Bytes,
            false,
        );
        // A borrowed view of an owned buffer (M6.3). Both are the return-
        // borrow form: the result borrows from `self`, which is the whole
        // point of `&str` return elision.
        let str_ref = Ty::Ref {
            mutable: false,
            inner: Box::new(Ty::Str),
        };
        add(
            "String#as_str",
            vec![("self".to_string(), string_shared.clone())],
            str_ref.clone(),
            false,
        );
        add(
            "Bytes#as_str",
            vec![(
                "self".to_string(),
                Ty::Ref {
                    mutable: false,
                    inner: Box::new(Ty::Bytes),
                },
            )],
            str_ref,
            false,
        );

        // A length-carrying view of text (A-8). The two constructors are the
        // only places a length is *derived* (`str.view` runs one `strlen`,
        // `String.view` reads the length it already has); every operation on a
        // `View` is length-aware, and none of them is a C string function - a
        // slice has no NUL after it, so `strlen`/`strcmp` on one would read
        // past the slice and report a different string than the program has.
        // That is also why none of these takes or returns a `str`.
        //
        // `self` is by value for every one of them: a `View` is two words of
        // `Copy` data (a pointer and a length), so a borrow would add an
        // indirection and buy nothing - the same choice the integer
        // conversions make.
        add("str#view", vec![("self".to_string(), Ty::Str)], Ty::View, false);
        add(
            "String#view",
            vec![("self".to_string(), string_shared.clone())],
            Ty::View,
            false,
        );
        add("View#len", vec![("self".to_string(), Ty::View)], Ty::Usize, false);
        add("View#is_empty", vec![("self".to_string(), Ty::View)], Ty::Bool, false);
        add(
            "View#get",
            vec![("self".to_string(), Ty::View), ("i".to_string(), Ty::Usize)],
            Ty::U8,
            false,
        );
        add(
            "View#slice",
            vec![
                ("self".to_string(), Ty::View),
                ("from".to_string(), Ty::Usize),
                ("to".to_string(), Ty::Usize),
            ],
            Ty::View,
            false,
        );
        add(
            "View#eq",
            vec![("self".to_string(), Ty::View), ("other".to_string(), Ty::View)],
            Ty::Bool,
            false,
        );
        // The one way to make a `str`-shaped value out of a slice, and the
        // reason it exists rather than an implicit conversion: a copy is the
        // only safe answer when the source may not be terminated.
        add(
            "View#to_string",
            vec![("self".to_string(), Ty::View)],
            Ty::String,
            false,
        );

        // Growing a `String` in place (S1). The same exclusive-borrow rule
        // as `Bytes`, and the same NUL-termination invariant: `push_str`
        // appends a borrowed view, so a view of `self` would alias the
        // buffer `realloc` may move - the borrow checker rejects that.
        let string_exclusive = Ty::Ref { mutable: true, inner: Box::new(Ty::String) };
        add(
            "String#push",
            vec![("self".to_string(), string_exclusive.clone()), ("b".to_string(), Ty::U8)],
            Ty::Unit,
            false,
        );
        add(
            "String#push_str",
            vec![("self".to_string(), string_exclusive.clone()), ("s".to_string(), Ty::Str)],
            Ty::Unit,
            false,
        );
        add(
            "String#reserve",
            vec![("self".to_string(), string_exclusive.clone()), ("extra".to_string(), Ty::Usize)],
            Ty::Unit,
            false,
        );
        add("String#clear", vec![("self".to_string(), string_exclusive)], Ty::Unit, false);
        add("String#with_capacity", vec![("n".to_string(), Ty::Usize)], string.clone(), true);

        // Failing deliberately (S1): `kelp_panic`/`kelp_assert` already exist in
        // the runtime, these two entries are all that was missing. They are
        // free functions, not methods - the map key is a bare name, which is
        // the same namespace `print` occupies.
        add("panic", vec![("msg".to_string(), Ty::Str)], Ty::Unit, false);
        add("assert", vec![("cond".to_string(), Ty::Bool)], Ty::Unit, false);

        // `thread.fence` (MK9-S3-M1 stage A2). Its key is module-qualified
        // rather than bare, because the function is reached through an import
        // (`thread.fence`) and not as a prelude name, and the shipped
        // `std/thread` module declares nothing: a fence is one instruction on
        // either side (`__atomic_thread_fence` in C, `fence seq_cst` in IR)
        // with no C body to write, so the compiler lowers it and the module's
        // source documents the surface instead, exactly as `std/atomic`
        // documents the atomic methods it does not declare. (Stage A2 first
        // tried this as an `extern fun fence(order: atomic.Ordering)` and
        // BUG-kQ7Rv was the extern prelude having no typedef for the enum;
        // that prelude now writes one, and this stays a builtin for the
        // instruction reason above.)
        add(
            crate::stdlib::FENCE_FN,
            vec![("order".to_string(), atomic_order_ty())],
            Ty::Unit,
            false,
        );

        // Integer conversions (S1). Every integer type has a `to_<T>` for
        // every integer type, because nothing else in the language converts
        // one width to another: operands must match exactly, so a `u64 % 10`
        // digit cannot reach `String.push(u8)` and a `u8` byte cannot index
        // with `usize` without one. Widening is exact; a conversion that can
        // lose a value is a checked narrowing that panics with the call's own
        // name (v1 has no `Option`), which the removed C backend emitted inline.
        pub(crate) const INT_TYPES: &[&str] =
            &["i32", "i64", "u32", "u64", "u8", "u16", "usize", "isize"];
        for src in INT_TYPES {
            for dst in INT_TYPES {
                add(
                    &format!("{src}#to_{dst}"),
                    vec![("self".to_string(), Ty::from_name(src).expect("integer type"))],
                    Ty::from_name(dst).expect("integer type"),
                    false,
                );
            }
            // Integer -> float (the jsonlibary port's fourth gap). An integer
            // never mixes with a float by itself - `n * 1.0` is a type error,
            // because operands must match exactly - and before this the only
            // way to widen one was to format it and parse it back
            // (`str.parse_f64(fmt.int(n).as_str())`), which allocates a
            // `String` to add zero to a number. `to_f64` is the exact
            // widening the hardware already has; `to_f32` is the lossy one,
            // and it is *unchecked* on purpose, because C's float conversion
            // is defined for every input (a value too large becomes an
            // infinity), so there is no lost-value case to refuse.
            //
            // The reverse (a float `to_<int>`) is deliberately absent: it can
            // lose a value, and v1 has no `Option` for it to report the loss
            // with. Write `math.trunc`/`math.floor` first and then a checked
            // integer conversion.
            for dst in ["f64", "f32"] {
                add(
                    &format!("{src}#to_{dst}"),
                    vec![("self".to_string(), Ty::from_name(src).expect("integer type"))],
                    Ty::from_name(dst).expect("float type"),
                    false,
                );
            }
        }

        // Float width conversion: `f32.to_f64()` is exact, `f64.to_f32()` rounds
        // to the nearest `f32` (an infinity when the value is too large), and
        // neither can lose a value in the checked sense an integer narrowing
        // can, so both are unchecked like the integer-to-float pair above.
        add("f32#to_f64", vec![("self".to_string(), Ty::F32)], Ty::F64, false);
        add("f64#to_f32", vec![("self".to_string(), Ty::F64)], Ty::F32, false);

        // Byte-order conversion (kernel stage 3, MK9-S8 / roadmap B-10). Every
        // integer wider than a byte has `swap_bytes`, `to_be` and `to_le`, plus
        // the two static forms written on the type name
        // (`u32.from_be(x)`/`u32.from_le(x)`) that the associated-builtin path
        // above already resolves, the same way `Bytes.empty()` is resolved.
        //
        // `u8` is deliberately absent: swapping one byte is the identity, and
        // `to_be`/`to_le` on a byte are the identity on every target, so the
        // three names would only invite a reader to wonder which they meant.
        // `i16` is absent because v1 has no `i16` scalar to be one (the
        // `repr(i16)` tag storage in kernel stage 2 is a different thing).
        for src in ["u16", "u32", "u64", "i32", "i64", "usize", "isize"] {
            let ty = Ty::from_name(src).expect("integer type");
            for op in ["swap_bytes", "to_be", "to_le"] {
                add(
                    &format!("{src}#{op}"),
                    vec![("self".to_string(), ty.clone())],
                    ty.clone(),
                    false,
                );
            }
            for op in ["from_be", "from_le"] {
                add(
                    &format!("{src}#{op}"),
                    vec![("x".to_string(), ty.clone())],
                    ty.clone(),
                    true,
                );
            }
        }
        m
    });
