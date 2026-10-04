# A-11 completion: places, borrows and lifetimes

*Plan of record, opened 2026-10-04. A-11 is expanded from the completed
partial-move, last-use and provenance slices to cover the remaining ownership
expressiveness work. The goal is to remove conservative refusals only where a
source-level ownership and lifetime argument is explicit; it is not a general
relaxation of moves or aliasing.*

## Exit criteria

1. **Complete (untested):** a `mut` reference cannot be forwarded to two
   exclusive parameters in one call. `refused_mut_param_reborrow_twice.kp`
   pins it, with a paired shared-reborrow acceptance fixture.
2. **Implemented, pending execution:** a move from a nested owned place has one projection path, clears exactly its
   final owning slot, and makes every prefix unusable as a whole until that
   path is reinitialised. Both backends and ASAN/valgrind must cover a nested
   record, a conditional move and a reinitialisation.
3. A temporary that supplies a returned borrow is promoted to a named compiler
   temporary whose lifetime is exactly that of the binding receiving the
   borrow. Escapes, reassignment, branches and `defer` must each be tested.
4. Stored borrowed values (`str`, `View`, `&T`, and borrowed `dyn`) carry an
   explicit owner-root set in their containing declaration. The checker must
   reject construction when no owner in the same aggregate outlives the field,
   reject mutation/move of a live owner, and reject every return or container
   insertion that would erase the relationship.
5. The existing no-alias rules remain: a borrow never permits a move from its
   referent, and a field through a shared or exclusive borrow is not moved by
   value. The added capability is lifetime tracking, not moving another
   frame's value.

## Order

1. **Call reborrows** — completed first because it is a soundness hole and
   local to `borrowck.rs`.
2. **Projection paths** — extend `Projection`, the move state and both backend
   clear operations before accepting any nested move in inference.
3. **Temporary promotion** — model the compiler-generated owner as a normal
   binding and reuse last-use releases; no special scope exception.
4. **Aggregate borrows** — add owner annotations to TIR declarations, then
   allow only intra-aggregate borrows whose owner is a stable field or
   parameter. No self-referential aggregate, heap relocation or implicit
   lifetime inference is admitted.
5. **Audit** — update `syntax.md`, roadmap A-11/H rows, and all refusal tests;
   run the full LLVM and C-oracle, sanitizer and differential gates.

## Non-goals

- General explicit lifetime syntax or inference across arbitrary function
  boundaries.
- Moving an owned field or enum payload through a borrow.
- Self-referential records, borrowed container elements, or a stored closure
  capturing a borrow unless their owner relationship has a separately accepted
  representation and drop argument.

The dependency in step 4 is intentionally narrow. If a desired API needs one
of the non-goals, it becomes a new design decision instead of silently
expanding A-11.
