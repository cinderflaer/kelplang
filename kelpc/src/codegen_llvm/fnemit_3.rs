use super::*;

impl<'g, 'a> FnGen<'g, 'a> {
    /// `expr?`: evaluate the operand once into a temporary, branch on its failure
    /// tag, and on the failure path rebuild the failure in the enclosing
    /// function's return type, free the frame, and return.
    ///
    /// This is the earlier emitter's statement expression, as control flow: the two
    /// arms are two blocks and the operator's value is the success payload read
    /// out of the temporary. The temporary is deliberately never dropped: exactly
    /// one variant's payload is moved out of it on either path, and dropping what
    /// is left would free what was moved.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn emit_try(
        &mut self,
        sub: &Expr,
        source: &str,
        target: &str,
        ok_variant: usize,
        err_variant: usize,
        target_err_variant: usize,
        err_has_payload: bool,
        convert: &Option<(String, bool)>,
        span: Span,
        ty: &Ty,
    ) -> Result<Option<Value<'a>>, String> {
        let src = self
            .g
            .enums
            .get(source)
            .cloned()
            .ok_or_else(|| no_value(&format!("the enum '{source}'")))?;
        let dst = self
            .g
            .enums
            .get(target)
            .cloned()
            .ok_or_else(|| no_value(&format!("the enum '{target}'")))?;
        let src_ty = src.llvm_ty;
        let dst_ty = dst.llvm_ty;

        let value = self.value_of(sub)?;
        // An operand that diverged (`panic(...)?`) leaves the block terminated and
        // no tag to test; the failure path is already the only one.
        if self.terminated {
            return Ok(None);
        }
        let src_slot = self.temp_alloca(src_ty, "try.src");
        self.g.builder.build_store(value, src_slot);
        let tag_ptr = self
            .g
            .builder
            .build_struct_gep(src_ty, src_slot, ENUM_TAG_FIELD, "tag");
        let tag = self.g.builder.build_load(src.tag_ty, tag_ptr, "tag");
        let want = self.g.ctx.const_int(src.tag_ty, err_variant as u64, false);
        let failed = self.g.builder.build_icmp(IntCmp::Eq, tag, want, "iserr");

        let fail_bb = self.cur_fn.append_block("try.err");
        let ok_bb = self.cur_fn.append_block("try.ok");
        self.g.builder.build_cond_br(failed, fail_bb, ok_bb);

        // The failure arm: the failure value in the *enclosing function's* return
        // type, then the frame's drops, then the return. The payload is copied,
        // not moved, because the source temporary is never dropped either.
        self.g.builder.position_at_end(fail_bb);
        self.cur = fail_bb;
        self.terminated = false;
        let dst_slot = self.temp_alloca(dst_ty, "try.ret");
        self.g
            .builder
            .build_store(self.g.ctx.const_null(dst_ty), dst_slot);
        let dst_tag_ptr = self
            .g
            .builder
            .build_struct_gep(dst_ty, dst_slot, ENUM_TAG_FIELD, "tag");
        let dst_tag = self
            .g
            .ctx
            .const_int(dst.tag_ty, target_err_variant as u64, false);
        self.g.builder.build_store(dst_tag, dst_tag_ptr);
        // `err_has_payload` is arity, not type: an `Err` field typed `unit`
        // has one declared field and no payload struct
        // (`VariantDef::is_payload_less`, `MISSING-r7Wqb`), so the tag above
        // is the whole failure value and there is nothing here to copy.
        let err_is_unit = matches!(src.variants[err_variant].fields.first(), Some(Ty::Unit));
        if err_has_payload && !err_is_unit {
            let src_payload_ty = src.variants[err_variant].payload_ty;
            let src_union = self
                .g
                .builder
                .build_struct_gep(src_ty, src_slot, ENUM_UNION_FIELD, "kelp_v");
            let src_field = self
                .g
                .builder
                .build_struct_gep(src_payload_ty, src_union, 0, "payload");
            let payload_ty = self.lty(&src.variants[err_variant].fields[0])?;
            let payload = self.g.builder.build_load(payload_ty, src_field, "payload");
            // A-12 slice 6: a differing failure payload is converted by the
            // static `From` method `?` resolved before it is stored into the
            // target's failure variant. A `datac` parameter is a borrow, so a
            // by-ref conversion is handed the payload's address; a `Copy`
            // scalar is handed the loaded value.
            let stored = match convert {
                Some((key, by_ref)) => {
                    let info = self
                        .g
                        .funcs
                        .get(key)
                        .cloned()
                        .ok_or_else(|| no_value(&format!("the conversion '{key}'")))?;
                    let arg = if *by_ref { src_field.into() } else { payload };
                    self.g.builder.build_call(info.func, info.ty, &[arg], "from")
                }
                None => payload,
            };
            let dst_payload_ty = dst.variants[target_err_variant].payload_ty;
            let dst_union = self
                .g
                .builder
                .build_struct_gep(dst_ty, dst_slot, ENUM_UNION_FIELD, "kelp_v");
            let dst_field = self
                .g
                .builder
                .build_struct_gep(dst_payload_ty, dst_union, 0, "payload");
            self.g.builder.build_store(stored, dst_field);
        }
        let drops = self.drops_at_try(span);
        self.release_regions_from(0)?;
        self.emit_drops(&drops)?;
        let ret = self.g.builder.build_load(dst_ty, dst_slot, "try.ret");
        self.leave_frame()?;
        self.g.builder.build_ret(ret);
        self.terminated = true;

        // The success arm is where the operator's value lives. A `unit`
        // success type has no payload to read (`MISSING-r7Wqb`): `expr?` is
        // then no value at all, the same as any other `unit`-typed
        // expression.
        self.g.builder.position_at_end(ok_bb);
        self.cur = ok_bb;
        self.terminated = false;
        if *ty == Ty::Unit {
            return Ok(None);
        }
        let ok_payload_ty = src.variants[ok_variant].payload_ty;
        let ok_union = self
            .g
            .builder
            .build_struct_gep(src_ty, src_slot, ENUM_UNION_FIELD, "kelp_v");
        let ok_field = self
            .g
            .builder
            .build_struct_gep(ok_payload_ty, ok_union, 0, "payload");
        let lty = self.lty(ty)?;
        Ok(Some(self.g.builder.build_load(lty, ok_field, "try.ok")))
    }

    /// The `tir` type and the address of one `static`'s current-CPU copy
    /// (kernel stage 7; per-CPU, kernel stage 8). For an ordinary static the
    /// address is the global itself; for a `percpu` one it is
    /// `&global[cpu_index()]`, computed through the same stride type the global
    /// was declared with.
    pub(crate) fn static_slot(&self, name: &str) -> Result<(Ty, Value<'a>), String> {
        let (global, ty, percpu) = self
            .g
            .static_place(name)
            .ok_or_else(|| no_value(&format!("the static '{name}'")))?;
        let Some(percpu) = percpu else {
            return Ok((ty, global));
        };
        let idx = self.g.cpu_index(self.cur_fn)?;
        let slot = self
            .g
            .builder
            .build_gep(percpu.stride_ty, global, idx, "percpu.slot");
        let cell = if percpu.padded {
            self.g
                .builder
                .build_struct_gep(percpu.stride_ty, slot, 0, "percpu")
        } else {
            slot
        };
        Ok((ty, cell))
    }

    /// The address of `base.field`, plus the LLVM struct type it indexes and the
    /// field's index: what a field read and a field write both need.
    pub(crate) fn field_place(
        &mut self,
        base: &Expr,
        field: &str,
    ) -> Result<(Value<'a>, Type<'a>, u32), String> {
        let (struct_ty, index) = self.field_slot(&base.ty, field)?;
        let ptr = self.place_of(base)?;
        Ok((ptr, struct_ty, index))
    }

    /// The LLVM struct type and field index a `datac` field resolves to, from
    /// the base's type. Separated from [`Self::field_place`] because the staged
    /// temporary path has the base's *address* already and only needs the
    /// projection.
    pub(crate) fn field_slot(&self, base_ty: &Ty, field: &str) -> Result<(Type<'a>, u32), String> {
        let struct_name = struct_name_of_ty(base_ty)
            .ok_or_else(|| refuse_type(base_ty))?
            .to_string();
        let index = self
            .g
            .field_index
            .get(&struct_name)
            .and_then(|names| names.iter().position(|n| n == field))
            .ok_or_else(|| no_value(&format!("the field '{struct_name}.{field}'")))?;
        let struct_ty = *self
            .g
            .structs
            .get(&struct_name)
            .ok_or_else(|| no_value(&format!("the datac '{struct_name}'")))?;
        Ok((struct_ty, index as u32))
    }

    /// The address of a *place* expression: a local's slot, a field of another
    /// place, or the pointer a `datac` or heap borrow names.
    ///
    /// An rvalue base (`mk().field`, a literal's field) has no address, so it is
    /// stored into a temporary slot first, exactly as the earlier emitter hoists one.
    /// The temporary is handed back to the caller, which is what frees it: this
    /// slice's `datac`s can own, so a staged temporary's drop is the caller's
    /// business, not silently dropped here.
    pub(crate) fn place_of(&mut self, e: &Expr) -> Result<Value<'a>, String> {
        match &e.kind {
            ExprKind::Ident(name) => {
                // A `pd` with no hidden parameter here is its own global, so the
                // global is the place itself, whatever the expression's type says
                // (a `tl` read in a function is typed as a borrow of its storage).
                let slot = match self.lookup(name) {
                    Some(slot) => slot,
                    None => {
                        return self
                            .pd_place(name)
                            .ok_or_else(|| no_value(&format!("the local '{name}'")));
                    }
                };
                if is_borrow_ty(&e.ty) {
                    // The binding holds the borrow: its own value is the pointer.
                    Ok(self.g.builder.build_load(self.g.ctx.opaque_ptr(), slot, name))
                } else {
                    Ok(slot)
                }
            }
            // A `static`'s place is its own module-scope global; a `percpu`
            // static's is the current CPU's copy.
            ExprKind::StaticRef(name) => self.static_slot(name).map(|(_, slot)| slot),
            ExprKind::FieldAccess { base, field } => {
                let (ptr, struct_ty, index) = self.field_place(base, field)?;
                Ok(self
                    .g
                    .builder
                    .build_struct_gep(struct_ty, ptr, index, field))
            }
            // One payload field of one variant: the same union projection a
            // read takes, handed out as an address. This is what a pattern
            // binding of an owned payload borrows through.
            ExprKind::EnumField {
                base,
                variant,
                field,
            } => self.enum_field_place(base, *variant, *field),
            // A field of the enclosing closure's environment (S2E): the
            // environment owns the capture, so the field is its address. A
            // *borrowed* capture is an exception: the field holds the pointer,
            // so the place is the pointee it names, exactly as a borrow-typed
            // local's place is (`kelp wiki page language/functions.md`).
            ExprKind::Capture { index } => {
                let slot = self.capture_place(*index)?;
                if is_borrow_ty(&e.ty) {
                    Ok(self
                        .g
                        .builder
                        .build_load(self.g.ctx.opaque_ptr(), slot, "capture.ptr"))
                } else {
                    Ok(slot)
                }
            }
            // A borrow in a place position is already an address.
            ExprKind::Ref { expr: sub, .. } => self.place_of(sub),
            ExprKind::Deref { expr: sub } => self.value_of(sub),
            _ => {
                // A borrow-typed rvalue is already an address: a call returning
                // `&T` (`List#get`/`Map#get` on an owned element) yields the
                // pointer itself, which is exactly what a field read or a chain
                // step wants. Staging it into a slot would hand back the
                // address of the *pointer* instead of the pointee.
                if matches!(e.ty, Ty::Ref { .. }) {
                    return self.value_of(e);
                }
                // A staged temporary that owns memory has to be freed, and the
                // sites that own that duty are the borrow staging
                // (`borrow_place`) and the value-position field read
                // (`emit_field_read`); both intercept a chain rooted at an owned
                // rvalue before it reaches here. Reaching one anyway means a
                // shape no site claimed, so it is refused by name rather than
                // silently leaked: an owned temporary nobody frees is a leak,
                // and a hidden one is worse than a build failure.
                if self.g.owns_memory(&e.ty) {
                    return Err(refuse("'an owned temporary' at a place this lowering does not stage", "S3"));
                }
                let value = self.value_of(e)?;
                let lty = self.lty(&e.ty)?;
                let slot = self.temp_alloca(lty, "field.base");
                self.g.builder.build_store(value, slot);
                Ok(slot)
            }
        }
    }

    /// An alloca for a temporary, created at the start of the entry block like
    /// a named local, so `mem2reg` can promote it and a loop body does not grow
    /// the stack once per iteration.
    pub(crate) fn temp_alloca(&self, ty: Type<'a>, name: &str) -> Value<'a> {
        self.g.builder.position_at_start(self.entry);
        let slot = self.g.builder.build_alloca(ty, name);
        self.g.builder.position_at_end(self.cur);
        slot
    }

    /// The LLVM type of a `tir` type, with this program's `datac` table.
    pub(crate) fn lty(&self, ty: &Ty) -> Result<Type<'a>, String> {
        llvm_type(self.g.ctx, &self.g.structs, &self.g.containers, ty)
    }

    /// Evaluate an expression that must have a value, or report the internal
    /// error that it did not (a unit-typed expression in a value position).
    pub(crate) fn value_of(&mut self, e: &Expr) -> Result<Value<'a>, String> {
        self.emit_expr(e)?
            .ok_or_else(|| no_value("an expression that should produce a value"))
    }

    /// One call argument. An explicit borrow of an owned *temporary* is staged
    /// into a slot of its own and handed back in `held` for the caller to free
    /// after the call, which is what the argument staging rule does; every
    /// other argument is its own value.
    pub(crate) fn arg_value(
        &mut self,
        a: &Expr,
        held: &mut Vec<(Value<'a>, Ty)>,
    ) -> Result<Value<'a>, String> {
        if matches!(a.kind, ExprKind::Ref { .. }) && !is_str_ref(&a.ty) {
            return self.borrow_place(a, held);
        }
        self.value_of(a)
    }

    /// The address a borrow expression denotes, staging an owned temporary when
    /// the borrow hangs off one.
    ///
    /// `mkName().name` borrows a field of a `datac` nothing else owns: the
    /// temporary is copied into a slot of its own, the field's address is taken
    /// inside that copy, and the slot is handed back in `held` for the caller to
    /// free after its operation (the call or the `printf`). This is
    /// the argument staging rule's borrow arm and
    /// `stage_borrowed_field`, which stage the *whole* temporary and not the
    /// field, so a `datac` that owns two buffers frees both.
    ///
    /// A borrow that is not rooted at an owned rvalue is an ordinary place, so
    /// the question is only whether the chain's root is one.
    pub(crate) fn borrow_place(
        &mut self,
        arg: &Expr,
        held: &mut Vec<(Value<'a>, Ty)>,
    ) -> Result<Value<'a>, String> {
        if let ExprKind::Ref { expr: sub, .. } = &arg.kind {
            if let Some(root) = temporary_root(sub) {
                if self.g.owns_memory(&root.ty) {
                    let root_ty = root.ty.clone();
                    let lty = self.lty(&root_ty)?;
                    // In a spawning program the staged temporary is owned by the
                    // frame until the operation frees it, so a panic in the
                    // operation still drops it (decision 125).
                    let slot = if self.frame_owns(&root_ty) {
                        let slot = self.alloca(&root_ty, "borrow.tmp")?;
                        self.register_frame_slot(&root_ty, slot)?;
                        slot
                    } else {
                        self.temp_alloca(lty, "borrow.tmp")
                    };
                    let value = self.value_of(root)?;
                    self.g.builder.build_store(value, slot);
                    let ptr = self.chain_addr(slot, sub)?;
                    held.push((slot, root_ty));
                    return Ok(ptr);
                }
            }
        }
        self.place_of(arg)
    }

    /// One argument of a runtime or extern call. An owned aggregate over 16
    /// bytes is spelled as a pointer, which is the only way such an argument
    /// crosses to C (BUG-vN4tR, MK7 §3.3): the value is staged into a slot of its
    /// own and the slot's address is what crosses. The value is *moved* into the
    /// call, so unlike `arg_value`'s borrow staging the slot is not freed
    /// afterwards. Everything else follows `arg_value`.
    pub(crate) fn rt_arg_value(
        &mut self,
        a: &Expr,
        held: &mut Vec<(Value<'a>, Ty)>,
    ) -> Result<Value<'a>, String> {
        if is_aggregate_ty(&a.ty) {
            let lty = self.lty(&a.ty)?;
            if crosses_c_boundary_by_value(&a.ty, lty, self.g.layout) {
                let slot = self.temp_alloca(lty, "arg.byref");
                let value = self.value_of(a)?;
                self.g.builder.build_store(value, slot);
                return Ok(slot);
            }
        }
        self.arg_value(a, held)
    }

    /// The address of the field chain `e` read out of `root`, the staged
    /// temporary its base is rooted at (the earlier emitter's `chain_off`).
    pub(crate) fn chain_addr(&mut self, root: Value<'a>, e: &Expr) -> Result<Value<'a>, String> {
        match &e.kind {
            ExprKind::FieldAccess { base, field } => {
                let base_ptr = self.chain_addr(root, base)?;
                let struct_name = struct_name_of_ty(&base.ty)
                    .ok_or_else(|| refuse_type(&base.ty))?
                    .to_string();
                let struct_ty = *self
                    .g
                    .structs
                    .get(&struct_name)
                    .ok_or_else(|| no_value(&format!("the datac '{struct_name}'")))?;
                let index = self
                    .g
                    .field_index
                    .get(&struct_name)
                    .and_then(|names| names.iter().position(|n| n == field))
                    .ok_or_else(|| no_value(&format!("the field '{struct_name}.{field}'")))?;
                Ok(self
                    .g
                    .builder
                    .build_struct_gep(struct_ty, base_ptr, index as u32, field))
            }
            // A chain that steps through an enum payload: the union field, then
            // the variant's payload struct, then the field.
            ExprKind::EnumField {
                base,
                variant,
                field,
            } => {
                let base_ptr = self.chain_addr(root, base)?;
                let info = self.enum_info_of(&base.ty)?;
                let union_ptr = self.g.builder.build_struct_gep(
                    info.llvm_ty,
                    base_ptr,
                    ENUM_UNION_FIELD,
                    "kelp_v",
                );
                let v = &info.variants[*variant];
                let payload_ty = v.payload_ty;
                let index = v.payload_index(*field);
                Ok(self
                    .g
                    .builder
                    .build_struct_gep(payload_ty, union_ptr, index, "field"))
            }
            _ => Ok(root),
        }
    }

    pub(crate) fn int_const(&self, ty: &Ty, value: i128) -> Result<Value<'a>, String> {
        let lty = self.lty(ty)?;
        // Cut the `i128` literal to the expression's own type. `LLVMConstInt`
        // takes the low 64 bits and truncates to the type's width, which is the
        // same narrowing the earlier emitter spells as a C cast; the sign flag is
        // irrelevant for a value already in two's complement.
        Ok(self.g.ctx.const_int(lty, value as u64, false))
    }

    pub(crate) fn float_const(&self, ty: &Ty, value: f64) -> Result<Value<'a>, String> {
        if !ty.is_float() {
            return Err(refuse(&format!("a float literal of type '{}'", ty.display()), "S1"));
        }
        Ok(self.g.ctx.const_real(self.lty(ty)?, value))
    }

    pub(crate) fn string_global(&self, text: &str) -> Result<Value<'a>, String> {
        // A global string is NUL-terminated, so an interior NUL would truncate
        // silently. S1 refuses it by name rather than miscompiling it, and the
        // frontend refuses a raw NUL in a string literal before this point.
        if text.as_bytes().contains(&0) {
            return Err(refuse("'a string literal containing NUL'", "S2"));
        }
        Ok(self.g.builder.global_string(text, "str"))
    }

    /// Load a binding. A load at a move site also clears the source slot: the
    /// value is copied out and the binding is zeroed, so a drop the binding
    /// still owes on a path that did not move it frees nothing rather than a
    /// second copy (MK7 §3.4; the earlier emitter wrote `({0})` for the same
    /// reason). A binding moved on every path gets no `at_move` entry, so this
    /// leaves its slot alone and nothing drops it.
    pub(crate) fn load_ident(&mut self, e: &Expr, name: &str) -> Result<Value<'a>, String> {
        let lty = self.lty(&e.ty)?;
        let slot = match self.lookup(name) {
            Some(slot) => slot,
            // A `pd` global named by a function with no hidden parameter of its
            // own (`main`, or another `pd`'s initializer). Its storage outlives
            // every frame, so it is never a move source and is never cleared.
            None => match self.pd_place(name) {
                // A borrow-typed read of a `pd` with no hidden parameter (a `tl`
                // read in a function) is the global's address; any other is its value.
                Some(global) if is_borrow_ty(&e.ty) => return Ok(global),
                Some(global) => return Ok(self.g.builder.build_load(lty, global, name)),
                None => return Err(no_value(&format!("the local '{name}'"))),
            },
        };
        let value = self.g.builder.build_load(lty, slot, name);
        // A spawning program clears the slot at every owned move, not only at
        // those some path leaves live: its frame drops whatever a panic finds
        // still holding a value (decision 125).
        let moved = self.g.drops.moves_at(&self.fn_name, e)
            || (self.g.spawn_frames && self.g.drops.moves_any(&self.fn_name, e));
        if self.g.owns_memory(&e.ty) && moved {
            self.clear_owned(slot, &e.ty)?;
        }
        Ok(value)
    }

    pub(crate) fn lookup(&self, name: &str) -> Option<Value<'a>> {
        for scope in self.scopes.iter().rev() {
            if let Some(v) = scope.get(name) {
                return Some(*v);
            }
        }
        None
    }

    /// The module-scope global a `pd` names, when the name is not a local: the
    /// hidden parameter a referencing function receives is bound in a scope
    /// like any other parameter, so a miss here is `main` or a `pd` initializer
    /// (`infer.rs::PdInfo::arg` hands those the global's own address).
    pub(crate) fn pd_place(&self, name: &str) -> Option<Value<'a>> {
        self.g.pd_globals.get(name).map(|(global, _)| *global)
    }

    /// Create a local's slot in the entry block and restore the insertion point.
    ///
    /// `mem2reg` promotes only slots in the entry block, and by the time a `let`
    /// in a later statement allocates one the entry block may already have been
    /// terminated by a `return`, so the slot can never go at the block's
    /// absolute *end* (that would land after the terminator). It also must not
    /// always go at the block's absolute *start* either: that would insert
    /// each new local *before* every one already hoisted there, reversing
    /// their declaration order in the IR every function actually has more
    /// than one local in (confirmed for `while i < N { ... }` with two `var`s,
    /// MISSING-n4Zwq: `hash`, declared first, ended up allocated *after* `i`).
    /// LLVM's own optimizer is not blind to that order: `mem2reg`'s promotion
    /// walk, and the SCEV-based passes downstream of it (`indvars`, the
    /// unroller) that decide which of a loop's phis is its primary induction
    /// variable, are sensitive to the order values are produced in, so a
    /// reversal here silently costs real codegen quality later, well past
    /// where it looks like a purely cosmetic ordering choice. Confirmed
    /// experimentally: swapping only the two `alloca`s' relative order in
    /// `loop.kp`'s hand-edited pre-optimization `.ll`, with nothing else
    /// touched, flips which phi `opt -O2` puts first at the loop header and
    /// closes the extra-`imul` gap `MISSING-n4Zwq` measured.
    ///
    /// So each `alloca` chains in right after the *previous* one this
    /// function hoisted (`alloca_after`), preserving declaration order among
    /// them, and only the very first one falls back to the block's start.
    /// Chaining after a real instruction (rather than "the end") is what
    /// keeps this valid once the block gains a terminator: the previous
    /// `alloca` was always inserted before any terminator existed, and
    /// inserting right after it is still before that terminator, however
    /// much later it is added. This is the same technique Clang's own `-O0`
    /// codegen uses for the same reason (a tracked `AllocaInsertPt` new
    /// entry-block allocas chain after), not a trick invented for this one
    /// benchmark.
    pub(crate) fn alloca(&mut self, ty: &Ty, name: &str) -> Result<Value<'a>, String> {
        let lty = self.lty(ty)?;
        Ok(self.entry_alloca_raw(lty, name))
    }

    /// An `alloca` of an LLVM type, hoisted into the entry block after the
    /// previous one (`alloca`'s own rule).
    pub(crate) fn entry_alloca_raw(&mut self, lty: Type<'a>, name: &str) -> Value<'a> {
        match self.alloca_after {
            Some(prev) => self.g.builder.position_after(prev, self.entry),
            None => self.g.builder.position_at_start(self.entry),
        }
        let slot = self.g.builder.build_alloca(lty, name);
        self.alloca_after = Some(slot);
        self.g.builder.position_at_end(self.cur);
        slot
    }

    /// Registers an owned binding's slot with the cleanup frame (decision 125).
    /// The slot is cleared in the entry block straight after its alloca, so a
    /// slot whose declaration has not run yet holds nothing for the handler to
    /// drop. Does nothing when the function has no frame.
    pub(crate) fn register_owned(&mut self, ty: &Ty, slot: Value<'a>) -> Result<(), String> {
        if self.frame.is_none() || !self.g.owns_memory(ty) {
            return Ok(());
        }
        let lty = self.lty(ty)?;
        let null = self.g.ctx.const_null(lty);
        self.g.builder.position_after(slot, self.entry);
        self.g.builder.build_store(null, slot);
        self.g.builder.position_at_end(self.cur);
        self.register_flagged(ty, slot);
        Ok(())
    }

    /// Records a registered slot with a fresh liveness flag, cleared in the entry
    /// block and set at the current point: the value is about to be stored there.
    /// A zero value can be a live one (a `datac` whose fields are all zero), so
    /// the null of the slot alone cannot tell the handler what is still owned.
    pub(crate) fn register_flagged(&mut self, ty: &Ty, slot: Value<'a>) {
        let flag = self.entry_alloca_raw(self.g.ctx.i8(), "live.flag");
        let off = self.g.ctx.const_int(self.g.ctx.i8(), 0, false);
        self.g.builder.position_after(flag, self.entry);
        self.g.builder.build_store(off, flag);
        self.g.builder.position_at_end(self.cur);
        let on = self.g.ctx.const_int(self.g.ctx.i8(), 1, false);
        self.g.builder.build_store(on, flag);
        self.registered.push((slot, ty.clone(), flag));
    }

    /// The liveness flag of a slot the frame registered, if it has one.
    pub(crate) fn flag_of(&self, slot: Value<'a>) -> Option<Value<'a>> {
        self.registered.iter().find(|(s, _, _)| s.as_raw() == slot.as_raw()).map(|(_, _, f)| *f)
    }

    /// Clears an owned slot after its value has been moved out or dropped: the
    /// slot's null, and its flag off, so the frame does not drop it again.
    pub(crate) fn clear_owned(&mut self, slot: Value<'a>, ty: &Ty) -> Result<(), String> {
        let lty = self.lty(ty)?;
        self.g.builder.build_store(self.g.ctx.const_null(lty), slot);
        if let Some(flag) = self.flag_of(slot) {
            let off = self.g.ctx.const_int(self.g.ctx.i8(), 0, false);
            self.g.builder.build_store(off, flag);
        }
        Ok(())
    }

    /// Whether a temporary of `ty` is owned by this function's frame while it is
    /// staged: it has a drop that frees it (`call_drop`), and this function has a
    /// frame. A spawn-free program has no frame, so nothing takes this path there.
    pub(crate) fn frame_owns(&self, ty: &Ty) -> bool {
        self.frame.is_some() && self.g.owns_memory(ty) && self.g.managed_drop(ty)
    }

    /// Registers a staged temporary's slot with the cleanup frame, cleared in the
    /// entry block, the way `register_owned` registers a named binding's slot.
    pub(crate) fn register_frame_slot(&mut self, ty: &Ty, slot: Value<'a>) -> Result<(), String> {
        let lty = self.lty(ty)?;
        let null = self.g.ctx.const_null(lty);
        self.g.builder.position_after(slot, self.entry);
        self.g.builder.build_store(null, slot);
        self.g.builder.position_at_end(self.cur);
        self.register_flagged(ty, slot);
        Ok(())
    }

    /// An owned argument's slot: allocated, registered with the frame, and set to
    /// the evaluated value. The caller clears it before the call moves the value
    /// into the callee, which registers its own parameter (decision 125).
    pub(crate) fn frame_arg_slot(&mut self, ty: &Ty, value: Value<'a>) -> Result<Value<'a>, String> {
        let slot = self.alloca(ty, "arg.tmp")?;
        self.register_frame_slot(ty, slot)?;
        self.g.builder.build_store(value, slot);
        Ok(slot)
    }

    /// Frees the temporaries an operation staged (`held`) after the operation. A
    /// slot the frame owns is cleared too, so the frame does not drop it again on
    /// a later panic.
    pub(crate) fn free_held(&mut self, held: Vec<(Value<'a>, Ty)>) -> Result<(), String> {
        for (slot, ty) in held {
            self.g.call_drop(slot, &ty)?;
            if self.frame_owns(&ty) {
                self.clear_owned(slot, &ty)?;
            }
        }
        Ok(())
    }

    pub(crate) fn emit_assign(&mut self, target: &Expr, value: &Expr, span: Span) -> Result<(), String> {
        // A `unit` target has no slot to write: the value expression is still
        // emitted, for whatever it does, and there is no store to make.
        if target.ty == Ty::Unit {
            self.emit_expr(value)?;
            return Ok(());
        }
        let v = self.value_of(value)?;
        if self.terminated {
            return Ok(());
        }
        match &target.kind {
            ExprKind::Ident(name) => {
                let slot = match self.lookup(name) {
                    Some(slot) => slot,
                    None => self
                        .pd_place(name)
                        .ok_or_else(|| no_value(&format!("the local '{name}'")))?,
                };
                // The old value goes after the new one has been read (it may
                // read through the binding being overwritten) and before it is
                // stored (`DropPlan::at_assign`; `b = b` is not one of these).
                let drops = self.drops_at_assign(span);
                self.emit_drops(&drops)?;
                self.g.builder.build_store(v, slot);
                // The slot holds a new value the frame owns from now on.
                if let Some(flag) = self.flag_of(slot) {
                    let on = self.g.ctx.const_int(self.g.ctx.i8(), 1, false);
                    self.g.builder.build_store(on, flag);
                }
                Ok(())
            }
            // A field write: the field's address, then a store of the new
            // value. `borrowck.rs` records an `at_assign` entry with
            // `binding: None` for this shape too now (roadmap open item 15,
            // case 3), so the old field value is freed first, the same as an
            // `Ident` target - just at this address instead of a resolved
            // binding, since there is no declaration to key one to.
            ExprKind::FieldAccess { base, field } => {
                let (ptr, struct_ty, index) = self.field_place(base, field)?;
                let field_ptr = self
                    .g
                    .builder
                    .build_struct_gep(struct_ty, ptr, index, field);
                let drops = self.drops_at_assign(span);
                if let Some(d) = drops.first() {
                    self.g.call_drop(field_ptr, &d.ty)?;
                }
                self.g.builder.build_store(v, field_ptr);
                Ok(())
            }
            // A store through a borrow: `*p = v` (or `(*p).field = v` through
            // the `FieldAccess` arm above, whose `place_of` steps through the
            // pointer). The pointer is the borrow's own value, so the store
            // writes into whatever the borrow names. The value `*p` pointed
            // at is freed first, the same way and for the same reason the
            // `FieldAccess` arm above now does (roadmap open item 15, case
            // 2) - without it, the value a `*p = value` overwrote had no
            // binding anywhere in this frame to be freed by, ever.
            ExprKind::Deref { expr: sub } => {
                let ptr = self.value_of(sub)?;
                let drops = self.drops_at_assign(span);
                if let Some(d) = drops.first() {
                    self.g.call_drop(ptr, &d.ty)?;
                }
                self.g.builder.build_store(v, ptr);
                Ok(())
            }
            // A store into a `static` (kernel stage 7): the global is the
            // place, and a static never owns memory, so no old value is
            // dropped.
            ExprKind::StaticRef(name) => {
                let slot = self.static_slot(name)?.1;
                self.g.builder.build_store(v, slot);
                Ok(())
            }
            // The only other place a Kelp assignment can name is a chain this
            // slice cannot address, which the frontend does not produce.
            _ => Err(refuse("'assignment through a borrow'", "S2F")),
        }
    }

    pub(crate) fn emit_unary(&mut self, op: UnOp, sub: &Expr, ty: &Ty) -> Result<Value<'a>, String> {
        let v = self.value_of(sub)?;
        Ok(match op {
            UnOp::Neg => {
                if ty.is_float() {
                    self.g.builder.build_fneg(v, "neg")
                } else {
                    let zero = self.zero_const(ty)?;
                    self.g
                        .builder
                        .build_int_binop(IntBinOp::Sub, zero, v, "neg")
                }
            }
            UnOp::Not => {
                if *ty != Ty::Bool {
                    return Err(refuse("'logical not on a non-bool'", "S1"));
                }
                // `i1` xor `true` is logical negation of a boolean.
                let one = self.g.ctx.const_bool(true);
                self.g.builder.build_int_binop(IntBinOp::Xor, v, one, "not")
            }
            UnOp::BitNot => {
                let all = self.all_ones(ty)?;
                self.g
                    .builder
                    .build_int_binop(IntBinOp::Xor, v, all, "bitnot")
            }
        })
    }

    pub(crate) fn emit_binary(&mut self, op: BinOp, lhs: &Expr, rhs: &Expr) -> Result<Value<'a>, String> {
        // `&&`/`||` short-circuit, so they are control flow, not a bitwise op.
        match op {
            BinOp::And => return self.emit_logical(lhs, rhs, true),
            BinOp::Or => return self.emit_logical(lhs, rhs, false),
            _ => {}
        }
        let l = self.value_of(lhs)?;
        let r = self.value_of(rhs)?;
        let ot = &lhs.ty;
        // A shift by the width or more (or a negative amount) is a panic, as
        // an out-of-bounds index is; an amount known to be in range needs no
        // check. An LLVM shift by >= width would be poison.
        if let (BinOp::Shl | BinOp::Shr, Some(bits)) = (op, shift_width(ot)) {
            if !shift_amount_in_range(rhs, bits) {
                let lty = self.lty(ot)?;
                let width = self.g.ctx.const_int(lty, bits as u64, false);
                let bad = self.g.builder.build_icmp(IntCmp::Uge, r, width, "shift.bad");
                self.guard_or_panic(bad, "shift amount out of range")?;
            }
        }
        let float = ot.is_float();
        let signed = is_signed(ot);
        let b = &self.g.builder;
        Ok(match op {
            BinOp::Add => {
                if float {
                    b.build_float_binop(FloatBinOp::Add, l, r, "fadd")
                } else {
                    b.build_int_binop(IntBinOp::Add, l, r, "add")
                }
            }
            BinOp::Sub => {
                if float {
                    b.build_float_binop(FloatBinOp::Sub, l, r, "fsub")
                } else {
                    b.build_int_binop(IntBinOp::Sub, l, r, "sub")
                }
            }
            BinOp::Mul => {
                if float {
                    b.build_float_binop(FloatBinOp::Mul, l, r, "fmul")
                } else {
                    b.build_int_binop(IntBinOp::Mul, l, r, "mul")
                }
            }
            BinOp::Div => {
                if float {
                    b.build_float_binop(FloatBinOp::Div, l, r, "fdiv")
                } else if signed {
                    b.build_int_binop(IntBinOp::SDiv, l, r, "sdiv")
                } else {
                    b.build_int_binop(IntBinOp::UDiv, l, r, "udiv")
                }
            }
            BinOp::Mod => {
                if float {
                    return Err(refuse("'the float remainder operator'", "S1"));
                } else if signed {
                    b.build_int_binop(IntBinOp::SRem, l, r, "srem")
                } else {
                    b.build_int_binop(IntBinOp::URem, l, r, "urem")
                }
            }
            BinOp::Eq | BinOp::NotEq => {
                let eq = op == BinOp::Eq;
                if float {
                    b.build_fcmp(if eq { RealCmp::Oeq } else { RealCmp::One }, l, r, "fcmp")
                } else {
                    b.build_icmp(if eq { IntCmp::Eq } else { IntCmp::Ne }, l, r, "icmp")
                }
            }
            BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq => {
                if float {
                    let cmp = match op {
                        BinOp::Lt => RealCmp::Olt,
                        BinOp::LtEq => RealCmp::Ole,
                        BinOp::Gt => RealCmp::Ogt,
                        _ => RealCmp::Oge,
                    };
                    b.build_fcmp(cmp, l, r, "fcmp")
                } else {
                    let cmp = match (op, signed) {
                        (BinOp::Lt, true) => IntCmp::Slt,
                        (BinOp::LtEq, true) => IntCmp::Sle,
                        (BinOp::Gt, true) => IntCmp::Sgt,
                        (BinOp::GtEq, true) => IntCmp::Sge,
                        (BinOp::Lt, false) => IntCmp::Ult,
                        (BinOp::LtEq, false) => IntCmp::Ule,
                        (BinOp::Gt, false) => IntCmp::Ugt,
                        _ => IntCmp::Uge,
                    };
                    b.build_icmp(cmp, l, r, "icmp")
                }
            }
            BinOp::BitAnd => b.build_int_binop(IntBinOp::And, l, r, "and"),
            BinOp::BitOr => b.build_int_binop(IntBinOp::Or, l, r, "or"),
            BinOp::BitXor => b.build_int_binop(IntBinOp::Xor, l, r, "xor"),
            BinOp::Shl => b.build_int_binop(IntBinOp::Shl, l, r, "shl"),
            BinOp::Shr => {
                if signed {
                    b.build_int_binop(IntBinOp::AShr, l, r, "ashr")
                } else {
                    b.build_int_binop(IntBinOp::LShr, l, r, "lshr")
                }
            }
            BinOp::And | BinOp::Or => unreachable!("handled above as control flow"),
        })
    }
}
