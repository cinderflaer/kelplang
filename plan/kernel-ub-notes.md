# The undefined-behaviour catalogue, stage by stage

One section per kernel-path stage: what new way to trigger undefined or
implementation-defined behaviour the stage adds, and what the compiler does and
does not check. `plan/kernel-path.md` is the stage list; `plan/unsafe-and-pointers.md`
is stage 1's design.

## Stage 1: MK9-S2 `unsafe` and pointers

**New ways to reach UB.** `Ptr<T>` is a bare machine address with no provenance,
no lifetime and no ownership, and it may be null. Every unsafe operation is a
promise the compiler does not check:

- `p.read()` / `p.write(v)` on an address that is not a live, correctly aligned
  `T`, or on memory of a different type, is UB. `Ptr.from_addr(a)` accepts any
  integer, so a program can name any address, including one it was never
  granted.
- `p.offset(n)` / `p.byte_offset(n)` scale or step with no bounds check and wrap
  around the address space; a resulting address that does not point inside one
  allocated object is UB to dereference.
- `p.read_volatile()` / `p.write_volatile(v)` make the *access* unmergeable,
  unreorderable and unelidable, but they say nothing about the address being
  valid, and they are not a synchronisation mechanism.
- `Ptr.alloc<T>(count)` returns uninitialised memory, and reading a slot before
  writing it reads an indeterminate value. `p.free()` on an address not from
  `alloc`, twice, or while another pointer is still used, is UB.
- `Ptr.from_ref(&x)` / `Ptr.from_mut(x)` produce a pointer that does **not**
  extend the borrow: using the pointer after the place is gone (a returned
  pointer to a dead frame, a pointer kept past a `move`) is UB, and writing
  through a pointer derived from a *shared* borrow is a data race with any
  reader.
- Mixing a `Ptr<T>` with `Atomic<T>` over the same bytes is a data race; the
  language has no rule that ties the two operations to one location.

**What the compiler does check.** That an `unsafe` operation is written inside
`unsafe { ... }` or an `unsafe fun`; that a module containing `unsafe` is listed
in `coral.toml`'s `[unsafe] modules`; that a `Ptr<T>` element is a type with a
machine-level layout; that `read`/`read_volatile` only read `Copy` elements (so
no second owner of an owned value is made); and that the volatile accesses are
emitted as volatile access on both backends.

**What it does not check.** Anything about the address, its alignment, its
provenance, its lifetime, or whether it is still allocated. There is no
provenance model and no aliasing model yet (roadmap D-1); this stage is the
first thing those have to describe.
