# `String` and `Bytes` in freestanding mode (kernel stage 12, MK9-S5-M2)

*A design note, not a Mark. `language/freestanding.md` (kelp wiki) owns the mode; this is the
record of what this stage added and why.*

## What changes

Kernel stage 7 rerouted `Box`, `List` and `Map` through `[target]
alloc`/`dealloc` but left `String` and `Bytes` refused, because their hosted
bodies reach `strlen`, `memcpy` and `realloc`. This stage adds libc-free bodies
for the operations a kernel needs, written into the object by the LLVM backend
exactly as `emit_freestanding_str_bodies` writes the five `str` operations and
`emit_freestanding_mem_bodies` writes the weak `mem*` loops.

- With an allocator configured, `String` and `Bytes` are legal types anywhere
  an allowed type is (a local, a field, a parameter, a return, a container
  element). Without one they stay refused, with a message that names the two
  manifest keys.
- `View` is admitted as of kernel stage 23 (`language/freestanding.md` (kelp wiki)): it
  is a borrowed `{ptr, len}` pair with no storage, and `len`/`is_empty`/`get`/
  `slice`/`eq` are byte loops the backend emits, with `View.to_string` in the
  allocator-only set. `Thread` stays refused, by name, with or without an
  allocator.
- Every operation below is a byte loop over the buffer the project's allocator
  returned; no libc symbol is emitted, and the `llvm-nm -u` check stays clean.

## The allowed operations

`String`: `String.empty`, `String.from_str`, `len`, `is_empty`, `as_str`,
`clone`, `push`, `push_str`, `reserve`, `with_capacity`, `clear`, `view`,
equality (through `str.eq` on `as_str`, which lowers to the freestanding
`View.eq`), `str.concat` and the integer-to-text builders `str.from_u64` and
`str.from_i64` (the "interpolation" surface: a kernel builds text with
`str.concat` over those, because `"${}"` is a print-only construct and `print`
needs `printf`). Drop.

`Bytes`: `Bytes.empty`, `Bytes.from_str`, `len`, `is_empty`, `get`, `set`,
`push`, `reserve`, `clear`, `clone`. Drop. (`Bytes.new(n)` has no such
builtin; a zeroed buffer of `n` bytes is `Bytes.empty()` followed by `push`, or
`mem.alloc` when a later stage reroutes `std/mem`.)

Everything else is refused at the call, by name, with "needs the hosted
runtime": `str.split`, `str.join`, `str.repeat`, `str.to_upper`,
`str.char_count`, `str.from_f64`, the parsers, `Bytes.as_str`'s UTF-8
validation, `Bytes.append` and every `String`/`Bytes` method not listed above.
`str.from_f64` is the one this stage singles out: its formatting is
`snprintf`, so a float is refused by name rather than left to fail at link.
The refusal happens in the front end, so `kelpc check` and `kelpc build` give
the same error and nothing reaches the linker.

## Allocation and growth

Every allocation goes through the project's `alloc(size, align)` with
`align = 1`, and every free through `dealloc(ptr, size, 1)`. The size passed
to `dealloc` is `cap + 1`, the exact byte count the matching allocation asked
for (an empty buffer allocates one byte so `data[0]` is always a NUL).

Growth is `kelp_buf_reserve`'s, emitted into the module with the same signature
the hosted runtime object has, so `emit_grow_call` at each append site is
unchanged:

1. if `len + extra <= cap`, return;
2. `want = cap == 0 ? 8 : cap`, then double until `want >= len + extra`;
3. grow to `want + 1` bytes, through the project's `realloc` when the manifest
   names one, else alloc-a-fresh-block, copy `cap + 1` bytes, dealloc the old
   block;
4. store the new data pointer and capacity, and write the NUL terminator at
   `data[len]`.

A null from any of `alloc`, `realloc` reaches `check_alloc`, which calls the
project's `[target] panic` handler with `"out of memory"`, exactly as the
container bodies do.

## Undefined behaviour

`language/unsafe-ub-catalogue.md` (kelp wiki) stage 12 carries it: the allocator's alignment and
size promises are unchecked, a `realloc` that returns a moved block is fine but
one that returns an aliasing block is not, and `dealloc` is handed `cap + 1`
bytes with alignment 1 so a bump allocator that ignores the arguments still
balances. The compiler checks none of this.
