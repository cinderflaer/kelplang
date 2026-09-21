//! The thin wrapper over LLVM's C API, and `llvm-sys`'s only client.
//!
//! MK7 §1.2 decides this shape: the raw bindings crate plus a small module of
//! our own, because the wrapper is the part we would have to understand anyway
//! and because it keeps the choice reversible. `codegen_llvm.rs` (next
//! milestone) calls this module and nothing else in the compiler calls LLVM, so
//! the wrapper can be swapped for `inkwell` later without the lowering moving.
//!
//! MK7 §1.1: everything under `llvm-sys` is an `unsafe` extern. This module is
//! where that `unsafe` lives, and it is deliberately thin: each method names one
//! C API call and does the smallest amount of work needed to make it hard to
//! call wrongly (a `CString` for the `char*` parameters, a `String` for an
//! LLVM-allocated message, the dispose that pairs with the create).
//!
//! # Ownership
//!
//! Four types are guards, whose `Drop` calls the C API's disposer:
//! [`Context`] (`LLVMContextCreate`/`LLVMContextDispose`), [`Module`]
//! (`LLVMModuleCreateWithNameInContext`/`LLVMDisposeModule`), [`Builder`]
//! (`LLVMCreateBuilderInContext`/`LLVMDisposeBuilder`) and [`TargetMachine`]
//! (`LLVMCreateTargetMachine`/`LLVMDisposeTargetMachine`).
//!
//! Every other handle is a `Copy` newtype over a bare LLVM pointer:
//! [`Type`], [`FnType`], [`Value`], [`Function`] and [`BasicBlock`]. LLVM owns
//! the object behind each one (a type is owned by its context, a function and a
//! basic block by their module), so the pointer must not outlive its owner. The
//! `'ctx` lifetime every one of them carries ties it to the [`Context`] that
//! created it, which is what stops a handle from outliving the guard. A handle
//! is also invalidated by nothing else: LLVM keeps a type or a value alive as
//! long as its context is alive, so a handle stays usable while the module it
//! was built in still exists.
//!
//! One caveat the C API has and the types cannot express: an `LLVMModuleRef` is
//! passed to [`TargetMachine::emit_object`], so a module must not be dropped
//! while codegen is running over it. LLVM does not free it; that is the usual
//! borrow rule of the call, and codegen is synchronous.

// `codegen_llvm.rs` is the one caller now, but the wrapper keeps the
// `as_raw` escape hatch and names more of the C API than the S1 lowering reaches
// yet, so a method can sit unused without being dead in the sense worth a
// warning. The allow goes away when the lowering stops growing.
#![allow(dead_code)]

use std::ffi::{c_char, c_uint, CStr, CString};
use std::marker::PhantomData;
use std::path::Path;
use std::ptr::null_mut;

use llvm_sys::analysis::{LLVMVerifierFailureAction, LLVMVerifyModule};
use llvm_sys::core::{
    LLVMAddFunction, LLVMAddGlobal, LLVMAddIncoming, LLVMAppendBasicBlockInContext,
    LLVMBuildAShr, LLVMBuildAdd, LLVMBuildAlloca, LLVMBuildAnd, LLVMBuildBr, LLVMBuildCall2,
    LLVMBuildCondBr, LLVMBuildFCmp, LLVMBuildFNeg, LLVMBuildFPExt, LLVMBuildFPToSI,
    LLVMBuildFPToUI, LLVMBuildFPTrunc, LLVMBuildGlobalString, LLVMBuildICmp,
    LLVMBuildInsertValue, LLVMBuildLShr, LLVMBuildLoad2, LLVMBuildMul, LLVMBuildOr, LLVMBuildPhi,
    LLVMBuildRet, LLVMBuildRetVoid, LLVMBuildSDiv, LLVMBuildSExt, LLVMBuildSIToFP, LLVMBuildSRem,
    LLVMBuildSelect, LLVMBuildShl, LLVMBuildStore, LLVMBuildStructGEP2,
    LLVMBuildSub, LLVMBuildTrunc, LLVMBuildUDiv, LLVMBuildUIToFP, LLVMBuildURem,
    LLVMBuildUnreachable, LLVMBuildXor, LLVMBuildZExt, LLVMConstInt, LLVMConstReal,
    LLVMContextCreate, LLVMContextDispose, LLVMCreateBuilderInContext, LLVMDisposeBuilder,
    LLVMDisposeMessage, LLVMDisposeModule, LLVMDoubleTypeInContext, LLVMFloatTypeInContext,
    LLVMFunctionType, LLVMGetFirstInstruction, LLVMGetNamedFunction, LLVMGetParam, LLVMGetTarget,
    LLVMGetUndef, LLVMInt16TypeInContext, LLVMInt1TypeInContext, LLVMInt32TypeInContext,
    LLVMInt64TypeInContext,
    LLVMInt8TypeInContext, LLVMModuleCreateWithNameInContext, LLVMPointerTypeInContext,
    LLVMPositionBuilderAtEnd, LLVMPositionBuilderBefore, LLVMPrintModuleToString, LLVMSetTarget,
    LLVMStructCreateNamed, LLVMStructSetBody, LLVMVoidTypeInContext,
};
use llvm_sys::{LLVMIntPredicate, LLVMRealPredicate};
use llvm_sys::prelude::{
    LLVMBasicBlockRef, LLVMBool, LLVMBuilderRef, LLVMContextRef, LLVMModuleRef, LLVMTypeRef,
    LLVMValueRef,
};
use llvm_sys::target::{LLVM_InitializeNativeAsmPrinter, LLVM_InitializeNativeTarget};
use llvm_sys::target_machine::{
    LLVMCodeGenFileType, LLVMCodeGenOptLevel, LLVMCodeModel, LLVMCreateTargetMachine,
    LLVMDisposeTargetMachine, LLVMGetDefaultTargetTriple, LLVMGetTargetFromTriple, LLVMRelocMode,
    LLVMTargetMachineEmitToFile, LLVMTargetMachineRef,
};

/// A `CString` for a name or a path on its way into the C API.
///
/// LLVM's `char*` parameters are NUL-terminated and Rust's `str` may contain an
/// interior NUL, which the C string would silently truncate. Every name this
/// module passes is a function name, a symbol name or a path, so an interior NUL
/// is a programming error, and this turns it into a panic that names the string.
fn cstring(s: &str) -> CString {
    CString::new(s).unwrap_or_else(|_| panic!("`{s}` contains a NUL byte and cannot cross into C"))
}

/// Copy a NUL-terminated string LLVM owns without freeing it.
///
/// The accessors that return `*const c_char` (`LLVMGetTarget`) hand back memory
/// owned by the LLVM object, so this must not free it, unlike [`take_message`].
///
/// # Safety
///
/// `ptr` must point at a NUL-terminated string that is valid now and for the
/// duration of the call. LLVM returns a null pointer for "no value" (an unset
/// module triple, for example), which this maps to the empty string.
unsafe fn copy_str(ptr: *const c_char) -> String {
    if ptr.is_null() {
        return String::new();
    }
    // SAFETY: the caller guarantees a valid NUL-terminated string. The copy is
    // made before the pointer can be invalidated; nothing here frees it.
    unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned()
}

/// Take ownership of an LLVM-allocated message, copy it, and free it.
///
/// `LLVMPrintModuleToString`, `LLVMGetDefaultTargetTriple` and the error
/// out-parameters of `LLVMVerifyModule`, `LLVMGetTargetFromTriple` and
/// `LLVMTargetMachineEmitToFile` all return a `char*` the caller must release
/// with `LLVMDisposeMessage`. This is that release, and the one place a message
/// leaves LLVM's ownership.
///
/// # Safety
///
/// `ptr` must be null or a pointer an LLVM function allocated with `malloc` for
/// the caller to free. It is freed here and must not be used afterwards.
unsafe fn take_message(ptr: *mut c_char) -> String {
    if ptr.is_null() {
        return String::new();
    }
    // SAFETY: LLVM's contract is a NUL-terminated malloc'd string owned by the
    // caller; the copy happens before LLVMDisposeMessage frees it.
    let text = unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned();
    // SAFETY: the same pointer, now released exactly once, as its contract says.
    unsafe { LLVMDisposeMessage(ptr) };
    text
}

/// Load the target whose codegen runs on the machine `kelpc` itself runs on.
///
/// `LLVM_InitializeNativeTarget` plus `LLVM_InitializeNativeAsmPrinter`, the two
/// wrappers `llvm-sys` ships for LLVM's `LLVMInitializeNativeTarget` macros. LLVM
/// keeps no handle to dispose, so this is a plain function rather than a guard.
/// It is idempotent and cheap, so a caller may run it once per compilation
/// without tracking whether it already happened.
///
/// Each call returns 1 on failure, which happens when the library has no backend
/// for the host, that is, when the host triple is not one of the compiled-in
/// targets. That is a hard error rather than a fallback: `kelpc` can only emit
/// code for the host it was built for (MK7 §1.3 keeps linking with `cc`).
pub fn initialize_native_target() -> Result<(), String> {
    // SAFETY: both wrappers read no arguments and only register target code. The
    // failure return is checked below rather than assumed.
    let target_failed = unsafe { LLVM_InitializeNativeTarget() };
    if target_failed != 0 {
        return Err("LLVM has no code generator for this host".to_string());
    }
    // SAFETY: as above; this one registers the asm printer the object writer uses.
    let printer_failed = unsafe { LLVM_InitializeNativeAsmPrinter() };
    if printer_failed != 0 {
        return Err("LLVM has no assembly printer for this host".to_string());
    }
    Ok(())
}

/// The host triple, from `LLVMGetDefaultTargetTriple`.
///
/// The string LLVM was built for, which is what a module's triple and a
/// [`TargetMachine`]'s triple are set from. LLVM allocates the result for the
/// caller, released here by [`take_message`].
pub fn default_target_triple() -> String {
    // SAFETY: the call takes no arguments and returns a fresh malloc'd string,
    // which take_message copies and frees.
    let ptr = unsafe { LLVMGetDefaultTargetTriple() };
    unsafe { take_message(ptr) }
}

/// Guard for `LLVMContextRef`: the owner of every type and value handle below.
///
/// LLVM's context is the unit of isolation: two modules in one context share
/// their types, and every handle belongs to the context that created it. One
/// context per compilation is the usual shape.
pub struct Context {
    raw: LLVMContextRef,
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
    /// makes the result agree with the C struct `codegen_c.rs` writes, natural
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

    /// Set a named aggregate's body: `fields`, in declaration order
    /// (`LLVMStructSetBody`).
    ///
    /// `Packed` is 0, so the fields get the target's natural alignment and
    /// padding, which is exactly the C layout the backend must match
    /// (MK7 §3.1). Calling this twice on one type is legal in the C API but
    /// replaces the body, so the lowering does it once per declaration.
    pub fn struct_set_body(&self, ty: Type<'_>, fields: &[Type<'_>]) {
        let mut raw: Vec<LLVMTypeRef> = fields.iter().map(|f| f.raw).collect();
        // SAFETY: the type is a named struct owned by this context, the field
        // vector is live for the call, and 0 is `packed = false`.
        unsafe {
            LLVMStructSetBody(ty.raw, raw.as_mut_ptr(), raw.len() as c_uint, 0)
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

/// A type handle, owned by the [`Context`] that created it.
#[derive(Clone, Copy)]
pub struct Type<'ctx> {
    raw: LLVMTypeRef,
    context: PhantomData<&'ctx Context>,
}

impl<'ctx> Type<'ctx> {
    /// The underlying `LLVMTypeRef`, for a C API call this wrapper lacks.
    pub fn as_raw(self) -> LLVMTypeRef {
        self.raw
    }
}

/// A function type handle: the `ret (args...)` of a [`Function`].
#[derive(Clone, Copy)]
pub struct FnType<'ctx> {
    raw: LLVMTypeRef,
    context: PhantomData<&'ctx Context>,
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
    raw: LLVMValueRef,
    context: PhantomData<&'ctx Context>,
}

impl<'ctx> Value<'ctx> {
    /// The underlying `LLVMValueRef`, for a C API call this wrapper lacks.
    pub fn as_raw(self) -> LLVMValueRef {
        self.raw
    }
}

/// A function: a declaration or a definition in a [`Module`].
#[derive(Clone, Copy)]
pub struct Function<'ctx> {
    raw: LLVMValueRef,
    context: LLVMContextRef,
    context_marker: PhantomData<&'ctx Context>,
}

impl<'ctx> Function<'ctx> {
    /// The underlying `LLVMValueRef` (a function is a value in LLVM).
    pub fn as_raw(self) -> LLVMValueRef {
        self.raw
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
    raw: LLVMBasicBlockRef,
    context: PhantomData<&'ctx Context>,
}

impl<'ctx> BasicBlock<'ctx> {
    /// The underlying `LLVMBasicBlockRef`, for a C API call this wrapper lacks.
    pub fn as_raw(self) -> LLVMBasicBlockRef {
        self.raw
    }
}

/// Guard for `LLVMModuleRef`: one translation unit, and the owner of its
/// functions and basic blocks.
pub struct Module<'ctx> {
    raw: LLVMModuleRef,
    // Kept so that a function's declaration can name its context without
    // threading `&'ctx Context` through every method. LLVM has no
    // LLVMGetModuleContext, and the pointer is only valid because the lifetime
    // marker below ties this module to the guard that owns it.
    context: LLVMContextRef,
    context_marker: PhantomData<&'ctx Context>,
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

    /// Declare a function named `name` of type `ty` in this module
    /// (`LLVMAddFunction`).
    ///
    /// A name that already exists returns that existing function rather than a
    /// second declaration, which makes this safe to call again for a runtime
    /// symbol the module already mentions. The result is a declaration until a
    /// basic block with a terminator is appended to it.
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
        let name = cstring(name);
        Value {
            // SAFETY: the module is live and the name is valid.
            raw: unsafe { LLVMAddGlobal(self.raw, ty.as_raw(), name.as_ptr()) },
            context: PhantomData,
        }
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

/// Which integer binary operator to emit, named the way the language reads it
/// rather than the way LLVM opcodes are numbered. A `Copy` stand-in for the C
/// API's enum, so `codegen_llvm.rs` never reaches into `llvm-sys`.
///
/// `sdiv`/`udiv`, `srem`/`urem` and `ashr`/`lshr` are separate variants, not a
/// signedness parameter: the signedness the language sees lives in the type and
/// the lowerer picks the instruction. No variant carries `nsw` or `nuw`, because
/// the language promises wrapping (MK7 §3.4) and a flagged form promises the
/// opposite.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IntBinOp {
    Add,
    Sub,
    Mul,
    SDiv,
    UDiv,
    SRem,
    URem,
    And,
    Or,
    Xor,
    Shl,
    LShr,
    AShr,
}

/// Which floating-point binary operator to emit. Kept apart from [`IntBinOp`]
/// because LLVM keeps them apart: `fadd` and `add` are different instructions and
/// neither is a signedness choice of the other.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FloatBinOp {
    Add,
    Sub,
    Mul,
    Div,
}

/// An integer comparison. The signed and unsigned spellings are separate because
/// LLVM makes them a different instruction (`icmp slt` vs `icmp ult`), while the
/// language decides by the operand's type.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IntCmp {
    Eq,
    Ne,
    Slt,
    Sle,
    Sgt,
    Sge,
    Ult,
    Ule,
    Ugt,
    Uge,
}

/// A floating-point comparison, the ordered forms (`o` in LLVM's spelling).
///
/// `One` is `!=` as the language means it: true when the operands are unordered,
/// so a NaN compares not-equal to everything, itself included. The other five
/// are all false on a NaN, which is what an ordered predicate gives.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RealCmp {
    Oeq,
    One,
    Olt,
    Ole,
    Ogt,
    Oge,
}

/// Guard for `LLVMBuilderRef`: where the next instruction is emitted.
pub struct Builder<'ctx> {
    raw: LLVMBuilderRef,
    context: PhantomData<&'ctx Context>,
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

/// Guard for `LLVMTargetMachineRef`: the code generator for one triple.
///
/// This is what turns a verified module into an object, and it is why MK7 §1.3
/// keeps `cc` for linking: LLVM writes the object, the driver links it.
pub struct TargetMachine {
    raw: LLVMTargetMachineRef,
}

impl TargetMachine {
    /// A target machine for the host triple, at
    /// [`LLVMCodeGenOptLevel::LLVMCodeGenLevelDefault`].
    ///
    /// The shortcut for the one triple `kelpc` currently emits for. See
    /// [`for_triple`] for the parameters and the errors.
    ///
    /// [`for_triple`]: TargetMachine::for_triple
    pub fn for_host() -> Result<Self, String> {
        let triple = default_target_triple();
        TargetMachine::for_triple(&triple, LLVMCodeGenOptLevel::LLVMCodeGenLevelDefault)
    }

    /// A target machine for `triple` (`LLVMGetTargetFromTriple` and
    /// `LLVMCreateTargetMachine`), disposed on drop.
    ///
    /// `initialize_native_target` must have run first for a triple the host's
    /// LLVM build covers. CPU and features are left to the target's defaults and
    /// the code model to `LLVMCodeModelDefault`. Relocations are
    /// `LLVMRelocPIC`, not `LLVMRelocDefault`: LLVM's default for x86-64 Linux is
    /// a static reloc model, whose object carries absolute relocations, and the
    /// host `cc` builds PIE executables by default, so that object would fail to
    /// link with `relocation R_X86_64_32 can not be used when making a PIE
    /// object`. PIC, which is also what clang defaults to for this triple,
    /// produces the same kind of object the C backend's `cc` path does.
    ///
    /// `Err` carries LLVM's message when the triple names no compiled-in target,
    /// which is the one failure mode.
    pub fn for_triple(triple: &str, level: LLVMCodeGenOptLevel) -> Result<Self, String> {
        let triple = cstring(triple);
        let mut target = null_mut();
        let mut message: *mut c_char = null_mut();
        // SAFETY: the out-parameters point at our slots, and the target has no
        // dispose of its own (LLVM owns the registration).
        let status =
            unsafe { LLVMGetTargetFromTriple(triple.as_ptr(), &mut target, &mut message) };
        if status != 0 {
            let text = unsafe { take_message(message) };
            return Err(if text.is_empty() {
                format!("LLVM has no target for the triple '{}'", triple.to_string_lossy())
            } else {
                text
            });
        }
        let empty = cstring("");
        // SAFETY: the target came from the call above and is registered; the
        // empty CPU and features mean the defaults, and LLVM copies the strings.
        let raw = unsafe {
            LLVMCreateTargetMachine(
                target,
                triple.as_ptr(),
                empty.as_ptr(),
                empty.as_ptr(),
                level,
                LLVMRelocMode::LLVMRelocPIC,
                LLVMCodeModel::LLVMCodeModelDefault,
            )
        };
        if raw.is_null() {
            return Err(format!(
                "LLVM could not create a target machine for the triple '{}'",
                triple.to_string_lossy()
            ));
        }
        Ok(TargetMachine { raw })
    }

    /// The underlying `LLVMTargetMachineRef`, for a C API call this wrapper lacks.
    pub fn as_raw(&self) -> LLVMTargetMachineRef {
        self.raw
    }

    /// Write an object file for `module` to `path`
    /// (`LLVMTargetMachineEmitToFile` with `LLVMObjectFile`).
    ///
    /// The module should have a triple and should have verified: codegen assumes
    /// both. The write replaces `path`, and the file that results is what the
    /// driver hands to `cc`. `Err` carries LLVM's message when codegen fails.
    pub fn emit_object(&self, module: &Module<'_>, path: &Path) -> Result<(), String> {
        let path = cstring(&path.to_string_lossy());
        let mut message: *mut c_char = null_mut();
        // SAFETY: the machine and module are live, the path is valid, and the
        // out-parameter points at our slot.
        let status = unsafe {
            LLVMTargetMachineEmitToFile(
                self.raw,
                module.as_raw(),
                path.as_ptr(),
                LLVMCodeGenFileType::LLVMObjectFile,
                &mut message,
            )
        };
        if status == 0 {
            return Ok(());
        }
        let text = unsafe { take_message(message) };
        Err(if text.is_empty() {
            format!(
                "LLVM could not write an object to '{}'",
                path.to_string_lossy()
            )
        } else {
            text
        })
    }
}

impl Drop for TargetMachine {
    fn drop(&mut self) {
        // SAFETY: raw came from LLVMCreateTargetMachine and is disposed once.
        unsafe { LLVMDisposeTargetMachine(self.raw) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// A directory under the system temp directory, removed on drop.
    ///
    /// `CARGO_TARGET_TMPDIR` is only set for integration tests, so a unit test
    /// picks its own directory. The name carries the process id and the
    /// nanosecond clock, which is what keeps two test processes from sharing one
    /// directory; nothing else in this suite uses the system temp directory.
    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(label: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("the clock is after 1970")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "kelpc-llvm-ffi-{label}-{}-{nanos}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("could not create the test's temp directory");
            TempDir { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    /// The whole pipeline this milestone claims, in one test: build `i32 @main()`
    /// in IR (it calls `i32 @puts(i8*)` on a global string and returns 0),
    /// verify, print the IR, emit an object for the host triple, link with `cc`,
    /// and run the binary.
    ///
    /// The proof is the program's stdout: `hello, kelp\n` and exit status 0. A
    /// failure in any step fails loudly at that step, so the message says which
    /// half of the pipeline broke.
    #[test]
    fn hello_world_ir_emits_an_object_that_cc_links_and_that_prints() {
        initialize_native_target().expect("the host has an LLVM target but no code generator");
        let triple = default_target_triple();
        assert!(
            !triple.is_empty(),
            "LLVMGetDefaultTargetTriple returned an empty triple"
        );

        let out = TempDir::new("hello-world");
        let object = out.path().join("hello.o");
        let executable = out.path().join("hello");

        let ctx = Context::new();
        let mut module = Module::new(&ctx, "hello");
        module.set_triple(&triple);
        assert_eq!(
            module.triple(),
            triple,
            "the module's triple is not the one LLVMGetDefaultTargetTriple returned"
        );

        // i32 @puts(i8*) is a declaration: no body, so it is the linker that
        // resolves it from libc.
        let i32_ty = ctx.i32();
        let puts_ty = ctx.fn_type(i32_ty, &[ctx.opaque_ptr()], false);
        let puts = module.add_function("puts", puts_ty);
        assert!(
            module.get_function("puts").is_some(),
            "LLVMGetNamedFunction could not find the declaration LLVMAddFunction made"
        );

        // define i32 @main() { entry: %greeting = ...; call i32 @puts(ptr %greeting); ret i32 0 }
        let main = module.add_function("main", ctx.fn_type(i32_ty, &[], false));
        let builder = Builder::new(&ctx);
        builder.position_at_end(main.append_block("entry"));
        // The global carries the text without a newline because `puts` writes one
        // of its own: with a trailing `\n` in the string the program would print
        // `hello, kelp` followed by two newlines, and stdout would not be the
        // `hello, kelp\n` this test asserts.
        let greeting = builder.global_string("hello, kelp", "greeting");
        builder.build_call(puts, puts_ty, &[greeting], "");
        builder.build_ret(ctx.const_int(i32_ty, 0, false));

        if let Err(message) = module.verify() {
            panic!("LLVMVerifyModule rejected the module:\n{message}");
        }

        let ir = module.to_ir_string();
        println!("--- LLVM IR ---\n{ir}--- end LLVM IR ---");
        assert!(
            ir.contains("define i32 @main()"),
            "the IR has no i32 @main() definition:\n{ir}"
        );
        assert!(
            ir.contains(&format!("target triple = \"{triple}\"")),
            "the IR does not carry the host triple '{triple}':\n{ir}"
        );

        let machine = TargetMachine::for_host()
            .unwrap_or_else(|e| panic!("no target machine for the host triple '{triple}': {e}"));
        machine.emit_object(&module, &object).unwrap_or_else(|e| {
            panic!("could not emit an object to '{}': {e}", object.display())
        });
        assert!(object.is_file(), "emit_object wrote no object file");

        // Exactly how driver.rs::compile_c finishes today: hand `cc` the file.
        let link = Command::new("cc")
            .arg(&object)
            .arg("-o")
            .arg(&executable)
            .output()
            .expect("could not invoke 'cc' to link the object");
        assert!(
            link.status.success(),
            "cc failed linking '{}':\n{}",
            object.display(),
            String::from_utf8_lossy(&link.stderr)
        );

        let run = Command::new(&executable)
            .output()
            .expect("could not run the linked binary");
        assert!(
            run.status.success(),
            "the linked binary exited with {:?}\nstderr:\n{}",
            run.status,
            String::from_utf8_lossy(&run.stderr)
        );
        assert_eq!(
            run.stdout,
            b"hello, kelp\n",
            "stdout is not byte-identical to the literal: {:?}",
            run.stdout
        );
    }
}
