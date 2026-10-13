//! User `extern fun`s that take or return a `datac`, `Bytes` or `String` by
//! value, lowered to LLVM IR with the System V x86_64 classification.
//!
//! The Kelp side of such an extern is declared the way every user extern is
//! (`codegen_llvm`'s `declare_extern`): an aggregate over 16 bytes is passed as a
//! pointer to the caller's copy and, when it comes back, is returned through a
//! hidden first pointer; a smaller one is a first-class LLVM value. The body
//! that declaration names is defined here, in the program's own module, as a
//! trampoline that calls the native symbol the manifest allowlisted, with each
//! argument and result spelled the way the System V ABI passes it.
//!
//! The classes (System V AMD64 ABI, section 3.2.3):
//! - an aggregate over 16 bytes is MEMORY: an argument is copied to the stack
//!   (`byval`), and a result is written through a hidden first pointer;
//! - an aggregate of 16 bytes or less is one or two eightbytes, each INTEGER if
//!   a scalar in it is an integer or a pointer and SSE otherwise. An INTEGER
//!   eightbyte travels as an `i64` and an SSE one as a `double`, which holds the
//!   same bits.
//!
//! A narrow integer or a `bool` argument widens to `i32`, as a C caller passes
//! it, and a narrow result is truncated back.
//!
//! What this module does not lower keeps the path it was on: a `packed`,
//! over-aligned or field-less `datac`, an `enum`, a container (`List`, `Box`,
//! `Map`, an array), a view, a closure, and a call whose register arguments do
//! not all fit (six INTEGER, eight SSE), which SysV sends to memory and this
//! does not model.

use std::ffi::c_char;
use std::os::raw::c_uint;

use llvm_sys::core::{LLVMAddCallSiteAttribute, LLVMCreateTypeAttribute, LLVMGetEnumAttributeKindForName};

use crate::ast::Repr;
use crate::llvm_ffi::{Builder, Context, Module, Type, Value};
use crate::tir::{Function as KelpFn, Program};
use crate::types::Ty;

/// The INTEGER and SSE argument registers of the System V x86_64 ABI.
const INT_REGS: u32 = 6;
const SSE_REGS: u32 = 8;

/// One eightbyte's class.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Class {
    Int,
    Sse,
}

/// One scalar inside an aggregate: its byte offset, its size and its class.
#[derive(Clone, Copy)]
struct Leaf {
    offset: u64,
    size: u64,
    class: Class,
}

/// One by-value argument of a lowered extern.
enum Arg {
    /// A scalar or a pointer, passed as itself.
    Scalar(Ty),
    /// An aggregate over 16 bytes, passed in memory: the Kelp side gives its
    /// address, and the native side gets a `byval` copy of it.
    Memory(Ty),
    /// An aggregate of 16 bytes or less, passed in one register per eightbyte.
    Regs(Vec<Class>),
}

/// The result of a lowered extern.
enum Ret {
    Void,
    Scalar(Ty),
    /// An aggregate over 16 bytes: the native side writes it through a hidden
    /// first pointer, which is the Kelp side's own hidden pointer.
    Memory,
    /// An aggregate of 16 bytes or less, returned in one or two registers.
    Regs(Ty, Vec<Class>),
}

struct Plan {
    ret: Ret,
    params: Vec<Arg>,
}

/// Whether `ty` is an aggregate this module knows how to classify (before the
/// layout check): the heap types, a `datac`, and the containers it refuses.
fn is_aggregate(ty: &Ty) -> bool {
    matches!(
        ty,
        Ty::Bytes | Ty::String | Ty::Struct(_) | Ty::View | Ty::List(_) | Ty::Box(_) | Ty::Map(..) | Ty::Array(..)
    )
}

/// The LLVM type of a scalar or pointer in this module's context, the same one
/// the codegen gives it. `None` for a type this module does not pass.
fn scalar_llvm<'a>(ctx: &'a Context, ty: &Ty) -> Option<Type<'a>> {
    Some(match ty {
        Ty::I32 | Ty::U32 | Ty::Char => ctx.i32(),
        Ty::I64 | Ty::U64 | Ty::Usize | Ty::Isize => ctx.i64(),
        Ty::F32 => ctx.f32(),
        Ty::F64 => ctx.f64(),
        Ty::U8 => ctx.i8(),
        Ty::U16 => ctx.i16(),
        Ty::Bool => ctx.i1(),
        Ty::Str | Ty::Handle(_) | Ty::Ptr(_) => ctx.opaque_ptr(),
        Ty::Ref { inner, .. } if !matches!(**inner, Ty::Closure { .. } | Ty::ScopedFn { .. } | Ty::Fn { .. }) => {
            ctx.opaque_ptr()
        }
        _ => return None,
    })
}

/// The size, alignment and class of a scalar the layout places in memory.
fn scalar_leaf(ty: &Ty) -> Option<(u64, u64, Class)> {
    Some(match ty {
        Ty::I32 | Ty::U32 | Ty::Char => (4, 4, Class::Int),
        Ty::F32 => (4, 4, Class::Sse),
        Ty::I64 | Ty::U64 | Ty::Usize | Ty::Isize => (8, 8, Class::Int),
        Ty::F64 => (8, 8, Class::Sse),
        Ty::U8 | Ty::Bool => (1, 1, Class::Int),
        Ty::U16 => (2, 2, Class::Int),
        Ty::Str | Ty::Handle(_) | Ty::Ptr(_) => (8, 8, Class::Int),
        Ty::Ref { inner, .. } if !matches!(**inner, Ty::Closure { .. } | Ty::ScopedFn { .. } | Ty::Fn { .. }) => {
            (8, 8, Class::Int)
        }
        _ => return None,
    })
}

fn align_up(n: u64, align: u64) -> u64 {
    n.div_ceil(align) * align
}

/// Lays `ty` out at `base` the way the codegen's LLVM struct does (natural
/// alignment, no packing), pushing one leaf per scalar. Returns its size and
/// alignment, or `None` for a type this module does not lower.
fn flatten(program: &Program, ty: &Ty, base: u64, out: &mut Vec<Leaf>, depth: u32) -> Option<(u64, u64)> {
    if depth > 32 {
        return None;
    }
    match ty {
        Ty::Bytes | Ty::String => {
            for word in 0..3 {
                out.push(Leaf { offset: base + 8 * word, size: 8, class: Class::Int });
            }
            Some((24, 8))
        }
        Ty::Struct(name) => {
            if program.enums.iter().any(|e| e.name == *name) {
                return None;
            }
            let def = program.structs.iter().find(|s| s.name == *name)?;
            if !matches!(def.repr, Repr::C) || def.fields.is_empty() {
                return None;
            }
            let mut cur = 0;
            let mut align = 1;
            for field in &def.fields {
                let (size, falign) = flatten(program, &field.ty, 0, &mut Vec::new(), depth + 1)?;
                let off = align_up(cur, falign);
                flatten(program, &field.ty, base + off, out, depth + 1)?;
                cur = off + size;
                align = align.max(falign);
            }
            Some((align_up(cur, align), align))
        }
        _ => {
            let (size, align, class) = scalar_leaf(ty)?;
            out.push(Leaf { offset: base, size, class });
            Some((size, align))
        }
    }
}

/// How one by-value aggregate is passed: in memory, or in registers by class.
fn passing(program: &Program, ty: &Ty) -> Option<Result<Vec<Class>, ()>> {
    let mut leaves = Vec::new();
    let (size, _) = flatten(program, ty, 0, &mut leaves, 0)?;
    if leaves.iter().any(|l| l.offset % l.size != 0) {
        return None;
    }
    if size > 16 {
        return Some(Err(()));
    }
    let classes = (0..size.div_ceil(8))
        .map(|i| {
            let int = leaves.iter().any(|l| l.offset / 8 == i && l.class == Class::Int);
            if int {
                Class::Int
            } else {
                Class::Sse
            }
        })
        .collect();
    Some(Ok(classes))
}

/// The type of one eightbyte in a register.
fn eightbyte<'a>(ctx: &'a Context, class: Class) -> Type<'a> {
    match class {
        Class::Int => ctx.i64(),
        Class::Sse => ctx.f64(),
    }
}

/// The plan for a user extern that takes or returns an aggregate by value, or
/// `None` when the extern is not one this module lowers (it keeps its old path).
fn plan(program: &Program, f: &KelpFn) -> Option<Plan> {
    if !f.extern_ || !program.extern_ffi_symbols.contains_key(&f.name) {
        return None;
    }
    let mut any = false;
    let mut int_regs = 0u32;
    let mut sse_regs = 0u32;
    let ret = if f.ret == Ty::Unit {
        Ret::Void
    } else if is_aggregate(&f.ret) {
        any = true;
        match passing(program, &f.ret)? {
            Err(()) => Ret::Memory,
            Ok(classes) => Ret::Regs(f.ret.clone(), classes),
        }
    } else {
        scalar_leaf(&f.ret)?;
        Ret::Scalar(f.ret.clone())
    };
    if matches!(ret, Ret::Memory) {
        // The hidden pointer is the first integer argument.
        int_regs += 1;
    }
    let mut params = Vec::new();
    for (_, ty) in &f.params {
        if *ty == Ty::Unit {
            return None;
        }
        if is_aggregate(ty) {
            any = true;
            match passing(program, ty)? {
                Err(()) => params.push(Arg::Memory(ty.clone())),
                Ok(classes) => {
                    for c in &classes {
                        match c {
                            Class::Int => int_regs += 1,
                            Class::Sse => sse_regs += 1,
                        }
                    }
                    params.push(Arg::Regs(classes));
                }
            }
        } else {
            let (_, _, class) = scalar_leaf(ty)?;
            match class {
                Class::Int => int_regs += 1,
                Class::Sse => sse_regs += 1,
            }
            params.push(Arg::Scalar(ty.clone()));
        }
    }
    if !any || int_regs > INT_REGS || sse_regs > SSE_REGS {
        return None;
    }
    Some(Plan { ret, params })
}

/// Whether the user extern `f` is lowered by this module, so its body is the
/// trampoline defined here and it has no C wrapper.
pub(crate) fn lowered_in_ir(program: &Program, f: &KelpFn) -> bool {
    plan(program, f).is_some()
}

/// The native argument type for a scalar: a narrow integer or a `bool` widens to
/// `i32`, the register a C caller passes it in.
fn native_scalar<'a>(ctx: &'a Context, ty: &Ty) -> Option<Type<'a>> {
    match ty {
        Ty::U8 | Ty::U16 | Ty::Bool => Some(ctx.i32()),
        _ => scalar_llvm(ctx, ty),
    }
}

/// Attaches `byval(ty)` to argument `pos` (0-based) of `call`.
fn add_byval<'a>(ctx: &'a Context, call: Value<'a>, pos: usize, ty: Type<'a>) {
    let name = "byval";
    // SAFETY: the name is a static NUL-free string and the length is its byte
    // count; the kind is looked up in this context's table.
    let kind = unsafe { LLVMGetEnumAttributeKindForName(name.as_ptr().cast::<c_char>(), name.len()) };
    // SAFETY: the context and the type are live, and the attribute is owned by
    // the context.
    let attr = unsafe { LLVMCreateTypeAttribute(ctx.as_raw(), kind, ty.as_raw()) };
    // SAFETY: `call` is a call instruction, and index `pos + 1` is one of its
    // arguments (the return value is index 0).
    unsafe { LLVMAddCallSiteAttribute(call.as_raw(), (pos + 1) as c_uint, attr) };
}

/// Defines the trampoline of every lowered user extern that `owns` accepts, in
/// `module`. The extern's Kelp-side declaration already exists there with the
/// shape `declare_extern` gave it.
pub(crate) fn define_trampolines<'a>(
    ctx: &'a Context,
    module: &Module<'a>,
    program: &Program,
    owns: &dyn Fn(&KelpFn) -> bool,
    lty: &dyn Fn(&Ty) -> Result<Type<'a>, String>,
) -> Result<(), String> {
    for f in program.functions.iter().filter(|f| owns(f)) {
        if let Some(plan) = plan(program, f) {
            define(ctx, module, program, f, &plan, lty)?;
        }
    }
    Ok(())
}

fn define<'a>(
    ctx: &'a Context,
    module: &Module<'a>,
    program: &Program,
    f: &KelpFn,
    plan: &Plan,
    lty: &dyn Fn(&Ty) -> Result<Type<'a>, String>,
) -> Result<(), String> {
    let native = program
        .extern_ffi_symbols
        .get(&f.name)
        .ok_or_else(|| format!("the user extern '{}' has no native symbol", f.name))?;
    let kelp_name = crate::codegen_llvm::symbol_name(&f.name);
    let kelp = module
        .get_function(&kelp_name)
        .ok_or_else(|| format!("the user extern '{}' was not declared", f.name))?;
    let ptr = ctx.opaque_ptr();

    // The native declaration: the System V shape of the call.
    let mut native_params: Vec<Type<'a>> = Vec::new();
    let memory_ret = matches!(plan.ret, Ret::Memory);
    if memory_ret {
        native_params.push(ptr);
    }
    let missing = || format!("the user extern '{}' has a type this backend does not pass", f.name);
    for arg in &plan.params {
        match arg {
            Arg::Scalar(ty) => native_params.push(native_scalar(ctx, ty).ok_or_else(missing)?),
            Arg::Memory(_) => native_params.push(ptr),
            Arg::Regs(classes) => native_params.extend(classes.iter().map(|c| eightbyte(ctx, *c))),
        }
    }
    let native_ret = match &plan.ret {
        Ret::Void | Ret::Memory => ctx.void(),
        Ret::Scalar(ty) => native_scalar(ctx, ty).ok_or_else(missing)?,
        Ret::Regs(_, classes) if classes.len() == 1 => eightbyte(ctx, classes[0]),
        Ret::Regs(_, classes) => ctx.literal_struct(&[eightbyte(ctx, classes[0]), eightbyte(ctx, classes[1])]),
    };
    let native_ty = ctx.fn_type(native_ret, &native_params, false);
    let native_fn = match module.get_function(native) {
        Some(existing) => existing,
        None => module.add_function(native, native_ty),
    };

    let b = Builder::new(ctx);
    b.position_at_end(kelp.append_block("entry"));
    let mut next = 0usize;
    let mut args: Vec<Value<'a>> = Vec::new();
    let mut byval: Vec<(usize, Ty)> = Vec::new();
    if memory_ret {
        args.push(kelp.get_param(0));
        next = 1;
    }
    for arg in &plan.params {
        let incoming = kelp.get_param(next);
        next += 1;
        match arg {
            Arg::Scalar(ty) => {
                let value = match ty {
                    Ty::U8 | Ty::U16 | Ty::Bool => b.build_int_cast(false, true, incoming, ctx.i32(), "arg.widen"),
                    _ => incoming,
                };
                args.push(value);
            }
            Arg::Memory(ty) => {
                byval.push((args.len(), ty.clone()));
                args.push(incoming);
            }
            Arg::Regs(classes) => {
                let slot = b.build_alloca(ctx.array_type(ctx.i64(), classes.len() as u64), "agg.arg");
                b.build_store(incoming, slot);
                for (i, class) in classes.iter().enumerate() {
                    let at = b.build_gep(ctx.i64(), slot, ctx.const_int(ctx.i64(), i as u64, false), "agg.eb");
                    args.push(b.build_load(eightbyte(ctx, *class), at, "agg.word"));
                }
            }
        }
    }
    // A call that returns nothing takes no name: LLVM rejects a name on a void value.
    let call = b.build_call(native_fn, native_ty, &args, "");
    for (pos, ty) in &byval {
        add_byval(ctx, call, *pos, lty(ty)?);
    }

    match &plan.ret {
        Ret::Void | Ret::Memory => {
            b.build_ret_void();
        }
        Ret::Scalar(ty) => {
            let value = match ty {
                Ty::U8 | Ty::U16 | Ty::Bool => {
                    let target = scalar_llvm(ctx, ty).ok_or_else(missing)?;
                    b.build_int_cast(false, false, call, target, "ret.narrow")
                }
                _ => call,
            };
            b.build_ret(value);
        }
        Ret::Regs(ty, classes) => {
            let slot = b.build_alloca(ctx.array_type(ctx.i64(), classes.len() as u64), "agg.ret");
            if classes.len() == 1 {
                b.build_store(call, slot);
            } else {
                for i in 0..2u32 {
                    let part = b.build_extract_value(call, i, "agg.part");
                    let at = b.build_gep(ctx.i64(), slot, ctx.const_int(ctx.i64(), u64::from(i), false), "agg.eb");
                    b.build_store(part, at);
                }
            }
            let value = b.build_load(lty(ty)?, slot, "agg.value");
            b.build_ret(value);
        }
    }
    Ok(())
}
