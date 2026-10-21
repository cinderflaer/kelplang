use super::*;

/// Guard for `LLVMModuleRef`: one translation unit, and the owner of its
/// functions and basic blocks.
pub struct Module<'ctx> {
    pub(crate) raw: LLVMModuleRef,
    // Kept so that a function's declaration can name its context without
    // threading `&'ctx Context` through every method. LLVM has no
    // LLVMGetModuleContext, and the pointer is only valid because the lifetime
    // marker below ties this module to the guard that owns it.
    pub(crate) context: LLVMContextRef,
    pub(crate) context_marker: PhantomData<&'ctx Context>,
}

impl<'ctx> Module<'ctx> {
    /// An empty module named `name` in `ctx`
    /// (`LLVMModuleCreateWithNameInContext`), disposed on drop.
    ///
    /// The name is the IR module identifier and, for the C API, also the default
    /// source file name in the emitted object, so it should name the Kelp module
    /// being built.
    pub fn new(ctx: &'ctx Context, name: &str) -> Self {
        let name = cstring(name);
        Module {
            // SAFETY: the context is live for 'ctx and the name is valid.
            raw: unsafe { LLVMModuleCreateWithNameInContext(name.as_ptr(), ctx.as_raw()) },
            context: ctx.as_raw(),
            context_marker: PhantomData,
        }
    }

    /// The underlying `LLVMModuleRef`, for a C API call this wrapper lacks.
    ///
    /// This is what [`TargetMachine::emit_object`] eventually passes to LLVM, so
    /// a caller that reaches for the raw ref must keep the guard alive, not only
    /// the pointer.
    pub fn as_raw(&self) -> LLVMModuleRef {
        self.raw
    }

    /// Set the module's target triple (`LLVMSetTarget`), which selects the data
    /// layout and the object format when the module is emitted.
    ///
    /// The triple should come from [`default_target_triple`] (or from a
    /// deliberate cross target). An empty triple leaves the defaults, and codegen
    /// then assumes the target machine's layout rather than the module's.
    pub fn set_triple(&mut self, triple: &str) {
        let triple = cstring(triple);
        // SAFETY: the module is live and the string is valid for the call.
        unsafe { LLVMSetTarget(self.raw, triple.as_ptr()) };
    }

    /// The module's target triple (`LLVMGetTarget`), empty if unset.
    ///
    /// The returned string is owned by the module, so it is copied out through
    /// [`copy_str`] rather than freed.
    pub fn triple(&self) -> String {
        // SAFETY: the module is live; the pointer it returns stays valid while
        // it is, and copy_str only reads.
        unsafe { copy_str(LLVMGetTarget(self.raw)) }
    }

    /// Give the module `layout`, the target's data layout
    /// (`LLVMSetModuleDataLayout`), so its `target datalayout` string matches
    /// the machine that emits the object.
    ///
    /// This is not cosmetic. A data-layout string is what a pass uses to size
    /// a type and to place a struct field, and a module without one gets LLVM's
    /// built-in default, which is not the host's (the default aligns an `i64` to
    /// 4, the `x86-64` layout to 8). A pass that folds a struct GEP to a byte
    /// offset under the wrong layout then computes an offset codegen believes,
    /// which is a silent wrong-answer miscompile (MK7 S6 found it at `-O1` and
    /// above). Taking the layout from the same `TargetMachine` that will write
    /// the object is what makes the passes and codegen agree.
    pub fn set_data_layout(&self, layout: &DataLayout) {
        // SAFETY: the module and the layout are live, and LLVM copies the
        // layout out of the target-data handle.
        unsafe { LLVMSetModuleDataLayout(self.raw, layout.raw) };
    }

    /// Append `asm` to the module's module-level inline assembly
    /// (`LLVMAppendModuleInlineAsm`), emitted verbatim into the assembly the
    /// object is written from.
    ///
    /// This is how a freestanding build writes `_start` (kernel stage 4):
    /// loading `sp` from a linker-script symbol is not expressible in the
    /// language yet, and a module-level blob is the one place the backend can
    /// put it without a function to attach it to. The text is appended, so two
    /// calls write two blobs in order.
    pub fn append_inline_asm(&self, asm: &str) {
        // SAFETY: the module is live and the string's length is passed with it,
        // so LLVM copies exactly these bytes.
        unsafe { LLVMAppendModuleInlineAsm(self.raw, asm.as_ptr() as *const c_char, asm.len()) };
    }

    /// Declare a function named `name` of type `ty` in this module
    /// (`LLVMAddFunction`).
    ///
    /// A name the module already holds is *not* reused: LLVM renames the new
    /// function (`memcpy` becomes `memcpy.1`), so a caller that wants the
    /// existing declaration asks [`Module::get_function`] first. The result is
    /// a declaration until a basic block with a terminator is appended to it.
    pub fn add_function(&self, name: &str, ty: FnType<'ctx>) -> Function<'ctx> {
        let name = cstring(name);
        Function {
            // SAFETY: the module is live and the name is valid.
            raw: unsafe { LLVMAddFunction(self.raw, name.as_ptr(), ty.as_raw()) },
            context: self.context,
            context_marker: PhantomData,
        }
    }

    /// Declare an external global named `name` of type `ty` (`LLVMAddGlobal`).
    ///
    /// No initializer means the symbol is a declaration, resolved by the linker:
    /// this is how a libc object such as `stderr` (a `FILE*`) enters the module
    /// so a `load` can read it. A name the module already has is returned by
    /// `LLVMAddGlobal`, so this is safe to call again for the same symbol.
    pub fn add_external_global(&self, name: &str, ty: Type<'ctx>) -> Value<'ctx> {
        self.add_global(name, ty)
    }

    /// Define a module-scope global named `name` of type `ty` (`LLVMAddGlobal`).
    ///
    /// The global's storage exists in this object; the caller gives it an
    /// initializer and a linkage (`Value::set_initializer`,
    /// `Value::set_internal_linkage`). Without an initializer an `LLVMAddGlobal`
    /// with the default external linkage is a *declaration* the linker must
    /// resolve, which is the shape [`Module::add_external_global`] wants and not
    /// this one.
    pub fn add_global(&self, name: &str, ty: Type<'ctx>) -> Value<'ctx> {
        let name = cstring(name);
        Value {
            // SAFETY: the module is live and the name is valid.
            raw: unsafe { LLVMAddGlobal(self.raw, ty.as_raw(), name.as_ptr()) },
            context: PhantomData,
        }
    }

    /// Every function in the module in declaration order
    /// (`LLVMGetFirstFunction`/`LLVMGetNextFunction`), declarations included.
    /// The freestanding probe-stack sweep uses this to reach the functions the
    /// lowering declared through its own helpers, not only the ones the
    /// front-end loop names.
    pub fn functions(&self) -> Vec<Function<'ctx>> {
        let mut functions = Vec::new();
        // SAFETY: each handle is owned by this module and stays live while the
        // module does, which the returned lifetimes promise.
        let mut raw = unsafe { LLVMGetFirstFunction(self.raw) };
        while !raw.is_null() {
            functions.push(Function {
                raw,
                context: self.context,
                context_marker: PhantomData,
            });
            raw = unsafe { LLVMGetNextFunction(raw) };
        }
        functions
    }

    /// The function named `name` if the module has one (`LLVMGetNamedFunction`).
    ///
    /// This is how a second mention of a runtime symbol (a second call to `puts`,
    /// say) reuses the first declaration instead of asking [`add_function`] to
    /// resolve the name again.
    ///
    /// [`add_function`]: Module::add_function
    pub fn get_function(&self, name: &str) -> Option<Function<'ctx>> {
        let name = cstring(name);
        // SAFETY: the module is live and the name is valid.
        let raw = unsafe { LLVMGetNamedFunction(self.raw, name.as_ptr()) };
        if raw.is_null() {
            return None;
        }
        Some(Function {
            raw,
            context: self.context,
            context_marker: PhantomData,
        })
    }

    /// Run LLVM's verifier over the module (`LLVMVerifyModule`).
    ///
    /// `LLVMReturnStatusAction` returns a status instead of printing to stderr,
    /// and LLVM's description of what is wrong is returned in `Err`. A caller
    /// must run this before emitting: LLVM's object writer assumes a verified
    /// module, and an unverified one can crash it rather than fail the call.
    pub fn verify(&self) -> Result<(), String> {
        let mut message: *mut c_char = null_mut();
        // SAFETY: the module is live and the out-parameter points at our slot for
        // the malloc'd message LLVM writes there.
        let status = unsafe {
            LLVMVerifyModule(
                self.raw,
                LLVMVerifierFailureAction::LLVMReturnStatusAction,
                &mut message,
            )
        };
        // SAFETY: LLVM either left the slot null or put a malloc'd message in it.
        let text = unsafe { take_message(message) };
        if status == 0 {
            Ok(())
        } else if text.is_empty() {
            Err("LLVMVerifyModule failed without a message".to_string())
        } else {
            Err(text)
        }
    }

    /// Run the pass pipeline `pipeline` over the module (`LLVMRunPasses`, the
    /// new pass manager's textual entry point).
    ///
    /// `pipeline` is a spelling like `default<O2>`; `machine` supplies the
    /// target and the data layout the target-specific passes (the vectorizer,
    /// the unroller) need, and LLVM reads it rather than taking it, so it stays
    /// owned by the caller.
    ///
    /// The pass manager reports a failure as an `LLVMErrorRef` rather than a
    /// status code, which is why [`take_error`] exists; an unparseable pipeline
    /// and a pass that reports an error both arrive here. A pipeline that runs
    /// is not proof the module is still well formed, so the caller verifies
    /// again before emitting (see `codegen_llvm::compile`).
    pub fn run_passes(&self, pipeline: &str, machine: &TargetMachine) -> Result<(), String> {
        let pipeline = cstring(pipeline);
        // SAFETY: the options are freshly created and released once below, and
        // LLVM copies no pointer out of them.
        let options = unsafe { LLVMCreatePassBuilderOptions() };
        // SAFETY: the module and machine are live, the pipeline string is
        // NUL-terminated and outlives the call, and the options came from the
        // create above.
        let error = unsafe {
            LLVMRunPasses(self.raw, pipeline.as_ptr(), machine.as_raw(), options)
        };
        // SAFETY: the options are released exactly once, by the disposer that
        // pairs with the create.
        unsafe { LLVMDisposePassBuilderOptions(options) };
        if error.is_null() {
            return Ok(());
        }
        // SAFETY: the error is non-null and not yet consumed; this read is the
        // consuming one.
        let text = unsafe { take_error(error) };
        Err(if text.is_empty() {
            format!("LLVM could not run the pass pipeline '{}'", pipeline.to_string_lossy())
        } else {
            text
        })
    }

    /// Record `cu` as this module's compile unit, in the `llvm.dbg.cu` named
    /// metadata node (`LLVMAddNamedMetadataOperand`).
    ///
    /// A `DICompileUnit` is a metadata node and nothing else: it has to be
    /// named by this node as well, or the object carries no DWARF at all. The
    /// C API's `LLVMDIBuilderCreateCompileUnit` builds the node and stops
    /// there, exactly as clang's frontend does the same second half by hand.
    pub fn add_debug_compile_unit(&self, cu: Metadata<'ctx>) {
        const KEY: &str = "llvm.dbg.cu";
        let key = cstring(KEY);
        // SAFETY: the module is live and `cu` belongs to the same context, which
        // is what makes the metadata's context match the module's.
        unsafe {
            LLVMAddNamedMetadataOperand(
                self.raw,
                key.as_ptr(),
                LLVMMetadataAsValue(self.context, cu.raw),
            )
        };
    }

    /// Set the `Debug Info Version` module flag (`LLVMAddModuleFlag`) to the
    /// version this LLVM's metadata is written in
    /// (`LLVMDebugMetadataVersion`).
    ///
    /// The object writer reads this flag to decide whether the module carries
    /// debug info at all; without it the debug sections are dropped, so the
    /// compile unit above is not enough on its own. `LLVMAddModuleFlag` is a
    /// no-op when the flag already exists.
    pub fn set_debug_info_version(&self) {
        const KEY: &str = "Debug Info Version";
        let key = cstring(KEY);
        // SAFETY: the context is live, the key outlives the call, and the value
        // is an `i32` constant in that context, which is the metadata shape the
        // flag requires.
        unsafe {
            let value = LLVMConstInt(
                LLVMInt32TypeInContext(self.context),
                LLVMDebugMetadataVersion() as u64,
                0,
            );
            LLVMAddModuleFlag(
                self.raw,
                LLVMModuleFlagBehavior::LLVMModuleFlagBehaviorWarning,
                key.as_ptr(),
                KEY.len(),
                LLVMValueAsMetadata(value),
            )
        };
    }

    /// Sets the `uwtable` module flag to the async kind, so every function the
    /// module defines carries an unwind table. A worker that panics ends its
    /// thread with `pthread_exit`, which unwinds the worker's frames; glibc
    /// aborts when a frame has no table to unwind by, and a function the IR
    /// defines has none unless it is asked for. C functions carry one by default
    /// on x86-64, which is why the C backend never needed this.
    pub fn set_unwind_tables(&self) {
        const KEY: &str = "uwtable";
        let key = cstring(KEY);
        // SAFETY: the context is live, the key outlives the call, and the value
        // is an `i32` constant in that context, the shape the flag requires.
        unsafe {
            let value = LLVMConstInt(LLVMInt32TypeInContext(self.context), 2, 0);
            LLVMAddModuleFlag(
                self.raw,
                LLVMModuleFlagBehavior::LLVMModuleFlagBehaviorOverride,
                key.as_ptr(),
                KEY.len(),
                LLVMValueAsMetadata(value),
            )
        };
    }

    /// The module as textual LLVM IR (`LLVMPrintModuleToString`).
    ///
    /// This is the `--emit-ir` text of MK7 §4.4, and the binary already has an
    /// extension for it (`.ll`). LLVM assembles the string on each call and the
    /// result is freed here with [`take_message`].
    pub fn to_ir_string(&self) -> String {
        // SAFETY: the module is live and the call returns an owned malloc'd string.
        let ptr = unsafe { LLVMPrintModuleToString(self.raw) };
        // SAFETY: that string is ours to free, which take_message does.
        unsafe { take_message(ptr) }
    }
}

impl Drop for Module<'_> {
    fn drop(&mut self) {
        // SAFETY: raw came from LLVMModuleCreateWithNameInContext and is disposed
        // once, after every handle borrowed from it is out of scope.
        unsafe { LLVMDisposeModule(self.raw) };
    }
}
