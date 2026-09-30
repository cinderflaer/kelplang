# The path to a kernel written in Kelp

*A plan, not a Mark. It orders the existing MK9/MK10 items (`roadmap.md` block B) by what a first kernel
needs, names the milestone that proves each step, and records the one decision it changes.*

## The decision this changes

B-6 (freestanding mode) was **deferred on 2026-09-26** because "nothing concrete (an actual kernel, firmware
image or embedded target) is asking for this today" and it said to "revisit the moment a real freestanding
target exists". A kernel is now the stated goal, so that condition is met and the deferral is lifted. The
ambient-runtime assumption stays the default: freestanding is a mode a project opts into in `coral.toml`.

## The driver: "kernel 0"

A kernel that boots under QEMU's RISC-V `virt` machine (`qemu-system-riscv64 -machine virt -bios none
-nographic -kernel kernel.elf`, headless), sets up a stack, writes a line to the NS16550 UART at
`0x10000000`, and halts, **written in Kelp with no C and no libc**. Every stage below is justified by the
piece of kernel 0 it unlocks. The acceptance test is a script that boots the image under QEMU with a timeout
and compares the serial output.

**Why RISC-V `virt` and not x86-64 first.** QEMU's `-kernel` loads a 64-bit RISC-V ELF directly and starts it
at its entry point in a clean machine mode, and the UART is plain memory-mapped I/O, so kernel 0 exercises
raw pointers and `volatile` honestly. The x86-64 equivalent would need a 32-bit Multiboot stub that enters
long mode before any Kelp runs: real work, but it is bootstrapping, not language. LLVM 21 (the one the
compiler already links) builds the RISCV target, `lld` links it, and all three are in nixpkgs.

## Order, and why

| # | Stage (roadmap id) | What it adds | Unlocks in kernel 0 |
| --- | --- | --- | --- |
| 1 | **MK9-S2 unsafe and pointers** (B-3) | `unsafe` blocks, raw pointer type `ptr<T>`, offset/cast/addr, `read`/`write`, `read_volatile`/`write_volatile` | Talking to a device register and to memory the language did not allocate |
| 2 | **MK9-S1-M2 layout** (B-2) | `repr(C)`, `packed`, alignment, `size_of`/`align_of`/`offset_of`, explicit enum discriminants | Page-table entries, trap frames, device-register blocks |
| 3 | **MK9-S8 data formats** (B-10) | endianness conversion, unaligned load/store | Reading a boot-info structure, later a disk format |
| 4 | **MK9-S4-M1/M2 freestanding** (B-6) | a `freestanding` project mode: no libc, no runtime preamble, custom entry point, custom panic handler, linker script, section control, target triple, static ELF | An ELF QEMU can load and start |
| 5 | **MK9-S7-M1/M3 ISA surface** (B-9, B-16) | inline asm with operands and clobbers, naked functions, interrupt/calling-convention attributes | `wfi`, setting the stack pointer, CSR reads and writes, the trap vector, the first context switch |
| 6 | **MK9-S6-M1 `defer`, `Drop`** (B-8) | deterministic cleanup a kernel can point at | Releasing locks and frames on every exit path |
| 7 | **MK9-S5 allocators** (B-7) | a global allocator hook, arenas, alignment, fallible allocation | The first frame allocator and a kernel heap |
| 8 | **MK9-S3-M3 per-CPU storage** (B-11) | a per-CPU storage class | Per-CPU run queues, current-task pointers |
| 9 | **MK9-S4-M4/M5** (B-12, B-13) | mapping/unmapping, page-table walks; a syscall ABI and untrusted-pointer validation | Userspace |
| 10 | **MK9-S7-M2, MK9-S6-M2** (B-14, B-15) | stack guards and probing; panic semantics with no OS | A kernel that fails loudly instead of silently |
| 11 | **MK10-S1** (C-3) | MMIO regions, IRQ lines, frames as *capabilities* | The safety story: a driver can only touch what it was granted |

Stages 1-5 are kernel 0. 6-11 turn it into something that can run more than one thing.

**Status: stage 2 (MK9-S1-M2 layout control) is done.** Stage 1 (MK9-S2 unsafe and pointers) is
also done: `unsafe` blocks and `unsafe fun`, the raw pointer type `Ptr<T>`,
`offset`/`byte_offset`/`cast`/`addr`/`is_null`, `read`/`write` and their volatile twins,
`Ptr.alloc`/`free`, and the `coral.toml` `[unsafe] modules` audit list all work on both backends
(`plan/unsafe-and-pointers.md`).

Stage 2 adds `repr(c)` (the default, now guaranteed), `repr(packed)` and `repr(align(N))`, the
compile-time queries `size_of<T>()`/`align_of<T>()`/`offset_of<T>("field")`, and explicit
discriminants on a field-less enum with `repr(u8|u16|u32|u64|i8|i16|i32|i64)` storage and
`to_int`/`from_int`. Both backends write the same layout (`__attribute__((packed))`/`aligned(N)`
and a packed struct body or an alignment anchor), and the C helper in `tests/projects/ffi_layout`
checks the packed byte layout against a real C compiler
(`plan/layout-control.md`).

## Two rules that keep the language honest

1. **`unsafe` is auditable, not ambient.** A module may contain `unsafe` only if `coral.toml` lists it
   (`[unsafe]` with `modules = [...]`), the same shape as `[extern.*]`: the set of modules that can leave the
   checked language is a short, reviewable list, and a dependency cannot grant itself the power.
2. **Both backends, always.** LLVM is the default backend and C is the oracle. A stage is not done until both
   produce the same observable behaviour, and freestanding output is an LLVM-backend feature first (the C
   backend's runtime preamble is libc), with the C oracle covering the hosted parts.

## What is deliberately not here

- A specific kernel design (scheduler, filesystem, drivers). The language work above is what they would sit on.
- x86-64 and aarch64 kernels. The target triple is a parameter of stage 4; kernel 0 is riscv64 only, and an x86-64
  kernel follows once the stages are in.
- Memory-safety proofs about `unsafe` code. D-1/D-2/D-3 stay open; the UB catalogue has to grow a section per
  stage, and each stage's design note says what it adds to it.
