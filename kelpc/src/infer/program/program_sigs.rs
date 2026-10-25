use super::*;

pub(crate) fn declare_signatures(program: &ast::Program, project: &Project, decls: TypeDecls) -> std::result::Result<(TypeDecls, Signatures), Diagnostic> {
    let TypeDecls {
        declared_keys,
        declared_drops,
        traits,
        trait_impls,
        env,
        fns,
        structs,
        struct_order,
        const_fns,
        consts,
        enums,
        enum_order,
    } = decls;
    let assoc_bindings = collect_assoc_bindings(
        &traits,
        &trait_impls,
        &structs,
        &enums,
        &env,
        project,
        &consts,
        &const_fns,
    )?;
    let env = TypeEnv {
        assoc: std::rc::Rc::new(assoc_bindings),
        ..env.clone()
    };

    let mut sigs = HashMap::new();
    let mut overloads: HashMap<String, Vec<(Vec<Ty>, String)>> = HashMap::new();
    // Signature key per declaration, in declaration order: once overloads
    // exist a function's key is not derivable from its AST alone, and both
    // the passdown scan and the body checker need the same spelling the call
    // sites will use.
    let mut decl_keys: Vec<String> = Vec::with_capacity(program.functions.len());
    // A-12: one entry per declared half of a key pair, and one per type that
    // declared one, so a type with only half can be named with the span of the
    // half it did declare.
    let mut key_pair_halves: HashMap<String, KeyPairHalf> = HashMap::new();
    let mut key_pair_types: Vec<String> = Vec::new();
    for f in &program.functions {
        // A trait (A-12 slice 2) is named in a bound, which is a type
        // position, so it cannot share a name with a declared type.
        if f.receiver.is_none() && traits.contains_key(&f.name) {
            return Err(Diagnostic::new(
                format!("'{}' is used for both a function and a trait", f.name),
                f.span,
                f.module.clone(),
            ));
        }
        // A receiver method's key already can't collide with a struct name
        // (it always contains `#`), so this check only applies to plain
        // functions.
        if f.receiver.is_none() && structs.contains_key(&f.name) {
            return Err(Diagnostic::new(
                format!("'{}' is used for both a function and a datac", f.name),
                f.span,
                f.module.clone(),
            ));
        }
        // A declared key pair is a declaration on the concrete type, and it is
        // a shape the map calls directly rather than instantiating, so both of
        // these are refused before the generic-template skip below, which
        // would otherwise drop the declaration without a word. The rest of the
        // signature is checked once its types are resolved.
        if let Some(r) = &f.receiver {
            let half = f.name == crate::types::KEY_PAIR_HASH_METHOD
                || f.name == crate::types::KEY_PAIR_EQ_METHOD;
            if half && !r.type_args.is_empty() {
                return Err(Diagnostic::new(
                    format!(
                        "'{}.{}' cannot have type parameters: a key pair is declared on the type itself, so write it on '{}' rather than on the generic template",
                        r.type_name, f.name, r.type_name
                    ),
                    f.span,
                    f.module.clone(),
                ));
            }
            if f.name == crate::types::DROP_METHOD && !r.type_args.is_empty() {
                return Err(Diagnostic::new(
                    format!(
                        "'{}.drop' cannot have type parameters: a destructor is declared on the type itself, so write it on '{}' rather than on the generic template",
                        r.type_name, r.type_name
                    ),
                    f.span,
                    f.module.clone(),
                ));
            }
            if half && r.mutable {
                return Err(Diagnostic::new(
                    format!(
                        "'{}.{}' cannot take 'mut': a key pair's receiver is a shared borrow, so write 'fun {}.{}()' without 'mut'",
                        r.type_name, f.name, r.type_name, f.name
                    ),
                    f.span,
                    f.module.clone(),
                ));
            }
            // A destructor type is not deep-copyable, and the map clones every
            // key in, so a declared key pair on one would be a second owner of
            // the resource the destructor frees.
            if half && env.declared_drops.contains(&r.type_name) {
                return Err(Diagnostic::new(
                    format!(
                        "'{}.{}' cannot be declared: a type with a destructor is not deep-copyable, and a key pair is cloned into the map, so the resource would be freed twice (kernel stage 9, MK9-S6-M1)",
                        r.type_name, f.name
                    ),
                    f.span,
                    f.module.clone(),
                ));
            }
        }
        // A-2: a generic function has no signature until a call site
        // instantiates it; it is a template, so it gets a declaration key
        // (which the passdown scan needs) and no `sigs` entry.
        if !decl_type_params(f).is_empty() {
            check_no_param_default(f, "a generic function takes no default value yet")?;
            decl_keys.push(sig_key(f));
            continue;
        }
        if f.receiver.is_some() {
            check_no_param_default(f, "a receiver method takes no default value yet")?;
        } else if f.extern_ {
            check_no_param_default(f, "an extern fun takes no default value: its native symbol gets every argument")?;
        } else if matches!(f.body, ast::FunctionBody::Signature) {
            check_no_param_default(f, "a trait method takes no default value")?;
        }
        let defaults: Vec<Option<ast::Expr>> = if f.params.iter().any(|p| p.default.is_some()) {
            f.params.iter().map(|p| p.default.clone()).collect()
        } else {
            Vec::new()
        };
        let prefix = project.prefix_of(&f.module);
        let aliases = project.alias_prefixes_of(&f.module);
        let scope = TypeScope {
            structs: &structs,
            enums: &enums,
            env: env.clone(),
            params: &EMPTY_PARAMS,
            prefix,
            aliases: &aliases,
            later_datacs: None,
            in_progress: None,
            consts: Some(&consts),
            const_fns: Some(&const_fns),
            handles: &env.handles,
        };
        let mut params = Vec::new();
        if let Some(r) = &f.receiver {
            let ty = scope.resolve_receiver(r).map_err(|e| e.in_module(&f.module))?;
            // A-12 slice 3: an impl method on a builtin type is legal exactly
            // when the builtin already carries methods (`builtin_type_name`
            // names it); the `for` type is what `check_trait_impl_names`
            // vetted, so this is the same set. Every other method is still on
            // a declared `datac`/`enum`.
            if struct_name_of(&ty).is_none()
                && !(f.trait_impl.is_some() && builtin_type_name(&ty).is_some())
            {
                return Err(Diagnostic::new(
                    format!(
                        "'{}' is a builtin type - its methods are provided by the compiler and cannot be declared",
                        r.type_name
                    ),
                    f.span,
                    f.module.clone(),
                ));
            }
            params.push(("self".to_string(), ty));
        }
        for p in &f.params {
            let ty = scope.resolve_param(p).map_err(|e| e.in_module(&f.module))?;
            params.push((p.name.clone(), ty));
        }
        // `=> T` never goes through `resolve_param_type`, so a function's
        // return type is always a plain owned `Ty` - never `Ty::Ref`. That's
        // what makes the borrow checker's call-scoped release sound: no
        // surface syntax exists yet for a function to hand a borrow back to
        // its caller, so a borrow can never escape the call frame that took
        // it.
        let ret = match &f.return_type {
            Some(t) => scope.resolve_return(t).map_err(|e| e.in_module(&f.module))?,
            None => Ty::Unit,
        };
        // A-12: `fun T.hash()` and `fun T.eq(other: &T)` on a user type are
        // that type's declared `Map` key pair, so the shape is checked here,
        // where the receiver, the parameters and the return type are already
        // resolved, and the type is recorded so the two halves can be matched
        // up once every declaration has been seen. Only `hash` makes the type
        // a key: a lone `eq` (H-5) is a valid declaration that gives the type
        // `==`/`!=`, so it is validated here but kept out of `key_pair_types`.
        if let Some(r) = &f.receiver {
            if f.name == crate::types::KEY_PAIR_HASH_METHOD
                || f.name == crate::types::KEY_PAIR_EQ_METHOD
            {
                check_key_pair_method(f, &params, &ret)?;
                if f.name == crate::types::KEY_PAIR_HASH_METHOD
                    && !key_pair_types.contains(&r.type_name)
                {
                    key_pair_types.push(r.type_name.clone());
                }
                key_pair_halves.entry(r.type_name.clone()).or_insert(KeyPairHalf {
                    method: if f.name == crate::types::KEY_PAIR_HASH_METHOD {
                        crate::types::KEY_PAIR_HASH_METHOD
                    } else {
                        crate::types::KEY_PAIR_EQ_METHOD
                    },
                    span: f.span,
                    module: f.module.clone(),
                });
            }
        }
        // Kernel stage 9, MK9-S6-M1: `fun mut T.drop()` on a user type is
        // that type's destructor, so its shape is checked here, where the
        // receiver, the parameters and the return type are resolved. The
        // duplicate-key check below is what refuses a second one, since both
        // spellings share the `T#drop` key.
        if f.receiver.is_some() && f.name == crate::types::DROP_METHOD {
            check_drop_method(f, &ret)?;
        }
        // A returned borrow has to have something to borrow from, which the
        // borrow checker enforces per return site (M6.3): either the first
        // reference parameter, or a `str` literal (whose storage is static,
        // which is what `fun classify(n: i32) => &str = "pos"` needs).
        // `borrowck.rs` cannot be stricter here without knowing the body.
        let key = sig_key_for(f, prefix, &params);
        if sigs.contains_key(&key) {
            // A-12 slice 2: when either declaration is an `impl` method, the
            // collision is between a trait method and something the type
            // already has, so the message names the trait instead of the
            // generic "defined more than once".
            let other = program
                .functions
                .iter()
                .find(|o| !std::ptr::eq(*o, f) && sig_key(o) == sig_key(f));
            if let Some(message) = trait_method_collision_message(f, other) {
                return Err(Diagnostic::new(message, f.span, f.module.clone()));
            }
            let message = match &f.receiver {
                Some(r) => format!("method '{}.{}' is defined more than once", r.type_name, f.name),
                None => format!(
                    "function '{}' is defined more than once for the same parameter types",
                    display_callee(&key)
                ),
            };
            return Err(Diagnostic::new(message, f.span, f.module.clone()));
        }
        if f.receiver.is_none()
            && is_shipped_module(&f.module)
            && OVERLOADABLE.contains(&bare_name(&f.name, prefix))
        {
            let types = params.iter().map(|(_, t)| t.clone()).collect();
            overloads
                .entry(sig_key(f))
                .or_default()
                .push((types, key.clone()));
        }
        decl_keys.push(key.clone());
        sigs.insert(key, FnSig { defaults, params, ret });
    }

    // A-12 slice 2: every `impl Name for Type` block, checked against the
    // trait's own signatures now that both sides are resolved.
    check_trait_impl_signatures(
        program,
        &traits,
        &trait_impls,
        &structs,
        &enums,
        &sigs,
        &consts,
        &const_fns,
        &env,
    )?;

    // A-12: a declared `Map` key is both halves. `key_pair_types` holds the
    // types that declare `hash`, so a missing `eq` is refused here and the
    // message names the half that is missing; a half whose own signature was
    // wrong was already refused above. A lone `eq` is not in this list: it is
    // a valid declaration that gives the type `==`/`!=` and no key (H-5).
    for name in &key_pair_types {
        let has = |method: &str| {
            sigs.contains_key(&crate::types::key_pair_method_key(name, method))
        };
        let (hash, eq) = (
            has(crate::types::KEY_PAIR_HASH_METHOD),
            has(crate::types::KEY_PAIR_EQ_METHOD),
        );
        if hash == eq {
            continue;
        }
        let half = &key_pair_halves[name];
        let missing = if hash {
            crate::types::KEY_PAIR_EQ_METHOD
        } else {
            crate::types::KEY_PAIR_HASH_METHOD
        };
        return Err(Diagnostic::new(
            format!(
                "'{name}' declares '{}' but not '{missing}': a declared `Map` key needs both halves, 'fun {name}.hash() => u64' and 'fun {name}.eq(other: &{name}) => bool'",
                half.method
            ),
            half.span,
            half.module.clone(),
        ));
    }

    // A-12: the map keeps its own copy of every key it stores, so a key's type
    // has to be deep-copyable, which a derived key always is
    // (`value_is_keyable` names nothing `value_is_clonable` does not). A
    // declared pair is the escape hatch from the *key* fixpoint and not from
    // this one, so the same requirement is checked here rather than leaving a
    // backend to fail on a clone that was never emitted.
    {
        let instances = env.instances.borrow();
        let clonable = crate::types::clonable_types(
            &type_decls_of(&structs, &enums, &instances),
            &env.declared_drops,
        );
        for name in &key_pair_types {
            if clonable.contains(name) {
                continue;
            }
            let half = &key_pair_halves[name];
            return Err(Diagnostic::new(
                format!(
                    "'{name}' declares its own key pair, but the map clones a key in and '{name}' has no deep copy: a key may not hold a field that cannot be copied, so a `List`, `Map` or `Box` field (or a `datac`/`enum` that holds one) is refused"
                ),
                half.span,
                half.module.clone(),
            ));
        }
    }

    // ---- M5.2: passdowns -------------------------------------------------
    //
    // Order matters and is forced by what depends on what:
    //   1. sigs above are final *without* passdown entries, because a
    //      passdown is handed over as a hidden argument at call sites
    //      (see `PdInfo`) rather than as part of the signature's declared
    //      parameter list.
    //   2. pd types are then resolved (declared ones directly, inferred ones
    //      from a type-only probe of the initializer, with only the
    //      passdowns declared before that point visible).
    //   3. the wiring scan says which functions reference which passdowns.
    //   4. initializers and function bodies are checked for real.
    // Which user types own heap memory, transitively, computed once: the
    // array repeat literal and the drop planner both ask it (A-18).

    let decls = TypeDecls {
        declared_keys,
        declared_drops,
        traits,
        trait_impls,
        env,
        fns,
        structs,
        struct_order,
        const_fns,
        consts,
        enums,
        enum_order,
    };
    Ok((decls, Signatures { sigs, overloads, decl_keys }))
}

/// Refuses a default value on a declaration that takes none (issue #296): the
/// reason is named, and the span is the parameter that has the default.
fn check_no_param_default(f: &ast::FunctionDecl, reason: &str) -> std::result::Result<(), Diagnostic> {
    match f.params.iter().find(|p| p.default.is_some()) {
        None => Ok(()),
        Some(p) => Err(Diagnostic::new(
            format!("parameter '{}' of '{}' has a default value, and {reason}", p.name, f.name),
            f.span,
            f.module.clone(),
        )),
    }
}
