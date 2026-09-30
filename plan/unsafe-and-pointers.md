# Stage 1: `unsafe` and raw pointers (MK9-S2 / roadmap B-3)

*Design for kernel-path stage 1. `plan/kernel-path.md` is the context; this file says what is built
and what is deliberately left out.*

## What this adds

A raw pointer type `Ptr<T>` and the `unsafe` marker that is the only way to dereference one.

- **`Ptr<T>`** is a builtin generic type, threaded exactly as `Atomic<T>` is: `types.rs` carries it,
  `infer.rs` gives it method signatures keyed `Ptr#<method>`, and both backends lower those keys.
  It is `Copy`, owns nothing, needs no drop and carries no borrow tracking, and it may be null.
- **The element** is any type with a C/LLVM layout: a scalar, `bool`, `char`, a `datac`/`enum`, a
  fixed-size array, an `extern type` handle, or another `Ptr`. `str`, `String`, `Bytes`, `List`,
  `Box`, `Map`, `View`, a borrow, a closure, a function value and `Atomic<T>` are refused by name:
  a raw pointer is a machine address, and none of those is one machine-level value.
- **Safe operations** (legal anywhere): `Ptr.null()`, `Ptr.from_addr(a)`, `p.addr()`, `p.is_null()`,
  `p.offset(n)`, `p.byte_offset(n)`, `p.cast<U>()`, `Ptr.from_ref(&x)`, `Ptr.from_mut(x)`, and
  `==`/`!=` between two pointers of one type. `offset`/`byte_offset` scale or step in bytes with no
  bounds check, because that is what a raw pointer is for.
- **Unsafe operations** (legal only inside `unsafe { ... }` or an `unsafe fun`): `p.read()`,
  `p.write(v)`, `p.read_volatile()`, `p.write_volatile(v)`, `Ptr.alloc<T>(count)` and `p.free()`.
  Calling one outside unsafe is an error that names the operation.
- **`unsafe` blocks** are an expression and a statement, written like a block. `unsafe` is a
  contextual word: it introduces a block only when a `{` follows, so it stays usable as a name.
- **`unsafe fun name(...)`** declares a function that only unsafe context may call.
- **Volatile** is never merged, reordered with another volatile access, or elided: the C backend
  writes a `volatile`-qualified access, LLVM sets the instruction's volatile flag
  (`LLVMSetVolatile`).

## The audit rule

A module may contain `unsafe` (a block or an `unsafe fun`) only if `coral.toml` lists it under
`[unsafe] modules = ["<module id>", ...]`, parsed exactly as `[extern.*]` is. The list is the set of
modules allowed to leave the checked language, and it is short and reviewable on purpose. A project
with no `[unsafe]` section behaves exactly as before: no module may contain `unsafe`, and no `Ptr`
operation is reachable at all.

## Ownership, and what the pointer does not carry

`Ptr.from_ref(&x)` / `Ptr.from_mut(x)` take the address of a `val`/`var` place, but the resulting
pointer does **not** extend the borrow. The borrow checker sees the borrow the call takes and no
more; using the pointer after the place is gone, or writing through it at all, is the caller's
responsibility, which is the point of `unsafe`. This is recorded in `plan/kernel-ub-notes.md`.

## What is deliberately not here

- Provenance and address-space control (roadmap MK9-S2-M1's later halves). A pointer here is a plain
  integer-shaped address, and `from_addr` can name any of them.
- `AtomicPtr`, `transmute`, unaligned or non-`Copy` reads. `read` requires `T` to be `Copy`, so a
  read cannot double-own a `String`.
- Any memory-safety proof about `unsafe` code. D-1/D-2/D-3 stay open; the UB catalogue grows a
  section per stage.
