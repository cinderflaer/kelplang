use crate::llvm_ffi::{Context, Module};
use crate::types::Ty;

/// The C-boundary rule (BUG-vN4tR, MK7 3.3): an aggregate over two
/// eightbytes crosses a C boundary only through a pointer. The threshold is
/// the target's own data layout, so exactly sixteen bytes is not over it and
/// a three-field `datac` is. `declare_extern` and `rt_arg_value` both ask
/// this one function, so this is the rule the two sites share.
#[test]
fn an_aggregate_over_two_eightbytes_crosses_a_c_boundary_by_pointer() {
    use super::crosses_c_boundary_by_value;
    use crate::llvm_ffi::{initialize_native_target, TargetMachine};

    initialize_native_target().expect("the native target");
    let machine = TargetMachine::for_host().expect("a target machine for the host");
    let layout = machine.data_layout();
    let ctx = Context::new();
    let view = ctx.struct_type("kelp_View_crossing_test");
    ctx.struct_set_body(view, &[ctx.opaque_ptr(), ctx.i64()]);
    let pair = ctx.struct_type("kelp_Pair_crossing_test");
    ctx.struct_set_body(pair, &[ctx.i64(), ctx.i64()]);
    let wide = ctx.struct_type("kelp_Wide_crossing_test");
    ctx.struct_set_body(wide, &[ctx.i64(), ctx.i64(), ctx.i64()]);

    assert!(!crosses_c_boundary_by_value(&Ty::View, view, &layout));
    assert!(!crosses_c_boundary_by_value(
        &Ty::Struct("Pair".to_string()),
        pair,
        &layout
    ));
    assert!(crosses_c_boundary_by_value(
        &Ty::Struct("Wide".to_string()),
        wide,
        &layout
    ));
}

/// The sub-word C ABI (BUG-rK2mW): every declaration this backend makes for
/// a body compiled as C carries the `zeroext` clang gives the same narrow
/// parameter or return. Without it the argument register's upper bits reach
/// the body untruncated, which is how a port gets read out of the padding
/// beside the `u16` field of a `datac`.
///
/// The two real tables are asked rather than a re-spelled prototype, so the
/// test fails when a new narrow helper is added there without the
/// attribute: `declare_runtime` is where `kelp_Bytes__set`'s byte parameter
/// and its narrow returns come from, and `declare_cap_fns` where the two
/// `uint16_t` ports do. `extend_narrow_c_abi` is the shared seam
/// `declare_extern` calls for a program's own `extern fun`, and the return
/// half is covered with it because the runtime's own `i8`/`i1` returns are.
#[test]
fn a_sub_word_c_parameter_or_return_carries_its_extension() {
    use super::{declare_cap_fns, declare_runtime, extend_narrow_c_abi};

    let ctx = Context::new();
    let module = Module::new(&ctx, "narrow_c_abi");
    let ptr = ctx.opaque_ptr();
    let bytes = ctx.struct_type("kelp_Bytes");
    ctx.struct_set_body(bytes, &[ptr, ctx.i64(), ctx.i64()]);
    let string = ctx.struct_type("kelp_String");
    ctx.struct_set_body(string, &[ptr, ctx.i64(), ctx.i64()]);
    let view = ctx.struct_type("kelp_View");
    ctx.struct_set_body(view, &[ptr, ctx.i64()]);
    declare_runtime(&ctx, &module, bytes, string, view, false, false, false);
    declare_cap_fns(&ctx, &module);

    // `declare_extern`'s half of the rule, with the two narrow shapes a
    // program's own `extern fun` can spell: a sub-word return and a
    // sub-word parameter. The `sret` form is not spelled here because a
    // return that needs it is an aggregate, and an aggregate is exactly
    // what takes no extension.
    let narrow_ret = ctx.i16();
    let narrow_params = [ptr, ctx.i8(), ctx.i1()];
    let probe = module.add_function(
        "probe_narrow_extern",
        ctx.fn_type(narrow_ret, &narrow_params, false),
    );
    extend_narrow_c_abi(probe, narrow_ret, &narrow_params);

    let ir = module.to_ir_string();
    for want in [
        // The return half: a body returning a `bool`, a byte or a `u16`
        // extends it before it returns, and a caller without the attribute
        // reads the register's upper bits as part of the value.
        "declare zeroext i1 @kelp_Bytes__is_empty(ptr)",
        "declare zeroext i8 @kelp_Bytes__get(ptr, i64)",
        // `declare_runtime`'s parameters.
        "declare void @kelp_Bytes__set(ptr, i64, i8 zeroext)",
        "declare void @kelp_Bytes__push(ptr, i8 zeroext)",
        // `declare_cap_fns`'s two `uint16_t` ports, the parameter the port
        // in `datac Url` reaches through `net.connect`.
        "declare void @kelp_cap_check_hostport(ptr, ptr, i16 zeroext, ptr, i64, ptr, ptr)",
        "declare void @kelp_cap_check_port(ptr, i16 zeroext, ptr, i64, ptr, ptr)",
        // `declare_extern`'s seam: the return and each sub-word parameter.
        "declare zeroext i16 @probe_narrow_extern(ptr, i8 zeroext, i1 zeroext)",
    ] {
        assert!(ir.contains(want), "missing '{want}':\n{ir}");
    }
    // The attribute is the sub-word rule, not "every parameter": a wider
    // integer and a pointer take none.
    assert!(!ir.contains("i64 zeroext"), "an i64 took zeroext:\n{ir}");
    assert!(!ir.contains("ptr zeroext"), "a pointer took zeroext:\n{ir}");
}

/// Kernel stage 10: the `_start` stub is chosen by the manifest's triple,
/// and a triple with no stub is refused by name rather than emitted as
/// another architecture's entry point.
#[test]
fn the_freestanding_entry_stub_is_per_architecture() {
    let riscv = super::freestanding_entry_asm("riscv64-unknown-none-elf", "kelp_kmain")
        .expect("riscv64 has a stub");
    assert!(riscv.contains("la sp, __stack_top"), "{riscv}");
    assert!(riscv.contains("wfi"), "{riscv}");

    let arm = super::freestanding_entry_asm("aarch64-unknown-none", "kelp_kmain")
        .expect("aarch64 has a stub");
    assert!(arm.contains("ldr x0, =__stack_top"), "{arm}");
    assert!(arm.contains("mov sp, x0"), "{arm}");
    assert!(arm.contains("str xzr, [x0], #8"), "{arm}");
    assert!(arm.contains("bl kelp_kmain"), "{arm}");
    assert!(arm.contains("wfe"), "{arm}");

    let x86 = super::freestanding_entry_asm("x86_64-unknown-none", "kelp_kmain")
        .expect("x86_64 has a stub");
    assert!(x86.contains(".note.Xen"), "{x86}");
    assert!(x86.contains(".code32"), "{x86}");
    assert!(x86.contains(".code64"), "{x86}");
    assert!(x86.contains("rep stosl"), "{x86}");
    assert!(x86.contains("rep stosq"), "{x86}");
    assert!(x86.contains("call kelp_kmain"), "{x86}");
    assert!(x86.contains("hlt"), "{x86}");
    // The `-elf` spelling of the same triple takes the same branch.
    let x86_elf = super::freestanding_entry_asm("x86_64-unknown-none-elf", "kelp_kmain")
        .expect("the -elf spelling has the same prefix");
    assert_eq!(x86_elf, x86);

    let refused = super::freestanding_entry_asm("i686-unknown-none", "kelp_kmain")
        .expect_err("x86-32 has no stub");
    assert!(refused.contains("i686-unknown-none"), "{refused}");
}
