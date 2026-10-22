use super::*;

/// A type handle, owned by the [`Context`] that created it.
#[derive(Clone, Copy)]
pub struct Type<'ctx> {
    pub(crate) raw: LLVMTypeRef,
    pub(crate) context: PhantomData<&'ctx Context>,
}

impl<'ctx> Type<'ctx> {
    /// The underlying `LLVMTypeRef`, for a C API call this wrapper lacks.
    pub fn as_raw(self) -> LLVMTypeRef {
        self.raw
    }

    /// The width of this integer type in bits (`LLVMGetIntTypeWidth`), or
    /// `None` when the type is not an integer.
    ///
    /// The C ABI needs the width to decide whether a parameter or a return is
    /// a *sub-word* integer, which is the question the `zeroext`/`signext`
    /// attribute answers, so this is what [`Function::add_parameter_extension`]
    /// and [`Function::add_return_extension`] are keyed on rather than the
    /// Kelp type: a declaration site that has only the LLVM type (the runtime
    /// and capability tables) has to ask the same question the ones with a
    /// `Ty` do.
    pub fn int_width(self) -> Option<c_uint> {
        // SAFETY: the handle is live in the context that created it, and LLVM
        // reads it only for the duration of each call.
        match unsafe { LLVMGetTypeKind(self.raw) } {
            LLVMTypeKind::LLVMIntegerTypeKind => {
                // SAFETY: the kind just read says this is an integer type.
                Some(unsafe { LLVMGetIntTypeWidth(self.raw) })
            }
            _ => None,
        }
    }
}

/// A function type handle: the `ret (args...)` of a [`Function`].
#[derive(Clone, Copy)]
pub struct FnType<'ctx> {
    pub(crate) raw: LLVMTypeRef,
    pub(crate) context: PhantomData<&'ctx Context>,
}

impl<'ctx> FnType<'ctx> {
    /// The underlying `LLVMTypeRef` (a function type is a type in LLVM).
    pub fn as_raw(self) -> LLVMTypeRef {
        self.raw
    }
}

/// A value handle: a constant, a function, or the result of an instruction.
#[derive(Clone, Copy)]
pub struct Value<'ctx> {
    pub(crate) raw: LLVMValueRef,
    pub(crate) context: PhantomData<&'ctx Context>,
}

impl<'ctx> Value<'ctx> {
    /// Whether this value is a constant (`LLVMIsConstant`), which is what lets
    /// an aggregate of such values be built in one step.
    pub fn is_constant(&self) -> bool {
        // SAFETY: the value handle is live and LLVM reads only its own kind.
        unsafe { LLVMIsConstant(self.raw) != 0 }
    }

    /// The underlying `LLVMValueRef`, for a C API call this wrapper lacks.
    pub fn as_raw(self) -> LLVMValueRef {
        self.raw
    }

    /// Give a module-scope global its initializer (`LLVMSetInitializer`), which
    /// is what turns the declaration [`Module::add_global`] made into a
    /// definition. The value must be an LLVM constant of the global's type.
    pub fn set_initializer(&self, constant: Value<'ctx>) {
        // SAFETY: the caller passes a global this wrapper created and a constant
        // of its type; both are live in the same context.
        unsafe { LLVMSetInitializer(self.raw, constant.as_raw()) };
    }

    /// Mark a global's storage read-only (`LLVMSetGlobalConstant`), the `const`
    /// in `static const`.
    pub fn set_constant(&self, is_constant: bool) {
        // SAFETY: as above; the argument is a global handle's own ref.
        unsafe { LLVMSetGlobalConstant(self.raw, is_constant as LLVMBool) };
    }

    /// Give a global internal linkage (`LLVMSetLinkage` with
    /// `LLVMInternalLinkage`), which is C's `static`: the symbol is private to
    /// this object and never a link-time name.
    pub fn set_internal_linkage(&self) {
        // SAFETY: as above; the argument is a global handle's own ref.
        unsafe { LLVMSetLinkage(self.raw, LLVMLinkage::LLVMInternalLinkage) };
    }

    /// Give a global appending linkage (`LLVMSetLinkage` with
    /// `LLVMAppendingLinkage`), the linkage `@llvm.used` itself carries so the
    /// linker never drops a symbol a program reaches only through its section.
    pub fn set_appending_linkage(&self) {
        // SAFETY: as above; the argument is a global handle's own ref.
        unsafe { LLVMSetLinkage(self.raw, LLVMLinkage::LLVMAppendingLinkage) };
    }

    /// Make a global thread-local with the initial-exec model
    /// (`LLVMSetThreadLocal` and `LLVMSetThreadLocalMode`), the model the
    /// runtime's own per-thread variables use: one load from the thread
    /// pointer, no `__tls_get_addr` call.
    pub fn set_thread_local_initial_exec(&self) {
        // SAFETY: as above; the mode value is LLVM's `LLVMInitialExecTLSModel`.
        unsafe {
            LLVMSetThreadLocal(self.raw, 1);
            LLVMSetThreadLocalMode(self.raw, llvm_sys::LLVMThreadLocalMode::LLVMInitialExecTLSModel);
        }
    }

    /// Give a global (or a function) weak linkage (`LLVMSetLinkage` with
    /// `LLVMWeakAnyLinkage`), which is C's `__attribute__((weak))`: this object
    /// defines the symbol, but the linker prefers any strong definition from
    /// another object, so a program that supplies its own wins. The
    /// freestanding `mem*` fallbacks are defined this way (kernel gap 4).
    pub fn set_weak_linkage(&self) {
        // SAFETY: as above; the argument is a global handle's own ref.
        unsafe { LLVMSetLinkage(self.raw, LLVMLinkage::LLVMWeakAnyLinkage) };
    }

    /// Place a global (or a function) in section `section` (`LLVMSetSection`),
    /// which is how a `section("...")` clause reaches the object writer
    /// (kernel stage 4). An empty name would be the target's default, so the
    /// caller passes only a name a manifest wrote.
    pub fn set_section(&self, section: &str) {
        let section = cstring(section);
        // SAFETY: as above; LLVM copies the string.
        unsafe { LLVMSetSection(self.raw, section.as_ptr()) };
    }

    /// Set this function's alignment in bytes (`LLVMSetAlignment`), which is
    /// what the `align(N)` clause lowers to (kernel gap 2): the object writer
    /// emits the `.p2align` that puts the first byte at an `N`-aligned address,
    /// which is what a trap vector needs.
    pub fn set_function_alignment(&self, align: u32) {
        // SAFETY: `raw` is a function value, which is the one kind
        // `LLVMSetAlignment` accepts besides a global variable.
        unsafe { LLVMSetAlignment(self.raw, align) };
    }

    /// Set a global variable's alignment in bytes (`LLVMSetAlignment`), which
    /// is what a `percpu` static's 64-byte cache-line alignment lowers to
    /// (kernel stage 8): the object writer emits the alignment that keeps each
    /// copy on a cache line of its own.
    pub fn set_global_alignment(&self, align: u32) {
        // SAFETY: `raw` is a global variable value, the other kind
        // `LLVMSetAlignment` accepts.
        unsafe { LLVMSetAlignment(self.raw, align) };
    }
}

/// The sign of a sub-word integer that crosses to a C body, as LLVM's
/// `zeroext` and `signext` parameter and return attributes.
///
/// A register argument is at least thirty-two bits wide and a C body reads the
/// whole register for a parameter narrower than that, so clang extends every
/// sub-word parameter at the call and every sub-word return inside the body.
/// Without the attribute, LLVM passes whatever the upper bits of the register
/// happen to hold, and the body's widening reads them as part of the value
/// (BUG-rK2mW). The C prototype's signedness picks the variant: an unsigned
/// type (`uint8_t`, `uint16_t`, `bool`) is zero-extended and a signed one
/// (`int8_t`, `int16_t`, `char`) sign-extended.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IntExtension {
    /// The C type is unsigned, so the upper bits are zero (`zeroext`).
    ZeroExt,
    /// The C type is signed, so the upper bits repeat the sign bit (`signext`).
    SignExt,
}

impl IntExtension {
    /// The attribute's own spelling.
    fn name(self) -> &'static str {
        match self {
            IntExtension::ZeroExt => "zeroext",
            IntExtension::SignExt => "signext",
        }
    }
}

/// A function: a declaration or a definition in a [`Module`].
#[derive(Clone, Copy)]
pub struct Function<'ctx> {
    pub(crate) raw: LLVMValueRef,
    pub(crate) context: LLVMContextRef,
    pub(crate) context_marker: PhantomData<&'ctx Context>,
}

impl<'ctx> Function<'ctx> {
    /// The underlying `LLVMValueRef` (a function is a value in LLVM).
    pub fn as_raw(self) -> LLVMValueRef {
        self.raw
    }

    /// The function's own address as an ordinary pointer value, which is what a
    /// Kelp function value *is* (A-4). In LLVM a function is already a value of
    /// pointer type, so this is the same `LLVMValueRef` handed back as a
    /// [`Value`]; the copy is what lets it be stored in a slot, passed as an
    /// argument or wrapped in a closure without a second instruction.
    pub fn as_value(self) -> Value<'ctx> {
        Value {
            raw: self.raw,
            context: PhantomData,
        }
    }

    /// The `index`-th parameter of this function (`LLVMGetParam`).
    ///
    /// The parameters exist as values as soon as the declaration does, which is
    /// what lets a definition's entry block store each one into its slot before
    /// any statement runs. The caller must not ask for an index past the last
    /// parameter: the C API reads whatever the function's type says is there.
    pub fn get_param(&self, index: usize) -> Value<'ctx> {
        Value {
            // SAFETY: the function is live and the index is in range by the
            // caller's contract (its parameter list was taken from the same
            // function type the declaration was made with).
            raw: unsafe { LLVMGetParam(self.raw, index as c_uint) },
            context: PhantomData,
        }
    }

    /// Attach `ext` to the `index`-th parameter of this function
    /// (`LLVMAddAttributeAtIndex`). The index is 1-based, as the C API and the
    /// textual IR both number parameters; a return attribute is
    /// [`Function::add_return_extension`].
    ///
    /// The attribute goes on the declaration, and that is where LLVM's call
    /// lowering reads it: a direct call to this callee carries the callee's
    /// parameter attributes without the call site restating them, so the
    /// declaration is the one place a C-boundary prototype has to agree with
    /// the C header (BUG-rK2mW).
    pub fn add_parameter_extension(&self, index: usize, ext: IntExtension) {
        assert!(index >= 1, "parameter attribute indices are 1-based");
        self.add_enum_attribute(index as LLVMAttributeIndex, ext.name());
    }

    /// Attach `noalias` to the `index`-th parameter
    /// (`LLVMAddAttributeAtIndex`).
    ///
    /// The promise is the callee's own: for the duration of the call, nothing
    /// the body reaches through this pointer is reached through anything but a
    /// pointer derived from it. A `mut` parameter is an exclusive borrow
    /// (`borrowck`), so every call site already guarantees it, and the analysis
    /// it buys is what lets LLVM reorder and vectorize accesses through the
    /// array parameters against each other instead of treating each load and
    /// store as possibly touching every array (the reason a kernel like
    /// `nbody`'s pair loop stayed scalar before this).
    pub fn add_parameter_noalias(&self, index: usize) {
        assert!(index >= 1, "parameter attribute indices are 1-based");
        self.add_enum_attribute(index as LLVMAttributeIndex, "noalias");
    }

    /// Attach `byval(ty)` to parameter `index` (1-based): the argument is
    /// passed by value as a copy of `ty`. An x86-64 `x86_intrcc` handler's
    /// first argument is the interrupt frame, which LLVM requires to be byval.
    pub fn add_parameter_byval(&self, index: usize, ty: Type<'_>) {
        assert!(index >= 1, "parameter attribute indices are 1-based");
        let name = "byval";
        // SAFETY: the name is a NUL-free static string and the length is its
        // own byte count; the kind is looked up in this context's table.
        let kind = unsafe {
            LLVMGetEnumAttributeKindForName(name.as_ptr() as *const c_char, name.len())
        };
        assert!(kind != 0, "this LLVM has no 'byval' attribute");
        // SAFETY: the kind is a type attribute kind, the type is live in this
        // context, and the created attribute is owned by the context.
        let attr = unsafe { LLVMCreateTypeAttribute(self.context, kind, ty.raw) };
        // SAFETY: the function is live and `index` is a parameter index of its
        // own signature.
        unsafe { LLVMAddAttributeAtIndex(self.raw, index as LLVMAttributeIndex, attr) };
    }

    /// Attach `ext` to this function's return value
    /// (`LLVMAddAttributeAtIndex` at `LLVMAttributeReturnIndex`), the other half
    /// of the sub-word C ABI: a body returning a narrow integer extends it
    /// before it returns, and a declaration that does not say so lets a caller
    /// read the register's upper bits as part of the value (BUG-rK2mW).
    pub fn add_return_extension(&self, ext: IntExtension) {
        self.add_enum_attribute(LLVMAttributeReturnIndex, ext.name());
    }

    /// Attach LLVM's `naked` attribute to this function (kernel stage 5): the
    /// backend emits no prologue and no epilogue, so the body's own asm is the
    /// whole function. The kind is looked up by name for the same reason every
    /// other enum attribute here is.
    pub fn mark_naked(&self) {
        self.add_enum_attribute(LLVMAttributeFunctionIndex, "naked");
    }

    /// Attach `noinline` to this function (kernel stage 5): a naked function
    /// must not be inlined into a caller it has no ABI for, and LLVM's `naked`
    /// implies it on most targets but not all.
    pub fn mark_noinline(&self) {
        self.add_enum_attribute(LLVMAttributeFunctionIndex, "noinline");
    }

    /// Attach the `interrupt` string attribute with `mode` (kernel stage 5):
    /// LLVM's riscv backend then saves every register the body touches and
    /// returns with `mret` (`"machine"`), instead of a plain `ret`.
    pub fn mark_interrupt(&self, mode: &str) {
        self.add_string_attribute(LLVMAttributeFunctionIndex, "interrupt", mode);
    }

    /// Set this function's calling convention (`LLVMSetFunctionCallConv`). An
    /// `interrupt fun` on x86-64 uses `x86_intrcc`, which saves and restores
    /// the registers and returns with `iretq`.
    pub fn set_calling_conv(&self, cc: llvm_sys::LLVMCallConv) {
        // SAFETY: the function is live; the calling convention is a plain
        // enum value LLVM stores on the function.
        unsafe { LLVMSetFunctionCallConv(self.raw, cc as u32) };
    }

    /// Attach `optnone` and `noinline` to this function (kernel stage 7): the
    /// freestanding `str` bodies are written as byte loops and must stay byte
    /// loops. LLVM's loop-idiom pass rewrites a NUL scan into a call to
    /// `strlen` (and a byte compare into `memcmp`), which is exactly the libc
    /// reference a freestanding image cannot resolve; the pass skips an
    /// `optnone` function, and the verifier requires the `noinline` that comes
    /// with it.
    pub fn mark_optnone(&self) {
        self.add_enum_attribute(LLVMAttributeFunctionIndex, "optnone");
        self.mark_noinline();
    }

    /// Attach `nobuiltin` to this function (kernel gap 4): LLVM must not treat a
    /// call to it as a library builtin, and, the half that matters here, its
    /// loop-idiom pass must not rewrite a loop *inside* it into a call to the
    /// library function of the same name. Without this, the body of the
    /// freestanding `memcpy` fallback is recognized as a `memcpy` and turned
    /// into a call to itself.
    pub fn mark_nobuiltin(&self) {
        self.add_enum_attribute(LLVMAttributeFunctionIndex, "nobuiltin");
    }

    /// Attach LLVM's `noredzone` to this function (kernel stage 20): an x86-64
    /// freestanding image may take an interrupt before it has an IDT, and the
    /// System V ABI's 128-byte red zone below `rsp` is clobbered by any
    /// interrupt handler, so the compiler must not keep live values there.
    pub fn mark_noredzone(&self) {
        self.add_enum_attribute(LLVMAttributeFunctionIndex, "noredzone");
    }

    /// Attach LLVM's `probe-stack="inline-asm"` to this function (kernel stage
    /// 14): a frame larger than one page is built by touching each page in
    /// order from the top, so a guard page below the stack catches the overflow
    /// instead of a single `sub sp` stepping over it. The value is a string
    /// attribute, not an enum kind, so it goes through the string half of the
    /// attribute API; `inline-asm` is the value that makes the probe a sequence
    /// of the target's own stores rather than a call to `__probestack`.
    pub fn mark_probe_stack(&self) {
        self.add_string_attribute(LLVMAttributeFunctionIndex, "probe-stack", "inline-asm");
    }

    /// Whether this function is a declaration with no body
    /// (`LLVMIsDeclaration`): a freestanding probe-stack sweep leaves an
    /// `extern fun`'s declaration alone because there is no prologue to probe.
    pub fn is_declaration(&self) -> bool {
        // SAFETY: the function handle is live and LLVM reads only its own
        // global-value kind.
        unsafe { LLVMIsDeclaration(self.raw) != 0 }
    }

    /// Attach a string attribute named `key` with value `value` at `index`
    /// (`LLVMCreateStringAttribute`), the string half of the attribute API.
    /// Unlike an enum attribute the value is part of the attribute, and LLVM
    /// copies both strings, so the `CString`s only have to outlive the call.
    fn add_string_attribute(&self, index: LLVMAttributeIndex, key: &str, value: &str) {
        let key = cstring(key);
        let value = cstring(value);
        // SAFETY: both strings are live for this call and LLVM copies them into
        // the attribute it creates; `index` is a valid function attribute index.
        let attr = unsafe {
            LLVMCreateStringAttribute(
                self.context,
                key.as_ptr(),
                key.as_bytes().len() as c_uint,
                value.as_ptr(),
                value.as_bytes().len() as c_uint,
            )
        };
        // SAFETY: the function is live and the attribute is owned by the
        // context both sides share.
        unsafe { LLVMAddAttributeAtIndex(self.raw, index, attr) };
    }

    /// Attach one enum attribute named `name` at `index`, looking the kind up
    /// by name because `llvm-sys` exports no constant for it (the C API's kind
    /// ids are deliberately unstable).
    fn add_enum_attribute(&self, index: LLVMAttributeIndex, name: &str) {
        // SAFETY: the name is a NUL-free static string and the length is its
        // own byte count; the kind is looked up in this context's table.
        let kind = unsafe {
            LLVMGetEnumAttributeKindForName(name.as_ptr() as *const c_char, name.len())
        };
        assert!(
            kind != 0,
            "this LLVM has no enum attribute named '{name}'"
        );
        // SAFETY: the kind is a real enum attribute kind in this context and
        // the value is unused for a flag attribute; the created attribute is
        // owned by the context and borrowed by the call below.
        let attr = unsafe { LLVMCreateEnumAttribute(self.context, kind, 0) };
        // SAFETY: the function is live and `index` is the return index or a
        // parameter index of its own signature.
        unsafe { LLVMAddAttributeAtIndex(self.raw, index, attr) };
    }

    /// Append an empty basic block named `name` (`LLVMAppendBasicBlockInContext`).
    ///
    /// The block is empty until the builder emits into it and has no terminator,
    /// so the module does not verify until [`Builder::build_ret`] or another
    /// terminator runs. The returned handle is owned by this function's module.
    pub fn append_block(&self, name: &str) -> BasicBlock<'ctx> {
        let name = cstring(name);
        BasicBlock {
            // SAFETY: the context owns both the function and the new block, and
            // the name is a valid NUL-terminated string.
            raw: unsafe { LLVMAppendBasicBlockInContext(self.context, self.raw, name.as_ptr()) },
            context: PhantomData,
        }
    }
}

/// A basic block handle, owned by the module of the [`Function`] it was appended to.
#[derive(Clone, Copy)]
pub struct BasicBlock<'ctx> {
    pub(crate) raw: LLVMBasicBlockRef,
    pub(crate) context: PhantomData<&'ctx Context>,
}

impl<'ctx> BasicBlock<'ctx> {
    /// The underlying `LLVMBasicBlockRef`, for a C API call this wrapper lacks.
    pub fn as_raw(self) -> LLVMBasicBlockRef {
        self.raw
    }
}
