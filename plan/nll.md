# A bound borrow ends at its last use (B-1 of the borrow checker, A-11 slice 2)

*Design note for the task that closes `roadmap.md` open item 30. `borrowck.rs` is the code;
this is the rule, the algorithm, and the shapes it deliberately keeps refusing.*

## The rule

A borrow held in a local binding is live from its creation to the last use of that binding, or
of a value derived from it that is still reachable: a reborrow bound to another local, a `View`
made from it, a closure that captured it, a returned borrow. Along every path that can still
reach a use. Once no use remains the borrow is dead, and the owner may be mutated, moved or
reassigned.

Owners still **drop lexically**, at scope end, exactly as before. This task changes only when a
borrow stops conflicting. `DropPlan`, codegen and both backends are untouched: the drop plan is
computed from `needs_drop`/`MoveState`, which this pass does not change.

Outcome for the open-item-30 example: `val first = names.get(0); ... names.push(x)` is accepted
once `first` has had its last use, in one block, with no extra `{ }`.

## The algorithm

No CFG. The walk already keeps one entry per scope-durable borrow in the scope that declared it
(`Scope::scope_borrows`, previously a bare `Vec<String>` of owner roots). Each entry now carries
`release_after`: the statement index, in the block that owns the entry, after which the borrow is
dead. `usize::MAX` means the old lexical answer (release at `pop_scope`).

- A `val`/`var` whose declared type binds a borrow computes its own last use with a **backwards
  scan of its own block** (`last_use_index`): the largest statement index at or after the
  declaration whose statement mentions the binding, or `block.stmts.len()` when the block tail
  does. Mentions are found with a structural walk (`expr_mentions`), so a use nested in a loop, an
  `if` arm, a scoped literal's captures, an `asm` operand or a template piece all count.
- If the declaration created a **fresh borrow** (the initializer took one: `&expr`, `l.get(i)`,
  `s.view()`, a borrow-returning call), the entries it added get that release index.
- If the declaration is an **alias** of an existing borrow (its initializer is a bare borrow
  `val q = p`, a `View` bound from one, a `str` derived from one), no new entry is created here;
  the entries already recorded for the same owner root **in the same block** are extended to
  `max(release, last use of this binding)`. That is what makes `val p = l.get(0); val q = p;
  l.push(x); print(q)` still refused.
- After each statement, `check_block` releases the entries in its own scope whose `release_after`
  has been reached. Anything left is released at `pop_scope`, exactly as today.

The structured rules fall out of statement granularity:

- **A loop body.** A use inside a loop is a use of the loop statement, so the borrow is live for
  the whole loop, including the statements before the use in the body, because the next iteration
  reaches them. Nothing special is needed: `last_use_index` already returns the loop's index.
- **An `if`/`when` branch.** A use in one arm is a use of the `if` statement, so the borrow is live
  up to the branch point and through the whole statement. This is coarser than the brief's "and
  only in that branch afterwards": a mutation of the owner in the *other* arm is still refused
  because the borrow is treated as live for the whole `if`. That is sound (late release only
  over-refuses), it is what the accepted fixture `nll_if_branch_then_mutate` needs (the mutation
  is after the `if`), and branch-precise release would need the release point to be a set, not one
  index, with a merge rule. Recorded as the one deliberate coarseness.
- **`return`/`break`/`continue`.** `return` is a statement like any other: the borrow ends after
  it on that path. Kelp has no `break`/`continue`; a loop body that always returns is handled by
  the existing `diverges` logic.
- **`defer`.** The lowering (`defer.rs`) splices the action at the block end and before every
  `return` *before* this pass runs, so a deferred use is an ordinary statement mention and the
  borrow stays live to it.
- **A scoped block or closure capturing the borrow.** A scoped literal's captures are `Ref`
  nodes in the same statement, and the call the literal was written in is that statement, so the
  borrow is live for the call. An ordinary closure cannot capture a borrow at all (unchanged).
- **A returned borrow.** `check_return_borrow` still ties a declared borrowed return to the first
  reference parameter; returning a borrow rooted in a local is still refused by name. NLL does not
  weaken that check.

## What it still refuses (each a fixture)

- the owner mutated while the borrow is used later in the same block (`nll_mutate_before_use`);
- a borrow used in a later iteration of its loop while the owner is mutated earlier in the body
  (`nll_borrow_used_next_iteration`);
- a borrow a scoped block captured while the owner is mutated (borrowed exclusively) by the call it
  was written in (`nll_scoped_block_mutates_owner`): the captured borrow lives for that call, so
  the exclusive argument aliases it. (A write *inside* the block body is refused earlier still, by
  the scoped-capture rule, because an owned capture is a shared borrow and `push` needs an
  exclusive one; that refusal is not this pass's.)
- a borrow of a local owner still returned from the function (`nll_borrow_returned_from_local`);
- a `defer` that uses the borrow after the owner was mutated (`nll_defer_use_after_mutation`);
- a `View` (or `str`) made from a `String` used after the `String` is reassigned
  (`nll_view_after_reassign`).

## What changed in an existing fixture

None: every existing borrow fixture keeps its outcome. `grow_while_borrowed`,
`view_pins_owner`, `borrow_conflict` and `move_while_borrowed` all still use their borrow after
the mutation, and `view_released_at_scope_end` was already accepted through an extra `{ }`, which
still works (the block's own `pop_scope` releases it).
