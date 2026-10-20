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

/// Per-function state: the current block, the entry block, and the local slots.
pub(crate) struct FnGen<'g, 'a> {
    pub(crate) g: &'g Gen<'a>,
    pub(crate) cur_fn: LlvmFunction<'a>,
    pub(crate) entry: BasicBlock<'a>,
    pub(crate) cur: BasicBlock<'a>,
    /// Whether the current block already ends in a terminator, so the walk stops
    /// emitting into it. A `return` terminates; a block after one is dead code.
    pub(crate) terminated: bool,
    /// One map per lexical scope, innermost last, name to alloca slot. A slot is
    /// created in the entry block so `mem2reg` can promote it.
    pub(crate) scopes: Vec<HashMap<String, Value<'a>>>,
    /// The slot each owned binding's declaration made, keyed by the binding the
    /// drop plan recorded. A drop resolves through here rather than through
    /// `scopes`, because one spelling can name several bindings and the drop
    /// belongs to the binding that owns the value (BUG-n7Wq3).
    pub(crate) bindings: HashMap<BindingId, Value<'a>>,
    pub(crate) fn_name: String,
    /// Canonical id of the module the function was declared in. The capability
    /// gate is per module, so a gated call site has to know whose grant it is
    /// checked against (MK7 S3, `Gen::gated_check_needed`).
    pub(crate) module: String,
    pub(crate) ret_ty: Ty,
    /// The function's `tir` body, borrowed for `'a` from the program so the
    /// walk does not hold a borrow of `self`. `None` in the synthetic walk a
    /// `pd` initializer is emitted through, whose single expression is in
    /// `pd_init`.
    pub(crate) body: Option<&'a Block>,
    /// The `pd` initializer this walk emits, when it is the generated
    /// `kelp_init_pds` rather than a function body, and the global it assigns
    /// (`tir::Pd::init`, MK7 §5).
    pub(crate) pd_init: Option<&'a Expr>,
    pub(crate) pd_global: Option<Value<'a>>,
    /// The parameter list, in declaration order (a `fun` literal's environment
    /// first, when it has one).
    pub(crate) params: Vec<(String, Ty)>,
    /// The LLVM environment struct of this literal, when it captured something
    /// (S2E): `Some` makes the generated function take a hidden first `ptr`
    /// parameter and the body's `Capture` reads project into that struct.
    pub(crate) env_ty: Option<Type<'a>>,
    /// The slot holding the hidden environment pointer, bound in the entry
    /// block. One location, so `mem2reg` can promote it out.
    pub(crate) env_slot: Option<Value<'a>>,
    /// The `DISubprogram` this function's source locations hang from, when it
    /// has one (MK7 S4). `None` leaves every instruction of the body without a
    /// location.
    pub(crate) debug_scope: Option<Metadata<'a>>,
    /// The `.kp` span that names this function's own line, which is the
    /// location its entry block starts under. `None` for the synthetic walk a
    /// passdown initializer is emitted through, which has a span of its own
    /// instead.
    pub(crate) decl_span: Option<Span>,
    /// This function's generated cleanup frame (decision 125): a three-word
    /// record the runtime links on the thread's list. `None` unless the program
    /// spawns a worker, and for `naked` and `pd` initializer walks.
    pub(crate) frame: Option<Value<'a>>,
    /// The owned binding slots registered with the frame, in declaration order,
    /// with their types and their liveness flags (an `i8`, 1 while the slot holds
    /// a value the frame owns). The handler drops each one whose flag is set.
    pub(crate) registered: Vec<(Value<'a>, Ty, Value<'a>)>,
    /// The most recently entry-hoisted `alloca`, so the next one chains in
    /// right after it (`alloca`'s own doc comment) instead of always at the
    /// block's absolute start. `None` before the first one.
    pub(crate) alloca_after: Option<Value<'a>>,
    /// `naked fun` (kernel stage 5): the body is one `asm` and the asm itself
    /// returns, so no parameter slots, no `return` of the declared type, and an
    /// `unreachable` terminator after the asm.
    pub(crate) naked: bool,
    /// The loops currently being emitted, innermost last: the block a
    /// `continue` branches to (the loop's condition) and the block a `break`
    /// branches to (the loop's exit). `kelp wiki page language/iterators.md` adds the two loop
    /// control statements, and this is what gives them somewhere to go.
    pub(crate) loops: Vec<(BasicBlock<'a>, BasicBlock<'a>)>,
    /// How many `break` statements have been emitted so far in this function;
    /// `emit_while` compares it before and after a `while true` body to know
    /// whether the loop has an exit edge at all.
    pub(crate) breaks_emitted: usize,
    /// The manual block regions being emitted, innermost last (decision 139).
    /// Each one's release runs when its body ends.
    pub(crate) regions: Vec<RegionSlot<'a>>,
    /// The `regions` depth at each loop being emitted, parallel to `loops`: a
    /// `break` or `continue` releases the regions opened inside its loop.
    pub(crate) loop_regions: Vec<usize>,
}

/// One manual block region being emitted: its name (for the names of its
/// instructions), and the two slots that hold the block's element address and
/// its root handle. The slots are null and zero until a `name.box()` fills them.
#[derive(Clone)]
pub(crate) struct RegionSlot<'a> {
    pub(crate) name: String,
    pub(crate) elem: Ty,
    pub(crate) base: Value<'a>,
    pub(crate) handle: Value<'a>,
}

// ===========================================================================
// The containers (MK7 S2D): `List<T>`, `Map<K, V>`, `Box<T>` and `[T; N]`.
//
// MK7 §3.2 names the exception this section exists for: the fixed runtime stays
// C, but the monomorphized container code cannot, because the earlier emitter
// generates one function set per element or value type. So this backend
// generates the same sets into the module, one per instantiation, with the same
// function names and the same bodies the earlier emitter writes. A container's header
// is a named struct whose field order and widths match the C typedefs field for
// field, but it never crosses to C by value: the runtime helpers it calls take
// a pointer (MK7 §3.3 and BUG-vN4tR).
// ===========================================================================

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

/// Declare the libc primitives the container function sets reach, exactly as
/// the earlier emitter reaches them through the standard headers. The signatures are
/// the ones `<stdlib.h>`/`<string.h>` declare, on the ABI this machine uses:
/// `size_t` is `i64` and `int` is `i32`.
/// The argument indices one capability check reads, in the order the check
/// reads them. the earlier emitter's `checked_arg_flags` enumerates the same set; this
/// one keeps the order, which is what the two-argument `HostPort` check needs.
pub(crate) fn cap_check_indices(check: &crate::stdlib::CapCheck) -> Vec<usize> {
    use crate::stdlib::CapCheck;
    match *check {
        CapCheck::Path(i) | CapCheck::Name(i) | CapCheck::Host(i) | CapCheck::Port(i) => vec![i],
        CapCheck::HostPort(host, port) => vec![host, port],
    }
}

/// The symbol of the compiled-in allowlist array for one `(module, domain)`
/// grant, spelled exactly as `the earlier emitter's `CapTable`::symbol` spells it: the
/// array is C data in the per-program object, and the IR declaration here has to
/// name the same symbol the object defines.
pub(crate) fn cap_array_symbol(module: &str, domain: &str) -> String {
    format!(
        "kelp_caps_{}_{}",
        crate::types::c_ident(&module.replace('/', "_")),
        domain
    )
}

/// Declare the capability check's C entry points, with the signatures
/// `the runtime capability glue` defines.
///
/// The resource is a `const char*` for every check but the two that name a
/// port, where it is the `uint16_t` the grant is matched on. `count` is a
/// `size_t`, so `i64`. A declaration is free: the object only defines these when
/// the program contains a gated call, and an unreferenced declaration leaves no
/// undefined symbol behind.
pub(crate) fn declare_cap_fns<'a>(
    ctx: &'a Context,
    module: &'a Module<'a>,
) -> HashMap<&'static str, (LlvmFunction<'a>, FnType<'a>)> {
    let ptr = ctx.opaque_ptr();
    let i16t = ctx.i16();
    let i64t = ctx.i64();
    let void = ctx.void();
    let mut out = HashMap::new();
    let mut decl = |name: &'static str, params: &[Type<'a>]| {
        let ty = ctx.fn_type(void, params, false);
        let func = module.add_function(name, ty);
        extend_narrow_c_abi(func, void, params);
        out.insert(name, (func, ty));
    };
    let text = [ptr, ptr, ptr, i64t, ptr, ptr];
    decl("kelp_cap_check_path", &text);
    decl("kelp_cap_check_name", &text);
    decl("kelp_cap_check_host", &text);
    decl("kelp_cap_check_hostport", &[ptr, ptr, i16t, ptr, i64t, ptr, ptr]);
    decl("kelp_cap_check_port", &[ptr, i16t, ptr, i64t, ptr, ptr]);
    // Declared so the two `CapCheck::Name` fast paths in `emit_cap_checks` can
    // call straight into the same denial `kelp_cap_check_name`'s own loop
    // calls, rather than duplicating its message or its `exit(1)`. Every
    // other check kind still denies from inside its own runtime call and
    // never reaches this declaration directly.
    decl("kelp_cap_deny", &text);
    out
}

pub(crate) fn declare_libc<'a>(
    ctx: &'a Context,
    module: &'a Module<'a>,
) -> HashMap<&'static str, (LlvmFunction<'a>, FnType<'a>)> {
    let ptr = ctx.opaque_ptr();
    let i32t = ctx.i32();
    let i64t = ctx.i64();
    let void = ctx.void();
    let mut out = HashMap::new();
    let mut decl = |name: &'static str, ret: Type<'a>, params: &[Type<'a>]| {
        let ty = ctx.fn_type(ret, params, false);
        let func = module.add_function(name, ty);
        out.insert(name, (func, ty));
    };
    decl("malloc", ptr, &[i64t]);
    decl("realloc", ptr, &[ptr, i64t]);
    decl("free", void, &[ptr]);
    // The Box small-block free lists (speed-alloc, the runtime crate): the runtime
    // object defines them, so they are declared with the libc surface.
    decl("kelp_box_pool", ptr, &[]);
    decl("kelp_box_alloc", ptr, &[i64t, i64t]);
    decl("kelp_box_free", void, &[ptr, i64t, i64t]);
    decl("memcpy", ptr, &[ptr, ptr, i64t]);
    decl("memcmp", i32t, &[ptr, ptr, i64t]);
    decl("memset", ptr, &[ptr, i32t, i64t]);
    decl("strlen", i64t, &[ptr]);
    // The one registration an owned `pd` has: its drop point is process exit
    // (the passdown globals emitter), so the generated `kelp_init_pds`
    // hands `kelp_drop_pds` to `atexit`. Declared unconditionally like every
    // other libc symbol here; an unused declaration costs nothing.
    decl("atexit", i32t, &[ptr]);
    out
}

/// The two memory primitives a freestanding build has no libc for: `memcpy` and
/// `memset`, defined in the module with internal linkage (kernel stage 7). The
/// container bodies call them through [`Gen::mem_fn`], so a freestanding image
/// references no libc symbol. `intrinsic` forms are avoided deliberately: the
/// bodies use volatile accesses, which LLVM's loop-idiom pass will not turn into
/// an `llvm.memcpy`/`llvm.memset` call, so the RISC-V backend never emits a
/// libcall to a libc symbol that is not there.
pub(crate) fn define_freestanding_mem<'a>(
    ctx: &'a Context,
    module: &'a Module<'a>,
) -> HashMap<&'static str, (LlvmFunction<'a>, FnType<'a>)> {
    let ptr = ctx.opaque_ptr();
    let i8t = ctx.i8();
    let i32t = ctx.i32();
    let i64t = ctx.i64();
    let void = ctx.void();
    let mut out = HashMap::new();
    let builder = Builder::new(ctx);

    // `memcpy(ptr dst, ptr src, usize n)`, one volatile byte at a time.
    {
        let ty = ctx.fn_type(void, &[ptr, ptr, i64t], false);
        // The `__` prefix keeps the name out of the `kelp_`-prefixed namespace
        // a user function is mangled into, so a program that declares its own
        // `memcpy` cannot collide with this definition.
        let func = module.add_function("__kelp_freestanding_memcpy", ty);
        func.as_value().set_internal_linkage();
        let entry = func.append_block("entry");
        builder.position_at_end(entry);
        let (dst, src, n) = (func.get_param(0), func.get_param(1), func.get_param(2));
        let i_slot = builder.build_alloca(i64t, "i");
        builder.build_store(ctx.const_int(i64t, 0, false), i_slot);
        let header = func.append_block("cond");
        builder.build_br(header);
        builder.position_at_end(header);
        let i = builder.build_load(i64t, i_slot, "i");
        let more = builder.build_icmp(IntCmp::Ult, i, n, "more");
        let body = func.append_block("body");
        let exit = func.append_block("exit");
        builder.build_cond_br(more, body, exit);
        builder.position_at_end(body);
        let sp = builder.build_gep(i8t, src, i, "src");
        let dp = builder.build_gep(i8t, dst, i, "dst");
        let byte = builder.build_load(i8t, sp, "byte");
        builder.set_volatile(byte);
        let store = builder.build_store(byte, dp);
        builder.set_volatile(store);
        let next = builder.build_int_binop(IntBinOp::Add, i, ctx.const_int(i64t, 1, false), "i1");
        builder.build_store(next, i_slot);
        builder.build_br(header);
        builder.position_at_end(exit);
        builder.build_ret_void();
        out.insert("memcpy", (func, ty));
    }

    // `memset(ptr dst, int value, usize n)`, the same one volatile byte at a
    // time. The `i32` value is the libc signature's, so the call sites do not
    // change; only the low byte is stored.
    {
        let ty = ctx.fn_type(void, &[ptr, i32t, i64t], false);
        let func = module.add_function("__kelp_freestanding_memset", ty);
        func.as_value().set_internal_linkage();
        let entry = func.append_block("entry");
        builder.position_at_end(entry);
        let (dst, value, n) = (func.get_param(0), func.get_param(1), func.get_param(2));
        let byte = builder.build_int_cast(false, false, value, i8t, "byte");
        let i_slot = builder.build_alloca(i64t, "i");
        builder.build_store(ctx.const_int(i64t, 0, false), i_slot);
        let header = func.append_block("cond");
        builder.build_br(header);
        builder.position_at_end(header);
        let i = builder.build_load(i64t, i_slot, "i");
        let more = builder.build_icmp(IntCmp::Ult, i, n, "more");
        let body = func.append_block("body");
        let exit = func.append_block("exit");
        builder.build_cond_br(more, body, exit);
        builder.position_at_end(body);
        let dp = builder.build_gep(i8t, dst, i, "dst");
        let store = builder.build_store(byte, dp);
        builder.set_volatile(store);
        let next = builder.build_int_binop(IntBinOp::Add, i, ctx.const_int(i64t, 1, false), "i1");
        builder.build_store(next, i_slot);
        builder.build_br(header);
        builder.position_at_end(exit);
        builder.build_ret_void();
        out.insert("memset", (func, ty));
    }

    out
}

impl<'a> Gen<'a> {
    /// The pointer type LLVM 21 uses for every pointer.
    pub(crate) fn pty(&self) -> Type<'a> {
        self.ctx.opaque_ptr()
    }

    pub(crate) fn v_i64(&self, v: u64) -> Value<'a> {
        self.ctx.const_int(self.ctx.i64(), v, false)
    }

    pub(crate) fn v_i32(&self, v: u64) -> Value<'a> {
        self.ctx.const_int(self.ctx.i32(), v, false)
    }

    pub(crate) fn v_i8(&self, v: u64) -> Value<'a> {
        self.ctx.const_int(self.ctx.i8(), v, false)
    }

    /// The `elem`-typed address `index` elements past `ptr`, the plain
    /// non-`inbounds` GEP MK7 §3.4 wants for a data-dependent index.
    pub(crate) fn gep(&self, elem: Type<'a>, ptr: Value<'a>, index: Value<'a>, name: &str) -> Value<'a> {
        self.builder.build_gep(elem, ptr, index, name)
    }

    /// The address of field `idx` of the aggregate at `ptr`. The index is a
    /// compile-time constant inside an object of that exact type, which is the
    /// one place MK7 §3.4 allows the `inbounds` form.
    pub(crate) fn fld(&self, agg: Type<'a>, ptr: Value<'a>, idx: u32, name: &str) -> Value<'a> {
        self.builder.build_struct_gep(agg, ptr, idx, name)
    }

    pub(crate) fn ld(&self, ty: Type<'a>, ptr: Value<'a>, name: &str) -> Value<'a> {
        self.builder.build_load(ty, ptr, name)
    }

    pub(crate) fn st(&self, value: Value<'a>, ptr: Value<'a>) {
        self.builder.build_store(value, ptr);
    }

    pub(crate) fn cfn(&self, symbol: &str) -> Result<(LlvmFunction<'a>, FnType<'a>), String> {
        self.container_fns
            .get(symbol)
            .copied()
            .ok_or_else(|| no_value(&format!("the container function '{symbol}'")))
    }

    pub(crate) fn libc_fn(&self, name: &str) -> Result<(LlvmFunction<'a>, FnType<'a>), String> {
        self.libc
            .get(name)
            .copied()
            .ok_or_else(|| no_value(&format!("the libc function '{name}'")) )
    }
    pub(crate) fn cap_fn(&self, name: &str) -> Result<(LlvmFunction<'a>, FnType<'a>), String> {
        self.cap_fns
            .get(name)
            .copied()
            .ok_or_else(|| no_value(&format!("the capability check '{name}'")))
    }

    /// Append a block to `func` without moving the builder. A caller that
    /// creates several blocks before the terminator that branches to them
    /// needs exactly this: the terminator goes into the *current* block, and
    /// each destination is entered with an explicit `position_at_end`.
    pub(crate) fn block(&self, func: LlvmFunction<'a>, name: &str) -> BasicBlock<'a> {
        func.append_block(name)
    }

    /// A loop header: branch to a fresh block from wherever the builder is, and
    /// continue in it. This is the one place a block is created *and* entered,
    /// because a loop header is branched to once from the pre-header and once
    /// from the body.
    pub(crate) fn loop_header(&self, func: LlvmFunction<'a>, name: &str) -> BasicBlock<'a> {
        let bb = func.append_block(name);
        self.builder.build_br(bb);
        self.builder.position_at_end(bb);
        bb
    }

    /// Emit `kelp_panic(msg)` and terminate the block, for the bounds checks
    /// and the empty-container panics whose text must be byte-identical to the
    /// text of the runtime's own panics.
    /// Point `alloc_fns` at this project's own allocator (kernel stage 7,
    /// MK9-S5-M1): the manifest's `[target] alloc`/`dealloc`/`realloc` names,
    /// resolved to the LLVM functions `declare_functions` already declared for
    /// them. It runs after every function is in `funcs` and before any
    /// container body, because a container body calls these directly.
    pub(crate) fn resolve_freestanding_allocator(&mut self) -> Result<(), String> {
        let Some(fs) = self.freestanding.clone() else {
            return Ok(());
        };
        let (Some(alloc), Some(dealloc)) = (fs.alloc.clone(), fs.dealloc.clone()) else {
            return Ok(());
        };
        let lookup = |name: &str| -> Result<(LlvmFunction<'a>, FnType<'a>), String> {
            self.funcs
                .get(name)
                .map(|info| (info.func, info.ty))
                .ok_or_else(|| {
                    format!(
                        "the manifest's '[target]' names the allocator function '{name}', which the program does not declare"
                    )
                })
        };
        let alloc = lookup(&alloc)?;
        let dealloc = lookup(&dealloc)?;
        let realloc = match &fs.realloc {
            Some(name) => Some(lookup(name)?),
            None => None,
        };
        self.alloc_fns = Some(AllocFns {
            alloc,
            dealloc,
            realloc,
        });
        Ok(())
    }

    pub(crate) fn emit_panic(&self, msg: &str) -> Result<(), String> {
        let ptr = self.builder.global_string(msg, "panic.msg");
        self.emit_panic_ptr(ptr);
        Ok(())
    }

    /// Store a statement's file and line into the two location globals, which
    /// every check in a generated body reads (kernel stage 15).
    pub(crate) fn store_panic_site(&self, module: &str, span: Span) {
        let (file, line) = match self.sources.path_line_col(module, span) {
            Some((_, line, _)) => (self.panic_files.get(module).copied(), line),
            None => (None, 0),
        };
        let file = file.unwrap_or_else(|| self.ctx.const_null(self.ctx.opaque_ptr()));
        let line = self.ctx.const_int(self.ctx.i32(), line as u64, false);
        let file_g = self
            .panic_file_global
            .expect("a freestanding panic file global");
        let line_g = self
            .panic_line_global
            .expect("a freestanding panic line global");
        self.builder.build_store(file, file_g);
        self.builder.build_store(line, line_g);
    }

    /// Emit a call to the panic path with a message pointer the caller already
    /// built, then `unreachable`. In a freestanding build the callee is the
    /// compiler's entry, which takes the message plus the file and line the
    /// current statement stored (kernel stage 15); a hosted build still calls
    /// the runtime's one-argument `kelp_panic`.
    pub(crate) fn emit_panic_ptr(&self, msg: Value<'a>) {
        if self.freestanding.is_some() {
            let file = self.load_panic_file();
            let line = self.load_panic_line();
            self.builder
                .build_call(self.panic_fn, self.panic_ty, &[msg, file, line], "");
        } else {
            self.builder
                .build_call(self.panic_fn, self.panic_ty, &[msg], "");
        }
        self.builder.build_unreachable();
    }

    /// The two location globals a freestanding build declares before any body.
    pub(crate) fn load_panic_file(&self) -> Value<'a> {
        let global = self.panic_file_global.expect("a freestanding panic file global");
        self.builder.build_load(self.ctx.opaque_ptr(), global, "panic.file")
    }

    pub(crate) fn load_panic_line(&self) -> Value<'a> {
        let global = self.panic_line_global.expect("a freestanding panic line global");
        self.builder.build_load(self.ctx.i32(), global, "panic.line")
    }

    /// `if (i >= len) panic(msg);`, the bounds check every indexed builtin
    /// accessor opens with. The text is the runtime's own, so the message a
    /// program prints is the same one the runtime object would have printed.
    pub(crate) fn emit_bounds_check(
        &self,
        func: LlvmFunction<'a>,
        len: Value<'a>,
        i: Value<'a>,
        msg: &str,
    ) -> Result<(), String> {
        let oob = self.builder.build_icmp(IntCmp::Uge, i, len, "oob");
        let out = self.block(func, "panic");
        let ok = self.block(func, "in.bounds");
        self.builder.build_cond_br(oob, out, ok);
        self.builder.position_at_end(out);
        self.emit_panic(msg)?;
        self.builder.position_at_end(ok);
        Ok(())
    }

    /// The current CPU's index (kernel stage 8): the value of the register the
    /// kernel sets at boot, read with one inline-asm instruction (`tp` on
    /// riscv64, `TPIDR_EL1` on aarch64). The compiler never sets the register,
    /// and it never trusts a value it did not produce: in a debug build
    /// (`-O0`) the index is compared against the manifest's `[target] cpus`
    /// and panics through the project's own handler when it is out of range,
    /// while a release build trusts it (`kelp wiki page language/unsafe-ub-catalogue.md` stage 11).
    pub(crate) fn cpu_index(&self, func: LlvmFunction<'a>) -> Result<Value<'a>, String> {
        let triple = self
            .freestanding
            .as_ref()
            .map(|fs| fs.triple.as_str())
            .ok_or_else(|| no_value("'cpu_index()' outside a freestanding build"))?;
        let template = if triple.starts_with("riscv64") || triple.starts_with("riscv32") {
            "mv {0}, tp"
        } else if triple.starts_with("aarch64") || triple.starts_with("arm64") {
            "mrs {0}, TPIDR_EL1"
        } else {
            return Err(format!(
                "kelpc has no 'cpu_index()' lowering for the triple '{triple}': it reads 'tp' on riscv64 and 'TPIDR_EL1' on aarch64"
            ));
        };
        let usize_ty = self.lty2(&Ty::Usize)?;
        let fn_ty = self.ctx.fn_type(usize_ty, &[], false);
        let text = asm_operand_text(template, &[0], '$');
        let callee = self.ctx.inline_asm(fn_ty, &text, "=r", true, false);
        let idx = self
            .builder
            .build_indirect_call(callee, fn_ty, &[], "cpu");
        if self.debug_checks {
            let n = self.ctx.const_int(usize_ty, self.cpus, false);
            self.emit_bounds_check(func, n, idx, "cpu index out of range")?;
        }
        Ok(idx)
    }

    /// The inline half of `kelp_buf_reserve` for an owned buffer: test
    /// `len + extra <= cap` where the callee tests it, which is its own first
    /// line, and call the callee only when the buffer has to grow. Both sides
    /// of the comparison are `size_t` sums that wrap the same way in C, so an
    /// overflow takes the growth branch in both. The buffers stay the runtime's
    /// (`realloc` and the NUL terminator are its business).
    pub(crate) fn emit_grow_call(
        &self,
        func: LlvmFunction<'a>,
        agg: Type<'a>,
        self_p: Value<'a>,
        extra: Value<'a>,
    ) -> Result<(), String> {
        let i64t = self.ctx.i64();
        let len = self.ld(i64t, self.fld(agg, self_p, L_LEN, "len"), "len");
        let cap = self.ld(i64t, self.fld(agg, self_p, L_CAP, "cap"), "cap");
        let need = self
            .builder
            .build_int_binop(IntBinOp::Add, len, extra, "need");
        let fits = self.builder.build_icmp(IntCmp::Ule, need, cap, "fits");
        let grow = self.block(func, "grow");
        let done = self.block(func, "reserved");
        self.builder.build_cond_br(fits, done, grow);
        self.builder.position_at_end(grow);
        let reserve = self.rt("kelp_buf_reserve")?;
        let data_p = self.fld(agg, self_p, L_DATA, "data");
        let cap_p = self.fld(agg, self_p, L_CAP, "cap");
        self.call(reserve.func, reserve.ty, &[data_p, len, cap_p, extra], "");
        self.builder.build_br(done);
        self.builder.position_at_end(done);
        Ok(())
    }

    pub(crate) fn call(&self, func: LlvmFunction<'a>, ty: FnType<'a>, args: &[Value<'a>], name: &str) -> Value<'a> {
        self.builder.build_call(func, ty, args, name)
    }

    // ---- container types -------------------------------------------------

    /// Create one named LLVM struct per container instantiation the program
    /// uses, with no body yet. The bodies are set later (`set_container_bodies`
    /// for the pointer containers, and alongside the aggregates for the inline
    /// arrays), because an array's body names the element type it holds.
    pub(crate) fn declare_container_types(&mut self) -> Result<(), String> {
        let inst = self.instantiations.clone();
        let mut lists: Vec<Ty> = inst.lists.iter().cloned().collect();
        lists.sort_by_key(|t| (container_depth(t), container_sort_name(t)));
        let mut boxes: Vec<Ty> = inst.boxes.iter().cloned().collect();
        boxes.sort_by_key(|t| (container_depth(t), container_sort_name(t)));
        let mut maps: Vec<(Ty, Ty)> = inst.maps.iter().cloned().collect();
        maps.sort_by_key(|(k, v)| format!("{}{}", container_sort_name(k), container_sort_name(v)));
        let mut arrays: Vec<Ty> = inst.arrays.iter().cloned().collect();
        arrays.sort_by_key(|t| (container_depth(t), container_sort_name(t)));

        // The closure signature types first (S2E): a container element may be
        // one (`Box<closure(...)>`), a parameter certainly may, and a later
        // closure signature may name an earlier one. The earlier emitter emits these
        // before every container and `datac` for the same reason.
        let mut closures: Vec<Ty> = inst.closures.iter().cloned().collect();
        closures.sort_by_key(|t| (container_depth(t), container_sort_name(t)));
        for sig in &closures {
            let (params, ret) = match sig {
                Ty::Closure { params, ret } | Ty::ScopedFn { params, ret } => (params, ret),
                _ => continue,
            };
            let ty = self.ctx.struct_type(&closure_c_type(params, ret));
            self.containers.insert(sig.clone(), ty);
        }

        // The borrowed trait objects first (A-12 slice 4): a fat pair of
        // pointers, one identified struct per trait. Both the shared and the
        // exclusive spelling resolve to the same struct, exactly as `c_type`
        // gives them the same C name.
        let pair = self.ctx.opaque_ptr();
        let mut dyn_traits: Vec<String> = self
            .program
            .dyn_traits
            .iter()
            .cloned()
            .chain(self.program.dyn_vtables.iter().map(|v| v.trait_name.clone()))
            .collect();
        dyn_traits.sort();
        dyn_traits.dedup();
        for trait_name in &dyn_traits {
            let ty = self.ctx.struct_type(&crate::types::dyn_c_type(trait_name));
            self.ctx.struct_set_body(ty, &[pair, pair]);
            for mutable in [false, true] {
                self.containers.insert(
                    Ty::Dyn {
                        mutable,
                        trait_name: trait_name.to_string(),
                    },
                    ty,
                );
            }
            // An owned trait object is the same fat pair (A-12 slice 7).
            self.containers.insert(
                Ty::BoxDyn {
                    trait_name: trait_name.clone(),
                },
                ty,
            );
        }
        for elem in &lists {
            let ty = self.ctx.struct_type(&list_c_type(elem));
            self.containers.insert(Ty::List(Box::new(elem.clone())), ty);
        }
        for elem in &boxes {
            let ty = self.ctx.struct_type(&box_c_type(elem));
            self.containers.insert(Ty::Box(Box::new(elem.clone())), ty);
        }
        for (key, value) in &maps {
            let name = map_c_type(key, value);
            let entry = self.ctx.struct_type(&format!("{name}_entry"));
            let ty = self.ctx.struct_type(&name);
            self.map_entries
                .insert(Ty::Map(Box::new(key.clone()), Box::new(value.clone())), entry);
            self.containers
                .insert(Ty::Map(Box::new(key.clone()), Box::new(value.clone())), ty);
        }
        for arr in &arrays {
            if let Ty::Array(elem, len) = arr {
                let ty = self.ctx.struct_type(&crate::types::array_c_type(elem, *len));
                self.containers.insert(arr.clone(), ty);
            }
        }
        // An `Atomic<T>` is one field of a scalar type, so its body can be set
        // here, before any `datac` exists, exactly as a pointer container's can.
        let mut atomics: Vec<Ty> = inst.atomics.iter().cloned().collect();
        atomics.sort_by_key(container_sort_name);
        for atomic in &atomics {
            let Ty::Atomic(elem) = atomic else {
                continue;
            };
            let ty = self.ctx.struct_type(&crate::types::atomic_c_type(elem));
            let field = self.atomic_mem_lty(elem)?;
            self.ctx.struct_set_body(ty, &[field]);
            self.containers.insert(atomic.clone(), ty);
        }
        // The pointer containers have no by-value dependency on a user type, so
        // their bodies are settable now, before any `datac` type exists.
        self.set_container_bodies()
    }

    /// Set the bodies of every pointer container (`List`, `Box`, `Map`). Safe to
    /// do before any `datac` body exists: a container holds its contents through
    /// a pointer, so none of these fields names a user type by value.
    pub(crate) fn set_container_bodies(&self) -> Result<(), String> {
        let ptr = self.pty();
        let i64t = self.ctx.i64();
        let i8t = self.ctx.i8();
        for (ty, lty) in &self.containers {
            match ty {
                Ty::List(_) => self.ctx.struct_set_body(*lty, &[ptr, i64t, i64t]),
                Ty::Box(_) => self.ctx.struct_set_body(*lty, &[ptr]),
                // The closure's fat pointer, field for field
                // `write_closure_typedef`'s three members (S2E): the code
                // pointer, the environment and the environment's destructor.
                // A scoped function value is the same three members.
                Ty::Closure { .. } | Ty::ScopedFn { .. } => {
                    self.ctx.struct_set_body(*lty, &[ptr, ptr, ptr])
                }
                Ty::Map(_, _) => self.ctx.struct_set_body(
                    *lty,
                    &[ptr, i64t, i64t, i64t, ptr, i64t, i64t],
                ),
                _ => {}
            }
        }
        for (map, entry_ty) in &self.map_entries {
            if let Ty::Map(key, value) = map {
                // A `datac` key or value is held by value, and no `datac` type
                // exists yet, so that entry's body waits for
                // `set_late_map_entry_bodies`.
                if matches!(**key, Ty::Struct(_)) || matches!(**value, Ty::Struct(_)) {
                    continue;
                }
                let key_ty = llvm_type(self.ctx, &self.structs, &self.containers, key)?;
                let value_ty = llvm_type(self.ctx, &self.structs, &self.containers, value)?;
                self.ctx.struct_set_body(*entry_ty, &[key_ty, value_ty, i8t]);
            }
        }
        Ok(())
    }

    /// Set the body of every map entry that holds a `datac` by value, as a key
    /// or a value. Runs after `declare_aggregates`, because the laid-out type
    /// is what names it; the container-body pass above runs before any `datac`
    /// type exists.
    pub(crate) fn set_late_map_entry_bodies(&self) -> Result<(), String> {
        let i8t = self.ctx.i8();
        for (map, entry_ty) in &self.map_entries {
            let Ty::Map(key, value) = map else {
                continue;
            };
            if !matches!(**key, Ty::Struct(_)) && !matches!(**value, Ty::Struct(_)) {
                continue;
            }
            let key_ty = self.lty2(key)?;
            let value_ty = self.lty2(value)?;
            self.ctx.struct_set_body(*entry_ty, &[key_ty, value_ty, i8t]);
        }
        Ok(())
    }

    /// Set one array instantiation's body, after any array nested inside it
    /// (`emit_arrays_for`, the mirror of the C helper of the same name).
    pub(crate) fn emit_array_body(&self, ty: &Ty, emitted: &mut HashSet<Ty>) -> Result<(), String> {
        let Ty::Array(elem, len) = ty else {
            return Ok(());
        };
        self.emit_array_body(elem, emitted)?;
        if !emitted.insert(ty.clone()) {
            return Ok(());
        }
        let elem_ty = llvm_type(self.ctx, &self.structs, &self.containers, elem)?;
        let arr = self.ctx.array_type(elem_ty, *len);
        if let Some(lty) = self.containers.get(ty) {
            self.ctx.struct_set_body(*lty, &[arr]);
        }
        Ok(())
    }

    /// Whether a value of `ty` owes a drop call in this build. Every build but
    /// a managed one answers yes, which keeps its drops as they were. A managed
    /// build answers yes only for a resource (`gc_descr::managed_needs_drop`).
    pub(crate) fn managed_drop(&self, ty: &Ty) -> bool {
        !self.managed || crate::gc_descr::managed_needs_drop(ty, &self.resources)
    }

    /// One zeroed GC object of `elem` (the managed form of `malloc_for`): the
    /// call names the element's descriptor, so the collector knows its pointer
    /// slots. Only meaningful in a managed build.
    pub(crate) fn alloc_elem(&self, elem: &Ty, elem_ty: Type<'a>) -> Result<Value<'a>, String> {
        if !self.managed {
            return self.malloc_for(elem_ty);
        }
        let gc = self.gc.as_ref().ok_or_else(|| no_value("the managed GC runtime"))?;
        let descr = gc.descriptor(self.ctx, self.module, self, elem)?;
        let (func, fty) = gc.alloc;
        Ok(self.call(func, fty, &[descr], "gc"))
    }

    /// The GC descriptor of one element of a container buffer (`gc_descr`):
    /// a `List` buffer is an array of its element's descriptor, so the
    /// collector reads the element's pointer slots at each stride. Managed only.
    pub(crate) fn elem_descr(&self, elem: &Ty) -> Result<Value<'a>, String> {
        let gc = self.gc.as_ref().ok_or_else(|| no_value("the managed GC runtime"))?;
        gc.descriptor(self.ctx, self.module, self, elem)
    }

    /// `kelp_gc_alloc_array(descr, count)`: a zeroed GC array of `count`
    /// elements of the descriptor's type. Managed only.
    pub(crate) fn gc_array(&self, descr: Value<'a>, count: Value<'a>) -> Result<Value<'a>, String> {
        let gc = self.gc.as_ref().ok_or_else(|| no_value("the managed GC runtime"))?;
        let (func, fty) = gc.alloc_array;
        Ok(self.call(func, fty, &[descr, count], "gc.arr"))
    }

    /// Managed growth of a container buffer: a fresh GC array of `count`
    /// elements with the first `old_bytes` of the old buffer copied over. The
    /// old block is not freed; the collector reclaims it once unreachable.
    pub(crate) fn gc_grow(
        &self,
        old: Value<'a>,
        old_bytes: Value<'a>,
        descr: Value<'a>,
        count: Value<'a>,
        func: LlvmFunction<'a>,
    ) -> Result<Value<'a>, String> {
        let buf = self.gc_array(descr, count)?;
        self.check_alloc(buf, func)?;
        let (memcpy, mty) = self.mem_fn("memcpy")?;
        self.call(memcpy, mty, &[buf, old, old_bytes], "");
        Ok(buf)
    }

    /// The root descriptor of a runtime record that holds a value of `ty` (a
    /// spawn's argument or result, a Mutex or RwLock value): the value's own
    /// descriptor when it holds collected memory, and a null pointer otherwise.
    /// A hosted build has no collector, so it is always null. Section 3.
    pub(crate) fn record_descr(&self, ty: &Ty) -> Result<Value<'a>, String> {
        if !self.managed || !crate::gc_descr::record_needs_root(self, ty)? {
            return Ok(self.ctx.const_null(self.pty()));
        }
        self.elem_descr(ty)
    }

    /// The descriptor of a `Map`'s entry record `{ key, value, live }`, which
    /// is the element type of its entries buffer (`gc_descr::map_entry_layout`).
    /// `name` is the map's C name, so each map gets one global. Managed only.
    pub(crate) fn map_entry_descr(&self, key: &Ty, value: &Ty, name: &str) -> Result<Value<'a>, String> {
        let gc = self.gc.as_ref().ok_or_else(|| no_value("the managed GC runtime"))?;
        let (size, align, offsets) = crate::gc_descr::map_entry_layout(self, key, value)?;
        Ok(gc.descriptor_raw(
            self.ctx,
            self.module,
            &format!("map_entry.{name}"),
            size,
            align,
            &offsets,
        ))
    }

    /// The environment descriptor of a capturing literal, one per literal
    /// (`gc_descr::env_layout` over its captures). Managed only.
    pub(crate) fn closure_env_descr(&self, fn_name: &str) -> Result<Value<'a>, String> {
        let gc = self.gc.as_ref().ok_or_else(|| no_value("the managed GC runtime"))?;
        let env = self
            .program
            .functions
            .iter()
            .find(|f| f.name == fn_name)
            .and_then(|f| f.env.as_ref())
            .ok_or_else(|| no_value(&format!("the environment of '{fn_name}'")))?;
        let captures: Vec<Ty> = env.fields.iter().map(|(_, ty)| ty.clone()).collect();
        let (size, align, offsets) = crate::gc_descr::env_layout(self, &captures)?;
        Ok(gc.descriptor_raw(
            self.ctx,
            self.module,
            &format!("env.{fn_name}"),
            size,
            align,
            &offsets,
        ))
    }

    /// The one-field environment of a widened function value. It holds a code
    /// pointer, which is not a GC pointer, so its descriptor is noscan.
    pub(crate) fn coerce_env_descr(&self) -> Result<Value<'a>, String> {
        let gc = self.gc.as_ref().ok_or_else(|| no_value("the managed GC runtime"))?;
        let size = self.layout.abi_size_of(self.coerce_env_ty);
        let align = self.layout.abi_alignment_of(self.coerce_env_ty) as u64;
        Ok(gc.descriptor_raw(self.ctx, self.module, "coerce_env", size, align, &[]))
    }

    /// `malloc(sizeof(<ty>))` lowered to a constant byte count, through the
    /// project's own allocator when the manifest configured one.
    pub(crate) fn malloc_for(&self, ty: Type<'a>) -> Result<Value<'a>, String> {
        let size = self.layout.abi_size_of(ty);
        let align = self.layout.abi_alignment_of(ty);
        self.heap_alloc(self.v_i64(size), align as u64)
    }

    /// Whether a `Box<T>` block goes through the runtime's small-block free
    /// lists: the default libc allocator only, since a project's own allocator
    /// and a freestanding program keep their existing path.
    pub(crate) fn box_pooled(&self) -> bool {
        !self.managed && self.alloc_fns.is_none() && self.freestanding.is_none()
    }

    /// The calling thread's Box pool (`kelp_box_pool` in `kelp-rt`): one call
    /// per pooled Box operation, which returns the per-class list heads at its
    /// base. The pool is created on the thread's first use, so the inline pop
    /// and push below never see a missing pool (speed-alloc).
    pub(crate) fn box_pool(&self) -> Result<Value<'a>, String> {
        let (func, fty) = self.libc_fn("kelp_box_pool")?;
        Ok(self.call(func, fty, &[], "box.pool"))
    }

    /// The `Box<T>` allocation of `ty` in function `f`. Pooled, a block of a
    /// size class with a free block is popped from the thread's list inline;
    /// otherwise, or when the list is empty, `kelp_box_alloc` runs. Unpooled, it
    /// is `malloc_for`. The result is nullable, as `malloc` is.
    pub(crate) fn box_alloc_in(&self, f: LlvmFunction<'a>, ty: Type<'a>) -> Result<Value<'a>, String> {
        if !self.box_pooled() {
            return self.malloc_for(ty);
        }
        let size = self.layout.abi_size_of(ty);
        let align = self.layout.abi_alignment_of(ty) as u64;
        let Some(class) = box_pool_class(size, align) else {
            return self.box_alloc_call(size, align);
        };
        let pool = self.box_pool()?;
        let slot = self.builder.build_gep(self.pty(), pool, self.v_i64(class), "box.slot");
        let head = self.ld(self.pty(), slot, "box.head");
        let null = self.ctx.const_null(self.pty());
        let empty = self.builder.build_icmp(IntCmp::Eq, head, null, "box.empty");
        let pop = self.block(f, "box.pop");
        let slow = self.block(f, "box.new");
        let join = self.block(f, "box.join");
        self.builder.build_cond_br(empty, slow, pop);
        self.builder.position_at_end(pop);
        let next = self.ld(self.pty(), head, "box.next");
        self.st(next, slot);
        self.builder.build_br(join);
        self.builder.position_at_end(slow);
        let fresh = self.box_alloc_call(size, align)?;
        self.builder.build_br(join);
        self.builder.position_at_end(join);
        let out = self.builder.build_phi(self.pty(), "box");
        self.builder.add_incoming(out, &[(head, pop), (fresh, slow)]);
        Ok(out)
    }

    /// `kelp_box_alloc(size, align)`: the runtime's slow path, which refills
    /// a class from malloc or hands out a block for a size it does not pool.
    pub(crate) fn box_alloc_call(&self, size: u64, align: u64) -> Result<Value<'a>, String> {
        let (func, fty) = self.libc_fn("kelp_box_alloc")?;
        Ok(self.call(func, fty, &[self.v_i64(size), self.v_i64(align)], "box.new"))
    }

    /// The `Box<T>` release of a `size`-byte, `align`-aligned value in function
    /// `f`. Pooled, the block is pushed onto the thread's class list inline;
    /// otherwise `kelp_box_free` runs. Unpooled, it is `heap_dealloc`.
    pub(crate) fn box_free_in(&self, _f: LlvmFunction<'a>, ptr: Value<'a>, size: u64, align: u64) -> Result<(), String> {
        // A managed build frees nothing: the collector reclaims the block.
        if self.managed {
            return Ok(());
        }
        if !self.box_pooled() {
            return self.heap_dealloc(ptr, self.v_i64(size), align);
        }
        let Some(class) = box_pool_class(size, align) else {
            return self.box_free_call(ptr, size, align);
        };
        let pool = self.box_pool()?;
        let slot = self.builder.build_gep(self.pty(), pool, self.v_i64(class), "box.slot");
        let head = self.ld(self.pty(), slot, "box.head");
        self.st(head, ptr);
        self.st(ptr, slot);
        Ok(())
    }

    /// `kelp_box_free(ptr, size, align)`: the runtime's free for a block the
    /// inline path does not take, a size the pool does not keep.
    pub(crate) fn box_free_call(&self, ptr: Value<'a>, size: u64, align: u64) -> Result<(), String> {
        let (func, fty) = self.libc_fn("kelp_box_free")?;
        self.call(func, fty, &[ptr, self.v_i64(size), self.v_i64(align)], "");
        Ok(())
    }

    /// One byte-copy or zero-fill primitive: this module's own definition in a
    /// freestanding build, libc's otherwise (kernel stage 7).
    pub(crate) fn mem_fn(&self, name: &str) -> Result<(LlvmFunction<'a>, FnType<'a>), String> {
        if let Some(pair) = self.mem.get(name) {
            return Ok(*pair);
        }
        self.libc_fn(name)
    }

    /// `alloc(bytes, align)` lowered to the project's own allocator when the
    /// manifest configured one, or to libc's `malloc` otherwise (kernel stage 7,
    /// MK9-S5-M1). The result is nullable; the caller runs `check_alloc`, which
    /// routes a null to the project's panic handler with "out of memory".
    pub(crate) fn heap_alloc(&self, bytes: Value<'a>, align: u64) -> Result<Value<'a>, String> {
        if self.managed {
            // A hosted build allocates only typed objects (`alloc_elem`). An
            // untyped byte allocation has no descriptor, and a noscan one would
            // hide the pointers a `List` or `Map` buffer holds from the collector.
            return Err("a hosted build: this container allocation has no GC type descriptor yet (WP3 follow-up)".to_string());
        }
        match &self.alloc_fns {
            Some(a) => {
                let align = self.v_i64(align);
                Ok(self.call(a.alloc.0, a.alloc.1, &[bytes, align], "heap"))
            }
            None => {
                let (func, fty) = self.libc_fn("malloc")?;
                Ok(self.call(func, fty, &[bytes], "heap"))
            }
        }
    }

    /// `dealloc(ptr, bytes, align)` lowered to the project's own allocator, or
    /// to libc's `free` otherwise. The byte count and alignment are what a
    /// freestanding allocator needs and libc's `free` ignores; they come from
    /// the type whose storage is being released, so no allocation records them.
    pub(crate) fn heap_dealloc(
        &self,
        ptr: Value<'a>,
        bytes: Value<'a>,
        align: u64,
    ) -> Result<(), String> {
        if self.managed {
            // The collector reclaims the object; a managed build emits no free.
            return Ok(());
        }
        match &self.alloc_fns {
            Some(a) => {
                let align = self.v_i64(align);
                self.call(a.dealloc.0, a.dealloc.1, &[ptr, bytes, align], "");
                Ok(())
            }
            None => {
                let (func, fty) = self.libc_fn("free")?;
                self.call(func, fty, &[ptr], "");
                Ok(())
            }
        }
    }

    /// As `heap_dealloc`, for a size and align that are only known at runtime:
    /// an owned trait object reads them out of its vtable (A-12 slice 7). A
    /// project with its own allocator needs both, and libc's `free` needs
    /// neither.
    pub(crate) fn heap_dealloc_dyn(
        &self,
        ptr: Value<'a>,
        bytes: Value<'a>,
        align: Value<'a>,
    ) -> Result<(), String> {
        if self.managed {
            return Ok(());
        }
        match &self.alloc_fns {
            Some(a) => {
                self.call(a.dealloc.0, a.dealloc.1, &[ptr, bytes, align], "");
                Ok(())
            }
            None => {
                let (func, fty) = self.libc_fn("free")?;
                self.call(func, fty, &[ptr], "");
                Ok(())
            }
        }
    }

    /// `realloc(ptr, new)` lowered through the project's allocator, or libc's
    /// `realloc` otherwise. A project that names an allocator but no `realloc`
    /// gets the literal fallback its manifest documents: allocate, copy the old
    /// bytes, free the old block. The returned pointer is checked non-null here,
    /// because a `realloc` call site has no natural place to branch.
    pub(crate) fn heap_realloc(
        &self,
        old: Value<'a>,
        old_bytes: Value<'a>,
        new_bytes: Value<'a>,
        align: u64,
        func: LlvmFunction<'a>,
    ) -> Result<Value<'a>, String> {
        if self.managed {
            return Err("a hosted build: this container growth has no GC type descriptor yet (WP3 follow-up)".to_string());
        }
        match &self.alloc_fns {
            Some(a) => {
                if let Some((realloc, rty)) = a.realloc {
                    let align = self.v_i64(align);
                    let buf = self.call(
                        realloc,
                        rty,
                        &[old, old_bytes, new_bytes, align],
                        "buf",
                    );
                    self.check_alloc(buf, func)?;
                    return Ok(buf);
                }
                let new = self.heap_alloc(new_bytes, align)?;
                self.check_alloc(new, func)?;
                let (memcpy, mty) = self.mem_fn("memcpy")?;
                self.call(memcpy, mty, &[new, old, old_bytes], "");
                self.heap_dealloc(old, old_bytes, align)?;
                Ok(new)
            }
            None => {
                let (realloc, rty) = self.libc_fn("realloc")?;
                let buf = self.call(realloc, rty, &[old, new_bytes], "buf");
                self.check_alloc(buf, func)?;
                Ok(buf)
            }
        }
    }

    /// `kelp_panic("out of memory")` when `ptr` is NULL, the same check every
    /// generated C allocation makes. The block control continues in is returned,
    /// because a caller that tracks its own current block (the function-body
    /// walk) has to be told where the branch landed; the container bodies, which
    /// position the builder by hand, ignore it.
    pub(crate) fn check_alloc(&self, ptr: Value<'a>, func: LlvmFunction<'a>) -> Result<BasicBlock<'a>, String> {
        let null = self.ctx.const_null(self.pty());
        let isnull = self.builder.build_icmp(IntCmp::Eq, ptr, null, "oom");
        let oom = self.block(func, "oom");
        let ok = self.block(func, "alloc.ok");
        self.builder.build_cond_br(isnull, oom, ok);
        self.builder.position_at_end(oom);
        self.emit_panic("out of memory")?;
        self.builder.position_at_end(ok);
        Ok(ok)
    }

}
