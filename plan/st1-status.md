# st1 status: what is finished and what is not

`plan/sync.md` is the decided design. This note says exactly how far it is
implemented, so a later pass starts from the truth rather than from the design.

## Finished, on both backends, with tests

- **`RwLock<T>`** (`std/sync`): `read`, `write`, `try_read`, `try_write` and
  `into_inner`. Many readers or one writer, writer-preferring, poison on a panic
  inside `write`, no poison on a panic inside `read`. The runtime is
  `kelp_RwLock` in `kelpc/runtime/kelp_rt.c`; the pointer is `Ty::RwLock`. The
  scoped blocks reuse the mutex machinery. `try_*` returns `Option<R>` and shares
  the block-not-run shape with `Mutex.wait_while_timeout` (`Option` construction
  is `codegen_c.rs::option_literal` and `codegen_llvm.rs::emit_option_from_run`).
- **`Condvar`** (`std/sync`): `Condvar.new`, `notify_one`, `notify_all`, and
  `Mutex.wait_while` / `wait_while_timeout`. The predicate is a checked Kelp
  function value; the runtime re-tests it after every wakeup. The runtime is
  `kelp_Condvar` plus `kelp_mutex_wait_while{,_timeout}`, which use the mutex's
  own cleanup handler so a panic in the predicate or block poisons and releases
  the mutex. `Condvar` is `Ty::Condvar`; `&Condvar` has no const in C for the
  same reason `&Mutex` has none.
- **Freestanding refusals** by name for `RwLock` and `Condvar`
  (`tests/projects/freestanding_rwlock`, `freestanding_condvar`).
- **Tests** in `tests/fixtures/`: `rwlock_basic`, `rwlock_try`,
  `rwlock_concurrent` (two readers overlap, a queued writer goes first, both
  proved with atomics and no sleep), `rwlock_spawn_borrow`, `rwlock_poison`,
  `rwlock_read_releases`, `rwlock_read_borrow_returned`,
  `rwlock_read_borrow_stored`, `condvar_handshake`, `condvar_timeout`; all are
  registered in `kelpc/tests/compile_and_run.rs` and pass on the default LLVM
  backend and under `KELPC_ORACLE=c`.

One shared fix landed with this: a scoped block that captured a local used to
hand its environment to the runtime as a compound literal inside its own
statement expression, which C scoped too narrowly and left dangling. The
`Mutex.lock`, `RwLock.read`/`write`/`try_*` and `Mutex.wait_while` lowerings now
stage the block through `stage_argument`, which keeps the environment in the
call's own prelude. `rwlock_try_nested` (removed) reproduced the
stack-use-after-scope under AddressSanitizer and is how the fix was found.

## Not started

- **Channels** (`Sender<T>`, `Receiver<T>`, `chan.bounded`/`chan.unbounded`,
  `send`, `recv`, `try_recv`, `recv_timeout`, `Sender.clone`, `Drop`, and
  `for v in rx`). The runtime record and the two builtin types are the remaining
  work. Two pieces need a mechanism that does not exist yet:
  - `chan.bounded`/`chan.unbounded` return a pair, so the checker needs the
    destructuring-friendly tuple form (`val (tx, rx) = chan.bounded(4)`); tuple
    construction in both codegens is the first piece.
  - `Sender.send` returns `Result<(), SendErr<T>>` with the value back, and
    `for v in rx` needs a builtin iterator. `SendErr<T>` can be a shipped
    `datac` holding `T`; the iterator is the larger piece. A cheaper but
    behavior-equivalent shape is `Result<(), T>` unless the named `SendErr` is
    required.
- **Thread pool** (`pool.new`, `Pool.spawn`, `Pool.join`, `Pool.panicked`,
  `Drop`), which depends on the channel. Catching a job's panic so the same
  worker survives needs a thread-local `jmp_buf` checked by `kelp_panic` before
  it reaches `pthread_exit`; that is a change to the panic path itself, and it
  is the reason the pool is a later slice.

## Not weakened

No check was relaxed and no test was deleted. `plan/kernel-ub-notes.md` carries
the stage's undefined-behaviour note, and `plan/user-ffi-gap-analysis.md` records
that no FFI gap is closed by it.
