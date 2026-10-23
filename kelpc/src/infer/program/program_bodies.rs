use super::*;

pub(crate) fn finish_program(program: &ast::Program, project: &Project, wiring: &PdWiring, decls: TypeDecls, signatures: Signatures) -> std::result::Result<tir::Program, Diagnostic> {
    let TypeDecls {
        declared_keys,
        declared_drops,
        traits,
        env,
        fns,
        structs,
        struct_order,
        const_fns,
        consts,
        enums,
        enum_order,
        ..
    } = decls;
    let Signatures { sigs, overloads, decl_keys } = signatures;
    let owned_structs = crate::types::owned_types(
        &structs.values().cloned().collect::<Vec<_>>(),
        &enums.values().cloned().collect::<Vec<_>>(),
        &declared_drops,
    );

    // Kernel stage 7: every `static` is resolved and folded before any body or
    // passdown is checked, because both may name one and the backends need the
    // folded initializer.
    let (statics, static_decls) = collect_statics(
        program,
        project,
        &sigs,
        &structs,
        &enums,
        &consts,
        &const_fns,
        &owned_structs,
        &overloads,
        &env,
        &fns,
    )?;

    let all_pds = collect_pds(
        program,
        project,
        &sigs,
        &structs,
        &enums,
        &consts,
        &const_fns,
        &owned_structs,
        &overloads,
        &env,
        &fns,
        &statics,
    )?;
    let wired = scan_all(program, &all_pds, &structs, &decl_keys);

    // Kernel stage 4: a freestanding project has no hosted `main`. Its entry
    // point is the `fun` the manifest's `[target] entry` names, in the entry
    // module, and the same rule the hosted entry has holds for it: a program
    // without one is a compile error rather than an artifact with an entry
    // point nothing points at.
    let entry_key = if project.target().is_freestanding() {
        project.target().entry().unwrap_or("main")
    } else {
        "main"
    };
    if !sigs.contains_key(entry_key) {
        let message = if entry_key == "main" {
            "no 'main' function found".to_string()
        } else {
            format!(
                "no entry function '{entry_key}' found: coral.toml's '[target] entry' names a `fun` with no parameters and no return in the entry module"
            )
        };
        let mut diag = Diagnostic::new(message, Span::new(0, 0), project.entry.clone());
        if entry_key == "main" {
            diag = diag.with_help("add fun main() to the entry file. It takes no arguments and returns nothing.");
        }
        return Err(diag);
    }

    let mut checked_pds = Vec::new();
    let mut initialized: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (decl, pd) in program.pds.iter().zip(all_pds.iter()) {
        let init = check_pd_initializer(
            &decl.init,
            pd,
            &sigs,
            &structs,
            &enums,
            &consts,
            &const_fns,
            &owned_structs,
            &overloads,
            project,
            &all_pds,
            &wired,
            wiring,
            &initialized,
            &env,
            &fns,
            &statics,
        )
        .map_err(|e| e.in_module(&pd.module))?;
        initialized.insert(pd.hidden.clone());
        if decl.thread_local {
            check_thread_local(decl, &pd.value_ty, &init)?;
        }
        checked_pds.push(tir::Pd {
            name: pd.hidden.clone(),
            qualified: pd.qualified.clone(),
            value_ty: pd.value_ty.clone(),
            init,
            module: decl.module.clone(),
            thread_local: decl.thread_local,
        });
    }

    let mut functions = Vec::new();
    for (i, f) in program.functions.iter().enumerate() {
        // A-2: a generic function's body is checked once per instantiation,
        // in the worklist loop below - there is no signature to check it
        // against yet.
        if !decl_type_params(f).is_empty() {
            continue;
        }
        let key = &decl_keys[i];
        let sig = sigs[key].clone();
        let checked = check_function(
            f,
            key,
            &sig,
            &HashMap::new(),
            &sigs,
            &structs,
            &enums,
            &consts,
            &const_fns,
            &owned_structs,
            &overloads,
            project,
            &all_pds,
            &wired,
            wiring,
            &env,
            &fns,
            &statics,
        )
        .map_err(|e| e.in_module(&f.module))?;
        functions.push(checked);
    }

    // A-2: drain the instantiation worklist. Checking an instantiated body can
    // create another instantiation, so this runs until the queue is empty;
    // `MAX_FUNCTION_INSTANTIATIONS` is what turns an unbounded chain into a
    // diagnostic.
    loop {
        let next = fns.instances.borrow_mut().pending.pop_front();
        let Some((key, type_args)) = next else {
            break;
        };
        let Some(tmpl_key) = fns.instances.borrow().origin.get(&key).cloned() else {
            continue;
        };
        let tmpl = fns.templates[&tmpl_key].clone();
        let f = &program.functions[tmpl.ast_index];
        let sig = fns
            .instances
            .borrow()
            .sigs
            .get(&key)
            .cloned()
            .expect("an instantiation is registered before it is queued");
        let bindings: HashMap<String, Ty> = tmpl
            .params
            .iter()
            .cloned()
            .zip(type_args.iter().cloned())
            .collect();
        let mut bindings = bindings;
        bindings.extend(assoc_projection_bindings(
            &env,
            &tmpl.params,
            &tmpl.bounds,
            &tmpl.prefix,
            &tmpl.aliases,
            &type_args,
        ));
        let checked = check_function(
            f,
            &key,
            &sig,
            &bindings,
            &sigs,
            &structs,
            &enums,
            &consts,
            &const_fns,
            &owned_structs,
            &overloads,
            project,
            &all_pds,
            &wired,
            wiring,
            &env,
            &fns,
            &statics,
        )
        .map_err(|e| e.in_module(&f.module))?;
        functions.push(checked);
    }
    // The capability data the removed C backend compiles in: each module's
    // patterns per domain, and which functions are gated (M7). Sorted, because
    // `Project::caps` is a map and a backend that emitted its entries in
    // iteration order would produce different IR for the same sources on two
    // runs (roadmap A-14 slice 1's determinism requirement).
    let mut caps: Vec<tir::CapsGrant> = project
        .caps()
        .iter()
        .flat_map(|(module, domains)| {
            domains.iter().map(|(domain, patterns)| tir::CapsGrant {
                module: module.clone(),
                domain: domain.clone(),
                patterns: patterns.clone(),
            })
        })
        .collect();
    caps.sort_by(|a, b| (&a.module, &a.domain).cmp(&(&b.module, &b.domain)));
    // A-2: the instantiations discovered on demand are part of the program's
    // declarations, and a hand-written type may contain one by value, so the
    // two sets are merged and given one topological order before codegen sees
    // them.
    let mut records: Vec<StructDef> = struct_order
        .iter()
        .map(|name| structs[name].clone())
        .collect();
    let mut sums: Vec<EnumDef> = enum_order.iter().map(|name| enums[name].clone()).collect();
    {
        let st = env.instances.borrow();
        // Sorted by name: the instances live in a map, and a vector in a hash
        // map's iteration order makes the same sources produce different IR on
        // two runs (the type-creation order, and with it every type's printed
        // number, would differ). `order_declarations` assigns each type's
        // `order` from a `(order, name)` sort, so the input order here decides
        // nothing else.
        let mut instances: Vec<StructDef> = st.records.values().cloned().collect();
        instances.sort_by(|a, b| a.name.cmp(&b.name));
        records.extend(instances);
        let mut instance_sums: Vec<EnumDef> = st.sums.values().cloned().collect();
        instance_sums.sort_by(|a, b| a.name.cmp(&b.name));
        sums.extend(instance_sums);
    }
    order_declarations(&mut records, &mut sums);
    // A-4: one function per `fun` literal, synthesized while bodies were
    // checked (a literal inside a literal's body included). They are ordinary
    // functions from here on - borrowck drops their owned parameters and
    // codegen emits them with a prototype like any other.
    functions.extend(fns.instances.borrow_mut().literals.drain(..));
    // Gap 7: the fixed-arity wrappers the program's variadic user externs
    // need. They are ordinary `extern fun`s to every later pass - an LLVM
    // declaration and a C body that forwards to the native symbol - so they
    // join the same list, and `extern_ffi_symbols` below points each one at
    // the extern's own native symbol.
    let variadic_wrappers: Vec<(String, tir::Function)> =
        fns.instances.borrow_mut().variadic_wrappers.drain(..).collect();
    functions.extend(variadic_wrappers.iter().map(|(_, f)| f.clone()));
    // The native symbol each user-declared `extern fun` binds to
    // (MK9-S1-M1, roadmap B-1), keyed by the function's full, prefixed name  - 
    // the removed C backend's `write_extern` reads this instead of `externs.rs` when a
    // function's module has no compiler-supplied body, and writes a
    // direct-call trampoline to the symbol. The map holds the renamed C
    // symbol when the manifest entry is `"kelp_name=c_symbol"`. Only a
    // function whose extern was actually *accepted* on this manifest path
    // reaches this map: the `FunctionBody::Extern` arm above already refused
    // every other extern this pass would otherwise have to re-validate here.
    let mut extern_ffi_symbols = HashMap::new();
    for f in &functions {
        if !f.extern_ {
            continue;
        }
        let bare = bare_name(&f.name, project.prefix_of(&f.module));
        if crate::externs::has_body(&f.module, bare) {
            continue;
        }
        if let Some(symbol) = project.extern_ffi_of(&f.module).and_then(|d| d.symbol_of(bare)) {
            extern_ffi_symbols.insert(f.name.clone(), symbol.to_string());
        }
    }
    // Each wrapper calls the same native symbol as the extern it wraps.
    for (extern_key, wrapper) in &variadic_wrappers {
        if let Some(symbol) = extern_ffi_symbols.get(extern_key) {
            extern_ffi_symbols.insert(wrapper.name.clone(), symbol.clone());
        }
    }

    // The `section("...")` consts, as data globals (kernel stage 4). A const
    // whose section names a place but whose value is not a fixed-size integer
    // array has no data to place, so it is refused here rather than silently
    // dropped; a const with no section is folded and inlined as before.
    let mut const_data: Vec<tir::ConstData> = Vec::new();
    for decl in &program.consts {
        let Some(section) = &decl.section else {
            continue;
        };
        let Some(info) = consts.get(&decl.name) else {
            continue;
        };
        match (&info.value.ty, &info.value.kind) {
            (Ty::Array(_, _), tir::ExprKind::ArrayLit(_)) => const_data.push(tir::ConstData {
                name: decl.name.clone(),
                module: decl.module.clone(),
                ty: info.value.ty.clone(),
                section: section.clone(),
                values: match &info.value.kind {
                    tir::ExprKind::ArrayLit(elems) => elems
                        .iter()
                        .map(|e| match &e.kind {
                            tir::ExprKind::IntLit(v) => *v,
                            _ => 0,
                        })
                        .collect(),
                    _ => Vec::new(),
                },
            }),
            _ => {
                return Err(Diagnostic::new(
                    format!(
                        "'section(\"{section}\")' on const '{}' needs a fixed-size array of integers: that is the one const value a section can place",
                        bare_name(&decl.name, project.prefix_of(&decl.module))
                    ),
                    decl.span,
                    decl.module.clone(),
                ));
            }
        }
    }
    const_data.sort_by(|a, b| a.name.cmp(&b.name));

    // A-12 slice 4: one vtable per `(trait, type)` pair a coercion used, in a
    // stable order so both backends emit the same tables. Each slot names the
    // concrete method's own key and carries its signature, so the thunk a
    // backend generates needs no lookup of its own.
    let mut dyn_pairs: Vec<(String, String)> = env.dyn_pairs.borrow().iter().cloned().collect();
    dyn_pairs.sort();
    let mut dyn_vtables: Vec<tir::DynVtable> = Vec::new();
    for (trait_name, type_name) in dyn_pairs {
        let mut flat = Vec::new();
        flattened_trait_methods(&traits, &trait_name, &mut flat);
        let trait_prefix = traits
            .get(&trait_name)
            .map(|info| info.prefix.clone())
            .unwrap_or_default();
        let mut slots = Vec::new();
        for m in &flat {
            // A builtin `for` type keeps its written name in the impl record
            // (`impl Name for i32` stores `i32`), while `resolve.rs` qualifies
            // the method's receiver with the declaring module's prefix. So the
            // concrete key is the direct one for a `datac`/`enum` and, for a
            // builtin in a module that has a prefix, the prefixed spelling.
            let direct = format!("{type_name}#{}", m.name);
            let prefixed = format!("{trait_prefix}{type_name}#{}", m.name);
            let Some((target, sig)) = sigs
                .get(&direct)
                .map(|s| (direct.clone(), s))
                .or_else(|| sigs.get(&prefixed).map(|s| (prefixed.clone(), s)))
            else {
                // No concrete method for this pair: the impls table said the
                // pair exists, so a missing signature is an internal error.
                return Err(Diagnostic::new(
                    format!(
                        "internal: '&dyn {trait_name}' names method '{}', and no concrete method '{direct}' exists for '{type_name}'",
                        m.name
                    ),
                    m.span,
                    m.module.clone(),
                ));
            };
            slots.push(tir::DynVtableSlot {
                method: m.name.clone(),
                target,
                params: sig
                    .params
                    .iter()
                    .map(|(_, t)| t.clone())
                    .collect(),
                ret: sig.ret.clone(),
            });
        }
        // The concrete type the erased data pointer addresses: the receiver of
        // the concrete method, which is a borrow for every non-`Copy` type and
        // the value itself for a `Copy` builtin (A-12 slice 7). The vtable's
        // drop entry needs the `Ty` rather than the name, because dropping a
        // `Box<dyn Trait>` runs this type's own drop glue.
        let concrete = match slots.first().map(|s| s.params.first()) {
            Some(Some(Ty::Ref { inner, .. })) => inner.as_ref().clone(),
            Some(Some(t)) => t.clone(),
            _ => Ty::Unit,
        };
        dyn_vtables.push(tir::DynVtable {
            trait_name,
            type_name,
            concrete,
            slots,
        });
    }
    let mut dyn_traits: Vec<String> = env.dyn_traits.borrow().iter().cloned().collect();
    dyn_traits.sort();

    // Tidepool: every heap slot the pass recorded, decided now that every
    // declaration's facts are known (`managed::check_heap_uses`).
    if env.managed {
        let uses = env.heap_uses.borrow().clone();
        // Every `(trait, concrete type)` pair a coercion used. A vtable with no
        // slots does not name its concrete type, so that trait stays undecided.
        let coerced: Vec<(String, Option<Ty>)> = dyn_vtables
            .iter()
            .map(|v| {
                let concrete = (!v.slots.is_empty()).then(|| v.concrete.clone());
                (v.trait_name.clone(), concrete)
            })
            .collect();
        managed::check_heap_uses(&uses, &|name: &str| {
            match name.strip_prefix("dyn ") {
                Some(trait_name) => managed::trait_object_facts(trait_name, &coerced),
                None => managed::decl_facts(&records, &sums, &declared_drops, name),
            }
        })?;
    }

    Ok(tir::Program {
        structs: records,
        enums: sums,
        pds: checked_pds,
        functions,
        caps,
        gated: project.gated().clone(),
        declared_keys: (*declared_keys).clone(),
        drop_types: (*declared_drops).clone(),
        extern_ffi_symbols,
        const_data,
        statics: static_decls,
        dyn_vtables,
        dyn_traits,
    })
}
