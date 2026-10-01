# The compiler's memory floor

*Added 2026-10-01 after four out-of-memory kills on a 7.8 GB host. `kelpc/src/guard.rs`.*

`kelpc build` and `kelpc run` end themselves when the machine is nearly out of memory, with a message,
instead of leaving the kernel's OOM killer to choose a victim (on a small host the victim is often not the
compiler, and can be a whole login session).

- **How.** A watchdog thread reads `MemAvailable` from `/proc/meminfo` every 200 ms. Below the floor it
  kills the `cc`/`ld.lld`/`pkg-config` children the driver spawned (registered by `guarded_output`; they are
  the largest consumers and would outlive `kelpc`), prints one `error:` line and leaves with `_exit(75)`
  (`EX_TEMPFAIL`: the same input can succeed once memory is free). `_exit` and not `std::process::exit`,
  because the main thread is still inside LLVM and running exit handlers under it corrupts the heap.
- **The floor.** `KELPC_MIN_AVAILABLE_MB`, default 384; `0` turns the guard off; an unparsable value is
  the default.
- **What it does not police.** `kelpc run` stops the guard before it starts the compiled program, so a
  program's own memory use is never limited by it. `fmt`, `lint`, `check` and `version` do not start it.
  Only Linux is covered; with no `/proc/meminfo` the guard does nothing. Disk space is not checked.
- **Tests.** Unit tests for the `/proc/meminfo` and floor parsing (`guard.rs`); `tests/memory_guard.rs`
  builds with a floor no machine meets and expects status 75 and the message, and with `0` expects success.
