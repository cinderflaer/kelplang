# Exclusive captures for scoped blocks (language completeness 7 of 8)

*Design note and shipped first version. `plan/scoped-blocks.md` made a trailing block capture the
enclosing frame by shared borrow only; this note lets a block **write** what it captures. Verdict:
**implement now, and it is implemented** (section 5). The fixtures named below are in
`kelpc/tests/fixtures/excl_*.kp`, the tests are `a_scoped_block_can_write_what_it_captures`,
`an_exclusive_capture_conflicts_with_the_rest_of_its_call` and
`an_exclusive_capture_is_refused_where_it_cannot_be_sound` in `compile_and_run.rs`.*

## 0. What was found first

The brief says `state.count += 1` inside a block "is refused (exclusive capture)". It was not. A
write through a captured binding (`s.count += n`, `n += k`) hit `check_target_writable`'s
`unreachable!("not a place expression: Capture { .. }")`, an **internal compiler error**; only a
mutating *call* (`bump(s)`, `xs.push(1)`) reached a real diagnostic ("argument 's' requires an
exclusive borrow, found a shared one"). So there was no rule to relax, only a crash to replace.
Fixing the crash is part of this change: a write through a capture is now either an exclusive
capture (the rule below) or a refusal that names the reason.

A second, older gap showed up while testing the rule: two reborrows of one `mut` *parameter* in one
call were not checked. It is fixed with the A-11 follow-up: an `&mut`-typed identifier passed as an
argument is registered as an exclusive reborrow for the call, even though inference represents it
as a plain `Ident` rather than a fresh `Ref` node. Thus `fun h(s: mut State) { g(s, s) }` is refused
by the same-call alias check, and `refused_mut_param_reborrow_twice.kp` pins it.

## 1. The rule

**A scoped block captures a binding by exclusive borrow exactly when its body needs one.** There is
no new syntax and no annotation; the capture's mode is inferred per binding from the body:

- A binding is *needs-exclusive* when the body writes through it (`s.count += n`, `n = 5`), takes
  `&mut` of it or of a field, calls a `mut`-receiver method on it (`xs.push(n)`), or passes it to a
  `mut` parameter (`update(state, msg)`).
- A needs-exclusive capture is allowed when the enclosing binding is one of: an owned `var` (a
  `datac`, a collection, a `String`, **or a `Copy` scalar**), or a `mut` parameter (already an
  exclusive borrow, reborrowed for the call). Anything else is refused (section 2, R5-R7).
- Every other capture stays what it was: a shared borrow of an owned binding, a copied pointer for
  a borrow-typed binding, a copied value for a `Copy` binding the block only reads.
- The block's captures live exactly as long as the call the block is written in, and each
  exclusive capture is an exclusive borrow of its owner for that whole call. That is the entire
  semantics; everything below is a consequence.

**Interactions** (each is a fixture; the exact errors are in section 2):

| Situation | Outcome |
|---|---|
| Second capture of the same owner in the same call (two blocks, or the block and another argument) | refused: the second borrow conflicts with the first (`already borrowed exclusively`) |
| The callee's other arguments mention the owner (`two(s) { s.count += n }`, shared or `mut`) | refused (`already borrowed exclusively` / `already borrowed as shared` / the `mut`-parameter message) |
| The call's receiver is the captured owner (`s.each { s.count += 1 }`), or a field of it (`s.bag.each { s.count += 1 }`) | refused: the receiver's reservation conflicts. Roots are whole variables, so a field of `s` and `s` conflict |
| A live borrow of the owner still used after the call (`val first = s.items.get(0)` ... `print(first)`) | refused: borrows end at their last use (`plan/nll.md`) and this one has not ended |
| The same borrow dead before the call | accepted: the call is the only user of the owner, and the owner is free again afterwards (`excl_capture_field`) |
| Nested block capturing the outer block's exclusive capture | refused in v1, with a message that names the two ways out (section 5) |
| `defer` in the block | the lowering splices it into the block body, so it runs when the block returns, once per call; it is an ordinary statement of the block (`excl_capture_defer_loop`) |
| `defer` in the enclosing function that reads the owner | runs at function end; the capture was released when the call returned, so it is fine |
| `return` inside the block | leaves only the block (a literal is its own function); the call and its borrows end normally (`excl_capture_returns_value`) |
| The callee loops, calling the block many times | fine: one call, one borrow; the block's own body is checked as a function whose capture is a `mut` parameter, so every use inside it is already exclusive-checked |
| The block returns a borrow (`str`, `View`, `&T`) while it holds an exclusive capture | refused: the borrow would outlive the call that holds the owner exclusively; return an owned value |
| A `Copy` `var` the block writes (`total += n`) | captured by exclusive borrow, reads and writes go through the pointer, the new value is visible after the call (`excl_capture_copy_var`); a `Copy` `var` the block only reads is still copied in |

## 2. Why it is sound, in terms of the existing machinery

Three facts carry the argument; none is new.

1. **A scoped value cannot outlive its call** (`Ty::ScopedFn` is a type, `plan/scoped-blocks.md`).
   Every escape is already an ordinary type mismatch, so an exclusive borrow inside the environment
   has exactly the lifetime of the call. Nothing durable can hold it.
2. **A call's argument borrows start together and release together**
   (`borrowck.rs::take_call_arguments`). The scoped literal's captures were already registered there
   as `start_borrow` entries (shared); an exclusive capture is the same entry with `mutable = true`,
   so the existing conflict table (`start_borrow`: exclusive vs shared vs reserved receiver)
   decides every cell of the interaction table. Borrow roots (`plan/provenance.md`) make a block
   that captures a cursor or a re-pointed borrow conflict with its real owner, and last-use liveness
   (`plan/nll.md`) makes a borrow that is dead before the call not conflict at all.
3. **Inside the block the capture is a `mut` parameter.** The block body is its own function; a
   capture typed `&mut T` is read, written and passed on exactly as a `mut T` parameter is, with
   the same checks (`check_target_writable`, the `mut`-argument rule). The one thing a parameter
   has that a capture did not is a *caller* that checks exclusivity; that caller is the
   `take_call_arguments` entry above.

One new check is needed because a `&mut` *name* passed on is not a `Ref` node (the gap in
section 0): a capture that copies a `&mut`-typed binding (a `mut` parameter) into the environment is
a reborrow, and `take_call_arguments` looks at every other argument of the same call and at every
earlier exclusive capture for the same root before registering it.

The inference is a bounded retry: the literal is checked with every owned `var` capture shared; a
write (or a `mut` argument) through one records the name instead of being final; the literal is
checked again with exactly those names typed `&mut T`. The set only grows and is bounded by the
number of captures. Refusals that name nothing upgradable return as they are.

### Worked examples

Accepted (each is a fixture, output verified on the LLVM backend and the C oracle):

- **A1 `excl_capture_field`.** `each { n -> s.count += n }` on `var s`, `each` calling the block
  twice: prints `13`, then `113` after `s.count += 100` (the owner is free after the call).
- **A2 `excl_capture_copy_var`.** `var total = 0 ... each { n -> total += n  calls += 1  if total
  > 3 { total += 1000 } }` prints `1006` and `3`.
- **A3 `excl_capture_list_push`.** `seen.push(n * 10)` and `names.push(...)` inside the block;
  after the call the lists are read normally (`0 10 20 3`).
- **A4 `excl_capture_mut_param`.** `fun tally(c: mut Counter) { each { n -> c.hits += n }
  c.hits += 100 }` prints `103`: the block reborrows the parameter for the call only.
- **A5 `excl_capture_two_owners`.** One block writes `a` and `b` and reads `scale` (a shared
  capture) in the same call; afterwards both owners are written again.
- **A6 `excl_capture_returns_value` / `excl_capture_defer_loop` / `excl_capture_drain`.** An owned
  return value, `return` inside the block, `defer` in the block and in the function, a callee
  loop, and the Elm drain loop of section 3.

Refused (exact text from the compiler; the span is the capture or the argument it conflicts with):

- **R1 `excl_refused_same_call_arg`** (`two(s) { n -> s.count += n }`, `two(s: mut State, ...)`):
  `error: cannot borrow 's': already borrowed exclusively`.
- **R2 `excl_refused_two_blocks`** (`both(fun(n: i32) { s.count += n }, fun(n: i32) { s.count +=
  n * 2 })`): `error: cannot borrow 's': already borrowed exclusively`.
- **R3 `excl_refused_shared_arg`** (`show_each(s) { n -> s.count += n }`, the callee reads `view:
  State`): `error: cannot borrow 's' as exclusive: already borrowed as shared`.
- **R4 `excl_refused_receiver`** (`s.each { n -> s.count += n }` on `fun mut State.each`) and
  `excl_refused_param_aliases_owner` (`s.bag.each { ... s.count ... }`): `error: cannot borrow 's'
  as exclusive: already borrowed exclusively`.
- **R4b `excl_refused_live_borrow`** (`val first = s.items.get(0)` ... block pushes ...
  `print(first)`): `error: cannot borrow 's' as exclusive: already borrowed as shared`.
- **R5 `excl_refused_val`** (`val s = ...` written in a block): `error: cannot write to captured
  's': it is not declared 'var' in the enclosing function; declare it 'var' so a scoped block can
  write it`.
- **R6 `excl_refused_shared_param`** (`fun bump_all(s: State) { each { n -> s.count += n } }`):
  `error: cannot write to captured 's': it is a shared borrow in the enclosing function, so a block
  cannot write through it; take it as a 'mut' parameter there`.
- **R7 `excl_refused_returns_borrow`** (block writes `s.count`, tail `s.label.as_str()`): `error:
  this block writes through 's' (an exclusive capture) and returns a borrow ('str'): the borrow
  would outlive the call that holds 's' exclusively; return an owned value (a copy or a
  'String.from_str(...)') instead`.
- **R8 `excl_refused_nested`** (inner `each { b -> s.count += b }` inside a block that already
  writes `s`): `error: cannot capture 's' into this nested block: it is an exclusive capture of the
  enclosing block, and a block inside a block cannot hold it; pass 's' to the inner call as an
  ordinary argument, or capture it in the outer function`.
- **R9 `excl_refused_mut_param_twice`** (`fun run(s: mut State) { two(s) { n -> s.count += n } }`):
  `error: cannot borrow 's' as exclusive: this call also uses it, and a block that writes through a
  captured exclusive borrow needs the call's other arguments to leave it alone`.
- **R10 `excl_refused_stored`** (`fun keep(f: scoped fun(i32)).return: Handler { return Handler(f = f)
  }`): the scoped-value escape check, unchanged: `this is a scoped function value: it may only be
  passed to another 'scoped fun(...)' parameter ...`. Exclusivity did not touch it.

## 3. What it buys Polyp, and what it does not

**Buys.** Any callback that runs *during* a call can now write the frame that wrote it. Three
Polyp shapes:

1. The message pump, without threading the model through the callback. Before, the loop owned the
   `while` and called `update(model, msg)` itself; with a queue that exposes `drain(f: scoped
   fun(Msg))` the loop lives where the queue's state is, and the application writes only the
   handler (`excl_capture_drain` is this program, run on both backends):

   ```kelp
   fun pump(model: mut Model, q: mut Queue<Msg>) {
       q.drain { msg ->
           update(model, msg)
           mark(model)
       }
   }
   ```
2. Tallies and bookkeeping while a builder block emits children. The todo `view` computes
   `done_count`/`permille` in separate passes over `s.items`; the column block can do both in its
   one loop, because the progress bar only needs the count after the loop (`excl_capture_view` is
   this shape, with a stand-in builder, on both backends):

   ```kelp
   pub fun view(s: Model).return: core.Node<Msg> {
       var done = 0
       return core.column { k ->
           var i: usize = 0
           while i < s.items.len() {
               val item = s.items.get(i)
               if item.done { done += 1 }
               k.keyed(str.from_i64(item.id.to_i64()).as_str()) { r ->
                   r.text(item.title.as_str())      // the inner block only reads
               }
               i += 1
           }
           k.progress(done * 1000 / s.items.len().to_i32())
       }
   }
   ```
   Only one block level writes `done`; the inner `keyed` block reads `item` and writes nothing,
   which is allowed. The real todo nests `scrolled` between the two, and a counter written from
   the *inner* block of a pair is the nested case v1 refuses (section 5). The gain here is small
   by design: a view is a pure function of the model.
3. `Mutex.lock { g -> total += g.value }` and the other `std/sync` block APIs: a lock block that
   updates a local accumulator was previously a compile error, and is now the natural spelling.

**Does not buy.**

- A **stored** callback. `button("+1", on_click)` keeps `on_click: move M` (a `Msg` value): a
  retained widget tree outlives every call that built it, and a callback that writes `state` would
  have to hold `state` after the call that created it returned. That is exactly what the scoped type
  forbids, `excl_refused_stored` pins it, and the Elm message shape stays the right design for
  event handlers.
- A write from a block nested in a block (v1), from two blocks of one call to the same owner, or
  from a block whose callee also takes the owner (all refused, section 2).
- Disjoint-field capture: roots are whole variables, so a block writing `s.count` while the callee
  borrows `s.items` conflicts. This matches every other borrow in the checker and is the same
  limit `plan/polyp-v3.md` records for `rects`.

## 4. Implementation (as shipped)

Files and functions:

- `kelpc/src/infer.rs`
  - `Ctx`: three fields, `outer_mut` (the enclosing writable bindings), `excl_captures` (this
    pass's upgrades) and `excl_needed` (names the pass wrote or passed as `mut`).
  - `check_fn_literal` is now the retry loop around `check_fn_literal_pass` (the old body). Failed
    passes truncate the literals they emitted and restore the literal counter.
  - `Ctx::note_capture` types an upgraded capture `&mut T`, a `Copy` one included.
  - `check_target_writable` has a `Capture` arm (the old ICE), and `note_excl_need` records the
    name. The `mut`-argument check in `check_call_arg` calls it too.
  - The identifier resolution returns `Deref(Capture)` for an upgraded `Copy` capture, so reads and
    writes go through the pointer.
  - The capture initializer is `&mut place` for an upgraded owned capture; the refusals for a
    returned borrow and a nested capture are in the capture loop.
- `kelpc/src/borrowck.rs::take_call_arguments`: the `Ref` captures were already registered with
  their `mutable` flag; a copied `&mut` binding is registered the same way after the same-call
  check.
- Codegen: **nothing**. The C and LLVM environments already hold a pointer per borrowed capture and
  a `&mut` field reads and writes exactly like a `&` one; both backends ran every fixture.

Tests: 9 accepted fixtures and 12 refused ones, `excl_capture_*` and `excl_refused_*` (list
above). The existing `scoped_*` and `nll_scoped_*` fixtures are unchanged and pass; no check was
weakened.

## 5. Verdict

**Implement now: done.** The rule is local (one retry loop, one extra registration in the call
borrow table), reuses the existing conflict table for every interaction, needed no backend work,
and the one thing that looked risky (`&mut` names passed on are not `Ref` nodes) is covered by an
explicit same-call check.

Not done, and why (each is a follow-up, none blocks the rule):

1. **Nested blocks.** A block inside a block that uses a name the outer block captures exclusively
   is refused (even a read: copying a `&mut` into a second environment is a reborrow the checker
   cannot yet follow). A name the outer block does not use exclusively is captured by the inner
   block as before, and an inner block may read the outer block's own locals. Allowing it needs the borrow checker to give a `Capture` an owner root inside a literal
   body (so the inner call's other arguments can be checked against it); today a capture has none.
   Until then the message names the two ways out.
2. **Wiki and `fmt`**: `fmt.rs` still has no block-rewrapping rule (open item 75); the wiki pages
   are written from `plan/language-tour-2026-10.md`.
