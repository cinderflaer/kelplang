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
//! (`LLVMCreateTargetMachine`/`LLVMDisposeTargetMachine`). [`DataLayout`] is a
//! fifth, added for the tagged-union layout (MK7 S2C), and [`DebugBuilder`] a
//! sixth, added with the DWARF debug metadata (MK7 S4).
//!
//! Every other handle is a `Copy` newtype over a bare LLVM pointer:
//! [`Type`], [`FnType`], [`Value`], [`Function`], [`BasicBlock`] and
//! [`Metadata`]. LLVM owns
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

use std::cell::Cell;
use std::ffi::{c_char, c_uint, CStr, CString};
use std::marker::PhantomData;
use std::path::Path;
use std::ptr::null_mut;

use llvm_sys::analysis::{LLVMVerifierFailureAction, LLVMVerifyModule};
use llvm_sys::core::{
    LLVMAddAttributeAtIndex, LLVMAddCase, LLVMAddFunction, LLVMAddGlobal, LLVMAddIncoming,
    LLVMAddModuleFlag,
    LLVMAddNamedMetadataOperand, LLVMAppendBasicBlockInContext,
    LLVMArrayType2,
    LLVMBuildAShr, LLVMBuildAdd, LLVMBuildAlloca, LLVMBuildAnd, LLVMBuildAtomicCmpXchg,
    LLVMBuildAtomicRMW, LLVMBuildBr, LLVMBuildCall2,
    LLVMBuildCondBr, LLVMBuildExtractValue, LLVMBuildFCmp, LLVMBuildFNeg, LLVMBuildFPExt,
    LLVMBuildFPToSI,
    LLVMBuildFPToUI, LLVMBuildFPTrunc, LLVMBuildFence, LLVMBuildGEP2, LLVMBuildGlobalString,
    LLVMBuildICmp,
    LLVMBuildInsertValue, LLVMBuildIntToPtr, LLVMBuildLShr, LLVMBuildLoad2, LLVMBuildMul, LLVMBuildOr, LLVMBuildPhi,
    LLVMBuildPtrToInt,
    LLVMBuildRet, LLVMBuildRetVoid, LLVMBuildSDiv, LLVMBuildSExt, LLVMBuildSIToFP, LLVMBuildSRem,
    LLVMBuildSelect, LLVMBuildShl, LLVMBuildStore, LLVMBuildStructGEP2,
    LLVMBuildSub, LLVMBuildSwitch, LLVMBuildTrunc, LLVMBuildUDiv, LLVMBuildUIToFP, LLVMBuildURem,
    LLVMBuildUnreachable, LLVMBuildXor, LLVMBuildZExt, LLVMConstInt, LLVMConstNull, LLVMConstReal,
    LLVMAppendModuleInlineAsm,
    LLVMContextCreate, LLVMContextDispose, LLVMCreateBuilderInContext, LLVMCreateEnumAttribute,
    LLVMDisposeBuilder,
    LLVMDisposeMessage, LLVMDisposeModule, LLVMDoubleTypeInContext, LLVMFloatTypeInContext,
    LLVMFunctionType, LLVMGetEnumAttributeKindForName, LLVMGetFirstInstruction,
    LLVMGetIntTypeWidth, LLVMGetMDKindIDInContext, LLVMGetNamedFunction, LLVMGetNextInstruction,
    LLVMGetParam,
    LLVMGetInlineAsm,
    LLVMStructTypeInContext,
    LLVMGetTarget, LLVMGetTypeKind,
    LLVMGetUndef, LLVMInt16TypeInContext, LLVMInt1TypeInContext, LLVMInt32TypeInContext,
    LLVMInt64TypeInContext,
    LLVMInt8TypeInContext, LLVMMDNodeInContext2, LLVMMDStringInContext2, LLVMMetadataAsValue,
    LLVMModuleCreateWithNameInContext,
    LLVMPointerTypeInContext,
    LLVMPositionBuilderAtEnd, LLVMPositionBuilderBefore, LLVMPrintModuleToString,
    LLVMSetAlignment, LLVMSetCurrentDebugLocation2, LLVMSetGlobalConstant, LLVMSetInitializer,
    LLVMSetLinkage, LLVMSetMetadata, LLVMSetOrdering, LLVMSetSection, LLVMSetTarget,
    LLVMSetVolatile,
    LLVMConstArray2,
    LLVMStructCreateNamed, LLVMStructSetBody, LLVMValueAsMetadata, LLVMVectorType,
    LLVMVoidTypeInContext,
};
use llvm_sys::debuginfo::{
    LLVMCreateDIBuilder, LLVMDIBuilderCreateCompileUnit, LLVMDIBuilderCreateDebugLocation,
    LLVMDIBuilderCreateFile, LLVMDIBuilderCreateFunction, LLVMDIBuilderCreateSubroutineType,
    LLVMDIBuilderFinalize, LLVMDebugMetadataVersion, LLVMDisposeDIBuilder,
    LLVMDWARFEmissionKind, LLVMDWARFSourceLanguage, LLVMDIFlagPrototyped,
    LLVMMetadataReplaceAllUsesWith, LLVMSetSubprogram, LLVMTemporaryMDNode,
};
use llvm_sys::error::{LLVMDisposeErrorMessage, LLVMErrorRef, LLVMGetErrorMessage};
use llvm_sys::LLVMLinkage;
use llvm_sys::transforms::pass_builder::{
    LLVMCreatePassBuilderOptions, LLVMDisposePassBuilderOptions, LLVMRunPasses,
};
use llvm_sys::{
    LLVMAttributeFunctionIndex, LLVMAttributeIndex, LLVMAttributeReturnIndex, LLVMInlineAsmDialect,
    LLVMIntPredicate, LLVMModuleFlagBehavior, LLVMRealPredicate, LLVMTypeKind,
};
use llvm_sys::prelude::{
    LLVMBasicBlockRef, LLVMBool, LLVMBuilderRef, LLVMContextRef, LLVMDIBuilderRef,
    LLVMMetadataRef, LLVMModuleRef, LLVMTypeRef, LLVMValueRef,
};
use llvm_sys::target::{
    LLVMABIAlignmentOfType, LLVMABISizeOfType, LLVMByteOrder, LLVMByteOrdering,
    LLVMDisposeTargetData, LLVMSetModuleDataLayout, LLVMTargetDataRef,
    LLVM_InitializeNativeAsmParser, LLVM_InitializeNativeAsmPrinter, LLVM_InitializeNativeTarget,
    LLVMInitializeAArch64AsmPrinter, LLVMInitializeAArch64Target, LLVMInitializeAArch64TargetInfo,
    LLVMInitializeAArch64TargetMC, LLVMInitializeRISCVAsmPrinter, LLVMInitializeRISCVTarget,
    LLVMInitializeRISCVTargetInfo, LLVMInitializeRISCVTargetMC, LLVMInitializeX86AsmPrinter,
    LLVMInitializeX86Target, LLVMInitializeX86TargetInfo, LLVMInitializeX86TargetMC,
    LLVMInitializeAArch64AsmParser, LLVMInitializeRISCVAsmParser, LLVMInitializeX86AsmParser,
};
use llvm_sys::target_machine::{
    LLVMCodeGenFileType, LLVMCodeGenOptLevel, LLVMCodeModel, LLVMCreateTargetDataLayout,
    LLVMCreateTargetMachine, LLVMDisposeTargetMachine, LLVMGetDefaultTargetTriple,
    LLVMGetTargetFromTriple, LLVMRelocMode, LLVMTargetMachineEmitToFile, LLVMTargetMachineRef,
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

/// Take the message out of an `LLVMErrorRef` and consume the error.
///
/// `LLVMRunPasses` reports a failed pipeline as an error object rather than a
/// status code, and `LLVMGetErrorMessage` is the consuming read: the error is
/// not usable after it, and the string it returns is freed with
/// `LLVMDisposeErrorMessage` rather than `LLVMDisposeMessage`.
///
/// # Safety
///
/// `err` must be a non-null error an LLVM function returned and not yet
/// consumed.
unsafe fn take_error(err: LLVMErrorRef) -> String {
    // SAFETY: the caller guarantees an unconsumed error; this read consumes it.
    let ptr = unsafe { LLVMGetErrorMessage(err) };
    if ptr.is_null() {
        return String::new();
    }
    // SAFETY: the pointer is the NUL-terminated string LLVM allocated; the copy
    // happens before it is freed.
    let text = unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned();
    // SAFETY: the same pointer, released exactly once, by the disposer that
    // pairs with LLVMGetErrorMessage.
    unsafe { LLVMDisposeErrorMessage(ptr) };
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
    // Inline asm (kernel stage 5) goes through the target's own assembler, so
    // the MC asm parser has to be registered as well. The asm printer alone is
    // not enough: without this, any `asm(...)` aborts the build with "we don't
    // have an asm parser for this target".
    // SAFETY: as above; this one registers the asm parser.
    let parser_failed = unsafe { LLVM_InitializeNativeAsmParser() };
    if parser_failed != 0 {
        return Err("LLVM has no assembly parser for this host".to_string());
    }
    Ok(())
}

/// Load the code generator for the triple a freestanding build targets (kernel
/// stage 4).
///
/// [`initialize_native_target`] registers only the host's own backend, which is
/// the whole of what a hosted build needs; a freestanding build targets a triple
/// the host is not, so its backend has to be loaded by name. This calls the
/// same three `LLVMInitialize<Target>*` entries per architecture that LLVM's
/// own `LLVMInitializeAllTargets` macro expands to, for the architectures this
/// compiler can build for. Calling them directly rather than through the
/// "all targets" wrapper is deliberate: that wrapper is a C static inline in
/// `llvm-c/Target.h`, so which targets it registers is decided by the target
/// list baked into the LLVM headers on the build machine, and a header list
/// without `riscv` would make it silently register nothing this mode needs.
///
/// `Err` names the triple when no architecture matches; the caller turns that
/// into the build's diagnostic.
pub fn initialize_target_for(triple: &str) -> Result<(), String> {
    // SAFETY: every one of these reads no arguments and only registers target
    // code. They are idempotent, so a second freestanding build in one process
    // is not a problem, and a target that is already registered is a no-op.
    unsafe {
        if triple.starts_with("riscv64") || triple.starts_with("riscv32") {
            LLVMInitializeRISCVTargetInfo();
            LLVMInitializeRISCVTarget();
            LLVMInitializeRISCVTargetMC();
            LLVMInitializeRISCVAsmPrinter();
            // The module-level `_start` blob is assembled by the target's own
            // asm parser, so a freestanding build needs this one too.
            LLVMInitializeRISCVAsmParser();
        } else if triple.starts_with("aarch64") || triple.starts_with("arm64") {
            LLVMInitializeAArch64TargetInfo();
            LLVMInitializeAArch64Target();
            LLVMInitializeAArch64TargetMC();
            LLVMInitializeAArch64AsmPrinter();
            LLVMInitializeAArch64AsmParser();
        } else if triple.starts_with("x86_64") || triple.starts_with("i686") {
            LLVMInitializeX86TargetInfo();
            LLVMInitializeX86Target();
            LLVMInitializeX86TargetMC();
            LLVMInitializeX86AsmPrinter();
            LLVMInitializeX86AsmParser();
        } else {
            return Err(format!(
                "kelpc has no code generator to load for the freestanding triple '{triple}'"
            ));
        }
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
    /// (`plan/inline-asm.md`).
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

    /// Place a global (or a function) in section `section` (`LLVMSetSection`),
    /// which is how a `section("...")` clause reaches the object writer
    /// (kernel stage 4). An empty name would be the target's default, so the
    /// caller passes only a name a manifest wrote.
    pub fn set_section(&self, section: &str) {
        let section = cstring(section);
        // SAFETY: as above; LLVM copies the string.
        unsafe { LLVMSetSection(self.raw, section.as_ptr()) };
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
    raw: LLVMValueRef,
    context: LLVMContextRef,
    context_marker: PhantomData<&'ctx Context>,
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

/// The one ordering every atomic instruction is emitted at (MK9-S3-M1 stage
/// A1). The surface states exactly one (`Ordering.SeqCst`), so the lowering has
/// nothing to choose between and the C API's enum is named once, here, rather
/// than at each of the four instructions that carry it.
pub const SEQ_CST: llvm_sys::LLVMAtomicOrdering =
    llvm_sys::LLVMAtomicOrdering::LLVMAtomicOrderingSequentiallyConsistent;

/// One atomic read-modify-write (`atomicrmw`). Named in our own terms so
/// `codegen_llvm.rs` never has to name an `llvm-sys` type: this module is the
/// only client of that crate.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AtomicRmw {
    Xchg,
    Add,
    Sub,
    And,
    Or,
    Xor,
}

impl AtomicRmw {
    fn as_raw(self) -> llvm_sys::LLVMAtomicRMWBinOp {
        use llvm_sys::LLVMAtomicRMWBinOp as Op;
        match self {
            AtomicRmw::Xchg => Op::LLVMAtomicRMWBinOpXchg,
            AtomicRmw::Add => Op::LLVMAtomicRMWBinOpAdd,
            AtomicRmw::Sub => Op::LLVMAtomicRMWBinOpSub,
            AtomicRmw::And => Op::LLVMAtomicRMWBinOpAnd,
            AtomicRmw::Or => Op::LLVMAtomicRMWBinOpOr,
            AtomicRmw::Xor => Op::LLVMAtomicRMWBinOpXor,
        }
    }
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
    /// lowering sets it where the C backend writes its `#line` marker.
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

/// The portable architecture baseline `LLVMCreateTargetMachine` is given as
/// its CPU name for `triple` (MISSING-hV6qL). Not the host's actual CPU —
/// that is `LLVMGetHostCPUName`, which is `-march=native` and makes the
/// object non-portable, a different and unrelated thing. This just names
/// the architecture's own floor, the same baseline `clang -O2` (with no
/// `-march`) targets, so LLVM's X86 subtarget stops falling back to `i586`
/// tuning for an empty CPU string — a fallback with a real, cited FIXME in
/// LLVM's own source, not a deliberate choice anyone made for that string.
///
/// An architecture this doesn't know keeps the old empty-string behavior:
/// `kelpc` only ever builds for the host's own triple today
/// (`default_target_triple`), so only that triple's prefix needs an entry
/// here for the fallback to stop biting; an unrecognized prefix is exactly
/// as portable as before this function existed, just not any better tuned.
fn baseline_cpu(triple: &str) -> &'static str {
    if triple.starts_with("x86_64") {
        "x86-64"
    } else if triple.starts_with("aarch64") || triple.starts_with("arm64") {
        "generic"
    } else {
        ""
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
    /// [`LLVMCodeGenOptLevel::LLVMCodeGenLevelDefault`] (the `-O2` codegen
    /// level).
    ///
    /// The shortcut for the one triple `kelpc` currently emits for at the
    /// default level. [`for_host_at`] is the shaped-for-a-level entry point
    /// `codegen_llvm::compile` uses; see [`for_triple`] for the parameters and
    /// the errors.
    ///
    /// [`for_host_at`]: TargetMachine::for_host_at
    /// [`for_triple`]: TargetMachine::for_triple
    pub fn for_host() -> Result<Self, String> {
        TargetMachine::for_host_at(LLVMCodeGenOptLevel::LLVMCodeGenLevelDefault)
    }

    /// A target machine for the host triple at `level`, the codegen half of an
    /// optimization level (`opt::OptLevel::codegen_level`).
    ///
    /// The level chooses instruction selection, scheduling and register
    /// allocation; the module passes are the other half and `run_passes` runs
    /// them over the module this machine writes.
    pub fn for_host_at(level: LLVMCodeGenOptLevel) -> Result<Self, String> {
        let triple = default_target_triple();
        TargetMachine::for_triple(&triple, level)
    }

    /// A target machine for `triple` (`LLVMGetTargetFromTriple` and
    /// `LLVMCreateTargetMachine`), disposed on drop.
    ///
    /// `initialize_native_target` must have run first for a triple the host's
    /// LLVM build covers. The CPU is `baseline_cpu(triple)` (MISSING-hV6qL):
    /// an empty CPU string makes LLVM's X86 subtarget fall back to tuning for
    /// `i586`, whose scheduling model has no micro-op buffer, which silently
    /// turns off partial loop unrolling for every program this backend
    /// builds — not a feature choice, an LLVM fallback with no legitimate use
    /// here. Features are still left empty: no CPU-specific instruction-set
    /// extension is assumed, only the architecture's own baseline name is
    /// given, so this stays as portable as the old empty string was and is
    /// not `-march=native`. The code model is `LLVMCodeModelDefault`.
    /// Relocations are
    /// `LLVMRelocPIC`, not `LLVMRelocDefault`: LLVM's default for x86-64 Linux is
    /// a static reloc model, whose object carries absolute relocations, and the
    /// host `cc` builds PIE executables by default, so that object would fail to
    /// link with `relocation R_X86_64_32 can not be used when making a PIE
    /// object`. PIC, which is also what clang defaults to for this triple,
    /// produces the same kind of object the C backend's `cc` path does.
    ///
    /// `Err` carries LLVM's message when the triple names no compiled-in target,
    /// which is the one failure mode.
    pub fn for_triple(triple_str: &str, level: LLVMCodeGenOptLevel) -> Result<Self, String> {
        TargetMachine::create(
            triple_str,
            baseline_cpu(triple_str),
            "",
            level,
            LLVMRelocMode::LLVMRelocPIC,
            LLVMCodeModel::LLVMCodeModelDefault,
        )
    }

    /// A target machine for a freestanding triple (kernel stage 4): the same
    /// creation as [`for_triple`], with the choices a bare-metal image needs.
    ///
    /// A `riscv64` triple gets the `generic-rv64` CPU and `+m,+a,+c`: the
    /// extensions the QEMU `virt` machine's baseline core has, and the ones any
    /// RISC-V code has to assume to be portable across the ISA's profiles.
    /// Every other triple keeps `baseline_cpu` and no features, exactly as the
    /// hosted path does. Relocations are `LLVMRelocStatic`, because there is no
    /// dynamic loader to fix up a position-independent object, and the code
    /// model is `LLVMCodeModelMedium`, which reaches the whole address space a
    /// kernel image is placed in without the long-call sequences of `large`.
    ///
    /// [`for_triple`]: TargetMachine::for_triple
    pub fn for_freestanding(
        triple_str: &str,
        level: LLVMCodeGenOptLevel,
    ) -> Result<Self, String> {
        let (cpu, features) = if triple_str.starts_with("riscv64") {
            ("generic-rv64", "+m,+a,+c")
        } else {
            (baseline_cpu(triple_str), "")
        };
        TargetMachine::create(
            triple_str,
            cpu,
            features,
            level,
            LLVMRelocMode::LLVMRelocStatic,
            LLVMCodeModel::LLVMCodeModelMedium,
        )
    }

    /// The shared body of [`for_triple`] and [`for_freestanding`]: look the
    /// triple up, then create the machine with the given subtarget choices.
    ///
    /// [`for_triple`]: TargetMachine::for_triple
    /// [`for_freestanding`]: TargetMachine::for_freestanding
    fn create(
        triple_str: &str,
        cpu: &str,
        features: &str,
        level: LLVMCodeGenOptLevel,
        reloc: LLVMRelocMode,
        code_model: LLVMCodeModel,
    ) -> Result<Self, String> {
        let triple = cstring(triple_str);
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
        let cpu = cstring(cpu);
        let features = cstring(features);
        // SAFETY: the target came from the call above and is registered; LLVM
        // copies both strings. `cpu` is a portable architecture baseline (see
        // `baseline_cpu`), never the host's own CPU — that would be
        // `-march=native` and make the object non-portable, which this is
        // not.
        let raw = unsafe {
            LLVMCreateTargetMachine(
                target,
                triple.as_ptr(),
                cpu.as_ptr(),
                features.as_ptr(),
                level,
                reloc,
                code_model,
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

/// Guard for `LLVMTargetDataRef`: the layout the target machine computes for a
/// type.
///
/// The LLVM backend needs two numbers from it to build a tagged union, because
/// an LLVM struct has no way to declare a field's alignment other than through
/// the field's own type: the widest payload's size sets the union's size, and
/// its alignment is the field type the union is anchored on (see
/// `codegen_llvm.rs::enum_union_type`). Asking the target instead of assuming
/// keeps the layout a property of the triple rather than of x86-64 (MK7 §3.1).
///
/// Created from a [`TargetMachine`] with [`TargetMachine::data_layout`] and
/// disposed on drop.
pub struct DataLayout {
    raw: LLVMTargetDataRef,
}

impl TargetMachine {
    /// This machine's data layout (`LLVMCreateTargetDataLayout`), disposed on
    /// drop. LLVM allocates a fresh `LLVMTargetDataRef` per call, so this is a
    /// guard rather than a view of data the machine owns.
    pub fn data_layout(&self) -> DataLayout {
        // SAFETY: the machine is live and owns the target it was created from.
        DataLayout {
            raw: unsafe { LLVMCreateTargetDataLayout(self.raw) },
        }
    }
}

impl DataLayout {
    /// The store size of `ty` in bytes as the target lays it out
    /// (`LLVMABISizeOfType`), including the trailing padding up to the type's
    /// own alignment. `ty` must be a sized type.
    pub fn abi_size_of(&self, ty: Type<'_>) -> u64 {
        // SAFETY: the layout and the type are live, and the type has a body
        // (an opaque one would be LLVM's own error, not unsoundness here).
        unsafe { LLVMABISizeOfType(self.raw, ty.as_raw()) }
    }

    /// The ABI alignment of `ty` in bytes as the target lays it out
    /// (`LLVMABIAlignmentOfType`). `ty` must be a sized type.
    pub fn abi_alignment_of(&self, ty: Type<'_>) -> u32 {
        // SAFETY: as above.
        unsafe { LLVMABIAlignmentOfType(self.raw, ty.as_raw()) }
    }

    /// Whether the target lays a multi-byte value out least-significant byte
    /// first (`LLVMByteOrder`). The byte-order conversions read this rather
    /// than assuming the host, so `to_le` is the identity exactly on a
    /// little-endian target and a swap on a big-endian one.
    pub fn is_little_endian(&self) -> bool {
        // SAFETY: the layout is live and `LLVMByteOrder` only reads it.
        unsafe { LLVMByteOrder(self.raw) == LLVMByteOrdering::LLVMLittleEndian }
    }
}

impl Drop for DataLayout {
    fn drop(&mut self) {
        // SAFETY: raw came from LLVMCreateTargetDataLayout and is disposed once.
        unsafe { LLVMDisposeTargetData(self.raw) };
    }
}

/// Handle for `LLVMMetadataRef`: one debug descriptor (a file, a compile unit,
/// a subroutine type, a subprogram or a source location).
///
/// LLVM owns the node: it lives in the metadata context of the LLVM context
/// that built it and is freed with that context, so this is a `Copy` handle
/// rather than a guard. The lifetime marker ties it to the [`Context`] the
/// [`DebugBuilder`] that made it was built in, which is what stops a node from
/// outliving the context it belongs to.
#[derive(Clone, Copy)]
pub struct Metadata<'ctx> {
    raw: LLVMMetadataRef,
    context_marker: PhantomData<&'ctx Context>,
}

/// Guard for `LLVMDIBuilderRef`: the DWARF debug-information builder of one
/// module.
///
/// This is where the metadata a `.kp` file and a `.kp` line become DWARF is
/// constructed (MK7 §4.4). The C API's builder is a `DIBuilder` in C++, and it
/// is disposed here the same way [`Context`] and [`Module`] are: the builder
/// must be finalized before it is disposed, so [`DebugBuilder::finalize`] runs
/// first, and it must be disposed before the module it was built for drops,
/// which the lifetime marker and the ordinary drop order of the two guards give.
///
/// The builder carries no ordering requirement of its own beyond that: a
/// subprogram may be created before or after the function it describes, as long
/// as the module is live for both.
pub struct DebugBuilder<'ctx> {
    raw: LLVMDIBuilderRef,
    // The context is kept rather than re-read from the module: a source location
    // is built from the context plus a scope, and LLVM has no "builder's
    // context" accessor. Valid because the marker ties this to the guard that
    // owns the context.
    context: LLVMContextRef,
    // `finalize` is idempotent from LLVM's side only in the sense that nothing
    // reads its return; running it twice is harmless but running it after the
    // module has been written is too late, so the flag is what keeps the drop
    // path from being the second caller.
    finalized: Cell<bool>,
    // Whether a pass pipeline runs over the module. DWARF carries it as the
    // compile unit's and each subprogram's `isOptimized` flag, which is what a
    // debugger reads to know an instruction may have moved. It is `opt::OptLevel::is_optimized`,
    // false only at `-O0`.
    optimized: bool,
    context_marker: PhantomData<&'ctx Context>,
}

impl<'ctx> DebugBuilder<'ctx> {
    /// A debug builder for `module` (`LLVMCreateDIBuilder`), finalized and
    /// disposed on drop.
    ///
    /// `optimized` is the compile unit's `isOptimized` flag and the same flag
    /// on each subprogram: true when a pass pipeline will run over the module,
    /// so a debugger is told the lines it reads are the optimized ones. The
    /// unresolved-node variant is not used: everything here is created
    /// explicitly, in order, and nothing is left for a later pass to tie up.
    pub fn new(module: &Module<'ctx>, optimized: bool) -> Self {
        DebugBuilder {
            // SAFETY: the module is live for 'ctx, and the builder is owned and
            // disposed on drop.
            raw: unsafe { LLVMCreateDIBuilder(module.as_raw()) },
            context: module.context,
            finalized: Cell::new(false),
            optimized,
            context_marker: PhantomData,
        }
    }

    /// A file descriptor for `path` (`LLVMDIBuilderCreateFile`).
    ///
    /// DWARF carries a file as a name plus a directory; a reader joins them and
    /// an empty directory means the compilation directory. `split_path` keeps
    /// the two parts of a `.kp` path apart for that reason.
    pub fn file(&self, path: &Path) -> Metadata<'ctx> {
        let (directory, filename) = split_path(path);
        let filename = cstring(&filename);
        let directory = cstring(&directory);
        Metadata {
            // SAFETY: the builder is live and both strings outlive the call.
            raw: unsafe {
                LLVMDIBuilderCreateFile(
                    self.raw,
                    filename.as_ptr(),
                    filename.as_bytes().len(),
                    directory.as_ptr(),
                    directory.as_bytes().len(),
                )
            },
            context_marker: PhantomData,
        }
    }

    /// A compile unit whose main file is `file`
    /// (`LLVMDIBuilderCreateCompileUnit`).
    ///
    /// The language is spelled `C` because DWARF has no Kelp code and the tag is
    /// otherwise inert: what this milestone emits is a line table (a file, a
    /// subprogram and a source location per statement), never a type or a
    /// variable, so nothing downstream reads the language out of the unit.
    ///
    /// `isOptimized` is `true` when a pass pipeline runs over this module
    /// (`opt::OptLevel::is_optimized`): the flag tells a debugger the line it
    /// reads may be an instruction that moved, and it is false at `-O0`, where
    /// the IR the lowering wrote is the IR the target machine lowers.
    pub fn compile_unit(&self, file: Metadata<'ctx>, producer: &str) -> Metadata<'ctx> {
        let producer = cstring(producer);
        let empty = cstring("");
        Metadata {
            // SAFETY: the builder is live, `file` is a metadata node in the same
            // context, and every string outlives the call. Empty is passed for
            // the flags, the split name, the sysroot and the SDK, and 0 for the
            // runtime version and the DWO id: there is no split DWARF and no
            // profiled debug info here.
            raw: unsafe {
                LLVMDIBuilderCreateCompileUnit(
                    self.raw,
                    LLVMDWARFSourceLanguage::LLVMDWARFSourceLanguageC,
                    file.raw,
                    producer.as_ptr(),
                    producer.as_bytes().len(),
                    self.optimized as LLVMBool,
                    empty.as_ptr(),
                    0,
                    0,
                    empty.as_ptr(),
                    0,
                    LLVMDWARFEmissionKind::LLVMDWARFEmissionKindFull,
                    0,
                    0,
                    0,
                    empty.as_ptr(),
                    0,
                    empty.as_ptr(),
                    0,
                )
            },
            context_marker: PhantomData,
        }
    }

    /// The type of a subroutine that takes no arguments and returns nothing
    /// (`LLVMDIBuilderCreateSubroutineType`).
    ///
    /// One shared node for every function is enough because nothing here
    /// describes a parameter or a value: the unit is a line table, and a
    /// subprogram needs a type only to be well formed. `LLVMDIFlagPrototyped`
    /// says the empty list is a real prototype rather than an unknown one.
    pub fn subroutine_type(&self, file: Metadata<'ctx>) -> Metadata<'ctx> {
        // An empty parameter list is an empty array, and LLVM's C API takes the
        // array as a pointer plus a length. The zero-length array is borrowed
        // for the call only; LLVM copies it into a metadata node.
        let mut params: [LLVMMetadataRef; 0] = [];
        Metadata {
            // SAFETY: the builder is live, `file` belongs to its context, and the
            // array pointer is valid for the zero elements the length claims.
            raw: unsafe {
                LLVMDIBuilderCreateSubroutineType(
                    self.raw,
                    file.raw,
                    params.as_mut_ptr(),
                    0,
                    LLVMDIFlagPrototyped,
                )
            },
            context_marker: PhantomData,
        }
    }

    /// A subprogram `name` defined at `line` of `file`, in `scope`, with the
    /// subroutine type `ty` (`LLVMDIBuilderCreateFunction`).
    ///
    /// `scope` is the function's file, which is what clang passes for a
    /// file-scope function; the unit is the builder's own compile unit either
    /// way, so the descriptor does not have to be handed the unit to be part of
    /// it. `linkage_name` is the symbol the object defines, which is not always
    /// the `.kp` name (a mangled function keeps its mangled symbol).
    /// `scope_line` is the line the body opens on and is given the same value as
    /// `line`, which is what a one-line declaration wants; `is_local_to_unit`
    /// is false and `is_optimized` is the builder's own flag, for the reason
    /// `compile_unit` gives.
    #[allow(clippy::too_many_arguments)]
    pub fn subprogram(
        &self,
        scope: Metadata<'ctx>,
        name: &str,
        linkage_name: &str,
        file: Metadata<'ctx>,
        line: u32,
        ty: Metadata<'ctx>,
    ) -> Metadata<'ctx> {
        let name = cstring(name);
        let linkage_name = cstring(linkage_name);
        Metadata {
            // SAFETY: the builder is live, every metadata node belongs to its
            // context, and the strings outlive the call.
            raw: unsafe {
                LLVMDIBuilderCreateFunction(
                    self.raw,
                    scope.raw,
                    name.as_ptr(),
                    name.as_bytes().len(),
                    linkage_name.as_ptr(),
                    linkage_name.as_bytes().len(),
                    file.raw,
                    line,
                    ty.raw,
                    0,
                    1,
                    line,
                    LLVMDIFlagPrototyped,
                    self.optimized as LLVMBool,
                )
            },
            context_marker: PhantomData,
        }
    }

    /// A source location at `line:column` inside `scope`
    /// (`LLVMDIBuilderCreateDebugLocation`).
    ///
    /// There is no inlined-at location: nothing in either backend inlines.
    pub fn location(&self, line: u32, column: u32, scope: Metadata<'ctx>) -> Metadata<'ctx> {
        Metadata {
            // SAFETY: the builder's context is live and `scope` is a metadata
            // node in it; a null inlined-at is LLVM's "not inlined".
            raw: unsafe {
                LLVMDIBuilderCreateDebugLocation(
                    self.context,
                    line,
                    column,
                    scope.raw,
                    null_mut(),
                )
            },
            context_marker: PhantomData,
        }
    }

    /// Attach `subprogram` to `function` (`LLVMSetSubprogram`), which is what
    /// makes `function`'s own line the subprogram's `line` in the object.
    pub fn set_subprogram(&self, function: Function<'ctx>, subprogram: Metadata<'ctx>) {
        // SAFETY: the function and the metadata node are live and belong to one
        // module's context.
        unsafe { LLVMSetSubprogram(function.as_raw(), subprogram.raw) };
    }

    /// Construct the descriptors the builder deferred
    /// (`LLVMDIBuilderFinalize`).
    ///
    /// This must run after the last metadata node is made and before the module
    /// is written, so `codegen_llvm.rs` calls it explicitly; the `Drop` below
    /// repeats it only for a builder that never got there, because disposing an
    /// unfinalized builder is what LLVM's own contract forbids.
    pub fn finalize(&self) {
        if self.finalized.replace(true) {
            return;
        }
        // SAFETY: the builder is live and this is its first finalize.
        unsafe { LLVMDIBuilderFinalize(self.raw) };
    }

    /// The underlying `LLVMDIBuilderRef`, for a C API call this wrapper lacks.
    pub fn as_raw(&self) -> LLVMDIBuilderRef {
        self.raw
    }
}

impl Drop for DebugBuilder<'_> {
    fn drop(&mut self) {
        // SAFETY: the builder is live; `finalize` runs at most once either way.
        self.finalize();
        // SAFETY: raw came from LLVMCreateDIBuilder and is disposed once.
        unsafe { LLVMDisposeDIBuilder(self.raw) };
    }
}

/// Split a `.kp` path into the directory and the file name a `DIFile` carries
/// (`LLVMDIBuilderCreateFile`).
///
/// DWARF keeps the two apart and a reader joins them, so an empty directory
/// means "the compilation directory". A bare file name therefore gets `.`, the
/// directory that means the process's working directory, rather than an empty
/// string a reader would have to interpret.
fn split_path(path: &Path) -> (String, String) {
    match (path.parent(), path.file_name()) {
        (Some(dir), Some(name)) if !dir.as_os_str().is_empty() => (
            dir.to_string_lossy().into_owned(),
            name.to_string_lossy().into_owned(),
        ),
        (_, Some(name)) => (".".to_string(), name.to_string_lossy().into_owned()),
        _ => (String::new(), path.to_string_lossy().into_owned()),
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
