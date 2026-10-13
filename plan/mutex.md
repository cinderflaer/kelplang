# `Mutex<T>` in `std/sync` (B-5's remainder)

*Design note. `plan/mk9-memory-model.md` is the spec for the memory model and threads; this is
the mutex the last line of its B-5 entry left open ("`Mutex` itself is what remains").
`language/functions.md` (kelp wiki) is the mechanism the surface is built on.*

## The surface

```
fun Mutex.new(value: move T).return: Mutex<T>
fun Mutex<T>.lock(block: scoped fun(mut T).return: R).return: R
fun Mutex<T>.into_inner().return: T
```

`Mutex<T>` is a compiler builtin generic in the shape `Atomic<T>`/`Thread<T>` have: one
monomorphized type per element, not `Copy`, with a drop. `T` is any owned, borrow-free value
(`is_mutex_element`), so `Mutex<i64>`, `Mutex<String>` and `Mutex<List<i32>>` all work; `unit`, a
function value and a borrow are refused by name.

**The value is reached only through a scoped block.** `m.lock { v -> ... }` passes `v: mut T` to a
`scoped fun(mut T).return: R`, so the mutable borrow is the existing non-escaping one: it cannot be
stored in a `datac`, returned, put in a container, or captured by an ordinary closure, because
those are all ordinary type mismatches (`language/functions.md` (kelp wiki)). The block's tail is the call's
value. `into_inner` is the only other way out and it consumes the whole mutex.

**The block's return type is inferred from the block.** `R` is not known before the block is
checked (`m.lock { v -> *v }` has no written type), so the literal is checked with the
`infer_scoped_ret` flag: its parameter types come from the expected `scoped fun(mut T).return: R`, and
its return is the body's tail type. When the call site names a type (`return m.lock { v -> ... }`
in a function returning `Result<_, _>`), that type is used instead, which is what lets a `?`
inside the block propagate the block's declared return.

## Locking, release and poisoning

The runtime half is a pthread mutex in `kelp_rt.c`; both backends call the same functions, which is
the same sharing `thread.spawn`'s runtime half has. `lock` lowers to `kelp_mutex_run(record, body,
code, env, out)`, which acquires the lock, checks the poison flag, calls the generated block body
with the `&mut T`, and releases the lock on the normal way out. A `return` or `?` inside the block
returns **from the block literal**, so the runtime call still returns and the unlock still runs; a
`break` does not exist in the language.

A panic inside the block is the one exit the ordinary call cannot see. The block body calls
`kelp_panic`, which in a worker ends the thread with `pthread_exit`. `kelp_mutex_run` registers a
`pthread_cleanup_push` handler before the body runs, so `pthread_exit` runs it: the lock is
released and the mutex is marked **POISONED**. A later `lock` panics with `mutex poisoned`. There
is no unwinding (MK9 section 2), so the panicking frame's owned values are still abandoned; the
mutex itself is not, which is the whole point of the cleanup handler.

## The `Sync` rule

A `&Mutex<T>` is a shared borrow whose pointee has no reachable `&mut` path: the only way to reach
`T` is the lock, which the runtime serializes. The `Sync`-shaped crossing rule therefore accepts it
for the same reason it accepts `&Atomic<T>`, and it is the one place the rule changed:
`reachable_mut_path` answers `false` for `Ty::Mutex(_)`. That is true whatever `T` is, including a
`datac` with a `var` field, because the field is unreachable without the lock. The element predicate
(`is_mutex_element`, the `Send`-shaped `!is_borrowed_field_ty`) keeps a borrow out of the slot in
the first place.

## Hosted only

The runtime half is libc's pthread mutex, so a freestanding manifest refuses `Mutex<T>` with a
message that names the future alternative: a spin lock over an `Atomic<T>`. The C backend is the
oracle for the hosted paths; freestanding is LLVM-only by design, and neither backend can lower a
mutex without the hosted runtime.

## What is deliberately out of scope

- Try-lock, timed lock and an ordering slot: v1's surface is `lock` and `into_inner`.
- A `lock` that returns a guard value: the scoped block is the whole reason the borrow cannot
  escape, and a returned guard would reintroduce the escape.
- Reader/writer locks, condition variables, and cross-process semaphores.
- A spin-lock `Mutex` for freestanding (named in the refusal, not built).
