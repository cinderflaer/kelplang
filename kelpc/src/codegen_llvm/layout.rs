use super::*;

/// The map's fields, in the order `write_container_typedef` declares them, and
/// one entry's three, in `write_map`'s order.
pub(crate) const M_ENTRIES: u32 = 0;
pub(crate) const M_ENTRY_COUNT: u32 = 1;
pub(crate) const M_ENTRY_CAP: u32 = 2;
pub(crate) const M_LEN: u32 = 3;
/// One index slot is a `u32` holding `entry index + 1`: `0` is empty, and this
/// is the tombstone a `remove` leaves behind. `SIZE_MAX` marks "no tombstone
/// seen yet" in `find`, which is a slot position and not a slot's content, so
/// the two sentinels are different widths on purpose.
pub(crate) const INDEX_TOMBSTONE: u64 = u32::MAX as u64;
pub(crate) const M_INDEX: u32 = 4;
pub(crate) const M_INDEX_CAP: u32 = 5;
pub(crate) const M_INDEX_USED: u32 = 6;
pub(crate) const E_KEY: u32 = 0;
pub(crate) const E_VALUE: u32 = 1;
pub(crate) const E_LIVE: u32 = 2;

// ---- The builtin heap layouts: one source of truth (WP5, stage 1 plan 3.2) ----
//
// `LAYOUTS` is the single list of the runtime's shared shapes. The IR struct
// bodies (`builtin_struct_types`) are generated from it. It uses only `core`
// types and `&'static` data, no kelpc types, so the Rust runtime's `abi` module
// can take this block verbatim; kelpc then reads the table through its path
// dependency on that module instead of this copy.

/// The IR width of one field: `Ptr` is an opaque pointer, `I64` a 64-bit integer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    Ptr,
    I64,
}

/// One field of a builtin layout: its IR width.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Field {
    pub kind: FieldKind,
}

/// One builtin heap layout: the key the compiler indexes it by, the name of its
/// IR struct type, and its fields in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub name: &'static str,
    pub ir_name: &'static str,
    pub fields: &'static [Field],
}

/// The builtin heap layouts, field for field the structs the runtime crate
/// defines (MK7 3.3): a pointer, a length and a capacity for `Bytes` and
/// `String`, and a pointer and a length for `View`.
pub const LAYOUTS: &[Layout] = &[
    Layout {
        name: "Bytes",
        ir_name: "kelp_Bytes",
        fields: &[
            Field { kind: FieldKind::Ptr },
            Field { kind: FieldKind::I64 },
            Field { kind: FieldKind::I64 },
        ],
    },
    Layout {
        name: "String",
        ir_name: "kelp_String",
        fields: &[
            Field { kind: FieldKind::Ptr },
            Field { kind: FieldKind::I64 },
            Field { kind: FieldKind::I64 },
        ],
    },
    Layout {
        name: "View",
        ir_name: "kelp_View",
        fields: &[
            Field { kind: FieldKind::Ptr },
            Field { kind: FieldKind::I64 },
        ],
    },
];

/// The IR struct type of every builtin layout, keyed by `Layout::name`, with its
/// body set. The names are the C typedef names, so a pointer to one crosses into
/// the runtime with the layout the object was compiled for (MK7 3.3).
pub(crate) fn builtin_struct_types<'a>(ctx: &'a Context) -> HashMap<String, Type<'a>> {
    let mut out = HashMap::new();
    for layout in LAYOUTS {
        let ty = ctx.struct_type(layout.ir_name);
        let fields: Vec<Type<'a>> = layout
            .fields
            .iter()
            .map(|field| match field.kind {
                FieldKind::Ptr => ctx.opaque_ptr(),
                FieldKind::I64 => ctx.i64(),
            })
            .collect();
        ctx.struct_set_body(ty, &fields);
        out.insert(layout.name.to_string(), ty);
    }
    out
}

/// Whether a `tir` type is lowered to an LLVM aggregate rather than a scalar or
/// a pointer. Only an aggregate can be too large to cross a C boundary by value,
/// which is the one thing the verifier cannot see (BUG-vN4tR).
pub(crate) fn is_aggregate_ty(ty: &Ty) -> bool {
    matches!(
        ty,
        Ty::Bytes
            | Ty::String
            | Ty::View
            | Ty::Struct(_)
            | Ty::List(_)
            | Ty::Box(_)
            | Ty::Map(..)
            | Ty::Array(..)
            | Ty::Closure { .. }
            | Ty::ScopedFn { .. }
    )
}

/// Whether a value of type `ty`, lowered to `llvm_ty`, is an aggregate over two
/// eightbytes: too large for the SysV ABI to pass in a register pair, so it can
/// cross a C boundary only through a pointer (BUG-vN4tR, MK7 §3.3).
///
/// This is the one name for that rule, and the two places the backend spells a
/// type across a C boundary both ask it, so they cannot drift:
///
/// * `declare_extern`: a parameter that is one is refused by name, a return that
///   is one is declared `void` with the destination as argument 0 (`sret`).
/// * `rt_arg_value`: a call argument that is one is staged into a slot and the
///   slot's address is what crosses.
///
/// `declare_runtime` spells its own 24-byte returns the same way with the `sret`
/// flag, and the `kelp_String__from_bytes_ptr`/`kelp_String__into_bytes_ptr`
/// adapters exist so no 24-byte argument needs the by-value form at all.
pub(crate) fn crosses_c_boundary_by_value(ty: &Ty, llvm_ty: Type<'_>, layout: &DataLayout) -> bool {
    is_aggregate_ty(ty) && layout.abi_size_of(llvm_ty) > 16
}

/// The C ABI's extension attribute for a parameter or a return declared with
/// the LLVM type `llvm_ty`, if it is a sub-word integer (BUG-rK2mW).
///
/// Thirty-two bits is the width a register argument travels in, so a narrower
/// integer leaves the register's upper bits to whatever held them, and the C
/// body's widening to `int` reads them as part of the value. clang annotates
/// every sub-word parameter and return of the body it compiles; this is the
/// same rule on the declaration this backend writes.
///
/// The signedness the variant carries is the C prototype's, and every narrow
/// prototype this backend declares is unsigned: `Ty::c_type` spells `u8` and
/// `u16` as `uint8_t` and `uint16_t` and `bool` as `bool`, and a Kelp `char` is
/// `uint32_t`. So the variant is always `ZeroExt` here; a signed narrow C type
/// would take `SignExt`, and the language has no type that reaches one.
pub(crate) fn c_int_extension(llvm_ty: Type<'_>) -> Option<IntExtension> {
    match llvm_ty.int_width()? {
        1 | 8 | 16 => Some(IntExtension::ZeroExt),
        _ => None,
    }
}

/// Give a C-boundary declaration the sub-word extension attributes clang would
/// give the same prototype (BUG-rK2mW).
///
/// Every declaration site that spells a prototype of a body compiled as C asks
/// this one function, so the three cannot drift: `declare_extern` for a
/// program's own `extern fun` bodies, `declare_runtime` for the runtime crate
/// helpers and `declare_cap_fns` for the capability checks of
/// `the runtime capability glue`. One call per declaration, with the same LLVM return
/// and parameter list the declaration was made from, and no call-site
/// attribute: LLVM's call lowering reads a direct callee's parameter
/// attributes, so the declaration is what an argument is extended to.
pub(crate) fn extend_narrow_c_abi(func: LlvmFunction<'_>, ret: Type<'_>, params: &[Type<'_>]) {
    if let Some(ext) = c_int_extension(ret) {
        func.add_return_extension(ext);
    }
    // The attribute index is the 1-based position in this same `params` slice,
    // so a leading `sret` pointer only shifts the indices after it and needs no
    // handling of its own.
    for (index, ty) in params.iter().enumerate() {
        if let Some(ext) = c_int_extension(*ty) {
            func.add_parameter_extension(index + 1, ext);
        }
    }
}

/// Declare every runtime helper this slice calls, with the signature
/// the runtime crate defines and the ABI shape it is compiled under (MK7 §3.3). The
/// object `build.rs` produces is compiled from that same text, so a signature
/// that drifts is caught by neither the verifier (which knows nothing of the
/// object) nor the linker at build time: it is caught only when a fixture runs. The
/// list is therefore exactly the surface the S2B fixtures reach: `int` arguments
/// are `i32`, `size_t` and `uint64_t` are `i64`, `uint8_t` is `i8`, `bool` is
/// `i1`; a `View` (two eightbytes) travels by value in registers both ways;
/// and a struct larger than that is returned through the hidden pointer this
/// module models as an explicit first argument (see [`RtFn::sret`]).
///
/// `kelp_String__into_bytes` is deliberately absent: it takes a 24-byte struct
/// *by value*, and LLVM's backend passes such an argument in registers while the
/// SysV ABI puts it on the stack, so a call to one would not cross correctly.
/// The runtime object carries pointer-spelled adapters for both directions
/// (`kelp_String__from_bytes_ptr`, `kelp_String__into_bytes_ptr`, BUG-vN4tR),
/// registered below under the builtin's own mangled name so a call site reaches
/// them through the same lookup every other runtime method uses. Because every
/// `String` construction in the stdlib goes through `from_bytes`, that direction
/// is not left out.
///
/// The one entry that is not a cross-object call: `kelp_buf_reserve`, which only
/// the `String` append bodies `Gen::emit_runtime_bodies` emits as IR reach.
pub(crate) fn declare_runtime<'a>(
    ctx: &'a Context,
    module: &'a Module<'a>,
    bytes: Type<'a>,
    string: Type<'a>,
    view: Type<'a>,
    managed: bool,
    spawn_frames: bool,
    blocks: bool,
) -> HashMap<String, RtFn<'a>> {
    let ptr = ctx.opaque_ptr();
    let i1 = ctx.i1();
    let i8 = ctx.i8();
    let i64 = ctx.i64();
    let void = ctx.void();
    let mut out: HashMap<String, RtFn<'a>> = HashMap::new();
    let decl = |out: &mut HashMap<String, RtFn<'a>>,
                name: &str,
                ret: Type<'a>,
                params: &[Type<'a>],
                yields: bool,
                sret: bool| {
        let (llvm_ret, mut all) = if sret {
            (void, vec![ptr])
        } else {
            (ret, Vec::new())
        };
        all.extend_from_slice(params);
        let ty = ctx.fn_type(llvm_ret, &all, false);
        let func = module.add_function(name, ty);
        extend_narrow_c_abi(func, llvm_ret, &all);
        out.insert(
            name.to_string(),
            RtFn {
                func,
                ty,
                yields,
                ret: if yields { Some(ret) } else { None },
                sret,
            },
        );
    };
    if blocks {
        // The manual block runtime (decision 139): a block is made by
        // `kelp_gc_block_alloc(descr, &base, &handle)`, which returns a status,
        // and released by `kelp_gc_block_free(handle, base, descr)`, which
        // does nothing for a null `base` (a block whose box never ran).
        decl(&mut out, "kelp_gc_block_alloc", ctx.i32(), &[ptr, ptr, ptr], false, false);
        decl(&mut out, "kelp_gc_block_free", ctx.i32(), &[i64, ptr, ptr], false, false);
    }
    // The panic path and the assertion: `kelp_panic` never returns, but the
    // module still terminates the block it is called from with `unreachable`.
    decl(&mut out, "kelp_panic", void, &[ptr], false, false);
    decl(&mut out, "kelp_assert", void, &[i1], false, false);
    // The thread runtime (MK9-S3-M1 stage C1). The handle is one opaque pointer,
    // and every accessor takes it and hands back one of the slots, so the
    // record's layout stays the runtime crate's business. `kelp_thread_start` takes
    // the per-signature body's address as an ordinary pointer, the same shape
    // the earlier emitter calls it with.
    decl(&mut out, "kelp_thread_new", ptr, &[i64, i64], true, false);
    decl(
        &mut out,
        "kelp_thread_start",
        void,
        &[ptr, ptr, ptr, ptr, ptr],
        false,
        false,
    );
    if managed {
        // The managed entry roots the spawn's values (section 3). Hosted IR does not name it.
        decl(&mut out, "kelp_thread_start_rooted", void, &[ptr, ptr, ptr, ptr, ptr, ptr, ptr], false, false);
    }
    if spawn_frames {
        // The generated frames of a spawning program (decision 125): enter at a
        // function's start, leave before each return.
        decl(&mut out, "kelp_cleanup_enter", void, &[ptr, ptr, ptr], false, false);
        decl(&mut out, "kelp_cleanup_leave", void, &[ptr], false, false);
        // The spawn entry points that also pass the argument's drop (decision 125).
        decl(&mut out, "kelp_thread_start_dropping", void, &[ptr, ptr, ptr, ptr, ptr, ptr], false, false);
        if managed {
            decl(&mut out, "kelp_thread_start_rooted_dropping", void, &[ptr, ptr, ptr, ptr, ptr, ptr, ptr, ptr], false, false);
        }
    }
    decl(&mut out, "kelp_thread_wait", ctx.i32(), &[ptr], true, false);
    decl(&mut out, "kelp_thread_arg", ptr, &[ptr], true, false);
    decl(&mut out, "kelp_thread_result", ptr, &[ptr], true, false);
    decl(&mut out, "kelp_thread_message", ptr, &[ptr], true, false);
    decl(&mut out, "kelp_thread_dispose", void, &[ptr], false, false);
    decl(&mut out, "kelp_thread_drop", void, &[ptr], false, false);
    // The mutex runtime (B-5): one opaque record, `kelp_mutex_new(value, size,
    // drop)` builds it, `kelp_mutex_run` holds the lock while the generated
    // body runs, `kelp_mutex_into` moves the value out and frees it, and
    // `kelp_mutex_drop` is the slot's drop.
    decl(&mut out, "kelp_mutex_new", ptr, &[ptr, i64, ptr], true, false);
    if managed {
        decl(&mut out, "kelp_mutex_new_rooted", ptr, &[ptr, i64, ptr, ptr], true, false);
    }
    decl(&mut out, "kelp_mutex_run", void, &[ptr, ptr, ptr, ptr, ptr], false, false);
    decl(&mut out, "kelp_mutex_into", void, &[ptr, ptr], false, false);
    decl(&mut out, "kelp_mutex_drop", void, &[ptr], false, false);
    decl(&mut out, "kelp_mutex_share", ptr, &[ptr], false, false);
    decl(&mut out, "kelp_mutex_swap", void, &[ptr, ptr, ptr], false, false);
    decl(&mut out, "kelp_condvar_share", ptr, &[ptr], false, false);
    // The reader/writer lock and condition variable runtime (`std/sync`).
    decl(&mut out, "kelp_rwlock_new", ptr, &[ptr, i64, ptr], true, false);
    if managed {
        decl(&mut out, "kelp_rwlock_new_rooted", ptr, &[ptr, i64, ptr, ptr], true, false);
    }
    decl(&mut out, "kelp_rwlock_read", void, &[ptr, ptr, ptr, ptr, ptr], false, false);
    decl(&mut out, "kelp_rwlock_write", void, &[ptr, ptr, ptr, ptr, ptr], false, false);
    decl(&mut out, "kelp_rwlock_try_read", ctx.i32(), &[ptr, ptr, ptr, ptr, ptr], true, false);
    decl(&mut out, "kelp_rwlock_try_write", ctx.i32(), &[ptr, ptr, ptr, ptr, ptr], true, false);
    decl(&mut out, "kelp_rwlock_into", void, &[ptr, ptr], false, false);
    decl(&mut out, "kelp_rwlock_drop", void, &[ptr], false, false);
    decl(&mut out, "kelp_condvar_new", ptr, &[], true, false);
    decl(&mut out, "kelp_condvar_notify_one", void, &[ptr], false, false);
    decl(&mut out, "kelp_condvar_notify_all", void, &[ptr], false, false);
    decl(&mut out, "kelp_condvar_drop", void, &[ptr], false, false);
    decl(
        &mut out,
        "kelp_mutex_wait_while",
        void,
        &[ptr, ptr, ptr, ptr, ptr, ctx.i32(), ptr, ptr, ptr, ptr],
        false,
        false,
    );
    decl(
        &mut out,
        "kelp_mutex_wait_while_timeout",
        ctx.i32(),
        &[ptr, ptr, ptr, ptr, ptr, ctx.i32(), i64, ptr, ptr, ptr, ptr],
        true,
        false,
    );
    // `kelp_Bytes`
    decl(&mut out, "kelp_Bytes__empty", bytes, &[], true, true);
    decl(&mut out, "kelp_Bytes__from_str", bytes, &[ptr], true, true);
    decl(&mut out, "kelp_Bytes__clone", bytes, &[ptr], true, true);
    decl(&mut out, "kelp_Bytes__len", i64, &[ptr], true, false);
    decl(&mut out, "kelp_Bytes__is_empty", i1, &[ptr], true, false);
    decl(&mut out, "kelp_Bytes__get", i8, &[ptr, i64], true, false);
    decl(&mut out, "kelp_Bytes__set", void, &[ptr, i64, i8], false, false);
    decl(&mut out, "kelp_Bytes__push", void, &[ptr, i8], false, false);
    decl(&mut out, "kelp_Bytes__append", void, &[ptr, ptr], false, false);
    decl(&mut out, "kelp_Bytes__reserve", void, &[ptr, i64], false, false);
    decl(&mut out, "kelp_Bytes__clear", void, &[ptr], false, false);
    decl(&mut out, "kelp_Bytes__as_str", ptr, &[ptr], true, false);
    decl(&mut out, "kelp_bytes_drop", void, &[ptr], false, false);
    // `kelp_String`
    decl(&mut out, "kelp_String__empty", string, &[], true, true);
    decl(&mut out, "kelp_String__from_str", string, &[ptr], true, true);
    decl(&mut out, "kelp_String__clone", string, &[ptr], true, true);
    decl(&mut out, "kelp_String__with_capacity", string, &[i64], true, true);
    decl(&mut out, "kelp_String__len", i64, &[ptr], true, false);
    decl(&mut out, "kelp_String__char_count", i64, &[ptr], true, false);
    decl(&mut out, "kelp_String__is_empty", i1, &[ptr], true, false);
    decl(&mut out, "kelp_String__as_str", ptr, &[ptr], true, false);
    decl(&mut out, "kelp_String__view", view, &[ptr], true, false);
    decl(&mut out, "kelp_String__push", void, &[ptr, i8], false, false);
    decl(&mut out, "kelp_String__push_str", void, &[ptr, ptr], false, false);
    decl(&mut out, "kelp_String__reserve", void, &[ptr, i64], false, false);
    decl(&mut out, "kelp_String__clear", void, &[ptr], false, false);
    decl(&mut out, "kelp_string_drop", void, &[ptr], false, false);
    // The growth routine both owned buffers share. The append bodies this
    // module emits test `len + extra <= cap` themselves, which is the callee's
    // own first line, and reach this declaration only when the buffer really
    // has to grow: the point of the declaration is that the common append never
    // crosses the object boundary.
    decl(&mut out, "kelp_buf_reserve", void, &[ptr, i64, ptr, i64], false, false);
    // `kelp_View`: the two-word struct is passed and returned by value, which is
    // the ABI's `INTEGER, INTEGER` e-classes on this target, so it crosses
    // unchanged.
    decl(&mut out, "kelp_str__view", view, &[ptr], true, false);
    decl(&mut out, "kelp_View__len", i64, &[view], true, false);
    decl(&mut out, "kelp_View__is_empty", i1, &[view], true, false);
    decl(&mut out, "kelp_View__get", i8, &[view, i64], true, false);
    decl(&mut out, "kelp_View__slice", view, &[view, i64, i64], true, false);
    decl(&mut out, "kelp_View__eq", i1, &[view, view], true, false);
    decl(&mut out, "kelp_View__to_string", string, &[view], true, true);
    // The map key helpers `write_map` calls: `#define static` in `build.rs`
    // gives them external linkage, so the IR declares them rather than
    // re-deriving the hash and the equality (MK7 §3.2, §3.3). `kelp_map_hash`
    // is FNV-1a over the key's bytes; `kelp_map_mix` is the 64-bit finalizer a
    // scalar key goes through; `kelp_map_key_eq` compares a stored `String`
    // against a `str` view length-aware; `kelp_map_combine` folds one hashed
    // field or tag into a derived key's hash, the one constant that has to be
    // the same in both backends (A-6).
    decl(&mut out, "kelp_map_hash", i64, &[ptr, i64], true, false);
    decl(&mut out, "kelp_map_mix", i64, &[i64], true, false);
    decl(&mut out, "kelp_map_key_eq", i1, &[ptr, ptr, i64], true, false);
    decl(&mut out, "kelp_map_combine", i64, &[i64, i64], true, false);
    // `String#from_bytes`'s C entry point takes a 24-byte `kelp_Bytes` by value,
    // which an IR call cannot cross (BUG-vN4tR, above). The runtime object's
    // adapter spells that argument as a pointer, and it returns the `String`
    // through the hidden first argument clang gives a 24-byte return.
    {
        let ty = ctx.fn_type(void, &[ptr, ptr], false);
        let func = module.add_function("kelp_String__from_bytes_ptr", ty);
        out.insert(
            // The key is the *builtin's* mangled name, so `emit_call` finds it
            // with the same `symbol_name(callee)` lookup it uses for every
            // other runtime method; the function it calls is the adapter.
            symbol_name("String#from_bytes"),
            RtFn {
                func,
                ty,
                yields: true,
                ret: Some(string),
                sret: true,
            },
        );
    }
    // `String#into_bytes` has the opposite shape and the same problem: its C
    // form takes a 24-byte `kelp_String` *by value* and returns a 24-byte
    // `kelp_Bytes`. The adapter takes the receiver by pointer and returns the
    // buffer through the hidden first argument, and `rt_arg_value` stages the
    // receiver into a slot of its own, so the call never passes a 24-byte
    // aggregate by value in either direction (BUG-vN4tR, MK7 §3.3).
    {
        let ty = ctx.fn_type(void, &[ptr, ptr], false);
        let func = module.add_function("kelp_String__into_bytes_ptr", ty);
        out.insert(
            symbol_name("String#into_bytes"),
            RtFn {
                func,
                ty,
                yields: true,
                ret: Some(bytes),
                sret: true,
            },
        );
    }
    out
}

/// Map a `tir` type to its LLVM type. The scalar subset, the builtin heap types,
/// `str`, a `datac` `structs` holds a type for, and a container `containers`
/// holds a named struct for.
/// The LLVM type an enum's tag is stored in: the C typedef's width, taken from
/// the `repr(<int>)` clause (kernel stage 2). The width is all LLVM needs; the
/// signedness lives beside it in `EnumInfo::tag_signed`, where the comparison
/// and the hash's widening read it.
pub(crate) fn enum_tag_llvm_type<'a>(ctx: &'a Context, bytes: u64) -> Type<'a> {
    match bytes {
        1 => ctx.i8(),
        2 => ctx.i16(),
        8 => ctx.i64(),
        _ => ctx.i32(),
    }
}

/// A zero-length array whose element type has ABI alignment `n`, for the first
/// field of a `repr(align(N))` datac's LLVM body. LLVM's array alignment is its
/// element's, and its size is zero, so it raises the struct's own alignment
/// without moving any field. An integer type carries the alignment up to 16
/// bytes; above that a vector of `n` bytes is the type whose alignment is `n`.
pub(crate) fn alignment_anchor<'a>(ctx: &'a Context, n: u64) -> Type<'a> {
    if n <= 16 {
        ctx.array_type(ctx.int((n * 8) as u32), 0)
    } else {
        ctx.array_type(ctx.vector_type(ctx.i8(), n as u32), 0)
    }
}

/// Whether a folded static initializer is all zero, which is what decides
/// between `.bss` and `.data` on the LLVM backend.
pub(crate) fn static_init_is_zero_llvm(init: &crate::tir::StaticInit) -> bool {
    use crate::tir::StaticInit as SI;
    match init {
        SI::Int(v) => *v == 0,
        SI::Float(v) => *v == 0.0,
        SI::Bool(b) => !*b,
        SI::Char(c) => *c == 0,
        SI::Array(elems) => elems.iter().all(static_init_is_zero_llvm),
        SI::Repeat { value, .. } => static_init_is_zero_llvm(value),
        SI::Struct(fields) => fields.iter().all(static_init_is_zero_llvm),
        SI::Atomic(inner) => static_init_is_zero_llvm(inner),
    }
}

/// Whether a static's type contains an `Atomic<T>` anywhere, so the global is
/// emitted non-constant even when the declaration is not `static mut`.
pub(crate) fn ty_contains_atomic(ty: &Ty, structs: &[StructDef]) -> bool {
    match ty {
        Ty::Atomic(_) => true,
        Ty::Array(elem, _) => ty_contains_atomic(elem, structs),
        Ty::Struct(name) => structs
            .iter()
            .find(|s| &s.name == name)
            .map(|s| s.fields.iter().any(|f| ty_contains_atomic(&f.ty, structs)))
            .unwrap_or(false),
        _ => false,
    }
}

pub(crate) fn llvm_type<'a>(
    ctx: &'a Context,
    structs: &HashMap<String, Type<'a>>,
    containers: &HashMap<Ty, Type<'a>>,
    ty: &Ty,
) -> Result<Type<'a>, String> {
    Ok(match ty {
        Ty::I32 | Ty::U32 | Ty::Char => ctx.i32(),
        Ty::I64 | Ty::Int | Ty::U64 | Ty::Uint | Ty::Usize | Ty::Isize => ctx.i64(),
        Ty::U8 => ctx.i8(),
        Ty::U16 => ctx.i16(),
        Ty::F32 => ctx.f32(),
        Ty::F64 | Ty::Float => ctx.f64(),
        Ty::Bool => ctx.i1(),
        // `str` is a bare `const char*`, which is the pointer LLVM 21 uses for
        // every pointer. S1 needs it so `print("...")` has a string to hand
        // `%s`; it is not otherwise part of the scalar subset. A borrow of `str`
        // is the same pointer (there is no second indirection), so `&str` maps
        // here too.
        Ty::Str => ctx.opaque_ptr(),
        Ty::Ref { inner, .. } if matches!(**inner, Ty::Str) => ctx.opaque_ptr(),
        // `Thread<T>` is one pointer to the worker record `thread.spawn`
        // allocates (MK9-S3-M1 stage C1), so the element type does not reach
        // the IR at all; it decides the size of the result slot and the
        // `Result<T, ThreadErr>` a join builds, both of which the call sites
        // name themselves.
        Ty::Thread(_) => ctx.opaque_ptr(),
        // `Mutex<T>` is one pointer to the runtime record `Mutex.new`
        // allocates (B-5), so the element type does not reach the IR either;
        // only the call sites that copy the value in or out name it.
        Ty::Mutex(_) => ctx.opaque_ptr(),
        // `RwLock<T>` and `Condvar` are the same one pointer to a runtime
        // record (`std/sync`), so the element type never reaches the IR.
        Ty::RwLock(_) | Ty::Condvar => ctx.opaque_ptr(),
        // An opaque handle (gap 1) is a plain pointer, so the two backends
        // agree without a layout table: the earlier emitter's `void*` typedef and
        // this `ptr` are the same value at the ABI.
        Ty::Handle(_) => ctx.opaque_ptr(),
        // A raw pointer (MK9-S2 stage 1) is the same plain `ptr`: LLVM 21's
        // opaque pointer carries no pointee type, so the element type is
        // information only the Kelp type keeps (it decides the load/store and
        // the GEP width, which the call sites name themselves).
        Ty::Ptr(_) => ctx.opaque_ptr(),
        // The builtin heap types, in the same field order the runtime crate
        // defines them (MK7 §3.3). `View` is the borrowed pair.
        Ty::Bytes => *structs
            .get("Bytes")
            .ok_or_else(|| no_value("the builtin 'Bytes' layout"))?,
        Ty::String => *structs
            .get("String")
            .ok_or_else(|| no_value("the builtin 'String' layout"))?,
        Ty::View => *structs
            .get("View")
            .ok_or_else(|| no_value("the builtin 'View' layout"))?,
        // A borrow of a heap type is its address, which is the pointer every
        // runtime helper taking `const kelp_Bytes*` wants.
        Ty::Ref { inner, .. }
            if matches!(**inner, Ty::Bytes | Ty::String | Ty::View) =>
        {
            ctx.opaque_ptr()
        }
        // A `datac` parameter resolves to a borrow of the struct, and a borrow is
        // its address (MK7 §3.3: pass a pointer, not a value, between frames).
        Ty::Ref { inner, .. } if matches!(**inner, Ty::Struct(_)) => ctx.opaque_ptr(),
        // A container is a named struct here; a borrow of one is its address,
        // which is what every method's `self` and every container parameter is.
        Ty::List(_) | Ty::Box(_) | Ty::Map(..) | Ty::Array(..) | Ty::Atomic(_) => *containers
            .get(ty)
            .ok_or_else(|| no_value(&format!("the container '{}'", ty.display())))?,
        Ty::Ref { inner, .. }
            if matches!(
                **inner,
                Ty::List(_) | Ty::Box(_) | Ty::Map(..) | Ty::Array(..) | Ty::Atomic(_)
            ) =>
        {
            ctx.opaque_ptr()
        }
        // Every other borrow is one pointer too (S2F): a `mut i64`
        // parameter, an `&i32` local, a field borrow, a borrow a container
        // method returns. A borrow of a function value or of a closure is the
        // same pointer, and it is what a closure parameter is (S2E): the
        // parameter rule borrows a non-`Copy` closure, so the fat pointer's
        // address is what a callee projects through.
        Ty::Ref { .. } => ctx.opaque_ptr(),
        // A plain function value is the function's address (S2E): one `ptr`,
        // `Copy`, owning nothing.
        Ty::Fn { .. } => ctx.opaque_ptr(),
        // A closure is the fat pointer `{ fn, env, drop }`, one identified
        // struct per signature and so a named type here like a container
        // (S2E).
        Ty::Closure { .. } | Ty::ScopedFn { .. } => *containers
            .get(ty)
            .ok_or_else(|| no_value(&format!("the closure '{}'", ty.display())))?,
        // A borrowed trait object (A-12 slice 4) is the fat pair
        // `{ data, vtable }`, an identified struct per trait created in
        // `declare_container_types` (both mutability spellings resolve to the
        // one struct). An *owned* one (A-12 slice 7) is the same pair, so it
        // resolves to the same identified struct.
        Ty::Dyn { .. } | Ty::BoxDyn { .. } => *containers
            .get(ty)
            .ok_or_else(|| no_value(&format!("the trait object '{}'", ty.display())))?,
        Ty::Struct(name) => *structs
            .get(name)
            .ok_or_else(|| refuse(&format!("the datac '{name}'"), "S2A"))?,
        other => return Err(refuse_type(other)),
    })
}

/// The return type of a function: `void` for unit, otherwise the type itself.
pub(crate) fn llvm_ret_type<'a>(
    ctx: &'a Context,
    structs: &HashMap<String, Type<'a>>,
    containers: &HashMap<Ty, Type<'a>>,
    ty: &Ty,
) -> Result<Type<'a>, String> {
    if *ty == Ty::Unit {
        Ok(ctx.void())
    } else {
        llvm_type(ctx, structs, containers, ty)
    }
}

/// The name of the `datac` a (possibly borrowed) type names, if any.
pub(crate) fn struct_name_of_ty(ty: &Ty) -> Option<&str> {
    match ty {
        Ty::Struct(name) => Some(name),
        Ty::Ref { inner, .. } => struct_name_of_ty(inner),
        _ => None,
    }
}

/// Whether a type is a borrow: a `Ty::Ref` of anything. The binding holds one
/// pointer, so a place read has to load it rather than hand out the slot; the
/// pointee does not matter, because `llvm_type` gives every `Ref` the same
/// `ptr` (S2F). `&str` is included, since a `&str` binding holds the `char*`
/// itself.
pub(crate) fn is_borrow_ty(ty: &Ty) -> bool {
    matches!(ty, Ty::Ref { .. })
}

/// Whether a type is a borrow of `str` (or `str` itself), which is the pointer
/// rather than a second indirection.
pub(crate) fn is_str_ref(ty: &Ty) -> bool {
    match ty {
        Ty::Str => true,
        Ty::Ref { inner, .. } => matches!(**inner, Ty::Str),
        _ => false,
    }
}

/// Whether a block's tail diverges instead of producing a value. `panic` is the
/// one expression the frontend puts there (`infer.rs` accepts it against any
/// declared return type), and the earlier emitter special-cases the same call in
/// `write_function_body`; without this a value-returning function whose tail is
/// a `panic` asks the unit-typed call for a value and reports an internal error.
pub(crate) fn is_diverging_tail(tail: &Expr) -> bool {
    matches!(&tail.kind, ExprKind::Call { callee, .. } if callee == "panic")
}

/// Whether an expression has stable storage, mirroring the subset of
/// the earlier emitter's `is_place` this slice lowers.
/// The largest bound a `while` condition may state literally and still have
/// its loop fully unrolled by LLVM (`llvm_ffi::Context::mark_loop_unroll_full`).
///
/// The step is what makes a literal bound mean a small trip count: a loop that
/// runs `i < N` with a literal `N` runs at most `N + 1` times, so forcing the
/// unroll cannot grow the code by more than that, while a bound that is a
/// value or a wide literal is left to LLVM's own cost model. Sixteen is the
/// count LLVM's own `unroll-full-max-count` defaults to.
pub(crate) const FORCED_UNROLL_BOUND: i128 = 16;

/// The bound of a `while` condition that states one as a small literal:
/// `i < N` or `i <= N` with `N` at most [`FORCED_UNROLL_BOUND`].
///
/// Only the two upper-bound comparisons qualify. `i > N`/`i >= N` states a
/// lower bound, which says nothing about the trip count, and `i != N` can be
/// reached by a step that never lands on `N`.
pub(crate) fn forced_unroll_bound(cond: &Expr) -> Option<i128> {
    let ExprKind::Binary { op, rhs, .. } = &cond.kind else {
        return None;
    };
    if !matches!(op, BinOp::Lt | BinOp::LtEq) {
        return None;
    }
    let ExprKind::IntLit(bound) = rhs.kind else {
        return None;
    };
    (0..=FORCED_UNROLL_BOUND).contains(&bound).then_some(bound)
}

pub(crate) fn is_place_expr(e: &Expr) -> bool {
    matches!(
        e.kind,
        ExprKind::Ident(_)
            | ExprKind::StaticRef(_)
            | ExprKind::FieldAccess { .. }
            | ExprKind::EnumField { .. }
            | ExprKind::Deref { .. }
            | ExprKind::Capture { .. }
    )
}

/// The rvalue a field chain hangs off, when the chain's own address is not
/// storage already: the earlier emitter's `temporary_root`, minus the rows this slice
/// does not lower. `None` for a place, `Some(root)` for a chain rooted at an
/// rvalue. An `enum` payload field is part of a chain exactly as a `datac` field
/// is (the earlier emitter walks the two the same way).
pub(crate) fn temporary_root(expr: &Expr) -> Option<&Expr> {
    let mut cur = expr;
    loop {
        match &cur.kind {
            ExprKind::FieldAccess { base, .. } | ExprKind::EnumField { base, .. } => {
                // A chain that steps through a borrow lives in the pointee, so
                // nothing has to be staged for it.
                if matches!(base.ty, Ty::Ref { .. }) {
                    return None;
                }
                cur = base;
            }
            _ => break,
        }
    }
    if is_place_expr(cur) { None } else { Some(cur) }
}

/// The identified LLVM type name of one `enum`'s union, and of one variant's
/// payload struct. Named after the tag `struct_tag` gives the enum, so the IR
/// names the same type the C spelling does and two enums cannot collide.
pub(crate) fn enum_union_name(name: &str) -> String {
    format!("{}.kelp_v", struct_tag(name))
}

pub(crate) fn enum_payload_name(name: &str, variant: usize) -> String {
    format!("{}.kelp_v_{variant}", struct_tag(name))
}

/// `size` rounded up to a multiple of `align`, which is a power of two. The
/// union's size rule, the same one C's own layout uses.
pub(crate) fn round_up(size: u64, align: u32) -> u64 {
    let align = align as u64;
    size.div_ceil(align) * align
}

/// The mangled symbol of the monomorphized function a builtin container method
/// resolves to, mirroring the earlier emitter's `call_value`'s name construction: the
/// element type comes from the receiver argument (or, for the associated
/// `List.of(v)`/`Box.of(v)`, from `v` itself, and for `empty`/`Map.new()` from
/// the call's own result type). `get` has two spellings, by value for a `Copy`
/// element and by pointer for an owned one, decided by the call's own result
/// type. `None` for a call that is not a container method.
pub(crate) fn container_symbol(callee: &str, args: &[Expr], ret: &Ty) -> Option<String> {
    use crate::types::{array_c_type, array_elem_of, array_len_of, box_c_type, box_elem_of};
    for (prefix, name_of, elem_of) in [
        (
            "List#",
            crate::types::list_c_type as fn(&Ty) -> String,
            crate::types::list_elem_of as fn(&Ty) -> Option<&Ty>,
        ),
        (
            "Box#",
            box_c_type as fn(&Ty) -> String,
            box_elem_of as fn(&Ty) -> Option<&Ty>,
        ),
    ] {
        let Some(op) = callee.strip_prefix(prefix) else {
            continue;
        };
        let elem = if op == "of" {
            args.first().map(|a| a.ty.clone())
        } else if op == "empty" || op == "new" || op == "with_capacity" {
            elem_of(ret).cloned()
        } else {
            args.first().and_then(|a| elem_of(&a.ty)).cloned()
        };
        if let Some(elem) = elem {
            let base = name_of(&elem);
            let suffix = if op == "get" && matches!(ret, Ty::Ref { .. }) {
                "get_const"
            } else {
                op
            };
            return Some(format!("{base}__{suffix}"));
        }
    }
    if let Some(op) = callee.strip_prefix("Array#") {
        let e = args.first()?;
        let elem = array_elem_of(&e.ty)?.clone();
        let len = array_len_of(&e.ty)?;
        let base = array_c_type(&elem, len);
        let suffix = if op == "get" && matches!(ret, Ty::Ref { .. }) {
            "get_const"
        } else {
            op
        };
        return Some(format!("{base}__{suffix}"));
    }
    if let Some(op) = callee.strip_prefix("Map#") {
        let (value, key) = if op == "new" || op == "with_capacity" {
            (
                crate::types::map_elem_of(ret).cloned(),
                crate::types::map_key_of(ret).cloned(),
            )
        } else {
            let e = args.first()?;
            (
                crate::types::map_elem_of(&e.ty).cloned(),
                crate::types::map_key_of(&e.ty).cloned(),
            )
        };
        if let (Some(value), Some(key)) = (value, key) {
            let base = map_c_type(&key, &value);
            let suffix = if op == "get" && matches!(ret, Ty::Ref { .. }) {
                "get_const"
            } else {
                op
            };
            return Some(format!("{base}__{suffix}"));
        }
    }
    None
}

/// The `List#get`/`List#set` or `Bytes#get`/`Bytes#set` call whose index is
/// `x.to_usize()` for a signed integer `x`: the source type, the conversion's
/// callee name (for its panic text) and `x`. `None` for every other call.
pub(crate) fn fused_index_call<'e>(callee: &str, args: &'e [Expr]) -> Option<(Ty, &'e str, &'e Expr)> {
    let op = callee
        .strip_prefix("List#")
        .or_else(|| callee.strip_prefix("Bytes#"))?;
    if op != "get" && op != "set" {
        return None;
    }
    // The length is read before the value of a `set` is evaluated, so the value
    // must not be able to change a length: only a read-only expression is fused.
    if op == "set" && !read_only_expr(args.get(2)?) {
        return None;
    }
    let index = args.get(1)?;
    let ExprKind::Call { callee: conv, args: conv_args } = &index.kind else {
        return None;
    };
    let (src, dst) = conv.split_once("#to_")?;
    if dst != "usize" {
        return None;
    }
    let from = Ty::from_name(src)?;
    if !from.is_integer() || !is_signed(&from) {
        return None;
    }
    let inner = conv_args.first()?;
    Some((from, conv.as_str(), inner))
}
