# A reference binding remembers where it was borrowed from (borrow checker 2 of 2, open item 33)

*Design note for the task that closes `roadmap.md` open item 33. `borrowck.rs` is the
code; this is the rule, the algorithm, and the shapes that stay refused. It builds on
`plan/nll.md` (a bound borrow ends at its last use), which is slice 2 of A-11.*

## The problem

`var cur = &list; cur = cur.next.get()` used to be refused with "cannot assign to 'cur'
while it is borrowed". The borrow of `cur.next` was registered against the binding `cur`
(the pointer) rather than against `list` (the owner), so rebinding the pointer looked
like it would invalidate a borrow of its pointee. It does not: rebinding a pointer is
safe, and a recursive structure could therefore only be walked by recursion.

## The rule

A binding whose type is a reference (a shared or exclusive borrow, a `View`, a `str`, a
`&dyn`) carries a **root set**: the owners (places) its current value may borrow from.
The empty set means "no recorded provenance", and the binding's own name is then its
root.

- A `val`/`var` initialization sets the root set from the initializer's root(s).
- `cur = <expr>` REPLACES the root set with the new expression's roots, so rebinding
  ends the old borrow relationship at that point and creates the new one. The old
  roots' pins are released there; the new roots are pinned from the assignment to the
  binding's last use in the block that declared it.
- A branch (`if`/`when`) whose arms assign different roots joins them: the root set
  after the join is the union, and every root in it stays pinned.
- A loop body that assigns a different root is handled the same way, by union: the
  root set after the loop contains every root an iteration could have produced. That
  is the conservative answer and needs no fixpoint iteration.

`borrow_root` of an expression `cur.field...` where `cur` is a reference binding
resolves to the roots of `cur`, not to `cur` itself. A borrow made through the cursor
is therefore a borrow of the **owner**: it conflicts with mutating the owner (kept
refused) and does not block reassigning the cursor (now accepted).

## The algorithm

- Each `VarState` carries `roots: Vec<String>`; each scope-durable borrow entry
  (`ScopeBorrow`) carries the `holder`: the reference binding that keeps the borrow
  alive, or `None` for a temporary a statement produced.
- Borrow pins (shared count / exclusive) live in one frame-wide table keyed by owner
  root, not on the root's `VarState`. That is what lets a retarget inside an inner
  block reach a root declared in an outer one, and it is what makes the branch merge a
  matter of recomputing pins from the merged entries.
- On a declaration the initializer's fresh entries are tagged with the new binding as
  holder and given a release point: the binding's own last use in its block (the NLL
  rule `plan/nll.md` already computes). A root the initializer did not borrow (an alias
  like `val q = p`) gets a fresh pin and entry here.
- On a reassignment of a reference binding, the entries it holds are released, the new
  root set is computed from the value, and fresh entries are **adopted into the scope
  that declared the binding** (a retarget in a loop body or an inner block must not be
  released when that block ends). The release point is the old one, or `usize::MAX`
  when the binding is new here.
- `Scope::merge` unions the two branches' root sets for each binding and unions their
  borrow entries by (holder, root, mutable); `merge_into` recomputes the frame-wide
  pins from the merged entries. A binding that one arm points at A and the other at B
  therefore ends up with both pinned.
- A statement's temporary borrows are released by holder: entries with a holder
  survive the statement (`release_unheld_since`), so the borrow a cursor assignment
  stored is not released at the end of the assignment's own statement.

## Soundness (each rule has a fixture)

- While any cursor with root `R` is live, `R` cannot be mutated, moved or dropped:
  the pin is on `R`, and `reinit`/`use_value`/`start_borrow` all consult it.
- Two cursors into the same root are fine for shared borrows (the pin is a shared
  count); a `mut` cursor into `R` excludes every other borrow of `R` (the pin is
  exclusive).
- A cursor may never outlive its root: returning a cursor derived from a local is
  still refused by `check_return_borrow`, which now compares root *sets*.
- Reassigning a cursor to a borrow of a different owner switches its root and releases
  the old one at that point, so the old owner is mutable again as soon as the cursor is
  rebound (the old value is dead there).

## What it still refuses (each a fixture)

- mutating a list while a cursor into it is used later (`cursor_mutate_while_used`);
- two `mut` cursors into one root (`cursor_two_mut_cursors`);
- aliasing one cursor into another and then mutating the owner
  (`cursor_alias_reassign`): the aliasing binding gets its own pin, so the owner
  stays borrowed after the first holder is gone;
- mutating an owner a joined value may point into, from either half of a
  value-position `if` (`cursor_branch_join`, `cursor_branch_join2`) and from the
  arm that assigned a cursor (`cursor_branch_union`);
- returning a cursor derived from a local (`cursor_returned_from_local`);
- reading a cursor after its root was moved: the move is refused
  (`cursor_root_moved`), because the cursor keeps the owner borrowed.

One rule got stricter as a consequence, and it is a fixture too: copying an
exclusive borrow into a second binding (`val q = p` where `p: &mut T`) is
refused (`cursor_mut_alias`), because the second binding would need a second
exclusive pin. Kelp's references are `Copy`, so this was the one way to make two
live `&mut` aliases to one place; the hold-every-root rule closes it by refusing
the copy instead of allowing it.

## What changed in an existing fixture

None. Every existing borrow, NLL and view fixture keeps its outcome; the two existing
refusals that name a returned borrow (`nll_borrow_returned_from_local.kp`) and a
reassigned owner (`nll_view_after_reassign.kp`) produce the same messages.

## What stays out

Explicit lifetime syntax (`'a`) stays out, as decided. A `View`/`str` in a `datac`
field, a borrow in a container, and a borrow returned from a temporary are still
refused for the same reason: there is no written lifetime to tie them to their owner.
This slice makes the *existing* elided-borrow rule track provenance through a cursor;
it adds no new lifetime surface.
