use super::*;

impl<'a> Gen<'a> {
    pub(crate) fn new(
        ctx: &'a Context,
        module: &'a Module<'a>,
        program: &'a Program,
        drops: &'a DropPlan,
        sources: &'a SourceMap,
        layout: &'a DataLayout,
        optimized: bool,
        freestanding: Option<Freestanding>,
        unit: Option<Unit>,
        managed: Option<HeapLimits>,
    ) -> Result<Self, String> {
        let ptr = ctx.opaque_ptr();
        // The one libc symbol this slice reaches: `printf`, for `print` and
        // templates. `kelp_panic` and `kelp_assert` are the runtime's own now
        // that its object is linked, so they are declared with the rest of the
        // helpers instead of defined here.
        let printf_ty = ctx.fn_type(ctx.i32(), &[ptr], true);
        let printf_fn = module.add_function("printf", printf_ty);
        // The builtin heap layouts, from `LAYOUTS`, the one table the C prelude
        // and the runtime's own struct spellings are also checked against
        // (MK7 §3.3). Keeping the order and the widths identical is what lets a
        // pointer to one cross into the runtime; the runtime's functions all take
        // the pair by pointer or by value through the same C ABI.
        let structs = builtin_struct_types(ctx);
        let bytes_ty = structs["Bytes"];
        let string_ty = structs["String"];
        let view_ty = structs["View"];
        // The widening's one-field environment (S2E): `{ ptr f }`, where `f` is
        // the code pointer a thunk calls through. One type for every signature,
        // since a pointer is a pointer here.
        let coerce_env_ty = ctx.struct_type("kelp_ClosureCoerceEnv");
        ctx.struct_set_body(coerce_env_ty, &[ptr]);
        // A program that spawns a worker gets the generated frames (MK9 move
        // tracking, decision 125). A program without one emits none of them.
        let spawn_frames = !collect_instantiations(program).spawns.is_empty();
        // A program with a manual block region links the collector (decision
        // 139), so it gets the block entry points and the descriptors. Every
        // other program is lowered exactly as before.
        let blocks = drops.has_regions();
        let runtime = declare_runtime(
            ctx,
            module,
            bytes_ty,
            string_ty,
            view_ty,
            managed.is_some(),
            spawn_frames,
            blocks,
        );
        let panic = *runtime
            .get("kelp_panic")
            .expect("kelp_panic is declared with the runtime");
        let type_decls = type_decls(&program.structs, &program.enums);
        Ok(Gen {
            ctx,
            module,
            builder: Builder::new(ctx),
            program,
            funcs: HashMap::new(),
            printf_fn,
            printf_ty,
            panic_fn: panic.func,
            panic_ty: panic.ty,
            panic_file_global: None,
            panic_line_global: None,
            panic_files: HashMap::new(),
            layout,
            little_endian: layout.is_little_endian(),
            structs,
            enums: HashMap::new(),
            field_index: HashMap::new(),
            drops,
            runtime,
            drop_fns: HashMap::new(),
            owned: owned_types(&program.structs, &program.enums, &program.drop_types),
            containers: HashMap::new(),
            map_entries: HashMap::new(),
            container_fns: HashMap::new(),
            clone_fns: HashMap::new(),
            cmp_fns: HashMap::new(),
            hash_fns: HashMap::new(),
            eq_fns: HashMap::new(),
            closure_fns: HashMap::new(),
            dyn_fns: HashMap::new(),
            dyn_tables: HashMap::new(),
            dyn_table_tys: HashMap::new(),
            env_types: HashMap::new(),
            coerce_env_ty,
            libc: declare_libc(ctx, module),
            alloc_fns: None,
            mem: if freestanding.is_some() {
                define_freestanding_mem(ctx, module)
            } else {
                HashMap::new()
            },
            instantiations: collect_instantiations(program),
            spawn_frames,
            clonable: crate::types::clonable_types(&type_decls, &program.drop_types),
            sortable: crate::types::sortable_types(&type_decls),
            keyable: crate::types::keyable_types(&type_decls, &program.declared_keys, &program.drop_types),
            pd_globals: HashMap::new(),
            static_globals: HashMap::new(),
            cpus: freestanding.as_ref().map(|fs| fs.cpus).unwrap_or(1),
            debug_checks: !optimized,
            init_pds_fn: None,
            drop_pds_fn: None,
            argc_global: module.add_external_global("kelp_argc", ctx.i32()),
            argv_global: module.add_external_global("kelp_argv", ctx.opaque_ptr()),
            sources,
            cap_arrays: HashMap::new(),
            cap_fns: declare_cap_fns(ctx, module),
            debug: DebugBuilder::new(module, optimized),
            debug_scopes: HashMap::new(),
            pd_init_scope: None,
            freestanding,
            unit,
            managed: managed.is_some(),
            managed_heap: managed,
            gc: (managed.is_some() || blocks).then(|| crate::gc_descr::GcTypes::declare(ctx, module)),
            resources: if managed.is_some() { crate::gc_descr::resource_types(program) } else { HashSet::new() },
        })
    }

    /// Whether this pass defines the small builtin accessors (`Bytes`,
    /// `String` and `View` methods) as IR. The whole-program build does, so its
    /// own `-O2` inlines them at the call site. The incremental mode does not:
    /// the runtime object exports the same functions, and a second definition
    /// of one would be a multiple-definition error at link time. Every unit
    /// calls the runtime object's copies instead (roadmap A-14 slice 1).
    pub(crate) fn emits_runtime_accessors(&self) -> bool {
        self.unit.is_none()
    }

    /// Whether this pass emits the compiler-generated helpers (the per-type
    /// drops, clones, comparators and hash pairs, the container function sets,
    /// the closure machinery, the `&dyn` vtables, the runtime accessors and the
    /// `pd` globals). Only the `_support` unit does in the incremental mode;
    /// every other unit declares them and calls into `_support`'s definitions
    /// (roadmap A-14 slice 1).
    pub(crate) fn defines_helpers(&self) -> bool {
        self.unit.as_ref().map_or(true, |unit| unit.support)
    }

    /// Defines the trampolines of the lowered user externs this pass owns
    /// (`extern_agg::define_trampolines`).
    pub(crate) fn define_aggregate_externs(&self) -> Result<(), String> {
        crate::extern_agg::define_trampolines(
            self.ctx,
            self.module,
            self.program,
            &|f| self.defines_function(f),
            // The codegen's own LLVM types, so a trampoline and the declaration it
            // defines name the same struct types.
            &|ty| llvm_type(self.ctx, &self.structs, &self.containers, ty),
        )
    }

    /// Whether this pass emits the body of the user function `f`: every
    /// function in the whole-program mode, and a module's own functions in the
    /// incremental mode.
    pub(crate) fn defines_function(&self, f: &Function) -> bool {
        match &self.unit {
            None => true,
            Some(unit) => !unit.support && unit.name == f.module,
        }
    }

    /// Whether this pass emits the definitions of the `static`s, `section(...)`
    /// consts and functions declared in `module`: the module's own unit in the
    /// incremental mode.
    pub(crate) fn defines_module_items(&self, module: &str) -> bool {
        match &self.unit {
            None => true,
            Some(unit) => !unit.support && unit.name == module,
        }
    }

    /// Whether a definition keyed by `key` keeps external linkage: in the
    /// incremental mode only what another unit references and what `_support`
    /// defines does, since `_support`'s helpers are reachable from every unit.
    /// The whole-program mode never asks: it is the one unit and sets linkage
    /// the way it always has.
    pub(crate) fn keeps_external(&self, key: &str) -> bool {
        self.unit
            .as_ref()
            .map_or(false, |unit| unit.support || unit.exported.contains(key))
    }

    /// Declare every `datac` and `enum`, its drop, clone and comparator
    /// functions, the container function sets, the passdown globals, the
    /// capability allowlist declarations and every function.
    ///
    /// Declaring everything before any body is emitted means a construct this
    /// lowering cannot handle fails with that construct named, rather than with
    /// the first expression that mentions it.
    pub(crate) fn declare(&mut self) -> Result<(), String> {
        let program = self.program;
        // The container types first, so a `datac` or `enum` field that *is* one
        // resolves its own named struct rather than a refused type. A container
        // typedef is emitted before every `datac` in the earlier emitter for the same
        // reason (a container holds its contents through a pointer).
        self.declare_container_types()?;
        // The aggregate types next, so a field this slice cannot lay out is
        // reported by its own construct rather than behind an unrelated std
        // declaration.
        self.declare_aggregates()?;
        // A map entry that holds a `datac` by value, as a key or a value, is
        // only settable once the `datac` types exist. The earlier emitter splits the
        // same typedef for the same reason.
        self.set_late_map_entry_bodies()?;
        // The environment struct of each capturing literal (S2E), after every
        // aggregate type exists and before any signature names the `ptr` that
        // carries it.
        self.declare_closure_envs()?;
        // A `pd` is a module-scope global, the generated `kelp_init_pds` that
        // fills it in declaration order, and (for an owned one) the `atexit`
        // registration that is its only drop point. Declared here, after the
        // aggregate and container types a `pd`'s value may name, and filled in
        // by `emit_pd_globals` once the drops it needs exist.
        self.declare_pd_globals()?;
        // Each `(module, domain)` grant's compiled-in allowlist array: the data
        // is C in the per-program object (`program_c_source`, the same text and
        // the same escaping the earlier emitter writes), and this is the symbol a
        // check passes to `kelp_cap_check_*` as `patterns`.
        for grant in &program.caps {
            let symbol = cap_array_symbol(&grant.module, &grant.domain);
            let array = self.module.add_external_global(&symbol, self.ctx.opaque_ptr());
            self.cap_arrays
                .insert((grant.module.clone(), grant.domain.clone()), array);
        }
        // `program.caps` holds each module's granted patterns, which is data, not
        // a construct. The allowlist arrays are declared above and the check
        // itself is emitted at each gated call site (`cap_plan`,
        // `emit_cap_checks`), exactly where the earlier emitter emits it.

        self.declare_drop_fns();
        // The derived deep-copy and order, then the container function sets: a
        // set's own `clone`/`sort`/drop calls the generated per-`datac`
        // `kelp_clone_<name>`/`kelp_cmp_<name>`, so both exist first.
        self.declare_clone_cmp_fns();
        self.declare_container_fns()?;
        // The closure machinery (S2E): the per-signature drop, the widening
        // thunk and its destructor, and the shim a known function is widened
        // through. Declared after the container sets because a container whose
        // element is a closure (`Box<closure(...)>`) calls the element's drop.
        self.declare_closure_fns()?;
        // The `&dyn` machinery (A-12 slice 4): one thunk and one read-only
        // table per vtable the program uses. Declared after the closure
        // machinery for the same reason, and before any function body.
        self.declare_dyn_fns()?;

        let ctx = self.ctx;

        for f in &program.functions {
            // An `extern fun` has no `.kp` body: its body is supplied by the
            // compiler (`externs.rs`), so the function is *declared* here with
            // the body's own C signature and a call lowers as a call to it
            // (MK7 S3). The body then reaches the link through the object
            // `program_c_source` builds.
            if f.extern_ {
                self.declare_extern(f)?;
                continue;
            }
            let (params, ret) = if f.name == "main" {
                // The earlier emitter's `main` is `int main(int, char**)`; this
                // mirrors the type the C standard cares about and returns 0.
                // The two parameters are not Kelp parameters: `main` saves
                // them into the runtime's `kelp_argc`/`kelp_argv`, which is
                // how `process.args()` reads this process's own argv (the
                // passdown mechanism cannot carry it, because the one function
                // that can take no hidden parameter is `main`).
                (vec![ctx.i32(), ctx.opaque_ptr()], ctx.i32())
            } else {
                let mut params: Vec<Type<'a>> = Vec::new();
                // A literal that captured something receives its environment as
                // a hidden first parameter, as a `void*` the body casts back to
                // the literal's own environment struct (A-4 phase E). The
                // environment's layout stays out of the signature, which is what
                // lets two closures of one signature share a code-pointer type.
                if f.env.is_some() {
                    params.push(ctx.opaque_ptr());
                }
                // A `unit` parameter has no LLVM type at all (`llvm_type`
                // refuses it), so it takes no place in the signature and
                // nothing crosses into the call for it. The parameter list a
                // caller passes is built by the same filter in
                // `emit_call`'s argument loop, so the two cannot disagree
                // about which argument goes where.
                // An `interrupt fun` on x86-64 (kernel stage 5): `x86_intrcc` takes
                // the interrupt frame as its first pointer argument. The Kelp
                // declaration has no parameters, and the hidden one is never read.
                if f.interrupt
                    && self
                        .freestanding
                        .as_ref()
                        .is_some_and(|fs| fs.triple.starts_with("x86_64"))
                {
                    params.push(ctx.opaque_ptr());
                }
                params.extend(
                    f.params
                        .iter()
                        .filter(|(_, ty)| *ty != Ty::Unit)
                        .map(|(_, ty)| llvm_type(ctx, &self.structs, &self.containers, ty))
                        .collect::<Result<Vec<_>, _>>()?,
                );
                let ret = llvm_ret_type(ctx, &self.structs, &self.containers, &f.ret)?;
                (params, ret)
            };
            let ty = ctx.fn_type(ret, &params, false);
            let func = self.module.add_function(&symbol_name(&f.name), ty);
            // In the incremental mode this module defines only its own unit's
            // functions; a function of another module stays a declaration, and
            // a function no other unit references is `internal` so the unit
            // can optimize it the way the whole-program module does (roadmap
            // A-14 slice 1).
            if self.unit.is_some() && self.defines_function(f) && !self.keeps_external(&f.name) {
                func.as_value().set_internal_linkage();
            }
            // The `section("...")` clause a declaration wrote (kernel stage 4)
            // places its code where a linker script can find it.
            if let Some(section) = &f.section {
                func.as_value().set_section(section);
            }
            // The `align(N)` clause a declaration wrote (kernel gap 2) sets
            // the function's alignment, which is what puts a trap handler's
            // first byte at an `N`-aligned address.
            if let Some(align) = f.align {
                func.as_value().set_function_alignment(align);
            }
            // A `naked fun` (kernel stage 5): no prologue and no epilogue, and
            // never inlined, because its body's asm is the whole ABI.
            if f.naked {
                func.mark_naked();
                func.mark_noinline();
            }
            // An `interrupt fun` (kernel stage 5): the CPU enters it, so it is
            // never inlined, and the backend's interrupt lowering does the rest:
            // `x86_intrcc` on x86-64, the `interrupt` attribute on riscv64.
            if f.interrupt {
                func.mark_noinline();
                let triple = self.freestanding.as_ref().map(|fs| fs.triple.as_str()).unwrap_or("");
                if triple.starts_with("x86_64") {
                    // `x86_intrcc` takes the interrupt frame by value as its
                    // first argument: the CPU pushes RIP, CS, RFLAGS, RSP and SS.
                    let frame = ctx.literal_struct(&[ctx.i64(), ctx.i64(), ctx.i64(), ctx.i64(), ctx.i64()]);
                    func.add_parameter_byval(1, frame);
                    func.set_calling_conv(llvm_sys::LLVMCallConv::LLVMX86INTRCallConv);
                } else if triple.starts_with("riscv64") {
                    func.mark_interrupt("machine");
                } else {
                    return Err(format!(
                        "an 'interrupt fun' has no lowering for the triple '{triple}': kelpc lowers it for riscv64 and x86_64"
                    ));
                }
            }
            // A `mut` parameter is an exclusive borrow (`borrowck`), so no other
            // live pointer reaches the same object through the call and LLVM may
            // assume `noalias` of it. A shared parameter gets nothing: two
            // shared parameters may be one object passed twice.
            if f.name != "main" {
                let mut index = if f.env.is_some() { 2 } else { 1 };
                for (pname, pty) in f.params.iter().filter(|(_, pty)| *pty != Ty::Unit) {
                    // A `pd`'s hidden parameter is the exception: the same
                    // storage stays reachable through the generated global, and
                    // a callee that took the global's own address (`relay` in
                    // `tests/fixtures/pd_write_reaches_callers.kp`, which needs
                    // no hidden parameter because it only calls) writes memory
                    // this pointer would claim no one else can.
                    if !pname.starts_with("pd#") && matches!(pty, Ty::Ref { mutable: true, .. }) {
                        func.add_parameter_noalias(index);
                    }
                    index += 1;
                }
            }
            self.funcs.insert(
                f.name.clone(),
                FnInfo {
                    func,
                    ty,
                    ret_ty: f.ret.clone(),
                },
            );
        }
        // The DWARF skeleton last, because it needs each function's LLVM value
        // to attach a subprogram to, and the passdown globals for the generated
        // `kelp_init_pds`.
        self.resolve_freestanding_panic()?;
        self.resolve_freestanding_allocator()?;
        self.declare_debug();
        Ok(())
    }

    /// Points the panic path at this project's own handler (kernel stage 4),
    /// wrapped in the compiler-emitted entry that detects a double panic and
    /// carries the location (kernel stage 15). It runs after every function is
    /// declared so the user's handler is in `funcs`, and before any body,
    /// because a bounds or overflow check emitted in a body calls
    /// `self.panic_fn` directly.
    ///
    /// `panic_fn` becomes the entry (`kelp_panic_entry`), which always takes
    /// `(msg, file, line)`; the project's handler and how many arguments it
    /// takes are local to this function. When the manifest names no handler,
    /// the first-panic branch calls a generated infinite loop.
    pub(crate) fn resolve_freestanding_panic(&mut self) -> Result<(), String> {
        let Some(fs) = self.freestanding.clone() else {
            return Ok(());
        };
        let ptr = self.ctx.opaque_ptr();
        let i32t = self.ctx.i32();
        let void = self.ctx.void();

        // The state the entry reads and writes, all zero-initialised in `.bss`
        // so the linker script's own clear covers it.
        let active = self.module.add_global("kelp_panic_active", i32t);
        active.set_initializer(self.ctx.const_int(i32t, 0, false));
        active.set_section(".bss");
        let file_g = self.module.add_global("kelp_panic_file", ptr);
        file_g.set_initializer(self.ctx.const_null(ptr));
        file_g.set_section(".bss");
        let line_g = self.module.add_global("kelp_panic_line", i32t);
        line_g.set_initializer(self.ctx.const_int(i32t, 0, false));
        line_g.set_section(".bss");
        self.panic_file_global = Some(file_g);
        self.panic_line_global = Some(line_g);

        // One string global per module's source file name. A statement's
        // location is then a pointer store, not a fresh global per check.
        for f in &self.program.functions {
            if self.panic_files.contains_key(&f.module) {
                continue;
            }
            let Some((path, _, _)) = self.sources.path_line_col(&f.module, f.span) else {
                continue;
            };
            let name = path
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let mut bytes: Vec<Value<'a>> = name
                .bytes()
                .map(|b| self.ctx.const_int(self.ctx.i8(), b as u64, false))
                .collect();
            bytes.push(self.ctx.const_int(self.ctx.i8(), 0, false));
            let arr_ty = self.ctx.array_type(self.ctx.i8(), bytes.len() as u64);
            let arr = self.ctx.const_array(self.ctx.i8(), &bytes);
            let index = self.panic_files.len();
            let g = self.module.add_global(&format!("panic.file.{index}"), arr_ty);
            g.set_initializer(arr);
            g.set_constant(true);
            self.panic_files.insert(f.module.clone(), g);
        }

        // The project's handler, or a generated loop that ignores all three
        // arguments when the manifest names none.
        let (handler, handler_ty, arity) = match fs.panic.clone() {
            Some(name) => {
                let info = self.funcs.get(&name).cloned().ok_or_else(|| {
                    format!(
                        "the manifest's '[target] panic' names '{name}', which the program does not declare"
                    )
                })?;
                let arity = self
                    .program
                    .functions
                    .iter()
                    .find(|f| f.name == name && !f.extern_)
                    .map(|f| f.params.len())
                    .unwrap_or(1);
                (info.func, info.ty, arity)
            }
            None => {
                let ty = self.ctx.fn_type(void, &[ptr, ptr, i32t], false);
                let func = self.module.add_function("kelp_freestanding_panic", ty);
                func.as_value().set_internal_linkage();
                // Two blocks: the entry must have no predecessors, so it jumps
                // into the self-loop rather than being it.
                let entry_bb = func.append_block("entry");
                let loop_bb = func.append_block("loop");
                self.builder.position_at_end(entry_bb);
                self.builder.build_br(loop_bb);
                self.builder.position_at_end(loop_bb);
                self.builder.build_br(loop_bb);
                (func, ty, 3)
            }
        };

        // The weak double-panic fallback: loop on the target's wait
        // instruction. A project's own strong `kelp_double_panic`, in an object
        // linked beside this one, overrides it the way its own `memcpy` does.
        let halt_ty = self.ctx.fn_type(void, &[], false);
        let dp = self.module.add_function("kelp_double_panic", halt_ty);
        dp.as_value().set_weak_linkage();
        let dp_entry = dp.append_block("entry");
        let dp_loop = dp.append_block("loop");
        self.builder.position_at_end(dp_entry);
        self.builder.build_br(dp_loop);
        self.builder.position_at_end(dp_loop);
        let template = halt_asm_template(&fs.triple)?;
        let halt = self.ctx.inline_asm(halt_ty, template, "", true, false);
        self.builder.build_indirect_call(halt, halt_ty, &[], "");
        self.builder.build_br(dp_loop);

        // The entry every check calls. The exchange makes the re-entry test
        // atomic, so two harts that panic at once cannot both be first.
        let entry_ty = self.ctx.fn_type(void, &[ptr, ptr, i32t], false);
        let entry = self.module.add_function("kelp_panic_entry", entry_ty);
        entry.as_value().set_internal_linkage();
        let entry_bb = entry.append_block("entry");
        let call_bb = entry.append_block("call");
        let double_bb = entry.append_block("double");
        self.builder.position_at_end(entry_bb);
        let one = self.ctx.const_int(i32t, 1, false);
        let old = self
            .builder
            .build_atomicrmw(AtomicRmw::Xchg, active, one);
        let already = self.builder.build_icmp(
            IntCmp::Ne,
            old,
            self.ctx.const_int(i32t, 0, false),
            "already",
        );
        self.builder.build_cond_br(already, double_bb, call_bb);
        self.builder.position_at_end(double_bb);
        self.builder.build_call(dp, halt_ty, &[], "");
        self.builder.build_unreachable();
        self.builder.position_at_end(call_bb);
        let msg = entry.get_param(0);
        let file = entry.get_param(1);
        let line = entry.get_param(2);
        let args: Vec<Value<'a>> = if arity == 3 {
            vec![msg, file, line]
        } else {
            vec![msg]
        };
        self.builder.build_call(handler, handler_ty, &args, "");
        self.builder.build_unreachable();

        self.panic_fn = entry;
        self.panic_ty = entry_ty;
        Ok(())
    }

    /// Define one read-only data global per `section("...")` const (kernel
    /// stage 4), so a linker script can place the bytes where the clause asked.
    /// The values are the folded element list `infer.rs` built; the global is
    /// `constant` and carries the named section. Uses of the const still fold to
    /// its literal, so this global is a *symbol for the linker*, not a load: a
    /// kernel boot table is placed by the script and reached through the
    /// address, not read back through the const's name.
    pub(crate) fn emit_const_data(&self) -> Result<(), String> {
        for data in &self.program.const_data {
            // A `section("...")` const is the linker's symbol, not a load (a
            // use folds to its literal), so the units that do not own it need
            // no declaration of it: the owning unit defines it once (roadmap
            // A-14 slice 1).
            if !self.defines_module_items(&data.module) {
                continue;
            }
            let Ty::Array(elem, len) = &data.ty else {
                continue;
            };
            let llvm_elem = llvm_type(self.ctx, &self.structs, &self.containers, elem)?;
            let arr_ty = self.ctx.array_type(llvm_elem, *len);
            let values: Vec<Value<'a>> = data
                .values
                .iter()
                .map(|v| self.ctx.const_int(llvm_elem, *v as u64, *v < 0))
                .collect();
            let arr = self.ctx.const_array(llvm_elem, &values);
            let global = self.module.add_global(&symbol_name(&data.name), arr_ty);
            global.set_initializer(arr);
            global.set_constant(true);
            global.set_section(&data.section);
        }
        Ok(())
    }

    /// Declare one module-scope global per `static` (kernel stage 7), with its
    /// folded constant as the initializer. An all-zero static with no
    /// `section(...)` gets `zeroinitializer` and, being non-constant, lands in
    /// `.bss`; anything else carries its constant and an immutable one is
    /// marked `constant` for `.rodata`. A static holding an `Atomic<T>` is
    /// emitted non-constant even when the declaration is immutable, because
    /// an atomic writes through a shared borrow.
    pub(crate) fn declare_statics(&mut self) -> Result<(), String> {
        for s in &self.program.statics {
            // A `static` another unit owns stays a declaration here (roadmap
            // A-14 slice 1): the defining unit's object carries the storage and
            // this one reaches it by name.
            if !self.defines_module_items(&s.module) {
                let lty = llvm_type(self.ctx, &self.structs, &self.containers, &s.ty)?;
                let global = self.module.add_external_global(&symbol_name(&s.name), lty);
                self.static_globals.insert(
                    s.name.clone(),
                    StaticGlobal {
                        global,
                        ty: s.ty.clone(),
                        percpu: None,
                    },
                );
                continue;
            }
            if s.percpu {
                self.declare_percpu_static(s)?;
                continue;
            }
            let lty = llvm_type(self.ctx, &self.structs, &self.containers, &s.ty)?;
            let global = self.module.add_global(&symbol_name(&s.name), lty);
            let zero = static_init_is_zero_llvm(&s.init);
            let atomic = ty_contains_atomic(&s.ty, &self.program.structs);
            let bare_zero = zero && s.section.is_none();
            if bare_zero {
                global.set_initializer(self.ctx.const_null(lty));
            } else {
                global.set_initializer(self.llvm_static_init(&s.init, &s.ty)?);
            }
            global.set_constant(!s.mutable && !atomic && !bare_zero);
            if let Some(section) = &s.section {
                global.set_section(section);
            }
            if self.unit.is_none() || !self.keeps_external(&s.name) {
                global.set_internal_linkage();
            }
            self.static_globals.insert(
                s.name.clone(),
                StaticGlobal {
                    global,
                    ty: s.ty.clone(),
                    percpu: None,
                },
            );
        }
        self.emit_llvm_used();
        Ok(())
    }

    /// One `percpu` static (kernel stage 8): `[cpus x stride]`, where each
    /// element is the declared type `T` (or `{ T, pad }` when `T`'s size is not
    /// a multiple of 64), so consecutive copies start a cache line apart. The
    /// folded initializer is replicated into every copy, the global is aligned
    /// to 64 bytes and placed in `.percpu` unless the declaration wrote its own
    /// `section(...)`, and the copy count is the manifest's `[target] cpus`.
    pub(crate) fn declare_percpu_static(&mut self, s: &crate::tir::Static) -> Result<(), String> {
        let elem_lty = llvm_type(self.ctx, &self.structs, &self.containers, &s.ty)?;
        let size = self.layout.abi_size_of(elem_lty);
        // The stride has to be a multiple of 64 so every copy is cache-line
        // aligned. `T`'s size is always a multiple of its own alignment, and
        // every alignment here divides 64 (an over-64 `repr(align)` makes the
        // size itself a multiple of 64), so padding to the next multiple of 64
        // is exactly the stride.
        let pad = (64 - (size % 64)) % 64;
        let padded_name = format!("kelp_percpu_{}", s.name);
        let (stride_ty, padded) = if pad == 0 {
            (elem_lty, false)
        } else {
            let pad_ty = self.ctx.array_type(self.ctx.i8(), pad);
            let ty = self.ctx.struct_type(&padded_name);
            self.ctx.struct_set_body(ty, &[elem_lty, pad_ty]);
            (ty, true)
        };
        let arr_ty = self.ctx.array_type(stride_ty, self.cpus);
        let global = self.module.add_global(&symbol_name(&s.name), arr_ty);
        let elem_init = self.llvm_static_init(&s.init, &s.ty)?;
        let copy = if padded {
            let pad_ty = self.ctx.array_type(self.ctx.i8(), pad);
            self.ctx
                .const_named_struct(stride_ty, &[elem_init, self.ctx.const_null(pad_ty)])
        } else {
            elem_init
        };
        let copies = vec![copy; self.cpus as usize];
        global.set_initializer(self.ctx.const_array(stride_ty, &copies));
        global.set_global_alignment(64);
        // A per-CPU object is always written through, so it is never `constant`;
        // the declared `section(...)` wins, for the same reason it does on an
        // ordinary static, and the default is `.percpu`.
        global.set_section(s.section.as_deref().unwrap_or(".percpu"));
        global.set_internal_linkage();
        self.static_globals.insert(
            s.name.clone(),
            StaticGlobal {
                global,
                ty: s.ty.clone(),
                percpu: Some(PercpuStatic { stride_ty, padded }),
            },
        );
        Ok(())
    }

    /// The `@llvm.used` global that keeps every static in the object even when
    /// LLVM can constant-fold every read of it (an immutable table, a
    /// `static mut` nothing writes). The earlier emitter's `__attribute__((used))` is
    /// the same fact, and a freestanding image's section placement has to match
    /// on both.
    pub(crate) fn emit_llvm_used(&mut self) {
        // Only the statics this pass defines: a declaration of another unit's
        // static is not this module's to keep alive (roadmap A-14 slice 1).
        let owned: Vec<&crate::tir::Static> = self
            .program
            .statics
            .iter()
            .filter(|s| self.defines_module_items(&s.module))
            .collect();
        if owned.is_empty() {
            return;
        }
        let ptr = self.ctx.opaque_ptr();
        let values: Vec<Value<'a>> = owned
            .iter()
            .filter_map(|s| self.static_globals.get(&s.name).map(|sg| sg.global))
            .collect();
        let arr_ty = self.ctx.array_type(ptr, values.len() as u64);
        let arr = self.ctx.const_array(ptr, &values);
        let used = self.module.add_global("llvm.used", arr_ty);
        used.set_initializer(arr);
        used.set_appending_linkage();
        used.set_section("llvm.metadata");
    }

    /// One folded static initializer as an LLVM constant.
    pub(crate) fn llvm_static_init(
        &self,
        init: &crate::tir::StaticInit,
        ty: &Ty,
    ) -> Result<Value<'a>, String> {
        use crate::tir::StaticInit as SI;
        match (init, ty) {
            (SI::Int(v), t) => Ok(self.ctx.const_int(self.lty2(t)?, *v as u64, *v < 0)),
            (SI::Float(v), t) => Ok(self.ctx.const_real(self.lty2(t)?, *v)),
            (SI::Bool(b), _) => Ok(self.ctx.const_bool(*b)),
            (SI::Char(c), t) => Ok(self.ctx.const_int(self.lty2(t)?, *c as u64, false)),
            // An array is `{ [N x elem] v }`, so the element array is built
            // and wrapped in the named struct exactly as `emit_array_lit`
            // does at runtime.
            (SI::Array(elems), Ty::Array(elem, _len)) => {
                let elem_ty = self.lty2(elem)?;
                let values: Vec<Value<'a>> = elems
                    .iter()
                    .map(|e| self.llvm_static_init(e, elem))
                    .collect::<Result<_, _>>()?;
                let arr = self.ctx.const_array(elem_ty, &values);
                Ok(self.ctx.const_named_struct(self.cty(ty)?, &[arr]))
            }
            (SI::Repeat { value, len: _ }, Ty::Array(elem, len)) => {
                let elem_ty = self.lty2(elem)?;
                let v = self.llvm_static_init(value, elem)?;
                let values = vec![v; *len as usize];
                let arr = self.ctx.const_array(elem_ty, &values);
                Ok(self.ctx.const_named_struct(self.cty(ty)?, &[arr]))
            }
            (SI::Struct(fields), Ty::Struct(name)) => {
                let sd = self
                    .program
                    .structs
                    .iter()
                    .find(|s| &s.name == name)
                    .ok_or_else(|| no_value(&format!("the struct '{name}'")))?;
                let values: Vec<Value<'a>> = fields
                    .iter()
                    .zip(&sd.fields)
                    .map(|(value, field)| self.llvm_static_init(value, &field.ty))
                    .collect::<Result<_, _>>()?;
                let struct_ty = *self
                    .structs
                    .get(name)
                    .ok_or_else(|| no_value(&format!("the struct '{name}'")))?;
                Ok(self.ctx.const_named_struct(struct_ty, &values))
            }
            (SI::Atomic(inner), Ty::Atomic(elem)) => {
                let atom_ty = self.cty(ty)?;
                let member = self.atomic_mem_lty(elem)?;
                let value = match (inner.as_ref(), &**elem) {
                    (SI::Bool(b), _) => self.ctx.const_int(member, u64::from(*b), false),
                    _ => self.llvm_static_init(inner, elem)?,
                };
                Ok(self.ctx.const_named_struct(atom_ty, &[value]))
            }
            _ => Ok(self.ctx.const_null(self.lty2(ty)?)),
        }
    }

    /// Build the DWARF skeleton (MK7 S4, §4.4): one compile unit for the
    /// module, one `DIFile` per `.kp` source, one `DISubprogram` per
    /// `.kp`-declared function.
    ///
    /// Nothing here is a source *location* yet. The locations are given to the
    /// builder as each statement is emitted (`FnGen::locate`), which is what
    /// puts the statement, not only the function, in the line table: the same
    /// resolution one `#line` per statement bought the earlier emitter
    /// (its `write_line_marker`).
    ///
    /// The line and column come from the same `Span`s the statements carry,
    /// through `diag::SourceMap::path_line_col`, the one accessor the debug
    /// information reads.
    ///
    /// A program whose sources were never registered gets no compile unit and
    /// no subprogram at all, and every instruction then carries no location.
    pub(crate) fn declare_debug(&mut self) {
        let program = self.program;
        let sources = self.sources;
        // In the incremental mode a unit's compile unit describes that unit's
        // own `.kp` functions, so a fault in it names one of its own files
        // (roadmap A-14 slice 1); the `_support` unit has no `.kp` function and
        // so no compile unit at all.
        let unit_functions: Vec<&Function> = program
            .functions
            .iter()
            .filter(|f| match &self.unit {
                None => true,
                Some(unit) => !unit.support && unit.name == f.module,
            })
            .collect();
        // The compile unit's own file: the module that declares `main`, since
        // that is the translation unit's entry point, falling back to the first
        // declaration. A program with no function to describe has no line to
        // name, so it gets no metadata rather than an empty compile unit.
        let main_module = unit_functions
            .iter()
            .find(|f| f.name == "main" && !f.extern_)
            .or_else(|| unit_functions.iter().find(|f| !f.extern_))
            .map(|f| f.module.clone());
        let Some(main_module) = main_module else {
            return;
        };
        // One `DIFile` per module the program declares a function in. A
        // `DILocation`'s file is its scope's, and a function belongs to exactly
        // one module, so one file per module is what every location inside its
        // functions resolves through.
        let mut files: HashMap<String, Metadata<'a>> = HashMap::new();
        for f in &unit_functions {
            if files.contains_key(&f.module) {
                continue;
            }
            if let Some((path, _, _)) = sources.path_line_col(&f.module, f.span) {
                files.insert(f.module.clone(), self.debug.file(path));
            }
        }
        let Some(&main_file) = files.get(&main_module) else {
            return;
        };
        // The producer is the human-facing release name, padded the way every
        // other spelling of it is (`main.rs::release_name`).
        let producer = format!("kelpc {}", crate::release_name(crate::VERSION));
        let cu = self.debug.compile_unit(main_file, &producer);
        // A compile unit that is not named in `llvm.dbg.cu`, beside the `Debug
        // Info Version` flag, is dropped from the object: both halves are what
        // makes the sections exist.
        self.module.add_debug_compile_unit(cu);
        self.module.set_debug_info_version();

        for f in &unit_functions {
            // An `extern fun` has no body in this module: its body is
            // compiled into the program's own object (S3A), so there is no
            // instruction here to locate.
            if f.extern_ {
                continue;
            }
            let (Some(&file), Some((_, line, _))) =
                (files.get(&f.module), sources.path_line_col(&f.module, f.span))
            else {
                continue;
            };
            let ty = self.debug.subroutine_type(file);
            let sp = self.debug.subprogram(
                file,
                &f.name,
                &symbol_name(&f.name),
                file,
                line,
                ty,
            );
            if let Some(info) = self.funcs.get(&f.name) {
                self.debug.set_subprogram(info.func, sp);
            }
            self.debug_scopes.insert(f.name.clone(), sp);
        }

        // The passdown initializers are emitted into one generated function, so
        // they share one subprogram. Its file is the first passdown's, which is
        // exact for the common case of one `.kp` file and only approximate for a
        // program whose passdowns come from several: the *lines* of the
        // statements below are each passdown's own, and only the scope's file is
        // shared.
        if let Some(init_fn) = self.init_pds_fn {
            // The initializers are emitted into the generated function itself,
            // which is `_support`'s in the incremental mode; another unit only
            // has the declaration to call (roadmap A-14 slice 1).
            if !self.defines_helpers() {
                return;
            }
            // Every initializer has to be locatable before the function gets a
            // subprogram: a function that carries debug info must have a
            // location on each of its calls (LLVM's verifier says so, because
            // the inliner needs one), and one initializer from a module with no
            // registered source would leave the whole function unverifiable.
            let all_located = program
                .pds
                .iter()
                .all(|pd| sources.path_line_col(&pd.module, pd.init.span).is_some());
            if let (true, Some(pd)) = (all_located, program.pds.first()) {
                if let (Some(&file), Some((_, line, _))) = (
                    files.get(&pd.module),
                    sources.path_line_col(&pd.module, pd.init.span),
                ) {
                    let ty = self.debug.subroutine_type(file);
                    let sp = self.debug.subprogram(
                        file,
                        "kelp_init_pds",
                        "kelp_init_pds",
                        file,
                        line,
                        ty,
                    );
                    self.debug.set_subprogram(init_fn, sp);
                    self.pd_init_scope = Some(sp);
                }
            }
        }
    }

    /// Point the builder at the `.kp` line `span` of `module` names, in
    /// `scope`, so every instruction emitted until the next call to this
    /// carries it (MK7 S4, §4.4).
    ///
    /// The LLVM half of the line-marker emitter: `codegen_llvm.rs`
    /// calls it from the same places, with the same spans, so the two backends
    /// put a statement at the same source line.
    ///
    /// A span the source map cannot resolve, or a function with no subprogram,
    /// clears the location instead. An instruction that names no line is
    /// honest, and one that names a line it did not come from is not.
    pub(crate) fn locate(&self, scope: Option<Metadata<'a>>, module: &str, span: Span) {
        match (scope, self.sources.path_line_col(module, span)) {
            (Some(scope), Some((_, line, column))) => self
                .builder
                .set_debug_location(self.debug.location(line, column, scope)),
            _ => self.builder.clear_debug_location(),
        }
    }
}
