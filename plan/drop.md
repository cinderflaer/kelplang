# User `Drop` (kernel stage 6, MK9-S6-M1, the second half of B-8)

*Design note. `language/kernel.md` (kelp wiki) orders the stages; `language/statics-and-defer.md` (kelp wiki) is the other half of
row 6. This one gives a user type a destructor the compiler runs at every drop point the
`DropPlan` already has, so a kernel can couple a lock flag to the value that holds it.*

## The surface

```
datac Guard(val locked: bool)

fun mut Guard.drop() {
    LOCK.store(value = false, order = atomic.Ordering.SeqCst)
}
```

`fun mut T.drop()` on a user `datac`/`enum` is that type's destructor. It takes the value as an
exclusive borrow (`mut` receiver), takes no parameters, and returns nothing. There is no trait
keyword, no `impl` block, and no new punctuation: `impl` in this grammar already means "this
module imports that module" (`plan/syntax.md` 2.1), and the closest form the grammar does have
is the one A-12 slice 1 already chose for a declared `Map` key pair, where **the method name is
the declaration** (`fun T.hash()` / `fun T.eq(other: &T)`, `plan/syntax.md` 9.4). `drop` is the
same shape: one fixed name, one fixed signature, checked at the declaration.

## Semantics

- When an owned value of a `Drop` type reaches a drop point the `DropPlan` already records (block
  end, before a `return`, reassignment of an owned `var`, an element a container frees, the drop
  of a containing `datac`/`enum`, and the `atexit` drop of an owned `pd`), the generated drop
  calls `fun mut T.drop()` **first** and then drops the fields recursively, in declaration order
  (`enum`: the live variant's payload in declaration order). Fields are dropped by the same
  generated `kelp_drop_<T>` the type already had, so the user body is one extra call at the top
  and every existing drop point, container drop and recursive drop reaches it unchanged.
- A moved-out value is not dropped, exactly as before: a destructor cannot run for a value that
  has an owner somewhere else. A *partial* move of a destructor-typed field or enum payload is
  refused by name instead: the partial-move lowering zeroes the slot the value came from, and the
  base's own drop would then run the destructor a second time on the empty slot. Take the value
  whole (`val g = holder`), or use it through a borrow.
- A `Drop` type is not `Copy`. User types are never `Copy` in v1, so the rule holds by
  construction, and the compiler enforces the consequence that matters: a type with a destructor
  is excluded from every derived-copy fixpoint (`clonable_types`, `keyable_types`), so
  `List<Guard>.clone()`, `List.slice`, `Map.values`/`get_or` and a `Map` key of a `Drop` type are
  refused rather than silently producing two owners of one resource.
- An explicit destructor call is an error: `g.drop()` is refused by name, because the value's
  drop already runs when the value dies and a second run would free a live resource twice.
- Containers work through the same function: `List<Guard>` (element drop), `Box<Guard>`,
  `Map<.., Guard>` (value drop), `Option<Guard>`-shaped enums (variant payload drop), and a
  `Guard` inside a `datac` inside a `List`. A closure that captured a `Guard` owns it in its
  environment and drops it where the environment is dropped.
- An **owned trait object**, `Box<dyn Name>` (A-12 slice 7, `plan/traits.md` 12), drops through
  its vtable: the box runs the vtable's drop entry, which is the concrete type's own
  `kelp_drop_<T>` (so the user `drop` and then the fields, exactly as a `Box<T>` would), and then
  frees the allocation. The concrete type is erased at the drop site, which is why the drop is a
  vtable entry rather than the per-element function a `Box<T>` calls. Everything else is
  unchanged: a `List<Box<dyn Name>>` frees each element through the generated
  `kelp_box_dyn_drop__<Trait>`, a `datac` field of that type is dropped by its own generated drop,
  and the user `drop` of the concrete types runs in reverse declaration order with the rest.
- `defer` and `Drop` are independent: a `defer` is spliced before the block's ordinary drops, so
  on an ordinary exit a deferred action runs before the value's destructor. On a `panic` neither
  runs: there is still **no unwinding** (`plan/mk9-memory-model.md` 2), the frame is abandoned,
  and the leak is the same one that document already admits. `language/unsafe-ub-catalogue.md` (kelp wiki) carries
  the stage's section.

## How it is implemented

- `infer.rs` recognises `fun mut T.drop()` while it builds one signature per declaration, checks
  the shape there (an exclusive receiver, no parameters, no return), and records the set of
  destructor types on the typed program (`tir::Program::drop_types`), the same way
  `declared_keys` records a declared key pair. The method body is an ordinary function, emitted
  by both backends with the type's own `mut` receiver.
- `types::owned_types` seeds its owned set with the destructor types, so a destructor type has a
  `kelp_drop_<T>` even when every field is a scalar. `needs_drop` then answers true, which is the
  whole interface the drop plan, the containers and the `atexit` pd drop already use.
- `codegen_c.rs` and `codegen_llvm.rs` both call the user drop at the top of the generated
  `kelp_drop_<T>`: the C backend calls the mangled `kelp_<T>__drop(v)`, the LLVM backend calls
  the `LlvmFunction` `declare_functions` already declared for it. Nothing downstream of the
  drop points changes.
- `borrowck.rs` refuses a partial move of a destructor-typed field or payload (`FieldAccess` and
  `EnumField` whose `expr.ty` is a destructor type), which is the one move shape the generated
  drop cannot make correct on its own.

## What the compiler does not check

A destructor's body is not checked for running an operation that itself drops a value of the
same type, for being idempotent, or for being panic-free. A destructor that panics is a panic
from the drop point, with no unwinding to run the remaining drops of the frame. A destructor
referenced by a passdown reads the generated global directly, the way `main` does, so it takes
no hidden passdown parameter. A partial move of a *non*-destructor field out of a destructor
type is still allowed when the field itself is not a destructor type (a `Copy` read, or a
`String` field, whose zeroed slot drops as an empty value); only the destructor-typed slot is
refused. Nothing about the panic path is new: a value that a panic abandons leaks.
