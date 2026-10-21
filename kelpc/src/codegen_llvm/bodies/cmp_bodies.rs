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

    /// One `List<T>` function set, the mirror of the earlier emitter's `write_list` and
    /// `write_list_s1`. The list is `{ data, len, cap }`, the element's size and
    /// drop decide the bodies, and every bounds check panics with the C
    /// backend's own text.
    pub(crate) fn emit_list_bodies(&self, elem: &Ty) -> Result<(), String> {
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

        // --- empty ---
        {
            let _ = self.start(&format!("{base}__empty"), "entry")?;
            let l = self.builder.build_alloca(list_ty, "l");
            self.st(self.ctx.const_null(list_ty), l);
            let v = self.ld(list_ty, l, "ret");
            self.builder.build_ret(v);
        }

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

        // --- cmp(a, b) and sort(self) ---
        if sortable {
            let (f, _) = self.start(&format!("{base}__cmp"), "entry")?;
            let a = f.get_param(0);
            let b = f.get_param(1);
            let av = self.ld(elem_ty, a, "a");
            let bv = self.ld(elem_ty, b, "b");
            let out = match elem {
                Ty::Bool => {
                    let ax = self.builder.build_int_cast(false, true, av, i32t, "ax");
                    let bx = self.builder.build_int_cast(false, true, bv, i32t, "bx");
                    self.builder.build_int_binop(IntBinOp::Sub, ax, bx, "cmp")
                }
                Ty::String | Ty::Bytes => {
                    let alen = self.ld(i64t, self.fld(elem_ty, a, 1, "alen"), "alen");
                    let blen = self.ld(i64t, self.fld(elem_ty, b, 1, "blen"), "blen");
                    let min = self.builder.build_select(
                        self.builder.build_icmp(IntCmp::Ult, alen, blen, "lt"),
                        alen,
                        blen,
                        "n",
                    );
                    let adata = self.ld(ptr, self.fld(elem_ty, a, 0, "ad"), "ad");
                    let bdata = self.ld(ptr, self.fld(elem_ty, b, 0, "bd"), "bd");
                    let (memcmp, mty) = self.libc_fn("memcmp")?;
                    let c = self.call(memcmp, mty, &[adata, bdata, min], "memcmp");
                    let c1 = self.builder.build_select(
                        self.builder.build_icmp(IntCmp::Slt, c, self.v_i32(0), "neg"),
                        self.v_i32(u64::MAX),
                        self.v_i32(1),
                        "c1",
                    );
                    let npos = self.builder.build_icmp(IntCmp::Ugt, min, self.v_i64(0), "npos");
                    let cnonzero = self.builder.build_icmp(IntCmp::Ne, c, self.v_i32(0), "cnz");
                    let taken = self
                        .builder
                        .build_int_binop(IntBinOp::And, npos, cnonzero, "taken");
                    let llt = self.builder.build_icmp(IntCmp::Ult, alen, blen, "llt");
                    let lgt = self.builder.build_icmp(IntCmp::Ugt, alen, blen, "lgt");
                    let len_cmp = self.builder.build_select(
                        llt,
                        self.v_i32(u64::MAX),
                        self.builder.build_select(lgt, self.v_i32(1), self.v_i32(0), "l0"),
                        "lc",
                    );
                    self.builder.build_select(taken, c1, len_cmp, "cmp")
                }
                Ty::Struct(name) => {
                    let (cmp, cty2) = self
                        .cmp_fns
                        .get(name)
                        .copied()
                        .ok_or_else(|| no_value(&format!("the order of '{name}'")))?;
                    self.call(cmp, cty2, &[a, b], "cmp")
                }
                _ => {
                    // `(*a > *b) - (*a < *b)`, with `fcmp` for a float element
                    // (a `List<f64>` sorts, and C's `>`/`<` on a double are
                    // `fcmp ogt`/`olt`).
                    let (gt, lt) = if elem.is_float() {
                        (
                            self.builder.build_fcmp(RealCmp::Ogt, av, bv, "gt"),
                            self.builder.build_fcmp(RealCmp::Olt, av, bv, "lt"),
                        )
                    } else {
                        let pred_gt = if is_signed(elem) { IntCmp::Sgt } else { IntCmp::Ugt };
                        let pred_lt = if is_signed(elem) { IntCmp::Slt } else { IntCmp::Ult };
                        (
                            self.builder.build_icmp(pred_gt, av, bv, "gt"),
                            self.builder.build_icmp(pred_lt, av, bv, "lt"),
                        )
                    };
                    let g = self.builder.build_int_cast(false, true, gt, i32t, "g");
                    let l = self.builder.build_int_cast(false, true, lt, i32t, "l");
                    self.builder.build_int_binop(IntBinOp::Sub, g, l, "cmp")
                }
            };
            self.builder.build_ret(out);

            // One sift-down for a max-heap: heapify and heapsort below both
            // use it. `cmp` returns > 0 when its first argument is greater,
            // so the larger child is the one with cmp(child, sibling) > 0 and
            // a swap happens only while cmp(child, root) > 0. `base` is the
            // offset of the heap's own index 0 within `self`'s data, so the
            // same sift-down serves both a full-list heapsort and the
            // depth-limited introsort fallback's heapsort over one partition.
            let (f, _) = self.start(&format!("{base}__sift_down"), "entry")?;
            let self_p = f.get_param(0);
            let base_off = f.get_param(1);
            let heap_len = f.get_param(3);
            let root_slot = self.builder.build_alloca(i64t, "root");
            self.st(f.get_param(2), root_slot);
            let data = self.ld(ptr, self.fld(list_ty, self_p, L_DATA, "data"), "data");
            let (cmpsym, cmpty) = self.cfn(&format!("{base}__cmp"))?;
            let sift = self.loop_header(f, "sift.cond");
            let root = self.ld(i64t, root_slot, "root");
            let twice = self.builder.build_int_binop(IntBinOp::Mul, root, self.v_i64(2), "twice");
            let child = self.builder.build_int_binop(IntBinOp::Add, twice, self.v_i64(1), "child");
            let in_heap = self.builder.build_icmp(IntCmp::Ult, child, heap_len, "in_heap");
            let pick = self.block(f, "sift.pick");
            let compare = self.block(f, "sift.compare");
            let exit = self.block(f, "sift.end");
            self.builder.build_cond_br(in_heap, pick, exit);
            self.builder.position_at_end(pick);
            let child1 = self.builder.build_int_binop(IntBinOp::Add, child, self.v_i64(1), "child1");
            let has_right = self.builder.build_icmp(IntCmp::Ult, child1, heap_len, "has_right");
            let cmp_child = self.block(f, "sift.cmpchild");
            let after_pick = self.block(f, "sift.afterpick");
            self.builder.build_cond_br(has_right, cmp_child, after_pick);
            self.builder.position_at_end(cmp_child);
            let child_abs = self.builder.build_int_binop(IntBinOp::Add, base_off, child, "child_abs");
            let child1_abs = self.builder.build_int_binop(IntBinOp::Add, base_off, child1, "child1_abs");
            let lptr = self.gep(elem_ty, data, child_abs, "l");
            let rptr = self.gep(elem_ty, data, child1_abs, "r");
            let c = self.call(cmpsym, cmpty, &[lptr, rptr], "cmp");
            let right_bigger = self.builder.build_icmp(IntCmp::Slt, c, self.v_i32(0), "rb");
            let bigger = self.builder.build_select(right_bigger, child1, child, "bigger");
            self.builder.build_br(after_pick);
            self.builder.position_at_end(after_pick);
            let chosen = self.builder.build_phi(i64t, "chosen");
            self.builder.add_incoming(chosen, &[(child, pick), (bigger, cmp_child)]);
            self.builder.build_br(compare);
            self.builder.position_at_end(compare);
            let root_abs = self.builder.build_int_binop(IntBinOp::Add, base_off, root, "root_abs");
            let chosen_abs = self.builder.build_int_binop(IntBinOp::Add, base_off, chosen, "chosen_abs");
            let rootp = self.gep(elem_ty, data, root_abs, "rootp");
            let childp = self.gep(elem_ty, data, chosen_abs, "childp");
            let gt = self.call(cmpsym, cmpty, &[childp, rootp], "cmp");
            let child_greater = self.builder.build_icmp(IntCmp::Sgt, gt, self.v_i32(0), "gt");
            let swap = self.block(f, "sift.swap");
            self.builder.build_cond_br(child_greater, swap, exit);
            self.builder.position_at_end(swap);
            let pv = self.ld(elem_ty, rootp, "pv");
            let cv = self.ld(elem_ty, childp, "cv");
            self.st(cv, rootp);
            self.st(pv, childp);
            self.st(chosen, root_slot);
            self.builder.build_br(sift);
            self.builder.position_at_end(exit);
            self.builder.build_ret_void();

            // heapsort over the half-open range `[lo, hi)`: guaranteed
            // O(n log n), in place, no extra buffer. Used both as introsort's
            // depth-limit fallback (over a partition) and, with
            // `lo = 0, hi = len`, as an ordinary whole-list heapsort.
            let (f, _) = self.start(&format!("{base}__heapsort_range"), "entry")?;
            let self_p = f.get_param(0);
            let lo = f.get_param(1);
            let hi = f.get_param(2);
            let n = self.builder.build_int_binop(IntBinOp::Sub, hi, lo, "n");
            let data = self.ld(ptr, self.fld(list_ty, self_p, L_DATA, "data"), "data");
            let (sift, sfty) = self.cfn(&format!("{base}__sift_down"))?;
            let start_slot = self.builder.build_alloca(i64t, "start");
            let half = self.builder.build_int_binop(IntBinOp::UDiv, n, self.v_i64(2), "half");
            self.st(half, start_slot);
            let heap = self.loop_header(f, "hr.heap.cond");
            let s = self.ld(i64t, start_slot, "s");
            let spos = self.builder.build_icmp(IntCmp::Ugt, s, self.v_i64(0), "spos");
            let hbody = self.block(f, "hr.heap.body");
            let hexit = self.block(f, "hr.heap.end");
            self.builder.build_cond_br(spos, hbody, hexit);
            self.builder.position_at_end(hbody);
            let s1 = self.builder.build_int_binop(IntBinOp::Sub, s, self.v_i64(1), "s1");
            self.st(s1, start_slot);
            self.call(sift, sfty, &[self_p, lo, s1, n], "");
            self.builder.build_br(heap);
            self.builder.position_at_end(hexit);
            let end_slot = self.builder.build_alloca(i64t, "end");
            self.st(n, end_slot);
            let sort = self.loop_header(f, "hr.sort.cond");
            let e = self.ld(i64t, end_slot, "e");
            let epos = self.builder.build_icmp(IntCmp::Ugt, e, self.v_i64(1), "epos");
            let sbody = self.block(f, "hr.sort.body");
            let sexit = self.block(f, "hr.sort.end");
            self.builder.build_cond_br(epos, sbody, sexit);
            self.builder.position_at_end(sbody);
            let e1 = self.builder.build_int_binop(IntBinOp::Sub, e, self.v_i64(1), "e1");
            self.st(e1, end_slot);
            let lo_e1 = self.builder.build_int_binop(IntBinOp::Add, lo, e1, "lo_e1");
            let d0 = self.gep(elem_ty, data, lo, "d0");
            let de = self.gep(elem_ty, data, lo_e1, "de");
            let tv = self.ld(elem_ty, d0, "tv");
            let ev = self.ld(elem_ty, de, "ev");
            self.st(ev, d0);
            self.st(tv, de);
            self.call(sift, sfty, &[self_p, lo, self.v_i64(0), e1], "");
            self.builder.build_br(sort);
            self.builder.position_at_end(sexit);
            self.builder.build_ret_void();

            // Insertion sort over `[lo, hi)`: lower constant factor than
            // quicksort for a small partition, so introsort finishes any
            // partition below the cutoff with this instead of recursing
            // further.
            let (f, _) = self.start(&format!("{base}__insertion_sort"), "entry")?;
            let self_p = f.get_param(0);
            let lo = f.get_param(1);
            let hi = f.get_param(2);
            let data = self.ld(ptr, self.fld(list_ty, self_p, L_DATA, "data"), "data");
            let (cmpsym, cmpty) = self.cfn(&format!("{base}__cmp"))?;
            let i_slot = self.builder.build_alloca(i64t, "i");
            let lo1 = self.builder.build_int_binop(IntBinOp::Add, lo, self.v_i64(1), "lo1");
            self.st(lo1, i_slot);
            let outer = self.loop_header(f, "ins.cond");
            let i = self.ld(i64t, i_slot, "i");
            let more = self.builder.build_icmp(IntCmp::Ult, i, hi, "more");
            let obody = self.block(f, "ins.body");
            let oend = self.block(f, "ins.end");
            self.builder.build_cond_br(more, obody, oend);
            self.builder.position_at_end(obody);
            let at_i = self.gep(elem_ty, data, i, "at_i");
            let key_slot = self.builder.build_alloca(elem_ty, "key");
            self.st(self.ld(elem_ty, at_i, "keyv"), key_slot);
            let j_slot = self.builder.build_alloca(i64t, "j");
            self.st(i, j_slot);
            let inner = self.loop_header(f, "ins.shift.cond");
            let j = self.ld(i64t, j_slot, "j");
            let jgt = self.builder.build_icmp(IntCmp::Ugt, j, lo, "jgt");
            let check = self.block(f, "ins.shift.check");
            let iend = self.block(f, "ins.shift.end");
            self.builder.build_cond_br(jgt, check, iend);
            self.builder.position_at_end(check);
            let j1 = self.builder.build_int_binop(IntBinOp::Sub, j, self.v_i64(1), "j1");
            let prevp = self.gep(elem_ty, data, j1, "prevp");
            let c = self.call(cmpsym, cmpty, &[prevp, key_slot], "cmp");
            let gtc = self.builder.build_icmp(IntCmp::Sgt, c, self.v_i32(0), "gtc");
            let shift = self.block(f, "ins.shift.body");
            self.builder.build_cond_br(gtc, shift, iend);
            self.builder.position_at_end(shift);
            let prevv = self.ld(elem_ty, prevp, "prevv");
            let atj = self.gep(elem_ty, data, j, "atj");
            self.st(prevv, atj);
            self.st(j1, j_slot);
            self.builder.build_br(inner);
            self.builder.position_at_end(iend);
            let atj_final = self.gep(elem_ty, data, j, "atj_final");
            let keyv = self.ld(elem_ty, key_slot, "keyv_final");
            self.st(keyv, atj_final);
            let i1 = self.builder.build_int_binop(IntBinOp::Add, i, self.v_i64(1), "i1");
            self.st(i1, i_slot);
            self.builder.build_br(outer);
            self.builder.position_at_end(oend);
            self.builder.build_ret_void();

            // The index (among `a`, `b`, `c`) whose element is the median of
            // the three, the standard pivot choice that defeats quicksort's
            // classic already-sorted/reverse-sorted worst cases without
            // anything fancier.
            let (f, _) = self.start(&format!("{base}__median3"), "entry")?;
            let self_p = f.get_param(0);
            let a = f.get_param(1);
            let b = f.get_param(2);
            let c_idx = f.get_param(3);
            let data = self.ld(ptr, self.fld(list_ty, self_p, L_DATA, "data"), "data");
            let (cmpsym, cmpty) = self.cfn(&format!("{base}__cmp"))?;
            let at_a = self.gep(elem_ty, data, a, "at_a");
            let at_b = self.gep(elem_ty, data, b, "at_b");
            let at_c = self.gep(elem_ty, data, c_idx, "at_c");
            let cab = self.call(cmpsym, cmpty, &[at_a, at_b], "cab");
            let ab_lt = self.builder.build_icmp(IntCmp::Slt, cab, self.v_i32(0), "ab_lt");
            let ablk = self.block(f, "m3.ablt");
            let eblk = self.block(f, "m3.age");
            self.builder.build_cond_br(ab_lt, ablk, eblk);

            self.builder.position_at_end(ablk);
            let cbc = self.call(cmpsym, cmpty, &[at_b, at_c], "cbc");
            let bc_lt = self.builder.build_icmp(IntCmp::Slt, cbc, self.v_i32(0), "bc_lt");
            let retb1 = self.block(f, "m3.retb1");
            let chkac1 = self.block(f, "m3.chkac1");
            self.builder.build_cond_br(bc_lt, retb1, chkac1);
            self.builder.position_at_end(retb1);
            self.builder.build_ret(b);
            self.builder.position_at_end(chkac1);
            let cac1 = self.call(cmpsym, cmpty, &[at_a, at_c], "cac1");
            let ac_lt1 = self.builder.build_icmp(IntCmp::Slt, cac1, self.v_i32(0), "ac_lt1");
            let retc1 = self.block(f, "m3.retc1");
            let reta1 = self.block(f, "m3.reta1");
            self.builder.build_cond_br(ac_lt1, retc1, reta1);
            self.builder.position_at_end(retc1);
            self.builder.build_ret(c_idx);
            self.builder.position_at_end(reta1);
            self.builder.build_ret(a);

            self.builder.position_at_end(eblk);
            let cac2 = self.call(cmpsym, cmpty, &[at_a, at_c], "cac2");
            let ac_lt2 = self.builder.build_icmp(IntCmp::Slt, cac2, self.v_i32(0), "ac_lt2");
            let reta2 = self.block(f, "m3.reta2");
            let chkbc2 = self.block(f, "m3.chkbc2");
            self.builder.build_cond_br(ac_lt2, reta2, chkbc2);
            self.builder.position_at_end(reta2);
            self.builder.build_ret(a);
            self.builder.position_at_end(chkbc2);
            let cbc2 = self.call(cmpsym, cmpty, &[at_b, at_c], "cbc2");
            let bc_lt2 = self.builder.build_icmp(IntCmp::Slt, cbc2, self.v_i32(0), "bc_lt2");
            let retc2 = self.block(f, "m3.retc2");
            let retb2 = self.block(f, "m3.retb2");
            self.builder.build_cond_br(bc_lt2, retc2, retb2);
            self.builder.position_at_end(retc2);
            self.builder.build_ret(c_idx);
            self.builder.position_at_end(retb2);
            self.builder.build_ret(b);

            // Partition over the inclusive range `[lo, hi]` around a
            // median-of-three pivot, returning the split point `j`:
            // everything in `[lo, j]` compares <= everything in
            // `[j + 1, hi]`. This hand-builds pdqsort/BlockQuicksort's
            // branchless partition as LLVM IR, not a plain Hoare scan (see
            // the earlier emitter's `{base}__partition`, and `KELP_SORT_BLOCK`'s
            // doc comment above, for the full explanation of the technique
            // and why it helps on random data): while at least two full
            // blocks remain between the cursors `l` and `rr`, each round
            // scans one block from each side and records, into the
            // `offsets_l`/`offsets_r` stack buffers, the in-block offsets of
            // elements on the wrong side of the pivot. That scan is the
            // branch-free part: `wrong64 = zext(icmp sge/sle ...)` folds the
            // comparison's outcome into an integer added to a running count
            // instead of a conditional branch, so there is no per-element
            // control-flow decision for the branch predictor to get wrong on
            // random data. Only `num = min(num_l, num_r)` pairs are swapped
            // each round; whichever side has leftover offsets keeps its
            // block active and those offsets carry over (via `start_l`/
            // `start_r`) to be matched against next round's fresh block from
            // the other side, exactly as pdqsort's own reference
            // implementation does.
            //
            // Once fewer than `2 * KELP_SORT_BLOCK` elements remain between
            // `l` and `rr`, the block loop stops and the tail is finished
            // with the exact same branchy Hoare scan the previous, block-
            // free version of this function used end to end (starting from
            // `l`/`rr` rather than `lo`/`hi`): any offsets collected above
            // but not yet swapped are still sitting exactly where they
            // started inside `[l, rr)` (a swap only ever happens for a
            // matched pair; an unmatched offset is pure bookkeeping, never
            // acted on), so `[l, rr)` is still plain, valid, unpartitioned
            // data and the tail scan re-derives its own classification from
            // scratch. This wastes at most `O(KELP_SORT_BLOCK)` redundant
            // comparisons per partition call, negligible next to the
            // elements handled branch-free above. The pivot value is copied
            // into a local once (a plain element copy, never cloned or
            // dropped, exactly as every swap here), so it stays valid as a
            // sentinel no matter how the elements around it move, and
            // comparing against that copy rather than tracking the pivot's
            // index is what keeps an all-equal-keys input from degrading:
            // every scan, block or tail, stops as soon as it reaches a key
            // equal to the pivot rather than running to the end.
            let (f, _) = self.start(&format!("{base}__partition"), "entry")?;
            let self_p = f.get_param(0);
            let lo = f.get_param(1);
            let hi = f.get_param(2);
            let data = self.ld(ptr, self.fld(list_ty, self_p, L_DATA, "data"), "data");
            let (cmpsym, cmpty) = self.cfn(&format!("{base}__cmp"))?;
            let (median3, m3ty) = self.cfn(&format!("{base}__median3"))?;
            let span = self.builder.build_int_binop(IntBinOp::Sub, hi, lo, "span");
            let half = self.builder.build_int_binop(IntBinOp::UDiv, span, self.v_i64(2), "half");
            let mid = self.builder.build_int_binop(IntBinOp::Add, lo, half, "mid");
            let pividx = self.call(median3, m3ty, &[self_p, lo, mid, hi], "pividx");
            let pivptr = self.gep(elem_ty, data, pividx, "pivptr");
            let pivot_slot = self.builder.build_alloca(elem_ty, "pivot");
            self.st(self.ld(elem_ty, pivptr, "pv"), pivot_slot);

            let l_slot = self.builder.build_alloca(i64t, "l");
            self.st(lo, l_slot);
            let rr_slot = self.builder.build_alloca(i64t, "rr");
            let hi_p1 = self.builder.build_int_binop(IntBinOp::Add, hi, self.v_i64(1), "hi_p1");
            self.st(hi_p1, rr_slot);
            let offsets_arr_ty = self.ctx.array_type(i64t, KELP_SORT_BLOCK);
            let offsets_l = self.builder.build_alloca(offsets_arr_ty, "offsets_l");
            let offsets_r = self.builder.build_alloca(offsets_arr_ty, "offsets_r");
            let num_l_slot = self.builder.build_alloca(i64t, "num_l");
            self.st(self.v_i64(0), num_l_slot);
            let num_r_slot = self.builder.build_alloca(i64t, "num_r");
            self.st(self.v_i64(0), num_r_slot);
            let start_l_slot = self.builder.build_alloca(i64t, "start_l");
            self.st(self.v_i64(0), start_l_slot);
            let start_r_slot = self.builder.build_alloca(i64t, "start_r");
            self.st(self.v_i64(0), start_r_slot);

            let blk_cond = self.loop_header(f, "part.blk.cond");
            let l = self.ld(i64t, l_slot, "l");
            let rr = self.ld(i64t, rr_slot, "rr");
            let blk_width = self.builder.build_int_binop(IntBinOp::Sub, rr, l, "blk.width");
            let has_room = self.builder.build_icmp(
                IntCmp::Ugt,
                blk_width,
                self.v_i64(2 * KELP_SORT_BLOCK),
                "has_room",
            );
            let blk_body = self.block(f, "part.blk.body");
            let blk_done = self.block(f, "part.blk.done");
            self.builder.build_cond_br(has_room, blk_body, blk_done);
            self.builder.position_at_end(blk_body);
            let rbase = self.builder.build_int_binop(
                IntBinOp::Sub,
                rr,
                self.v_i64(KELP_SORT_BLOCK),
                "rbase",
            );

            // if (num_l == 0): refill offsets_l from the block [l, l+BLOCK).
            let num_l0 = self.ld(i64t, num_l_slot, "num_l0");
            let num_l0_zero = self.builder.build_icmp(IntCmp::Eq, num_l0, self.v_i64(0), "num_l0_zero");
            let fill_l = self.block(f, "part.fill_l");
            let after_fill_l = self.block(f, "part.after_fill_l");
            self.builder.build_cond_br(num_l0_zero, fill_l, after_fill_l);
            self.builder.position_at_end(fill_l);
            self.st(self.v_i64(0), start_l_slot);
            self.st(self.v_i64(0), num_l_slot);
            let fl_i_slot = self.builder.build_alloca(i64t, "fl_i");
            self.st(self.v_i64(0), fl_i_slot);
            let fl_cond = self.loop_header(f, "part.fl.cond");
            let fl_i = self.ld(i64t, fl_i_slot, "fl_i.v");
            let fl_more = self.builder.build_icmp(IntCmp::Ult, fl_i, self.v_i64(KELP_SORT_BLOCK), "fl_more");
            let fl_body = self.block(f, "part.fl.body");
            let fl_done = self.block(f, "part.fl.done");
            self.builder.build_cond_br(fl_more, fl_body, fl_done);
            self.builder.position_at_end(fl_body);
            let fl_numl = self.ld(i64t, num_l_slot, "fl_numl");
            let fl_off_ptr = self.gep(i64t, offsets_l, fl_numl, "fl_off_ptr");
            self.st(fl_i, fl_off_ptr);
            let fl_idx = self.builder.build_int_binop(IntBinOp::Add, l, fl_i, "fl_idx");
            let fl_elem_ptr = self.gep(elem_ty, data, fl_idx, "fl_elem_ptr");
            let fl_cmp = self.call(cmpsym, cmpty, &[fl_elem_ptr, pivot_slot], "fl_cmp");
            let fl_wrong = self.builder.build_icmp(IntCmp::Sge, fl_cmp, self.v_i32(0), "fl_wrong");
            let fl_wrong64 = self.builder.build_int_cast(false, true, fl_wrong, i64t, "fl_wrong64");
            let fl_numl1 = self.builder.build_int_binop(IntBinOp::Add, fl_numl, fl_wrong64, "fl_numl1");
            self.st(fl_numl1, num_l_slot);
            let fl_i1 = self.builder.build_int_binop(IntBinOp::Add, fl_i, self.v_i64(1), "fl_i1");
            self.st(fl_i1, fl_i_slot);
            self.builder.build_br(fl_cond);
            self.builder.position_at_end(fl_done);
            self.builder.build_br(after_fill_l);
            self.builder.position_at_end(after_fill_l);

            // if (num_r == 0): refill offsets_r from the block [rbase, rr).
            let num_r0 = self.ld(i64t, num_r_slot, "num_r0");
            let num_r0_zero = self.builder.build_icmp(IntCmp::Eq, num_r0, self.v_i64(0), "num_r0_zero");
            let fill_r = self.block(f, "part.fill_r");
            let after_fill_r = self.block(f, "part.after_fill_r");
            self.builder.build_cond_br(num_r0_zero, fill_r, after_fill_r);
            self.builder.position_at_end(fill_r);
            self.st(self.v_i64(0), start_r_slot);
            self.st(self.v_i64(0), num_r_slot);
            let fr_i_slot = self.builder.build_alloca(i64t, "fr_i");
            self.st(self.v_i64(0), fr_i_slot);
            let fr_cond = self.loop_header(f, "part.fr.cond");
            let fr_i = self.ld(i64t, fr_i_slot, "fr_i.v");
            let fr_more = self.builder.build_icmp(IntCmp::Ult, fr_i, self.v_i64(KELP_SORT_BLOCK), "fr_more");
            let fr_body = self.block(f, "part.fr.body");
            let fr_done = self.block(f, "part.fr.done");
            self.builder.build_cond_br(fr_more, fr_body, fr_done);
            self.builder.position_at_end(fr_body);
            let fr_numr = self.ld(i64t, num_r_slot, "fr_numr");
            let fr_off_ptr = self.gep(i64t, offsets_r, fr_numr, "fr_off_ptr");
            self.st(fr_i, fr_off_ptr);
            let fr_idx = self.builder.build_int_binop(IntBinOp::Add, rbase, fr_i, "fr_idx");
            let fr_elem_ptr = self.gep(elem_ty, data, fr_idx, "fr_elem_ptr");
            let fr_cmp = self.call(cmpsym, cmpty, &[fr_elem_ptr, pivot_slot], "fr_cmp");
            let fr_wrong = self.builder.build_icmp(IntCmp::Sle, fr_cmp, self.v_i32(0), "fr_wrong");
            let fr_wrong64 = self.builder.build_int_cast(false, true, fr_wrong, i64t, "fr_wrong64");
            let fr_numr1 = self.builder.build_int_binop(IntBinOp::Add, fr_numr, fr_wrong64, "fr_numr1");
            self.st(fr_numr1, num_r_slot);
            let fr_i1 = self.builder.build_int_binop(IntBinOp::Add, fr_i, self.v_i64(1), "fr_i1");
            self.st(fr_i1, fr_i_slot);
            self.builder.build_br(fr_cond);
            self.builder.position_at_end(fr_done);
            self.builder.build_br(after_fill_r);
            self.builder.position_at_end(after_fill_r);

            // num = min(num_l, num_r); swap that many matched pairs.
            let num_l1 = self.ld(i64t, num_l_slot, "num_l1");
            let num_r1 = self.ld(i64t, num_r_slot, "num_r1");
            let l_smaller = self.builder.build_icmp(IntCmp::Ult, num_l1, num_r1, "l_smaller");
            let num = self.builder.build_select(l_smaller, num_l1, num_r1, "num");
            let start_l1 = self.ld(i64t, start_l_slot, "start_l1");
            let start_r1 = self.ld(i64t, start_r_slot, "start_r1");
            let sw_k_slot = self.builder.build_alloca(i64t, "sw_k");
            self.st(self.v_i64(0), sw_k_slot);
            let sw_cond = self.loop_header(f, "part.sw.cond");
            let sw_k = self.ld(i64t, sw_k_slot, "sw_k.v");
            let sw_more = self.builder.build_icmp(IntCmp::Ult, sw_k, num, "sw_more");
            let sw_body = self.block(f, "part.sw.body");
            let sw_done = self.block(f, "part.sw.done");
            self.builder.build_cond_br(sw_more, sw_body, sw_done);
            self.builder.position_at_end(sw_body);
            let sw_li_slot = self.builder.build_int_binop(IntBinOp::Add, start_l1, sw_k, "sw_li_slot");
            let sw_off_l_ptr = self.gep(i64t, offsets_l, sw_li_slot, "sw_off_l_ptr");
            let sw_off_l = self.ld(i64t, sw_off_l_ptr, "sw_off_l");
            let sw_li = self.builder.build_int_binop(IntBinOp::Add, l, sw_off_l, "sw_li");
            let sw_ri_slot = self.builder.build_int_binop(IntBinOp::Add, start_r1, sw_k, "sw_ri_slot");
            let sw_off_r_ptr = self.gep(i64t, offsets_r, sw_ri_slot, "sw_off_r_ptr");
            let sw_off_r = self.ld(i64t, sw_off_r_ptr, "sw_off_r");
            let sw_ri = self.builder.build_int_binop(IntBinOp::Add, rbase, sw_off_r, "sw_ri");
            let sw_li_ptr = self.gep(elem_ty, data, sw_li, "sw_li_ptr");
            let sw_ri_ptr = self.gep(elem_ty, data, sw_ri, "sw_ri_ptr");
            let sw_lv = self.ld(elem_ty, sw_li_ptr, "sw_lv");
            let sw_rv = self.ld(elem_ty, sw_ri_ptr, "sw_rv");
            self.st(sw_rv, sw_li_ptr);
            self.st(sw_lv, sw_ri_ptr);
            let sw_k1 = self.builder.build_int_binop(IntBinOp::Add, sw_k, self.v_i64(1), "sw_k1");
            self.st(sw_k1, sw_k_slot);
            self.builder.build_br(sw_cond);
            self.builder.position_at_end(sw_done);

            let new_num_l = self.builder.build_int_binop(IntBinOp::Sub, num_l1, num, "new_num_l");
            let new_start_l = self.builder.build_int_binop(IntBinOp::Add, start_l1, num, "new_start_l");
            self.st(new_num_l, num_l_slot);
            self.st(new_start_l, start_l_slot);
            let new_num_r = self.builder.build_int_binop(IntBinOp::Sub, num_r1, num, "new_num_r");
            let new_start_r = self.builder.build_int_binop(IntBinOp::Add, start_r1, num, "new_start_r");
            self.st(new_num_r, num_r_slot);
            self.st(new_start_r, start_r_slot);

            let num_l_zero2 = self.builder.build_icmp(IntCmp::Eq, new_num_l, self.v_i64(0), "num_l_zero2");
            let adv_l = self.block(f, "part.adv_l");
            let after_adv_l = self.block(f, "part.after_adv_l");
            self.builder.build_cond_br(num_l_zero2, adv_l, after_adv_l);
            self.builder.position_at_end(adv_l);
            let l_adv = self.builder.build_int_binop(IntBinOp::Add, l, self.v_i64(KELP_SORT_BLOCK), "l_adv");
            self.st(l_adv, l_slot);
            self.builder.build_br(after_adv_l);
            self.builder.position_at_end(after_adv_l);

            let num_r_zero2 = self.builder.build_icmp(IntCmp::Eq, new_num_r, self.v_i64(0), "num_r_zero2");
            let adv_r = self.block(f, "part.adv_r");
            let after_adv_r = self.block(f, "part.after_adv_r");
            self.builder.build_cond_br(num_r_zero2, adv_r, after_adv_r);
            self.builder.position_at_end(adv_r);
            let rr_adv = self.builder.build_int_binop(IntBinOp::Sub, rr, self.v_i64(KELP_SORT_BLOCK), "rr_adv");
            self.st(rr_adv, rr_slot);
            self.builder.build_br(after_adv_r);
            self.builder.position_at_end(after_adv_r);
            self.builder.build_br(blk_cond);

            // Tail: finish `[l, rr)` with the original branchy Hoare scan.
            self.builder.position_at_end(blk_done);
            let l_final = self.ld(i64t, l_slot, "l_final");
            let rr_final = self.ld(i64t, rr_slot, "rr_final");
            let i_slot = self.builder.build_alloca(i64t, "i");
            let l_final_m1 = self.builder.build_int_binop(IntBinOp::Sub, l_final, self.v_i64(1), "l_final_m1");
            self.st(l_final_m1, i_slot);
            let j_slot = self.builder.build_alloca(i64t, "j");
            self.st(rr_final, j_slot);
            let iscan = self.loop_header(f, "part.iscan");
            let ii = self.ld(i64t, i_slot, "ii");
            let ii1 = self.builder.build_int_binop(IntBinOp::Add, ii, self.v_i64(1), "ii1");
            self.st(ii1, i_slot);
            let at_i = self.gep(elem_ty, data, ii1, "at_i");
            let ci = self.call(cmpsym, cmpty, &[at_i, pivot_slot], "ci");
            let ilt = self.builder.build_icmp(IntCmp::Slt, ci, self.v_i32(0), "ilt");
            let jscan = self.block(f, "part.jscan");
            self.builder.build_cond_br(ilt, iscan, jscan);
            self.builder.position_at_end(jscan);
            let jj = self.ld(i64t, j_slot, "jj");
            let jj1 = self.builder.build_int_binop(IntBinOp::Sub, jj, self.v_i64(1), "jj1");
            self.st(jj1, j_slot);
            let at_j = self.gep(elem_ty, data, jj1, "at_j");
            let cj = self.call(cmpsym, cmpty, &[at_j, pivot_slot], "cj");
            let jgt = self.builder.build_icmp(IntCmp::Sgt, cj, self.v_i32(0), "jgt");
            let check = self.block(f, "part.check");
            self.builder.build_cond_br(jgt, jscan, check);
            self.builder.position_at_end(check);
            let ci2 = self.ld(i64t, i_slot, "ci2");
            let cj2 = self.ld(i64t, j_slot, "cj2");
            let ge = self.builder.build_icmp(IntCmp::Uge, ci2, cj2, "ge");
            let ret_blk = self.block(f, "part.ret");
            let swap_blk = self.block(f, "part.swap");
            self.builder.build_cond_br(ge, ret_blk, swap_blk);
            self.builder.position_at_end(ret_blk);
            self.builder.build_ret(cj2);
            self.builder.position_at_end(swap_blk);
            let at_i2 = self.gep(elem_ty, data, ci2, "at_i2");
            let at_j2 = self.gep(elem_ty, data, cj2, "at_j2");
            let iv = self.ld(elem_ty, at_i2, "iv");
            let jv = self.ld(elem_ty, at_j2, "jv");
            self.st(jv, at_i2);
            self.st(iv, at_j2);
            self.builder.build_br(iscan);

            // Introsort's quicksort driver over the half-open range
            // `[lo, hi)`. Below the insertion-sort cutoff, finish the
            // partition with plain insertion sort. Otherwise partition
            // around a median-of-three pivot and recurse into the smaller
            // side while looping into the larger one, which bounds the real
            // call stack to O(log n) on its own; the separate `depth_limit`,
            // decremented once per partition step and starting at roughly
            // `2 * floor(log2(n))`, is what actually bounds the *work*, not
            // just the stack: once it hits zero, the remaining range is
            // handed to the existing heapsort instead of continuing to
            // partition, which is what keeps an adversarial input (one that
            // would make plain quicksort's partitioning stay badly
            // imbalanced) at O(n log n) overall rather than degrading toward
            // O(n^2).
            let (f, _) = self.start(&format!("{base}__quicksort"), "entry")?;
            let self_p = f.get_param(0);
            let lo_slot = self.builder.build_alloca(i64t, "lo");
            self.st(f.get_param(1), lo_slot);
            let hi_slot = self.builder.build_alloca(i64t, "hi");
            self.st(f.get_param(2), hi_slot);
            let depth_slot = self.builder.build_alloca(i64t, "depth");
            self.st(f.get_param(3), depth_slot);
            let (ins, insty) = self.cfn(&format!("{base}__insertion_sort"))?;
            let (part, partty) = self.cfn(&format!("{base}__partition"))?;
            let (heap, heapty) = self.cfn(&format!("{base}__heapsort_range"))?;
            let (qs, qsty) = self.cfn(&format!("{base}__quicksort"))?;
            let cond = self.loop_header(f, "qs.cond");
            let lo = self.ld(i64t, lo_slot, "lo");
            let hi = self.ld(i64t, hi_slot, "hi");
            let width = self.builder.build_int_binop(IntBinOp::Sub, hi, lo, "width");
            let big = self.builder.build_icmp(
                IntCmp::Ugt,
                width,
                self.v_i64(KELP_SORT_INSERTION_CUTOFF),
                "big",
            );
            let body = self.block(f, "qs.body");
            let done = self.block(f, "qs.done");
            self.builder.build_cond_br(big, body, done);
            self.builder.position_at_end(body);
            let depth = self.ld(i64t, depth_slot, "depth");
            let depth_zero = self.builder.build_icmp(IntCmp::Eq, depth, self.v_i64(0), "depth_zero");
            let fallback = self.block(f, "qs.fallback");
            let part_blk = self.block(f, "qs.partition");
            self.builder.build_cond_br(depth_zero, fallback, part_blk);
            self.builder.position_at_end(fallback);
            self.call(heap, heapty, &[self_p, lo, hi], "");
            self.builder.build_ret_void();
            self.builder.position_at_end(part_blk);
            let depth1 = self.builder.build_int_binop(IntBinOp::Sub, depth, self.v_i64(1), "depth1");
            self.st(depth1, depth_slot);
            let hi_incl = self.builder.build_int_binop(IntBinOp::Sub, hi, self.v_i64(1), "hi_incl");
            let p = self.call(part, partty, &[self_p, lo, hi_incl], "p");
            let p1 = self.builder.build_int_binop(IntBinOp::Add, p, self.v_i64(1), "p1");
            let left_len = self.builder.build_int_binop(IntBinOp::Sub, p1, lo, "left_len");
            let right_len = self.builder.build_int_binop(IntBinOp::Sub, hi, p1, "right_len");
            let left_smaller = self.builder.build_icmp(IntCmp::Ult, left_len, right_len, "left_smaller");
            let take_left = self.block(f, "qs.left");
            let take_right = self.block(f, "qs.right");
            self.builder.build_cond_br(left_smaller, take_left, take_right);
            self.builder.position_at_end(take_left);
            self.call(qs, qsty, &[self_p, lo, p1, depth1], "");
            self.st(p1, lo_slot);
            self.builder.build_br(cond);
            self.builder.position_at_end(take_right);
            self.call(qs, qsty, &[self_p, p1, hi, depth1], "");
            self.st(p1, hi_slot);
            self.builder.build_br(cond);
            self.builder.position_at_end(done);
            let lo_f = self.ld(i64t, lo_slot, "lo_f");
            let hi_f = self.ld(i64t, hi_slot, "hi_f");
            self.call(ins, insty, &[self_p, lo_f, hi_f], "");
            self.builder.build_ret_void();

            // introsort: quicksort with a median-of-three pivot and an
            // insertion-sort cutoff, falling back to the heapsort above once
            // recursion depth passes roughly `2 * floor(log2(n))`. In place
            // throughout, no extra buffer: a swap or a pivot copy is an
            // element-sized copy, so an owned element is only moved, never
            // cloned or dropped, exactly as `{base}__reverse` elsewhere.
            let (f, _) = self.start(&format!("{base}__sort"), "entry")?;
            let self_p = f.get_param(0);
            let len = self.ld(i64t, self.fld(list_ty, self_p, L_LEN, "len"), "len");
            let small = self.builder.build_icmp(IntCmp::Ult, len, self.v_i64(2), "small");
            let early = self.block(f, "sort.early");
            let cont = self.block(f, "sort.cont");
            self.builder.build_cond_br(small, early, cont);
            self.builder.position_at_end(early);
            self.builder.build_ret_void();
            self.builder.position_at_end(cont);
            let (qs, qsty) = self.cfn(&format!("{base}__quicksort"))?;
            let nn_slot = self.builder.build_alloca(i64t, "nn");
            self.st(len, nn_slot);
            let depth_slot = self.builder.build_alloca(i64t, "depth");
            self.st(self.v_i64(0), depth_slot);
            let logcond = self.loop_header(f, "sort.log.cond");
            let nn = self.ld(i64t, nn_slot, "nn");
            let more = self.builder.build_icmp(IntCmp::Ugt, nn, self.v_i64(1), "more");
            let logbody = self.block(f, "sort.log.body");
            let logend = self.block(f, "sort.log.end");
            self.builder.build_cond_br(more, logbody, logend);
            self.builder.position_at_end(logbody);
            let nn2 = self.builder.build_int_binop(IntBinOp::LShr, nn, self.v_i64(1), "nn2");
            self.st(nn2, nn_slot);
            let d = self.ld(i64t, depth_slot, "d");
            let d1 = self.builder.build_int_binop(IntBinOp::Add, d, self.v_i64(1), "d1");
            self.st(d1, depth_slot);
            self.builder.build_br(logcond);
            self.builder.position_at_end(logend);
            let dfinal = self.ld(i64t, depth_slot, "dfinal");
            let depth_limit = self.builder.build_int_binop(IntBinOp::Mul, dfinal, self.v_i64(2), "depth_limit");
            self.call(qs, qsty, &[self_p, self.v_i64(0), len, depth_limit], "");
            self.builder.build_ret_void();
        }

        // --- contains(self, v) ---
        if comparable {
            let (f, _) = self.start(&format!("{base}__contains"), "entry")?;
            let self_p = f.get_param(0);
            let v = f.get_param(1);
            let out_slot = self.builder.build_alloca(i1t, "found");
            let i_slot = self.builder.build_alloca(i64t, "i");
            self.st(self.ctx.const_bool(false), out_slot);
            self.st(self.v_i64(0), i_slot);
            let header = self.loop_header(f, "has.cond");
            let i = self.ld(i64t, i_slot, "i");
            let len = self.ld(i64t, self.fld(list_ty, self_p, L_LEN, "len"), "len");
            let more = self.builder.build_icmp(IntCmp::Ult, i, len, "more");
            let body = self.block(f, "has.body");
            let exit = self.block(f, "has.end");
            self.builder.build_cond_br(more, body, exit);
            self.builder.position_at_end(body);
            let data = self.ld(ptr, self.fld(list_ty, self_p, L_DATA, "data"), "data");
            let at = self.gep(elem_ty, data, i, "at");
            let hit = match elem {
                Ty::String | Ty::Bytes => {
                    let alen = self.ld(i64t, self.fld(elem_ty, at, 1, "alen"), "alen");
                    let vlen = self.ld(i64t, self.fld(elem_ty, v, 1, "vlen"), "vlen");
                    let leq = self.builder.build_icmp(IntCmp::Eq, alen, vlen, "leq");
                    let zero = self.builder.build_icmp(IntCmp::Eq, vlen, self.v_i64(0), "z");
                    // `zleq` is computed in `body`, so the phi is the first
                    // instruction of `after` (a phi must be grouped at the top).
                    let zeros = self.builder.build_int_binop(IntBinOp::And, zero, leq, "zleq");
                    let data_cmp = self.block(f, "has.cmp");
                    let after = self.block(f, "has.after");
                    self.builder.build_cond_br(zero, after, data_cmp);
                    self.builder.position_at_end(data_cmp);
                    let adata = self.ld(ptr, self.fld(elem_ty, at, 0, "ad"), "ad");
                    let vdata = self.ld(ptr, self.fld(elem_ty, v, 0, "vd"), "vd");
                    let (memcmp, mty) = self.libc_fn("memcmp")?;
                    let c = self.call(memcmp, mty, &[adata, vdata, vlen], "memcmp");
                    let c0 = self.builder.build_icmp(IntCmp::Eq, c, self.v_i32(0), "c0");
                    // Length first, then the bytes: `"app"` must not equal
                    // `"apple"` even though `memcmp` over the shorter prefix
                    // says the bytes agree.
                    let ceq = self.builder.build_int_binop(IntBinOp::And, leq, c0, "ceq");
                    self.builder.build_br(after);
                    self.builder.position_at_end(after);
                    let hit_phi = self.builder.build_phi(i1t, "hit");
                    self.builder.add_incoming(hit_phi, &[(zeros, body), (ceq, data_cmp)]);
                    hit_phi
                }
                // A float element is `Copy`, so `contains` is offered for it
                // and C's `==` on a double is `fcmp oeq`; an integer compare
                // here is invalid IR on the emitted `double`.
                _ if elem.is_float() => {
                    self.builder
                        .build_fcmp(RealCmp::Oeq, self.ld(elem_ty, at, "av"), v, "hit")
                }
                _ => self.builder.build_icmp(IntCmp::Eq, self.ld(elem_ty, at, "av"), v, "hit"),
            };
            let found = self.block(f, "has.found");
            let next = self.block(f, "has.next");
            self.builder.build_cond_br(hit, found, next);
            self.builder.position_at_end(found);
            self.st(self.ctx.const_bool(true), out_slot);
            self.builder.build_br(exit);
            self.builder.position_at_end(next);
            let inext = self.builder.build_int_binop(IntBinOp::Add, i, self.v_i64(1), "i1");
            self.st(inext, i_slot);
            self.builder.build_br(header);
            self.builder.position_at_end(exit);
            let out = self.ld(i1t, out_slot, "found");
            self.builder.build_ret(out);
        }

        // --- slice(self, from, to) and clone(self) ---
        if clonable {
            let (f, _) = self.start(&format!("{base}__slice"), "entry")?;
            let self_p = f.get_param(0);
            let from = f.get_param(1);
            let to = f.get_param(2);
            let len = self.ld(i64t, self.fld(list_ty, self_p, L_LEN, "len"), "len");
            let a = self.builder.build_icmp(IntCmp::Ugt, from, to, "from_gt_to");
            let b = self.builder.build_icmp(IntCmp::Ugt, to, len, "to_gt_len");
            let bad = self.builder.build_int_binop(IntBinOp::Or, a, b, "oob");
            let oob = self.block(f, "oob");
            let ok = self.block(f, "slice");
            self.builder.build_cond_br(bad, oob, ok);
            self.builder.position_at_end(oob);
            self.emit_panic("List.slice: range out of bounds")?;
            self.builder.position_at_end(ok);
            let out_slot = self.builder.build_alloca(list_ty, "out");
            self.st(self.ctx.const_null(list_ty), out_slot);
            let n = self.builder.build_int_binop(IntBinOp::Sub, to, from, "n");
            let (reserve, rty) = self.cfn(&format!("{base}__reserve"))?;
            self.call(reserve, rty, &[out_slot, n], "");
            let i_slot = self.builder.build_alloca(i64t, "i");
            self.st(from, i_slot);
            let header = self.loop_header(f, "slice.cond");
            let i = self.ld(i64t, i_slot, "i");
            let more = self.builder.build_icmp(IntCmp::Ult, i, to, "more");
            let body = self.block(f, "slice.body");
            let exit = self.block(f, "slice.end");
            self.builder.build_cond_br(more, body, exit);
            self.builder.position_at_end(body);
            let data = self.ld(ptr, self.fld(list_ty, self_p, L_DATA, "data"), "data");
            let src = self.gep(elem_ty, data, i, "src");
            let olen = self.ld(i64t, self.fld(list_ty, out_slot, L_LEN, "olen"), "olen");
            let odata = self.ld(ptr, self.fld(list_ty, out_slot, L_DATA, "odata"), "odata");
            let dst = self.gep(elem_ty, odata, olen, "dst");
            self.clone_into(src, dst, elem)?;
            let olen1 = self.builder.build_int_binop(IntBinOp::Add, olen, self.v_i64(1), "olen1");
            self.st(olen1, self.fld(list_ty, out_slot, L_LEN, "len"));
            let inext = self.builder.build_int_binop(IntBinOp::Add, i, self.v_i64(1), "i1");
            self.st(inext, i_slot);
            self.builder.build_br(header);
            self.builder.position_at_end(exit);
            let out = self.ld(list_ty, out_slot, "slice");
            self.builder.build_ret(out);

            let (f, _) = self.start(&format!("{base}__clone"), "entry")?;
            let self_p = f.get_param(0);
            let len = self.ld(i64t, self.fld(list_ty, self_p, L_LEN, "len"), "len");
            let (slice, sty) = self.cfn(&format!("{base}__slice"))?;
            let out = self.call(slice, sty, &[self_p, self.v_i64(0), len], "clone");
            self.builder.build_ret(out);
        }

        // --- drop(self) ---
        {
            let (f, _) = self.start(&drop_fn_name(&Ty::List(Box::new(elem.clone()))), "entry")?;
            let self_p = f.get_param(0);
            let (clear, cty2) = self.cfn(&format!("{base}__clear"))?;
            self.call(clear, cty2, &[self_p], "");
            let cap = self.ld(i64t, self.fld(list_ty, self_p, L_CAP, "cap"), "cap");
            let bytes = self
                .builder
                .build_int_binop(IntBinOp::Mul, cap, self.v_i64(elem_size), "bytes");
            let data = self.ld(ptr, self.fld(list_ty, self_p, L_DATA, "data"), "data");
            self.heap_dealloc(data, bytes, elem_align)?;
            self.st(self.ctx.const_null(ptr), self.fld(list_ty, self_p, L_DATA, "data"));
            self.st(self.v_i64(0), self.fld(list_ty, self_p, L_CAP, "cap"));
            self.builder.build_ret_void();
        }
        Ok(())
    }

    // ---- builtin runtime bodies -------------------------------------------
}
