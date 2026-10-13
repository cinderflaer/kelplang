# Polyp v2: keyed children and a real patch

*Implementation note for `examples/polyp/` v2. Not a Mark; `meta/roadmap.md` (kelp wiki) owns the schedule. The API
surface is v1 plus what is listed here; `plan/polyp-v3.md` is the later split of the library into a
backend-neutral core and one module per backend, which keeps this design and the counters it
describes.*

## What changed

v1 rebuilt the whole tree on every state change and had one identity rule: the `i`-th `Entry`
reused the `i`-th entry widget. v2 keeps the live GTK widget tree and **patches** it.

- A child may carry a **key** (`Key.Str` or `Key.Int`). Inside one container the keys must be
  unique; a duplicate **panics**, naming the key (`polyp: duplicate key 'task-7' among the children
  of one container`). Matching is by key when the new child has one, and by position and widget kind
  otherwise.
- A matched widget is **updated in place**: label markup, button label, checkbox label and state,
  progress fraction, spacer height, box spacing, margins, expand, alignment and CSS classes, and
  an entry's text only when it differs and the entry is not the focus widget. A keyed child whose
  position changed is reordered (`gtk_box_reorder_child_after`), never rebuilt.
- An unmatched new child is created and inserted at its index (`gtk_box_insert_child_after`); a
  child the new tree does not name is removed (`gtk_box_remove` or a parent `set_child`) and its
  slots are retired.
- Because matched widgets are the same GTK objects, **widget identity, focus (any focused widget),
  selection and scroll position survive a patch**. The loop checks that the widget focused before a
  patch is still focused after it (`polyp.focus_survived()`).

## Ids, and why a handler cannot post a stale one

v1 ids were tree positions: the `i`-th `Button` in a depth-first walk, and an entry's id was its
ordinal. That cannot survive a patch: a widget that stays while its position changes would post an
id that names a different `Msg`.

v2 ids are **slots**. The loop owns one grow-only `List<Slot<M>>` for the whole run; a button or
checkbox takes the next slot index at creation and its handler closure captures that index forever.
An entry's slot also holds its constructor and its live widget handle. Every patch writes the new
`Msg` (or constructor) into the slot of each matched widget and leaves the id alone, marks the slots
of removed widgets `Dead`, and never reuses an index. So a handler always posts the id it was born
with, the loop finds the `Msg` the *current* tree put there, and an id whose widget was removed
resolves to `Dead` and is dropped: **a handler can post an id, never a stale message**. The loop
takes a slot out (`remove` + `insert` a `Dead` in its place) before `update`, so the value it hands
`update` is owned.

Consequence for tests: the counter's scheduled posts still hit `Inc` then `Dec`, because slots are
handed out in creation order and the counter's two buttons are the first slots.

## Widgets and layout added

`scrolled`, `checkbox(label, checked, msg)`, `progress(fraction)`, `separator()`, the container
options `spacing`, `margin`, `expand`, `align(Align)`, `class(name)`, and the app-level
`polyp.css(text)`, which loads a `GtkCssProvider` on the default display at
`GTK_STYLE_PROVIDER_PRIORITY_APPLICATION`. All of it is ordinary GTK; the manifest symbol list in
the README grew by exactly the calls those need. `Node` now carries a key per child and a spacing
per container, so the plain constructors wrap their `List<Node<M>>` into `Child` values.

## Still not there

- **No lazy or virtualized lists.** Every child in the model is a live widget; a list of 10,000 rows
  builds 10,000 widgets.
- **No animation and no transitions.** A patch sets the new property in one step.
- **No theming beyond CSS.** `polyp.css` is the only hook; there is no palette, no dark-mode
  switch, no per-widget style objects.
- **No platform but GTK on Linux.** The library is a GTK binding, not an abstraction over one.
- **Ids are slots, so they are stable but internal.** An application that posts a raw id (rather
  than carrying a `Msg`) still leans on the creation order; the smoke runs do, deliberately.
- **No exclusive captures.** A builder block still reads the borrowed state and cannot write it.

## Language gaps found

None that blocked this task. Two shapes needed a workaround worth recording:

- A field read by value out of a `mut` receiver (a method's `self`) is a move out of a borrow, so
  the pending key and options the builder carries live in `Box`es and are `take()`n, not read.
- A `pd var` counter written by a call chain conflicts with itself (two functions cannot hold one
  passdown exclusively), so the patch counters are `pd val AtomicI32` cells incremented through
  their shared-borrow operations. This is the shape `syntax.md` 12.5 recommends and it cost
  nothing to write.
