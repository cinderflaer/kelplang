use super::*;

impl<'g, 'a> FnGen<'g, 'a> {
    pub(crate) fn new(g: &'g Gen<'a>, f: &'a Function) -> Result<Self, String> {
        let info = g
            .funcs
            .get(&f.name)
            .cloned()
            .ok_or_else(|| no_value("a declared function"))?;
        let entry = info.func.append_block("entry");
        let env_ty = match &f.env {
            Some(_) => Some(
                *g.env_types
                    .get(&f.name)
                    .ok_or_else(|| no_value(&format!("the environment of '{}'", f.name)))?,
            ),
            None => None,
        };
        Ok(FnGen {
            g,
            cur_fn: info.func,
            entry,
            cur: entry,
            terminated: false,
            scopes: Vec::new(),
            bindings: HashMap::new(),
            fn_name: f.name.clone(),
            module: f.module.clone(),
            ret_ty: f.ret.clone(),
            body: Some(&f.body),
            pd_init: None,
            pd_global: None,
            params: f.params.clone(),
            env_ty,
            env_slot: None,
            debug_scope: g.debug_scopes.get(&f.name).copied(),
            decl_span: Some(f.span),
            alloca_after: None,
            frame: None,
            registered: Vec::new(),
            naked: f.naked,
            loops: Vec::new(),
            breaks_emitted: 0,
            regions: Vec::new(),
            loop_regions: Vec::new(),
        })
    }

    /// The synthetic walk a `pd` initializer is emitted through (MK7 §5): no
    /// body, no parameters, and one expression whose result is stored into the
    /// passdown's own global.
    ///
    /// `fn_name` is empty and the module is the declaring one, exactly as
    /// the passdown globals emitter builds its `Gen`: the drop plan has no
    /// entry keyed to a `pd` (`borrowck.rs` keys by function, and an initializer
    /// is not one), and a gated call inside the initializer is checked against
    /// the module that declared it, which is the only carrier of that module.
    pub(crate) fn for_pd_init(
        g: &'g Gen<'a>,
        pd: &'a Pd,
        cur_fn: LlvmFunction<'a>,
        entry: BasicBlock<'a>,
        start: BasicBlock<'a>,
        global: Value<'a>,
    ) -> Result<Self, String> {
        Ok(FnGen {
            g,
            cur_fn,
            entry,
            cur: start,
            terminated: false,
            scopes: Vec::new(),
            bindings: HashMap::new(),
            fn_name: String::new(),
            module: pd.module.clone(),
            ret_ty: pd.value_ty.clone(),
            body: None,
            pd_init: Some(&pd.init),
            pd_global: Some(global),
            params: Vec::new(),
            env_ty: None,
            env_slot: None,
            debug_scope: g.pd_init_scope,
            decl_span: Some(pd.init.span),
            alloca_after: None,
            frame: None,
            registered: Vec::new(),
            naked: false,
            loops: Vec::new(),
            breaks_emitted: 0,
            regions: Vec::new(),
            loop_regions: Vec::new(),
        })
    }

    /// Aim the builder at the `.kp` line `span` names, so every instruction
    /// emitted until the next call to this carries it (MK7 S4, §4.4).
    ///
    /// This is the LLVM half of the line-marker emitter, called from
    /// the same places with the same span, so the two backends put a statement
    /// at the same source line. A span the source map cannot resolve, or a
    /// function with no subprogram, clears the location instead: an instruction
    /// that names no line is honest, and one that names a line it did not come
    /// from is not.
    pub(crate) fn locate(&self, span: Span) {
        self.g.locate(self.debug_scope, &self.module, span);
        // Kernel stage 15: the statement's own file and line, left where a
        // check inside a generated body can read them. A `naked fun` gets
        // nothing: its body is one `asm` and any instruction before it would
        // break the ABI the asm is the whole of (kernel stage 5).
        if self.g.freestanding.is_some() && !self.naked {
            self.g.store_panic_site(&self.module, span);
        }
    }

    /// Start the function under its own declaration line, which is what makes
    /// the entry block, and so the prologue, resolve to the function rather
    /// than to the previous function's last statement.
    pub(crate) fn locate_entry(&self) {
        match self.decl_span {
            Some(span) => self.locate(span),
            None => self.g.builder.clear_debug_location(),
        }
    }

    /// Emit one `pd` initializer and store its value into the passdown's
    /// global. Returns whether it diverged, which is what stops the enclosing
    /// `kelp_init_pds` from assigning a later passdown into a block that has
    /// already ended.
    pub(crate) fn emit_pd_init(mut self) -> Result<(bool, BasicBlock<'a>), String> {
        self.g.builder.position_at_end(self.cur);
        // The initializer is a `.kp` expression, so it resolves to its own line
        // even though it is emitted into a generated function (MK7 S4).
        self.locate_entry();
        self.scopes.push(HashMap::new());
        let init = self
            .pd_init
            .ok_or_else(|| no_value("a passdown initializer"))?;
        let value = self.emit_expr(init)?;
        if !self.terminated {
            let value = value.ok_or_else(|| no_value("the value of a passdown initializer"))?;
            let global = self
                .pd_global
                .ok_or_else(|| no_value("the storage of a passdown initializer"))?;
            self.g.builder.build_store(value, global);
        }
        Ok((self.terminated, self.cur))
    }

    pub(crate) fn emit(mut self) -> Result<(), String> {
        self.g.builder.position_at_end(self.entry);
        // The entry block starts under the function's own line: the parameter
        // slots and the `kelp_init_pds` call are part of the function, so they
        // must not carry whatever the previously emitted function left behind
        // (MK7 S4).
        self.locate_entry();
        self.scopes.push(HashMap::new());

        // A `pd` init function's body is the initializer walk, called first by
        // `main`; the passdowns themselves are globals, so nothing is
        // registered here.
        if self.pd_init.is_some() {
            self.emit_pd_init()?;
            return Ok(());
        }

        // A spawning program gives every function a cleanup frame (decision
        // 125): three pointer words, the record `kelp_cleanup_enter` fills.
        if self.g.spawn_frames && !self.naked {
            let frame_ty = self.g.ctx.array_type(self.g.pty(), 3);
            self.frame = Some(self.entry_alloca_raw(frame_ty, "cleanup.frame"));
        }

        // `main` runs the passdown initializers before its first statement,
        // exactly where the function emitter inserts
        // `kelp_init_pds();`, and saves its own `argc`/`argv` into the
        // runtime's globals, where `process.args()` reads them. `main`
        // receives no hidden parameter, so it reads the generated globals
        // directly; the slots a referencing function gets are bound below for
        // every other function.
        if self.fn_name == "main" {
            // Managed: the collector starts first (paper 3.4), with the minimum
            // heap and the manifest's `heap_max` as the maximum.
            if let (Some(gc), Some(limits)) = (&self.g.gc, self.g.managed_heap) {
                let (init, ity) = gc.init;
                let min = self.g.ctx.const_int(self.g.ctx.i64(), crate::gc_descr::DEFAULT_HEAP_MIN, false);
                let max = self.g.ctx.const_int(self.g.ctx.i64(), limits.max, false);
                let low = self.g.ctx.const_int(self.g.ctx.i64(), limits.start_low, false);
                let high = self.g.ctx.const_int(self.g.ctx.i64(), limits.start_high, false);
                let headroom = self.g.ctx.const_int(self.g.ctx.i64(), limits.headroom_pct, false);
                let rule = self.g.ctx.const_int(self.g.ctx.i64(), limits.trigger_rule, false);
                self.g.builder.build_call(init, ity, &[min, max, low, high, headroom, rule], "");
            }
            // A program with block regions attaches its main thread to the
            // collector before the first block is made (`block::alloc` needs
            // the thread attached); the heap keeps the runtime's defaults.
            if let Some(gc) = &self.g.gc {
                let (attach, aty) = gc.thread_attach;
                self.g.builder.build_call(attach, aty, &[], "");
            }
            if let Some(init) = self.g.init_pds_fn {
                let ty = self.g.ctx.fn_type(self.g.ctx.void(), &[], false);
                self.g.builder.build_call(init, ty, &[], "");
            }
            let argc = self.cur_fn.get_param(0);
            let argv = self.cur_fn.get_param(1);
            self.g.builder.build_store(argc, self.g.argc_global);
            self.g.builder.build_store(argv, self.g.argv_global);
        }

        // A literal that captured something receives its environment as a
        // hidden first parameter (S2E). Bind it into a slot of its own, so
        // every `Capture` read loads one location.
        let param_offset = if self.env_ty.is_some() {
            let raw = self.cur_fn.get_param(0);
            let slot = self.temp_alloca(self.g.pty(), "env");
            self.g.builder.build_store(raw, slot);
            self.env_slot = Some(slot);
            1
        } else {
            0
        };

        // Bind each parameter into its own slot, so its name is a load like any
        // other local. A `unit` parameter has no slot and no LLVM parameter of
        // its own: a read of it names a value with no representation, which
        // `emit_expr` answers with no value, so the index into the LLVM
        // signature only advances for the parameters that were passed.
        if self.fn_name != "main" {
            let mut llvm_index = param_offset;
            for (i, (name, ty)) in self.params.clone().iter().enumerate() {
                if *ty == Ty::Unit {
                    continue;
                }
                let value = self.cur_fn.get_param(llvm_index);
                llvm_index += 1;
                let slot = self.alloca(ty, name)?;
                self.g.builder.build_store(value, slot);
                self.scopes
                    .last_mut()
                    .expect("the function scope is open")
                    .insert(name.clone(), slot);
                self.bindings.insert(BindingId::Param(i), slot);
                // A parameter the function owns is dropped by its frame on a
                // panic, as a local is (decision 125; borrowck's `define_owned`).
                self.register_owned(ty, slot)?;
            }
        }

        let body = self
            .body
            .ok_or_else(|| no_value("the body of a function"))?;
        self.emit_block_body(body)?;

        if !self.terminated {
            // A naked function's asm is what returns; a `return` of the
            // declared type would be an instruction the attribute forbids.
            //
            // A body ending in `while true { ... }` never falls through
            // either: the loop has no exit edge, so the block this point is
            // emitted into has no predecessor and no value is ever returned
            // here. A declared type with no zero value (`String`, `Bytes`, a
            // container) has nothing to `ret`, which is the case that made
            // this a refusal rather than an unreachable block.
            if self.naked {
                self.g.builder.build_unreachable();
                self.terminated = true;
            } else {
                self.emit_fallthrough_return()?;
            }
        }
        if let Some(frame) = self.frame {
            self.emit_frame_setup(frame)?;
        }
        Ok(())
    }

    /// Builds this function's handler and the code that enters the frame
    /// (decision 125). The handler is `<name>.unwind`: it takes the registry, a
    /// table of the registered slots' addresses, and drops each slot with its
    /// type. A null slot drops nothing, and a slot is cleared at every move and
    /// after its drop, so the handler drops only what the function still owns.
    /// The enter call and the registry go into the entry block after its
    /// allocas, so they run before any code of the body.
    pub(crate) fn emit_frame_setup(&mut self, frame: Value<'a>) -> Result<(), String> {
        let registered = std::mem::take(&mut self.registered);
        let ptr = self.g.pty();

        // The handler: one drop per registered slot whose flag is set, read
        // through the registry. The registry holds a pair per slot: the slot's
        // address, then the address of its flag.
        let handler_ty = self.g.ctx.fn_type(self.g.ctx.void(), &[ptr], false);
        let handler = self.g.module.add_function(&format!("{}.unwind", self.fn_name), handler_ty);
        let handler_entry = handler.append_block("entry");
        self.g.builder.position_at_end(handler_entry);
        let registry = handler.get_param(0);
        // Newest first, as on normal scope exit: the registry index is the
        // registration index, so the reversed walk keeps each slot's address.
        for (i, (_, ty, _)) in registered.iter().enumerate().rev() {
            let at = self.g.builder.build_gep(ptr, registry, self.g.v_i64(2 * i as u64), "slot.at");
            let slot = self.g.builder.build_load(ptr, at, "slot");
            let flag_at =
                self.g.builder.build_gep(ptr, registry, self.g.v_i64(2 * i as u64 + 1), "flag.at");
            let flag_ptr = self.g.builder.build_load(ptr, flag_at, "flag.ptr");
            let flag = self.g.builder.build_load(self.g.ctx.i8(), flag_ptr, "flag");
            let live = self.g.builder.build_icmp(
                IntCmp::Ne,
                flag,
                self.g.ctx.const_int(self.g.ctx.i8(), 0, false),
                "live",
            );
            let drop_block = handler.append_block("drop.live");
            let next_block = handler.append_block("drop.next");
            self.g.builder.build_cond_br(live, drop_block, next_block);
            self.g.builder.position_at_end(drop_block);
            self.g.call_drop(slot, ty)?;
            self.g.builder.build_br(next_block);
            self.g.builder.position_at_end(next_block);
        }
        self.g.builder.build_ret_void();

        // The entry block: the registry (its slot addresses, which are all
        // allocas in this block), then the enter call, after the last alloca.
        match self.alloca_after {
            Some(prev) => self.g.builder.position_after(prev, self.entry),
            None => self.g.builder.position_at_start(self.entry),
        }
        let registry_value = if registered.is_empty() {
            self.g.ctx.const_null(ptr)
        } else {
            let table_ty = self.g.ctx.array_type(ptr, 2 * registered.len() as u64);
            let table = self.g.builder.build_alloca(table_ty, "cleanup.slots");
            for (i, (slot, _, flag)) in registered.iter().enumerate() {
                let at = self.g.builder.build_gep(ptr, table, self.g.v_i64(2 * i as u64), "slot.at");
                self.g.builder.build_store(*slot, at);
                let flag_at =
                    self.g.builder.build_gep(ptr, table, self.g.v_i64(2 * i as u64 + 1), "flag.at");
                self.g.builder.build_store(*flag, flag_at);
            }
            table
        };
        let enter = self.g.rt("kelp_cleanup_enter")?;
        self.g.call(
            enter.func,
            enter.ty,
            &[frame, handler.as_value(), registry_value],
            "",
        );
        self.g.builder.position_at_end(self.cur);
        Ok(())
    }

    /// Emit a block's statements and then its tail: as a `return` when the tail
    /// belongs to a value-returning function, discarded otherwise, followed by
    /// the block's end-of-scope drops (the earlier emitter draws both lines in
    /// `write_function_body`).
    pub(crate) fn emit_block_body(&mut self, block: &Block) -> Result<(), String> {
        for stmt in &block.stmts {
            if self.terminated {
                break;
            }
            self.emit_stmt(stmt)?;
        }
        if self.terminated {
            return Ok(());
        }
        let drops = self.drops_at_block_end(block);
        if let Some(tail) = &block.tail {
            // The tail is an expression in statement position here, and the C
            // backend marks it as one (the block body emitter).
            self.locate(tail.span);
            // A body whose tail is a `panic` diverges: `infer.rs` accepts it
            // against any declared return type, so it is a statement rather
            // than a returned value (the function body emitter
            // draws the same line). The call ends the block itself, so no
            // `ret` and no drop follows it.
            if is_diverging_tail(tail) {
                self.emit_expr(tail)?;
                return Ok(());
            }
            if self.ret_ty != Ty::Unit {
                // Read the returned value *before* freeing the frame's locals:
                // the tail may read through one of them.
                let value = if drops.is_empty() {
                    self.value_of(tail)?
                } else {
                    self.value_of_then_drops(tail, &drops)?
                };
                if !self.terminated {
                    self.leave_frame()?;
                    self.g.builder.build_ret(value);
                    self.terminated = true;
                }
            } else {
                self.emit_expr(tail)?;
                if !self.terminated {
                    self.emit_drops(&drops)?;
                }
            }
            return Ok(());
        }
        self.emit_drops(&drops)
    }

    /// Emit a block as an expression: its tail is the block's value, `None` when
    /// the block is unit or already diverged. Used for nested blocks and for the
    /// two arms of an `if`.
    pub(crate) fn emit_block_value(&mut self, block: &Block) -> Result<Option<Value<'a>>, String> {
        self.scopes.push(HashMap::new());
        let result = self.emit_block_value_inner(block);
        self.scopes.pop();
        result
    }

    pub(crate) fn emit_block_value_inner(&mut self, block: &Block) -> Result<Option<Value<'a>>, String> {
        for stmt in &block.stmts {
            if self.terminated {
                break;
            }
            self.emit_stmt(stmt)?;
        }
        if self.terminated {
            return Ok(None);
        }
        let drops = self.drops_at_block_end(block);
        if let Some(tail) = &block.tail {
            self.locate(tail.span);
        }
        match &block.tail {
            // A unit tail is a statement, not a value: it is emitted and the
            // block's own drops follow it, so a `{ ...; call() }` block in
            // statement position costs nothing extra.
            Some(tail) if tail.ty == Ty::Unit => {
                self.emit_expr(tail)?;
                if !self.terminated {
                    self.emit_drops(&drops)?;
                }
                Ok(None)
            }
            // A block's value has to be read before its own end-of-scope drops
            // run, so it passes through a temporary when anything is freed.
            Some(tail) if !drops.is_empty() => Ok(Some(self.value_of_then_drops(tail, &drops)?)),
            Some(tail) => self.emit_expr(tail),
            None => {
                self.emit_drops(&drops)?;
                Ok(None)
            }
        }
    }

    pub(crate) fn emit_stmt(&mut self, stmt: &Stmt) -> Result<(), String> {
        // One location per statement, from the span the statement itself
        // carries: a `Let` is the initializer's span (its first token *is* the
        // `val`), a `Return` carries its own, a `While` its condition's. The
        // same four spans the statement emitter wrote its `#line` from.
        match stmt {
            Stmt::Let { init, .. } => self.locate(init.span),
            Stmt::Return { span, .. } => self.locate(*span),
            Stmt::While { cond, .. } => self.locate(cond.span),
            Stmt::Break { span } | Stmt::Continue { span } => self.locate(*span),
            Stmt::Expr(e) => self.locate(e.span),
        }
        match stmt {
            Stmt::Let { name, ty, init } => {
                // A `[v; N]` local is filled in place (kernel stage 14): building
                // it as an aggregate value and storing it makes LLVM's
                // instcombine constant-fold one `insertvalue` per element, which
                // is enough to exhaust memory on a frame-sized array. The slot is
                // a fresh alloca, so the fill writes it directly.
                if let (Ty::Array(elem, len), ExprKind::ArrayRepeat { value }) = (ty, &init.kind) {
                    let v = self.value_of(value)?;
                    // An initializer that diverged leaves the binding dead.
                    if self.terminated {
                        return Ok(());
                    }
                    let slot = self.alloca(ty, name)?;
                    self.scopes
                        .last_mut()
                        .expect("a scope is always open")
                        .insert(name.clone(), slot);
                    self.bindings.insert(BindingId::Local(stmt_id(stmt)), slot);
                    self.register_owned(ty, slot)?;
                    let symbol =
                        format!("{}__fill_into", crate::types::array_c_type(elem, *len));
                    let (func, fty) = self.g.cfn(&symbol)?;
                    self.g.call(func, fty, &[slot, v], "");
                    return Ok(());
                }
                // A long all-constant array literal is a private constant global
                // plus one `memcpy` into the local (MISSING-Nf3Rb). Handing
                // LLVM the literal as one `[N x T]` aggregate value makes its
                // optimizer quadratic in N (about 3 s at `-O2` for 5,000
                // elements). Short literals keep the aggregate form: their IR
                // is what every other array literal in the corpus looks like.
                if let (Ty::Array(elem, len), ExprKind::ArrayLit(elems)) = (ty, &init.kind) {
                    if elems.len() >= BIG_ARRAY_LITERAL && elems.len() == *len as usize {
                        let mut values = Vec::with_capacity(elems.len());
                        for e in elems {
                            values.push(self.value_of(e)?);
                        }
                        if self.terminated {
                            return Ok(());
                        }
                        let elem_ty = self.lty(elem)?;
                        if values.iter().all(|v| v.is_constant()) {
                            let arr_ty = self.g.ctx.array_type(elem_ty, *len);
                            let table = self.g.ctx.const_array(elem_ty, &values);
                            let global = self.g.module.add_global("kelp.array.lit", arr_ty);
                            global.set_initializer(table);
                            global.set_constant(true);
                            global.set_internal_linkage();
                            let slot = self.alloca(ty, name)?;
                            self.scopes
                                .last_mut()
                                .expect("a scope is always open")
                                .insert(name.clone(), slot);
                            self.bindings.insert(BindingId::Local(stmt_id(stmt)), slot);
                            self.register_owned(ty, slot)?;
                            let bytes = self.g.layout.abi_size_of(arr_ty);
                            let (memcpy, mty) = self.g.mem_fn("memcpy")?;
                            let size = self.g.v_i64(bytes);
                            self.g.call(memcpy, mty, &[slot, global, size], "");
                            return Ok(());
                        }
                        let agg = self.array_lit_from_values(values, ty)?;
                        let slot = self.alloca(ty, name)?;
                        self.scopes
                            .last_mut()
                            .expect("a scope is always open")
                            .insert(name.clone(), slot);
                        self.bindings.insert(BindingId::Local(stmt_id(stmt)), slot);
                        self.register_owned(ty, slot)?;
                        self.g.builder.build_store(agg, slot);
                        return Ok(());
                    }
                }
                let value = self.emit_expr(init)?;
                // An initializer that diverged leaves the binding dead: emitting
                // its store would put an instruction after the block's terminator.
                if self.terminated {
                    return Ok(());
                }
                // A `unit` binding is zero-sized: it gets no slot and no drop,
                // and the initializer's own emission is all the statement has.
                // A read of the name is answered by `emit_expr` with no value,
                // which is what a uniform binding of `unit` costs.
                if *ty == Ty::Unit {
                    return Ok(());
                }
                let slot = self.alloca(ty, name)?;
                self.scopes
                    .last_mut()
                    .expect("a scope is always open")
                    .insert(name.clone(), slot);
                self.bindings.insert(BindingId::Local(stmt_id(stmt)), slot);
                self.register_owned(ty, slot)?;
                match value {
                    Some(v) => {
                        self.g.builder.build_store(v, slot);
                    }
                    None if *ty == Ty::Unit => {}
                    None => return Err(no_value(&format!("the initializer of '{name}'"))),
                }
            }
            Stmt::Return { value, span } => {
                let drops = self.drops_at_return(*span);
                match value {
                    Some(e) => {
                        // A `unit`-typed value has nothing to build a `ret`
                        // from: the expression's side effects are the whole
                        // statement, and the return is the bare return a
                        // written `return` is (which is what makes
                        // `return f()` legal when `f` returns `unit`).
                        if e.ty == Ty::Unit {
                            self.emit_expr(e)?;
                            if !self.terminated {
                                self.release_regions_from(0)?;
                                self.emit_drops(&drops)?;
                                self.emit_fallthrough_return()?;
                            }
                        } else {
                            let v = if drops.is_empty() && self.regions.is_empty() {
                                self.value_of(e)?
                            } else {
                                self.value_of_then_exit(e, &drops, 0)?
                            };
                            if !self.terminated {
                                self.leave_frame()?;
                                self.g.builder.build_ret(v);
                            }
                        }
                    }
                    None => {
                        self.release_regions_from(0)?;
                        self.emit_drops(&drops)?;
                        self.emit_fallthrough_return()?;
                    }
                }
                self.terminated = true;
            }
            Stmt::While { cond, body, .. } => self.emit_while(cond, body)?,
            // Free the loop's own owned locals, then branch. `self.terminated`
            // stops the rest of the block, exactly as a `return` does.
            Stmt::Break { span } => {
                let drops = self.drops_at_break(*span);
                let from = self.loop_region_depth()?;
                self.release_regions_from(from)?;
                self.emit_drops(&drops)?;
                self.breaks_emitted += 1;
                if !self.terminated {
                    let (_, exit) = *self
                        .loops
                        .last()
                        .ok_or_else(|| "a 'break' outside a loop".to_string())?;
                    self.g.builder.build_br(exit);
                }
                self.terminated = true;
            }
            Stmt::Continue { span } => {
                let drops = self.drops_at_continue(*span);
                let from = self.loop_region_depth()?;
                self.release_regions_from(from)?;
                self.emit_drops(&drops)?;
                if !self.terminated {
                    let (header, _) = *self
                        .loops
                        .last()
                        .ok_or_else(|| "a 'continue' outside a loop".to_string())?;
                    self.g.builder.build_br(header);
                }
                self.terminated = true;
            }
            Stmt::Expr(e) => {
                self.emit_expr(e)?;
            }
        }
        Ok(())
    }

    /// A manual block region (`block name: T { ... }`, decision 139). Its two
    /// slots start null and zero, the body is emitted as a block whose value
    /// is the region's value, and the region's release runs after the body,
    /// before the value is used. A `name.box()` in the body (lowered with the
    /// block's entry points) fills the slots.
    pub(crate) fn emit_region(&mut self, name: &str, elem: &Ty, body: &Block) -> Result<Option<Value<'a>>, String> {
        let ptr = self.g.ctx.opaque_ptr();
        let i64 = self.g.ctx.i64();
        let base = self.entry_alloca_raw(ptr, &format!("block.{name}.base"));
        let handle = self.entry_alloca_raw(i64, &format!("block.{name}.handle"));
        self.g.builder.build_store(self.g.ctx.const_null(ptr), base);
        self.g.builder.build_store(self.g.ctx.const_int(i64, 0, false), handle);
        self.regions.push(RegionSlot {
            name: name.to_string(),
            elem: elem.clone(),
            base,
            handle,
        });
        let value = self.emit_block_value(body);
        let slot = self.regions.pop().expect("the region was pushed above");
        let value = value?;
        if !self.terminated {
            self.release_region(&slot)?;
        }
        Ok(value)
    }

    /// Release the regions opened at or after depth `from`, innermost first, on
    /// the way out of a `return`, `break`, `continue` or `?`.
    pub(crate) fn release_regions_from(&mut self, from: usize) -> Result<(), String> {
        let slots: Vec<RegionSlot<'a>> = self.regions[from..].iter().rev().cloned().collect();
        for slot in &slots {
            self.release_region(slot)?;
        }
        Ok(())
    }

    /// Evaluate `e`, store it, release the regions from depth `from`, then run
    /// `drops`: the exit-path form of `value_of_then_drops`. Regions go first,
    /// because a region's element never refers to an owned local.
    pub(crate) fn value_of_then_exit(&mut self, e: &Expr, drops: &[DropTarget], from: usize) -> Result<Value<'a>, String> {
        let value = self.value_of(e)?;
        if self.terminated {
            return Ok(value);
        }
        let lty = self.lty(&e.ty)?;
        let slot = self.temp_alloca(lty, "exit.tmp");
        self.g.builder.build_store(value, slot);
        self.release_regions_from(from)?;
        self.emit_drops(drops)?;
        Ok(self.g.builder.build_load(lty, slot, "exit.val"))
    }

    /// The index of variant `variant` of the enum `name`, from the program's tables.
    pub(crate) fn variant_index(&self, name: &str, variant: &str) -> Result<usize, String> {
        self.g
            .program
            .enums
            .iter()
            .find(|e| e.name == name)
            .and_then(|e| e.variants.iter().position(|v| v.name == variant))
            .ok_or_else(|| no_value(&format!("the variant '{variant}' of '{name}'")))
    }

    /// Write the enum `name`'s variant `variant` with `values` as its payload into
    /// `slot`, the way `emit_enum_lit` writes a literal (a fresh slot per value).
    pub(crate) fn store_enum_into(&mut self, slot: Value<'a>, name: &str, variant: usize, values: &[Value<'a>]) -> Result<(), String> {
        let info = self
            .g
            .enums
            .get(name)
            .cloned()
            .ok_or_else(|| no_value(&format!("the enum '{name}'")))?;
        self.g.builder.build_store(self.g.ctx.const_null(info.llvm_ty), slot);
        let tag_ptr = self.g.builder.build_struct_gep(info.llvm_ty, slot, ENUM_TAG_FIELD, "tag");
        let tag = self.g.ctx.const_int(info.tag_ty, variant as u64, false);
        self.g.builder.build_store(tag, tag_ptr);
        if values.is_empty() {
            return Ok(());
        }
        let union_ptr = self.g.builder.build_struct_gep(info.llvm_ty, slot, ENUM_UNION_FIELD, "kelp_v");
        let payload_ty = info
            .variants
            .get(variant)
            .ok_or_else(|| no_value(&format!("the variant {variant} of '{name}'")))?
            .payload_ty;
        for (index, value) in values.iter().enumerate() {
            let field_ptr = self.g.builder.build_struct_gep(payload_ty, union_ptr, index as u32, "field");
            self.g.builder.build_store(*value, field_ptr);
        }
        Ok(())
    }

    /// `name.box()` (decision 139): one zeroed element of `elem` in the region
    /// `name`, as `Ok(box)`, or `Err(BlockError.Full)` when the collector refuses.
    /// A box made again in the same region (a loop body) first frees the element
    /// the last one made: its `let` is dead by then, and the region holds one.
    pub(crate) fn emit_block_box(&mut self, name: &str, elem: &Ty, ty: &Ty) -> Result<Value<'a>, String> {
        let Ty::Struct(rname) = ty else {
            return Err(format!("the box of the block '{name}' has no Result type"));
        };
        let rname = rname.clone();
        let slot = self
            .regions
            .iter()
            .rev()
            .find(|r| r.name == name)
            .cloned()
            .ok_or_else(|| no_value(&format!("the block '{name}'")))?;
        let ptr = self.g.ctx.opaque_ptr();
        let i64 = self.g.ctx.i64();
        let i32 = self.g.ctx.i32();
        self.release_region(&slot)?;
        self.g.builder.build_store(self.g.ctx.const_null(ptr), slot.base);
        self.g.builder.build_store(self.g.ctx.const_int(i64, 0, false), slot.handle);
        let gc = self.g.gc.as_ref().ok_or_else(|| no_value("the collector for a block box"))?;
        let descr = gc.descriptor(self.g.ctx, self.g.module, self.g, elem)?;
        let alloc = self.g.rt("kelp_gc_block_alloc")?;
        let status = self.g.call(alloc.func, alloc.ty, &[descr, slot.base, slot.handle], "block.status");
        let zero = self.g.ctx.const_int(i32, 0, false);
        let made = self.g.builder.build_icmp(IntCmp::Eq, status, zero, "block.made");
        let res_lty = self.lty(ty)?;
        let res = self.temp_alloca(res_lty, "box.res");
        let made_bb = self.cur_fn.append_block("box.made");
        let full_bb = self.cur_fn.append_block("box.full");
        let join = self.cur_fn.append_block("box.join");
        self.g.builder.build_cond_br(made, made_bb, full_bb);

        self.g.builder.position_at_end(made_bb);
        self.cur = made_bb;
        let base = self.g.builder.build_load(ptr, slot.base, "box.base");
        let ok = self.variant_index(&rname, "Ok")?;
        self.store_enum_into(res, &rname, ok, &[base])?;
        self.g.builder.build_br(join);

        self.g.builder.position_at_end(full_bb);
        self.cur = full_bb;
        let be_name = crate::infer::block_error_name();
        let be_lty = self.lty(&Ty::Struct(be_name.clone()))?;
        let be_slot = self.temp_alloca(be_lty, "block.error");
        self.store_enum_into(be_slot, &be_name, 0, &[])?;
        let be = self.g.builder.build_load(be_lty, be_slot, "block.full");
        let err = self.variant_index(&rname, "Err")?;
        self.store_enum_into(res, &rname, err, &[be])?;
        self.g.builder.build_br(join);

        self.g.builder.position_at_end(join);
        self.cur = join;
        self.terminated = false;
        Ok(self.g.builder.build_load(res_lty, res, "box.val"))
    }

    /// Free the block of one region: `kelp_gc_block_free(handle, base, descr)`.
    /// The runtime does nothing for a null `base`, so a region whose box never
    /// ran costs one call and no free.
    pub(crate) fn release_region(&mut self, slot: &RegionSlot<'a>) -> Result<(), String> {
        let ptr = self.g.ctx.opaque_ptr();
        let i64 = self.g.ctx.i64();
        let gc = self.g.gc.as_ref().ok_or_else(|| no_value("the collector for a block region"))?;
        let descr = gc.descriptor(self.g.ctx, self.g.module, self.g, &slot.elem)?;
        let rt = self.g.rt("kelp_gc_block_free")?;
        let base = self.g.builder.build_load(ptr, slot.base, &format!("free.{}.base", slot.name));
        let h = self.g.builder.build_load(i64, slot.handle, &format!("free.{}.handle", slot.name));
        self.g.call(rt.func, rt.ty, &[h, base, descr], "");
        Ok(())
    }

    /// The region depth of the innermost loop being emitted.
    pub(crate) fn loop_region_depth(&self) -> Result<usize, String> {
        self.loop_regions
            .last()
            .copied()
            .ok_or_else(|| "a 'break' or 'continue' outside a loop".to_string())
    }

    pub(crate) fn drops_at_block_end(&self, block: &Block) -> Vec<DropTarget> {
        self.g.drops.at_block_end(&self.fn_name, block).to_vec()
    }

    pub(crate) fn drops_at_return(&self, span: Span) -> Vec<DropTarget> {
        self.g.drops.at_return(&self.fn_name, span).to_vec()
    }

    pub(crate) fn drops_at_assign(&self, span: Span) -> Vec<DropTarget> {
        self.g.drops.at_assign(&self.fn_name, span).to_vec()
    }

    /// Owned locals freed by the `?` at `span`, on its failure arm only: the same
    /// set and the same order a written `return` frees (`DropPlan::at_try`).
    pub(crate) fn drops_at_try(&self, span: Span) -> Vec<DropTarget> {
        self.g.drops.at_try(&self.fn_name, span).to_vec()
    }

    pub(crate) fn drops_at_break(&self, span: Span) -> Vec<DropTarget> {
        self.g.drops.at_break(&self.fn_name, span).to_vec()
    }

    pub(crate) fn drops_at_continue(&self, span: Span) -> Vec<DropTarget> {
        self.g.drops.at_continue(&self.fn_name, span).to_vec()
    }

    /// Free the still-live owned locals the plan names, in the plan's order
    /// (reverse declaration order, so a later binding is freed before an earlier
    /// one it may have taken its value from).
    pub(crate) fn emit_drops(&mut self, targets: &[DropTarget]) -> Result<(), String> {
        for t in targets {
            let slot = self
                .drop_slot(t)
                .ok_or_else(|| no_value(&format!("the owned local '{}'", t.name)))?;
            self.g.call_drop(slot, &t.ty)?;
            // A spawning program clears the slot the drop freed, so the frame
            // finds nothing there on a panic (decision 125).
            if self.g.spawn_frames && self.g.owns_memory(&t.ty) {
                self.clear_owned(slot, &t.ty)?;
            }
        }
        Ok(())
    }

    /// The slot a drop of `target` frees: the alloca the binding's own
    /// declaration made, so a rebound or shadowed spelling frees the value the
    /// declaration owns and not whichever binding of that spelling is newest
    /// (BUG-n7Wq3). A target the plan recorded no binding for (an assignment to
    /// a `pd`'s storage) and one this walk did not declare fall back to the
    /// resolution a read of the name uses.
    pub(crate) fn drop_slot(&self, target: &DropTarget) -> Option<Value<'a>> {
        target
            .binding
            .and_then(|binding| self.bindings.get(&binding).copied())
            .or_else(|| self.lookup(&target.name))
    }

    /// Evaluate `e`, store it in a temporary, then run `drops`. A returned or
    /// block-tailed value may read through a local being freed, so it has to be
    /// read first (the temporary-then-drops rule). An expression that
    /// diverged leaves nothing to store and no drop to run.
    pub(crate) fn value_of_then_drops(
        &mut self,
        e: &Expr,
        drops: &[DropTarget],
    ) -> Result<Value<'a>, String> {
        let value = self.value_of(e)?;
        if self.terminated {
            return Ok(value);
        }
        let lty = self.lty(&e.ty)?;
        let slot = self.temp_alloca(lty, "drop.tmp");
        self.g.builder.build_store(value, slot);
        self.emit_drops(drops)?;
        Ok(self.g.builder.build_load(lty, slot, "drop.val"))
    }

    /// A managed build's safepoint poll at the current position (paper 5.1):
    /// one call to `kelp_gc_safepoint`, which the collector answers with one
    /// load when no collection is requested. Nothing in a hosted build.
    pub(crate) fn gc_poll(&self) {
        if let Some(gc) = &self.g.gc {
            let (func, fty) = gc.safepoint;
            self.g.builder.build_call(func, fty, &[], "");
        }
    }

    pub(crate) fn emit_while(&mut self, cond: &Expr, body: &Block) -> Result<(), String> {
        let header = self.cur_fn.append_block("while.cond");
        let body_bb = self.cur_fn.append_block("while.body");
        let exit_bb = self.cur_fn.append_block("while.exit");
        self.g.builder.build_br(header);

        self.g.builder.position_at_end(header);
        self.cur = header;
        self.terminated = false;
        let c = self.value_of(cond)?;
        self.g.builder.build_cond_br(c, body_bb, exit_bb);

        self.g.builder.position_at_end(body_bb);
        self.cur = body_bb;
        self.terminated = false;
        // A `continue` branches to the condition and a `break` to the exit;
        // both run their loop's own drops first (see `emit_stmt`).
        self.loops.push((header, exit_bb));
        self.loop_regions.push(self.regions.len());
        let breaks_before = self.breaks_emitted;
        let body = self.emit_block_value(body);
        self.loop_regions.pop();
        self.loops.pop();
        body?;
        if !self.terminated {
            // Managed: a safepoint poll on every back edge (paper 5.1), so a
            // loop with no call still lets a collection stop this thread.
            self.gc_poll();
            let back = self.g.builder.build_br(header);
            // A loop whose bound is a small literal is unrolled in full, so its
            // body is the straight line of steps the SLP vectorizer packs `nbody`
            // across. See `FORCED_UNROLL_BOUND`.
            if forced_unroll_bound(cond).is_some() {
                self.g.ctx.mark_loop_unroll_full(back);
            }
        }

        self.g.builder.position_at_end(exit_bb);
        self.cur = exit_bb;
        self.terminated = false;
        // `while true` with no `break` in its body has no exit edge: the only
        // ways out are a `return` or a `panic`, so the exit block is
        // unreachable and nothing after the loop (including a function's
        // fallthrough return, which a type with no zero value cannot have) is
        // emitted. A loop that does `break` keeps its exit block live.
        if matches!(cond.kind, ExprKind::BoolLit(true)) && self.breaks_emitted == breaks_before {
            self.g.builder.build_unreachable();
            self.terminated = true;
        }
        Ok(())
    }
}
