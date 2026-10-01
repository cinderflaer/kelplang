# Address spaces: Sv39 page tables on riscv64 (kernel stage 10, MK9-S4-M8)

*Design for the mapping half of `plan/kernel-path.md`'s row 9 (MK9-S4-M4/M5). The syscall ABI
and untrusted-pointer validation in that row are not part of this stage. No compiler change was
needed: the stage is `examples/kernel0-vm` and `examples/kernel0-vm-fault`, written in what the
language already has.*

## What this adds

`examples/kernel0-vm/src/vm.kp` is the Sv39 module, in Kelp:

- The PTE bit constants (`V R W X U G A D`), the three-level VPN split (`vpn0`/`vpn1`/`vpn2`,
  `page_offset`), the PTE encoder (`pte_encode`, plus `pte_ppn`/`pte_flags`/`pte_valid`/
  `pte_is_leaf`) and the `satp` encoder and decoders (`satp_encode`, `satp_mode`, `satp_asid`,
  `satp_ppn`: mode 8, ASID 0, PPN).
- A frame allocator over the linker script's `__frames_start`..`__frames_end` region: a bump
  cursor, one 4 KiB frame per call, zeroed before it is returned because a fresh table's absent
  entries must read as invalid. A zero return means exhausted; page-table pages come from here.
- `map_page`, `map_range`, `unmap_page` and `translate(root, va) -> Option<usize>`. The walker
  allocates and links a table at every absent non-leaf entry, refuses to split a superpage, and
  reads a leaf at any level (so a 2 MiB or 1 GiB mapping translates with the bits it ignores taken
  from the address).

Every memory access is a `Ptr<u64>` volatile load or store, and the only two CSRs in the module
are `satp` and `sfence.vma`, written through `asm`. Both are `unsafe`, so `vm` is on the
manifest's `[unsafe] modules` list.

`examples/kernel0-vm/src/main.kp` is the boot the QEMU run exercises, one line per step:
machine-mode start, a root table, an identity map, the UART page, the high page, the drop to
supervisor mode, paging on, the alias, the unmap. The image, its stack and the frame region are
identity-mapped as one 4 MiB RWX span; splitting `.text`/`.rodata`/`.data` apart needs section
bounds the language cannot name, and the point of the stage is the walk, not W^X. Nothing else is
mapped. The UART page is RW, and one page at `0x3F_FFFF_F000` (one page below 2^38, the highest
canonical Sv39 low address) maps to a fresh frame. Under paging, the kernel writes `0xC0FFEE`
through the high virtual name, reads it back, asks `translate` for the physical address and reads
the same bytes through the identity map: `vm: alias ok`. It then unmaps the high page, flushes the
TLB and checks that `translate` answers `None`.

The transition to S-mode is the standard path, all four steps in `enter_supervisor`: `pmpaddr0`
all ones with `pmpcfg0 = 0x1F` (one NAPOT entry covering memory, without which every S-mode
access faults because no entry matches), `medeleg` granting the page faults to S-mode,
`mstatus.MPP = S`, and `mret` through `mepc`. The tables are built in machine mode (which does not
translate) and `satp` is written in supervisor mode, so the code that turns the MMU on is already
running at the privilege the MMU applies to.

`examples/kernel0-vm-fault/src/trap.kp` is the second example's S-mode handler: `stvec` names a
naked `align(4)` function (kernel stage 5's idiom), which saves the caller-saved registers, calls
`report` through `sscratch`, and either advances `sepc` by the faulting instruction's own length
(read from its low bits, because the target enables the compressed extension) and `sret`s, or
halts. `kmain` maps one leaf with R and no W and `smain` takes two faults on purpose: a load from
an address no leaf maps (`scause` 13, `stval` the address) and a store to the read-only leaf (15).
One boot prints both, so the two QEMU runs the acceptance allows are one run with two phases.

## The one thing the language could not spell

A linker-script symbol has no spelling in Kelp, so `__frames_start` and `__frames_end` arrive
through `asm("la {0}, __frames_start", out("r") _)`, exactly as the compiler's own `_start` stub
reaches `__stack_top`. This is the stage-5 surface used as designed rather than a gap: the
alternative was a new `extern`-symbol construct, and the two-symbol region a frame allocator needs
is not worth one. What it costs is the promise the compiler cannot check: the two symbols are
whatever the script says, and a script that omits them resolves both to 0, so the allocator reports
exhausted instead of failing to link (`plan/kernel-ub-notes.md`, stage 11).

## Deliberately not here

- **TLB shootdown.** `flush_tlb` is one local `sfence.vma`; a mapping change on one hart is not
  propagated to another, and there is no IPI or generation counter.
- **ASIDs.** `satp` is always written with ASID 0, so every table switch would also need a full
  flush; the decoder exists, the allocator does not.
- **Huge pages.** The walker translates a superpage leaf and refuses to split one, but
  `map_page` only ever writes a 4 KiB leaf.
- **Sv48.** Only mode 8 is encoded.
- **W^X.** The identity map is one RWX span, as above.
- **Demand paging, guard pages, a kernel address-space abstraction.** Frames are never freed once
  a table holds them, and a fault handler must resume or halt by hand.

## How it is verified

`examples/kernel0-vm/run.sh` and `examples/kernel0-vm-fault/run.sh` boot under
`timeout 20 qemu-system-riscv64 -machine virt -bios none -nographic -no-reboot -serial mon:stdio
-kernel <elf>` and pin every line, including `vm: alias ok` and
`fault: scause 0xd stval 0x40000000` / `fault: scause 0xf stval 0x3ffffff000`.
`cargo test --test compile_and_run -- sv39 vm` runs the hosted encoder project
(`kelpc/tests/projects/vm_pure`, both backends), the two boots and the `llvm-nm -u` clean check on
both images. The toolchain is `dev-kernel.nix`'s.
