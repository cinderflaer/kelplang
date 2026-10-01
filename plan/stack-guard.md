# Stack guards and probing (kernel stage 14, MK9-S7-M2, the stack half)

*Design for the stack half of `plan/kernel-path.md`'s row 10. The panic-semantics
half (MK9-S6-M2) is not this stage. `plan/address-spaces.md` and
`examples/kernel0-vm` are the context for the address space; this file says what
the guard page needs from the compiler and what the example proves.*

## What this adds

- **The `probe-stack` attribute.** Every function a freestanding LLVM build emits
  carries LLVM's `"probe-stack"="inline-asm"` (`codegen_llvm.rs` sweeps the
  module after the lowering and skips declarations; `llvm_ffi.rs` gains
  `Function::mark_probe_stack`, the string half of the attribute API). A frame
  larger than one page is then built by touching each page in order from the top,
  so a guard page below the stack traps the frame instead of the frame's single
  `sub sp` stepping over the hole. Hosted builds carry no attribute: the sweep is
  inside `if freestanding.is_some()`.
- **`examples/kernel0-stack/`**, a riscv64 `virt` kernel in S-mode with Sv39
  (its `vm.kp` is a copy of `kernel0-vm`'s, not a cross-example dependency). The
  linker script puts the 16 KiB trap stack below the image, an unmapped 4 KiB
  guard page above it, and the 8 KiB kernel stack above the guard. `kmain` maps
  `[image, __guard_bottom)` and `[__guard_top, __frames_end)` and leaves the
  guard page with no leaf.
- **Two phases, one boot.** `smain` installs the trap vector, turns paging on,
  and calls `recurse_small`, a recursion with a 16-byte frame that fills the
  kernel stack until the frame that crosses `__stack_bottom` stores its saved
  `ra` into the guard. The S-mode handler runs on the trap stack (`sscratch`
  holds its top; the naked entry swaps `sp`), prints
  `stack overflow: scause 0xf stval 0x...`, checks `stval` against the guard
  bounds and, for the first fault, points `sepc` at `phase_b` and returns; the
  entry resets `sp` to `__stack_top` and `sret`s there. `phase_b` calls
  `recurse_big`, whose one `[u8; 16384]` frame is larger than the whole kernel
  stack; the probe walks it down page by page into the same guard, and the
  handler prints and halts.

## The compiler change the example forced

`[v; N]` used to lower to a generated `{array}__fill(v)` that returns the array
by value, and a `var buf: [u8; 16384] = [v; 16384]` then stored that value into
the binding's slot. At the default `-O2`, LLVM's instcombine constant-folds the
16384-element `insertvalue` chain the by-value store produces and exhausts
memory: a 3 GiB ceiling was not enough for a 12288-byte array, and the same
hosted shape failed the same way. The lowering now fills the local's own slot in
place through a new `{array}__fill_into(dst, v)` helper (`array_specs` and
`emit_array_bodies` in `codegen_llvm.rs`; `Stmt::Let` calls it when the
initializer is `ArrayRepeat`), so no aggregate value crosses the store and the
loop keeps a runtime index SROA cannot split. The C oracle's `fill` is untouched;
the observable behaviour is the same.

Two more shapes are promises the example has to keep rather than compiler
features:

- A recursive call after which the caller still does work is not a tail call, but
  LLVM's accumulator transform can still rewrite the recursion into a loop on one
  frame. Both recursion functions write the result through a volatile pointer
  after the call; a tail call cannot be reordered across the volatile store, so
  the recursion stays real at `-O2`.
- The base case is `depth >= limit` with `limit` read through a volatile pointer,
  so LLVM cannot prove the recursion non-terminating and collapse it to an
  infinite loop before the stack fills.

## What the example proves

The boot's own QEMU output is the assertion: both faults print `scause 0xf`
(a store page fault) and an `stval` the kernel checks is inside
`[__guard_bottom, __guard_top)`, and the test parses the printed bounds and both
`stval`s to check the same range independently. Phase b is what the probe is for:
without it the frame's `sub sp, sp, 16400` would leave `sp` below the guard and
the frame's own `memset` would write over the trap stack, which is mapped, with
no fault at all.

## Deliberately not here

- **Demand paging or a growable stack.** The guard is a fixed hole; a fault halts
  after it reports, and nothing maps a page to make room.
- **W^X, section bounds, ASIDs, TLB shootdown.** Unchanged from
  `plan/address-spaces.md`.
- **Panic semantics with no OS** (the other half of row 10): no abort message, no
  register dump, no `panic` beyond the handler's two lines.
- **A probe-free path for the C backend.** Freestanding is LLVM-only by design,
  and a hosted C build has no guard page to probe for.

## How it is verified

`examples/kernel0-stack/run.sh` boots the image under `timeout 20
qemu-system-riscv64 -machine virt -bios none -nographic -no-reboot -serial
mon:stdio -kernel <elf>` through the `kernel0` harness and pins `stack: m-mode`,
`stack: paging on`, `stack: phase a`, `stack: stval in guard`, `stack: phase b`,
`stack: stval in guard` and `stack: halted`.
`cargo test --test compile_and_run kernel0_stack` runs the boot and also parses
the guard bounds and both fault addresses;
`freestanding_big_frames_probe_every_page` builds the example and asserts with
`llvm-objdump` that `kelp_recurse_big`'s disassembly has at least three
page-sized `sub sp` steps each followed by a `sd zero, 0(sp)` probe, and that the
`.ll` carries `"probe-stack"="inline-asm"`;
`hosted_functions_carry_no_stack_probe` asserts a hosted `.ll` has no such
attribute. All three need `ld.lld` (and QEMU for the boot), so they are for
`dev-kernel.nix`; `llvm-nm -u` on the image stays clean.
