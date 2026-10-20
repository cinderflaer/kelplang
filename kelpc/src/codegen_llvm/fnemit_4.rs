use super::*;

impl<'g, 'a> FnGen<'g, 'a> {
    /// `a && b` / `a || b` as two blocks and a phi, so the right operand is
    /// evaluated only when the left did not decide, matching C's `&&`/`||`.
    pub(crate) fn emit_logical(
        &mut self,
        lhs: &Expr,
        rhs: &Expr,
        is_and: bool,
    ) -> Result<Value<'a>, String> {
        let l = self.value_of(lhs)?;
        let rhs_bb = self.cur_fn.append_block("logic.rhs");
        let short_bb = self.cur_fn.append_block("logic.short");
        let merge_bb = self.cur_fn.append_block("logic.end");
        if is_and {
            self.g.builder.build_cond_br(l, rhs_bb, short_bb);
        } else {
            self.g.builder.build_cond_br(l, short_bb, rhs_bb);
        }

        self.g.builder.position_at_end(rhs_bb);
        self.cur = rhs_bb;
        self.terminated = false;
        let r = self.value_of(rhs)?;
        let rhs_end = self.cur;
        self.g.builder.build_br(merge_bb);

        self.g.builder.position_at_end(short_bb);
        self.cur = short_bb;
        self.terminated = false;
        self.g.builder.build_br(merge_bb);

        self.g.builder.position_at_end(merge_bb);
        self.cur = merge_bb;
        self.terminated = false;
        let phi = self.g.builder.build_phi(self.g.ctx.i1(), "logic");
        let short = self.g.ctx.const_bool(!is_and);
        self.g.builder.add_incoming(phi, &[(r, rhs_end), (short, short_bb)]);
        Ok(phi)
    }

    pub(crate) fn emit_if(
        &mut self,
        cond: &Expr,
        then_block: &Block,
        else_block: Option<&Block>,
        ty: &Ty,
    ) -> Result<Option<Value<'a>>, String> {
        let c = self.value_of(cond)?;
        let then_bb = self.cur_fn.append_block("if.then");
        let else_bb = self.cur_fn.append_block("if.else");
        let merge_bb = self.cur_fn.append_block("if.end");
        self.g.builder.build_cond_br(c, then_bb, else_bb);

        self.g.builder.position_at_end(then_bb);
        self.cur = then_bb;
        self.terminated = false;
        let then_val = self.emit_block_value(then_block)?;
        let then_end = self.cur;
        let then_reaches = !self.terminated;
        if then_reaches {
            self.g.builder.build_br(merge_bb);
        }

        self.g.builder.position_at_end(else_bb);
        self.cur = else_bb;
        self.terminated = false;
        let else_val = match else_block {
            Some(b) => self.emit_block_value(b)?,
            None => None,
        };
        let else_end = self.cur;
        let else_reaches = !self.terminated;
        if else_reaches {
            self.g.builder.build_br(merge_bb);
        }

        self.g.builder.position_at_end(merge_bb);
        self.cur = merge_bb;
        self.terminated = false;

        // A unit-typed `if` has no value to join, and neither arm may have
        // reached the merge. An `if` with no reaching arm and no value is dead
        // code after a divergence: terminate the merge.
        if *ty == Ty::Unit {
            if !then_reaches && !else_reaches {
                self.g.builder.build_unreachable();
                self.terminated = true;
            }
            return Ok(None);
        }

        let mut incoming: Vec<(Value<'a>, BasicBlock<'a>)> = Vec::new();
        if then_reaches {
            let v = match then_val {
                Some(v) => v,
                None => self.zero_const(ty)?,
            };
            incoming.push((v, then_end));
        }
        if else_reaches {
            let v = match else_val {
                Some(v) => v,
                None => self.zero_const(ty)?,
            };
            incoming.push((v, else_end));
        }
        if incoming.is_empty() {
            // Both arms diverge, so this value is never used; the merge is dead.
            // The phi must not be built first: one with no incoming does not
            // verify.
            self.g.builder.build_unreachable();
            self.terminated = true;
            return Ok(Some(self.zero_const(ty)?));
        }
        let lty = self.lty(ty)?;
        let phi = self.g.builder.build_phi(lty, "if");
        self.g.builder.add_incoming(phi, &incoming);
        Ok(Some(phi))
    }

    /// `[a, b, c]` as one array *value*: the element array via `insertvalue`,
    /// then the wrapper struct's single field.
    pub(crate) fn emit_array_lit(
        &mut self,
        elems: &[Expr],
        ty: &Ty,
    ) -> Result<Value<'a>, String> {
        let Ty::Array(elem, len) = ty else {
            return Err(no_value("an array literal's type"));
        };
        let mut values = Vec::with_capacity(elems.len());
        for e in elems {
            values.push(self.value_of(e)?);
        }
        let _ = (elem, len);
        self.array_lit_from_values(values, ty)
    }

    /// The aggregate value of an array literal whose elements are already
    /// emitted.
    pub(crate) fn array_lit_from_values(
        &mut self,
        values: Vec<Value<'a>>,
        ty: &Ty,
    ) -> Result<Value<'a>, String> {
        let Ty::Array(elem, len) = ty else {
            return Err(no_value("an array literal's type"));
        };
        let array_ty = self.g.cty(ty)?;
        let elem_ty = self.lty(elem)?;
        let arr_ty = self.g.ctx.array_type(elem_ty, *len);
        // An all-constant literal is one constant array: folding a chain of
        // `insertvalue`s into a constant aggregate copies the whole aggregate
        // at every step, which is quadratic in the length.
        let arr = if values.len() == *len as usize && values.iter().all(|v| v.is_constant()) {
            self.g.ctx.const_array(elem_ty, &values)
        } else {
            let mut arr = self.g.ctx.undef(arr_ty);
            for (index, v) in values.into_iter().enumerate() {
                arr = self.g.builder.build_insert_value(arr, v, index as u32, "elem");
            }
            arr
        };
        let mut agg = self.g.ctx.undef(array_ty);
        agg = self.g.builder.build_insert_value(agg, arr, 0, "v");
        Ok(agg)
    }

    /// `[v; N]` through the generated `{array}__fill(v)` helper.
    pub(crate) fn emit_array_repeat(&mut self, value: &Expr, ty: &Ty) -> Result<Value<'a>, String> {
        let Ty::Array(elem, len) = ty else {
            return Err(no_value("a repeat literal's type"));
        };
        let v = self.value_of(value)?;
        let symbol = format!("{}__fill", crate::types::array_c_type(elem, *len));
        let (func, fty) = self.g.cfn(&symbol)?;
        Ok(self.g.call(func, fty, &[v], "fill"))
    }

    /// `asm(...)` (kernel stage 5): one inline-asm call. The outputs come first
    /// in constraint order, then the inputs, and an `inout` contributes both (an
    /// output and an input tied to it), which is the layout LLVM's constraint
    /// string and its call arguments both use.
    pub(crate) fn emit_asm(
        &mut self,
        block: &crate::tir::AsmBlock,
        value_ty: &Ty,
    ) -> Result<Option<Value<'a>>, String> {
        // Outputs in appearance order: one LLVM return member each. The
        // anonymous output's type is the asm expression's own.
        let mut out_tys: Vec<Type<'a>> = Vec::new();
        let mut out_places: Vec<Option<Value<'a>>> = Vec::new();
        let mut numbers: Vec<usize> = vec![0; block.operands.len()];
        for (i, op) in block.operands.iter().enumerate() {
            if op.kind == AsmOperandKind::In {
                continue;
            }
            match &op.operand {
                Some(place) => {
                    out_tys.push(self.lty(&place.ty)?);
                    out_places.push(Some(self.place_of(place)?));
                }
                None => {
                    out_tys.push(self.lty(value_ty)?);
                    out_places.push(None);
                }
            }
            numbers[i] = out_tys.len() - 1;
        }
        let out_count = out_tys.len();
        // Inputs in appearance order: the call's arguments. An `inout` reads
        // its place, exactly as an `in` reads its value.
        let mut args: Vec<Value<'a>> = Vec::new();
        let mut in_tys: Vec<Type<'a>> = Vec::new();
        for (i, op) in block.operands.iter().enumerate() {
            let value = match (op.kind, &op.operand) {
                (AsmOperandKind::Out, _) => continue,
                (_, Some(e)) => e,
                (_, None) => unreachable!("only an 'out' may have no operand"),
            };
            // An `inout`'s template number is its own output's, which it already
            // has; only an `in` takes a number after the outputs. LLVM counts
            // an `inout`'s tied input as an operand of its own, which is why
            // the counter here counts both.
            if op.kind == AsmOperandKind::In {
                numbers[i] = out_count + args.len();
            }
            args.push(self.value_of(value)?);
            in_tys.push(self.lty(&value.ty)?);
        }

        // The constraint string: one output per out/inout, then one input per
        // in/inout, with an `inout`'s input tied to its own output's index.
        let mut constraints: Vec<String> = Vec::new();
        for op in block.operands.iter() {
            if op.kind != AsmOperandKind::In {
                constraints.push(format!("={}", op.constraint));
            }
        }
        for (i, op) in block.operands.iter().enumerate() {
            match op.kind {
                AsmOperandKind::In => constraints.push(op.constraint.clone()),
                // An `inout`'s input is tied to its own output's index.
                AsmOperandKind::InOut => constraints.push(numbers[i].to_string()),
                AsmOperandKind::Out => {}
            }
        }
        for clobber in &block.clobbers {
            constraints.push(format!("~{{{clobber}}}"));
        }
        if !block.nomem {
            constraints.push("~{memory}".to_string());
        }

        let template = asm_operand_text(&block.template, &numbers, '$');
        let ret = match out_count {
            0 => self.g.ctx.void(),
            1 => out_tys[0],
            _ => self.g.ctx.literal_struct(&out_tys),
        };
        let fn_ty = self.g.ctx.fn_type(ret, &in_tys, false);
        let intel = is_x86_triple(&self.g.module.triple());
        let callee = self
            .g
            .ctx
            .inline_asm(fn_ty, &template, &constraints.join(","), true, intel);
        // A void instruction cannot carry a name in LLVM IR, and an asm with no
        // output is void.
        let call_name = if out_count == 1 { "asm.out" } else { "" };
        let call = self
            .g
            .builder
            .build_indirect_call(callee, fn_ty, &args, call_name);

        if out_count == 0 {
            return Ok(None);
        }
        let values: Vec<Value<'a>> = if out_count == 1 {
            vec![call]
        } else {
            (0..out_count)
                .map(|k| self.g.builder.build_extract_value(call, k as u32, "asm.out"))
                .collect()
        };
        for (value, place) in values.iter().zip(out_places.iter()) {
            if let Some(place) = place {
                self.g.builder.build_store(*value, *place);
            }
        }
        match block
            .value_out
            .and_then(|i| numbers.get(i).copied())
            .and_then(|i| values.get(i).copied())
        {
            Some(value) => Ok(Some(value)),
            None => Ok(None),
        }
    }

    /// Evaluate each argument a capability check reads, once, into a value the
    /// check and the call share. A checked argument is a `str` or `&str`
    /// (`stdlib::is_str_like_ty`), which is a bare pointer with no ownership, so
    /// `value_of` is exactly what the call path does with it (`arg_value` and
    /// `rt_arg_value` both fall through to `value_of` for a `str`-like
    /// argument). This is the eval-once half of BUG-Kf7Rn: a check that read a
    /// fresh rendering of the expression could admit one value and the call
    /// pass another.
    pub(crate) fn prepare_checked_args(
        &mut self,
        args: &[Expr],
        plan: Option<&CapPlan>,
    ) -> Result<HashMap<usize, Value<'a>>, String> {
        let mut prepared: HashMap<usize, Value<'a>> = HashMap::new();
        let Some(plan) = plan else {
            return Ok(prepared);
        };
        for check in &plan.checks {
            for index in cap_check_indices(check) {
                if prepared.contains_key(&index) {
                    continue;
                }
                let value = self.value_of(&args[index])?;
                prepared.insert(index, value);
            }
        }
        Ok(prepared)
    }

    /// Emit the runtime allowlist checks for one call site, before the call:
    /// one `kelp_cap_check_*` per resource argument, each with the domain, the
    /// resource, the module's compiled-in pattern array and its count, the
    /// calling module and the `.kp` call site.
    ///
    /// This is the capability-checked call's statement expression without
    /// the statement: the same symbols, the same argument order, and the same
    /// strings, because the deny path's text is pinned by the tests and the two
    /// backends must produce it byte for byte.
    pub(crate) fn emit_cap_checks(
        &mut self,
        plan: Option<&CapPlan>,
        prepared: &HashMap<usize, Value<'a>>,
    ) -> Result<(), String> {
        use crate::stdlib::CapCheck;
        let Some(plan) = plan else {
            return Ok(());
        };
        let domain = self.string_global(&plan.domain)?;
        let module = self.string_global(&self.module)?;
        let at = self.string_global(&plan.at)?;
        let patterns = *self
            .g
            .cap_arrays
            .get(&(self.module.clone(), plan.domain.clone()))
            .ok_or_else(|| no_value("the allowlist of a gated call site"))?;
        let count = self.g.v_i64(plan.pattern_count as u64);
        // Read once per plan, not once per check: every `CapCheck::Name` here
        // shares the one grant `plan.patterns` already is.
        let fast_path = name_fast_path(&plan.patterns);
        for check in &plan.checks {
            let value = |index: usize| -> Result<Value<'a>, String> {
                prepared
                    .get(&index)
                    .copied()
                    .ok_or_else(|| no_value("a checked call argument"))
            };
            // The two fast paths: hand-built IR in this same module, so
            // LLVM's own optimizer can see through and inline them, instead
            // of the call every other check kind still emits below into
            // the runtime capability glue's separately-compiled object (see the module doc
            // comment above `cap_plan` for why that call is otherwise opaque
            // to it). `deny_args` is exactly the argument list
            // `kelp_cap_check_name`'s own loop would have handed
            // `kelp_cap_deny`, so a denial from either fast path is
            // byte-for-byte the same message.
            if let CapCheck::Name(i) = *check {
                if let Some(path) = &fast_path {
                    let name = value(i)?;
                    let deny_args = [domain, name, patterns, count, module, at];
                    let ok = match path {
                        NameFastPath::Wildcard => {
                            self.emit_cap_name_wildcard_fast(name, &deny_args)?
                        }
                        NameFastPath::Literal(pattern) => {
                            let pattern_ptr = self.string_global(pattern)?;
                            self.emit_cap_name_literal_fast(name, pattern, pattern_ptr, &deny_args)?
                        }
                    };
                    self.cur = ok;
                    continue;
                }
            }
            let (func, ty) = match check {
                CapCheck::Path(_) => self.g.cap_fn("kelp_cap_check_path")?,
                CapCheck::Name(_) => self.g.cap_fn("kelp_cap_check_name")?,
                CapCheck::Host(_) => self.g.cap_fn("kelp_cap_check_host")?,
                CapCheck::HostPort(..) => self.g.cap_fn("kelp_cap_check_hostport")?,
                CapCheck::Port(_) => self.g.cap_fn("kelp_cap_check_port")?,
            };
            let args: Vec<Value<'a>> = match *check {
                CapCheck::Path(i) | CapCheck::Name(i) | CapCheck::Host(i) => {
                    vec![domain, value(i)?, patterns, count, module, at]
                }
                CapCheck::HostPort(host, port) => {
                    vec![domain, value(host)?, value(port)?, patterns, count, module, at]
                }
                CapCheck::Port(i) => vec![domain, value(i)?, patterns, count, module, at],
            };
            self.g.builder.build_call(func, ty, &args, "");
        }
        Ok(())
    }

    /// `CapCheck::Name`'s lone-`*` fast path (see `NameFastPath::Wildcard`):
    /// scan `name` for a `/` byte, byte by byte, exactly the way
    /// the runtime capability glue's own fast path and `std/rt.kp`'s `glob_match` both do.
    /// Finding a `/` before the NUL terminator denies, calling
    /// `kelp_cap_deny` with `deny_args` unchanged (the same six arguments the
    /// runtime match loop's own denial would have used); reaching the
    /// terminator with none found is the check passing. The returned block is
    /// where the builder is left positioned, for the caller to adopt as its
    /// own `self.cur`.
    pub(crate) fn emit_cap_name_wildcard_fast(
        &self,
        name: Value<'a>,
        deny_args: &[Value<'a>],
    ) -> Result<BasicBlock<'a>, String> {
        let func = self.cur_fn;
        let i8t = self.g.ctx.i8();
        let i64t = self.g.ctx.i64();
        let deny_bb = self.g.block(func, "capname.deny");
        let ok_bb = self.g.block(func, "capname.ok");
        let idx = self.g.builder.build_alloca(i64t, "capname.idx");
        self.g.st(self.g.v_i64(0), idx);
        let cond = self.g.loop_header(func, "capname.scan.cond");
        let i = self.g.ld(i64t, idx, "capname.i");
        let cptr = self.g.gep(i8t, name, i, "capname.cptr");
        let c = self.g.ld(i8t, cptr, "capname.c");
        let is_nul = self.g.builder.build_icmp(IntCmp::Eq, c, self.g.v_i8(0), "capname.nul");
        let is_slash = self.g.builder.build_icmp(IntCmp::Eq, c, self.g.v_i8(47), "capname.slash");
        let stop_now = self.g.builder.build_int_binop(IntBinOp::Or, is_nul, is_slash, "capname.stop");
        let body = self.g.block(func, "capname.scan.body");
        let stop = self.g.block(func, "capname.scan.stop");
        self.g.builder.build_cond_br(stop_now, stop, body);
        self.g.builder.position_at_end(body);
        let next = self.g.builder.build_int_binop(IntBinOp::Add, i, self.g.v_i64(1), "capname.next");
        self.g.st(next, idx);
        self.g.builder.build_br(cond);
        self.g.builder.position_at_end(stop);
        // `is_slash` was computed in `cond`, which dominates `stop` (`cond`
        // branches straight to it), so reusing the value here rather than
        // recomputing it is ordinary SSA use across a dominated block, the
        // same way this file's other hand-built loops reuse a header's value
        // in the block it branches to.
        self.g.builder.build_cond_br(is_slash, deny_bb, ok_bb);
        self.g.builder.position_at_end(deny_bb);
        let (deny_fn, deny_ty) = self.g.cap_fn("kelp_cap_deny")?;
        self.g.builder.build_call(deny_fn, deny_ty, deny_args, "");
        self.g.builder.build_unreachable();
        self.g.builder.position_at_end(ok_bb);
        Ok(ok_bb)
    }

    /// `CapCheck::Name`'s literal fast path (see `NameFastPath::Literal`):
    /// compare `name` against `pattern`'s bytes one at a time, over a
    /// compile-time-known length (no `strlen` needed, `pattern.len()` is
    /// already known in Rust), then confirm `name[pattern.len()]` is the NUL
    /// terminator so a name that merely has `pattern` as a prefix (`"PATHX"`
    /// against a grant of `"PATH"`) does not spuriously pass. Any mismatch,
    /// in the loop or at the terminator, denies with `deny_args` unchanged; a
    /// full match is the check passing. `pattern_ptr` is `pattern` interned
    /// as an LLVM global string constant (`self.string_global`, the same
    /// helper the domain/module/`at` strings above use).
    pub(crate) fn emit_cap_name_literal_fast(
        &self,
        name: Value<'a>,
        pattern: &str,
        pattern_ptr: Value<'a>,
        deny_args: &[Value<'a>],
    ) -> Result<BasicBlock<'a>, String> {
        let func = self.cur_fn;
        let i8t = self.g.ctx.i8();
        let i64t = self.g.ctx.i64();
        let len = pattern.len() as u64;
        let deny_bb = self.g.block(func, "capname.deny");
        let ok_bb = self.g.block(func, "capname.ok");
        let idx = self.g.builder.build_alloca(i64t, "capname.idx");
        self.g.st(self.g.v_i64(0), idx);
        let cond = self.g.loop_header(func, "capname.cmp.cond");
        let i = self.g.ld(i64t, idx, "capname.i");
        let more = self.g.builder.build_icmp(IntCmp::Ult, i, self.g.v_i64(len), "capname.more");
        let body = self.g.block(func, "capname.cmp.body");
        let after = self.g.block(func, "capname.cmp.after");
        self.g.builder.build_cond_br(more, body, after);
        self.g.builder.position_at_end(body);
        let np = self.g.gep(i8t, name, i, "capname.np");
        let pp = self.g.gep(i8t, pattern_ptr, i, "capname.pp");
        let nv = self.g.ld(i8t, np, "capname.nv");
        let pv = self.g.ld(i8t, pp, "capname.pv");
        let eq = self.g.builder.build_icmp(IntCmp::Eq, nv, pv, "capname.eq");
        let cont = self.g.block(func, "capname.cmp.cont");
        self.g.builder.build_cond_br(eq, cont, deny_bb);
        self.g.builder.position_at_end(cont);
        let next = self.g.builder.build_int_binop(IntBinOp::Add, i, self.g.v_i64(1), "capname.next");
        self.g.st(next, idx);
        self.g.builder.build_br(cond);
        self.g.builder.position_at_end(after);
        let endp = self.g.gep(i8t, name, self.g.v_i64(len), "capname.endp");
        let endc = self.g.ld(i8t, endp, "capname.endc");
        let is_nul = self.g.builder.build_icmp(IntCmp::Eq, endc, self.g.v_i8(0), "capname.endnul");
        self.g.builder.build_cond_br(is_nul, ok_bb, deny_bb);
        self.g.builder.position_at_end(deny_bb);
        let (deny_fn, deny_ty) = self.g.cap_fn("kelp_cap_deny")?;
        self.g.builder.build_call(deny_fn, deny_ty, deny_args, "");
        self.g.builder.build_unreachable();
        self.g.builder.position_at_end(ok_bb);
        Ok(ok_bb)
    }

    /// A value of the element type as the storage type: a zero extension for
    /// `bool`, and the value itself for every integer.
    pub(crate) fn atomic_to_mem(&self, elem: &Ty, value: Value<'a>) -> Result<Value<'a>, String> {
        if !matches!(elem, Ty::Bool) {
            return Ok(value);
        }
        Ok(self.g.builder.build_int_cast(false, true, value, self.g.ctx.i8(), "atomic.wide"))
    }

    /// The storage type read back as a value of the element type: a truncation
    /// for `bool`, which keeps the low bit. That is the whole byte only because
    /// the atomic store is the one thing that writes it and it writes 0 or 1;
    /// any other byte would read back as its low bit rather than as true.
    pub(crate) fn atomic_from_mem(&self, elem: &Ty, raw: Value<'a>) -> Result<Value<'a>, String> {
        if !matches!(elem, Ty::Bool) {
            return Ok(raw);
        }
        Ok(self.g.builder.build_int_cast(false, false, raw, self.g.ctx.i1(), "atomic.narrow"))
    }

    /// One atomic operation on an `Atomic<T>` (MK9-S3-M1 stage A1), as the one
    /// LLVM instruction that is the operation: `load atomic`, `store atomic`,
    /// `atomicrmw` and `cmpxchg`, every one of them `seq_cst`, the only ordering
    /// v1 states.
    ///
    /// The ordering argument is never evaluated: with one ordering in the
    /// surface there is nothing for the backend to choose, and the instruction's
    /// own `seq_cst` is the whole of what the argument says.
    ///
    /// `compare_exchange` is one `cmpxchg` plus the `Result<T, T>` MK9 4 asks
    /// for. The `cmpxchg` pair's field 0 is the value the instruction read from
    /// memory: the compared value on success (so, the value that was there) and
    /// the value actually found on failure. One `cmpxchg` therefore supplies
    /// both halves of both variants with no second access, which is the point of
    /// `Result` over a `bool` plus a reload.
    /// A `Ptr<T>` operation (MK9-S2 stage 1), as LLVM IR: one instruction per
    /// operation, except `alloc`, which is the runtime's allocator plus the
    /// same NULL check every generated allocation makes.
    ///
    /// The volatile pair loads and stores through the same instructions and
    /// then sets the volatile flag, which is what forbids the optimizer to
    /// merge it, reorder it against another volatile access, or elide it.
    pub(crate) fn emit_ptr_call(
        &mut self,
        op: &str,
        args: &[Expr],
        ret: &Ty,
    ) -> Result<Option<Value<'a>>, String> {
        let ptr = self.g.pty();
        let result_elem = || {
            crate::types::ptr_elem_of(ret)
                .cloned()
                .ok_or_else(|| no_value("the element type of a Ptr operation's result"))
        };
        let recv_elem = || {
            crate::types::ptr_elem_of(&args[0].ty)
                .cloned()
                .ok_or_else(|| no_value("the element type of a Ptr operation's receiver"))
        };
        match op {
            "null" => {
                // The element type is only in the Kelp type: a null pointer is
                // a null pointer whatever it addresses.
                result_elem()?;
                Ok(Some(self.g.ctx.const_null(ptr)))
            }
            "from_addr" => {
                result_elem()?;
                let addr = self.value_of(&args[0])?;
                Ok(Some(self.g.builder.build_inttoptr(addr, ptr)))
            }
            "from_ref" | "from_mut" => {
                result_elem()?;
                // The borrow argument already evaluated to the place's address.
                Ok(Some(self.value_of(&args[0])?))
            }
            // `Ptr.from_fn(f)` (kernel gap 2): the function itself as a `ptr`
            // constant. The argument is a `FnRef` (a declared `fun`, or a
            // literal that captured nothing), whose value is already the
            // function's address, so there is nothing to compute.
            "from_fn" => {
                result_elem()?;
                Ok(Some(self.value_of(&args[0])?))
            }
            "alloc" => {
                let elem = result_elem()?;
                let lty = self.lty(&elem)?;
                let size = self.g.layout.abi_size_of(lty);
                let count = self.value_of(&args[0])?;
                let bytes = self.g.builder.build_int_binop(
                    IntBinOp::Mul,
                    count,
                    self.g.v_i64(size),
                    "ptr.bytes",
                );
                let (func, fty) = self.g.libc_fn("malloc")?;
                let heap = self.g.builder.build_call(func, fty, &[bytes], "ptr.heap");
                self.cur = self.g.check_alloc(heap, self.cur_fn)?;
                Ok(Some(heap))
            }
            // The fallible twin (kernel stage 13): the allocator's own null is
            // the result rather than a panic. The byte count is checked for
            // wrap first, on the same rule the earlier emitter's `ptr_call` writes as
            // C, so a huge count never reaches the allocator as a small block;
            // both outcomes land in one slot so the expression has one value
            // and the allocator is only called on the non-wrapping path.
            "try_alloc" => {
                let elem = result_elem()?;
                let lty = self.lty(&elem)?;
                let size = self.g.layout.abi_size_of(lty);
                let align = self.g.layout.abi_alignment_of(lty) as u64;
                let count = self.value_of(&args[0])?;
                let limit = if size == 0 { u64::MAX } else { u64::MAX / size };
                let over = self.g.builder.build_icmp(
                    IntCmp::Ugt,
                    count,
                    self.g.v_i64(limit),
                    "try.over",
                );
                let bytes = self.g.builder.build_int_binop(
                    IntBinOp::Mul,
                    count,
                    self.g.v_i64(size),
                    "try.bytes",
                );
                let over_bb = self.cur_fn.append_block("try.over");
                let ok_bb = self.cur_fn.append_block("try.ok");
                let join_bb = self.cur_fn.append_block("try.join");
                let slot = self.temp_alloca(self.g.pty(), "try.slot");
                self.g.builder.build_cond_br(over, over_bb, ok_bb);
                self.g.builder.position_at_end(over_bb);
                self.cur = over_bb;
                self.terminated = false;
                self.g.builder.build_store(self.g.ctx.const_null(self.g.pty()), slot);
                self.g.builder.build_br(join_bb);
                self.g.builder.position_at_end(ok_bb);
                self.cur = ok_bb;
                self.terminated = false;
                let heap = self.g.heap_alloc(bytes, align)?;
                self.g.builder.build_store(heap, slot);
                self.g.builder.build_br(join_bb);
                self.g.builder.position_at_end(join_bb);
                self.cur = join_bb;
                self.terminated = false;
                Ok(Some(self.g.builder.build_load(self.g.pty(), slot, "try.ptr")))
            }
            "addr" => {
                let v = self.value_of(&args[0])?;
                let i64t = self.g.ctx.i64();
                Ok(Some(self.g.builder.build_ptrtoint(v, i64t)))
            }
            "is_null" => {
                let v = self.value_of(&args[0])?;
                let null = self.g.ctx.const_null(ptr);
                Ok(Some(self.g.builder.build_icmp(IntCmp::Eq, v, null, "isnull")))
            }
            "offset" => {
                let elem = recv_elem()?;
                let lty = self.lty(&elem)?;
                let v = self.value_of(&args[0])?;
                let n = self.value_of(&args[1])?;
                Ok(Some(self.g.builder.build_gep(lty, v, n, "ptr.offset")))
            }
            "byte_offset" => {
                recv_elem()?;
                let v = self.value_of(&args[0])?;
                let n = self.value_of(&args[1])?;
                let byte = self.g.ctx.i8();
                Ok(Some(self.g.builder.build_gep(byte, v, n, "ptr.byte")))
            }
            // A cast reinterprets: with opaque pointers the value does not
            // change at all, and only the Kelp type records the new element.
            "cast" => {
                result_elem()?;
                Ok(Some(self.value_of(&args[0])?))
            }
            "read" | "read_volatile" | "read_unaligned" => {
                let elem = recv_elem()?;
                let lty = self.lty(&elem)?;
                let place = self.value_of(&args[0])?;
                let v = self.g.builder.build_load(lty, place, "ptr.read");
                if op == "read_volatile" {
                    self.g.builder.set_volatile(v);
                }
                // The unaligned read is the same load with no alignment
                // requirement: `align 1` is what lets the address be anything
                // the pointer holds, which is the whole point of the method.
                if op == "read_unaligned" {
                    self.g.builder.set_alignment(v, 1);
                }
                Ok(Some(v))
            }
            "write" | "write_volatile" | "write_unaligned" => {
                recv_elem()?;
                let place = self.value_of(&args[0])?;
                let value = self.value_of(&args[1])?;
                let store = self.g.builder.build_store(value, place);
                if op == "write_volatile" {
                    self.g.builder.set_volatile(store);
                }
                if op == "write_unaligned" {
                    self.g.builder.set_alignment(store, 1);
                }
                Ok(None)
            }
            "free" => {
                recv_elem()?;
                let v = self.value_of(&args[0])?;
                let (func, fty) = self.g.libc_fn("free")?;
                self.g.builder.build_call(func, fty, &[v], "");
                Ok(None)
            }
            other => Err(no_value(&format!("the pointer operation '{other}'"))),
        }
    }

    /// Branch to a panic block when `cond` holds, then continue: the shape
    /// `emit_conversion`'s range check uses, so an out-of-bounds `Bytes.read_*`
    /// leaves the same unreachable panic block a narrowing conversion does.
    pub(crate) fn guard_or_panic(&mut self, cond: Value<'a>, msg: &str) -> Result<(), String> {
        let panic_bb = self.cur_fn.append_block("bounds.panic");
        let ok_bb = self.cur_fn.append_block("bounds.ok");
        self.g.builder.build_cond_br(cond, panic_bb, ok_bb);

        self.g.builder.position_at_end(panic_bb);
        self.cur = panic_bb;
        self.terminated = false;
        let msg_ptr = self.string_global(msg)?;
        self.g.emit_panic_ptr(msg_ptr);

        self.g.builder.position_at_end(ok_bb);
        self.cur = ok_bb;
        self.terminated = false;
        Ok(())
    }

    /// Emit one `llvm.bswap.iN` intrinsic call on `v`. The intrinsic is what
    /// the earlier emitter's `__builtin_bswap*` lowers to, so both backends run the
    /// same machine instruction.
    pub(crate) fn bswap(&mut self, v: Value<'a>, bits: u32) -> Result<Value<'a>, String> {
        let ty = match bits {
            16 => self.g.ctx.i16(),
            32 => self.g.ctx.i32(),
            64 => self.g.ctx.i64(),
            _ => return Err(no_value("a byte swap")),
        };
        let fty = self.g.ctx.fn_type(ty, &[ty], false);
        let name = format!("llvm.bswap.i{bits}");
        // `add_function` would mangle the name of an already-declared intrinsic
        // (`llvm.bswap.i32.20`), which the verifier rejects as a badly mangled
        // intrinsic, so the second and later calls reuse the first declaration.
        let func = match self.g.module.get_function(&name) {
            Some(existing) => existing,
            None => self.g.module.add_function(&name, fty),
        };
        Ok(self.g.builder.build_call(func, fty, &[v], "bswap"))
    }

    /// One integer byte-order conversion (kernel stage 3, MK9-S8):
    /// `x.swap_bytes()`, `x.to_be()`, `x.to_le()` and the static
    /// `u32.from_be(x)`/`u32.from_le(x)`. `None` for any other `Type#op` key.
    ///
    /// `to_le` is the identity exactly on a little-endian target, which is the
    /// target's own answer (`Gen::little_endian`) rather than an assumption
    /// about the host.
    pub(crate) fn emit_endian(
        &mut self,
        op: &str,
        ty: &Ty,
        args: &[Expr],
    ) -> Result<Option<Value<'a>>, String> {
        let swap = match op {
            "swap_bytes" => true,
            "to_be" | "from_be" => self.g.little_endian,
            "to_le" | "from_le" => !self.g.little_endian,
            _ => return Ok(None),
        };
        let Some(bits) = endian_bits(ty) else {
            return Ok(None);
        };
        let v = self.value_of(&args[0])?;
        Ok(Some(if swap { self.bswap(v, bits)? } else { v }))
    }

    /// A byte-order read or write on a `Bytes` (kernel stage 3, MK9-S8).
    ///
    /// The bounds check is the same wrapped-add test
    /// the earlier emitter's `bytes_endian_call` writes, and the access is an `align 1`
    /// load or store, so the offset needs no alignment. A field whose order is
    /// not the target's swaps on the way in or out; one whose order already
    /// matches is a plain load or store.
    pub(crate) fn emit_bytes_endian(
        &mut self,
        op: &str,
        args: &[Expr],
    ) -> Result<Option<Value<'a>>, String> {
        let (width, big, value_ty) = crate::types::bytes_endian_access(op)
            .ok_or_else(|| no_value("a byte-order access"))?;
        let bytes = *self
            .g
            .structs
            .get("Bytes")
            .ok_or_else(|| no_value("the builtin 'Bytes' layout"))?;
        let bits = endian_bits(&value_ty)
            .ok_or_else(|| no_value("the width of a byte-order access"))?;
        let i64t = self.g.ctx.i64();
        let ptr = self.g.pty();

        let buf = self.value_of(&args[0])?;
        let offset = self.value_of(&args[1])?;
        let len = self.g.ld(i64t, self.g.fld(bytes, buf, L_LEN, "len"), "len");
        let end = self.g.builder.build_int_binop(
            IntBinOp::Add,
            offset,
            self.g.v_i64(width as u64),
            "end",
        );
        // A near-`usize::MAX` offset wraps the addition past `len`, so the wrap
        // itself is a failure too.
        let wrapped = self.g.builder.build_icmp(IntCmp::Ult, end, offset, "wrapped");
        let past = self.g.builder.build_icmp(IntCmp::Ugt, end, len, "past");
        let oob = self.g.builder.build_int_binop(IntBinOp::Or, wrapped, past, "oob");
        let msg = format!("Bytes.{op}: offset out of bounds");
        self.guard_or_panic(oob, &msg)?;

        let data = self.g.ld(ptr, self.g.fld(bytes, buf, L_DATA, "data"), "data");
        let at = self.g.gep(self.g.ctx.i8(), data, offset, "at");
        let lty = self.lty(&value_ty)?;
        // Swap when the field's byte order is not the target's: a big-endian
        // field on a little-endian target, or the reverse.
        let target_big_endian = !self.g.little_endian;
        let swap = big != target_big_endian;
        if op.starts_with("read_") {
            let raw = self.g.builder.build_load(lty, at, "read");
            self.g.builder.set_alignment(raw, 1);
            Ok(Some(if swap { self.bswap(raw, bits)? } else { raw }))
        } else {
            let value = self.value_of(&args[2])?;
            let value = if swap { self.bswap(value, bits)? } else { value };
            let store = self.g.builder.build_store(value, at);
            self.g.builder.set_alignment(store, 1);
            Ok(None)
        }
    }

    pub(crate) fn emit_atomic_call(
        &mut self,
        op: &str,
        args: &[Expr],
        ret: &Ty,
    ) -> Result<Option<Value<'a>>, String> {
        // `Atomic.new(v)` is the one operation with no place: it builds the
        // cell, and it takes the value as its only argument.
        if op == "new" {
            let elem = crate::types::atomic_elem_of(ret)
                .cloned()
                .ok_or_else(|| no_value("the element type of Atomic.new"))?;
            let value = self.value_of(&args[0])?;
            let field = self.atomic_to_mem(&elem, value)?;
            let lty = self.g.cty(ret)?;
            let agg = self.g.ctx.undef(lty);
            return Ok(Some(
                self.g.builder.build_insert_value(agg, field, 0, "atomic.new"),
            ));
        }
        let elem = crate::types::atomic_elem_of(&args[0].ty)
            .cloned()
            .ok_or_else(|| no_value("the element type of an atomic operation"))?;
        // The receiver is a borrow, so its value *is* the address of the cell.
        let place = self.value_of(&args[0])?;
        let mem = self.g.atomic_mem_lty(&elem)?;
        let rmw = |op: &str| -> Result<AtomicRmw, String> {
            Ok(match op {
                "swap" => AtomicRmw::Xchg,
                "fetch_add" => AtomicRmw::Add,
                "fetch_sub" => AtomicRmw::Sub,
                "fetch_and" => AtomicRmw::And,
                "fetch_or" => AtomicRmw::Or,
                "fetch_xor" => AtomicRmw::Xor,
                other => return Err(no_value(&format!("the atomic operation '{other}'"))),
            })
        };
        match op {
            "load" => {
                let raw = self.g.builder.build_load_atomic(mem, place, "atomic.load");
                Ok(Some(self.atomic_from_mem(&elem, raw)?))
            }
            "store" => {
                let value = self.value_of(&args[1])?;
                let value = self.atomic_to_mem(&elem, value)?;
                self.g.builder.build_store_atomic(value, place);
                Ok(None)
            }
            "swap" | "fetch_add" | "fetch_sub" | "fetch_and" | "fetch_or" | "fetch_xor" => {
                let value = self.value_of(&args[1])?;
                let value = self.atomic_to_mem(&elem, value)?;
                let raw = self.g.builder.build_atomicrmw(rmw(op)?, place, value);
                Ok(Some(self.atomic_from_mem(&elem, raw)?))
            }
            "compare_exchange" => {
                let current = self.value_of(&args[1])?;
                let current = self.atomic_to_mem(&elem, current)?;
                let new = self.value_of(&args[2])?;
                let new = self.atomic_to_mem(&elem, new)?;
                let pair = self.g.builder.build_cmpxchg(place, current, new);
                let prev = self.g.builder.build_extract_value(pair, 0, "atomic.prev");
                let ok = self.g.builder.build_extract_value(pair, 1, "atomic.ok");
                let prev = self.atomic_from_mem(&elem, prev)?;
                Ok(Some(self.emit_cmpxchg_result(ret, ok, prev)?))
            }
            other => Err(no_value(&format!("the atomic operation '{other}'"))),
        }
    }
}
