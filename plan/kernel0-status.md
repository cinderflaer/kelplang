# Kernel 0: what boots, and what is still missing

*The integration stage of `plan/kernel-path.md`'s "kernel 0": the pieces stages 1 to 5 added,
wired into an image QEMU's riscv64 `virt` machine actually starts. This file says what was
built, what could not be, and in what order the gaps matter. Stage 7 closed gaps 1 and 3
(a libc-free `str` surface and a panic handler that can print its message).*

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
  stdout, not from the source. Stage 7 added **`examples/kernel0-panic/`**, a copy of the
  driver whose `kmain` reads one element past an array's end; its `run.sh` asserts the
  *shape* of the handler's line (starts with `panic: `, contains `index out of bounds`)
  rather than a whole line, because the text is the compiler's.
- **`kelpc/tests/compile_and_run.rs`**: the same two boots as acceptance tests
  (`kernel0_boots_and_prints_its_greeting_over_the_uart`,
  `kernel0_handles_a_trap_and_continues_after_it`), skipped unless `ld.lld` and
  `qemu-system-riscv64` are both on PATH, exactly as the stage-4 acceptance test skips.
  Stage 7 added `kernel0_panic_prints_the_checks_own_message_over_the_uart` for the new
  example, `freestanding_str_builds_the_five_operations_with_no_undefined_symbols` and
  `a_std_str_function_outside_the_freestanding_five_is_refused` for the surface, and the
  hosted `the_hosted_str_core_operations_are_unchanged` (which runs on both backends).

Kernel 0 needed no compiler change: stages 1 to 5 already carry every construct
the original two examples use. Stage 7 (the gaps below) did change the compiler,
and those changes are described with the gaps.

## Gaps, most important first

1. **~~A freestanding program cannot use `str` as text.~~ Closed (stage 7).** A
   freestanding program now has five `str` operations, with the same call
   spellings the hosted module exposes: `str.len`, `str.byte`, `str.eq`,
   `str.starts_with` and `str.ptr`. The three raw externs are lowered by the
   LLVM backend as byte loops inside the module itself
   (`codegen_llvm::emit_freestanding_str_bodies`), so there is no `strlen` call
   and no per-program C object, and the lowerings carry `optnone` because the
   target's own loop-idiom pass would otherwise rewrite the NUL scan into
   `strlen`. The module is present but not callable as a whole: the call-site
   check accepts exactly those five and refuses every other function in it by
   name
   (`str.concat`, `str.split`, `str.parse_i64`, `str.trim`, anything that
   allocates or formats), and `prune` drops the rest of the module (and
   `std/mem`, which it imports) from the AST before inference, so their bodies
   and the type instances they would instantiate never exist. Both messages
   here are now `str` literals; nothing copies a `[u8; N]` any more.
2. **There is no function-address expression.** `trap.install` writes `la {0},
   kelp_trap__trap_handler` inside an asm template: the vector is spelled with the module's
   mangled symbol, a name neither the type checker nor the linker checks. `Ptr<T>` cannot be
   made from a `fun` value and a `fun` name is not a machine-word operand, so the only route
   today is the assembler's own symbol lookup. Stage 5 deliberately left out `interrupt` and
   calling-convention attributes; a `Ptr.from_fun`/`&fun` form is the other half of the same
   hole, and it is what a kernel needs before a trap vector is a checked thing.
3. **~~The panic handler's message is unusable.~~ Closed (stage 7).** The
   compiler's checks already passed the real message (`[target] panic` receives
   `fun(msg: str)` and the text is the check's own, byte-identical to the C
   backend's), and with gap 1 closed `kp_panic` can print it. `examples/kernel0`
   prints `panic: ` followed by the message; `examples/kernel0-panic` reads one
   element past an array on purpose and its `run.sh` asserts that the UART
   printed a line starting `panic: ` and containing `index out of bounds`.
   Distinct texts in this mode: the `Array.get`/`List.get`/`Bytes.get`/`View.get`
   and `str.byte` index checks, the narrowing conversions' `value out of range`,
   and the `panic(msg)` builtin's own text. Integer overflow is not one of them:
   the language wraps, so there is no overflow check to message.
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

- A libc-free `str` length/byte surface (gap 1): closed in stage 7, with the
  five operations and an example that proves the message prints.
- An address-of-function form (gap 2) and, after it, the `interrupt`/calling-convention
  attributes stage 5 deferred, so a trap handler is a declaration the compiler understands.
- A global allocator hook and arenas (plan stage 7) before anything that allocates at boot.
- Per-CPU storage (stage 8) and the MMU/syscall work (stage 9) before userspace.
- Stack guards, probing and the panic semantics of stage 10, which is what turns gap 3 into a
  real diagnostic.

## How it was verified

`nix-shell ../../overnight/dev-kernel.nix --run 'examples/kernel0/run.sh'` prints
`hello from kelp`; the same for `examples/kernel0-trap/run.sh` prints `trap: ecall` then
`done`, and for `examples/kernel0-panic/run.sh` prints
`panic: Array.get: index out of bounds`. `cargo test --test compile_and_run kernel0` runs both through QEMU (0.29 s), and the
full suite is run as the task requires.
