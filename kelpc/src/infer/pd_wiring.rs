use super::*;

/// One checked `static` (kernel stage 7).
#[derive(Debug, Clone)]
pub(crate) struct StaticInfo {
    /// The module-prefixed name, which is the emitted symbol.
    pub(crate) qualified: String,
    /// The bare name a function in the declaring module writes.
    pub(crate) name: String,
    pub(crate) ty: Ty,
    /// `static mut NAME` - every access is `unsafe`.
    pub(crate) mutable: bool,
}

/// One checked passdown (M5.2).
#[derive(Debug, Clone)]
pub(crate) struct PdInfo {
    /// Module-prefixed name, for diagnostics.
    pub(crate) qualified: String,
    /// The bare name a function in the declaring module writes.
    pub(crate) name: String,
    /// Internal name shared by the generated C global and the hidden
    /// parameter a referencing function receives. `#` can't appear in a
    /// user identifier, so `pd#x` can never collide with a local of the
    /// same *written* name (which C would otherwise reject as a duplicate
    /// declaration in the function's own scope).
    pub(crate) hidden: String,
    /// The passdown's own type.
    pub(crate) value_ty: Ty,
    /// Whether it was declared `pd var` - used for the wording of the
    /// "read-only inside a function" diagnostic, and to tell a write that
    /// can be given an exclusive borrow from one that can never be allowed.
    pub(crate) mutable: bool,
    /// Whether it was declared `tl`: each thread has its own copy, so a borrow of
    /// it never crosses to a worker (issue 234).
    pub(crate) thread_local: bool,
    pub(crate) module: String,
    /// Global initialization order (position in the merged program), used to
    /// keep hidden-parameter order and the generated init function stable.
    pub(crate) index: usize,
}

impl PdInfo {
    /// The *borrow* of the passdown's storage: what a call site passes for
    /// this passdown's hidden parameter, and what a read dereferences.
    ///
    /// `exclusive` is the referencing function's mode (roadmap item 17): a
    /// function that *writes* the passdown - directly or through a callee -
    /// holds it as an exclusive borrow, and one that only reads it holds a
    /// shared one. A `pd var` that no function writes is therefore a shared
    /// borrow everywhere, so `show(cfg)` does not collide with the
    /// passdown's own hidden argument.
    pub(crate) fn borrow_ty(&self, exclusive: bool) -> Ty {
        Ty::Ref {
            mutable: exclusive,
            inner: Box::new(self.value_ty.clone()),
        }
    }

    /// Two shapes, because the two contexts reach the storage differently:
    /// inside a function the hidden name is already the borrow (`&*hidden`
    /// would be a pointer to a pointer), while `main` and a `pd` initializer
    /// have no hidden parameter and name the generated global directly, so
    /// the borrow is explicit.
    pub(crate) fn arg(&self, span: Span, global: bool, exclusive: bool) -> tir::Expr {
        let ty = self.borrow_ty(exclusive);
        if global {
            let global = tir::Expr {
                kind: tir::ExprKind::Ident(self.hidden.clone()),
                ty: self.value_ty.clone(),
                span,
            };
            return tir::Expr {
                kind: tir::ExprKind::Ref {
                    mutable: exclusive,
                    expr: Box::new(global),
                },
                ty,
                span,
            };
        }
        tir::Expr {
            kind: tir::ExprKind::Ident(self.hidden.clone()),
            ty,
            span,
        }
    }

    /// A read of the passdown (M6.4): `*` of the borrow, so reads and writes
    /// go through the same pointer, and the borrow's mutability is what
    /// decides whether a write through it is allowed.
    pub(crate) fn value(&self, span: Span, global: bool, exclusive: bool) -> tir::Expr {
        tir::Expr {
            kind: tir::ExprKind::Deref {
                expr: Box::new(self.arg(span, global, exclusive)),
            },
            ty: self.value_ty.clone(),
            span,
        }
    }
}

/// What one pass of body checking discovers about passdown writes, so the
/// next pass can hand the passdowns over with the right borrow (roadmap item
/// 17). The probe pass runs with every passdown shared, records the uses that
/// need an exclusive borrow instead of refusing them, and records the call
/// edges; `close_pd_modes` then walks the edges to a fixpoint, and the pass
/// that emits the program runs with the result.
#[derive(Debug, Default)]
pub(crate) struct PdProbe {
    /// `(caller's wiring key, hidden name)` - the caller uses the passdown
    /// exclusively, so it must hold an exclusive borrow of it.
    pub(crate) exclusive_uses: std::collections::HashSet<(String, String)>,
    /// `(caller's wiring key, callee's wiring key)` - the call graph the
    /// closure walks: a function that calls a writer holds the passdown
    /// exclusively too, or it could hand a shared borrow to a callee that
    /// writes through it.
    pub(crate) edges: std::collections::HashSet<(String, String)>,
}

/// The passdown borrow modes a body check runs under.
#[derive(Clone, Default)]
pub(crate) struct PdWiring {
    /// `(wiring key, hidden name) -> the function writes the passdown`. A
    /// missing entry means the shared borrow, which is the common case.
    pub(crate) modes: std::rc::Rc<HashMap<(String, String), bool>>,
    /// `Some` only in the probe pass (see `PdProbe`).
    pub(crate) probe: Option<std::rc::Rc<std::cell::RefCell<PdProbe>>>,
    /// `Some` only in the receiver-mode probe (issue #286): the spans of the
    /// inferable methods whose body writes self.
    pub(crate) writers: Option<std::rc::Rc<std::cell::RefCell<std::collections::HashSet<crate::lexer::Span>>>>,
}

impl PdWiring {
    /// Whether the function filed under `key` holds `pd` exclusively.
    pub(crate) fn exclusive(&self, key: &str, pd: &PdInfo) -> bool {
        self.modes
            .get(&(key.to_string(), pd.hidden.clone()))
            .copied()
            .unwrap_or(false)
    }
}

impl Ctx {
    /// Remembers `done`, the result of checking `expr`, for the `check_call_arg`
    /// that will check the same expression (see `probes`).
    pub(crate) fn keep_probe(&mut self, expr: &ast::Expr, done: tir::Expr) {
        if self.probes.len() >= 4096 {
            self.probes.clear();
        }
        self.probes.push((expr as *const ast::Expr, expr.span(), done));
    }

    pub(crate) fn take_probe(&mut self, expr: &ast::Expr) -> Option<tir::Expr> {
        let at = expr as *const ast::Expr;
        let span = expr.span();
        let i = self.probes.iter().rposition(|(p, s, _)| *p == at && *s == span)?;
        Some(self.probes.swap_remove(i).2)
    }

    /// Records a write to a place rooted at `self` (issue #286), and refuses it
    /// when the method is a datac-block method with a shared receiver, so the
    /// refusal names the `mut` form. In the receiver-mode probe nothing is refused.
    pub(crate) fn note_self_write(&self, place: &tir::Expr) -> Result<()> {
        if crate::borrowck::tir_root_ident(place) != Some("self") {
            return Ok(());
        }
        self.self_written.set(true);
        if self.self_probe {
            return Ok(());
        }
        if let (Some(msg), Some(info)) = (&self.block_note, self.lookup("self")) {
            if matches!(info.ty, Ty::Ref { mutable: false, .. }) {
                return Err(TypeError {
                    help: None,
                    message: msg.clone(),
                    span: place.span,
                });
            }
        }
        Ok(())
    }

    pub(crate) fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    pub(crate) fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    pub(crate) fn define(&mut self, name: &str, ty: Ty, mutable: bool) {
        self.scopes
            .last_mut()
            .unwrap()
            .insert(name.to_string(), VarInfo { ty, mutable });
    }

    pub(crate) fn lookup(&self, name: &str) -> Option<&VarInfo> {
        for scope in self.scopes.iter().rev() {
            if let Some(info) = scope.get(name) {
                return Some(info);
            }
        }
        None
    }

    /// Resolves a written type name to a canonical `Ty::Handle` name, or
    /// `None` when it is not a handle: `Buffer` in the module that declares it,
    /// `mem.Buffer` through an alias anywhere else. This is the same
    /// qualification `TypeScope::resolve_base` gives a handle in a signature.
    pub(crate) fn handle_type_named(&self, name: &str) -> Option<String> {
        let qualified = qualify_written(name, &self.prefix, &self.aliases);
        self.env.handles.contains(&qualified).then_some(qualified)
    }

    /// The gate on `null()`/`is_null()`/`cast()` (gap 1): a handle operation is
    /// legal only in a module with its own `[extern."<module id>"]` section,
    /// even when the handle type itself is imported.
    pub(crate) fn require_extern_section(&self, what: &str, span: Span) -> Result<()> {
        if self.has_extern_section {
            return Ok(());
        }
        Err(TypeError {
            help: None,
            message: format!(
                "'{what}' is only legal in a module with a coral.toml '[extern.\"<module id>\"]' section, because only there can a native handle exist"
            ),
            span,
        })
    }

    /// Rewrites a binding's type in the scope that holds it. Used for one
    /// thing only: an open container whose element type an operation decided
    /// after the binding was declared, so every later statement (and the
    /// emitted local) sees the real type rather than the marker.
    pub(crate) fn set_binding_ty(&mut self, name: &str, ty: Ty) {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(info) = scope.get_mut(name) {
                info.ty = ty;
                return;
            }
        }
    }

    /// Looks up `name` as a field of the current receiver's `datac`, if
    /// this function has one.
    pub(crate) fn receiver_field_ty(&self, name: &str) -> Option<Ty> {
        let sname = self.receiver_struct.as_deref()?;
        self.struct_def(sname)?.field(name).map(|f| f.ty.clone())
    }

    /// Whether `name` is a field of the named `datac` - used only to tell a
    /// literal's diagnostic "this is a field of the receiver you are in, and
    /// reading it by value is a partial move" apart from "undefined name"
    /// (A-4 phase E).
    pub(crate) fn field_of(&self, owner: &str, name: &str) -> Option<Ty> {
        self.struct_def(owner)?.field(name).map(|f| f.ty.clone())
    }

    /// Records `name` as a capture of the `fun` literal being checked and
    /// returns its type, or `None` when the name is not an enclosing binding
    /// (A-4 phase E). One per distinct name, in the order the body first
    /// names them, because that order is the environment's field order: the
    /// body's `ExprKind::Capture` indices and the environment the removed C backend
    /// writes have to agree.
    pub(crate) fn note_capture(&mut self, name: &str, span: Span) -> Option<Ty> {
        let ty = self.outer_bindings.get(name).cloned()?;
        // A scoped literal's environment is stack and owns nothing, so an
        // owned non-`Copy` capture is a *shared borrow* of the enclosing
        // binding rather than a move of it (`kelp wiki page language/functions.md`). The
        // body then reads `&T`, which is what makes a field read and a method
        // call through the capture type-check, and what makes a write through
        // it a borrow error.
        let ty = if self.borrowed_captures
            && (!ty.is_copy() || self.excl_captures.contains(name))
            && !crate::types::is_borrowed_field_ty(&ty)
        {
            Ty::Ref {
                mutable: self.excl_captures.contains(name),
                inner: Box::new(ty),
            }
        } else {
            ty
        };
        // Tidepool: an owned environment is a heap object, so a value
        // captured into it is a heap slot. A scoped literal's environment is
        // the stack, so it is not one.
        if !self.borrowed_captures {
            let slot = if self.spawn_entry {
                managed::HeapSlot::SpawnEntryCapture
            } else {
                managed::HeapSlot::ClosureCapture
            };
            note_heap_use(&self.env, slot, &ty, span);
        }
        if !self.captures.iter().any(|(n, _)| n == name) {
            self.captures.push((name.to_string(), ty.clone()));
        }
        Some(ty)
    }

    /// The visibility gate (A-13): an item another module reaches through an
    /// alias is reachable only when its declaration is `pub` there. `key` is
    /// the canonical lookup key, and `alias.item` is how the reference is
    /// written. Called only after the item is known to exist, so the refusal
    /// always means "private" and never "missing".
    pub(crate) fn check_exported(&self, key: &str, alias: &str, item: &str, span: Span) -> Result<()> {
        if exported(key, &self.env) {
            return Ok(());
        }
        Err(private_item_error(alias, item, span))
    }

    /// The visibility gate for a type path an expression names through an
    /// alias (`shapes.Shape.Empty`, `shapes.Shape.Circle(...)`). A local,
    /// parameter or passdown of the alias's own name shadows it, exactly as
    /// it shadows any other name lookup, so `a.b` stays a field of the value
    /// `a` and is not read as an alias at all.
    pub(crate) fn check_type_path_exported(&self, path: &str, span: Span) -> Result<()> {
        if let Some((alias, _)) = path.split_once('.') {
            if self.lookup(alias).is_some() || self.pds.contains_key(alias) {
                return Ok(());
            }
        }
        check_type_exported(
            path,
            &self.structs,
            &self.enums,
            &self.aliases,
            &self.env,
            span,
        )
    }

    /// The capability gate (M7): a gated module's item is reachable only
    /// from a module that holds the domain. This is a *resolution* error, not
    /// a runtime check - the point of the feature is that code without a
    /// grant has no name to reach the resource with.
    pub(crate) fn check_capability(&self, key: &str, span: Span) -> Result<()> {
        let Some(domain) = self.gated.get(key) else {
            return Ok(());
        };
        // A module's own items are reachable from inside it with no grant:
        // the gate exists to stop *other* modules from naming the resource,
        // and a gated stdlib module has to be able to call its own helpers
        // (`process.run` is `spawn` plus a drain loop, and `std/net` will
        // want the same shape). Nothing outside the compiler can declare a
        // gated item - `stdlib::domain_of` only knows stdlib ids - so this
        // exemption cannot widen a user module's authority.
        let own = !self.prefix.is_empty() && key.starts_with(self.prefix.as_str());
        if self.caps.contains_key(domain) {
            // Recorded for `kelpc caps` and the `[dependencies] caps` check. The
            // gate's answer is the same with or without the record.
            if !own {
                record_cap_use(&self.module, domain, key, span);
            }
            return Ok(());
        }
        if own {
            return Ok(());
        }
        Err(TypeError {
            help: None,
            message: format!(
                "'{}' needs the '{}' capability, which module '{}' was not granted; the module that imports this one must delegate it (`impl \"<this module>\" caps({} = [\"<pattern>\"])`)",
                display_callee(key),
                domain,
                self.module,
                domain
            ),
            span,
        })
    }

    /// `resolve.rs` stores a non-entry module's top-level names with their
    /// module prefix, but the plan's rule is that a file's own items are
    /// reachable *bare* inside that file - so a bare lookup has to try the
    /// prefixed spelling too. The entry module's prefix is empty, so this is
    /// the identity there.
    pub(crate) fn qualify_own(&self, name: &str) -> String {
        if self.prefix.is_empty() {
            name.to_string()
        } else {
            format!("{}{name}", self.prefix)
        }
    }

    pub(crate) fn type_scope(&self) -> TypeScope<'_> {
        TypeScope {
            structs: &self.structs,
            enums: &self.enums,
            env: self.env.clone(),
            params: &self.type_args,
            prefix: &self.prefix,
            aliases: &self.aliases,
            // Every datac is known by the time bodies are checked.
            later_datacs: None,
            in_progress: None,
            consts: Some(&self.consts),
            const_fns: Some(&self.const_fns),
            handles: &self.env.handles,
        }
    }

    /// A function's checked signature, whether it was declared by hand or is
    /// an instantiation of a generic template (A-2).
    pub(crate) fn sig_of(&self, key: &str) -> Option<FnSig> {
        if let Some(s) = self.sigs.get(key) {
            return Some(s.clone());
        }
        self.fns.instances.borrow().sigs.get(key).cloned()
    }

    /// The passdowns a callee needs, looking through an instantiation to the
    /// template it came from: the wiring scan runs on declarations, so its
    /// table is keyed by the declaration, not by each instance.
    pub(crate) fn wired_for(&self, key: &str) -> Option<Vec<PdInfo>> {
        if let Some(w) = self.wired.get(key) {
            return Some(w.clone());
        }
        let tmpl = self.fns.instances.borrow().origin.get(key).cloned()?;
        self.wired.get(&tmpl).cloned()
    }

    /// The key the passdown wiring table is filed under: an instantiation's
    /// wiring is its template's (A-2), and so is its borrow mode - every use
    /// site of one instance is `wired_for`-ed through the same entry.
    pub(crate) fn wiring_key_of(&self, key: &str) -> String {
        if self.wired.contains_key(key) {
            return key.to_string();
        }
        self.fns
            .instances
            .borrow()
            .origin
            .get(key)
            .cloned()
            .unwrap_or_else(|| key.to_string())
    }

    /// Whether the body filed under `key` holds `pd` exclusively (roadmap
    /// item 17).
    pub(crate) fn pd_exclusive_for(&self, key: &str, pd: &PdInfo) -> bool {
        self.pd_modes
            .get(&(key.to_string(), pd.hidden.clone()))
            .copied()
            .unwrap_or(false)
    }

    /// Whether this body holds `pd` exclusively: it writes it, or it calls a
    /// function that does (roadmap item 17). Everything else is a shared
    /// borrow, so a read-only reference never blocks another one.
    pub(crate) fn pd_exclusive(&self, pd: &PdInfo) -> bool {
        self.pd_exclusive_for(&self.fn_key, pd)
    }

    /// A use that needs an exclusive borrow. In the probe pass it is what the
    /// next pass is computed from, so it is recorded and allowed; in the pass
    /// that emits the program the modes already say the answer, and reaching
    /// here means the closure missed the use - say so instead of emitting an
    /// unchecked write.
    pub(crate) fn note_pd_exclusive_use(&self, pd: &PdInfo) -> bool {
        let Some(probe) = &self.pd_probe else {
            return false;
        };
        probe
            .borrow_mut()
            .exclusive_uses
            .insert((self.fn_key.clone(), pd.hidden.clone()));
        true
    }

    /// The definition of a user type, whether it was declared by hand or is
    /// an instantiation of a generic template (A-2). Every lookup that used
    /// to read `self.structs` goes through here, so an instance key
    /// (`Pair<i32, String>`) behaves exactly like a declared name.
    pub(crate) fn struct_def(&self, name: &str) -> Option<StructDef> {
        if let Some(s) = self.structs.get(name) {
            return Some(s.clone());
        }
        self.env.instances.borrow().records.get(name).cloned()
    }

    /// As `struct_def`, for the `enum` table.
    pub(crate) fn enum_def(&self, name: &str) -> Option<EnumDef> {
        if let Some(e) = self.enums.get(name) {
            return Some(e.clone());
        }
        self.env.instances.borrow().sums.get(name).cloned()
    }

    /// Whether `name` is an `enum` (declared or instantiated).
    pub(crate) fn is_enum(&self, name: &str) -> bool {
        self.enums.contains_key(name) || self.env.instances.borrow().sums.contains_key(name)
    }

    /// Whether `ty` is an `enum`, which an `enum` is a plain `Struct` element of
    /// (so `is_map_element` takes it), but a `Map` value may not be (`map_enum_value_error`).
    pub(crate) fn is_enum_ty(&self, ty: &Ty) -> bool {
        matches!(ty, Ty::Struct(name) if self.is_enum(name))
    }

    /// Every `datac`/`enum` declaration the checker knows, generic instances
    /// included, as `(name, each field's type)` - what the derived
    /// clone/order/key fixpoints (`types::clonable_types`) run over.
    pub(crate) fn type_decls(&self) -> Vec<(String, Vec<Ty>)> {
        type_decls_of(&self.structs, &self.enums, &self.env.instances.borrow())
    }

    /// Whether `List<T>.clone`/`.slice` can be offered for a value of `ty`:
    /// a scalar, `String`/`Bytes`, or a `datac`/`enum` all of whose fields can
    /// be deep-copied.
    pub(crate) fn value_clonable(&self, ty: &Ty) -> bool {
        let set = crate::types::clonable_types(&self.type_decls(), &self.env.declared_drops);
        crate::types::value_is_clonable(ty, &set)
    }

    /// Whether `List<T>.sort` can be offered for a value of `ty`, by the same
    /// declaration-graph rule.
    pub(crate) fn value_sortable(&self, ty: &Ty) -> bool {
        let set = crate::types::sortable_types(&self.type_decls());
        crate::types::value_is_sortable(ty, &set)
    }

    /// The user types that are `Map` keys, by the same declaration-graph rule
    /// (A-6), plus the types that declare their own pair (A-12).
    pub(crate) fn keyable_set(&self) -> std::collections::HashSet<String> {
        crate::types::map_key_types(
            &self.type_decls(),
            &self.env.declared_keys,
            &self.env.declared_drops,
        )
    }

    /// Instantiates a generic template for one use site (A-2). Idempotent: a
    /// key that already exists is returned as-is, which is also what makes a
    /// recursive instantiation terminate.
    pub(crate) fn instantiate_type(&self, name: &str, args: &[Ty], span: Span) -> Result<Ty> {
        self.type_scope().instantiate(name, args, span)
    }

    /// Whether a passdown referenced from here is already initialized. True
    /// inside any function body (`init_state` is `None` there) and for
    /// anything declared before the initializer currently being checked.
    pub(crate) fn pd_initialized(&self, hidden: &str) -> bool {
        match &self.init_state {
            None => true,
            Some(done) => done.contains(hidden),
        }
    }
}

/// Every `datac`/`enum` declaration as `(name, each field's type)`, generic
/// instances included: what the derived clone, order and map-key fixpoints run
/// over. A name that is an `enum` also sits in `structs` as an empty record
/// (one name-resolution path covers both kinds), so the enum table's entry is
/// the one to keep.
pub(crate) fn type_decls_of(
    structs: &HashMap<String, StructDef>,
    enums: &HashMap<String, EnumDef>,
    instances: &InstanceState,
) -> Vec<(String, Vec<Ty>)> {
    let mut out: Vec<(String, Vec<Ty>)> = Vec::new();
    let mut push = |name: &str, fields: Vec<Ty>| out.push((name.to_string(), fields));
    for (name, s) in structs {
        if enums.contains_key(name) {
            continue;
        }
        push(name, s.fields.iter().map(|f| f.ty.clone()).collect());
    }
    let payloads = |e: &EnumDef| -> Vec<Ty> {
        e.variants
            .iter()
            .flat_map(|v| v.fields.iter().map(|f| f.ty.clone()))
            .collect()
    };
    for (name, e) in enums {
        push(name, payloads(e));
    }
    for (name, s) in &instances.records {
        push(name, s.fields.iter().map(|f| f.ty.clone()).collect());
    }
    for (name, e) in &instances.sums {
        push(name, payloads(e));
    }
    out
}

/// Everything needed to turn a *written* type name into a `Ty`, with M5's
/// module qualification rules:
/// - a builtin (`i32`, `f64`, `str`, ...) is itself;
/// - `alias.Item` resolves through the current module's `impl` table;
/// - a bare name belongs to the current module, so it gets that module's
///   prefix - imported types are never reachable unqualified (strict
///   module-qualified namespace, per the plan).
pub(crate) struct TypeScope<'a> {
    pub(crate) structs: &'a HashMap<String, StructDef>,
    /// The declared `enum`s, which share one declaration order with the
    /// `datac`s above: a `Map` key's keyability recurses through an enum's
    /// variant payloads, and an enum is not in `structs` with them.
    pub(crate) enums: &'a HashMap<String, EnumDef>,
    /// Generic templates and the instances built so far (A-2). Shared, and
    /// mutable through a `RefCell`, because resolving `Pair<i32, String>`
    /// *creates* the instantiation it names - including from inside another
    /// instantiation's fields.
    pub(crate) env: TypeEnv,
    /// The type parameters in scope, bound to concrete arguments: non-empty
    /// only while a generic template's own fields are being resolved. A bare
    /// name found here wins over every other name, so `datac Pair<A, B>`'s
    /// `A` is the argument, never a type that happens to share the name.
    pub(crate) params: &'a HashMap<String, Ty>,
    pub(crate) prefix: &'a str,
    pub(crate) aliases: &'a HashMap<String, String>,
    /// Qualified names of `datac`s in this module that are declared *after*
    /// the declaration currently being checked. Used only to turn "unknown
    /// type 'Inner'" into an explanation: a `datac` may only name `datac`s
    /// declared before it (the checker builds its struct table in
    /// declaration order, so forward references were never resolvable).
    /// `None` at the sites where every `datac` in the program is already
    /// known (signatures, bodies, passdown types).
    pub(crate) later_datacs: Option<&'a std::collections::HashSet<String>>,
    /// Qualified name of the `datac` whose fields are being resolved right
    /// now, if any. A field of that type *by value* is rejected (a
    /// self-referential struct has no finite size); through a container it
    /// is allowed, which is how `datac Node(val next: Box<Node>)` works.
    pub(crate) in_progress: Option<&'a str>,
    /// The folded `const`s of the whole program, and the functions a compile
    /// time fold may call (A-19). Both are here for one reason: a type may
    /// carry an expression, and the only expression a type carries is an
    /// array's `[T; N]` length (A-18). `None` at the few sites where no
    /// constant is in reach, which turns a length that needs one into a
    /// diagnostic instead of a wrong answer.
    pub(crate) consts: Option<&'a HashMap<String, ConstInfo>>,
    pub(crate) const_fns: Option<&'a ConstFns>,
    /// The canonical names of the program's opaque handles (gap 1), so a
    /// written `Widget` or `ffi.Widget` resolves to `Ty::Handle` instead of
    /// falling through to "unknown type".
    pub(crate) handles: &'a std::collections::HashSet<String>,
}

/// The user types that declare their own `Map` key (A-12): the receiver name of
/// every `fun T.hash()` declaration in the program.
///
/// `hash` is the half that makes the type a key, which is what makes a declared
/// key possible with no trait and no new syntax: the pair is the two method
/// names. The signature loop in `check_program_pass` then insists the
/// declaration is one the map can actually call, so a `hash` with no `eq` or a
/// malformed one is an error at the declaration rather than a `Map` use site
/// that can only say the type is not a key. A lone `eq` (H-5) is deliberately
/// *not* here: it gives the type `==`/`!=` without making it a key.
pub(crate) fn declared_key_names(program: &ast::Program) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    for f in &program.functions {
        if let Some(r) = &f.receiver {
            if f.name == crate::types::KEY_PAIR_HASH_METHOD {
                out.insert(r.type_name.clone());
            }
        }
    }
    out
}

/// The user types that declare their own destructor (kernel stage 9,
/// MK9-S6-M1): the receiver name of every `fun mut T.drop()` in the program.
///
/// Like a declared key pair, the method name *is* the declaration and there is
/// no trait and no new syntax; this is the one set both backends and
/// `types::owned_types` read to know which types get a user call at the top of
/// their generated `kelp_drop_<T>`.
pub(crate) fn declared_drop_names(program: &ast::Program) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    for f in &program.functions {
        if let Some(r) = &f.receiver {
            if f.name == crate::types::DROP_METHOD {
                out.insert(r.type_name.clone());
            }
        }
    }
    out
}

/// The message for two methods that share one `Type#name` key where at least
/// one of them came from an `impl` block (A-12 slice 2): a trait method and an
/// explicit method of the type, or two traits' methods, cannot both answer a
/// call of that name. `None` when neither declaration is a trait impl, which
/// leaves the ordinary "defined more than once" message to the caller.
pub(crate) fn trait_method_collision_message(
    a: &ast::FunctionDecl,
    b: Option<&ast::FunctionDecl>,
) -> Option<String> {
    let site_a = a.trait_impl.as_ref();
    let site_b = b.and_then(|o| o.trait_impl.as_ref());
    if site_a.is_none() && site_b.is_none() {
        return None;
    }
    let owner = a
        .receiver
        .as_ref()
        .map(|r| r.type_name.clone())
        .unwrap_or_default();
    let method = &a.name;
    let message = match (site_a, site_b) {
        (Some(a), Some(b)) => format!(
            "traits '{}' and '{}' both declare '{}.{method}': one type cannot answer a call of that name through two traits, so rename the method in one of them",
            a.trait_name, b.trait_name, owner
        ),
        (Some(a), None) => format!(
            "'impl {} for {}' declares '{}.{method}', which '{}' already has as an explicit method: a trait method and an explicit method cannot share a name, so rename one of them",
            a.trait_name, a.self_type, owner, owner
        ),
        (None, Some(b)) => format!(
            "'impl {} for {}' declares '{}.{method}', which '{}' already has as an explicit method: a trait method and an explicit method cannot share a name, so rename one of them",
            b.trait_name, b.self_type, owner, owner
        ),
        (None, None) => return None,
    };
    Some(message)
}

/// Whether one declared destructor is the shape the compiler calls: an
/// exclusive (`mut`) receiver, no parameters, and no return value. Checked at
/// the declaration, where the receiver, the parameters and the return type are
/// already resolved, so a malformed destructor is an error at its own line
/// rather than a call the generated drop cannot make.
pub(crate) fn check_drop_method(
    f: &ast::FunctionDecl,
    ret: &Ty,
) -> std::result::Result<(), Diagnostic> {
    let receiver = f
        .receiver
        .as_ref()
        .expect("only a receiver method is a destructor");
    let owner = &receiver.type_name;
    let err = |message: String| Diagnostic::new(message, f.span, f.module.clone());
    if !receiver.mutable {
        return Err(err(format!(
            "'{owner}.drop' must take its receiver with 'mut': the drop body reads and may adjust the value it is running for, so write 'fun mut {owner}.drop()'"
        )));
    }
    if !f.params.is_empty() {
        return Err(err(format!(
            "'{owner}.drop' takes no parameters: the compiler calls it with the value being dropped, so write 'fun mut {owner}.drop()'"
        )));
    }
    if *ret != Ty::Unit {
        return Err(err(format!(
            "'{owner}.drop' must not return a value: a drop runs where the value dies and has nowhere to hand a result, so write 'fun mut {owner}.drop()'"
        )));
    }
    Ok(())
}

/// Every `trait Name { ... }` in the program, keyed by qualified name (A-12
/// slice 2). The signatures are kept as written, because `Self` is only bound
/// when a type implements the trait.
///
/// The shape checks that need no resolved type are here: a method name is
/// declared once, and `drop` is not a trait method at all (a destructor is a
/// property of the type, written `fun mut T.drop()` in its own module).
pub(crate) fn collect_traits(
    program: &ast::Program,
    project: &Project,
) -> std::result::Result<HashMap<String, TraitInfo>, Diagnostic> {
    let mut out: HashMap<String, TraitInfo> = HashMap::new();
    for t in &program.traits {
        let mut names: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for m in &t.methods {
            if m.name == crate::types::DROP_METHOD {
                return Err(Diagnostic::new(
                    format!(
                        "a trait method cannot be named 'drop': a destructor belongs to the type and is written 'fun mut {}.drop()' in the type's own module, not through a trait",
                        m.receiver.as_ref().map(|r| r.type_name.as_str()).unwrap_or("T")
                    ),
                    m.span,
                    t.module.clone(),
                ));
            }
            if m.variadic {
                return Err(Diagnostic::new(
                    format!(
                        "trait '{}' method '{}' writes '...', which only a user 'extern fun' may write: a trait signature declares the parameters an impl has to take",
                        t.name, m.name
                    ),
                    m.span,
                    t.module.clone(),
                ));
            }
            if !names.insert(&m.name) {
                return Err(Diagnostic::new(
                    format!("trait '{}' declares method '{}' more than once", t.name, m.name),
                    m.span,
                    t.module.clone(),
                ));
            }
        }
        if !t.type_params.is_empty() && !t.assoc_types.is_empty() {
            return Err(Diagnostic::new(
                format!(
                    "trait '{}' declares both type parameters and an associated type, which is not in this slice: a generic trait has no single associated-type binding to name",
                    t.name
                ),
                t.span,
                t.module.clone(),
            ));
        }
        if out.contains_key(&t.name) {
            return Err(Diagnostic::new(
                format!("trait '{}' is defined more than once", t.name),
                t.span,
                t.module.clone(),
            ));
        }
        out.insert(
            t.name.clone(),
            TraitInfo {
                name: t.name.clone(),
                type_params: t.type_params.clone(),
                type_param_bounds: Vec::new(),
                prefix: project.prefix_of(&t.module).to_string(),
                aliases: project.alias_prefixes_of(&t.module),
                methods: t.methods.clone(),
                supertraits: Vec::new(),
                assoc_types: Vec::new(),
                module: t.module.clone(),
            },
        );
    }
    // A-12 slice 3: a supertrait is resolved in the trait's own module, once
    // every trait is known, so `trait B: A` may name an `A` declared later in
    // load order. It has to name a declared trait, and the relation has to be
    // acyclic.
    for t in &program.traits {
        let mut canonical = Vec::new();
        // A-12 slice 5: the associated types and their bounds, resolved in the
        // trait's own module exactly as a supertrait is.
        let mut assoc: Vec<AssocTypeInfo> = Vec::new();
        for a in &t.assoc_types {
            let mut bounds = Vec::new();
            for b in &a.bounds {
                let key = match qualify_written_name(
                    &b.name,
                    project.prefix_of(&t.module),
                    &project.alias_prefixes_of(&t.module),
                ) {
                    Some(key) => key,
                    None => {
                        return Err(Diagnostic::new(
                            format!(
                                "'{}' names no module alias, so the bound on 'type {}' of '{}' cannot be resolved",
                                b.name, a.name, t.name
                            ),
                            b.span,
                            t.module.clone(),
                        ))
                    }
                };
                if !out.contains_key(&key) {
                    return Err(Diagnostic::new(
                        format!(
                            "'{}' is not a trait, so it cannot bound 'type {}' of '{}': a bound names a declared trait ('trait {} {{ ... }}')",
                            b.name, a.name, t.name, b.name
                        ),
                        b.span,
                        t.module.clone(),
                    ));
                }
                bounds.push((b.name.clone(), key));
            }
            assoc.push(AssocTypeInfo {
                name: a.name.clone(),
                bounds,
            });
        }
        for s in &t.supertraits {
            // A supertrait of a generic trait may not be written with
            // arguments in this slice (`trait A<T>: B<T>`): the substitution a
            // parameterized supertrait needs is a follow-up, and a plain
            // supertrait works for both a generic and an ordinary trait.
            if !s.args.is_empty() {
                return Err(Diagnostic::new(
                    format!(
                        "a supertrait with arguments ('{}') is not in this slice: write the supertrait without arguments ('{}'), and bound the parameter where it is used",
                        s.display(),
                        s.name
                    ),
                    s.span,
                    t.module.clone(),
                ));
            }
            let key = match qualify_written_name(
                &s.name,
                project.prefix_of(&t.module),
                &project.alias_prefixes_of(&t.module),
            ) {
                Some(key) => key,
                None => {
                    return Err(Diagnostic::new(
                        format!(
                            "'{}' names no module alias, so the supertrait of '{}' cannot be resolved",
                            s.name, t.name
                        ),
                        s.span,
                        t.module.clone(),
                    ))
                }
            };
            if !out.contains_key(&key) {
                return Err(Diagnostic::new(
                    format!(
                        "'{}' is not a trait, so it cannot be a supertrait of '{}': a supertrait names a declared trait ('trait {} {{ ... }}')",
                        s.name, t.name, s.name
                    ),
                    s.span,
                    t.module.clone(),
                ));
            }
            if !canonical.contains(&key) {
                canonical.push(key);
            }
        }
        // A bound on the trait's own type parameter (`trait Store<K, V: Named>`)
        // is resolved in the trait's module exactly as a supertrait is, and
        // checked against the impl's argument below.
        let mut param_bounds: Vec<Vec<(String, String)>> = Vec::new();
        for bounds in &t.type_param_bounds {
            let mut resolved = Vec::new();
            for b in bounds {
                if !b.bound_eq.is_empty() {
                    return Err(Diagnostic::new(
                        format!(
                            "an equality constraint on a generic trait's parameter ('{}') is not in this slice: bound the parameter with a plain trait name",
                            b.display()
                        ),
                        b.span,
                        t.module.clone(),
                    ));
                }
                if !b.args.is_empty() {
                    return Err(Diagnostic::new(
                        format!(
                            "a bound with arguments ('{}') is not in this slice: a generic trait's parameter is bounded by a plain trait name",
                            b.display()
                        ),
                        b.span,
                        t.module.clone(),
                    ));
                }
                let key = match qualify_written_name(
                    &b.name,
                    project.prefix_of(&t.module),
                    &project.alias_prefixes_of(&t.module),
                ) {
                    Some(key) => key,
                    None => {
                        return Err(Diagnostic::new(
                            format!(
                                "'{}' names no module alias, so the bound on '{}' of '{}' cannot be resolved",
                                b.name, t.name, t.name
                            ),
                            b.span,
                            t.module.clone(),
                        ))
                    }
                };
                if !out.contains_key(&key) {
                    return Err(Diagnostic::new(
                        format!(
                            "'{}' is not a trait, so it cannot bound a parameter of '{}': a bound names a declared trait ('trait {} {{ ... }}')",
                            b.name, t.name, b.name
                        ),
                        b.span,
                        t.module.clone(),
                    ));
                }
                resolved.push((b.name.clone(), key));
            }
            param_bounds.push(resolved);
        }
        if let Some(info) = out.get_mut(&t.name) {
            info.supertraits = canonical;
            info.assoc_types = assoc;
            info.type_param_bounds = param_bounds;
        }
    }
    if let Some(cycle) = supertrait_cycle(&out) {
        let name = &cycle[0];
        let span = program
            .traits
            .iter()
            .find(|t| &t.name == name)
            .map(|t| t.span)
            .unwrap_or_else(|| program.traits[0].span);
        let module = program
            .traits
            .iter()
            .find(|t| &t.name == name)
            .map(|t| t.module.clone())
            .unwrap_or_default();
        return Err(Diagnostic::new(
            format!(
                "the supertrait relation is cyclic: {}: a trait cannot inherit from itself",
                cycle.join(" inherits from ")
            ),
            span,
            module,
        ));
    }
    Ok(out)
}
