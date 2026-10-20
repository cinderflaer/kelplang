use super::*;

/// Whether the supertrait relation has a cycle, returning one cycle (its names,
/// first repeated once) so the error can name every participant. The relation
/// has to be acyclic or the transitive bound check would not terminate.
pub(crate) fn supertrait_cycle(traits: &HashMap<String, TraitInfo>) -> Option<Vec<String>> {
    pub(crate) fn dfs(
        name: &str,
        traits: &HashMap<String, TraitInfo>,
        state: &mut HashMap<String, u8>,
        path: &mut Vec<String>,
    ) -> Option<Vec<String>> {
        state.insert(name.to_string(), 1);
        path.push(name.to_string());
        for sup in &traits[name].supertraits {
            match state.get(sup).copied().unwrap_or(0) {
                1 => {
                    let start = path.iter().position(|p| p == sup).unwrap_or(0);
                    let mut cycle: Vec<String> = path[start..].to_vec();
                    cycle.push(sup.clone());
                    return Some(cycle);
                }
                0 => {
                    if let Some(c) = dfs(sup, traits, state, path) {
                        return Some(c);
                    }
                }
                _ => {}
            }
        }
        path.pop();
        state.insert(name.to_string(), 2);
        None
    }
    let mut state: HashMap<String, u8> = HashMap::new();
    for name in traits.keys() {
        if state.get(name).copied().unwrap_or(0) == 0 {
            let mut path = Vec::new();
            if let Some(c) = dfs(name, traits, &mut state, &mut path) {
                return Some(c);
            }
        }
    }
    None
}

/// The methods of `name` and its supertraits, supertraits first, recursively in
/// declaration order, then the trait's own methods in source order (A-12 slice 4).
/// This is the vtable's slot order and the order a method call through a `&dyn`
/// resolves in, computed the same way at both sites so the two cannot drift.
pub(crate) fn flattened_trait_methods<'a>(
    traits: &'a HashMap<String, TraitInfo>,
    name: &str,
    out: &mut Vec<&'a ast::FunctionDecl>,
) {
    let Some(info) = traits.get(name) else {
        return;
    };
    for sup in &info.supertraits {
        flattened_trait_methods(traits, sup, out);
    }
    for m in &info.methods {
        out.push(m);
    }
}

/// Whether a written type mentions `Self` anywhere. The object-safety check
/// uses this: a method that returns `Self`, or takes another `Self`, cannot be
/// dispatched through a trait object because the concrete type is not known at
/// the call.
pub(crate) fn type_mentions_self(ty: &ast::TypeExpr) -> bool {
    if ty.name == "Self" {
        return true;
    }
    if ty
        .tuple
        .as_ref()
        .is_some_and(|elems| elems.iter().any(type_mentions_self))
    {
        return true;
    }
    if ty.args.iter().any(type_mentions_self) {
        return true;
    }
    if ty
        .array_elem
        .as_ref()
        .is_some_and(|elem| type_mentions_self(elem))
    {
        return true;
    }
    if let Some(params) = &ty.fn_params {
        if params.iter().any(|p| type_mentions_self(&p.ty)) {
            return true;
        }
    }
    if ty
        .fn_ret
        .as_ref()
        .is_some_and(|ret| type_mentions_self(ret))
    {
        return true;
    }
    false
}

/// `name` and every supertrait reachable from it, in source order, without
/// repeats. A projection through a subtrait names a supertrait's associated
/// type, exactly as a bound on the subtrait satisfies a bound on the supertrait
/// (A-12 slices 3 and 5).
pub(crate) fn trait_keys_with_supertraits(env: &TypeEnv, name: &str) -> Vec<String> {
    pub(crate) fn go(env: &TypeEnv, name: &str, out: &mut Vec<String>) {
        if out.iter().any(|k| k == name) {
            return;
        }
        out.push(name.to_string());
        if let Some(info) = env.traits.get(name) {
            for sup in &info.supertraits {
                go(env, sup, out);
            }
        }
    }
    let mut out = Vec::new();
    go(env, name, &mut out);
    out
}

/// The first associated type `name` or one of its supertraits declares, if any
/// (A-12 slice 5). A trait that has one cannot be a trait object.
pub(crate) fn first_assoc_type<'a>(
    traits: &'a HashMap<String, TraitInfo>,
    name: &str,
) -> Option<&'a str> {
    let info = traits.get(name)?;
    if let Some(a) = info.assoc_types.first() {
        return Some(a.name.as_str());
    }
    for sup in &info.supertraits {
        if let Some(found) = first_assoc_type(traits, sup) {
            return Some(found);
        }
    }
    None
}

/// Object safety for a trait object, `&dyn Name` (A-12 slice 4,
/// `design/traits.md (kelp wiki)` 9.2) or `Box<dyn Name>` (slice 7): no method may return
/// `Self`, no parameter other than the receiver may mention `Self`, and no two
/// methods of the flattened list may share a name (the vtable holds one slot per
/// name). A violation names the spelling the program wrote, the trait and the
/// method, because the same rule now guards two type spellings.
pub(crate) fn check_dyn_object_safety(
    spelling: &str,
    name: &str,
    span: Span,
    traits: &HashMap<String, TraitInfo>,
) -> Result<()> {
    // A-12 slice 6: a generic trait has no object-safe form, because the
    // vtable has no slot for a type parameter. Refused by name here, where the
    // `dyn` type is written.
    if let Some(info) = traits.get(name) {
        if !info.type_params.is_empty() {
            return Err(TypeError {
                help: None,
                message: format!(
                    "'{}' is not allowed: the trait takes type parameters ('{}'), and a trait object has no vtable slot for one; use a bounded type parameter ('<T: {}<...>>') instead",
                    spelling,
                    info.type_params.join(", "),
                    name
                ),
                span,
            });
        }
    }
    // A-12 slice 5: a trait that declares (or inherits) an associated type has
    // no object-safe form, because the erased value carries no single binding
    // to name. Refused by name here, where the `dyn` type is written.
    if let Some(item) = first_assoc_type(traits, name) {
        return Err(TypeError {
            help: None,
            message: format!(
                "'{}' is not allowed: the trait declares the associated type '{}', and a trait object does not know which type an implementor binds; use a bounded type parameter ('<T: {}>') instead",
                spelling, item, name
            ),
            span,
        });
    }
    let mut flat = Vec::new();
    flattened_trait_methods(traits, name, &mut flat);
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for m in &flat {
        // A-12 slice 6: a static method has no receiver for the erased pointer
        // to stand for, so it has no vtable slot.
        if m.receiver.is_none() {
            return Err(TypeError {
                help: None,
                message: format!(
                    "'{}' is not allowed: method '{}' has no receiver ('Self'), so there is no value for the vtable to dispatch on; call it on a concrete type instead",
                    spelling, m.name
                ),
                span,
            });
        }
        if let Some(ret) = &m.return_type {
            if type_mentions_self(ret) {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "'{}' is not allowed: method '{}' returns 'Self', and a trait object does not know the concrete type it would build",
                        spelling, m.name
                    ),
                    span,
                });
            }
        }
        for p in &m.params {
            if type_mentions_self(&p.ty) {
                return Err(TypeError {
                    help: None,
                    message: format!(
                        "'{}' is not allowed: method '{}' takes a 'Self' parameter, which a trait object cannot pass (the vtable erases the concrete type); make the parameter a borrow of a concrete type, or use a generic function instead",
                        spelling, m.name
                    ),
                    span,
                });
            }
        }
        if !seen.insert(m.name.as_str()) {
            return Err(TypeError {
                help: None,
                message: format!(
                    "'{}' is not allowed: two methods are named '{}', and a vtable holds one slot per name",
                    spelling, m.name
                ),
                span,
            });
        }
    }
    Ok(())
}

/// Every `impl Name for Type { ... }` in the program (A-12 slice 2), with both
/// names already canonical: `resolve.rs` qualified them where they were
/// written. The trait has to exist and the type has to be declared; those two
/// answers, plus coherence and the module rule, are checked by
/// `check_trait_impl_names`.
pub(crate) fn collect_trait_impls(program: &ast::Program) -> Vec<TraitImplInfo> {
    program
        .trait_impls
        .iter()
        .map(|i| TraitImplInfo {
            trait_name: i.trait_name.clone(),
            trait_instance: i.trait_instance.clone(),
            trait_arg_names: i.trait_arg_names.clone(),
            type_name: i.type_name.clone(),
            methods: i.methods.clone(),
            assoc_types: i
                .assoc_types
                .iter()
                .map(|a| (a.name.clone(), a.ty.clone(), a.span))
                .collect(),
            module: i.module.clone(),
            impl_span: i.impl_span,
            type_params: i.type_params.clone(),
            type_param_bounds: i.type_param_bounds.clone(),
            projection_bounds: i.projection_bounds.clone(),
        })
        .collect()
}

/// A written name as its canonical key, the way `TypeScope::qualify` does it:
/// a bare name belongs to the current module, and `alias.Item` resolves through
/// that module's `impl` table. `None` when the name's first segment is not an
/// imported alias.
pub(crate) fn qualify_written_name(
    name: &str,
    prefix: &str,
    aliases: &HashMap<String, String>,
) -> Option<String> {
    match name.split_once('.') {
        Some((alias, item)) => aliases.get(alias).map(|p| format!("{p}{item}")),
        None => Some(format!("{prefix}{name}")),
    }
}

/// One declaration's written signature, for a diagnostic: `fun mut
/// Self.scale(f: f64)`. A trait's signature is what an impl has to match, so
/// the error quotes it back the way it was written.
pub(crate) fn render_fn_decl(f: &ast::FunctionDecl) -> String {
    let mut out = String::from("fun ");
    if let Some(r) = &f.receiver {
        if r.mutable {
            out.push_str("mut ");
        }
        out.push_str(&r.type_name);
        if !r.type_args.is_empty() {
            out.push_str(&format!("<{}>", r.type_args.join(", ")));
        }
        out.push('.');
    }
    out.push_str(&f.name);
    out.push('(');
    let parts: Vec<String> = f
        .params
        .iter()
        .map(|p| format!("{}{}: {}", p.mode.keyword(), p.name, p.ty.display()))
        .collect();
    out.push_str(&parts.join(", "));
    out.push(')');
    if let Some(t) = &f.return_type {
        out.push_str(&format!(" => {}", t.display()));
    }
    out
}

/// Resolves one trait method signature with `Self` bound to the implementing
/// type: the receiver, each parameter and the return, through the same
/// resolution a declaration's own signature goes through, so a trait signature
/// and its impl mean exactly the same types.
pub(crate) fn resolve_trait_method_sig(
    trait_info: &TraitInfo,
    m: &ast::FunctionDecl,
    self_ty: &Ty,
    trait_args: &[Ty],
    structs: &HashMap<String, StructDef>,
    enums: &HashMap<String, EnumDef>,
    env: &TypeEnv,
    consts: &HashMap<String, ConstInfo>,
    const_fns: &std::rc::Rc<ConstFns>,
) -> Result<FnSig> {
    let mut params_map: HashMap<String, Ty> = HashMap::new();
    params_map.insert("Self".to_string(), self_ty.clone());
    // A-12 slice 6: the trait's own parameters, bound to the impl's concrete
    // arguments, so `fun from(value: T) => Self` resolves for `From<IoErr>` as
    // `fn(IoErr) -> AppErr`.
    for (param, arg) in trait_info.type_params.iter().zip(trait_args.iter()) {
        params_map.insert(param.clone(), arg.clone());
    }
    // A-12 slice 5: the associated types the implementing type binds, so
    // `Self::Item` in the trait's own signature resolves to the concrete type.
    if let Some(name) = struct_name_of(self_ty)
        .map(|s| s.to_string())
        .or_else(|| builtin_type_name(self_ty).map(|s| s.to_string()))
    {
        for key in trait_keys_with_supertraits(env, &trait_info.name) {
            if let Some(bindings) = env.assoc.get(&(key, name.clone())) {
                for (item, ty) in bindings {
                    params_map
                        .entry(format!("Self::{item}"))
                        .or_insert_with(|| ty.clone());
                }
            }
        }
    }
    let scope = TypeScope {
        structs,
        enums,
        env: env.clone(),
        params: &params_map,
        prefix: &trait_info.prefix,
        aliases: &trait_info.aliases,
        later_datacs: None,
        in_progress: None,
        consts: Some(consts),
        const_fns: Some(const_fns),
        handles: &env.handles,
    };
    let receiver = m.receiver.as_ref();
    let mut params = Vec::new();
    if let Some(receiver) = receiver {
        params.push(("self".to_string(), scope.resolve_receiver(receiver)?));
    }
    for p in &m.params {
        params.push((p.name.clone(), scope.resolve_param(p)?));
    }
    let ret = match &m.return_type {
        Some(t) => scope.resolve_return(t)?,
        None => Ty::Unit,
    };
    Ok(FnSig { params, ret })
}

/// Coherence (MISSING-Ve8Nc step 4): two impls of one trait that could both
/// apply to the same type are an error at the later one, with the earlier one
/// named as a note. Two impls overlap when their type patterns unify, whatever
/// their bounds: a blanket impl overlaps every other impl of its trait, and two
/// impls of one generic type overlap. There is no specialization, so no bound
/// is used to tell two impls apart. Two concrete impls for one type keep their
/// own message in `check_trait_impl_names`.
pub(crate) fn check_trait_impl_overlap(
    traits: &HashMap<String, TraitInfo>,
    impls: &[TraitImplInfo],
) -> std::result::Result<(), Diagnostic> {
    let is_blanket = |i: &TraitImplInfo| i.type_params.contains(&i.type_name);
    let display = |i: &TraitImplInfo| {
        if i.type_params.is_empty() || is_blanket(i) {
            i.type_name.clone()
        } else {
            format!("{}<{}>", i.type_name, i.type_params.join(", "))
        }
    };
    for (j, imp) in impls.iter().enumerate() {
        for earlier in &impls[..j] {
            if earlier.trait_instance != imp.trait_instance {
                continue;
            }
            let blanket = is_blanket(imp) || is_blanket(earlier);
            let same_generic = !imp.type_params.is_empty()
                && !earlier.type_params.is_empty()
                && imp.type_name == earlier.type_name;
            if !blanket && !same_generic {
                continue;
            }
            let trait_name = traits
                .get(&imp.trait_name)
                .map(|t| t.name.clone())
                .unwrap_or_else(|| imp.trait_name.clone());
            let message = if blanket {
                format!(
                    "'impl {} for {}' overlaps 'impl {} for {}': a blanket impl applies to every type that meets its bound, so it overlaps every other impl of '{}', and there is no specialization to choose between them: a blanket impl has to be the trait's only impl",
                    trait_name,
                    display(imp),
                    trait_name,
                    display(earlier),
                    trait_name
                )
            } else {
                format!(
                    "'impl {} for {}' overlaps 'impl {} for {}': both apply to every '{}', so one type would have two impls of '{}', and there is no specialization to choose between them",
                    trait_name,
                    display(imp),
                    trait_name,
                    display(earlier),
                    imp.type_name,
                    trait_name
                )
            };
            let mut err = Diagnostic::new(message, imp.impl_span, imp.module.clone());
            err.notes.push(crate::diag::DiagnosticNote {
                message: format!("the overlapping impl of '{trait_name}' is written here"),
                span: earlier.impl_span,
                module: earlier.module.clone(),
            });
            return Err(err);
        }
    }
    Ok(())
}

/// The name-level checks on every `impl Name for Type { ... }` block (A-12
/// slice 2): the trait exists, the type is a declared `datac`/`enum`, the impl
/// is in one of the two modules, and no second impl of the same trait for the
/// same type exists.
///
/// Runs before the signature loop, so a second impl is reported as the
/// coherence error it is rather than as the duplicate method the two blocks
/// happen to share.
pub(crate) fn check_trait_impl_names(
    program: &ast::Program,
    traits: &HashMap<String, TraitInfo>,
    impls: &[TraitImplInfo],
    env: &TypeEnv,
) -> std::result::Result<(), Diagnostic> {
    // The module each declared type belongs to, for the "the impl lives in the
    // trait's module or the type's" rule.
    let mut type_modules: HashMap<&str, &str> = HashMap::new();
    for d in &program.datacs {
        type_modules.insert(&d.name, &d.module);
    }
    for e in &program.enums {
        type_modules.insert(&e.name, &e.module);
    }
    // The (trait, type) pairs declared anywhere in the program, so a
    // supertrait requirement (A-12 slice 3) is answered once every impl has
    // been seen rather than in source order.
    let declared: std::collections::HashSet<(&str, &str)> = impls
        .iter()
        .map(|i| (i.trait_instance.as_str(), i.type_name.as_str()))
        .collect();
    let mut seen: HashMap<(&str, &str), &TraitImplInfo> = HashMap::new();
    for imp in impls {
        let Some(info) = traits.get(&imp.trait_name) else {
            return Err(Diagnostic::new(
                format!(
                    "'impl {} for {}' names no declared trait: declare 'trait {} {{ ... }}' first (or import the module that does)",
                    imp.trait_name, imp.type_name, imp.trait_name
                ),
                imp.impl_span,
                imp.module.clone(),
            ));
        };
        // A-12 slice 6: the impl names the trait instance, so it has to supply
        // exactly the arguments the trait declares, and each one has to satisfy
        // the bound the trait writes on that parameter.
        if info.type_params.is_empty() && !imp.trait_arg_names.is_empty() {
            return Err(Diagnostic::new(
                format!(
                    "trait '{}' takes no type arguments, and 'impl {} for {}' writes {}: write 'impl {} for {}' (an operator trait's right-hand side is always '&Self', so there is no 'Add<Other>')",
                    info.name,
                    imp.trait_name,
                    imp.type_name,
                    imp.trait_arg_names.len(),
                    imp.trait_name,
                    imp.type_name
                ),
                imp.impl_span,
                imp.module.clone(),
            ));
        }
        if imp.trait_arg_names.len() != info.type_params.len() {
            return Err(Diagnostic::new(
                format!(
                    "trait '{}' takes {} type argument(s), and 'impl {} for {}' writes {}: write 'impl {}<{}> for {}' to name the instance",
                    info.name,
                    info.type_params.len(),
                    imp.trait_name,
                    imp.type_name,
                    imp.trait_arg_names.len(),
                    imp.trait_name,
                    info.type_params.join(", "),
                    imp.type_name
                ),
                imp.impl_span,
                imp.module.clone(),
            ));
        }
        for (i, bounds) in info.type_param_bounds.iter().enumerate() {
            let Some(arg) = imp.trait_arg_names.get(i) else {
                continue;
            };
            for (written, key) in bounds {
                let name = Ty::from_name(arg)
                    .and_then(|t| builtin_type_name(&t).map(|s| s.to_string()))
                    .unwrap_or_else(|| arg.clone());
                if !implements_trait(env, key, &name) {
                    return Err(Diagnostic::new(
                        format!(
                            "'{}' does not satisfy the bound '{written}' on parameter '{}' of '{}': write 'impl {written} for {arg} {{ ... }}' in the trait's module or in the type's",
                            arg, info.type_params[i], info.name
                        ),
                        imp.impl_span,
                        imp.module.clone(),
                    ));
                }
            }
        }
        // A builtin `for` type (A-12 slice 3) has no module of its own, so the
        // impl must sit in the trait's module (the orphan rule); a declared
        // `datac`/`enum` may be implemented in either owner's module.
        let type_module: &str = if imp.type_params.contains(&imp.type_name) {
            // A blanket impl names no type of its own, so the orphan rule leaves
            // it the trait's module alone.
            if imp.module != info.module {
                return Err(Diagnostic::new(
                    format!(
                        "'impl {} for {}' is written in module '{}', but a blanket impl names no type of its own, so it lives in the trait's module ('{}'): write it there",
                        imp.trait_name, imp.type_name, imp.module, info.module
                    ),
                    imp.impl_span,
                    imp.module.clone(),
                ));
            }
            info.module.as_str()
        } else if let Some(_builtin) = builtin_for_type_name(&imp.type_name) {
            if imp.module != info.module {
                return Err(Diagnostic::new(
                    format!(
                        "'impl {} for {}' is written in module '{}', but '{}' is a builtin type, which has no module: implement a trait for a builtin in the trait's own module ('{}')",
                        imp.trait_name, imp.type_name, imp.module, imp.type_name, info.module
                    ),
                    imp.impl_span,
                    imp.module.clone(),
                ));
            }
            info.module.as_str()
        } else if let Some(module) = type_modules.get(imp.type_name.as_str()).copied() {
            module
        } else if crate::types::Ty::from_name(&imp.type_name).is_some() {
            return Err(Diagnostic::new(
                format!(
                    "'{}' is a builtin type that carries no methods, so 'impl {} for {}' has nothing to add: implement the trait for a 'datac'/'enum', or for a builtin whose methods the compiler already provides ('i32', 'String', ...)",
                    imp.type_name, imp.trait_name, imp.type_name
                ),
                imp.impl_span,
                imp.module.clone(),
            ));
        } else {
            return Err(Diagnostic::new(
                format!(
                    "'{}' is not a declared 'datac' or 'enum', so 'impl {} for {}' has nothing to implement",
                    imp.type_name, imp.trait_name, imp.type_name
                ),
                imp.impl_span,
                imp.module.clone(),
            ));
        };
        if imp.module != info.module && imp.module != type_module {
            return Err(Diagnostic::new(
                format!(
                    "'impl {} for {}' is written in module '{}', which owns neither: an impl lives in the trait's module ('{}') or in the type's ('{}')",
                    imp.trait_name, imp.type_name, imp.module, info.module, type_module
                ),
                imp.impl_span,
                imp.module.clone(),
            ));
        }
        // Reaching across a module boundary is the ordinary visibility rule
        // (A-13): a trait or a type an impl names from another module has to be
        // exported by the module that declares it.
        for (name, owner, what) in [
            (&imp.trait_name, info.module.as_str(), "trait"),
            (&imp.type_name, type_module, "type"),
        ] {
            if owner != imp.module && !exported(name, env) {
                return Err(Diagnostic::new(
                    format!(
                        "'impl {} for {}' names the {what} '{name}', which module '{owner}' does not export: add 'pub' to its declaration, or write the impl in '{owner}'",
                        imp.trait_name, imp.type_name
                    ),
                    imp.impl_span,
                    imp.module.clone(),
                ));
            }
        }
        // A-12 slice 3: implementing a trait means implementing its
        // supertraits too, because a bound on the supertrait is what the
        // callers of the subtrait's methods rely on.
        for sup in &info.supertraits {
            if !declared.contains(&(sup.as_str(), imp.type_name.as_str())) {
                let display = traits.get(sup).map(|t| t.name.as_str()).unwrap_or(sup);
                return Err(Diagnostic::new(
                    format!(
                        "'impl {} for {}' is missing its supertrait: '{}' needs 'impl {} for {}' as well, because trait '{}' inherits from '{}'",
                        imp.trait_name, imp.type_name, imp.trait_name, display, imp.type_name, info.name, display
                    ),
                    imp.impl_span,
                    imp.module.clone(),
                ));
            }
        }
        if let Some(first) = seen.insert((&imp.trait_instance, &imp.type_name), imp) {
            let mut err = Diagnostic::new(
                format!(
                    "trait '{}' already has an impl for '{}' (written in module '{}'): a trait is implemented for a type at most once",
                    info.name, imp.type_name, first.module
                ),
                imp.impl_span,
                imp.module.clone(),
            );
            err.notes.push(crate::diag::DiagnosticNote {
                message: format!("the other impl of '{}' for '{}' is written here", info.name, first.type_name),
                span: first.impl_span,
                module: first.module.clone(),
            });
            return Err(err);
        }
    }
    Ok(())
}

/// The concrete type each `impl` binds for every associated type its trait
/// declares (A-12 slice 5), with the shape checks. The binding is resolved in
/// the impl's own module, exactly as an impl method's signature is, and each
/// bound the trait declares is checked against the bound type here, so a
/// generic caller can rely on it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn collect_assoc_bindings(
    traits: &HashMap<String, TraitInfo>,
    impls: &[TraitImplInfo],
    structs: &HashMap<String, StructDef>,
    enums: &HashMap<String, EnumDef>,
    env: &TypeEnv,
    project: &Project,
    consts: &HashMap<String, ConstInfo>,
    const_fns: &std::rc::Rc<ConstFns>,
) -> std::result::Result<HashMap<(String, String), HashMap<String, Ty>>, Diagnostic> {
    let mut out: HashMap<(String, String), HashMap<String, Ty>> = HashMap::new();
    for imp in impls {
        let info = &traits[&imp.trait_name];
        if !imp.type_params.is_empty() {
            // A generic impl binds its associated types over its parameters, so
            // they are resolved per instance (`resolve_impl_assoc`); here only
            // that every declared one is bound.
            for declared in &info.assoc_types {
                if !imp.assoc_types.iter().any(|(name, _, _)| *name == declared.name) {
                    return Err(Diagnostic::new(
                        format!(
                            "trait '{}' declares 'type {}', and 'impl {} for {}' does not bind it: add 'type {} = <type>' to the block",
                            info.name, declared.name, imp.trait_name, imp.type_name, declared.name
                        ),
                        imp.impl_span,
                        imp.module.clone(),
                    ));
                }
            }
            continue;
        }
        let prefix = project.prefix_of(&imp.module);
        let aliases = project.alias_prefixes_of(&imp.module);
        let scope = TypeScope {
            structs,
            enums,
            env: env.clone(),
            params: &EMPTY_PARAMS,
            prefix,
            aliases: &aliases,
            later_datacs: None,
            in_progress: None,
            consts: Some(consts),
            const_fns: Some(const_fns),
            handles: &env.handles,
        };
        let mut bound: HashMap<String, Ty> = HashMap::new();
        for declared in &info.assoc_types {
            let Some((_, written, span)) =
                imp.assoc_types.iter().find(|(name, _, _)| *name == declared.name)
            else {
                return Err(Diagnostic::new(
                    format!(
                        "trait '{}' declares 'type {}', and 'impl {} for {}' does not bind it: add 'type {} = <concrete type>' to the block",
                        info.name, declared.name, imp.trait_name, imp.type_name, declared.name
                    ),
                    imp.impl_span,
                    imp.module.clone(),
                ));
            };
            let ty = scope
                .resolve(written, false)
                .map_err(|e| e.in_module(&imp.module))?;
            let name = struct_name_of(&ty)
                .map(|s| s.to_string())
                .or_else(|| builtin_type_name(&ty).map(|s| s.to_string()));
            for (written_bound, key) in &declared.bounds {
                let implements = name
                    .as_deref()
                    .map(|n| implements_trait(env, key, n))
                    .unwrap_or(false);
                if !implements {
                    return Err(Diagnostic::new(
                        format!(
                            "'type {} = {}' in 'impl {} for {}' does not satisfy the bound 'type {}: {}': '{}' does not implement '{}', which the trait requires of every binding",
                            declared.name,
                            ty.display(),
                            imp.trait_name,
                            imp.type_name,
                            declared.name,
                            written_bound,
                            ty.display(),
                            written_bound
                        ),
                        *span,
                        imp.module.clone(),
                    ));
                }
            }
            bound.insert(declared.name.clone(), ty);
        }
        for (name, _, span) in &imp.assoc_types {
            if !info.assoc_types.iter().any(|a| a.name == *name) {
                return Err(Diagnostic::new(
                    format!(
                        "'type {name}' is not declared by trait '{}': an 'impl' binds exactly the trait's associated types, so remove it or add 'type {name}' to the trait",
                        info.name
                    ),
                    *span,
                    imp.module.clone(),
                ));
            }
        }
        out.insert((imp.trait_instance.clone(), imp.type_name.clone()), bound);
    }
    Ok(out)
}

/// A builtin type a trait may be implemented for (A-12 slice 3): one the
/// compiler already gives methods to, which is exactly the set `builtin_type_name`
/// names. `None` for a `datac`/`enum` name, a name that resolves to nothing, or
/// a builtin with no method surface (`bool`, `char`, `f32`, `f64`, `unit`).
pub(crate) fn builtin_for_type_name(name: &str) -> Option<Ty> {
    let ty = crate::types::Ty::from_name(name)?;
    builtin_type_name(&ty).map(|_| ty)
}

/// Checks that every `impl Name for Type { ... }` block implements exactly the
/// signatures its trait declares (A-12 slice 2). Runs after the signature loop,
/// because the impl's methods have to be resolved before they can be compared,
/// and it is the only place that knows both resolved sides.
#[allow(clippy::too_many_arguments)]
pub(crate) fn check_trait_impl_signatures(
    program: &ast::Program,
    traits: &HashMap<String, TraitInfo>,
    impls: &[TraitImplInfo],
    structs: &HashMap<String, StructDef>,
    enums: &HashMap<String, EnumDef>,
    sigs: &HashMap<String, FnSig>,
    consts: &HashMap<String, ConstInfo>,
    const_fns: &std::rc::Rc<ConstFns>,
    env: &TypeEnv,
) -> std::result::Result<(), Diagnostic> {
    // Key -> declaration, so a mismatch can quote the impl back the way it was
    // written rather than describing a resolved `FnSig`.
    let decl_by_key: HashMap<String, &ast::FunctionDecl> = program
        .functions
        .iter()
        .map(|f| (sig_key(f), f))
        .collect();
    for imp in impls {
        let info = &traits[&imp.trait_name];
        if !imp.type_params.is_empty() {
            // A blanket impl takes no trait default yet: every method it has
            // with a body is written in the block (the defaults are not copied).
            if imp.type_params.contains(&imp.type_name) {
                for m in &info.methods {
                    if !matches!(m.body, ast::FunctionBody::Signature) && !imp.methods.contains(&m.name) {
                        return Err(Diagnostic::new(
                            format!(
                                "trait '{}' gives '{}' a default body, and 'impl {} for {}' uses that default: a blanket impl writes every method itself (a default in a blanket impl is not supported yet)",
                                info.name, m.name, imp.trait_name, imp.type_name
                            ),
                            imp.impl_span,
                            imp.module.clone(),
                        ));
                    }
                }
            }
            // Template methods have no signature until an instance is made, so
            // a generic impl is checked where it is used; here only that it
            // writes every method the trait has no body for.
            for m in &info.methods {
                let needs_body = matches!(m.body, ast::FunctionBody::Signature);
                if needs_body && !imp.methods.contains(&m.name) {
                    return Err(Diagnostic::new(
                        format!(
                            "trait '{}' declares '{}', and 'impl {} for {}' does not implement it: a generic impl writes every method (it cannot use the trait's default)",
                            info.name,
                            render_fn_decl(m),
                            imp.trait_name,
                            imp.type_name,
                        ),
                        imp.impl_span,
                        imp.module.clone(),
                    ));
                }
            }
            continue;
        }
        let self_ty = builtin_for_type_name(&imp.type_name)
            .unwrap_or_else(|| Ty::Struct(imp.type_name.clone()));
        let trait_args = trait_arg_tys(&imp.trait_arg_names);
        for m in &info.methods {
            let key = impl_method_key(imp, m);
            let Some(impl_sig) = sigs.get(&key) else {
                return Err(Diagnostic::new(
                    format!(
                        "trait '{}' declares '{}', and 'impl {} for {}' does not implement it: add '{} {{ ... }}' to the block",
                        info.name,
                        render_fn_decl(m),
                        imp.trait_name,
                        imp.type_name,
                        render_fn_decl(m)
                    ),
                    imp.impl_span,
                    imp.module.clone(),
                ));
            };
            let declared = resolve_trait_method_sig(
                info,
                m,
                &self_ty,
                &trait_args,
                structs,
                enums,
                env,
                consts,
                const_fns,
            )
            .map_err(|e| e.in_module(&info.module))?;
            let impl_decl = decl_by_key.get(&key).copied();
            let receiver_mut = |f: &ast::FunctionDecl| {
                f.receiver.as_ref().map(|r| r.mutable).unwrap_or(false)
            };
            if let Some(d) = impl_decl {
                if receiver_mut(d) != receiver_mut(m) {
                    return Err(Diagnostic::new(
                        format!(
                            "method '{}' takes its receiver differently than trait '{}' declares it: the trait writes '{}', the impl writes '{}'",
                            m.name,
                            info.name,
                            render_fn_decl(m),
                            render_fn_decl(d)
                        ),
                        d.span,
                        d.module.clone(),
                    ));
                }
            }
            if declared.params != impl_sig.params || declared.ret != impl_sig.ret {
                let written = impl_decl
                    .map(render_fn_decl)
                    .unwrap_or_else(|| key.clone());
                return Err(Diagnostic::new(
                    format!(
                        "method '{}' does not match trait '{}': the trait declares '{}', the impl writes '{}'",
                        m.name,
                        info.name,
                        render_fn_decl(m),
                        written
                    ),
                    impl_decl.map(|d| d.span).unwrap_or(imp.impl_span),
                    impl_decl
                        .map(|d| d.module.clone())
                        .unwrap_or_else(|| imp.module.clone()),
                ));
            }
        }
        for name in &imp.methods {
            if !info.methods.iter().any(|m| &m.name == name) {
                return Err(Diagnostic::new(
                    format!(
                        "'{}' is not declared by trait '{}': an 'impl' implements exactly the trait's methods, so remove it or add the signature to the trait",
                        name, info.name
                    ),
                    imp.impl_span,
                    imp.module.clone(),
                ));
            }
        }
    }
    Ok(())
}
