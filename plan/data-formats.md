# Data formats (kernel stage 3, MK9-S8 / roadmap B-10)

*The design note for endianness conversion, the byte-order readers and writers
on `Bytes`, and unaligned pointer access. `language/kernel.md` (kelp wiki) is the stage
list; this is stage 3. The undefined-behaviour half is
`language/unsafe-ub-catalogue.md` (kelp wiki)'s stage 3 section.*

## What the stage adds

Three surfaces, one reason each:

- **Integer byte-order conversion.** Every integer **wider than a byte** has
  `x.swap_bytes()`, `x.to_be()` and `x.to_le()`, and every one of them also has
  the two static forms written on the type name, `u32.from_be(x)` and
  `u32.from_le(x)`. The set is `u16`, `u32`, `u64`, `i32`, `i64`, `usize`,
  `isize`.
  - **`u8` has none, deliberately.** Swapping one byte is the identity, and
    `to_be`/`to_le` on a byte are the identity on every target, so the three
    names would differ only in spelling. `Bytes.get` is the byte-sized read.
  - **`i16` does not exist in v1.** The task lists it, but there is no `i16`
    scalar type to hang a method on: kernel stage 2's `repr(i16)` names a tag
    *storage* width and is a different thing. `u16` covers the 16-bit case.
  - The static forms are resolved by the **existing associated-builtin path**
    (`BUILTINS` with `associated: true`, keyed `u32#from_be`), the same
    mechanism `Bytes.empty()` and `AtomicI64.new(0)` use. A type name as a
    receiver was already a value-less receiver the checker resolves before it
    checks a value, so no new module is needed: a `std/endian` of free
    functions would be a second spelling of a mechanism the language has. The
    argument must be the same type the call is written on (`u32.from_be` takes
    a `u32`), which is what Rust's `from_be` does and what makes the two
    spellings interchangeable.
  - **`to_le` is the identity only because the target is little-endian**, and
    that is asked, not assumed: the LLVM backend reads the target's own byte
    order from its data layout (`LLVMByteOrder`), and the C backend compiles the
    direction through `__BYTE_ORDER__`, the C compiler's own report of the
    target. On a big-endian target `to_le` swaps and `to_be` does not, on both
    backends, with no source change.

- **Byte-order readers and writers on `Bytes`.** `b.read_u16_le(offset)`,
  `read_u16_be`, `read_u32_le/be`, `read_u64_le/be`, `read_i32_le/be`,
  `read_i64_le/be`, and the matching `b.write_*(offset, value)` for each. The
  receiver is `&Bytes` for a read and `mut Bytes` for a write (the same
  exclusive borrow `push` takes), and every offset is a `usize`.
  - **Bounds-checked, with a clear panic.** `offset + width <= len`, tested with
    the wrapped addition as a failure too, so an offset near `usize`'s maximum
    cannot wrap under `len`. The message names the method:
    `Bytes.read_u32_le: offset out of bounds`, the same shape `Bytes.get` uses.
  - **No alignment requirement.** The access is a `memcpy` on C and an `align 1`
    load or store on LLVM, so a field at offset 1 is as legal as one at offset 0.
  - `u8` needs no reader (`Bytes.get`), `usize`/`isize` are deliberately absent
    (a wire format fixes its field widths, not the host's pointer width), and
    the `read_*` pair spells its order in the name.
  - The name carries the width and the order, and the frontend, `codegen_c.rs`
    and `codegen_llvm.rs` read the one table `types.rs::bytes_endian_access`
    builds, so the three cannot disagree about what `read_u16_be` means.

- **`Ptr<T>` unaligned access.** `p.read_unaligned()` and
  `p.write_unaligned(v)`, both unsafe operations like the aligned pair. They
  have the same rules as `read`/`write`: `read_unaligned` only reads a `Copy`
  element (a second owner would be made otherwise), and neither converts byte
  order, because a raw pointer access to memory the program already laid out is
  not a data format. `Ptr<T>` exists on this branch (`Ty::Ptr`, kernel stage 1),
  so this item is not skipped.

## How the two backends write it

- **LLVM** (the default): one `llvm.bswap.iN` intrinsic for a swap, and nothing
  at all when the direction is the target's own; an `align 1` load or store
  (`LLVMSetAlignment`) for an unaligned access. The intrinsic declaration is
  reused by name rather than re-added, because `LLVMAddFunction` mangles a
  second declaration (`llvm.bswap.i32.20`) and the verifier rejects that.
- **C** (the oracle): `__builtin_bswap*` through `KELP_SWAP_*` / `KELP_TO_BE_*` /
  `KELP_TO_LE_*` macros in `runtime/kelp_rt.c`, whose direction is
  `__BYTE_ORDER__` and whose `SIZE` width comes from `sizeof(size_t)`. The swap
  is emitted at the call site rather than as a runtime call, so the common case
  is the same `bswap` instruction the intrinsic becomes. An unaligned access is
  a `memcpy` into or out of a temporary.

Both are the same machine operations, and the fixtures assert the same stdout
from both.

## The fixtures

- `endian_scalars.kp`: 23 hand-computed values covering every width, both
  signednesses, both directions, both static forms, and a double swap.
- `bytes_endian.kp`: a 10-byte header (a `u16` LE, a `u16` BE, a `u32` BE and a
  `u16` LE, adjacent with no padding) read field by field in **both** byte
  orders, then rebuilt with `write_*` and compared byte for byte with the pushed
  copy.
- `bytes_read_oob.kp`, `bytes_write_oob.kp`, `bytes_read_wrapped_oob.kp`: the
  panic message for a read past the end, a write past the end, and an offset
  that would wrap the bounds test.
- `tests/projects/unsafe_unaligned`: `read_unaligned`/`write_unaligned` at odd
  offsets inside one allocation, with the module in the `[unsafe] modules` list
  the audit rule requires.

There is no C-side `ffi_*` cross-check for this stage: the byte order is a
property of the bytes a program produces, so the two backends are compared by
the output they print rather than by a second compiler reading the layout (the
check `tests/projects/ffi_layout` wears for kernel stage 2).
