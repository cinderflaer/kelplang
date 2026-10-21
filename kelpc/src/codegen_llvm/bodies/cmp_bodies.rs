use super::*;

impl<'a> Gen<'a> {
    pub(crate) fn emit_clone_cmp_bodies(&self) -> Result<(), String> {
        let ptr = self.pty();
        let _ = ptr;
        for s in &self.program.structs {
            let fields: Vec<(String, Ty)> =
                s.fields.iter().map(|f| (f.name.clone(), f.ty.clone())).collect();
            self.emit_record_clone(&s.name, &fields)?;
            self.emit_record_cmp(&s.name, &fields)?;
            self.emit_record_hash(&s.name, &fields)?;
            self.emit_record_eq(&s.name, &fields)?;
        }
        for e in &self.program.enums {
            self.emit_enum_clone(e)?;
            self.emit_enum_cmp(e)?;
            self.emit_enum_hash(e)?;
            self.emit_enum_eq(e)?;
        }
        Ok(())
    }

    pub(crate) fn emit_record_clone(&self, name: &str, fields: &[(String, Ty)]) -> Result<(), String> {
        let Some((func, _)) = self.clone_fns.get(name).copied() else {
            return Ok(());
        };
        let entry = func.append_block("entry");
        self.builder.position_at_end(entry);
        let struct_ty = *self
            .structs
            .get(name)
            .ok_or_else(|| no_value(&format!("the datac '{name}'")))?;
        let src = func.get_param(0);
        let dst = func.get_param(1);
        for (i, (_, ty)) in fields.iter().enumerate() {
            let s = self.fld(struct_ty, src, i as u32, "src");
            let d = self.fld(struct_ty, dst, i as u32, "dst");
            self.clone_into(s, d, ty)?;
        }
        self.builder.build_ret_void();
        Ok(())
    }

    pub(crate) fn emit_record_cmp(&self, name: &str, fields: &[(String, Ty)]) -> Result<(), String> {
        let Some((func, _)) = self.cmp_fns.get(name).copied() else {
            return Ok(());
        };
        let entry = func.append_block("entry");
        self.builder.position_at_end(entry);
        let struct_ty = *self
            .structs
            .get(name)
            .ok_or_else(|| no_value(&format!("the datac '{name}'")))?;
        let a = func.get_param(0);
        let b = func.get_param(1);
        for (i, (_, ty)) in fields.iter().enumerate() {
            let ap = self.fld(struct_ty, a, i as u32, "a");
            let bp = self.fld(struct_ty, b, i as u32, "b");
            let cont = self.emit_field_cmp(func, ap, bp, ty)?;
            self.builder.position_at_end(cont);
        }
        self.builder.build_ret(self.v_i32(0));
        Ok(())
    }

    /// One field's contribution to a derived order, returning the block the walk
    /// continues in: the first differing field decides, exactly as
    /// `write_field_cmp` returns early on the first difference.
    pub(crate) fn emit_field_cmp(
        &self,
        func: LlvmFunction<'a>,
        ap: Value<'a>,
        bp: Value<'a>,
        ty: &Ty,
    ) -> Result<BasicBlock<'a>, String> {
        let ret_ty = self.lty2(ty)?;
        match ty {
            Ty::String | Ty::Bytes => {
                let (data_o, len_o) = (self.pty(), self.ctx.i64());
                let alp = self.fld(ret_ty, ap, 1, "alen");
                let blp = self.fld(ret_ty, bp, 1, "blen");
                let alen = self.ld(len_o, alp, "alen");
                let blen = self.ld(len_o, blp, "blen");
                let min = self.builder.build_select(
                    self.builder.build_icmp(IntCmp::Ult, alen, blen, "lt"),
                    alen,
                    blen,
                    "n",
                );
                // min > 0 ? memcmp(ap.data, bp.data, min) : 0
                let nonzero = self.builder.build_icmp(IntCmp::Ugt, min, self.v_i64(0), "nz");
                let cmp_bb = func.append_block("cmp.bytes");
                let after_cmp_bb = func.append_block("cmp.after");
                self.builder.build_cond_br(nonzero, cmp_bb, after_cmp_bb);
                self.builder.position_at_end(cmp_bb);
                let adata = self.ld(data_o, self.fld(ret_ty, ap, 0, "ad"), "ad");
                let bdata = self.ld(data_o, self.fld(ret_ty, bp, 0, "bd"), "bd");
                let (memcmp, memcmp_ty) = self.libc_fn("memcmp")?;
                let c = self.call(memcmp, memcmp_ty, &[adata, bdata, min], "memcmp");
                let ne = self.builder.build_icmp(IntCmp::Ne, c, self.v_i32(0), "ne");
                let diff_bb = func.append_block("cmp.diff");
                let keep_bb = func.append_block("cmp.keep");
                self.builder.build_cond_br(ne, diff_bb, keep_bb);
                self.builder.position_at_end(diff_bb);
                let sign = self.builder.build_select(
                    self.builder.build_icmp(IntCmp::Slt, c, self.v_i32(0), "neg"),
                    self.v_i32(u64::MAX),
                    self.v_i32(1),
                    "sign",
                );
                self.builder.build_ret(sign);
                self.builder.position_at_end(keep_bb);
                self.builder.build_br(after_cmp_bb);
                self.builder.position_at_end(after_cmp_bb);
                // Equal prefixes: the shorter buffer orders first.
                let lengths_differ =
                    self.builder.build_icmp(IntCmp::Ne, alen, blen, "lendiff");
                let len_bb = func.append_block("cmp.len");
                let cont = func.append_block("cmp.cont");
                self.builder.build_cond_br(lengths_differ, len_bb, cont);
                self.builder.position_at_end(len_bb);
                let lsign = self.builder.build_select(
                    self.builder.build_icmp(IntCmp::Ult, alen, blen, "alenlt"),
                    self.v_i32(u64::MAX),
                    self.v_i32(1),
                    "lsign",
                );
                self.builder.build_ret(lsign);
                self.builder.position_at_end(cont);
                Ok(cont)
            }
            Ty::Struct(name) => {
                let (func_c, ty_c) = self
                    .cmp_fns
                    .get(name)
                    .copied()
                    .ok_or_else(|| no_value(&format!("the order of '{name}'")))?;
                let c = self.call(func_c, ty_c, &[ap, bp], "cmp");
                let ne = self.builder.build_icmp(IntCmp::Ne, c, self.v_i32(0), "ne");
                let diff = func.append_block("cmp.diff");
                let cont = func.append_block("cmp.cont");
                self.builder.build_cond_br(ne, diff, cont);
                self.builder.position_at_end(diff);
                self.builder.build_ret(c);
                self.builder.position_at_end(cont);
                Ok(cont)
            }
            _ => {
                let av = self.ld(ret_ty, ap, "av");
                let bv = self.ld(ret_ty, bp, "bv");
                // C's `a != b` / `a < b` are `fcmp` for a float field, which is
                // exactly the `<`/`!=` `write_field_cmp` writes down.
                let (ne, lt) = if ty.is_float() {
                    (
                        self.builder.build_fcmp(RealCmp::One, av, bv, "ne"),
                        self.builder.build_fcmp(RealCmp::Olt, av, bv, "lt"),
                    )
                } else {
                    let pred = if is_signed(ty) { IntCmp::Slt } else { IntCmp::Ult };
                    (
                        self.builder.build_icmp(IntCmp::Ne, av, bv, "ne"),
                        self.builder.build_icmp(pred, av, bv, "lt"),
                    )
                };
                let diff = func.append_block("cmp.diff");
                let cont = func.append_block("cmp.cont");
                self.builder.build_cond_br(ne, diff, cont);
                self.builder.position_at_end(diff);
                let sign = self.builder.build_select(lt, self.v_i32(u64::MAX), self.v_i32(1), "sign");
                self.builder.build_ret(sign);
                self.builder.position_at_end(cont);
                Ok(cont)
            }
        }
    }

    pub(crate) fn emit_enum_clone(&self, e: &crate::types::EnumDef) -> Result<(), String> {
        let Some((func, _)) = self.clone_fns.get(&e.name).copied() else {
            return Ok(());
        };
        let info = self
            .enums
            .get(&e.name)
            .cloned()
            .ok_or_else(|| no_value(&format!("the enum '{}'", e.name)))?;
        let entry = func.append_block("entry");
        self.builder.position_at_end(entry);
        let src = func.get_param(0);
        let dst = func.get_param(1);
        let tag_ptr = self.fld(info.llvm_ty, src, ENUM_TAG_FIELD, "tag");
        let tag = self.ld(info.tag_ty, tag_ptr, "tag");
        self.st(tag, self.fld(info.llvm_ty, dst, ENUM_TAG_FIELD, "tag"));
        let mut cont = entry;
        for (i, v) in e.variants.iter().enumerate() {
            if v.is_payload_less() {
                continue;
            }
            self.builder.position_at_end(cont);
            let is_i = self.builder.build_icmp(
                IntCmp::Eq,
                tag,
                self.ctx.const_int(info.tag_ty, i as u64, false),
                "isvar",
            );
            let then_bb = func.append_block("clone.v");
            let next = func.append_block("clone.next");
            self.builder.build_cond_br(is_i, then_bb, next);
            self.builder.position_at_end(then_bb);
            let src_union = self.fld(info.llvm_ty, src, ENUM_UNION_FIELD, "kelp_v");
            let dst_union = self.fld(info.llvm_ty, dst, ENUM_UNION_FIELD, "kelp_v");
            for (fi, field) in v.real_fields().enumerate() {
                let s = self.fld(
                    info.variants[i].payload_ty,
                    src_union,
                    fi as u32,
                    "f",
                );
                let d = self.fld(
                    info.variants[i].payload_ty,
                    dst_union,
                    fi as u32,
                    "f",
                );
                self.clone_into(s, d, &field.ty)?;
            }
            self.builder.build_br(next);
            cont = next;
        }
        self.builder.position_at_end(cont);
        self.builder.build_ret_void();
        Ok(())
    }

    pub(crate) fn emit_enum_cmp(&self, e: &crate::types::EnumDef) -> Result<(), String> {
        let Some((func, _)) = self.cmp_fns.get(&e.name).copied() else {
            return Ok(());
        };
        let info = self
            .enums
            .get(&e.name)
            .cloned()
            .ok_or_else(|| no_value(&format!("the enum '{}'", e.name)))?;
        let entry = func.append_block("entry");
        self.builder.position_at_end(entry);
        let a = func.get_param(0);
        let b = func.get_param(1);
        let ta = self.ld(
            info.tag_ty,
            self.fld(info.llvm_ty, a, ENUM_TAG_FIELD, "tag"),
            "ta",
        );
        let tb = self.ld(
            info.tag_ty,
            self.fld(info.llvm_ty, b, ENUM_TAG_FIELD, "tag"),
            "tb",
        );
        // A different tag orders first, so every value of one variant sorts
        // together. The comparison is the tag type's own signedness, which is
        // what the C `a->kelp_tag < b->kelp_tag` compiles to.
        let ne = self.builder.build_icmp(IntCmp::Ne, ta, tb, "tagdiff");
        let diff = func.append_block("cmp.tag");
        let cont0 = func.append_block("cmp.variants");
        self.builder.build_cond_br(ne, diff, cont0);
        self.builder.position_at_end(diff);
        let lt = self.builder.build_icmp(
            if info.tag_signed {
                IntCmp::Slt
            } else {
                IntCmp::Ult
            },
            ta,
            tb,
            "lt",
        );
        let sign = self.builder.build_select(lt, self.v_i32(u64::MAX), self.v_i32(1), "sign");
        self.builder.build_ret(sign);

        let mut cont = cont0;
        for (i, v) in e.variants.iter().enumerate() {
            if v.is_payload_less() {
                continue;
            }
            self.builder.position_at_end(cont);
            let is_i = self
                .builder
                .build_icmp(IntCmp::Eq, ta, self.v_i32(i as u64), "isvar");
            let then_bb = func.append_block("cmp.v");
            let next = func.append_block("cmp.next");
            self.builder.build_cond_br(is_i, then_bb, next);
            self.builder.position_at_end(then_bb);
            let a_union = self.fld(info.llvm_ty, a, ENUM_UNION_FIELD, "kelp_v");
            let b_union = self.fld(info.llvm_ty, b, ENUM_UNION_FIELD, "kelp_v");
            let mut inner = then_bb;
            for (fi, field) in v.real_fields().enumerate() {
                self.builder.position_at_end(inner);
                let ap = self.fld(info.variants[i].payload_ty, a_union, fi as u32, "f");
                let bp = self.fld(info.variants[i].payload_ty, b_union, fi as u32, "f");
                inner = self.emit_field_cmp(func, ap, bp, &field.ty)?;
            }
            self.builder.position_at_end(inner);
            self.builder.build_br(next);
            cont = next;
        }
        self.builder.position_at_end(cont);
        self.builder.build_ret(self.v_i32(0));
        Ok(())
    }

    // ---- the derived hash and key equality of a datac/enum (A-6) -------

    /// `kelp_map_mix`'s three-instruction 64-bit finalizer
    /// (`x ^= x >> 33; x *= 0xff51afd7ed558ccd; x ^= x >> 33;`), hand-built
    /// as inline IR rather than a call into `kelp_rt.o`: that object is a
    /// separate compilation unit from the code generated here, so LLVM's
    /// optimizer can never see into `kelp_map_mix`'s body and inline it, no
    /// matter how small it is. The optimizer already inlines the same three
    /// instructions when they sit in the same file, so this reaches parity by building the same
    /// instructions here instead. Matches the C reference operation for
    /// operation, including the unsigned (logical) right shift both times,
    /// since `x` is `uint64_t` there.
    pub(crate) fn emit_map_mix(&self, x: Value<'a>) -> Value<'a> {
        let s1 = self
            .builder
            .build_int_binop(IntBinOp::LShr, x, self.v_i64(33), "mix.s1");
        let x1 = self.builder.build_int_binop(IntBinOp::Xor, x, s1, "mix.x1");
        let x2 = self.builder.build_int_binop(
            IntBinOp::Mul,
            x1,
            self.v_i64(0xff51afd7ed558ccd),
            "mix.x2",
        );
        let s2 = self
            .builder
            .build_int_binop(IntBinOp::LShr, x2, self.v_i64(33), "mix.s2");
        self.builder.build_int_binop(IntBinOp::Xor, x2, s2, "mix.x3")
    }

    /// `kelp_map_hash`'s FNV-1a loop over `data[0..len]`, hand-built as
    /// inline IR in `cur_fn` for the same reason `emit_map_mix` is: a call
    /// into `kelp_rt.o` is opaque to LLVM's optimizer, a splice into the
    /// same file (what the runtime's own copy gets) is not. Built in
    /// this file's usual byte-scanning-loop shape (an `alloca`-based index,
    /// `loop_header`, `gep`/`ld` per byte, `build_cond_br`), the same shape
    /// the capability name fast paths and `rehash`'s own walk above use.
    /// Matches the C reference operation for operation: `h` starts at the
    /// FNV offset basis, then per byte, xors in the byte and multiplies by
    /// the FNV prime. The per-byte load is zero-extended, never
    /// sign-extended, matching the reference's `(unsigned char)data[i]`
    /// widening, so a byte >= 0x80 hashes the same on both backends.
    pub(crate) fn emit_map_hash(&self, cur_fn: LlvmFunction<'a>, data: Value<'a>, len: Value<'a>) -> Value<'a> {
        let i8t = self.ctx.i8();
        let i64t = self.ctx.i64();
        let h_slot = self.builder.build_alloca(i64t, "maphash.h");
        self.st(self.v_i64(1469598103934665603), h_slot);
        let idx = self.builder.build_alloca(i64t, "maphash.idx");
        self.st(self.v_i64(0), idx);
        let cond = self.loop_header(cur_fn, "maphash.cond");
        let i = self.ld(i64t, idx, "maphash.i");
        let more = self
            .builder
            .build_icmp(IntCmp::Ult, i, len, "maphash.more");
        let body = self.block(cur_fn, "maphash.body");
        let after = self.block(cur_fn, "maphash.after");
        self.builder.build_cond_br(more, body, after);
        self.builder.position_at_end(body);
        let cptr = self.gep(i8t, data, i, "maphash.cptr");
        let c = self.ld(i8t, cptr, "maphash.c");
        let cw = self.builder.build_int_cast(false, true, c, i64t, "maphash.cw");
        let h = self.ld(i64t, h_slot, "maphash.h1");
        let hx = self.builder.build_int_binop(IntBinOp::Xor, h, cw, "maphash.hx");
        let hm = self.builder.build_int_binop(
            IntBinOp::Mul,
            hx,
            self.v_i64(1099511628211),
            "maphash.hm",
        );
        self.st(hm, h_slot);
        let next = self
            .builder
            .build_int_binop(IntBinOp::Add, i, self.v_i64(1), "maphash.next");
        self.st(next, idx);
        self.builder.build_br(cond);
        self.builder.position_at_end(after);
        self.ld(i64t, h_slot, "maphash.result")
    }

    /// Fold one already-hashed field or tag into a hash in progress, the way
    /// `kelp_map_combine` does: `h ^= kelp_map_mix(x); h *= 1099511628211;`.
    /// Reuses `emit_map_mix` rather than duplicating its body a second time,
    /// hand-built inline for the same reason `emit_map_mix` itself is.
    pub(crate) fn hash_combine(&self, h: Value<'a>, x: Value<'a>) -> Result<Value<'a>, String> {
        let mixed = self.emit_map_mix(x);
        let h1 = self.builder.build_int_binop(IntBinOp::Xor, h, mixed, "combine.h1");
        Ok(self.builder.build_int_binop(
            IntBinOp::Mul,
            h1,
            self.v_i64(1099511628211),
            "combine.h2",
        ))
    }

    /// One field's own hash before it is folded in: a `String` field's byte
    /// hash, a scalar's mix, and a nested keyable `datac`/`enum`'s own hash.
    /// The mirror of the earlier emitter's `write_field_hash`. `cur_fn` is the
    /// enclosing hash function being built, threaded through only for the
    /// `String` case's `emit_map_hash` loop, which needs a function to
    /// append its blocks to.
    pub(crate) fn emit_field_hash(
        &self,
        cur_fn: LlvmFunction<'a>,
        ty: &Ty,
        ptr: Value<'a>,
    ) -> Result<Value<'a>, String> {
        match ty {
            Ty::String => {
                let ret_ty = self.lty2(ty)?;
                let data = self.ld(self.pty(), self.fld(ret_ty, ptr, 0, "d"), "d");
                let len = self.ld(self.ctx.i64(), self.fld(ret_ty, ptr, 1, "n"), "n");
                Ok(self.emit_map_hash(cur_fn, data, len))
            }
            Ty::Struct(name) => {
                let (func, hty) = self
                    .hash_fns
                    .get(name)
                    .copied()
                    .ok_or_else(|| no_value(&format!("the hash of '{name}'")))?;
                Ok(self.call(func, hty, &[ptr], "hash"))
            }
            _ => {
                let v = self.ld(self.lty2(ty)?, ptr, "v");
                let wide = self.key_to_u64(ty, v)?;
                Ok(self.emit_map_mix(wide))
            }
        }
    }

    /// One field's equality in a derived key equality. A `String` field reuses
    /// the runtime's stored-key comparison, so the byte-length-aware rule is
    /// the one the `String` key path already uses. The mirror of
    /// the earlier emitter's `write_field_eq`.
    pub(crate) fn emit_field_eq(
        &self,
        ty: &Ty,
        ap: Value<'a>,
        bp: Value<'a>,
    ) -> Result<Value<'a>, String> {
        match ty {
            Ty::String => {
                let ret_ty = self.lty2(ty)?;
                let data = self.ld(self.pty(), self.fld(ret_ty, bp, 0, "d"), "d");
                let len = self.ld(self.ctx.i64(), self.fld(ret_ty, bp, 1, "n"), "n");
                let rt = self.rt("kelp_map_key_eq")?;
                Ok(self.call(rt.func, rt.ty, &[ap, data, len], "eq"))
            }
            Ty::Struct(name) => {
                let (func, ety) = self
                    .eq_fns
                    .get(name)
                    .copied()
                    .ok_or_else(|| no_value(&format!("the equality of '{name}'")))?;
                Ok(self.call(func, ety, &[ap, bp], "eq"))
            }
            _ => {
                let ret_ty = self.lty2(ty)?;
                let av = self.ld(ret_ty, ap, "av");
                let bv = self.ld(ret_ty, bp, "bv");
                Ok(self.builder.build_icmp(IntCmp::Eq, av, bv, "eq"))
            }
        }
    }

    pub(crate) fn emit_record_hash(&self, name: &str, fields: &[(String, Ty)]) -> Result<(), String> {
        let Some((func, _)) = self.hash_fns.get(name).copied() else {
            return Ok(());
        };
        let entry = func.append_block("entry");
        self.builder.position_at_end(entry);
        let struct_ty = *self
            .structs
            .get(name)
            .ok_or_else(|| no_value(&format!("the datac '{name}'")))?;
        let v = func.get_param(0);
        let h_slot = self.builder.build_alloca(self.ctx.i64(), "h");
        self.st(self.v_i64(0), h_slot);
        for (i, (_, ty)) in fields.iter().enumerate() {
            let fp = self.fld(struct_ty, v, i as u32, "f");
            let field_hash = self.emit_field_hash(func, ty, fp)?;
            let h = self.ld(self.ctx.i64(), h_slot, "h");
            let folded = self.hash_combine(h, field_hash)?;
            self.st(folded, h_slot);
        }
        let h = self.ld(self.ctx.i64(), h_slot, "h");
        self.builder.build_ret(h);
        Ok(())
    }

    pub(crate) fn emit_record_eq(&self, name: &str, fields: &[(String, Ty)]) -> Result<(), String> {
        let Some((func, _)) = self.eq_fns.get(name).copied() else {
            return Ok(());
        };
        let entry = func.append_block("entry");
        self.builder.position_at_end(entry);
        let struct_ty = *self
            .structs
            .get(name)
            .ok_or_else(|| no_value(&format!("the datac '{name}'")))?;
        let a = func.get_param(0);
        let b = func.get_param(1);
        let mut acc = self.ctx.const_bool(true);
        for (i, (_, ty)) in fields.iter().enumerate() {
            let ap = self.fld(struct_ty, a, i as u32, "a");
            let bp = self.fld(struct_ty, b, i as u32, "b");
            let eq = self.emit_field_eq(ty, ap, bp)?;
            acc = self.builder.build_int_binop(IntBinOp::And, acc, eq, "acc");
        }
        self.builder.build_ret(acc);
        Ok(())
    }

    pub(crate) fn emit_enum_hash(&self, e: &crate::types::EnumDef) -> Result<(), String> {
        let Some((func, _)) = self.hash_fns.get(&e.name).copied() else {
            return Ok(());
        };
        let info = self
            .enums
            .get(&e.name)
            .cloned()
            .ok_or_else(|| no_value(&format!("the enum '{}'", e.name)))?;
        let entry = func.append_block("entry");
        self.builder.position_at_end(entry);
        let v = func.get_param(0);
        let tag = self.ld(
            info.tag_ty,
            self.fld(info.llvm_ty, v, ENUM_TAG_FIELD, "tag"),
            "tag",
        );
        let h_slot = self.builder.build_alloca(self.ctx.i64(), "h");
        // The tag folds first, so two variants never share a hash of the same
        // payload shape; the earlier emitter writes the same fold from its tag. The
        // widening extension follows the tag's signedness, which is what the
        // C `(uint64_t)` cast of a signed or unsigned tag does.
        let wide = self
            .builder
            .build_int_cast(info.tag_signed, true, tag, self.ctx.i64(), "tagw");
        let tag_hash = self.emit_map_mix(wide);
        let h0 = self.hash_combine(self.v_i64(0), tag_hash)?;
        self.st(h0, h_slot);
        let mut cont = entry;
        for (i, variant) in e.variants.iter().enumerate() {
            if variant.is_payload_less() {
                continue;
            }
            self.builder.position_at_end(cont);
            let is_i = self.builder.build_icmp(
                IntCmp::Eq,
                tag,
                self.ctx.const_int(info.tag_ty, i as u64, false),
                "isvar",
            );
            let then_bb = func.append_block("hash.v");
            let next = func.append_block("hash.next");
            self.builder.build_cond_br(is_i, then_bb, next);
            self.builder.position_at_end(then_bb);
            let union = self.fld(info.llvm_ty, v, ENUM_UNION_FIELD, "kelp_v");
            for (fi, field) in variant.real_fields().enumerate() {
                let fp = self.fld(info.variants[i].payload_ty, union, fi as u32, "f");
                let field_hash = self.emit_field_hash(func, &field.ty, fp)?;
                let h = self.ld(self.ctx.i64(), h_slot, "h");
                let folded = self.hash_combine(h, field_hash)?;
                self.st(folded, h_slot);
            }
            self.builder.build_br(next);
            cont = next;
        }
        self.builder.position_at_end(cont);
        let h = self.ld(self.ctx.i64(), h_slot, "h");
        self.builder.build_ret(h);
        Ok(())
    }

    pub(crate) fn emit_enum_eq(&self, e: &crate::types::EnumDef) -> Result<(), String> {
        let Some((func, _)) = self.eq_fns.get(&e.name).copied() else {
            return Ok(());
        };
        let info = self
            .enums
            .get(&e.name)
            .cloned()
            .ok_or_else(|| no_value(&format!("the enum '{}'", e.name)))?;
        let entry = func.append_block("entry");
        self.builder.position_at_end(entry);
        let false_bb = func.append_block("eq.false");
        let a = func.get_param(0);
        let b = func.get_param(1);
        let ta = self.ld(
            info.tag_ty,
            self.fld(info.llvm_ty, a, ENUM_TAG_FIELD, "tag"),
            "ta",
        );
        let tb = self.ld(
            info.tag_ty,
            self.fld(info.llvm_ty, b, ENUM_TAG_FIELD, "tag"),
            "tb",
        );
        // A different tag is unequal at once, the same rule the earlier emitter's
        // `a->kelp_tag != b->kelp_tag` writes.
        let same_tag = self.builder.build_icmp(IntCmp::Eq, ta, tb, "sametag");
        let cont0 = func.append_block("eq.variants");
        self.builder.build_cond_br(same_tag, cont0, false_bb);
        let mut cont = cont0;
        for (i, variant) in e.variants.iter().enumerate() {
            if variant.is_payload_less() {
                continue;
            }
            self.builder.position_at_end(cont);
            let is_i = self.builder.build_icmp(
                IntCmp::Eq,
                ta,
                self.ctx.const_int(info.tag_ty, i as u64, false),
                "isvar",
            );
            let then_bb = func.append_block("eq.v");
            let next = func.append_block("eq.next");
            self.builder.build_cond_br(is_i, then_bb, next);
            self.builder.position_at_end(then_bb);
            let a_union = self.fld(info.llvm_ty, a, ENUM_UNION_FIELD, "kelp_v");
            let b_union = self.fld(info.llvm_ty, b, ENUM_UNION_FIELD, "kelp_v");
            let mut acc = self.ctx.const_bool(true);
            for (fi, field) in variant.real_fields().enumerate() {
                let ap = self.fld(info.variants[i].payload_ty, a_union, fi as u32, "f");
                let bp = self.fld(info.variants[i].payload_ty, b_union, fi as u32, "f");
                let eq = self.emit_field_eq(&field.ty, ap, bp)?;
                acc = self.builder.build_int_binop(IntBinOp::And, acc, eq, "acc");
            }
            let failed = self.builder.build_icmp(IntCmp::Eq, acc, self.ctx.const_bool(false), "ne");
            self.builder.build_cond_br(failed, false_bb, next);
            cont = next;
        }
        self.builder.position_at_end(cont);
        self.builder.build_ret(self.ctx.const_bool(true));
        self.builder.position_at_end(false_bb);
        self.builder.build_ret(self.ctx.const_bool(false));
        Ok(())
    }
}
