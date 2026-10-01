# Device capabilities: MMIO ranges (kernel stage 16, MK10-S1 / roadmap C-3)

*Design and record for kernel-path row 11's MMIO half. The IRQ-line and
physical-frame halves are not in this stage, and the "Next" section says what
they would need.*

## What this adds

One capability domain, `mmio`, and the compile-time check that gives it
meaning. A grant is a list of half-open address ranges, `"0xBASE+0xLEN"`
(`BASE` and `LEN` in hex or decimal, `+` the separator, `LEN` in bytes), or the
whole address space as `["*"]`:

```kelp
impl "uart" caps(mmio = ["0x10000000+0x100"])
```

The range covers `[base, base + len)`, so `0x100000FF` is the last byte the
grant above admits and `0x10000100` is the first it does not. The grant is
validated where it is written: a bare address, a zero length, a non-numeric
part and two ranges that share a byte are each a compile error at the `impl`
line or the manifest line.

## What it gates

`Ptr.from_addr(<addr>)` in a freestanding project, and nothing else. It is the
only place either backend turns an integer into a pointer (`codegen_llvm.rs`'s
`inttoptr`, `codegen_c.rs`'s cast through `uintptr_t`), and `Ptr.from_fn(..)`
goes the other way, so it stays ungated. Hosted `Ptr.from_addr` is unchanged and
ungated: this domain is about a machine whose addresses are facts, and a hosted
process has an OS between it and the hardware.

- A **constant** address (a literal, a `const`, or arithmetic and bitwise
  operators over those) must fall inside a granted range. The refusal names the
  address and the ranges the module holds.
- A **computed** address is refused unless the grant is `["*"]`, and the message
  says so. The compiler cannot prove a runtime value lands inside a range, so
  the honest answer is "name the whole space or do not do this".
- A module that holds **no** `mmio` grant and calls `Ptr.from_addr` is refused
  by name.

## Where the grant lives

The existing capability mechanism, unchanged: the manifest's `[caps]` section
grants the **entry module** (an entry module cannot write a `caps(...)` clause
on itself), and `impl "<module>" caps(mmio = [...])` delegates a range to an
imported module. A range can only be delegated by a module that holds it, with
the same no-escalation rule every other domain has.

`mmio` is the one domain the entry module does **not** hold by default: a
fallback of `*` would make the whole feature a no-op, and a device's address is
a fact about the machine rather than a resource the root of trust owns.
`caps(*)` therefore hands over every other domain's whole resource and leaves
`mmio` out; the whole-address-space grant has to be written as
`[caps] mmio = ["*"]` or `caps(mmio = ["*"])` where a reviewer will see it.

That is why the existing `examples/kernel0*` projects each grew a `[caps]`
section and a grant on the `impl` line that imports their driver. The UART
drivers name a compile-time constant, so they get exactly their device's
window (`0x10000000+0x100` for the riscv64 NS16550, `0x09000000+0x1000` for the
aarch64 PL011). Modules that turn a computed address into a pointer (a
page-table frame, a trap-frame slot, a user page, an `asm` result) get `["*"]`,
which is recorded per example in `plan/kernel-ub-notes.md`.

## Honest limits

- **Born-at only.** The check is on the address a pointer is *born* at. An
  offset applied afterwards (`p.byte_offset(n)`, `p.offset(n)`, `p.cast<U>()`)
  is not revisited, so a granted range admits any address reachable from it by
  arithmetic. Provenance (roadmap D-1) is what would close this, not a bigger
  range table.
- **Compile time, not runtime.** There is no runtime check and no fence: the
  grant is a resolution-time fact, so a `Ptr.from_addr` that reaches past its
  range by arithmetic is as unchecked as every other `unsafe` operation.
- **`mmio` does not say what is there.** A granted range need not be MMIO, and
  a fixed range need not be a device: `examples/kernel0-vm-fault` grants the two
  probe addresses it takes faults on. The domain scopes authority, not meaning.
- **Freestanding only.** LLVM is the only freestanding backend by design
  (kernel stage 4), so the C oracle never sees this check; hosted builds of both
  backends are unaffected.
- **Not IRQ lines or frames.** An interrupt line and a physical frame are the
  other two device capabilities row 11 names and neither is here.

## Next: IRQ lines and physical frames

Both are the same shape as a range and neither needs a new mechanism:
`irq = ["12"]` would gate the `csrw mie`/`mtvec`-adjacent surface the same way,
and `frames = ["0x80000000+0x400000"]` would gate `Ptr.from_addr` over a frame
allocator's region, which is the same address check this stage already runs.
What they need first is a place to apply them: an IRQ line is claimed at a call
(`irq.claim(n)`), not at a pointer's birth, and a frame is owned and returned
rather than merely named, so both want the runtime-derived authority of roadmap
C-1 and an ownership story (a frame is released when it is freed, not when the
grant is written). Until then a kernel grants the range its frame allocator
carves and says so.

## Tests

- The range parser and its edge cases are unit tests in `stdlib.rs`
  (`parse_mmio_range`, `mmio_ranges_overlap`, `mmio_covers`).
- `kelpc/tests/mmio.rs` writes a freestanding project per case and runs
  `kelpc check` (front end only, so no `ld.lld` and no QEMU): an address inside,
  `base + len - 1`, `base + len`, an address outside, `["*"]` with a computed
  address, a computed address against a range, a module with no grant, a
  malformed range, a zero-length range, overlapping ranges, the wildcard beside
  a range, and a second driver refused for reaching another device's address
  while the same two drivers each inside their own grant check cleanly.
- Every `examples/kernel0*` project still boots under QEMU in `dev-kernel.nix`
  after its grant was added; `plan/kernel-ub-notes.md` stage 16 carries the
  undefined behaviour this stage adds.
