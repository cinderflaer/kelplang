use super::*;

impl<'a> Gen<'a> {
    /// Create one named LLVM struct per container instantiation the program
    /// uses, with no body yet. The bodies are set later (`set_container_bodies`
    /// for the pointer containers, and alongside the aggregates for the inline
    /// arrays), because an array's body names the element type it holds.
    pub(crate) fn declare_container_types(&mut self) -> Result<(), String> {
        let inst = self.instantiations.clone();
        let mut lists: Vec<Ty> = inst.lists.iter().cloned().collect();
        lists.sort_by_key(|t| (container_depth(t), container_sort_name(t)));
        let mut boxes: Vec<Ty> = inst.boxes.iter().cloned().collect();
        boxes.sort_by_key(|t| (container_depth(t), container_sort_name(t)));
        let mut maps: Vec<(Ty, Ty)> = inst.maps.iter().cloned().collect();
        maps.sort_by_key(|(k, v)| format!("{}{}", container_sort_name(k), container_sort_name(v)));
        let mut arrays: Vec<Ty> = inst.arrays.iter().cloned().collect();
        arrays.sort_by_key(|t| (container_depth(t), container_sort_name(t)));

        // The closure signature types first (S2E): a container element may be
        // one (`Box<closure(...)>`), a parameter certainly may, and a later
        // closure signature may name an earlier one. The earlier emitter emits these
        // before every container and `datac` for the same reason.
        let mut closures: Vec<Ty> = inst.closures.iter().cloned().collect();
        closures.sort_by_key(|t| (container_depth(t), container_sort_name(t)));
        for sig in &closures {
            let (params, ret) = match sig {
                Ty::Closure { params, ret } | Ty::ScopedFn { params, ret } => (params, ret),
                _ => continue,
            };
            let ty = self.ctx.struct_type(&closure_c_type(params, ret));
            self.containers.insert(sig.clone(), ty);
        }

        // The borrowed trait objects first (A-12 slice 4): a fat pair of
        // pointers, one identified struct per trait. Both the shared and the
        // exclusive spelling resolve to the same struct, exactly as `c_type`
        // gives them the same C name.
        let pair = self.ctx.opaque_ptr();
        let mut dyn_traits: Vec<String> = self
            .program
            .dyn_traits
            .iter()
            .cloned()
            .chain(self.program.dyn_vtables.iter().map(|v| v.trait_name.clone()))
            .collect();
        dyn_traits.sort();
        dyn_traits.dedup();
        for trait_name in &dyn_traits {
            let ty = self.ctx.struct_type(&crate::types::dyn_c_type(trait_name));
            self.ctx.struct_set_body(ty, &[pair, pair]);
            for mutable in [false, true] {
                self.containers.insert(
                    Ty::Dyn {
                        mutable,
                        trait_name: trait_name.to_string(),
                    },
                    ty,
                );
            }
            // An owned trait object is the same fat pair (A-12 slice 7).
            self.containers.insert(
                Ty::BoxDyn {
                    trait_name: trait_name.clone(),
                },
                ty,
            );
        }
        for elem in &lists {
            let ty = self.ctx.struct_type(&list_c_type(elem));
            self.containers.insert(Ty::List(Box::new(elem.clone())), ty);
        }
        for elem in &boxes {
            let ty = self.ctx.struct_type(&box_c_type(elem));
            self.containers.insert(Ty::Box(Box::new(elem.clone())), ty);
        }
        for (key, value) in &maps {
            let name = map_c_type(key, value);
            let entry = self.ctx.struct_type(&format!("{name}_entry"));
            let ty = self.ctx.struct_type(&name);
            self.map_entries
                .insert(Ty::Map(Box::new(key.clone()), Box::new(value.clone())), entry);
            self.containers
                .insert(Ty::Map(Box::new(key.clone()), Box::new(value.clone())), ty);
        }
        for arr in &arrays {
            if let Ty::Array(elem, len) = arr {
                let ty = self.ctx.struct_type(&crate::types::array_c_type(elem, *len));
                self.containers.insert(arr.clone(), ty);
            }
        }
        // An `Atomic<T>` is one field of a scalar type, so its body can be set
        // here, before any `datac` exists, exactly as a pointer container's can.
        let mut atomics: Vec<Ty> = inst.atomics.iter().cloned().collect();
        atomics.sort_by_key(container_sort_name);
        for atomic in &atomics {
            let Ty::Atomic(elem) = atomic else {
                continue;
            };
            let ty = self.ctx.struct_type(&crate::types::atomic_c_type(elem));
            let field = self.atomic_mem_lty(elem)?;
            self.ctx.struct_set_body(ty, &[field]);
            self.containers.insert(atomic.clone(), ty);
        }
        // The pointer containers have no by-value dependency on a user type, so
        // their bodies are settable now, before any `datac` type exists.
        self.set_container_bodies()
    }

    /// Set the bodies of every pointer container (`List`, `Box`, `Map`). Safe to
    /// do before any `datac` body exists: a container holds its contents through
    /// a pointer, so none of these fields names a user type by value.
    pub(crate) fn set_container_bodies(&self) -> Result<(), String> {
        let ptr = self.pty();
        let i64t = self.ctx.i64();
        let i8t = self.ctx.i8();
        for (ty, lty) in &self.containers {
            match ty {
                Ty::List(_) => self.ctx.struct_set_body(*lty, &[ptr, i64t, i64t]),
                Ty::Box(_) => self.ctx.struct_set_body(*lty, &[ptr]),
                // The closure's fat pointer, field for field
                // `write_closure_typedef`'s three members (S2E): the code
                // pointer, the environment and the environment's destructor.
                // A scoped function value is the same three members.
                Ty::Closure { .. } | Ty::ScopedFn { .. } => {
                    self.ctx.struct_set_body(*lty, &[ptr, ptr, ptr])
                }
                Ty::Map(_, _) => self.ctx.struct_set_body(
                    *lty,
                    &[ptr, i64t, i64t, i64t, ptr, i64t, i64t],
                ),
                _ => {}
            }
        }
        for (map, entry_ty) in &self.map_entries {
            if let Ty::Map(key, value) = map {
                // A `datac` key or value is held by value, and no `datac` type
                // exists yet, so that entry's body waits for
                // `set_late_map_entry_bodies`.
                if matches!(**key, Ty::Struct(_)) || matches!(**value, Ty::Struct(_)) {
                    continue;
                }
                let key_ty = llvm_type(self.ctx, &self.structs, &self.containers, key)?;
                let value_ty = llvm_type(self.ctx, &self.structs, &self.containers, value)?;
                self.ctx.struct_set_body(*entry_ty, &[key_ty, value_ty, i8t]);
            }
        }
        Ok(())
    }

    /// Set the body of every map entry that holds a `datac` by value, as a key
    /// or a value. Runs after `declare_aggregates`, because the laid-out type
    /// is what names it; the container-body pass above runs before any `datac`
    /// type exists.
    pub(crate) fn set_late_map_entry_bodies(&self) -> Result<(), String> {
        let i8t = self.ctx.i8();
        for (map, entry_ty) in &self.map_entries {
            let Ty::Map(key, value) = map else {
                continue;
            };
            if !matches!(**key, Ty::Struct(_)) && !matches!(**value, Ty::Struct(_)) {
                continue;
            }
            let key_ty = self.lty2(key)?;
            let value_ty = self.lty2(value)?;
            self.ctx.struct_set_body(*entry_ty, &[key_ty, value_ty, i8t]);
        }
        Ok(())
    }

    /// Set one array instantiation's body, after any array nested inside it
    /// (`emit_arrays_for`, the mirror of the C helper of the same name).
    pub(crate) fn emit_array_body(&self, ty: &Ty, emitted: &mut HashSet<Ty>) -> Result<(), String> {
        let Ty::Array(elem, len) = ty else {
            return Ok(());
        };
        self.emit_array_body(elem, emitted)?;
        if !emitted.insert(ty.clone()) {
            return Ok(());
        }
        let elem_ty = llvm_type(self.ctx, &self.structs, &self.containers, elem)?;
        let arr = self.ctx.array_type(elem_ty, *len);
        if let Some(lty) = self.containers.get(ty) {
            self.ctx.struct_set_body(*lty, &[arr]);
        }
        Ok(())
    }

    /// Whether a value of `ty` owes a drop call in this build. Every build but
    /// a managed one answers yes, which keeps its drops as they were. A managed
    /// build answers yes only for a resource (`gc_descr::managed_needs_drop`).
    pub(crate) fn managed_drop(&self, ty: &Ty) -> bool {
        !self.managed || crate::gc_descr::managed_needs_drop(ty, &self.resources)
    }

    /// One zeroed GC object of `elem` (the managed form of `malloc_for`): the
    /// call names the element's descriptor, so the collector knows its pointer
    /// slots. Only meaningful in a managed build.
    pub(crate) fn alloc_elem(&self, elem: &Ty, elem_ty: Type<'a>) -> Result<Value<'a>, String> {
        if !self.managed {
            return self.malloc_for(elem_ty);
        }
        let gc = self.gc.as_ref().ok_or_else(|| no_value("the managed GC runtime"))?;
        let descr = gc.descriptor(self.ctx, self.module, self, elem)?;
        let (func, fty) = gc.alloc;
        Ok(self.call(func, fty, &[descr], "gc"))
    }

    /// The GC descriptor of one element of a container buffer (`gc_descr`):
    /// a `List` buffer is an array of its element's descriptor, so the
    /// collector reads the element's pointer slots at each stride. Managed only.
    pub(crate) fn elem_descr(&self, elem: &Ty) -> Result<Value<'a>, String> {
        let gc = self.gc.as_ref().ok_or_else(|| no_value("the managed GC runtime"))?;
        gc.descriptor(self.ctx, self.module, self, elem)
    }

    /// `kelp_gc_alloc_array(descr, count)`: a zeroed GC array of `count`
    /// elements of the descriptor's type. Managed only.
    pub(crate) fn gc_array(&self, descr: Value<'a>, count: Value<'a>) -> Result<Value<'a>, String> {
        let gc = self.gc.as_ref().ok_or_else(|| no_value("the managed GC runtime"))?;
        let (func, fty) = gc.alloc_array;
        Ok(self.call(func, fty, &[descr, count], "gc.arr"))
    }

    /// Managed growth of a container buffer: a fresh GC array of `count`
    /// elements with the first `old_bytes` of the old buffer copied over. The
    /// old block is not freed; the collector reclaims it once unreachable.
    pub(crate) fn gc_grow(
        &self,
        old: Value<'a>,
        old_bytes: Value<'a>,
        descr: Value<'a>,
        count: Value<'a>,
        func: LlvmFunction<'a>,
    ) -> Result<Value<'a>, String> {
        let buf = self.gc_array(descr, count)?;
        self.check_alloc(buf, func)?;
        let (memcpy, mty) = self.mem_fn("memcpy")?;
        self.call(memcpy, mty, &[buf, old, old_bytes], "");
        Ok(buf)
    }

    /// The root descriptor of a runtime record that holds a value of `ty` (a
    /// spawn's argument or result, a Mutex or RwLock value): the value's own
    /// descriptor when it holds collected memory, and a null pointer otherwise.
    /// A hosted build has no collector, so it is always null. Section 3.
    pub(crate) fn record_descr(&self, ty: &Ty) -> Result<Value<'a>, String> {
        if !self.managed || !crate::gc_descr::record_needs_root(self, ty)? {
            return Ok(self.ctx.const_null(self.pty()));
        }
        self.elem_descr(ty)
    }

    /// The descriptor of a `Map`'s entry record `{ key, value, live }`, which
    /// is the element type of its entries buffer (`gc_descr::map_entry_layout`).
    /// `name` is the map's C name, so each map gets one global. Managed only.
    pub(crate) fn map_entry_descr(&self, key: &Ty, value: &Ty, name: &str) -> Result<Value<'a>, String> {
        let gc = self.gc.as_ref().ok_or_else(|| no_value("the managed GC runtime"))?;
        let (size, align, offsets) = crate::gc_descr::map_entry_layout(self, key, value)?;
        Ok(gc.descriptor_raw(
            self.ctx,
            self.module,
            &format!("map_entry.{name}"),
            size,
            align,
            &offsets,
        ))
    }

    /// The environment descriptor of a capturing literal, one per literal
    /// (`gc_descr::env_layout` over its captures). Managed only.
    pub(crate) fn closure_env_descr(&self, fn_name: &str) -> Result<Value<'a>, String> {
        let gc = self.gc.as_ref().ok_or_else(|| no_value("the managed GC runtime"))?;
        let env = self
            .program
            .functions
            .iter()
            .find(|f| f.name == fn_name)
            .and_then(|f| f.env.as_ref())
            .ok_or_else(|| no_value(&format!("the environment of '{fn_name}'")))?;
        let captures: Vec<Ty> = env.fields.iter().map(|(_, ty)| ty.clone()).collect();
        let (size, align, offsets) = crate::gc_descr::env_layout(self, &captures)?;
        Ok(gc.descriptor_raw(
            self.ctx,
            self.module,
            &format!("env.{fn_name}"),
            size,
            align,
            &offsets,
        ))
    }

    /// The one-field environment of a widened function value. It holds a code
    /// pointer, which is not a GC pointer, so its descriptor is noscan.
    pub(crate) fn coerce_env_descr(&self) -> Result<Value<'a>, String> {
        let gc = self.gc.as_ref().ok_or_else(|| no_value("the managed GC runtime"))?;
        let size = self.layout.abi_size_of(self.coerce_env_ty);
        let align = self.layout.abi_alignment_of(self.coerce_env_ty) as u64;
        Ok(gc.descriptor_raw(self.ctx, self.module, "coerce_env", size, align, &[]))
    }

    /// `malloc(sizeof(<ty>))` lowered to a constant byte count, through the
    /// project's own allocator when the manifest configured one.
    pub(crate) fn malloc_for(&self, ty: Type<'a>) -> Result<Value<'a>, String> {
        let size = self.layout.abi_size_of(ty);
        let align = self.layout.abi_alignment_of(ty);
        self.heap_alloc(self.v_i64(size), align as u64)
    }

    /// Whether a `Box<T>` block goes through the runtime's small-block free
    /// lists: the default libc allocator only, since a project's own allocator
    /// and a freestanding program keep their existing path.
    pub(crate) fn box_pooled(&self) -> bool {
        !self.managed && self.alloc_fns.is_none() && self.freestanding.is_none()
    }

    /// The calling thread's Box pool (`kelp_box_pool` in `kelp-rt`): one call
    /// per pooled Box operation, which returns the per-class list heads at its
    /// base. The pool is created on the thread's first use, so the inline pop
    /// and push below never see a missing pool (speed-alloc).
    pub(crate) fn box_pool(&self) -> Result<Value<'a>, String> {
        let (func, fty) = self.libc_fn("kelp_box_pool")?;
        Ok(self.call(func, fty, &[], "box.pool"))
    }

    /// The `Box<T>` allocation of `ty` in function `f`. Pooled, a block of a
    /// size class with a free block is popped from the thread's list inline;
    /// otherwise, or when the list is empty, `kelp_box_alloc` runs. Unpooled, it
    /// is `malloc_for`. The result is nullable, as `malloc` is.
    pub(crate) fn box_alloc_in(&self, f: LlvmFunction<'a>, ty: Type<'a>) -> Result<Value<'a>, String> {
        if !self.box_pooled() {
            return self.malloc_for(ty);
        }
        let size = self.layout.abi_size_of(ty);
        let align = self.layout.abi_alignment_of(ty) as u64;
        let Some(class) = box_pool_class(size, align) else {
            return self.box_alloc_call(size, align);
        };
        let pool = self.box_pool()?;
        let slot = self.builder.build_gep(self.pty(), pool, self.v_i64(class), "box.slot");
        let head = self.ld(self.pty(), slot, "box.head");
        let null = self.ctx.const_null(self.pty());
        let empty = self.builder.build_icmp(IntCmp::Eq, head, null, "box.empty");
        let pop = self.block(f, "box.pop");
        let slow = self.block(f, "box.new");
        let join = self.block(f, "box.join");
        self.builder.build_cond_br(empty, slow, pop);
        self.builder.position_at_end(pop);
        let next = self.ld(self.pty(), head, "box.next");
        self.st(next, slot);
        self.builder.build_br(join);
        self.builder.position_at_end(slow);
        let fresh = self.box_alloc_call(size, align)?;
        self.builder.build_br(join);
        self.builder.position_at_end(join);
        let out = self.builder.build_phi(self.pty(), "box");
        self.builder.add_incoming(out, &[(head, pop), (fresh, slow)]);
        Ok(out)
    }

    /// `kelp_box_alloc(size, align)`: the runtime's slow path, which refills
    /// a class from malloc or hands out a block for a size it does not pool.
    pub(crate) fn box_alloc_call(&self, size: u64, align: u64) -> Result<Value<'a>, String> {
        let (func, fty) = self.libc_fn("kelp_box_alloc")?;
        Ok(self.call(func, fty, &[self.v_i64(size), self.v_i64(align)], "box.new"))
    }

    /// The `Box<T>` release of a `size`-byte, `align`-aligned value in function
    /// `f`. Pooled, the block is pushed onto the thread's class list inline;
    /// otherwise `kelp_box_free` runs. Unpooled, it is `heap_dealloc`.
    pub(crate) fn box_free_in(&self, _f: LlvmFunction<'a>, ptr: Value<'a>, size: u64, align: u64) -> Result<(), String> {
        // A managed build frees nothing: the collector reclaims the block.
        if self.managed {
            return Ok(());
        }
        if !self.box_pooled() {
            return self.heap_dealloc(ptr, self.v_i64(size), align);
        }
        let Some(class) = box_pool_class(size, align) else {
            return self.box_free_call(ptr, size, align);
        };
        let pool = self.box_pool()?;
        let slot = self.builder.build_gep(self.pty(), pool, self.v_i64(class), "box.slot");
        let head = self.ld(self.pty(), slot, "box.head");
        self.st(head, ptr);
        self.st(ptr, slot);
        Ok(())
    }

    /// `kelp_box_free(ptr, size, align)`: the runtime's free for a block the
    /// inline path does not take, a size the pool does not keep.
    pub(crate) fn box_free_call(&self, ptr: Value<'a>, size: u64, align: u64) -> Result<(), String> {
        let (func, fty) = self.libc_fn("kelp_box_free")?;
        self.call(func, fty, &[ptr, self.v_i64(size), self.v_i64(align)], "");
        Ok(())
    }

    /// One byte-copy or zero-fill primitive: this module's own definition in a
    /// freestanding build, libc's otherwise (kernel stage 7).
    pub(crate) fn mem_fn(&self, name: &str) -> Result<(LlvmFunction<'a>, FnType<'a>), String> {
        if let Some(pair) = self.mem.get(name) {
            return Ok(*pair);
        }
        self.libc_fn(name)
    }

    /// `alloc(bytes, align)` lowered to the project's own allocator when the
    /// manifest configured one, or to libc's `malloc` otherwise (kernel stage 7,
    /// MK9-S5-M1). The result is nullable; the caller runs `check_alloc`, which
    /// routes a null to the project's panic handler with "out of memory".
    pub(crate) fn heap_alloc(&self, bytes: Value<'a>, align: u64) -> Result<Value<'a>, String> {
        if self.managed {
            // A hosted build allocates only typed objects (`alloc_elem`). An
            // untyped byte allocation has no descriptor, and a noscan one would
            // hide the pointers a `List` or `Map` buffer holds from the collector.
            return Err("a hosted build: this container allocation has no GC type descriptor yet (WP3 follow-up)".to_string());
        }
        match &self.alloc_fns {
            Some(a) => {
                let align = self.v_i64(align);
                Ok(self.call(a.alloc.0, a.alloc.1, &[bytes, align], "heap"))
            }
            None => {
                let (func, fty) = self.libc_fn("malloc")?;
                Ok(self.call(func, fty, &[bytes], "heap"))
            }
        }
    }

    /// `dealloc(ptr, bytes, align)` lowered to the project's own allocator, or
    /// to libc's `free` otherwise. The byte count and alignment are what a
    /// freestanding allocator needs and libc's `free` ignores; they come from
    /// the type whose storage is being released, so no allocation records them.
    pub(crate) fn heap_dealloc(
        &self,
        ptr: Value<'a>,
        bytes: Value<'a>,
        align: u64,
    ) -> Result<(), String> {
        if self.managed {
            // The collector reclaims the object; a managed build emits no free.
            return Ok(());
        }
        match &self.alloc_fns {
            Some(a) => {
                let align = self.v_i64(align);
                self.call(a.dealloc.0, a.dealloc.1, &[ptr, bytes, align], "");
                Ok(())
            }
            None => {
                let (func, fty) = self.libc_fn("free")?;
                self.call(func, fty, &[ptr], "");
                Ok(())
            }
        }
    }

    /// As `heap_dealloc`, for a size and align that are only known at runtime:
    /// an owned trait object reads them out of its vtable (A-12 slice 7). A
    /// project with its own allocator needs both, and libc's `free` needs
    /// neither.
    pub(crate) fn heap_dealloc_dyn(
        &self,
        ptr: Value<'a>,
        bytes: Value<'a>,
        align: Value<'a>,
    ) -> Result<(), String> {
        if self.managed {
            return Ok(());
        }
        match &self.alloc_fns {
            Some(a) => {
                self.call(a.dealloc.0, a.dealloc.1, &[ptr, bytes, align], "");
                Ok(())
            }
            None => {
                let (func, fty) = self.libc_fn("free")?;
                self.call(func, fty, &[ptr], "");
                Ok(())
            }
        }
    }

    /// `realloc(ptr, new)` lowered through the project's allocator, or libc's
    /// `realloc` otherwise. A project that names an allocator but no `realloc`
    /// gets the literal fallback its manifest documents: allocate, copy the old
    /// bytes, free the old block. The returned pointer is checked non-null here,
    /// because a `realloc` call site has no natural place to branch.
    pub(crate) fn heap_realloc(
        &self,
        old: Value<'a>,
        old_bytes: Value<'a>,
        new_bytes: Value<'a>,
        align: u64,
        func: LlvmFunction<'a>,
    ) -> Result<Value<'a>, String> {
        if self.managed {
            return Err("a hosted build: this container growth has no GC type descriptor yet (WP3 follow-up)".to_string());
        }
        match &self.alloc_fns {
            Some(a) => {
                if let Some((realloc, rty)) = a.realloc {
                    let align = self.v_i64(align);
                    let buf = self.call(
                        realloc,
                        rty,
                        &[old, old_bytes, new_bytes, align],
                        "buf",
                    );
                    self.check_alloc(buf, func)?;
                    return Ok(buf);
                }
                let new = self.heap_alloc(new_bytes, align)?;
                self.check_alloc(new, func)?;
                let (memcpy, mty) = self.mem_fn("memcpy")?;
                self.call(memcpy, mty, &[new, old, old_bytes], "");
                self.heap_dealloc(old, old_bytes, align)?;
                Ok(new)
            }
            None => {
                let (realloc, rty) = self.libc_fn("realloc")?;
                let buf = self.call(realloc, rty, &[old, new_bytes], "buf");
                self.check_alloc(buf, func)?;
                Ok(buf)
            }
        }
    }

    /// `kelp_panic("out of memory")` when `ptr` is NULL, the same check every
    /// generated C allocation makes. The block control continues in is returned,
    /// because a caller that tracks its own current block (the function-body
    /// walk) has to be told where the branch landed; the container bodies, which
    /// position the builder by hand, ignore it.
    pub(crate) fn check_alloc(&self, ptr: Value<'a>, func: LlvmFunction<'a>) -> Result<BasicBlock<'a>, String> {
        let null = self.ctx.const_null(self.pty());
        let isnull = self.builder.build_icmp(IntCmp::Eq, ptr, null, "oom");
        let oom = self.block(func, "oom");
        let ok = self.block(func, "alloc.ok");
        self.builder.build_cond_br(isnull, oom, ok);
        self.builder.position_at_end(oom);
        self.emit_panic("out of memory")?;
        self.builder.position_at_end(ok);
        Ok(ok)
    }
}
