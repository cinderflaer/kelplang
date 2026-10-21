use super::*;

// ===========================================================================
// The closures (MK7 S2E): function values and the fat pointer a capturing
// `fun` literal becomes. The earlier emitter's shape is mirrored member for member
// (see the earlier emitter's `write_closure_typedef` and its neighbours): the value is
// `{ fn, env, drop }`, the code pointer takes the environment first, the
// environment and the widening's one-field wrapper are heap, and the only
// function in a signature's own set is the drop. Per-instantiation code is
// generated into the module, which is MK7 §3.2's one exception to "the runtime
// stays C".
// ===========================================================================

impl<'a> Gen<'a> {
    /// The LLVM types of one closure signature: its own parameters and its
    /// return, with `unit` becoming `void`. The fat pointer's own struct type is
    /// in `containers`, keyed by the `tir` closure type.
    pub(crate) fn closure_sig_types(&self, ty: &Ty) -> Result<(Vec<Type<'a>>, Type<'a>), String> {
        let (params, ret) = match ty {
            Ty::Closure { params, ret } | Ty::ScopedFn { params, ret } => (params, ret),
            _ => return Err(no_value("a closure signature")),
        };
        let params = params
            .iter()
            .map(|p| self.lty2(p))
            .collect::<Result<Vec<_>, _>>()?;
        let ret = if **ret == Ty::Unit {
            self.ctx.void()
        } else {
            self.lty2(ret)?
        };
        Ok((params, ret))
    }

    /// The code-pointer type of one closure signature: the environment first,
    /// then the closure's own parameters. The `fn` member, the widening thunk
    /// and the shim are all declared with it.
    pub(crate) fn closure_code_ty(&self, ty: &Ty) -> Result<FnType<'a>, String> {
        let (params, ret) = self.closure_sig_types(ty)?;
        let mut all = Vec::with_capacity(params.len() + 1);
        all.push(self.pty());
        all.extend(params);
        Ok(self.ctx.fn_type(ret, &all, false))
    }

    /// The struct type of a closure value, looking through a borrow: a
    /// `Ty::Ref` of a closure is the pointer a parameter of one takes, so the
    /// struct is the pointee's.
    pub(crate) fn closure_value_ty(&self, ty: &Ty) -> Result<Type<'a>, String> {
        match ty {
            Ty::Ref { inner, .. } => self.closure_value_ty(inner),
            Ty::Closure { .. } | Ty::ScopedFn { .. } => self.cty(ty),
            other => Err(refuse_type(other)),
        }
    }

    /// The `(params, ret)` LLVM types of a plain function value, looking
    /// through a borrow. `unit` becomes `void`.
    pub(crate) fn fn_value_sig(&self, ty: &Ty) -> Result<(Vec<Type<'a>>, Type<'a>), String> {
        let inner = match ty {
            Ty::Ref { inner, .. } => inner.as_ref(),
            other => other,
        };
        let Ty::Fn { params, ret } = inner else {
            return Err(no_value("a function value"));
        };
        // A `unit` parameter is not a machine parameter, exactly as it is not
        // one in the declaration `declare_functions` writes, so it is filtered
        // out here too and the call lowerer drops the matching argument.
        let params = params
            .iter()
            .filter(|p| **p != Ty::Unit)
            .map(|p| self.lty2(p))
            .collect::<Result<Vec<_>, _>>()?;
        let ret = if **ret == Ty::Unit {
            self.ctx.void()
        } else {
            self.lty2(ret)?
        };
        Ok((params, ret))
    }

    /// One generated S2E function by its symbol, so the lowering can take its
    /// address or call it. Every symbol in the map was declared in
    /// `declare_closure_fns`, before any body that names one.
    pub(crate) fn closure_symbol(&self, symbol: &str) -> Result<LlvmFunction<'a>, String> {
        self.closure_fns
            .get(symbol)
            .map(|(func, _)| *func)
            .ok_or_else(|| no_value(&format!("the closure function '{symbol}'")))
    }

    /// The environment struct of every capturing literal in the program: one
    /// field per capture, in capture order, the layout `write_env` gives the C
    /// struct. Emitted after every aggregate type exists, so a captured
    /// `datac` names a complete type, and before any signature, so the hidden
    /// `ptr` it is passed as needs nothing else.
    pub(crate) fn declare_closure_envs(&mut self) -> Result<(), String> {
        for f in &self.program.functions {
            let Some(env) = &f.env else {
                continue;
            };
            let fields = env
                .fields
                .iter()
                .map(|(_, ty)| llvm_type(self.ctx, &self.structs, &self.containers, ty))
                .collect::<Result<Vec<_>, _>>()?;
            let ty = self.ctx.struct_type(&env.struct_name);
            self.ctx.struct_set_body(ty, &fields);
            self.env_types.insert(f.name.clone(), ty);
        }
        Ok(())
    }

    /// Declare the S2E machinery: the per-signature drop, one environment
    /// destructor per capturing literal, the widening thunk and its
    /// environment's destructor, and the shim a known function value is widened
    /// through. One name each, the same names the earlier emitter gives them, so the
    /// IR reads like the generated C beside it.
    pub(crate) fn declare_closure_fns(&mut self) -> Result<(), String> {
        let inst = self.instantiations.clone();
        let ptr = self.pty();
        let void = self.ctx.void();

        let mut closures: Vec<Ty> = inst.closures.iter().cloned().collect();
        closures.sort_by_key(container_sort_name);
        for sig in &closures {
            // A scoped function value owns nothing (stack environment, `NULL`
            // drop), so it has no per-signature drop to declare.
            if matches!(sig, Ty::ScopedFn { .. }) {
                continue;
            }
            let symbol = drop_fn_name(sig);
            let ty = self.ctx.fn_type(void, &[ptr], false);
            let func = self.module.add_function(&symbol, ty);
            self.closure_fns.insert(symbol, (func, ty));
        }

        // One destructor per capturing literal (S2E): the function the closure
        // value carries in its `drop` member, which frees the environment the
        // captures were moved into. It is a separate function from the
        // per-signature closure drop because the environment's layout belongs
        // to the one literal that made it, exactly as in `write_env`.
        for f in &self.program.functions {
            let Some(env) = &f.env else {
                continue;
            };
            let ty = self.ctx.fn_type(void, &[ptr], false);
            let func = self.module.add_function(&env.drop_name, ty);
            self.closure_fns.insert(env.drop_name.clone(), (func, ty));
        }

        let mut coerced: Vec<Ty> = inst.coerced.iter().cloned().collect();
        coerced.sort_by_key(container_sort_name);
        for sig in &coerced {
            let code_ty = self.closure_code_ty(sig)?;
            let thunk = coerce_thunk_symbol(sig);
            let func = self.module.add_function(&thunk, code_ty);
            self.closure_fns.insert(thunk, (func, code_ty));
            let drop = coerce_drop_symbol(sig);
            let dty = self.ctx.fn_type(void, &[ptr], false);
            let dfunc = self.module.add_function(&drop, dty);
            self.closure_fns.insert(drop, (dfunc, dty));
        }

        let mut shims: Vec<(String, Ty)> = inst.shims.iter().cloned().collect();
        shims.sort_by_key(|(key, _)| key.clone());
        for (key, sig) in &shims {
            let code_ty = self.closure_code_ty(sig)?;
            let symbol = shim_symbol(key, sig);
            let func = self.module.add_function(&symbol, code_ty);
            self.closure_fns.insert(symbol, (func, code_ty));
        }

        // One worker body per `thread.spawn` signature (MK9-S3-M1 stage C1):
        // pointer parameters only, because the runtime's C trampoline is its
        // caller.
        let mut spawns: Vec<Ty> = inst.spawns.iter().cloned().collect();
        spawns.sort_by_key(container_sort_name);
        for sig in &spawns {
            let symbol = thread_body_symbol(sig);
            let ty = self.ctx.fn_type(void, &[ptr, ptr, ptr, ptr], false);
            let func = self.module.add_function(&symbol, ty);
            self.closure_fns.insert(symbol, (func, ty));
        }
        Ok(())
    }

    /// Declare the `&dyn` machinery (A-12 slice 4): one thunk per vtable slot
    /// and one read-only `[N x ptr]` table per `(trait, type)` pair. The table's
    /// initializer holds the thunks' addresses, so both are declared here, after
    /// the container and aggregate types a slot's signature may name and before
    /// any function body.
    pub(crate) fn declare_dyn_fns(&mut self) -> Result<(), String> {
        let ptr = self.ctx.opaque_ptr();
        let i64t = self.ctx.i64();
        let vtables = self.program.dyn_vtables.clone();
        for vtable in &vtables {
            let mut elements: Vec<Value<'a>> = Vec::with_capacity(vtable.slots.len());
            for slot in &vtable.slots {
                let symbol = crate::types::dyn_thunk_symbol(
                    &vtable.trait_name,
                    &vtable.type_name,
                    &slot.method,
                );
                let mut params: Vec<Type<'a>> = vec![ptr];
                for p in &slot.params[1..] {
                    if *p == Ty::Unit {
                        continue;
                    }
                    params.push(self.lty2(p)?);
                }
                let ret = if slot.ret == Ty::Unit {
                    self.ctx.void()
                } else {
                    self.lty2(&slot.ret)?
                };
                let ty = self.ctx.fn_type(ret, &params, false);
                let func = self.module.add_function(&symbol, ty);
                self.dyn_fns.insert(symbol, (func, ty));
                elements.push(func.as_value());
            }
            // The drop entry's thunk (A-12 slice 7): the concrete type's own
            // drop glue, emitted by `emit_dyn_bodies`. Its body is the twin of
            // the earlier emitter's `write_dyn_drop_thunk`.
            let drop_symbol =
                crate::types::dyn_drop_thunk_symbol(&vtable.trait_name, &vtable.type_name);
            let drop_ty = self.ctx.fn_type(self.ctx.void(), &[ptr], false);
            let drop_func = self.module.add_function(&drop_symbol, drop_ty);
            self.dyn_fns.insert(drop_symbol, (drop_func, drop_ty));

            // The table is `{ [N x ptr], drop, size, align }`: the method
            // slots keep the array shape every call site GEPs into, and the
            // three appended fields are what an owned trait object's free reads
            // (the size and align go to the project's own allocator, which
            // needs them; libc's `free` does not).
            let arr_ty = self.ctx.array_type(ptr, elements.len() as u64);
            let table_ty = self.ctx.literal_struct(&[arr_ty, ptr, i64t, i64t]);
            self.dyn_table_tys
                .insert(vtable.trait_name.clone(), table_ty);
            let concrete_lty = self.lty2(&vtable.concrete)?;
            let size = self.layout.abi_size_of(concrete_lty);
            let align = self.layout.abi_alignment_of(concrete_lty) as u64;
            // The table's *definition* is the `_support` unit's in the
            // incremental mode, because a coercion in any unit names it; the
            // unit that writes a `&dyn` only declares it (roadmap A-14 slice
            // 1).
            let global = if self.defines_helpers() {
                let global = self.module.add_global(
                    &crate::types::dyn_vtable_symbol(&vtable.trait_name, &vtable.type_name),
                    table_ty,
                );
                global.set_initializer(self.ctx.const_named_struct(
                    table_ty,
                    &[
                        self.ctx.const_array(ptr, &elements),
                        drop_func.as_value(),
                        self.ctx.const_int(i64t, size, false),
                        self.ctx.const_int(i64t, align, false),
                    ],
                ));
                global.set_constant(true);
                if self.unit.is_none() {
                    global.set_internal_linkage();
                }
                global
            } else {
                self.module.add_external_global(
                    &crate::types::dyn_vtable_symbol(&vtable.trait_name, &vtable.type_name),
                    table_ty,
                )
            };
            self.dyn_tables
                .insert((vtable.trait_name.clone(), vtable.type_name.clone()), global);
        }
        // One drop per *owned* trait object (A-12 slice 7), declared with the
        // other container drops so a `datac` or list that holds one can call it.
        for sym in self.box_dyn_drop_symbols() {
            let ty = self.ctx.fn_type(self.ctx.void(), &[self.ctx.opaque_ptr()], false);
            let func = self.module.add_function(&sym, ty);
            if self.unit.is_none() {
                func.as_value().set_internal_linkage();
            }
            self.container_fns.insert(sym, (func, ty));
        }
        Ok(())
    }

    /// The `kelp_box_dyn_drop__<Trait>` symbols this program needs, sorted so
    /// both backends declare them in the same order.
    pub(crate) fn box_dyn_drop_symbols(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .instantiations
            .box_dyns
            .iter()
            .filter_map(|ty| match ty {
                Ty::BoxDyn { trait_name } => Some(crate::types::box_dyn_drop_fn(trait_name)),
                _ => None,
            })
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// Define every function `declare_closure_fns` declared, in the order the C
    /// backend writes them: the per-signature drop, then the widening's
    /// machinery, then the shims.
    pub(crate) fn emit_closure_bodies(&self) -> Result<(), String> {
        let inst = self.instantiations.clone();
        // Each capturing literal's environment destructor first, because the
        // closure values built below carry its address.
        for f in &self.program.functions {
            if f.env.is_some() {
                self.emit_env_drop(f)?;
            }
        }
        let mut closures: Vec<Ty> = inst.closures.iter().cloned().collect();
        closures.sort_by_key(container_sort_name);
        for sig in &closures {
            if matches!(sig, Ty::ScopedFn { .. }) {
                continue;
            }
            self.emit_closure_drop(sig)?;
        }
        let mut coerced: Vec<Ty> = inst.coerced.iter().cloned().collect();
        coerced.sort_by_key(container_sort_name);
        for sig in &coerced {
            // A scoped widening is only the *shim* (the wrapped value always
            // names a function, and its environment is `null`), so neither the
            // heap thunk nor the drop is reachable for it.
            if matches!(sig, Ty::ScopedFn { .. }) {
                continue;
            }
            self.emit_coerce_drop(sig)?;
            self.emit_coerce_thunk(sig)?;
        }
        let mut shims: Vec<(String, Ty)> = inst.shims.iter().cloned().collect();
        shims.sort_by_key(|(key, _)| key.clone());
        for (key, sig) in &shims {
            self.emit_shim(key, sig)?;
        }
        let mut spawns: Vec<Ty> = inst.spawns.iter().cloned().collect();
        spawns.sort_by_key(container_sort_name);
        for sig in &spawns {
            self.emit_thread_body(sig)?;
        }
        Ok(())
    }

    /// One spawn signature's worker body (MK9-S3-M1 stage C1): load the moved
    /// argument out of its slot, call the closure's code pointer with its
    /// environment, and store the result where the handle expects it. The shape
    /// is the earlier emitter's `write_thread_body`, pointer for pointer, because the
    /// runtime's trampoline is the one caller of either.
    pub(crate) fn emit_thread_body(&self, sig: &Ty) -> Result<(), String> {
        let Ty::Closure { params, ret } = sig else {
            unreachable!("a thread body is emitted per closure signature: {}", sig.display())
        };
        let symbol = thread_body_symbol(sig);
        let (func, _) = *self
            .closure_fns
            .get(&symbol)
            .ok_or_else(|| no_value(&format!("the thread body '{symbol}'")))?;
        let entry = func.append_block("entry");
        self.builder.position_at_end(entry);
        let ptr = self.pty();
        let code = func.get_param(0);
        let env = func.get_param(1);
        let arg_ptr = func.get_param(2);
        let out_ptr = func.get_param(3);
        // Managed: the runtime thread joins the collector before its first
        // allocation. It leaves in the runtime's cleanup handler.
        if let Some(gc) = &self.gc {
            let (attach, aty) = gc.thread_attach;
            self.builder.build_call(attach, aty, &[], "");
        }
        let arg_ty = self.lty2(&params[0])?;
        let arg = self.builder.build_load(arg_ty, arg_ptr, "arg");
        let ret_lty = llvm_ret_type(self.ctx, &self.structs, &self.containers, ret)?;
        let call_ty = self.ctx.fn_type(ret_lty, &[ptr, arg_ty], false);
        let yields = **ret != Ty::Unit;
        let name = if yields { "call" } else { "" };
        let value = self
            .builder
            .build_indirect_call(code, call_ty, &[env, arg], name);
        if yields {
            self.builder.build_store(value, out_ptr);
        }
        // The thread leaves the collector in the runtime's cleanup handler
        // (`kelp_thread_release`), which runs on a normal return and on a
        // worker's panic alike, after the environment's drop.
        self.builder.build_ret_void();
        Ok(())
    }

    /// One capturing literal's environment destructor (S2E): drop each owned
    /// field through its own drop, then free the struct. The same order a
    /// `datac`'s drop uses, one level down (`write_env`).
    pub(crate) fn emit_env_drop(&self, f: &Function) -> Result<(), String> {
        let env = f
            .env
            .as_ref()
            .ok_or_else(|| no_value("the environment destructor of a plain function"))?;
        let (func, _) = *self
            .closure_fns
            .get(&env.drop_name)
            .ok_or_else(|| no_value(&format!("the environment destructor '{}'", env.drop_name)))?;
        let env_ty = *self
            .env_types
            .get(&f.name)
            .ok_or_else(|| no_value(&format!("the environment of '{}'", f.name)))?;
        let entry = func.append_block("entry");
        self.builder.position_at_end(entry);
        let raw = func.get_param(0);
        for (index, (name, ty)) in env.fields.iter().enumerate() {
            if !needs_drop(ty, &self.owned) {
                continue;
            }
            let field = self.builder.build_struct_gep(env_ty, raw, index as u32, name);
            self.call_drop(field, ty)?;
        }
        let bytes = self.v_i64(self.layout.abi_size_of(env_ty));
        let align = self.layout.abi_alignment_of(env_ty) as u64;
        self.heap_dealloc(raw, bytes, align)?;
        self.builder.build_ret_void();
        Ok(())
    }

    /// One closure signature's drop, the only function in its set: free the
    /// environment through the destructor the value carries, then clear the
    /// pointer. The `env` test comes first because a value widened from a plain
    /// function has no environment, and its destructor pointer is `null` too
    /// (`write_closure` draws the same line).
    pub(crate) fn emit_closure_drop(&self, sig: &Ty) -> Result<(), String> {
        let symbol = drop_fn_name(sig);
        let (func, _) = *self
            .closure_fns
            .get(&symbol)
            .ok_or_else(|| no_value(&format!("the closure drop '{symbol}'")))?;
        let closure_ty = self.closure_value_ty(sig)?;
        let ptr = self.pty();
        let void = self.ctx.void();
        let entry = func.append_block("entry");
        self.builder.position_at_end(entry);
        let c = func.get_param(0);
        let env_slot = self.fld(closure_ty, c, 1, "env");
        let env = self.ld(ptr, env_slot, "env");
        let null = self.ctx.const_null(ptr);
        let missing = self.builder.build_icmp(IntCmp::Eq, env, null, "no.env");
        let exit = func.append_block("exit");
        let live = func.append_block("drop");
        self.builder.build_cond_br(missing, exit, live);
        self.builder.position_at_end(live);
        let drop = self.ld(ptr, self.fld(closure_ty, c, 2, "drop"), "drop");
        let dty = self.ctx.fn_type(void, &[ptr], false);
        self.builder.build_indirect_call(drop, dty, &[env], "");
        self.st(null, env_slot);
        self.builder.build_br(exit);
        self.builder.position_at_end(exit);
        self.builder.build_ret_void();
        Ok(())
    }

    /// The destructor of a widened function value's one-field environment: a
    /// plain `free`, because the code pointer it holds owns nothing.
    pub(crate) fn emit_coerce_drop(&self, sig: &Ty) -> Result<(), String> {
        let symbol = coerce_drop_symbol(sig);
        let (func, _) = *self
            .closure_fns
            .get(&symbol)
            .ok_or_else(|| no_value(&format!("the coerce drop '{symbol}'")))?;
        let entry = func.append_block("entry");
        self.builder.position_at_end(entry);
        let raw = func.get_param(0);
        let bytes = self.v_i64(self.layout.abi_size_of(self.coerce_env_ty));
        let align = self.layout.abi_alignment_of(self.coerce_env_ty) as u64;
        self.heap_dealloc(raw, bytes, align)?;
        self.builder.build_ret_void();
        Ok(())
    }

    /// The thunk of a widened function value: read the code pointer back out of
    /// the environment and call through it. One thunk per *signature*, because
    /// what it does is the same for every value of that shape.
    pub(crate) fn emit_coerce_thunk(&self, sig: &Ty) -> Result<(), String> {
        let symbol = coerce_thunk_symbol(sig);
        let (func, _) = *self
            .closure_fns
            .get(&symbol)
            .ok_or_else(|| no_value(&format!("the coerce thunk '{symbol}'")))?;
        let (params, ret) = self.closure_sig_types(sig)?;
        let entry = func.append_block("entry");
        self.builder.position_at_end(entry);
        let raw = func.get_param(0);
        let f = self.ld(self.pty(), self.fld(self.coerce_env_ty, raw, 0, "coerce.f"), "f");
        let args: Vec<Value<'a>> = (0..params.len()).map(|i| func.get_param(i + 1)).collect();
        let call_ty = self.ctx.fn_type(ret, &params, false);
        if closure_is_unit(sig) {
            self.builder.build_indirect_call(f, call_ty, &args, "");
            self.builder.build_ret_void();
        } else {
            let v = self.builder.build_indirect_call(f, call_ty, &args, "call");
            self.builder.build_ret(v);
        }
        Ok(())
    }

    /// The shim of a widened *known* function value: take the environment a
    /// closure's `fn` member is called with, ignore it, and call the function it
    /// wraps. A named function needs no environment and no indirection, so the
    /// common widening allocates nothing.
    pub(crate) fn emit_shim(&self, key: &str, sig: &Ty) -> Result<(), String> {
        let symbol = shim_symbol(key, sig);
        let (func, _) = *self
            .closure_fns
            .get(&symbol)
            .ok_or_else(|| no_value(&format!("the shim '{symbol}'")))?;
        let (params, _) = self.closure_sig_types(sig)?;
        let entry = func.append_block("entry");
        self.builder.position_at_end(entry);
        let callee = self
            .funcs
            .get(key)
            .cloned()
            .ok_or_else(|| refuse(&format!("the function value '{key}'"), "S3"))?;
        let args: Vec<Value<'a>> = (0..params.len()).map(|i| func.get_param(i + 1)).collect();
        if closure_is_unit(sig) {
            self.call(callee.func, callee.ty, &args, "");
            self.builder.build_ret_void();
        } else {
            let v = self.call(callee.func, callee.ty, &args, "call");
            self.builder.build_ret(v);
        }
        Ok(())
    }

    /// Define every thunk `declare_dyn_fns` declared (A-12 slice 4). Each one
    /// takes the erased data pointer as its first parameter and calls the
    /// concrete method: a borrowed receiver *is* the pointer, a `Copy` builtin
    /// receiver is loaded from it.
    pub(crate) fn emit_dyn_bodies(&self) -> Result<(), String> {
        let vtables = self.program.dyn_vtables.clone();
        for vtable in &vtables {
            for slot in &vtable.slots {
                self.emit_dyn_thunk(vtable, slot)?;
            }
            self.emit_dyn_drop_thunk(vtable)?;
        }
        self.emit_box_dyn_drops()?;
        Ok(())
    }

    /// The drop entry's thunk (A-12 slice 7): the concrete type's own drop
    /// glue over the erased pointer, which is what `Box<dyn Trait>`'s free runs
    /// through the vtable. A type that needs no drop gets an empty body.
    pub(crate) fn emit_dyn_drop_thunk(&self, vtable: &crate::tir::DynVtable) -> Result<(), String> {
        let symbol =
            crate::types::dyn_drop_thunk_symbol(&vtable.trait_name, &vtable.type_name);
        let (func, _) = *self
            .dyn_fns
            .get(&symbol)
            .ok_or_else(|| no_value(&format!("the dyn drop thunk '{symbol}'")))?;
        let entry = func.append_block("entry");
        self.builder.position_at_end(entry);
        let raw = func.get_param(0);
        if needs_drop(&vtable.concrete, &self.owned) {
            self.call_drop(raw, &vtable.concrete)?;
        }
        self.builder.build_ret_void();
        Ok(())
    }

    /// One `kelp_box_dyn_drop__<Trait>` body (A-12 slice 7): free what an owned
    /// trait object points at through its vtable. The drop entry runs the
    /// concrete type's drop glue, and the size and align entries are what the
    /// project's own allocator needs (libc's `free` does not).
    pub(crate) fn emit_box_dyn_drops(&self) -> Result<(), String> {
        let mut traits: Vec<String> = self
            .instantiations
            .box_dyns
            .iter()
            .filter_map(|ty| match ty {
                Ty::BoxDyn { trait_name } => Some(trait_name.clone()),
                _ => None,
            })
            .collect();
        traits.sort();
        traits.dedup();
        for trait_name in &traits {
            let symbol = crate::types::box_dyn_drop_fn(trait_name);
            let (func, _) = *self
                .container_fns
                .get(&symbol)
                .ok_or_else(|| no_value(&format!("the box drop '{symbol}'")))?;
            let table_ty = *self
                .dyn_table_tys
                .get(trait_name)
                .ok_or_else(|| no_value(&format!("the vtable layout of '{trait_name}'")))?;
            let pair_ty = *self
                .containers
                .get(&Ty::BoxDyn {
                    trait_name: trait_name.clone(),
                })
                .ok_or_else(|| no_value(&format!("the trait object 'Box<dyn {trait_name}>'")))?;
            let entry = func.append_block("entry");
            self.builder.position_at_end(entry);
            let self_p = func.get_param(0);
            let data_slot = self.builder.build_struct_gep(pair_ty, self_p, 0, "data");
            let data = self.ld(self.pty(), data_slot, "data");
            let null = self.ctx.const_null(self.pty());
            let empty = self.builder.build_icmp(IntCmp::Eq, data, null, "empty");
            let done = self.block(func, "ret");
            let live = self.block(func, "live");
            self.builder.build_cond_br(empty, done, live);
            self.builder.position_at_end(live);
            let table_slot = self.builder.build_struct_gep(pair_ty, self_p, 1, "vtable");
            let table = self.ld(self.pty(), table_slot, "vtable");
            let drop_slot = self.builder.build_struct_gep(table_ty, table, 1, "drop");
            let drop_fn = self.ld(self.pty(), drop_slot, "drop");
            let drop_ty = self.ctx.fn_type(self.ctx.void(), &[self.pty()], false);
            self.builder.build_indirect_call(drop_fn, drop_ty, &[data], "");
            let i64t = self.ctx.i64();
            let size_slot = self.builder.build_struct_gep(table_ty, table, 2, "size");
            let size = self.ld(i64t, size_slot, "size");
            let align_slot = self.builder.build_struct_gep(table_ty, table, 3, "align");
            let align = self.ld(i64t, align_slot, "align");
            if self.managed {
                // The collector reclaims the block; only the drop above ran.
            } else if self.box_pooled() {
                let (free, fty) = self.libc_fn("kelp_box_free")?;
                self.call(free, fty, &[data, size, align], "");
            } else {
                self.heap_dealloc_dyn(data, size, align)?;
            }
            self.st(self.ctx.const_null(self.pty()), data_slot);
            self.builder.build_br(done);
            self.builder.position_at_end(done);
            self.builder.build_ret_void();
        }
        Ok(())
    }

    pub(crate) fn emit_dyn_thunk(
        &self,
        vtable: &crate::tir::DynVtable,
        slot: &crate::tir::DynVtableSlot,
    ) -> Result<(), String> {
        let symbol = crate::types::dyn_thunk_symbol(
            &vtable.trait_name,
            &vtable.type_name,
            &slot.method,
        );
        let (func, _) = *self
            .dyn_fns
            .get(&symbol)
            .ok_or_else(|| no_value(&format!("the dyn thunk '{symbol}'")))?;
        let entry = func.append_block("entry");
        self.builder.position_at_end(entry);
        let raw = func.get_param(0);
        let receiver = match slot.params.first() {
            Some(Ty::Ref { .. }) | None => raw,
            Some(t) => {
                let lty = self.lty2(t)?;
                self.ld(lty, raw, "recv")
            }
        };
        let mut args: Vec<Value<'a>> = vec![receiver];
        let mut next = 1u32;
        for p in &slot.params[1..] {
            if *p == Ty::Unit {
                continue;
            }
            args.push(func.get_param(next as usize));
            next += 1;
        }
        let target = self
            .funcs
            .get(&slot.target)
            .cloned()
            .ok_or_else(|| refuse(&format!("the trait method '{}'", slot.target), "S2A"))?;
        if slot.ret == Ty::Unit {
            self.call(target.func, target.ty, &args, "");
            self.builder.build_ret_void();
        } else {
            let v = self.call(target.func, target.ty, &args, "call");
            self.builder.build_ret(v);
        }
        Ok(())
    }
}

/// The mangled signature fragment of a closure type, for the generated S2E
/// symbols. The value is always a `Ty::Closure`; a `Ty::Fn` has no environment
/// and so no machinery of its own.
pub(crate) fn closure_mangle_of(ty: &Ty) -> String {
    match ty {
        Ty::Closure { params, ret } | Ty::ScopedFn { params, ret } => closure_mangle(params, ret),
        other => unreachable!("not a closure signature: {}", other.display()),
    }
}

/// The generated names of the widening's two halves and of the shim, mirroring
/// the earlier emitter's `coerce_thunk_name`/`coerce_drop_name`/`coerce_shim_name`.
pub(crate) fn coerce_thunk_symbol(ty: &Ty) -> String {
    format!("kelp_coerce__{}", closure_mangle_of(ty))
}

pub(crate) fn coerce_drop_symbol(ty: &Ty) -> String {
    format!("kelp_coerce_drop__{}", closure_mangle_of(ty))
}

pub(crate) fn shim_symbol(key: &str, ty: &Ty) -> String {
    format!("kelp_shim__{}__{}", crate::types::c_ident(key), closure_mangle_of(ty))
}

/// The symbol of one spawn signature's worker body (MK9-S3-M1 stage C1), the
/// same name `thread_body_name` gives the body: the runtime's
/// trampoline is the one caller of either.
pub(crate) fn thread_body_symbol(sig: &Ty) -> String {
    format!("kelp_thread_body__{}", closure_mangle_of(sig))
}

/// The signature a closure callee is called with, when the callee is a closure
/// value or a borrow of one. `None` for a plain function value, whose callee is
/// the pointer itself.
pub(crate) fn closure_callee_sig(ty: &Ty) -> Option<Ty> {
    match ty {
        Ty::Closure { .. } | Ty::ScopedFn { .. } => Some(ty.clone()),
        Ty::Ref { inner, .. }
            if matches!(**inner, Ty::Closure { .. } | Ty::ScopedFn { .. }) =>
        {
            Some((**inner).clone())
        }
        _ => None,
    }
}

/// Whether one closure signature returns `unit`, which decides between `ret`
/// and `ret void` in the three functions that are declared with it.
pub(crate) fn closure_is_unit(sig: &Ty) -> bool {
    matches!(sig, Ty::Closure { ret, .. } | Ty::ScopedFn { ret, .. } if **ret == Ty::Unit)
}

/// How deeply containers nest, used to emit a container after the containers it
/// contains. The mirror of the earlier emitter's `container_depth`.
pub(crate) fn container_depth(ty: &Ty) -> usize {
    match ty {
        Ty::List(elem) | Ty::Box(elem) => 1 + container_depth(elem),
        Ty::Map(..) => 1,
        Ty::Array(elem, _) => 1 + container_depth(elem),
        _ => 0,
    }
}

/// A stable sort key for the emission order: the C name of one element type.
pub(crate) fn container_sort_name(ty: &Ty) -> String {
    ty_mangle(ty)
}

/// One monomorphized container function: its mangled symbol, its LLVM return
/// type and its LLVM parameter types. What `declare_container_fns` declares and
/// `emit_container_bodies` defines, from one list, so the two cannot drift.
pub(crate) struct CfnSpec<'a> {
    pub(crate) symbol: String,
    pub(crate) ret: Type<'a>,
    pub(crate) params: Vec<Type<'a>>,
}

impl<'a> Gen<'a> {
    pub(crate) fn lty2(&self, ty: &Ty) -> Result<Type<'a>, String> {
        llvm_type(self.ctx, &self.structs, &self.containers, ty)
    }

    /// The LLVM type an `Atomic<T>`'s single field holds, which is the type its
    /// atomic instructions are emitted at.
    ///
    /// The element type for every element but `bool`, and `i8` for `bool`: an
    /// LLVM atomic access has to be byte-sized, and an `i1` (which is what a
    /// `bool` value is) is not, so `Atomic<bool>` stores a byte and the two
    /// crossings (`FnGen::atomic_to_mem` and `FnGen::atomic_from_mem`) are where
    /// a value becomes one and back. The C side has no such seam: a C `bool`
    /// *is* one byte.
    pub(crate) fn atomic_mem_lty(&self, elem: &Ty) -> Result<Type<'a>, String> {
        if matches!(elem, Ty::Bool) {
            return Ok(self.ctx.i8());
        }
        self.lty2(elem)
    }

    /// The named LLVM type of one container instantiation.
    pub(crate) fn cty(&self, ty: &Ty) -> Result<Type<'a>, String> {
        self.containers
            .get(ty)
            .copied()
            .ok_or_else(|| no_value(&format!("the container '{}'", ty.display())))
    }

    /// The LLVM global, `tir` type and per-CPU layout (if any) of one `static`
    /// (kernel stage 7). The caller resolves the address of the current CPU's
    /// copy through `FnGen::static_slot`.
    pub(crate) fn static_place(&self, name: &str) -> Option<(Value<'a>, Ty, Option<PercpuStatic<'a>>)> {
        self.static_globals
            .get(name)
            .map(|sg| (sg.global, sg.ty.clone(), sg.percpu))
    }

    /// Position the builder at a fresh block of the named container function.
    pub(crate) fn start(
        &self,
        symbol: &str,
        name: &str,
    ) -> Result<(LlvmFunction<'a>, BasicBlock<'a>), String> {
        let (func, _) = self.cfn(symbol)?;
        let bb = func.append_block(name);
        self.builder.position_at_end(bb);
        Ok((func, bb))
    }

    /// One runtime helper by symbol, with its declared type.
    pub(crate) fn rt(&self, symbol: &str) -> Result<RtFn<'a>, String> {
        self.runtime
            .get(symbol)
            .copied()
            .ok_or_else(|| no_value(&format!("the runtime '{symbol}'")))
    }

    /// Call an sret runtime helper: the destination pointer first, then the
    /// real arguments (`RtFn::sret`).
    pub(crate) fn sret_call(&self, symbol: &str, dst: Value<'a>, args: &[Value<'a>]) -> Result<(), String> {
        let rt = self.rt(symbol)?;
        let mut all = Vec::with_capacity(args.len() + 1);
        all.push(dst);
        all.extend_from_slice(args);
        self.call(rt.func, rt.ty, &all, "");
        Ok(())
    }

    /// Deep-copy one element value (a `String`/`Bytes` clone, a `datac`'s
    /// generated clone, or a plain copy) into caller storage that owns nothing
    /// yet. The mirror of the earlier emitter's `write_field_clone` for a container slot.
    pub(crate) fn clone_into(&self, src: Value<'a>, dst: Value<'a>, elem: &Ty) -> Result<(), String> {
        match elem {
            Ty::String => self.sret_call("kelp_String__clone", dst, &[src]),
            Ty::Bytes => self.sret_call("kelp_Bytes__clone", dst, &[src]),
            Ty::Struct(name) => {
                let (func, ty) = self
                    .clone_fns
                    .get(name)
                    .copied()
                    .ok_or_else(|| no_value(&format!("the clone of '{name}'")))?;
                self.call(func, ty, &[src, dst], "");
                Ok(())
            }
            _ => {
                let elem_ty = self.lty2(elem)?;
                let v = self.ld(elem_ty, src, "copy");
                self.st(v, dst);
                Ok(())
            }
        }
    }

    /// The value of a copied element (a scalar load), for a `Map` value. A
    /// `String`/`Bytes` or an owned `datac`/`enum` is deep-copied into `slot`
    /// first, by the same generated clone a `List` element takes. A
    /// non-clonable element type is never reached here, because
    /// `map_specs`/`emit_map_bodies` gate `get_or`/`values` on the
    /// declaration-graph predicate.
    pub(crate) fn clone_value(&self, src: Value<'a>, elem: &Ty, slot: Value<'a>) -> Result<Value<'a>, String> {
        match elem {
            Ty::String | Ty::Bytes | Ty::Struct(_) => {
                self.clone_into(src, slot, elem)?;
                Ok(self.ld(self.lty2(elem)?, slot, "clone"))
            }
            _ => Ok(self.ld(self.lty2(elem)?, src, "copy")),
        }
    }

    /// Widen a scalar map key to the `uint64_t` `kelp_map_mix` takes, by the
    /// key's own signedness, exactly as the C `(uint64_t)(k)` cast does.
    pub(crate) fn key_to_u64(&self, key: &Ty, v: Value<'a>) -> Result<Value<'a>, String> {
        let w = int_width(key).ok_or_else(|| no_value("a scalar map key"))?;
        if w == 64 {
            return Ok(v);
        }
        Ok(self
            .builder
            .build_int_cast(is_signed(key), true, v, self.ctx.i64(), "kw"))
    }

    /// The generated clone/cmp/hash/eq function handles, then the container
    /// sets.
    pub(crate) fn declare_clone_cmp_fns(&mut self) {
        let ptr = self.pty();
        let void = self.ctx.void();
        let i1t = self.ctx.i1();
        let i32t = self.ctx.i32();
        let i64t = self.ctx.i64();
        let mut clonable: Vec<String> = self.clonable.iter().cloned().collect();
        clonable.sort();
        for name in clonable {
            let ty = self.ctx.fn_type(void, &[ptr, ptr], false);
            let func = self
                .module
                .add_function(&clone_fn_name(&Ty::Struct(name.clone())), ty);
            self.clone_fns.insert(name, (func, ty));
        }
        let mut sortable: Vec<String> = self.sortable.iter().cloned().collect();
        sortable.sort();
        for name in sortable {
            let ty = self.ctx.fn_type(i32t, &[ptr, ptr], false);
            let func = self
                .module
                .add_function(&cmp_fn_name(&Ty::Struct(name.clone())), ty);
            self.cmp_fns.insert(name, (func, ty));
        }
        let mut keyable: Vec<String> = self.keyable.iter().cloned().collect();
        keyable.sort();
        for name in keyable {
            let hty = self.ctx.fn_type(i64t, &[ptr], false);
            let hfunc = self
                .module
                .add_function(&hash_fn_name(&Ty::Struct(name.clone())), hty);
            self.hash_fns.insert(name.clone(), (hfunc, hty));
            let ety = self.ctx.fn_type(i1t, &[ptr, ptr], false);
            let efunc = self
                .module
                .add_function(&eq_fn_name(&Ty::Struct(name.clone())), ety);
            self.eq_fns.insert(name, (efunc, ety));
        }
    }
}
