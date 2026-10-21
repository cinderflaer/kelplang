use super::*;

pub(crate) fn declare_types(program: &ast::Program, project: &Project) -> std::result::Result<TypeDecls, Diagnostic> {
    // A-2: the generic templates are collected before anything is resolved,
    // because a concrete declaration's field may name an instantiation
    // (`val p: Pair<i32, String>`) and the resolution of that field is what
    // creates it.
    let generics = collect_templates(program, project)?;
    // A-12: which user types declare their own `Map` key pair, decided from the
    // whole function list: a `datac` field may name `Map<T, V>` before any
    // signature has been resolved, so the key set has to be known that early.
    // The signature loop below then refuses a half or a malformed pair.
    let declared_keys = std::rc::Rc::new(declared_key_names(program));
    // Kernel stage 9, MK9-S6-M1: which user types declare `fun mut T.drop()`,
    // decided that early for the same reason as `declared_keys`: a container
    // element or `Map` key type may be resolved before any signature is, and
    // the derived-copy fixpoints must already exclude a destructor type.
    let declared_drops = std::rc::Rc::new(declared_drop_names(program));
    // A-12 slice 2: the program's traits, and the (trait, type) pairs its impls
    // declare, collected before any signature is resolved because a bound at a
    // call site is checked against the second of those and the first is what a
    // signature's `Self` is bound through.
    let traits = std::rc::Rc::new(collect_traits(program, project)?);
    let trait_impls = collect_trait_impls(program);
    // Coherence runs before the templates are collected, because two generic
    // impls of one type would otherwise be reported as one function defined twice.
    check_trait_impl_overlap(&traits, &trait_impls)?;
    let mut impls: HashMap<String, std::collections::HashSet<String>> = HashMap::new();
    let mut generic_impls: HashMap<String, Vec<GenericImpl>> = HashMap::new();
    let mut blanket_impls: Vec<BlanketImpl> = Vec::new();
    for imp in &trait_impls {
        if imp.type_params.contains(&imp.type_name) {
            if !imp.projection_bounds.is_empty() || imp.type_param_bounds.iter().flatten().any(|b| !b.bound_eq.is_empty()) {
                return Err(Diagnostic::new(
                    format!(
                        "an equality constraint or a 'where' projection bound on 'impl<...> {} for {}' is not in this slice: a blanket impl takes plain trait bounds, and a generic impl ('impl<T> {} for Type<T>') takes both",
                        imp.trait_name, imp.type_name, imp.trait_name
                    ),
                    imp.impl_span,
                    imp.module.clone(),
                ));
            }
            // `impl<T: Bound> Trait for T`: a blanket impl. It binds no associated
            // type yet, and its trait takes no arguments (the parser refuses those).
            if let Some((name, _, _)) = imp.assoc_types.first() {
                return Err(Diagnostic::new(
                    format!(
                        "'type {name}' in 'impl {} for {}': a blanket impl binds no associated type yet, so it writes the bound and the methods only",
                        imp.trait_name, imp.type_name
                    ),
                    imp.impl_span,
                    imp.module.clone(),
                ));
            }
            let bounds = impl_param_bounds(imp, &traits, project)?
                .into_iter()
                .next()
                .unwrap_or_default();
            blanket_impls.push(BlanketImpl {
                trait_key: imp.trait_instance.clone(),
                trait_name: imp.trait_name.clone(),
                bounds,
                methods: imp.methods.clone(),
                module: imp.module.clone(),
                impl_span: imp.impl_span,
            });
            continue;
        }
        if !imp.type_params.is_empty() {
            let bounds = impl_param_bounds(imp, &traits, project)?;
            let (eqs, projections) = impl_constraints(imp, &traits, project)?;
            generic_impls.entry(imp.trait_instance.clone()).or_default().push(GenericImpl {
                template: imp.type_name.clone(),
                bounds,
                assoc_types: imp.assoc_types.iter().map(|(n, t, _)| (n.clone(), t.clone())).collect(),
                params: imp.type_params.clone(),
                eqs,
                projections,
                prefix: project.prefix_of(&imp.module).to_string(),
                aliases: project.alias_prefixes_of(&imp.module),
            });
            continue;
        }
        impls
            .entry(imp.trait_instance.clone())
            .or_default()
            .insert(imp.type_name.clone());
    }
    check_blanket_cycles(&blanket_impls)?;
    let env = TypeEnv {
        generics: std::rc::Rc::new(generics),
        instances: std::rc::Rc::new(std::cell::RefCell::new(InstanceState::default())),
        declared_keys: declared_keys.clone(),
        declared_drops: declared_drops.clone(),
        managed: project.target().has_shared_pool(),
        heap_uses: std::rc::Rc::new(std::cell::RefCell::new(Vec::new())),
        exported: std::rc::Rc::new(collect_exports(program)),
        // `resolve.rs` has prefixed every handle name, so the set is already
        // canonical. A handle is a type like any other and is known before any
        // body is checked, which is why a module may name another module's
        // handle in a signature as freely as a `datac`.
        handles: std::rc::Rc::new(
            program
                .extern_types
                .iter()
                .map(|e| e.name.clone())
                .collect(),
        ),
        handle_parents: std::rc::Rc::new(build_handle_parents(program, project)?),
        variadic_externs: std::rc::Rc::new(
            program
                .functions
                .iter()
                .filter(|f| f.extern_ && f.variadic)
                .map(|f| (f.name.clone(), f.module.clone()))
                .collect(),
        ),
        traits: traits.clone(),
        impls: std::rc::Rc::new(impls),
        trait_impls: std::rc::Rc::new(trait_impls.clone()),
        dyn_pairs: std::rc::Rc::new(std::cell::RefCell::new(std::collections::HashSet::new())),
        dyn_traits: std::rc::Rc::new(std::cell::RefCell::new(std::collections::HashSet::new())),
        assoc: std::rc::Rc::new(HashMap::new()),
        iter_nexts: std::rc::Rc::new(collect_iter_nexts(program)),
        generic_impls: std::rc::Rc::new(generic_impls),
        blanket_impls: std::rc::Rc::new(blanket_impls),
    };
    // A-2: the generic *function* templates, collected before signatures are
    // built so a call site can instantiate one on demand.
    let fns = FuncEnv {
        templates: std::rc::Rc::new(collect_func_templates(
            program,
            project,
            &env.generics,
        )?),
        instances: std::rc::Rc::new(std::cell::RefCell::new(FunctionState::default())),
    };
    let mut structs = HashMap::new();
    // Structs are resolved in declaration order, which is also the order
    // the removed C backend must emit their C typedefs in: a `datac` may only name
    // `datac`s declared before it (see `TypeScope::later_datacs`), so
    // declaration order is a valid topological order. Emitting them from a
    // `HashMap`'s iteration order instead - which is what this did before -
    // made a struct-embedding-struct program compile or fail *at random*
    // from one run to the next.
    let mut struct_order: Vec<String> = Vec::new();
    // A-1: `datac`s and `enum`s share one declaration order, because either
    // may name the other's earlier-declared half by value. `merged` is that
    // order, and `later_of` answers the "declared after this type" question
    // that turns a forward reference into an explanation instead of
    // "unknown type".
    let mut merged: Vec<(usize, String, bool)> = program
        .datacs
        .iter()
        .map(|d| (d.order, d.name.clone(), false))
        .chain(program.enums.iter().map(|e| (e.order, e.name.clone(), true)))
        .collect();
    merged.sort_by_key(|(order, _, _)| *order);
    let later_of: HashMap<String, std::collections::HashSet<String>> = merged
        .iter()
        .enumerate()
        .map(|(i, (_, name, _))| {
            (
                name.clone(),
                merged[i + 1..].iter().map(|(_, n, _)| n.clone()).collect(),
            )
        })
        .collect();
    // Compile-time constants (M6.3, A-19) are folded before any type is
    // declared, because canonical order puts every `const` before every
    // `datac`/`enum`, so a const's own declared type can only name a builtin,
    // and the struct table it is folded against is empty by construction.
    // Folding this early is what makes a const reachable from a *type*: a
    // `datac` field's array length (`[Entry; PAGE_COUNT]`, A-18) is written in
    // a field list, which is resolved long after a function body is.
    //
    // A fold may call a plain `fun` (A-19), and the table it calls through is
    // the declarations themselves, built here from the AST before any
    // signature exists, so a `const` and a `fun` may reference each other in
    // either order.
    let const_fns = std::rc::Rc::new(collect_const_fns(program, project));
    let consts = collect_consts(program, project, &HashMap::new(), &HashMap::new(), &env, &const_fns)?;

    let mut enums: HashMap<String, EnumDef> = HashMap::new();
    let mut enum_order: Vec<String> = Vec::new();
    // `BlockError` is the built-in error of a manual block (decision 139): one
    // variant, `Full`. It is registered for every program in the same tables a
    // declared enum is, so a block's `Result` names it with no import, and its
    // layout comes from the same enum path codegen already reads.
    {
        let name = block_error_name();
        enums.insert(
            name.clone(),
            EnumDef {
                name: name.clone(),
                variants: vec![crate::types::VariantDef {
                    name: "Full".to_string(),
                    fields: Vec::new(),
                    discriminant: None,
                }],
                repr: Repr::C,
                order: 0,
            },
        );
        enum_order.push(name.clone());
        structs.insert(
            name.clone(),
            StructDef {
                name,
                fields: Vec::new(),
                repr: Repr::C,
                order: 0,
            },
        );
    }
    for (order, name, is_enum) in &merged {
        // A-2: a generic `datac`/`enum` is a *template*, not a concrete type.
        // It gets no entry in either table and no declaration order; it is
        // instantiated on demand by `TypeScope::instantiate`.
        let template_only = if *is_enum {
            program
                .enums
                .iter()
                .any(|e| &e.name == name && !e.type_params.is_empty())
        } else {
            program
                .datacs
                .iter()
                .any(|d| &d.name == name && !d.type_params.is_empty())
        };
        if template_only {
            continue;
        }
        if *is_enum {
            let e = program
                .enums
                .iter()
                .find(|e| &e.name == name)
                .expect("merged order names only declared enums");
            if enums.contains_key(name) {
                return Err(Diagnostic::new(
                    format!("enum '{}' is defined more than once", e.name),
                    e.span,
                    e.module.clone(),
                ));
            }
            if structs.contains_key(name) {
                return Err(Diagnostic::new(
                    format!("'{}' is used for both a datac and an enum", e.name),
                    e.span,
                    e.module.clone(),
                ));
            }
            if traits.contains_key(name) {
                return Err(Diagnostic::new(
                    format!("'{}' is used for both a trait and a datac/enum", e.name),
                    e.span,
                    e.module.clone(),
                ));
            }
            if Ty::from_name(&e.name).is_some() {
                return Err(Diagnostic::new(
                    format!("'{}' is a builtin type and cannot be redeclared as an enum", e.name),
                    e.span,
                    e.module.clone(),
                ));
            }
            // Registered *before* its payloads are resolved, exactly as a
            // `datac` is: that is what lets a payload name the enum being
            // declared behind a `Box`/`List` (the recursive case), and
            // `in_progress` below is what rejects naming it by value.
            structs.insert(
                e.name.clone(),
                StructDef {
                    name: e.name.clone(),
                    fields: Vec::new(),
                    repr: Repr::C,
                    order: *order,
                },
            );
            let prefix = project.prefix_of(&e.module);
            let aliases = project.alias_prefixes_of(&e.module);
            // A payload of the enum being declared, *by value*, is the
            // infinite-size case `TypeScope::in_progress` rejects; behind a
            // `Box` or a `List` it is the recursive type (`Cons` holding
            // `Box<List2>`) the container rules already allow.
            let in_progress = Some(e.name.clone());
            let later_datacs = later_of[name].clone();
            let mut variants = Vec::new();
            for v in &e.variants {
                if variants
                    .iter()
                    .any(|vd: &crate::types::VariantDef| vd.name == v.name)
                {
                    return Err(Diagnostic::new(
                        format!(
                            "variant '{}' is defined more than once in enum '{}'",
                            v.name, e.name
                        ),
                        v.span,
                        e.module.clone(),
                    ));
                }
                let mut fields: Vec<FieldDef> = Vec::new();
                for f in &v.fields {
                    if fields.iter().any(|fd: &FieldDef| fd.name == f.name) {
                        return Err(Diagnostic::new(
                            format!(
                                "payload field '{}' is defined more than once in variant '{}'",
                                f.name, v.name
                            ),
                            f.span,
                            e.module.clone(),
                        ));
                    }
                    let ty = TypeScope {
                        structs: &structs,
                        enums: &enums,
                        env: env.clone(),
                        params: &EMPTY_PARAMS,
                        prefix,
                        aliases: &aliases,
                        later_datacs: Some(&later_datacs),
                        in_progress: in_progress.as_deref(),
                        consts: Some(&consts),
                        const_fns: Some(&const_fns),
                        handles: &env.handles,
                    }
                    .resolve(&f.ty, false)
                    .map_err(|err| err.in_module(&e.module))?;
                    if crate::types::is_borrowed_field_ty(&ty) {
                        return Err(Diagnostic::new(
                            format!(
                                "payload field '{}' is a borrowed type ('{}') - an enum holds owned data only in v1, so a '&str', 'View' or reference payload is not expressible - an owned 'String' is the way to keep text in one",
                                f.name,
                                ty.display()
                            ),
                            f.span,
                            e.module.clone(),
                        ));
                    }
                    fields.push(FieldDef {
                        name: f.name.clone(),
                        ty,
                        mutable: f.mutable,
                    });
                }
                variants.push(crate::types::VariantDef {
                    name: v.name.clone(),
                    fields,
                    discriminant: v.discriminant,
                });
            }
            enum_order.push(e.name.clone());
            let edef = EnumDef {
                name: e.name.clone(),
                variants,
                repr: e.repr.clone().unwrap_or(Repr::C),
                order: *order,
            };
            validate_enum_layout(&edef, e.span, &e.module)?;
            enums.insert(e.name.clone(), edef);
            // The placeholder inserted above stays: an enum name is a type
            // name like any other, so one name-resolution path covers it.
            // Reading a field off an enum is caught explicitly in
            // `check_expr`/`check_call`, where the real message lives.
            continue;
        }
        let d = program
            .datacs
            .iter()
            .find(|d| &d.name == name)
            .expect("merged order names only declared datacs");
        let later_datacs = later_of[name].clone();
        if enums.contains_key(&d.name) {
            return Err(Diagnostic::new(
                format!("'{}' is used for both a datac and an enum", d.name),
                d.span,
                d.module.clone(),
            ));
        }
        if structs.contains_key(&d.name) {
            return Err(Diagnostic::new(
                format!("datac '{}' is defined more than once", d.name),
                d.span,
                d.module.clone(),
            ));
        }
        if traits.contains_key(&d.name) {
            return Err(Diagnostic::new(
                format!("'{}' is used for both a trait and a datac/enum", d.name),
                d.span,
                d.module.clone(),
            ));
        }
        if Ty::from_name(&d.name).is_some() {
            return Err(Diagnostic::new(
                format!("'{}' is a builtin type and cannot be redeclared as a datac", d.name),
                d.span,
                d.module.clone(),
            ));
        }
        let prefix = project.prefix_of(&d.module);
        let aliases = project.alias_prefixes_of(&d.module);
        // A datac is visible *inside itself* (M8): the entry is a placeholder
        // whose fields are filled in below, which is all `resolve` needs to
        // name the type. A field of it by value is then rejected as an
        // infinite size (`TypeScope::in_progress`); behind a `Box` or a
        // `List` it is the recursive type the milestone is for.
        structs.insert(
            d.name.clone(),
            StructDef {
                name: d.name.clone(),
                fields: Vec::new(),
                repr: Repr::C,
                order: *order,
            },
        );
        let in_progress = Some(d.name.clone());
        let mut fields = Vec::new();
        for f in &d.fields {
            if fields.iter().any(|fd: &FieldDef| fd.name == f.name) {
                return Err(Diagnostic::new(
                    format!("field '{}' is defined more than once in datac '{}'", f.name, d.name),
                    f.span,
                    d.module.clone(),
                ));
            }
            let ty = TypeScope {
                structs: &structs,
                enums: &enums,
                env: env.clone(),
                params: &EMPTY_PARAMS,
                prefix,
                aliases: &aliases,
                later_datacs: Some(&later_datacs),
                in_progress: in_progress.as_deref(),
                consts: Some(&consts),
                const_fns: Some(&const_fns),
                handles: &env.handles,
            }
            .resolve(&f.ty, false)
            .map_err(|e| e.in_module(&d.module))?;
            // An owned field (`Bytes`, `String`, or a `datac` holding one) is
            // allowed from M6.2 on: the removed C backend generates a drop function
            // per datac, so dropping a struct recurses into its fields.
            if matches!(ty, Ty::Ref { .. } | Ty::Dyn { .. }) {
                return Err(Diagnostic::new(
                    format!(
                        "field '{}' is a borrowed type ('{}') - a datac holds owned data only in v1, so a '&str', 'View' or reference field is not expressible - an owned 'String' is the way to keep text in one",
                        f.name,
                        ty.display()
                    ),
                    f.span,
                    d.module.clone(),
                ));
            }
            fields.push(FieldDef {
                name: f.name.clone(),
                ty,
                mutable: f.mutable,
            });
        }
        if let Some(Repr::Int(name)) = &d.repr {
            return Err(Diagnostic::new(
                format!(
                    "'repr({name})' names the integer type an enum tag is stored in: a 'datac' takes 'repr(c)', 'repr(packed)' or 'repr(align(N))'"
                ),
                d.span,
                d.module.clone(),
            ));
        }
        struct_order.push(d.name.clone());
        // Replaces the placeholder with the real definition. Order is kept
        // (same key, same position in `struct_order`), so codegen's emission
        // order is unchanged.
        structs.insert(
            d.name.clone(),
            StructDef {
                name: d.name.clone(),
                fields,
                repr: d.repr.clone().unwrap_or(Repr::C),
                order: *order,
            },
        );
    }

    // A-12 slice 2: the name-level half of trait validation, before any
    // signature is built, so a second impl of one trait for one type is
    // reported as a coherence error rather than as the duplicate method the
    // two blocks happen to share.
    check_trait_impl_names(program, &traits, &trait_impls, &env)?;

    // A-12 slice 5: every impl's associated-type bindings, resolved in the
    // impl's own module and checked against the trait's declarations. This runs
    // before any signature is built, because a `Self::Item` in an impl method
    // was already substituted by `resolve.rs` and a bound at a call site needs
    // the table. The result is folded into `env` before anything downstream
    // reads it.

    Ok(TypeDecls {
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
    })
}
