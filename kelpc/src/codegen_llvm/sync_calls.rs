use super::*;

impl<'g, 'a> FnGen<'g, 'a> {
    /// `RwLock<T>` operations (`std/sync`). The C twin is
    /// the earlier emitter's `rwlock_call`, runtime call for runtime call.
    pub(crate) fn emit_rwlock_call(
        &mut self,
        op: &str,
        args: &[Expr],
        ret: &Ty,
    ) -> Result<Option<Value<'a>>, String> {
        let ptr = self.g.pty();
        match op {
            "new" => {
                let Ty::RwLock(elem) = ret else {
                    unreachable!("RwLock.new hands back an RwLock<T>")
                };
                let value = self.value_of(&args[0])?;
                let elem_ty = self.lty(elem)?;
                let slot = self.temp_alloca(elem_ty, "rwlock.value");
                self.g.builder.build_store(value, slot);
                let size = self.g.layout.abi_size_of(elem_ty);
                let drop = self
                    .drop_fn_value(elem)?
                    .unwrap_or_else(|| self.g.ctx.const_null(ptr));
                let handle = if self.g.managed {
                    let descr = self.g.record_descr(elem)?;
                    let rt = self.g.rt("kelp_rwlock_new_rooted")?;
                    self.g.call(rt.func, rt.ty, &[slot, self.g.v_i64(size), drop, descr], "rwlock")
                } else {
                    let rt = self.g.rt("kelp_rwlock_new")?;
                    self.g.call(rt.func, rt.ty, &[slot, self.g.v_i64(size), drop], "rwlock")
                };
                self.cur = self.g.check_alloc(handle, self.cur_fn)?;
                Ok(Some(handle))
            }
            "read" | "write" | "try_read" | "try_write" => {
                let block = &args[1];
                let (_, block_ret) = crate::types::closure_parts(&block.ty)
                    .ok_or_else(|| no_value("an rwlock block signature"))?;
                let cell = self.value_of(&args[0])?;
                let rw = self.g.builder.build_load(ptr, cell, "rwlock");
                let closure = self.value_of(block)?;
                let code = self.g.builder.build_extract_value(closure, 0, "fn");
                let env = self.g.builder.build_extract_value(closure, 1, "env");
                let body = self.g.closure_symbol(&thread_body_symbol(
                    &crate::types::closure_shape(&block.ty),
                ))?;
                let yields = *block_ret != Ty::Unit;
                let out_ty = if yields { self.lty(block_ret)? } else { self.g.ctx.i8() };
                let out = self.temp_alloca(out_ty, "rwlock.out");
                let out_ptr = if yields { out } else { self.g.ctx.const_null(ptr) };
                let symbol = match op {
                    "read" => "kelp_rwlock_read",
                    "write" => "kelp_rwlock_write",
                    "try_read" => "kelp_rwlock_try_read",
                    "try_write" => "kelp_rwlock_try_write",
                    _ => unreachable!(),
                };
                let rt = self.g.rt(symbol)?;
                if !op.starts_with("try_") {
                    self.g.builder.build_call(
                        rt.func,
                        rt.ty,
                        &[rw, body.as_value(), code, env, out_ptr],
                        "",
                    );
                    if yields {
                        return Ok(Some(self.g.builder.build_load(out_ty, out, "rwlock.value")));
                    }
                    return Ok(None);
                }
                let ran = self.g.call(
                    rt.func,
                    rt.ty,
                    &[rw, body.as_value(), code, env, out_ptr],
                    "rwlock.try",
                );
                let zero = self.g.ctx.const_int(self.g.ctx.i32(), 0, false);
                let ok = self.g.builder.build_icmp(IntCmp::Ne, ran, zero, "rwlock.ran");
                let payload = if yields {
                    Some(self.g.builder.build_load(out_ty, out, "rwlock.value"))
                } else {
                    None
                };
                Ok(Some(self.emit_option_from_run(ret, ok, payload)?))
            }
            "into_inner" => {
                let rw = self.value_of(&args[0])?;
                let rt = self.g.rt("kelp_rwlock_into")?;
                if *ret == Ty::Unit {
                    self.g.builder.build_call(rt.func, rt.ty, &[rw, self.g.ctx.const_null(ptr)], "");
                    return Ok(None);
                }
                let out_ty = self.lty(ret)?;
                let out = self.temp_alloca(out_ty, "rwlock.inner");
                self.g.builder.build_call(rt.func, rt.ty, &[rw, out], "");
                Ok(Some(self.g.builder.build_load(out_ty, out, "inner")))
            }
            other => Err(no_value(&format!("the rwlock operation '{other}'"))),
        }
    }

    /// `Condvar` operations (`std/sync`): the constructor and the two
    /// notifies, one runtime call each. The C twin is
    /// the earlier emitter's `condvar_call`.
    pub(crate) fn emit_condvar_call(
        &mut self,
        op: &str,
        args: &[Expr],
        _ret: &Ty,
    ) -> Result<Option<Value<'a>>, String> {
        let ptr = self.g.pty();
        match op {
            "new" => {
                let rt = self.g.rt("kelp_condvar_new")?;
                let cv = self.g.call(rt.func, rt.ty, &[], "condvar");
                self.cur = self.g.check_alloc(cv, self.cur_fn)?;
                Ok(Some(cv))
            }
            "share" => {
                let cell = self.value_of(&args[0])?;
                let cv = self.g.builder.build_load(ptr, cell, "condvar");
                let rt = self.g.rt("kelp_condvar_share")?;
                Ok(Some(self.g.call(rt.func, rt.ty, &[cv], "condvar.share")))
            }
            "notify_one" | "notify_all" => {
                let cell = self.value_of(&args[0])?;
                let cv = self.g.builder.build_load(ptr, cell, "condvar");
                let symbol = if op == "notify_one" {
                    "kelp_condvar_notify_one"
                } else {
                    "kelp_condvar_notify_all"
                };
                let rt = self.g.rt(symbol)?;
                self.g.builder.build_call(rt.func, rt.ty, &[cv], "");
                Ok(None)
            }
            other => Err(no_value(&format!("the condvar operation '{other}'"))),
        }
    }

    /// `Mutex#wait_while` / `wait_while_timeout` (`std/sync`). The C twin is
    /// the earlier emitter's `mutex_wait_value`: the runtime calls the predicate body
    /// while the lock is held, releases it around each sleep, then calls the
    /// block body; the timed form returns `Option`.
    pub(crate) fn emit_mutex_wait(
        &mut self,
        op: &str,
        args: &[Expr],
        ret: &Ty,
    ) -> Result<Option<Value<'a>>, String> {
        let timed = op == "wait_while_timeout";
        let ptr = self.g.pty();
        let block = args
            .last()
            .ok_or_else(|| no_value("the block of a wait"))?;
        let (_, block_ret) = crate::types::closure_parts(&block.ty)
            .ok_or_else(|| no_value("a wait block signature"))?;
        let pred = &args[2];
        let cell = self.value_of(&args[0])?;
        let m = self.g.builder.build_load(ptr, cell, "mutex");
        let cv_cell = self.value_of(&args[1])?;
        let cv = self.g.builder.build_load(ptr, cv_cell, "condvar");
        let pred_v = self.value_of(pred)?;
        let pcode = self.g.builder.build_extract_value(pred_v, 0, "fn");
        let penv = self.g.builder.build_extract_value(pred_v, 1, "env");
        let block_v = self.value_of(block)?;
        let code = self.g.builder.build_extract_value(block_v, 0, "fn");
        let env = self.g.builder.build_extract_value(block_v, 1, "env");
        let pred_body = self.g.closure_symbol(&thread_body_symbol(
            &crate::types::closure_shape(&pred.ty),
        ))?;
        let body = self.g.closure_symbol(&thread_body_symbol(
            &crate::types::closure_shape(&block.ty),
        ))?;
        let yields = *block_ret != Ty::Unit;
        let out_ty = if yields { self.lty(block_ret)? } else { self.g.ctx.i8() };
        let out = self.temp_alloca(out_ty, "wait.out");
        let out_ptr = if yields { out } else { self.g.ctx.const_null(ptr) };
        let millis = if timed {
            self.value_of(&args[3])?
        } else {
            self.g.ctx.const_int(self.g.ctx.i64(), 0, false)
        };
        // A `Copy` element's predicate parameter is the value itself and an
        // owned element's is a shared borrow, so the runtime is told which
        // address to hand the generated body (`implicit_borrow_wrap`).
        let pred_by_ref = crate::types::closure_parts(&pred.ty)
            .map(|(params, _)| matches!(params.first(), Some(Ty::Ref { .. })))
            .unwrap_or(false);
        let pbr = self
            .g
            .ctx
            .const_int(self.g.ctx.i32(), if pred_by_ref { 1 } else { 0 }, false);
        let symbol = if timed {
            "kelp_mutex_wait_while_timeout"
        } else {
            "kelp_mutex_wait_while"
        };
        let rt = self.g.rt(symbol)?;
        let mut call_args: Vec<Value<'a>> = vec![
            m,
            cv,
            pred_body.as_value(),
            pcode,
            penv,
            pbr,
        ];
        if timed {
            call_args.push(millis);
        }
        call_args.push(body.as_value());
        call_args.push(code);
        call_args.push(env);
        call_args.push(out_ptr);
        if !timed {
            self.g.builder.build_call(rt.func, rt.ty, &call_args, "");
            if yields {
                return Ok(Some(self.g.builder.build_load(out_ty, out, "wait.value")));
            }
            return Ok(None);
        }
        let ran = self.g.call(rt.func, rt.ty, &call_args, "wait");
        let zero = self.g.ctx.const_int(self.g.ctx.i32(), 0, false);
        let ok = self.g.builder.build_icmp(IntCmp::Ne, ran, zero, "wait.ran");
        let payload = if yields {
            Some(self.g.builder.build_load(out_ty, out, "wait.value"))
        } else {
            None
        };
        Ok(Some(self.emit_option_from_run(ret, ok, payload)?))
    }

    /// `thread.join(handle)` (MK9-S3-M1 stage C1): wait for the worker, then
    /// build `Result<T, ThreadErr>` from the result slot or the message the
    /// worker panicked with. The result slot is read only on the success path,
    /// because a worker that panicked never wrote it; the handle is freed once
    /// the value is out of it. The C twin is the earlier emitter's `thread_join_value`.
    pub(crate) fn emit_thread_join(&mut self, args: &[Expr], ret: &Ty) -> Result<Option<Value<'a>>, String> {
        let handle_arg = args
            .first()
            .ok_or_else(|| no_value("the handle of thread.join"))?;
        let Ty::Thread(elem) = &handle_arg.ty else {
            unreachable!("thread.join's argument is a Thread<T> handle")
        };
        let Ty::Struct(result_name) = ret else {
            unreachable!("thread.join hands back a Result")
        };
        let info = self
            .g
            .enums
            .get(result_name)
            .cloned()
            .ok_or_else(|| no_value(&format!("the enum '{result_name}'")))?;
        // Recognized by variant name, never by the enum's own name: the same
        // rule `compare_exchange`'s arm and `?` use, so a `std/result` replaced
        // by any two-variant `Ok`/`Err` shape still lowers. The names live in
        // the declaration; the layout above is keyed by index.
        let decl = self
            .g
            .program
            .enums
            .iter()
            .find(|e| e.name == *result_name)
            .ok_or_else(|| no_value(&format!("the enum '{result_name}'")))?;
        let variant_of = |variant: &str| {
            decl.variants
                .iter()
                .position(|v| v.name == variant)
                .expect("the Result shape has one arm per variant")
        };
        let ok = variant_of("Ok");
        let err = variant_of("Err");

        let handle = self.value_of(handle_arg)?;
        let wait = self.g.rt("kelp_thread_wait")?;
        let panicked = self.g.call(wait.func, wait.ty, &[handle], "panicked");
        let zero = self.g.ctx.const_int(self.g.ctx.i32(), 0, false);
        let failed = self
            .g
            .builder
            .build_icmp(IntCmp::Ne, panicked, zero, "failed");
        let result_ty = self.lty(ret)?;
        let slot = self.temp_alloca(result_ty, "join.result");
        self.g
            .builder
            .build_store(self.g.ctx.const_null(result_ty), slot);
        let err_bb = self.cur_fn.append_block("join.err");
        let ok_bb = self.cur_fn.append_block("join.ok");
        let done_bb = self.cur_fn.append_block("join.done");
        self.g.builder.build_cond_br(failed, err_bb, ok_bb);

        // The failure arm: the message the worker recorded, into a `ThreadErr`.
        self.g.builder.position_at_end(err_bb);
        self.cur = err_bb;
        self.terminated = false;
        let msg_fn = self.g.rt("kelp_thread_message")?;
        let message = self.g.call(msg_fn.func, msg_fn.ty, &[handle], "message");
        let string_ty = *self
            .g
            .structs
            .get("String")
            .ok_or_else(|| no_value("the builtin 'String' layout"))?;
        let text_slot = self.temp_alloca(string_ty, "join.text");
        self.g
            .sret_call("kelp_String__from_str", text_slot, &[message])?;
        let text = self.g.builder.build_load(string_ty, text_slot, "text");
        let err_ty = crate::infer::thread_err_ty();
        let err_lty = self.lty(&err_ty)?;
        let mut err_value = self.g.ctx.undef(err_lty);
        err_value = self
            .g
            .builder
            .build_insert_value(err_value, text, 0, "message");
        self.store_enum_variant(slot, &info, err, &[err_value])?;
        self.g.builder.build_br(done_bb);

        // The success arm: the worker's value out of the result slot. A
        // worker that returns `unit` wrote nothing there (`emit_thread_body`),
        // and `unit` has no LLVM type to load one as, so the arm reads
        // nothing and stores the `Ok` tag alone (`MISSING-r7Wqb`).
        self.g.builder.position_at_end(ok_bb);
        self.cur = ok_bb;
        self.terminated = false;
        let res_fn = self.g.rt("kelp_thread_result")?;
        let res_ptr = self.g.call(res_fn.func, res_fn.ty, &[handle], "result");
        if **elem == Ty::Unit {
            self.store_enum_variant(slot, &info, ok, &[])?;
        } else {
            let elem_lty = self.lty(elem)?;
            let value = self.g.builder.build_load(elem_lty, res_ptr, "value");
            self.store_enum_variant(slot, &info, ok, &[value])?;
        }
        self.g.builder.build_br(done_bb);

        self.g.builder.position_at_end(done_bb);
        self.cur = done_bb;
        self.terminated = false;
        let dispose = self.g.rt("kelp_thread_dispose")?;
        self.g.call(dispose.func, dispose.ty, &[handle], "");
        Ok(Some(self.g.builder.build_load(result_ty, slot, "joined")))
    }

    /// Store one variant's payload into an `enum` slot that has already been
    /// zeroed: the tag, then each payload field into the variant's own payload
    /// struct. What `emit_enum_lit` does for a literal, for the one value a
    /// `thread.join` builds out of results rather than out of written payloads
    /// (MK9-S3-M1 stage C1).
    pub(crate) fn store_enum_variant(
        &self,
        slot: Value<'a>,
        info: &EnumInfo<'a>,
        variant: usize,
        fields: &[Value<'a>],
    ) -> Result<(), String> {
        let tag_ptr = self
            .g
            .builder
            .build_struct_gep(info.llvm_ty, slot, ENUM_TAG_FIELD, "tag");
        let tag = self.g.ctx.const_int(info.tag_ty, variant as u64, false);
        self.g.builder.build_store(tag, tag_ptr);
        if fields.is_empty() {
            return Ok(());
        }
        let payload_ty = info
            .variants
            .get(variant)
            .ok_or_else(|| no_value("the variant of a join's result"))?
            .payload_ty;
        let union_ptr = self.g.builder.build_struct_gep(
            info.llvm_ty,
            slot,
            ENUM_UNION_FIELD,
            "kelp_v",
        );
        for (index, value) in fields.iter().enumerate() {
            let field_ptr = self.g.builder.build_struct_gep(
                payload_ty,
                union_ptr,
                index as u32,
                "field",
            );
            self.g.builder.build_store(*value, field_ptr);
        }
        Ok(())
    }

    pub(crate) fn emit_call(
        &mut self,
        callee: &str,
        args: &[Expr],
        ret: &Ty,
        span: Span,
    ) -> Result<Option<Value<'a>>, String> {
        // Managed: a safepoint poll before every call (paper 5.1).
        self.gc_poll();
        if callee == "std.source.file" {
            let path = self
                .g
                .sources
                .path_line_col(&self.module, span)
                .map(|(path, _, _)| path.to_string_lossy().into_owned())
                .unwrap_or_else(|| self.module.clone());
            return Ok(Some(self.string_global(&path)?));
        }
        if callee == "std.source.line" {
            let line = self
                .g
                .sources
                .path_line_col(&self.module, span)
                .map(|(_, line, _)| line)
                .unwrap_or(0);
            return Ok(Some(self.int_const(ret, line as i128)?));
        }
        // `cpu_index()` (kernel stage 8): the register the kernel set at boot,
        // read with one inline-asm instruction. `cpu_count()` is folded to a
        // literal in `infer.rs`, so it never reaches here.
        if callee == "cpu_index" {
            return Ok(Some(self.g.cpu_index(self.cur_fn)?));
        }
        // `Box.empty()` on an owned trait object (A-12 slice 7): the null fat
        // pair, the one value a boxed trait object can hold without a concrete
        // type to point at.
        if callee == "BoxDyn#empty" {
            let lty = self.lty(&ret)?;
            return Ok(Some(self.g.ctx.const_null(lty)));
        }
        // `Handle.null()` and `w.is_null()` (gap 1): a handle is `ptr`, so the
        // null handle is `ptr null` and the test is a pointer comparison.
        // Both keys are synthetic (see `types::HANDLE_NULL_KEY`).
        if callee == crate::types::HANDLE_NULL_KEY {
            return Ok(Some(self.g.ctx.const_null(self.g.ctx.opaque_ptr())));
        }
        if callee == crate::types::HANDLE_IS_NULL_KEY {
            let arg = args
                .first()
                .ok_or_else(|| no_value("the handle of is_null"))?;
            let value = self.value_of(arg)?;
            let null = self.g.ctx.const_null(self.g.ctx.opaque_ptr());
            return Ok(Some(self.g.builder.build_icmp(
                IntCmp::Eq,
                value,
                null,
                "is_null",
            )));
        }
        // A gated call site the earlier emitter would guard with a runtime allowlist
        // check is checked here the same way: `cap_plan` decides the domain,
        // the arguments matched and the pattern count, and every resource
        // argument is evaluated once into a value the check and the call share
        // (BUG-Kf7Rn: evaluating it twice admits one value and passes another).
        let plan = self.g.cap_plan(&self.module, callee, args, span);
        let prepared = self.prepare_checked_args(args, plan.as_ref())?;

        // `print`/`print!` reach libc `printf`, as the earlier emitter does.
        if callee == "print" || callee == "print!" {
            let arg = args
                .first()
                .ok_or_else(|| no_value("the argument of print"))?;
            self.emit_template_print(callee == "print", arg)?;
            return Ok(None);
        }

        // `panic` reaches the runtime's `kelp_panic(const char*)`. The message
        // is a `str`/`&str` (already the pointer) or an owned `String` (the
        // buffer's `data`); the call never returns, so the block ends here and
        // no later statement of this block may be emitted.
        if callee == "panic" {
            let arg = args
                .first()
                .ok_or_else(|| no_value("the argument of panic"))?;
            let msg = self.heap_data_ptr(arg)?;
            self.g.emit_panic_ptr(msg);
            self.terminated = true;
            return Ok(None);
        }

        // `assert` reaches the runtime's `kelp_assert(bool)`, which panics with
        // the same "assertion failed" text the earlier emitter gets.
        if callee == "assert" {
            let arg = args
                .first()
                .ok_or_else(|| no_value("the condition of assert"))?;
            let cond = self.value_of(arg)?;
            let rt = *self
                .g
                .runtime
                .get("kelp_assert")
                .ok_or_else(|| no_value("the runtime 'kelp_assert'"))?;
            self.g.builder.build_call(rt.func, rt.ty, &[cond], "");
            return Ok(None);
        }

        // `n.to_u8()` and friends: the key is `"<src>#to_<dst>"`, the same shape
        // the earlier emitter's `call_value` matches.
        if let Some((src, dst)) = callee.split_once("#to_") {
            if let (Some(from), Some(to)) = (Ty::from_name(src), Ty::from_name(dst)) {
                if from.is_integer() && (to.is_integer() || to.is_float()) {
                    let arg = args
                        .first()
                        .ok_or_else(|| no_value("the argument of a conversion"))?;
                    let v = self.emit_conversion(callee, &from, &to, arg)?;
                    return Ok(Some(v));
                }                if from.is_float() && to.is_float() {
                    let arg = args
                        .first()
                        .ok_or_else(|| no_value("the argument of a conversion"))?;
                    let v = self.value_of(arg)?;
                    let to_ty = self.lty(&to)?;
                    let widen = matches!(from, Ty::F32);
                    return Ok(Some(self.g.builder.build_fp_cast(widen, v, to_ty, "to_f")));
                }
            }
        }

        // Byte-order conversion (kernel stage 3, MK9-S8): `x.swap_bytes()`,
        // `x.to_be()`, `x.to_le()` and the static `u32.from_be(x)`/
        // `u32.from_le(x)`. One `llvm.bswap.*` intrinsic, or nothing when the
        // direction is the target's own, exactly as the earlier emitter's `endian_call`
        // emits one `__builtin_bswap*` or the identity.
        if let Some((src, op)) = callee.split_once('#') {
            if let Some(ty) = Ty::from_name(src) {
                if let Some(v) = self.emit_endian(op, &ty, args)? {
                    return Ok(Some(v));
                }
            }
        }
        // A byte-order read or write on a `Bytes` (kernel stage 3, MK9-S8): an
        // align-1 load or store through the buffer's `data` pointer, with the
        // swap on the way in or out. The older `Bytes` methods still go through
        // the runtime table below.
        if let Some(op) = callee.strip_prefix("Bytes#") {
            if crate::types::bytes_endian_access(op).is_some() {
                return self.emit_bytes_endian(op, args);
            }
        }

        // An atomic operation (MK9-S3-M1 stage A1): one instruction, emitted
        // here rather than as a call into a monomorphized function set, because
        // the instruction *is* the operation.
        if let Some(op) = callee.strip_prefix("Atomic#") {
            return self.emit_atomic_call(op, args, ret);
        }

        // A `Ptr<T>` operation (MK9-S2 stage 1): one instruction, or one libc
        // allocation, per operation. the earlier emitter's `ptr_call` writes the same
        // operations as C.
        if let Some(op) = callee.strip_prefix("Ptr#") {
            return self.emit_ptr_call(op, args, ret);
        }

        // `thread.fence` (MK9-S3-M1 stage A2): one `fence seq_cst` instruction,
        // the same shape the atomic operations above take. the earlier emitter
        // spells the same operation as its one-line C builtin at the call site,
        // and this side emits the LLVM instruction, because a fence is a
        // memory-model operation each backend states in its own terms. The
        // ordering argument is not evaluated, for the reason `emit_atomic_call`
        // gives: v1 has one ordering and the instruction's own `seq_cst` is the
        // whole of what the argument says.
        if callee == crate::stdlib::FENCE_FN {
            self.g.builder.build_fence();
            return Ok(None);
        }

        // `thread.spawn` / `thread.join` (MK9-S3-M1 stage C1): the runtime owns
        // the thread and the panic policy, and each call site is the handle
        // build or the result build around it. the earlier emitter's `call_value`
        // dispatches the same two keys to the same shape in C.
        if callee == crate::stdlib::SPAWN_FN {
            return self.emit_thread_spawn(args, ret);
        }
        if callee == crate::stdlib::JOIN_FN {
            return self.emit_thread_join(args, ret);
        }

        // A `Mutex<T>` operation (B-5, `std/sync`): three runtime calls, the
        // same three the earlier emitter's `mutex_call` writes in C.
        if let Some(op) = callee.strip_prefix("Mutex#") {
            if op == "wait_while" || op == "wait_while_timeout" {
                return self.emit_mutex_wait(op, args, ret);
            }
            return self.emit_mutex_call(op, args, ret);
        }
        // An `RwLock<T>` or `Condvar` operation (`std/sync`).
        if let Some(op) = callee.strip_prefix("RwLock#") {
            return self.emit_rwlock_call(op, args, ret);
        }
        if let Some(op) = callee.strip_prefix("Condvar#") {
            return self.emit_condvar_call(op, args, ret);
        }

        if let Some(info) = self.g.funcs.get(callee).cloned() {
            let mut held: Vec<(Value<'a>, Ty)> = Vec::new();
            let mut values: Vec<Value<'a>> = Vec::with_capacity(args.len());
            // The owned arguments a spawning program stages in frame slots, each
            // cleared again just before the call moves it into the callee.
            let mut moved: Vec<(Value<'a>, Ty)> = Vec::new();
            for (index, a) in args.iter().enumerate() {
                // A `unit` argument crosses as nothing: the callee's own
                // signature has no parameter for it (`declare_functions`
                // filters the same way), so the argument is evaluated for
                // whatever it does and no value is passed.
                if a.ty == Ty::Unit {
                    self.emit_expr(a)?;
                    // A `unit` argument that diverged (`panic`) ends the block,
                    // so there is no call left to make.
                    if self.terminated {
                        return Ok(None);
                    }
                    continue;
                }
                let v = match prepared.get(&index) {
                    Some(v) => *v,
                    None => self.arg_value(a, &mut held)?,
                };
                // An owned argument is held by the frame from the moment it is
                // evaluated, so a panic in a later argument still drops it.
                if self.frame_owns(&a.ty) {
                    let slot = self.frame_arg_slot(&a.ty, v)?;
                    moved.push((slot, a.ty.clone()));
                }
                values.push(v);
            }
            // The allowlist checks run before the call, after every argument has
            // been evaluated: the statement expression `cap_checked_call` builds
            // has the same order.
            self.emit_cap_checks(plan.as_ref(), &prepared)?;
            // The values move into the callee now. Its frame owns each one from
            // its entry on, so the caller's slots are cleared first: a panic in
            // the callee must not drop a value twice.
            for (slot, ty) in &moved {
                self.clear_owned(*slot, ty)?;
            }
            // A call that yields `void` must not be given a name: an instruction
            // with a name has to have a value, and the verifier rejects a named
            // void one. A unit method (a `mut` receiver that writes a field and
            // returns nothing) is exactly that shape.
            let yields_value = info.ret_ty != Ty::Unit;
            let name = if yields_value { "call" } else { "" };
            let v = self.g.builder.build_call(info.func, info.ty, &values, name);
            // An owned temporary staged for the argument is freed *after* the
            // call, as the earlier emitter's `hoisted_call` frees it.
            self.free_held(held)?;
            if !yields_value {
                return Ok(None);
            }
            return Ok(Some(v));
        }

        // A builtin method on `Bytes`/`String`/`View`: reached by declaration
        // and never re-implemented here, with the machine-level signature
        // the runtime crate is compiled with. A helper that returns a struct through
        // the hidden pointer takes the destination as its explicit first
        // argument, which is the same call clang would emit.
        if let Some(rt) = self.g.runtime.get(&symbol_name(callee)).copied() {
            // A `Bytes` element access with a signed `x.to_usize()` index: the
            // same fused check as a `List` one (`emit_indexed_fused`).
            if plan.is_none() && !rt.sret {
                if let Some((from, conv, inner)) = fused_index_call(callee, args) {
                    let symbol = symbol_name(callee);
                    if let Some(len) = self.fused_len_fn(&symbol, true) {
                        return self.emit_indexed_fused(
                            len,
                            (rt.func, rt.ty),
                            true,
                            args,
                            ret,
                            &from,
                            conv,
                            inner,
                        );
                    }
                }
            }
            return self.call_runtime_entry(rt, args, &prepared, plan.as_ref());
        }

        // A builtin container method: the monomorphized function this slice
        // generated into the module, named exactly as the earlier emitter names it.
        if let Some(symbol) = container_symbol(callee, args, ret) {
            let Some((func, fty)) = self.g.container_fns.get(&symbol).copied() else {
                return Err(refuse(&format!("the call to '{callee}'"), "S2D"));
            };
            // A `List` element access whose index is a signed `x.to_usize()`:
            // one bounds branch in place of the sign branch and the bounds
            // branch (`emit_indexed_fused`). Anything else takes the call.
            if plan.is_none() {
                if let Some((from, conv, inner)) = fused_index_call(callee, args) {
                    if let Some(len) = self.fused_len_fn(&symbol, false) {
                        return self.emit_indexed_fused(
                            len, (func, fty), false, args, ret, &from, conv, inner,
                        );
                    }
                }
            }
            let mut held: Vec<(Value<'a>, Ty)> = Vec::new();
            let mut values: Vec<Value<'a>> = Vec::with_capacity(args.len());
            for (index, a) in args.iter().enumerate() {
                match prepared.get(&index) {
                    Some(v) => values.push(*v),
                    None => values.push(self.arg_value(a, &mut held)?),
                }
            }
            self.emit_cap_checks(plan.as_ref(), &prepared)?;
            let yields_value = *ret != Ty::Unit;
            let name = if yields_value { "call" } else { "" };
            let v = self.g.builder.build_call(func, fty, &values, name);
            self.free_held(held)?;
            return Ok(if yields_value { Some(v) } else { None });
        }

        // Anything else is outside this slice. An `extern fun` is the runtime
        // and is S3; any other name is a construct this stage has no lowering
        // for.
        let stage = if self
            .g
            .program
            .functions
            .iter()
            .any(|f| f.name == callee && f.extern_)
        {
            "S3"
        } else {
            "S2B"
        };
        Err(refuse(&format!("the call to '{callee}'"), stage))
    }
}
