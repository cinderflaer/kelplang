use super::*;

/// Every container instantiation a program mentions, the mirror of
/// the instantiation table (MK7 S2D). Kept separate from the `datac` table
/// because a container is lowered as a *function set*, one per element or value
/// type, exactly as the earlier emitter writes one C function set per instantiation.
#[derive(Default, Clone)]
pub(crate) struct Instantiations {
    pub(crate) lists: HashSet<Ty>,
    pub(crate) boxes: HashSet<Ty>,
    pub(crate) maps: HashSet<(Ty, Ty)>,
    pub(crate) arrays: HashSet<Ty>,
    /// Closure signatures (S2E). A closure's type is `{ fn, env, drop }`,
    /// monomorphized per signature exactly as the earlier emitter monomorphizes its
    /// typedef, so one exists per signature the program mentions.
    pub(crate) closures: HashSet<Ty>,
    /// Closure signatures a plain function value is widened into somewhere, so
    /// each needs the thunk and the one-field environment that do the widening.
    pub(crate) coerced: HashSet<Ty>,
    /// `Atomic<T>` instantiations (MK9-S3-M1 stage A1): one identified struct of
    /// one field per element type, which is the whole representation. There is
    /// no function set beside it, because each operation is one instruction
    /// emitted at the call site.
    pub(crate) atomics: HashSet<Ty>,
    /// Widened values whose function is *known* (a declared `fun` named as a
    /// value, or a literal that captured nothing, whose value is a `FnRef`
    /// too). Each gets a shim instead of the heap environment and the thunk.
    pub(crate) shims: HashSet<(String, Ty)>,
    /// The closure signatures a `thread.spawn` entry reaches (MK9-S3-M1 stage
    /// C1). Each needs one generated worker body, the IR twin of the C
    /// backend's `write_thread_body`: the runtime's trampoline calls it inside
    /// the new thread, so its parameters are pointers and the element type
    /// stays out of it.
    pub(crate) spawns: HashSet<Ty>,
    /// Owned trait objects (A-12 slice 7): `Box<dyn Trait>` values need one
    /// drop function per *trait*, because the value behind the data pointer is
    /// freed through the vtable rather than by a per-element routine.
    pub(crate) box_dyns: HashSet<Ty>,
}


/// Whether a target triple is an x86 one, which is where the asm dialect is
/// Intel rather than the target's default (kernel stage 5). Chosen per triple
/// so one Kelp template reads destination-first on x86 and on riscv64 alike
/// (`kelp wiki page language/unsafe.md`).
pub(crate) fn is_x86_triple(triple: &str) -> bool {
    matches!(
        triple.split('-').next().unwrap_or(""),
        "x86_64" | "i386" | "i486" | "i586" | "i686"
    )
}

/// The `(name, each field's type)` pairs of every declaration, for the derived
/// clone/order fixpoints. The mirror of the earlier emitter's `type_decls`.
pub(crate) fn type_decls(
    structs: &[crate::types::StructDef],
    enums: &[crate::types::EnumDef],
) -> Vec<(String, Vec<Ty>)> {
    let enum_names: HashSet<&str> = enums.iter().map(|e| e.name.as_str()).collect();
    let mut out: Vec<(String, Vec<Ty>)> = Vec::new();
    for s in structs {
        if enum_names.contains(s.name.as_str()) {
            continue;
        }
        out.push((s.name.clone(), s.fields.iter().map(|f| f.ty.clone()).collect()));
    }
    for e in enums {
        out.push((
            e.name.clone(),
            e.variants
                .iter()
                .flat_map(|v| v.fields.iter().map(|f| f.ty.clone()))
                .collect(),
        ));
    }
    out
}

/// Every container instantiation a program mentions, the mirror of
/// the earlier emitter's `collect_containers`. Over-collecting is harmless (an unused set
/// is unused code), under-collecting is not, so every expression's own type and
/// every declaration's fields are noted, and a `Map` also notes the `List<K>`
/// and `List<V>` its `keys`/`values` return.
pub(crate) fn collect_instantiations(program: &Program) -> Instantiations {
    pub(crate) fn note_ty(ty: &Ty, out: &mut Instantiations) {
        match ty {
            Ty::List(elem) => {
                if **elem != Ty::Unit {
                    out.lists.insert((**elem).clone());
                }
                note_ty(elem, out);
            }
            Ty::Box(elem) => {
                if **elem != Ty::Unit {
                    out.boxes.insert((**elem).clone());
                }
                note_ty(elem, out);
            }
            Ty::Map(key, value) => {
                // An *open* map (`Map.new()` with no type anywhere yet) carries
                // `Unit` in a slot, which names no instantiation; the first
                // `insert` fills it in (`types::fill_open_container`).
                if **key == Ty::Unit || **value == Ty::Unit {
                    return;
                }
                // `keys()`/`values()` are generated with every map and return
                // `List<K>`/`List<V>`, so both instantiations exist even when
                // the program never calls them.
                out.maps.insert(((**key).clone(), (**value).clone()));
                out.lists.insert((**key).clone());
                out.lists.insert((**value).clone());
                note_ty(key, out);
                note_ty(value, out);
            }
            Ty::Array(elem, _) => {
                out.arrays.insert(ty.clone());
                note_ty(elem, out);
            }
            // An atomic is one field of a scalar type: one instantiation per
            // element type the program mentions, with nothing to note below it
            // that is not a scalar.
            Ty::Atomic(elem) => {
                out.atomics.insert(ty.clone());
                note_ty(elem, out);
            }
            // A closure is emitted once per signature, and its parameters and
            // return type may name another closure or a container. A scoped
            // function value is the same fat pointer, so it shares the same
            // container entry and the same closure set.
            Ty::Closure { params, ret } | Ty::ScopedFn { params, ret } => {
                out.closures.insert(ty.clone());
                for p in params {
                    note_ty(p, out);
                }
                note_ty(ret, out);
            }
            // An owned trait object (A-12 slice 7) needs one drop function
            // per trait; the fat-pair type itself comes with the vtable
            // machinery, exactly as the borrowed spelling's does.
            Ty::BoxDyn { .. } => {
                out.box_dyns.insert(ty.clone());
            }
            Ty::Ref { inner, .. } => note_ty(inner, out),
            _ => {}
        }
    }
    pub(crate) fn note_block(block: &Block, out: &mut Instantiations) {
        for stmt in &block.stmts {
            match stmt {
                Stmt::Let { init, .. } => note_expr(init, out),
                Stmt::While { cond, body, .. } => {
                    note_expr(cond, out);
                    note_block(body, out);
                }
                Stmt::Return { value, .. } => {
                    if let Some(e) = value {
                        note_expr(e, out);
                    }
                }
                Stmt::Break { .. } | Stmt::Continue { .. } => {}
                Stmt::Expr(e) => note_expr(e, out),
            }
        }
        if let Some(tail) = &block.tail {
            note_expr(tail, out);
        }
    }
    pub(crate) fn note_expr(expr: &Expr, out: &mut Instantiations) {
        note_ty(&expr.ty, out);
        match &expr.kind {
            ExprKind::Unary { expr, .. }
            | ExprKind::Ref { expr, .. }
            | ExprKind::Deref { expr }
            | ExprKind::EnumTag { expr, .. }
            | ExprKind::Try { expr, .. }
            | ExprKind::ArrayRepeat { value: expr }
            | ExprKind::BoxDynCoerce { expr, .. }
            | ExprKind::BoxDynBorrow { expr, .. }
            | ExprKind::BoxDynIsEmpty { expr } => note_expr(expr, out),
            // A widened function value: the target signature needs the thunk
            // and its environment, and a value that *names* a function needs a
            // shim instead. The wrapped value needs the walk too.
            ExprKind::FnToClosure { func } => {
                if let ExprKind::FnRef(key) = &func.kind {
                    out.shims.insert((key.clone(), expr.ty.clone()));
                }
                out.coerced.insert(expr.ty.clone());
                note_expr(func, out);
            }
            ExprKind::Binary { lhs, rhs, .. } => {
                note_expr(lhs, out);
                note_expr(rhs, out);
            }
            ExprKind::Assign { target, value } => {
                note_expr(target, out);
                note_expr(value, out);
            }
            ExprKind::Call { callee, args } => {
                // `thread.spawn(entry, arg)` (MK9-S3-M1 stage C1): the entry is
                // a closure by the checker's coercion, and that signature is
                // what the worker body is generated for.
                if callee == crate::stdlib::SPAWN_FN {
                    if let Some(entry) = args.first() {
                        out.spawns.insert(entry.ty.clone());
                    }
                }
                // `Mutex#lock` (B-5): the block's closure signature gets the
                // same generated body a worker's does, because the runtime
                // calls both through one pointer shape.
                if callee == "Mutex#lock" {
                    if let Some(block) = args.get(1) {
                        out.spawns.insert(crate::types::closure_shape(&block.ty));
                    }
                }
                // `RwLock#read`/`write`/`try_read`/`try_write`: the block is a
                // scoped closure of the same shape a mutex lock's is.
                if callee == "RwLock#read"
                    || callee == "RwLock#write"
                    || callee == "RwLock#try_read"
                    || callee == "RwLock#try_write"
                {
                    if let Some(block) = args.last() {
                        out.spawns.insert(crate::types::closure_shape(&block.ty));
                    }
                }
                // `Mutex#wait_while`/`wait_while_timeout`: the predicate is a
                // plain closure and the block a scoped one, and both are called
                // through the generated body of their own signature.
                if callee == "Mutex#wait_while" || callee == "Mutex#wait_while_timeout" {
                    if let Some(pred) = args.get(2) {
                        out.spawns.insert(crate::types::closure_shape(&pred.ty));
                    }
                    if let Some(block) = args.last() {
                        out.spawns.insert(crate::types::closure_shape(&block.ty));
                    }
                }
                for a in args {
                    note_expr(a, out);
                }
            }
            // A variadic user `extern fun` call (gap 7) has no `thread.spawn`
            // shape, so it only walks its arguments.
            ExprKind::VariadicCall { args, .. } => {
                for a in args {
                    note_expr(a, out);
                }
            }
            ExprKind::IndirectCall { callee, args } => {
                note_expr(callee, out);
                for a in args {
                    note_expr(a, out);
                }
            }
            ExprKind::FieldAccess { base, .. }
            | ExprKind::EnumField { base, .. } => note_expr(base, out),
            ExprKind::StructLit { fields, .. } => {
                for (_, v) in fields {
                    note_expr(v, out);
                }
            }
            ExprKind::EnumLit { fields, .. } => {
                for f in fields {
                    note_expr(f, out);
                }
            }
            ExprKind::If {
                cond,
                then_block,
                else_block,
            } => {
                note_expr(cond, out);
                note_block(then_block, out);
                if let Some(b) = else_block {
                    note_block(b, out);
                }
            }
            ExprKind::Block(block) => note_block(block, out),
            ExprKind::ClosureLit { captures, .. }
            | ExprKind::ScopedClosureLit { captures, .. } => {
                for c in captures {
                    note_expr(c, out);
                }
            }
            ExprKind::Template { pieces, .. } => {
                for p in pieces {
                    if let TemplatePiece::Value(v) = p {
                        note_expr(v, out);
                    }
                }
            }
            ExprKind::ArrayLit(elems) => {
                for e in elems {
                    note_expr(e, out);
                }
            }
            _ => {}
        }
    }

    let mut out = Instantiations::default();
    for s in &program.structs {
        for f in &s.fields {
            note_ty(&f.ty, &mut out);
        }
    }
    for e in &program.enums {
        for v in &e.variants {
            for f in &v.fields {
                note_ty(&f.ty, &mut out);
            }
        }
    }
    // A `pd` is a module-scope global with a drop of its own, so its declared
    // type needs its container function set even when nothing reads it: a
    // `pd val xs = List.of(7)` no expression calls `.get` on still drops at
    // exit, and the drop is a call into that set.
    for pd in &program.pds {
        note_ty(&pd.value_ty, &mut out);
        note_expr(&pd.init, &mut out);
    }
    for f in &program.functions {
        for (_, ty) in &f.params {
            note_ty(ty, &mut out);
        }
        note_ty(&f.ret, &mut out);
        // A literal's environment fields are values the generated function
        // holds, so their types need instantiations too (a captured `Box`, a
        // captured `String`).
        if let Some(env) = &f.env {
            for (_, ty) in &env.fields {
                note_ty(ty, &mut out);
            }
        }
        if !f.extern_ {
            note_block(&f.body, &mut out);
        }
    }
    out
}
