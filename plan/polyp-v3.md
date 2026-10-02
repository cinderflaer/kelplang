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
        polyp (lib.kp)         the v1/v2 surface an application imports: column { },
             |                 row { }, keyed { }, widgets, class, css, run, stats,
             |                 schedule_*
             v
        core.kp  (hosted, pure Kelp: no GTK, no extern, no unsafe)
        ----------------------------------------------------------
        Node<M>, Children, Key, Opts, kind constants, Props, Path/Seg, Patch
        diff(old, new) -> List<Patch>          the matching rules, pure
        trait Backend                          one interface, no `M` in it
        apply_patches(backend, patches)        the one walk every backend delegates to
        run<B: Backend, S, M>(...)             the Elm loop, the ids, the counters
        Event, Stats, the focus check
             |
      +------+-----------------------------------+
      |                                          |
   gtk_backend.kp (hosted, GTK)        headless_backend.kp (hosted, pure Kelp)
   the live GTK widget tree,           an in-memory retained tree, the same
   real signals, focus,                counters, a scripted event list and a
   gtk.kp + queue.kp + the C shim      text dump of the tree
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

Matching is v2's, with one tightening: a new child with a key takes the first unused old child
with the same key, and a new child without one takes the old child at the same index when it is
unused, of the same kind, **and unkeyed too**. v2 also let an unkeyed new child take a keyed old
one at its index, which changes a node's identity from a key to a position and so cannot be
expressed as a stable path; the wider rule was dropped rather than given a second path field.
A duplicate key among one container's children panics with the key, before any widget is touched.

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
    fun mut Self.apply(patches: &List<Patch>)     // one line: core.apply_patches(self, patches)
    // The four widget operations the shared walker calls. A backend turns a path into its own
    // node; `at` is the place in the parent, and the parent is the path without its last segment.
    fun mut Self.make(path: &Path, at: i32, kind: i32, props: &Props)
    fun mut Self.patch_widget(path: &Path, props: &Props)
    fun mut Self.drop_widget(path: &Path)
    fun mut Self.place_widget(path: &Path, to_index: i32)
    // What the loop needs from a display.
    fun mut Self.pump()                           // GTK: one main-context iteration
    fun mut Self.poll() -> Event                  // Post(id) / Edit / Focus / Quit / Idle
    fun Self.path_of(id) -> option.Option<Path>
    fun Self.focus() -> option.Option<i32>
    fun mut Self.set_focus(id)
    fun mut Self.set_entry_text(id, text: &str)   // through the display's own signal
    fun Self.entry_text(id) -> String
    fun Self.entry_ids() -> List<i32>             // entries in tree order
    fun Self.entries_built() -> i32
    fun Self.stats() -> Stats
}
```

`apply` is implemented once per backend as a one line delegation to `core.apply_patches`, so the
walk that turns a path into a widget call, and the id rule, live in one place and cannot drift
between backends. What each backend writes is the widget operations on *its own* retained tree (a
live `Mount` for GTK, an `HNode` for the headless one) plus the events its display produces. The
loop `run<B: Backend, S, M>(backend: mut B, init: move S, view: fun (S).R: Node<M>,
update: fun (mut S, M)) -> S` owns the state, the path-to-id table, the focus check and the
counters, and it names no backend.

This is the trait form the design asked for, and the language handles it: a trait method may take
`&List<Patch>` and return `option.Option<Path>`, a generic function may take `mut B` and call the
bound's methods, and an impl may live in the type's own module (`gtk_backend.kp`) while the trait
lives in `core.kp` (`tests/projects/traits_cross` is the same shape). Three constraints were
found, none of them a change of plan:

- A *default* method whose signature names a type of the trait's own module is resolved again in
  the implementing module and does not see it, so `apply` is written out per backend instead of
  defaulted; the shared walker makes that one line.
- `option.Option<T>` is reached as `option.Option<T>`, its variants match by their bare names, and
  a module that names it imports `std/option`.
- A capability is checked at the *calling* module, so the facade `lib.kp` has to delegate `ui` to
  `gtk_backend.kp` (`impl "deps/polyp/gtk_backend" caps(ui = ["*"])`); v2 needed no clause because
  the GTK calls were in the same module the application granted.

## The facade, and what an application imports

`lib.kp` stays the module an application imports: `polyp.column { k -> ... }`, the plain
constructors, `polyp.run`, `polyp.css`, the `schedule_*` helpers and the counters, each one a thin
forward to `core` or `gtk_backend`. Kelp has no type re-export, so the tree type is spelled
`core.Node<M>` in a `view` signature and the examples import `deps/polyp/core` as well; that is the
one source change v3 asked of them, and both smoke scripts' assertions are untouched. The
`[extern.*]` sections an application pastes are unchanged, so no `coral.toml` changed.

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
