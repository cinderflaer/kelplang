# `std/sync` beyond `Mutex`: `RwLock`, `Condvar`, channels and a thread pool

*Design note. `plan/mutex.md` is the shape this follows: a compiler builtin type in
the `Atomic<T>`/`Thread<T>`/`Mutex<T>` family, one opaque pointer to a runtime record,
scoped blocks for the borrows that cannot escape, and the same runtime on both
backends. This note records the surface, the two backends' split, and what each part
costs; `plan/kernel-ub-notes.md` carries the undefined behaviour this stage adds.*

## The shape all four share

Every type here owns a **runtime record** with no Kelp-visible layout: the C backend
calls `kelp_*.c` helpers directly and the LLVM backend declares the same helpers in
`declare_runtime`, so no backend ever names a field. The guarded value is reached only
through a **scoped block** (`plan/scoped-blocks.md`), whose borrow cannot be stored or
returned. Every record is **hosted only**: the freestanding manifest refuses each by
name, because the runtime half is a pthread object.

`RwLock<T>`, `Condvar`, `Sender<T>` and `Receiver<T>` are all **new builtin types**
(`Ty::RwLock`, `Ty::Condvar`, `Ty::Sender`, `Ty::Receiver`) rather than pure-Kelp
`datac`s. The reason is sharing: a channel's two ends and a lock's many users all reach
one heap record from several threads, and v1 has no `Arc`/`Rc` to spell that. A builtin
record with the runtime doing the reference counting is the same answer `Thread<T>` and
`Mutex<T>` already took.

## 1. `RwLock<T>` — `std/sync`

```
fun RwLock.new(value: move T).R: RwLock<T>
fun RwLock<T>.read(block: scoped fun(&T).R: R).R: R
fun RwLock<T>.write(block: scoped fun(mut T).R: R).R: R
fun RwLock<T>.try_read(block: scoped fun(&T).R: R).R: Option<R>
fun RwLock<T>.try_write(block: scoped fun(mut T).R: R).R: Option<R>
fun RwLock<T>.into_inner().R: T
```

`read` hands the block a `&T`; `write` hands it `mut T`. Many readers run at once, one
writer runs alone, and a waiting writer **blocks new readers** so a stream of readers
cannot starve it. The runtime (`kelp_RwLock` in `kelp_rt.c`) is a mutex plus two
condition variables and three counters (`readers`, `writer`, `writers_waiting`); it is
writer-preferring on purpose rather than `pthread_rwlock_t`'s unspecified policy.

A panic inside `write` **poisons** the lock (the runtime's cleanup handler releases the
write and sets the flag; a later `read`/`write` panics `"rwlock poisoned"`). A panic
inside `read` releases the read and does **not** poison: a reader leaves the value
untouched, so there is nothing to be suspicious of.

`try_read`/`try_write` are the shape that fits scoped blocks: the block runs only when
the lock was acquired, and the call's value is `Option<R>` (`None` when the lock was
busy and the block did not run). A busy `try_read` also fails when writers are waiting,
so the writer-preference rule holds. `try_*` never poison.

`T` is `is_rwlock_element`, the same `Send`-shaped predicate `Mutex<T>` uses. `RwLock<T>`
is `Sync` exactly as `Mutex<T>` is: `reachable_mut_path` answers `false` for it.

## 2. `Condvar` — `std/sync`

```
fun Condvar.new().R: Condvar
fun Condvar.notify_one()
fun Condvar.notify_all()
fun Mutex<T>.wait_while(cv: &Condvar, pred: fun(&T).R: bool, block: scoped fun(mut T).R: R).R: R
fun Mutex<T>.wait_while_timeout(cv: &Condvar, pred: fun(&T).R: bool, millis: i64, block: scoped fun(mut T).R: R).R: Option<R>
```

`cv.wait(...)` needs the mutex's own lock to be released atomically around the wait,
which a scoped guard cannot express, so the decided shape puts the wait on the mutex:
`m.wait_while(cv, pred) { v -> ... }` waits until `pred(&v)` is false, re-testing after
every wakeup (spurious wakeups are the library's problem, not the caller's), and then
runs the block with the lock held. There is no bare `wait`.

`wait_while_timeout` waits until the predicate goes false or `millis` elapses. It shares
`try_*`'s `Option<R>` shape: `None` means the deadline passed with the predicate still
true and the block did not run, `Some(v)` is the block's value.

## 3. Channels — `std/sync`

```
fun chan.bounded<T>(cap: i64).R: (Sender<T>, Receiver<T>)
fun chan.unbounded<T>().R: (Sender<T>, Receiver<T>)
fun Sender<T>.send(value: move T).R: Result<(), SendErr<T>>
fun Sender<T>.clone().R: Sender<T>
fun Receiver<T>.recv().R: Result<T, RecvErr>
fun Receiver<T>.try_recv().R: Option<T>
fun Receiver<T>.recv_timeout(millis: i64).R: Result<T, RecvTimeout>
```

The two ends are the same runtime record (`kelp_Channel`): one mutex, two condition
variables kind (not-empty, not-full), a ring buffer, the sender count, and a
`receiver_alive` flag, with the record's own reference count. `send` returns the value
back in `SendErr<T>` when the receiver is gone. `Sender.clone` bumps the sender count;
`Drop` of the last sender closes the buffer and wakes every receiver, and `Drop` of the
receiver marks it gone and wakes every blocked sender. The `Drop` is the user-`Drop`
feature (`plan/drop.md`), declared on the builtin's drop path, and the runtime does the
work.

`Receiver` is iterable: `for v in rx` yields `T` until every sender is gone. This needs
a builtin `Iterator` implementation, which is the one piece this stage does not finish;
see `plan/st1-status.md`.

## 4. Thread pool — `std/thread`

```
fun pool.new(workers: i32).R: Pool
fun Pool.spawn(job: move closure().R: unit)
fun Pool.join()
fun Pool.panicked().R: i32
```

`Pool` owns a runtime record: a mutex, a condition variable, a FIFO of type-erased jobs
(each a `(body, code, env, drop)` tuple, the same shape a `thread.spawn` worker record
already carries), a worker count and a panic counter. `spawn` enqueues and wakes one
worker; `join` waits for the queue to drain, sets the stop flag, wakes everyone and
waits for the workers to end; `Drop` joins. A panicking job runs the panic path inside
the worker; the runtime catches it with a thread-local `jmp_buf`, records it and lets
the **same worker** run the next job, which is what "does not kill the worker" means.
`join` is `Drop`'s body, so a pool that goes out of scope stops its workers.

`Pool` is a builtin for the same sharing reason as the channel. It is a **later slice**
of this stage: `plan/st1-status.md` says what is missing if it is not finished.

## Hosted only, and the two backends

The four records are pthread-backed, so `freestanding.rs` refuses each kind by name with
a message naming the future alternative (a spin lock over an `Atomic<T>`, a kernel
wait queue). The C backend and the LLVM backend call the same `kelp_rt.c` functions, so
the observable behaviour is identical; every run-test passes on both, and the fixtures
live in `tests/fixtures/` (bounded, terminating) with a freestanding refusal project per
kind.

## What is deliberately out of scope

- A blocking `Condvar.wait` that hands back a guard; the mutex method is the shape.
- A `select`/`try_recv`-with-deadline over several channels.
- A fair queue or priority in the pool; FIFO only.
- More than one ordering: everything here is `SeqCst` by construction.
