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
| 9 | **MK9-S4-M4/M5/M8** (B-12, B-13) | mapping/unmapping and page-table walks (done: Sv39, `plan/address-spaces.md`); a syscall ABI and untrusted-pointer validation (done: `plan/syscall-abi.md`) | Userspace |
| 10 | **MK9-S7-M2, MK9-S6-M2** (B-14, B-15) | stack guards and probing; panic semantics with no OS | A kernel that fails loudly instead of silently |
| 11 | **MK10-S1** (C-3) | MMIO regions, IRQ lines and frames as *capabilities* (done) | The safety story: a driver can only touch what it was granted |

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
project that releases a lock flag with a `defer` are in the suite.

**Stage 6's `Drop` half is done too** (MK9-S6-M1, `plan/drop.md`): `fun mut T.drop()` on a
user `datac`/`enum` is that type's destructor, the same "the method name is the declaration"
shape A-12 slice 1 gave a declared `Map` key pair, so no trait keyword and no new syntax. The
generated `kelp_drop_<T>` calls the user body first and drops the fields after, on both
backends, so every drop point the `DropPlan` already had (block end, before `return`, owned
reassignment, a container element, a containing `datac`/`enum`, the `atexit` pd drop) reaches
it unchanged; a destructor type is excluded from every derived-copy fixpoint, so it is not
`Copy`, and an explicit `g.drop()` is refused by name. The fixtures cover order, early return,
reassignment, container removal, a moved-out value, an `enum` variant, a closure capture and
the RAII atomic lock guard, and a freestanding riscv64 project releases a lock flag through a
destructor with no undefined symbol. Both `defer` and `Drop` still have no panic-path cleanup:
there is no unwinding (`plan/mk9-memory-model.md` §2), so a panic leaks the frame.

**Row 7's hook half is done** (MK9-S5-M1, `plan/freestanding.md`): a freestanding
manifest may name `alloc`, `dealloc` and an optional `realloc` in `[target]`, and
`Box<T>`, `List<T>` and `Map<V>` of allowed element types then allocate and free
through them instead of libc, on the LLVM backend that freestanding mode is.
Without the pair the three containers are refused exactly as before, with a
message that names the keys; `View`/`Thread` stay refused either way. A failed
allocation reaches the project's panic handler with "out of memory".
`examples/kernel0-heap` is the idiom: a bump allocator over a `static mut`
arena, a `List<i32>` of 1..=10, and a counting wrapper that prints `balanced`
when every allocation reached a matching free. Arenas as a language feature and
a fallible allocation surface are not in this stage.

**Row 7's `String`/`Bytes` half is done too** (MK9-S5-M2,
`plan/freestanding-string.md`): with an allocator configured, `String` and
`Bytes` become legal and the operations a kernel needs - the constructors, the
length and byte accessors, `push`/`push_str`, `clone`, `reserve`, `clear`,
`str.concat` and the integer-to-text builders, and drop - are emitted as
libc-free byte loops into the object, growing by doubling through the project's
`realloc` when one is named and alloc + copy + dealloc otherwise. Every other
`String`/`Bytes`/`std/str` operation is refused at the call with "needs the
hosted runtime" (including `str.from_f64`), so nothing is left for the linker to
report. `examples/kernel0-string` is the acceptance: it concatenates, formats
an integer, grows a `Bytes` past several doublings, prints four lines through
the UART under QEMU and proves `ALLOCS == DEALLOCS`; `llvm-nm -u` stays clean.
Without an allocator `String`/`Bytes` stay refused, and `View`/`Thread` stay
refused either way.

**Row 7's arena and fallible-allocation halves are done too** (MK9-S5-M3,
`plan/arena.md`): `Ptr.try_alloc<T>(count)` is `Ptr.alloc`'s non-panicking twin,
`unsafe` and aligned for `T`, libc's `malloc` on both hosted backends and the
manifest's own `[target] alloc` in a freestanding image, with a byte count that
would wrap `usize` answered by the null pointer rather than a short block. The
shipped `std/arena` is a bump arena over a caller-owned byte region, written in
Kelp with no libc call, so it is legal in a hosted program and in a freestanding
image with no allocator at all: `alloc(size, align)` aligns the returned
*address*, returns the null `Ptr<u8>` when the padded end would exceed `cap` or
wrap, and panics on a non-power-of-two `align`; `reset`, `used` and `remaining`
are the rest. `alloc_zeroed` writes through the returned pointer, which is why
`std/arena` is the first shipped module on the compiler's own `[unsafe]` list
(`stdlib::UNSAFE_MODULES`, consulted by `resolve.rs` beside the manifest's). The
acceptance is a hosted pair of fixtures/projects on both backends plus two
riscv64 freestanding projects under QEMU - one carving a `static mut [u8; 4096]`
arena with no allocator, one proving `Ptr.try_alloc` reaches the project's own
allocator - both with `llvm-nm -u` clean. Layouting a `Ptr<T>` `datac` field was
the one compiler-side change the arena needed; it is the same one-pointer reply
the opaque handle already had.

**Row 8's per-CPU half is done** (MK9-S3-M3, `plan/percpu.md`): a freestanding
project may write `percpu static mut NAME: T = INIT`, and a manifest's
`[target] cpus = N` (1 to 64, default 1) makes it `N` copies laid out as one
64-byte-aligned array in `.percpu`, indexed by the new `cpu_index()` intrinsic
(`tp` on riscv64, `TPIDR_EL1` on aarch64; the kernel sets the register).
`cpu_count()` folds to the manifest's constant, a `percpu` without `mut` is
refused, and both the class and the intrinsics are freestanding-only, refused
by `infer.rs` on both backends. The riscv64 `_start` stub now parks every hart
whose `mhartid` is not zero before it loads `sp`, so a `-smp 2` image with no
firmware does not have two harts racing through boot;
`examples/kernel0-percpu/run.sh` boots one, writes a per-CPU static on hart 0,
reads the other copy by setting `tp = 1` by hand, and asserts the four lines
from QEMU's stdout. A per-CPU allocator or run queue is still a kernel design,
not this stage.

**Row 11's MMIO half is done** (MK10-S1, roadmap C-3, `plan/device-caps.md`):
the capability model now reaches devices. A new `mmio` domain takes a list of
half-open address ranges (`caps(mmio = ["0x10000000+0x100"])`) or the whole
address space as `["*"]`, and a freestanding `Ptr.from_addr` is a compile error
unless the module holds a grant that covers the address a constant expression
names, or holds `["*"]` for an address the compiler cannot fold. The grant lives
where every other domain's does, except that the entry module does **not** hold
`mmio` by default: the address of a device is a fact about the machine, so it
has to be written down in the manifest's `[caps]` section and delegated to a
driver with `impl "uart" caps(mmio = [...])`. `caps(*)` deliberately skips
`mmio`, so the whole-address-space escape hatch is always visible. Every
`examples/kernel0*` manifest grew a `[caps]` section and every driver its grant,
and all thirteen still boot under QEMU; `kelpc/tests/mmio.rs` covers the
accepting and refusing shapes with `kelpc check` (front end only, no `ld.lld`).
The check is on the address a pointer is *born* at, so an offset applied later
is not revisited; IRQ lines and physical frames are the rest of the row.

**Row 11's IRQ-line and frame halves are done too** (MK10-S1, roadmap C-3,
`plan/irq-frame-caps.md`): the two remaining device capabilities are domains
now, `irq` with decimal line numbers and inclusive ranges
(`caps(irq = ["10", "32-47"])`) and `frames` with the same `BASE+LEN` byte
windows `mmio` uses, parsed and validated by the same code. A line or a frame
is a *value*: `irq.claim(n)` and `frame.claim(addr)` are gated constructors in
the shipped `std/irq` and `std/frame`, so a driver can be handed exactly what
it was granted, and a `Line`/`Frame`'s constructor is sealed to its module so
another module can only get one from a claim. A constant argument must fall
inside a granted range, a `frames` address must be 4096-aligned (a compile
error for a constant and a panic for a computed one), and a computed argument
needs the whole-resource `["*"]`. Unlike `mmio`, the check is in `infer.rs` and
applies to a hosted program and a freestanding image alike; the entry module
holds both domains by default, so a kernel narrows them in its `[caps]`
section. `examples/kernel0-caps` is the acceptance, `kelpc/tests/irq_frames.rs`
the accepting and refusing front-end cases, and every other `examples/kernel0*`
image still boots.

**What the next stages need.** `defer`/`Drop` (row 6), the whole of row 7 (the allocator hook,
arenas and the fallible allocation surface), per-CPU storage (row 8), mapping plus the syscall
ABI (row 9), both halves of row 10 (guards and probing, panic semantics) and the whole of row 11
(MMIO ranges, IRQ lines and physical frames) are all done. The language work the kernel path
names is complete; §"What is deliberately not here" is what is left.

**Stage 10 (a second architecture) is done.** The `_start` stub and the target machine are now
chosen by the manifest's `[target] triple` instead of assuming riscv64: `riscv64` keeps
`generic-rv64`/`+m,+a,+c`, the medium code model and the `la sp` stub, and `aarch64-unknown-none`
gets the `generic` CPU, the **small** code model (LLVM refuses medium on AArch64) and an AArch64
stub that loads `sp` with `ldr x0, =__stack_top; mov sp, x0`, clears `[__bss_start, __bss_end)`
the same word at a time and halts on `wfe`. Any other triple is refused by name at the stub.
`examples/kernel0-aarch64` is the PL011 driver (`DR` at `0x09000000`, `FR` bit 5 the FIFO-full
flag) whose `kmain` prints `hello from kelp (aarch64)` and halts; it boots under
`qemu-system-aarch64 -M virt -cpu cortex-a57 -nographic -serial mon:stdio -kernel <elf>` and
`examples/kernel0-aarch64/run.sh` and two tests in `kelpc/tests/compile_and_run.rs` (an
`llvm-readelf`/`llvm-nm`/`llvm-objdump` check of the ELF and the boot) cover it. The riscv64
`examples/kernel0*` all still boot after the change.

**Row 9's mapping half is done** (MK9-S4-M8, `plan/address-spaces.md`): `examples/kernel0-vm`
builds Sv39 tables in Kelp (the PTE bits, the VPN split, the PTE and `satp` encoders, a zeroing
frame allocator over the linker script's `__frames_start`..`__frames_end`, `map_page`,
`map_range`, `unmap_page` and `translate`), identity-maps the image and the UART, maps a page at
`0x3F_FFFF_F000`, drops from machine mode to supervisor mode through `pmpaddr0`/`medeleg`/`mstatus`
and `mret`, turns the MMU on, and proves the high virtual name and the identity-mapped physical
name are one frame. `examples/kernel0-vm-fault` takes a load page fault (13) and a store page
fault (15) on purpose into a naked S-mode `stvec` handler that prints `scause` and `stval`. No
compiler change was needed: the only thing the language could not spell is a linker-script symbol,
and that is stage 5's `asm` with `la`. What this stage deliberately leaves out is TLB
shootdown, ASIDs, huge-page *creation*, Sv48 and a per-section (W^X) identity map.

**Row 9's other half is done too** (MK9-S4-M9, `plan/syscall-abi.md`):
`examples/kernel0-user` adds the `TrapFrame` and the naked `trap_entry` that builds it on a
per-hart trap stack (swapping `sp` with `sscratch`), the drop to U-mode through `sepc`,
`sstatus.SPP` and `sret`, one Sv39 address space per program with its code page at `0x10000` and
its stack page at `0x20000`, the `ecall` ABI (number in `a7`, arguments in `a0`..`a5`, result in
`a0`, a negative result an error) with `write` and `exit`, and the untrusted-pointer check that
makes `write` safe: a length cap, a range bound that cannot wrap before it is checked, and a
page-table walk that demands the U and R bits on every page before a byte is copied through a
kernel buffer. Two hand-written programs prove it, one ending in `exit(7)` and one dying on a
load page fault while the kernel prints the cause and keeps running. No compiler change was
needed, and what the stage leaves out is the process model: no scheduler, one process at a time,
no preemption or timer interrupt, no `fork`, no TLB shootdown, no ASIDs, no huge-page *creation*,
no Sv48 and no per-section (W^X) identity map.

**Row 10's stack half is done** (MK9-S7-M2, `plan/stack-guard.md`): every function a freestanding
LLVM build emits carries `probe-stack="inline-asm"`, so a frame larger than a page touches each
page in order and a guard page catches the overflow instead of the frame stepping over it;
hosted builds carry no attribute. `examples/kernel0-stack` is the acceptance: an Sv39 riscv64
kernel whose kernel stack sits above an unmapped guard page, with a 16 KiB trap stack below the
hole. One boot recurses with a small frame into the guard, the S-mode handler reports `scause`
and `stval` on that stack and resumes a second phase at the top of the kernel stack, and that
phase overflows the same guard with one `[u8; 16384]` frame. What the example forced is a
lowering fix: `[v; N]` now fills the local's own slot in place (`{array}__fill_into`) rather than
returning the array by value, because LLVM's instcombine exhausted memory folding the by-value
store of a frame-sized array at `-O2`.

**Row 10's panic half is done too** (MK9-S6-M2, `plan/panic-semantics.md`): a freestanding panic
handler may be declared `fun(msg: str)` as before or
`fun(msg: str, file: str, line: u32)`, and the compiler refuses any other shape by name. Every
check calls one compiler-emitted entry that exchanges a `.bss` flag (so a check inside the handler
does not recurse) and, on re-entry, calls `kelp_double_panic`, which the compiler defines weakly as
a `wfi`/`wfe` loop and a project may override with a strong definition. The file and line are the
same per-statement `SourceMap` values the hosted line tables carry; generated accessor bodies read
them from two `.bss` globals the current statement stored. `examples/kernel0-panic2` boots one
phase per check (array bounds, narrowing, `str.byte`, the `panic` builtin, `option.unwrap`, and a
double panic) and asserts each exact line; `plan/panic-semantics.md` has the check table and
records that overflow wraps and division by zero stays an unchecked UB, so neither has a message.

## Two rules that keep the language honest

1. **`unsafe` is auditable, not ambient.** A module may contain `unsafe` only if `coral.toml` lists it
   (`[unsafe]` with `modules = [...]`), the same shape as `[extern.*]`: the set of modules that can leave the
   checked language is a short, reviewable list, and a dependency cannot grant itself the power.
2. **Both backends, always.** LLVM is the default backend and C is the oracle. A stage is not done until both
   produce the same observable behaviour, and freestanding output is an LLVM-backend feature first (the C
   backend's runtime preamble is libc), with the C oracle covering the hosted parts.

## What is deliberately not here

- A specific kernel design (scheduler, filesystem, drivers). The language work above is what they would sit on.
- x86-64 kernels. aarch64 is the second architecture and it boots (`examples/kernel0-aarch64`);
  an x86-64 kernel still needs a 32-bit PVH or Multiboot entry stub that enters long mode before
  any Kelp runs, which is bootstrapping rather than language work, so it follows once the stages are in.
- Memory-safety proofs about `unsafe` code. D-1/D-2/D-3 stay open; the UB catalogue has to grow a section per
  stage, and each stage's design note says what it adds to it.
