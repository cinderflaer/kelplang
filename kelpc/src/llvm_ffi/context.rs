use super::*;

/// Guard for `LLVMContextRef`: the owner of every type and value handle below.
///
/// LLVM's context is the unit of isolation: two modules in one context share
/// their types, and every handle belongs to the context that created it. One
/// context per compilation is the usual shape.
pub struct Context {
    pub(crate) raw: LLVMContextRef,
}

impl Context {
    /// A new, empty context (`LLVMContextCreate`), disposed on drop.
    pub fn new() -> Self {
        Context {
            // SAFETY: LLVMContextCreate takes no arguments and returns an owned,
            // non-null context.
            raw: unsafe { LLVMContextCreate() },
        }
    }

    /// The underlying `LLVMContextRef`, for the C API calls this wrapper does
    /// not cover yet.
    ///
    /// Handing this out is the escape hatch MK7 §1.2 anticipates: the wrapper
    /// stays small by letting a call site drop to the raw binding instead of
    /// growing a method for every C API call. The pointer is only valid while
    /// this guard is alive.
    pub fn as_raw(&self) -> LLVMContextRef {
        self.raw
    }

    /// Mark `br`, the backedge of one loop, with LLVM's `llvm.loop.unroll.full`
    /// loop option (`LLVMSetMetadata` under the `llvm.loop` kind).
    ///
    /// The option tells the unroller to unroll the loop however large the body
    /// is. That is what a small constant-bound loop needs: LLVM's own cost model
    /// declines a body the size of a kernel's inner step, and the SLP vectorizer
    /// packs across bodies only when they are already straight line, so a loop
    /// the model leaves alone never becomes the shape the packing needs. Only a
    /// loop whose bound is a small literal is marked
    /// (`codegen_llvm::forced_unroll`), so the option never reaches a loop whose
    /// trip count is unbounded or large.
    ///
    /// One `llvm.loop` node names the loop: its first operand is the node
    /// itself and the options follow it. LLVM requires that self reference, and
    /// only a distinct node can carry it, so the node is built around a
    /// temporary placeholder that is then replaced by the node it sits in
    /// (`LLVMTemporaryMDNode` and `LLVMMetadataReplaceAllUsesWith`, the pattern
    /// the C API's own loop metadata is built with).
    pub fn mark_loop_unroll_full(&self, br: Value<'_>) {
        const KEY: &str = "llvm.loop";
        const OPTION: &str = "llvm.loop.unroll.full";
        // SAFETY: the context is live, the strings outlive the calls, and every
        // node is built in this context. A string and a node survive as long as
        // metadata does, which is the context's own lifetime, so the handles
        // below outlive the loop they mark.
        unsafe {
            let option =
                LLVMMDStringInContext2(self.raw, OPTION.as_ptr() as *const c_char, OPTION.len());
            let mut property_operands = [option];
            let property = LLVMMDNodeInContext2(
                self.raw,
                property_operands.as_mut_ptr(),
                property_operands.len(),
            );
            // The placeholder stands in for the node's first operand, which
            // has to be the node itself; replacing every use of it with the
            // node completes the cycle. The C API can only build a temporary
            // (forward-declaring) node directly, and a temporary node is
            // rejected by the verifier, so the node with the cycle is the
            // ordinary one: RAUW-ing the temporary operand makes LLVM resolve
            // it, and a self-referential node resolves to a distinct one.
            let placeholder = LLVMTemporaryMDNode(self.raw, null_mut(), 0);
            let mut members = [placeholder, property];
            let id = LLVMMDNodeInContext2(self.raw, members.as_mut_ptr(), members.len());
            LLVMMetadataReplaceAllUsesWith(placeholder, id);
            let kind = LLVMGetMDKindIDInContext(
                self.raw,
                KEY.as_ptr() as *const c_char,
                KEY.len() as c_uint,
            );
            // SAFETY: `br` is a branch instruction of a module in this context
            // and `id` is a metadata node of the same context, which is what
            // attaching it requires.
            LLVMSetMetadata(br.as_raw(), kind, LLVMMetadataAsValue(self.raw, id));
        }
    }

    /// `i1`, LLVM's boolean: also the type of a comparison result.
    pub fn i1(&self) -> Type<'_> {
        self.integer(LLVMInt1TypeInContext)
    }

    /// `i8`, LLVM's single byte.
    pub fn i8(&self) -> Type<'_> {
        self.integer(LLVMInt8TypeInContext)
    }

    /// `i16`.
    pub fn i16(&self) -> Type<'_> {
        self.integer(LLVMInt16TypeInContext)
    }

    /// `i32`, the width of C `int` and so of most of the runtime's `int` returns.
    pub fn i32(&self) -> Type<'_> {
        self.integer(LLVMInt32TypeInContext)
    }

    /// `i64`, and the width `usize` uses on the host.
    pub fn i64(&self) -> Type<'_> {
        self.integer(LLVMInt64TypeInContext)
    }

    /// `void`, the return type of a unit-returning function.
    ///
    /// A function that returns `void` still needs a function *type* whose return
    /// type is this one, which is why it is a value in its own right.
    pub fn void(&self) -> Type<'_> {
        Type {
            // SAFETY: the call takes the live context and returns a type owned
            // by it.
            raw: unsafe { LLVMVoidTypeInContext(self.raw) },
            context: PhantomData,
        }
    }

    /// `float`, the width of Kelp's `f32`.
    pub fn f32(&self) -> Type<'_> {
        Type {
            // SAFETY: the call takes the live context and returns a type owned
            // by it.
            raw: unsafe { LLVMFloatTypeInContext(self.raw) },
            context: PhantomData,
        }
    }

    /// `double`, the width of Kelp's `f64`.
    pub fn f64(&self) -> Type<'_> {
        Type {
            // SAFETY: as in f32.
            raw: unsafe { LLVMDoubleTypeInContext(self.raw) },
            context: PhantomData,
        }
    }

    /// The opaque pointer type, `ptr` in textual IR.
    ///
    /// LLVM 21 has typed pointers only in name: `LLVMPointerTypeInContext` with
    /// an address space of 0 is the single pointer type, and what it points at
    /// is carried by the value, not by the type unlike the pre-15 API. This is
    /// the type for `i8*` parameters and for the returns of the runtime's
    /// allocation functions.
    pub fn opaque_ptr(&self) -> Type<'_> {
        Type {
            // SAFETY: address space 0 is the default and exists in every context.
            raw: unsafe { LLVMPointerTypeInContext(self.raw, 0) },
            context: PhantomData,
        }
    }

    /// An integer type of `bits` width (`LLVMIntTypeInContext`).
    ///
    /// The named widths have their own methods; this is for the rest. LLVM
    /// itself accepts widths far above these, so the bound is Kelp's: its
    /// integer types stop at `i128` and a wider intermediate type would be a
    /// lowering mistake, not a language feature.
    pub fn int(&self, bits: c_uint) -> Type<'_> {
        assert!(
            (1..=128).contains(&bits),
            "{bits} is not one of Kelp's integer widths"
        );
        Type {
            // SAFETY: the width is in range and the context is live.
            raw: unsafe { llvm_sys::core::LLVMIntTypeInContext(self.raw, bits) },
            context: PhantomData,
        }
    }

    /// A named aggregate type, `%name` in textual IR (`LLVMStructCreateNamed`).
    ///
    /// The type starts *opaque* (no body), which is what lets a `datac` be
    /// named before its fields' own types exist: `struct_set_body` fills it in
    /// once every field type does. LLVM's layout, not this wrapper's, is what
    /// makes the result agree with the C struct the removed C backend wrote, natural
    /// alignment included.
    pub fn struct_type(&self, name: &str) -> Type<'_> {
        let name = cstring(name);
        Type {
            // SAFETY: the context is live and the name is a valid C string; the
            // type is owned by the context.
            raw: unsafe { LLVMStructCreateNamed(self.raw, name.as_ptr()) },
            context: PhantomData,
        }
    }

    /// The array type `[count x elem]` (`LLVMArrayType2`).
    ///
    /// The tagged-union layout is the one caller: a union has no LLVM type of
    /// its own, so it is emitted as a struct anchored on the widest payload's
    /// alignment type plus a byte array to the union's size (see
    /// `codegen_llvm.rs::enum_union_body`).
    pub fn array_type(&self, elem: Type<'_>, count: u64) -> Type<'_> {
        Type {
            // SAFETY: the element type is live and owned by this context; the
            // array type is owned by the context too.
            raw: unsafe { LLVMArrayType2(elem.as_raw(), count) },
            context: PhantomData,
        }
    }

    /// Set a named aggregate's body: `fields`, in declaration order
    /// (`LLVMStructSetBody`).
    ///
    /// `Packed` is 0, so the fields get the target's natural alignment and
    /// padding, which is exactly the C layout the backend must match
    /// (MK7 §3.1). Calling this twice on one type is legal in the C API but
    /// replaces the body, so the lowering does it once per declaration.
    pub fn struct_set_body(&self, ty: Type<'_>, fields: &[Type<'_>]) {
        self.struct_set_body_packed(ty, fields, false);
    }

    /// Set a named aggregate's body with the `packed` flag: `packed = 1` gives
    /// every field alignment 1 and inserts no padding, which is exactly the C
    /// `__attribute__((packed))` layout `repr(packed)` asks for (kernel stage
    /// 2). The two backends must agree byte for byte, so the flag is threaded
    /// to the one C API call that decides it.
    pub fn struct_set_body_packed(&self, ty: Type<'_>, fields: &[Type<'_>], packed: bool) {
        let mut raw: Vec<LLVMTypeRef> = fields.iter().map(|f| f.raw).collect();
        // SAFETY: the type is a named struct owned by this context, the field
        // vector is live for the call, and `packed` is a valid LLVMBool.
        unsafe {
            LLVMStructSetBody(
                ty.raw,
                raw.as_mut_ptr(),
                raw.len() as c_uint,
                packed as LLVMBool,
            )
        }
    }

    /// An anonymous aggregate type of `fields` in order
    /// (`LLVMStructTypeInContext`), the shape a multi-output `asm` call returns:
    /// one member per output operand, in constraint order.
    pub fn literal_struct(&self, fields: &[Type<'_>]) -> Type<'_> {
        let mut raw: Vec<LLVMTypeRef> = fields.iter().map(|f| f.raw).collect();
        Type {
            // SAFETY: the field types are live in this context, the vector is
            // read only for the call, and this struct is unpacked exactly as
            // the inline-asm return shape LLVM expects.
            raw: unsafe {
                LLVMStructTypeInContext(self.raw, raw.as_mut_ptr(), raw.len() as c_uint, 0)
            },
            context: PhantomData,
        }
    }

    /// The vector type `<count x elem>` (`LLVMVectorType`). The one caller is
    /// the alignment anchor a `repr(align(N))` struct body carries: for an `N`
    /// wider than an `i128`, a vector of `N` bytes is the only LLVM type whose
    /// ABI alignment is `N`.
    pub fn vector_type(&self, elem: Type<'_>, count: u32) -> Type<'_> {
        Type {
            // SAFETY: the element type is live and owned by this context.
            raw: unsafe { LLVMVectorType(elem.as_raw(), count) },
            context: PhantomData,
        }
    }

    /// An inline-assembly value (`LLVMGetInlineAsm`): the callee of a call
    /// instruction that is one target instruction rather than a function.
    ///
    /// `ty` is the generated function type (each output operand is a member of
    /// the return, each input operand is a parameter, in constraint order),
    /// `constraints` is LLVM's constraint string, `sideeffect` is the `volatile`
    /// flag (Kelp always sets it: it has no pure asm), and `intel` selects the
    /// Intel dialect instead of the target's default, which is how one Kelp
    /// template reads as destination-first on x86 and on riscv64 alike
    /// (`kelp wiki page language/unsafe.md`).
    ///
    /// The template is passed with an explicit length, so a NUL byte inside it
    /// is not a C-string terminator; the same text reaches GCC as a C literal
    /// on the other backend.
    pub fn inline_asm<'ctx>(
        &self,
        ty: FnType<'ctx>,
        template: &str,
        constraints: &str,
        sideeffect: bool,
        intel: bool,
    ) -> Value<'ctx> {
        let dialect = if intel {
            LLVMInlineAsmDialect::LLVMInlineAsmDialectIntel
        } else {
            LLVMInlineAsmDialect::LLVMInlineAsmDialectATT
        };
        Value {
            // SAFETY: the function type is live in this context and both
            // strings are read only for the call, with the lengths given.
            raw: unsafe {
                LLVMGetInlineAsm(
                    ty.as_raw(),
                    template.as_ptr() as *const c_char,
                    template.len(),
                    constraints.as_ptr() as *const c_char,
                    constraints.len(),
                    sideeffect as LLVMBool,
                    0,
                    dialect,
                    0,
                )
            },
            context: PhantomData,
        }
    }

    /// The undefined value of `ty` (`LLVMGetUndef`).
    ///
    /// The starting aggregate for a chain of `insertvalue`: each field is
    /// written before the value is read, so the undef parts are never observed.
    pub fn undef(&self, ty: Type<'_>) -> Value<'_> {
        Value {
            // SAFETY: the type is live, and undef is a constant owned by the
            // context.
            raw: unsafe { LLVMGetUndef(ty.raw) },
            context: PhantomData,
        }
    }

    /// The all-zero constant of `ty` (`LLVMConstNull`): a null pointer, a zero
    /// integer, or an aggregate's `zeroinitializer`.
    ///
    /// A move clears its source slot with this, so the drop a binding still owes
    /// on a path that did not move it frees NULL rather than a second copy of
    /// the same buffer (MK7 §3.4; the C backend writes `({0})` for the same
    /// reason).
    pub fn const_null(&self, ty: Type<'_>) -> Value<'_> {
        Value {
            // SAFETY: the type is live, and the constant is owned by the context.
            raw: unsafe { LLVMConstNull(ty.raw) },
            context: PhantomData,
        }
    }

    /// The function type `ret (params...)`, variadic or not (`LLVMFunctionType`).
    ///
    /// A function *type*, not a function: [`Module::add_function`] pairs one with
    /// a name to declare something callable. The parameter slice is copied into
    /// LLVM, so the caller keeps its own array.
    pub fn fn_type(&self, ret: Type<'_>, params: &[Type<'_>], var_arg: bool) -> FnType<'_> {
        let mut params: Vec<LLVMTypeRef> = params.iter().map(|p| p.raw).collect();
        FnType {
            // SAFETY: the pointer and count describe the same live vector, and
            // LLVM reads them only for the duration of the call.
            raw: unsafe {
                LLVMFunctionType(
                    ret.raw,
                    params.as_mut_ptr(),
                    params.len() as c_uint,
                    var_arg as LLVMBool,
                )
            },
            context: PhantomData,
        }
    }

    /// The integer constant `value` of type `ty` (`LLVMConstInt`).
    ///
    /// `sign_extend` says how to read a `value` that does not fit `ty`: false
    /// truncates through zero-extension in the unsigned sense, true through sign.
    /// It is a parameter rather than a policy because the C API asks for it.
    pub fn const_int(&self, ty: Type<'_>, value: u64, sign_extend: bool) -> Value<'_> {
        Value {
            // SAFETY: both arguments are values LLVM copies into a constant.
            raw: unsafe { LLVMConstInt(ty.raw, value, sign_extend as LLVMBool) },
            context: PhantomData,
        }
    }

    /// The `i1` constant `true` or `false`.
    ///
    /// There is no separate boolean constant in the C API: `LLVMConstInt` on
    /// `i1`, which is what this wraps so the lowering does not have to spell it.
    pub fn const_bool(&self, value: bool) -> Value<'_> {
        self.const_int(self.i1(), u64::from(value), false)
    }

    /// The bytes followed by one NUL, as an `[n+1 x i8]` constant
    /// (`LLVMConstStringInContext` with the terminator). A global takes it as
    /// its initializer, so the text needs no builder position.
    pub fn const_c_string(&self, bytes: &[u8]) -> Value<'_> {
        Value {
            // SAFETY: the context is live, and the pointer and length describe
            // `bytes` for the call, which LLVM copies into the constant.
            raw: unsafe {
                LLVMConstStringInContext2(
                    self.raw,
                    bytes.as_ptr().cast(),
                    bytes.len(),
                    0,
                )
            },
            context: PhantomData,
        }
    }

    /// The constant array `[elem; values.len()]` (`LLVMConstArray2`), the
    /// initializer a `section("...")` const's global is given (kernel stage
    /// 4). The returned value's type is the array type, so a caller asks for
    /// it with [`Context::array_type`] and passes both to
    /// [`Module::add_global`] and [`Value::set_initializer`].
    ///
    /// [`Context::array_type`]: Context::array_type
    /// [`Module::add_global`]: Module::add_global
    /// [`Value::set_initializer`]: Value::set_initializer
    pub fn const_array(&self, elem: Type<'_>, values: &[Value<'_>]) -> Value<'_> {
        let mut raws: Vec<LLVMValueRef> = values.iter().map(|v| v.raw).collect();
        Value {
            // SAFETY: the element type and every value are live in this
            // context; LLVM copies the array.
            raw: unsafe { LLVMConstArray2(elem.raw, raws.as_mut_ptr(), raws.len() as u64) },
            context: PhantomData,
        }
    }

    /// The constant of the *named* struct type `ty` with the given element
    /// values (`LLVMConstNamedStruct`), which is what a `datac` or `Atomic<T>`
    /// static's initializer is. `LLVMConstStruct` would build a literal struct
    /// type instead, and a global's initializer has to match its own named
    /// type, so the named form is the one that works here.
    pub fn const_named_struct(&self, ty: Type<'_>, values: &[Value<'_>]) -> Value<'_> {
        let mut raws: Vec<LLVMValueRef> = values.iter().map(|v| v.raw).collect();
        Value {
            // SAFETY: the type and every value are live in this context; LLVM
            // copies the element list.
            raw: unsafe { LLVMConstNamedStruct(ty.raw, raws.as_mut_ptr(), raws.len() as c_uint) },
            context: PhantomData,
        }
    }

    /// The floating-point constant `value` of type `ty` (`LLVMConstReal`).
    ///
    /// The C API takes the value as a C `double` and converts to `ty`'s own
    /// format, so an `f32` constant carries the rounding a `f32` literal has,
    /// and an infinity or a NaN literal is just the corresponding `double`
    /// value: LLVM's `APFloat` conversion has a spelling for both, which
    /// LLVM's textual printer then writes as a hexadecimal form (`0x7FF0...`)
    /// because no decimal literal denotes one.
    pub fn const_real(&self, ty: Type<'_>, value: f64) -> Value<'_> {
        Value {
            // SAFETY: both arguments are values LLVM copies into a constant.
            raw: unsafe { LLVMConstReal(ty.raw, value) },
            context: PhantomData,
        }
    }

    /// A named integer type, through one of the width-specific C calls.
    fn integer(
        &self,
        ctor: unsafe extern "C" fn(LLVMContextRef) -> LLVMTypeRef,
    ) -> Type<'_> {
        Type {
            // SAFETY: the constructor takes the live context and returns a type
            // owned by it.
            raw: unsafe { ctor(self.raw) },
            context: PhantomData,
        }
    }
}

impl Default for Context {
    fn default() -> Self {
        Context::new()
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        // SAFETY: raw came from LLVMContextCreate and is disposed once, here. Any
        // handle into this context is already unusable, because every one of them
        // borrows this value for 'ctx.
        unsafe { LLVMContextDispose(self.raw) };
    }
}
