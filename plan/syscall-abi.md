# Trap frames, user mode and the syscall ABI (kernel stage 11, MK9-S4-M9)

*Design for the second half of `plan/kernel-path.md`'s row 9. The mapping half is
`plan/address-spaces.md`; this stage adds the trap frame, the drop to U-mode, the
`ecall` ABI and the untrusted-pointer validation the row asked for. No compiler
change was needed: `examples/kernel0-user` is written in what the language
already has, and the one construct it leans on (a linker-script symbol) is the
stage-5 `asm` with `la`.*

## What this adds

`examples/kernel0-user/` is a freestanding riscv64 image, the same boot as
`examples/kernel0-vm` (Sv39, the drop from machine mode, the UART driver), plus
five things:

- `src/trap.kp`: the `TrapFrame`, the naked `trap_entry`, `trap_dispatch`, and
  `enter_user`.
- `src/syscall.kp`: the syscall numbers, `write` and `exit`, and the copy path
  that never touches a user address directly.
- `src/user.kp`: two hand-written user programs, each a naked function in its
  own `.user1` / `.user2` section, copied into its program's code page at boot.
- `src/vm.kp`: `plan/address-spaces.md`'s walker plus `user_page_pa`, the walk
  that demands the U and R bits and is the only way user memory is read.
- `kernel.ld`: the identity-mapped span, this hart's trap stack, the enlarged
  frame region and the two user-program sections.

## The trap frame

```kelp
repr(c) datac TrapFrame(
    val x1: u64, ... val x31: u64,
    val sepc: u64, val sstatus: u64)
```

`x1`..`x31` at offsets 0 to 240, `sepc` at 248, `sstatus` at 256, so the frame is
264 bytes and 8-byte aligned. Only `x2` is special: the hardware leaves `sp` in
the register, not the frame, so the entry recovers the interrupted `sp` from
`sscratch` (below) and stores it in the `x2` slot, and the resume path puts it
back through `sscratch` as well.

The layout is a convention the compiler cannot see: the naked entry hardcodes
the numbers in its asm, and both ends are pinned by tests. The boot checks
`size_of<TrapFrame>()`, `offset_of<TrapFrame>("x17")`, `("sepc")` and
`("sstatus")` against the same constants the asm uses and prints `user: frame ok`
only when they agree; `kelpc/tests/projects/trap_frame_layout` prints the six
numbers on both backends and the test pins them.

## The trap stack and its slot block

Each hart gets one trap stack from the linker script
(`__trap_stack_bottom`..`__trap_stack_top`, 4 KiB, in `.bss`). `sscratch` holds
its top while user code runs, so the entry's first instruction is the swap:

```
csrrw sp, sscratch, sp
```

which leaves `sp` at the trap stack top and puts the interrupted `sp` in
`sscratch`. The frame is then built 304 bytes below the top; the 40 bytes
between the frame's end and the top are the hart's slot block, and the entry
reads them at fixed offsets from the frame base:

| slot | offset | what it holds |
| --- | --- | --- |
| entry `sp` | 272 | the user stack pointer `enter_user` is about to hand the program |
| resume `ra` | 280 | where the kernel continues when a program ends |
| resume `sp` | 288 | the kernel stack pointer that goes with it |
| dispatch | 296 | the address of `trap_dispatch` |

The 264 bytes from offset 0 to 263 are the frame; 264 to 271 are padding that
keeps the frame base 16-byte aligned, which is the calling convention the `jalr`
to `trap_dispatch` has to satisfy.

Putting the dispatch address in a slot rather than in a register is not an
aesthetic choice: a `naked fun`'s single `asm` is emitted with no prologue, so
there is no instruction that could compute `Ptr.from_fn(trap_dispatch).addr()`
before it, and a naked asm with an operand does not assemble (the operand is
emitted as text). The slot is written once by `trap.install`, which is ordinary
Kelp code.

The entry calls `trap_dispatch` with `a0` = the frame address and looks at its
return value: nonzero means resume the program, zero means the kernel leaves.
The leave path loads the resume `sp`/`ra` from the slots and jumps, so the
kernel's `enter_user` frame is picked up exactly where it was left; the reply
to a program that just exited or was killed never has to `sret` back into it.

The dispatch address, the resume pair and the trap stack are one hart's; a
second hart would need its own copy of the block and its own `sscratch`, which
this stage does not build.

## Entering user mode

`enter_user(root, entry, sp)` is ordinary Kelp except for the last call:

1. `satp` gets `vm.satp_encode(root)` and one `sfence.vma`. The kernel image,
   its stacks and the frame region are identity-mapped in every program's root
   as non-user pages, so the kernel keeps executing across the switch and the
   trap entry can run under the program's table.
2. `sepc` gets `entry`, `sscratch` gets the trap stack top, and the program's
   `sp` goes into the slot block's entry slot.
3. `sstatus.SPP` is cleared and `sstatus.SPIE` set, so the `sret` drops to
   U-mode and a later trap leaves interrupts enabled on return.
4. `drop_to_user`, a `naked fun` whose whole body is one `asm`, stores its own
   `ra` and `sp` into the resume slots, loads `sp` from the entry slot and
   `sret`s.

`enter_user` therefore returns normally when the program exits or is killed,
which is what lets the boot run two programs in sequence.

## The syscall ABI

`ecall` from U-mode. The number is in `a7`, the arguments in `a0`..`a5`, the
result comes back in `a0`, and a negative result is an error. Registers are read
and written through the frame, and `sepc` is advanced by 4 (an `ecall` is never
the compressed form).

| number | name | arguments | returns |
| --- | --- | --- | --- |
| 0 | `write` | `a0` = fd (only 1), `a1` = pointer, `a2` = length | the length written |
| 1 | `exit` | `a0` = code | does not return |

Errors are the small negative errno values: `-9` (`EBADF`) for a file descriptor
other than 1, `-14` (`EFAULT`) for a pointer the kernel will not copy from, and
`-38` (`ENOSYS`) for a number the kernel does not know. `exit` is the one
syscall that does not return: the handler prints `user exited <code>` and the
dispatcher tells the entry to leave the kernel rather than resume the program.

## Untrusted pointers

`write` is the only way user memory crosses into the kernel, and it copies
through a kernel buffer. Before any byte moves:

- `len > 4096` is refused, so one call can never ask for more than the kernel's
  copy buffer (one frame).
- `ptr > USER_LIMIT - len`, with `USER_LIMIT = 2^38` (one past the highest
  canonical Sv39 low address), refuses a range that would wrap `ptr + len` and
  refuses every pointer at or above the user limit. The arithmetic is written so
  the addition itself cannot overflow, because the compiler's own overflow check
  would panic the kernel instead of returning an error.
- The copy walks the program's page table one page at a time with
  `vm.user_page_pa`, a walk that answers only when the leaf has **both** `PTE_U`
  and `PTE_R`. A kernel address that happens to be mapped (the identity map is in
  every program's table) is not user-readable, so it is refused here and not by
  a lucky address range.
- Each page's bytes are read from the **physical** address the walk returned,
  through the identity map, into the buffer, and the buffer is what the UART
  driver writes. The user virtual address is never dereferenced, so a page the
  walk rejects cannot be reached by a second access that races a remap.

A refusal prints the demo's own line (`user: efault ok`) and returns `-14`; the
kernel keeps running and the program sees the error in `a0`.

## The two programs

Both are naked functions whose section the linker script places outside
`[__bss_start, __bss_end)` and inside the identity-mapped span. At boot the
kernel copies the section's bytes into a fresh frame (the program's code page,
mapped `U|R|X` at `0x10000`) and maps a second fresh frame as the stack
(`U|R|W` at `0x20000`, `sp` = `0x21000`). The copy is byte for byte and the
programs' own references are pc-relative (`la` against labels in the same
section), so the relocated bytes still address their own data; the page is not
writable, so the first program copies its marker string onto its stack before it
patches the digit into it.

Program one calls `write(1, "hello from user\n", 16)`, then
`write(1, 0x80000000, 16)` (a mapped kernel page), then builds
`bad write -> X` on its stack, replaces `X` with `55 + (-a0)` (the hex digit of
the errno magnitude, so `-14` prints `E`), writes it, and calls `exit(7)`.
Program two loads from `0x40000000`, which no leaf maps in its address space:
the load page fault is `scause` 13, the handler prints `user killed: scause 13`
and leaves, and the kernel prints `kernel survived`.

## What is deliberately not here

- **A scheduler.** One process at a time, one hart, no run queue and no context
  switch between two live programs: program two starts only after program one has
  ended.
- **Preemption.** No timer interrupt is armed, so a program that never makes a
  syscall never gives the kernel back control, and a spinning program is not
  killed.
- **`fork`, `exec`, `wait` and process teardown.** Pages are never freed; a
  killed program's frames stay allocated, and no address space is ever reused.
  There is no parent/child relation and nothing to wait on.
- **ASIDs and TLB shootdown.** `satp` is written with ASID 0 and each switch is
  followed by a full `sfence.vma`.
- **Copy-on-write, demand paging, guard pages, and a user address-space
  abstraction.** The two programs' roots are built by hand at boot.
- **An interrupt path.** `trap_entry` assumes the trap came from U-mode, which is
  the only source the kernel installs: it swaps `sscratch` unconditionally and
  there is no `medeleg` bit for an S-mode trap it means to handle. A kernel-mode
  fault would swap the stack pointer out from under the running handler and is
  the one way to wedge this image.
- **Signals, `sret` to a kernel thread, and more than six syscall arguments.**

## How it is verified

`examples/kernel0-user/run.sh` boots the image under
`timeout 20 qemu-system-riscv64 -machine virt -bios none -nographic -no-reboot
-serial mon:stdio -kernel <elf>` and pins six lines: `user: frame ok`,
`hello from user`, `user: efault ok`, `bad write -> E`, `user exited 7`,
`user killed: scause 13` and `kernel survived`. The lines come from QEMU's own
stdout, not from the project's intent.
`cargo test --test compile_and_run -- kernel0_user` runs the boot and the
`llvm-nm -u` clean check on the image, and
`cargo test --test compile_and_run -- trap_frame` runs the hosted layout project
on both backends. The toolchain is `dev-kernel.nix`'s.
