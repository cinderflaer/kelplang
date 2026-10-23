use super::*;

impl<'g, 'a> FnGen<'g, 'a> {
    /// The call one variadic user `extern fun` call site makes (gap 7): the
    /// generated fixed-arity wrapper, never the native symbol itself. The
    /// wrapper is declared like any other extern (`declare_extern`), so its
    /// `RtFn` is in the runtime table under the wrapper's own symbol, and the
    /// capability plan still reads the extern's key - that is what carries the
    /// domain and the argument positions the grant governs.
    pub(crate) fn emit_variadic_call(
        &mut self,
        callee: &str,
        wrapper: &str,
        args: &[Expr],
        span: Span,
    ) -> Result<Option<Value<'a>>, String> {
        let plan = self.g.cap_plan(&self.module, callee, args, span);
        let prepared = self.prepare_checked_args(args, plan.as_ref())?;
        let rt = self
            .g
            .runtime
            .get(&symbol_name(wrapper))
            .copied()
            .ok_or_else(|| no_value(&format!("the variadic wrapper '{wrapper}'")))?;
        self.call_runtime_entry(rt, args, &prepared, plan.as_ref())
    }

    /// The runtime-table call itself: one extern body compiled into the
    /// per-program object, or one runtime helper. Shared by a plain
    /// extern call and a variadic wrapper call, so the two cannot disagree
    /// about how a 24-byte return or a checked argument crosses.
    pub(crate) fn call_runtime_entry(
        &mut self,
        rt: RtFn<'a>,
        args: &[Expr],
        prepared: &HashMap<usize, Value<'a>>,
        plan: Option<&CapPlan>,
    ) -> Result<Option<Value<'a>>, String> {
        let mut held: Vec<(Value<'a>, Ty)> = Vec::new();
        if rt.sret {
            let ret_ty = rt
                .ret
                .ok_or_else(|| no_value("the return type of an sret helper"))?;
            let slot = self.temp_alloca(ret_ty, "sret.out");
            let mut values: Vec<Value<'a>> = Vec::with_capacity(args.len() + 1);
            values.push(slot);
            for (index, a) in args.iter().enumerate() {
                match prepared.get(&index) {
                    Some(v) => values.push(*v),
                    None => values.push(self.rt_arg_value(a, &mut held)?),
                }
            }
            self.emit_cap_checks(plan, prepared)?;
            self.g.builder.build_call(rt.func, rt.ty, &values, "");
            self.free_held(held)?;
            return Ok(Some(self.g.builder.build_load(ret_ty, slot, "sret.val")));
        }
        let mut values: Vec<Value<'a>> = Vec::with_capacity(args.len());
        for (index, a) in args.iter().enumerate() {
            match prepared.get(&index) {
                Some(v) => values.push(*v),
                None => values.push(self.rt_arg_value(a, &mut held)?),
            }
        }
        self.emit_cap_checks(plan, prepared)?;
        let name = if rt.yields { "call" } else { "" };
        let v = self.g.builder.build_call(rt.func, rt.ty, &values, name);
        self.free_held(held)?;
        Ok(if rt.yields { Some(v) } else { None })
    }

    pub(crate) fn emit_conversion(
        &mut self,
        callee: &str,
        from: &Ty,
        to: &Ty,
        arg: &Expr,
    ) -> Result<Value<'a>, String> {
        let v = self.value_of(arg)?;
        let to_ty = self.lty(to)?;

        // Integer to float: `int_bounds` has no bounds for a float target, so
        // `numeric_conversion` treats it as a plain cast, and so does this.
        if to.is_float() {
            return Ok(self
                .g
                .builder
                .build_int_to_fp(is_signed(from), v, to_ty, "to_f"));
        }

        let (Some((s_min, s_max)), Some((t_min, t_max))) =
            (Ty::int_bounds(from), Ty::int_bounds(to))
        else {
            return Err(no_value("an integer-to-integer conversion"));
        };

        // A widening (or a same-width re-interpretation whose source range fits
        // the target) is a plain cast.
        if s_min >= t_min && s_max <= t_max {
            return Ok(self.cast_int(from, to, v)?);
        }

        let from_ty = self.lty(from)?;
        let signed = is_signed(from);
        let mut checks: Vec<Value<'a>> = Vec::new();
        // Only the bounds the source's own range cannot rule out, as
        // `numeric_conversion` does: a check that cannot fail is not emitted.
        if s_min < t_min {
            let bound = self.g.ctx.const_int(from_ty, t_min as u64, false);
            checks.push(self.g.builder.build_icmp(
                if signed { IntCmp::Slt } else { IntCmp::Ult },
                v,
                bound,
                "lo",
            ));
        }
        if s_max > t_max {
            let bound = self.g.ctx.const_int(from_ty, t_max as u64, false);
            checks.push(self.g.builder.build_icmp(
                if signed { IntCmp::Sgt } else { IntCmp::Ugt },
                v,
                bound,
                "hi",
            ));
        }
        let cond = checks
            .into_iter()
            .reduce(|a, b| self.g.builder.build_int_binop(IntBinOp::Or, a, b, "oor"))
            .expect("at least one bound is missing when the ranges do not fit");

        let panic_bb = self.cur_fn.append_block("range.panic");
        let ok_bb = self.cur_fn.append_block("range.ok");
        self.g.builder.build_cond_br(cond, panic_bb, ok_bb);

        // The failing arm panics with the `.kp` spelling of the call, the same
        // text `numeric_conversion` passes to `kelp_panic`.
        self.g.builder.position_at_end(panic_bb);
        self.cur = panic_bb;
        self.terminated = false;
        let msg = format!("{}: value out of range", callee.replace('#', "."));
        let msg_ptr = self.string_global(&msg)?;
        self.g.emit_panic_ptr(msg_ptr);

        self.g.builder.position_at_end(ok_bb);
        self.cur = ok_bb;
        self.terminated = false;
        self.cast_int(from, to, v)
    }

    /// The length function of the container an element access `symbol` (ending
    /// in `__get` or `__set`) reads: the monomorphized `<base>__len` of a
    /// `List`, or the runtime's `<base>__len` for a `Bytes` (`runtime`). `None`
    /// for any other symbol, which keeps the plain call.
    pub(crate) fn fused_len_fn(&self, symbol: &str, runtime: bool) -> Option<(LlvmFunction<'a>, FnType<'a>)> {
        let base = symbol
            .strip_suffix("__get")
            .or_else(|| symbol.strip_suffix("__set"))?;
        let len = format!("{base}__len");
        if runtime {
            self.g.runtime.get(&len).map(|rt| (rt.func, rt.ty))
        } else {
            self.g.container_fns.get(&len).copied()
        }
    }

    /// `list.get(x.to_usize())` / `list.set(x.to_usize(), v)` (and the `Bytes`
    /// forms) for a signed `x`, with the conversion's sign check and the
    /// container's bounds check fused into one branch on the unsigned index.
    ///
    /// A negative `x` is an unsigned value at least the length, so it is out of
    /// bounds and takes the slow arm, where the sign is tested: the message is
    /// the conversion's for a negative index and the container's for a large
    /// one. The in-bounds arm calls the container function, whose own check the
    /// branch has already decided, so LLVM removes it. The slow arm calls it too,
    /// which panics (`kelp_panic` does not return) and ends the block.
    ///
    /// The evaluation order is the call's: the container, then `x`, then (for
    /// `set`) the value, then the access. The value is read-only (`fused_index_call`
    /// admits no other), so emitting it in each arm changes no state and keeps
    /// its own panics in the order they had: after the conversion's panic, and
    /// before the container's.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn emit_indexed_fused(
        &mut self,
        len: (LlvmFunction<'a>, FnType<'a>),
        call: (LlvmFunction<'a>, FnType<'a>),
        runtime_abi: bool,
        args: &[Expr],
        ret: &Ty,
        from: &Ty,
        conv: &str,
        inner: &Expr,
    ) -> Result<Option<Value<'a>>, String> {
        let (len_fn, len_ty) = len;
        let mut held: Vec<(Value<'a>, Ty)> = Vec::new();
        let container = if runtime_abi {
            self.rt_arg_value(&args[0], &mut held)?
        } else {
            self.arg_value(&args[0], &mut held)?
        };
        let x = self.value_of(inner)?;
        let n = self.g.builder.build_call(len_fn, len_ty, &[container], "len");
        let u = self.cast_int(from, &Ty::Usize, x)?;
        let oob = self.g.builder.build_icmp(IntCmp::Uge, u, n, "index.oob");
        let slow = self.cur_fn.append_block("index.slow");
        let fast = self.cur_fn.append_block("index.in");
        self.g.builder.build_cond_br(oob, slow, fast);

        // Out of bounds: a negative index is the conversion's panic, anything
        // else reaches the container's own check through the call and panics.
        self.g.builder.position_at_end(slow);
        self.cur = slow;
        self.terminated = false;
        let from_ty = self.lty(from)?;
        let zero = self.g.ctx.const_int(from_ty, 0, false);
        let neg = self.g.builder.build_icmp(IntCmp::Slt, x, zero, "index.neg");
        let conv_bb = self.cur_fn.append_block("index.conv");
        let rest = self.cur_fn.append_block("index.list");
        self.g.builder.build_cond_br(neg, conv_bb, rest);

        // The same text `emit_conversion` panics with for this conversion.
        self.g.builder.position_at_end(conv_bb);
        self.cur = conv_bb;
        let msg = format!("{}: value out of range", conv.replace('#', "."));
        let msg_ptr = self.string_global(&msg)?;
        self.g.emit_panic_ptr(msg_ptr);

        self.g.builder.position_at_end(rest);
        self.cur = rest;
        let mut slow_held: Vec<(Value<'a>, Ty)> = Vec::new();
        let slow_values = self.indexed_call_args(args, runtime_abi, container, u, &mut slow_held)?;
        let (func, fty) = call;
        self.g.builder.build_call(func, fty, &slow_values, "");
        self.g.builder.build_unreachable();

        // In bounds: the same arguments and the call, whose check folds away.
        self.g.builder.position_at_end(fast);
        self.cur = fast;
        self.terminated = false;
        let mut held: Vec<(Value<'a>, Ty)> = Vec::new();
        let values = self.indexed_call_args(args, runtime_abi, container, u, &mut held)?;
        let yields_value = *ret != Ty::Unit;
        let name = if yields_value { "call" } else { "" };
        let v = self.g.builder.build_call(func, fty, &values, name);
        self.free_held(held)?;
        Ok(if yields_value { Some(v) } else { None })
    }

    /// The argument values of an indexed container call once the container and
    /// the index are known: the container, the unsigned index, then the rest
    /// (the value of a `set`), evaluated the way the call evaluates them.
    pub(crate) fn indexed_call_args(
        &mut self,
        args: &[Expr],
        runtime_abi: bool,
        container: Value<'a>,
        index: Value<'a>,
        held: &mut Vec<(Value<'a>, Ty)>,
    ) -> Result<Vec<Value<'a>>, String> {
        let mut values: Vec<Value<'a>> = Vec::with_capacity(args.len());
        values.push(container);
        values.push(index);
        for a in &args[2..] {
            values.push(if runtime_abi {
                self.rt_arg_value(a, held)?
            } else {
                self.arg_value(a, held)?
            });
        }
        Ok(values)
    }

    /// Cast an integer value to `to`, with no range check: the caller has already
    /// decided the value fits. A same-width conversion is no instruction.
    pub(crate) fn cast_int(&self, from: &Ty, to: &Ty, v: Value<'a>) -> Result<Value<'a>, String> {
        let fw = int_width(from).ok_or_else(|| no_value("an integer source type"))?;
        let tw = int_width(to).ok_or_else(|| no_value("an integer target type"))?;
        if fw == tw {
            return Ok(v);
        }
        let to_ty = self.lty(to)?;
        Ok(self
            .g
            .builder
            .build_int_cast(is_signed(from), tw > fw, v, to_ty, "cast"))
    }

    /// The `print` path for one value: `printf` with the specifier and promotion
    /// the printf piece rule uses, so stdout is byte-identical. An
    /// owned temporary is freed right after the `printf`, exactly where the C
    /// backend frees it.
    pub(crate) fn emit_template_print(&mut self, newline: bool, arg: &Expr) -> Result<(), String> {
        let (spec, mut values, holds) = self.printf_piece(arg)?;
        let fmt = if newline {
            format!("{spec}\n")
        } else {
            spec
        };
        let fmt_ptr = self.string_global(&fmt)?;
        let mut all = Vec::with_capacity(values.len() + 1);
        all.push(fmt_ptr);
        all.append(&mut values);
        self.g
            .builder
            .build_call(self.g.printf_fn, self.g.printf_ty, &all, "");
        for (slot, ty) in holds {
            self.g.call_drop(slot, &ty)?;
        }
        Ok(())
    }

    /// A template: one `printf` whose format string is the literal chunks (with
    /// `%` doubled) plus each interpolated value's specifier. The earlier emitter
    /// builds the same format string in the earlier emitter's `Template` arm; an
    /// interpolated owned temporary is freed after the `printf`, as `print`'s is.
    pub(crate) fn emit_template(
        &mut self,
        newline: bool,
        pieces: &[TemplatePiece],
    ) -> Result<(), String> {
        let (fmt, args, holds) = self.template_format(newline, pieces)?;
        let fmt_ptr = self.string_global(&fmt)?;
        let mut all = Vec::with_capacity(args.len() + 1);
        all.push(fmt_ptr);
        all.extend(args);
        self.g
            .builder
            .build_call(self.g.printf_fn, self.g.printf_ty, &all, "");
        for (slot, ty) in holds {
            self.g.call_drop(slot, &ty)?;
        }
        Ok(())
    }

    /// A template as a `String` value: `snprintf` twice, once to measure the
    /// text and once into a buffer of that size owned by a new `String`. The
    /// format and its arguments are the ones `print` would use, so the text is
    /// the printed text without the newline. Owned interpolated temporaries are
    /// freed after the second call, as the `printf` form frees them.
    pub(crate) fn emit_string_template(&mut self, pieces: &[TemplatePiece]) -> Result<Value<'a>, String> {
        let (fmt, args, holds) = self.template_format(false, pieces)?;
        let fmt_ptr = self.string_global(&fmt)?;
        let ptr = self.g.ctx.opaque_ptr();
        let i64t = self.g.ctx.i64();
        // Declared here, on first use, so a module without a String template
        // keeps the IR it had before: `snprintf` is variadic, as `printf` is.
        let snprintf_ty = self.g.ctx.fn_type(self.g.ctx.i32(), &[ptr, i64t], true);
        let snprintf = match self.g.module.get_function("snprintf") {
            Some(f) => f,
            None => self.g.module.add_function("snprintf", snprintf_ty),
        };
        let null = self.g.ctx.const_null(ptr);
        let zero = self.g.ctx.const_int(i64t, 0, false);
        let one = self.g.ctx.const_int(i64t, 1, false);

        let mut measure = vec![null, zero, fmt_ptr];
        measure.extend(args.iter().copied());
        let n32 = self
            .g
            .builder
            .build_call(snprintf, snprintf_ty, &measure, "n");
        let n = self
            .g
            .builder
            .build_int_cast(true, true, n32, i64t, "n64");
        let cap = self
            .g
            .builder
            .build_int_binop(IntBinOp::Add, n, one, "cap");

        let lty = self.lty(&Ty::String)?;
        let slot = self.temp_alloca(lty, "tmpl.str");
        self.g.sret_call("kelp_String__with_capacity", slot, &[cap])?;
        let (data, _) = self.buffer_pair(slot, lty);

        let mut fill = vec![data, cap, fmt_ptr];
        fill.extend(args);
        self.g
            .builder
            .build_call(snprintf, snprintf_ty, &fill, "");
        let len_ptr = self.g.builder.build_struct_gep(lty, slot, 1, "tmpl.len");
        self.g.builder.build_store(n, len_ptr);
        self.free_held(holds)?;
        Ok(self.g.builder.build_load(lty, slot, "tmpl"))
    }

    /// The printf format of a template and its promoted arguments, with the
    /// owned temporaries each interpolated value holds. Literal `%` is doubled.
    pub(crate) fn template_format(
        &mut self,
        newline: bool,
        pieces: &[TemplatePiece],
    ) -> Result<(String, Vec<Value<'a>>, Vec<(Value<'a>, Ty)>), String> {
        let mut fmt = String::new();
        let mut args: Vec<Value<'a>> = Vec::new();
        let mut holds: Vec<(Value<'a>, Ty)> = Vec::new();
        for piece in pieces {
            match piece {
                TemplatePiece::Text(text) => {
                    for c in text.chars() {
                        if c == '%' {
                            fmt.push_str("%%");
                        } else {
                            fmt.push(c);
                        }
                    }
                }
                TemplatePiece::Value(value) => {
                    let (spec, mut values, piece_holds) = self.printf_piece(value)?;
                    fmt.push_str(&spec);
                    args.append(&mut values);
                    holds.extend(piece_holds);
                }
            }
        }
        if newline {
            fmt.push('\n');
        }
        Ok((fmt, args, holds))
    }

    /// The `data` pointer of a value `panic` reads as text: a `str`/`&str` is
    /// already the pointer, an owned `String` (or a borrow of one) contributes
    /// the first field of its buffer. This is the IR twin of the earlier emitter's
    /// `panic` argument staging, minus the rows it does not reach.
    pub(crate) fn heap_data_ptr(&mut self, arg: &Expr) -> Result<Value<'a>, String> {
        match &arg.ty {
            Ty::Str => self.value_of(arg),
            Ty::Ref { inner, .. } if matches!(**inner, Ty::Str) => self.value_of(arg),
            Ty::String => {
                let v = self.value_of(arg)?;
                let lty = self.lty(&arg.ty)?;
                let slot = self.temp_alloca(lty, "panic.msg");
                self.g.builder.build_store(v, slot);
                let (data, _) = self.buffer_pair(slot, lty);
                Ok(data)
            }
            Ty::Ref { inner, .. } if matches!(**inner, Ty::String) => {
                // A borrow of a field of an owned temporary (`panic(mk().msg)`)
                // stages the temporary the way every other borrow argument
                // does. Nothing is freed afterwards: a panic does not return, so
                // the drop would be unreachable (the earlier emitter leaves it out for
                // the same reason).
                let ptr = self.borrow_place(arg, &mut Vec::new())?;
                let lty = self.lty(inner)?;
                let (data, _) = self.buffer_pair(ptr, lty);
                Ok(data)
            }
            other => Err(refuse_type(other)),
        }
    }

    /// The `(data, len)` pair of a heap value or view, read out of the struct at
    /// `ptr`. Field 0 is the pointer and field 1 the length in all three builtin
    /// layouts, which is what lets one routine serve `Bytes`, `String` and
    /// `View`.
    pub(crate) fn buffer_pair(&self, ptr: Value<'a>, struct_ty: Type<'a>) -> (Value<'a>, Value<'a>) {
        let data_ptr = self.g.builder.build_struct_gep(struct_ty, ptr, 0, "data");
        let data = self
            .g
            .builder
            .build_load(self.g.ctx.opaque_ptr(), data_ptr, "data");
        let len_ptr = self.g.builder.build_struct_gep(struct_ty, ptr, 1, "len");
        let len = self
            .g
            .builder
            .build_load(self.g.ctx.i64(), len_ptr, "len");
        (data, len)
    }

    /// If `arg` is a heap buffer or a view, its `(data, len)` pair and the
    /// temporary the caller must free afterwards. `None` for anything else, which
    /// then takes the scalar path.
    pub(crate) fn buffer_fields(
        &mut self,
        arg: &Expr,
    ) -> Result<Option<(Value<'a>, Value<'a>, Vec<(Value<'a>, Ty)>)>, String> {
        match &arg.ty {
            Ty::Bytes | Ty::String => {
                let v = self.value_of(arg)?;
                let lty = self.lty(&arg.ty)?;
                let slot = self.temp_alloca(lty, "buf.tmp");
                self.g.builder.build_store(v, slot);
                let (data, len) = self.buffer_pair(slot, lty);
                Ok(Some((data, len, vec![(slot, arg.ty.clone())])))
            }
            Ty::View => {
                let v = self.value_of(arg)?;
                let lty = self.lty(&arg.ty)?;
                let slot = self.temp_alloca(lty, "view.tmp");
                self.g.builder.build_store(v, slot);
                let (data, len) = self.buffer_pair(slot, lty);
                Ok(Some((data, len, Vec::new())))
            }
            Ty::Ref { inner, .. }
                if matches!(**inner, Ty::Bytes | Ty::String | Ty::View) =>
            {
                let mut held: Vec<(Value<'a>, Ty)> = Vec::new();
                let ptr = self.borrow_place(arg, &mut held)?;
                let lty = self.lty(inner)?;
                let (data, len) = self.buffer_pair(ptr, lty);
                Ok(Some((data, len, held)))
            }
            _ => Ok(None),
        }
    }

    /// The format specifier and the already-promoted variadic arguments for one
    /// value, mirroring the printf piece rule exactly. A heap buffer
    /// or a view contributes `%.*s` with an `int` length and a pointer, so the
    /// specifier comes back with two arguments; the third slot carries a
    /// temporary the caller frees after the `printf`.
    pub(crate) fn printf_piece(
        &mut self,
        arg: &Expr,
    ) -> Result<(String, Vec<Value<'a>>, Vec<(Value<'a>, Ty)>), String> {
        if let Some((data, len, holds)) = self.buffer_fields(arg)? {
            let len = self
                .g
                .builder
                .build_int_cast(false, false, len, self.g.ctx.i32(), "len");
            return Ok(("%.*s".to_string(), vec![len, data], holds));
        }
        let v = self.value_of(arg)?;
        let spec: &'static str = match &arg.ty {
            Ty::I32 => "%d",
            Ty::I64 | Ty::Int => "%lld",
            Ty::U32 => "%u",
            Ty::U64 | Ty::Uint => "%llu",
            // A vararg narrower than `int` must be promoted here: LLVM does not,
            // unlike C, whose default argument promotions do it for free.
            Ty::U8 | Ty::U16 => {
                let promoted = self
                    .g
                    .builder
                    .build_int_cast(false, true, v, self.g.ctx.i32(), "promote");
                return Ok(("%u".to_string(), vec![promoted], Vec::new()));
            }
            Ty::Usize => "%zu",
            Ty::Isize => "%lld",
            // `%f` reads a `double` out of the vector file, so an `f32` is
            // promoted, exactly as C's default argument promotions do.
            Ty::F32 => {
                let promoted = self
                    .g
                    .builder
                    .build_fp_cast(true, v, self.g.ctx.f64(), "promote");
                return Ok(("%f".to_string(), vec![promoted], Vec::new()));
            }
            Ty::F64 | Ty::Float => "%f",
            Ty::Bool => {
                let t = self.string_global("true")?;
                let f = self.string_global("false")?;
                let selected = self.g.builder.build_select(v, t, f, "bool");
                return Ok(("%s".to_string(), vec![selected], Vec::new()));
            }
            Ty::Char => "%c",
            Ty::Str => "%s",
            Ty::Ref { inner, .. } if matches!(**inner, Ty::Str) => "%s",
            other => return Err(refuse_type(other)),
        };
        Ok((spec.to_string(), vec![v], Vec::new()))
    }

    pub(crate) fn all_ones(&self, ty: &Ty) -> Result<Value<'a>, String> {
        let bits = int_width(ty).ok_or_else(|| refuse_type(ty))?;
        let mask = if bits >= 64 { u64::MAX } else { (1u64 << bits) - 1 };
        Ok(self.g.ctx.const_int(self.lty(ty)?, mask, false))
    }

    pub(crate) fn zero_const(&self, ty: &Ty) -> Result<Value<'a>, String> {
        if ty.is_float() {
            Ok(self.g.ctx.const_real(self.lty(ty)?, 0.0))
        } else if *ty == Ty::Bool {
            Ok(self.g.ctx.const_bool(false))
        } else if ty.is_integer() || *ty == Ty::Char {
            Ok(self.g.ctx.const_int(self.lty(ty)?, 0, false))
        } else if matches!(ty, Ty::Struct(_)) {
            // Only reached for a value the frontend already guarantees is
            // present on every path that reads it, so `undef` is never observed.
            Ok(self.g.ctx.undef(self.lty(ty)?))
        } else {
            Err(refuse_type(ty))
        }
    }

    /// Unlinks this function's cleanup frame before a `ret` (decision 125). The
    /// frame's own drops have run by then; a `ret` with no frame emits nothing.
    pub(crate) fn leave_frame(&mut self) -> Result<(), String> {
        if let Some(frame) = self.frame {
            let leave = self.g.rt("kelp_cleanup_leave")?;
            self.g.call(leave.func, leave.ty, &[frame], "");
        }
        Ok(())
    }

    /// The terminator for a function that reached the end of its body without a
    /// `return`: `main` returns 0 (mirroring the earlier emitter's rewrite of a bare
    /// `return;` in `main`), a unit function returns nothing, and a value-returning
    /// function (which `infer.rs` does not let fall through) returns zero.
    pub(crate) fn emit_fallthrough_return(&mut self) -> Result<(), String> {
        self.leave_frame()?;
        if self.fn_name == "main" {
            self.g
                .builder
                .build_ret(self.g.ctx.const_int(self.g.ctx.i32(), 0, false));
        } else if self.ret_ty == Ty::Unit {
            self.g.builder.build_ret_void();
        } else {
            let zero = self.zero_const(&self.ret_ty)?;
            self.g.builder.build_ret(zero);
        }
        self.terminated = true;
        Ok(())
    }
}
