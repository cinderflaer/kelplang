# IRQ lines and physical frames as capabilities (kernel stage 18, MK10-S1 / roadmap C-3)

*Design and record for kernel-path row 11's second half. The MMIO half is
`plan/device-caps.md`; this file owns the two device capabilities it left
open.*

## What this adds

Two capability domains, `irq` and `frames`, and the two gated constructors
that give them meaning. A grant is a list of ranges or the whole resource as
`["*"]`:

```kelp
impl "uart" caps(mmio = ["0x10000000+0x100"])
impl "drv"  caps(irq = ["10"], frames = ["0x80100000+0x100000"])
```

- `irq` reads a decimal line number (`"10"`) or an inclusive decimal range
  (`"32-47"`). Hex is refused, so a line number reads the same to a reviewer and
  to the checker, and `"47-32"` is refused as the empty range it is.
- `frames` reads the same `"0xBASE+0xLEN"` byte window as `mmio`, parsed and
  validated by the same code (`stdlib::parse_byte_range`, `ranges_overlap`,
  `range_covers`). There is one range type for all three domains, not two
  copies: `irq`'s grammar lowers to the same `(start, len)` pair.

A grant is validated where it is written: a malformed pattern, a zero length, a
reversed line range and two ranges that share a line or a byte are each a
compile error at the `impl` line or the manifest line. `["*"]` cannot be listed
beside a range, per domain.

## What this gates

`irq.claim(<n>)` and `frame.claim(<addr>)`, the two calls that build the
capability values, and every other item of the two shipped modules
(`Line.number`, `Frame.addr`, `Frame.offset`) through the ordinary domain gate.
Both modules are gated stdlib modules (`stdlib::domain_of` maps `std/irq` to
`irq` and `std/frame` to `frames`), so a module must hold the domain to call
anything in them, and possession of a `Line` is not by itself authority to read
it.

- A **constant** line or address must fall inside a granted range. The refusal
  names the value and the ranges the module holds.
- A **computed** line or address is refused unless the grant is `["*"]`, and the
  message says so: the compiler cannot prove a runtime value lands in a range.
- A `frames` constant must also be 4096-aligned; the check runs before the
  range question and of the wildcard too, because a misaligned frame is not a
  frame. A computed address is checked at run time by `Frame.claim`, which
  panics with `Frame.claim: address is not 4096-aligned`.
- A module that holds **no** grant and calls either constructor is refused by
  name, at the domain gate.

This check runs in `infer.rs`, in the front end, for a **hosted and a
freestanding** program alike: `kelpc check` and `kelpc build` agree, and both
backends see the same refusal. That is deliberately unlike `mmio`, whose
`Ptr.from_addr` is ungated when hosted; a line and a frame are values the
language can carry in either mode. A hosted program has no hardware, so the
line number and the frame address it claims are only the numbers the compiler
admitted, and nothing in the module pretends otherwise.

## Capability by construction

The value, not the name, is the receipt for a grant, and `claim` is the only
way one enters a program:

```kelp
// std/irq
datac Line(val n: u32)
fun claim(n: u32).R: Line = Line(n = n)
fun Line.number().R: u32 = n

// std/frame
datac Frame(val addr: usize)
fun claim(addr: usize).R: Frame { ... }   // the 4096-alignment check
fun Frame.addr().R: usize = addr
fun Frame.offset(byte: usize).R: usize    // bounds-checked against 4096
```

A `Line` and a `Frame` are handed around as ordinary values, so a driver can be
given exactly what the entry claimed and nothing else, and the type is the
record of the grant.

**How the constructor is sealed.** Kelp has no field-level `pub`, and a
compiler-shipped module exports every item (the transitional rule in
`plan/syntax.md` 13.4), so a bare `datac Line(val n: u32)` would be
constructible from another module by `irq.Line(n = 3)`. The compiler therefore
carries a short list of sealed capability types (`stdlib::SEALED_TYPES`) and
`infer.rs::check_module_item` refuses the `alias.Type(...)` spelling wherever
the caller is not the declaring module. Inside `std/irq` the declaration is
ordinary Kelp; outside, only `claim` builds one. This is the one place where
"private to its module" is a rule of its own rather than the A-13 export check.

## Honest limits

- **Born-at only, like `mmio`.** `Frame.offset` bounds one byte against the
  frame's own 4096, but the `usize` it returns is ordinary arithmetic
  afterwards: a caller that walks on with `+ n` is outside the frame and the
  compiler does not revisit it. Provenance (roadmap D-1) is what would close
  that.
- **Nothing says what is there.** A granted line need not exist on the machine,
  and a granted frame need not be RAM; the domain scopes authority, not
  meaning. Nothing ties a line number to a controller, a mask bit or an
  installed handler, and nothing ties a frame address to real memory.
- **DMA is not tracked.** A frame handed to a device for a DMA transfer is not
  followed: the module may free it, reuse it or hand out a second `Frame` for
  the same address, and the compiler will not see the conflict. There is no
  ownership story beyond the value, and no runtime liveness check.
- **Compile time, not runtime.** The grant is a resolution-time fact. The
  `frames` alignment test for a computed address is the one runtime check, and
  a misaligned address under a range grant is already a compile error.
- **`irq` and `frames` are held by default.** Unlike `mmio`, the entry module
  holds both as `["*"]` unless its `[caps]` section narrows them, so an
  un-narrowed project can claim any line or frame. A kernel that wants the
  narrow story writes the section, as `examples/kernel0-caps` does.

## Example

`examples/kernel0-caps` is the acceptance: a riscv64 image whose `[caps]`
section holds exactly one line and one frame, whose entry claims both and hands
the receipts to `src/drv.kp`, and whose driver prints `line 10`,
`frame 0x80100000` and `offset 0x80100010` over the NS16550. `src/ungranted.kp`
is the compile-error fixture the test suite copies into a scratch project: a
module granted line 10 that reaches for 11.

## Tests

- The range grammar and its edge cases are unit tests in `stdlib.rs`
  (`parse_byte_range`, `parse_irq_range`, `ranges_overlap`, `range_covers`).
- `kelpc/tests/irq_frames.rs` writes a hosted scratch project per case and runs
  `kelpc check` (front end only, no `ld.lld`): inside the grant, the last
  granted line and the next refused, the frame range's end exclusive, `["*"]`
  for both domains, a computed value against a range, a misaligned constant
  frame, a module with no grant, a forged `irq.Line(n = 3)`, a bad grant, an
  empty line range, a zero-length frame range and overlapping ranges.
- The acceptance boots are `examples/kernel0-caps/run.sh` and the
  `kernel0_caps` test in `kelpc/tests/compile_and_run.rs`, where `ld.lld` and
  `qemu-system-riscv64` exist; every other example still boots.
