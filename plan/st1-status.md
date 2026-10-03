# st1 status: what is finished and what is not

`plan/sync.md` is the decided design. This note says exactly how far it is
implemented and where the built shape differs from it.

## Finished, on both backends, with tests

- **`RwLock<T>`** (`std/sync`): `read`, `write`, `try_read`, `try_write` and
  `into_inner`. Many readers or one writer, writer-preferring, poison on a panic
  inside `write`, no poison on a panic inside `read`. The runtime is
  `kelp_RwLock` in `kelpc/runtime/kelp_rt.c`; the pointer is `Ty::RwLock`. `try_*`
  returns `Option<R>`.
- **`Condvar`** (`std/sync`): `Condvar.new`, `notify_one`, `notify_all`, and
  `Mutex.wait_while` / `wait_while_timeout`. Tests: `rwlock_*`, `condvar_*`.
- **Channels** (`std/sync`, `kelpc/std/sync.kp`): `sync.bounded(cap)`,
  `sync.unbounded()`, `Sender.send`, `Sender.clone`, `Receiver.recv`, `try_recv`,
  `recv_timeout`, closing when every sender or the receiver drops, and
  `for v in rx`. Tests: `chan_bounded_mpsc` (4 producers, 1 consumer, bounded,
  sum 124500), `chan_close`, `chan_recv_timeout`, `chan_zero_capacity`,
  `chan_non_sendable`, `chan_send_a_lock`, and the project
  `freestanding_channel`.
- **Thread pool** (`std/pool`, `kelpc/std/pool.kp`): `pool.new(workers)`,
  `Pool.spawn`, `Pool.join`, `Pool.panicked`, `Drop` joins. Tests: `pool_sum`
  (1000 jobs, sum 499500), `pool_panics` (panicking jobs counted, worker
  survives), `pool_drop_joins` (drop joins, idle pool, double join, results home
  over a channel), `pool_spawn_after_join`, `pool_zero_workers`, and the project
  `freestanding_pool`.
- **Mutex additions** shared by both: `Mutex.share()`, `Condvar.share()` and
  `Mutex.swap(value)` (`mutex_share_swap`).

Every program above passes on the default LLVM backend and under
`KELPC_ORACLE=c`.

## Where the built shape differs from the plan

- **No new builtin `Sender`/`Receiver`/`Pool` types.** A channel is pure Kelp over
  two additions to the existing builtins. `Mutex.share()`/`Condvar.share()` give a
  second owned handle to the same runtime record (a reference count was added to
  `kelp_Mutex` and `kelp_Condvar`; `into_inner` on a record that is still shared
  panics). `Mutex.swap(value)` moves an owned value in and the old one out in one
  locked step; it exists because a scoped block captures an owned value by borrow
  only, so `send` stages the value in a private one-element mutex and the block
  swaps it across.
- **`send` returns `Option<T>`** (`None` = queued, `Some(value)` = the receiver is
  gone and here is the value), not `Result<(), SendErr<T>>`: `Ok(())` cannot be
  written because no expression produces `unit`. `recv` returns
  `Result<T, RecvErr>` (`RecvErr.Closed`), `recv_timeout` returns
  `Result<T, RecvTimeout>` (`Timeout`/`Closed`), `try_recv` returns `Option<T>`.
- **The pair is a tuple**: tuples already worked end to end, so
  `var (tx, rx) = sync.bounded<i64>(4)` (`var` because a `for` over `rx` needs a
  mutable place). The functions are `sync.bounded` and `sync.unbounded`.
- **Closing uses two non-generic `Drop` types** (`SendEnd`, `RecvEnd`). `Drop` on a
  generic type is refused by name (`'T.drop' cannot have type parameters`), so
  the destructor sits on the part of each end that has no type parameter.
- **`for v in rx`**: a trait impl cannot name a generic instantiation
  (`impl Iterator for Receiver<T>` is a parse refusal), so `implements_iterator`
  in `infer.rs` now also accepts a generic `datac` whose template declares
  `fun mut X<T>.next().R: option.Option<..>`; the desugar is an ordinary `next()`
  call, so a wrong shape is that call's type error.
- **The pool's panic survival does not use a thread-local `jmp_buf`.** A
  `longjmp` out of a job would skip the `pthread_cleanup` handlers a
  `Mutex.lock` block pushed, leaving the mutex locked and unpoisoned. Instead each
  worker runs every job on a thread of its own and joins it; an `Err` join is the
  panic, it is counted, and the worker takes the next job. The cost is one thread
  creation per job and that `thread.current_id()` differs between jobs on one
  worker. `kelp_panic` is unchanged.
- **The pool is module `std/pool`** (`pool.new(n)`), not part of `std/thread`: a
  module cannot call its own builtin `thread.spawn` by name, and the task wrote
  `pool.new`. The queue holds `Job(run: closure())` because a bare closure cannot
  be a `List` element.
- **Hosted only** is by module: `std/sync` and `std/pool` are in the freestanding
  admission default (hosted), so the refusal names the module, not each type. The
  existing `Mutex`/`RwLock`/`Condvar` refusals by type are unchanged.

## Surprises worth knowing

- **A partial move out of a type that merely contains a destructor type is not
  refused.** `borrowck.rs` refuses moving a destructor-typed field or tuple element,
  but not one whose type contains one (`Sender<T>` contains `SendEnd`). The base's
  drop then runs the user destructor on a zeroed value. The channel ends defend
  against it (`SendEnd.live`/`RecvEnd.live` are `false` in a zeroed value, and
  `drop` returns at once), which is what lets a tuple pair be destructured. The
  general check is not made transitive here.
- A generic function cannot be a function value, so the wait predicates are `fun`
  literals inside the generic method; a closure cannot be a `List` element; a
  `Thread<T>` cannot be a `List` element (the pool detaches its workers and counts
  them with `alive`/`done` instead of keeping handles).

## Not weakened

No check was relaxed and no test was deleted. `plan/kernel-ub-notes.md` carries the
stage's undefined-behaviour note, and `plan/user-ffi-gap-analysis.md` records that no
FFI gap is closed by it.
