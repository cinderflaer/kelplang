use super::*;

impl<'a> Gen<'a> {
    /// Emit bodies for the small builtin accessors whose whole cost is the call
    /// itself, and give each definition internal linkage so this module's own
    /// `-O2` inlines it the way clang inlines the same text spliced into the C
    /// backend's translation unit. The runtime object still defines every one of
    /// them for the callers that stay out of line, and an internal definition is
    /// no link-time name, so the two never meet. The bodies, their bounds checks
    /// and their panic strings are the runtime crate's, so nothing observable
    /// moves.
    ///
    /// Only fixed-layout accessors and the one-byte append are here: an owned
    /// buffer, a `String` free of a layout question, and a `View`, which is two
    /// eightbytes by value. The rest of the runtime (allocation, drops, the map
    /// hash, the capability checks) stays an out-of-line call.
    pub(crate) fn emit_runtime_bodies(&self) -> Result<(), String> {
        let i8t = self.ctx.i8();
        let i64t = self.ctx.i64();
        let ptr = self.pty();
        let bytes = *self
            .structs
            .get("Bytes")
            .ok_or_else(|| no_value("the builtin 'Bytes' layout"))?;
        let string = *self
            .structs
            .get("String")
            .ok_or_else(|| no_value("the builtin 'String' layout"))?;

        // Begin an already-declared helper's body, and make the definition
        // module-private: it is no link-time name, and this module's own `-O2`
        // is free to inline it away or drop it.
        let body = |symbol: &str| -> Result<LlvmFunction<'a>, String> {
            let rt = self.rt(symbol)?;
            // Module-private: it is no link-time name, and this module's own
            // `-O2` is free to inline it away or drop it. Only the whole-program
            // build emits these bodies at all; the incremental mode calls the
            // runtime object's exported copies (`emits_runtime_accessors`).
            rt.func.as_value().set_internal_linkage();
            let bb = rt.func.append_block("entry");
            self.builder.position_at_end(bb);
            Ok(rt.func)
        };
        let len_of = |agg: Type<'a>, self_p: Value<'a>| -> Value<'a> {
            self.ld(i64t, self.fld(agg, self_p, L_LEN, "len"), "len")
        };
        let data_of = |agg: Type<'a>, self_p: Value<'a>| -> Value<'a> {
            self.ld(ptr, self.fld(agg, self_p, L_DATA, "data"), "data")
        };

        // --- Bytes.len / is_empty / get / set / push ---
        {
            let f = body("kelp_Bytes__len")?;
            let n = len_of(bytes, f.get_param(0));
            self.builder.build_ret(n);
        }
        {
            let f = body("kelp_Bytes__is_empty")?;
            let n = len_of(bytes, f.get_param(0));
            let empty = self.builder.build_icmp(IntCmp::Eq, n, self.v_i64(0), "empty");
            self.builder.build_ret(empty);
        }
        {
            let f = body("kelp_Bytes__get")?;
            let self_p = f.get_param(0);
            let i = f.get_param(1);
            let n = len_of(bytes, self_p);
            self.emit_bounds_check(f, n, i, "Bytes.get: index out of bounds")?;
            let at = self.gep(i8t, data_of(bytes, self_p), i, "at");
            let byte = self.ld(i8t, at, "byte");
            self.builder.build_ret(byte);
        }
        {
            let f = body("kelp_Bytes__set")?;
            let self_p = f.get_param(0);
            let i = f.get_param(1);
            let value = f.get_param(2);
            let n = len_of(bytes, self_p);
            self.emit_bounds_check(f, n, i, "Bytes.set: index out of bounds")?;
            let at = self.gep(i8t, data_of(bytes, self_p), i, "at");
            self.st(value, at);
            self.builder.build_ret_void();
        }
        {
            let f = body("kelp_Bytes__push")?;
            let self_p = f.get_param(0);
            let byte = f.get_param(1);
            let n = len_of(bytes, self_p);
            self.emit_grow_call(f, bytes, self_p, self.v_i64(1))?;
            let at = self.gep(i8t, data_of(bytes, self_p), n, "at");
            self.st(byte, at);
            let next = self.builder.build_int_binop(IntBinOp::Add, n, self.v_i64(1), "len1");
            self.st(next, self.fld(bytes, self_p, L_LEN, "len"));
            let term = self.gep(i8t, data_of(bytes, self_p), next, "term");
            self.st(self.v_i8(0), term);
            self.builder.build_ret_void();
        }

        // --- String.len / is_empty ---
        {
            let f = body("kelp_String__len")?;
            let n = len_of(string, f.get_param(0));
            self.builder.build_ret(n);
        }
        {
            let f = body("kelp_String__is_empty")?;
            let n = len_of(string, f.get_param(0));
            let empty = self.builder.build_icmp(IntCmp::Eq, n, self.v_i64(0), "empty");
            self.builder.build_ret(empty);
        }

        // --- String.push ---
        {
            let f = body("kelp_String__push")?;
            let self_p = f.get_param(0);
            let ch = f.get_param(1);
            let n = len_of(string, self_p);
            self.emit_grow_call(f, string, self_p, self.v_i64(1))?;
            let at = self.gep(i8t, data_of(string, self_p), n, "at");
            self.st(ch, at);
            let next = self.builder.build_int_binop(IntBinOp::Add, n, self.v_i64(1), "len1");
            self.st(next, self.fld(string, self_p, L_LEN, "len"));
            let term = self.gep(i8t, data_of(string, self_p), next, "term");
            self.st(self.v_i8(0), term);
            self.builder.build_ret_void();
        }

        // --- String.push_str ---
        {
            let f = body("kelp_String__push_str")?;
            let self_p = f.get_param(0);
            let text = f.get_param(1);
            let (strlen, strlen_ty) = self.libc_fn("strlen")?;
            let n = self.call(strlen, strlen_ty, &[text], "n");
            let none = self.builder.build_icmp(IntCmp::Eq, n, self.v_i64(0), "len0");
            let done = self.block(f, "ret");
            let copy = self.block(f, "copy");
            self.builder.build_cond_br(none, done, copy);
            self.builder.position_at_end(copy);
            let len = len_of(string, self_p);
            self.emit_grow_call(f, string, self_p, n)?;
            let dst = self.gep(i8t, data_of(string, self_p), len, "dst");
            let (memcpy, memcpy_ty) = self.libc_fn("memcpy")?;
            self.call(memcpy, memcpy_ty, &[dst, text, n], "");
            let next = self.builder.build_int_binop(IntBinOp::Add, len, n, "len1");
            self.st(next, self.fld(string, self_p, L_LEN, "len"));
            let term = self.gep(i8t, data_of(string, self_p), next, "term");
            self.st(self.v_i8(0), term);
            self.builder.build_br(done);
            self.builder.position_at_end(done);
            self.builder.build_ret_void();
        }

        // --- String.reserve ---
        {
            let f = body("kelp_String__reserve")?;
            let self_p = f.get_param(0);
            let extra = f.get_param(1);
            self.emit_grow_call(f, string, self_p, extra)?;
            self.builder.build_ret_void();
        }

        // --- View.len / is_empty / get ---
        {
            let f = body("kelp_View__len")?;
            let n = self.builder.build_extract_value(f.get_param(0), 1, "len");
            self.builder.build_ret(n);
        }
        {
            let f = body("kelp_View__is_empty")?;
            let n = self.builder.build_extract_value(f.get_param(0), 1, "len");
            let empty = self.builder.build_icmp(IntCmp::Eq, n, self.v_i64(0), "empty");
            self.builder.build_ret(empty);
        }
        {
            let f = body("kelp_View__get")?;
            let self_v = f.get_param(0);
            let i = f.get_param(1);
            let n = self.builder.build_extract_value(self_v, 1, "len");
            self.emit_bounds_check(f, n, i, "View.get: index out of bounds")?;
            let text = self.builder.build_extract_value(self_v, 0, "ptr");
            let at = self.gep(i8t, text, i, "at");
            let byte = self.ld(i8t, at, "byte");
            self.builder.build_ret(byte);
        }
        Ok(())
    }

    /// Emit the libc-free `str` surface a freestanding build needs: the five
    /// `std/str` operations the allowed set admits, and the `View` accessors
    /// their Kelp bodies reach. The hosted definitions are `strlen`/`memcmp`
    /// in the runtime crate and the `externs.rs` bodies, neither of which is
    /// linked into a bare-metal image, so each one is emitted here as a byte
    /// loop and given internal linkage, exactly as `emit_runtime_bodies`
    /// does for the hosted accessors.
    ///
    /// `str.byte_at_raw`'s bounds check is the one new panic site: it calls the
    /// compiler's panic entry (`self.panic_fn`) with the same
    /// `"str.byte: index out of bounds"` text the runtime body passes, so the message
    /// a kernel prints for an over-long index is the one the hosted build
    /// prints. The line is the current statement's, since the accessor body is
    /// shared by every call site.
    ///
    /// A program with no `std/str` in it has none of the three raw externs
    /// declared, and this emits nothing; the `View` accessors are always
    /// declared, and an unused internal definition references nothing the
    /// image does not already define.
    pub(crate) fn emit_freestanding_str_bodies(&self) -> Result<(), String> {
        let i8t = self.ctx.i8();
        let i64t = self.ctx.i64();
        let view = *self
            .structs
            .get("View")
            .ok_or_else(|| no_value("the builtin 'View' layout"))?;

        let body = |symbol: &str| -> Result<LlvmFunction<'a>, String> {
            let rt = self.rt(symbol)?;
            rt.func.as_value().set_internal_linkage();
            // The loops below are the lowering, not an input to be improved: an
            // optimizable NUL scan is turned into a `strlen` call, which is the
            // libc reference this whole mode exists to keep out of the image.
            rt.func.mark_optnone();
            let bb = rt.func.append_block("entry");
            self.builder.position_at_end(bb);
            Ok(rt.func)
        };

        // --- str.byte_len_raw ---
        if self.runtime.contains_key("kelp_std__str__byte_len_raw") {
            let f = body("kelp_std__str__byte_len_raw")?;
            let n = self.emit_cstr_len(f, f.get_param(0))?;
            self.builder.build_ret(n);

            // --- str.byte_at_raw ---
            let f = body("kelp_std__str__byte_at_raw")?;
            let s = f.get_param(0);
            let i = f.get_param(1);
            let n = self.emit_cstr_len(f, s)?;
            self.emit_bounds_check(f, n, i, "str.byte: index out of bounds")?;
            let at = self.gep(i8t, s, i, "at");
            let byte = self.ld(i8t, at, "byte");
            self.builder.build_ret(byte);

            // --- str.byte_ptr_raw ---
            let f = body("kelp_std__str__byte_ptr_raw")?;
            self.builder.build_ret(f.get_param(0));
        }

        // --- str.view ---
        // The one place a `str`'s length is derived. A `str` this mode can hold
        // is a literal or a panic message, so it is never null and the hosted
        // body's null test has nothing to answer here.
        {
            let f = body("kelp_str__view")?;
            let s = f.get_param(0);
            let n = self.emit_cstr_len(f, s)?;
            let slot = self.builder.build_alloca(view, "view");
            self.st(s, self.fld(view, slot, 0, "ptr"));
            self.st(n, self.fld(view, slot, 1, "len"));
            let v = self.ld(view, slot, "view");
            self.builder.build_ret(v);
        }

        // --- View.len ---
        {
            let f = body("kelp_View__len")?;
            let n = self.builder.build_extract_value(f.get_param(0), 1, "len");
            self.builder.build_ret(n);
        }

        // --- View.get ---
        {
            let f = body("kelp_View__get")?;
            let v = f.get_param(0);
            let i = f.get_param(1);
            let n = self.builder.build_extract_value(v, 1, "len");
            self.emit_bounds_check(f, n, i, "View.get: index out of bounds")?;
            let text = self.builder.build_extract_value(v, 0, "ptr");
            let at = self.gep(i8t, text, i, "at");
            let byte = self.ld(i8t, at, "byte");
            self.builder.build_ret(byte);
        }

        // --- View.eq ---
        // `memcmp` over the shorter prefix would call "ab" and "abc" equal, so
        // the lengths are compared first, exactly as the runtime object does.
        {
            let f = body("kelp_View__eq")?;
            let a = f.get_param(0);
            let b = f.get_param(1);
            let la = self.builder.build_extract_value(a, 1, "la");
            let lb = self.builder.build_extract_value(b, 1, "lb");
            let pa = self.builder.build_extract_value(a, 0, "pa");
            let pb = self.builder.build_extract_value(b, 0, "pb");
            self.emit_length_byte_eq(f, pa, la, pb, lb)?;
        }

        // --- View.slice ---
        // The hosted body is a pointer and a subtraction plus a bounds panic;
        // `std/str`'s trimming and searching and `std/regex`'s match text are
        // written on it, so a freestanding build needs the same function.
        {
            let f = body("kelp_View__slice")?;
            let self_v = f.get_param(0);
            let from = f.get_param(1);
            let to = f.get_param(2);
            let len = self.builder.build_extract_value(self_v, 1, "len");
            let ptr = self.builder.build_extract_value(self_v, 0, "ptr");
            let from_gt_to = self.builder.build_icmp(IntCmp::Ugt, from, to, "bad.from");
            let to_gt_len = self.builder.build_icmp(IntCmp::Ugt, to, len, "bad.to");
            let bad = self
                .builder
                .build_int_binop(IntBinOp::Or, from_gt_to, to_gt_len, "bad");
            let out = self.block(f, "panic");
            let ok = self.block(f, "in.bounds");
            self.builder.build_cond_br(bad, out, ok);
            self.builder.position_at_end(out);
            self.emit_panic("View.slice: index out of bounds")?;
            self.builder.position_at_end(ok);
            let at = self.gep(i8t, ptr, from, "at");
            let n = self
                .builder
                .build_int_binop(IntBinOp::Sub, to, from, "slice.len");
            let slot = self.builder.build_alloca(view, "view");
            self.st(at, self.fld(view, slot, 0, "ptr"));
            self.st(n, self.fld(view, slot, 1, "len"));
            let v = self.ld(view, slot, "view");
            self.builder.build_ret(v);
        }

        // --- kelp_map_key_eq ---
        // The derived key equality of a `datac`/`enum` with a `String` field
        // compares the stored field through this helper (`emit_field_eq`), and
        // the hosted body is `memcmp` in the runtime object, which a
        // freestanding image does not have. The lengths are compared first for
        // the same reason `View.eq` compares them: `memcmp` over the shorter
        // prefix would call "ab" and "abc" equal.
        if self.runtime.contains_key("kelp_map_key_eq") {
            let f = body("kelp_map_key_eq")?;
            let key = f.get_param(0);
            let data = f.get_param(1);
            let len = f.get_param(2);
            let string = *self
                .structs
                .get("String")
                .ok_or_else(|| no_value("the builtin 'String' layout"))?;
            let key_len = self.ld(i64t, self.fld(string, key, 1, "key.len"), "key.len");
            let key_data = self.ld(self.pty(), self.fld(string, key, 0, "key.data"), "key.data");
            self.emit_length_byte_eq(f, key_data, key_len, data, len)?;
        }
        Ok(())
    }

    /// The body `View.eq` and the `String`-field arm of `kelp_map_key_eq`
    /// share: compare the two lengths, then the bytes, and return `true` or
    /// `false`. Emits the whole function body, terminators included, because
    /// both callers are just the two pointer/length pairs.
    pub(crate) fn emit_length_byte_eq(
        &self,
        f: LlvmFunction<'a>,
        pa: Value<'a>,
        la: Value<'a>,
        pb: Value<'a>,
        lb: Value<'a>,
    ) -> Result<(), String> {
        let i8t = self.ctx.i8();
        let i64t = self.ctx.i64();
        let same_len = self.builder.build_icmp(IntCmp::Eq, la, lb, "same.len");
        let scan = self.block(f, "eq.scan");
        let no = self.block(f, "eq.false");
        self.builder.build_cond_br(same_len, scan, no);
        self.builder.position_at_end(no);
        self.builder.build_ret(self.ctx.const_int(self.ctx.i1(), 0, false));

        self.builder.position_at_end(scan);
        let i_slot = self.builder.build_alloca(i64t, "i");
        self.st(self.v_i64(0), i_slot);
        let header = self.loop_header(f, "eq.cond");
        let i = self.ld(i64t, i_slot, "i");
        let more = self.builder.build_icmp(IntCmp::Ult, i, la, "more");
        let next = self.block(f, "eq.next");
        let yes = self.block(f, "eq.true");
        self.builder.build_cond_br(more, next, yes);
        self.builder.position_at_end(yes);
        self.builder.build_ret(self.ctx.const_int(self.ctx.i1(), 1, false));
        self.builder.position_at_end(next);
        let x = self.ld(i8t, self.gep(i8t, pa, i, "xa"), "x");
        let y = self.ld(i8t, self.gep(i8t, pb, i, "yb"), "y");
        let differs = self.builder.build_icmp(IntCmp::Ne, x, y, "differs");
        let step = self.block(f, "eq.step");
        let diff = self.block(f, "eq.diff");
        self.builder.build_cond_br(differs, diff, step);
        self.builder.position_at_end(diff);
        self.builder.build_ret(self.ctx.const_int(self.ctx.i1(), 0, false));
        self.builder.position_at_end(step);
        self.st(self.builder.build_int_binop(IntBinOp::Add, i, self.v_i64(1), "i1"), i_slot);
        self.builder.build_br(header);
        Ok(())
    }

    /// Emit the libc-free `String`/`Bytes` bodies a freestanding build with an
    /// allocator needs (kernel stage 12, MK9-S5-M2). Each is a byte loop over
    /// the buffer the project's own allocator returned, and every allocation and
    /// free goes through the `[target]` hook with alignment 1; the hosted
    /// runtime object defines the same symbols, and these definitions replace
    /// it in a freestanding image. Everything is internal and `optnone`, so the
    /// module's own pipeline cannot rewrite a NUL scan into `strlen`.
    ///
    /// `kelp_buf_reserve` is the growth routine both buffers share: double the
    /// capacity from 8 until it holds `len + extra`, grow through the project's
    /// `realloc` when the manifest names one and through alloc + copy + dealloc
    /// otherwise, and keep the block NUL-terminated. Only called when an
    /// allocator is configured: with none, the front end has already refused
    /// every `String`/`Bytes`, and a body reaching libc's `malloc` would be the
    /// very reference this mode excludes.
    pub(crate) fn emit_freestanding_buffer_bodies(&self) -> Result<(), String> {
        let i8t = self.ctx.i8();
        let i64t = self.ctx.i64();
        let ptr = self.pty();
        let bytes = *self
            .structs
            .get("Bytes")
            .ok_or_else(|| no_value("the builtin 'Bytes' layout"))?;
        let string = *self
            .structs
            .get("String")
            .ok_or_else(|| no_value("the builtin 'String' layout"))?;
        let view = *self
            .structs
            .get("View")
            .ok_or_else(|| no_value("the builtin 'View' layout"))?;

        let body = |symbol: &str| -> Result<LlvmFunction<'a>, String> {
            let rt = self.rt(symbol)?;
            rt.func.as_value().set_internal_linkage();
            rt.func.mark_optnone();
            let bb = rt.func.append_block("entry");
            self.builder.position_at_end(bb);
            Ok(rt.func)
        };
        let len_of = |agg: Type<'a>, self_p: Value<'a>| -> Value<'a> {
            self.ld(i64t, self.fld(agg, self_p, L_LEN, "len"), "len")
        };
        let cap_of = |agg: Type<'a>, self_p: Value<'a>| -> Value<'a> {
            self.ld(i64t, self.fld(agg, self_p, L_CAP, "cap"), "cap")
        };
        let data_of = |agg: Type<'a>, self_p: Value<'a>| -> Value<'a> {
            self.ld(ptr, self.fld(agg, self_p, L_DATA, "data"), "data")
        };

        // --- kelp_buf_reserve(data_p, len, cap_p, extra) ---
        {
            let f = body("kelp_buf_reserve")?;
            let data_p = f.get_param(0);
            let len = f.get_param(1);
            let cap_p = f.get_param(2);
            let extra = f.get_param(3);
            let cap = self.ld(i64t, cap_p, "cap");
            let need = self
                .builder
                .build_int_binop(IntBinOp::Add, len, extra, "need");
            let fits = self.builder.build_icmp(IntCmp::Ule, need, cap, "fits");
            let grow = self.block(f, "grow");
            let done = self.block(f, "reserve.done");
            self.builder.build_cond_br(fits, done, grow);
            self.builder.position_at_end(grow);

            let want_slot = self.builder.build_alloca(i64t, "want");
            let no_cap = self.builder.build_icmp(IntCmp::Eq, cap, self.v_i64(0), "cap0");
            let want0 = self
                .builder
                .build_select(no_cap, self.v_i64(8), cap, "want0");
            self.st(want0, want_slot);
            let doubling = self.loop_header(f, "want.cond");
            let w = self.ld(i64t, want_slot, "want");
            let more = self.builder.build_icmp(IntCmp::Ult, w, need, "more");
            let next_want = self.block(f, "want.double");
            let ready = self.block(f, "want.ready");
            self.builder.build_cond_br(more, next_want, ready);
            self.builder.position_at_end(next_want);
            let doubled = self
                .builder
                .build_int_binop(IntBinOp::Mul, w, self.v_i64(2), "want2");
            self.st(doubled, want_slot);
            self.builder.build_br(doubling);
            self.builder.position_at_end(ready);
            let want = self.ld(i64t, want_slot, "want");
            let new_bytes = self
                .builder
                .build_int_binop(IntBinOp::Add, want, self.v_i64(1), "new.bytes");

            let old = self.ld(ptr, data_p, "old");
            let no_old = self
                .builder
                .build_icmp(IntCmp::Eq, old, self.ctx.const_null(ptr), "old.null");
            let p_slot = self.builder.build_alloca(ptr, "buf");
            let fresh = self.block(f, "reserve.fresh");
            let reuse = self.block(f, "reserve.reuse");
            let store = self.block(f, "reserve.store");
            self.builder.build_cond_br(no_old, fresh, reuse);

            self.builder.position_at_end(fresh);
            let p = self.heap_alloc(new_bytes, 1)?;
            self.check_alloc(p, f)?;
            self.st(p, p_slot);
            self.builder.build_br(store);

            self.builder.position_at_end(reuse);
            let old_bytes = self
                .builder
                .build_int_binop(IntBinOp::Add, cap, self.v_i64(1), "old.bytes");
            let grown = self.heap_realloc(old, old_bytes, new_bytes, 1, f)?;
            self.st(grown, p_slot);
            self.builder.build_br(store);

            self.builder.position_at_end(store);
            let buf = self.ld(ptr, p_slot, "buf");
            self.st(buf, data_p);
            self.st(want, cap_p);
            let term = self.gep(i8t, buf, len, "term");
            self.st(self.v_i8(0), term);
            self.builder.build_br(done);

            self.builder.position_at_end(done);
            self.builder.build_ret_void();
        }

        // --- String.empty / Bytes.empty ---
        for (symbol, agg) in [
            ("kelp_String__empty", string),
            ("kelp_Bytes__empty", bytes),
        ] {
            let f = body(symbol)?;
            let dst = f.get_param(0);
            let p = self.heap_alloc(self.v_i64(1), 1)?;
            self.check_alloc(p, f)?;
            self.st(self.v_i8(0), p);
            self.st(p, self.fld(agg, dst, L_DATA, "data"));
            self.st(self.v_i64(0), self.fld(agg, dst, L_LEN, "len"));
            self.st(self.v_i64(0), self.fld(agg, dst, L_CAP, "cap"));
            self.builder.build_ret_void();
        }

        // --- String.from_str / Bytes.from_str ---
        for (symbol, agg) in [
            ("kelp_String__from_str", string),
            ("kelp_Bytes__from_str", bytes),
        ] {
            let f = body(symbol)?;
            let dst = f.get_param(0);
            let s = f.get_param(1);
            let n = self.emit_cstr_len(f, s)?;
            let n1 = self
                .builder
                .build_int_binop(IntBinOp::Add, n, self.v_i64(1), "n1");
            let p = self.heap_alloc(n1, 1)?;
            self.check_alloc(p, f)?;
            let (memcpy, mty) = self.mem_fn("memcpy")?;
            self.call(memcpy, mty, &[p, s, n1], "");
            self.st(p, self.fld(agg, dst, L_DATA, "data"));
            self.st(n, self.fld(agg, dst, L_LEN, "len"));
            self.st(n, self.fld(agg, dst, L_CAP, "cap"));
            self.builder.build_ret_void();
        }

        // --- String.clone / Bytes.clone ---
        for (symbol, agg) in [
            ("kelp_String__clone", string),
            ("kelp_Bytes__clone", bytes),
        ] {
            let f = body(symbol)?;
            let dst = f.get_param(0);
            let self_p = f.get_param(1);
            let n = len_of(agg, self_p);
            let src = data_of(agg, self_p);
            let n1 = self
                .builder
                .build_int_binop(IntBinOp::Add, n, self.v_i64(1), "n1");
            let p = self.heap_alloc(n1, 1)?;
            self.check_alloc(p, f)?;
            let (memcpy, mty) = self.mem_fn("memcpy")?;
            self.call(memcpy, mty, &[p, src, n], "");
            let term = self.gep(i8t, p, n, "term");
            self.st(self.v_i8(0), term);
            self.st(p, self.fld(agg, dst, L_DATA, "data"));
            self.st(n, self.fld(agg, dst, L_LEN, "len"));
            self.st(n, self.fld(agg, dst, L_CAP, "cap"));
            self.builder.build_ret_void();
        }

        // --- String.with_capacity(n) ---
        {
            let f = body("kelp_String__with_capacity")?;
            let dst = f.get_param(0);
            let n = f.get_param(1);
            let n1 = self
                .builder
                .build_int_binop(IntBinOp::Add, n, self.v_i64(1), "n1");
            let p = self.heap_alloc(n1, 1)?;
            self.check_alloc(p, f)?;
            self.st(self.v_i8(0), p);
            self.st(p, self.fld(string, dst, L_DATA, "data"));
            self.st(self.v_i64(0), self.fld(string, dst, L_LEN, "len"));
            self.st(n, self.fld(string, dst, L_CAP, "cap"));
            self.builder.build_ret_void();
        }

        // --- len / is_empty, for both buffers ---
        for (len_symbol, empty_symbol, agg) in [
            ("kelp_String__len", "kelp_String__is_empty", string),
            ("kelp_Bytes__len", "kelp_Bytes__is_empty", bytes),
        ] {
            {
                let f = body(len_symbol)?;
                let n = len_of(agg, f.get_param(0));
                self.builder.build_ret(n);
            }
            {
                let f = body(empty_symbol)?;
                let n = len_of(agg, f.get_param(0));
                let empty = self.builder.build_icmp(IntCmp::Eq, n, self.v_i64(0), "empty");
                self.builder.build_ret(empty);
            }
        }

        // --- String.as_str / String.view ---
        {
            let f = body("kelp_String__as_str")?;
            let d = data_of(string, f.get_param(0));
            self.builder.build_ret(d);
        }
        {
            let f = body("kelp_String__view")?;
            let self_p = f.get_param(0);
            let d = data_of(string, self_p);
            let n = len_of(string, self_p);
            let slot = self.builder.build_alloca(view, "view");
            self.st(d, self.fld(view, slot, 0, "ptr"));
            self.st(n, self.fld(view, slot, 1, "len"));
            let v = self.ld(view, slot, "view");
            self.builder.build_ret(v);
        }

        // --- push, for both buffers ---
        for (symbol, agg) in [
            ("kelp_String__push", string),
            ("kelp_Bytes__push", bytes),
        ] {
            let f = body(symbol)?;
            let self_p = f.get_param(0);
            let byte = f.get_param(1);
            self.emit_grow_call(f, agg, self_p, self.v_i64(1))?;
            let n = len_of(agg, self_p);
            let at = self.gep(i8t, data_of(agg, self_p), n, "at");
            self.st(byte, at);
            let next = self
                .builder
                .build_int_binop(IntBinOp::Add, n, self.v_i64(1), "len1");
            self.st(next, self.fld(agg, self_p, L_LEN, "len"));
            let term = self.gep(i8t, data_of(agg, self_p), next, "term");
            self.st(self.v_i8(0), term);
            self.builder.build_ret_void();
        }

        // --- String.push_str ---
        {
            let f = body("kelp_String__push_str")?;
            let self_p = f.get_param(0);
            let text = f.get_param(1);
            let n = self.emit_cstr_len(f, text)?;
            let none = self.builder.build_icmp(IntCmp::Eq, n, self.v_i64(0), "len0");
            let ret = self.block(f, "push_str.ret");
            let copy = self.block(f, "push_str.copy");
            self.builder.build_cond_br(none, ret, copy);
            self.builder.position_at_end(copy);
            let len = len_of(string, self_p);
            self.emit_grow_call(f, string, self_p, n)?;
            let dst = self.gep(i8t, data_of(string, self_p), len, "dst");
            let (memcpy, mty) = self.mem_fn("memcpy")?;
            self.call(memcpy, mty, &[dst, text, n], "");
            let next = self
                .builder
                .build_int_binop(IntBinOp::Add, len, n, "len1");
            self.st(next, self.fld(string, self_p, L_LEN, "len"));
            let term = self.gep(i8t, data_of(string, self_p), next, "term");
            self.st(self.v_i8(0), term);
            self.builder.build_br(ret);
            self.builder.position_at_end(ret);
            self.builder.build_ret_void();
        }

        // --- String.reserve ---
        {
            let f = body("kelp_String__reserve")?;
            let self_p = f.get_param(0);
            let extra = f.get_param(1);
            self.emit_grow_call(f, string, self_p, extra)?;
            self.builder.build_ret_void();
        }

        // --- clear, for both buffers ---
        for (symbol, agg) in [
            ("kelp_String__clear", string),
            ("kelp_Bytes__clear", bytes),
        ] {
            let f = body(symbol)?;
            let self_p = f.get_param(0);
            self.st(self.v_i64(0), self.fld(agg, self_p, L_LEN, "len"));
            let d = data_of(agg, self_p);
            let no_data = self
                .builder
                .build_icmp(IntCmp::Eq, d, self.ctx.const_null(ptr), "no.data");
            let set = self.block(f, "clear.set");
            let done = self.block(f, "clear.done");
            self.builder.build_cond_br(no_data, done, set);
            self.builder.position_at_end(set);
            self.st(self.v_i8(0), d);
            self.builder.build_br(done);
            self.builder.position_at_end(done);
            self.builder.build_ret_void();
        }

        // --- View.to_string ---
        // The one safe conversion from a slice to an owned `String`: copy the
        // bytes and terminate the copy. Only emitted with an allocator, which
        // is when the front end admits `String` at all and when this body's
        // `heap_alloc` has the project's own function behind it.
        {
            let f = body("kelp_View__to_string")?;
            let dst = f.get_param(0);
            let self_v = f.get_param(1);
            let n = self.builder.build_extract_value(self_v, 1, "len");
            let src = self.builder.build_extract_value(self_v, 0, "ptr");
            let n1 = self
                .builder
                .build_int_binop(IntBinOp::Add, n, self.v_i64(1), "n1");
            let p = self.heap_alloc(n1, 1)?;
            self.check_alloc(p, f)?;
            let (memcpy, mty) = self.mem_fn("memcpy")?;
            self.call(memcpy, mty, &[p, src, n], "");
            let term = self.gep(i8t, p, n, "term");
            self.st(self.v_i8(0), term);
            self.st(p, self.fld(string, dst, L_DATA, "data"));
            self.st(n, self.fld(string, dst, L_LEN, "len"));
            self.st(n, self.fld(string, dst, L_CAP, "cap"));
            self.builder.build_ret_void();
        }

        // --- Bytes.get / Bytes.set ---
        {
            let f = body("kelp_Bytes__get")?;
            let self_p = f.get_param(0);
            let i = f.get_param(1);
            let n = len_of(bytes, self_p);
            self.emit_bounds_check(f, n, i, "Bytes.get: index out of bounds")?;
            let at = self.gep(i8t, data_of(bytes, self_p), i, "at");
            let byte = self.ld(i8t, at, "byte");
            self.builder.build_ret(byte);
        }
        {
            let f = body("kelp_Bytes__set")?;
            let self_p = f.get_param(0);
            let i = f.get_param(1);
            let value = f.get_param(2);
            let n = len_of(bytes, self_p);
            self.emit_bounds_check(f, n, i, "Bytes.set: index out of bounds")?;
            let at = self.gep(i8t, data_of(bytes, self_p), i, "at");
            self.st(value, at);
            self.builder.build_ret_void();
        }

        // --- kelp_string_drop / kelp_bytes_drop ---
        for (symbol, agg) in [
            ("kelp_string_drop", string),
            ("kelp_bytes_drop", bytes),
        ] {
            let f = body(symbol)?;
            let self_p = f.get_param(0);
            let d = data_of(agg, self_p);
            let cap = cap_of(agg, self_p);
            let size = self
                .builder
                .build_int_binop(IntBinOp::Add, cap, self.v_i64(1), "size");
            let no_data = self
                .builder
                .build_icmp(IntCmp::Eq, d, self.ctx.const_null(ptr), "no.data");
            let release = self.block(f, "drop.free");
            let done = self.block(f, "drop.done");
            self.builder.build_cond_br(no_data, done, release);
            self.builder.position_at_end(release);
            self.heap_dealloc(d, size, 1)?;
            self.builder.build_br(done);
            self.builder.position_at_end(done);
            self.builder.build_ret_void();
        }
        Ok(())
    }

    /// Emit the five libc memory routines a freestanding image needs and that
    /// LLVM's own lowering of a large aggregate copy assumes: `memcpy`,
    /// `memmove`, `memset`, `memcmp` and `bcmp`, as byte loops with no libc
    /// behind them (kernel gap 4). Until this, a copy of a large aggregate left
    /// an undefined `memcpy` for `ld.lld` to report.
    ///
    /// Each is weak (`Value::set_weak_linkage`) so a project or another object
    /// in the image that defines its own wins at the link, and `nobuiltin`
    /// (`Function::mark_nobuiltin`) so the target's loop-idiom pass cannot
    /// rewrite a loop *inside* one of these into a call to the library function
    /// of the same name, which for three of them would be the function itself.
    ///
    /// `memcpy`, `memset` and `memcmp` are already declared by `declare_libc`
    /// with the exact signatures the compiler's own aggregate copies call, so
    /// their bodies are added to those declarations; `memmove` and `bcmp` are
    /// new. The width is the `i64` those declarations already use. Getting the
    /// name right is what makes the loops safe: LLVM's loop-idiom pass refuses
    /// to turn a loop into a call to the function the loop is already in, so a
    /// definition named `memset` survives while one named `memset.1` does not
    /// (`Module::add_function` renames rather than returning an existing
    /// declaration, so the declaration has to be fetched, not added again).
    pub(crate) fn emit_freestanding_mem_bodies(&self) -> Result<(), String> {
        let i8t = self.ctx.i8();
        let i32t = self.ctx.i32();
        let i64t = self.ctx.i64();
        let ptr = self.pty();
        let size_t = i64t;
        let zero = self.ctx.const_int(size_t, 0, false);
        let one = self.ctx.const_int(size_t, 1, false);

        // The definition of `name`: the declaration `declare_libc` already made
        // when there is one, made weak, marked as not-a-builtin, and given a
        // body by the caller.
        let def = |name: &str, ret: Type<'a>, params: &[Type<'a>]| -> (LlvmFunction<'a>, FnType<'a>) {
            let ty = self.ctx.fn_type(ret, params, false);
            let func = match self.module.get_function(name) {
                Some(func) => func,
                None => self.module.add_function(name, ty),
            };
            func.as_value().set_weak_linkage();
            func.mark_nobuiltin();
            (func, ty)
        };

        // --- memcpy: forward byte copy, returns dst ---
        {
            let (func, _) = def("memcpy", ptr, &[ptr, ptr, size_t]);
            let entry = func.append_block("entry");
            self.builder.position_at_end(entry);
            let dst = func.get_param(0);
            let src = func.get_param(1);
            let n = func.get_param(2);
            let i_slot = self.builder.build_alloca(size_t, "i");
            self.st(zero, i_slot);
            let header = self.loop_header(func, "copy.cond");
            let i = self.ld(size_t, i_slot, "i");
            let more = self.builder.build_icmp(IntCmp::Ult, i, n, "more");
            let body = self.block(func, "copy.body");
            let done = self.block(func, "copy.done");
            self.builder.build_cond_br(more, body, done);
            self.builder.position_at_end(body);
            let byte = self.ld(i8t, self.gep(i8t, src, i, "src.at"), "byte");
            self.st(byte, self.gep(i8t, dst, i, "dst.at"));
            self.st(self.builder.build_int_binop(IntBinOp::Add, i, one, "i1"), i_slot);
            self.builder.build_br(header);
            self.builder.position_at_end(done);
            self.builder.build_ret(dst);
        }

        // --- memmove: forward when the destination is at or below the source,
        // backward otherwise, which is the direction that cannot overwrite a
        // byte it has not read yet ---
        {
            let (func, _) = def("memmove", ptr, &[ptr, ptr, size_t]);
            let entry = func.append_block("entry");
            self.builder.position_at_end(entry);
            let dst = func.get_param(0);
            let src = func.get_param(1);
            let n = func.get_param(2);
            let d = self.builder.build_ptrtoint(dst, size_t);
            let s = self.builder.build_ptrtoint(src, size_t);
            let i_slot = self.builder.build_alloca(size_t, "i");
            let forward = self.block(func, "fwd");
            let backward = self.block(func, "bwd");
            let at_or_below = self.builder.build_icmp(IntCmp::Ule, d, s, "at.or.below");
            self.builder.build_cond_br(at_or_below, forward, backward);

            self.builder.position_at_end(forward);
            self.st(zero, i_slot);
            let fwd_header = self.loop_header(func, "fwd.cond");
            let i = self.ld(size_t, i_slot, "i");
            let more = self.builder.build_icmp(IntCmp::Ult, i, n, "more");
            let fwd_body = self.block(func, "fwd.body");
            let fwd_done = self.block(func, "fwd.done");
            self.builder.build_cond_br(more, fwd_body, fwd_done);
            self.builder.position_at_end(fwd_body);
            let byte = self.ld(i8t, self.gep(i8t, src, i, "src.at"), "byte");
            self.st(byte, self.gep(i8t, dst, i, "dst.at"));
            self.st(self.builder.build_int_binop(IntBinOp::Add, i, one, "i1"), i_slot);
            self.builder.build_br(fwd_header);
            self.builder.position_at_end(fwd_done);
            self.builder.build_ret(dst);

            self.builder.position_at_end(backward);
            self.st(n, i_slot);
            let bwd_header = self.loop_header(func, "bwd.cond");
            let i = self.ld(size_t, i_slot, "i");
            let any = self.builder.build_icmp(IntCmp::Ugt, i, zero, "any");
            let bwd_body = self.block(func, "bwd.body");
            let bwd_done = self.block(func, "bwd.done");
            self.builder.build_cond_br(any, bwd_body, bwd_done);
            self.builder.position_at_end(bwd_body);
            let i1 = self.builder.build_int_binop(IntBinOp::Sub, i, one, "i1");
            self.st(i1, i_slot);
            let byte = self.ld(i8t, self.gep(i8t, src, i1, "src.at"), "byte");
            self.st(byte, self.gep(i8t, dst, i1, "dst.at"));
            self.builder.build_br(bwd_header);
            self.builder.position_at_end(bwd_done);
            self.builder.build_ret(dst);
        }

        // --- memset: forward byte store, returns dst ---
        {
            let (func, _) = def("memset", ptr, &[ptr, i32t, size_t]);
            let entry = func.append_block("entry");
            self.builder.position_at_end(entry);
            let dst = func.get_param(0);
            let fill = self
                .builder
                .build_int_cast(false, false, func.get_param(1), i8t, "fill");
            let n = func.get_param(2);
            let i_slot = self.builder.build_alloca(size_t, "i");
            self.st(zero, i_slot);
            let header = self.loop_header(func, "set.cond");
            let i = self.ld(size_t, i_slot, "i");
            let more = self.builder.build_icmp(IntCmp::Ult, i, n, "more");
            let body = self.block(func, "set.body");
            let done = self.block(func, "set.done");
            self.builder.build_cond_br(more, body, done);
            self.builder.position_at_end(body);
            self.st(fill, self.gep(i8t, dst, i, "dst.at"));
            self.st(self.builder.build_int_binop(IntBinOp::Add, i, one, "i1"), i_slot);
            self.builder.build_br(header);
            self.builder.position_at_end(done);
            self.builder.build_ret(dst);
        }

        // --- memcmp and bcmp: the first differing byte's difference, 0 when
        // the ranges match. The byte difference is what `memcmp` promises and
        // it is a valid `bcmp` answer too (any nonzero is "different") ---
        for name in ["memcmp", "bcmp"] {
            let (func, _) = def(name, i32t, &[ptr, ptr, size_t]);
            let entry = func.append_block("entry");
            self.builder.position_at_end(entry);
            let a = func.get_param(0);
            let b = func.get_param(1);
            let n = func.get_param(2);
            let i_slot = self.builder.build_alloca(size_t, "i");
            self.st(zero, i_slot);
            let header = self.loop_header(func, "cmp.cond");
            let i = self.ld(size_t, i_slot, "i");
            let more = self.builder.build_icmp(IntCmp::Ult, i, n, "more");
            let body = self.block(func, "cmp.body");
            let equal = self.block(func, "cmp.equal");
            self.builder.build_cond_br(more, body, equal);
            self.builder.position_at_end(body);
            let x = self.ld(i8t, self.gep(i8t, a, i, "a.at"), "x");
            let y = self.ld(i8t, self.gep(i8t, b, i, "b.at"), "y");
            let differs = self.builder.build_icmp(IntCmp::Ne, x, y, "differs");
            let step = self.block(func, "cmp.step");
            let differ = self.block(func, "cmp.differ");
            self.builder.build_cond_br(differs, differ, step);
            self.builder.position_at_end(differ);
            let wx = self.builder.build_int_cast(false, true, x, i32t, "x.wide");
            let wy = self.builder.build_int_cast(false, true, y, i32t, "y.wide");
            let delta = self.builder.build_int_binop(IntBinOp::Sub, wx, wy, "delta");
            self.builder.build_ret(delta);
            self.builder.position_at_end(step);
            self.st(self.builder.build_int_binop(IntBinOp::Add, i, one, "i1"), i_slot);
            self.builder.build_br(header);
            self.builder.position_at_end(equal);
            self.builder.build_ret(self.ctx.const_int(i32t, 0, false));
        }
        Ok(())
    }
}
