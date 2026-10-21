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
    LLVMConstNamedStruct,
    LLVMAppendModuleInlineAsm,
    LLVMContextCreate, LLVMContextDispose, LLVMCreateBuilderInContext, LLVMCreateEnumAttribute,
    LLVMCreateStringAttribute,
    LLVMDisposeBuilder,
    LLVMDisposeMessage, LLVMDisposeModule, LLVMDoubleTypeInContext, LLVMFloatTypeInContext,
    LLVMFunctionType, LLVMGetEnumAttributeKindForName, LLVMGetFirstFunction,
    LLVMGetFirstInstruction,
    LLVMGetIntTypeWidth, LLVMGetMDKindIDInContext, LLVMGetNamedFunction, LLVMGetNextFunction,
    LLVMGetNextInstruction,
    LLVMGetParam, LLVMGetVersion, LLVMIsDeclaration,
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
    LLVMSetThreadLocal, LLVMSetThreadLocalMode, LLVMSetVolatile,
    LLVMConstArray2, LLVMConstStringInContext2, LLVMIsConstant,
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


mod context;
pub use context::*;

mod values;
pub use values::*;

mod module;
pub use module::*;

mod ops;
pub use ops::*;

mod builder;
pub use builder::*;

mod target;
pub use target::*;

mod debug;
pub use debug::*;

#[cfg(test)]
#[path = "llvm_ffi/tests.rs"]
mod tests;
