use super::*;

/// The `Ty::Struct` names a value of `ty` contains *by value*, looking through
/// a borrow and stopping at a container: a `List`/`Box`/`Map` holds a pointer
/// and its typedef is emitted (with a forward-declared struct tag) before every
/// declaration anyway.
pub(crate) fn by_value_structs(ty: &Ty, out: &mut Vec<String>) {
    match ty {
        Ty::Struct(name) => out.push(name.clone()),
        Ty::Ref { inner, .. } => by_value_structs(inner, out),
        Ty::List(_) | Ty::Box(_) | Ty::Map(..) => {}
        _ => {}
    }
}

/// Gives every declaration - hand-written and instantiated alike - a position
/// in one topological order (A-2). A type that contains another *by value* has
/// to be emitted after it, because C needs the complete type; hand-written
/// declarations already arrive in declaration order, which is topological
/// among themselves, but an instantiation can be named by a hand-written field
/// (`val p: Pair<i32, String>`), so the two sets have to be merged rather than
/// concatenated. Ties are broken by the incoming order, so the emitted C is
/// deterministic.
pub(crate) fn order_declarations(records: &mut [StructDef], sums: &mut [EnumDef]) {
    let mut deps: HashMap<String, Vec<String>> = HashMap::new();
    for r in records.iter() {
        let mut out = Vec::new();
        for f in &r.fields {
            by_value_structs(&f.ty, &mut out);
        }
        deps.insert(r.name.clone(), out);
    }
    for e in sums.iter() {
        let mut out = Vec::new();
        for v in &e.variants {
            for f in &v.fields {
                by_value_structs(&f.ty, &mut out);
            }
        }
        deps.insert(e.name.clone(), out);
    }

    let mut seed: Vec<(usize, String)> = records
        .iter()
        .map(|r| (r.order, r.name.clone()))
        .chain(sums.iter().map(|e| (e.order, e.name.clone())))
        .collect();
    seed.sort();

    pub(crate) fn visit(
        name: &str,
        deps: &HashMap<String, Vec<String>>,
        done: &mut HashMap<String, usize>,
        visiting: &mut std::collections::HashSet<String>,
        next: &mut usize,
    ) {
        if done.contains_key(name) || !visiting.insert(name.to_string()) {
            return;
        }
        if let Some(d) = deps.get(name) {
            for dep in d {
                if deps.contains_key(dep) {
                    visit(dep, deps, done, visiting, next);
                }
            }
        }
        visiting.remove(name);
        done.insert(name.to_string(), *next);
        *next += 1;
    }

    let mut done: HashMap<String, usize> = HashMap::new();
    let mut visiting: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut next = 0usize;
    for (_, name) in &seed {
        visit(name, &deps, &mut done, &mut visiting, &mut next);
    }
    for r in records.iter_mut() {
        if let Some(o) = done.get(&r.name) {
            r.order = *o;
        }
    }
    for e in sums.iter_mut() {
        if let Some(o) = done.get(&e.name) {
            e.order = *o;
        }
    }
}

/// The passdowns of one module, keyed by the bare name a function writes.
/// `before` limits it to passdowns declared earlier - used while checking a
/// `pd` initializer, so "use before declaration" is a real error rather than
/// a silent read of a zero-initialized global.
pub(crate) fn pd_map(all: &[PdInfo], module: &str, before: Option<usize>) -> HashMap<String, PdInfo> {
    all.iter()
        .filter(|p| p.module == module && before.map_or(true, |i| p.index < i))
        .map(|p| (p.name.clone(), p.clone()))
        .collect()
}

/// Resolves every passdown's type, in merged-program order (module load
/// order, then declaration order within a module), checking for duplicate
/// declarations and name clashes on the way.
pub(crate) fn collect_pds(
    program: &ast::Program,
    project: &Project,
    sigs: &HashMap<String, FnSig>,
    structs: &HashMap<String, StructDef>,
    enums: &HashMap<String, EnumDef>,
    consts: &HashMap<String, ConstInfo>,
    const_fns: &std::rc::Rc<ConstFns>,
    owned_structs: &std::collections::HashSet<String>,
    overloads: &HashMap<String, Vec<(Vec<Ty>, String)>>,
    env: &TypeEnv,
    fns: &FuncEnv,
    statics: &HashMap<String, StaticInfo>,
) -> std::result::Result<Vec<PdInfo>, Diagnostic> {
    let mut out: Vec<PdInfo> = Vec::new();
    for (index, decl) in program.pds.iter().enumerate() {
        // `resolve.rs` already prefixed `decl.name`; the bare name is what
        // functions in this module write.
        let prefix = project.prefix_of(&decl.module);
        let bare = decl
            .name
            .strip_prefix(prefix)
            .unwrap_or(&decl.name)
            .to_string();
        if out
            .iter()
            .any(|p| p.module == decl.module && p.name == bare)
        {
            return Err(Diagnostic::new(
                format!("passdown '{bare}' is declared more than once"),
                decl.span,
                decl.module.clone(),
            ));
        }
        if structs.contains_key(&decl.name)
            || statics.contains_key(&decl.name)
            || sigs.keys().any(|k| k == &decl.name)
        {
            return Err(Diagnostic::new(
                format!("'{}' is used for both a passdown and another top-level item", bare),
                decl.span,
                decl.module.clone(),
            ));
        }

        let aliases = project.alias_prefixes_of(&decl.module);
        let value_ty = match &decl.ty {
            Some(t) => TypeScope {
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
            }
            .resolve(t, false)
            .map_err(|e| e.in_module(&decl.module))?,
            None => {
                // Type-only probe: the initializer is checked again for real
                // once every passdown type (and the wiring table) is known.
                // `init_state` is set so diagnostics can say "a `pd`
                // initializer only sees passdowns declared before it".
                let mut ctx = base_ctx(
                    &decl.module,
                    sigs,
                    structs,
                    enums,
                    consts,
                    const_fns,
                    owned_structs,
                    overloads,
                    project,
                    &out,
                    None,
                    &HashMap::new(),
                    env,
                    fns,
                    statics,
                );
                ctx.init_state = Some(out.iter().map(|p| p.hidden.clone()).collect());
                check_expr(&decl.init, &mut ctx, None)
                    .map_err(|e| e.in_module(&decl.module))?
                    .ty
            }
        };
        // An owned passdown is allowed from M6.2 on: it lives in a generated
        // file-scope global and is freed by an `atexit` handler, which is
        // v1's only drop point at process exit (see `write_pd_globals`).
        out.push(PdInfo {
            qualified: decl.name.clone(),
            name: bare,
            hidden: format!("pd#{}", decl.name),
            value_ty,
            mutable: decl.mutable,
            thread_local: decl.thread_local,
            module: decl.module.clone(),
            index,
        });
    }
    Ok(out)
}

/// Checks one `pd` initializer for real, with every passdown that is already
/// initialized visible and nothing else.
/// The checks a `tl` declaration must pass (issue 234). Its type is a scalar, so
/// no heap or destructor value is ever a thread's own copy, and its initializer
/// is a literal (optionally negated), so every thread's copy starts from a
/// constant. A refusal names the declaration and the reason.
pub(crate) fn check_thread_local(decl: &ast::PdDecl, ty: &Ty, init: &tir::Expr) -> std::result::Result<(), Diagnostic> {
    let scalar = matches!(
        ty,
        Ty::I32 | Ty::I64 | Ty::Int | Ty::U32 | Ty::U64 | Ty::Uint | Ty::F32 | Ty::F64 | Ty::Float | Ty::Bool | Ty::Char | Ty::U8 | Ty::U16 | Ty::Usize | Ty::Isize
    );
    if !scalar {
        return Err(Diagnostic::new(
            format!(
                "'tl {}' has type '{}', and thread-local storage takes a scalar in this release: a heap-owning or 'Drop' value needs per-thread root registration first (issue 234)",
                decl.name,
                ty.display()
            ),
            decl.span,
            decl.module.clone(),
        ));
    }
    pub(crate) fn is_literal(e: &tir::Expr) -> bool {
        match &e.kind {
            tir::ExprKind::IntLit(_)
            | tir::ExprKind::FloatLit(_)
            | tir::ExprKind::BoolLit(_)
            | tir::ExprKind::CharLit(_) => true,
            tir::ExprKind::Unary { op: crate::ast::UnOp::Neg, expr } => is_literal(expr),
            _ => false,
        }
    }
    if !is_literal(init) {
        return Err(Diagnostic::new(
            format!(
                "the initializer of 'tl {}' is not a literal: each thread's copy starts from the literal, so it cannot call or read anything (issue 234)",
                decl.name
            ),
            decl.init.span(),
            decl.module.clone(),
        ));
    }
    Ok(())
}

pub(crate) fn check_pd_initializer(
    init: &ast::Expr,
    pd: &PdInfo,
    sigs: &HashMap<String, FnSig>,
    structs: &HashMap<String, StructDef>,
    enums: &HashMap<String, EnumDef>,
    consts: &HashMap<String, ConstInfo>,
    const_fns: &std::rc::Rc<ConstFns>,
    owned_structs: &std::collections::HashSet<String>,
    overloads: &HashMap<String, Vec<(Vec<Ty>, String)>>,
    project: &Project,
    all: &[PdInfo],
    wired: &HashMap<String, Vec<PdInfo>>,
    wiring: &PdWiring,
    initialized: &std::collections::HashSet<String>,
    env: &TypeEnv,
    fns: &FuncEnv,
    statics: &HashMap<String, StaticInfo>,
) -> Result<tir::Expr> {
    let mut ctx = base_ctx(
        &pd.module,
        sigs,
        structs,
        enums,
        consts,
        const_fns,
        owned_structs,
        overloads,
        project,
        all,
        Some(pd.index),
        wired,
        env,
        fns,
        statics,
    );
    // An initializer has no hidden parameters of its own: it reads the
    // globals that earlier initializers already assigned.
    ctx.pd_global_borrow = true;
    ctx.init_state = Some(initialized.clone());
    ctx.fn_key = format!("pd#{}", pd.name);
    ctx.pd_modes = wiring.modes.clone();
    ctx.pd_probe = wiring.probe.clone();
    check_expr(init, &mut ctx, Some(&pd.value_ty))
}

pub(crate) fn base_ctx(
    module: &str,
    sigs: &HashMap<String, FnSig>,
    structs: &HashMap<String, StructDef>,
    enums: &HashMap<String, EnumDef>,
    consts: &HashMap<String, ConstInfo>,
    const_fns: &std::rc::Rc<ConstFns>,
    owned_structs: &std::collections::HashSet<String>,
    overloads: &HashMap<String, Vec<(Vec<Ty>, String)>>,
    project: &Project,
    all_pds: &[PdInfo],
    before: Option<usize>,
    wired: &HashMap<String, Vec<PdInfo>>,
    env: &TypeEnv,
    fns: &FuncEnv,
    statics: &HashMap<String, StaticInfo>,
) -> Ctx {
    let module_pds: Vec<&PdInfo> = all_pds.iter().filter(|p| p.module == module).collect();
    let pds = module_pds
        .iter()
        .filter(|p| before.map_or(true, |i| p.index < i))
        .map(|p| (p.name.clone(), (*p).clone()))
        .collect();
    let foreign_pds = all_pds
        .iter()
        .filter(|p| p.module != module)
        .map(|p| (p.name.clone(), p.module.clone()))
        .collect();
    Ctx {
        block_names: std::collections::HashSet::new(),
        active_blocks: Vec::new(),
        let_names: std::collections::HashSet::new(),
        box_bindings: HashMap::new(),
        outer_boxes: HashMap::new(),
        box_sink_ok: None,
        box_sink_here: None,
        block_result_used: false,
        box_let_ok: false,
        field_base_ok: false,
        field_base_here: false,
        block_box_count: HashMap::new(),
        sigs: sigs.clone(),
        structs: structs.clone(),
        enums: enums.clone(),
        consts: consts.clone(),
        statics: statics.clone(),
        const_fns: const_fns.clone(),
        owned_structs: owned_structs.clone(),
        overloads: overloads.clone(),
        module: module.to_string(),
        caps: project.caps_of(module),
        gated: project.gated().clone(),
        has_extern_section: project.extern_ffi_of(module).is_some(),
        variadic_externs: env.variadic_externs.clone(),
        ret_ty: Ty::Unit,
        fn_name: String::new(),
        scopes: vec![HashMap::new()],
        prefix: project.prefix_of(module).to_string(),
        aliases: project.alias_prefixes_of(module),
        env: env.clone(),
        fns: fns.clone(),
        type_args: HashMap::new(),
        pds,
        foreign_pds,
        wired: wired.clone(),
        init_state: None,
        pd_global_borrow: false,
        own_hidden: Vec::new(),
        fn_key: String::new(),
        pd_modes: std::rc::Rc::new(HashMap::new()),
        pd_probe: None,
        self_probe: false,
        self_written: std::cell::Cell::new(false),
        block_note: None,
        in_fn_literal: false,
        borrowed_captures: false,
        spawn_entry: false,
        spawn_entry_next: false,
        outer_mut: Default::default(),
        excl_captures: Default::default(),
        outer_excl: Default::default(),
        excl_needed: Default::default(),
        outer_bindings: HashMap::new(),
        captures: Vec::new(),
        outer_receiver: None,
        receiver_struct: None,
        when_temps: 0,
        for_temps: 0,
        loop_depth: 0,
        tuple_temps: 0,
        defer_empty_container: false,
        probes: Vec::new(),
        str_arg_site: None,
        box_store_into: None,
        unsafe_depth: 0,
        unsafe_fns: project.unsafe_fns().clone(),
        must_use_fns: project.must_use_fns().clone(),
        interrupt_fns: project.interrupt_fns().clone(),
        freestanding: project.target().is_freestanding(),
        cpus: project.target().cpus(),
        infer_scoped_ret: false,
    }
}

/// sig key -> the passdowns a function references. Computed from the AST
/// before bodies are checked, because a call site has to know what its
/// callee needs in order to pass it.
pub(crate) fn scan_all(
    program: &ast::Program,
    all: &[PdInfo],
    structs: &HashMap<String, StructDef>,
    decl_keys: &[String],
) -> HashMap<String, Vec<PdInfo>> {
    let mut wired = HashMap::new();
    for (i, f) in program.functions.iter().enumerate() {
        // `main` gets no hidden parameters (see `Ctx::pd_global_borrow`), and
        // it is not callable, so it never needs an entry here either.
        if f.receiver.is_none() && f.name == "main" {
            continue;
        }
        let visible = pd_map(all, &f.module, None);
        let mut used = scan_pd_uses(f, &visible, structs);
        // A `tl` is never passed as a hidden argument: each thread reads its own
        // copy through the global, so a function value can name a function that
        // reads one (issue 234).
        used.retain(|p| !p.thread_local);
        if !used.is_empty() {
            wired.insert(decl_keys[i].clone(), used);
        }
    }
    wired
}

/// Scope-aware walk for the passdowns a function body actually names.
/// Shadowing is honored exactly as `check_expr` honors it: a parameter,
/// receiver field, or `val`/`var` in scope hides a passdown of the same name,
/// so a hidden name is not a passdown reference. A method call's name, a
/// field access's field name, and a call's callee are never passdown
/// references either - only bare identifiers are.
pub(crate) struct PdScan {
    pub(crate) scopes: Vec<std::collections::HashSet<String>>,
    /// Field names of the current receiver's `datac`, which shadow passdowns
    /// the same way locals do.
    pub(crate) receiver_fields: std::collections::HashSet<String>,
    /// Fields only count as shadowing inside a receiver-form function.
    pub(crate) has_receiver: bool,
    pub(crate) visible: std::collections::HashSet<String>,
    pub(crate) used: Vec<String>,
}

impl PdScan {
    pub(crate) fn bound(&self, name: &str) -> bool {
        (self.has_receiver && self.receiver_fields.contains(name))
            || self.scopes.iter().rev().any(|s| s.contains(name))
    }

    pub(crate) fn define(&mut self, name: &str) {
        self.scopes.last_mut().unwrap().insert(name.to_string());
    }

    pub(crate) fn note(&mut self, name: &str) {
        if !self.bound(name) && self.visible.contains(name) && !self.used.iter().any(|u| u == name)
        {
            self.used.push(name.to_string());
        }
    }

    pub(crate) fn scan_block(&mut self, block: &ast::Block) {
        self.scopes.push(std::collections::HashSet::new());
        for stmt in &block.stmts {
            self.scan_stmt(stmt);
        }
        self.scopes.pop();
    }

    pub(crate) fn scan_stmt(&mut self, stmt: &ast::Stmt) {
        match stmt {
            ast::Stmt::Let { name, init, .. } => {
                // Initializer first: `val name = name` reads the outer one.
                self.scan_expr(init);
                self.define(name);
            }
            ast::Stmt::LetTuple { pattern, init, .. } => {
                // The same order as a `let`: the initializer is read before
                // any position it binds exists, so `val (a, b) = a` reads the
                // outer `a`.
                self.scan_expr(init);
                let mut names: Vec<(String, Vec<usize>)> = Vec::new();
                pattern.binds(&mut Vec::new(), &mut names);
                for (name, _) in &names {
                    self.define(name);
                }
            }
            ast::Stmt::While { cond, body, .. } => {
                self.scan_expr(cond);
                self.scan_block(body);
            }
            ast::Stmt::For {
                binding,
                iterable,
                body,
                ..
            } => {
                self.scan_expr(iterable);
                self.scopes.push(std::collections::HashSet::new());
                self.define(binding);
                for stmt in &body.stmts {
                    self.scan_stmt(stmt);
                }
                self.scopes.pop();
            }
            ast::Stmt::Return { value, .. } => {
                if let Some(v) = value {
                    self.scan_expr(v);
                }
            }
            ast::Stmt::Expr { expr, .. } => self.scan_expr(expr),
            ast::Stmt::Defer { expr, .. } | ast::Stmt::Errdefer { expr, .. } => self.scan_expr(expr),
            ast::Stmt::Break { .. } | ast::Stmt::Continue { .. } => {}
        }
    }

    pub(crate) fn scan_expr(&mut self, expr: &ast::Expr) {
        match expr {
            // A `fun` literal's body is a function of its own, and it cannot
            // read a passdown at all (A-4), so the enclosing scan does not
            // descend into it.
            ast::Expr::FnLit { .. } => {}
            ast::Expr::Ident(name, _) => self.note(name),
            ast::Expr::Unary { expr, .. }
            | ast::Expr::Ref { expr, .. }
            | ast::Expr::Try { expr, .. }
            | ast::Expr::Deref { expr, .. } => self.scan_expr(expr),
            ast::Expr::Binary { lhs, rhs, .. } => {
                self.scan_expr(lhs);
                self.scan_expr(rhs);
            }
            ast::Expr::Call { args, .. } => {
                for a in args {
                    self.scan_expr(&a.value);
                }
            }
            ast::Expr::ArrayLit { elems, .. } => {
                for e in elems {
                    self.scan_expr(e);
                }
            }
            ast::Expr::ArrayRepeat { value, len, .. } => {
                self.scan_expr(value);
                self.scan_expr(len);
            }
            ast::Expr::TupleLit { elems, .. } => {
                for e in elems {
                    self.scan_expr(e);
                }
            }
            // Only the base - a field named like a passdown isn't a read of
            // it.
            ast::Expr::FieldAccess { base, .. } => self.scan_expr(base),
            ast::Expr::Assign { target, value, .. } => {
                self.scan_expr(target);
                self.scan_expr(value);
            }
            ast::Expr::If {
                cond,
                then_block,
                else_block,
                ..
            } => {
                self.scan_expr(cond);
                self.scan_block(then_block);
                match else_block.as_deref() {
                    Some(ast::ElseBranch::Block(b)) => self.scan_block(b),
                    Some(ast::ElseBranch::If(e)) => self.scan_expr(e),
                    None => {}
                }
            }
            ast::Expr::Block(b) => self.scan_block(b),
            ast::Expr::MethodCall { receiver, args, .. } => {
                self.scan_expr(receiver);
                for a in args {
                    self.scan_expr(&a.value);
                }
            }
            ast::Expr::Template { parts, .. } => {
                for p in parts {
                    if let ast::TemplatePart::Expr(e) = p {
                        self.scan_expr(e);
                    }
                }
            }
            // A pattern binding shadows a passdown of the same name inside
            // its arm body, exactly as a `val` does, so the bindings push a
            // scope of their own (A-1).
            ast::Expr::When {
                scrutinee, arms, ..
            } => {
                self.scan_expr(scrutinee);
                for arm in arms {
                    let mut shadowed = std::collections::HashSet::new();
                    if let Some(p) = &arm.pattern {
                        for b in p.binds.iter().flatten() {
                            shadowed.insert(b.clone());
                        }
                    }
                    self.scopes.push(shadowed);
                    self.scan_expr(&arm.body);
                    self.scopes.pop();
                }
            }
            ast::Expr::IntLit(..)
            | ast::Expr::CharLit(..)
            | ast::Expr::ByteLit(..)
            | ast::Expr::FloatLit(..)
            | ast::Expr::StringLit(..)
            | ast::Expr::BoolLit(..) => {}
            // An `unsafe { ... }` block's body is scanned like any other
            // block: a passdown named inside one is still a passdown the
            // enclosing function has to receive.
            ast::Expr::Unsafe { block, .. } => self.scan_block(block),
            ast::Expr::ManualBlock { body, .. } => self.scan_block(body),
            // The operand expressions of an `asm(...)` are scanned for the same
            // reason: a place or a value inside one may name a passdown.
            ast::Expr::Asm { operands, .. } => {
                for op in operands {
                    if let Some(e) = &op.operand {
                        self.scan_expr(e);
                    }
                }
            }
        }
    }
}

pub(crate) fn scan_pd_uses(
    f: &ast::FunctionDecl,
    visible: &HashMap<String, PdInfo>,
    structs: &HashMap<String, StructDef>,
) -> Vec<PdInfo> {
    if visible.is_empty() {
        return Vec::new();
    }
    let mut scan = PdScan {
        scopes: Vec::new(),
        receiver_fields: std::collections::HashSet::new(),
        has_receiver: f.receiver.is_some(),
        visible: visible.keys().cloned().collect(),
        used: Vec::new(),
    };
    scan.scopes.push(std::collections::HashSet::new());
    if let Some(r) = &f.receiver {
        // `resolve.rs` already prefixed the receiver type name.
        if let Some(sdef) = structs.get(&r.type_name) {
            scan.receiver_fields = sdef.fields.iter().map(|f| f.name.clone()).collect();
        }
        scan.define("self");
    }
    for p in &f.params {
        scan.define(&p.name);
    }
    match &f.body {
        ast::FunctionBody::Block(b) => scan.scan_block(b),
        ast::FunctionBody::Expr(e) => scan.scan_expr(e),
        // An `extern` has no body to scan for passdown references, and a
        // trait signature has none either.
        ast::FunctionBody::Extern | ast::FunctionBody::Signature => {}
    }

    let mut used: Vec<PdInfo> = scan
        .used
        .iter()
        .map(|name| visible[name].clone())
        .collect();
    // Declaration order, not reference order - so the hidden parameters (and
    // therefore the generated C signatures) are stable.
    used.sort_by_key(|p| p.index);
    used
}

/// Resolves a parameter's type per `design/syntax.md (kelp wiki)` §5: a `datac` param is
/// an implicit shared borrow unless `mut`-marked (exclusive); everything
/// else (Copy types) stays a plain by-value type unless explicitly
/// `mut`-marked, since borrowing a Copy scalar by default would add
/// indirection nobody asked for. `move T` (A-4) is the third form, and the
/// only one that resolves an owned type to the bare type: the callee takes
/// ownership, so `TypeScope::resolve_param` handles it before this function
/// is reached. See `TypeScope::resolve_param` and `resolve_receiver` for the
/// M5 module-qualification rule.
pub(crate) fn implicit_borrow_wrap(base: Ty, mut_marked: bool) -> Ty {
    if mut_marked {
        Ty::Ref { mutable: true, inner: Box::new(base) }
    } else if base.is_copy() {
        // A `Copy` value is passed by value: copying one is copying a scalar, a
        // pair of words, or a pointer, so a borrow would only add an
        // indirection. That is `str`, `View`, a function value and the scalars.
        base
    } else {
        // Everything that owns memory, so the caller keeps it. This is the whole
        // list: `datac`/`enum`, `Bytes`, `String`, the containers, a closure -
        // and a fixed-size array whose element owns something (A-18). Asking
        // `is_copy` rather than listing the types is what keeps the two in step:
        // an array of scalars is a value and is passed like one, while
        // `[String; 2]` is an owner, so a plain parameter borrows it exactly as
        // a `String` parameter does instead of consuming the caller's array.
        Ty::Ref { mutable: false, inner: Box::new(base) }
    }
}

/// Unwraps a (possibly borrowed) type down to the `datac` name it names, if
/// any - used by field access/assignment so `.field` auto-derefs through a
/// `Ref` base exactly like an owned struct.
pub(crate) fn struct_name_of(ty: &Ty) -> Option<&str> {
    match ty {
        Ty::Struct(s) => Some(s),
        Ty::Ref { inner, .. } => struct_name_of(inner),
        _ => None,
    }
}

/// The message types `panic` takes: the borrowed `str` every call site
/// already had, and an owned `String` (MISSING-8). `print`'s set is wider on
/// purpose (numbers, `bool`, `Bytes`); a panic message that renders as a
/// length-and-pointer pair is a string in every case.
pub(crate) fn is_panic_message(ty: &Ty) -> bool {
    is_str_like(ty)
        || matches!(ty, Ty::String)
        || matches!(ty, Ty::Ref { inner, .. } if matches!(**inner, Ty::String))
}

/// Whether a type is one of the builtin owned buffers, or a borrow of one -
/// the types `print` renders straight out of their buffer.
pub(crate) fn is_printable_buffer(ty: &Ty) -> bool {
    match ty {
        // A `View` is printable because its length travels with it: `print`
        // emits `%.*s` from the pair, so a slice prints as the slice.
        Ty::Bytes | Ty::String | Ty::View => true,
        Ty::Ref { inner, .. } => matches!(**inner, Ty::Bytes | Ty::String),
        _ => false,
    }
}

/// `print`/templates read an owned buffer without consuming it, so a *named*
/// one is borrowed here rather than passed by value (which would be a move).
///
/// A temporary is passed through as the owned value it is: the removed C backend
/// hoists it into a C temporary and frees it right after the `printf`, which
/// is what makes `print(str.to_upper(s))` allocate once, print, and free
/// (M6.3 - until then this was a diagnostic telling the author to bind a
/// `val` first).
pub(crate) fn borrow_for_print(value: tir::Expr, _what: &str, _ctx: &Ctx) -> Result<tir::Expr> {
    if !matches!(value.ty, Ty::Bytes | Ty::String) {
        return Ok(value);
    }
    if !is_tir_place(&value) {
        return Ok(value);
    }
    let inner = value.ty.clone();
    let span = value.span;
    Ok(tir::Expr {
        kind: tir::ExprKind::Ref {
            mutable: false,
            expr: Box::new(value),
        },
        ty: Ty::Ref {
            mutable: false,
            inner: Box::new(inner),
        },
        span,
    })
}

/// A "place": something with stable storage that can be borrowed or
/// assigned into - an `Ident`, a `.field` chain rooted in one, or a
/// dereferenced borrow (`*p`, `(*p).field`).
pub(crate) fn is_tir_place(expr: &tir::Expr) -> bool {
    matches!(
        expr.kind,
        tir::ExprKind::Ident(_)
            | tir::ExprKind::StaticRef(_)
            | tir::ExprKind::FieldAccess { .. }
            | tir::ExprKind::EnumField { .. }
            // A capture is a place too: the environment holds the value (A-4
            // phase E), so `prefix.as_str()` inside a literal borrows it
            // rather than borrowing a temporary.
            | tir::ExprKind::Capture { .. }
            | tir::ExprKind::Deref { .. }
    )
}

/// A place whose storage is still there after the enclosing statement, which
/// is what a returned borrow and an exclusive borrow both need.
///
/// `is_tir_place` is the weaker question, "can an address be taken?", and
/// a field of a temporary answers yes: the removed C backend stages the temporary so
/// the field has an address. That address dies with the statement, though, so
/// `mkName().name.as_str()` is a borrow of a temporary the same way
/// `String.from_str("x").as_str()` is, one projection deeper, and it needs the
/// same refusal. A `Deref` is durable, and so is any chain that steps through
/// one: what a pointer points at is somebody else's storage, which is what
/// makes `headers.get(i).name.as_str()` fine where `mkName().name.as_str()` is
/// not.
pub(crate) fn is_durable_place(expr: &tir::Expr) -> bool {
    match &expr.kind {
        tir::ExprKind::FieldAccess { base, .. } | tir::ExprKind::EnumField { base, .. } => {
            matches!(base.ty, Ty::Ref { .. }) || is_durable_place(base)
        }
        tir::ExprKind::Deref { .. } => true,
        _ => is_tir_place(expr),
    }
}

/// `ast::is_place`, kept callable under its old local name at every existing
/// call site - the definition itself now lives in `ast.rs` so `parser.rs` can
/// share it (see `parse_if_let`).
pub(crate) fn is_ast_place(expr: &ast::Expr) -> bool {
    ast::is_place(expr)
}

/// Whether a checked place-expression's storage is currently writable:
/// either its root is a plain owned `var`-bound local, or its root (or the
/// pointer stepped through by a `*`) is statically typed as an *exclusive*
/// `Ty::Ref`. Used both by assignment (`p.x = ...`, `*p = ...`) and by
/// `&mut expr` (taking an exclusive borrow requires the place already be
/// writable).
pub(crate) fn check_target_writable(target: &tir::Expr, ctx: &Ctx) -> Result<()> {
    match &target.kind {
        // A static's own mutability decides whether it can be assigned, once
        // the `unsafe` gate has already let the name through (kernel stage 7).
        tir::ExprKind::StaticRef(name) => match ctx.statics.get(name) {
            Some(s) if s.mutable => Ok(()),
            Some(s) => Err(TypeError {
                help: None,
                message: format!(
                    "cannot assign to 'static {}': it is not declared 'static mut'",
                    s.name
                ),
                span: target.span,
            }),
            None => Ok(()),
        },
        tir::ExprKind::Ident(name) => {
            let Some(info) = ctx.lookup(name) else {
                return Err(TypeError {
                    help: None,
                    message: format!("cannot assign to '{name}'"),
                    span: target.span,
                });
            };
            match &info.ty {
                Ty::Ref { mutable, .. } => {
                    if *mutable {
                        Ok(())
                    } else if info.mutable {
                        // Reassigning a `var` that *holds* a shared borrow
                        // rebinds the pointer; it does not write through it,
                        // so it needs no exclusivity. This is what makes a
                        // cursor over a borrowed structure expressible
                        // (`var cur = list` ... `cur = cur.next.get()`), which
                        // a recursive type needs before it can be walked
                        // iteratively (M8).
                        Ok(())
                    } else {
                        Err(TypeError {
                            help: None,
                            message: format!("cannot write to '{name}': it is a shared borrow, not 'mut'"),
                            span: target.span,
                        })
                    }
                }
                _ => {
                    if info.mutable {
                        Ok(())
                    } else {
                        Err(TypeError {
                            help: None,
                            message: format!("cannot assign to '{name}': not declared 'var'"),
                            span: target.span,
                        })
                    }
                }
            }
        }
        tir::ExprKind::FieldAccess { base, .. } => check_target_writable(base, ctx),
        tir::ExprKind::Deref { expr: inner } => match &inner.ty {
            Ty::Ref { mutable: true, .. } => Ok(()),
            Ty::Ref { mutable: false, .. } => {
                // A `pd val` read is a Deref of a shared borrow, so it lands
                // here - worth the passdown's own wording, since "shared
                // borrow" is not what the author wrote (M6.4). A `pd var`
                // holds an exclusive borrow exactly when its body writes it,
                // so a write that arrives through a shared borrow is either
                // the probe pass learning that this function must be upgraded
                // (recorded, allowed, and discarded) or a closure that missed
                // a use (roadmap item 17).
                if let Some(name) = pd_place_hidden(inner) {
                    if let Some(pd) = ctx.pds.values().find(|p| p.hidden == name).cloned() {
                        if !pd.mutable {
                            return Err(TypeError {
                                help: None,
                                message: format!(
                                    "cannot assign to passdown '{}': it is declared 'pd val' (only a 'pd var' can be assigned inside a function)",
                                    pd.qualified
                                ),
                                span: target.span,
                            });
                        }
                        if ctx.note_pd_exclusive_use(&pd) {
                            return Ok(());
                        }
                        return Err(TypeError {
                            help: None,
                            message: format!(
                                "cannot write to passdown '{}' through a shared borrow",
                                pd.qualified
                            ),
                            span: target.span,
                        });
                    }
                }
                Err(TypeError {
                    help: None,
                    message: "cannot write through a shared borrow".to_string(),
                    span: target.span,
                })
            }
            _ => unreachable!("a Deref's operand is always Ref-typed, guaranteed by check_expr"),
        },
        // A write through a captured binding (`kelp wiki page language/functions.md`).
        // A scoped block's capture of an owned `var` is typed `&T` until a
        // write (or a `mut` argument) shows it needs `&mut T`;
        // `check_fn_literal` retries the literal with the capture upgraded,
        // so reaching here with a shared type means the capture could not be
        // (or is being probed), and the message says which.
        tir::ExprKind::Capture { index } => {
            let name = ctx.captures.get(*index).map(|(n, _)| n.clone()).unwrap_or_default();
            match &target.ty {
                Ty::Ref { mutable: true, .. } => Ok(()),
                Ty::Ref { mutable: false, .. } => {
                    note_excl_need(target, ctx);
                    let outer_borrow = ctx
                        .outer_bindings
                        .get(&name)
                        .is_some_and(crate::types::is_borrowed_field_ty);
                    let message = if outer_borrow {
                        format!("cannot write to captured '{name}': it is a shared borrow in the enclosing function, so a block cannot write through it; take it as a 'mut' parameter there")
                    } else if ctx.outer_mut.contains(&name) {
                        format!("cannot write to captured '{name}': it is captured by shared borrow here")
                    } else {
                        format!("cannot write to captured '{name}': it is not declared 'var' in the enclosing function; declare it 'var' so a scoped block can write it")
                    };
                    Err(TypeError { message, span: target.span, help: None })
                }
                _ => {
                    note_excl_need(target, ctx);
                    Err(TypeError {
                        help: None,
                    message: if ctx.borrowed_captures && ctx.outer_mut.contains(&name) {
                        format!("cannot write to captured '{name}': it is copied in by value here")
                    } else if ctx.borrowed_captures {
                        format!("cannot write to captured '{name}': it is not declared 'var' in the enclosing function; declare it 'var' so a scoped block can write it")
                    } else {
                        format!("cannot write to captured '{name}': a closure owns its own copy of it, so a write would change only that copy; return the new value, or use a scoped block over a 'var'")
                    },
                    span: target.span,
                    })
                }
            }
        }
        // A function name, a literal or a call result is not somewhere a value
        // can be stored (`f -= 1`, `1 = x`).
        _ => Err(TypeError {
            help: None,
            message: "cannot assign to this expression: it is not a variable, a field or an element".to_string(),
            span: target.span,
        }),
    }
}
