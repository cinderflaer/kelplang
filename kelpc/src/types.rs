use crate::ast::Repr;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Ty {
    I32,
    I64,
    U32,
    U64,
    F32,
    F64,
    Bool,
    Char,
    Unit,
    Str,
    /// `u8`/`u16`: byte-level integers, and `usize`/`isize`: the index and
    /// size types `Bytes` is measured and indexed with. Added in M6 because
    /// a byte buffer cannot be described without them.
    U8,
    U16,
    Usize,
    Isize,
    /// The builtin owned heap byte buffer (M6): a pointer plus a length,
    /// allocated on construction and freed at the end of the owning scope.
    /// Non-`Copy`, so it moves; `&Bytes` is the borrow (`Ty::Ref`).
    Bytes,
    /// The builtin owned heap utf-8 text type (M6.2). Same representation as
    /// `Bytes` (pointer plus length) and the same move/drop rules; it exists
    /// as its own type so text and arbitrary bytes are not interchangeable.
    /// `String.from_bytes`/`String.into_bytes` convert with an ownership
    /// transfer, not a cast.
    String,
    /// A **length-carrying borrowed text view** (A-8, open items 22/29/44):
    /// a pointer *and* a length, `{ const char* ptr; size_t len; }`.
    ///
    /// It exists because `str` is a bare `const char*`: it has no length, so
    /// every scan re-derives one with `str.len` (a `strlen`), every slice
    /// allocates a `String` (`str.copy_range`), and a `datac` cannot hold one
    /// at all — which is why the JSON reader in this repository copied its
    /// whole input before parsing. A `View` carries its length, so a slice is
    /// a pointer and a subtraction.
    ///
    /// **It follows `str`'s rules exactly, and the reason is soundness.** A
    /// view borrows the text it points at, and v1 has no lifetime syntax
    /// (A-11) to tie a field to its source, so a `View` is refused as a
    /// `datac` field, an enum payload, a `List` element and a `Map` key, and
    /// it never converts to `str` implicitly: a *slice* has no NUL after it,
    /// so `str.len`/`str.eq`/`strcmp` on one would read past the slice. Every
    /// operation on a `View` is length-aware. `View.to_string` copies with the
    /// length, which is the one safe way to make a `str` of a slice.
    ///
    /// `Copy`, like a borrow: copying a pointer and a length moves nothing.
    View,
    /// A `datac` value type, named by its declaration.
    Struct(String),
    /// The builtin owned growable sequence (collections milestone):
    /// `List<T>`, where `T` is a scalar, `String`, `Bytes`, or a `datac`.
    /// Non-`Copy`: assigning or passing one moves it, and it is freed at the
    /// end of the owning scope — its elements first, then its buffer. The
    /// element type is part of the type, and codegen monomorphizes: one C
    /// struct plus one set of functions per element type actually used (see
    /// `codegen_c.rs::write_list`).
    List(Box<Ty>),
    /// The builtin owning pointer (M8): `Box<T>` holds one `T` on the heap,
    /// or nothing at all (`Box.empty()`), which is what makes a recursive
    /// `datac` expressible — `datac Node(val next: Box<Node>)` would be
    /// infinitely sized by value, and a pointer is the only way out.
    /// Non-`Copy`; dropping it drops the value it holds and frees the box.
    /// Like `List`, monomorphized: one C struct plus one function set per
    /// element type actually used.
    Box(Box<Ty>),
    /// The builtin owned keyed container (MK5-S5): `Map<K, V>` — a key type
    /// and a value type, both monomorphized. `Map<V>` is the shorthand for a
    /// `String` key, which is what every pre-A-2 use site writes, and the two
    /// spellings are the same `Ty`. A key must be hashable and comparable by
    /// the compiler: a `String`, an integer, `bool`, a `char`, or a `datac` or
    /// `enum` whose every field is itself keyable (`is_map_key` for the builtin
    /// set, `is_map_key_with` for the derived one). Non-`Copy`; a map
    /// owns its keys and its values, and drops both at the end of the owning
    /// scope. The value type is part of the type, so `Map<String, i64>` and
    /// `Map<i32, i64>` are different C types with different functions.
    /// `Map<String, Map<i32>>` and `Map<String, List<i32>>` stay refused: the
    /// value set is narrower than the `List` element set, which since A-10
    /// slice 1 also takes a `List` or a `Box`.
    Map(Box<Ty>, Box<Ty>),
    /// A borrow of a value of type `inner` — shared (`mutable: false`) or
    /// exclusive (`mutable: true`). Never appears as a function's declared
    /// return type in M4 (no surface syntax produces one there — see
    /// `infer::resolve_type`), so a borrow can never escape the call frame
    /// that created it; that's what makes the borrow checker's call-scoped
    /// release sound without real lifetime tracking. A reference is always
    /// Copy regardless of `inner` — copying a pointer doesn't move what it
    /// points at.
    Ref { mutable: bool, inner: Box<Ty> },
    /// A borrowed trait object (A-12 slice 4): `&dyn Name` / `&mut dyn Name`,
    /// where `trait_name` is the trait's canonical key. The value is a *fat
    /// pair*: the address of the borrowed value (the erased data pointer) and
    /// the address of a static, read-only vtable (see `tir::DynVtable`).
    ///
    /// Like `Ty::Ref` it is `Copy`, owns nothing and drops nothing: it is a
    /// borrow, so the value it points at is still owned by whoever it was
    /// borrowed from, and `borrowck.rs` tracks it as the `&x` it was written
    /// from. `mutable` is the `mut` of `&mut dyn Name`, which decides whether a
    /// `mut Self` method may be called through it. It is legal only as a
    /// parameter or a local: a field, a container element, a return and a
    /// `static` each refuse it, exactly as they refuse a `Ty::Ref`.
    Dyn { mutable: bool, trait_name: String },
    /// An owned trait object (A-12 slice 7): `Box<dyn Name>`, where
    /// `trait_name` is the trait's canonical key. The value is the *same* fat
    /// pair a `&dyn Name` is - the erased data pointer and a pointer to the
    /// pair's static vtable - but the box **owns** the allocation the data
    /// pointer addresses, so it is not `Copy` and it drops what it holds. The
    /// vtable carries a drop entry (`tir::DynVtable`), so the concrete type's
    /// drop glue runs through it before the allocation is freed.
    ///
    /// It is a type of its own rather than a `Box` whose element is a
    /// `Ty::Dyn`: the element of a `Box` is owned by value and a trait object
    /// is not a sized value, so `Box<dyn Name>` has no element type to name.
    /// It is legal wherever an owned value is: a field, an enum payload, a
    /// container element, a parameter, a return and a local.
    BoxDyn { trait_name: String },
    /// A function value (A-4): a plain C function pointer, with no captured
    /// environment — so it is `Copy`, it owns nothing, and it needs no drop.
    /// `params` are the *declared* parameter types, already through the
    /// parameter rule (an owned type is a borrow unless the type was written
    /// `move T`), which is what makes a written function type and a
    /// declaration's signature the same type when they spell the same thing.
    ///
    /// A bare name of a declared `fun` is how a value of this type is made;
    /// calling one goes through `tir::ExprKind::IndirectCall`. A `fun`
    /// literal that captured nothing is the same value; one that *captured*
    /// something is a `Ty::Closure` below, which carries the environment it
    /// captured. This variant is deliberately the environment-free half: a
    /// plain C function pointer, `Copy` (A-4 phases B/C).
    Fn { params: Vec<Ty>, ret: Box<Ty> },
    /// A function value that carries an environment (A-4 phase E): what a
    /// `fun` literal whose body names a binding from the enclosing scope
    /// becomes. Written `closure(T).R: U` — the same shape as a function
    /// type, under the `closure` keyword, because it is called the same way.
    ///
    /// The representation differs from `Ty::Fn` in exactly the ways that
    /// matter to every pass: a closure is `{ fn, env, drop }` — a *fat*
    /// pointer, monomorphized per signature — so it is **not `Copy`**, it
    /// owns the heap environment the captures were moved into, and it drops
    /// that environment when it dies. A `Ty::Fn` stays a plain C function
    /// pointer: a declared function, and a literal that captured nothing,
    /// have no environment to carry (see `codegen_c.rs::write_closure`).
    ///
    /// Calling one goes through `tir::ExprKind::IndirectCall`, exactly as a
    /// `Ty::Fn` does; codegen is what knows a closure's call spelling
    /// (`c.fn(c.env, ...)`). A capturing literal is the only way a value of
    /// this type is made, so a program cannot build a closure without
    /// something to capture.
    Closure { params: Vec<Ty>, ret: Box<Ty> },
    /// A **scoped** function value (`plan/scoped-blocks.md`): a parameter
    /// written `scoped fun(T).R: U`, which accepts only a `fun` literal or a
    /// trailing block written at the call site and may not outlive the call.
    ///
    /// It is closure-shaped at the ABI level (`{ fn, env, drop }`, `fn`
    /// taking the environment first), and the environment is the enclosing
    /// function's stack with a `NULL` drop, because nothing is heap and
    /// nothing is owned. It is a **type of its own** rather than a flag: the
    /// list of places a scoped value must not go (a `datac` field, a return,
    /// a container, a non-scoped parameter, another closure's environment) is
    /// a list of type positions, so every one of them is an ordinary mismatch
    /// and no side table can fall out of date.
    ///
    /// Legal only as a top-level parameter of a declaration. It owns nothing,
    /// so it needs no drop, and it never reaches a container element or a
    /// type argument. **`Copy`**, deliberately: the value is a code pointer
    /// plus a pointer to a stack environment that is alive for the call, so
    /// copying it inside the callee is copying two pointers, and the one
    /// entry point a parameter rule would otherwise add (an implicit `&`) is
    /// not wanted — the callee holds the fat pointer itself.
    ScopedFn { params: Vec<Ty>, ret: Box<Ty> },
    /// A **fixed-size array** (A-18): `[T; N]`, with `N` a compile-time
    /// constant. The storage is inline: one `N`-element block on whatever
    /// stack frame holds it, with no allocator in the path. That is the whole
    /// point, since a page table, an IDT/GDT or a register-save frame has to
    /// exist before an allocator can be trusted.
    ///
    /// The length is part of the type, so `[u8; 4]` and `[u8; 8]` are
    /// different types with different generated C structs, exactly as
    /// `List<i32>` and `List<String>` are. The element is monomorphized the
    /// same way a list's is: one `kelp_Array__<elem>__<N>` typedef, one
    /// function set, one drop function per `(element, length)` pair used.
    ///
    /// `Copy` **iff the element is**: an array of scalars is a value you can
    /// copy, while an array of `String` owns those strings and moves, which
    /// is the rule `plan/syntax.md` 3 states for the type.
    Array(Box<Ty>, u64),
    /// The builtin atomic cell (MK9-S3-M1 stage A1): `Atomic<T>`, one location
    /// every access to which is a single atomic operation.
    ///
    /// Monomorphized per element type exactly as `List<T>` is, and the element
    /// set is closed: `bool` and the integer types (`is_atomic_element`). The
    /// representation is one machine word, so every operation goes through the
    /// `__atomic_*` builtins in C and through `load atomic`, `store atomic`,
    /// `atomicrmw` and `cmpxchg` in LLVM.
    ///
    /// **Not `Copy`, and needing no drop.** Not `Copy` because a copy would be
    /// a second handle to one location, and every rule about an atomic is about
    /// the location rather than the handle; no drop because it owns nothing,
    /// which is what lets a `pd val Atomic<i64>` be the process-lifetime shared
    /// counter of MK9 1 without the `atexit` machinery an owned passdown needs.
    Atomic(Box<Ty>),
    /// The builtin thread handle (MK9-S3-M1 stage B1): `Thread<T>`, what
    /// `thread.spawn` hands back and `thread.join` consumes.
    ///
    /// Monomorphized per element type exactly as `Atomic<T>` and `List<T>` are,
    /// and `T` is the value the worker gives the joiner. **Not `Copy`.** It
    /// *does* own heap memory, one allocation: the worker record `thread.spawn`
    /// builds, which holds the running thread's identity, its result slot and
    /// the argument moved into it. Stage C1 gives it a drop (`kelp_thread_drop`)
    /// that detaches a worker nobody joined and leaks that record, because the
    /// record cannot be freed while the worker may still write through it; the
    /// decision is recorded in `plan/mk9-memory-model.md` section 5.
    ///
    /// A handle is accepted as a local, a `datac`/`enum` field and a parameter,
    /// and refused as a `List`/`Map`/`Box`/array element for the reason the
    /// refusal message gives: joining is a one-shot thing a container's own
    /// copy-and-drop routines would duplicate.
    Thread(Box<Ty>),
    /// The builtin mutex (B-5, `std/sync`): `Mutex<T>`, a value whose `T` is
    /// reached only through a scoped block (`m.lock { v -> ... }`).
    ///
    /// Monomorphized per element type exactly as `Atomic<T>` and `Thread<T>`
    /// are, and `T` is the owned, borrow-free value the mutex guards
    /// (`is_mutex_element`). **Not `Copy`.** The representation is one opaque
    /// pointer to a record the C runtime owns (a `pthread_mutex_t`, a poison
    /// flag and the `T` slot), so the pointer crosses both backends unchanged
    /// and the runtime's layout stays `kelp_rt.c`'s business. Its drop frees
    /// that record, running `T`'s own drop first.
    Mutex(Box<Ty>),
    /// An **opaque native handle** (gap 1): `extern type Widget`, what a
    /// binding passes around in place of a `GtkWidget*`. The payload is the
    /// handle's canonical, module-qualified name, exactly as `Ty::Struct`
    /// carries a `datac`'s.
    ///
    /// It is pointer-sized and `Copy`, and it is deliberately inert: no
    /// dereference, no arithmetic, no null literal and no conversion to or
    /// from an integer. `==`/`!=` between two handles of the same type are
    /// allowed and compare the pointers, and nothing else is. It owns
    /// nothing, so it needs no drop, and because the C spelling is a plain
    /// `void*` behind a per-type typedef, it crosses an `extern fun` boundary
    /// unchanged on both backends.
    Handle(String),
    /// A **raw pointer** (MK9-S2 stage 1, roadmap B-3): `Ptr<T>`, a machine
    /// address with no borrow tracking and no ownership.
    ///
    /// It is `Copy`, owns nothing, needs no drop and may be null, which is
    /// what a device register, a page table or memory the runtime did not
    /// allocate has to be reachable through. The representation is one
    /// machine pointer on both backends, so a `Ptr<T>` crosses an `extern
    /// fun` boundary unchanged.
    ///
    /// The element set is closed (`is_ptr_element`): a scalar, `bool`,
    /// `char`, a `datac`/`enum`, a fixed-size array, a handle or another
    /// `Ptr`. The owned and borrowed types are refused because none of them
    /// is one machine-level value.
    Ptr(Box<Ty>),
}

/// The element type one of the `Atomic*` names stands for, or `None` when
/// `name` is not one: `AtomicBool` is `Atomic<bool>`, `AtomicU8` is
/// `Atomic<u8>`, and so on. MK9 4 spells the instantiations out rather than
/// making every program write `Atomic<i64>`, and a name is also the only thing
/// an associated constructor can be written on (`AtomicI64.new(0)`), so both
/// spellings exist and both resolve to the same `Ty`.
///
/// There is no `AtomicI8`/`AtomicI16`: the element set is `bool` and v1's
/// integer types, and v1 has no `i8`/`i16` for those two names to mean.
pub fn atomic_alias_elem(name: &str) -> Option<Ty> {
    Some(match name {
        "AtomicBool" => Ty::Bool,
        "AtomicU8" => Ty::U8,
        "AtomicU16" => Ty::U16,
        "AtomicU32" => Ty::U32,
        "AtomicU64" => Ty::U64,
        "AtomicUsize" => Ty::Usize,
        "AtomicI32" => Ty::I32,
        "AtomicI64" => Ty::I64,
        "AtomicIsize" => Ty::Isize,
        _ => return None,
    })
}

/// The `Atomic*` name that stands for one element type, the inverse of
/// `atomic_alias_elem`. `infer.rs`'s trait-impl table keys a builtin `for` type
/// by the name the `impl` wrote (`impl Name for AtomicI64`), so a vtable target
/// recovered from a `Ty::Atomic` has to spell the same name back.
pub fn atomic_alias_name(elem: &Ty) -> Option<&'static str> {
    Some(match elem {
        Ty::Bool => "AtomicBool",
        Ty::U8 => "AtomicU8",
        Ty::U16 => "AtomicU16",
        Ty::U32 => "AtomicU32",
        Ty::U64 => "AtomicU64",
        Ty::Usize => "AtomicUsize",
        Ty::I32 => "AtomicI32",
        Ty::I64 => "AtomicI64",
        Ty::Isize => "AtomicIsize",
        _ => return None,
    })
}

impl Ty {
    pub fn from_name(name: &str) -> Option<Ty> {
        Some(match name {
            "i32" => Ty::I32,
            "i64" => Ty::I64,
            "u32" => Ty::U32,
            "u64" => Ty::U64,
            "f32" => Ty::F32,
            "f64" => Ty::F64,
            "bool" => Ty::Bool,
            "char" => Ty::Char,
            "unit" => Ty::Unit,
            "str" => Ty::Str,
            "u8" => Ty::U8,
            "u16" => Ty::U16,
            "usize" => Ty::Usize,
            "isize" => Ty::Isize,
            "Bytes" => Ty::Bytes,
            "String" => Ty::String,
            "View" => Ty::View,
            // `AtomicI64` and its siblings name one instantiation of the
            // builtin `Atomic<T>`, so the name stands for the whole cell rather
            // than for the element it holds.
            _ => {
                return atomic_alias_elem(name).map(|elem| Ty::Atomic(Box::new(elem)));
            }
        })
    }

    /// Whether a value of this type is `Copy`. Everything is, except the
    /// kinds of thing that own heap memory: a `datac` and the builtin
    /// `Bytes`/`String`. Reading a non-`Copy` value as a whole moves it (M3).
    pub fn is_copy(&self) -> bool {
        match self {
            // An array copies exactly when its element does: `[i32; 4]` is a
            // value, `[String; 4]` owns four strings and moves.
            Ty::Array(elem, _) => elem.is_copy(),
            // A borrowed trait object is two pointers, exactly like a borrow:
            // copying it moves nothing.
            Ty::Dyn { .. } => true,
            // An owned trait object is two pointers too, but it owns the
            // allocation the data pointer addresses, so copying it would make
            // two owners of one value: it moves like the `Box` it is.
            Ty::BoxDyn { .. } => false,
            _ => !matches!(
                self,
                Ty::Struct(_)
                    | Ty::Atomic(_)
                    | Ty::Thread(_)
                    | Ty::Mutex(_)
                    | Ty::Bytes
                    | Ty::String
                    | Ty::List(_)
                    | Ty::Box(_)
                    | Ty::Map(..)
                    | Ty::Closure { .. }
            ),
        }
    }

    /// Whether this type *itself* owns heap memory, ignoring whether a
    /// `datac` does. Use `needs_drop` below when the struct table is
    /// available — a `datac` with an owned field owns memory too, and out of
    /// context `Ty::Struct(_)` cannot know that.
    pub fn owns_heap(&self) -> bool {
        match self {
            // An array owns whatever its elements own, one drop call per
            // element at scope exit.
            Ty::Array(elem, _) => elem.owns_heap(),
            other => matches!(
                other,
                Ty::Bytes
                    | Ty::String
                    | Ty::List(_)
                    | Ty::Box(_)
                    // An owned trait object frees the value behind its data
                    // pointer through the vtable's drop entry (A-12 slice 7).
                    | Ty::BoxDyn { .. }
                    | Ty::Map(..)
                    // A `Thread<T>` owns the worker record `thread.spawn`
                    // allocated, so it is dropped (detached) like any other
                    // owned value (MK9-S3-M1 stage C1).
                    | Ty::Thread(_)
                    | Ty::Mutex(_)
                    | Ty::Closure { .. },
            ),
        }
    }

    pub fn is_numeric(&self) -> bool {
        matches!(
            self,
            Ty::I32
                | Ty::I64
                | Ty::U32
                | Ty::U64
                | Ty::F32
                | Ty::F64
                | Ty::U8
                | Ty::U16
                | Ty::Usize
                | Ty::Isize
        )
    }

    pub fn is_integer(&self) -> bool {
        matches!(
            self,
            Ty::I32 | Ty::I64 | Ty::U32 | Ty::U64 | Ty::U8 | Ty::U16 | Ty::Usize | Ty::Isize
        )
    }

    pub fn is_float(&self) -> bool {
        matches!(self, Ty::F32 | Ty::F64)
    }

    /// The full range of an integer type, as `(min, max)`: `min` signed so a
    /// signed type can express its own floor, `max` unsigned so `u64`/`usize`
    /// can express theirs. `None` for anything that is not an integer.
    ///
    /// Two passes need this and they have to agree: `codegen_c.rs` uses it to
    /// decide whether a `to_<T>` conversion needs a range check, and
    /// `infer.rs` uses it to decide which conversions *have* a `try_to_<T>`
    /// twin and what that twin tests.
    pub fn int_bounds(ty: &Ty) -> Option<(i128, u128)> {
        Some(match ty {
            Ty::I32 => (i32::MIN as i128, i32::MAX as u128),
            Ty::I64 | Ty::Isize => (i64::MIN as i128, i64::MAX as u128),
            Ty::U8 => (0, u8::MAX as u128),
            Ty::U16 => (0, u16::MAX as u128),
            Ty::U32 => (0, u32::MAX as u128),
            Ty::U64 | Ty::Usize => (0, u64::MAX as u128),
            _ => return None,
        })
    }

    pub fn c_type(&self) -> String {
        match self {
            Ty::I32 => "int32_t".to_string(),
            Ty::I64 => "int64_t".to_string(),
            Ty::U32 => "uint32_t".to_string(),
            Ty::U64 => "uint64_t".to_string(),
            Ty::F32 => "float".to_string(),
            Ty::F64 => "double".to_string(),
            Ty::Bool => "bool".to_string(),
            // A `char` is a Unicode scalar value, not a C byte: one code
            // point, which is what makes `'中'` representable. `%c` in
            // `print` still prints its low byte, which is the ASCII case it
            // is there for.
            Ty::Char => "uint32_t".to_string(),
            Ty::Unit => "void".to_string(),
            Ty::Str => "const char*".to_string(),
            Ty::U8 => "uint8_t".to_string(),
            Ty::U16 => "uint16_t".to_string(),
            Ty::Usize => "size_t".to_string(),
            Ty::Isize => "ptrdiff_t".to_string(),
            Ty::Bytes => "kelp_Bytes".to_string(),
            Ty::String => "kelp_String".to_string(),
            Ty::View => "kelp_View".to_string(),
            Ty::List(elem) => list_c_type(elem),
            Ty::Box(elem) => box_c_type(elem),
            Ty::Map(key, value) => map_c_type(key, value),
            Ty::Array(elem, len) => array_c_type(elem, *len),
            Ty::Atomic(elem) => atomic_c_type(elem),
            Ty::Thread(_) => thread_c_type(),
            Ty::Mutex(_) => mutex_c_type(),
            Ty::Handle(name) => handle_c_type(name),
            // A pointer is the C pointer to whatever type it addresses. An
            // array's generated struct name and a datac's `struct_` name are
            // both ordinary C types, so the element spells itself.
            Ty::Ptr(elem) => format!("{}*", elem.c_type()),
            // A `&str` is the same C value as the `str` it points at (both
            // `const char*`): a reference to an already-borrowed view is not
            // a pointer to a pointer. The borrow lives in the type system
            // and the borrow checker, not in the representation.
            Ty::Ref { inner, .. } if matches!(**inner, Ty::Str) => "const char*".to_string(),
            // A function *type* is a pointer type, and a pointer type cannot
            // be printed as `ret (*)(params)` and then followed by a name
            // (`c_decl` is what puts a declarator together). Inside another
            // declarator this spelling is the correct C fragment, which is
            // all a return position needs.
            Ty::Fn { params, ret } => {
                format!("{} (*)({})", ret.c_type(), fn_params_c(params))
            }
            // A closure is an ordinary C struct (a fat pointer), so it is
            // named by its typedef and never needs the declarator trick
            // above — including in a return position, which is why a
            // function may return a closure but not a bare function value. A
            // scoped function value has the same fat-pointer shape, so it is
            // spelled by the same typedef.
            Ty::Closure { params, ret } | Ty::ScopedFn { params, ret } => {
                closure_c_type(params, ret)
            }
            Ty::Struct(name) => format!("struct_{}", c_ident(name)),
            // A borrowed trait object is the fat pair: the erased data pointer
            // and a pointer to the trait's vtable, one C struct per trait (the
            // shared and exclusive spellings are the same C value; the borrow
            // checker is what distinguishes them).
            // An owned trait object is the same fat pair the borrowed one is:
            // the C struct is `{ void* data; const VTable* vtable; }` either
            // way, and the borrow checker, not the C type, is what tells the
            // owning value from the borrowed one.
            Ty::Dyn { trait_name, .. } | Ty::BoxDyn { trait_name } => dyn_c_type(trait_name),
            Ty::Ref { mutable, inner } => {
                if *mutable {
                    format!("{}*", inner.c_type())
                } else if matches!(**inner, Ty::Mutex(_)) {
                    // A `Mutex<T>` is itself one pointer, so a shared borrow of
                    // one is a pointer to a pointer and the `const` the shared
                    // spelling normally adds would make `&m` and the
                    // parameter's `&Mutex<T>` two incompatible C types. The
                    // borrow checker, not the C qualifier, is what stops a
                    // write through a `&Mutex<T>`.
                    format!("{}*", inner.c_type())
                } else {
                    format!("const {}*", inner.c_type())
                }
            }
        }
    }

    pub fn display(&self) -> String {
        match self {
            Ty::I32 => "i32".to_string(),
            Ty::I64 => "i64".to_string(),
            Ty::U32 => "u32".to_string(),
            Ty::U64 => "u64".to_string(),
            Ty::F32 => "f32".to_string(),
            Ty::F64 => "f64".to_string(),
            Ty::Bool => "bool".to_string(),
            Ty::Char => "char".to_string(),
            Ty::Unit => "unit".to_string(),
            Ty::Str => "str".to_string(),
            Ty::U8 => "u8".to_string(),
            Ty::U16 => "u16".to_string(),
            Ty::Usize => "usize".to_string(),
            Ty::Isize => "isize".to_string(),
            Ty::Bytes => "Bytes".to_string(),
            Ty::String => "String".to_string(),
            Ty::View => "View".to_string(),
            Ty::Struct(name) => name.clone(),
            Ty::List(elem) => format!("List<{}>", elem.display()),
            Ty::Box(elem) => format!("Box<{}>", elem.display()),
            Ty::Atomic(elem) => format!("Atomic<{}>", elem.display()),
            Ty::Thread(elem) => format!("Thread<{}>", elem.display()),
            Ty::Mutex(elem) => format!("Mutex<{}>", elem.display()),
            Ty::Handle(name) => name.clone(),
            Ty::Ptr(elem) => format!("Ptr<{}>", elem.display()),
            Ty::Array(elem, len) => format!("[{}; {}]", elem.display(), len),
            Ty::Map(key, value) => format!("Map<{}, {}>", key.display(), value.display()),
            Ty::Dyn { mutable: true, trait_name } => format!("&mut dyn {trait_name}"),
            Ty::Dyn { mutable: false, trait_name } => format!("&dyn {trait_name}"),
            Ty::BoxDyn { trait_name } => format!("Box<dyn {trait_name}>"),
            Ty::Ref { mutable: true, inner } => format!("mut {}", inner.display()),
            Ty::Ref { mutable: false, inner } => format!("&{}", inner.display()),
            Ty::Fn { params, ret } => {
                // The return is written `.R: T` exactly as a declaration
                // writes it, and a `Unit` return is left off — so a written
                // function type and a signature read the same way.
                let mut s = format!("fun({})", join_types(params));
                if **ret != Ty::Unit {
                    s.push_str(&format!(".R: {}", ret.display()));
                }
                s
            }
            Ty::Closure { params, ret } => {
                // The closure type reads exactly as the function type does,
                // one keyword apart, because the two are called identically
                // and differ only in what the value owns.
                let mut s = format!("closure({})", join_types(params));
                if **ret != Ty::Unit {
                    s.push_str(&format!(".R: {}", ret.display()));
                }
                s
            }
            Ty::ScopedFn { params, ret } => {
                let mut s = format!("scoped fun({})", join_types(params));
                if **ret != Ty::Unit {
                    s.push_str(&format!(".R: {}", ret.display()));
                }
                s
            }
        }
    }

    /// The C declaration of a variable called `name` with this type: the C
    /// type followed by the name, except for a function value, where the name
    /// goes *inside* the pointer declarator (`int32_t (*kelp_f)(int32_t)`).
    /// Every declaration site — a parameter, a `val`/`var`, a `datac` field,
    /// an enum payload, a passdown — goes through this rather than pairing
    /// `c_type()` with a name by hand, because a function type is the one type
    /// where that pairing does not produce valid C.
    pub fn c_decl(&self, name: &str) -> String {
        match self {
            Ty::Fn { params, ret } => {
                format!("{} (*{name})({})", ret.c_type(), fn_params_c(params))
            }
            other => format!("{} {name}", other.c_type()),
        }
    }
}

/// `ret (*)(params)`'s parameter half: `void` for none, the C parameter types
/// otherwise. Shared with `c_decl`, since a declarator and a bare function
/// pointer type differ only in where the name goes, and with `codegen_c.rs`,
/// which writes the same list for a function that returns a function value. A
/// `unit` parameter is not written, for the same reason a `unit` parameter of
/// a `fun` declaration is not (`codegen_c.rs::write_params`): no value crosses
/// for it, so a pointer to such a function takes exactly the parameters the
/// callee's own definition takes.
pub(crate) fn fn_params_c(params: &[Ty]) -> String {
    if params.iter().all(|p| *p == Ty::Unit) {
        "void".to_string()
    } else {
        params
            .iter()
            .filter(|p| **p != Ty::Unit)
            .map(|p| p.c_type())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// `T, U` for a diagnostic or a written type.
fn join_types(types: &[Ty]) -> String {
    types.iter().map(|t| t.display()).collect::<Vec<_>>().join(", ")
}

/// The mangled signature fragment a closure instantiation is named by:
/// `i32__i32` is `closure(i32).R: i32`. Parameters first, in order, then the
/// return — the same order `ty_mangle`'s function-type arm uses, under a
/// different prefix.
pub fn closure_mangle(params: &[Ty], ret: &Ty) -> String {
    format!(
        "Closure__{}{}",
        params.iter().map(|p| format!("{}__", ty_mangle(p))).collect::<String>(),
        ty_mangle(ret)
    )
}

/// The `(params, ret)` of a closure-shaped signature, whichever of the three
/// spellings it is (`fun`, `closure`, `scoped fun`). `None` for anything else.
pub fn closure_parts(ty: &Ty) -> Option<(&[Ty], &Ty)> {
    match ty {
        Ty::Fn { params, ret }
        | Ty::Closure { params, ret }
        | Ty::ScopedFn { params, ret } => Some((params.as_slice(), ret)),
        _ => None,
    }
}

/// The mangled signature fragment of a closure-shaped type. Shared by the
/// closure machinery and the `Mutex.lock` body, which is one body per
/// signature whichever spelling produced it (B-5).
pub fn closure_mangle_of(ty: &Ty) -> String {
    match closure_parts(ty) {
        Some((params, ret)) => closure_mangle(params, ret),
        None => unreachable!("not a closure signature: {}", ty.display()),
    }
}

/// The `Ty::Closure` spelling of a closure-shaped signature, so the body emitted
/// for a `scoped fun(...)` block shares the key a thread's `closure(...)` entry
/// uses (B-5).
pub fn closure_shape(ty: &Ty) -> Ty {
    match ty {
        Ty::Fn { params, ret }
        | Ty::Closure { params, ret }
        | Ty::ScopedFn { params, ret } => Ty::Closure {
            params: params.clone(),
            ret: ret.clone(),
        },
        other => other.clone(),
    }
}

/// The generated C struct tag for a closure instantiation (A-4 phase E):
/// `{ fn, env, drop }`, where `fn` takes the environment as its hidden first
/// parameter. Monomorphized per signature, like a container, so the C
/// function-pointer member is typed rather than `void*`.
pub fn closure_c_type(params: &[Ty], ret: &Ty) -> String {
    format!("kelp_{}", closure_mangle(params, ret))
}

/// The name of one field of a literal's environment (`c0`, `c1`, …). One
/// place names them, because `infer.rs` writes the environment's field list
/// and `codegen_c.rs` emits the reads.
pub fn capture_field(index: usize) -> String {
    format!("c{index}")
}

/// The generated C struct tag for a literal's environment, and the function
/// that frees it (A-4 phase E). One place names both, because `infer.rs`
/// records them on the generated function and `codegen_c.rs` emits them.
pub fn env_struct_name(literal_key: &str) -> String {
    format!("kelp_Env__{}", c_ident(literal_key))
}

/// See `env_struct_name`.
pub fn env_drop_name(literal_key: &str) -> String {
    format!("kelp_env_drop__{}", c_ident(literal_key))
}

/// Which user-declared types own heap memory, transitively: a `datac` whose
/// field owns memory, or an `enum` with one such payload in any variant.
///
/// A type may only name a type declared before it *by value* (open item 12),
/// so one pass in merged declaration order is already a valid fixpoint — no
/// repeating until stable. Merged order matters here for the first time in
/// A-1: a `datac` and an `enum` may each name the other's earlier-declared
/// half, so the two declaration lists have to be walked as one sequence.
///
/// `drop_types` is the set of user types that declare their own destructor
/// (kernel stage 9, MK9-S6-M1); each is seeded into the result, because a
/// destructor type needs a `kelp_drop_<T>` to hold the user call even when it
/// owns no memory, and a later type that holds one by value then owns memory
/// through it.
pub fn owned_types(
    structs: &[StructDef],
    enums: &[EnumDef],
    drop_types: &std::collections::HashSet<String>,
) -> std::collections::HashSet<String> {
    enum Decl<'a> {
        Record(&'a StructDef),
        Sum(&'a EnumDef),
    }
    let mut decls: Vec<(usize, Decl)> = structs
        .iter()
        .map(|s| (s.order, Decl::Record(s)))
        .chain(enums.iter().map(|e| (e.order, Decl::Sum(e))))
        .collect();
    decls.sort_by_key(|(order, _)| *order);

    let mut owned = drop_types.clone();
    for (_, decl) in decls {
        let (name, fields_own) = match decl {
            Decl::Record(s) => (
                &s.name,
                s.fields.iter().any(|f| needs_drop(&f.ty, &owned)),
            ),
            // An `enum` owns memory as soon as *one* variant does: the drop
            // function switches on the live tag, so any owned payload means
            // the value needs a drop call.
            Decl::Sum(e) => (
                &e.name,
                e.variants
                    .iter()
                    .flat_map(|v| v.fields.iter())
                    .any(|f| needs_drop(&f.ty, &owned)),
            ),
        };
        if fields_own {
            owned.insert(name.clone());
        }
    }
    owned
}

/// Whether a `List<T>` element type has an order, so `List.sort` means
/// something. Numbers and `bool` compare by value; `String`/`Bytes` compare
/// byte-wise (length-aware, not `strcmp` — an embedded NUL is possible). A
/// `datac` element has no defined order, and a borrow can never be an element
/// at all.
pub fn list_sortable_elem(ty: &Ty) -> bool {
    ty.is_numeric() || matches!(ty, Ty::Bool | Ty::String | Ty::Bytes)
}
/// Whether a `List<T>` element type has equality, so `List.contains` means
/// something. Same set as `list_sortable_elem`, for the same reason: a
/// `datac` has no `==` in v1.
pub fn list_comparable_elem(ty: &Ty) -> bool {
    list_sortable_elem(ty)
}

/// Whether one field, payload or `List` element of type `ty` can be deep-copied
/// once its user types can. `clonable` holds the `datac`/`enum` names that can
/// be (see `clonable_types`). This is the single gate for every copy *out* of a
/// container, `List.slice`/`List.clone` and `Map.get_or`/`Map.values` alike.
pub fn value_is_clonable(ty: &Ty, clonable: &std::collections::HashSet<String>) -> bool {
    ty.is_copy()
        || matches!(ty, Ty::String | Ty::Bytes)
        || matches!(ty, Ty::Struct(n) if clonable.contains(n))
}

/// Whether one field or payload of type `ty` has an order, once its user types
/// have. `String`/`Bytes` compare byte-wise and length-aware; a `datac`/`enum`
/// compares field by field in declaration order.
pub fn value_is_sortable(ty: &Ty, sortable: &std::collections::HashSet<String>) -> bool {
    ty.is_numeric()
        || matches!(ty, Ty::Bool | Ty::Char | Ty::String | Ty::Bytes)
        || matches!(ty, Ty::Struct(n) if sortable.contains(n))
}

/// Whether one field, payload or `datac`/`enum` value of type `ty` can be a
/// `Map` key once its user types can. This is the derived key set: the same
/// scalars `is_map_key` names, plus a `datac`/`enum` every one of whose fields
/// or variant payloads is itself keyable.
pub fn value_is_keyable(ty: &Ty, keyable: &std::collections::HashSet<String>) -> bool {
    is_map_key(ty) || matches!(ty, Ty::Struct(n) if keyable.contains(n))
}

/// The user types whose value is a key *by derivation* (A-6): the least
/// fixpoint in which a `datac`/`enum` is keyable once every field or payload it
/// holds is. The same shape as `clonable_types`/`sortable_types`, over the same
/// declaration graph, minus the types in `declared`.
///
/// `declared` names the types that declare their own `hash`/`eq` pair (A-12).
/// A declared pair is used *instead of* a derived hash and equality, so such a
/// type is never in this set, exactly as if it were not keyable at all: the
/// derived pair the backends emit is decided by this set alone, and a field
/// that names one of these types is therefore not keyable by derivation either.
pub fn keyable_types(
    decls: &[(String, Vec<Ty>)],
    declared: &std::collections::HashSet<String>,
    drop_types: &std::collections::HashSet<String>,
) -> std::collections::HashSet<String> {
    let plain: Vec<(String, Vec<Ty>)> = decls
        .iter()
        .filter(|(name, _)| !declared.contains(name) && !drop_types.contains(name))
        .cloned()
        .collect();
    fixpoint(&plain, value_is_keyable)
}

/// The names a `Map` key may be, as `is_map_key_with` asks the question: the
/// derived key set plus the types that declare their own pair (A-12). A `Map`
/// takes a derived key and a declaring one, so this set is the union of the
/// two.
pub fn map_key_types(
    decls: &[(String, Vec<Ty>)],
    declared: &std::collections::HashSet<String>,
    drop_types: &std::collections::HashSet<String>,
) -> std::collections::HashSet<String> {
    let mut set = keyable_types(decls, declared, drop_types);
    set.extend(declared.iter().filter(|n| !drop_types.contains(*n)).cloned());
    set
}

/// The user types whose value can be deep-copied: the least fixpoint in which a
/// `datac`/`enum` is clonable once every field or payload it holds is. `decls`
/// is one `(name, each field's type)` per declaration, including generic
/// instances.
///
/// A type that reaches itself by value stays out of the set, which is the
/// honest answer: v1's declaration rule already forbids a directly recursive
/// `datac`, and a recursion through a `Box`/`List` is not a shape a plain copy
/// can follow without a decision about what the copy owns.
pub fn clonable_types(
    decls: &[(String, Vec<Ty>)],
    drop_types: &std::collections::HashSet<String>,
) -> std::collections::HashSet<String> {
    let plain: Vec<(String, Vec<Ty>)> = decls
        .iter()
        .filter(|(name, _)| !drop_types.contains(name))
        .cloned()
        .collect();
    fixpoint(&plain, value_is_clonable)
}

/// The same fixpoint for the order a `datac`/`enum` gets when it is sorted:
/// fields in declaration order, a `String`/`Bytes` byte-wise, an `enum` by tag
/// and then by the live variant's payload. A `List`/`Box`/`Map` field is not
/// ordered, so a record that holds one is not sortable either.
pub fn sortable_types(decls: &[(String, Vec<Ty>)]) -> std::collections::HashSet<String> {
    fixpoint(decls, value_is_sortable)
}

fn fixpoint(
    decls: &[(String, Vec<Ty>)],
    ok: fn(&Ty, &std::collections::HashSet<String>) -> bool,
) -> std::collections::HashSet<String> {
    let mut set: std::collections::HashSet<String> = std::collections::HashSet::new();
    loop {
        let mut changed = false;
        for (name, fields) in decls {
            if set.contains(name) {
                continue;
            }
            if fields.iter().all(|f| ok(f, &set)) {
                set.insert(name.clone());
                changed = true;
            }
        }
        if !changed {
            return set;
        }
    }
}

/// The generated C function that deep-copies a `datac`/`enum` from `src` into
/// `dst` (the caller's storage, which must not already own anything).
pub fn clone_fn_name(ty: &Ty) -> String {
    match ty {
        Ty::Struct(name) => format!("kelp_clone_{}", c_ident(name)),
        other => unreachable!("no clone rule for type '{}'", other.display()),
    }
}

/// The generated C function that orders two values of a `datac`/`enum`: the
/// sign of the difference, like `memcmp`.
pub fn cmp_fn_name(ty: &Ty) -> String {
    match ty {
        Ty::Struct(name) => format!("kelp_cmp_{}", c_ident(name)),
        other => unreachable!("no order rule for type '{}'", other.display()),
    }
}

/// The generated function that hashes a keyable `datac`/`enum` key: the
/// combine helper folded over the tag (an `enum`) and then each field.
pub fn hash_fn_name(ty: &Ty) -> String {
    match ty {
        Ty::Struct(name) => format!("kelp_hash_{}", c_ident(name)),
        other => unreachable!("no hash rule for type '{}'", other.display()),
    }
}

/// The generated function that tests two keyable `datac`/`enum` keys for
/// equality, the authority the map's probe consults.
pub fn eq_fn_name(ty: &Ty) -> String {
    match ty {
        Ty::Struct(name) => format!("kelp_eq_{}", c_ident(name)),
        other => unreachable!("no equality rule for type '{}'", other.display()),
    }
}

/// The two method names a `datac`/`enum` declares to be its own `Map` key pair
/// (A-12): `fun T.hash().R: u64` and `fun T.eq(other: &T).R: bool`. The names
/// belong to the pair rather than to the type, which is why declaring either
/// one is read as a statement about the type's key.
pub const KEY_PAIR_HASH_METHOD: &str = "hash";
pub const KEY_PAIR_EQ_METHOD: &str = "eq";

/// The canonical key of one trait *instance* (A-12 slice 6): the trait's
/// canonical name, plus its arguments when it has any (`pkg.From<pkg.IoErr>`).
/// A trait with no type parameters keys by its name alone, exactly as it did
/// before generic traits existed, so nothing about an ordinary trait changes.
/// The key is also what a static impl method's function name is suffixed with,
/// which is how `From<IoErr>`'s `from` and `From<String>`'s `from` for one type
/// stay distinct.
pub fn trait_instance_key(name: &str, args: &[String]) -> String {
    if args.is_empty() {
        name.to_string()
    } else {
        format!("{name}<{}>", args.join(","))
    }
}

/// One half of a declared key pair as `sigs`/`tir` spell it: the `Type#name`
/// key `infer.rs`'s `sig_key` gives every receiver method. The `Map`'s call
/// target is this key mangled by `c_ident`, so it names the method's own
/// declaration and the two cannot drift.
pub fn key_pair_method_key(type_name: &str, method: &str) -> String {
    format!("{}#{}", type_name, method)
}

/// The destructor's method name (kernel stage 9, MK9-S6-M1). Like the two
/// halves of a key pair, the name *is* the declaration: `fun mut T.drop()` on
/// a user type makes that type's own drop run at every drop point. Both
/// backends reach the body through the same `Type#name` key `sig_key` gives
/// every receiver method.
pub const DROP_METHOD: &str = "drop";

/// The `sigs`/`tir` key of a type's declared destructor.
pub fn drop_method_key(type_name: &str) -> String {
    format!("{}#{}", type_name, DROP_METHOD)
}

/// The canonical names of the shipped operator traits (`std/ops`, language-
/// completeness 5). `infer.rs` lowers `+`/`-`/`*`/`/`/`%`, unary `-` and the
/// order operators through these, so the name is a fixed spelling rather than
/// "whatever trait declares a method called `add`".
pub const ADD_TRAIT: &str = "std.ops.Add";
pub const SUB_TRAIT: &str = "std.ops.Sub";
pub const MUL_TRAIT: &str = "std.ops.Mul";
pub const DIV_TRAIT: &str = "std.ops.Div";
pub const REM_TRAIT: &str = "std.ops.Rem";
pub const NEG_TRAIT: &str = "std.ops.Neg";
pub const ORD_TRAIT: &str = "std.ops.Ord";

/// The canonical name of `std/fmt`'s `Display`, the trait a value implements
/// to appear in `"${x}"` and as the argument of `print`/`print!`.
pub const DISPLAY_TRAIT: &str = "std.fmt.Display";

/// The emitted symbol of a type's declared destructor, the same spelling on
/// both backends (`codegen_c.rs::write_signature` and
/// `codegen_llvm.rs::symbol_name` both mangle the `Type#name` key the same
/// way). The generated `kelp_drop_<T>` calls this at the top of its body.
pub fn user_drop_fn_name(type_name: &str) -> String {
    format!("kelp_{}", c_ident(&drop_method_key(type_name)))
}

/// Whether `ty` is a user type that declares its own destructor (kernel stage
/// 9, MK9-S6-M1). A partial move of a field or payload of such a type is
/// refused by `borrowck.rs`: the move zeroes the slot it came from and the
/// base's own drop still calls the generated drop on the zero value, which for
/// a destructor type would run the user body a second time.
pub fn is_drop_type(ty: &Ty, drop_types: &std::collections::HashSet<String>) -> bool {
    matches!(ty, Ty::Struct(name) if drop_types.contains(name))
}

/// Whether a value of `ty` has to be freed — the builtin owned types, or a
/// `datac` that (transitively) holds one.
pub fn needs_drop(ty: &Ty, owned_structs: &std::collections::HashSet<String>) -> bool {
    match ty {
        Ty::Struct(name) => owned_structs.contains(name),
        // An array is freed element by element, so it needs a drop call
        // exactly when its element does.
        Ty::Array(elem, _) => needs_drop(elem, owned_structs),
        other => other.owns_heap(),
    }
}

/// The C function that frees a value of `ty`, taking a pointer to it.
/// Generated per `datac` by `codegen_c.rs`; the builtins are in the fixed
/// runtime preamble.
pub fn drop_fn_name(ty: &Ty) -> String {
    match ty {
        Ty::Bytes => "kelp_bytes_drop".to_string(),
        Ty::String => "kelp_string_drop".to_string(),
        // One runtime function for every element type: a handle detaches the
        // worker and frees nothing, so the element type is not part of the
        // step (MK9-S3-M1 stage C1).
        Ty::Thread(_) => "kelp_thread_drop".to_string(),
        // One runtime function for every element type: the record carries the
        // element drop pointer `Mutex.new` stored, so the element type is not
        // part of the step (B-5).
        Ty::Mutex(_) => "kelp_mutex_drop".to_string(),
        Ty::Struct(name) => format!("kelp_drop_{}", c_ident(name)),
        Ty::List(elem) => format!("kelp_list_drop__{}", ty_mangle(elem)),
        Ty::Box(elem) => format!("kelp_box_drop__{}", ty_mangle(elem)),
        // One drop per trait: the value behind the data pointer is dropped
        // through the vtable's own drop entry, so the concrete type is not part
        // of the function's name (A-12 slice 7).
        Ty::BoxDyn { trait_name } => box_dyn_drop_fn(trait_name),
        Ty::Array(elem, len) => format!("kelp_array_drop__{}__{}", ty_mangle(elem), len),
        Ty::Map(key, value) => {
            format!("kelp_map_drop__{}__{}", ty_mangle(key), ty_mangle(value))
        }
        Ty::Closure { params, ret } => {
            format!("kelp_closure_drop__{}", closure_mangle(params, ret))
        }
        // A scoped function value owns nothing (its environment is stack and
        // its drop is NULL), so it has no drop function to name.
        other => unreachable!("no drop rule for type '{}'", other.display()),
    }
}

/// Whether `ty` may be a `List<T>` element. A borrow cannot be one (an
/// element outlives the borrow that produced it, and v1 has no lifetime
/// syntax to say otherwise), and neither can `str` (the same borrowed view),
/// `View`, `unit`, a function value, an atomic, or a `Map`.
///
/// A `List<T>` and a `Box<T>` *are* elements (A-10 slice 1), but only once
/// the container's own element type is decided: an element lives in the list's
/// own buffer, so the element type has to be a complete, droppable C type with
/// a name, and a container that still carries the open-container marker has
/// neither. `container_depth` in `codegen_c.rs` emits a nested container
/// before the one that contains it, so every typedef and every function the
/// outer list calls already exists. A `Map` stays out because a map value is
/// not a container either.
pub fn is_list_element(ty: &Ty) -> bool {
    match ty {
        Ty::List(_) | Ty::Box(_) => !contains_open_container(ty),
        // An owned trait object is a complete two-word struct with its own
        // drop, so a list can hold one and free it element by element.
        Ty::BoxDyn { .. } => true,
        Ty::Bytes | Ty::String | Ty::Struct(_) => true,
        // A handle is pointer-sized and `Copy`, so it lives in a list's
        // buffer as a pointer and needs no element drop, exactly as a scalar
        // does.
        Ty::Handle(_) => true,
        Ty::Ref { .. } | Ty::Str | Ty::View | Ty::Unit | Ty::Map(..) => false,
        // A raw pointer is pointer-sized and `Copy`, so it lives inline in a
        // list's buffer and needs no element drop, exactly as a handle does.
        Ty::Ptr(_) => true,
        other => other.is_numeric() || matches!(other, Ty::Bool | Ty::Char),
    }
}

/// Whether `ty` is a container that still carries the open-container marker in
/// its element slot, or a `List`/`Box` that holds such a container inside it.
/// This walks `List`/`Box` elements only, because those are the two kinds that
/// were widened to a container (A-10 slice 1) and a `Map` value is never an
/// element.
///
/// The marker is `Unit` where an element type goes (see `is_open_container`),
/// and no program wrote it: the generated typedef, the `push` prototype and the
/// drop all name the element type, so a container that still carries the marker
/// cannot be an element. `is_list_element` and `is_box_element` therefore
/// refuse it at any depth: an outer container naming an inner one would only
/// carry the marker further in.
pub fn contains_open_container(ty: &Ty) -> bool {
    match ty {
        Ty::List(elem) | Ty::Box(elem) => **elem == Ty::Unit || contains_open_container(elem),
        _ => false,
    }
}

/// Whether `ty` may be a `Map<K, V>` value: a scalar, `String`, `Bytes`, or a
/// `datac`. Narrower than the `List` element set, which also takes a `List` or
/// a `Box` (A-10 slice 1): a map value sits inline in the entry but is read and
/// written as a plain location by the map's own routines, so a container value
/// is left out for now. `Map<String, Map<i32>>` and `Map<String, List<i32>>`
/// stay impossible.
pub fn is_map_element(ty: &Ty) -> bool {
    is_plain_element(ty)
}

/// The element set a `Map` value and an array element share, and the set the
/// `List` element check held before A-10 slice 1 widened it: a scalar,
/// `String`, `Bytes`, or a `datac`. Both stores read and write an element as a
/// plain location, so a container, a borrow or a view, an atomic, a function
/// value, and `unit` are all out.
fn is_plain_element(ty: &Ty) -> bool {
    match ty {
        Ty::Bytes | Ty::String | Ty::Struct(_) => true,
        // A pointer is one machine word, so it is a plain element on the same
        // footing a handle is.
        Ty::Ptr(_) => true,
        Ty::Ref { .. } | Ty::Str | Ty::View | Ty::Unit | Ty::List(_) | Ty::Box(_) | Ty::Map(..) => {
            false
        }
        other => other.is_numeric() || matches!(other, Ty::Bool | Ty::Char),
    }
}

/// Whether `ty` may be an array element or an array itself: `[T; N]` holds its
/// elements inline, so the element set is a scalar, `String`, `Bytes`, a
/// `datac`, or another array, so a nested `[[u8; 4]; 8]` is a page table and is
/// as expressible as it is in C. A `List`/`Box`/`Map` element stays refused
/// even now that a `List` element may be one: an array element is held inline,
/// so `[[u8; 4]; 8]` stays a fixed buffer, while a container element would make
/// the array a variable-size thing it is not. A borrow is refused for the
/// reason it always is: an element outlives the borrow it came from.
pub fn is_array_element(ty: &Ty) -> bool {
    match ty {
        Ty::Array(elem, _) => is_array_element(elem),
        other => is_plain_element(other),
    }
}

/// Whether `ty` names a key the compiler itself can hash and compare, with no
/// declaration graph consulted: `String` (byte-wise, with a length so an
/// embedded NUL is safe), the integers, `bool`, and `char`. A float is refused
/// because `nan != nan` would make an inserted key unfindable, and a container
/// or a borrow is refused because neither has a hash and an equality the
/// compiler can derive.
///
/// A `datac`/`enum` key is not answered here: whether one is keyable depends on
/// its fields, so a caller that has the declaration graph uses
/// `is_map_key_with` instead. This is the builtin half, and `is_map_key_with`
/// is the two of them together.
pub fn is_map_key(ty: &Ty) -> bool {
    matches!(ty, Ty::String | Ty::Bool | Ty::Char) || ty.is_integer()
}

/// Whether `ty` may be a `Map<K, V>` key, given the user types a key may be
/// (`map_key_types`): the builtin key set, or a `datac`/`enum` that is keyable
/// by derivation or that declares its own `hash`/`eq` pair (A-12). The key's
/// hash and its equality are whichever pair the type has, so nothing here
/// needs a trait or a comparator; a type outside the set is refused.
pub fn is_map_key_with(ty: &Ty, keyable: &std::collections::HashSet<String>) -> bool {
    value_is_keyable(ty, keyable)
}

/// Whether `ty` may be a `Box<T>` element. A box holds a *pointer*, so the
/// element may be anything with a size, including another container (as long
/// as that container's own element type is decided, see
/// `contains_open_container`) and, crucially, the `datac` the box is a field
/// of (that is how a recursive type is written). A borrow or a `str` view is
/// still refused: a box outlives the borrow it was made from, and there is
/// nowhere to put that lifetime.
pub fn is_box_element(ty: &Ty) -> bool {
    match ty {
        // A closure is an ordinary C struct, and it is emitted before every
        // `Box` instantiation that names one (A-4 phase E), so a box is the
        // one container that can hold a closure — `Box<closure().R: i32>` is
        // how an *optional* callback is written (`Box.empty()`).
        Ty::Bytes
        | Ty::String
        | Ty::Struct(_)
        | Ty::Closure { .. }
        // A fixed-size array is a plain inline struct, so a box can hold one
        // (A-18): `Box<[u8; 4096]>` is a heap allocation of a fixed-size
        // buffer, which is how one is kept alive past its frame.
        | Ty::Array(..)
        // A handle is a pointer, so a box of one is a pointer to a pointer:
        // `Box<Widget>` is the owned spelling of "an optional handle".
        | Ty::Handle(_)
        // A raw pointer is the same shape: `Box<Ptr<i32>>` is a pointer to a
        // pointer, held on the heap.
        | Ty::Ptr(_) => {
            true
        }
        // A `Box` holds a pointer, so its own element slot may hold another
        // container, but only one whose element type is decided: the marker
        // would be a `void` element the generated routines cannot name.
        Ty::List(_) | Ty::Box(_) => !contains_open_container(ty),
        // A `View` is a borrow of text that outlives the box, with nowhere to
        // put the lifetime — the same reason `str` is refused (A-8).
        Ty::Ref { .. } | Ty::Str | Ty::View | Ty::Unit | Ty::Dyn { .. } => false,
        other => other.is_numeric() || matches!(other, Ty::Bool | Ty::Char),
    }
}

/// The C name fragment for a container's element type: `List<i32>` becomes
/// `i32`, `List<shapes.circle.Circle>` becomes `shapes__circle__Circle`
/// (through the same `c_ident` mangling every other generated symbol uses),
/// and a nested container nests the fragment (`Box<List<i32>>` becomes
/// `Box__List__i32`). One function for both containers, so a name can never
/// collide between them and a nested one cannot produce an invalid C
/// identifier.
pub fn ty_mangle(ty: &Ty) -> String {
    match ty {
        Ty::Struct(name) => c_ident(name),
        Ty::Bytes => "Bytes".to_string(),
        Ty::String => "String".to_string(),
        Ty::List(elem) => format!("List__{}", ty_mangle(elem)),
        Ty::Box(elem) => format!("Box__{}", ty_mangle(elem)),
        Ty::Map(key, value) => format!("Map__{}__{}", ty_mangle(key), ty_mangle(value)),
        Ty::Array(elem, len) => format!("Array__{}__{}", ty_mangle(elem), len),
        Ty::Atomic(elem) => format!("Atomic__{}", ty_mangle(elem)),
        Ty::Thread(elem) => format!("Thread__{}", ty_mangle(elem)),
        Ty::Mutex(elem) => format!("Mutex__{}", ty_mangle(elem)),
        // The canonical name is module-qualified (`ffi.Widget`), so it goes
        // through the same mangling every other generated symbol uses rather
        // than straight into a C identifier.
        Ty::Handle(name) => format!("Handle__{}", c_ident(name)),
        // A borrowed trait object is a struct per trait, so its mangled name is
        // the trait's, under its own prefix.
        Ty::Dyn { trait_name, .. } => format!("Dyn__{}", c_ident(trait_name)),
        Ty::BoxDyn { trait_name } => format!("BoxDyn__{}", c_ident(trait_name)),
        // A `Ptr<T>` type argument names the same C type `c_type` does, under
        // the mangling every other generated symbol uses.
        Ty::Ptr(elem) => format!("Ptr__{}", ty_mangle(elem)),
        // A function type never reaches a container or a type argument (both
        // are refused in `infer.rs`), but it is reached by `drop_fn_name`'s
        // unreachable arm and by diagnostics, so it gets an identifier-shaped
        // answer rather than a string with `(`/`)`/`:` in it.
        Ty::Fn { params, ret } => format!(
            "Fn__{}{}",
            params.iter().map(|p| format!("{}__", ty_mangle(p))).collect::<String>(),
            ty_mangle(ret)
        ),
        // A closure signature mangles the same way, under its own prefix, so
        // a program using both spellings of one signature gets two distinct C
        // types — which is the point: one is a pointer, the other owns an
        // environment.
        Ty::Closure { params, ret } => closure_mangle(params, ret),
        // A scoped signature is the same fat pointer, so it shares the
        // closure's C type rather than getting a second, identical typedef.
        Ty::ScopedFn { params, ret } => closure_mangle(params, ret),
        // Everything else is a scalar, `View`/`unit`, or a borrow. A scalar's
        // written form is already an identifier; a borrow's is `&T` or
        // `mut T`, and `&T` is spelled `refT` here. What that spelling could
        // *not* survive is a module-qualified inner type: `&json.Item` came
        // out `refjson.Item`, a name with a `.` in it, so the closure typedef
        // for `closure(&Item)` declared inside the `json` module itself was
        // `kelp_Closure__refjson.Item__unit` — not a C identifier at all, and
        // every generated use of it failed to compile. That is the shape a
        // library hits when it offers a callback over its own type, which is
        // why the fallback goes through `c_ident`, the one mangling every
        // other generated symbol uses, instead of replacing `&` by hand.
        other => c_ident(&other.display().replace('&', "ref")),
    }
}

/// The generated C struct tag for `List<T>`.
pub fn list_c_type(elem: &Ty) -> String {
    format!("kelp_List__{}", ty_mangle(elem))
}

/// The generated C struct tag for `Box<T>`.
pub fn box_c_type(elem: &Ty) -> String {
    format!("kelp_Box__{}", ty_mangle(elem))
}

/// The generated C struct tag for `Map<K, V>`.
pub fn map_c_type(key: &Ty, value: &Ty) -> String {
    format!("kelp_Map__{}__{}", ty_mangle(key), ty_mangle(value))
}

/// The generated C struct tag for `[T; N]` (A-18).
///
/// The storage is inline, so this is an ordinary struct wrapping a C array:
/// `typedef struct { T v[N]; } kelp_Array__T__N;`. The wrapper (rather than a
/// bare C array) is what makes the value obey Kelp's rules in C: assignment
/// copies it, a function can take and return it by value, and nothing decays
/// to a pointer behind the type system's back.
pub fn array_c_type(elem: &Ty, len: u64) -> String {
    format!("kelp_Array__{}__{}", ty_mangle(elem), len)
}

/// The generated C struct tag for `Atomic<T>`.
///
/// The storage is one plain machine word: `typedef struct { T v; } kelp_Atomic__T;`. Nothing marks the member `_Atomic` because every access is
/// written as an `__atomic_*` builtin (or, for the load, the same builtin)
/// rather than as a plain read or write, which is the property this type
/// exists to have. A plain member would not force anyone writing code by hand
/// to reach for an atomic operation, but nothing by hand writes this type:
/// every access is a call the two backends lower to one instruction.
pub fn atomic_c_type(elem: &Ty) -> String {
    format!("kelp_Atomic__{}", ty_mangle(elem))
}

/// The generated C type for `Thread<T>` (MK9-S3-M1 stage C1): the handle is one
/// pointer to the worker record `thread.spawn` allocates, so the C type is
/// uniform over the element type exactly as `str` is uniform over what it
/// points at. The element type stays in the Kelp type, which is what decides
/// the result slot's size and the `Result<T, ThreadErr>` a join builds, so it
/// is deliberately not part of this name.
pub fn thread_c_type() -> String {
    "kelp_Thread*".to_string()
}

/// The C type for `Mutex<T>` (B-5, `std/sync`): one pointer to the runtime's
/// `kelp_Mutex` record, uniform over the element type for the same reason
/// `Thread<T>` is. The element type stays in the Kelp type, where it decides
/// what the generated `new` copies in and what `into_inner` hands back; the
/// record's own layout, including the `T` slot, is `kelp_rt.c`'s.
pub fn mutex_c_type() -> String {
    "kelp_Mutex*".to_string()
}

/// The generated C typedef for one opaque handle. The typedef is what gives
/// `void*` a name, so two handles are two distinct C types a reader can trace
/// back to their declarations.
pub fn handle_c_type(name: &str) -> String {
    format!("kelp_Handle__{}", c_ident(name))
}

/// The synthetic callee keys `infer.rs` lowers the two handle operations to.
/// A call's callee is a string that codegen matches, and these two never come
/// from a declaration, so they use a spelling no Kelp identifier can have.
/// `null` carries no argument and codegen reads the handle type off the call's
/// own `Ty::Handle`; `is_null` carries the handle as its one argument.
pub const HANDLE_NULL_KEY: &str = "$handle$null";
pub const HANDLE_IS_NULL_KEY: &str = "$handle$is_null";

/// Whether `ty` may be a `Ptr<T>`'s element: any type with a C/LLVM layout
/// that is one machine-level value, which is what a raw address can point at.
///
/// The owned heap types (`String`, `Bytes`, `List`, `Box`, `Map`), the borrowed
/// ones (`str`, `View`, a reference), a closure, a function value and the
/// builtin fixed-address `Atomic<T>` are all refused: none of them is one
/// machine value, and a pointer to one would be a pointer into a structure whose
/// shape the language has not promised.
pub fn is_ptr_element(ty: &Ty) -> bool {
    match ty {
        Ty::Struct(_) | Ty::Handle(_) | Ty::Ptr(_) => true,
        // An array is inline storage with a fixed size, so a pointer to one is
        // an ordinary pointer to a block.
        Ty::Array(elem, _) => is_ptr_element(elem),
        other => other.is_numeric() || matches!(other, Ty::Bool | Ty::Char),
    }
}

/// What a `Bytes.read_*`/`Bytes.write_*` method name says about the field it
/// moves: the field's byte width, whether the bytes in the buffer are
/// big-endian, and the Kelp type the value has on either side (kernel stage 3,
/// MK9-S8). `None` for any other name, which is how both code generators tell
/// a byte-order access from the older `Bytes` methods (`get`, `set`, ...) that
/// go through the runtime object.
///
/// The name itself carries the whole answer, so the frontend, `codegen_c.rs`
/// and `codegen_llvm.rs` read one table rather than each parsing the name and
/// disagreeing about a width.
pub fn bytes_endian_access(op: &str) -> Option<(usize, bool, Ty)> {
    let rest = op.strip_prefix("read_").or_else(|| op.strip_prefix("write_"))?;
    let (value, order) = rest.rsplit_once('_')?;
    let big_endian = match order {
        "be" => true,
        "le" => false,
        _ => return None,
    };
    let width = match value {
        "u16" => 2,
        "u32" | "i32" => 4,
        "u64" | "i64" => 8,
        _ => return None,
    };
    Some((width, big_endian, Ty::from_name(value)?))
}

/// Why a type cannot be a `Ptr<T>`'s element, in one sentence that names the
/// reason the type itself has: an owned value the language manages, a borrow,
/// or a value that is not one machine-level thing.
pub fn ptr_element_error(ty: &Ty) -> String {
    match ty {
        Ty::Str => "'str' is a borrowed view, not an address a pointer can hold".to_string(),
        Ty::View => "'View' is a borrowed view, not an address a pointer can hold".to_string(),
        Ty::Ref { mutable, inner } => format!(
            "'{}' is a borrow: take the address of the *value* with 'Ptr.from_ref'/'Ptr.from_mut' instead",
            Ty::Ref { mutable: *mutable, inner: inner.clone() }.display()
        ),
        Ty::String | Ty::Bytes | Ty::List(_) | Ty::Box(_) | Ty::Map(..) => format!(
            "'{}' is an owned heap value, which is not one machine-level value a raw pointer can address",
            ty.display()
        ),
        Ty::Closure { .. } | Ty::Fn { .. } => format!(
            "'{}' is a function value, not data a pointer can address",
            ty.display()
        ),
        Ty::Atomic(_) => format!(
            "'{}' is an atomic cell, whose operations the language controls; point at the plain value it holds instead",
            ty.display()
        ),
        Ty::Unit => "'unit' is not a value".to_string(),
        other => format!(
            "'{}' is not a type a raw pointer can address: a 'Ptr<T>' element is a scalar, 'bool', 'char', a 'datac'/'enum', a fixed-size array, an 'extern type' handle or another 'Ptr'",
            other.display()
        ),
    }
}

/// The element type of a `Ptr<T>`, looking through a borrow — `p.read()` arrives
/// with the receiver typed `&Ptr<T>` (the receiver may be a borrow of a place).
pub fn ptr_elem_of(ty: &Ty) -> Option<&Ty> {
    match ty {
        Ty::Ptr(elem) => Some(elem),
        Ty::Ref { inner, .. } => ptr_elem_of(inner),
        _ => None,
    }
}

/// Whether `ty` may be a `Thread<T>`'s element: anything a worker may hand back
/// to its joiner, which is anything that can be moved through the boundary.
///
/// A borrow, a `str` and a `View` are refused for the reason they are refused
/// in every stored position: each points at data somebody else owns, and the
/// frame that owns it ends before the joiner can read the result
/// (`is_borrowed_field_ty` is the same predicate the stored positions use, so
/// this cannot drift from them).
pub fn is_thread_element(ty: &Ty) -> bool {
    !is_borrowed_field_ty(ty)
}

/// Whether `ty` may be an `Atomic<T>`'s element: the integer types and `bool`,
/// and nothing else.
///
/// A float has no atomic hardware operation worth claiming (`Atomic<f32>` would
/// be a compare-and-swap loop, not an atomic add); `String`, `Bytes`, a `datac`
/// and a container each own memory or carry a tag, so a single-word atomic
/// cannot hold one; a borrow and a `View` are somebody else's data. The set is
/// closed rather than open-ended because the C and LLVM lowerings both have to
/// name the element's width, and a closed set is what makes that a total
/// function instead of a diagnostic at codegen time.
pub fn is_atomic_element(ty: &Ty) -> bool {
    matches!(ty, Ty::Bool) || ty.is_integer()
}

/// Whether `ty` may be a `Mutex<T>`'s element: an owned value with no borrows,
/// the same `Send`-shaped predicate `Thread<T>` uses (`is_thread_element`). A
/// `Mutex<T>` shares `T` across threads, so a borrow, a `str` or a `View` in
/// the slot would be a thread reading data another frame owns.
pub fn is_mutex_element(ty: &Ty) -> bool {
    !is_borrowed_field_ty(ty)
        && !matches!(
            ty,
            Ty::Unit | Ty::Fn { .. } | Ty::Closure { .. } | Ty::ScopedFn { .. }
        )
}

/// The element type of a `Mutex<T>`, looking through a borrow: `m.lock { ... }`
/// arrives with the receiver typed `&Mutex<T>` (a method receiver is
/// auto-borrowed, and a `pd` mutex is reached through its address).
pub fn mutex_elem_of(ty: &Ty) -> Option<&Ty> {
    match ty {
        Ty::Mutex(elem) => Some(elem),
        Ty::Ref { inner, .. } => mutex_elem_of(inner),
        _ => None,
    }
}

/// The element type of an `Atomic<T>`, looking through a borrow: `c.load(...)`
/// arrives with the receiver typed `&Atomic<T>` (a method receiver is
/// auto-borrowed, and a `pd` atomic is reached through its address).
pub fn atomic_elem_of(ty: &Ty) -> Option<&Ty> {
    match ty {
        Ty::Atomic(elem) => Some(elem),
        Ty::Ref { inner, .. } => atomic_elem_of(inner),
        _ => None,
    }
}

/// The `List<T>` or `Box<T>` behind an expression's type, looking through a
/// borrow — `l.push(x)` and `b.get()` both arrive with the receiver typed
/// `&List<T>`/`&Box<T>` (a container parameter is a borrow, and a method
/// receiver is auto-borrowed).
pub fn list_elem_of(ty: &Ty) -> Option<&Ty> {
    match ty {
        Ty::List(elem) => Some(elem),
        Ty::Ref { inner, .. } => list_elem_of(inner),
        _ => None,
    }
}

/// Whether this type is an *open* container: one a bare `List.empty()`,
/// `Box.empty()` or `Map.new()` made, with no type written anywhere to say
/// what it holds.
///
/// Such a container carries `Unit` in the slot its element type goes
/// (`List<Unit>`, `Box<Unit>`, `Map<Unit, Unit>`). The marker cannot be
/// confused with a type a program wrote: `is_list_element`, `is_box_element`
/// and `is_map_key` all refuse `Unit`, so `resolve_type` has no way to build
/// one from source. The first operation that has to know the element is
/// therefore free to fill it in (see `fill_open_container`).
pub fn is_open_container(ty: &Ty) -> bool {
    match ty {
        Ty::List(elem) | Ty::Box(elem) => **elem == Ty::Unit,
        Ty::Map(key, value) => **key == Ty::Unit || **value == Ty::Unit,
        Ty::Ref { inner, .. } => is_open_container(inner),
        _ => false,
    }
}

/// The type an open container becomes once `from` says what it holds: `from`
/// is the container a `push`/`insert`/`set` argument built, or the one a
/// surrounding position expects. `None` when the two are not the same kind of
/// container, or when what `from` holds is not something a container may hold,
/// so an open container whose element the caller cannot supply stays open for
/// the check that explains why.
///
/// A slot that is already decided keeps its type: an open container only ever
/// has one, and a `Map` whose key came from the binding's written type keeps
/// that key.
pub fn fill_open_container(
    open: &Ty,
    from: &Ty,
    keyable: &std::collections::HashSet<String>,
) -> Option<Ty> {
    let (open, from) = match (open, from) {
        (Ty::Ref { inner, .. }, _) => (inner.as_ref(), from),
        (_, Ty::Ref { inner, .. }) => (open, inner.as_ref()),
        _ => (open, from),
    };
    match (open, from) {
        (Ty::List(_), Ty::List(elem)) if is_list_element(elem) => Some(Ty::List(elem.clone())),
        (Ty::Box(_), Ty::Box(elem)) if is_box_element(elem) => Some(Ty::Box(elem.clone())),
        (Ty::Map(open_key, open_value), Ty::Map(from_key, from_value)) => {
            if !is_map_key_with(from_key, keyable) || !is_map_element(from_value) {
                return None;
            }
            if **open_key != Ty::Unit && open_key != from_key {
                return None;
            }
            if **open_value != Ty::Unit && open_value != from_value {
                return None;
            }
            Some(Ty::Map(from_key.clone(), from_value.clone()))
        }
        _ => None,
    }
}

/// As `list_elem_of`, for `Box<T>`.
pub fn box_elem_of(ty: &Ty) -> Option<&Ty> {
    match ty {
        Ty::Box(elem) => Some(elem),
        Ty::Ref { inner, .. } => box_elem_of(inner),
        _ => None,
    }
}

/// As `list_elem_of`, for `Map<K, V>` — the *value* type. The key is what the
/// call-site argument has to match, and everything else about a map method
/// (`get`, `insert`, `remove`, …) is keyed on the value.
pub fn map_elem_of(ty: &Ty) -> Option<&Ty> {
    match ty {
        Ty::Map(_, value) => Some(value),
        Ty::Ref { inner, .. } => map_elem_of(inner),
        _ => None,
    }
}

/// As `map_elem_of`, for the *key* type.
pub fn map_key_of(ty: &Ty) -> Option<&Ty> {
    match ty {
        Ty::Map(key, _) => Some(key),
        Ty::Ref { inner, .. } => map_key_of(inner),
        _ => None,
    }
}

/// The element type of an array, looking through a borrow: `a.len()` and
/// `a.get(i)` arrive with the receiver typed `&[T; N]`, since an owned array
/// parameter is a borrow.
pub fn array_elem_of(ty: &Ty) -> Option<&Ty> {
    match ty {
        Ty::Array(elem, _) => Some(elem),
        Ty::Ref { inner, .. } => array_elem_of(inner),
        _ => None,
    }
}

/// The length of an array, looking through a borrow.
pub fn array_len_of(ty: &Ty) -> Option<u64> {
    match ty {
        Ty::Array(_, len) => Some(*len),
        Ty::Ref { inner, .. } => array_len_of(inner),
        _ => None,
    }
}

/// The C spelling of an element type *inside a container's own typedef*.
/// A container is emitted before the `datac`s (its buffer is a pointer), so
/// a `datac` element is named by its forward-declared struct tag rather than
/// by its typedef name; every other element is named as usual.
///
/// A borrow of a `datac` has to name the tag too, and for the same reason: a
/// closure typedef is emitted before the `datac`s as well (a `datac` field may
/// hold a closure, so the closure type has to exist first), and a callback
/// parameter of a module's own type — `closure(&Item)` — is exactly a `datac`
/// behind a pointer. Falling back to `c_type()` there spelled it
/// `const struct_Item*`, a typedef name that is not declared until after the
/// closure typedef that uses it (BUG-2). `&str` keeps its own spelling, since
/// `c_type()` already collapses it to `const char*`.
pub fn container_elem_c_type(elem: &Ty) -> String {
    match elem {
        Ty::Struct(name) => format!("struct tag_{}", c_ident(name)),
        Ty::Ref { mutable, inner } if !matches!(**inner, Ty::Str) => {
            let pointee = container_elem_c_type(inner);
            if *mutable || matches!(**inner, Ty::Mutex(_)) {
                format!("{pointee}*")
            } else {
                format!("const {pointee}*")
            }
        }
        other => other.c_type(),
    }
}

/// The C struct tag for a `datac`, so a container can point at a struct
/// whose body has not been emitted yet (`Box<Node>` inside `Node` itself).
pub fn struct_tag(name: &str) -> String {
    format!("tag_{}", c_ident(name))
}

/// The name a tuple type's derived record is keyed by: the tuple as it is
/// written, element by element in position order (`(i32, i32)`).
///
/// A tuple is a record with positional fields (`plan/syntax.md` 3), and v1's
/// record type is `Ty::Struct(<key>)`, the same shape A-2 gives a generic
/// instantiation (`Pair<i32, String>`). The key is the written form rather
/// than a mangled identifier because a written form cannot be a declared name:
/// `(` is not an identifier byte, so a derived tuple record can never collide
/// with a `datac`/`enum` a program declares, and `Ty::display` of the record
/// then reads exactly as the source wrote it. `ty_mangle` turns it into a C
/// identifier the way it does for every other name.
pub fn tuple_struct_name(elems: &[Ty]) -> String {
    format!(
        "({})",
        elems
            .iter()
            .map(|e| e.display())
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// Whether a record name is one of the compiler's derived tuple records. The
/// key is the tuple as written, so this is a property of the name alone: no
/// declared `datac`/`enum` name begins with `(`.
pub fn is_tuple_name(name: &str) -> bool {
    name.starts_with('(')
}

/// The field name one tuple position holds in its derived record. One place
/// names it, because the record's field list, the `tir` field access a
/// destructuring binding lowers to, and the `Projection` a partial move of one
/// position records all have to agree.
pub fn tuple_field_name(index: usize) -> String {
    format!("f{index}")
}

/// The position a derived record's field name stands for, or `None` for a
/// declared field. The inverse of `tuple_field_name`, so a pass that reports
/// the place a tuple element was read from names the position a reader counts
/// rather than the name the record uses.
pub fn tuple_field_index(name: &str) -> Option<usize> {
    name.strip_prefix('f')?.parse::<usize>().ok()
}

/// The `datac` record one tuple type *is*: its fields are its positions, named
/// `f0`, `f1`, ... in element order, each holding that element by value.
///
/// The declaration is derived (nothing in the source declares it), and
/// `infer.rs` registers it on first use, exactly as it registers a generic
/// instantiation (A-2). `order` is the same sentinel an instance carries:
/// `order_declarations` gives every derived record its place in the merged
/// declaration order before codegen sees it.
pub fn tuple_struct_def(name: &str, elems: &[Ty]) -> StructDef {
    StructDef {
        name: name.to_string(),
        fields: elems
            .iter()
            .enumerate()
            .map(|(i, ty)| FieldDef {
                name: tuple_field_name(i),
                ty: ty.clone(),
                mutable: false,
            })
            .collect(),
        repr: Repr::C,
        order: usize::MAX,
    }
}

/// The size and alignment of one type, in bytes, as the C compiler lays it out
/// on the x86-64 target this compiler builds for. It is what `size_of`,
/// `align_of` and `offset_of` report, and it exists so the LLVM backend and the
/// C oracle can be checked against the same numbers rather than each assuming
/// the other agrees (kernel stage 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub size: u64,
    pub align: u64,
}

fn round_up(value: u64, align: u64) -> u64 {
    debug_assert!(align.is_power_of_two());
    (value + align - 1) & !(align - 1)
}

/// One field's contribution, and the offset it lands at: the C rule, with the
/// type's own `repr` deciding whether padding is present and whether the
/// alignment is raised.
fn record_layout(
    fields: &[FieldDef],
    repr: &Repr,
    structs: &std::collections::HashMap<String, StructDef>,
    enums: &std::collections::HashMap<String, EnumDef>,
) -> Option<Layout> {
    let packed = matches!(repr, Repr::Packed);
    let mut offset = 0u64;
    let mut align = 1u64;
    for f in fields {
        let l = type_layout(&f.ty, structs, enums)?;
        if !packed {
            offset = round_up(offset, l.align);
            align = align.max(l.align);
        }
        offset += l.size;
    }
    if let Repr::Align(n) = repr {
        align = align.max(*n);
    }
    Some(Layout {
        size: round_up(offset, align),
        align,
    })
}

/// The size and alignment of one type, or `None` for a type this compiler does
/// not lay out by value (which no query may name).
pub fn type_layout(
    ty: &Ty,
    structs: &std::collections::HashMap<String, StructDef>,
    enums: &std::collections::HashMap<String, EnumDef>,
) -> Option<Layout> {
    Some(match ty {
        Ty::U8 | Ty::Bool => Layout { size: 1, align: 1 },
        Ty::U16 => Layout { size: 2, align: 2 },
        Ty::I32 | Ty::U32 | Ty::F32 | Ty::Char => Layout { size: 4, align: 4 },
        Ty::I64 | Ty::U64 | Ty::F64 | Ty::Usize | Ty::Isize => Layout { size: 8, align: 8 },
        // `unit` stores nothing; a `unit` field is filtered out of every
        // aggregate, so the pair below only ever answers a direct query.
        Ty::Unit => Layout { size: 0, align: 1 },
        // A pointer, an opaque handle and a function value are one machine
        // word; `str` is the pointer it borrows.
        Ty::Str | Ty::Handle(_) | Ty::Ptr(_) | Ty::Fn { .. } | Ty::Ref { .. } | Ty::Thread(_) => {
            Layout { size: 8, align: 8 }
        }
        // A borrowed or owned trait object is the fat pair: two pointers.
        Ty::Dyn { .. } | Ty::BoxDyn { .. } => Layout { size: 16, align: 8 },
        Ty::Mutex(_) => Layout { size: 8, align: 8 },
        Ty::View => Layout { size: 16, align: 8 },
        Ty::Bytes | Ty::String | Ty::List(_) | Ty::Closure { .. } | Ty::ScopedFn { .. } => {
            Layout { size: 24, align: 8 }
        }
        Ty::Box(_) => Layout { size: 8, align: 8 },
        // `{ entries, entry_count, entry_cap, len, index, index_cap, index_used }`.
        Ty::Map(_, _) => Layout { size: 56, align: 8 },
        // `{ elem v; }`, one machine word of the element.
        Ty::Atomic(elem) => type_layout(elem, structs, enums)?,
        Ty::Array(elem, len) => {
            let l = type_layout(elem, structs, enums)?;
            Layout {
                size: l.size.saturating_mul(*len),
                align: l.align,
            }
        }
        Ty::Struct(name) => {
            if let Some(e) = enums.get(name) {
                let (tag_bytes, _) = e.tag_storage();
                let tag = Layout {
                    size: tag_bytes,
                    align: tag_bytes,
                };
                let mut union_align = 1u64;
                let mut union_size = 0u64;
                for v in &e.variants {
                    // A payload-less variant is still one `char` in the C
                    // union (`VariantDef::is_payload_less`), so its member is
                    // one byte rather than an empty struct.
                    let p = if v.is_payload_less() {
                        Layout { size: 1, align: 1 }
                    } else {
                        record_layout(&v.fields, &Repr::C, structs, enums)?
                    };
                    union_align = union_align.max(p.align);
                    union_size = union_size.max(p.size);
                }
                let union_size = round_up(union_size, union_align);
                let union_offset = round_up(tag.size, union_align);
                let align = tag.align.max(union_align);
                Layout {
                    size: round_up(union_offset + union_size, align),
                    align,
                }
            } else {
                let s = structs.get(name)?;
                record_layout(&s.fields, &s.repr, structs, enums)?
            }
        }
    })
}

/// The byte offset of one field of one `datac`, or `None` when the type is not
/// a `datac` this compiler lays out or the field is not one of its own.
pub fn field_offset(
    name: &str,
    field: &str,
    structs: &std::collections::HashMap<String, StructDef>,
    enums: &std::collections::HashMap<String, EnumDef>,
) -> Option<u64> {
    let s = structs.get(name)?;
    if enums.contains_key(name) {
        return None;
    }
    let packed = matches!(s.repr, Repr::Packed);
    let mut offset = 0u64;
    for f in &s.fields {
        let l = type_layout(&f.ty, structs, enums)?;
        if !packed {
            offset = round_up(offset, l.align);
        }
        if f.name == field {
            return Some(offset);
        }
        offset += l.size;
    }
    None
}

/// The C identifier of a *fixed-arity* wrapper around a variadic user
/// `extern fun` (gap 7), from the extern's own key and the extra-argument
/// types one call passes. Deterministic, so two call sites that pass the same
/// types share one wrapper, and the typechecker and a backend cannot disagree
/// about which one a call names. The `c_name`/`symbol_name` prefix a backend
/// adds turns this into `kelp_va_<mangled fn>_<mangled types>`.
pub fn variadic_wrapper_name(extern_key: &str, extra: &[Ty]) -> String {
    let mut name = format!("va_{}", c_ident(extern_key));
    for ty in extra {
        name.push('_');
        name.push_str(&c_ident(&ty.display()));
    }
    name
}

/// Whether `ty` may be passed as an *extra* argument of a variadic user
/// `extern fun` (gap 7): an integer, `bool`, `f32`/`f64`, `str` or an opaque
/// handle. A `char` is not here: it is a code point, not an integer the
/// native side could read back from a `va_arg` of any fixed width. There is
/// no implicit conversion either way, so a type outside this set is a compile
/// error rather than a cast.
pub fn is_variadic_abi_ty(ty: &Ty) -> bool {
    ty.is_integer()
        || matches!(ty, Ty::Bool | Ty::F32 | Ty::F64 | Ty::Str | Ty::Handle(_))
        || matches!(ty, Ty::Ref { inner, .. } if matches!(**inner, Ty::Str))
}

/// Mangles an `.kp` top-level name into a valid C identifier.
///
/// M5 prefixes every module's items with their module path
/// (`shapes.circle.Circle`), and a receiver method is keyed `Type#name` —
/// neither `.` nor `#` is legal in a C identifier, so both become `__`.
/// M6.3's stdlib overloads add `name@i32` keys, so `@` becomes `__ov_` and
/// the `,` between parameter types becomes `_`. A-2's instantiation keys add
/// `<`/`>` and the `, ` between type arguments (`Pair<i32, String>`), so `<>`
/// become `__l__`/`__g__` and the space is dropped. Not fully
/// collision-proof against a hand-written `shapes__circle__Circle` (nothing
/// can be, short of a real mangling scheme), but low-risk and simple, same
/// tradeoff as the pre-M5 `#`-only version.
///
/// BUG-zR8Wm: a module's id is its file stem, so a file named `we"ird.kp`
/// reaches here with the quote in it, and the capability array symbol the
/// backends spell from that name (`kelp_caps_we"ird_fs`) was not a C
/// identifier, so `cc` refused the whole build. Every byte the mappings above
/// leave alone is therefore outside `[A-Za-z0-9_]`, and the final sweep
/// spells each one as `__x` and its two hex digits: legal, fixed width, and
/// distinct per byte.
pub fn c_ident(name: &str) -> String {
    let mapped = name
        .replace('.', "__")
        .replace('#', "__")
        .replace('@', "__ov_")
        .replace(',', "_")
        .replace('<', "__l__")
        .replace('>', "__g__")
        // A function type argument (`Holder<fun(i32).R: i32>`) is the one
        // type whose written form carries parentheses and a colon, and its
        // instantiation key is built from that written form, so the two have
        // to mangle too or the key is not a C identifier.
        .replace('(', "__lp__")
        .replace(')', "__rp__")
        .replace(':', "__c__")
        .replace(' ', "");
    if mapped.bytes().all(is_ident_byte) {
        return mapped;
    }
    let mut out = String::with_capacity(mapped.len() + 4);
    for b in mapped.bytes() {
        if is_ident_byte(b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("__x{b:02x}"));
        }
    }
    out
}

/// Whether one byte may stand in a C identifier as itself.
fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Whether `ty` is a *borrowed* type in the sense that matters for a `datac`
/// field or an `enum` payload: it points at data somebody else owns, so it
/// cannot be stored. (`Box`, `List` and `Map` are not reached here — they
/// *own* what they point at.)
///
/// One predicate rather than a `matches!` at each of the four sites that
/// enforce this, because there are four and a new borrowed type has to be
/// remembered at every one of them: the struct-table pass and the generic
/// instantiation path each check datac fields and enum payloads separately.
/// `View` (A-8) was missed at two of the four on the first attempt.
pub fn is_borrowed_field_ty(ty: &Ty) -> bool {
    matches!(ty, Ty::Str | Ty::View | Ty::Ref { .. } | Ty::Dyn { .. })
}

/// The C typedef name of the fat pair a borrowed trait object is: one struct per
/// trait, shared by `&dyn Name` and `&mut dyn Name` (the two are the same C
/// value; the borrow checker, not a C qualifier, tells them apart).
pub fn dyn_c_type(trait_name: &str) -> String {
    format!("kelp_Dyn__{}", c_ident(trait_name))
}

/// The C typedef name of one trait's vtable struct: one field per method, the
/// supertraits' methods first (`plan/traits.md` 9.4).
pub fn dyn_vtable_c_type(trait_name: &str) -> String {
    format!("kelp_VTable__{}", c_ident(trait_name))
}

/// The emitted symbol of one `(trait, type)` vtable, the same name in both
/// backends so a diagnostic or a `nm` line reads the same either way.
pub fn dyn_vtable_symbol(trait_name: &str, type_name: &str) -> String {
    format!(
        "kelp_vtable__{}__{}",
        c_ident(trait_name),
        c_ident(type_name)
    )
}

/// The emitted symbol of the thunk one vtable slot points at.
pub fn dyn_thunk_symbol(trait_name: &str, type_name: &str, method: &str) -> String {
    format!(
        "kelp_dyn_thunk__{}__{}__{}",
        c_ident(trait_name),
        c_ident(type_name),
        c_ident(method)
    )
}

/// The emitted symbol of the thunk the vtable's drop entry points at (A-12
/// slice 7): the concrete type's own drop glue over the erased pointer.
pub fn dyn_drop_thunk_symbol(trait_name: &str, type_name: &str) -> String {
    format!(
        "kelp_dyn_drop__{}__{}",
        c_ident(trait_name),
        c_ident(type_name)
    )
}

/// The emitted symbol that frees an *owned* trait object (A-12 slice 7), one
/// per trait: `free` on the box's allocation, after the vtable's drop entry has
/// run the concrete type's own drop glue. The concrete type is not part of the
/// name, because the value behind the pointer is reached through the vtable.
pub fn box_dyn_drop_fn(trait_name: &str) -> String {
    format!("kelp_box_dyn_drop__{}", c_ident(trait_name))
}

/// A function's checked signature. Parameter names are kept (not just
/// types) because call sites can pass named arguments (`f(x = 1)`), which
/// `infer.rs` resolves against these names.
#[derive(Debug, Clone)]
pub struct FnSig {
    pub params: Vec<(String, Ty)>,
    pub ret: Ty,
}

/// One `datac` field: `val`/`var` name: Type.
#[derive(Debug, Clone)]
pub struct FieldDef {
    pub name: String,
    pub ty: Ty,
    /// `var` field: reassignable (given a `var`-bound value). `val` field:
    /// never reassignable, regardless of the binding's own mutability.
    pub mutable: bool,
}

#[derive(Debug, Clone)]
pub struct StructDef {
    pub name: String,
    pub fields: Vec<FieldDef>,
    /// The `repr(...)` clause, or `Repr::C` for the default C layout
    /// (kernel stage 2). It decides the field offsets and the type's own
    /// alignment, in this backend and in the C oracle alike.
    pub repr: Repr,
    /// Position in the program's merged declaration order, across every
    /// `datac` *and* `enum` (A-1). A field or payload may only name an
    /// earlier declaration by value, so this order is a valid topological
    /// order — for `owned_types`' single pass and for the C typedefs
    /// `codegen_c.rs` emits.
    pub order: usize,
}

/// One variant of an `enum` (A-1): a name and the payload fields it carries.
#[derive(Debug, Clone)]
pub struct VariantDef {
    pub name: String,
    pub fields: Vec<FieldDef>,
    /// `Off = 0`: the explicit discriminant, already resolved to a value. A
    /// field-less enum's real tag value is [`EnumDef::discriminants`], which
    /// fills an unwritten discriminant in as the previous value plus one.
    pub discriminant: Option<i128>,
}

/// A tagged union (A-1): exactly one variant is live in a value. At the type
/// level an `enum` is a `Ty::Struct`, so it is nominal, non-`Copy` and
/// droppable exactly like a `datac`; the difference lives here, in the
/// declaration table, in the pattern checker and in the C layout
/// (`tag` + `union` instead of a flat field list).
#[derive(Debug, Clone)]
pub struct EnumDef {
    pub name: String,
    pub variants: Vec<VariantDef>,
    /// The `repr(...)` clause, or `Repr::C` for the default C layout and an
    /// `i32` tag (kernel stage 2). Only `repr(<int>)` changes an enum's
    /// layout: it picks the tag's storage type.
    pub repr: Repr,
    /// See `StructDef::order`.
    pub order: usize,
}

impl EnumDef {
    pub fn variant(&self, name: &str) -> Option<(usize, &VariantDef)> {
        self.variants.iter().enumerate().find(|(_, v)| v.name == name)
    }

    /// Whether every variant carries no payload. Only a field-less enum may
    /// spell explicit discriminants and take a `repr(<int>)` tag type, so this
    /// is the gate for both.
    pub fn is_field_less(&self) -> bool {
        self.variants.iter().all(|v| v.is_payload_less())
    }

    /// The tag's storage, as `(bytes, signed)`, taken from the `repr(<int>)`
    /// name. It is the width the C typedef and the LLVM struct both use, and
    /// it is deliberately not a `Ty`: v1 has no `i8`/`i16` *value* type, but
    /// `repr(i8)`/`repr(i16)` still name a one- or two-byte tag.
    pub fn tag_storage(&self) -> (u64, bool) {
        match &self.repr {
            Repr::Int(name) => match name.as_str() {
                "u8" => (1, false),
                "u16" => (2, false),
                "u32" => (4, false),
                "u64" => (8, false),
                "i8" => (1, true),
                "i16" => (2, true),
                "i64" => (8, true),
                _ => (4, true),
            },
            _ => (4, true),
        }
    }

    /// The C spelling of the tag's storage type, which is what the generated
    /// `typedef` declares it as.
    pub fn tag_c_type(&self) -> &'static str {
        match self.tag_storage() {
            (1, false) => "uint8_t",
            (2, false) => "uint16_t",
            (4, false) => "uint32_t",
            (8, false) => "uint64_t",
            (1, true) => "int8_t",
            (2, true) => "int16_t",
            (8, true) => "int64_t",
            _ => "int32_t",
        }
    }

    /// The integer `Ty` a tag value crosses the surface as: the narrowest
    /// Kelp integer that holds the storage. Kelp has no `i8`/`i16`, so those
    /// two spellings report `i32`; every other one reports itself.
    pub fn tag_ty(&self) -> Ty {
        match &self.repr {
            Repr::Int(name) => match name.as_str() {
                "u8" => Ty::U8,
                "u16" => Ty::U16,
                "u32" => Ty::U32,
                "u64" => Ty::U64,
                "i64" => Ty::I64,
                _ => Ty::I32,
            },
            _ => Ty::I32,
        }
    }

    /// The tag value of every variant, in declaration order: a written
    /// discriminant stands, and an unwritten one is the previous value plus
    /// one, starting at 0 (kernel stage 2).
    pub fn discriminants(&self) -> Vec<i128> {
        let mut out = Vec::with_capacity(self.variants.len());
        let mut next: i128 = 0;
        for v in &self.variants {
            let value = v.discriminant.unwrap_or(next);
            out.push(value);
            next = value.saturating_add(1);
        }
        out
    }
}

impl VariantDef {
    /// The fields this variant actually stores: every declared field except
    /// one typed `unit`, which carries no runtime representation
    /// (`MISSING-r7Wqb`). `Ok`/`Some` instantiated with a `unit` type
    /// argument — the shape `thread.join` builds for a `Thread<unit>` — is
    /// declared with one field, and that field stores nothing.
    pub fn real_fields(&self) -> impl Iterator<Item = &FieldDef> {
        self.fields.iter().filter(|f| f.ty != Ty::Unit)
    }

    /// Whether this variant's union member is the bare placeholder byte
    /// (`char kelp_none` in C, `i8` in LLVM) rather than a payload struct: no
    /// fields at all, or every field `unit`. Both backends lay out, construct,
    /// read and derive over a variant by this rule rather than by
    /// `fields.is_empty()`, so a `unit` payload is exactly as payload-less as
    /// no payload.
    pub fn is_payload_less(&self) -> bool {
        self.real_fields().next().is_none()
    }
}

impl StructDef {
    pub fn field(&self, name: &str) -> Option<&FieldDef> {
        self.fields.iter().find(|f| f.name == name)
    }
}

#[cfg(test)]
mod tests {
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
            c_ident("Holder<fun(i32).R: i32>"),
            "Holder__l__fun__lp__i32__rp____R__c__i32__g__"
        );
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
}
