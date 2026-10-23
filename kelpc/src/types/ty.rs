use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Ty {
    I32,
    I64,
    /// `int`, the default integer (issue #275): its own type of the target's
    /// native width (`NATIVE_INT_BITS`), with the operators of `i64`. It is not
    /// `i64` under another name, so the two do not mix without `as`.
    Int,
    /// `uint`, the native unsigned integer (issue #275, owner direction): `u64`
    /// on a 64-bit target, `u32` on a 32-bit one. Its own type, as `int` is.
    Uint,
    U32,
    U64,
    F32,
    F64,
    /// `float`, the native float (issue #275, owner direction): `f64` on a 64-bit
    /// target, `f32` on a 32-bit one. Its own type, as `int` is.
    Float,
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
    /// at all - which is why the JSON reader in this repository copied its
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
    /// end of the owning scope - its elements first, then its buffer. The
    /// element type is part of the type, and codegen monomorphizes: one C
    /// struct plus one set of functions per element type actually used (see
    /// the removed C backend's `write_list`).
    List(Box<Ty>),
    /// The builtin owning pointer (M8): `Box<T>` holds one `T` on the heap,
    /// or nothing at all (`Box.empty()`), which is what makes a recursive
    /// `datac` expressible - `datac Node(val next: Box<Node>)` would be
    /// infinitely sized by value, and a pointer is the only way out.
    /// Non-`Copy`; dropping it drops the value it holds and frees the box.
    /// Like `List`, monomorphized: one C struct plus one function set per
    /// element type actually used.
    Box(Box<Ty>),
    /// The builtin owned keyed container (MK5-S5): `Map<K, V>` - a key type
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
    /// A borrow of a value of type `inner` - shared (`mutable: false`) or
    /// exclusive (`mutable: true`). Never appears as a function's declared
    /// return type in M4 (no surface syntax produces one there - see
    /// `infer::resolve_type`), so a borrow can never escape the call frame
    /// that created it; that's what makes the borrow checker's call-scoped
    /// release sound without real lifetime tracking. A reference is always
    /// Copy regardless of `inner` - copying a pointer doesn't move what it
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
    /// environment - so it is `Copy`, it owns nothing, and it needs no drop.
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
    /// becomes. Written `closure(T) => U` - the same shape as a function
    /// type, under the `closure` keyword, because it is called the same way.
    ///
    /// The representation differs from `Ty::Fn` in exactly the ways that
    /// matter to every pass: a closure is `{ fn, env, drop }` - a *fat*
    /// pointer, monomorphized per signature - so it is **not `Copy`**, it
    /// owns the heap environment the captures were moved into, and it drops
    /// that environment when it dies. A `Ty::Fn` stays a plain C function
    /// pointer: a declared function, and a literal that captured nothing,
    /// have no environment to carry (see the removed C backend's `write_closure`).
    ///
    /// Calling one goes through `tir::ExprKind::IndirectCall`, exactly as a
    /// `Ty::Fn` does; codegen is what knows a closure's call spelling
    /// (`c.fn(c.env, ...)`). A capturing literal is the only way a value of
    /// this type is made, so a program cannot build a closure without
    /// something to capture.
    Closure { params: Vec<Ty>, ret: Box<Ty> },
    /// A **scoped** function value (`kelp wiki page language/functions.md`): a parameter
    /// written `scoped fun(T) => U`, which accepts only a `fun` literal or a
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
    /// not wanted - the callee holds the fat pointer itself.
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
    /// is the rule `design/syntax.md (kelp wiki)` 3 states for the type.
    Array(Box<Ty>, u64),
    /// The value of one **const type parameter** at one instantiation (A-20):
    /// `Buf<16>`'s `16`, or `Flags<true>`'s `true`. `value` is the folded
    /// value and `ty` is the parameter's declared type (`usize`, `u8`..`u64`,
    /// `i32`, `i64`, `bool`).
    ///
    /// It is a `Ty` only so that one monomorphization path can carry both
    /// kinds of generic argument: it appears in an instantiation's argument
    /// list, in the key `Buf<16>`, and in the parameter bindings while the
    /// template's fields and body are resolved. It never survives an
    /// instantiation as a real type: a field, a parameter, a return and a
    /// container element all refuse it, and the body it appears in resolves
    /// `N` to a literal.
    ConstInt { value: i128, ty: Box<Ty> },
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
    /// decision is recorded in `kelp issue #245` section 5.
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
    /// The builtin reader/writer lock (`std/sync`): `RwLock<T>`, whose `T` is
    /// reached through `read`/`write` scoped blocks.
    ///
    /// Monomorphized per element type exactly as `Mutex<T>` is, and `T` is the
    /// owned, borrow-free value it guards (`is_rwlock_element`). **Not `Copy`.**
    /// The representation is one opaque pointer to a runtime record (a state
    /// mutex, two condition variables, the reader/writer counters, a poison flag
    /// and the `T` slot), so the pointer crosses both backends unchanged. Its
    /// drop frees that record, running `T`'s own drop first.
    RwLock(Box<Ty>),
    /// The builtin condition variable (`std/sync`): `Condvar`, one opaque
    /// pointer to a `pthread_cond_t` the runtime owns. **Not `Copy`** and it
    /// owns heap memory, so it needs a drop (`kelp_condvar_drop`).
    Condvar,
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

/// The width of `int` on the build target, in bits (issue #275): the target's
/// native integer width. kelpc builds only the host triple, so this is the
/// host's pointer width; a cross build asks [`native_int_bits_for_triple`].
pub const NATIVE_INT_BITS: u32 = usize::BITS;

/// The width of `float` on the build target, in bits (issue #275): the target's
/// native float, `f64` on a 64-bit target and `f32` on a 32-bit one.
pub const NATIVE_FLOAT_BITS: u32 = 64;

/// The native integer width of a target triple, from its architecture name:
/// 32 for the 32-bit architectures kelpc knows, 64 for the rest. A host build
/// reads `NATIVE_INT_BITS`; a cross build asks this function for its triple.
#[cfg_attr(not(test), allow(dead_code))]
pub fn native_int_bits_for_triple(triple: &str) -> u32 {
    let arch = triple.split('-').next().unwrap_or(triple);
    let is_32 = matches!(arch, "i386" | "i486" | "i586" | "i686" | "arm" | "armv6" | "armv7" | "armv7a" | "armv7s" | "thumbv7em" | "thumbv7m" | "thumbv6m" | "thumb" | "riscv32" | "wasm32" | "mips" | "mipsel" | "powerpc" | "sparc" | "msp430" | "avr" | "xtensa");
    if is_32 {
        32
    } else {
        64
    }
}

// The codegen maps `int` onto the same 64-bit arms as `i64`. A 32-bit target
// needs those arms to read `NATIVE_INT_BITS` first; until then, refuse to build.
const _: () = assert!(
    NATIVE_INT_BITS == 64 && NATIVE_FLOAT_BITS == 64,
    "int, uint and float map to 64-bit codegen arms; a 32-bit host needs them to read NATIVE_INT_BITS and NATIVE_FLOAT_BITS"
);

impl Ty {
    pub fn from_name(name: &str) -> Option<Ty> {
        Some(match name {
            "i32" => Ty::I32,
            "i64" => Ty::I64,
            // `int` is the default integer type (issue #275), a distinct integer of
            // the target's native width, the type an unannotated literal takes.
            "int" => Ty::Int,
            "uint" => Ty::Uint,
            "float" => Ty::Float,
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
            // The one non-generic builtin in the family: `Condvar` is one
            // pointer to a runtime record whatever it guards.
            "Condvar" => Ty::Condvar,
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
                    | Ty::RwLock(_)
                    | Ty::Condvar
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
    /// available - a `datac` with an owned field owns memory too, and out of
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
                    | Ty::RwLock(_)
                    | Ty::Condvar
                    | Ty::Closure { .. },
            ),
        }
    }

    pub fn is_numeric(&self) -> bool {
        matches!(
            self,
            Ty::I32
                | Ty::I64
                | Ty::Int
                | Ty::U32
                | Ty::U64
                | Ty::Uint
                | Ty::F32
                | Ty::F64
                | Ty::Float
                | Ty::U8
                | Ty::U16
                | Ty::Usize
                | Ty::Isize
        )
    }

    pub fn is_integer(&self) -> bool {
        matches!(
            self,
            Ty::I32 | Ty::I64 | Ty::Int | Ty::U32 | Ty::U64 | Ty::Uint | Ty::U8 | Ty::U16 | Ty::Usize | Ty::Isize
        )
    }

    pub fn is_float(&self) -> bool {
        matches!(self, Ty::F32 | Ty::F64 | Ty::Float)
    }

    /// The full range of an integer type, as `(min, max)`: `min` signed so a
    /// signed type can express its own floor, `max` unsigned so `u64`/`usize`
    /// can express theirs. `None` for anything that is not an integer.
    ///
    /// Two passes need this and they have to agree: the removed C backend uses it to
    /// decide whether a `to_<T>` conversion needs a range check, and
    /// `infer.rs` uses it to decide which conversions *have* a `try_to_<T>`
    /// twin and what that twin tests.
    pub fn int_bounds(ty: &Ty) -> Option<(i128, u128)> {
        Some(match ty {
            Ty::I32 => (i32::MIN as i128, i32::MAX as u128),
            Ty::I64 | Ty::Int | Ty::Isize => (i64::MIN as i128, i64::MAX as u128),
            Ty::U8 => (0, u8::MAX as u128),
            Ty::U16 => (0, u16::MAX as u128),
            Ty::U32 => (0, u32::MAX as u128),
            Ty::U64 | Ty::Uint | Ty::Usize => (0, u64::MAX as u128),
            _ => return None,
        })
    }

    pub fn display(&self) -> String {
        match self {
            Ty::I32 => "i32".to_string(),
            Ty::I64 => "i64".to_string(),
            Ty::Int => "int".to_string(),
            Ty::Uint => "uint".to_string(),
            Ty::Float => "float".to_string(),
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
            Ty::RwLock(elem) => format!("RwLock<{}>", elem.display()),
            Ty::Condvar => "Condvar".to_string(),
            Ty::Handle(name) => name.clone(),
            Ty::Ptr(elem) => format!("Ptr<{}>", elem.display()),
            Ty::Array(elem, len) => format!("[{}; {}]", elem.display(), len),
            Ty::ConstInt { value, ty } => {
                match **ty {
                    Ty::Bool => if *value == 0 { "false".to_string() } else { "true".to_string() },
                    // A character argument is spelled as its literal where that is
                    // a plain letter or digit, and as its code point otherwise, so
                    // no written spelling can collide with another argument's.
                    Ty::Char => match u32::try_from(*value).ok().and_then(char::from_u32) {
                        Some(c) if c.is_ascii_alphanumeric() => format!("'{c}'"),
                        _ => format!("U+{value:04X}"),
                    },
                    _ => value.to_string(),
                }
            }
            Ty::Map(key, value) => format!("Map<{}, {}>", key.display(), value.display()),
            Ty::Dyn { mutable: true, trait_name } => format!("&mut dyn {trait_name}"),
            Ty::Dyn { mutable: false, trait_name } => format!("&dyn {trait_name}"),
            Ty::BoxDyn { trait_name } => format!("Box<dyn {trait_name}>"),
            Ty::Ref { mutable: true, inner } => format!("mut {}", inner.display()),
            Ty::Ref { mutable: false, inner } => format!("&{}", inner.display()),
            Ty::Fn { params, ret } => {
                // The return is written `=> T` exactly as a declaration
                // writes it, and a `Unit` return is left off - so a written
                // function type and a signature read the same way.
                let mut s = format!("fun({})", join_types(params));
                if **ret != Ty::Unit {
                    s.push_str(&format!(" => {}", ret.display()));
                }
                s
            }
            Ty::Closure { params, ret } => {
                // The closure type reads exactly as the function type does,
                // one keyword apart, because the two are called identically
                // and differ only in what the value owns.
                let mut s = format!("closure({})", join_types(params));
                if **ret != Ty::Unit {
                    s.push_str(&format!(" => {}", ret.display()));
                }
                s
            }
            Ty::ScopedFn { params, ret } => {
                let mut s = format!("scoped fun({})", join_types(params));
                if **ret != Ty::Unit {
                    s.push_str(&format!(" => {}", ret.display()));
                }
                s
            }
        }
    }

}
