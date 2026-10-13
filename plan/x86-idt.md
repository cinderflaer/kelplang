# Kernel stage 22: the x86-64 IDT, the CPU exceptions and the PIC+PIT timer

*A design note, not a Mark. `language/kernel.md` (kelp wiki) owns the stage list;
`language/kernel.md` (kelp wiki) owns the boot flow this builds on; `language/unsafe.md` (kelp wiki) owns the
`asm`/`naked` surface the gates are written with. This file says what the stage
built and what it deliberately does not.*

## What the stage is for

`examples/kernel0-x86_64-lmode` loads a null IDT and executes `ud2`, so the
`#UD` has no gate and the machine triple-faults. That is the hole this stage
closes: `examples/kernel0-x86_64-idt` builds a real IDT, handles four CPU
exceptions and takes the 8254/8259 timer interrupt, with the same shape the
riscv64 trap stages used (a naked vector, a frame the compiler does not know
about, a Kelp dispatcher, and a resuming `iretq`/`mret`).

## The gate and the IDT

An `IdtEntry` is a `repr(c) datac` of exactly 16 bytes: a 16-bit offset, the
code selector (`0x08`, the boot GDT's 64-bit code segment, read out of the
x86-64 `_start` stub's own GDT), an 8-bit IST (0: no stack switch), an 8-bit
type/attribute byte (`0x8E`: present, ring 0, 64-bit interrupt gate), the
offset's middle 16 bits, the offset's high 32 bits and a reserved word. The
table is `static mut IDT: [IdtEntry; 256]`; `install` writes the five gates the
kernel owns (`#DE` 0, `#UD` 6, `#GP` 13, `#PF` 14 and the timer's 32) from
`Ptr.from_fn(gate).addr()`, using the same offset split (low 16, mid 16, high
32) the descriptor wants. The other 251 gates stay absent: an absent gate is a
`#GP`, which has a handler, so nothing is left pointing at zero.

`lidt` reads ten bytes (a 16-bit limit and a 64-bit base), so the descriptor is
its own `repr(packed) datac` and the gate cannot double as one. Both layouts are
pinned by `tests/projects/x86_idt_layout` on both backends and re-checked at
boot by `trap.layout_ok`, because the gates' asm hardcodes the offsets and the
compiler checks neither end.

## The frame and the gates

An exception taken in ring 0 pushes three words: `RIP`, `CS` and `RFLAGS`. A
privilege change would push `RSP` and `SS` as well, but there is no user mode on
x86-64 yet, so every gate here sees three. Vectors 8, 10-14 and 17 push an
error code below them; the four exception gates the kernel installs split into
"the CPU pushed one" (13, 14) and "the CPU did not" (0, 6), and the second kind
pushes a zero first so both reach the same layout.

The gate then pushes the fifteen general registers in an order that makes them
land at their `TrapFrame` offsets, pushes the vector number, moves the frame
base to `rdi` (the first argument of a SysV call), aligns `rsp` down to 16 bytes
with `and rsp, -16` (the interrupted code's stack is 16-aligned at a call, but
a fault can be taken anywhere), calls the Kelp `trap_dispatch`, restores the
registers, and ends in `iretq`.

Three details are not free:

- **The gate's own target.** `trap_dispatch` is reached through an `asm`
  operand (`in("r") Ptr.from_fn(trap_dispatch).addr()`), so no mangled symbol is
  spelled in the module. LLVM materialises it with one `leaq` before the asm
  blob; the two registers the blob writes before it consumes that operand
  (`rbx`, the frame base, and `rdi`, the argument) are declared clobbers, so the
  allocator cannot park the address in one of them.
- **The frame base is kept in `rbx` across the call**, because `rsp` has been
  moved by the alignment and the callee preserves callee-saved registers.
- **The interrupted `rsp` is not saved**, because a same-level `iretq` does not
  restore one: the CPU leaves `rsp` pointing past the three-word frame, which is
  where the interrupted code already was. Nothing in the handler needs the old
  value.

`trap_dispatch(frame)` reads the vector and prints
`exception <vector> err <code> rip <hex>`, plus ` cr2 <hex>` (from the CR2
register, `asm`) for `#PF`. `#UD` and `#DE` are the two faults the kernel raises
on purpose: the dispatcher adds the faulting instruction's length from a small
table (`ud2` is two bytes, the 64-bit `div rcx` the boot writes is three) to the
saved `rip`, so the `iretq` resumes at the next instruction. `#GP` and `#PF`
have no repair here; the dispatcher prints and halts.

## The timer

The BIOS leaves the 8259 pair mapped over vectors 8..15, which collide with the
CPU's own exceptions, so the boot remaps them with ICW1..4: the master to
32..39 and the slave to 40..47, the slave on IRQ2. Only IRQ0 is then unmasked
(master IMR `0xFE`, slave IMR `0xFF`), which is the one line this kernel wants.
Channel 0 of the 8254 is programmed through port 0x43 (mode 3, lo/hi, binary)
with a divisor of 11932, so the PIT's 1.193182 MHz input becomes about 100 Hz.
Vector 32's gate is the same naked shape with a dummy-zero error code, so the
timer and the exceptions share one `TrapFrame` and one dispatcher.

Five ticks are counted in `pic.on_tick`: it prints `tick <n>`, sends the EOI
(`outb(0x20, 0x20)`) and, on the fifth tick, masks the line itself before it
publishes the count, so a sixth tick cannot slip in between the handler's return
and the boot's own mask. `kmain` spins until the count is five, masks the line
again (idempotent), prints `x86: done` and halts.

## The order of the boot's checks

`kmain` builds the IDT, takes a `#UD` and a divide by zero (both repaired and
resumed), starts the timer, waits out five ticks, masks it and prints
`x86: done`, and then stores through 8 GiB. The stub's identity map covers the
first 1 GiB only, so that store is a not-present supervisor write: the `#PF`
gate prints its line and halts, which makes it the last thing the serial port
ever sees. The manifest grants `mmio` exactly that one page, the way
`examples/kernel0-vm-fault` grants the two addresses it faults on.

## What is not here

The APIC (local and I/O), SMP, ACPI, a TSS or an IST stack, a separate trap
stack, `syscall`/`sysret`, x86 user mode, and the SSE/XMM state in the frame
(the gate saves the general registers only, and nothing in this kernel uses
`xmm` between two faults). Priorities, blocking and preemption of user mode are
the same open work they are for riscv64.
