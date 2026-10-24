//! The program's `extern fun` object: the code a hosted build used to compile
//! from C text, lowered to LLVM IR and written by the program's own target
//! machine. A hosted build calls no C compiler.
//!
//! The object holds three things and nothing else:
//!
//! - a trampoline for each user `extern fun`, one whose module allowlisted its
//!   native symbol in `coral.toml`. The program's IR calls the trampoline with
//!   Kelp's own shapes, and the trampoline calls the native symbol;
//! - the native-callback glue for a `move closure fun(...)` parameter of a user
//!   `extern fun`: a thunk the native side calls as its function pointer, and a
//!   destructor it calls as its `destroy_notify`;
//! - each `(module, domain)` grant's allowlist array, which `kelp-rt`'s
//!   capability checks read by symbol (`kelp-rt/src/cap.rs`).
//!
//! The stdlib's `extern fun` bodies are Rust exports in `kelp-rt`, listed in
//! `externs::RUST_PORTED`, so this object defines nothing for them.
//!
//! A user `extern fun` that takes or returns a `datac`, `Bytes` or `String` by
//! value is lowered by `extern_agg`, into the program's own module, and is not
//! written here. Any other aggregate by value is refused. Scalars, pointers,
//! handles and callbacks cross as before.

use std::collections::HashMap;
use std::path::Path;

use crate::codegen_llvm::{
    cap_array_symbol, extend_narrow_c_abi, is_aggregate_ty, llvm_type, symbol_name,
};
use crate::llvm_ffi::{Builder, Context, FnType, Function, IntCmp, Module, TargetMachine, Type, Value};
use crate::tir::{Function as TirFunction, Program};
use crate::types::{closure_mangle, Ty};

/// Whether the program carries a `std/tls` extern, which is the one thing that
/// needs OpenSSL's headers at compile time and `-lssl -lcrypto` at link time.
/// `std/http` and `std/ws` import `std/tls`, so a program that uses either has
/// it too.
pub(crate) fn uses_tls(program: &Program) -> bool {
    program
        .functions
        .iter()
        .any(|f| f.extern_ && f.module == "std/tls")
}


/// The name of one closure signature's native-callback thunk (gap 4): the
/// `func` half of the `(func, user_data, destroy_notify)` triple.
pub(crate) fn callback_thunk_name(params: &[Ty], ret: &Ty) -> String {
    format!("kelp_cb_thunk__{}", closure_mangle(params, ret))
}

/// The name of one closure signature's callback destructor (gap 4): the
/// `destroy_notify` half, which drops the closure's environment and frees the
/// box that carried it.
pub(crate) fn callback_destroy_name(params: &[Ty], ret: &Ty) -> String {
    format!("kelp_cb_destroy__{}", closure_mangle(params, ret))
}

/// The closure signatures a program's user `extern fun`s take as native
/// callbacks, one per signature, in a deterministic order.
fn callback_signatures(program: &Program) -> Vec<Ty> {
    let mut sigs: Vec<Ty> = Vec::new();
    for f in &program.functions {
        if !f.extern_ || !program.extern_ffi_symbols.contains_key(&f.name) {
            continue;
        }
        for (_, ty) in &f.params {
            if matches!(ty, Ty::Closure { .. }) && !sigs.contains(ty) {
                sigs.push(ty.clone());
            }
        }
    }
    sigs.sort_by_key(|t| match t {
        Ty::Closure { params, ret } => closure_mangle(params, ret),
        _ => String::new(),
    });
    sigs
}

/// A closure value as the runtime lays it out: the function, its environment and
/// the environment's drop, three pointers in that order (24 bytes).
fn closure_box_ty(ctx: &Context) -> Type<'_> {
    let ptr = ctx.opaque_ptr();
    ctx.literal_struct(&[ptr, ptr, ptr])
}

/// The LLVM type of a user `extern fun` parameter or return that is passed as a
/// scalar or a pointer. Aggregates are refused: see the module comment.
fn scalar_ty<'a>(ctx: &'a Context, ty: &Ty, what: &str, name: &str) -> Result<Type<'a>, String> {
    if is_aggregate_ty(ty) {
        return Err(format!(
            "the user extern '{name}' takes or returns {what} '{}' by value, which the llvm backend cannot pass to a native symbol yet: pass a pointer or a handle instead",
            aggregate_word(ty)
        ));
    }
    llvm_type(ctx, &HashMap::new(), &HashMap::new(), ty)
}

fn aggregate_word(ty: &Ty) -> &'static str {
    match ty {
        Ty::Bytes => "bytes",
        Ty::String => "String",
        Ty::View => "View",
        Ty::Struct(_) => "a datac",
        Ty::List(_) => "List",
        Ty::Box(_) => "Box",
        Ty::Map(..) => "Map",
        Ty::Array(..) => "an array",
        Ty::Closure { .. } => "a closure",
        Ty::ScopedFn { .. } => "a scoped function",
        _ => "an aggregate",
    }
}

/// A narrow integer crosses a native call as the 32-bit register it travels in,
/// so the native declaration widens it and the result is narrowed back.
fn native_width(ty: Type<'_>) -> bool {
    matches!(ty.int_width(), Some(1 | 8 | 16))
}

/// The LLVM type a native parameter is declared with: a narrow integer widens to
/// `i32`, as the C prototype's `uint8_t` or `bool` travels in a register.
fn native_param<'a>(ctx: &'a Context, ty: Type<'a>) -> Type<'a> {
    if native_width(ty) {
        ctx.i32()
    } else {
        ty
    }
}

/// Declares `name` with `ty` once, reusing a declaration already in the module.
/// A user `extern fun` may bind a symbol the lowering also calls (`malloc`), and
/// LLVM would rename a second declaration of the same name.
fn declare<'a>(module: &Module<'a>, name: &str, ty: FnType<'a>) -> Function<'a> {
    module
        .get_function(name)
        .unwrap_or_else(|| module.add_function(name, ty))
}

/// Writes the program's externs object to `out` with `machine`, the target
/// machine the program's own object is written with. Returns `false` and writes
/// nothing when the program has no trampoline, callback or allowlist to emit.
///
/// `hosted` is set for a hosted program, whose functions carry unwind tables as
/// C functions do by default: a panic on a worker unwinds through them, and a
/// callback thunk is one of those frames.
pub(crate) fn lower_externs_object(
    program: &Program,
    machine: &TargetMachine,
    out: &Path,
    hosted: bool,
) -> Result<bool, String> {
    // A user extern that `extern_agg` lowers has its trampoline in the program's own
    // module, so this object leaves it out.
    let user: Vec<&TirFunction> = program
        .functions
        .iter()
        .filter(|f| {
            f.extern_
                && program.extern_ffi_symbols.contains_key(&f.name)
                && !crate::extern_agg::lowered_in_ir(program, f)
        })
        .collect();
    for f in program.functions.iter().filter(|f| f.extern_) {
        if !program.extern_ffi_symbols.contains_key(&f.name)
            && !crate::externs::is_rust_ported(&f.module, &f.name)
        {
            return Err(format!("no compiler-supplied body for extern '{}'", f.name));
        }
    }
    let callbacks = callback_signatures(program);
    // The allowlists are needed only by a program with a gated call site, the
    // same condition that decided the capability glue was emitted before.
    let caps: &[_] = if program.gated.is_empty() { &[] } else { &program.caps };
    if user.is_empty() && callbacks.is_empty() && caps.is_empty() {
        return Ok(false);
    }

    let ctx = Context::new();
    let mut module = Module::new(&ctx, "kelp_externs");
    module.set_triple(machine.triple());
    module.set_data_layout(&machine.data_layout());
    let ptr = ctx.opaque_ptr();
    let void = ctx.void();
    let i64t = ctx.i64();

    // `malloc` and `free` back the callback boxes, and `kelp_panic` reports an
    // allocation failure. The C lowering called the same three.
    let malloc_ty = ctx.fn_type(ptr, &[i64t], false);
    let malloc = declare(&module, "malloc", malloc_ty);
    let free_ty = ctx.fn_type(void, &[ptr], false);
    let free = declare(&module, "free", free_ty);
    let panic_ty = ctx.fn_type(void, &[ptr], false);
    let panic_fn = declare(&module, "kelp_panic", panic_ty);

    // Each closure signature's thunk and destructor, defined before the
    // trampolines name them.
    for sig in &callbacks {
        let Ty::Closure { params, ret } = sig else {
            unreachable!("callback signatures are closure types");
        };
        define_callback_glue(&ctx, &module, params, ret, free, free_ty)?;
    }

    for f in &user {
        let symbol = program
            .extern_ffi_symbols
            .get(&f.name)
            .expect("a user extern has a native symbol");
        define_trampoline(&ctx, &module, f, symbol, malloc, malloc_ty, panic_fn, panic_ty)?;
    }

    // Each grant's allowlist, as a NUL-terminated array of pattern strings with
    // a final NULL. `kelp-rt`'s check reads it by symbol.
    for (i, grant) in caps.iter().enumerate() {
        let symbol = cap_array_symbol(&grant.module, &grant.domain);
        let mut elems: Vec<Value<'_>> = Vec::new();
        for (j, pattern) in grant.patterns.iter().enumerate() {
            let text = ctx.const_c_string(pattern.as_bytes());
            let ty = ctx.array_type(ctx.i8(), pattern.len() as u64 + 1);
            let global = module.add_global(&format!("kelp_cap_str_{i}_{j}"), ty);
            global.set_initializer(text);
            global.set_constant(true);
            global.set_internal_linkage();
            elems.push(global);
        }
        elems.push(ctx.const_null(ptr));
        let arr_ty = ctx.array_type(ptr, elems.len() as u64);
        let array = module.add_global(&symbol, arr_ty);
        array.set_initializer(ctx.const_array(ptr, &elems));
        array.set_constant(true);
    }

    if hosted {
        module.set_unwind_tables();
    }
    module.verify()?;
    machine.emit_object(&module, out)?;
    Ok(true)
}

/// Defines one closure signature's thunk and destructor (gap 4).
///
/// The thunk is what a native `(func, user_data, destroy_notify)` triple calls.
/// GLib's convention puts `user_data` last, while a Kelp closure's `fn` takes its
/// environment first, so the thunk reorders them: `user_data` is the boxed
/// closure and the thunk calls `c->fn(c->env, a0, ...)`. The destructor runs the
/// closure's `drop` on the environment, if there is one, and frees the box.
fn define_callback_glue<'a>(
    ctx: &'a Context,
    module: &Module<'a>,
    params: &[Ty],
    ret: &Ty,
    free: Function<'a>,
    free_ty: FnType<'a>,
) -> Result<(), String> {
    let ptr = ctx.opaque_ptr();
    let void = ctx.void();
    // A `unit` argument has no value, so it takes no place in the thunk, as in
    // every other generated signature.
    let real: Vec<&Ty> = params.iter().filter(|p| **p != Ty::Unit).collect();
    let mut real_tys: Vec<Type<'a>> = Vec::new();
    for p in &real {
        real_tys.push(scalar_ty(ctx, p, "a callback parameter", "callback")?);
    }
    let ret_ty = if *ret == Ty::Unit {
        None
    } else {
        Some(scalar_ty(ctx, ret, "a callback return", "callback")?)
    };
    let ret_or_void = ret_ty.unwrap_or(void);

    let thunk_name = callback_thunk_name(params, ret);
    let mut thunk_params = real_tys.clone();
    thunk_params.push(ptr);
    let thunk_fn_ty = ctx.fn_type(ret_or_void, &thunk_params, false);
    let thunk = module.add_function(&thunk_name, thunk_fn_ty);
    extend_narrow_c_abi(thunk, ret_or_void, &thunk_params);
    thunk.as_value().set_internal_linkage();
    let builder = Builder::new(ctx);
    let entry = thunk.append_block("entry");
    builder.position_at_end(entry);
    let user_data = thunk.get_param(thunk_params.len() - 1);
    let boxed = closure_box_ty(ctx);
    let inner_params: Vec<Type<'a>> = std::iter::once(ptr).chain(real_tys.iter().copied()).collect();
    let inner_ty = ctx.fn_type(ret_or_void, &inner_params, false);
    let fn_slot = builder.build_struct_gep(boxed, user_data, 0, "fn_slot");
    let fn_ptr = builder.build_load(ptr, fn_slot, "fn");
    let env_slot = builder.build_struct_gep(boxed, user_data, 1, "env_slot");
    let env = builder.build_load(ptr, env_slot, "env");
    let mut args: Vec<Value<'a>> = vec![env];
    for i in 0..real_tys.len() {
        args.push(thunk.get_param(i));
    }
    // A void call cannot carry a name: LLVM rejects one.
    let result = builder.build_indirect_call(fn_ptr, inner_ty, &args, if ret_ty.is_some() { "r" } else { "" });
    match ret_ty {
        Some(_) => {
            builder.build_ret(result);
        }
        None => {
            builder.build_ret_void();
        }
    }

    let destroy_name = callback_destroy_name(params, ret);
    let destroy_ty = ctx.fn_type(void, &[ptr], false);
    let destroy = module.add_function(&destroy_name, destroy_ty);
    destroy.as_value().set_internal_linkage();
    let entry = destroy.append_block("entry");
    let drop_block = destroy.append_block("drop");
    let free_block = destroy.append_block("free");
    builder.position_at_end(entry);
    let cb = destroy.get_param(0);
    let env_slot = builder.build_struct_gep(boxed, cb, 1, "env_slot");
    let env = builder.build_load(ptr, env_slot, "env");
    let has_env = builder.build_icmp(IntCmp::Ne, env, ctx.const_null(ptr), "has_env");
    builder.build_cond_br(has_env, drop_block, free_block);
    builder.position_at_end(drop_block);
    let drop_slot = builder.build_struct_gep(boxed, cb, 2, "drop_slot");
    let drop_fn = builder.build_load(ptr, drop_slot, "drop");
    let drop_ty = ctx.fn_type(void, &[ptr], false);
    builder.build_indirect_call(drop_fn, drop_ty, &[env], "");
    builder.build_store(ctx.const_null(ptr), env_slot);
    builder.build_br(free_block);
    builder.position_at_end(free_block);
    builder.build_call(free, free_ty, &[cb], "");
    builder.build_ret_void();
    Ok(())
}

/// Defines one user `extern fun`'s trampoline: the function the program's IR
/// calls, under the name `symbol_name` gives it, which calls the native symbol.
#[allow(clippy::too_many_arguments)]
fn define_trampoline<'a>(
    ctx: &'a Context,
    module: &Module<'a>,
    f: &TirFunction,
    symbol: &str,
    malloc: Function<'a>,
    malloc_ty: FnType<'a>,
    panic_fn: Function<'a>,
    panic_ty: FnType<'a>,
) -> Result<(), String> {
    let ptr = ctx.opaque_ptr();
    let void = ctx.void();
    let ret_ty = if f.ret == Ty::Unit {
        None
    } else {
        Some(scalar_ty(ctx, &f.ret, "a return", &f.name)?)
    };
    let ret_or_void = ret_ty.unwrap_or(void);

    // The trampoline's own parameters, as the program's IR declares them: a
    // closure is a pointer to the caller's 24-byte slot.
    let mut params: Vec<Type<'a>> = Vec::new();
    let mut closure_at: Vec<bool> = Vec::new();
    for (pname, pty) in &f.params {
        if matches!(pty, Ty::Closure { .. }) {
            params.push(ptr);
            closure_at.push(true);
        } else {
            params.push(scalar_ty(ctx, pty, "a parameter", pname)?);
            closure_at.push(false);
        }
    }
    let trampoline_ty = ctx.fn_type(ret_or_void, &params, false);
    let trampoline = module.add_function(&symbol_name(&f.name), trampoline_ty);
    extend_narrow_c_abi(trampoline, ret_or_void, &params);

    // The native symbol, with a closure expanded into the triple it takes.
    let mut native_params: Vec<Type<'a>> = Vec::new();
    for (i, is_closure) in closure_at.iter().enumerate() {
        if *is_closure {
            native_params.extend([ptr, ptr, ptr]);
        } else {
            native_params.push(native_param(ctx, params[i]));
        }
    }
    let native_ret = ret_ty.map(|t| native_param(ctx, t)).unwrap_or(void);
    let native_ty = ctx.fn_type(native_ret, &native_params, false);
    let native = declare(module, symbol, native_ty);

    let builder = Builder::new(ctx);
    let entry = trampoline.append_block("entry");
    builder.position_at_end(entry);
    let mut args: Vec<Value<'a>> = Vec::new();
    for (i, (_, pty)) in f.params.iter().enumerate() {
        let value = trampoline.get_param(i);
        if let Ty::Closure { params: cp, ret: cr } = pty {
            // The box outlives this call, because the native side may keep the
            // `user_data` past it, so the closure is copied onto the heap.
            let boxed = builder.build_call(malloc, malloc_ty, &[ctx.const_int(ctx.i64(), 24, false)], "cb_box");
            let is_null = builder.build_icmp(IntCmp::Eq, boxed, ctx.const_null(ptr), "cb_null");
            let oom = trampoline.append_block(&format!("oom_{i}"));
            let ok = trampoline.append_block(&format!("cb_ok_{i}"));
            builder.build_cond_br(is_null, oom, ok);
            builder.position_at_end(oom);
            let msg = builder.global_string("out of memory", &format!("kelp_oom_{i}"));
            builder.build_call(panic_fn, panic_ty, &[msg], "");
            builder.build_unreachable();
            builder.position_at_end(ok);
            let closure = builder.build_load(closure_box_ty(ctx), value, &format!("cb_val_{i}"));
            builder.build_store(closure, boxed);
            let thunk = module
                .get_function(&callback_thunk_name(cp, cr))
                .expect("a callback's thunk is defined before its trampoline");
            let destroy = module
                .get_function(&callback_destroy_name(cp, cr))
                .expect("a callback's destructor is defined before its trampoline");
            args.push(thunk.as_value());
            args.push(boxed);
            args.push(destroy.as_value());
        } else if native_width(params[i]) {
            let widened = builder.build_int_cast(false, true, value, ctx.i32(), "widen");
            args.push(widened);
        } else {
            args.push(value);
        }
    }
    let call = builder.build_call(native, native_ty, &args, if ret_ty.is_some() { "native" } else { "" });
    match ret_ty {
        None => {
            builder.build_ret_void();
        }
        Some(t) if native_width(t) => {
            let narrowed = builder.build_int_cast(false, false, call, t, "narrow");
            builder.build_ret(narrowed);
        }
        Some(_) => {
            builder.build_ret(call);
        }
    }
    Ok(())
}
