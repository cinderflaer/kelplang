use super::*;

/// Loads the entry file, follows every `impl` transitively, rejects import
/// cycles and bad paths, and returns the merged program plus the module
/// tables `infer.rs` needs.
///
/// `entry_text` overrides the entry module's text with a buffer instead of
/// what is on disk at `entry_path` (`kelpc check --text`, MISSING-b6Ytr): the
/// path is still resolved and canonicalized normally, so module identity, the
/// `src/` layout rule and every other module's disk read are unaffected, and
/// only the one module named by `entry_path` reads from the override.
pub fn load(
    entry_path: &Path,
    sources: &mut SourceMap,
    entry_text: Option<String>,
    mut timings: Option<&mut Timings>,
) -> Result<Loaded, Diagnostic> {
    let given = fs::canonicalize(entry_path).map_err(|e| {
        Diagnostic::new(
            format!("cannot read '{}': {e}", entry_path.display()),
            Span::new(0, 0),
            "",
        )
    })?;

    let (
        entry_file,
        root,
        src_dir,
        source_name,
        source_version,
        http_pool,
        manifest_caps,
        manifest_extern,
        manifest_unsafe,
        manifest_target,
        manifest_incremental,
        manifest_sanitize,
        manifest_emit,
        manifest_obfuscate,
        manifest_alias,
        manifest_alias_source,
        manifest_dependencies,
    ) = if given.is_dir() {
            let manifest_path = given.join(MANIFEST_NAME);
            let text = fs::read_to_string(&manifest_path).map_err(|_| {
                Diagnostic::new(
                    format!(
                        "'{}' is a directory but has no {} (pass a .{} file, or add a manifest)",
                        given.display(),
                        MANIFEST_NAME,
                        EXT
                    ),
                    Span::new(0, 0),
                    "",
                )
            })?;
            let manifest = parse_manifest(&text, &manifest_path)?;
            // `[build] emit = "html"` compiles `[build] emit_entry` (a file under
            // `src/`) in place of the manifest's entry, so the HTML program is a
            // separate module graph and its grants are its own.
            let entry_file = match (manifest.emit, &manifest.emit_entry) {
                (Emit::Html, Some(emit_entry)) => {
                    let path = given.join("src").join(emit_entry);
                    fs::canonicalize(&path).map_err(|e| {
                        Diagnostic::new(
                            format!(
                                "cannot read the [build] emit_entry point '{emit_entry}' ({}): {e}",
                                path.display()
                            ),
                            Span::new(0, 0),
                            "",
                        )
                    })?
                }
                _ => fs::canonicalize(given.join(&manifest.entry)).map_err(|e| {
                    Diagnostic::new(
                        format!(
                            "cannot read the manifest's entry point '{}': {e}",
                            manifest.entry
                        ),
                        Span::new(0, 0),
                        "",
                    )
                })?,
            };
            (
                entry_file,
                Some(given.clone()),
                given.join("src"),
                manifest.name,
                manifest.version,
                manifest.http_pool,
                manifest.caps,
                manifest.extern_modules,
                manifest.unsafe_modules,
                manifest.target,
                manifest.incremental,
                manifest.sanitize,
                manifest.emit,
                manifest.obfuscate,
                manifest.alias,
                manifest.alias_source,
                manifest.dependencies,
            )
        } else {
            match find_manifest_root(&given) {
                Some(root) => {
                    let manifest_path = root.join(MANIFEST_NAME);
                    let text = fs::read_to_string(&manifest_path).unwrap_or_default();
                    let manifest = parse_manifest(&text, &manifest_path)?;
                    let src_dir = root.join("src");
                    if !given.starts_with(&src_dir) {
                        return Err(Diagnostic::new(
                            format!(
                                "entry file '{}' is not inside '{}'",
                                given.display(),
                                src_dir.display()
                            ),
                            Span::new(0, 0),
                            "",
                        ));
                    }
                    (
                        given,
                        Some(root),
                        src_dir,
                        manifest.name,
                        manifest.version,
                        manifest.http_pool,
                        manifest.caps,
                        manifest.extern_modules,
                        manifest.unsafe_modules,
                        manifest.target,
                        manifest.incremental,
                        manifest.sanitize,
                        manifest.emit,
                        manifest.obfuscate,
                        manifest.alias,
                        manifest.alias_source,
                        manifest.dependencies,
                    )
                }
                None => {
                    let dir = given
                        .parent()
                        .map(|p| p.to_path_buf())
                        .unwrap_or_else(|| PathBuf::from("."));
                    (
                        given,
                        None,
                        dir,
                        None,
                        None,
                        crate::stdlib::DEFAULT_HTTP_POOL,
                        None,
                        HashMap::new(),
                        Vec::new(),
                        TargetDecl::hosted(),
                        false,
                        None,
                        Emit::Binary,
                        Obfuscate::Off,
                        Vec::new(),
                        None,
                        Vec::new(),
                    )
                }
            }
        };

    let entry = module_id_of(&entry_file, &src_dir)?;

    let mut loader = Loader {
        src_dir: src_dir.clone(),
        entry: entry.clone(),
        entry_text,
        manifest_alias,
        manifest_alias_source,
        sources,
        http_pool,
        aliases: AliasTable::new(),
        raw: HashMap::new(),
        parsed: HashMap::new(),
        state: HashMap::new(),
        stack: Vec::new(),
        modules: Vec::new(),
        prefixes: HashMap::new(),
        grants: Vec::new(),
        all_grants: Vec::new(),
        deprecations: Vec::new(),
        extern_modules: manifest_extern,
        unsafe_modules: manifest_unsafe,
    };
    let parse_start = Instant::now();
    loader.load_project()?;
    if let Some(sink) = &mut timings {
        sink.parse = parse_start.elapsed();
    }
    let resolve_start = Instant::now();

    let mut program = ast::Program {
        impls: Vec::new(),
        consts: Vec::new(),
        statics: Vec::new(),
        pds: Vec::new(),
        datacs: Vec::new(),
        enums: Vec::new(),
        extern_types: Vec::new(),
        traits: Vec::new(),
        trait_impls: Vec::new(),
        functions: Vec::new(),
        script: None,
    };
    // `alias -> the target module's symbol prefix`, per module. Built before
    // the merge loop because an `impl` line's two names are qualified with it
    // (A-12 slice 2): the trait and the `for` type may each be this module's
    // own or reached through an alias, and the canonical key is what every
    // later pass looks a trait up by.
    let mut alias_prefixes = HashMap::new();
    for m in &loader.modules {
        let mut table = HashMap::new();
        for (alias, target) in &m.aliases {
            let prefix = loader.prefixes.get(target).cloned().unwrap_or_default();
            table.insert(alias.clone(), prefix);
        }
        alias_prefixes.insert(m.id.clone(), table);
    }

    for m in &loader.modules {
        let parsed = &loader.parsed[&m.id];
        let module_alias_prefixes = alias_prefixes.get(&m.id).cloned().unwrap_or_default();
        // A written type name as its canonical, module-qualified key: a bare
        // name is this module's own, and `alias.Item` goes through the import
        // table: exactly the rule `TypeScope::qualify` applies, but at the
        // point the name was written.
        let canon = |name: &str, span: Span| -> Result<String, Diagnostic> {
            match name.split_once('.') {
                Some((alias, item)) => match module_alias_prefixes.get(alias) {
                    Some(p) => Ok(format!("{p}{item}")),
                    None => Err(Diagnostic::new(
                        format!("unknown module alias '{alias}' in type '{name}': was it imported?"),
                        span,
                        m.id.clone(),
                    )),
                },
                None => Ok(format!("{}{}", m.prefix, name)),
            }
        };
        // `unsafe` is auditable, not ambient (kernel-path rule 1): a module may
        // contain an `unsafe` block or an `unsafe fun` only if `coral.toml`
        // lists it, the same shape `[extern.*]` takes. A project with no
        // `[unsafe]` section lists nothing, so it cannot contain `unsafe` at
        // all and behaves exactly as before this stage. A module *shipped with
        // the compiler* is admitted by the compiler's own short list instead
        // (`stdlib::UNSAFE_MODULES`, kernel stage 13): no project should have to
        // write `"std/arena"` to import it, and only the compiler's source can
        // add that entry, so a dependency still cannot grant itself the power.
        if let Some((span, _)) = ast::first_unsafe(parsed) {
            if !loader.unsafe_modules.contains(&m.id)
                && !crate::stdlib::may_use_unsafe(&m.id)
            {
                return Err(Diagnostic::new(
                    format!(
                        "module '{}' is not listed in coral.toml [unsafe] modules",
                        m.id
                    ),
                    span,
                    m.id.clone(),
                ));
            }
        }
        for c in &parsed.consts {
            let mut c = c.clone();
            c.name = format!("{}{}", m.prefix, c.name);
            c.module = m.id.clone();
            program.consts.push(c);
        }
        for s in &parsed.statics {
            let mut s = s.clone();
            s.name = format!("{}{}", m.prefix, s.name);
            s.module = m.id.clone();
            program.statics.push(s);
        }
        for p in &parsed.pds {
            let mut p = p.clone();
            p.name = format!("{}{}", m.prefix, p.name);
            p.module = m.id.clone();
            program.pds.push(p);
        }
        for d in &parsed.datacs {
            let mut d = d.clone();
            d.name = format!("{}{}", m.prefix, d.name);
            d.module = m.id.clone();
            program.datacs.push(d);
        }
        for e in &parsed.enums {
            let mut e = e.clone();
            // The enum's name is prefixed like a `datac`'s, and each variant
            // is addressed as `Enum.Variant` at the surface - the same
            // `Type.member` shape a method call has, which is what makes
            // construction parse without a new expression form (A-1).
            e.name = format!("{}{}", m.prefix, e.name);
            e.module = m.id.clone();
            program.enums.push(e);
        }
        // An opaque handle is a module's own type, prefixed and exported like
        // a `datac`. It is legal only where a native surface exists to reach a
        // symbol with: a project's own `[extern.*]` section, or a module the
        // compiler ships (`stdlib::source`), whose extern bodies come from
        // `externs.rs` and need no manifest. The refusal is here, next to the
        // name resolution that would otherwise let it cross modules.
        for e in &parsed.extern_types {
            if !loader.extern_modules.contains_key(&m.id)
                && crate::stdlib::source(&m.id).is_none()
            {
                return Err(Diagnostic::new(
                    format!(
                        "'extern type {}' is only legal in a module with a coral.toml '[extern.\"{}\"]' section",
                        e.name, m.id
                    ),
                    e.span,
                    m.id.clone(),
                ));
            }
            let mut e = e.clone();
            e.name = format!("{}{}", m.prefix, e.name);
            e.module = m.id.clone();
            program.extern_types.push(e);
        }
        for t in &parsed.traits {
            let mut t = t.clone();
            t.name = format!("{}{}", m.prefix, t.name);
            t.module = m.id.clone();
            program.traits.push(t);
        }
        for i in &parsed.trait_impls {
            let mut i = i.clone();
            i.trait_name = canon(&i.trait_name, i.trait_span)?;
            // A builtin `for` type has no module to be prefixed with, and
            // `Ty::from_name` is exactly the set of builtin names (A-12 slice
            // 3); a declared `datac`/`enum` is qualified the usual way.
            // A blanket impl's `for` type is its own parameter (`impl<T> Tr for
            // T`), which names no declared type, so it is kept as written.
            i.type_name = if i.type_params.contains(&i.type_name)
                || crate::types::Ty::from_name(&i.type_name).is_some()
            {
                i.type_name.clone()
            } else {
                canon(&i.type_name, i.type_span)?
            };
            // `impl From<IoErr> for AppErr` (A-12 slice 6): each trait argument
            // is a named type or a builtin, canonicalized the same way. The
            // canonical instance key is what the methods below carry, so a
            // static impl method of `From<IoErr>` and one of `From<String>` are
            // two functions rather than a name collision.
            let mut arg_names = Vec::new();
            for a in &i.trait_args {
                arg_names.push(canon_trait_arg(a, &canon, &m.id)?);
            }
            i.trait_instance =
                crate::types::trait_instance_key(&i.trait_name, &arg_names);
            i.trait_arg_names = arg_names;
            i.module = m.id.clone();
            program.trait_impls.push(i);
        }
        for f in &parsed.functions {
            let mut f = f.clone();
            match &mut f.receiver {
                // A method's key is `Type#name`, so the *receiver type* gets
                // the prefix, not the method name. An `impl` block's method is
                // the exception (A-12 slice 2): its receiver was written
                // `Self`, and the `for` type's canonical name is substituted
                // for it below, so prefixing `Self` here would name nothing.
                Some(r) if f.trait_impl.is_none() => {
                    r.type_name = format!("{}{}", m.prefix, r.type_name)
                }
                Some(_) => {}
                // A static member of a trait `impl` (A-12 slice 6) is keyed
                // from its impl site, so its bare name is kept and the
                // instance is what `infer.rs` appends.
                None if f.trait_impl.is_some() => {}
                None => f.name = format!("{}{}", m.prefix, f.name),
            }
            if let Some(mut site) = f.trait_impl.clone() {
                let written_trait = site.trait_name.clone();
                let written_type = site.self_type.clone();
                // A blanket impl's method: its receiver was written `Self` over the
                // impl's own parameter, so `Self` becomes that parameter name.
                let blanket = f
                    .receiver
                    .as_ref()
                    .is_some_and(|r| r.type_args.contains(&written_type));
                let self_type = if blanket {
                    written_type.clone()
                } else {
                    canon(&written_type, f.span)?
                };
                // A-12 slice 6: the canonical trait-instance key, computed from
                // the arguments this method's own `impl` line wrote, so two
                // instantiations of one trait for one type are two functions
                // rather than a name collision.
                let trait_key = canon(&written_trait, f.span)?;
                let mut arg_names = Vec::new();
                for a in &site.trait_args {
                    arg_names.push(canon_trait_arg(a, &canon, &m.id)?);
                }
                // A-12 slice 5: the associated types this block binds, as
                // written, so a `Self::Item` in the method's signature becomes
                // the concrete type before the signature loop resolves it.
                // Matched on the canonical instance (an associated type only
                // exists on an ordinary trait, whose instance is its name).
                let impl_decl = program.trait_impls.iter().find(|i| {
                    i.trait_name == trait_key
                        && i.type_name == self_type
                        && i.trait_arg_names == arg_names
                });
                let assoc = impl_decl
                    .map(|i| assoc_bindings(&i.assoc_types))
                    .unwrap_or_default();
                site.self_type = self_type.clone();
                site.trait_name = trait_key.clone();
                site.trait_instance = crate::types::trait_instance_key(&trait_key, &arg_names);
                f.trait_impl = Some(site);
                substitute_self(&mut f, &self_type, &assoc);
                if blanket {
                    // The receiver names the blanket impl itself, so its methods
                    // key as `impl#Trait#name` and never collide with another
                    // blanket impl's method of the same name.
                    if let Some(r) = &mut f.receiver {
                        r.type_name = crate::types::blanket_receiver_name(&trait_key);
                    }
                }
            }
            f.module = m.id.clone();
            program.functions.push(f);
        }
    }

    // A-12 slice 3: a trait default method. Every `impl Trait for Type` that
    // omits a method the trait gives a body gets a concrete copy of it, with
    // `Self` substituted and tagged with the impl site, so the ordinary
    // signature loop checks and emits it exactly as if the impl had written
    // it. The signature resolves in the impl's module (the impl wrote the
    // `for` type, so it can name it); the body resolves in the trait's module,
    // where it was written, through `body_module`. A builtin's impl is in the
    // trait's module, so the two coincide there.
    let default_sources: Vec<(String, String, String, String, String, Vec<String>, Vec<ast::FunctionDecl>, std::collections::HashMap<String, ast::TypeExpr>, Vec<(String, ast::TypeExpr)>)> = program
        .trait_impls
        .iter()
        .map(|i| {
            let trait_decl = program.traits.iter().find(|t| t.name == i.trait_name);
            let trait_module = trait_decl.map(|t| t.module.clone()).unwrap_or_default();
            // A blanket impl takes no trait default yet (infer.rs refuses it by
            // name), so none is generated for it.
            let blanket = i.type_params.contains(&i.type_name);
            let defaults: Vec<ast::FunctionDecl> = trait_decl
                .filter(|_| !blanket)
                .map(|t| {
                    t.methods
                        .iter()
                        .filter(|m| !matches!(m.body, ast::FunctionBody::Signature))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            // A-12 slice 6: the trait's own parameters, paired with the impl's
            // written arguments, so a synthesized default's signature has its
            // `T` replaced by the concrete type the impl names.
            let params: Vec<(String, ast::TypeExpr)> = trait_decl
                .map(|t| t.type_params.iter().cloned().zip(i.trait_args.iter().cloned()).collect())
                .unwrap_or_default();
            (
                i.trait_name.clone(),
                i.trait_instance.clone(),
                i.type_name.clone(),
                trait_module,
                i.module.clone(),
                i.methods.clone(),
                defaults,
                assoc_bindings(&i.assoc_types),
                params,
            )
        })
        .collect();
    for (trait_name, trait_instance, type_name, trait_module, impl_module, written, defaults, assoc, params) in default_sources {
        for m in defaults {
            if written.contains(&m.name) {
                continue;
            }
            let mut f = m;
            f.trait_impl = Some(ast::TraitImplSite {
                trait_name: trait_name.clone(),
                trait_args: Vec::new(),
                trait_instance: trait_instance.clone(),
                self_type: type_name.clone(),
            });
            substitute_type_params(&mut f, &params);
            substitute_self(&mut f, &type_name, &assoc);
            f.module = impl_module.clone();
            f.body_module = Some(trait_module.clone());
            program.functions.push(f);
        }
    }

    // One merged declaration order across `datac`s and `enum`s (A-1). Each
    // declaration carries its index *within its own file*, and the module
    // list is already in load order, so renumbering here is what gives
    // `types::owned_types` and the removed C backend a single sequence to walk: a
    // field or payload may only name a type declared before it by value, and
    // the two declaration lists are separate vectors.
    let mut merged_order = 0usize;
    for m in &loader.modules {
        let mut local: Vec<(usize, bool, usize)> = Vec::new();
        for (i, d) in program.datacs.iter().enumerate() {
            if d.module == m.id {
                local.push((d.order, false, i));
            }
        }
        for (i, e) in program.enums.iter().enumerate() {
            if e.module == m.id {
                local.push((e.order, true, i));
            }
        }
        local.sort_by_key(|(order, _, _)| *order);
        for (_, is_enum, idx) in local {
            if is_enum {
                program.enums[idx].order = merged_order;
            } else {
                program.datacs[idx].order = merged_order;
            }
            merged_order += 1;
        }
    }

    // ---- M7: capability sets -------------------------------------------
    //
    // A module's set is *received*, never self-declared, with one exception:
    // the entry module's set. By default it still holds everything (`/**`/
    // `*`, every domain), but `coral.toml`'s `[caps]` section (C-2) lets a
    // project restrict it to exactly what is declared there - the natural
    // fix for open item 18, since `main` cannot write a `caps(...)` clause on
    // itself. `manifest_caps` is `None` whenever the section is absent (no
    // manifest at all, or a manifest that does not have one), which is what
    // keeps every `coral.toml` written before this existed behaving exactly
    // as before. Every other module holds exactly what its importers granted
    // it. That makes the no-escalation rule checkable - a module can
    // delegate less than it has, never more - and is what keeps an ungranted
    // `fs.read` a resolution error instead of a runtime check.
    let mut caps: HashMap<String, HashMap<String, Vec<String>>> = HashMap::new();
    let entry_caps = manifest_caps.unwrap_or_else(|| {
        let mut whole = HashMap::new();
        for domain in crate::stdlib::DOMAINS {
            // `mmio` is the one domain the entry module does not hold by
            // default: its patterns are a machine's address ranges rather
            // than a resource the root of trust owns, so a freestanding
            // project has to write `[caps] mmio = [...]` down before any
            // module may build a pointer from an address
            // (`kelp wiki page language/capabilities.md`).
            if *domain == "mmio" {
                continue;
            }
            whole.insert(
                domain.to_string(),
                vec![crate::stdlib::whole_resource(domain).to_string()],
            );
        }
        whole
    });
    caps.insert(entry.clone(), entry_caps);
    for g in &loader.grants {
        let slot = caps
            .entry(g.target.clone())
            .or_default()
            .entry(g.domain.clone())
            .or_default();
        for pattern in &g.patterns {
            if !slot.contains(pattern) {
                slot.push(pattern.clone());
            }
        }
    }
    // `caps(*)` hands the target the same set the entry module holds: the
    // whole resource of every domain. That happens here, before the
    // no-escalation check below, so a module that *received* the wildcard can
    // delegate it onward exactly as it could delegate a pattern it was given.
    for g in &loader.all_grants {
        let slot = caps.entry(g.target.clone()).or_default();
        for domain in crate::stdlib::DOMAINS {
            // `caps(*)` is the whole resource of every domain *except* mmio:
            // the whole address space is the escape hatch a reviewer greps
            // for, so it is never handed over implicitly (`kelp wiki page language/capabilities.md`).
            if *domain == "mmio" {
                continue;
            }
            let wildcard = crate::stdlib::whole_resource(domain).to_string();
            let patterns = slot.entry(domain.to_string()).or_default();
            if !patterns.contains(&wildcard) {
                patterns.push(wildcard);
            }
        }
    }
    for g in &loader.all_grants {
        // A module that declares nothing gated and reaches no module that
        // does is ungated, so there is no domain to be granted: `caps(*)` on
        // it is noise at best and a hint that the wrong module was named at
        // worst. Refusing it by name is the same refusal the ungranted-
        // namespace error is: the clause and the module disagree about what
        // there is to hand over.
        if !reaches_a_domain(&loader.modules, &g.target, &loader.extern_modules) {
            return Err(Diagnostic::new(
                format!(
                    "module '{}' is ungated: it reaches no capability domain, so `caps(*)` would grant it nothing; drop the clause",
                    g.target
                ),
                g.span,
                g.importer.clone(),
            ));
        }
        let held = caps.get(&g.importer).cloned().unwrap_or_default();
        for domain in crate::stdlib::DOMAINS {
            // The wildcard is not handed over for `mmio`, so it is not required
            // of the importer either; see the granting loop above.
            if *domain == "mmio" {
                continue;
            }
            let wildcard = crate::stdlib::whole_resource(domain);
            let holds_it = held
                .get(*domain)
                .map(|patterns| {
                    patterns
                        .iter()
                        .any(|h| pattern_covered(h, wildcard, domain))
                })
                .unwrap_or(false);
            if holds_it {
                continue;
            }
            let shown = held
                .get(*domain)
                .filter(|patterns| !patterns.is_empty())
                .map(|patterns| patterns.join(", "))
                .unwrap_or_else(|| "none".to_string());
            // The wildcard is one clause over every domain, so the message
            // names `caps(*)` and the domain that failed rather than a single
            // pattern the importer never wrote.
            return Err(Diagnostic::new(
                format!(
                    "module '{}' cannot give '{}' the whole-resource wildcard for '{}' with `caps(*)`: it does not hold that wildcard itself (it holds: {}){}; a module can delegate less than it has, never more",
                    g.importer,
                    g.target,
                    domain,
                    shown,
                    if held.get(*domain).map(|p| p.is_empty()).unwrap_or(true) {
                        format!(
                            "; '{}' holds nothing for '{}' because the module that imports it wrote no 'caps({} = [...])' or `caps(*)` grant on that `impl` line",
                            g.importer, domain, domain
                        )
                    } else {
                        String::new()
                    }
                ),
                g.span,
                g.importer.clone(),
            ));
        }
    }
    for g in &loader.grants {
        let held = caps
            .get(&g.importer)
            .and_then(|d| d.get(&g.domain))
            .cloned()
            .unwrap_or_default();
        for pattern in &g.patterns {
            if !held
                .iter()
                .any(|h| pattern_covered(h, pattern, &g.domain))
            {
                // When the importer holds nothing, the message so far names
                // *it* and leaves the reader looking at the wrong module: the
                // line to change is the `impl` in whoever imported this
                // module, which wrote no grant at all. The entry module is the
                // one importer with no such line, and its grant is the
                // manifest's `[caps]` section - the common case for `mmio`,
                // the one domain the entry module does not hold by default.
                let missing = if !held.is_empty() {
                    String::new()
                } else if g.importer == entry {
                    format!(
                        "; the entry module is granted by coral.toml's '[caps]' section, and this project's does not name '{}' (write `[caps] {} = [...]`)",
                        g.domain, g.domain
                    )
                } else {
                    format!(
                        "; '{}' holds nothing here because the module that imports it wrote no 'caps({} = [...])' grant on that `impl` line",
                        g.importer, g.domain
                    )
                };
                return Err(Diagnostic::new(
                    format!(
                        "module '{}' cannot grant '{}' access to '{}': it does not hold that pattern itself (held: {}){}; a module can delegate less than it has, never more",
                        g.importer,
                        g.domain,
                        pattern,
                        if held.is_empty() {
                            "none".to_string()
                        } else {
                            held.join(", ")
                        },
                        missing,
                    ),
                    g.span,
                    g.importer.clone(),
                ));
            }
        }
    }

    // Which of the shipped items are gated: a module that gates a domain
    // contributes **every item it declares**, its `extern fun` primitives and
    // its receiver methods included. The domain gate is what decides whether
    // the name resolves at all, and the resource patterns are the runtime
    // half, so an item that is left out of this table is reachable by any
    // module, granted or not: that was the `*_raw` bypass (`BUG-wQ9tL`) and
    // the reason a receiver method of a gated module reached its syscall with
    // no grant. The declaring module's own calls stay exempt in
    // `infer.rs::check_capability` by prefix, which is what lets a gated
    // module wrap its own primitive. (Overloaded names only exist in
    // `std/math`, which is ungated, so the plain key is enough.)
    let mut gated = HashMap::new();
    // The functions an `unsafe` context is required to call, by canonical key
    // (MK9-S2 stage 1). `infer.rs` consults this at every call site, and the
    // list is exactly the modules the manifest listed under `[unsafe]`,
    // because `resolve.rs` refused any `unsafe` elsewhere above.
    let mut unsafe_fns = std::collections::HashSet::new();
    for m in &loader.modules {
        let parsed = &loader.parsed[&m.id];
        for f in &parsed.functions {
            if f.unsafe_ {
                let key = match &f.receiver {
                    Some(r) => format!("{}{}#{}", m.prefix, r.type_name, f.name),
                    None => format!("{}{}", m.prefix, f.name),
                };
                unsafe_fns.insert(key);
            }
        }
    }
    // The `interrupt fun` handlers (kernel stage 5). `infer` refuses a direct
    // call to one: the CPU enters it, and nothing returns through a call.
    let mut interrupt_fns = std::collections::HashSet::new();
    for m in &loader.modules {
        let parsed = &loader.parsed[&m.id];
        for f in &parsed.functions {
            if f.interrupt {
                let key = match &f.receiver {
                    Some(r) => format!("{}{}#{}", m.prefix, r.type_name, f.name),
                    None => format!("{}{}", m.prefix, f.name),
                };
                interrupt_fns.insert(key);
            }
        }
    }
    let mut must_use_fns = std::collections::HashSet::new();
    for m in &loader.modules {
        let parsed = &loader.parsed[&m.id];
        for f in &parsed.functions {
            if f.must_use {
                let key = match &f.receiver {
                    Some(r) => format!("{}{}#{}", m.prefix, r.type_name, f.name),
                    None => format!("{}{}", m.prefix, f.name),
                };
                must_use_fns.insert(key);
            }
        }
    }
    for m in &loader.modules {
        let Some(domain) = domain_of_with_extern(&m.id, &loader.extern_modules) else {
            continue;
        };
        let parsed = &loader.parsed[&m.id];
        // A stdlib module can gate its items on more than its one domain
        // (`std/os` leaves its target facts ungated), so each item asks
        // `gated_item_domain`. A user `[extern.*]` module has no per-item table,
        // so every item takes the module's domain.
        let stdlib_module = crate::stdlib::domain_of(&m.id).is_some();
        for f in &parsed.functions {
            let item_domain = if stdlib_module {
                crate::stdlib::gated_item_domain(&m.id, &f.name).map(str::to_string)
            } else {
                Some(domain.clone())
            };
            let Some(item_domain) = item_domain else {
                continue;
            };
            // A method's key is `Type#name` and the *receiver type* carries
            // the module prefix (`infer.rs::sig_key`, and the prefixing loop
            // above); a free function's key is its prefixed name.
            let key = match &f.receiver {
                Some(r) => format!("{}{}#{}", m.prefix, r.type_name, f.name),
                None => format!("{}{}", m.prefix, f.name),
            };
            gated.insert(key, item_domain);
        }
    }

    let interfaces = {
        let source_map: &SourceMap = &*loader.sources;
        build_interfaces(&loader.modules, source_map)
    };

    let project = Project {
        root,
        src_dir,
        entry,
        modules: loader.modules,
        name: source_name,
        version: source_version,
        prefixes: loader.prefixes,
        alias_prefixes,
        caps,
        gated,
        extern_modules: loader.extern_modules,
        unsafe_fns,
        must_use_fns,
        interrupt_fns,
        target: manifest_target,
        incremental: manifest_incremental,
        sanitize: manifest_sanitize,
        emit: manifest_emit,
        obfuscate: manifest_obfuscate,
        dependencies: manifest_dependencies,
        grants: loader
            .grants
            .iter()
            .map(|g| GrantRecord {
                importer: g.importer.clone(),
                target: g.target.clone(),
                domain: g.domain.clone(),
                patterns: g.patterns.clone(),
            })
            .collect(),
        wildcard_grants: loader
            .all_grants
            .iter()
            .map(|g| (g.importer.clone(), g.target.clone()))
            .collect(),
    };
    if let Some(sink) = &mut timings {
        sink.resolve = resolve_start.elapsed();
    }
    Ok(Loaded {
        program,
        project,
        deprecations: loader.deprecations,
        interfaces,
    })
}
