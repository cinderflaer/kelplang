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

## Stage 2: MK9-S1-M2 layout

**New ways to reach UB.** Layout control makes the address arithmetic stage 1
already allowed easier to get wrong, and adds one promise about an address:

- `repr(packed)` puts a field at an offset its own type is not aligned for.
  The language has no pointer-to-field that escapes, so reading the field
  through the value is fine, but `offset_of<Packed>("field")` plus `Ptr`
  arithmetic can build a misaligned `Ptr<T>`; `read`/`write` there is UB for
  the same reason it always was, and the packed clause is what makes it easy.
- `repr(align(N))` is a **promise** that a value of the type is `N`-aligned.
  The compiler aligns every alloca and global it creates (the C attribute and
  the LLVM alignment anchor are the same fact), but a value reached through a
  `Ptr` it did not create (`Ptr.from_addr`, a cast, a `byte_offset`) carries no
  such guarantee, and reading or writing it as the aligned type is UB.
- A `repr(packed)` type read through a `Ptr` to a *different* type is the same
  type-punning UB as before, now more tempting because the packed bytes are easy
  to reinterpret.
- A read of an address built from `offset_of` and `Ptr` arithmetic is UB when
the offset does not land inside the same allocated object, exactly the stage 1
rule; `offset_of` only reports the offset, it does not make the address valid.

**What the compiler does check.** That a `repr` clause is one of `c`, `packed`,
`align(N)` or an integer type in the right position; that `N` is a power of two;
that an explicit discriminant is written only on a field-less enum, is unique,
and fits the tag's storage type; that a layout query names a type with a
machine-level layout and, for `offset_of`, a field the type has. `size_of`,
`align_of` and `offset_of` are compile-time constants, so they add no runtime
behaviour at all; they only let a program compute an address the compiler still
does not validate.

**What it does not check.** The alignment of any address a program builds, or
that memory reached through a `Ptr` holds a value of the type it is read as.
There is still no provenance model (roadmap D-1); `repr` makes the alignment
part of D-1's description rather than adding a check.

## Stage 3: MK9-S8 data formats

**New ways to reach UB.** This stage adds one new promise and one new
unchecked offset, both in unsafe code or on a caller-controlled one:

- `Ptr.read_unaligned`/`write_unaligned` drop the *alignment* half of what
  `read`/`write` already required, and nothing else. The address still has to
  point at a live, initialised `T` inside one allocated object, exactly the
  stage 1 rule; an unaligned access to a valid object is defined, and an
  aligned access to a bad address is still UB. There is no provenance model
  (roadmap D-1), so the compiler cannot tell either case from the other.
- The `Bytes` byte-order readers and writers take a caller-supplied `usize`
  offset. The compiler checks nothing about where a `Bytes` came from, but the
  runtime bounds check (`offset + width <= len`, with the wrapped addition a
  failure too) means an offset outside the buffer panics instead of reaching
  the memory, so the offset alone cannot make these two UB.

**What the compiler does check.** That the byte-order methods are written on
an integer or a `Bytes` (the associated `u32.from_be(x)` forms take a value of
the same type); that a `Bytes` write has a `mut` receiver, so the borrow
checker sees the exclusive borrow the mutation needs; that
`Ptr.read_unaligned` only reads a `Copy` element; and that both unaligned
operations are inside an `unsafe` block or an `unsafe fun` in a module
`coral.toml` lists under `[unsafe] modules`. Both backends emit the unaligned
access as an align-1 load or store (`align 1` on LLVM, `memcpy` on C) and the
swap as `llvm.bswap.*` / `__builtin_bswap*`, and `to_le`/`to_be` read the
target's own byte order rather than the host's.

**What it does not check.** The alignment of any address, as before, and
nothing about a `Bytes` beyond its length: whether the bytes at an offset hold
the field the program says they do is the program's claim about the format,
not the type system's. The two APIs are also not bounds-checked against each
other: a `write_u32_le` may overwrite a field a `read_u16_le` expects, and the
language has no rule that ties two accesses to one location (roadmap D-1).

## Stage 4: MK9-S4-M1/M2 freestanding

**New ways to reach UB.** This stage removes the ambient runtime and hands the
program machine state, so the ways to be wrong are the ones C has:

- `_start` sets `sp` from the linker-script symbol `__stack_top`. If the script
  defines it over program data, over the vector table or below the image, the
  first push corrupts something; the compiler checks that a script was named,
  not what it contains. The stub clears nothing else, so it runs with whatever
  the boot loader left in every other register and CSR.
- The entry function's body is ordinary Kelp code, but nothing has run yet: a
  `String` or a `List` there would call an allocator that does not exist, which
  is why those types are refused, not why they are safe. `Atomic<T>` on bare
  metal is only as correct as the target's memory model makes it, and the
  language still publishes no model for a `Ptr<T>` and an `Atomic<T>` over the
  same bytes (stage 1's rule).
- A `section("...")` clause names an object-file section a script may or may
  not place. A clause naming a section the script does not mention silently
  drops the code or data from the image; the compiler only checks the clause's
  syntax and, for a `const`, that the value is a fixed-size integer array.
- The user's panic handler is a promise the compiler does not check. Nothing
  requires it to diverge, so a handler that returns comes back to the bounds or
  overflow check that called it, which then falls through into whatever the
  check was guarding. The default handler (an infinite loop) is the only one
  the compiler itself guarantees does not return.

**What the compiler does check.** That a `[target]` section names a `mode` and,
for freestanding, a `triple`, an `entry` and a `linker_script`; that the entry
is a `fun` in the entry module with no parameters and no return; that a named
panic handler is a `fun(msg: str)` with no return; and the allowed set of
`src/freestanding.rs`, construct by construct, on both `kelpc check` and
`kelpc build`. It also refuses the C backend for a freestanding project rather
than producing an image with libc in it.

**What it does not check.** Anything about the linker script's contents, the
address `__stack_top` names, the alignment or reachability of a section, or
whether the panic handler diverges. The generated `_start` and the script are
one contract split across two files, and only a boot (the next stages) tests it
end to end.
