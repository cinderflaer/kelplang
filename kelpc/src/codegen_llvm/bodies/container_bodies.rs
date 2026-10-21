use super::*;

impl<'a> Gen<'a> {
    /// The length of a NUL-terminated `str`, as a loop. This is the one place
    /// the compiler derives the length of a `str` without `strlen`: the scan
    /// stops at the first zero byte.
    pub(crate) fn emit_cstr_len(&self, func: LlvmFunction<'a>, s: Value<'a>) -> Result<Value<'a>, String> {
        let i8t = self.ctx.i8();
        let i64t = self.ctx.i64();
        let i_slot = self.builder.build_alloca(i64t, "len.i");
        self.st(self.v_i64(0), i_slot);
        let header = self.loop_header(func, "len.cond");
        let i = self.ld(i64t, i_slot, "i");
        let byte = self.ld(i8t, self.gep(i8t, s, i, "at"), "byte");
        let nul = self.builder.build_icmp(IntCmp::Eq, byte, self.v_i8(0), "nul");
        let step = self.block(func, "len.step");
        let done = self.block(func, "len.end");
        self.builder.build_cond_br(nul, done, step);
        self.builder.position_at_end(step);
        self.st(self.builder.build_int_binop(IntBinOp::Add, i, self.v_i64(1), "i1"), i_slot);
        self.builder.build_br(header);
        self.builder.position_at_end(done);
        Ok(self.ld(i64t, i_slot, "len"))
    }

    // ---- container bodies -------------------------------------------------

    pub(crate) fn emit_container_bodies(&self) -> Result<(), String> {
        for elem in self.lists_sorted() {
            self.emit_list_bodies(&elem)?;
        }
        for elem in self.boxes_sorted() {
            self.emit_box_bodies(&elem)?;
        }
        for (key, value) in self.maps_sorted() {
            self.emit_map_bodies(&key, &value)?;
        }
        for arr in self.arrays_sorted() {
            if let Ty::Array(elem, len) = arr {
                self.emit_array_bodies(&elem, len)?;
            }
        }
        Ok(())
    }

    /// One `Box<T>` function set, the mirror of the earlier emitter's `write_box`. The box
    /// holds one pointer or `NULL`; `take` moves the value out, which is what
    /// makes a recursive `datac` expressible without `Option`.
    pub(crate) fn emit_box_bodies(&self, elem: &Ty) -> Result<(), String> {
        let base = box_c_type(elem);
        let box_ty = self.cty(&Ty::Box(Box::new(elem.clone())))?;
        let elem_ty = self.lty2(elem)?;
        let elem_size = self.layout.abi_size_of(elem_ty);
        let elem_align = self.layout.abi_alignment_of(elem_ty) as u64;
        let ptr = self.pty();
        let i1t = self.ctx.i1();
        let drop_elem = needs_drop(elem, &self.owned);
        let null = self.ctx.const_null(ptr);

        // --- empty ---
        {
            let _ = self.start(&format!("{base}__empty"), "entry")?;
            let b = self.builder.build_alloca(box_ty, "b");
            self.st(null, self.fld(box_ty, b, 0, "value"));
            let v = self.ld(box_ty, b, "empty");
            self.builder.build_ret(v);
        }

        // --- of(v) ---
        {
            let (f, _) = self.start(&format!("{base}__of"), "entry")?;
            let v = f.get_param(0);
            let b = self.builder.build_alloca(box_ty, "b");
            let p = if self.managed {
                self.alloc_elem(elem, elem_ty)?
            } else {
                self.box_alloc_in(f, elem_ty)?
            };
            self.check_alloc(p, f)?;
            self.st(v, p);
            self.st(p, self.fld(box_ty, b, 0, "value"));
            let out = self.ld(box_ty, b, "of");
            self.builder.build_ret(out);
        }

        // --- is_empty ---
        {
            let (f, _) = self.start(&format!("{base}__is_empty"), "entry")?;
            let value = self.ld(ptr, self.fld(box_ty, f.get_param(0), 0, "value"), "value");
            let e = self.builder.build_icmp(IntCmp::Eq, value, null, "empty");
            self.builder.build_ret(e);
        }

        // --- get / get_const ---
        if elem.is_copy() {
            let (f, _) = self.start(&format!("{base}__get"), "entry")?;
            let value = self.ld(ptr, self.fld(box_ty, f.get_param(0), 0, "value"), "value");
            let empty = self.builder.build_icmp(IntCmp::Eq, value, null, "empty");
            let bad = self.block(f, "empty");
            let ok = self.block(f, "get");
            self.builder.build_cond_br(empty, bad, ok);
            self.builder.position_at_end(bad);
            self.emit_panic("Box.get: the box is empty")?;
            self.builder.position_at_end(ok);
            let v = self.ld(elem_ty, value, "get");
            self.builder.build_ret(v);
        } else {
            let (f, _) = self.start(&format!("{base}__get_const"), "entry")?;
            let value = self.ld(ptr, self.fld(box_ty, f.get_param(0), 0, "value"), "value");
            let empty = self.builder.build_icmp(IntCmp::Eq, value, null, "empty");
            let bad = self.block(f, "empty");
            let ok = self.block(f, "get");
            self.builder.build_cond_br(empty, bad, ok);
            self.builder.position_at_end(bad);
            self.emit_panic("Box.get: the box is empty")?;
            self.builder.position_at_end(ok);
            self.builder.build_ret(value);
        }

        // --- set(self, v) ---
        {
            let (f, _) = self.start(&format!("{base}__set"), "entry")?;
            let self_p = f.get_param(0);
            let v = f.get_param(1);
            let slot = self.fld(box_ty, self_p, 0, "value");
            let value = self.ld(ptr, slot, "value");
            let empty = self.builder.build_icmp(IntCmp::Eq, value, null, "empty");
            let null_bb = self.block(f, "alloc");
            let have_bb = self.block(f, "have");
            let stored = self.block(f, "store");
            self.builder.build_cond_br(empty, null_bb, have_bb);
            self.builder.position_at_end(null_bb);
            let p = if self.managed {
                self.alloc_elem(elem, elem_ty)?
            } else {
                self.box_alloc_in(f, elem_ty)?
            };
            self.check_alloc(p, f)?;
            self.st(p, slot);
            self.builder.build_br(stored);
            self.builder.position_at_end(have_bb);
            if drop_elem {
                self.call_drop(value, elem)?;
            }
            self.builder.build_br(stored);
            self.builder.position_at_end(stored);
            let now = self.ld(ptr, slot, "value");
            self.st(v, now);
            self.builder.build_ret_void();
        }

        // --- take(self) ---
        {
            let (f, _) = self.start(&format!("{base}__take"), "entry")?;
            let self_p = f.get_param(0);
            let slot = self.fld(box_ty, self_p, 0, "value");
            let value = self.ld(ptr, slot, "value");
            let empty = self.builder.build_icmp(IntCmp::Eq, value, null, "empty");
            let bad = self.block(f, "empty");
            let ok = self.block(f, "take");
            self.builder.build_cond_br(empty, bad, ok);
            self.builder.position_at_end(bad);
            self.emit_panic("Box.take: the box is empty")?;
            self.builder.position_at_end(ok);
            let v = self.ld(elem_ty, value, "take");
            self.box_free_in(f, value, elem_size, elem_align)?;
            self.st(null, slot);
            self.builder.build_ret(v);
        }

        // --- drop(self) ---
        {
            let (f, _) = self.start(&drop_fn_name(&Ty::Box(Box::new(elem.clone()))), "entry")?;
            let self_p = f.get_param(0);
            let slot = self.fld(box_ty, self_p, 0, "value");
            let value = self.ld(ptr, slot, "value");
            let empty = self.builder.build_icmp(IntCmp::Eq, value, null, "empty");
            let done = self.block(f, "ret");
            let live = self.block(f, "live");
            self.builder.build_cond_br(empty, done, live);
            self.builder.position_at_end(live);
            if drop_elem {
                self.call_drop(value, elem)?;
            }
            self.box_free_in(f, value, elem_size, elem_align)?;
            self.st(null, slot);
            self.builder.build_br(done);
            self.builder.position_at_end(done);
            self.builder.build_ret_void();
        }
        let _ = i1t;
        Ok(())
    }

    /// One `[T; N]` function set, the mirror of the earlier emitter's `write_array`. The
    /// array is `{ T v[N] }` held inline, so nothing here allocates; `len()` is
    /// the type's own constant and `get`/`set` are the same bounds check
    /// `List` uses.
    pub(crate) fn emit_array_bodies(&self, elem: &Ty, len: u64) -> Result<(), String> {
        let base = crate::types::array_c_type(elem, len);
        let array_ty = self.cty(&Ty::Array(Box::new(elem.clone()), len))?;
        let elem_ty = self.lty2(elem)?;
        let i64t = self.ctx.i64();
        let i1t = self.ctx.i1();
        let drop_elem = needs_drop(elem, &self.owned);
        let n = self.v_i64(len);

        // --- len / is_empty ---
        {
            let (f, _) = self.start(&format!("{base}__len"), "entry")?;
            let _ = f;
            self.builder.build_ret(n);
        }
        {
            let (f, _) = self.start(&format!("{base}__is_empty"), "entry")?;
            let _ = f;
            self.builder.build_ret(self.ctx.const_bool(false));
        }

        // --- bounds(i) ---
        {
            let (f, _) = self.start(&format!("{base}__bounds"), "entry")?;
            let i = f.get_param(0);
            let bad = self.builder.build_icmp(IntCmp::Uge, i, n, "oob");
            let oob = self.block(f, "oob");
            let ok = self.block(f, "inbounds");
            self.builder.build_cond_br(bad, oob, ok);
            self.builder.position_at_end(oob);
            self.emit_panic("Array.get: index out of bounds")?;
            self.builder.position_at_end(ok);
            self.builder.build_ret_void();
        }

        // --- get / get_const ---
        if elem.is_copy() {
            let (f, _) = self.start(&format!("{base}__get"), "entry")?;
            let self_p = f.get_param(0);
            let i = f.get_param(1);
            let (bounds, bty) = self.cfn(&format!("{base}__bounds"))?;
            self.call(bounds, bty, &[i], "");
            let v0 = self.fld(array_ty, self_p, 0, "v");
            let addr = self.gep(elem_ty, v0, i, "at");
            let v = self.ld(elem_ty, addr, "get");
            self.builder.build_ret(v);
        } else {
            let (f, _) = self.start(&format!("{base}__get_const"), "entry")?;
            let self_p = f.get_param(0);
            let i = f.get_param(1);
            let (bounds, bty) = self.cfn(&format!("{base}__bounds"))?;
            self.call(bounds, bty, &[i], "");
            let v0 = self.fld(array_ty, self_p, 0, "v");
            let addr = self.gep(elem_ty, v0, i, "at");
            self.builder.build_ret(addr);
        }

        // --- set(self, i, v) ---
        {
            let (f, _) = self.start(&format!("{base}__set"), "entry")?;
            let self_p = f.get_param(0);
            let i = f.get_param(1);
            let v = f.get_param(2);
            let (bounds, bty) = self.cfn(&format!("{base}__bounds"))?;
            self.call(bounds, bty, &[i], "");
            let v0 = self.fld(array_ty, self_p, 0, "v");
            let addr = self.gep(elem_ty, v0, i, "at");
            if drop_elem {
                self.call_drop(addr, elem)?;
            }
            self.st(v, addr);
            self.builder.build_ret_void();
        }

        // --- fill(v) ---
        {
            let (f, _) = self.start(&format!("{base}__fill"), "entry")?;
            let v = f.get_param(0);
            let out = self.builder.build_alloca(array_ty, "r");
            let v0 = self.fld(array_ty, out, 0, "v");
            let i_slot = self.builder.build_alloca(i64t, "i");
            self.st(self.v_i64(0), i_slot);
            let header = self.loop_header(f, "fill.cond");
            let i = self.ld(i64t, i_slot, "i");
            let more = self.builder.build_icmp(IntCmp::Ult, i, n, "more");
            let body = self.block(f, "fill.body");
            let exit = self.block(f, "fill.end");
            self.builder.build_cond_br(more, body, exit);
            self.builder.position_at_end(body);
            let addr = self.gep(elem_ty, v0, i, "at");
            self.st(v, addr);
            let next = self.builder.build_int_binop(IntBinOp::Add, i, self.v_i64(1), "i1");
            self.st(next, i_slot);
            self.builder.build_br(header);
            self.builder.position_at_end(exit);
            let out_v = self.ld(array_ty, out, "fill");
            self.builder.build_ret(out_v);
        }

        // --- fill_into(dst, v) ---
        {
            let (f, _) = self.start(&format!("{base}__fill_into"), "entry")?;
            let dst = f.get_param(0);
            let v = f.get_param(1);
            let v0 = self.fld(array_ty, dst, 0, "v");
            let i_slot = self.builder.build_alloca(i64t, "i");
            self.st(self.v_i64(0), i_slot);
            let header = self.loop_header(f, "fill.cond");
            let i = self.ld(i64t, i_slot, "i");
            let more = self.builder.build_icmp(IntCmp::Ult, i, n, "more");
            let body = self.block(f, "fill.body");
            let exit = self.block(f, "fill.end");
            self.builder.build_cond_br(more, body, exit);
            self.builder.position_at_end(body);
            let addr = self.gep(elem_ty, v0, i, "at");
            self.st(v, addr);
            let next = self.builder.build_int_binop(IntBinOp::Add, i, self.v_i64(1), "i1");
            self.st(next, i_slot);
            self.builder.build_br(header);
            self.builder.position_at_end(exit);
            self.builder.build_ret_void();
        }

        // --- drop(self) ---
        {
            let (f, _) = self.start(
                &drop_fn_name(&Ty::Array(Box::new(elem.clone()), len)),
                "entry",
            )?;
            let self_p = f.get_param(0);
            if drop_elem {
                let v0 = self.fld(array_ty, self_p, 0, "v");
                let i_slot = self.builder.build_alloca(i64t, "i");
                self.st(self.v_i64(0), i_slot);
                let header = self.loop_header(f, "drop.cond");
                let i = self.ld(i64t, i_slot, "i");
                let more = self.builder.build_icmp(IntCmp::Ult, i, n, "more");
                let body = self.block(f, "drop.body");
                let exit = self.block(f, "drop.end");
                self.builder.build_cond_br(more, body, exit);
                self.builder.position_at_end(body);
                let addr = self.gep(elem_ty, v0, i, "at");
                self.call_drop(addr, elem)?;
                let next = self.builder.build_int_binop(IntBinOp::Add, i, self.v_i64(1), "i1");
                self.st(next, i_slot);
                self.builder.build_br(header);
                self.builder.position_at_end(exit);
            }
            self.builder.build_ret_void();
        }
        let _ = i1t;
        Ok(())
    }

    pub(crate) fn map_key_arg(&self, key: &Ty) -> Result<Type<'a>, String> {
        if key == &Ty::String {
            Ok(self.pty())
        } else if matches!(key, Ty::Struct(_)) {
            // A user key travels as a shared borrow: the map clones its own
            // copy in, and passing an owned `datac` (or one with an owned
            // field) by value would move or alias it.
            Ok(self.pty())
        } else {
            self.lty2(key)
        }
    }

    pub(crate) fn map_specs(&self, key: &Ty, value: &Ty) -> Result<Vec<CfnSpec<'a>>, String> {
        let base = map_c_type(key, value);
        let map_ty = self.cty(&Ty::Map(Box::new(key.clone()), Box::new(value.clone())))?;
        let value_ty = self.lty2(value)?;
        let key_arg = self.map_key_arg(key)?;
        let list_key_ty = self.cty(&Ty::List(Box::new(key.clone())))?;
        let list_value_ty = self.cty(&Ty::List(Box::new(value.clone())))?;
        let ptr = self.pty();
        let void = self.ctx.void();
        let (i1t, i64t) = (self.ctx.i1(), self.ctx.i64());
        let clonable = crate::types::value_is_clonable(value, &self.clonable);
        let mut out = vec![
            spec(&format!("{base}__new"), map_ty, &[]),
            spec(&format!("{base}__with_capacity"), map_ty, &[i64t]),
            spec(&format!("{base}__find"), i64t, &[ptr, key_arg, ptr]),
            spec(&format!("{base}__rehash"), void, &[ptr, i64t]),
            spec(&format!("{base}__insert"), void, &[ptr, key_arg, value_ty]),
            spec(&format!("{base}__has"), i1t, &[ptr, key_arg]),
        ];
        if value.is_copy() {
            out.push(spec(&format!("{base}__get"), value_ty, &[ptr, key_arg]));
        } else {
            out.push(spec(&format!("{base}__get_const"), ptr, &[ptr, key_arg]));
        }
        if clonable {
            out.push(spec(
                &format!("{base}__get_or"),
                value_ty,
                &[ptr, key_arg, value_ty],
            ));
        }
        out.push(spec(&format!("{base}__remove"), value_ty, &[ptr, key_arg]));
        out.push(spec(&format!("{base}__clear"), void, &[ptr]));
        out.push(spec(&format!("{base}__len"), i64t, &[ptr]));
        out.push(spec(&format!("{base}__is_empty"), i1t, &[ptr]));
        out.push(spec(&format!("{base}__keys"), list_key_ty, &[ptr]));
        if clonable {
            out.push(spec(&format!("{base}__values"), list_value_ty, &[ptr]));
        }
        out.push(spec(
            &drop_fn_name(&Ty::Map(Box::new(key.clone()), Box::new(value.clone()))),
            void,
            &[ptr],
        ));
        Ok(out)
    }

    /// The LLVM function that hashes a key of a user type: the type's own
    /// declared `hash` method when it declares a key pair (A-12), else the
    /// derived `kelp_hash_<T>`. Exactly one of the two is emitted for a key
    /// type, because a declared pair is used *instead of* the derived one.
    pub(crate) fn key_hash_fn(&self, name: &str) -> Result<(LlvmFunction<'a>, FnType<'a>), String> {
        if self.program.declared_keys.contains(name) {
            let key = crate::types::key_pair_method_key(name, crate::types::KEY_PAIR_HASH_METHOD);
            let info = self
                .funcs
                .get(&key)
                .ok_or_else(|| no_value(&format!("the hash of '{name}'")))?;
            return Ok((info.func, info.ty));
        }
        self.hash_fns
            .get(name)
            .copied()
            .ok_or_else(|| no_value(&format!("the hash of '{name}'")))
    }

    /// As `key_hash_fn`, for the key's equality.
    pub(crate) fn key_eq_fn(&self, name: &str) -> Result<(LlvmFunction<'a>, FnType<'a>), String> {
        if self.program.declared_keys.contains(name) {
            let key = crate::types::key_pair_method_key(name, crate::types::KEY_PAIR_EQ_METHOD);
            let info = self
                .funcs
                .get(&key)
                .ok_or_else(|| no_value(&format!("the equality of '{name}'")))?;
            return Ok((info.func, info.ty));
        }
        self.eq_fns
            .get(name)
            .copied()
            .ok_or_else(|| no_value(&format!("the equality of '{name}'")))
    }

    /// `kelp_map_hash`/`kelp_map_mix`/`kelp_hash_<T>` over a key *argument* (a
    /// `str` view for a `String` key, the value itself for a scalar, a borrow
    /// for a user key), the mirror of `write_map`'s `hash_arg`. `cur_fn` is
    /// the enclosing map method being built (`find`, `insert`, ...),
    /// threaded through only for the `String` case's `emit_map_hash` loop,
    /// which needs a function to append its blocks to.
    pub(crate) fn map_key_hash_arg(
        &self,
        cur_fn: LlvmFunction<'a>,
        key: &Ty,
        k: Value<'a>,
    ) -> Result<Value<'a>, String> {
        if key == &Ty::String {
            let (strlen, sty) = self.libc_fn("strlen")?;
            let n = self.call(strlen, sty, &[k], "len");
            Ok(self.emit_map_hash(cur_fn, k, n))
        } else if let Ty::Struct(name) = key {
            let (func, hty) = self.key_hash_fn(name)?;
            Ok(self.call(func, hty, &[k], "hash"))
        } else {
            let wide = self.key_to_u64(key, k)?;
            Ok(self.emit_map_mix(wide))
        }
    }

    /// The same hash over a *stored* key (a `kelp_String` inline in the entry,
    /// a scalar, or a user key inline in the entry). `cur_fn` as in
    /// `map_key_hash_arg`.
    pub(crate) fn map_key_hash_stored(
        &self,
        cur_fn: LlvmFunction<'a>,
        key: &Ty,
        entry_ty: Type<'a>,
        entry: Value<'a>,
    ) -> Result<Value<'a>, String> {
        if key == &Ty::String {
            let key_addr = self.fld(entry_ty, entry, 0, "key");
            let data = self.ld(self.pty(), self.fld(self.lty2(key)?, key_addr, 0, "d"), "d");
            let len = self.ld(self.ctx.i64(), self.fld(self.lty2(key)?, key_addr, 1, "n"), "n");
            Ok(self.emit_map_hash(cur_fn, data, len))
        } else if let Ty::Struct(name) = key {
            let key_addr = self.fld(entry_ty, entry, 0, "key");
            let (func, hty) = self.key_hash_fn(name)?;
            Ok(self.call(func, hty, &[key_addr], "hash"))
        } else {
            let key_ty = self.lty2(key)?;
            let kv = self.ld(key_ty, self.fld(entry_ty, entry, 0, "key"), "key");
            let wide = self.key_to_u64(key, kv)?;
            Ok(self.emit_map_mix(wide))
        }
    }

    /// Whether the entry's stored key equals the argument `k`.
    pub(crate) fn map_key_eq_stored(
        &self,
        key: &Ty,
        entry_ty: Type<'a>,
        entry: Value<'a>,
        k: Value<'a>,
    ) -> Result<Value<'a>, String> {
        if key == &Ty::String {
            let key_addr = self.fld(entry_ty, entry, 0, "key");
            let (strlen, sty) = self.libc_fn("strlen")?;
            let n = self.call(strlen, sty, &[k], "len");
            let rt = self.rt("kelp_map_key_eq")?;
            Ok(self.call(rt.func, rt.ty, &[key_addr, k, n], "eq"))
        } else if let Ty::Struct(name) = key {
            let key_addr = self.fld(entry_ty, entry, 0, "key");
            let (func, ety) = self.key_eq_fn(name)?;
            Ok(self.call(func, ety, &[key_addr, k], "eq"))
        } else {
            let key_ty = self.lty2(key)?;
            let kv = self.ld(key_ty, self.fld(entry_ty, entry, 0, "key"), "key");
            Ok(self.builder.build_icmp(IntCmp::Eq, kv, k, "eq"))
        }
    }

    /// The key value an `insert` stores: a `String` key is copied in from the
    /// `str` view, a user key is cloned in from the borrow, a scalar key is
    /// stored as it arrived.
    pub(crate) fn map_key_value(&self, key: &Ty, k: Value<'a>, slot: Value<'a>) -> Result<Value<'a>, String> {
        if key == &Ty::String {
            self.sret_call("kelp_String__from_str", slot, &[k])?;
            Ok(self.ld(self.lty2(key)?, slot, "key"))
        } else if matches!(key, Ty::Struct(_)) {
            let key_ty = self.lty2(key)?;
            self.clone_into(k, slot, key)?;
            Ok(self.ld(key_ty, slot, "key"))
        } else {
            Ok(k)
        }
    }

    /// A copy of the entry's stored key, for `keys()`: a `String` or a user key
    /// is cloned, a scalar is loaded.
    pub(crate) fn map_key_clone(
        &self,
        key: &Ty,
        entry_ty: Type<'a>,
        entry: Value<'a>,
        slot: Value<'a>,
    ) -> Result<Value<'a>, String> {
        if key == &Ty::String {
            let key_addr = self.fld(entry_ty, entry, 0, "key");
            self.sret_call("kelp_String__clone", slot, &[key_addr])?;
            Ok(self.ld(self.lty2(key)?, slot, "key"))
        } else if matches!(key, Ty::Struct(_)) {
            let key_addr = self.fld(entry_ty, entry, 0, "key");
            let key_ty = self.lty2(key)?;
            self.clone_into(key_addr, slot, key)?;
            Ok(self.ld(key_ty, slot, "key"))
        } else {
            let key_ty = self.lty2(key)?;
            Ok(self.ld(key_ty, self.fld(entry_ty, entry, 0, "key"), "key"))
        }
    }

    /// Free the entry's stored key: a `String` buffer is freed, a user key is
    /// dropped through its generated drop, a scalar owns nothing.
    pub(crate) fn map_key_drop(
        &self,
        key: &Ty,
        entry_ty: Type<'a>,
        entry: Value<'a>,
    ) -> Result<(), String> {
        if key == &Ty::String {
            let key_addr = self.fld(entry_ty, entry, 0, "key");
            let rt = self.rt("kelp_string_drop")?;
            self.call(rt.func, rt.ty, &[key_addr], "");
        } else if matches!(key, Ty::Struct(_)) {
            let key_addr = self.fld(entry_ty, entry, 0, "key");
            self.call_drop(key_addr, key)?;
        }
        Ok(())
    }

    /// One `Map<K, V>` function set, the mirror of the earlier emitter's `write_map`.
    /// Entries sit in insertion order in a dense array and a separate
    /// open-addressed index of `entry index + 1` in a `u32` (`0` empty,
    /// `UINT32_MAX` tombstone) does the lookup.
    pub(crate) fn emit_map_bodies(&self, key: &Ty, value: &Ty) -> Result<(), String> {
        let base = map_c_type(key, value);
        let map_ty = self.cty(&Ty::Map(Box::new(key.clone()), Box::new(value.clone())))?;
        let entry_ty = *self
            .map_entries
            .get(&Ty::Map(Box::new(key.clone()), Box::new(value.clone())))
            .ok_or_else(|| no_value("the map entry layout"))?;
        let key_ty = self.lty2(key)?;
        let value_ty = self.lty2(value)?;
        let key_arg = self.map_key_arg(key)?;
        let list_key_ty = self.cty(&Ty::List(Box::new(key.clone())))?;
        let list_value_ty = self.cty(&Ty::List(Box::new(value.clone())))?;
        let ptr = self.pty();
        let (i1t, i8t, i32t, i64t) = (self.ctx.i1(), self.ctx.i8(), self.ctx.i32(), self.ctx.i64());
        let entry_size = self.layout.abi_size_of(entry_ty);
        let entry_align = self.layout.abi_alignment_of(entry_ty) as u64;
        // The index is an `i32` array, so its entries are four-byte aligned.
        let index_align = 4u64;
        let value_drops = needs_drop(value, &self.owned);
        let clonable = crate::types::value_is_clonable(value, &self.clonable);
        let size_max = u64::MAX;

        // --- new ---
        {
            let _ = self.start(&format!("{base}__new"), "entry")?;
            let m = self.builder.build_alloca(map_ty, "m");
            self.st(self.ctx.const_null(map_ty), m);
            let v = self.ld(map_ty, m, "new");
            self.builder.build_ret(v);
        }

        // --- with_capacity(cap) ---
        {
            let (f, _) = self.start(&format!("{base}__with_capacity"), "entry")?;
            let cap = f.get_param(0);
            let m = self.builder.build_alloca(map_ty, "m");
            self.st(self.ctx.const_null(map_ty), m);
            let empty = self.block(f, "wc.empty");
            let sized = self.block(f, "wc.sized");
            let zerocap = self.builder.build_icmp(IntCmp::Eq, cap, self.v_i64(0), "zerocap");
            self.builder.build_cond_br(zerocap, empty, sized);
            self.builder.position_at_end(sized);
            let bytes = self.builder.build_int_binop(IntBinOp::Mul, cap, self.v_i64(entry_size), "bytes");
            let entries = if self.managed {
                let descr = self.map_entry_descr(key, value, &base)?;
                self.gc_array(descr, cap)?
            } else {
                self.heap_alloc(bytes, entry_align)?
            };
            self.check_alloc(entries, f)?;
            self.st(entries, self.fld(map_ty, m, M_ENTRIES, "entries"));
            self.st(cap, self.fld(map_ty, m, M_ENTRY_CAP, "entry_cap"));
            // The index is built for `cap` live entries exactly as the first
            // insert after `cap` inserts would have: same `rehash`, same size.
            let (rehash, rhty) = self.cfn(&format!("{base}__rehash"))?;
            self.call(rehash, rhty, &[m, cap], "");
            self.builder.build_br(empty);
            self.builder.position_at_end(empty);
            let v = self.ld(map_ty, m, "map");
            self.builder.build_ret(v);
        }

        // --- find(self, k, found_out) ---
        {
            let (f, _) = self.start(&format!("{base}__find"), "entry")?;
            let self_p = f.get_param(0);
            let k = f.get_param(1);
            let found_p = f.get_param(2);
            self.st(self.ctx.const_bool(false), found_p);
            let icap = self.ld(i64t, self.fld(map_ty, self_p, M_INDEX_CAP, "icap"), "icap");
            let zero = self.builder.build_icmp(IntCmp::Eq, icap, self.v_i64(0), "noindex");
            let none = self.block(f, "find.none");
            let cont = self.block(f, "find.cont");
            self.builder.build_cond_br(zero, none, cont);
            self.builder.position_at_end(none);
            self.builder.build_ret(self.v_i64(0));
            self.builder.position_at_end(cont);
            let mask = self.builder.build_int_binop(IntBinOp::Sub, icap, self.v_i64(1), "mask");
            let h = self.map_key_hash_arg(f, key, k)?;
            let i0 = self.builder.build_int_binop(IntBinOp::And, h, mask, "i0");
            let i_slot = self.builder.build_alloca(i64t, "i");
            let tomb_slot = self.builder.build_alloca(i64t, "tomb");
            self.st(i0, i_slot);
            self.st(self.v_i64(size_max), tomb_slot);
            let loop_bb = self.block(f, "find.loop");
            let empty_bb = self.block(f, "find.empty");
            let probe_bb = self.block(f, "find.probe");
            let tomb_bb = self.block(f, "find.tomb");
            let live_bb = self.block(f, "find.live");
            let eq_bb = self.block(f, "find.eq");
            let found_bb = self.block(f, "find.found");
            let next_bb = self.block(f, "find.next");
            self.builder.build_br(loop_bb);
            // loop
            self.builder.position_at_end(loop_bb);
            let i = self.ld(i64t, i_slot, "i");
            let index = self.ld(ptr, self.fld(map_ty, self_p, M_INDEX, "index"), "index");
            let slot_addr = self.gep(i32t, index, i, "slot");
            let slot = self.ld(i32t, slot_addr, "slot");
            let iszero = self.builder.build_icmp(IntCmp::Eq, slot, self.v_i32(0), "iszero");
            self.builder.build_cond_br(iszero, empty_bb, probe_bb);
            self.builder.position_at_end(empty_bb);
            // empty: the first tombstone seen, else this slot
            self.builder.position_at_end(empty_bb);
            let t = self.ld(i64t, tomb_slot, "tomb");
            let istomb = self.builder.build_icmp(IntCmp::Eq, t, self.v_i64(size_max), "istomb");
            let sel = self.builder.build_select(istomb, i, t, "slot");
            self.builder.build_ret(sel);
            // probe: tombstone, or a live entry to compare
            self.builder.position_at_end(probe_bb);
            let istomb2 = self.builder.build_icmp(IntCmp::Eq, slot, self.v_i32(INDEX_TOMBSTONE), "istomb");
            self.builder.build_cond_br(istomb2, tomb_bb, live_bb);
            self.builder.position_at_end(tomb_bb);
            let t2 = self.ld(i64t, tomb_slot, "tomb");
            let istomb3 = self.builder.build_icmp(IntCmp::Eq, t2, self.v_i64(size_max), "istomb");
            let keep = self.builder.build_select(istomb3, i, t2, "tomb");
            self.st(keep, tomb_slot);
            self.builder.build_br(next_bb);
            self.builder.position_at_end(live_bb);
            let entries = self.ld(ptr, self.fld(map_ty, self_p, M_ENTRIES, "entries"), "entries");
            let slot64 = self.builder.build_int_cast(false, true, slot, i64t, "slot64");
            let e0 = self.builder.build_int_binop(IntBinOp::Sub, slot64, self.v_i64(1), "e");
            let entry = self.gep(entry_ty, entries, e0, "entry");
            let live = self.ld(i8t, self.fld(entry_ty, entry, E_LIVE, "live"), "live");
            let dead = self.builder.build_icmp(IntCmp::Eq, live, self.v_i8(0), "dead");
            self.builder.build_cond_br(dead, next_bb, eq_bb);
            self.builder.position_at_end(eq_bb);
            let eq = self.map_key_eq_stored(key, entry_ty, entry, k)?;
            self.builder.build_cond_br(eq, found_bb, next_bb);
            self.builder.position_at_end(found_bb);
            self.st(self.ctx.const_bool(true), found_p);
            self.builder.build_ret(i);
            self.builder.position_at_end(next_bb);
            let i1 = self.builder.build_int_binop(IntBinOp::Add, i, self.v_i64(1), "i1");
            let i2 = self.builder.build_int_binop(IntBinOp::And, i1, mask, "i2");
            self.st(i2, i_slot);
            self.builder.build_br(loop_bb);
        }

        // --- rehash(self, want) ---
        {
            let (f, _) = self.start(&format!("{base}__rehash"), "entry")?;
            let self_p = f.get_param(0);
            let want = f.get_param(1);
            let cap_slot = self.builder.build_alloca(i64t, "cap");
            self.st(self.v_i64(8), cap_slot);
            let header = self.loop_header(f, "rehash.cond");
            let cap = self.ld(i64t, cap_slot, "cap");
            let four = self.builder.build_int_binop(IntBinOp::Mul, want, self.v_i64(4), "want4");
            let need = self.builder.build_icmp(IntCmp::Ult, cap, four, "need");
            let body = self.block(f, "rehash.body");
            let exit = self.block(f, "rehash.end");
            self.builder.build_cond_br(need, body, exit);
            self.builder.position_at_end(body);
            let cap2 = self.builder.build_int_binop(IntBinOp::Mul, cap, self.v_i64(2), "cap2");
            self.st(cap2, cap_slot);
            self.builder.build_br(header);
            self.builder.position_at_end(exit);
            let final_cap = self.ld(i64t, cap_slot, "cap");
            let old_cap = self.ld(i64t, self.fld(map_ty, self_p, M_INDEX_CAP, "icap"), "icap");
            let old_bytes = self.builder.build_int_binop(IntBinOp::Mul, old_cap, self.v_i64(4), "old.bytes");
            let old = self.ld(ptr, self.fld(map_ty, self_p, M_INDEX, "index"), "index");
            self.heap_dealloc(old, old_bytes, index_align)?;
            let bytes = self.builder.build_int_binop(IntBinOp::Mul, final_cap, self.v_i64(4), "bytes");
            let buf = if self.managed {
                // The index is `u32` slots: a noscan array, so the collector never reads it.
                let descr = self.elem_descr(&Ty::U32)?;
                self.gc_array(descr, final_cap)?
            } else {
                self.heap_alloc(bytes, index_align)?
            };
            self.check_alloc(buf, f)?;
            let (memset, sety) = self.mem_fn("memset")?;
            self.call(memset, sety, &[buf, self.v_i32(0), bytes], "");
            self.st(buf, self.fld(map_ty, self_p, M_INDEX, "index"));
            self.st(final_cap, self.fld(map_ty, self_p, M_INDEX_CAP, "index_cap"));
            self.st(self.v_i64(0), self.fld(map_ty, self_p, M_INDEX_USED, "index_used"));
            // Walk the live entries and re-place each in the fresh index.
            let i_slot = self.builder.build_alloca(i64t, "e");
            self.st(self.v_i64(0), i_slot);
            let eh = self.loop_header(f, "rehash.walk");
            let e = self.ld(i64t, i_slot, "e");
            let count = self.ld(i64t, self.fld(map_ty, self_p, M_ENTRY_COUNT, "count"), "count");
            let more = self.builder.build_icmp(IntCmp::Ult, e, count, "more");
            let ebody = self.block(f, "rehash.walk.body");
            let eexit = self.block(f, "rehash.walk.end");
            self.builder.build_cond_br(more, ebody, eexit);
            self.builder.position_at_end(ebody);
            let entries = self.ld(ptr, self.fld(map_ty, self_p, M_ENTRIES, "entries"), "entries");
            let entry = self.gep(entry_ty, entries, e, "entry");
            let live = self.ld(i8t, self.fld(entry_ty, entry, E_LIVE, "live"), "live");
            let dead = self.builder.build_icmp(IntCmp::Eq, live, self.v_i8(0), "dead");
            let skip = self.block(f, "rehash.skip");
            let place = self.block(f, "rehash.place");
            self.builder.build_cond_br(dead, skip, place);
            self.builder.position_at_end(place);
            let h = self.map_key_hash_stored(f, key, entry_ty, entry)?;
            let mask = self.builder.build_int_binop(IntBinOp::Sub, final_cap, self.v_i64(1), "mask");
            let j_slot = self.builder.build_alloca(i64t, "slot");
            let j0 = self.builder.build_int_binop(IntBinOp::And, h, mask, "j0");
            self.st(j0, j_slot);
            let jh = self.loop_header(f, "rehash.probe");
            let j = self.ld(i64t, j_slot, "j");
            let index = self.ld(ptr, self.fld(map_ty, self_p, M_INDEX, "index"), "index");
            let slot_addr = self.gep(i32t, index, j, "at");
            let holder = self.ld(i32t, slot_addr, "holder");
            let occupied = self.builder.build_icmp(IntCmp::Ne, holder, self.v_i32(0), "occupied");
            let jbody = self.block(f, "rehash.probe.body");
            let jexit = self.block(f, "rehash.probe.end");
            self.builder.build_cond_br(occupied, jbody, jexit);
            self.builder.position_at_end(jbody);
            let jn = self.builder.build_int_binop(IntBinOp::Add, j, self.v_i64(1), "j1");
            let jm = self.builder.build_int_binop(IntBinOp::And, jn, mask, "jm");
            self.st(jm, j_slot);
            self.builder.build_br(jh);
            self.builder.position_at_end(jexit);
            let jf = self.ld(i64t, j_slot, "j");
            let index2 = self.ld(ptr, self.fld(map_ty, self_p, M_INDEX, "index"), "index");
            let placed = self.gep(i32t, index2, jf, "at");
            let ep1 = self.builder.build_int_binop(IntBinOp::Add, e, self.v_i64(1), "ep1");
            let ep1_32 = self.builder.build_int_cast(false, false, ep1, i32t, "ep1_32");
            self.st(ep1_32, placed);
            let used = self.ld(i64t, self.fld(map_ty, self_p, M_INDEX_USED, "used"), "used");
            let used1 = self.builder.build_int_binop(IntBinOp::Add, used, self.v_i64(1), "used1");
            self.st(used1, self.fld(map_ty, self_p, M_INDEX_USED, "used"));
            self.builder.build_br(skip);
            self.builder.position_at_end(skip);
            let en = self.builder.build_int_binop(IntBinOp::Add, e, self.v_i64(1), "e1");
            self.st(en, i_slot);
            self.builder.build_br(eh);
            self.builder.position_at_end(eexit);
            self.builder.build_ret_void();
        }

        // --- has(self, k) ---
        {
            let (f, _) = self.start(&format!("{base}__has"), "entry")?;
            let self_p = f.get_param(0);
            let k = f.get_param(1);
            let found_slot = self.builder.build_alloca(i1t, "found");
            let (find, fnty) = self.cfn(&format!("{base}__find"))?;
            self.call(find, fnty, &[self_p, k, found_slot], "");
            let out = self.ld(i1t, found_slot, "has");
            self.builder.build_ret(out);
        }

        // --- get / get_const ---
        if value.is_copy() {
            let (f, _) = self.start(&format!("{base}__get"), "entry")?;
            let self_p = f.get_param(0);
            let k = f.get_param(1);
            let found_slot = self.builder.build_alloca(i1t, "found");
            let (find, fnty) = self.cfn(&format!("{base}__find"))?;
            let slot = self.call(find, fnty, &[self_p, k, found_slot], "slot");
            let found = self.ld(i1t, found_slot, "found");
            let missing = self.builder.build_icmp(IntCmp::Eq, found, self.ctx.const_bool(false), "missing");
            let bad = self.block(f, "missing");
            let ok = self.block(f, "get");
            self.builder.build_cond_br(missing, bad, ok);
            self.builder.position_at_end(bad);
            self.emit_panic("Map.get: key not found")?;
            self.builder.position_at_end(ok);
            let value_ptr = self.map_value_ptr(map_ty, entry_ty, self_p, slot)?;
            let v = self.ld(value_ty, value_ptr, "get");
            self.builder.build_ret(v);
        } else {
            let (f, _) = self.start(&format!("{base}__get_const"), "entry")?;
            let self_p = f.get_param(0);
            let k = f.get_param(1);
            let found_slot = self.builder.build_alloca(i1t, "found");
            let (find, fnty) = self.cfn(&format!("{base}__find"))?;
            let slot = self.call(find, fnty, &[self_p, k, found_slot], "slot");
            let found = self.ld(i1t, found_slot, "found");
            let missing = self.builder.build_icmp(IntCmp::Eq, found, self.ctx.const_bool(false), "missing");
            let bad = self.block(f, "missing");
            let ok = self.block(f, "get");
            self.builder.build_cond_br(missing, bad, ok);
            self.builder.position_at_end(bad);
            self.emit_panic("Map.get: key not found")?;
            self.builder.position_at_end(ok);
            let value_ptr = self.map_value_ptr(map_ty, entry_ty, self_p, slot)?;
            self.builder.build_ret(value_ptr);
        }

        // --- get_or(self, k, fallback) ---
        if clonable {
            let (f, _) = self.start(&format!("{base}__get_or"), "entry")?;
            let self_p = f.get_param(0);
            let k = f.get_param(1);
            let fallback = f.get_param(2);
            let found_slot = self.builder.build_alloca(i1t, "found");
            let clone_slot = self.builder.build_alloca(value_ty, "clone");
            let (find, fnty) = self.cfn(&format!("{base}__find"))?;
            let slot = self.call(find, fnty, &[self_p, k, found_slot], "slot");
            let found = self.ld(i1t, found_slot, "found");
            let missing = self.builder.build_icmp(IntCmp::Eq, found, self.ctx.const_bool(false), "missing");
            let fb_slot = self.builder.build_alloca(value_ty, "fallback");
            self.st(fallback, fb_slot);
            let miss = self.block(f, "missing");
            let hit = self.block(f, "hit");
            self.builder.build_cond_br(missing, miss, hit);
            self.builder.position_at_end(miss);
            let out = self.ld(value_ty, fb_slot, "fallback");
            self.builder.build_ret(out);
            self.builder.position_at_end(hit);
            let value_ptr = self.map_value_ptr(map_ty, entry_ty, self_p, slot)?;
            let result = self.clone_value(value_ptr, value, clone_slot)?;
            match value {
                Ty::String => {
                    let rt = self.rt("kelp_string_drop")?;
                    self.call(rt.func, rt.ty, &[fb_slot], "");
                }
                Ty::Bytes => {
                    let rt = self.rt("kelp_bytes_drop")?;
                    self.call(rt.func, rt.ty, &[fb_slot], "");
                }
                // The caller moved the fallback in and this path does not
                // hand it back, so the fallback is dropped here like the
                // the way `map__get_or` drops it.
                Ty::Struct(_) if value_drops => self.call_drop(fb_slot, value)?,
                _ => {}
            }
            self.builder.build_ret(result);
        }

        // --- remove(self, k) ---
        {
            let (f, _) = self.start(&format!("{base}__remove"), "entry")?;
            let self_p = f.get_param(0);
            let k = f.get_param(1);
            let found_slot = self.builder.build_alloca(i1t, "found");
            let (find, fnty) = self.cfn(&format!("{base}__find"))?;
            let slot = self.call(find, fnty, &[self_p, k, found_slot], "slot");
            let found = self.ld(i1t, found_slot, "found");
            let missing = self.builder.build_icmp(IntCmp::Eq, found, self.ctx.const_bool(false), "missing");
            let bad = self.block(f, "missing");
            let ok = self.block(f, "remove");
            self.builder.build_cond_br(missing, bad, ok);
            self.builder.position_at_end(bad);
            self.emit_panic("Map.remove: key not found")?;
            self.builder.position_at_end(ok);
            let index0 = self.ld(ptr, self.fld(map_ty, self_p, M_INDEX, "index"), "index");
            let holder0 = self.ld(i32t, self.gep(i32t, index0, slot, "at"), "holder");
            let holder64 = self.builder.build_int_cast(false, true, holder0, i64t, "holder64");
            let e = self.builder.build_int_binop(IntBinOp::Sub, holder64, self.v_i64(1), "e");
            let entries = self.ld(ptr, self.fld(map_ty, self_p, M_ENTRIES, "entries"), "entries");
            let entry = self.gep(entry_ty, entries, e, "entry");
            let value_slot = self.fld(entry_ty, entry, E_VALUE, "value");
            let out_slot = self.builder.build_alloca(value_ty, "out");
            self.st(self.ld(value_ty, value_slot, "v"), out_slot);
            self.map_key_drop(key, entry_ty, entry)?;
            self.st(self.v_i8(0), self.fld(entry_ty, entry, E_LIVE, "live"));
            let index = self.ld(ptr, self.fld(map_ty, self_p, M_INDEX, "index"), "index");
            let slot_addr = self.gep(i32t, index, slot, "at");
            self.st(self.v_i32(INDEX_TOMBSTONE), slot_addr);
            let len = self.ld(i64t, self.fld(map_ty, self_p, M_LEN, "len"), "len");
            let len1 = self.builder.build_int_binop(IntBinOp::Sub, len, self.v_i64(1), "len1");
            self.st(len1, self.fld(map_ty, self_p, M_LEN, "len"));
            let out = self.ld(value_ty, out_slot, "removed");
            self.builder.build_ret(out);
        }

        // --- insert(self, k, v) ---
        {
            let (f, _) = self.start(&format!("{base}__insert"), "entry")?;
            let self_p = f.get_param(0);
            let k = f.get_param(1);
            let v = f.get_param(2);
            let key_slot = self.builder.build_alloca(key_ty, "key.tmp");
            let icap = self.ld(i64t, self.fld(map_ty, self_p, M_INDEX_CAP, "icap"), "icap");
            let iused = self.ld(i64t, self.fld(map_ty, self_p, M_INDEX_USED, "iused"), "iused");
            let len = self.ld(i64t, self.fld(map_ty, self_p, M_LEN, "len"), "len");
            let needs = self.builder.build_int_binop(IntBinOp::Add, iused, self.v_i64(1), "need");
            let needs4 = self.builder.build_int_binop(IntBinOp::Mul, needs, self.v_i64(4), "need4");
            let cap3 = self.builder.build_int_binop(IntBinOp::Mul, icap, self.v_i64(3), "cap3");
            let crowded = self.builder.build_icmp(IntCmp::Uge, needs4, cap3, "crowded");
            let noindex = self.builder.build_icmp(IntCmp::Eq, icap, self.v_i64(0), "noindex");
            let grow = self.builder.build_int_binop(IntBinOp::Or, crowded, noindex, "grow");
            let rehash_bb = self.block(f, "insert.rehash");
            let placed = self.block(f, "insert.placed");
            self.builder.build_cond_br(grow, rehash_bb, placed);
            self.builder.position_at_end(rehash_bb);
            let want = self.builder.build_int_binop(IntBinOp::Add, len, self.v_i64(1), "want");
            let (rehash, rhty) = self.cfn(&format!("{base}__rehash"))?;
            self.call(rehash, rhty, &[self_p, want], "");
            self.builder.build_br(placed);
            self.builder.position_at_end(placed);
            let found_slot = self.builder.build_alloca(i1t, "found");
            let (find, fnty) = self.cfn(&format!("{base}__find"))?;
            let slot = self.call(find, fnty, &[self_p, k, found_slot], "slot");
            let found = self.ld(i1t, found_slot, "found");
            let have = self.block(f, "insert.replace");
            let fresh = self.block(f, "insert.fresh");
            self.builder.build_cond_br(found, have, fresh);
            self.builder.position_at_end(have);
            let value_ptr = self.map_value_ptr(map_ty, entry_ty, self_p, slot)?;
            if value_drops {
                self.call_drop(value_ptr, value)?;
            }
            self.st(v, value_ptr);
            self.builder.build_ret_void();
            self.builder.position_at_end(fresh);
            let count = self.ld(i64t, self.fld(map_ty, self_p, M_ENTRY_COUNT, "count"), "count");
            let ecap = self.ld(i64t, self.fld(map_ty, self_p, M_ENTRY_CAP, "ecap"), "ecap");
            let full = self.builder.build_icmp(IntCmp::Eq, count, ecap, "full");
            let resize = self.block(f, "insert.resize");
            let have_space = self.block(f, "insert.space");
            self.builder.build_cond_br(full, resize, have_space);
            self.builder.position_at_end(resize);
            let is_zero = self.builder.build_icmp(IntCmp::Eq, ecap, self.v_i64(0), "zero");
            let double = self.builder.build_int_binop(IntBinOp::Mul, ecap, self.v_i64(2), "double");
            let new_cap = self.builder.build_select(is_zero, self.v_i64(8), double, "newcap");
            let entries = self.ld(ptr, self.fld(map_ty, self_p, M_ENTRIES, "entries"), "entries");
            let bytes = self.builder.build_int_binop(IntBinOp::Mul, new_cap, self.v_i64(entry_size), "bytes");
            let old_bytes = self.builder.build_int_binop(IntBinOp::Mul, ecap, self.v_i64(entry_size), "old.bytes");
            let new_entries = if self.managed {
                let descr = self.map_entry_descr(key, value, &base)?;
                self.gc_grow(entries, old_bytes, descr, new_cap, f)?
            } else {
                self.heap_realloc(entries, old_bytes, bytes, entry_align, f)?
            };
            self.st(new_entries, self.fld(map_ty, self_p, M_ENTRIES, "entries"));
            self.st(new_cap, self.fld(map_ty, self_p, M_ENTRY_CAP, "entry_cap"));
            self.builder.build_br(have_space);
            self.builder.position_at_end(have_space);
            let e = self.ld(i64t, self.fld(map_ty, self_p, M_ENTRY_COUNT, "count"), "count");
            let e1 = self.builder.build_int_binop(IntBinOp::Add, e, self.v_i64(1), "e1");
            self.st(e1, self.fld(map_ty, self_p, M_ENTRY_COUNT, "entry_count"));
            let entries = self.ld(ptr, self.fld(map_ty, self_p, M_ENTRIES, "entries"), "entries");
            let entry = self.gep(entry_ty, entries, e, "entry");
            let key_value = self.map_key_value(key, k, key_slot)?;
            self.st(key_value, self.fld(entry_ty, entry, E_KEY, "key"));
            self.st(v, self.fld(entry_ty, entry, E_VALUE, "value"));
            self.st(self.v_i8(1), self.fld(entry_ty, entry, E_LIVE, "live"));
            // Bump `index_used` only when the slot was empty (not a tombstone).
            let index = self.ld(ptr, self.fld(map_ty, self_p, M_INDEX, "index"), "index");
            let slot_addr = self.gep(i32t, index, slot, "at");
            let holder = self.ld(i32t, slot_addr, "holder");
            let was_empty = self.builder.build_icmp(IntCmp::Eq, holder, self.v_i32(0), "wasempty");
            let bump = self.block(f, "insert.bump");
            let done = self.block(f, "insert.done");
            self.builder.build_cond_br(was_empty, bump, done);
            self.builder.position_at_end(bump);
            let used = self.ld(i64t, self.fld(map_ty, self_p, M_INDEX_USED, "used"), "used");
            let used1 = self.builder.build_int_binop(IntBinOp::Add, used, self.v_i64(1), "used1");
            self.st(used1, self.fld(map_ty, self_p, M_INDEX_USED, "index_used"));
            self.builder.build_br(done);
            self.builder.position_at_end(done);
            let index2 = self.ld(ptr, self.fld(map_ty, self_p, M_INDEX, "index"), "index");
            let slot_addr2 = self.gep(i32t, index2, slot, "at");
            let ep1 = self.builder.build_int_binop(IntBinOp::Add, e, self.v_i64(1), "ep1");
            let ep1_32 = self.builder.build_int_cast(false, false, ep1, i32t, "ep1_32");
            self.st(ep1_32, slot_addr2);
            let len2 = self.ld(i64t, self.fld(map_ty, self_p, M_LEN, "len"), "len");
            let len3 = self.builder.build_int_binop(IntBinOp::Add, len2, self.v_i64(1), "len1");
            self.st(len3, self.fld(map_ty, self_p, M_LEN, "len"));
            self.builder.build_ret_void();
        }

        // --- clear(self) ---
        {
            let (f, _) = self.start(&format!("{base}__clear"), "entry")?;
            let self_p = f.get_param(0);
            let i_slot = self.builder.build_alloca(i64t, "e");
            self.st(self.v_i64(0), i_slot);
            let header = self.loop_header(f, "clear.cond");
            let e = self.ld(i64t, i_slot, "e");
            let count = self.ld(i64t, self.fld(map_ty, self_p, M_ENTRY_COUNT, "count"), "count");
            let more = self.builder.build_icmp(IntCmp::Ult, e, count, "more");
            let body = self.block(f, "clear.body");
            let exit = self.block(f, "clear.end");
            self.builder.build_cond_br(more, body, exit);
            self.builder.position_at_end(body);
            let entries = self.ld(ptr, self.fld(map_ty, self_p, M_ENTRIES, "entries"), "entries");
            let entry = self.gep(entry_ty, entries, e, "entry");
            let live = self.ld(i8t, self.fld(entry_ty, entry, E_LIVE, "live"), "live");
            let dead = self.builder.build_icmp(IntCmp::Eq, live, self.v_i8(0), "dead");
            let skip = self.block(f, "clear.skip");
            let free_bb = self.block(f, "clear.free");
            self.builder.build_cond_br(dead, skip, free_bb);
            self.builder.position_at_end(free_bb);
            let value_slot = self.fld(entry_ty, entry, E_VALUE, "value");
            if value_drops {
                self.call_drop(value_slot, value)?;
            }
            self.map_key_drop(key, entry_ty, entry)?;
            self.st(self.v_i8(0), self.fld(entry_ty, entry, E_LIVE, "live"));
            self.builder.build_br(skip);
            self.builder.position_at_end(skip);
            let en = self.builder.build_int_binop(IntBinOp::Add, e, self.v_i64(1), "e1");
            self.st(en, i_slot);
            self.builder.build_br(header);
            self.builder.position_at_end(exit);
            self.st(self.v_i64(0), self.fld(map_ty, self_p, M_ENTRY_COUNT, "entry_count"));
            self.st(self.v_i64(0), self.fld(map_ty, self_p, M_LEN, "len"));
            self.st(self.v_i64(0), self.fld(map_ty, self_p, M_INDEX_USED, "index_used"));
            let index = self.ld(ptr, self.fld(map_ty, self_p, M_INDEX, "index"), "index");
            let noindex = self.builder.build_icmp(IntCmp::Eq, index, self.ctx.const_null(ptr), "noindex");
            let clear_index = self.block(f, "clear.index");
            let end = self.block(f, "clear.ret");
            self.builder.build_cond_br(noindex, end, clear_index);
            self.builder.position_at_end(clear_index);
            let icap = self.ld(i64t, self.fld(map_ty, self_p, M_INDEX_CAP, "icap"), "icap");
            let bytes = self.builder.build_int_binop(IntBinOp::Mul, icap, self.v_i64(4), "bytes");
            let (memset, sety) = self.mem_fn("memset")?;
            self.call(memset, sety, &[index, self.v_i32(0), bytes], "");
            self.builder.build_br(end);
            self.builder.position_at_end(end);
            self.builder.build_ret_void();
        }

        // --- len / is_empty ---
        {
            let (f, _) = self.start(&format!("{base}__len"), "entry")?;
            let n = self.ld(i64t, self.fld(map_ty, f.get_param(0), M_LEN, "len"), "len");
            self.builder.build_ret(n);
        }
        {
            let (f, _) = self.start(&format!("{base}__is_empty"), "entry")?;
            let n = self.ld(i64t, self.fld(map_ty, f.get_param(0), M_LEN, "len"), "len");
            let e = self.builder.build_icmp(IntCmp::Eq, n, self.v_i64(0), "empty");
            self.builder.build_ret(e);
        }

        // --- keys(self) ---
        {
            let (f, _) = self.start(&format!("{base}__keys"), "entry")?;
            let self_p = f.get_param(0);
            let out_slot = self.builder.build_alloca(list_key_ty, "out");
            let key_slot = self.builder.build_alloca(key_ty, "key.tmp");
            self.st(self.ctx.const_null(list_key_ty), out_slot);
            let len = self.ld(i64t, self.fld(map_ty, self_p, M_LEN, "len"), "len");
            let list_reserve = self.cfn(&format!("kelp_List__{}__reserve", ty_mangle(key)))?;
            self.call(list_reserve.0, list_reserve.1, &[out_slot, len], "");
            let i_slot = self.builder.build_alloca(i64t, "e");
            self.st(self.v_i64(0), i_slot);
            let header = self.loop_header(f, "keys.cond");
            let e = self.ld(i64t, i_slot, "e");
            let count = self.ld(i64t, self.fld(map_ty, self_p, M_ENTRY_COUNT, "count"), "count");
            let more = self.builder.build_icmp(IntCmp::Ult, e, count, "more");
            let body = self.block(f, "keys.body");
            let exit = self.block(f, "keys.end");
            self.builder.build_cond_br(more, body, exit);
            self.builder.position_at_end(body);
            let entries = self.ld(ptr, self.fld(map_ty, self_p, M_ENTRIES, "entries"), "entries");
            let entry = self.gep(entry_ty, entries, e, "entry");
            let live = self.ld(i8t, self.fld(entry_ty, entry, E_LIVE, "live"), "live");
            let dead = self.builder.build_icmp(IntCmp::Eq, live, self.v_i8(0), "dead");
            let skip = self.block(f, "keys.skip");
            let append = self.block(f, "keys.append");
            self.builder.build_cond_br(dead, skip, append);
            self.builder.position_at_end(append);
            let kv = self.map_key_clone(key, entry_ty, entry, key_slot)?;
            let olen = self.ld(i64t, self.fld(list_key_ty, out_slot, L_LEN, "olen"), "olen");
            let odata = self.ld(ptr, self.fld(list_key_ty, out_slot, L_DATA, "odata"), "odata");
            let dst = self.gep(key_ty, odata, olen, "dst");
            self.st(kv, dst);
            let olen1 = self.builder.build_int_binop(IntBinOp::Add, olen, self.v_i64(1), "olen1");
            self.st(olen1, self.fld(list_key_ty, out_slot, L_LEN, "len"));
            self.builder.build_br(skip);
            self.builder.position_at_end(skip);
            let en = self.builder.build_int_binop(IntBinOp::Add, e, self.v_i64(1), "e1");
            self.st(en, i_slot);
            self.builder.build_br(header);
            self.builder.position_at_end(exit);
            let out = self.ld(list_key_ty, out_slot, "keys");
            self.builder.build_ret(out);
        }

        // --- values(self) ---
        if clonable {
            let (f, _) = self.start(&format!("{base}__values"), "entry")?;
            let self_p = f.get_param(0);
            let out_slot = self.builder.build_alloca(list_value_ty, "out");
            let clone_slot = self.builder.build_alloca(value_ty, "value.tmp");
            self.st(self.ctx.const_null(list_value_ty), out_slot);
            let len = self.ld(i64t, self.fld(map_ty, self_p, M_LEN, "len"), "len");
            let list_reserve = self.cfn(&format!("kelp_List__{}__reserve", ty_mangle(value)))?;
            self.call(list_reserve.0, list_reserve.1, &[out_slot, len], "");
            let i_slot = self.builder.build_alloca(i64t, "e");
            self.st(self.v_i64(0), i_slot);
            let header = self.loop_header(f, "values.cond");
            let e = self.ld(i64t, i_slot, "e");
            let count = self.ld(i64t, self.fld(map_ty, self_p, M_ENTRY_COUNT, "count"), "count");
            let more = self.builder.build_icmp(IntCmp::Ult, e, count, "more");
            let body = self.block(f, "values.body");
            let exit = self.block(f, "values.end");
            self.builder.build_cond_br(more, body, exit);
            self.builder.position_at_end(body);
            let entries = self.ld(ptr, self.fld(map_ty, self_p, M_ENTRIES, "entries"), "entries");
            let entry = self.gep(entry_ty, entries, e, "entry");
            let live = self.ld(i8t, self.fld(entry_ty, entry, E_LIVE, "live"), "live");
            let dead = self.builder.build_icmp(IntCmp::Eq, live, self.v_i8(0), "dead");
            let skip = self.block(f, "values.skip");
            let append = self.block(f, "values.append");
            self.builder.build_cond_br(dead, skip, append);
            self.builder.position_at_end(append);
            let value_slot = self.fld(entry_ty, entry, E_VALUE, "value");
            let vv = self.clone_value(value_slot, value, clone_slot)?;
            let olen = self.ld(i64t, self.fld(list_value_ty, out_slot, L_LEN, "olen"), "olen");
            let odata = self.ld(ptr, self.fld(list_value_ty, out_slot, L_DATA, "odata"), "odata");
            let dst = self.gep(value_ty, odata, olen, "dst");
            self.st(vv, dst);
            let olen1 = self.builder.build_int_binop(IntBinOp::Add, olen, self.v_i64(1), "olen1");
            self.st(olen1, self.fld(list_value_ty, out_slot, L_LEN, "len"));
            self.builder.build_br(skip);
            self.builder.position_at_end(skip);
            let en = self.builder.build_int_binop(IntBinOp::Add, e, self.v_i64(1), "e1");
            self.st(en, i_slot);
            self.builder.build_br(header);
            self.builder.position_at_end(exit);
            let out = self.ld(list_value_ty, out_slot, "values");
            self.builder.build_ret(out);
        }

        // --- drop(self) ---
        {
            let (f, _) = self.start(
                &drop_fn_name(&Ty::Map(Box::new(key.clone()), Box::new(value.clone()))),
                "entry",
            )?;
            let self_p = f.get_param(0);
            let i_slot = self.builder.build_alloca(i64t, "e");
            self.st(self.v_i64(0), i_slot);
            let header = self.loop_header(f, "drop.cond");
            let e = self.ld(i64t, i_slot, "e");
            let count = self.ld(i64t, self.fld(map_ty, self_p, M_ENTRY_COUNT, "count"), "count");
            let more = self.builder.build_icmp(IntCmp::Ult, e, count, "more");
            let body = self.block(f, "drop.body");
            let exit = self.block(f, "drop.end");
            self.builder.build_cond_br(more, body, exit);
            self.builder.position_at_end(body);
            let entries = self.ld(ptr, self.fld(map_ty, self_p, M_ENTRIES, "entries"), "entries");
            let entry = self.gep(entry_ty, entries, e, "entry");
            let live = self.ld(i8t, self.fld(entry_ty, entry, E_LIVE, "live"), "live");
            let dead = self.builder.build_icmp(IntCmp::Eq, live, self.v_i8(0), "dead");
            let skip = self.block(f, "drop.skip");
            let free_bb = self.block(f, "drop.free");
            self.builder.build_cond_br(dead, skip, free_bb);
            self.builder.position_at_end(free_bb);
            let value_slot = self.fld(entry_ty, entry, E_VALUE, "value");
            if value_drops {
                self.call_drop(value_slot, value)?;
            }
            self.map_key_drop(key, entry_ty, entry)?;
            self.builder.build_br(skip);
            self.builder.position_at_end(skip);
            let en = self.builder.build_int_binop(IntBinOp::Add, e, self.v_i64(1), "e1");
            self.st(en, i_slot);
            self.builder.build_br(header);
            self.builder.position_at_end(exit);
            let rows = self.ld(i64t, self.fld(map_ty, self_p, M_ENTRY_CAP, "ecap"), "ecap");
            let entry_bytes = self.builder.build_int_binop(IntBinOp::Mul, rows, self.v_i64(entry_size), "entry.bytes");
            let icols = self.ld(i64t, self.fld(map_ty, self_p, M_INDEX_CAP, "icap"), "icap");
            let index_bytes = self.builder.build_int_binop(IntBinOp::Mul, icols, self.v_i64(4), "index.bytes");
            let entries = self.ld(ptr, self.fld(map_ty, self_p, M_ENTRIES, "entries"), "entries");
            let index = self.ld(ptr, self.fld(map_ty, self_p, M_INDEX, "index"), "index");
            self.heap_dealloc(entries, entry_bytes, entry_align)?;
            self.heap_dealloc(index, index_bytes, index_align)?;
            self.st(self.ctx.const_null(map_ty), self_p);
            self.builder.build_ret_void();
        }
        let _ = (value_ty, list_value_ty, list_key_ty, key_arg);
        Ok(())
    }
}
