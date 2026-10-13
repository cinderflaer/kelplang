# `defer` (kernel stage 6, MK9-S6-M1)

*Design note. `language/kernel.md` (kelp wiki) orders the stages; this one gives a kernel deterministic
cleanup it can point at: a lock flag released on every ordinary way out of a block, written
where the lock is taken. It is the `defer` half of B-8; user `Drop` is not in it.*

## The surface

```
fun with_lock().return: bool {
    defer LOCK.store(value = false, order = atomic.Ordering.SeqCst)
    LOCK.store(value = true, order = atomic.Ordering.SeqCst)
    ...
    return true
}
```

`defer EXPR` is a statement. `EXPR` is one expression (a block counts: `defer { ... }`), and it
is checked and emitted exactly as if it were written at every exit of the block it is written
in; the value of `EXPR` is discarded. `defer` is a keyword (the lexer's keyword table), so no
program that named a variable `defer` changes meaning.

## Semantics

- A deferred action runs when the **block** that contains the `defer` is left. Which actions
  run is *positional*: an action runs only if control reached its `defer` statement (so a
  `defer` written after an early `return` does not run on that path). Actions of one block run
  last-in-first-out; an inner block's actions run before the outer block's.
- Every ordinary exit is covered: falling off the end of the block, a `return` in the block,
  and a `return` nested in the block's inner blocks, `if`s and loops.
- A `panic` does **not** run any deferred action: a panic never returns, and the compiler's
  panic path is a call that ends the process. A resource held by a defer at a panic leaks,
  exactly as every other frame value does.
- The deferred code runs with the locals that were in scope at the `defer` statement, still
  alive and unmoved. A local moved out before the block exits and then named by the deferred
  code is the ordinary "use of moved value" error, reported at the `defer`. A `defer` that
  names a local declared *after* it is refused: that local is not in scope where the `defer`
  is written ("a deferred action sees only the locals already in scope where the 'defer' is
  written").
- A deferred action may not contain `return` (it has no caller to return to; the block is not
  left by the action), and, the day the language has them, `break`/`continue` that would leave
  it. Today `break` and `continue` are not Kelp statements at all (`plan/syntax.md` 11), so
  there is nothing to lower at a loop exit; the parser names both rather than reading them as
  unknown identifiers, so a program that tries them is refused by name.
- A `defer` and the `?` operator cannot share a block. `?` leaves the enclosing *function*,
  which would skip the deferred actions of every block between it and the function body, and
  the splice lowering below has no exit for it. This is refused by name: "defer with an early
  `?` exit is not supported yet". The same refusal covers a `when` arm only through the
  `return`/`?` inside it, since an arm is an expression, not an exit of its own.

## How it is implemented

The lowering is a small pass (`kelpc/src/defer.rs`) that runs on the parsed `Program` inside
`resolve.rs`, before name resolution, the type checker, the borrow checker and both backends.
Nothing downstream learns a new form: every deferred action is an ordinary expression in an
ordinary block by the time `infer.rs` sees the tree, so inference, move checking, drop
placement and both code generators apply unchanged.

For one block, with `D1..Dn` its defers in source order:

- the block's statements are rebuilt in order, with each `defer` removed and kept as a cleanup
  action;
- an `Stmt::Return` is replaced by `val #ret = e` (when it has a value), then the cleanup
  actions active at that point (`Dk, ..., D1` for the defers before it) each as
  `val #dN = <action>`, then `return #ret`. The value is evaluated *first*, so a return that
  moves a local the cleanup names is a use of a moved value, as the semantics say;
- a block whose last statement was an expression keeps that tail: it becomes
  `{ val #tail = t; <cleanups>; #tail }` so the block's value survives the trailing cleanup;
- a block that falls through with defers appends the same cleanup bindings at the end.

Defers are re-materialized at every exit, so the pass duplicates the deferred text; each copy's
statements carry the `defer` statement's span, so a diagnostic about the deferred code names
the `defer` rather than the `return` it was spliced before. The `#` prefix of every synthesized
name is the same trick `#for`/`#when` use: `#` cannot appear in a lexed identifier, so a
synthesized binding cannot collide with a written one.

The pass refuses, in the same walk:

- any `?` in a block that has a `defer` (the message above), not descending into a `fun`
  literal, whose `?` belongs to the closure;
- a `return`/`break`/`continue` inside a deferred action (again not descending into a `fun`
  literal);
- a deferred action that names a local declared after the `defer`, by a scope-aware
  free-variable walk.

## What the compiler does not check

A deferred action is not checked for being idempotent, terminating, or free of a second panic:
`defer f()` where `f` panics or loops is the program's problem, and the duplication means a
`defer` with a side effect performs that effect once per dynamic exit path (which is once).
Nothing about the panic path is checked: a `defer` that releases a lock is not run when a panic
takes the process, so the lock stays held in the core that panicked. `language/unsafe-ub-catalogue.md` (kelp wiki)
carries the stage's section.

A block in *value* position (`val v = { defer d(); x }`, `if c { defer d(); x } else { y }`) is
lowered through a synthesized binding, which is the nested-block form the language already has,
so the value's borrows follow the existing block rules rather than the direct-expression ones.
That is a property of block values today, not of `defer`: a view read out of a block does not
pin its owner the way `val v = s.as_str()` does (`val v = { s.as_str() }` behaves the same way
with and without the `defer`), and a borrow held in a local and returned is refused naming the
local as the root. A function *body* is not this case: its tail is lowered as an explicit
`return`, so `defer_borrow_return.kp` keeps its elided lifetime.
