# Stage 13: arenas and a fallible allocation surface (MK9-S5-M3)

*Design for kernel-path row 7's last two halves. `plan/kernel-path.md` is the
context; this file says what is built, what shape it takes and what is
deliberately left out.*

## What this adds

**A fallible allocation twin.** `Ptr.try_alloc<T>(count)` is `Ptr.alloc<T>` minus
the panic: `unsafe` like it, the same element set and the same alignment promise
("aligned for `T`"), but a failed allocation is the null `Ptr<T>` rather than a
call to the panic handler. Hosted, it is libc's `malloc` on both backends and the
C oracle agrees byte for byte; freestanding, it is the project's own `[target]
alloc` hook, so a kernel gets a fallible allocation without libc. The byte count
`count * size_of::<T>()` is checked before the call on both backends: a request
that would wrap the `usize` product returns null instead of asking the allocator
for a small block. That is the one place `try_alloc` is deliberately stricter
than `Ptr.alloc`, whose product can still wrap; `alloc` is left exactly as it was.

**A bump arena, `std/arena`.** Written in Kelp, no externs and no libc call, so
it works in a hosted program and in a freestanding image alike (it needs no
`[target] alloc` pair, because it allocates nothing: it hands out a caller-owned
region). The surface is the one row 7 asked for:

```text
datac Arena(var base: Ptr<u8>, val cap: usize, var used: usize)
fun new(base: Ptr<u8>, cap: usize).return: Arena
fun mut Arena.alloc(size: usize, align: usize).return: Ptr<u8>
fun mut Arena.alloc_zeroed(size: usize, align: usize).return: Ptr<u8>
fun mut Arena.reset()
fun Arena.used().return: usize
fun Arena.remaining().return: usize
```

`alloc` rounds `used` up to `align`, refuses a request whose aligned end would
exceed `cap`, and returns the null `Ptr<u8>` when it does not fit; `align` must
be a power of two and a zero or non-power value panics with
`arena align must be a power of two`. The arithmetic is overflow-safe in the
order that matters: the padding is computed with `%` and `/` (no `used + align`
that can wrap), and both `used + padding` and the aligned `+ size` are refused
when they would wrap, so a huge `size` returns null rather than a small
allocation. `reset` sets `used` back to zero, which makes the whole region
available again. `alloc_zeroed` is the one unsafe operation in the module: it
writes zeros through the returned raw pointer, which is what a kernel's frame
allocator wants and what puts the module on the `[unsafe]` list below.

**`Arena.new` is spelled `arena.new(base, cap)`.** Kelp has no associated-function
call: a dotted receiver is always a *value*, and a `fun T.name` declaration is a
method with an implicit `self`, so `Arena.new(...)` is not a name the language can
resolve. Adding one is compiler surface the stage does not need, so the
constructor is a module-level `new` reached through the `impl` alias exactly as
every other shipped function is (`str.len`, `mem.alloc`, `list.map`). The datac's
own literal constructor, `arena.Arena(base = b, cap = c, used = 0)`, still works.

## The `[unsafe]` gate for a shipped module

`std/arena` is the first module *shipped with the compiler* that contains
`unsafe`. The audit rule (`resolve.rs`) requires the module's id to be in the
project's `coral.toml` `[unsafe] modules` list, which is right for a project
module but wrong for the compiler's own text: no project should have to list
`"std/arena"` to import it. So the compiler keeps its own short list,
`stdlib::UNSAFE_MODULES`, and `resolve.rs` admits a module that is either in the
manifest's list or on that list. A project module can never put itself on the
compiler's list (the list is in the compiler's source, not a file the project
writes), so the rule "a dependency cannot grant itself the power" is unchanged;
what the compiler's list admits is only the code the compiler already ships and a
reviewer reads in `kelpc/std/`. `std/arena` is its first entry, because
`alloc_zeroed` writes through a raw pointer.

## Freestanding and hosted

Nothing in `std/arena` reaches the runtime, so the freestanding allowed set
(`freestanding.rs`) admits the module beside `std/atomic`, `std/option` and
`std/result`, and `prune` keeps its bodies. `Ptr.try_alloc` is admitted only when
the manifest configures `[target] alloc`/`dealloc`, the same condition as `Box`,
`List`, `Map`, `String` and `Bytes`; without the pair its refusal names the two
keys. The freestanding acceptance is a riscv64 project that carves a
`static mut [u8; 4096]` arena, writes and reads sentinels through the returned
pointers, prints its results over the UART and links with no undefined symbol.

The C backend refuses freestanding projects by design (its preamble is libc), so
the hosted halves of this stage run on both backends through the suite's two
invocations, and only the freestanding project is LLVM-only.

## What is deliberately not here

- A global allocator *trait* or a per-type allocator. The manifest already names
  one global pair; an arena is the per-subsystem tool and it needs no trait.
- Typed carving (`Arena.alloc<T>`), huge-page requests, DMA-safe allocation, and
  freeing a single allocation: an arena reclaims in bulk (`reset`) or not at all.
- Any memory-safety proof about `unsafe` code (D-1/D-2/D-3 stay open); the new
  ways to reach UB are in `plan/kernel-ub-notes.md`'s stage 13.
