# Kernel 0: what boots, and what is still missing

*The integration stage of `plan/kernel-path.md`'s "kernel 0": the pieces stages 1 to 5 added,
wired into an image QEMU's riscv64 `virt` machine actually starts. This file says what was
built, what could not be, and in what order the gaps matter. Stage 7 closed gaps 1 and 3
(a libc-free `str` surface and a panic handler that can print its message), kernel
gap 2 (the function-address form) is closed by `Ptr.from_fn` and the `align(N)` clause, and
kernel gap 4's `.bss`-clear half is closed by the compiler-emitted clear loop and the weak
`mem*` fallbacks.*

## What was built

- **`examples/kernel0/`**, a full freestanding project: `coral.toml` (`[target]` with
  `mode = "freestanding"`, `triple = "riscv64-unknown-none-elf"`, `entry = "kmain"`,
  `linker_script = "kernel.ld"` and `panic = "kp_panic"`; `[unsafe] modules = ["uart"]`),
  `kernel.ld`, `src/main.kp` (the entry and the panic handler) and `src/uart.kp` (the NS16550
  driver: poll LSR at offset 5 for bit 5, then write THR at offset 0 through `Ptr<u8>` volatile
  stores, plus a `wfi` halt written as `asm`). `kmain` prints `hello from kelp` and halts.
- **`examples/kernel0-trap/`**, the trap half: `src/trap.kp` installs `mtvec` from
  `Ptr.from_fn(trap_handler).addr()` and stashes the helper's address in `mscratch`,
  `src/main.kp` takes a machine-mode `ecall`, and the naked handler saves the
  caller-saved registers, calls `report` to print `trap: ecall`, restores and
  `mret`s; `kmain` then prints `done`. `trap_handler` carries `align(4)` (kernel
  gap 2) and its own `section(".text.trap")`, because `mtvec`'s low two bits are
  the vector mode.
- **`examples/kernel0/run.sh`** and **`examples/kernel0-trap/run.sh`**: build with `kelpc`,
  boot under `timeout 20 qemu-system-riscv64 -machine virt -bios none -nographic -no-reboot
  -serial mon:stdio -kernel <elf>`, read QEMU's stdout until the last expected line appears,
  kill QEMU, and assert every expected line appears whole. The lines come from QEMU's own
  stdout, not from the source. Stage 7 added **`examples/kernel0-panic/`**, a copy of the
  driver whose `kmain` reads one element past an array's end; its `run.sh` asserts the
  *shape* of the handler's line (starts with `panic: `, contains `index out of bounds`)
  rather than a whole line, because the text is the compiler's.
- **`examples/kernel0-bss/`** (kernel gap 4): proves the entry's clear loop
  ran by loading a 64-byte marker into a `PROGBITS` `.bss.dirty` section the
  script places inside `[__bss_start, __bss_end)` and printing `bss clear` only
  when every byte of it reads zero. Its `run.sh` is kernel 0's with the
  expected line pinned.
- **`kelpc/tests/compile_and_run.rs`**: the same two boots as acceptance tests
  (`kernel0_boots_and_prints_its_greeting_over_the_uart`,
  `kernel0_handles_a_trap_and_continues_after_it`), skipped unless `ld.lld` and
  `qemu-system-riscv64` are both on PATH, exactly as the stage-4 acceptance test skips.
  Stage 7 added `kernel0_panic_prints_the_checks_own_message_over_the_uart` for the new
  example, `freestanding_str_builds_the_five_operations_with_no_undefined_symbols` and
  `a_std_str_function_outside_the_freestanding_five_is_refused` for the surface, and the
  hosted `the_hosted_str_core_operations_are_unchanged` (which runs on both backends).
  Kernel gap 4 added `kernel0_bss_clears_an_image_loaded_marker_before_the_entry_runs`
  and the four freestanding runtime tests (`freestanding_memcpy`'s zero undefined
  symbols, the weak `mem*` bodies, the `_start` clear range, and the project that
  defines its own `memcpy`).

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
2. **~~There is no function-address expression.~~ Closed (kernel gap 2).**
   `Ptr.from_fn(f)` is the address of a declared `fun` or a `fun` literal that
   captured nothing, as a checked `Ptr<u8>`, legal outside `unsafe` on both
   backends. A closure (one that captured something) is refused with a message
   that says why: it carries an environment its code address cannot describe.
   A top-level `fun` may also carry `align(N)` (a power of two up to 4096),
   lowered as the function's alignment on both backends, so a naked handler's
   first byte can be made `mtvec`-aligned. `examples/kernel0-trap` installs its
   vector with `Ptr.from_fn(trap_handler).addr()` and names no mangled symbol.
   `plan/kernel-ub-notes.md` stage 8 is the design's UB catalogue. Stage 5
   deliberately left out `interrupt` and calling-convention attributes; those
   remain open, and they are what a trap handler would use instead of a naked
   body plus `align(N)`.
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
4. **~~The `.bss` is not cleared.~~ Closed by kernel gap 4; interrupts are
   still not set up.** `_start` now zeroes `[__bss_start, __bss_end)` a word at
   a time after it loads `sp` and before it calls the entry, with both bounds
   declared weak so a script that omits them still links
   (`plan/freestanding.md`). The compiler also defines the five `mem*` routines
   LLVM's own aggregate copies call, weakly and as loops, so a kernel with
   `static` data or a big aggregate copy links with no libc. What remains of
   this gap is the interrupt half: `kernel0-trap` installs `mtvec` itself, and
   nothing enables `mstatus.MIE` or models the privilege transition. The plan's
   stage 10 (panic semantics with no OS) and stage 11 (MMIO capabilities) are
   where that becomes a checked story rather than a comment.
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
- An address-of-function form (gap 2): closed in kernel gap 2, with
  `Ptr.from_fn` and the `align(N)` clause. The `interrupt`/calling-convention
  attributes stage 5 deferred stay open, so a trap handler is still a naked
  body the compiler does not understand as a handler.
- A global allocator hook and arenas (plan stage 7) before anything that allocates at boot.
- Per-CPU storage (stage 8) and the MMU/syscall work (stage 9) before userspace.
- Stack guards, probing and the panic semantics of stage 10, which is what turns gap 3 into a
  real diagnostic.

## How it was verified

`nix-shell ../../overnight/dev-kernel.nix --run 'examples/kernel0/run.sh'` prints
`hello from kelp`; the same for `examples/kernel0-trap/run.sh` prints `trap: ecall` then
`done`, for `examples/kernel0-panic/run.sh` prints
`panic: Array.get: index out of bounds`, and for `examples/kernel0-bss/run.sh` prints
`bss clear` (kernel gap 4). `cargo test --test compile_and_run kernel0` runs all four through QEMU (0.29 s), and the
full suite is run as the task requires.
