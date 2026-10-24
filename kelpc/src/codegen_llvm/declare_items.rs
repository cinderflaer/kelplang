use super::*;

impl<'a> Gen<'a> {
    /// Finalize the DWARF metadata (`LLVMDIBuilderFinalize`).
    ///
    /// This must run after the last location is attached and before the module
    /// is verified or written, so `compile` calls it between `emit_all` and
    /// `Module::verify`.
    pub(crate) fn finalize_debug(&self) {
        self.debug.finalize();
    }

    /// Declare one module-scope global per `pd`, plus the two generated
    /// functions the passdown globals emitter emits beside them (MK7 §3.1,
    /// §5).
    ///
    /// Each global has internal linkage and a zero initializer, which is C's
    /// `static <type> <name>;`: the storage is private to this object and the
    /// linker never sees the name. The name is the passdown's own
    /// `pd#<name>` identifier mangled once (`symbol_name`), so the storage a
    /// function with no hidden parameter reads is the same one the initializer
    /// filled.
    ///
    /// A `pd` initializer *is* code that runs, so a gated call inside one is
    /// checked against the *declaring* module's grant (`Pd::module` is the
    /// only carrier of that module, since an initializer is not a function).
    /// The walk that emits it carries that module for the same reason.
    pub(crate) fn declare_pd_globals(&mut self) -> Result<(), String> {
        let ctx = self.ctx;
        if self.program.pds.is_empty() {
            return Ok(());
        }
        let owned: Vec<bool> = self
            .program
            .pds
            .iter()
            .map(|pd| self.owns_memory(&pd.value_ty))
            .collect();
        for pd in &self.program.pds {
            let lty = llvm_type(ctx, &self.structs, &self.containers, &pd.value_ty)?;
            if pd.thread_local {
                // Thread-local storage (issue 234): each thread's copy starts at
                // the literal, so the global carries it as its initializer and no
                // startup code stores into it. An external declaration names no
                // initializer.
                let initial = self.tl_initial_value(pd)?;
                let global = if self.defines_helpers() {
                    let global = self.module.add_global(&symbol_name(&pd.name), lty);
                    global.set_initializer(initial);
                    if self.unit.is_none() {
                        global.set_internal_linkage();
                    }
                    global
                } else {
                    self.module.add_external_global(&symbol_name(&pd.name), lty)
                };
                global.set_thread_local_initial_exec();
                self.pd_globals
                    .insert(pd.name.clone(), (global, pd.value_ty.clone()));
                continue;
            }
            // The passdown storage and the two generated functions that fill
            // and free it are the `_support` unit's in the incremental mode
            // (roadmap A-14 slice 1): `main` and a pd's own initializer read the
            // global from outside the declaring module, so a unit that does not
            // own it declares the global and calls into `_support`'s bodies.
            let global = if self.defines_helpers() {
                let global = self.module.add_global(&symbol_name(&pd.name), lty);
                global.set_initializer(ctx.const_null(lty));
                if self.unit.is_none() {
                    global.set_internal_linkage();
                }
                global
            } else {
                self.module.add_external_global(&symbol_name(&pd.name), lty)
            };
            self.pd_globals
                .insert(pd.name.clone(), (global, pd.value_ty.clone()));
        }
        let void_fn = ctx.fn_type(ctx.void(), &[], false);
        self.init_pds_fn = Some(self.module.add_function("kelp_init_pds", void_fn));
        // An owned passdown has no scope to die at, so its drop point is process
        // exit. A program whose passdowns all own nothing gets no handler at
        // all, exactly as the earlier emitter emits none.
        if owned.iter().any(|owns| *owns) {
            self.drop_pds_fn = Some(self.module.add_function("kelp_drop_pds", void_fn));
        }
        Ok(())
    }

    /// The constant every thread's copy of a `tl` starts from (issue 234). The
    /// frontend admits only a literal, optionally negated, and a scalar type.
    pub(crate) fn tl_initial_value(&self, pd: &crate::tir::Pd) -> Result<Value<'a>, String> {
        pub(crate) fn literal(e: &crate::tir::Expr) -> Option<(i128, f64, bool, Option<bool>)> {
            use crate::tir::ExprKind as K;
            match &e.kind {
                K::IntLit(v) => Some((*v, 0.0, false, None)),
                K::CharLit(c) => Some((*c as i128, 0.0, false, None)),
                K::FloatLit(f) => Some((0, *f, true, None)),
                K::BoolLit(b) => Some((i128::from(*b), 0.0, false, Some(*b))),
                K::Unary { op: crate::ast::UnOp::Neg, expr } => {
                    let (i, f, is_float, b) = literal(expr)?;
                    Some((-i, -f, is_float, b))
                }
                _ => None,
            }
        }
        let (int, real, is_float, boolean) = literal(&pd.init)
            .ok_or_else(|| no_value(&format!("the literal initializer of 'tl {}'", pd.qualified)))?;
        let lty = llvm_type(self.ctx, &self.structs, &self.containers, &pd.value_ty)?;
        if is_float {
            return Ok(self.ctx.const_real(lty, real));
        }
        if let Some(b) = boolean {
            return Ok(self.ctx.const_int(lty, u64::from(b), false));
        }
        Ok(self.ctx.const_int(lty, int as u64, false))
    }

    /// Fill the two generated functions declared by [`Gen::declare_pd_globals`]:
    /// `kelp_drop_pds` frees every owned passdown in declaration order, and
    /// `kelp_init_pds` registers it with `atexit` and then runs each
    /// initializer in declaration order, storing the result into that
    /// passdown's global.
    ///
    /// The order is what makes a chained initializer (`pd val doubled =
    /// base * 2`) read the value an earlier one wrote. An initializer that
    /// diverges (`panic`) ends the init function, so the remaining passdowns
    /// are never assigned, which is the same fate the earlier emitter's sequential
    /// statements have.
    pub(crate) fn emit_pd_globals(&self) -> Result<(), String> {
        let Some(init_fn) = self.init_pds_fn else {
            return Ok(());
        };
        if let Some(drop_fn) = self.drop_pds_fn {
            let entry = drop_fn.append_block("entry");
            self.builder.position_at_end(entry);
            for pd in &self.program.pds {
                if pd.thread_local || !self.owns_memory(&pd.value_ty) {
                    continue;
                }
                let (global, _) = *self
                    .pd_globals
                    .get(&pd.name)
                    .ok_or_else(|| no_value(&format!("the passdown '{}'", pd.qualified)))?;
                self.call_drop(global, &pd.value_ty)?;
            }
            self.builder.build_ret_void();
        }

        let entry = init_fn.append_block("entry");
        self.builder.position_at_end(entry);
        if let Some(drop_fn) = self.drop_pds_fn {
            // The registration is generated glue with no `.kp` statement of its
            // own, but it is a call inside a function that carries a subprogram,
            // and LLVM's verifier requires such a call to have a location. It is
            // attributed to the first passdown's initializer, which is the
            // passdown whose drop it registers, through the same helper the
            // initializer statements themselves use.
            let first = self.program.pds.first();
            match first {
                Some(pd) => self.locate(self.pd_init_scope, &pd.module, pd.init.span),
                None => self.builder.clear_debug_location(),
            }
            let (atexit, ty) = self.libc_fn("atexit")?;
            self.builder
                .build_call(atexit, ty, &[drop_fn.as_value()], "");
        }
        let mut diverged = false;
        // An initializer can end in a different block than it began in (an
        // allocation's failure branch continues in `alloc.ok`), so the next
        // passdown continues where the previous one stopped, not in `entry`.
        let mut current = entry;
        for pd in self.program.pds.iter().filter(|pd| !pd.thread_local) {
            let (global, _) = *self
                .pd_globals
                .get(&pd.name)
                .ok_or_else(|| no_value(&format!("the passdown '{}'", pd.qualified)))?;
            let fg = FnGen::for_pd_init(self, pd, init_fn, entry, current, global)?;
            let (done, block) = fg.emit_pd_init()?;
            diverged = done;
            current = block;
            if diverged {
                break;
            }
        }
        if !diverged {
            self.builder.build_ret_void();
        }
        Ok(())
    }

    /// The gated-call plan for one call site, or `None` when the call is not a
    /// checked one. The mirror of the capability plan, field for
    /// field on purpose: which calls are checked, against which domain, with
    /// which arguments and which pattern count, is the security-relevant
    /// decision, and two backends answering it differently would be a hole in
    /// one of them.
    ///
    /// The whole gate is frontend: resolution already refuses an ungranted
    /// name, so a call that reaches codegen is one the calling module may make.
    /// What the check adds is the *pattern* test at the call site, and a call
    /// that names no resource (`time.now_unix`, `rand.below`, `process.exit`)
    /// needs none, exactly as it gets none from the earlier emitter.
    ///
    /// A gated stdlib module calling its own item is a checked call site only
    /// when something handed that module patterns for its own domain (an
    /// `impl "std/env" caps(env = [...])` line): with no such grant the
    /// module never appears in `program.caps` for that domain and the `?`
    /// below returns `None`.
    pub(crate) fn cap_plan(
        &self,
        module: &str,
        callee: &str,
        args: &[Expr],
        span: Span,
    ) -> Option<CapPlan> {
        let domain = self.program.gated.get(callee)?;
        let grant = self
            .program
            .caps
            .iter()
            .find(|g| g.module == module && g.domain == *domain)?;
        let arg_is_str: Vec<bool> = args
            .iter()
            .map(|a| crate::stdlib::is_str_like_ty(&a.ty))
            .collect();
        // The per-call table is keyed on the bare name the author wrote
        // (`read`, not `fs.read`), the qualified key being what `gated` holds.
        let bare = callee.rsplit('.').next().unwrap_or(callee);
        let mut checks = crate::stdlib::checked_args(domain, bare, &arg_is_str);
        // Mirror of the capability plan's elision, field for field:
        // a path domain's whole-resource pattern (`/**`) matches any string
        // unconditionally in `glob_match` (`std/rt.kp`, the `**` branch's
        // `if rest == plen { return true }` fast path), so when the calling
        // module's own grant for this domain holds that literal pattern, a
        // `CapCheck::Path` here is provably always going to pass and is
        // dropped before codegen rather than emitted as a runtime call. Name
        // domains (`env`, `net`) are not elided by this rule: their
        // whole-resource pattern is a lone `*`, which does not cross a `/`
        // byte in `glob_match` and so is not a true unconditional match.
        if crate::stdlib::patterns_are_paths(domain)
            && grant.patterns.iter().any(|p| p == crate::stdlib::whole_resource(domain))
        {
            checks.retain(|c| !matches!(c, crate::stdlib::CapCheck::Path(_)));
        }
        // Mirror of the name-domain elision in the capability plan,
        // field for field: a domain-declaring module's own forwarding call (the
        // thin wrapper over its `extern fun`) needs no `CapCheck::Name` when its
        // own list for the domain is exactly the whole-resource `*`, because an
        // `env` grant can never name a `/` and `*` crosses none, so the caller's
        // own check has already admitted a name the whole-resource list admits.
        // A narrower list is the effective gate and keeps its check; a caller
        // that is not the domain's defining module is never elided, so an
        // ordinary `*` holder keeps its own check and its own slashy-name
        // denial.
        if !crate::stdlib::patterns_are_paths(domain)
            && crate::stdlib::domain_of(module) == Some(domain.as_str())
            && grant.patterns.len() == 1
            && grant.patterns[0] == crate::stdlib::whole_resource(domain)
        {
            checks.retain(|c| !matches!(c, crate::stdlib::CapCheck::Name(_)));
        }
        if checks.is_empty() {
            return None;
        }
        // The call site, as the author wrote it: the same `locate` the C
        // backend calls, so a denial names the same `file:line:col`.
        let at = self
            .sources
            .locate(module, span)
            .unwrap_or_else(|| module.to_string());
        Some(CapPlan {
            domain: domain.clone(),
            pattern_count: grant.patterns.len(),
            patterns: grant.patterns.clone(),
            at,
            checks,
        })
    }

    /// Declare one `extern fun` as the C function its compiler-supplied body
    /// defines (MK7 S3).
    ///
    /// The declared type is the **C ABI's** shape of that function, not its
    /// written spelling. An aggregate larger than two eightbytes comes back
    /// through a hidden pointer in the first argument slot, which is what clang
    /// compiles the body's `return` into; LLVM's own lowering of an aggregate
    /// *return* is not that, so declaring the return by value would produce a
    /// call that segfaults against the clang object (BUG-vN4tR, MK7 §3.3).
    ///
    /// Nothing here can see that object, so the agreement is kept by reading, and
    /// the shapes it depends on are stated rather than assumed:
    ///
    /// * every parameter is a pointer or a scalar. `infer.rs`'s
    ///   `implicit_borrow_wrap` turns a parameter that owns memory into a borrow,
    ///   and the runtime's own text uses `kelp_data->len` accordingly, so no
    ///   aggregate crosses *into* the body by value. The guard below stays so
    ///   that a future extern whose parameter is a bare aggregate fails by name
    ///   rather than crossing wrongly.
    /// * the only aggregate returns in the whole extern table are `Bytes` and
    ///   `String`, both 24 bytes, so both take `sret`; `str` is a bare pointer
    ///   and every other return is a scalar.
    pub(crate) fn declare_extern(&mut self, f: &Function) -> Result<(), String> {
        let ctx = self.ctx;
        let ret_llvm = if f.ret == Ty::Unit {
            None
        } else {
            Some(llvm_type(ctx, &self.structs, &self.containers, &f.ret)?)
        };
        let symbol = symbol_name(&f.name);
        // A user extern that takes or returns an aggregate by value is lowered to
        // IR by `extern_agg`, which defines its body in the owning unit.
        let lowered = crate::extern_agg::lowered_in_ir(self.program, f);
        // The bare name inside the module, the last dot-separated component,
        // which is how `externs.rs` keys a body: `(module, bare name)`.
        let bare = f.name.rsplit('.').next().unwrap_or(&f.name);
        // `std/math`'s square root is LLVM's own `llvm.sqrt.f64` rather than the
        // body `externs.rs` carries: the body is a `call` at every use, and a
        // call in the loop is what keeps the SLP vectorizer from packing the
        // arithmetic around it (its remark is `call instruction cannot be
        // vectorized`). The intrinsic is the same `sqrtsd` the body's `sqrt`
        // compiles to; only the errno a C call would set is lost, and Kelp has
        // no errno.
        if f.module == "std/math" && bare == "sqrt_raw" {
            let Some(ret) = ret_llvm else {
                return Err(no_value("the return type of 'sqrt_raw'"));
            };
            let ty = ctx.fn_type(ret, &[ret], false);
            let func = self.module.add_function("llvm.sqrt.f64", ty);
            self.runtime.insert(
                symbol,
                RtFn {
                    func,
                    ty,
                    yields: true,
                    ret: ret_llvm,
                    sret: false,
                },
            );
            return Ok(());
        }
        let sret = ret_llvm
            .map(|ty| crosses_c_boundary_by_value(&f.ret, ty, self.layout))
            .unwrap_or(false);
        let mut params: Vec<Type<'a>> = Vec::new();
        if sret {
            params.push(ctx.opaque_ptr());
        }
        for (pname, pty) in &f.params {
            let llvm_pty = llvm_type(ctx, &self.structs, &self.containers, pty)?;
            // A `move closure fun(...)` parameter of a user extern is the
            // native-callback shape (gap 4). A closure is a three-pointer fat
            // pointer, 24 bytes, which `rt_arg_value` already passes through a
            // stack slot; the per-program object's trampoline takes that
            // pointer (`write_extern`'s `closure_by_ref`) and boxes the
            // closure for the native side, so the declaration is a plain
            // pointer, exactly as for any aggregate past two eightbytes.
            if matches!(pty, Ty::Closure { .. })
                && self.program.extern_ffi_symbols.contains_key(&f.name)
            {
                params.push(ctx.opaque_ptr());
                continue;
            }
            if crosses_c_boundary_by_value(pty, llvm_pty, self.layout) {
                // A lowered extern takes the pointer to the caller's copy, which
                // its trampoline reads (`extern_agg`).
                if lowered {
                    params.push(ctx.opaque_ptr());
                    continue;
                }
                return Err(refuse(
                    &format!("the extern '{}' taking '{}' by value", f.name, pname),
                    "S3",
                ));
            }
            params.push(llvm_pty);
        }
        // A struct that comes back through the hidden pointer returns `void`
        // here, and the destination is argument 0: that is the shape clang
        // compiles the C function's return into, and it is not what an LLVM
        // *value* return of the same struct lowers to (BUG-vN4tR).
        let ret = match (sret, ret_llvm) {
            (true, _) => ctx.void(),
            (false, Some(ty)) => ty,
            (false, None) => ctx.void(),
        };
        let ty = ctx.fn_type(ret, &params, false);
        let symbol = symbol_name(&f.name);
        let func = self.module.add_function(&symbol, ty);
        extend_narrow_c_abi(func, ret, &params);
        // The declarations live in the same table as the runtime helpers, and a
        // call reaches either through the same lookup (`emit_call`): a body and
        // a runtime helper differ in where their text is compiled, not in how
        // the IR calls them.
        self.runtime.insert(
            symbol,
            RtFn {
                func,
                ty,
                yields: ret_llvm.is_some(),
                ret: ret_llvm,
                sret,
            },
        );
        Ok(())
    }

    /// Declare one identified LLVM struct type per `datac`, with the fields in
    /// declaration order and the target's natural alignment (MK7 §3.1).
    ///
    /// Field types are checked before any type is created, so a `datac` this
    /// slice cannot lay out fails with the construct named and nothing half
    /// declared. Every named type is created first and its body set afterwards,
    /// so a field may refer to a `datac` declared later in the vector; the
    /// frontend already guarantees the by-value dependency is acyclic, and the
    /// order here only has to be *a* complete one.
    /// Declare one identified LLVM type per `datac` and per `enum`, and set
    /// every body, in the program's merged declaration order (MK7 §3.1).
    ///
    /// Field and payload types are checked before any type is created, so an
    /// aggregate this slice cannot lay out fails with the construct named and
    /// nothing half declared. Every named type is created first and its body set
    /// afterwards, so a field may name a type whose body is set later in the
    /// same pass; the frontend already guarantees the by-value dependency is
    /// acyclic, and `order` (`types::StructDef::order` / `EnumDef::order`) is
    /// that topological order, shared by the two lists.
    ///
    /// The tagged union is where that order matters for a second reason: the
    /// union's size and alignment are read back from the target's data layout,
    /// so every payload type it measures is complete by the time it is built.
    pub(crate) fn declare_aggregates(&mut self) -> Result<(), String> {
        let program = self.program;
        let ctx = self.ctx;
        for s in &program.structs {
            for f in &s.fields {
                refuse_field(&f.ty)?;
            }
        }
        for e in &program.enums {
            for v in &e.variants {
                // A `unit` field carries no data (`MISSING-r7Wqb`): it is
                // never laid out, so it is never checked against the layouts
                // `refuse_field` recognizes.
                for f in v.real_fields() {
                    refuse_field(&f.ty)?;
                }
            }
        }

        // Create every named type before any body is set, so a body may refer
        // to a type whose own body comes later in this same pass.
        for s in &program.structs {
            let ty = ctx.struct_type(&struct_tag(&s.name));
            self.structs.insert(s.name.clone(), ty);
            // A `repr(align(N))` body carries a hidden zero-length alignment
            // anchor as its first field, which shifts every real field's index
            // by one; the name table is what `field_slot` reads an index from,
            // so the shift lives here and nowhere else.
            let mut names: Vec<String> = s.fields.iter().map(|f| f.name.clone()).collect();
            if matches!(s.repr, crate::ast::Repr::Align(_)) {
                names.insert(0, DATAC_ALIGN_FIELD.to_string());
            }
            self.field_index.insert(s.name.clone(), names);
        }
        for e in &program.enums {
            let llvm_ty = ctx.struct_type(&struct_tag(&e.name));
            self.structs.insert(e.name.clone(), llvm_ty);
            let union_ty = ctx.struct_type(&enum_union_name(&e.name));
            // A managed build's enum tag is always an i32 (the collector's
            // descriptor reads a u32 at tag_off), whatever `repr` names.
            let (tag_bytes, tag_signed) = if self.managed { (4, true) } else { e.tag_storage() };
            let tag_ty = enum_tag_llvm_type(ctx, tag_bytes);
            let variants = e
                .variants
                .iter()
                .enumerate()
                .map(|(i, v)| EnumVariant {
                    // A payload-less variant is a bare `i8` rather than an empty
                    // struct: an empty struct is zero bytes in LLVM and one in
                    // C, and the one byte is what sizes a union every variant of
                    // which carries nothing. A variant whose fields are every
                    // one `unit` is payload-less by the same rule
                    // (`VariantDef::is_payload_less`, `MISSING-r7Wqb`): a `unit`
                    // field stores nothing, so `Ok`/`Some` instantiated with
                    // `unit` is exactly as bare as `None`/`Err`'s empty case.
                    payload_ty: if v.is_payload_less() {
                        ctx.i8()
                    } else {
                        ctx.struct_type(&enum_payload_name(&e.name, i))
                    },
                    fields: v.fields.iter().map(|f| f.ty.clone()).collect(),
                })
                .collect();
            self.enums.insert(
                e.name.clone(),
                EnumInfo {
                    llvm_ty,
                    tag_ty,
                    tag_signed,
                    union_ty,
                    variants,
                },
            );
        }

        // Set every body in the merged declaration order, so a field or payload
        // only ever names a type whose body is already complete.
        enum Decl<'p> {
            Record(&'p crate::types::StructDef),
            Sum(&'p crate::types::EnumDef),
        }
        let mut decls: Vec<(usize, Decl)> = program
            .structs
            .iter()
            .map(|s| (s.order, Decl::Record(s)))
            .chain(program.enums.iter().map(|e| (e.order, Decl::Sum(e))))
            .collect();
        decls.sort_by_key(|(order, _)| *order);
        // An array field is held *by value*, so its body has to be set before the
        // `datac` or payload that names it. `emit_array_body` recurses into the
        // arrays nested inside it, innermost first, exactly as the earlier emitter
        // emits them just before the declaration that needs them.
        let mut emitted_arrays: HashSet<Ty> = HashSet::new();
        for (_, decl) in decls {
            let field_tys: Vec<Ty> = match decl {
                Decl::Record(s) => s.fields.iter().map(|f| f.ty.clone()).collect(),
                Decl::Sum(e) => e
                    .variants
                    .iter()
                    .flat_map(|v| v.fields.iter().map(|f| f.ty.clone()))
                    .collect(),
            };
            for ty in &field_tys {
                self.emit_array_body(ty, &mut emitted_arrays)?;
            }
            match decl {
                Decl::Record(s) => {
                    let ty = *self
                        .structs
                        .get(&s.name)
                        .expect("every datac got a type above");
                    let mut fields = Vec::new();
                    // The anchor is field 0, so LLVM gives the type the
                    // alignment `repr(align(N))` asked for, exactly as the C
                    // `__attribute__((aligned(N)))` does. It is zero bytes, so
                    // it never moves a real field.
                    if let crate::ast::Repr::Align(n) = s.repr {
                        fields.push(alignment_anchor(ctx, n));
                    }
                    fields.extend(
                        s.fields
                            .iter()
                            .map(|f| llvm_type(ctx, &self.structs, &self.containers, &f.ty))
                            .collect::<Result<Vec<_>, _>>()?,
                    );
                    ctx.struct_set_body_packed(
                        ty,
                        &fields,
                        matches!(s.repr, crate::ast::Repr::Packed),
                    );
                }
                Decl::Sum(e) => self.declare_enum_body(e)?,
            }
        }
        // Every array the program uses that no by-value declaration pulled in
        // earlier: a local, a parameter, a return value, a `Box` element.
        let arrays: Vec<Ty> = self.instantiations.arrays.iter().cloned().collect();
        for ty in &arrays {
            self.emit_array_body(ty, &mut emitted_arrays)?;
        }
        Ok(())
    }

    /// Set one `enum`'s three bodies: each variant's payload struct, the union,
    /// then the outer `{ i32, union }`.
    ///
    /// The union's shape is the whole subtlety. C's `union { struct v0; struct
    /// v1; }` places every member at offset 0 inside the union and sizes the
    /// union to its widest member aligned to its widest alignment. LLVM has no
    /// union type and no per-field alignment, so the union becomes a struct
    /// **anchored on an integer type with the union's own alignment** and padded
    /// out to the union's size. The anchor is what makes LLVM align the union
    /// field to the payload's alignment, which is the offset C gives it; the
    /// padding is what makes the whole `enum` the size C gives it. Both numbers
    /// come from the target's data layout rather than from x86-64 arithmetic.
    pub(crate) fn declare_enum_body(&mut self, e: &crate::types::EnumDef) -> Result<(), String> {
        let ctx = self.ctx;
        let info = self
            .enums
            .get(&e.name)
            .cloned()
            .ok_or_else(|| no_value(&format!("the enum '{}'", e.name)))?;
        for (i, v) in e.variants.iter().enumerate() {
            let payload_ty = info.variants[i].payload_ty;
            if v.is_payload_less() {
                continue;
            }
            let fields = v
                .real_fields()
                .map(|f| llvm_type(ctx, &self.structs, &self.containers, &f.ty))
                .collect::<Result<Vec<_>, _>>()?;
            ctx.struct_set_body(payload_ty, &fields);
        }
        let body = self.enum_union_body(&info);
        ctx.struct_set_body(info.union_ty, &body);
        ctx.struct_set_body(info.llvm_ty, &[info.tag_ty, info.union_ty]);
        Ok(())
    }

    /// The union's field list: the alignment anchor, then a byte array filling
    /// out to the union's size. The byte count is exactly what makes the `enum`
    /// the size its widest payload rounds up to, so a value of the type is the
    /// same object the C typedef declares.
    pub(crate) fn enum_union_body(&self, info: &EnumInfo<'a>) -> Vec<Type<'a>> {
        let mut align = 1u32;
        let mut size = 0u64;
        for v in &info.variants {
            align = align.max(self.layout.abi_alignment_of(v.payload_ty));
            size = size.max(self.layout.abi_size_of(v.payload_ty));
        }
        let union_size = round_up(size, align);
        // An integer of exactly the union's alignment: LLVM gives `iN` the
        // alignment of the smallest power-of-two byte width that covers it, so
        // the anchor is what forces the union field to `align`.
        let anchor = self.ctx.int(align * 8);
        let anchor_size = self.layout.abi_size_of(anchor);
        let pad = union_size.saturating_sub(anchor_size);
        if pad == 0 {
            return vec![anchor];
        }
        vec![anchor, self.ctx.array_type(self.ctx.i8(), pad)]
    }

    /// Whether a value of `ty` owns memory in this program, which is exactly
    /// "a drop is owed for it" (`types::needs_drop`). The distinction matters at
    /// a staged temporary: an all-scalar `datac` may be staged for its address
    /// freely, while one that owns memory has to reach a drop site.
    pub(crate) fn owns_memory(&self, ty: &Ty) -> bool {
        needs_drop(ty, &self.owned)
    }

    /// Declare one drop function per `datac` or `enum` that owns memory
    /// (`types::needs_drop`), the mirror of the routine the earlier emitter wrote
    /// beside each typedef. The plan's targets call them by name.
    pub(crate) fn declare_drop_fns(&mut self) {
        let owned = owned_types(&self.program.structs, &self.program.enums, &self.program.drop_types);
        for s in &self.program.structs {
            if !needs_drop(&Ty::Struct(s.name.clone()), &owned)
                || !self.managed_drop(&Ty::Struct(s.name.clone()))
            {
                continue;
            }
            let name = format!("kelp_drop_{}", crate::types::c_ident(&s.name));
            let ty = self
                .ctx
                .fn_type(self.ctx.void(), &[self.ctx.opaque_ptr()], false);
            let func = self.module.add_function(&name, ty);
            self.drop_fns.insert(s.name.clone(), (func, ty));
        }
        // An `enum` whose own name is in `owned` has at least one variant with
        // an owned payload field, so its drop has at least one case.
        for e in &self.program.enums {
            if !needs_drop(&Ty::Struct(e.name.clone()), &owned)
                || !self.managed_drop(&Ty::Struct(e.name.clone()))
            {
                continue;
            }
            let name = format!("kelp_drop_{}", crate::types::c_ident(&e.name));
            let ty = self
                .ctx
                .fn_type(self.ctx.void(), &[self.ctx.opaque_ptr()], false);
            let func = self.module.add_function(&name, ty);
            self.drop_fns.insert(e.name.clone(), (func, ty));
        }
    }

    /// Define each generated drop: for a `datac`, one call per owned field in
    /// declaration order, on the field's own address; for an `enum`, one
    /// `switch` on the live tag, so only the live variant's owned payload fields
    /// are freed (MK7 §3.1, "one `switch` per drop"). A builtin buffer goes
    /// straight to the runtime's drop; a nested owned aggregate recurses through
    /// its own drop.
    pub(crate) fn emit_drop_fn_bodies(&self) -> Result<(), String> {
        let owned = owned_types(&self.program.structs, &self.program.enums, &self.program.drop_types);
        for s in &self.program.structs {
            let Some((func, _)) = self.drop_fns.get(&s.name) else {
                continue;
            };
            let entry = func.append_block("entry");
            self.builder.position_at_end(entry);
            let struct_ty = *self
                .structs
                .get(&s.name)
                .ok_or_else(|| no_value(&format!("the datac '{}'", s.name)))?;
            let self_ptr = func.get_param(0);
            if self.program.drop_types.contains(&s.name) {
                self.call_user_drop(self_ptr, &s.name)?;
            }
            for (index, field) in s.fields.iter().enumerate() {
                if !needs_drop(&field.ty, &owned) {
                    continue;
                }
                let field_ptr =
                    self.builder
                        .build_struct_gep(struct_ty, self_ptr, index as u32, &field.name);
                self.call_drop(field_ptr, &field.ty)?;
            }
            self.builder.build_ret_void();
        }
        for e in &self.program.enums {
            if !self.drop_fns.contains_key(&e.name) {
                continue;
            }
            self.emit_enum_drop_body(e, &owned)?;
        }
        Ok(())
    }

    /// One `enum`'s drop: `switch (tag) { case v: <drop v's owned payload>
    /// ... }`, which is the earlier emitter's version of the same function and the
    /// only place a tagged union frees anything.
    ///
    /// Only a variant with at least one owned payload field gets a case, exactly
    /// as `write_enum` skips the variants with nothing to free. The default block
    /// is the exit and there is no runtime tag check: every tag a program can
    /// produce is one of the variants, and the frontend's exhaustiveness is what
    /// says so.
    pub(crate) fn emit_enum_drop_body(&self, e: &crate::types::EnumDef, owned: &HashSet<String>) -> Result<(), String> {
        let (func, _) = *self
            .drop_fns
            .get(&e.name)
            .ok_or_else(|| no_value(&format!("the drop of '{}'", e.name)))?;
        let info = self
            .enums
            .get(&e.name)
            .cloned()
            .ok_or_else(|| no_value(&format!("the enum '{}'", e.name)))?;
        let entry = func.append_block("entry");
        self.builder.position_at_end(entry);
        let self_ptr = func.get_param(0);
        if self.program.drop_types.contains(&e.name) {
            self.call_user_drop(self_ptr, &e.name)?;
        }
        let tag_ptr = self
            .builder
            .build_struct_gep(info.llvm_ty, self_ptr, ENUM_TAG_FIELD, "tag");
        let tag = self.builder.build_load(info.tag_ty, tag_ptr, "tag");

        let done = func.append_block("drop.end");
        let cases: Vec<(usize, BasicBlock<'a>)> = e
            .variants
            .iter()
            .enumerate()
            .filter(|(_, v)| v.fields.iter().any(|f| needs_drop(&f.ty, owned)))
            .map(|(i, _)| (i, func.append_block(&format!("drop.v{i}"))))
            .collect();
        let switch = self.builder.build_switch(tag, done, cases.len() as u32);
        for (index, block) in &cases {
            let on = self.ctx.const_int(info.tag_ty, *index as u64, false);
            self.builder.add_case(switch, on, *block);
        }
        for (index, block) in &cases {
            self.builder.position_at_end(*block);
            let payload_ty = info.variants[*index].payload_ty;
            let union_ptr = self
                .builder
                .build_struct_gep(info.llvm_ty, self_ptr, ENUM_UNION_FIELD, "kelp_v");
            for (fi, field) in e.variants[*index].fields.iter().enumerate() {
                if !needs_drop(&field.ty, owned) {
                    continue;
                }
                // A `unit` field is never owned (`needs_drop` already skipped
                // it above), but a field *after* one in the same variant still
                // needs its position in the payload struct rather than its
                // declared position (`MISSING-r7Wqb`).
                let payload_index = info.variants[*index].payload_index(fi);
                let field_ptr =
                    self.builder
                        .build_struct_gep(payload_ty, union_ptr, payload_index, &field.name);
                self.call_drop(field_ptr, &field.ty)?;
            }
            self.builder.build_br(done);
        }
        self.builder.position_at_end(done);
        self.builder.build_ret_void();
        Ok(())
    }

    /// Call one type's declared destructor (`fun mut T.drop()`) at the top of
    /// its generated drop (kernel stage 9, MK9-S6-M1). `declare_functions` has
    /// already declared the body as an ordinary method, so the LLVM function is
    /// in `funcs` under the same `Type#name` key the earlier emitter mangles.
    pub(crate) fn call_user_drop(&self, ptr: Value<'a>, name: &str) -> Result<(), String> {
        let key = crate::types::drop_method_key(name);
        let info = self
            .funcs
            .get(&key)
            .ok_or_else(|| no_value(&format!("the declared destructor '{key}'")))?;
        self.builder.build_call(info.func, info.ty, &[ptr], "");
        Ok(())
    }

    /// Free the value at `ptr`: the runtime's builtin free, or the generated
    /// per-`datac` drop. A type that owns nothing has no call, which is the same
    /// silence the earlier emitter keeps for a dropless field.
    pub(crate) fn call_drop(&self, ptr: Value<'a>, ty: &Ty) -> Result<(), String> {
        // A managed build calls no drop for memory alone: the collector reclaims
        // it, so the drop of a memory-only value is no call at all.
        if !self.managed_drop(ty) {
            return Ok(());
        }
        let (func, fty) = match ty {
            Ty::Bytes | Ty::String => {
                let symbol = drop_fn_name(ty);
                let rt = self
                    .runtime
                    .get(&symbol)
                    .ok_or_else(|| no_value(&format!("the runtime drop '{symbol}'")))?;
                (rt.func, rt.ty)
            }
            Ty::Struct(name) => match self.drop_fns.get(name) {
                Some((func, fty)) => (*func, *fty),
                // An unowned `datac` has no drop function and nothing to free.
                None => return Ok(()),
            },
            // A container's drop is one of the monomorphized functions this
            // slice generates into the module (`kelp_list_drop__i32`), and an
            // owned trait object's is one per trait (`kelp_box_dyn_drop__X`),
            // declared with the container set for exactly this reason
            // (A-12 slice 7).
            Ty::List(_) | Ty::Box(_) | Ty::BoxDyn { .. } | Ty::Map(..) | Ty::Array(..) => {
                let symbol = drop_fn_name(ty);
                match self.container_fns.get(&symbol).copied() {
                    Some((func, fty)) => (func, fty),
                    None => return Ok(()),
                }
            }
            // A closure owns the environment its captures were moved into, so
            // its drop is the per-signature generated one (S2E). A plain
            // function value is `Copy` and owns nothing, so it is the silence
            // below.
            Ty::Closure { .. } => {
                let symbol = drop_fn_name(ty);
                match self.closure_fns.get(&symbol).copied() {
                    Some((func, fty)) => (func, fty),
                    None => return Ok(()),
                }
            }
            // A `Thread<T>` handle owns the worker record, and its drop is the
            // runtime's: detach the worker and free nothing (MK9-S3-M1 stage
            // C1). It takes the address of the slot holding the handle, exactly
            // as the drop call rule passes `&h`.
            Ty::Thread(_) => {
                let symbol = drop_fn_name(ty);
                let rt = self
                    .runtime
                    .get(&symbol)
                    .ok_or_else(|| no_value(&format!("the runtime drop '{symbol}'")))?;
                (rt.func, rt.ty)
            }
            // A `Mutex<T>` record is the runtime's too (B-5): `kelp_mutex_drop`
            // takes the address of the slot holding the pointer, runs the
            // element's own stored drop and frees the record.
            Ty::Mutex(_) => {
                let symbol = drop_fn_name(ty);
                let rt = self
                    .runtime
                    .get(&symbol)
                    .ok_or_else(|| no_value(&format!("the runtime drop '{symbol}'")))?;
                (rt.func, rt.ty)
            }
            // An `RwLock<T>` record and a `Condvar` are the runtime's too
            // (`std/sync`): the rwlock's drop runs the element's own stored
            // drop, the condvar's destroys the pthread object.
            Ty::RwLock(_) | Ty::Condvar => {
                let symbol = drop_fn_name(ty);
                let rt = self
                    .runtime
                    .get(&symbol)
                    .ok_or_else(|| no_value(&format!("the runtime drop '{symbol}'")))?;
                (rt.func, rt.ty)
            }
            _ => return Ok(()),
        };
        self.builder.build_call(func, fty, &[ptr], "");
        Ok(())
    }

    /// Emit every function body. Borrows `&self`: after `declare`, the module
    /// state is read-only and the C API builder is the only thing that changes.
    pub(crate) fn emit_all(&self) -> Result<(), String> {
        let program = self.program;
        for f in &program.functions {
            if f.extern_ {
                continue;
            }
            // The incremental mode emits one unit's functions per pass: a unit
            // declares every function (so a cross-unit call resolves) and
            // defines its own, and `_support` defines none of them (roadmap
            // A-14 slice 1).
            if !self.defines_function(f) {
                continue;
            }
            FnGen::new(self, f)?.emit()?;
        }
        Ok(())
    }
}
