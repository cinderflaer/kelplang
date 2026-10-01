# Kernel 0: what boots, and what is still missing

*The integration stage of `plan/kernel-path.md`'s "kernel 0": the pieces stages 1 to 5 added,
wired into an image QEMU's riscv64 `virt` machine actually starts. This file says what was
built, what could not be, and in what order the gaps matter.*

## What was built

- **`examples/kernel0/`**, a full freestanding project: `coral.toml` (`[target]` with
  `mode = "freestanding"`, `triple = "riscv64-unknown-none-elf"`, `entry = "kmain"`,
  `linker_script = "kernel.ld"` and `panic = "kp_panic"`; `[unsafe] modules = ["uart"]`),
  `kernel.ld`, `src/main.kp` (the entry and the panic handler) and `src/uart.kp` (the NS16550
  driver: poll LSR at offset 5 for bit 5, then write THR at offset 0 through `Ptr<u8>` volatile
  stores, plus a `wfi` halt written as `asm`). `kmain` prints `hello from kelp` and halts.
- **`examples/kernel0-trap/`**, the trap half: `src/trap.kp` installs `mtvec` with
  `csrw` from inline asm, `src/main.kp` takes a machine-mode `ecall`, and the naked handler
  saves the caller-saved registers, calls `report` to print `trap: ecall`, restores and
  `mret`s; `kmain` then prints `done`. `kernel.ld` aligns the handler's own section to 4,
  because `mtvec`'s low two bits are the vector mode.
- **`examples/kernel0/run.sh`** and **`examples/kernel0-trap/run.sh`**: build with `kelpc`,
  boot under `timeout 20 qemu-system-riscv64 -machine virt -bios none -nographic -no-reboot
  -serial mon:stdio -kernel <elf>`, read QEMU's stdout until the last expected line appears,
  kill QEMU, and assert every expected line appears whole. The lines come from QEMU's own
  stdout, not from the source.
- **`kelpc/tests/compile_and_run.rs`**: the same two boots as acceptance tests
  (`kernel0_boots_and_prints_its_greeting_over_the_uart`,
  `kernel0_handles_a_trap_and_continues_after_it`), skipped unless `ld.lld` and
  `qemu-system-riscv64` are both on PATH, exactly as the stage-4 acceptance test skips.

No compiler change was needed: stages 1 to 5 already carry every construct the examples use.

## Gaps, most important first

1. **A freestanding program cannot use `str` as text.** The allowed set permits a `str`
   literal, but the only length and byte operations live in `std/str`, whose bodies are
   `strlen`-based `extern`s and are refused in this mode. Both messages here are therefore
   `[u8; N]` constants copied to the stack and handed to the driver as `Ptr<u8>` plus a length.
   A kernel that formats, compares or slices text has nothing to call. A `str` is a
   pointer-and-length pair internally, so the missing piece is a small, libc-free
   `std/str` surface (or the builtin length/byte operations named directly) that the
   freestanding allowed set accepts.
2. **There is no function-address expression.** `trap.install` writes `la {0},
   kelp_trap__trap_handler` inside an asm template: the vector is spelled with the module's
   mangled symbol, a name neither the type checker nor the linker checks. `Ptr<T>` cannot be
   made from a `fun` value and a `fun` name is not a machine-word operand, so the only route
   today is the assembler's own symbol lookup. Stage 5 deliberately left out `interrupt` and
   calling-convention attributes; a `Ptr.from_fun`/`&fun` form is the other half of the same
   hole, and it is what a kernel needs before a trap vector is a checked thing.
3. **The panic handler's message is unusable.** `[target] panic` requires `fun(msg: str)`,
   and gap 1 means the handler cannot print it. `kp_panic` prints the literal `panic` and
   loops, so a bounds or overflow failure says that something failed, never which check.
   Closing gap 1 closes this.
4. **The `.bss` is not cleared and interrupts are not set up.** `_start` loads `sp` and calls
   the entry, nothing else (stage 4's contract). `kmain` does not use `.bss`, so kernel 0
   boots, but a kernel with statics has to zero it and set up `mtvec` before any `wfi`-with
   `mstatus.MIE`. The plan's stage 10 (panic semantics with no OS) and stage 11 (MMIO
   capabilities) are where this becomes a checked story rather than a comment.
5. **No C-oracle coverage.** Freestanding is LLVM-only by design (stage 4), so the C backend
   refuses these projects and the usual "both backends, same behaviour" rule does not apply
   to them. The rule still holds everywhere else.
6. **QEMU's `-bios none` start address is 0x80000000, not the ELF entry.** lld places
   `.eh_frame` as an orphan before an unnamed `.text`, so the driver functions' unwind tables
   put `_start` at 0x8000008c; QEMU then jumps to 0x80000000 and executes data. Both linker
   scripts here name `.eh_frame` explicitly. This is worth a sentence in
   `plan/freestanding.md`, which says a script should place `.text.boot` first without
   saying why the orphan can preempt it.

## What the next stages need

- A libc-free `str` length/byte surface (gap 1): without it, no kernel prints an argument.
- An address-of-function form (gap 2) and, after it, the `interrupt`/calling-convention
  attributes stage 5 deferred, so a trap handler is a declaration the compiler understands.
- A global allocator hook and arenas (plan stage 7) before anything that allocates at boot.
- Per-CPU storage (stage 8) and the MMU/syscall work (stage 9) before userspace.
- Stack guards, probing and the panic semantics of stage 10, which is what turns gap 3 into a
  real diagnostic.

## How it was verified

`nix-shell ../../overnight/dev-kernel.nix --run 'examples/kernel0/run.sh'` prints
`hello from kelp`; the same for `examples/kernel0-trap/run.sh` prints `trap: ecall` then
`done`; `cargo test --test compile_and_run kernel0` runs both through QEMU (0.29 s), and the
full suite is run as the task requires.
