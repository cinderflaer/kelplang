use super::*;

/// Guard for `LLVMBuilderRef`: where the next instruction is emitted.
pub struct Builder<'ctx> {
    pub(crate) raw: LLVMBuilderRef,
    pub(crate) context: PhantomData<&'ctx Context>,
}

impl<'ctx> Builder<'ctx> {
    /// A builder in `ctx` (`LLVMCreateBuilderInContext`), disposed on drop.
    ///
    /// A new builder has no insertion point, so emitting before
    /// [`position_at_end`] has run is an error; the C API has no return code for
    /// that and reads as LLVM's own assert.
    ///
    /// [`position_at_end`]: Builder::position_at_end
    pub fn new(ctx: &'ctx Context) -> Self {
        Builder {
            // SAFETY: the context is live for 'ctx; the builder is owned and
            // disposed on drop.
            raw: unsafe { LLVMCreateBuilderInContext(ctx.as_raw()) },
            context: PhantomData,
        }
    }

    /// The underlying `LLVMBuilderRef`, for a C API call this wrapper lacks.
    pub fn as_raw(&self) -> LLVMBuilderRef {
        self.raw
    }

    /// Emit subsequent instructions at the end of `block`
    /// (`LLVMPositionBuilderAtEnd`).
    pub fn position_at_end(&self, block: BasicBlock<'ctx>) {
        // SAFETY: the builder and the block are live and belong to the same
        // context, which LLVM requires.
        unsafe { LLVMPositionBuilderAtEnd(self.raw, block.as_raw()) };
    }

    /// Emit subsequent instructions at the *start* of `block`, before its
    /// first instruction (`LLVMGetFirstInstruction` + `LLVMPositionBuilderBefore`).
    ///
    /// This is the one insertion point that is valid in a block that already
    /// ends in a terminator, which is exactly the case a local's alloca slot
    /// needs: the slot belongs in the entry block (`mem2reg` promotes only
    /// those), and by the time a `let` in a later statement allocates one the
    /// entry block may already have been terminated by a `return`.
    pub fn position_at_start(&self, block: BasicBlock<'ctx>) {
        // SAFETY: the block is live; the returned instruction belongs to it.
        let first = unsafe { LLVMGetFirstInstruction(block.as_raw()) };
        if first.is_null() {
            // SAFETY: an empty block takes an insertion point at its end.
            unsafe { LLVMPositionBuilderAtEnd(self.raw, block.as_raw()) };
        } else {
            // SAFETY: the builder is live and `first` is an instruction of the
            // block, which is the contract `LLVMPositionBuilderBefore` wants.
            unsafe { LLVMPositionBuilderBefore(self.raw, first) };
        }
    }

    /// Emit subsequent instructions immediately after `instr`, which must be
    /// an instruction of `block` (`LLVMGetNextInstruction` +
    /// `LLVMPositionBuilderBefore`, or `LLVMPositionBuilderAtEnd` when `instr`
    /// is currently the block's last instruction).
    ///
    /// This is what chains a run of entry-block `alloca`s in declaration
    /// order: inserting each one right after the previous one, rather than
    /// always at the block's absolute start, keeps their order stable no
    /// matter how many more are hoisted in later, while remaining valid even
    /// after the block gains a terminator (the terminator, if any, is always
    /// after `instr`, never `instr` itself).
    pub fn position_after(&self, instr: Value<'ctx>, block: BasicBlock<'ctx>) {
        // SAFETY: `instr` is live and belongs to `block`.
        let next = unsafe { LLVMGetNextInstruction(instr.as_raw()) };
        if next.is_null() {
            // SAFETY: `instr` is the block's last instruction, so appending at
            // the block's end places the new instruction right after it.
            unsafe { LLVMPositionBuilderAtEnd(self.raw, block.as_raw()) };
        } else {
            // SAFETY: the builder is live and `next` is an instruction of
            // `block`, which is the contract `LLVMPositionBuilderBefore` wants.
            unsafe { LLVMPositionBuilderBefore(self.raw, next) };
        }
    }

    /// Give every subsequent instruction this source location
    /// (`LLVMSetCurrentDebugLocation2`).
    ///
    /// This is one call per *statement*, not one per instruction: LLVM's
    /// builder hands the location to each instruction it creates, which is how
    /// a `.kp` line reaches the line table of the object without the lowering
    /// touching every `build_*` call (MK7 §4.4). The location stays current
    /// until the next call to this or to [`clear_debug_location`], so the
    /// lowering sets it at each statement.
    ///
    /// [`clear_debug_location`]: Builder::clear_debug_location
    pub fn set_debug_location(&self, location: Metadata<'ctx>) {
        // SAFETY: the builder is live and the location is a metadata node in the
        // builder's own context, which is what LLVM's IRBuilder requires.
        unsafe { LLVMSetCurrentDebugLocation2(self.raw, location.raw) };
    }

    /// Give every subsequent instruction no source location
    /// (`LLVMSetCurrentDebugLocation2` with a null location).
    ///
    /// A generated function must not inherit the location of the one emitted
    /// before it; an instruction with no location is honest, a wrong one is not.
    pub fn clear_debug_location(&self) {
        // SAFETY: the builder is live; LLVM reads a null location as "none".
        unsafe { LLVMSetCurrentDebugLocation2(self.raw, null_mut()) };
    }

    /// Emit `ret v` (`LLVMBuildRet`).
    ///
    /// A `ret` terminates its block, so nothing else may be emitted after it in
    /// the same block, and every block needs a terminator before verification.
    pub fn build_ret(&self, value: Value<'ctx>) -> Value<'ctx> {
        Value {
            // SAFETY: the builder is positioned and the value is live.
            raw: unsafe { LLVMBuildRet(self.raw, value.as_raw()) },
            context: PhantomData,
        }
    }

    /// Emit a call to `callee`, whose type is `ty`, with `args`
    /// (`LLVMBuildCall2`).
    ///
    /// The type is passed alongside the callee because `LLVMBuildCall2` takes it
    /// explicitly (the `2` in the name is that change: it replaced the untyped
    /// `LLVMBuildCall`, which LLVM removed once pointers stopped carrying their
    /// pointee). The result's type is `ty`'s return type.
    ///
    /// `name` is the IR name of the result value, and should be empty when the
    /// result is unused.
    pub fn build_call(
        &self,
        callee: Function<'ctx>,
        ty: FnType<'ctx>,
        args: &[Value<'ctx>],
        name: &str,
    ) -> Value<'ctx> {
        let mut args: Vec<LLVMValueRef> = args.iter().map(|a| a.as_raw()).collect();
        let name = cstring(name);
        Value {
            // SAFETY: the builder is positioned, the callee and its type are
            // live, and LLVM reads the argument vector only during the call. An
            // empty vector still yields a valid pointer for count 0.
            raw: unsafe {
                LLVMBuildCall2(
                    self.raw,
                    ty.as_raw(),
                    callee.as_raw(),
                    args.as_mut_ptr(),
                    args.len() as c_uint,
                    name.as_ptr(),
                )
            },
            context: PhantomData,
        }
    }

    /// Emit a call through a function *pointer* rather than a declared symbol
    /// (`LLVMBuildCall2` with a value callee).
    ///
    /// A Kelp function value is a plain pointer and a closure is a fat pointer
    /// whose first member is one, so the indirect call and the call through a
    /// closure's code pointer are the same instruction with a different
    /// argument list. The type is passed explicitly for the same reason
    /// [`Builder::build_call`] passes it: a pointer carries no pointee any more.
    pub fn build_indirect_call(
        &self,
        callee: Value<'ctx>,
        ty: FnType<'ctx>,
        args: &[Value<'ctx>],
        name: &str,
    ) -> Value<'ctx> {
        let mut args: Vec<LLVMValueRef> = args.iter().map(|a| a.as_raw()).collect();
        let name = cstring(name);
        Value {
            // SAFETY: the builder is positioned, the callee and its type are
            // live, and LLVM reads the argument vector only during the call.
            raw: unsafe {
                LLVMBuildCall2(
                    self.raw,
                    ty.as_raw(),
                    callee.as_raw(),
                    args.as_mut_ptr(),
                    args.len() as c_uint,
                    name.as_ptr(),
                )
            },
            context: PhantomData,
        }
    }

    /// A private global string containing `text` plus LLVM's implicit NUL, and a
    /// pointer to it (`LLVMBuildGlobalString`).
    ///
    /// The pointer is what a `char*` parameter takes, which is how a Kelp string
    /// literal reaches the runtime's `printf` path.
    ///
    /// Caveat from the C API: this one call is not builder-position independent.
    /// LLVM adds the global to the module of the builder's *current block*, so
    /// [`position_at_end`] must have run first, on a block in the module that
    /// wants the global.
    ///
    /// [`position_at_end`]: Builder::position_at_end
    pub fn global_string(&self, text: &str, name: &str) -> Value<'ctx> {
        let text = cstring(text);
        let name = cstring(name);
        Value {
            // SAFETY: the builder is positioned inside a module, as documented
            // above, and both strings are valid.
            raw: unsafe { LLVMBuildGlobalString(self.raw, text.as_ptr(), name.as_ptr()) },
            context: PhantomData,
        }
    }

    /// Emit `ret void` (`LLVMBuildRetVoid`), for a unit-returning function.
    pub fn build_ret_void(&self) -> Value<'ctx> {
        Value {
            // SAFETY: the builder is positioned.
            raw: unsafe { LLVMBuildRetVoid(self.raw) },
            context: PhantomData,
        }
    }

    /// Emit an unconditional branch to `dest` (`LLVMBuildBr`).
    ///
    /// A `br` terminates its block, so nothing else may follow it there.
    pub fn build_br(&self, dest: BasicBlock<'ctx>) -> Value<'ctx> {
        Value {
            // SAFETY: the builder is positioned and dest is live.
            raw: unsafe { LLVMBuildBr(self.raw, dest.as_raw()) },
            context: PhantomData,
        }
    }

    /// Emit a conditional branch on `cond` (`LLVMBuildCondBr`).
    ///
    /// `cond` is an `i1`; the lowering gets one from a comparison, or from
    /// `xor`-ing a boolean, never from a wider integer.
    pub fn build_cond_br(
        &self,
        cond: Value<'ctx>,
        then_block: BasicBlock<'ctx>,
        else_block: BasicBlock<'ctx>,
    ) -> Value<'ctx> {
        Value {
            // SAFETY: the builder is positioned and every argument is live.
            raw: unsafe {
                LLVMBuildCondBr(self.raw, cond.as_raw(), then_block.as_raw(), else_block.as_raw())
            },
            context: PhantomData,
        }
    }

    /// Emit `unreachable` (`LLVMBuildUnreachable`), the terminator for a block
    /// the program cannot fall out of (the panic arm of a range check).
    pub fn build_unreachable(&self) -> Value<'ctx> {
        Value {
            // SAFETY: the builder is positioned.
            raw: unsafe { LLVMBuildUnreachable(self.raw) },
            context: PhantomData,
        }
    }

    /// Emit a stack slot of type `ty` (`LLVMBuildAlloca`).
    ///
    /// The lowerer emits these for locals and parameters: a Kelp binding is a
    /// slot that is `store`d and `load`ed, which is how a name a `while` body
    /// writes keeps one location, and how an `if` used as a value carries its
    /// result to the join without an explicit phi. LLVM's `mem2reg` promotes
    /// them back to SSA values.
    pub fn build_alloca(&self, ty: Type<'ctx>, name: &str) -> Value<'ctx> {
        let name = cstring(name);
        Value {
            // SAFETY: the builder is positioned and the type is live.
            raw: unsafe { LLVMBuildAlloca(self.raw, ty.as_raw(), name.as_ptr()) },
            context: PhantomData,
        }
    }

    /// Emit `load ty, ptr value` (`LLVMBuildLoad2`).
    ///
    /// `ty` is the pointee type, passed explicitly because a pointer no longer
    /// carries one; it must be the type the address was allocated with.
    pub fn build_load(&self, ty: Type<'ctx>, ptr: Value<'ctx>, name: &str) -> Value<'ctx> {
        let name = cstring(name);
        Value {
            // SAFETY: the builder is positioned and both arguments are live.
            raw: unsafe { LLVMBuildLoad2(self.raw, ty.as_raw(), ptr.as_raw(), name.as_ptr()) },
            context: PhantomData,
        }
    }

    /// Emit `load atomic ty, ptr value seq_cst` (MK9-S3-M1 stage A1).
    ///
    /// The C API has no builder call that takes an ordering, so the load is
    /// emitted and then marked atomic with `LLVMSetOrdering`. That is the same
    /// instruction the IR reader produces from the `atomic` keyword: the
    /// ordering is a property of the memory access, not of a separate
    /// instruction.
    pub fn build_load_atomic(&self, ty: Type<'ctx>, ptr: Value<'ctx>, name: &str) -> Value<'ctx> {
        let value = self.build_load(ty, ptr, name);
        // SAFETY: `value` is a load this builder just emitted, which is the one
        // kind of instruction `LLVMSetOrdering` accepts.
        unsafe { LLVMSetOrdering(value.as_raw(), SEQ_CST) };
        value
    }

    /// Emit `store atomic value, ptr place seq_cst` (MK9-S3-M1 stage A1), the
    /// same emit-then-mark shape as [`Self::build_load_atomic`].
    pub fn build_store_atomic(&self, value: Value<'ctx>, place: Value<'ctx>) {
        let store = self.build_store(value, place);
        // SAFETY: `store` is a store this builder just emitted.
        unsafe { LLVMSetOrdering(store.as_raw(), SEQ_CST) };
    }

    /// Emit one `atomicrmw <op> ptr place, value seq_cst` instruction, whose
    /// value is the operand that was in memory.
    pub fn build_atomicrmw(
        &self,
        op: AtomicRmw,
        place: Value<'ctx>,
        value: Value<'ctx>,
    ) -> Value<'ctx> {
        Value {
            // SAFETY: the builder is positioned and every argument is live. The
            // result is always a value (the old contents), so it needs no name
            // and no ordering fix-up: `LLVMBuildAtomicRMW` takes the ordering.
            raw: unsafe {
                LLVMBuildAtomicRMW(
                    self.raw,
                    op.as_raw(),
                    place.as_raw(),
                    value.as_raw(),
                    SEQ_CST,
                    0,
                )
            },
            context: PhantomData,
        }
    }

    /// Emit `cmpxchg ptr place, cmp, new seq_cst seq_cst`, whose value is the
    /// `{ T, i1 }` pair LLVM defines: field 0 is the value read from memory
    /// (the value that was there on success, the value actually found on
    /// failure) and field 1 says whether it matched.
    pub fn build_cmpxchg(
        &self,
        place: Value<'ctx>,
        cmp: Value<'ctx>,
        new: Value<'ctx>,
    ) -> Value<'ctx> {
        Value {
            // SAFETY: the builder is positioned and every argument is live. The
            // compare is not weak: `Atomic.compare_exchange` retries in a loop
            // the caller writes, so a spurious failure would become a real one
            // the user did not ask for (MK9 9 leaves the weak form open, and
            // this is the strong half of that question).
            raw: unsafe {
                LLVMBuildAtomicCmpXchg(
                    self.raw,
                    place.as_raw(),
                    cmp.as_raw(),
                    new.as_raw(),
                    SEQ_CST,
                    SEQ_CST,
                    0,
                )
            },
            context: PhantomData,
        }
    }

    /// Emit `fence seq_cst` (`LLVMBuildFence`), the one instruction
    /// `thread.fence` lowers to (MK9-S3-M1 stage A2).
    ///
    /// The scope is the default one rather than `syncscope("singlethread")`: the
    /// ordering v1 states is the whole-system total order C11 and LLVM `seq_cst`
    /// mean, so `singleThread` is false. A fence has no value, so the name is
    /// empty and nothing is returned.
    pub fn build_fence(&self) {
        // SAFETY: the builder is positioned and there is nothing else to be
        // live; the instruction is inserted where the builder points.
        unsafe { LLVMBuildFence(self.raw, SEQ_CST, 0, cstring("").as_ptr()) };
    }

    /// Emit `inttoptr value to ty` (`LLVMBuildIntToPtr`): the machine address a
    /// raw pointer holds, recovered from an integer (MK9-S2 stage 1).
    pub fn build_inttoptr(&self, value: Value<'ctx>, ty: Type<'ctx>) -> Value<'ctx> {
        Value {
            // SAFETY: the builder is positioned and both arguments are live.
            raw: unsafe { LLVMBuildIntToPtr(self.raw, value.as_raw(), ty.as_raw(), cstring("ptr").as_ptr()) },
            context: PhantomData,
        }
    }

    /// Emit `ptrtoint value to ty` (`LLVMBuildPtrToInt`): the address of a raw
    /// pointer as an integer (`p.addr()`).
    pub fn build_ptrtoint(&self, value: Value<'ctx>, ty: Type<'ctx>) -> Value<'ctx> {
        Value {
            // SAFETY: the builder is positioned and both arguments are live.
            raw: unsafe { LLVMBuildPtrToInt(self.raw, value.as_raw(), ty.as_raw(), cstring("addr").as_ptr()) },
            context: PhantomData,
        }
    }

    /// Mark a load or store volatile (`LLVMSetVolatile`), the flag that forbids
    /// the optimizer to merge it, reorder it against another volatile access, or
    /// elide it (MK9-S2 stage 1). `value` must be the load or store this builder
    /// just emitted.
    pub fn set_volatile(&self, value: Value<'ctx>) {
        // SAFETY: the caller passes an instruction this builder just emitted,
        // which is the one kind `LLVMSetVolatile` accepts.
        unsafe { LLVMSetVolatile(value.as_raw(), 1) };
    }

    /// Set the alignment in bytes of the load or store `value` this builder
    /// just emitted (`LLVMSetAlignment`): `1` is a legal access at every
    /// address, which is what an unaligned load or store needs. An instruction
    /// without this call carries its type's ABI alignment.
    pub fn set_alignment(&self, value: Value<'ctx>, align: u32) {
        // SAFETY: the caller passes an instruction this builder just emitted,
        // which is the one kind `LLVMSetAlignment` accepts.
        unsafe { LLVMSetAlignment(value.as_raw(), align) };
    }

    /// Emit `extractvalue agg, index` (`LLVMBuildExtractValue`): one field of
    /// an aggregate value, which is how the `cmpxchg` pair's halves are read.
    pub fn build_extract_value(&self, agg: Value<'ctx>, index: u32, name: &str) -> Value<'ctx> {
        let name = cstring(name);
        Value {
            // SAFETY: the builder is positioned and the aggregate is live.
            raw: unsafe {
                LLVMBuildExtractValue(self.raw, agg.as_raw(), index as c_uint, name.as_ptr())
            },
            context: PhantomData,
        }
    }

    /// Emit `store value, ptr place` (`LLVMBuildStore`).
    pub fn build_store(&self, value: Value<'ctx>, place: Value<'ctx>) -> Value<'ctx> {
        Value {
            // SAFETY: the builder is positioned and both arguments are live.
            raw: unsafe { LLVMBuildStore(self.raw, value.as_raw(), place.as_raw()) },
            context: PhantomData,
        }
    }

    /// Emit `getelementptr ty, ptr p, i32 0, i32 index` (`LLVMBuildStructGEP2`),
    /// the address of one field of the aggregate `p` points at.
    ///
    /// `ty` is the aggregate the pointer addresses, passed explicitly because a
    /// pointer no longer carries a pointee type. LLVM's `CreateStructGEP` writes
    /// the `inbounds` form, which is justified here and only here: the index is
    /// a compile-time constant selecting a field of an object whose type is
    /// `ty`, so the address is inside the object by construction. MK7 §3.4's rule
    /// is about a *runtime* index into an array, where the bounds are not proven;
    /// this call cannot express one.
    pub fn build_struct_gep(
        &self,
        ty: Type<'ctx>,
        ptr: Value<'ctx>,
        index: u32,
        name: &str,
    ) -> Value<'ctx> {
        let name = cstring(name);
        Value {
            // SAFETY: the builder is positioned and both arguments are live.
            raw: unsafe {
                LLVMBuildStructGEP2(self.raw, ty.as_raw(), ptr.as_raw(), index as c_uint, name.as_ptr())
            },
            context: PhantomData,
        }
    }

    /// Emit `getelementptr ty, ptr p, i64 index` (`LLVMBuildGEP2`), the address
    /// of the `index`-th `ty` at `p`.
    ///
    /// Deliberately the *non*-`inbounds` form: `index` is a runtime value here,
    /// and the container function sets index past a length the bounds check just
    /// proved only for the callers that guarded. MK7 §3.4 forbids `inbounds`
    /// where the bounds are not proven at the point of the GEP, so this is the
    /// plain form and the one `codegen_llvm.rs` uses for every data-dependent
    /// element address (`List.data[i]`, `Map.entries[e]`).
    pub fn build_gep(
        &self,
        ty: Type<'ctx>,
        ptr: Value<'ctx>,
        index: Value<'ctx>,
        name: &str,
    ) -> Value<'ctx> {
        let name = cstring(name);
        let mut indices = [index.as_raw()];
        Value {
            // SAFETY: the builder is positioned, the type, pointer and index are
            // live, and LLVM reads the index vector only during the call.
            raw: unsafe {
                LLVMBuildGEP2(
                    self.raw,
                    ty.as_raw(),
                    ptr.as_raw(),
                    indices.as_mut_ptr(),
                    1,
                    name.as_ptr(),
                )
            },
            context: PhantomData,
        }
    }

    /// Emit `insertvalue agg, elt, index` (`LLVMBuildInsertValue`): `agg` with
    /// field `index` replaced by `elt`.
    ///
    /// The lowering builds a `datac` literal this way, starting from
    /// [`Context::undef`] and writing every field in declaration order, so no
    /// aggregate is ever stored and re-loaded just to be constructed.
    pub fn build_insert_value(
        &self,
        agg: Value<'ctx>,
        elt: Value<'ctx>,
        index: u32,
        name: &str,
    ) -> Value<'ctx> {
        let name = cstring(name);
        Value {
            // SAFETY: the builder is positioned and all three arguments are live.
            raw: unsafe {
                LLVMBuildInsertValue(self.raw, agg.as_raw(), elt.as_raw(), index as c_uint, name.as_ptr())
            },
            context: PhantomData,
        }
    }

    /// Emit `phi ty [ ... ]` (`LLVMBuildPhi`).
    ///
    /// The value is incomplete until [`add_incoming`] runs, and an incomplete
    /// phi fails verification. The lowerer uses a phi exactly where two control
    /// flow paths carry one value to a join: a short-circuited `&&`/`||`.
    ///
    /// [`add_incoming`]: Builder::add_incoming
    pub fn build_phi(&self, ty: Type<'ctx>, name: &str) -> Value<'ctx> {
        let name = cstring(name);
        Value {
            // SAFETY: the builder is positioned and the type is live.
            raw: unsafe { LLVMBuildPhi(self.raw, ty.as_raw(), name.as_ptr()) },
            context: PhantomData,
        }
    }

    /// Add the `(value, predecessor)` pairs a phi joins (`LLVMAddIncoming`).
    ///
    /// The two slices are parallel and the phi must already have been emitted
    /// into the block that follows the listed predecessors. Calling this once
    /// with every predecessor is the whole input; calling it again appends.
    /// Emit `switch ty value, label %default [ ... ]` (`LLVMBuildSwitch`).
    ///
    /// The switch starts with no cases; [`add_case`] supplies them. The lowering
    /// uses one per generated `enum` drop, the same shape the C backend's
    /// `switch (v->kelp_tag)` has, and the default block is where an unknown tag
    /// lands: it is a compiler-visible hole rather than a runtime check, exactly
    /// as the C backend's missing `default:` arm is.
    ///
    /// [`add_case`]: Builder::add_case
    pub fn build_switch(
        &self,
        value: Value<'ctx>,
        default: BasicBlock<'ctx>,
        cases: u32,
    ) -> Value<'ctx> {
        Value {
            // SAFETY: the builder is positioned and both arguments are live. The
            // case count is a hint LLVM uses to size the instruction, not the
            // list of cases, which `add_case` appends to.
            raw: unsafe {
                LLVMBuildSwitch(self.raw, value.as_raw(), default.as_raw(), cases as c_uint)
            },
            context: PhantomData,
        }
    }

    /// Add one `case` arm to a switch (`LLVMAddCase`): when `value` equals
    /// `on`, control goes to `dest`. `case` must name the switch and `dest` a
    /// block of the same function.
    pub fn add_case(&self, case: Value<'ctx>, on: Value<'ctx>, dest: BasicBlock<'ctx>) {
        // SAFETY: the switch and both handles are live; every case added before
        // the block containing the switch is terminated is part of it.
        unsafe { LLVMAddCase(case.as_raw(), on.as_raw(), dest.as_raw()) };
    }

    pub fn add_incoming(&self, phi: Value<'ctx>, incoming: &[(Value<'ctx>, BasicBlock<'ctx>)]) {
        let mut values: Vec<LLVMValueRef> = incoming.iter().map(|(v, _)| v.as_raw()).collect();
        let mut blocks: Vec<LLVMBasicBlockRef> = incoming.iter().map(|(_, b)| b.as_raw()).collect();
        // SAFETY: the phi is live, and the two parallel vectors are live for the
        // duration of the call.
        unsafe {
            LLVMAddIncoming(
                phi.as_raw(),
                values.as_mut_ptr(),
                blocks.as_mut_ptr(),
                incoming.len() as c_uint,
            )
        };
    }

    /// Emit one of the integer binary operators `a op b`.
    pub fn build_int_binop(
        &self,
        op: IntBinOp,
        lhs: Value<'ctx>,
        rhs: Value<'ctx>,
        name: &str,
    ) -> Value<'ctx> {
        let name = cstring(name);
        // SAFETY: the builder is positioned and both operands are live. Every
        // arm is one C API builder call that reads the same three arguments.
        let raw = unsafe {
            match op {
                IntBinOp::Add => LLVMBuildAdd(self.raw, lhs.as_raw(), rhs.as_raw(), name.as_ptr()),
                IntBinOp::Sub => LLVMBuildSub(self.raw, lhs.as_raw(), rhs.as_raw(), name.as_ptr()),
                IntBinOp::Mul => LLVMBuildMul(self.raw, lhs.as_raw(), rhs.as_raw(), name.as_ptr()),
                IntBinOp::SDiv => LLVMBuildSDiv(self.raw, lhs.as_raw(), rhs.as_raw(), name.as_ptr()),
                IntBinOp::UDiv => LLVMBuildUDiv(self.raw, lhs.as_raw(), rhs.as_raw(), name.as_ptr()),
                IntBinOp::SRem => LLVMBuildSRem(self.raw, lhs.as_raw(), rhs.as_raw(), name.as_ptr()),
                IntBinOp::URem => LLVMBuildURem(self.raw, lhs.as_raw(), rhs.as_raw(), name.as_ptr()),
                IntBinOp::And => LLVMBuildAnd(self.raw, lhs.as_raw(), rhs.as_raw(), name.as_ptr()),
                IntBinOp::Or => LLVMBuildOr(self.raw, lhs.as_raw(), rhs.as_raw(), name.as_ptr()),
                IntBinOp::Xor => LLVMBuildXor(self.raw, lhs.as_raw(), rhs.as_raw(), name.as_ptr()),
                IntBinOp::Shl => LLVMBuildShl(self.raw, lhs.as_raw(), rhs.as_raw(), name.as_ptr()),
                IntBinOp::LShr => {
                    LLVMBuildLShr(self.raw, lhs.as_raw(), rhs.as_raw(), name.as_ptr())
                }
                IntBinOp::AShr => {
                    LLVMBuildAShr(self.raw, lhs.as_raw(), rhs.as_raw(), name.as_ptr())
                }
            }
        };
        Value {
            raw,
            context: PhantomData,
        }
    }

    /// Emit `fneg value` (`LLVMBuildFNeg`), the one unary float instruction.
    pub fn build_fneg(&self, value: Value<'ctx>, name: &str) -> Value<'ctx> {
        let name = cstring(name);
        Value {
            // SAFETY: the builder is positioned and the operand is live.
            raw: unsafe { LLVMBuildFNeg(self.raw, value.as_raw(), name.as_ptr()) },
            context: PhantomData,
        }
    }

    /// Emit one of the floating-point binary operators `a op b`.
    pub fn build_float_binop(
        &self,
        op: FloatBinOp,
        lhs: Value<'ctx>,
        rhs: Value<'ctx>,
        name: &str,
    ) -> Value<'ctx> {
        let name = cstring(name);
        // SAFETY: the builder is positioned and both operands are live.
        let raw = unsafe {
            match op {
                FloatBinOp::Add => {
                    llvm_sys::core::LLVMBuildFAdd(self.raw, lhs.as_raw(), rhs.as_raw(), name.as_ptr())
                }
                FloatBinOp::Sub => {
                    llvm_sys::core::LLVMBuildFSub(self.raw, lhs.as_raw(), rhs.as_raw(), name.as_ptr())
                }
                FloatBinOp::Mul => {
                    llvm_sys::core::LLVMBuildFMul(self.raw, lhs.as_raw(), rhs.as_raw(), name.as_ptr())
                }
                FloatBinOp::Div => {
                    llvm_sys::core::LLVMBuildFDiv(self.raw, lhs.as_raw(), rhs.as_raw(), name.as_ptr())
                }
            }
        };
        Value {
            raw,
            context: PhantomData,
        }
    }

    /// Emit an integer comparison (`LLVMBuildICmp`), yielding an `i1`.
    pub fn build_icmp(
        &self,
        cmp: IntCmp,
        lhs: Value<'ctx>,
        rhs: Value<'ctx>,
        name: &str,
    ) -> Value<'ctx> {
        let pred = match cmp {
            IntCmp::Eq => LLVMIntPredicate::LLVMIntEQ,
            IntCmp::Ne => LLVMIntPredicate::LLVMIntNE,
            IntCmp::Slt => LLVMIntPredicate::LLVMIntSLT,
            IntCmp::Sle => LLVMIntPredicate::LLVMIntSLE,
            IntCmp::Sgt => LLVMIntPredicate::LLVMIntSGT,
            IntCmp::Sge => LLVMIntPredicate::LLVMIntSGE,
            IntCmp::Ult => LLVMIntPredicate::LLVMIntULT,
            IntCmp::Ule => LLVMIntPredicate::LLVMIntULE,
            IntCmp::Ugt => LLVMIntPredicate::LLVMIntUGT,
            IntCmp::Uge => LLVMIntPredicate::LLVMIntUGE,
        };
        let name = cstring(name);
        Value {
            // SAFETY: the builder is positioned and both operands are live.
            raw: unsafe {
                LLVMBuildICmp(self.raw, pred, lhs.as_raw(), rhs.as_raw(), name.as_ptr())
            },
            context: PhantomData,
        }
    }

    /// Emit a floating-point comparison (`LLVMBuildFCmp`), yielding an `i1`.
    pub fn build_fcmp(
        &self,
        cmp: RealCmp,
        lhs: Value<'ctx>,
        rhs: Value<'ctx>,
        name: &str,
    ) -> Value<'ctx> {
        let pred = match cmp {
            RealCmp::Oeq => LLVMRealPredicate::LLVMRealOEQ,
            RealCmp::One => LLVMRealPredicate::LLVMRealUNE,
            RealCmp::Olt => LLVMRealPredicate::LLVMRealOLT,
            RealCmp::Ole => LLVMRealPredicate::LLVMRealOLE,
            RealCmp::Ogt => LLVMRealPredicate::LLVMRealOGT,
            RealCmp::Oge => LLVMRealPredicate::LLVMRealOGE,
        };
        let name = cstring(name);
        Value {
            // SAFETY: the builder is positioned and both operands are live.
            raw: unsafe {
                LLVMBuildFCmp(self.raw, pred, lhs.as_raw(), rhs.as_raw(), name.as_ptr())
            },
            context: PhantomData,
        }
    }

    /// Emit `select cond, then, else` (`LLVMBuildSelect`).
    ///
    /// The lowerer uses this to turn an `i1` into one of two pointers for a
    /// boolean `print`, not to implement control flow: a `select` evaluates both
    /// arms, so it cannot stand in for `&&`/`||` or an `if`.
    pub fn build_select(
        &self,
        cond: Value<'ctx>,
        then_value: Value<'ctx>,
        else_value: Value<'ctx>,
        name: &str,
    ) -> Value<'ctx> {
        let name = cstring(name);
        Value {
            // SAFETY: the builder is positioned and every argument is live.
            raw: unsafe {
                LLVMBuildSelect(
                    self.raw,
                    cond.as_raw(),
                    then_value.as_raw(),
                    else_value.as_raw(),
                    name.as_ptr(),
                )
            },
            context: PhantomData,
        }
    }

    /// Emit an integer width conversion: `zext`, `sext` or `trunc`
    /// (`LLVMBuildZExt`/`LLVMBuildSExt`/`LLVMBuildTrunc`).
    ///
    /// `signed` says whether the source is interpreted as signed, which decides
    /// between a sign and a zero extension; a truncation ignores it, because
    /// taking the low bits is the same operation either way. `widen` must be
    /// true exactly when the target is wider than the source: a same-width
    /// conversion emits nothing and the caller skips this method, and a wider
    /// target with this false would emit a `trunc` that the verifier rejects.
    pub fn build_int_cast(
        &self,
        signed: bool,
        widen: bool,
        value: Value<'ctx>,
        ty: Type<'ctx>,
        name: &str,
    ) -> Value<'ctx> {
        let name = cstring(name);
        // SAFETY: the builder is positioned and the value is live.
        let raw = unsafe {
            match (widen, signed) {
                (true, true) => LLVMBuildSExt(self.raw, value.as_raw(), ty.as_raw(), name.as_ptr()),
                (true, false) => LLVMBuildZExt(self.raw, value.as_raw(), ty.as_raw(), name.as_ptr()),
                (false, _) => LLVMBuildTrunc(self.raw, value.as_raw(), ty.as_raw(), name.as_ptr()),
            }
        };
        Value {
            raw,
            context: PhantomData,
        }
    }

    /// Emit a float width conversion: `fpext` when `widen`, else `fptrunc`
    /// (`LLVMBuildFPExt`/`LLVMBuildFPTrunc`).
    pub fn build_fp_cast(
        &self,
        widen: bool,
        value: Value<'ctx>,
        ty: Type<'ctx>,
        name: &str,
    ) -> Value<'ctx> {
        let name = cstring(name);
        // SAFETY: the builder is positioned and the value is live.
        let raw = unsafe {
            if widen {
                LLVMBuildFPExt(self.raw, value.as_raw(), ty.as_raw(), name.as_ptr())
            } else {
                LLVMBuildFPTrunc(self.raw, value.as_raw(), ty.as_raw(), name.as_ptr())
            }
        };
        Value {
            raw,
            context: PhantomData,
        }
    }

    /// Emit an integer-to-float conversion, `sitofp` or `uitofp`
    /// (`LLVMBuildSIToFP`/`LLVMBuildUIToFP`), by the source's signedness.
    pub fn build_int_to_fp(
        &self,
        signed: bool,
        value: Value<'ctx>,
        ty: Type<'ctx>,
        name: &str,
    ) -> Value<'ctx> {
        let name = cstring(name);
        // SAFETY: the builder is positioned and the value is live.
        let raw = unsafe {
            if signed {
                LLVMBuildSIToFP(self.raw, value.as_raw(), ty.as_raw(), name.as_ptr())
            } else {
                LLVMBuildUIToFP(self.raw, value.as_raw(), ty.as_raw(), name.as_ptr())
            }
        };
        Value {
            raw,
            context: PhantomData,
        }
    }

    /// Emit a float-to-integer conversion, `fptosi` or `fptoui`
    /// (`LLVMBuildFPToSI`/`LLVMBuildFPToUI`), by the target's signedness.
    pub fn build_fp_to_int(
        &self,
        signed: bool,
        value: Value<'ctx>,
        ty: Type<'ctx>,
        name: &str,
    ) -> Value<'ctx> {
        let name = cstring(name);
        // SAFETY: the builder is positioned and the value is live.
        let raw = unsafe {
            if signed {
                LLVMBuildFPToSI(self.raw, value.as_raw(), ty.as_raw(), name.as_ptr())
            } else {
                LLVMBuildFPToUI(self.raw, value.as_raw(), ty.as_raw(), name.as_ptr())
            }
        };
        Value {
            raw,
            context: PhantomData,
        }
    }
}

impl Drop for Builder<'_> {
    fn drop(&mut self) {
        // SAFETY: raw came from LLVMCreateBuilderInContext and is disposed once.
        unsafe { LLVMDisposeBuilder(self.raw) };
    }
}
