# Polyp v3: a backend-neutral core

*Design note for `examples/polyp/` v3. Not a Mark; `roadmap.md` owns the schedule, and
`plan/polyp-v2.md` is the v2 record this one starts from. Stage 1 of 5 is done: the core is split
out and the GTK code became one backend behind an interface. Later stages append their own
sections at the end.*

## Why

v2 fuses three things in one module: the Elm loop, the keyed tree diff and the GTK calls. The goal
of v3 is a Polyp that draws its UI on any platform, including a custom operating system with only
a framebuffer, with a predictable per-frame cost. That needs the answer to "what changed" to be
pure data, testable with no display, and the answer to "how is it shown" to be one replaceable
implementation of an interface.

## The layering

```
        application            view(S) -> Node<M>      update(mut S, M)
             |
        polyp (lib.kp)         the v1/v2 surface the examples use: column { }, row { },
             |                 keyed { }, widgets, class, css, run, stats, schedule_*
             |
   +---------+---------------------------+
   |                                     |
 core.kp  (hosted, pure Kelp)      headless_backend.kp (hosted, pure Kelp)
   Node<M>, Children, Key, Opts      an in-memory retained tree, the same counters,
   Kind, Props                       a scripted event list, a text dump
   Path/Seg, Patch
   trait Backend
   diff(old, new) -> List<Patch>
   run<B: Backend, S, M>(...)        gtk_backend.kp  (hosted, GTK)
   apply_patches(backend, patches)     the live GTK widget tree, the SAME counters,
   Event, Stats                        real signals, focus, gtk.kp + queue.kp + the C shim
```

`core.kp` holds the loop, the tree and the matching. It names no GTK symbol, has no `extern`
call, no `unsafe` and no libc beyond what hosted `std` already needs, so it is also the module a
future raster or soft backend reuses unchanged. A backend holds widgets and knows nothing about
`M`: it never sees a `Node`, only the plain `Props` a patch carries, and the events it produces
carry only integers and paths.

## The `Patch` type

`diff(old: &Node<M>, new: &Node<M>) -> List<Patch>` is a pure function of two trees. It emits one
of four patches:

```
Create(path, kind, props)     a node the old tree did not have
Update(path, props)           a matched node whose properties changed
Remove(path)                  a whole subtree the new tree does not name
Move(path, to_index)          a matched keyed node whose position changed
```

- `path` is a `Path`, a list of `Seg` from the root, whose last segment is `Index(i)` for an
  unkeyed child and `Keyed(i, key)` for a keyed one. The identity of a node is its key when it
  has one and its position otherwise, so a keyed node's path is stable across a reorder while an
  unkeyed one's is stable for as long as positional matching keeps it (a matched unkeyed new
  child is always at the index its old partner was, `plan/polyp-v2.md`'s rule). `Update`, `Remove`
  and `Move` name nodes that already exist, so their paths are the ones already live.
- `kind` is one of the `KIND_*` integers `gtk.kp` used, unchanged: the backend maps them to
  whatever its platform calls the widget, and the core never asks what they mean.
- `props` is a `Props` datac, plain data: `text`, `size`, `bold`, `label`, `checked`, `fraction`,
  `spacing`, `height`, `margin`, `expand`, `align` and `classes`. A backend never inspects `Node`
  itself, which is what lets it be `M`-free, and the reason `Node` can carry closures while a
  patch cannot.

Matching is v2's, unchanged: a new child with a key takes the first unused old child with the
same key; a new child without one takes the old child at the same index when it is unused and of
the same kind. A duplicate key among one container's children panics with the key, before any
widget is touched.

### The order patches are emitted in

The order is what makes a path valid at the moment its patch is applied, so it is part of the
design rather than an accident. For each container, and depth first:

1. `Update` for every matched child, in new order, each followed by that child's own subtree;
2. `Remove` for every unmatched old child, in descending old index (so a lower index is still the
   index it was, and a keyed removal cannot shift one);
3. `Create` for every unmatched new child, in new order (the patch's index is the position the
   node has in the new tree);
4. `Move` for matched children whose position changed, in increasing new index, and only when the
   matched sequence is not increasing (a monotonic match already keeps its relative order).

### Widget ids

A widget that can post an event needs an identity that survives a patch, and a path is that
identity. Both sides derive the same integer id from the same rule: the nth `Create` patch of an
interactive kind (`Entry`, `Button`, `Checkbox`) in one run has id n, ids only grow and are never
reused. The core keeps the id to path map that turns a posted id back into a node; the backend
keeps the id to widget map that turns it into a signal. Ids are what the v2 smoke runs already
lean on when they schedule a raw post.

## The `Backend` interface

```
trait Backend {
    fun mut Self.apply(patches: &List<Patch>)   // one line: core.apply_patches(self, patches)
    // The primitives the shared walker calls. `parent` and `at` place the widget in its box.
    fun mut Self.make(id, parent, at, kind, props: &Props)
    fun mut Self.patch(id, props: &Props)
    fun mut Self.drop(id)
    fun mut Self.place(id, parent, at)
    // What the loop needs from a display.
    fun mut Self.pump()                          // GTK: one main-context iteration
    fun mut Self.poll() -> Event                 // Post(id) / Edit / Focus / Quit / None
    fun Self.focus() -> Option<i32>
    fun mut Self.set_focus(id)
    fun mut Self.set_entry_text(id, text: &str)  // through the display's own signal
    fun Self.entry_text(id) -> String
    fun Self.entry_ids() -> List<i32>            // entries in tree order
    fun Self.path_of(id) -> Option<Path>
    fun Self.stats() -> Stats
}
```

`apply` is implemented once per backend as a one line delegation to `core.apply_patches`, so the
walk that turns a path into an id, allocates an id for a create, retires the ids of a removed
subtree and calls the primitives lives in the core and cannot drift between backends. A backend
therefore writes widget code only.

The loop `run<B: Backend, S, M>(backend: mut B, init: move S, view: fun (S).R: Node<M>,
update: fun (mut S, M)) -> S` owns the state, the slot table and the focus check, and it names no
backend. This is the trait form the design asked for, and the language handles it: a trait method
may take `&List<Patch>` and return an `Option<Path>`, a generic function may take `mut B` and call
the bound's methods, and an impl may live in the type's own module (`gtk_backend.kp`) while the
trait lives in `core.kp` (`tests/projects/traits_cross` is the same shape). The single constraint
found is that a *default* method whose signature names a type of the trait's own module is
resolved again in the implementing module and does not see it, so `apply` is written out per
backend instead of defaulted; the shared walker makes that one line.

## The five stages

1. **Core and backends (this stage).** `core.kp`, `gtk_backend.kp`, `headless_backend.kp`,
   `lib.kp` as the facade, the pure diff tests and the headless run of the todo program, on both
   compiler backends, with the two GTK smoke scripts unchanged.
2. **Raster.** A `Canvas` abstraction the core's `Props` can be drawn into: a framebuffer, a
   surface, or a software pixel buffer, with layout as data. No widget toolkit.
3. **Soft backend and input.** A pure-Kelp backend that owns the layout and draws through the
   raster, plus an input decoder that turns key and pointer bytes into `Event`s, so a Polyp program
   runs with no display server at all.
4. **Freestanding proof.** The core and the soft backend in a `[target]` freestanding project that
   boots under QEMU and draws to memory, which is the "custom operating system with a framebuffer"
   case. LLVM only, as freestanding always is.
5. **Performance.** Per-frame cost: no `Props` allocation for an unchanged node, a per-tree pass
   instead of a per-node one, a measured budget for the diff and the patch walk, and a benchmark
   fixture.

## What v2 kept

- The public surface: `polyp.column { k -> ... }` and the trailing-block builders, `keyed`,
  `keyed_int`, `with_key`, `spacing`, `margin`, `expand`, `align`, `class`, the plain
  constructors, `polyp.css`, `polyp.run(title, width, height, init, view, update)`, `stats`,
  `focus_present`, `focus_survived`, `entries_built`, `entry_text`, and the `schedule_*` calls.
  `lib.kp` is a thin facade that forwards to `core.kp`.
- The counters: created, updated, removed and live are the backend's, reported after each apply,
  and one entry widget built across a run still means one.
- Focus: the loop reads the focused widget before a patch and after it and checks it is the same
  widget, so a patch that drops it is a finding rather than an assumption.
- A matched entry whose text differs is written by the patch, through a signal-blocked write, and
  a focused entry is left alone, exactly as v2.
