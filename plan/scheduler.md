# A timer interrupt and a preemptive round-robin scheduler (kernel stage 21)

*Design note for `plan/kernel-path.md`'s next stage after userspace and capabilities. It is the
process-model half of row 9 that `plan/syscall-abi.md` deliberately left out: a machine timer, a
round-robin scheduler and preemption, still in M-mode with no paging. The undefined-behaviour
catalogue is `plan/kernel-ub-notes.md`'s stage 21 section.*

## What this adds

`examples/kernel0-sched` is a freestanding riscv64 `virt` image that:

- programs the CLINT's machine timer (`mtimecmp` at `0x02004000 + 8 * hart`, `mtime` at
  `0x0200BFF8`, 10 MHz) and enables `mie.MTIE` and `mstatus.MIE`; `coral.toml`'s `[caps]`
  grants exactly those two windows plus the NS16550's, and delegates each to the one module
  that builds a pointer from it;
- holds a `static mut [Task; 3]` table, where `Task` is a saved register file, the scheduler
  address the entry calls, a state, an id and a 4096-byte stack (the stack is inside the
  `datac`, so the whole table is one object in `.bss`);
- handles the timer in a naked `trap_entry` that saves all 31 general registers, `mepc` and
  `mstatus` into the *current task's* frame, calls `schedule(frame: Ptr<TrapFrame>)`, and
  restores whichever frame the scheduler returns, ending in `mret`;
- round-robins the next READY task on every timer tick (10000 `mtime` ticks, about a
  millisecond) and treats an `ecall` from a task as a yield through the same call;
- runs three tasks that never yield, each looping forever over its own counter and printing
  `A`/`B`/`C` plus the counter every 50000 iterations through a SPIN LOCK-protected UART
  write, so the only way their lines interleave is preemption;
- stops itself after 40 ticks and prints `sched: done ticks=40`, so the boot is bounded by
  ticks rather than by wall clock.

`examples/kernel0-sched/run.sh` boots it under `timeout 20` and asserts, against QEMU's own
stdout, that all three letters appear, that the letter changes at least three times, that no
line is torn, and that the stop line is whole. `kelpc/tests/compile_and_run.rs` runs that
script (skipped without `ld.lld` and `qemu-system-riscv64`).

## The compiler change it forced

One gap, and it is the table: a static could be a `datac`, an integer/`bool`/`char` array or
an `Atomic<T>`, but `[Task; 3]` was refused by name because an array's element had to be a
scalar. `infer.rs::check_static_type_inner` now accepts an array whose element is a `datac` or
another array, recursively, so the element obeys the same closed rule a `datac`-typed static's
fields already obey (a heap type, a borrow, a `View`, a function value, a `Ptr` or a handle is
still refused by the recursion). Both backends already emitted a static initializer for
`SI::Array` of `SI::Struct` and a nested array, so the change is the type rule alone. The
initializer stays a constant expression, and `static_globals`-style tests plus the new
`tests/projects/static_datac_array` pin it on both backends. The layout the naked entry
hardcodes is pinned separately by `tests/projects/sched_task_layout`.

## How a switch works

`mscratch` holds the address of the running task's frame, so the entry can write the register
file into the task table itself and needs no stack of its own. `t0` is swapped for the frame
address and then recovered from `mscratch` to be saved with the rest of the file; the word just
past `mstatus` in each frame is the address of `schedule`, which is how a naked function reaches
a Kelp function without spelling its mangled symbol (the idiom kernel stage 11 used for its
dispatcher slot). `schedule` reads `mcause`: the timer re-arms `mtimecmp`, counts the tick and
may stop the machine; an `ecall` advances the saved `mepc` past the instruction; anything else
prints the cause and stops. It then picks the next READY task, returns that task's frame, and
the entry makes it the new `mscratch`, restores the CSRs and the file, and `mret`s. The first
task is started by the boot's own `ecall` with a frame of its own, so there is one code path for
"switch to a task" and one for "switch back".

The console lock is an `Atomic<bool>` released by `defer`, and every lock acquisition clears
`mstatus.MIE` first. On a single hart that is what makes a line non-preemptible: a task cannot be
interrupted with the lock held, so the scheduler never runs mid-line and can never spin for a
lock an interrupted task is holding, and two tasks cannot interleave inside one line.

## What is deliberately not here

- Priorities, blocking, sleep/wake, a run queue with anything but "next READY", task exit and
  reaping. The state field exists so those have somewhere to go.
- SMP. The image runs on hart 0 (the compiler's `_start` stub parks the others), the timer
  compare register is hardcoded for hart 0, and the lock's interrupt masking is a
  single-hart argument that does not survive a second hart.
- Preemption of user mode. Stage 21 runs its tasks in machine mode; the U-mode process of
  `plan/syscall-abi.md` is still one program at a time with no timer.
- Anything about the timer's accuracy, drift or a wall-clock notion: `mtimecmp` is re-armed
  from a fresh `now`, so a delayed tick is skipped rather than made up, and the run is counted
  in ticks, never in time.
