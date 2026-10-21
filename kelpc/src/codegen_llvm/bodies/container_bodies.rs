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
}
