# Global `static` data (kernel stage 7, MK9-S4-M6)

*Design note. `plan/kernel-path.md` orders the stages; this one closes kernel 0's gap 4 ("the
`.bss` is not cleared ... a kernel with statics has to zero it") by giving the language the
storage a kernel needs for page tables, run queues and a console cursor.*

## The surface

```
static MAGIC: u32 = 0x2b1d
static mut CURSOR: usize = 0
pub static NAMES: [u8; 4] = [0x4b, 0x45, 0x4c, 0x50]
section(".data.boot") static mut BOOT: u32 = 1
```

`static NAME: T = INIT` is one object with a stable address for the whole program. `static mut`
is the mutable form. `pub` and the existing `section("...")` clause apply to both, and the
declaration sits in the canonical order next to `const` (after `impl`, before `datac`/`enum`/`fun`).

`T` is closed:

- a `Copy` scalar (`bool`, `char`, every integer, `f32`/`f64`),
- `[T; N]` where `T` is one of the allowed types: a `Copy` scalar, a `datac` of
  those, or another such array (kernel stage 21 widened the element from a
  scalar so that a scheduler's `[Task; N]` table has a spelling),
- a `datac` whose fields are all of the allowed types (recursively),
- `Atomic<T>`.

A heap type (`String`, `Bytes`, `List`, `Box`, `Map`), a borrow, a `View`, a function value, an
`extern type` handle and a `Ptr<T>` are all refused by name. Nothing that needs a drop can be a
static, so a static never owes cleanup and never joins the drop plan.

`INIT` must be a constant expression: a literal, an array literal (`[1, 2, 3]`) or repeat
(`[0; 4096]`), a `datac` constructor with constant arguments, `Atomic.new(<const>)` (and the
`AtomicI64.new(...)` alias), or another `const`. Anything else is refused with "the initializer
is not constant".

## Semantics

- One object, one address, for the lifetime of the program. A read of a `static` is a load; the
  address is `&NAME`, and `Ptr.from_ref(&NAME)` / `Ptr.from_mut(NAME)` work on one.
- `static NAME` is read-only (`.rodata`, or `.data` when it holds an `Atomic`, because an
  atomic's operations write through a shared borrow). Reading one is legal everywhere, and
  taking its address is legal (a shared borrow).
- `static mut NAME` is mutable. Every read, write, field access and address-taking of it is
  `unsafe`: it is legal only inside `unsafe { ... }` or an `unsafe fun`, so only in a module
  `coral.toml` lists under `[unsafe] modules`. The check fires where the name is resolved, so a
  read (`NAME + 1`), a write (`NAME = v`), a field (`NAME.f`), a method receiver (`NAME.get(i = 0)`)
  and an address (`Ptr.from_mut(NAME)`, `&NAME`) are all covered by the one gate. This is the
  same rule stage 1 set for `Ptr<T>`.
- An `Atomic<T>` static is the safe shared mutable: `COUNTER.fetch_add(1, order = SeqCst)` needs
  no `unsafe`, because every `Atomic` operation takes a shared borrow. This is the shape a
  kernel's per-CPU counter or a shared flag wants without an audit entry.
- A static initialised to all zero goes to `.bss` (LLVM `zeroinitializer`, the section default);
  any other value goes to `.data`; a `section("...")` clause overrides both. An immutable static
  with a nonzero value is emitted `constant` (read-only), except one that holds an `Atomic`,
  which is emitted mutable so its methods can write through the shared borrow.

## Where it lives

`static` is a lexer keyword and an `ast::StaticDecl`, prefixed per module by `resolve.rs` exactly
as a `const` is. `infer.rs` resolves the declared type against the same closed set, folds the
initializer into a `tir::StaticInit` tree (so no code runs at startup and a freestanding image
needs no init hook), and answers a name in a body with `tir::ExprKind::StaticRef`. The reference
is a *place*: codegen takes its address for `Ptr.from_ref`/`Ptr.from_mut`, stores through it for
an assignment, and loads from it everywhere else. `borrowck.rs` sees it as a place with no owner.

Both backends carry it: C writes one file-scope definition (`static`/`static const`, with
`__attribute__((section("...")))` when a clause names one), LLVM one `GlobalVariable` with
`zeroinitializer` or a constant initializer, internal linkage and the right section. A
freestanding program accepts a static of the allowed types by construction: a heap type cannot
appear in one, so the allowed set needs no extra test beyond "this is a static".

## What the compiler does not check

The address of a static is stable, but `Ptr.from_mut(NAME)` still hands out a raw pointer with no
provenance, exactly as stage 1 describes, and `static mut` is the language's own promise that the
program's `unsafe` code sequences its access: two threads that touch the same `static mut` without
an `Atomic` or a fence are a data race the compiler does not see, and a `static mut` written
inside an `unsafe` block is no more checked than a `Ptr` write. The `.bss` of a freestanding image
is zeroed by the loader only because QEMU's `-kernel` does so; `_start` still clears nothing.
`plan/kernel-ub-notes.md` carries the stage's section.
