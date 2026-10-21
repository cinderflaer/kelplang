use super::*;

impl<'g, 'a> FnGen<'g, 'a> {
    /// The `Result<T, T>` a `cmpxchg` becomes: `Ok(prev)` when the compare
    /// matched, `Err(prev)` when it did not, where `prev` is the one value the
    /// `cmpxchg` read and `ok` is its `i1` half.
    ///
    /// Written in `emit_enum_lit`'s layout and by the same two field indices,
    /// with one difference: the tag is a `select` of the two variant indices
    /// rather than a constant, because which arm is live is the compare's
    /// outcome. The variants are found by name (`Ok`/`Err`, the same recognition
    /// `?` uses) so this lowers against any two-variant `Ok`/`Err` shape, not
    /// against one particular declaration.
    pub(crate) fn emit_cmpxchg_result(
        &mut self,
        ret: &Ty,
        ok: Value<'a>,
        prev: Value<'a>,
    ) -> Result<Value<'a>, String> {
        let name = struct_name_of_ty(ret).ok_or_else(|| no_value("the Result type"))?;
        let variant_of = |variant: &str| -> Result<usize, String> {
            self.g
                .program
                .enums
                .iter()
                .find(|e| e.name == name)
                .and_then(|e| e.variant(variant))
                .map(|(index, _)| index)
                .ok_or_else(|| no_value(&format!("the '{variant}' variant of '{name}'")))
        };
        let ok_index = variant_of("Ok")?;
        let err_index = variant_of("Err")?;
        let info = self
            .g
            .enums
            .get(name)
            .cloned()
            .ok_or_else(|| no_value(&format!("the enum '{name}'")))?;
        let slot = self.temp_alloca(info.llvm_ty, "atomic.cx");
        self.g
            .builder
            .build_store(self.g.ctx.const_null(info.llvm_ty), slot);
        let tag_ptr = self
            .g
            .builder
            .build_struct_gep(info.llvm_ty, slot, ENUM_TAG_FIELD, "tag");
        let i32t = info.tag_ty;
        let ok_tag = self.g.ctx.const_int(i32t, ok_index as u64, false);
        let err_tag = self.g.ctx.const_int(i32t, err_index as u64, false);
        let tag = self.g.builder.build_select(ok, ok_tag, err_tag, "atomic.tag");
        self.g.builder.build_store(tag, tag_ptr);
        let payload_ty = info
            .variants
            .get(ok_index)
            .ok_or_else(|| no_value("the 'Ok' payload"))?
            .payload_ty;
        let union_ptr = self
            .g
            .builder
            .build_struct_gep(info.llvm_ty, slot, ENUM_UNION_FIELD, "kelp_v");
        let field_ptr = self
            .g
            .builder
            .build_struct_gep(payload_ty, union_ptr, 0, "payload");
        self.g.builder.build_store(prev, field_ptr);
        Ok(self.g.builder.build_load(info.llvm_ty, slot, "atomic.cx.val"))
    }

    /// `thread.spawn(entry, arg)` (MK9-S3-M1 stage C1): allocate the handle and
    /// its two slots, move the argument into the argument slot, and start the
    /// worker with the generated body for this signature. The handle's element
    /// is the entry's *return* type, which is the value the worker hands its
    /// joiner, so `sizeof` that type is the result slot. The C twin is
    /// the earlier emitter's `thread_spawn_value`, runtime call for runtime call.
    pub(crate) fn emit_thread_spawn(&mut self, args: &[Expr], ret: &Ty) -> Result<Option<Value<'a>>, String> {
        let Ty::Thread(elem) = ret else {
            unreachable!("thread.spawn hands back a Thread<T> handle")
        };
        let (entry, arg) = match args {
            [entry, arg] => (entry, arg),
            _ => return Err(no_value("the two arguments of thread.spawn")),
        };
        let Ty::Closure { params, .. } = &entry.ty else {
            unreachable!("thread.spawn's entry is a closure by the checker's coercion")
        };
        // The entry first, as the earlier emitter binds it to a temporary in front of
        // the call, so a field or a dereference is evaluated once.
        let closure = self.value_of(entry)?;
        let code = self.g.builder.build_extract_value(closure, 0, "fn");
        let env = self.g.builder.build_extract_value(closure, 1, "env");
        let drop = self.g.builder.build_extract_value(closure, 2, "drop");
        let arg_size = self.g.layout.abi_size_of(self.lty(&params[0])?);
        // The roots of the argument and the result (managed only; null when the
        // value holds no collected memory), so the record survives a clearing.

        // A worker that returns `unit` stores nothing: `unit` has no LLVM type
        // at all (`llvm_type` refuses it), and a handle on such a worker is one
        // `Thread<unit>` a program may drop but not read a result out of. The
        // runtime still allocates a byte for the slot.
        let result_size = if *elem.as_ref() == Ty::Unit {
            0
        } else {
            self.g.layout.abi_size_of(self.lty(elem)?)
        };

        let new = self.g.rt("kelp_thread_new")?;
        let handle = self.g.call(
            new.func,
            new.ty,
            &[self.g.v_i64(arg_size), self.g.v_i64(result_size)],
            "thread",
        );
        self.cur = self.g.check_alloc(handle, self.cur_fn)?;
        let arg_fn = self.g.rt("kelp_thread_arg")?;
        let arg_slot = self.g.call(arg_fn.func, arg_fn.ty, &[handle], "arg");
        let mut held: Vec<(Value<'a>, Ty)> = Vec::new();
        let moved = self.arg_value(arg, &mut held)?;
        self.g.builder.build_store(moved, arg_slot);
        let body = self.g.closure_symbol(&thread_body_symbol(&entry.ty))?;
        // The argument's own drop, for a spawn that never starts (the runtime drops
        // it then; a started body drops its own parameter).
        let arg_drop = self
            .drop_fn_value(&params[0])?
            .unwrap_or_else(|| self.g.ctx.const_null(self.g.pty()));
        if self.g.managed {
            // Managed: the spawn's argument and result are rooted (section 3).
            let arg_descr = self.g.record_descr(&params[0])?;
            let result_descr = if *elem.as_ref() == Ty::Unit {
                self.g.ctx.const_null(self.g.pty())
            } else {
                self.g.record_descr(elem)?
            };
            let start = self.g.rt("kelp_thread_start_rooted_dropping")?;
            self.g.call(
                start.func,
                start.ty,
                &[handle, body.as_value(), code, env, drop, arg_descr, result_descr, arg_drop],
                "",
            );
        } else {
            let start = self.g.rt("kelp_thread_start_dropping")?;
            self.g.call(
                start.func,
                start.ty,
                &[handle, body.as_value(), code, env, drop, arg_drop],
                "",
            );
        }
        self.free_held(held)?;
        Ok(Some(handle))
    }

    /// One element type's own drop, as a function-pointer value, or `None` when
    /// the type owns nothing. `Mutex.new` stores this so the runtime's record
    /// can free what it holds (B-5); it is the same lookup `call_drop` does,
    /// one level earlier because a call is being built rather than emitted.
    pub(crate) fn drop_fn_value(&self, ty: &Ty) -> Result<Option<Value<'a>>, String> {
        if !needs_drop(ty, &self.g.owned) {
            return Ok(None);
        }
        let func = match ty {
            Ty::Bytes | Ty::String | Ty::Thread(_) | Ty::Mutex(_) | Ty::RwLock(_) | Ty::Condvar => {
                let symbol = drop_fn_name(ty);
                self.g
                    .runtime
                    .get(&symbol)
                    .ok_or_else(|| no_value(&format!("the runtime drop '{symbol}'")))?
                    .func
            }
            Ty::Struct(name) => match self.g.drop_fns.get(name) {
                Some((func, _)) => *func,
                None => return Ok(None),
            },
            Ty::List(_) | Ty::Box(_) | Ty::Map(..) | Ty::Array(..) => {
                let symbol = drop_fn_name(ty);
                match self.g.container_fns.get(&symbol) {
                    Some((func, _)) => *func,
                    None => return Ok(None),
                }
            }
            Ty::Closure { .. } => {
                let symbol = drop_fn_name(ty);
                match self.g.closure_fns.get(&symbol) {
                    Some((func, _)) => *func,
                    None => return Ok(None),
                }
            }
            _ => return Ok(None),
        };
        Ok(Some(func.as_value()))
    }

    /// A `Mutex<T>` operation (B-5, `std/sync`): `new` builds the runtime record,
    /// `lock` runs the generated block body while the lock is held, and
    /// `into_inner` moves the value out and frees the record. The C twin is
    /// the earlier emitter's `mutex_call`, runtime call for runtime call.
    pub(crate) fn emit_mutex_call(&mut self, op: &str, args: &[Expr], ret: &Ty) -> Result<Option<Value<'a>>, String> {
        let ptr = self.g.pty();
        match op {
            "new" => {
                let Ty::Mutex(elem) = ret else {
                    unreachable!("Mutex.new hands back a Mutex<T>")
                };
                let value = self.value_of(&args[0])?;
                let elem_ty = self.lty(elem)?;
                let slot = self.temp_alloca(elem_ty, "mutex.value");
                self.g.builder.build_store(value, slot);
                let size = self.g.layout.abi_size_of(elem_ty);
                let drop = self
                    .drop_fn_value(elem)?
                    .unwrap_or_else(|| self.g.ctx.const_null(ptr));
                let handle = if self.g.managed {
                    let descr = self.g.record_descr(elem)?;
                    let rt = self.g.rt("kelp_mutex_new_rooted")?;
                    self.g.call(rt.func, rt.ty, &[slot, self.g.v_i64(size), drop, descr], "mutex")
                } else {
                    let rt = self.g.rt("kelp_mutex_new")?;
                    self.g.call(rt.func, rt.ty, &[slot, self.g.v_i64(size), drop], "mutex")
                };
                self.cur = self.g.check_alloc(handle, self.cur_fn)?;
                Ok(Some(handle))
            }
            "lock" => {
                let block = &args[1];
                let (_, block_ret) = crate::types::closure_parts(&block.ty)
                    .ok_or_else(|| no_value("Mutex.lock's block signature"))?;
                // The receiver is `&Mutex<T>`, so its value is the address of
                // the slot holding the record pointer.
                let cell = self.value_of(&args[0])?;
                let mutex = self.g.builder.build_load(ptr, cell, "mutex");
                let closure = self.value_of(block)?;
                let code = self.g.builder.build_extract_value(closure, 0, "fn");
                let env = self.g.builder.build_extract_value(closure, 1, "env");
                let body = self
                    .g
                    .closure_symbol(&thread_body_symbol(&crate::types::closure_shape(&block.ty)))?;
                let yields = *block_ret != Ty::Unit;
                // A `unit` body writes nothing, so the slot is one byte nothing
                // reads, exactly as a worker's result slot is.
                let out_ty = if yields { self.lty(block_ret)? } else { self.g.ctx.i8() };
                let out = self.temp_alloca(out_ty, "mutex.out");
                let out_ptr = if yields { out } else { self.g.ctx.const_null(ptr) };
                let rt = self.g.rt("kelp_mutex_run")?;
                self.g.builder.build_call(
                    rt.func,
                    rt.ty,
                    &[mutex, body.as_value(), code, env, out_ptr],
                    "",
                );
                if yields {
                    Ok(Some(self.g.builder.build_load(out_ty, out, "lock.value")))
                } else {
                    Ok(None)
                }
            }
            "into_inner" => {
                let Ty::Mutex(_) = &args[0].ty else {
                    unreachable!("Mutex.into_inner's receiver is a Mutex<T>")
                };
                let mutex = self.value_of(&args[0])?;
                let rt = self.g.rt("kelp_mutex_into")?;
                if *ret == Ty::Unit {
                    self.g.builder.build_call(rt.func, rt.ty, &[mutex, self.g.ctx.const_null(ptr)], "");
                    return Ok(None);
                }
                let out_ty = self.lty(ret)?;
                let out = self.temp_alloca(out_ty, "mutex.inner");
                self.g.builder.build_call(rt.func, rt.ty, &[mutex, out], "");
                Ok(Some(self.g.builder.build_load(out_ty, out, "inner")))
            }
            "swap" => {
                let cell = self.value_of(&args[0])?;
                let mutex = self.g.builder.build_load(ptr, cell, "mutex");
                let value = self.value_of(&args[1])?;
                let elem_ty = self.lty(ret)?;
                let incoming = self.temp_alloca(elem_ty, "mutex.in");
                self.g.builder.build_store(value, incoming);
                let outgoing = self.temp_alloca(elem_ty, "mutex.out");
                let rt = self.g.rt("kelp_mutex_swap")?;
                self.g.builder.build_call(rt.func, rt.ty, &[mutex, incoming, outgoing], "");
                Ok(Some(self.g.builder.build_load(elem_ty, outgoing, "swapped")))
            }
            "share" => {
                // The receiver is `&Mutex<T>`: the address of the slot that
                // holds the record pointer. The runtime bumps the count and
                // hands the same pointer back as the new owned handle.
                let cell = self.value_of(&args[0])?;
                let mutex = self.g.builder.build_load(ptr, cell, "mutex");
                let rt = self.g.rt("kelp_mutex_share")?;
                Ok(Some(self.g.call(rt.func, rt.ty, &[mutex], "mutex.share")))
            }
            other => Err(no_value(&format!("the mutex operation '{other}'"))),
        }
    }

    /// Store `Some(payload)` or `None` into a fresh `Option` slot according to
    /// `ran`, and hand back the loaded option. The `try_*` and timed-wait paths
    /// share it (`std/sync`), so the two backends build the same enum.
    pub(crate) fn emit_option_from_run(
        &mut self,
        ret: &Ty,
        ran: Value<'a>,
        payload: Option<Value<'a>>,
    ) -> Result<Value<'a>, String> {
        let Ty::Struct(name) = ret else {
            unreachable!("a scoped-block result is an Option")
        };
        let info = self.enum_info_of(ret)?;
        let decl = self
            .g
            .program
            .enums
            .iter()
            .find(|e| &e.name == name)
            .ok_or_else(|| no_value("the Option enum"))?;
        let some = decl
            .variants
            .iter()
            .position(|v| v.name == "Some")
            .ok_or_else(|| no_value("the 'Some' variant"))?;
        let none = decl
            .variants
            .iter()
            .position(|v| v.name == "None")
            .ok_or_else(|| no_value("the 'None' variant"))?;
        let ret_ty = self.lty(ret)?;
        let slot = self.temp_alloca(ret_ty, "option");
        self.g.builder.build_store(self.g.ctx.const_null(ret_ty), slot);
        let some_bb = self.cur_fn.append_block("opt.some");
        let none_bb = self.cur_fn.append_block("opt.none");
        let done_bb = self.cur_fn.append_block("opt.done");
        self.g.builder.build_cond_br(ran, some_bb, none_bb);
        self.g.builder.position_at_end(some_bb);
        self.cur = some_bb;
        self.terminated = false;
        match payload {
            Some(v) => self.store_enum_variant(slot, &info, some, &[v])?,
            None => self.store_enum_variant(slot, &info, some, &[])?,
        }
        self.g.builder.build_br(done_bb);
        self.g.builder.position_at_end(none_bb);
        self.cur = none_bb;
        self.terminated = false;
        self.store_enum_variant(slot, &info, none, &[])?;
        self.g.builder.build_br(done_bb);
        self.g.builder.position_at_end(done_bb);
        self.cur = done_bb;
        self.terminated = false;
        Ok(self.g.builder.build_load(ret_ty, slot, "option.value"))
    }
}
