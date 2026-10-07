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

## 1. `RwLock<T>` (`std/sync`)

```
fun RwLock.new(value: move T).return: RwLock<T>
fun RwLock<T>.read(block: scoped fun(&T).return: R).return: R
fun RwLock<T>.write(block: scoped fun(mut T).return: R).return: R
fun RwLock<T>.try_read(block: scoped fun(&T).return: R).return: Option<R>
fun RwLock<T>.try_write(block: scoped fun(mut T).return: R).return: Option<R>
fun RwLock<T>.into_inner().return: T
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

## 2. `Condvar` (`std/sync`)

```
fun Condvar.new().return: Condvar
fun Condvar.notify_one()
fun Condvar.notify_all()
fun Mutex<T>.wait_while(cv: &Condvar, pred: fun(&T).return: bool, block: scoped fun(mut T).return: R).return: R
fun Mutex<T>.wait_while_timeout(cv: &Condvar, pred: fun(&T).return: bool, millis: i64, block: scoped fun(mut T).return: R).return: Option<R>
```

`cv.wait(...)` needs the mutex's own lock to be released atomically around the wait,
which a scoped guard cannot express, so the decided shape puts the wait on the mutex:
`m.wait_while(cv, pred) { v -> ... }` waits until `pred(&v)` is false, re-testing after
every wakeup (spurious wakeups are the library's problem, not the caller's), and then
runs the block with the lock held. There is no bare `wait`.

`wait_while_timeout` waits until the predicate goes false or `millis` elapses. It shares
`try_*`'s `Option<R>` shape: `None` means the deadline passed with the predicate still
true and the block did not run, `Some(v)` is the block's value.

## 3. Channels (`std/sync`)

**Implemented, in pure Kelp. `plan/st1-status.md` records how the built shape
differs from the first design (no new builtin type).**

```
fun sync.bounded<T>(cap: usize).return: (Sender<T>, Receiver<T>)
fun sync.unbounded<T>().return: (Sender<T>, Receiver<T>)
fun Sender<T>.send(value: move T).return: option.Option<T>
fun Sender<T>.clone().return: Sender<T>
fun Receiver<T>.recv().return: result.Result<T, RecvErr>
fun Receiver<T>.try_recv().return: option.Option<T>
fun Receiver<T>.recv_timeout(millis: i64).return: result.Result<T, RecvTimeout>
for v in rx { ... }
```

Two tiny builtin operations carry everything: `Mutex.share()`/`Condvar.share()`
(another owned handle to the same runtime record; the record has a reference count
and the last handle's drop frees it) and `Mutex.swap(value)` (exchange the guarded
value in one locked step). With them a channel is ordinary Kelp: a `ChanMeta`
mutex (queue length, sender count, receiver flag, capacity) with two condition
variables is the lock every wait uses, and the queue is a `Mutex<List<T>>` only
touched while the meta mutex is held. `send` returns `None` when the value was
queued and `Some(value)` when the receiver is gone, so the value comes back and
the result is must-use. Closing is the `Drop` of two non-generic ends
(`SendEnd`, `RecvEnd`) that each handle owns, because a destructor cannot be
declared on a generic type. `for v in rx` is the compiler's `for` desugar over a
`next` method declared on the generic `Receiver<T>` template.

## 4. Thread pool (`std/pool`)

**Implemented, in pure Kelp.**

```
fun pool.new(workers: i32).return: Pool
fun Pool.spawn(job: move closure())
fun Pool.join()
fun Pool.panicked().return: i32
```

`Pool` holds shared handles to a state mutex, a `Mutex<List<Job>>` FIFO and two
condition variables. `new` starts the workers with `thread.spawn`; `spawn` queues a
job and wakes one; `join` sets the stop flag, wakes everyone, and waits until every
worker has drained the queue and exited; `Drop` calls `join`. A panicking job is
counted in `panicked()` and does not kill its worker: the worker runs each job on a
thread of its own and joins it (see `plan/st1-status.md` for why that replaces the
thread-local `jmp_buf`). The module is `std/pool` rather than a part of `std/thread`
because a module cannot call its own builtins (`thread.spawn`) by name.

## Hosted only, and the two backends

`RwLock`, `Condvar` and `Mutex` are pthread-backed, so `freestanding.rs` refuses each kind by name with
a message naming the future alternative (a spin lock over an `Atomic<T>`, a kernel
wait queue); `std/sync` and `std/pool` are hosted modules, refused by module name. The C backend and the LLVM backend call the same `kelp_rt.c` functions, so
the observable behaviour is identical; every run-test passes on both, and the fixtures
live in `tests/fixtures/` (bounded, terminating) with a freestanding refusal project per
kind.

## What is deliberately out of scope

- A blocking `Condvar.wait` that hands back a guard; the mutex method is the shape.
- A `select`/`try_recv`-with-deadline over several channels.
- A fair queue or priority in the pool; FIFO only.
- More than one ordering: everything here is `SeqCst` by construction.
