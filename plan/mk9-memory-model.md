# The memory model and threads: plan of record

*Status: **decided 2026-09-22; stages A1, A2, B1, C1, C2 and D1 implemented (A1 and A2 2026-09-22, B1 and C1 2026-09-24, C2 and D1 2026-09-25).** This is the spec step. The shared-borrow crossover in §1 is signed off by the owner (2026-09-22), so the document is unblocked; the four questions in §9 are deliberately left open, each decided when it comes up, and A2 has since decided the second one. Every stage in §6's sequence is now done; MK9-S3-M3 (thread-local and per-CPU storage) is deliberately outside that sequence (§6) and stays unopened.*

*Scope: MK9-S3-M1 (atomics, fences, ordering annotations, a `Send`/`Sync`-shaped discipline, published race semantics) and the parts of MK6-S1-M1 that depend on it (threads, the per-thread panic policy, join).*

*Relation to the other documents. The retired `mk7-llvm-backend.md` is archived in `meta/milestones.md` (kelp wiki)'s MK7 record; this is the plan of record for concurrency. `meta/roadmap.md` (kelp wiki) owns when, `architecture.md` owns what the language is, and the Backlog items this implements are `B-4` (atomics, ordering, a published memory model), `B-5` (threads) and, in part, `B-8` (a panic that takes the process) and `B-11` (thread-local storage, deferred).*

## 0. Where this starts from

What ships today, and why threads were held back:

- **Multi-process is the current parallelism.** `process.spawn` / `process.run` give real isolation and real concurrency in separate address spaces. That was MK5-S7-M1's honest answer for v1 and it stays.
- **One single-threaded event loop is the current concurrency.** `Socket.wait_readable` / `wait_writable` (poll readiness) plus `time.sleep_ms` serve many sockets from one thread.
- **There is no thread, no atomic, no `Mutex`, and no `unsafe`.** The borrow checker's central promise is that a `&mut T` is exclusive, and a second thread is exactly what would break that promise silently, which is why threads are gated behind this document rather than shipped beside it.
- **A panic takes the process.** `kelp_panic` is `_Noreturn`, prints `panic: <msg>` and calls `exit(1)` (`kelpc/runtime/kelp_rt.c:14`). In a multithreaded program that would let one worker's failure kill every other worker.
- **Open item 60 lists five prerequisites and two have shipped.** (a) function values and (b) `name: move T` landed with A-4 (MK8-S1-M2 phases A-F), so a thread entry point can be a declared `fun`, a `fun` literal, or a capturing closure carrying what it runs on. This document settles (c) the panic policy, (d) the per-thread frame and cleanup story, and (e) atomics plus the `Send`-shaped rule.
- **The plan already records the dependency.** "MK6-S1-M1 depends on MK9-S3-M1 (atomics, ordering, a published memory model)", `meta/roadmap.md` (kelp wiki) §Dependency notes: a thread milestone with no memory model and no atomics is not a milestone. That note offered two routes, move MK9-S3-M1 in front of MK6-S1-M1 or ship a documented minimal subset. This document takes the second route and says exactly what the subset is.

## 1. Decision one: race prevention is compile-time

**A data race is not writable, and the reason is the checker, not a runtime instrument and not a channel type.**

A race needs four things at once: one location, two threads, at least one write, and no ordering between them. The design removes them one at a time.

- **A `&mut T` is exclusive, and it does not cross a thread boundary at all.** A borrow's lifetime in v1 is lexical in the frame that made it (A-11's parked half is exactly the lifetime syntax that would tie it to another frame), so the checker cannot prove the sender stays out of the value while the thread runs. A `thread.spawn` argument is therefore an owned value, moved in.
- **A `&T` crosses in exactly one shape, proposed below**, because sharing a counter between threads has to be expressible or the atomics in §4 are unusable.
- **Everything else crosses by being moved.** A value moved to a thread is owned by that thread for as long as it holds it, which is the same rule that already governs a `move T` parameter.
- **A moved value may not contain a borrow**, and that clause is already enforced rather than newly written: `types::is_borrowed_field_ty` refuses a `&T`, `str` or `View` in a `datac` field, an enum payload, a `List` element or a `Map` value, so no such value can be constructed in the first place.

**Decided 2026-09-22, by owner sign-off:** a *shared* borrow crosses a thread boundary only when the place it borrows has **process lifetime**, which in v1 means a `pd`, and only when the borrowed type has **no reachable `&mut` path**. The first clause is checkable lexically, because a `pd` global's lifetime is the process and a frame local's is not. The second is the `Sync`-shaped rule: an `Atomic*` has no `&mut` path by construction, so a `&AtomicI64` may be shared across threads; a `datac` with a `var` field reachable from it may not, because two threads could otherwise write it through the same shared borrow. The reasoning is the one `Sync` uses: no unsynchronized mutation reachable from the shared borrow means the sharing is safe by construction. Without a crossover the atomics of §4 would have no second thread able to reach them, which defeats shipping them first.

**Rejected, with the reason recorded.**

- **Not a runtime race detector (ThreadSanitizer-shaped or a shadow-memory check).** It turns a race into a failure at run time when the language's promise is that the program does not compile, it costs a runtime that MK9-S4's freestanding target does not have, and it would make "correct" a property of the test run rather than of the source. A dynamic checker is still *wanted* as a testing instrument (MK11-S2-M1's Miri-equivalent), which is a different role: evidence about the checker, not the enforcement mechanism.
- **Not channels-only.** A channel is a value type a program may use, and a good one to have later, but it cannot be the mechanism: a channel-only model still lets two threads hold a `&mut T` to one location if the checker does not refuse it, and "communicate by moving" without a rule for what may move is not a discipline. The rule is what makes the race unwritable; a channel is one of the ways to obey it.
- **Not `Mutex`-only, for the same reason.** A lock is a library type that needs the same checker rule to be sound, so the rule comes first and the lock is a later convenience.

## 2. Decision two: a panic kills the thread, not the process

**A panic inside a spawned thread ends that thread, and the joiner receives an error. The process survives.**

- `thread.join(handle)` returns `Result<T, ThreadErr>`. `ThreadErr` carries the panic message; since the panic text already names the `.kp` line, the joiner can report where the worker died.
- Other threads keep running. The main thread decides what to do about the failure, which is the point: MK6-S2-M1's worker pool must not lose fifteen downloads because one URL 404s a panic.
- **The main thread keeps today's behaviour.** A panic in `main`, or in a thread nothing joins before the process ends, still ends the process with `exit(1)`. The rule is "a panic ends the thread it happened in", and the process ends when `main` ends.
- **No unwinding in v1, and the consequence is recorded rather than hidden.** Kelp has no exceptions and no `catch`, and a user `Drop` (`language/drop.md` (kelp wiki), MK9-S6-M1) runs at the ordinary drop points only, never on the panic path. So a panicking thread's frame is abandoned and the owned values in it are **not** dropped: a panic path leaks the frame. That is a deliberate trade against the two alternatives, both worse here: unwinding needs a `Drop` story the language does not have, and aborting the process is what this decision removes. MK9-S6-M1's `defer` and user `Drop` closed the ordinary exit paths and deliberately left the panic path as it was, and this document is where the remaining leak is admitted.
- **`_Noreturn` stays true.** The panic helpers terminate by calling `pthread_exit` (or the thread's equivalent) rather than returning, so the property the C backend relies on (`_Noreturn` is why a function whose tail is a `panic` needs no trailing `return`) is preserved.
- **This does not settle freestanding.** MK9-S6-M2 asks what a panic does with no OS underneath; there are no threads there and no `exit(1)` to call, and that stays a separate decision.

## 3. Decision three: `SeqCst` only, with the ordering slot reserved

**v1 ships one ordering, `SeqCst`. `Relaxed`, `Acquire` and `Release` are not in the surface syntax.**

*Rationale, as decided.* On x86-64 the `SeqCst` penalty is real but narrow: it costs extra only on stores (about 3.5 ns against 0.2 ns, an `XCHG` or `MFENCE` instead of a plain `MOV`), while loads and read-modify-write operations (`fetch_add`, `compare_exchange`) cost the same at any ordering. On ARM and RISC-V the gap would be larger, because even acquire and release need real barriers there. What the weaker orderings buy is lock-free data structures and spinlocks for kernel-grade code, which is not the v1 target.

**The API keeps the slot open.** Every operation that has an ordering takes it as an argument, even while exactly one value is legal:

```kelp
val n = counter.load(order = Ordering.SeqCst)          // v1: the only legal value
val m = counter.load(order = Ordering.Acquire)         // later: a new variant, the same call
```

rather than a hard-coded `load()` with nowhere to put an ordering. Adding a variant to `enum Ordering` is a widening, not a signature change: no v1 program breaks, and a program that wrote `SeqCst` keeps exactly the meaning it had. In v1 the enum has **exactly one variant, `SeqCst`**; what is reserved is the argument position, so naming an ordering that does not exist yet is the ordinary unknown-variant error rather than a special refusal. It also means a caller cannot be *wrong* about ordering in v1, because there is nothing to be wrong about, and the migration to a weaker ordering is always an explicit opt-in at a call site rather than a change of default.

`Ordering` is a **builtin `enum`**, not a constant, so the language's exhaustiveness check applies: the day a second variant exists, every `when` over an ordering is re-checked and the compiler names the missing arms instead of silently falling into a default.

**Semantics.** `SeqCst` means what C11, C++11 and LLVM `seq_cst` mean: a single total order over all sequentially consistent operations, with each operation ordered consistently with that total order and with program order, and with no data race between the atomic operations themselves. The published document that states this as the language's own definition is MK9-S3-M2; this section fixes the surface and names the semantics it borrows.

## 4. The atomics surface, v1

A builtin generic over a closed element set, in the shape the other builtins already use (`List<T>`, `Map<K, V>`, `Box<T>`, `[T; N]`):

```kelp
enum Ordering { SeqCst }

AtomicBool
AtomicU8  / AtomicU16 / AtomicU32 / AtomicU64 / AtomicUsize
AtomicI32 / AtomicI64 / AtomicIsize

fun Atomic<T>.new(value: T) => Atomic<T>
fun Atomic<T>.load(order: Ordering) => T
fun Atomic<T>.store(value: T, order: Ordering)
fun Atomic<T>.swap(value: T, order: Ordering) => T
fun Atomic<T>.compare_exchange(current: T, new: T, order: Ordering) => Result<T, T>

// the integer instantiations only; AtomicBool has no arithmetic
fun Atomic<T>.fetch_add(value: T, order: Ordering) => T
fun Atomic<T>.fetch_sub(value: T, order: Ordering) => T
fun Atomic<T>.fetch_and(value: T, order: Ordering) => T
fun Atomic<T>.fetch_or(value: T, order: Ordering) => T
fun Atomic<T>.fetch_xor(value: T, order: Ordering) => T

fun thread.fence(order: Ordering)
```

**Every receiver is shared, and that is the line §1 requires.** The `mut` spelling this section used to carry was wrong: an atomic mutates *through* a shared borrow, which is what leaves it **no `&mut` path by construction**, the property §1's `Sync`-shaped rule and B1 are built on, and the reason a `&Atomic<T>` rather than a `&mut Atomic<T>` is the thing that may cross a thread boundary. A `pd val` is the declaration a shared counter wants, because a `val` passdown hands a function a shared borrow, which is the borrow stage C1's thread argument needs and the one §5's second rule permits to cross. A `pd var` counter works too, including a store followed by a load in one function, because an atomic's operations are shared borrows reborrowed from the passdown's hidden reference; `var` costs the case where a function assigns to the passdown itself, which holds it exclusively, and an exclusive borrow is what §5 refuses to send to a thread.

**The `Atomic*` names are the element set, not a list of `datac`s.** `Atomic<T>` is a compiler builtin generic in the shape `List<T>`/`Box<T>`/`Map<K, V>`/`[T; N]` have, monomorphized per element type, not `Copy`, needing no drop; the names above are the instantiations it is spelled with (`AtomicI64` is `Atomic<i64>`). The element set is `bool` and v1's integer types, so there is no `AtomicI8`/`AtomicI16`; this table used to name them, and v1 has no `i8`/`i16` for them to mean.

Notes on the shape, each with its reason:

- **`compare_exchange` returns `Result<T, T>`** (`Ok` the previous value, `Err` the actual one) rather than a `bool` plus a second load. That matches the `Option`/`Result` vocabulary every fallible stdlib call now uses, and it removes the window a bool-then-load would have.
- **No `AtomicPtr`.** Raw pointers do not exist until MK9-S2-M1 (`unsafe`, B-3), so a pointer atomic would be a type with no way to make a value.
- **No float atomics.** There is no atomic hardware operation for them and no lock-free path worth the pretence.
- **Where `Ordering` lives: decided by A1, and A1 took the plain `enum`.** `Ordering` is declared in the shipped `std/atomic` module as `enum Ordering { SeqCst }`, and the builtin signatures name that type by lookup of its module key (the same way `try_to_<T>` names `std/option`'s `Option`). The cost is one `impl "std/atomic"` line and the module qualifier: a program writes `atomic.Ordering.SeqCst`, not `Ordering.SeqCst`. The other route would be a type the compiler knows by name, the way it knows `Bytes` and `View`: it needs no import, but it would also have to carry its own literal syntax, its own exhaustiveness support and its own codegen, which is a second copy of what `enum` already is. **The surface above is unchanged either way**, and the exhaustiveness check comes free with the enum route: a `when` over an ordering is re-checked the day a second variant exists.
- **The fence is a free `thread.fence(order)`, in the shipped `std/thread` module.** A2 decided this, which resolves open question 9.2. A standalone fence orders no location, so it is not an operation on an `Atomic<T>`: there is no cell for it to read or write, and a method would say otherwise. The module is where the thread surface goes, so B1 documents `spawn` and `join` there and C1 lowers them, rather than inventing a module around one function. What it lowers to is one instruction per backend, so it is a compiler builtin with no cell and no `.kp` body: the module's own source documents the surface and declares nothing, exactly as `std/atomic` documents the atomic methods it declares nowhere.
- **An atomic is both sendable and shareable**, which is what makes it the whole of v1's inter-thread communication for scalars. It is also the first type for which the `Sync`-shaped half of §1 has to hold, so it is the test case for that rule rather than an afterthought.
- **A `pd val` atomic is how a program shares a counter**, since a `pd` is the process-lifetime place a shared borrow may cross from (§1). It needs no drop, so the `atexit` story for owned passdowns does not apply.

### The thread handle and the two calls, v1

Stage B1 adds the checker surface below and stage C1 lowers it. Both calls are compiler builtins in the shipped `std/thread` module, beside the fence, and neither is in the builtin signature table: `T` comes from the value that crosses the boundary, so the checker builds the signature at the call site. **C1 landed 2026-09-24** (see §6): both calls are lowered, `Thread<T>` is one pointer to a worker record the runtime owns, and a handle nobody joins is dropped by detaching the worker.

```kelp
fun thread.spawn(entry: <a declared `fun`, a `fun` literal, or a capturing closure, taking one move parameter>, arg: move A) => Thread<T>
fun thread.join(handle: move Thread<T>) => Result<T, ThreadErr>
```

- **`T` is the entry point's return type and `A` its parameter's**, so the handle carries what the worker hands its joiner, and the argument is moved into the worker as the entry point's one parameter. The checker reads both off `entry` and builds the signature at the call site.
- **`Thread<T>` is a compiler builtin generic** in the shape `Atomic<T>` has: monomorphized per element type, not `Copy` (a copy would name one worker twice), and one pointer to a worker record the runtime owns, so it is dropped like any other owned value (§5). A handle is accepted as a local, a `datac`/`enum` field and a parameter, and refused as a `List`/`Map`/`Box`/array element, the rule an atomic already has: a handle is joined once, while a container's own routines copy and drop an element like a plain location.
- **The entry point is an ordinary function value or closure.** A declared `fun`, a `fun` literal and a capturing closure all work, through the existing closure machinery (MK8-S1-M2 phases A-F): the widening of a plain function value where a closure is expected needs no new rule.
- **`ThreadErr` is a plain `datac` in `std/thread`**, one owned `message: String` field carrying the panic text the worker died with (§2). It is named by the builtin `thread.join` signature through the module's key prefix, exactly as an `Atomic<T>` signature names `Ordering` through `std/atomic`.

## 5. What the checker owes, concretely

The rules to implement, in the order they unblock each other:

1. **A thread entry point's argument is moved.** `thread.spawn(f, arg)` takes `arg` by move, so a call site obeys the existing `move T` rules and no new move machinery is needed.
2. **Refuse a borrow argument that is not a `pd` borrow.** A `&mut T` argument, or a `&T`/`str`/`View` borrow of a frame local, is refused with a message naming the reason (the lifetime is the sending frame's, and the thread outlives it).
3. **Refuse a shared borrow whose pointee has a reachable `&mut` path.** The `Sync`-shaped rule: an `Atomic*` passes, a `datac` with a reachable `var` field does not.
4. **Refuse a moved value that contains a borrow.** Already enforced by `is_borrowed_field_ty` for stored positions; the new part is the top-level `&T`/`str`/`View` argument, which rule 2 covers.
5. **The rule attaches to `thread.spawn`, not to a user-facing marker.** Because A-12 (traits) has not shipped, there is no `Send`/`Sync` trait to implement, and inventing one now would be a language feature with one consumer. The discipline is **structural and builtin**: it lives in the checker beside the existing "a borrowed type cannot be stored" rule, the same way `is_map_key` is a builtin predicate rather than a trait. When A-12 lands, a user-facing marker can be layered on top of a rule that already works.

**Implemented as stage B1 (2026-09-24).** Rules 1 and 4 fall out of the `move T` signature: a `&mut T` is refused by name, and a moved value that carried a borrow could not be constructed in the first place, since `is_borrowed_field_ty` refuses a borrow, a `str` or a `View` in a `datac` field, an enum payload, a `List` element and a `Map` value. Rules 2 and 3 are checked on the argument as written: a shared borrow crosses only when its root place is a `pd` global of the sending module, and only when a walk over the pointee finds no reachable `var` field (through `datac` fields, enum payloads and container elements), which is why `&Atomic<T>` passes and a `datac` with a `var` field does not. Each refusal names its own reason, and each has one fixture refused by name (`kelpc/tests/fixtures/thread_spawn_*.kp`). The rule is checked in `infer.rs`; `types.rs` holds the `Thread<T>` builtin and its element predicate, and `stdlib.rs` the module id and the two call keys.

**Lowered as stage C1 (2026-09-24), and one rule the checker did not have to write.** A `Thread<T>` is a handle on memory the runtime owns, so it is dropped like any other owned value; what its drop *does* is the decision this stage had to record. Dropping a handle whose worker is still running cannot free the record, the argument slot or the result slot, because the worker writes through them: the drop therefore **detaches the worker and leaks the record**. That is a deliberate, safe leak of the same class as the one open item 15 records (a spawned process nobody waits for), and the alternative is worse here: refusing to drop a handle is a rule the language does not have (there is no `must_use`, and user `Drop` arrived at MK9-S6-M1 as a per-type destructor rather than a per-handle rule), and blocking the drop on the worker would turn a scope exit into a join nobody asked for. A handle that *is* joined is freed properly by the join itself, so the leak is exactly one record per handle a program drops without joining. The panicking worker's frame is the other admitted leak, and §2 already records it: a panic ends the worker by `pthread_exit`, so the values the abandoned frame owned are never dropped. What the runtime does *not* abandon is its own bookkeeping: the trampoline registers a `pthread_cleanup` handler, which `pthread_exit` runs on the way out, so the worker record and the closure's environment are freed on the panic path as well as on the normal one. `kelp_panic`, `kelp_panic_at`, `kelp_panic_errno` and `kelp_panic_gai` stay `_Noreturn`; the worker branch they take records the panic text (with the errno or getaddrinfo line folded in, since there is no stderr the main thread is watching) and calls `pthread_exit`.

## 6. Stages

Ordered by dependency. Each stage is one lane and lands with the suite green.

| stage | what | proves it |
| --- | --- | --- |
| **A1** | **done 2026-09-22.** `enum Ordering { SeqCst }` and the `Atomic<T>` builtins, both backends, single-threaded. The representation: `Atomic<T>` is a compiler builtin `Ty` in the shape `List<T>` has, monomorphized per element type, emitting `__atomic_*` builtins in C and `load atomic`/`store atomic`/`atomicrmw`/`cmpxchg` at `seq_cst` in LLVM; `Ordering` is a plain `enum` in the new `std/atomic` module (§4) | a counter incremented and read through an atomic matches a plain integer's result, and `compare_exchange` retries in a loop (`kelpc/tests/fixtures/atomic_counter.kp`, `atomic_int_fetch.kp`, `atomic_bool.kp`, run by both backends with identical stdout) |
| **A2** | **done 2026-09-22.** `thread.fence` as a compiler builtin in the new `std/thread` module (the C backend emits `__atomic_thread_fence(__ATOMIC_SEQ_CST)`, the LLVM backend `fence seq_cst`), and the memory-model statement in `plan/syntax.md` (12.5 and 16.16 landed with A1; the statement is 12.6 and the module is 16.17) | a fence compiles and emits the right LLVM atomic fence and C `__atomic_thread_fence` (`kelpc/tests/fixtures/thread_fence.kp`, run by both backends with identical stdout, and each backend's own artifact asserted to carry the instruction) |
| **B1** | **done 2026-09-24.** the checker rules of §5: `Thread<T>` as a builtin `Ty` (refused as a container element like an atomic, accepted as a local, a field and a parameter), `ThreadErr` in `std/thread`, the `thread.spawn`/`thread.join` signatures with their call keys, and rules 1 to 4 as refusals, each naming its own reason. No lowering: a call that passes every rule is refused with `MK9-S3-M1 stage C1` named | a probe per rule is refused by name (`kelpc/tests/fixtures/thread_spawn_mut_borrow.kp`, `thread_spawn_local_borrow.kp`, `thread_spawn_str_arg.kp`, `thread_spawn_var_field.kp`, `thread_spawn_borrowed_value.kp`), the one borrow that crosses is accepted (`thread_spawn_pd_atomic.kp`), and every accepted shape stops at the stage C1 interim refusal (`thread_spawn_accepted.kp`, `thread_spawn_closure.kp`, `thread_join_accepted.kp`) |
| **C1** | **done 2026-09-24.** `thread.spawn` / `thread.join` with the panic policy of §2: a real OS worker per spawn, the argument moved into it, the result slot read by the join, and a panic in a worker recorded as the join's `Err` rather than taking the process. Both backends lower it through the shared `runtime/kelp_rt.c` pthread machinery and one generated worker body per spawn signature. A handle nobody joins is dropped by detaching the worker and leaking its record, and a panicking worker's frame is abandoned: both leaks are admitted in §5 | N threads incrementing one `pd val` atomic produce the exact total (`kelpc/tests/fixtures/thread_stress.kp`, 8000 on both backends); a panicking thread is seen as `Err` by its joiner with the process still alive and another worker's work intact (`kelpc/tests/fixtures/thread_panic.kp`, stdout `worker exploded`, `0`, `42`, exit 0 on both backends); every accepted shape B1 pinned is lowered (`thread_spawn_accepted.kp`, `thread_spawn_closure.kp`, `thread_spawn_pd_atomic.kp`, `thread_join_accepted.kp`), and the §5 refusals stay refused by name, at the differential sweep and under valgrind |
| **C2** | **done 2026-09-25.** `thread.sleep_ms` and `thread.current_id`, both ordinary functions rather than checker builtins (unlike `spawn`/`join`, neither one's type depends on what crosses the thread boundary), each an `.kp` signature over an `extern fun` body the way `std/time` and `std/rand` already work: `sleep_ms` is `std/time`'s `nanosleep` loop under `std/thread`'s name (sleeping takes no capability, so a worker pool's backoff needs no `time` import), and `current_id` returns `(uint64_t)pthread_self()`, stable for a thread's whole lifetime and distinct between the main thread and any worker. "Whatever MK6-S2-M1 needs" beyond these two is deferred to that milestone, since nothing else is nameable yet | a worker sleeps and still hands its joiner a value (`kelpc/tests/fixtures/thread_sleep.kp`, stdout `42`); `current_id` is stable across two calls on the main thread and differs from a spawned worker's (`kelpc/tests/fixtures/thread_current_id.kp`, stdout `true`, `false`), both on both backends |
| **D1** | **done 2026-09-25.** MK9-S3-M2, the memory model as a standalone published document: `wiki/language/concurrency.md`, rewritten to state the race-prevention rule (§1), the ordering (§3) and the panic policy (§2) as the language's own definition rather than pointing at this file. `plan/syntax.md` §12.6's cross-reference now names the published page instead of "until MK9-S3-M2 publishes it" | the definition in §3 is written down as the language's own, not borrowed by reference |

Not in this sequence: MK9-S3-M3 (thread-local and per-CPU storage, `B-11`), which is its own storage-duration class and not needed by threads-in-a-hosted-program.

## 7. Gates

- **A stress gate is the only honest test of a memory model.** A1/C1 land with a program that spawns `N` threads, each doing `M` increments of one shared atomic, and asserts the exact total after every join. That catches a lost update, which a single-threaded test cannot.
- **A race gate is a rejection fixture.** Every rule in §5 gets one probe that must be refused *by name*, so the rule cannot rot silently.
- **A panic gate** spawns a thread that panics, joins it, asserts `Err`, and asserts the process exits 0 with the other thread's work intact.
- **Two-backend agreement.** The C backend (the oracle) and the LLVM backend must produce the same total for the same stress program at every optimization level, the same way the existing corpus does. An atomic instruction sequence differs enough between the two emitters that this is the differential gate that matters here.
- **The ordering slot is visible in every signature.** Every atomic operation takes an ordering argument, and a probe passing `Ordering.SeqCst` compiles and runs, so the reserved slot is in the API rather than merely planned. Because v1 has one variant, naming another is the ordinary unknown-variant error; the second variant is added later as a widening, and this gate is what makes that true.

## 8. Non-goals

- **`async`/`await`, green threads, an executor, thread pools as a language feature.** `async` needs a state-machine transform in codegen; a pool is a program, not a primitive, and §4 plus §2 are enough to write one.
- **`Relaxed`, `Acquire`, `Release` in the surface.** Reserved by §3, not shipped.
- **A runtime race detector as the enforcement mechanism.** Wanted as a testing instrument under MK11-S2-M1.
- **Channels and a `Mutex` as *checker* requirements.** A channel is still a value type obeying the rule of §1 and a reasonable next step. **`Mutex<T>` itself shipped as B-5's remainder** in `std/sync` (`stdlib/sync.md` (kelp wiki)): a compiler builtin in the shape `Atomic<T>` has, reached only through the scoped block `m.lock { v -> ... }`, released on every exit and poisoned by a panic inside the block. It obeys §1: a `&Mutex<T>` has no reachable `&mut` path, so the one crossing rule covers it, and the `Sync`-shaped walk answers `false` for `Ty::Mutex(_)` whatever `T` is.
- **`unsafe`, `AtomicPtr`, raw memory.** MK9-S2-M1.
- **Per-CPU and thread-local storage.** MK9-S3-M3.
- **A user-facing `Send`/`Sync` trait.** A-12 first; the rule is structural until then (§5).

## 9. Questions left open deliberately

Four forks the owner chose on 2026-09-22 to leave open, each to be decided when it actually comes up rather than speculatively. None of them blocked stage A1. A2 has since decided the second, A-10 has decided the third, and the crossover question that used to head this list is now a decision, recorded in §1.

1. **`compare_exchange` strong only, or weak as well?** §4 has the strong form. The weak form only helps inside a loop that retries anyway, so it is either one more signature (`compare_exchange_weak`) or not worth having.
2. **Is `fence` a free `thread.fence` or an `Atomic<T>.fence`?** §4 puts it on `thread`, because a standalone fence belongs to no location, but that is a naming choice rather than a semantic one. **Decided with A2, 2026-09-22: a free `thread.fence(order)`, in the new shipped `std/thread` module.** A fence orders no location, so it is not an operation on an `Atomic<T>`: there is no cell for a method to read or write. It is in `std/thread` because that is where C1's `spawn` and `join` go, so giving it one name now means C1 adds names to a module rather than inventing one. A2 landed it as a compiler builtin, one instruction per backend (§4, §6).
3. **May an atomic be a `datac` field, a `List` element, a `Map` value?** §4 does not say. Allowing it in a `List` is convenient and interacts with the container element rules (A-10), so it should be decided with that item rather than by accident. **Decided with A-10, 2026-09-23: an atomic stays refused as a `List`/`Map`/`Box`/array element, while a `datac` field of atomic type stays accepted.** The reason is that a container's own routines read and write an element as a plain location, and every access to an atomic has to be one atomic operation, so an element has nowhere for the atomic's operations to sit; a field, by contrast, is reached by field access, which is exactly where the atomic's own methods sit, so the field half is already fine. **A1 landed with the question still open, and did not decide it**, and the code has behaved this way since then; A-10 slice 1 is what wrote the decision down, in the same item that loosened the container element rules, so the two cannot drift.
4. **What does the checker do with a `pd` atomic inside a capability-gated module?** A shared-mutable global is a capability question as well as a memory-model one. This one is scoped to MK10-S1, where the vocabulary lives.

## 10. References

- `meta/roadmap.md` (kelp wiki): MK9-S3 (memory model), MK6-S1-M1 and MK6-S2-M1 (threads, concurrent downloads), open item 60 (every prerequisite in one place), the `B-4` / `B-5` / `B-8` / `B-11` backlog entries, and §Dependency notes for the MK6-S1-M1 to MK9-S3-M1 edge.
- `plan/architecture.md`: what v1 ships and what it defers; the language's ownership model, which §1 extends rather than replaces.
- `meta/milestones.md` (kelp wiki): the MK5-S7-M1 entry (what is possible without threads, and why threads wait).
- `plan/syntax.md`: the language and style authority. §3 (the type table), §4 (`enum`), §9 (ownership) and §20 (grammar) are what A1, C1 and the `Ordering` enum edit.
- `plan/mk7-llvm-backend.md` has been folded into `meta/milestones.md` (kelp wiki); the backend's atomic instruction emission is the emitter's concern, not a separate plan.
