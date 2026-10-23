use super::*;

/// Every mangling the backends already depend on survives, because the
/// symbols of a released build are spelled with them.
#[test]
fn the_existing_mappings_are_unchanged() {
    assert_eq!(c_ident("shapes.circle.Circle"), "shapes__circle__Circle");
    assert_eq!(c_ident("Type#name"), "Type__name");
    assert_eq!(c_ident("len@i32"), "len__ov_i32");
    assert_eq!(c_ident("Pair<i32, String>"), "Pair__l__i32_String__g__");
    assert_eq!(
        c_ident("Holder<fun(i32) => i32>"),
        "Holder__l__fun__lp__i32__rp____x3d__g__i32__g__"
    );
}

/// `int` is its own integer type (issue #275): it is named `int`, it is not
/// `i64` or `i32` under another name, it has the bounds of a 64-bit signed
/// integer on a 64-bit target, and its width follows the target triple.
#[test]
fn int_is_a_distinct_integer_of_the_targets_native_width() {
    assert_eq!(Ty::from_name("int"), Some(Ty::Int));
    assert_eq!(Ty::Int.display(), "int");
    assert_ne!(Ty::Int, Ty::I64);
    assert_ne!(Ty::Int, Ty::I32);
    assert!(Ty::Int.is_integer() && Ty::Int.is_numeric());
    assert_eq!(Ty::int_bounds(&Ty::Int), Ty::int_bounds(&Ty::I64));
    assert_eq!(native_int_bits_for_triple("x86_64-unknown-linux-gnu"), 64);
    assert_eq!(native_int_bits_for_triple("aarch64-apple-darwin"), 64);
    assert_eq!(native_int_bits_for_triple("i686-unknown-linux-gnu"), 32);
    assert_eq!(native_int_bits_for_triple("riscv32-unknown-elf"), 32);
    assert_eq!(native_int_bits_for_triple("wasm32-unknown-unknown"), 32);
}

/// BUG-zR8Wm: a module name is a file stem, and a file stem may hold any
/// byte but `/` and NUL, so the result has to be a C identifier for a name
/// no mapping above names. A byte outside `[A-Za-z0-9_]` gets its own
/// spelling, and two different bytes never share one.
#[test]
fn a_name_outside_the_mappings_comes_out_a_c_identifier() {
    assert_eq!(c_ident("we\"ird"), "we__x22ird");
    assert_eq!(c_ident("we\\ird"), "we__x5cird");
    assert_ne!(c_ident("we\"ird"), c_ident("we\\ird"));
    for name in ["we\"ird", "we\\ird", "wéird", "a!b", "semi;colon", "[x]"] {
        assert!(
            c_ident(name).bytes().all(is_ident_byte),
            "{name:?} mangled to {:?}",
            c_ident(name)
        );
    }
}

/// Gap 7's allowed set: every integer, `bool`, `f32`/`f64`, `str` (owned
/// or borrowed) and an opaque handle. A `char`, an owned buffer, a
/// container and an aggregate are not, and neither is a borrow of any of
/// those, because a native `va_arg` cannot read one back.
#[test]
fn the_variadic_extra_argument_types_are_the_ones_a_va_arg_can_read() {
    for ty in [
        Ty::I32,
        Ty::I64,
        Ty::Int,
        Ty::U32,
        Ty::U64,
        Ty::U8,
        Ty::U16,
        Ty::Usize,
        Ty::Isize,
        Ty::Bool,
        Ty::F32,
        Ty::F64,
        Ty::Str,
        Ty::Ref { mutable: false, inner: Box::new(Ty::Str) },
        Ty::Handle("helper.Widget".to_string()),
    ] {
        assert!(is_variadic_abi_ty(&ty), "{}", ty.display());
    }
    for ty in [
        Ty::Char,
        Ty::Unit,
        Ty::String,
        Ty::Bytes,
        Ty::List(Box::new(Ty::I32)),
        Ty::Ref { mutable: false, inner: Box::new(Ty::I32) },
    ] {
        assert!(!is_variadic_abi_ty(&ty), "{}", ty.display());
    }
}
