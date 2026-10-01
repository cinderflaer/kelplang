# Scoped trailing blocks: parameters, value blocks, and non-escaping callbacks

*Design note for the task that turns `plan/trailing-closure.md`'s prototype into something
Polyp can use: `polyp.column { k -> k.text(s.title) }`. Not a Mark; `roadmap.md` owns the schedule.*

## What the two parts are

Trailing blocks exist as a parser-only prototype (`plan/trailing-closure.md`): a same-line `{`
after a call is that call's last argument, written as a zero-parameter `fun` literal with a unit
return. Two things are missing before a builder API can be built on it.

**Part 1, block parameters and value blocks.** A trailing block may declare parameters
`{ x, y -> body }`, with no types written; the types come from the expected function type of the
parameter the block is passed to. A block whose body ends in an expression returns it when the
expected return type is non-unit; with a unit return the tail is discarded exactly as today. The
mode of each parameter follows the expected type, because a Kelp parameter type already carries
its mode: `fun(mut Counter)` is a function whose single parameter type is
`Ty::Ref { mutable: true, Counter }`, so a block parameter bound with that type *is* an exclusive
borrow with no second rule to keep in sync.

**Part 2, scoped blocks.** A function parameter written `scoped fun(T).R: U` accepts only a `fun`
literal or a trailing block written at the call site, never a stored value. The literal may
capture an enclosing binding **by borrow**, because the callee can only call it during the call.
The capturing literal is a stack environment with a no-op drop, not a heap one.

## Why the checker can be a type

The prototype's limit 6 (`plan/trailing-closure.md`) is that a closure owns its environment and
cannot capture a borrow, so a callback cannot read a borrowed `state`. The task's list of things a
scoped parameter must not do (stored in a `datac`, returned, put in a collection, passed to a
non-scoped parameter, captured by another closure) is exactly a list of places where a value's
type has to equal something else. That is the argument for making scopedness a **type** rather than
a side table of "scoped bindings" in `Ctx`:

- `Ty::ScopedFn { params, ret }` is a function value that may not outlive the call it was written
  in. It is distinct from `Ty::Fn` and `Ty::Closure`, so every ordinary escape is an ordinary type
  mismatch, with the existing diagnostic. No new check has to be added to `datac` fields, returns,
  `List`/`Box` elements or `move` parameters, and no list of sites can fall out of date.
- The one check that is *not* free is a capture: a `fun` literal that captures a `ScopedFn` binding
  would put it in an ordinary closure's environment, which can be returned. `check_fn_literal`
  refuses that by name.
- A scoped value may be re-bound (`val g = cb`) or passed to another scoped parameter; both keep it
  inside the call, so both stay legal.

The spelling is contextual. `scoped` is not a keyword: in a type position it is read as a modifier
only when the next token is `fun`, so a program that has a type or binding called `scoped` is
unchanged. It is accepted only on a top-level parameter of a declaration, because a scoped type
nested inside another function type could be stored as that type, which is precisely what the
modifier forbids.

## Semantics of a scoped literal

`Ty::ScopedFn` is closure-shaped at the ABI level, and that is the whole representation: `{ fn, env,
drop }`, `fn` taking the environment first, exactly as `Ty::Closure` is (`codegen_c.rs
::write_closure_typedef`). What differs is where the environment lives and what `drop` does.

- **Environment.** The literal's captures are stored in a struct allocated on the *enclosing*
  function's stack (a C statement-expression local / an LLVM `alloca`), because the value cannot
  outlive the frame that wrote it. `drop` is `NULL`, so nothing frees it and nothing drops through
  it.
- **Captures.** A capture is one of three things. A `Copy` binding (a scalar, an opaque handle, a
  function value) is copied. A value already of a borrowed type (`Ty::Ref`, `str`, `View`) is
  copied too: it is a pointer, and the borrow it carries is already live in the enclosing frame. An
  owned non-`Copy` binding is captured as a **shared borrow**: the environment field's type is
  `Ty::Ref { mutable: false, T }` and its initializer is a `Ref` node over the binding's name,
  exactly the node `infer.rs` writes for an implicit call-site borrow. Because every field is a
  `Copy` pointer or a `Copy` value, the environment owns nothing and the no-op drop is correct, not
  a shortcut.
- **Exclusive captures are refused.** A literal that writes through a captured binding needs an
  exclusive borrow; the message says so and names the shared alternative. This is the sound half of
  the task's "shared, or exclusive if it only reads through the `mut` places it is given"; the
  builder case that motivated the task reads only.

## Borrow checking

A scoped literal is only ever a call argument (`infer.rs` refuses it anywhere else), so its
captured borrows have exactly the lifetime of that call. `borrowck.rs::take_call_arguments` already
starts every direct `Ref` argument before any of them release, which is what makes
`f(&mut c, &c)` a conflict. The scoped literal joins that rule: for each `Ref` capture in a
`ScopedClosureLit` argument, `check_place` is run and `start_borrow` is recorded in the same `taken`
list, so a same-call aliasing `g(&mut s, { k -> use(s) })` is refused, and every borrow releases
together after the call. Nothing durable is registered, because nothing durable can hold one.

## Part 1 mechanics

`ast::Expr::FnLit` gains `infer_params: bool`, set by the block parser for `{ x -> ... }`.
`check_fn_literal` takes the expected type. When `infer_params` is set and the expected type is a
function, closure or scoped-function type, each parameter's binding type is the expected parameter
type at that index and the literal's return type is the expected return; with no expected function
type the literal is refused by name rather than guessing. A block with no header (`{ ... }`) keeps
today's behaviour when the expected type is a `fun()`/`closure()`/scoped `fun()`; with an expected
non-unit return its tail is the value, which is the value-block half.

A **generic** callee's `scoped fun(...)` parameter is the one place that needed more than the
signature: the call's type arguments are inferred before its arguments are checked, and the hint
handed to each argument came from `field_hint`, which resolved the written type through the storage
path and so refused the modifier. `field_hint` now resolves a `scoped` parameter through the same
parameter path a non-generic declaration's signature uses (`resolve_scoped_param`), which is what
lets `polyp.column { k -> ... }` take `k`'s type from the expected return type of `column<M>`
(fixture `scoped_block_generic`; the Polyp builder in `examples/polyp/lib.kp`).

## What is deliberately out of scope

- Exclusive (`mut`) captures, `move` captures, and a scoped literal that outlives its call.
- `scoped` on anything but a top-level declaration parameter (including a `fun` type's own
  parameters).
- A borrow returned from a scoped block, and a scoped block passed where a `closure(...)` is
  expected (or the reverse). The two types are separate, and the conversion between them is the one
  thing that would make escape possible again.
