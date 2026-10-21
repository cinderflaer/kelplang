use super::*;

/// The per-element-type values the `List<T>` body emitters share.
pub(crate) struct ListShape<'a> {
    pub(crate) base: String,
    pub(crate) list_ty: Type<'a>,
    pub(crate) elem_ty: Type<'a>,
    pub(crate) elem_size: u64,
    pub(crate) elem_align: u64,
    pub(crate) ptr: Type<'a>,
    pub(crate) i1t: Type<'a>,
    pub(crate) i32t: Type<'a>,
    pub(crate) i64t: Type<'a>,
    pub(crate) drop_elem: bool,
    pub(crate) sortable: bool,
    pub(crate) clonable: bool,
    pub(crate) comparable: bool,
}

impl<'a> Gen<'a> {
    /// The values every `List<T>` body shares, computed once per element type.
    pub(crate) fn list_shape(&self, elem: &Ty) -> Result<ListShape<'a>, String> {
        let base = list_c_type(elem);
        let list_ty = self.cty(&Ty::List(Box::new(elem.clone())))?;
        let elem_ty = self.lty2(elem)?;
        let elem_size = self.layout.abi_size_of(elem_ty);
        let elem_align = self.layout.abi_alignment_of(elem_ty) as u64;
        let ptr = self.pty();
        let (i1t, i32t, i64t) = (self.ctx.i1(), self.ctx.i32(), self.ctx.i64());
        let drop_elem = needs_drop(elem, &self.owned);
        let sortable = crate::types::value_is_sortable(elem, &self.sortable);
        let clonable = crate::types::value_is_clonable(elem, &self.clonable);
        let comparable = crate::types::list_comparable_elem(elem);
        Ok(ListShape { base, list_ty, elem_ty, elem_size, elem_align, ptr, i1t, i32t, i64t, drop_elem, sortable, clonable, comparable })
    }

    /// One `List<T>` function set, the mirror of the earlier emitter's `write_list` and
    /// `write_list_s1`. The list is `{ data, len, cap }`, the element's size and
    /// drop decide the bodies, and every bounds check panics with the C
    /// backend's own text.
    pub(crate) fn emit_list_bodies(&self, elem: &Ty) -> Result<(), String> {
        let shape = self.list_shape(elem)?;
        self.emit_list_empty(elem, &shape)?;
        self.emit_list_reserve(elem, &shape)?;
        self.emit_list_with_capacity(elem, &shape)?;
        self.emit_list_of(elem, &shape)?;
        self.emit_list_len_is_empty(elem, &shape)?;
        self.emit_list_push(elem, &shape)?;
        self.emit_list_bounds(elem, &shape)?;
        self.emit_list_get(elem, &shape)?;
        self.emit_list_set(elem, &shape)?;
        self.emit_list_pop(elem, &shape)?;
        self.emit_list_clear(elem, &shape)?;
        self.emit_list_reverse(elem, &shape)?;
        self.emit_list_insert(elem, &shape)?;
        self.emit_list_remove(elem, &shape)?;
        self.emit_list_cmp_and_sort(elem, &shape)?;
        self.emit_list_contains(elem, &shape)?;
        self.emit_list_slice_and_clone(elem, &shape)?;
        self.emit_list_drop(elem, &shape)?;
        Ok(())
    }

    /// empty
    pub(crate) fn emit_list_empty(&self, _elem: &Ty, shape: &ListShape<'a>) -> Result<(), String> {
        let base = shape.base.as_str();
        let ListShape { list_ty, .. } = *shape;
        // --- empty ---
        {
            let _ = self.start(&format!("{base}__empty"), "entry")?;
            let l = self.builder.build_alloca(list_ty, "l");
            self.st(self.ctx.const_null(list_ty), l);
            let v = self.ld(list_ty, l, "ret");
            self.builder.build_ret(v);
        }
        Ok(())
    }

    /// reserve(self, want)
    pub(crate) fn emit_list_reserve(&self, elem: &Ty, shape: &ListShape<'a>) -> Result<(), String> {
        let base = shape.base.as_str();
        let ListShape { list_ty, elem_size, elem_align, ptr, i64t, .. } = *shape;
        // --- reserve(self, want) ---
        {
            let (f, _) = self.start(&format!("{base}__reserve"), "entry")?;
            let self_p = f.get_param(0);
            let want = f.get_param(1);
            let cap = self.ld(i64t, self.fld(list_ty, self_p, L_CAP, "cap"), "cap");
            let enough = self.builder.build_icmp(IntCmp::Ule, want, cap, "enough");
            let done = self.block(f, "ret");
            let grow = self.block(f, "grow");
            self.builder.build_cond_br(enough, done, grow);
            self.builder.position_at_end(grow);
            let cap_slot = self.builder.build_alloca(i64t, "cap");
            let zero = self.builder.build_icmp(IntCmp::Eq, cap, self.v_i64(0), "zero");
            let start = self.builder.build_select(zero, self.v_i64(4), cap, "start");
            self.st(start, cap_slot);
            let header = self.loop_header(f, "grow.cond");
            let c = self.ld(i64t, cap_slot, "cap");
            let need = self.builder.build_icmp(IntCmp::Ult, c, want, "need");
            let body = self.block(f, "grow.body");
            let exit = self.block(f, "grow.end");
            self.builder.build_cond_br(need, body, exit);
            self.builder.position_at_end(body);
            let c2 = self.ld(i64t, cap_slot, "cap");
            let doubled = self.builder.build_int_binop(IntBinOp::Mul, c2, self.v_i64(2), "cap2");
            self.st(doubled, cap_slot);
            self.builder.build_br(header);
            self.builder.position_at_end(exit);
            let final_cap = self.ld(i64t, cap_slot, "cap");
            let size = self
                .builder
                .build_int_binop(IntBinOp::Mul, final_cap, self.v_i64(elem_size), "bytes");
            let old_size = self
                .builder
                .build_int_binop(IntBinOp::Mul, cap, self.v_i64(elem_size), "old.bytes");
            let data = self.ld(ptr, self.fld(list_ty, self_p, L_DATA, "data"), "data");
            let buf = if self.managed {
                let descr = self.elem_descr(elem)?;
                self.gc_grow(data, old_size, descr, final_cap, f)?
            } else {
                self.heap_realloc(data, old_size, size, elem_align, f)?
            };
            self.st(buf, self.fld(list_ty, self_p, L_DATA, "data"));
            self.st(final_cap, self.fld(list_ty, self_p, L_CAP, "cap"));
            self.builder.build_br(done);
            self.builder.position_at_end(done);
            self.builder.build_ret_void();
        }
        Ok(())
    }

    /// with_capacity(n): `empty` then `reserve(n)`, so the buffer is
    pub(crate) fn emit_list_with_capacity(&self, _elem: &Ty, shape: &ListShape<'a>) -> Result<(), String> {
        let base = shape.base.as_str();
        let ListShape { list_ty, .. } = *shape;
        // --- with_capacity(n): `empty` then `reserve(n)`, so the buffer is
        // sized once and the list grows past it like any other ---
        {
            let (f, _) = self.start(&format!("{base}__with_capacity"), "entry")?;
            let l = self.builder.build_alloca(list_ty, "l");
            self.st(self.ctx.const_null(list_ty), l);
            let (reserve, rty) = self.cfn(&format!("{base}__reserve"))?;
            self.call(reserve, rty, &[l, f.get_param(0)], "");
            let out = self.ld(list_ty, l, "wc");
            self.builder.build_ret(out);
        }
        Ok(())
    }

    /// of(v)
    pub(crate) fn emit_list_of(&self, _elem: &Ty, shape: &ListShape<'a>) -> Result<(), String> {
        let base = shape.base.as_str();
        let ListShape { list_ty, elem_ty, ptr, .. } = *shape;
        // --- of(v) ---
        {
            let (f, _) = self.start(&format!("{base}__of"), "entry")?;
            let v = f.get_param(0);
            let l = self.builder.build_alloca(list_ty, "l");
            self.st(self.ctx.const_null(list_ty), l);
            let (reserve, rty) = self.cfn(&format!("{base}__reserve"))?;
            self.call(reserve, rty, &[l, self.v_i64(1)], "");
            let data = self.ld(ptr, self.fld(list_ty, l, L_DATA, "data"), "data");
            let addr = self.gep(elem_ty, data, self.v_i64(0), "at");
            self.st(v, addr);
            self.st(self.v_i64(1), self.fld(list_ty, l, L_LEN, "len"));
            let out = self.ld(list_ty, l, "of");
            self.builder.build_ret(out);
        }
        Ok(())
    }

    /// len / is_empty
    pub(crate) fn emit_list_len_is_empty(&self, _elem: &Ty, shape: &ListShape<'a>) -> Result<(), String> {
        let base = shape.base.as_str();
        let ListShape { list_ty, i64t, .. } = *shape;
        // --- len / is_empty ---
        {
            let (f, _) = self.start(&format!("{base}__len"), "entry")?;
            let n = self.ld(i64t, self.fld(list_ty, f.get_param(0), L_LEN, "len"), "len");
            self.builder.build_ret(n);
        }
        {
            let (f, _) = self.start(&format!("{base}__is_empty"), "entry")?;
            let n = self.ld(i64t, self.fld(list_ty, f.get_param(0), L_LEN, "len"), "len");
            let e = self.builder.build_icmp(IntCmp::Eq, n, self.v_i64(0), "empty");
            self.builder.build_ret(e);
        }
        Ok(())
    }

    /// push(self, v)
    pub(crate) fn emit_list_push(&self, _elem: &Ty, shape: &ListShape<'a>) -> Result<(), String> {
        let base = shape.base.as_str();
        let ListShape { list_ty, elem_ty, ptr, i64t, .. } = *shape;
        // --- push(self, v) ---
        {
            let (f, _) = self.start(&format!("{base}__push"), "entry")?;
            let self_p = f.get_param(0);
            let v = f.get_param(1);
            let len = self.ld(i64t, self.fld(list_ty, self_p, L_LEN, "len"), "len");
            let want = self.builder.build_int_binop(IntBinOp::Add, len, self.v_i64(1), "want");
            let (reserve, rty) = self.cfn(&format!("{base}__reserve"))?;
            self.call(reserve, rty, &[self_p, want], "");
            let n = self.ld(i64t, self.fld(list_ty, self_p, L_LEN, "len"), "len");
            let data = self.ld(ptr, self.fld(list_ty, self_p, L_DATA, "data"), "data");
            let addr = self.gep(elem_ty, data, n, "at");
            self.st(v, addr);
            let next = self.builder.build_int_binop(IntBinOp::Add, n, self.v_i64(1), "len1");
            self.st(next, self.fld(list_ty, self_p, L_LEN, "len"));
            self.builder.build_ret_void();
        }
        Ok(())
    }

    /// bounds(self, i)
    pub(crate) fn emit_list_bounds(&self, _elem: &Ty, shape: &ListShape<'a>) -> Result<(), String> {
        let base = shape.base.as_str();
        let ListShape { list_ty, i64t, .. } = *shape;
        // --- bounds(self, i) ---
        {
            let (f, _) = self.start(&format!("{base}__bounds"), "entry")?;
            let self_p = f.get_param(0);
            let i = f.get_param(1);
            let len = self.ld(i64t, self.fld(list_ty, self_p, L_LEN, "len"), "len");
            let bad = self.builder.build_icmp(IntCmp::Uge, i, len, "oob");
            let oob = self.block(f, "oob");
            let ok = self.block(f, "inbounds");
            self.builder.build_cond_br(bad, oob, ok);
            self.builder.position_at_end(oob);
            self.emit_panic("List.get: index out of bounds")?;
            self.builder.position_at_end(ok);
            self.builder.build_ret_void();
        }
        Ok(())
    }

    /// get / get_const
    pub(crate) fn emit_list_get(&self, elem: &Ty, shape: &ListShape<'a>) -> Result<(), String> {
        let base = shape.base.as_str();
        let ListShape { list_ty, elem_ty, ptr, .. } = *shape;
        // --- get / get_const ---
        if elem.is_copy() {
            let (f, _) = self.start(&format!("{base}__get"), "entry")?;
            let self_p = f.get_param(0);
            let i = f.get_param(1);
            let (bounds, bty) = self.cfn(&format!("{base}__bounds"))?;
            self.call(bounds, bty, &[self_p, i], "");
            let data = self.ld(ptr, self.fld(list_ty, self_p, L_DATA, "data"), "data");
            let addr = self.gep(elem_ty, data, i, "at");
            let v = self.ld(elem_ty, addr, "get");
            self.builder.build_ret(v);
        } else {
            let (f, _) = self.start(&format!("{base}__get_const"), "entry")?;
            let self_p = f.get_param(0);
            let i = f.get_param(1);
            let (bounds, bty) = self.cfn(&format!("{base}__bounds"))?;
            self.call(bounds, bty, &[self_p, i], "");
            let data = self.ld(ptr, self.fld(list_ty, self_p, L_DATA, "data"), "data");
            let addr = self.gep(elem_ty, data, i, "at");
            self.builder.build_ret(addr);
        }
        Ok(())
    }

    /// set(self, i, v)
    pub(crate) fn emit_list_set(&self, elem: &Ty, shape: &ListShape<'a>) -> Result<(), String> {
        let base = shape.base.as_str();
        let ListShape { list_ty, elem_ty, ptr, drop_elem, .. } = *shape;
        // --- set(self, i, v) ---
        {
            let (f, _) = self.start(&format!("{base}__set"), "entry")?;
            let self_p = f.get_param(0);
            let i = f.get_param(1);
            let v = f.get_param(2);
            let (bounds, bty) = self.cfn(&format!("{base}__bounds"))?;
            self.call(bounds, bty, &[self_p, i], "");
            let data = self.ld(ptr, self.fld(list_ty, self_p, L_DATA, "data"), "data");
            let addr = self.gep(elem_ty, data, i, "at");
            if drop_elem {
                self.call_drop(addr, elem)?;
            }
            self.st(v, addr);
            self.builder.build_ret_void();
        }
        Ok(())
    }

    /// pop(self)
    pub(crate) fn emit_list_pop(&self, _elem: &Ty, shape: &ListShape<'a>) -> Result<(), String> {
        let base = shape.base.as_str();
        let ListShape { list_ty, elem_ty, ptr, i64t, .. } = *shape;
        // --- pop(self) ---
        {
            let (f, _) = self.start(&format!("{base}__pop"), "entry")?;
            let self_p = f.get_param(0);
            let len = self.ld(i64t, self.fld(list_ty, self_p, L_LEN, "len"), "len");
            let empty = self.builder.build_icmp(IntCmp::Eq, len, self.v_i64(0), "empty");
            let bad = self.block(f, "empty");
            let ok = self.block(f, "pop");
            self.builder.build_cond_br(empty, bad, ok);
            self.builder.position_at_end(bad);
            self.emit_panic("List.pop: the list is empty")?;
            self.builder.position_at_end(ok);
            let last = self.builder.build_int_binop(IntBinOp::Sub, len, self.v_i64(1), "last");
            self.st(last, self.fld(list_ty, self_p, L_LEN, "len"));
            let data = self.ld(ptr, self.fld(list_ty, self_p, L_DATA, "data"), "data");
            let addr = self.gep(elem_ty, data, last, "at");
            let v = self.ld(elem_ty, addr, "pop");
            self.builder.build_ret(v);
        }
        Ok(())
    }

    /// clear(self)
    pub(crate) fn emit_list_clear(&self, elem: &Ty, shape: &ListShape<'a>) -> Result<(), String> {
        let base = shape.base.as_str();
        let ListShape { list_ty, elem_ty, ptr, i64t, drop_elem, .. } = *shape;
        // --- clear(self) ---
        {
            let (f, _) = self.start(&format!("{base}__clear"), "entry")?;
            let self_p = f.get_param(0);
            if drop_elem {
                let i_slot = self.builder.build_alloca(i64t, "i");
                self.st(self.v_i64(0), i_slot);
                let header = self.loop_header(f, "clear.cond");
                let i = self.ld(i64t, i_slot, "i");
                let len = self.ld(i64t, self.fld(list_ty, self_p, L_LEN, "len"), "len");
                let more = self.builder.build_icmp(IntCmp::Ult, i, len, "more");
                let body = self.block(f, "clear.body");
                let exit = self.block(f, "clear.end");
                self.builder.build_cond_br(more, body, exit);
                self.builder.position_at_end(body);
                let data = self.ld(ptr, self.fld(list_ty, self_p, L_DATA, "data"), "data");
                let addr = self.gep(elem_ty, data, i, "at");
                self.call_drop(addr, elem)?;
                let next = self.builder.build_int_binop(IntBinOp::Add, i, self.v_i64(1), "i1");
                self.st(next, i_slot);
                self.builder.build_br(header);
                self.builder.position_at_end(exit);
            }
            self.st(self.v_i64(0), self.fld(list_ty, self_p, L_LEN, "len"));
            self.builder.build_ret_void();
        }
        Ok(())
    }

    /// reverse(self)
    pub(crate) fn emit_list_reverse(&self, _elem: &Ty, shape: &ListShape<'a>) -> Result<(), String> {
        let base = shape.base.as_str();
        let ListShape { list_ty, elem_ty, ptr, i64t, .. } = *shape;
        // --- reverse(self) ---
        {
            let (f, _) = self.start(&format!("{base}__reverse"), "entry")?;
            let self_p = f.get_param(0);
            let i_slot = self.builder.build_alloca(i64t, "i");
            let j_slot = self.builder.build_alloca(i64t, "j");
            self.st(self.v_i64(0), i_slot);
            let len = self.ld(i64t, self.fld(list_ty, self_p, L_LEN, "len"), "len");
            self.st(len, j_slot);
            let header = self.loop_header(f, "rev.cond");
            let i = self.ld(i64t, i_slot, "i");
            let j = self.ld(i64t, j_slot, "j");
            let i1 = self.builder.build_int_binop(IntBinOp::Add, i, self.v_i64(1), "i1");
            let more = self.builder.build_icmp(IntCmp::Ult, i1, j, "more");
            let body = self.block(f, "rev.body");
            let exit = self.block(f, "rev.end");
            self.builder.build_cond_br(more, body, exit);
            self.builder.position_at_end(body);
            let jm = self.builder.build_int_binop(IntBinOp::Sub, j, self.v_i64(1), "j1");
            self.st(jm, j_slot);
            let data = self.ld(ptr, self.fld(list_ty, self_p, L_DATA, "data"), "data");
            let ai = self.gep(elem_ty, data, i, "ai");
            let aj = self.gep(elem_ty, data, jm, "aj");
            let tv = self.ld(elem_ty, ai, "t");
            let u = self.ld(elem_ty, aj, "u");
            self.st(u, ai);
            self.st(tv, aj);
            let inx = self.builder.build_int_binop(IntBinOp::Add, i, self.v_i64(1), "i1");
            self.st(inx, i_slot);
            self.builder.build_br(header);
            self.builder.position_at_end(exit);
            self.builder.build_ret_void();
        }
        Ok(())
    }

    /// insert(self, i, v)
    pub(crate) fn emit_list_insert(&self, _elem: &Ty, shape: &ListShape<'a>) -> Result<(), String> {
        let base = shape.base.as_str();
        let ListShape { list_ty, elem_ty, ptr, i64t, .. } = *shape;
        // --- insert(self, i, v) ---
        {
            let (f, _) = self.start(&format!("{base}__insert"), "entry")?;
            let self_p = f.get_param(0);
            let i = f.get_param(1);
            let v = f.get_param(2);
            let len = self.ld(i64t, self.fld(list_ty, self_p, L_LEN, "len"), "len");
            let bad = self.builder.build_icmp(IntCmp::Ugt, i, len, "oob");
            let oob = self.block(f, "oob");
            let ok = self.block(f, "insert");
            self.builder.build_cond_br(bad, oob, ok);
            self.builder.position_at_end(oob);
            self.emit_panic("List.insert: index out of bounds")?;
            self.builder.position_at_end(ok);
            let want = self.builder.build_int_binop(IntBinOp::Add, len, self.v_i64(1), "want");
            let (reserve, rty) = self.cfn(&format!("{base}__reserve"))?;
            self.call(reserve, rty, &[self_p, want], "");
            let data = self.ld(ptr, self.fld(list_ty, self_p, L_DATA, "data"), "data");
            let j_slot = self.builder.build_alloca(i64t, "j");
            self.st(len, j_slot);
            let header = self.loop_header(f, "shift.cond");
            let j = self.ld(i64t, j_slot, "j");
            let more = self.builder.build_icmp(IntCmp::Ugt, j, i, "more");
            let body = self.block(f, "shift.body");
            let exit = self.block(f, "shift.end");
            self.builder.build_cond_br(more, body, exit);
            self.builder.position_at_end(body);
            let jm = self.builder.build_int_binop(IntBinOp::Sub, j, self.v_i64(1), "j1");
            let src = self.gep(elem_ty, data, jm, "src");
            let dst = self.gep(elem_ty, data, j, "dst");
            let sv = self.ld(elem_ty, src, "sv");
            self.st(sv, dst);
            self.st(jm, j_slot);
            self.builder.build_br(header);
            self.builder.position_at_end(exit);
            let at = self.gep(elem_ty, data, i, "at");
            self.st(v, at);
            let len1 = self.builder.build_int_binop(IntBinOp::Add, len, self.v_i64(1), "len1");
            self.st(len1, self.fld(list_ty, self_p, L_LEN, "len"));
            self.builder.build_ret_void();
        }
        Ok(())
    }

    /// remove(self, i)
    pub(crate) fn emit_list_remove(&self, _elem: &Ty, shape: &ListShape<'a>) -> Result<(), String> {
        let base = shape.base.as_str();
        let ListShape { list_ty, elem_ty, ptr, i64t, .. } = *shape;
        // --- remove(self, i) ---
        {
            let (f, _) = self.start(&format!("{base}__remove"), "entry")?;
            let self_p = f.get_param(0);
            let i = f.get_param(1);
            let len = self.ld(i64t, self.fld(list_ty, self_p, L_LEN, "len"), "len");
            let bad = self.builder.build_icmp(IntCmp::Uge, i, len, "oob");
            let oob = self.block(f, "oob");
            let ok = self.block(f, "remove");
            self.builder.build_cond_br(bad, oob, ok);
            self.builder.position_at_end(oob);
            self.emit_panic("List.remove: index out of bounds")?;
            self.builder.position_at_end(ok);
            let data = self.ld(ptr, self.fld(list_ty, self_p, L_DATA, "data"), "data");
            let out_slot = self.builder.build_alloca(elem_ty, "out");
            let at = self.gep(elem_ty, data, i, "at");
            self.st(self.ld(elem_ty, at, "v"), out_slot);
            let j_slot = self.builder.build_alloca(i64t, "j");
            self.st(i, j_slot);
            let header = self.loop_header(f, "shift.cond");
            let j = self.ld(i64t, j_slot, "j");
            let j1 = self.builder.build_int_binop(IntBinOp::Add, j, self.v_i64(1), "j1");
            let more = self.builder.build_icmp(IntCmp::Ult, j1, len, "more");
            let body = self.block(f, "shift.body");
            let exit = self.block(f, "shift.end");
            self.builder.build_cond_br(more, body, exit);
            self.builder.position_at_end(body);
            let src = self.gep(elem_ty, data, j1, "src");
            let dst = self.gep(elem_ty, data, j, "dst");
            let sv = self.ld(elem_ty, src, "sv");
            self.st(sv, dst);
            self.st(j1, j_slot);
            self.builder.build_br(header);
            self.builder.position_at_end(exit);
            let len1 = self.builder.build_int_binop(IntBinOp::Sub, len, self.v_i64(1), "len1");
            self.st(len1, self.fld(list_ty, self_p, L_LEN, "len"));
            let out = self.ld(elem_ty, out_slot, "removed");
            self.builder.build_ret(out);
        }
        Ok(())
    }
}
