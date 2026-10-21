use super::*;

impl<'a> Gen<'a> {
    /// The pointer type LLVM 21 uses for every pointer.
    pub(crate) fn pty(&self) -> Type<'a> {
        self.ctx.opaque_ptr()
    }

    pub(crate) fn v_i64(&self, v: u64) -> Value<'a> {
        self.ctx.const_int(self.ctx.i64(), v, false)
    }

    pub(crate) fn v_i32(&self, v: u64) -> Value<'a> {
        self.ctx.const_int(self.ctx.i32(), v, false)
    }

    pub(crate) fn v_i8(&self, v: u64) -> Value<'a> {
        self.ctx.const_int(self.ctx.i8(), v, false)
    }

    /// The `elem`-typed address `index` elements past `ptr`, the plain
    /// non-`inbounds` GEP MK7 §3.4 wants for a data-dependent index.
    pub(crate) fn gep(&self, elem: Type<'a>, ptr: Value<'a>, index: Value<'a>, name: &str) -> Value<'a> {
        self.builder.build_gep(elem, ptr, index, name)
    }

    /// The address of field `idx` of the aggregate at `ptr`. The index is a
    /// compile-time constant inside an object of that exact type, which is the
    /// one place MK7 §3.4 allows the `inbounds` form.
    pub(crate) fn fld(&self, agg: Type<'a>, ptr: Value<'a>, idx: u32, name: &str) -> Value<'a> {
        self.builder.build_struct_gep(agg, ptr, idx, name)
    }

    pub(crate) fn ld(&self, ty: Type<'a>, ptr: Value<'a>, name: &str) -> Value<'a> {
        self.builder.build_load(ty, ptr, name)
    }

    pub(crate) fn st(&self, value: Value<'a>, ptr: Value<'a>) {
        self.builder.build_store(value, ptr);
    }

    pub(crate) fn cfn(&self, symbol: &str) -> Result<(LlvmFunction<'a>, FnType<'a>), String> {
        self.container_fns
            .get(symbol)
            .copied()
            .ok_or_else(|| no_value(&format!("the container function '{symbol}'")))
    }

    pub(crate) fn libc_fn(&self, name: &str) -> Result<(LlvmFunction<'a>, FnType<'a>), String> {
        self.libc
            .get(name)
            .copied()
            .ok_or_else(|| no_value(&format!("the libc function '{name}'")) )
    }
    pub(crate) fn cap_fn(&self, name: &str) -> Result<(LlvmFunction<'a>, FnType<'a>), String> {
        self.cap_fns
            .get(name)
            .copied()
            .ok_or_else(|| no_value(&format!("the capability check '{name}'")))
    }

    /// Append a block to `func` without moving the builder. A caller that
    /// creates several blocks before the terminator that branches to them
    /// needs exactly this: the terminator goes into the *current* block, and
    /// each destination is entered with an explicit `position_at_end`.
    pub(crate) fn block(&self, func: LlvmFunction<'a>, name: &str) -> BasicBlock<'a> {
        func.append_block(name)
    }

    /// A loop header: branch to a fresh block from wherever the builder is, and
    /// continue in it. This is the one place a block is created *and* entered,
    /// because a loop header is branched to once from the pre-header and once
    /// from the body.
    pub(crate) fn loop_header(&self, func: LlvmFunction<'a>, name: &str) -> BasicBlock<'a> {
        let bb = func.append_block(name);
        self.builder.build_br(bb);
        self.builder.position_at_end(bb);
        bb
    }

    /// Emit `kelp_panic(msg)` and terminate the block, for the bounds checks
    /// and the empty-container panics whose text must be byte-identical to the
    /// text of the runtime's own panics.
    /// Point `alloc_fns` at this project's own allocator (kernel stage 7,
    /// MK9-S5-M1): the manifest's `[target] alloc`/`dealloc`/`realloc` names,
    /// resolved to the LLVM functions `declare_functions` already declared for
    /// them. It runs after every function is in `funcs` and before any
    /// container body, because a container body calls these directly.
    pub(crate) fn resolve_freestanding_allocator(&mut self) -> Result<(), String> {
        let Some(fs) = self.freestanding.clone() else {
            return Ok(());
        };
        let (Some(alloc), Some(dealloc)) = (fs.alloc.clone(), fs.dealloc.clone()) else {
            return Ok(());
        };
        let lookup = |name: &str| -> Result<(LlvmFunction<'a>, FnType<'a>), String> {
            self.funcs
                .get(name)
                .map(|info| (info.func, info.ty))
                .ok_or_else(|| {
                    format!(
                        "the manifest's '[target]' names the allocator function '{name}', which the program does not declare"
                    )
                })
        };
        let alloc = lookup(&alloc)?;
        let dealloc = lookup(&dealloc)?;
        let realloc = match &fs.realloc {
            Some(name) => Some(lookup(name)?),
            None => None,
        };
        self.alloc_fns = Some(AllocFns {
            alloc,
            dealloc,
            realloc,
        });
        Ok(())
    }

    pub(crate) fn emit_panic(&self, msg: &str) -> Result<(), String> {
        let ptr = self.builder.global_string(msg, "panic.msg");
        self.emit_panic_ptr(ptr);
        Ok(())
    }

    /// Store a statement's file and line into the two location globals, which
    /// every check in a generated body reads (kernel stage 15).
    pub(crate) fn store_panic_site(&self, module: &str, span: Span) {
        let (file, line) = match self.sources.path_line_col(module, span) {
            Some((_, line, _)) => (self.panic_files.get(module).copied(), line),
            None => (None, 0),
        };
        let file = file.unwrap_or_else(|| self.ctx.const_null(self.ctx.opaque_ptr()));
        let line = self.ctx.const_int(self.ctx.i32(), line as u64, false);
        let file_g = self
            .panic_file_global
            .expect("a freestanding panic file global");
        let line_g = self
            .panic_line_global
            .expect("a freestanding panic line global");
        self.builder.build_store(file, file_g);
        self.builder.build_store(line, line_g);
    }

    /// Emit a call to the panic path with a message pointer the caller already
    /// built, then `unreachable`. In a freestanding build the callee is the
    /// compiler's entry, which takes the message plus the file and line the
    /// current statement stored (kernel stage 15); a hosted build still calls
    /// the runtime's one-argument `kelp_panic`.
    pub(crate) fn emit_panic_ptr(&self, msg: Value<'a>) {
        if self.freestanding.is_some() {
            let file = self.load_panic_file();
            let line = self.load_panic_line();
            self.builder
                .build_call(self.panic_fn, self.panic_ty, &[msg, file, line], "");
        } else {
            self.builder
                .build_call(self.panic_fn, self.panic_ty, &[msg], "");
        }
        self.builder.build_unreachable();
    }

    /// The two location globals a freestanding build declares before any body.
    pub(crate) fn load_panic_file(&self) -> Value<'a> {
        let global = self.panic_file_global.expect("a freestanding panic file global");
        self.builder.build_load(self.ctx.opaque_ptr(), global, "panic.file")
    }

    pub(crate) fn load_panic_line(&self) -> Value<'a> {
        let global = self.panic_line_global.expect("a freestanding panic line global");
        self.builder.build_load(self.ctx.i32(), global, "panic.line")
    }

    /// `if (i >= len) panic(msg);`, the bounds check every indexed builtin
    /// accessor opens with. The text is the runtime's own, so the message a
    /// program prints is the same one the runtime object would have printed.
    pub(crate) fn emit_bounds_check(
        &self,
        func: LlvmFunction<'a>,
        len: Value<'a>,
        i: Value<'a>,
        msg: &str,
    ) -> Result<(), String> {
        let oob = self.builder.build_icmp(IntCmp::Uge, i, len, "oob");
        let out = self.block(func, "panic");
        let ok = self.block(func, "in.bounds");
        self.builder.build_cond_br(oob, out, ok);
        self.builder.position_at_end(out);
        self.emit_panic(msg)?;
        self.builder.position_at_end(ok);
        Ok(())
    }

    /// The current CPU's index (kernel stage 8): the value of the register the
    /// kernel sets at boot, read with one inline-asm instruction (`tp` on
    /// riscv64, `TPIDR_EL1` on aarch64). The compiler never sets the register,
    /// and it never trusts a value it did not produce: in a debug build
    /// (`-O0`) the index is compared against the manifest's `[target] cpus`
    /// and panics through the project's own handler when it is out of range,
    /// while a release build trusts it (`kelp wiki page language/unsafe-ub-catalogue.md` stage 11).
    pub(crate) fn cpu_index(&self, func: LlvmFunction<'a>) -> Result<Value<'a>, String> {
        let triple = self
            .freestanding
            .as_ref()
            .map(|fs| fs.triple.as_str())
            .ok_or_else(|| no_value("'cpu_index()' outside a freestanding build"))?;
        let template = if triple.starts_with("riscv64") || triple.starts_with("riscv32") {
            "mv {0}, tp"
        } else if triple.starts_with("aarch64") || triple.starts_with("arm64") {
            "mrs {0}, TPIDR_EL1"
        } else {
            return Err(format!(
                "kelpc has no 'cpu_index()' lowering for the triple '{triple}': it reads 'tp' on riscv64 and 'TPIDR_EL1' on aarch64"
            ));
        };
        let usize_ty = self.lty2(&Ty::Usize)?;
        let fn_ty = self.ctx.fn_type(usize_ty, &[], false);
        let text = asm_operand_text(template, &[0], '$');
        let callee = self.ctx.inline_asm(fn_ty, &text, "=r", true, false);
        let idx = self
            .builder
            .build_indirect_call(callee, fn_ty, &[], "cpu");
        if self.debug_checks {
            let n = self.ctx.const_int(usize_ty, self.cpus, false);
            self.emit_bounds_check(func, n, idx, "cpu index out of range")?;
        }
        Ok(idx)
    }

    /// The inline half of `kelp_buf_reserve` for an owned buffer: test
    /// `len + extra <= cap` where the callee tests it, which is its own first
    /// line, and call the callee only when the buffer has to grow. Both sides
    /// of the comparison are `size_t` sums that wrap the same way in C, so an
    /// overflow takes the growth branch in both. The buffers stay the runtime's
    /// (`realloc` and the NUL terminator are its business).
    pub(crate) fn emit_grow_call(
        &self,
        func: LlvmFunction<'a>,
        agg: Type<'a>,
        self_p: Value<'a>,
        extra: Value<'a>,
    ) -> Result<(), String> {
        let i64t = self.ctx.i64();
        let len = self.ld(i64t, self.fld(agg, self_p, L_LEN, "len"), "len");
        let cap = self.ld(i64t, self.fld(agg, self_p, L_CAP, "cap"), "cap");
        let need = self
            .builder
            .build_int_binop(IntBinOp::Add, len, extra, "need");
        let fits = self.builder.build_icmp(IntCmp::Ule, need, cap, "fits");
        let grow = self.block(func, "grow");
        let done = self.block(func, "reserved");
        self.builder.build_cond_br(fits, done, grow);
        self.builder.position_at_end(grow);
        let reserve = self.rt("kelp_buf_reserve")?;
        let data_p = self.fld(agg, self_p, L_DATA, "data");
        let cap_p = self.fld(agg, self_p, L_CAP, "cap");
        self.call(reserve.func, reserve.ty, &[data_p, len, cap_p, extra], "");
        self.builder.build_br(done);
        self.builder.position_at_end(done);
        Ok(())
    }

    pub(crate) fn call(&self, func: LlvmFunction<'a>, ty: FnType<'a>, args: &[Value<'a>], name: &str) -> Value<'a> {
        self.builder.build_call(func, ty, args, name)
    }

    // ---- container types -------------------------------------------------
}
