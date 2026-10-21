use super::*;

/// Declare the libc primitives the container function sets reach, exactly as
/// the earlier emitter reaches them through the standard headers. The signatures are
/// the ones `<stdlib.h>`/`<string.h>` declare, on the ABI this machine uses:
/// `size_t` is `i64` and `int` is `i32`.
/// The argument indices one capability check reads, in the order the check
/// reads them. the earlier emitter's `checked_arg_flags` enumerates the same set; this
/// one keeps the order, which is what the two-argument `HostPort` check needs.
pub(crate) fn cap_check_indices(check: &crate::stdlib::CapCheck) -> Vec<usize> {
    use crate::stdlib::CapCheck;
    match *check {
        CapCheck::Path(i) | CapCheck::Name(i) | CapCheck::Host(i) | CapCheck::Port(i) => vec![i],
        CapCheck::HostPort(host, port) => vec![host, port],
    }
}

/// The symbol of the compiled-in allowlist array for one `(module, domain)`
/// grant, spelled exactly as `the earlier emitter's `CapTable`::symbol` spells it: the
/// array is C data in the per-program object, and the IR declaration here has to
/// name the same symbol the object defines.
pub(crate) fn cap_array_symbol(module: &str, domain: &str) -> String {
    format!(
        "kelp_caps_{}_{}",
        crate::types::c_ident(&module.replace('/', "_")),
        domain
    )
}

/// Declare the capability check's C entry points, with the signatures
/// `the runtime capability glue` defines.
///
/// The resource is a `const char*` for every check but the two that name a
/// port, where it is the `uint16_t` the grant is matched on. `count` is a
/// `size_t`, so `i64`. A declaration is free: the object only defines these when
/// the program contains a gated call, and an unreferenced declaration leaves no
/// undefined symbol behind.
pub(crate) fn declare_cap_fns<'a>(
    ctx: &'a Context,
    module: &'a Module<'a>,
) -> HashMap<&'static str, (LlvmFunction<'a>, FnType<'a>)> {
    let ptr = ctx.opaque_ptr();
    let i16t = ctx.i16();
    let i64t = ctx.i64();
    let void = ctx.void();
    let mut out = HashMap::new();
    let mut decl = |name: &'static str, params: &[Type<'a>]| {
        let ty = ctx.fn_type(void, params, false);
        let func = module.add_function(name, ty);
        extend_narrow_c_abi(func, void, params);
        out.insert(name, (func, ty));
    };
    let text = [ptr, ptr, ptr, i64t, ptr, ptr];
    decl("kelp_cap_check_path", &text);
    decl("kelp_cap_check_name", &text);
    decl("kelp_cap_check_host", &text);
    decl("kelp_cap_check_hostport", &[ptr, ptr, i16t, ptr, i64t, ptr, ptr]);
    decl("kelp_cap_check_port", &[ptr, i16t, ptr, i64t, ptr, ptr]);
    // Declared so the two `CapCheck::Name` fast paths in `emit_cap_checks` can
    // call straight into the same denial `kelp_cap_check_name`'s own loop
    // calls, rather than duplicating its message or its `exit(1)`. Every
    // other check kind still denies from inside its own runtime call and
    // never reaches this declaration directly.
    decl("kelp_cap_deny", &text);
    out
}

pub(crate) fn declare_libc<'a>(
    ctx: &'a Context,
    module: &'a Module<'a>,
) -> HashMap<&'static str, (LlvmFunction<'a>, FnType<'a>)> {
    let ptr = ctx.opaque_ptr();
    let i32t = ctx.i32();
    let i64t = ctx.i64();
    let void = ctx.void();
    let mut out = HashMap::new();
    let mut decl = |name: &'static str, ret: Type<'a>, params: &[Type<'a>]| {
        let ty = ctx.fn_type(ret, params, false);
        let func = module.add_function(name, ty);
        out.insert(name, (func, ty));
    };
    decl("malloc", ptr, &[i64t]);
    decl("realloc", ptr, &[ptr, i64t]);
    decl("free", void, &[ptr]);
    // The Box small-block free lists (speed-alloc, the runtime crate): the runtime
    // object defines them, so they are declared with the libc surface.
    decl("kelp_box_pool", ptr, &[]);
    decl("kelp_box_alloc", ptr, &[i64t, i64t]);
    decl("kelp_box_free", void, &[ptr, i64t, i64t]);
    decl("memcpy", ptr, &[ptr, ptr, i64t]);
    decl("memcmp", i32t, &[ptr, ptr, i64t]);
    decl("memset", ptr, &[ptr, i32t, i64t]);
    decl("strlen", i64t, &[ptr]);
    // The one registration an owned `pd` has: its drop point is process exit
    // (the passdown globals emitter), so the generated `kelp_init_pds`
    // hands `kelp_drop_pds` to `atexit`. Declared unconditionally like every
    // other libc symbol here; an unused declaration costs nothing.
    decl("atexit", i32t, &[ptr]);
    out
}

/// The two memory primitives a freestanding build has no libc for: `memcpy` and
/// `memset`, defined in the module with internal linkage (kernel stage 7). The
/// container bodies call them through [`Gen::mem_fn`], so a freestanding image
/// references no libc symbol. `intrinsic` forms are avoided deliberately: the
/// bodies use volatile accesses, which LLVM's loop-idiom pass will not turn into
/// an `llvm.memcpy`/`llvm.memset` call, so the RISC-V backend never emits a
/// libcall to a libc symbol that is not there.
pub(crate) fn define_freestanding_mem<'a>(
    ctx: &'a Context,
    module: &'a Module<'a>,
) -> HashMap<&'static str, (LlvmFunction<'a>, FnType<'a>)> {
    let ptr = ctx.opaque_ptr();
    let i8t = ctx.i8();
    let i32t = ctx.i32();
    let i64t = ctx.i64();
    let void = ctx.void();
    let mut out = HashMap::new();
    let builder = Builder::new(ctx);

    // `memcpy(ptr dst, ptr src, usize n)`, one volatile byte at a time.
    {
        let ty = ctx.fn_type(void, &[ptr, ptr, i64t], false);
        // The `__` prefix keeps the name out of the `kelp_`-prefixed namespace
        // a user function is mangled into, so a program that declares its own
        // `memcpy` cannot collide with this definition.
        let func = module.add_function("__kelp_freestanding_memcpy", ty);
        func.as_value().set_internal_linkage();
        let entry = func.append_block("entry");
        builder.position_at_end(entry);
        let (dst, src, n) = (func.get_param(0), func.get_param(1), func.get_param(2));
        let i_slot = builder.build_alloca(i64t, "i");
        builder.build_store(ctx.const_int(i64t, 0, false), i_slot);
        let header = func.append_block("cond");
        builder.build_br(header);
        builder.position_at_end(header);
        let i = builder.build_load(i64t, i_slot, "i");
        let more = builder.build_icmp(IntCmp::Ult, i, n, "more");
        let body = func.append_block("body");
        let exit = func.append_block("exit");
        builder.build_cond_br(more, body, exit);
        builder.position_at_end(body);
        let sp = builder.build_gep(i8t, src, i, "src");
        let dp = builder.build_gep(i8t, dst, i, "dst");
        let byte = builder.build_load(i8t, sp, "byte");
        builder.set_volatile(byte);
        let store = builder.build_store(byte, dp);
        builder.set_volatile(store);
        let next = builder.build_int_binop(IntBinOp::Add, i, ctx.const_int(i64t, 1, false), "i1");
        builder.build_store(next, i_slot);
        builder.build_br(header);
        builder.position_at_end(exit);
        builder.build_ret_void();
        out.insert("memcpy", (func, ty));
    }

    // `memset(ptr dst, int value, usize n)`, the same one volatile byte at a
    // time. The `i32` value is the libc signature's, so the call sites do not
    // change; only the low byte is stored.
    {
        let ty = ctx.fn_type(void, &[ptr, i32t, i64t], false);
        let func = module.add_function("__kelp_freestanding_memset", ty);
        func.as_value().set_internal_linkage();
        let entry = func.append_block("entry");
        builder.position_at_end(entry);
        let (dst, value, n) = (func.get_param(0), func.get_param(1), func.get_param(2));
        let byte = builder.build_int_cast(false, false, value, i8t, "byte");
        let i_slot = builder.build_alloca(i64t, "i");
        builder.build_store(ctx.const_int(i64t, 0, false), i_slot);
        let header = func.append_block("cond");
        builder.build_br(header);
        builder.position_at_end(header);
        let i = builder.build_load(i64t, i_slot, "i");
        let more = builder.build_icmp(IntCmp::Ult, i, n, "more");
        let body = func.append_block("body");
        let exit = func.append_block("exit");
        builder.build_cond_br(more, body, exit);
        builder.position_at_end(body);
        let dp = builder.build_gep(i8t, dst, i, "dst");
        let store = builder.build_store(byte, dp);
        builder.set_volatile(store);
        let next = builder.build_int_binop(IntBinOp::Add, i, ctx.const_int(i64t, 1, false), "i1");
        builder.build_store(next, i_slot);
        builder.build_br(header);
        builder.position_at_end(exit);
        builder.build_ret_void();
        out.insert("memset", (func, ty));
    }

    out
}
