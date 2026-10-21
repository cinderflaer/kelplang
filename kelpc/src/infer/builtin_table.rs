use super::*;

/// A compiler-provided function (M6). These live in `sigs`' namespace (keyed
/// `Type#name`, exactly like a receiver method) but their bodies are emitted
/// by the removed C backend from the fixed runtime preamble, so nothing about call
/// resolution, argument matching, or passdown wiring needs to know they are
/// special - only codegen, and only in that the C function it names already
/// exists.
pub(crate) struct Builtin {
    pub(crate) params: Vec<(String, Ty)>,
    pub(crate) ret: Ty,
    /// `Bytes.empty()` - an *associated* function written on the type name,
    /// with no receiver argument - versus `b.len()`, which takes one.
    pub(crate) associated: bool,
}

/// The `Bytes` surface (M6). Kept deliberately small: construct, measure,
/// index, mutate, copy. Everything else about `Bytes` needs either `String`
/// or growable buffers, which are later milestones.
///
/// `List<T>` (collections milestone) is deliberately *not* in this static
/// table: its signatures depend on the element type, so they are built on
/// demand by `list_sig` below, keyed by the same `List#name` spelling.
pub(crate) static BUILTINS: std::sync::LazyLock<HashMap<String, Builtin>> =
    std::sync::LazyLock::new(|| {
        let bytes = Ty::Bytes;
        let shared = Ty::Ref { mutable: false, inner: Box::new(bytes.clone()) };
        let exclusive = Ty::Ref { mutable: true, inner: Box::new(bytes.clone()) };
        let mut m = HashMap::new();
        let mut add = |key: &str, params: Vec<(String, Ty)>, ret: Ty, associated: bool| {
            m.insert(key.to_string(), Builtin { params, ret, associated });
        };
        add("Bytes#empty", vec![], bytes.clone(), true);
        add("Bytes#from_str", vec![("s".to_string(), Ty::Str)], bytes.clone(), true);
        add("Bytes#len", vec![("self".to_string(), shared.clone())], Ty::Usize, false);
        add("Bytes#is_empty", vec![("self".to_string(), shared.clone())], Ty::Bool, false);
        add(
            "Bytes#get",
            vec![("self".to_string(), shared.clone()), ("i".to_string(), Ty::Usize)],
            Ty::U8,
            false,
        );
        add(
            "Bytes#set",
            vec![
                ("self".to_string(), exclusive),
                ("i".to_string(), Ty::Usize),
                ("b".to_string(), Ty::U8),
            ],
            Ty::Unit,
            false,
        );
        add("Bytes#clone", vec![("self".to_string(), shared)], bytes, false);

        // Growing a `Bytes` in place (S1). `push`/`append` need an exclusive
        // borrow, so the borrow checker refuses to grow a buffer while a
        // `str`/`&Bytes` view of it is alive - which is what makes the
        // reallocation sound without a lifetime syntax (see the bump of
        // `cap` in the runtime preamble).
        let bytes_exclusive = Ty::Ref { mutable: true, inner: Box::new(Ty::Bytes) };
        add(
            "Bytes#push",
            vec![("self".to_string(), bytes_exclusive.clone()), ("b".to_string(), Ty::U8)],
            Ty::Unit,
            false,
        );
        add(
            "Bytes#append",
            vec![
                ("self".to_string(), bytes_exclusive.clone()),
                ("other".to_string(), Ty::Ref { mutable: false, inner: Box::new(Ty::Bytes) }),
            ],
            Ty::Unit,
            false,
        );
        add(
            "Bytes#reserve",
            vec![("self".to_string(), bytes_exclusive.clone()), ("extra".to_string(), Ty::Usize)],
            Ty::Unit,
            false,
        );
        add("Bytes#clear", vec![("self".to_string(), bytes_exclusive)], Ty::Unit, false);

        // Byte-order reads and writes on a byte buffer (kernel stage 3,
        // MK9-S8): `b.read_u16_le(offset)` and its siblings, one name per
        // width and byte order, each bounds-checked and with no alignment
        // requirement. The receiver is `&Bytes` for a read and `mut Bytes` for
        // a write, so a write goes through the same exclusive borrow `push`
        // takes; every offset is a `usize`. `u8` needs no reader (`Bytes.get`
        // is one), `i16` does not exist, and `usize`/`isize` are absent because
        // a wire format fixes its field widths rather than the host's pointer
        // width.
        let bytes_shared = Ty::Ref { mutable: false, inner: Box::new(Ty::Bytes) };
        let bytes_mut = Ty::Ref { mutable: true, inner: Box::new(Ty::Bytes) };
        for value in ["u16", "u32", "u64", "i32", "i64"] {
            let ty = Ty::from_name(value).expect("integer type");
            for order in ["le", "be"] {
                add(
                    &format!("Bytes#read_{value}_{order}"),
                    vec![
                        ("self".to_string(), bytes_shared.clone()),
                        ("offset".to_string(), Ty::Usize),
                    ],
                    ty.clone(),
                    false,
                );
                add(
                    &format!("Bytes#write_{value}_{order}"),
                    vec![
                        ("self".to_string(), bytes_mut.clone()),
                        ("offset".to_string(), Ty::Usize),
                        ("value".to_string(), ty.clone()),
                    ],
                    Ty::Unit,
                    false,
                );
            }
        }

        // The `String` surface (M6.2): construct, measure, copy, and convert
        // to/from `Bytes` with an ownership transfer. `as_str`/`as_bytes`
        // return a borrow of the receiver, which needs the `&str`
        // return-elision rule (M6.3) before it can be typed at all.
        let string = Ty::String;
        let string_shared = Ty::Ref { mutable: false, inner: Box::new(string.clone()) };
        add("String#empty", vec![], string.clone(), true);
        add("String#from_str", vec![("s".to_string(), Ty::Str)], string.clone(), true);
        // By value: `from_bytes` takes ownership of the buffer, so the
        // receiver local is moved into this call rather than borrowed.
        add("String#from_bytes", vec![("b".to_string(), Ty::Bytes)], string.clone(), true);
        add("String#len", vec![("self".to_string(), string_shared.clone())], Ty::Usize, false);
        add(
            "String#char_count",
            vec![("self".to_string(), string_shared.clone())],
            Ty::Usize,
            false,
        );
        add("String#is_empty", vec![("self".to_string(), string_shared.clone())], Ty::Bool, false);
        add("String#clone", vec![("self".to_string(), string_shared.clone())], string.clone(), false);
        add(
            "String#into_bytes",
            vec![("self".to_string(), string.clone())],
            Ty::Bytes,
            false,
        );
        // A borrowed view of an owned buffer (M6.3). Both are the return-
        // borrow form: the result borrows from `self`, which is the whole
        // point of `&str` return elision.
        let str_ref = Ty::Ref {
            mutable: false,
            inner: Box::new(Ty::Str),
        };
        add(
            "String#as_str",
            vec![("self".to_string(), string_shared.clone())],
            str_ref.clone(),
            false,
        );
        add(
            "Bytes#as_str",
            vec![(
                "self".to_string(),
                Ty::Ref {
                    mutable: false,
                    inner: Box::new(Ty::Bytes),
                },
            )],
            str_ref,
            false,
        );

        // A length-carrying view of text (A-8). The two constructors are the
        // only places a length is *derived* (`str.view` runs one `strlen`,
        // `String.view` reads the length it already has); every operation on a
        // `View` is length-aware, and none of them is a C string function - a
        // slice has no NUL after it, so `strlen`/`strcmp` on one would read
        // past the slice and report a different string than the program has.
        // That is also why none of these takes or returns a `str`.
        //
        // `self` is by value for every one of them: a `View` is two words of
        // `Copy` data (a pointer and a length), so a borrow would add an
        // indirection and buy nothing - the same choice the integer
        // conversions make.
        add("str#view", vec![("self".to_string(), Ty::Str)], Ty::View, false);
        add(
            "String#view",
            vec![("self".to_string(), string_shared.clone())],
            Ty::View,
            false,
        );
        add("View#len", vec![("self".to_string(), Ty::View)], Ty::Usize, false);
        add("View#is_empty", vec![("self".to_string(), Ty::View)], Ty::Bool, false);
        add(
            "View#get",
            vec![("self".to_string(), Ty::View), ("i".to_string(), Ty::Usize)],
            Ty::U8,
            false,
        );
        add(
            "View#slice",
            vec![
                ("self".to_string(), Ty::View),
                ("from".to_string(), Ty::Usize),
                ("to".to_string(), Ty::Usize),
            ],
            Ty::View,
            false,
        );
        add(
            "View#eq",
            vec![("self".to_string(), Ty::View), ("other".to_string(), Ty::View)],
            Ty::Bool,
            false,
        );
        // The one way to make a `str`-shaped value out of a slice, and the
        // reason it exists rather than an implicit conversion: a copy is the
        // only safe answer when the source may not be terminated.
        add(
            "View#to_string",
            vec![("self".to_string(), Ty::View)],
            Ty::String,
            false,
        );

        // Growing a `String` in place (S1). The same exclusive-borrow rule
        // as `Bytes`, and the same NUL-termination invariant: `push_str`
        // appends a borrowed view, so a view of `self` would alias the
        // buffer `realloc` may move - the borrow checker rejects that.
        let string_exclusive = Ty::Ref { mutable: true, inner: Box::new(Ty::String) };
        add(
            "String#push",
            vec![("self".to_string(), string_exclusive.clone()), ("b".to_string(), Ty::U8)],
            Ty::Unit,
            false,
        );
        add(
            "String#push_str",
            vec![("self".to_string(), string_exclusive.clone()), ("s".to_string(), Ty::Str)],
            Ty::Unit,
            false,
        );
        add(
            "String#reserve",
            vec![("self".to_string(), string_exclusive.clone()), ("extra".to_string(), Ty::Usize)],
            Ty::Unit,
            false,
        );
        add("String#clear", vec![("self".to_string(), string_exclusive)], Ty::Unit, false);
        add("String#with_capacity", vec![("n".to_string(), Ty::Usize)], string.clone(), true);

        // Failing deliberately (S1): `kelp_panic`/`kelp_assert` already exist in
        // the runtime, these two entries are all that was missing. They are
        // free functions, not methods - the map key is a bare name, which is
        // the same namespace `print` occupies.
        add("panic", vec![("msg".to_string(), Ty::Str)], Ty::Unit, false);
        add("assert", vec![("cond".to_string(), Ty::Bool)], Ty::Unit, false);

        // `thread.fence` (MK9-S3-M1 stage A2). Its key is module-qualified
        // rather than bare, because the function is reached through an import
        // (`thread.fence`) and not as a prelude name, and the shipped
        // `std/thread` module declares nothing: a fence is one instruction on
        // either side (`__atomic_thread_fence` in C, `fence seq_cst` in IR)
        // with no C body to write, so the compiler lowers it and the module's
        // source documents the surface instead, exactly as `std/atomic`
        // documents the atomic methods it does not declare. (Stage A2 first
        // tried this as an `extern fun fence(order: atomic.Ordering)` and
        // BUG-kQ7Rv was the extern prelude having no typedef for the enum;
        // that prelude now writes one, and this stays a builtin for the
        // instruction reason above.)
        add(
            crate::stdlib::FENCE_FN,
            vec![("order".to_string(), atomic_order_ty())],
            Ty::Unit,
            false,
        );

        // Integer conversions (S1). Every integer type has a `to_<T>` for
        // every integer type, because nothing else in the language converts
        // one width to another: operands must match exactly, so a `u64 % 10`
        // digit cannot reach `String.push(u8)` and a `u8` byte cannot index
        // with `usize` without one. Widening is exact; a conversion that can
        // lose a value is a checked narrowing that panics with the call's own
        // name (v1 has no `Option`), which the removed C backend emitted inline.
        pub(crate) const INT_TYPES: &[&str] =
            &["i32", "i64", "u32", "u64", "u8", "u16", "usize", "isize"];
        for src in INT_TYPES {
            for dst in INT_TYPES {
                add(
                    &format!("{src}#to_{dst}"),
                    vec![("self".to_string(), Ty::from_name(src).expect("integer type"))],
                    Ty::from_name(dst).expect("integer type"),
                    false,
                );
            }
            // Integer -> float (the jsonlibary port's fourth gap). An integer
            // never mixes with a float by itself - `n * 1.0` is a type error,
            // because operands must match exactly - and before this the only
            // way to widen one was to format it and parse it back
            // (`str.parse_f64(fmt.int(n).as_str())`), which allocates a
            // `String` to add zero to a number. `to_f64` is the exact
            // widening the hardware already has; `to_f32` is the lossy one,
            // and it is *unchecked* on purpose, because C's float conversion
            // is defined for every input (a value too large becomes an
            // infinity), so there is no lost-value case to refuse.
            //
            // The reverse (a float `to_<int>`) is deliberately absent: it can
            // lose a value, and v1 has no `Option` for it to report the loss
            // with. Write `math.trunc`/`math.floor` first and then a checked
            // integer conversion.
            for dst in ["f64", "f32"] {
                add(
                    &format!("{src}#to_{dst}"),
                    vec![("self".to_string(), Ty::from_name(src).expect("integer type"))],
                    Ty::from_name(dst).expect("float type"),
                    false,
                );
            }
        }

        // Float width conversion: `f32.to_f64()` is exact, `f64.to_f32()` rounds
        // to the nearest `f32` (an infinity when the value is too large), and
        // neither can lose a value in the checked sense an integer narrowing
        // can, so both are unchecked like the integer-to-float pair above.
        add("f32#to_f64", vec![("self".to_string(), Ty::F32)], Ty::F64, false);
        add("f64#to_f32", vec![("self".to_string(), Ty::F64)], Ty::F32, false);

        // Byte-order conversion (kernel stage 3, MK9-S8 / roadmap B-10). Every
        // integer wider than a byte has `swap_bytes`, `to_be` and `to_le`, plus
        // the two static forms written on the type name
        // (`u32.from_be(x)`/`u32.from_le(x)`) that the associated-builtin path
        // above already resolves, the same way `Bytes.empty()` is resolved.
        //
        // `u8` is deliberately absent: swapping one byte is the identity, and
        // `to_be`/`to_le` on a byte are the identity on every target, so the
        // three names would only invite a reader to wonder which they meant.
        // `i16` is absent because v1 has no `i16` scalar to be one (the
        // `repr(i16)` tag storage in kernel stage 2 is a different thing).
        for src in ["u16", "u32", "u64", "i32", "i64", "usize", "isize"] {
            let ty = Ty::from_name(src).expect("integer type");
            for op in ["swap_bytes", "to_be", "to_le"] {
                add(
                    &format!("{src}#{op}"),
                    vec![("self".to_string(), ty.clone())],
                    ty.clone(),
                    false,
                );
            }
            for op in ["from_be", "from_le"] {
                add(
                    &format!("{src}#{op}"),
                    vec![("x".to_string(), ty.clone())],
                    ty.clone(),
                    true,
                );
            }
        }
        m
    });
