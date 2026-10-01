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

**Status: stages 1 to 5 are done.** Stage 1 (MK9-S2 unsafe and pointers): `unsafe` blocks and
`unsafe fun`, the raw pointer type `Ptr<T>`,
`offset`/`byte_offset`/`cast`/`addr`/`is_null`, `read`/`write` and their volatile twins,
`Ptr.alloc`/`free`, and the `coral.toml` `[unsafe] modules` audit list all work on both backends
(`plan/unsafe-and-pointers.md`).

Stage 2 (MK9-S1-M2 layout control) adds `repr(c)` (the default, now guaranteed), `repr(packed)`
and `repr(align(N))`, the compile-time queries `size_of<T>()`/`align_of<T>()`/`offset_of<T>("field")`,
and explicit discriminants on a field-less enum with `repr(u8|u16|u32|u64|i8|i16|i32|i64)` storage
and `to_int`/`from_int`. Both backends write the same layout (`__attribute__((packed))`/`aligned(N)`
and a packed struct body or an alignment anchor), and the C helper in `tests/projects/ffi_layout`
checks the packed byte layout against a real C compiler
(`plan/layout-control.md`).

Stage 3 adds the integer byte-order conversions (`x.swap_bytes()`, `x.to_be()`, `x.to_le()`,
`u32.from_be(x)`, `u32.from_le(x)` for every integer wider than a byte), the bounds-checked
`Bytes.read_*_le/be`/`write_*_le/be` accessors with no alignment requirement, and
`Ptr.read_unaligned`/`write_unaligned`. `to_le` is the identity because the target is
little-endian, asked of the target's own data layout on LLVM and of `__BYTE_ORDER__` on C, so a
big-endian target swaps instead (`plan/data-formats.md`).

Stage 4 (MK9-S4-M1/M2 freestanding, LLVM-only by design) adds a `coral.toml` `[target]`
section (`mode`, `triple`, `entry`, `linker_script`, optional `panic`), the LLVM target machine
for a non-host triple (static relocation, medium code model, `generic-rv64`/`+m,+a,+c` for
riscv64), a compiler-emitted `_start` stub that sets `sp` from `__stack_top` and calls the
entry, the `section("...")` clause on a top-level `const`/`fun` (integer-array consts emitted
as read-only data in their section), and an `ld.lld -T <script>` link with no libc and no
runtime object. The allowed set (no `print`, no `panic`/`assert` builtins, no heap types, no
`std/*` except `std/atomic`, `std/option` and `std/result`, no `pd`) is enforced by
`kelpc check` and `kelpc build` alike, and the overflow and bounds checks call the project's
`panic` handler or a default infinite loop. The acceptance artifact is
`tests/projects/freestanding_min`, checked with the llvm tools under `dev-kernel.nix`
(`plan/freestanding.md`, `plan/kernel-ub-notes.md`).

Stage 5 (MK9-S7-M1/M3 ISA surface, roadmap B-9/B-16) adds the statement and expression
`asm("<template>", in("...") e, out("...") place, inout("...") place, clobber("..."), volatile,
nomem, nostack)`, legal only inside `unsafe` and therefore only in a module the manifest
lists under `[unsafe] modules`; the anonymous `out("...") _` is the expression's value (`i64`
when the context names no type); and `naked fun`, whose body must be exactly one `asm` and
which lowers to LLVM's `naked` plus `noinline` (`__attribute__((naked, noinline))` on C) with
no prologue or epilogue. One template reads destination-first everywhere: the dialect is
Intel on x86 and the target's own syntax elsewhere, chosen per target so both backends
assemble the same text. Both backends carry the feature: LLVM through `LLVMGetInlineAsm` and
LLVM's `naked`/`noinline` enum attributes, the C oracle through GCC's extended asm and
`__attribute__((naked, noinline))`, and the run fixtures pass on both. `interrupt` and
calling-convention attributes are out of scope for this stage: a naked function whose body is
one `asm` covers a trap vector without them. `plan/inline-asm.md` is the design and
`plan/kernel-ub-notes.md` carries the stage's undefined behaviour.

**Kernel 0 boots.** The pieces above are now wired into `examples/kernel0`: a `[target]`
freestanding riscv64 project whose `kmain` polls the NS16550 UART's LSR bit 5 and writes
`hello from kelp` to THR through `Ptr<u8>` volatile stores, then halts on `wfi`;
`examples/kernel0-trap` installs `mtvec` at a naked handler, takes an `ecall`, prints
`trap: ecall` from the handler and `done` after `mret`. Both are built with `kelpc` and booted
under `timeout 20 qemu-system-riscv64 -machine virt -bios none -nographic -no-reboot
-serial mon:stdio -kernel <elf>`; `examples/kernel0/run.sh` and `examples/kernel0-trap/run.sh`
read QEMU's own stdout and assert the expected lines, and
`cargo test --test compile_and_run kernel0` runs both where the tools are present. No
compiler change was needed. What the examples could not do is in `plan/kernel0-status.md`, in
order: a libc-free `str` surface, an address-of-function form (plus the `interrupt` attribute
stage 5 deferred), and a usable panic message.

**Stage 7 closed two of those three.** A freestanding program now has `str.len`,
`str.byte`, `str.eq`, `str.starts_with` and `str.ptr`, lowered as byte loops inside
the module by the LLVM backend (no `strlen`, no C object), with every other
`std/str` function refused by name and pruned from the program before inference;
`examples/kernel0` prints its greeting through `puts(s: str)` and its panic
handler prints `panic: ` plus the compiler's own message, and the new
`examples/kernel0-panic` proves it by reading one element past an array on
purpose.

**Kernel gap 2 (the address of a function) is closed.** `Ptr.from_fn(f)` is the
address of a declared `fun` or a `fun` literal that captured nothing, as a
checked `Ptr<u8>`, legal outside `unsafe` on both backends; a closure is refused
by name, because it carries an environment its code address cannot describe. A
top-level `fun` may carry `align(N)` (a power of two up to 4096), lowered as the
function's alignment on both backends. `examples/kernel0-trap` installs its
trap vector with `Ptr.from_fn(trap_handler).addr()` and names no mangled symbol,
and `plan/kernel-ub-notes.md` stage 8 is the UB catalogue for the new surface.
The `interrupt`/calling-convention attributes stage 5 deferred are the remaining
half of that hole.

**Stage 6's `defer` half is done** (MK9-S6-M1, `plan/defer.md`): `defer EXPR` is a
statement whose action runs when its block is left, on every ordinary exit, last-in-first-out
against the block's other defers, by a lowering that duplicates the action at each exit before
type checking so both backends run ordinary code. A `defer` and a `?` in one block is refused
by name, a `return` inside a deferred action is refused, and a local moved before the exit is
the ordinary use-of-a-moved-value error. Both a hosted fixture set and a freestanding riscv64
project that releases a lock flag with a `defer` are in the suite. User `Drop`, the other half
of the row, is not in this stage.

**What the next stages need.** The table's stages 6 to 11 are the remaining work:
`defer`/`Drop` (row 6), a global allocator hook and arenas (row 7), per-CPU storage (row 8),
mapping and a syscall ABI (row 9), stack guards and panic semantics with no OS (row 10), and
MMIO regions and IRQ lines as capabilities (row 11). The allocator is what anything running at
boot needs first, and the panic semantics of row 10 are what turn a check that fires into a
diagnostic instead of a bare `panic` byte.

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
