use super::*;

/// The enclosing bindings a scoped literal may capture exclusively: writable
/// (`var`), not a borrow, not a function value.
pub(crate) fn writable_outer(ctx: &Ctx) -> std::collections::HashSet<String> {
    let mut m = std::collections::HashSet::new();
    for scope in ctx.scopes.iter() {
        for (k, v) in scope.iter() {
            if v.mutable
                && !crate::types::is_borrowed_field_ty(&v.ty)
                && !matches!(v.ty, Ty::Fn { .. } | Ty::Closure { .. } | Ty::ScopedFn { .. })
            {
                m.insert(k.clone());
            } else {
                m.remove(k);
            }
        }
    }
    m
}

/// Records that the place `e`, a capture of the scoped literal being checked,
/// was used where an exclusive borrow is needed, if the capture is one the
/// literal may upgrade (`kelp wiki page language/functions.md`). `check_fn_literal`'s
/// retry loop reads the set.
pub(crate) fn note_excl_need(e: &tir::Expr, ctx: &Ctx) {
    if let tir::ExprKind::Capture { index } = &e.kind {
        if let Some((name, _)) = ctx.captures.get(*index) {
            if ctx.borrowed_captures && ctx.outer_bindings.contains_key(name) {
                ctx.excl_needed.borrow_mut().insert(name.clone());
            }
        }
    }
}

/// `Type.name` for a receiver method, `name` for a plain function - for
/// diagnostics only (`sig_key` is the lookup key; this is what a person
/// reads).
pub(crate) fn display_name(f: &ast::FunctionDecl) -> String {
    match &f.receiver {
        Some(r) => format!("{}.{}", r.type_name, f.name),
        None => f.name.clone(),
    }
}

/// Checks the native-callback parameters of a user `extern fun` (gap 4).
///
/// A `move closure(...)` parameter is the one shape the C backend expands in
/// place into a `(thunk, env, destroy)` triple, so the closure's own signature
/// must be one the generated thunk can forward by value: every parameter and
/// the return are a scalar, `bool`, `str`, an opaque handle or `unit`, no
/// parameter takes ownership or an exclusive borrow, and the function has at
/// most one such parameter. A plain `closure(...)` parameter is refused by
/// name: the native side has to own the closure, or the `destroy` notify has
/// nothing safe to free.
pub(crate) fn check_extern_callback_params(f: &ast::FunctionDecl, sig: &FnSig) -> Result<()> {
    let declared = if f.receiver.is_some() {
        &sig.params[1..]
    } else {
        &sig.params[..]
    };
    let mut callbacks = 0usize;
    for (p, (_, ty)) in f.params.iter().zip(declared.iter()) {
        match ty {
            Ty::Closure { params, ret } => {
                callbacks += 1;
                let written = p.ty.display();
                if let Some(written_params) = &p.ty.fn_params {
                    for (wp, resolved) in written_params.iter().zip(params.iter()) {
                        if wp.mode != ast::ParamMode::Borrow {
                            return Err(TypeError {
                                help: None,
                                message: format!(
                                    "the callback closure type '{written}' takes '{}' by ownership or an exclusive borrow, but a native callback thunk forwards every argument by value: write the closure parameter as a scalar, 'bool', 'str', an opaque handle or 'unit'",
                                    wp.display()
                                ),
                                span: p.ty.span,
                            });
                        }
                        if !is_callback_abi_ty(resolved) {
                            return Err(TypeError {
                                help: None,
                                message: format!(
                                    "the callback closure type '{written}' passes '{}' by value, which a native callback thunk cannot forward: write the closure parameter as a scalar, 'bool', 'str', an opaque handle or 'unit'",
                                    wp.ty.display()
                                ),
                                span: p.ty.span,
                            });
                        }
                    }
                }
                if !is_callback_abi_ty(ret) {
                    return Err(TypeError {
                        help: None,
                        message: format!(
                            "the callback closure type '{written}' returns '{}', which a native callback thunk cannot forward: a callback closure must return a scalar, 'bool', 'str', an opaque handle or 'unit'",
                            ret.display()
                        ),
                        span: p.ty.span,
                    });
                }
            }
            Ty::Ref { inner, .. } if matches!(**inner, Ty::Closure { .. }) => {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "the parameter '{}' of an 'extern fun' is a closure handed to native code, which must own it: write the type 'move {}' so the destroy notify has an environment to free",
                        p.name,
                        p.ty.display()
                    ),
                    span: p.ty.span,
                });
            }
            _ => {}
        }
    }
    if callbacks > 1 {
        return Err(TypeError {
            help: None,
            message: format!(
                "'{}' takes {callbacks} callback closures, but a user 'extern fun' can expand at most one 'move closure(...)' parameter in this pass",
                f.name
            ),
            span: f.span,
        });
    }
    Ok(())
}

/// The types a native callback thunk forwards by value (gap 4): the scalars,
/// `bool`, `str`, an opaque handle and `unit`. Everything else would need a
/// pointer the language cannot spell yet.
pub(crate) fn is_callback_abi_ty(ty: &Ty) -> bool {
    ty.is_numeric()
        || matches!(ty, Ty::Bool | Ty::Char | Ty::Str | Ty::Unit | Ty::Handle(_))
}

pub(crate) fn check_function(
    f: &ast::FunctionDecl,
    key: &str,
    sig: &FnSig,
    type_args: &HashMap<String, Ty>,
    sigs: &HashMap<String, FnSig>,
    structs: &HashMap<String, StructDef>,
    enums: &HashMap<String, EnumDef>,
    consts: &HashMap<String, ConstInfo>,
    const_fns: &std::rc::Rc<ConstFns>,
    owned_structs: &std::collections::HashSet<String>,
    overloads: &HashMap<String, Vec<(Vec<Ty>, String)>>,
    project: &Project,
    all_pds: &[PdInfo],
    wired: &HashMap<String, Vec<PdInfo>>,
    wiring: &PdWiring,
    env: &TypeEnv,
    fns: &FuncEnv,
    statics: &HashMap<String, StaticInfo>,
) -> Result<tir::Function> {
    // Issue #286: the receiver-mode probe checks an inferable method with `self`
    // exclusive, so the writes its body makes are recorded rather than refused
    // (see `infer_receiver_modes` in `program.rs`). Call sites still see the
    // shared signature the probe started from.
    let probing = wiring.writers.is_some()
        && f.receiver.as_ref().is_some_and(|r| r.inferable)
        && matches!(f.body, ast::FunctionBody::Block(_) | ast::FunctionBody::Expr(_));
    let probe_sig;
    let sig = if probing {
        let mut s = sig.clone();
        if let Some((_, Ty::Ref { mutable, .. })) = s.params.first_mut() {
            *mutable = true;
        }
        probe_sig = s;
        &probe_sig
    } else {
        sig
    };
    // The `...` variadic marker (gap 7) is legal in exactly one place: a
    // *user* `extern fun`, meaning one whose module a `coral.toml`
    // `[extern."<module id>"]` section names. A compiler-shipped `extern
    // fun` cannot carry one (its fixed C body has no way to read extras), a
    // plain function cannot (there is no C call to pass them to), and a
    // generic one cannot, because the wrapper a call site names is fixed by
    // the instantiation and nothing here would specialize it.
    if f.variadic {
        if !f.extern_ {
            return Err(TypeError {
                help: None,
                message: format!(
                    "'{}' writes '...', which is only legal on a user 'extern fun': a plain function has no native call to pass the extra arguments to",
                    display_name(f)
                ),
                span: f.span,
            });
        }
        if project.extern_ffi_of(&f.module).is_none() {
            return Err(TypeError {
                help: None,
                message: format!(
                    "'{}' writes '...', but only a user 'extern fun' - one whose module has a coral.toml '[extern.\"{}\"]' section - may be variadic",
                    display_name(f),
                    f.module
                ),
                span: f.span,
            });
        }
        if !decl_type_params(f).is_empty() {
            return Err(TypeError {
                help: None,
                message: format!(
                    "'{}' is variadic and generic: a variadic user 'extern fun' cannot have type parameters",
                    display_name(f)
                ),
                span: f.span,
            });
        }
    }
    // A `naked fun` (kernel stage 5) is a declaration whose whole body is one
    // `asm(...)`: nothing else can be emitted around it, so nothing else may be
    // written. The check is here rather than in the parser because it is a
    // property of the declaration the type checker already owns.
    if f.naked {
        if f.extern_ {
            return Err(TypeError {
                help: None,
                message: "a 'naked fun' cannot be 'extern': it has a body of its own".to_string(),
                span: f.span,
            });
        }
        if f.receiver.is_some() {
            return Err(TypeError {
                help: None,
                message: "a 'naked fun' cannot be a receiver method: it has no compiler-emitted prologue, so there is nothing that could pass the receiver".to_string(),
                span: f.span,
            });
        }
        if f.name == "main" {
            return Err(TypeError {
                help: None,
                message: "a 'naked fun' cannot be 'main': the compiler emits the hosted entry point's own C signature for it".to_string(),
                span: f.span,
            });
        }
        if !f.params.is_empty() {
            return Err(TypeError {
                help: None,
                message: "a 'naked fun' takes no parameters: the asm reads the incoming registers itself, and the compiler binds nothing".to_string(),
                span: f.span,
            });
        }
        let single_asm = match &f.body {
            ast::FunctionBody::Block(b) => {
                b.stmts.len() == 1
                    && matches!(
                        &b.stmts[0],
                        ast::Stmt::Expr { expr: ast::Expr::Asm { .. }, .. }
                    )
            }
            ast::FunctionBody::Expr(e) => matches!(e, ast::Expr::Asm { .. }),
            ast::FunctionBody::Extern | ast::FunctionBody::Signature => false,
        };
        if !single_asm {
            return Err(TypeError {
                help: None,
                message: "a 'naked fun' body must be exactly one 'asm(...)' statement: nothing else can be emitted around it".to_string(),
                span: f.span,
            });
        }
    }
    // An `interrupt fun` (kernel stage 5) is a CPU handler: the CPU enters it
    // with an interrupt frame, so it runs only in a freestanding image, on a
    // target the backend lowers, and it takes and returns nothing.
    if f.interrupt {
        let triple = project.target().triple().unwrap_or("");
        let refuse = |message: String| {
            Err(TypeError { help: None, message, span: f.span })
        };
        if !project.target().is_freestanding() {
            return refuse(format!(
                "'{}' is an 'interrupt fun', which is only legal in a freestanding build: a CPU enters an interrupt handler only on a bare-metal target",
                display_name(f)
            ));
        }
        if !(triple.starts_with("riscv64") || triple.starts_with("x86_64")) {
            return refuse(format!(
                "'{}' is an 'interrupt fun', and the triple '{triple}' has no interrupt lowering: kelpc lowers it for riscv64 and x86_64",
                display_name(f)
            ));
        }
        if f.naked {
            return refuse("an 'interrupt fun' cannot be 'naked': the compiler emits the handler's register save and its return".to_string());
        }
        if f.extern_ || f.receiver.is_some() || f.name == "main" {
            return refuse(format!(
                "'{}' cannot be an 'interrupt fun': an interrupt handler is a plain module-level function",
                display_name(f)
            ));
        }
        if !decl_type_params(f).is_empty() {
            return refuse(format!(
                "'{}' is an 'interrupt fun' and generic: a handler is one function the CPU enters, so it cannot have type parameters",
                display_name(f)
            ));
        }
        if !f.params.is_empty() {
            return refuse(format!(
                "'{}' is an 'interrupt fun' and takes parameters: the CPU enters it with its own interrupt frame, and the call binds nothing",
                display_name(f)
            ));
        }
        if sig.ret != Ty::Unit {
            return refuse(format!(
                "'{}' is an 'interrupt fun' and returns a value: a handler returns through the CPU's interrupt return, so it returns nothing",
                display_name(f)
            ));
        }
    }
    let receiver_struct = f
        .receiver
        .as_ref()
        .and_then(|_| struct_name_of(&sig.params[0].1).map(|s| s.to_string()));
    // A declared destructor (kernel stage 9, MK9-S6-M1) reads its passdowns
    // the way `main` does: directly from the generated globals. The reason is
    // that the generated `kelp_drop_<T>` calls it from every drop point with
    // one pointer and nothing else, so a hidden passdown parameter would have
    // no caller able to supply it. The passdown surface is read-only inside a
    // function, so the global read is the same borrow `main` takes.
    let is_drop_method = f
        .receiver
        .as_ref()
        .is_some_and(|_| f.name == crate::types::DROP_METHOD);
    // Passdowns this function references become trailing hidden parameters
    // (see `PdInfo::arg` for the call-site side). `main` is the exception
    // downstream, not here: the removed C backend emitted a fixed `main(argc, argv)`
    // regardless of parameters, and the body's `pd#...` identifiers then
    // resolve to the file-scope globals - which is why the hidden name and
    // the global name are the same string.
    let hidden = if key == "main" || is_drop_method {
        Vec::new()
    } else {
        // An instantiation's passdown wiring is the template's: the scan runs
        // over declarations, and which passdowns a body names does not depend
        // on its type arguments (A-2). The borrow *mode* is the template's for
        // the same reason.
        let wiring_key = if wired.contains_key(key) {
            key.to_string()
        } else {
            fns.instances
                .borrow()
                .origin
                .get(key)
                .cloned()
                .unwrap_or_else(|| key.to_string())
        };
        wired.get(&wiring_key).cloned().unwrap_or_default()
    };
    let mut ctx = base_ctx(
        &f.module,
        sigs,
        structs,
        enums,
        consts,
        const_fns,
        owned_structs,
        overloads,
        project,
        all_pds,
        None,
        wired,
        env,
        fns,
        statics,
    );
    // A synthesized trait default resolves its body in the trait's module (see
    // `FunctionDecl::body_module`): its signature was substituted with the
    // implementing type, but its body's types and free names were written in
    // the trait's module.
    if let Some(m) = &f.body_module {
        ctx.prefix = project.prefix_of(m).to_string();
        ctx.aliases = project.alias_prefixes_of(m);
    }
    ctx.type_args = type_args.clone();
    // A-12 slice 5: an impl method's body may name `Self::Item` in a local
    // annotation. `resolve.rs` substituted it in the signature only, so add the
    // impl's bindings under the `Self::Item` spelling for the body.
    if let Some(site) = &f.trait_impl {
        for key in trait_keys_with_supertraits(&env, &site.trait_name) {
            if let Some(bindings) = env.assoc.get(&(key, site.self_type.clone())) {
                for (item, ty) in bindings {
                    ctx.type_args.insert(format!("Self::{item}"), ty.clone());
                }
            }
        }
    }
    ctx.ret_ty = sig.ret.clone();
    ctx.fn_name = f.name.clone();
    ctx.receiver_struct = receiver_struct;
    ctx.pd_global_borrow = key == "main" || is_drop_method;
    // `unsafe fun` (MK9-S2 stage 1) and `naked fun` (kernel stage 5): the whole
    // body is an unsafe context, so the unsafe-only pointer operations, and the
    // `asm` a naked body must be, are legal without a block around them. Every
    // other function starts at zero and needs an `unsafe { ... }`.
    ctx.unsafe_depth = if f.unsafe_ || f.naked { 1 } else { 0 };
    ctx.own_hidden = hidden.iter().map(|pd| pd.hidden.clone()).collect();
    let wiring_key = ctx.wiring_key_of(key);
    ctx.fn_key = wiring_key;
    ctx.pd_modes = wiring.modes.clone();
    ctx.pd_probe = wiring.probe.clone();
    ctx.self_probe = probing;
    ctx.block_note = f
        .receiver
        .as_ref()
        .filter(|r| r.in_block && !r.mutable)
        .map(|_| format!("'{}' writes its receiver, but a method inside a 'datac' block does not infer its receiver mode: write 'mut {}()' in the block", display_name(f), f.name));
    let mut params = Vec::new();
    // Parameters (and the receiver, if any) are immutable bindings, like
    // `val`; v1 has no syntax to declare a mutable *binding* for one (the
    // borrow itself can still be exclusive - that's `Ty::Ref { mutable:
    // true, .. }`, a property of the type, not the binding).
    if f.receiver.is_some() {
        let (pname, ty) = &sig.params[0];
        ctx.define(pname, ty.clone(), false);
        params.push((pname.clone(), ty.clone()));
    }
    let declared = if f.receiver.is_some() { &sig.params[1..] } else { &sig.params[..] };
    for (p, (_, ty)) in f.params.iter().zip(declared.iter()) {
        ctx.define(&p.name, ty.clone(), false);
        params.push((p.name.clone(), ty.clone()));
    }

    let body = match &f.body {
        ast::FunctionBody::Block(b) => check_block(b, &mut ctx, Some(&sig.ret))?,
        ast::FunctionBody::Expr(e) => {
            // A body with no `=> T` (Gitea #280) has no expected type: its own
            // type becomes the return type, worked out by `expr_return.rs`.
            let expected = if f.return_type.is_none() { None } else { Some(&sig.ret) };
            let checked = check_expr(e, &mut ctx, expected)?;
            tir::Block {
                stmts: vec![],
                tail: Some(Box::new(checked)),
            }
        }
        ast::FunctionBody::Extern => {
            // No `.kp` body. Either the removed C backend wrote the
            // compiler-supplied C (see `externs.rs`, stdlib modules only), or
            // - MK9-S1-M1, roadmap B-1 - this module has its own
            // `coral.toml` `[extern."<module id>"]` entry, in which case
            // the removed C backend wrote a direct-call trampoline to the native
            // symbol instead, gated the same way a stdlib extern is (see
            // `resolve.rs::domain_of_with_extern` and the `gated` map it
            // feeds). Confirm one of the two actually admits this
            // declaration before accepting it - otherwise the failure would
            // surface as a `cc` error at best, or as a silently missing
            // symbol, or (worse, for the manifest case) as a user extern
            // that reaches a native symbol its own manifest never
            // allowlisted.
            let prefix = project.prefix_of(&f.module);
            let bare = f.name.strip_prefix(prefix).unwrap_or(&f.name);
            if !crate::externs::has_body(&f.module, bare) {
                match project.extern_ffi_of(&f.module) {
                    Some(decl) if decl.symbol_of(bare).is_some() => {
                        check_extern_callback_params(f, sig)?;
                    }
                    Some(_) => {
                        return Err(TypeError {
                            help: None,
                            message: format!(
                                "extern function '{bare}' is not in this module's coral.toml '[extern.\"{}\"]' symbol allowlist - add it there before declaring 'extern fun {bare}'",
                                f.module
                            ),
                            span: f.span,
                        });
                    }
                    None => {
                        return Err(TypeError {
                            help: None,
                            message: format!(
                                "extern function '{bare}' has no compiler-supplied body - only compiler-shipped stdlib modules may declare 'extern fun', unless this module has its own coral.toml '[extern.\"{}\"]' section naming '{bare}' in its 'symbols' allowlist", f.module
                            ),
                            span: f.span,
                        });
                    }
                }
            }
            tir::Block {
                stmts: vec![],
                tail: None,
            }
        }
        // A trait method signature (A-12 slice 2) has no body and is never a
        // function of its own: the impl supplies the body, which is an
        // ordinary method this pass does check. Reaching here would mean a
        // signature had been registered as a declaration.
        ast::FunctionBody::Signature => {
            return Err(TypeError {
                help: None,
                message: format!(
                    "'{}' is a trait method signature and has no body: write the body in the 'impl' block",
                    display_name(f)
                ),
                span: f.span,
            })
        }
    };

    // A function can satisfy its return type either via a tail expression or
    // via explicit `return` statements on every path; the latter isn't
    // checked here (that needs real control-flow/divergence analysis, out of
    // scope for M1/M2), so a mismatch is only reported when a tail
    // expression is actually present and disagrees with the declared return
    // type.
    // A body whose tail is a `panic` diverges, so it satisfies any declared
    // return type: `fun unwrap<T>(o: &Option<T>) => T { ... panic("...") }` is
    // the "this cannot happen" accessor. Nothing else in the language is
    // divergence-aware - an `if` arm cannot be a `panic` - so this is
    // deliberately the narrow rule the accessors need.
    let tail_diverges = body
        .tail
        .as_ref()
        .is_some_and(|e| tail_diverges(e));
    if sig.ret != Ty::Unit
        && body.tail.is_some()
        && !tail_diverges
        && !f.naked
        && !str_view_compatible(&body.ty(), &sig.ret)
    {
        // A unit body that ends in an `if` with no `else` is the usual cause,
        // so the message says what to write instead.
        let hint = if body.ty() == Ty::Unit {
            "; end the block with 'return <value>', since an 'if' with no 'else' gives no value"
        } else {
            ""
        };
        return Err(TypeError {
            help: None,
            message: format!(
                "function '{}' declared to return '{}' but body evaluates to '{}'{}",
                display_name(f),
                sig.ret.display(),
                body.ty().display(),
                hint
            ),
            span: f.span,
        });
    }

    let body = if f.naked {
        // The single `asm(...)` becomes a statement rather than the block's
        // tail, so each backend emits it and then nothing: the asm itself is
        // what returns, and a `return` of the declared type would be an
        // instruction a naked function cannot have.
        let mut body = body;
        let asm = match body.tail.take() {
            Some(e) => *e,
            None => match body.stmts.pop() {
                Some(tir::Stmt::Expr(e)) => e,
                _ => unreachable!("a naked body is exactly one asm expression"),
            },
        };
        tir::Block {
            stmts: vec![tir::Stmt::Expr(asm)],
            tail: None,
        }
    } else {
        body
    };

    if probing && ctx.self_written.get() {
        if let Some(writers) = &wiring.writers {
            writers.borrow_mut().insert(f.span);
        }
    }
    Ok(tir::Function {
        name: key.to_string(),
        params: {
            params.extend(hidden.iter().map(|pd| {
                (
                    pd.hidden.clone(),
                    pd.borrow_ty(wiring.exclusive(&ctx.fn_key, pd)),
                )
            }));
            params
        },
        ret: sig.ret.clone(),
        body,
        // The declaration's own span, which is the line a backend names as the
        // function's: the C backend writes nothing for it today, and the LLVM
        // backend's `DISubprogram` reads it (MK7 S4).
        span: f.span,
        module: f.module.clone(),
        extern_: f.extern_,
        // Only a `fun` literal that captured something has an environment;
        // a declaration never does (A-4 phase E).
        env: None,
        naked: f.naked,
        interrupt: f.interrupt,
        align: f.align,
        section: f.section.clone(),
    })
}

/// A call to a `must_use fun` whose result goes nowhere (MISSING-Cn2Ym).
pub(crate) fn refuse_dropped_must_use(checked: &tir::Expr, ctx: &Ctx, span: Span) -> Result<()> {
    if let tir::ExprKind::Call { callee, .. } = &checked.kind {
        // A generic instance's key carries its type arguments (`Sender#send<i64>`).
        let plain = callee.split('<').next().unwrap_or(callee);
        if ctx.must_use_fns.contains(plain) {
            return Err(TypeError {
                help: None,
                message: format!(
                    "the result of '{}' must be used: it reports a failure or hands a value back. Branch on it, or write 'val _r = ...' to discard it on purpose",
                    display_callee(plain)
                ),
                span,
            });
        }
    }
    Ok(())
}

pub(crate) fn check_block(block: &ast::Block, ctx: &mut Ctx, expected_tail: Option<&Ty>) -> Result<tir::Block> {
    ctx.push_scope();
    let mut stmts = Vec::new();
    let mut tail = None;
    // The open containers this block declared (`var xs = List.empty()` with no
    // type written anywhere), and where each one's `let` sits in `stmts`. An
    // operation later in the block, or in a block under it, decides the
    // element type and rewrites the binding; the local emitted here has to be
    // rewritten with it, because the struct and the routines the backend
    // monomorphizes are chosen by the element type.
    let mut open: Vec<OpenBinding> = Vec::new();

    for (i, stmt) in block.stmts.iter().enumerate() {
        let is_last = i + 1 == block.stmts.len();
        // Statements are newline-terminated now (no `;`), so "textually
        // last in the block" is the only signal for tail-expression status.
        let is_tail = is_last && matches!(stmt, ast::Stmt::Expr { .. });

        if is_tail {
            if let ast::Stmt::Expr { expr, .. } = stmt {
                if matches!(expr, ast::Expr::Ident(..)) {
                    ctx.box_sink_ok = Some(BoxSink::Tail);
                }
                let checked = check_expr(expr, ctx, expected_tail)?;
                // A unit block throws its tail value away, so a `must_use`
                // call there is a dropped result like any statement's.
                if matches!(expected_tail, Some(Ty::Unit)) {
                    refuse_dropped_must_use(&checked, ctx, expr.span())?;
                }
                // A let-else that leaves on every path has no value to hand
                // back, so it stays a statement: the block then has no tail
                // for the declared return type to be checked against.
                if let_else_leaves(expr) {
                    stmts.push(tir::Stmt::Expr(checked));
                } else {
                    tail = Some(Box::new(checked));
                }
            }
        } else {
            let checked = check_stmt(stmt, ctx)?;
            if let ast::Stmt::Let { name, ty: None, init, .. } = stmt {
                let still_open = ctx
                    .lookup(name)
                    .map_or(false, |i| crate::types::is_open_container(&i.ty));
                if still_open {
                    open.push(OpenBinding {
                        name: name.clone(),
                        index: stmts.len(),
                        span: init.span(),
                        call: empty_container_call(init).map(|(c, m)| (c.to_string(), m.to_string())),
                    });
                }
            }
            // A destructuring binding lowers to several statements (the value
            // it takes apart, then one binding per position), and the open-
            // container repair below is keyed by a statement index, so the
            // count is what the next binding's index has to be measured
            // against. `check_stmt` is the only place that decides how many.
            stmts.extend(checked);
        }
        settle_open_bindings(&mut open, &mut stmts, ctx);
    }
    settle_open_bindings(&mut open, &mut stmts, ctx);

    if let Some(still_open) = open.first() {
        let ty = ctx.lookup(&still_open.name).map(|i| i.ty.clone()).unwrap_or(Ty::Unit);
        let error = open_container_declaration_error(&ty, still_open);
        ctx.pop_scope();
        return Err(error);
    }

    ctx.pop_scope();
    Ok(tir::Block { stmts, tail })
}

/// One `let` in a block whose container started open and may still be open.
pub(crate) struct OpenBinding {
    pub(crate) name: String,
    /// Where the `let` sits in the block's statement list.
    pub(crate) index: usize,
    /// The initializer's span, which is where the diagnostic points.
    pub(crate) span: Span,
    /// The `(Type, method)` the initializer was, when it was the associated
    /// call this is all about: the diagnostic names the constructor that was
    /// written rather than guessing between `Map.new()` and `Map.empty()`.
    pub(crate) call: Option<(String, String)>,
}

/// Rewrites every binding an operation has since decided, and forgets it. The
/// type an open container took is the one the scope now holds for the name,
/// so `check_expr`'s repair is the same repair as a deciding call's: both
/// `set_binding_ty` it, and this moves the result into the emitted local.
pub(crate) fn settle_open_bindings(open: &mut Vec<OpenBinding>, stmts: &mut [tir::Stmt], ctx: &Ctx) {
    open.retain(|binding| {
        let Some(info) = ctx.lookup(&binding.name) else {
            return false;
        };
        if crate::types::is_open_container(&info.ty) {
            return true;
        }
        let ty = info.ty.clone();
        if let Some(tir::Stmt::Let { ty: slot, init, .. }) = stmts.get_mut(binding.index) {
            *slot = ty.clone();
            init.ty = ty;
        }
        false
    });
}

/// The `Type.empty()`/`Type.new()` call an initializer is, when it is one.
/// Used only to name the constructor in a diagnostic: an open container that
/// nothing ever decided is reported where it was written, saying which call
/// left the element type out.
pub(crate) fn empty_container_call(expr: &ast::Expr) -> Option<(&str, &str)> {
    let ast::Expr::MethodCall { receiver, method, args, .. } = expr else {
        return None;
    };
    if !args.is_empty() {
        return None;
    }
    let ast::Expr::Ident(name, _) = receiver.as_ref() else {
        return None;
    };
    matches!(name.as_str(), "List" | "Box" | "Map").then(|| (name.as_str(), method.as_str()))
}

/// The diagnostic an open container gets when nothing in its block decided
/// what it holds: the same one a bare `List.empty()` was always refused with,
/// so the fix a reader is told is still the binding.
pub(crate) fn open_container_declaration_error(ty: &Ty, binding: &OpenBinding) -> TypeError {
    let message = match (&binding.call, ty) {
        (Some((container, method)), _) if container == "Map" => format!(
            "cannot tell what 'Map.{method}()' holds, write the type on the binding ('var m: Map<String, i32> = Map.{method}()')"
        ),
        (Some((container, _)), _) => format!(
            "cannot tell what '{container}.empty()' holds, write the element type on the binding ('var xs: {container}<String> = {container}.empty()') or start from '{container}.of(x)'"
        ),
        (None, Ty::Map(..)) => "cannot tell what 'Map.new()' holds, write the type on the binding ('var m: Map<String, i32> = Map.new()')".to_string(),
        (None, Ty::List(_)) => "cannot tell what 'List.empty()' holds, write the element type on the binding ('var xs: List<String> = List.empty()') or start from 'List.of(x)'".to_string(),
        (None, _) => "cannot tell what 'Box.empty()' holds, write the element type on the binding ('var b: Box<String> = Box.empty()') or start from 'Box.of(x)'".to_string(),
    };
    TypeError { message, span: binding.span, help: None }
}

pub(crate) fn check_stmt(stmt: &ast::Stmt, ctx: &mut Ctx) -> Result<Vec<tir::Stmt>> {
    match stmt {
        ast::Stmt::Let {
            name,
            mutable,
            ty,
            init,
            ..
        } => {
            if ctx.block_names.contains(name) {
                return Err(TypeError {
                    help: Some("name the binding something else: a block's name is unique in its function".to_string()),
                    message: format!("'{name}' is a block name in this function, so a 'let' may not shadow it"),
                    span: stmt_span(stmt),
                });
            }
            ctx.let_names.insert(name.clone());
            let box_block = match init {
                ast::Expr::Try { expr, .. } => match expr.as_ref() {
                    ast::Expr::MethodCall { receiver, method, args, .. } => block_box_receiver(receiver, method, args, ctx),
                    _ => None,
                },
                _ => None,
            };
            let box_let = box_block.is_some();
            if box_let && *mutable {
                return Err(TypeError {
                    help: Some("bind it with 'let', not 'var'".to_string()),
                    message: format!("a block box '{name}' is read-only: a 'var' may not hold one"),
                    span: stmt_span(stmt),
                });
            }
            ctx.box_let_ok = box_let;
            if matches!(init, ast::Expr::Ident(..)) {
                ctx.box_sink_ok = Some(BoxSink::Move(format!("'{name}'")));
            }
            let expected = match ty {
                Some(t) => Some(ctx.type_scope().resolve(t, false)?),
                None => None,
            };
            if let Some((root, rebased)) = binding_borrow_temporary(init, ctx.when_temps) {
                ctx.when_temps += 1;
                let tmp = format!("{}{}", BORROW_TEMP_PREFIX, ctx.when_temps - 1);
                let held = check_expr(root, ctx, None)?;
                let held_ty = held.ty.clone();
                ctx.define(&tmp, held_ty.clone(), false);
                let value = check_expr(&rebased, ctx, expected.as_ref())?;
                let final_ty = expected.unwrap_or_else(|| value.ty.clone());
                if final_ty != value.ty {
                    return Err(TypeError {
                        help: None,
                        message: format!(
                            "cannot assign value of type '{}' to '{name}' declared as '{}'",
                            value.ty.display(),
                            final_ty.display()
                        ),
                        span: stmt_span(stmt),
                    });
                }
                ctx.define(name, final_ty.clone(), *mutable);
                return Ok(vec![
                    tir::Stmt::Let { name: tmp, ty: held_ty, init: held },
                    tir::Stmt::Let { name: name.clone(), ty: final_ty, init: value },
                ]);
            }
            // A `let` with no written type whose initializer *is* a bare
            // `List.empty()`/`Box.empty()`/`Map.new()` is the one place an
            // empty container may leave its element open for a later
            // `push`/`insert` to decide. Anywhere else, including a call
            // somewhere inside the initializer, an empty container with no
            // type to take its element from is a declaration-time error,
            // exactly as it always was.
            let deferred = ty.is_none() && empty_container_call(init).is_some();
            let outer_defer = ctx.defer_empty_container;
            ctx.defer_empty_container = deferred;
            let init = check_expr(init, ctx, expected.as_ref());
            ctx.defer_empty_container = outer_defer;
            ctx.box_let_ok = false;
            let init = init?;
            let final_ty = expected.unwrap_or_else(|| init.ty.clone());
            if final_ty != init.ty {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "cannot assign value of type '{}' to '{name}' declared as '{}'",
                        init.ty.display(),
                        final_ty.display()
                    ),
                    span: stmt_span(stmt),
                });
            }
            match box_block {
                Some(block) => {
                    ctx.box_bindings.insert(name.clone(), block);
                }
                None => {
                    ctx.box_bindings.remove(name);
                }
            }
            ctx.define(name, final_ty.clone(), *mutable);
            Ok(vec![tir::Stmt::Let {
                name: name.clone(),
                ty: final_ty,
                init,
            }])
        }
        // `val (lo, hi) = min_max(xs)`: a destructuring binding
        // (MISSING-z2bB9, `design/syntax.md (kelp wiki)` 4). One binding becomes one owning
        // temporary plus one ordinary `let` per bound position, read out of
        // that temporary: the lowering is spelled here, in terms of the
        // record a tuple already is, so nothing downstream learns a new
        // statement and the positions the pattern ignores stay in the
        // temporary, whose own drop frees them.
        ast::Stmt::LetTuple {
            pattern,
            mutable,
            init,
            span,
        } => {
            let value = check_expr(init, ctx, None)?;
            // The initializer is moved into the destructuring, so a place is
            // taken by value exactly as `val p = place` takes it, and the
            // positions are then read out of that value one at a time.
            let value_ty = value.ty.clone();
            let elements = tuple_elems_of(&value_ty, ctx).ok_or_else(|| TypeError {
                help: None,
                message: format!(
                    "a destructuring binding takes one tuple value apart, and this initializer is a '{}': 'val (lo, hi) = ...' needs a tuple here ('(i32, i32)'), and a single value is bound with an ordinary 'val name = ...'",
                    value_ty.display()
                ),
                span: init.span(),
            })?;
            if elements.len() != pattern_arity(pattern) {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "this pattern has {} position(s), and the tuple it destructures ('{}') has {}",
                        pattern_arity(pattern),
                        value_ty.display(),
                        elements.len()
                    ),
                    span: *span,
                });
            }
            ctx.tuple_temps += 1;
            let tmp = format!("{}{}", TUPLE_TEMP_PREFIX, ctx.tuple_temps - 1);
            ctx.define(&tmp, value_ty.clone(), false);
            let mut stmts = vec![tir::Stmt::Let {
                name: tmp.clone(),
                ty: value_ty.clone(),
                init: value,
            }];
            lower_pattern(pattern, &tmp, &value_ty, &elements, *mutable, ctx, &mut stmts, *span)?;
            Ok(stmts)
        }
        ast::Stmt::While {
            cond,
            body,
            iterating,
            span,
        } => {
            let cond = check_expr(cond, ctx, Some(&Ty::Bool))?;
            if cond.ty != Ty::Bool {
                return Err(TypeError {
                    help: None,
                    message: format!("while condition must be 'bool', found '{}'", cond.ty.display()),
                    span: *span,
                });
            }
            ctx.loop_depth += 1;
            let body = check_block(body, ctx, None);
            ctx.loop_depth -= 1;
            let body = body?;
            let iterating = iterating.as_ref().and_then(|place| {
                ast::place_path(place).map(|path| tir::IteratedPlace {
                    path,
                    span: place.span(),
                })
            });
            Ok(vec![tir::Stmt::While { cond, body, iterating }])
        }
        // A `for` keeps its two desugars (`kelp wiki page language/iterators.md`): a `List<T>` or
        // `[T; N]` walks by index exactly as before, and anything that
        // implements `std.iter.Iterator` becomes a `var` over `next()`. Only
        // this pass knows the iterable's type, so this is where the choice is
        // made. A type-only probe is enough to choose and is what
        // `check_method_call` already does before it re-checks its receiver;
        // the desugar re-checks the iterable for real, once.
        ast::Stmt::For {
            binding,
            binding_span,
            iterable: written,
            body,
            span,
        } => {
            // `for x in xs.iter()` (and `for (k, v) in m.iter()`) walks the
            // same place as `for x in xs`: the element is the shared borrow
            // `List.get` already gives, and the body may not mutate `xs`.
            let stripped = for_iter_receiver(written, ctx)?;
            let iterable = stripped.as_ref().unwrap_or(written);
            let probe = check_expr(iterable, ctx, None)?;
            let iter_ty = probe.ty.clone();
            // `for (k, v) in map`: the parser read the pattern as a hidden
            // element binding and a `val (k, v) = <hidden>` at the top of the
            // body. A `Map` has no element that is a pair (its entries are not
            // stored as one), so the pattern is taken apart here: the loop walks
            // the keys, `k` is the key and `v` is `map.get(key)`.
            if crate::types::map_key_of(&iter_ty).is_some() {
                let desugared = desugar_map_for(binding, iterable, body, *span, &iter_ty)?;
                return check_stmt(&desugared, ctx);
            }
            let list = crate::types::list_elem_of(&iter_ty).is_some()
                || crate::types::array_elem_of(&iter_ty).is_some();
            if list {
                let desugared =
                    desugar_list_for(binding, *binding_span, iterable, body, *span, ctx);
                return check_stmt(&desugared, ctx);
            }
            if !implements_iterator(ctx, &iter_ty) {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "'for' walks a 'List<T>' or a type that implements 'Iterator', and '{}' is neither: implement 'std.iter.Iterator' for it ('type Item' and 'fun mut Self.next() => option.Option<Self::Item>'), or walk a 'List' by index",
                        iter_ty.display()
                    ),
                    span: *span,
                });
            }
            let desugared =
                desugar_iterator_for(binding, *binding_span, iterable, body, *span, &iter_ty, ctx);
            check_stmt(&desugared, ctx)
        }
        ast::Stmt::Break { span } | ast::Stmt::Continue { span } => {
            if ctx.loop_depth == 0 {
                let what = if matches!(stmt, ast::Stmt::Break { .. }) {
                    "break"
                } else {
                    "continue"
                };
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "'{what}' is a loop control statement, and no loop encloses it: write it inside a 'while' or a 'for'"
                    ),
                    span: *span,
                });
            }
            Ok(vec![if matches!(stmt, ast::Stmt::Break { .. }) {
                tir::Stmt::Break { span: *span }
            } else {
                tir::Stmt::Continue { span: *span }
            }])
        }
        ast::Stmt::Return { value, .. } => {
            let ret_ty = ctx.ret_ty.clone();
            let value = match value {
                Some(v) => Some(check_expr(v, ctx, Some(&ret_ty))?),
                None => None,
            };
            let actual = value.as_ref().map(|v| v.ty.clone()).unwrap_or(Ty::Unit);
            if !str_view_compatible(&actual, &ret_ty) {
                // A borrow where an owned value is wanted, inside a stdlib
                // module, is a `Copy`-only accessor: the arm that hands the
                // value out cannot take it from the borrow it was given.
                let note = if matches!(&actual, Ty::Ref { inner, .. } if **inner == ret_ty) {
                    stdlib_instantiation_note(ctx)
                } else {
                    String::new()
                };
                return Err(TypeError {
                    help: Some(format!("return a value of type '{}', or change the return type to '{}'.", ret_ty.display(), actual.display())),
                    message: format!(
                        "return type mismatch: expected '{}', found '{}'{note}",
                        ret_ty.display(),
                        actual.display()
                    ),
                    span: stmt_span(stmt),
                });
            }
            Ok(vec![tir::Stmt::Return {
                value,
                span: stmt_span(stmt),
            }])
        }
        ast::Stmt::Expr { expr, .. } => {
            let checked = check_expr(expr, ctx, None)?;
            refuse_dropped_must_use(&checked, ctx, stmt_span(stmt))?;
            // A non-tail expression statement discards its value, and nothing
            // owns an owned value afterwards - v1 has no drop point for a
            // temporary, so this would leak. Reject it instead.
            if !checked.ty.is_copy() {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "this expression produces an owned '{}' value that is never used - bind it to a 'val' or 'var', print it, or pass it to something that takes it by value",
                        checked.ty.display()
                    ),
                    span: stmt_span(stmt),
                });
            }
            Ok(vec![tir::Stmt::Expr(checked)])
        }
        // `defer` is lowered away by `defer.rs` before this pass runs, so one
        // reaching the checker is a compiler bug rather than a program error.
        ast::Stmt::Defer { span, .. } | ast::Stmt::Errdefer { span, .. } => Err(TypeError {
            help: None,
            message: "internal error: a 'defer' statement reached the type checker; it should have been lowered before checking"
                .to_string(),
            span: *span,
        }),
    }
}
