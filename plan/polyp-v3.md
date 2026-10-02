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

## Stage 2: the software rasterizer

*Stage 2 of 5. `examples/polyp/soft/` is the module family this section designs; stage 3
builds the soft backend on it and stage 4 proves it freestanding.*

Stage 2 is the layer stage 3's soft backend draws through, and it is also the first thing in
Polyp that has to survive a bare-metal build. The hard constraints are therefore fixed before
any of the API: **integer only** (no `f32`/`f64` anywhere, so no soft-float libcall on a
riscv64 target), **no libc and no OS call** (the only external facts are a pixel pointer and a
stride the platform hands over), **no heap allocation in any drawing function** (scratch is
the caller's, a fixed `static` or a stack array), and **only the modules that touch raw pixels
carry `unsafe`**. `plan/freestanding.md` is what stage 4 may use; nothing in the four core
modules names a `std` module except `std/str`'s `len`/`byte`, which the freestanding backend
lowers itself.

### The modules

```
examples/polyp/soft/
  fb.kp      Fb, Rect, the pixel store/load, checksum            (unsafe: raw pixel access)
  draw.kp    ClipStack and every primitive                       (unsafe: raw pixel access)
  font.kp    the embedded 5x7 face, metrics and draw_text        (no unsafe)
  damage.kp  the 16-rectangle dirty list                         (no unsafe)
  debug.kp   dump_ppm, hosted only                               (unsafe: raw pixel reads)
```

The split is by what a module is allowed to do, not by size. `font`/`damage` do all their
geometry in integers over the `Rect`/`Fb` values `fb.kp` and `draw.kp` expose and never
dereference anything, so they need no audit entry; the three that read or write through a
`Ptr<u32>` are the only ones on a project's `[unsafe] modules` list. A consumer's manifest
therefore reads:

```toml
[unsafe]
modules = ["deps/polyp/soft/fb", "deps/polyp/soft/draw", "deps/polyp/soft/debug"]
```

and `font`/`damage` are deliberately absent. `debug.kp` is a separate file rather than a
`when` inside `fb.kp` so the freestanding build never names `std/fs`: a module that is not
imported is not compiled, and stage 4's project does not import it. It is also the one module in
`soft/` that allocates (a `Bytes` for the PPM it writes), which is fine because it is hosted debug
output, not a drawing function, and is never compiled freestanding.

### `Fb` and `Rect`

```text
datac Fb(val pixels: Ptr<u32>, val width: i32, val height: i32, val stride_px: i32)
datac Rect(val x: i32, val y: i32, val w: i32, val h: i32)
```

A pixel is `0xAARRGGBB`. `stride_px` is in pixels, not bytes, because the platform that hands
over a framebuffer thinks in pixels and the byte offset is `pixels.offset(y * stride_px + x)`.
`fb.new(pixels, width, height, stride_px)` is the constructor; a caller with a
`static mut [u32; N]` writes `val base: Ptr<[u32; N]> = unsafe { Ptr.from_mut(FRAME) }` and
then `fb.new(pixels = base.cast<u32>(), ...)`. There is no N-generic helper because Kelp has
no const generics (`[T; N]`'s `N` is a literal or a `const`, never a type parameter), and
`Ptr.from_ref(&buf).cast<u32>()` cannot be written as one chained expression (see the gaps
below), so the cast is a separate binding. That is the documented idiom, not a missing API.

`Rect` is total: every operation accepts an empty (`w <= 0 || h <= 0`), negative-sized or
off-canvas rectangle and answers something defined. `right()`/`bottom()`/`area()` widen to
`i64` first and `intersect`/`union`/`contains` compare `i64`s, so two rectangles at
`i32::MAX` do not wrap; the one narrowing (`clamp_i32`) saturates rather than panics.
`intersect` of anything empty is the empty rectangle, `union` of empty is the other operand.
`touches(a, b)` is true when they overlap or share an edge (a zero-width gap still merges),
which is the predicate the damage list coalesces on.

### The primitives

Every primitive takes `(fb, clip, ...)` where `clip` is a `&ClipStack`, and clips to
`clip.top() ∩ fb.bounds()` before it touches a pixel. Each returns the number of pixels it
wrote, so stage 5 can assert on it. No primitive allocates.

- `fill_rect`, `hline`, `vline`: an opaque store of the colour; the count is the area of the
  clipped rectangle.
- `stroke_rect(x, y, w, h, thickness)`: four **disjoint** bands (top, bottom, left middle,
  right middle), each clamped independently, so no pixel is written twice and the count is the
  area of the ring.
- `blend_pixel` and `blend_over(dst, src)`: source-over with an 8-bit alpha, exact and
  integer. Per 8-bit channel `c` of `src`/`dst`, with `a` the source alpha,
  `out = (src_c * a + dst_c * (255 - a) + 127) / 255` in `u64`. `a == 255` returns `src`
  exactly and `a == 0` returns `dst` exactly; both are properties the test asserts. All four
  channels are blended, alpha included.
- `blit_mask8(x, y, w, h, mask: Ptr<u8>, color)`: `mask` is `w * h` tightly packed coverage
  bytes; a byte `m` blends `color` with source alpha `(color.a * m + 127) / 255`, a byte `0`
  touches nothing and is not counted. This is the glyph path.
- `line`: the classic Bresenham error loop over the major axis, exactly one pixel per major
  step (`max(|dx|, |dy|) + 1` pixels), symmetric in sign, with a single pixel for a degenerate
  point. The test's independent reference is the closed form for the same set: for the major
  step `i`, the minor offset is `(2 * dminor * i + dmajor - 1) / (2 * dmajor)`, full-precision
  integer, which is Bresenham's "round to nearest, ties toward the start" written without an
  error accumulator.

### Rounded rectangles, without floats and without seams

`fill_round_rect(x, y, w, h, radius, color)` clamps `radius` to `[0, min(w, h) / 2]` and
computes one span per row. A row in a corner has an inset from the exact integer circle
predicate, in doubled coordinates so no division appears:

```text
pixel (i, row) of the top-left corner is inside iff
    (2 * (r - 1 - i) + 1)^2 + (2 * (r - 1 - row) + 1)^2 <= (2 * r)^2
```

which is the pixel-center test. `i = 0` is the outer corner, so the predicate cuts a quarter
disc out of each `r x r` corner square and the remainder of the row is a single span; there is
no float, no accumulated error and no second write anywhere. This is the "integer circle" the
stage asked for, realised as an exact squared comparison rather than the classic midpoint
error recurrence: the recurrence's half-step tie choice is not one closed-form predicate, and
the test's reference has to be a full-precision integer computation that agrees pixel for
pixel, so the exact predicate is the one both sides share. The corner row of radius 1 is
uncut (`inset 0`) and that is correct: a radius of one only shaves the corner pixel when the
doubled test says so. `stroke_round_rect` is the outer shape minus the inner shape at
`(x + t, y + t, w - 2t, h - 2t, r - t)`, computed as two disjoint spans per row, so the count
is exact and nothing is overdrawn. Both are tested to write nothing outside the bounding
rectangle (a sentinel background is checked outside it) and to write exactly the reference's
pixel count.

### The clip stack

```text
datac ClipStack(var rects: [Rect; 8], var depth: usize)
fun clip(rect: Rect).R: ClipStack
fun mut ClipStack.push(rect: Rect)
fun mut ClipStack.pop()
fun ClipStack.top().R: Rect
```

A push intersects the new rectangle with the current top, so the stack is monotone; `depth`
starts at 1 and never exceeds 8. Two API contracts panic rather than silently misbehave, and
they are the only panic fixtures this stage adds: pushing onto a full stack panics
`clip stack depth 8 exceeded`, and popping the base panics `clip stack is empty` (the base
rectangle is what makes `top()` total). Both are exercised by the test project under
`POLYP_SOFT_TEST=clip_depth` / `clip_empty`, with the process bounded like every other run.

### The font

A 5x7 face for printable ASCII 32..126, hand-drawn for this stage (not copied from a licensed
font), stored as one `static FONT: [u8; 95 * 5]`: five bytes per glyph, one per column, bit
`r` of a column byte is the pixel at row `r` with row 0 at the top. The glyph cell is 5x7 and
the advance is 6 columns; `text_width(&str, scale)` is `bytes * 6 * scale` (`i64` arithmetic,
saturated into `i32`) and `text_height(scale)` is `7 * scale`. `draw_glyph` scales the glyph
into a fixed `[u8; 5 * 4 * 7 * 4]` coverage mask (the 4x maximum) and hands it to
`blit_mask8`, so the font never touches a pixel itself and needs no `unsafe`. `draw_text`
advances the pen by `6 * scale` per **byte** and returns the total pixels written. A byte
outside 32..126 renders a fixed replacement box, and a multi-byte UTF-8 sequence renders one
box per byte; the limit is documented in the module and in the README. `scale` must be in
1..=4 and a call outside that panics `text scale must be in 1..=4`, which is the third and
last panic fixture.

### Damage tracking

```text
const MAX_DAMAGE: usize = 16
datac Damage(var rects: [Rect; 16], var len: usize)
fun new().R: Damage
fun mut Damage.add(rect: Rect)
fun mut Damage.clear()
fun Damage.count().R: usize
fun Damage.at(i: usize).R: Rect
fun Damage.bounds().R: Rect
```

The policy, in order, and it never allocates and never lets `len` exceed 16:

1. an empty rectangle is dropped;
2. if `len < 16` and the rectangle overlaps or touches an existing one, that entry becomes the
   union, then a coalesce pass merges any entries the union made touch, and the call returns;
3. otherwise, if `len < 16`, the rectangle is appended;
4. otherwise (the list is full and nothing touches), the pair `(i, j)` minimising the wasted
   area `union(i, j).area() - area(i) - area(j)` is merged, which drops `len` to 15, the new
   rectangle is appended, and a coalesce pass runs; ties on the wasted area break on the
   lowest `(i, j)` so the result is deterministic.

The coalesce pass is what makes the list small in the common case: after any change, touching
pairs are merged until none remain, and each merge strictly reduces `len`, so it terminates in
at most 15 merges. The alternative the stage allowed (collapse everything to one bounding
rectangle when full) is not used because the least-waste pair preserves more detail for the
same cap; `bounds()` exists for a caller that really wants the one-rectangle answer. The test
asserts the two invariants that matter: `count() <= 16` after every `add`, and every point that
was ever added is covered by the current list (checked per pixel over the whole test canvas,
not just rectangle containment).

### Determinism, checksums and the tests

`checksum(fb)` is FNV-1a 64 (offset basis `14695981039346656037`, prime `1099511628211`) over
the pixels row by row, ignoring `stride_px` padding, feeding each pixel's four bytes
little-endian (`b0 = p & 0xFF`, then `b1`, `b2`, `b3`). The byte order is fixed by masking the
`u32` value rather than reading memory, so the checksum is the same on a little- or
big-endian host. `debug.dump_ppm(fb, path)` writes a binary `P6` PPM through `std/fs`; it is
hosted-only, is compiled only when a program imports `debug.kp`, and the test writes it into
the cargo target directory and checks the header and one pixel's bytes.

`examples/polyp-soft-test/` next to the library carries the tests. They run in the plain
development shell with no display, and the Rust test builds it once and runs it on the backend
under test (LLVM by default, `KELPC_ORACLE=c` for the oracle), asserting the exact stdout:

- every primitive against an independent per-pixel reference implementation written in the
  test, with full-precision integers, over seeded LCG inputs: random rectangles (negative and
  zero sizes included), random clip rectangles (empty and off-canvas included), random
  endpoints for the line, random alphas for the blend, and explicit cases where the shape
  crosses each of the four edges of the clip and of the framebuffer;
- the round rectangle and its stroke additionally against a per-pixel predicate reference, with
  a sentinel check that nothing outside the bounding rectangle changed;
- text at scales 1, 2 and 3 against a reference that places the font columns itself, plus the
  replacement box for a byte outside ASCII;
- the damage invariants above;
- golden FNV checksums for three fixed scenes: `shapes` (fills, strokes, a line and a round
  rectangle), `text` (all three scales), and `blend` (an overlapping alpha blend), printed as
  three lines the Rust test pins.

The three panic contracts are the only rejection/panic fixtures, as above.

### What this stage found, and did not do

- Kelp has no method call with explicit type arguments on a *call expression* receiver:
  `Ptr.from_ref(&buf).cast<u32>()` is refused ("'cast' does not take type arguments"), while
  `val b: Ptr<[u32; N]> = Ptr.from_ref(&buf)` followed by `b.cast<u32>()` is accepted. The
  modules bind the receiver first; this is recorded in `plan/user-ffi-gap-analysis.md`.
- No const generics, so the "view a `static mut [u32; N]`" helper is `fb.new` plus a
  `.cast<u32>()` in the caller.
- No `Copy` for a `datac`, even one whose fields are all scalars, so `Rect` is copied by an
  explicit `dup()` or a constructor. The reference implementations and the primitive
  signatures are written around that.
- No `break`/`continue`, so the Bresenham loop and the circle boundary search exit through a
  flag; no `[]` subscript, so array reads and writes are `get`/`set`; and a statement ends at a
  newline, so an array literal or a `static` initializer has to be written on one line (the font
  table is one 475-number line).
- A borrow taken in an `if`/`while` *condition* is live for the whole statement, so
  `if fb.touches(rects.get(i = i), r) { rects.set(...) }` is refused as an exclusive borrow of
  what the condition already reads. The damage methods copy the element out first (`dup()`) and
  keep the test in its own statement; the check is not weakened, the shape is written around it.
- A `mut` parameter of a *`Copy`* array (`[u32; N]`) is passed by value rather than borrowed, so
  a mutable scratch image in the test is a `datac` field (`Img`/`Grid`) rather than a bare array.
  The drawing modules take `&ClipStack`/`&Fb` and never need a mutable array parameter.
- Deliberately absent: a general `Blend` operator beyond source-over (no multiply/screen), any
  gradient or pattern fill, sub-pixel positioning, and a colour type beyond `u32`. Stage 3
  consumes exactly the surface above and nothing more.

## Stage 3: the soft backend, layout, input and a platform

*Stage 3 of 5. `examples/polyp/soft_backend.kp` is the retained widget tree stage 1's interface
asked for, `examples/polyp/soft/event.kp` and `soft/platform.kp` are the input and display
boundaries, and the two platform implementations are `soft/headless_platform.kp` and
`soft/ppm_platform.kp`. Stage 4 boots exactly this pair on bare metal.*

The stage adds no display dependency and no libc: a program that imports it links no GTK, opens
no window, and draws through the stage-2 primitives only. The stage-2 constraints bind: integer
geometry, no allocation on the per-frame path, and `unsafe` only in modules a manifest lists.

### Modules

```
examples/polyp/
  soft_backend.kp          SoftBackend: the retained tree, layout, painting, input
  soft/event.kp            Event: what a platform feeds in (pointer, scroll, key, char)
  soft/platform.kp         trait Platform: present / now_ms / poll_event / framebuffer
  soft/headless_platform.kp  an in-memory framebuffer, scripted input, recorded presents
  soft/ppm_platform.kp     HeadlessPlatform plus a PPM dump when POLYP_DUMP_DIR is set
  soft/frac.kp             an f64's bit pattern as an integer permille (see below)
```

`soft/event.kp` and `soft/platform.kp` do no pointer work and are off a consumer's `[unsafe]`
list. `soft/headless_platform.kp` allocates the framebuffer through `Ptr.alloc<u32>` and
`soft/ppm_platform.kp` adds `soft/debug`'s PPM writer, so both are listed; `soft/frac.kp` reads
an `f64`'s bits through a `Ptr<u64>` and is listed on its own so the audit entry is one short
file. `soft_backend.kp` itself dereferences no pixel and is off the list: it calls the stage-2
primitives, which carry their own entries.

### The retained tree, and what grows

```text
datac SNode(
    var kind: i32, var slot: i32, var props: core.Props, var key: core.Key,
    var kids: List<SNode>,
    var uid: i32, var rect: fb.Rect, var measured_w: i32, var measured_h: i32,
    var dirty: bool, var laid: bool, var scroll: i32, var content_h: i32,
    var hover: bool, var pressed: bool, var caret: i32,
)
```

The tree is the same shape `headless_backend.kp` keeps, so the two backends cannot drift about
widget identity: an interactive widget's `slot` is handed out by counting interactive `make`
calls from zero, and a path is resolved with the same `core.child_at` / sibling-index rule. Only
`make` grows anything (one `SNode` and one `List` per created widget, in the widget's parent),
and only `List.push` inside `make`/`insert_into` allocates. `patch_widget`, `drop_widget`,
`place_widget`, `layout` and `paint` allocate nothing: damage is the stage-2 fixed `Damage`, the
clip is a fixed `ClipStack`, and the only per-frame scratch is the glyph mask `font.kp` already
builds on the stack. Two input handlers do own a temporary, both only when they run: `Tab`/
`Shift-Tab` collects the interactive ids into a `List<i32>`, and an entry edit builds the new
`String`. Neither is the per-frame path. A hosted run may therefore use `List` freely, and a
per-frame profile has no allocator call to find.

`uid` is a grow-only node number used only for tests and diagnostics; `slot` is the interactive
id the core knows.

### Layout: measure and arrange, integer only

Two passes over the tree. `measure` computes a node's intrinsic size bottom-up and caches it in
`measured_w`/`measured_h`; `arrange` assigns a rectangle top-down and places children. All
arithmetic is `i32`/`i64`; no float appears (see `soft/frac.kp` below for the one place the core
hands over an `f64`).

Metrics, one `Theme` value a platform can replace:

```text
pad 5, radius 3, border 1, gap 6, row_h 20, box 12, entry_min_w 90,
scrollbar_w 6, progress_h 8, sep 1, text scale 1 (1..=4)
```

Intrinsic sizes: `Text` is `font.text_width(text, scale)` by `font.text_height(scale)`; `Spacer`
is `0` by `props.height`; `Button` is `label + 2*pad` by `max(text_height + 2*pad, row_h)`;
`Entry` is `max(entry_min_w, text + 2*pad)` by the same height; `Checkbox` is `box + gap +
label` by `max(box, text_height)`; `Progress` is `entry_min_w` by `progress_h`; `Separator` is
`0` by `sep`; a `Column`/`Row` sums its children's sizes plus `spacing * (n - 1)` and takes the
maximum across the other axis; `Scrolled` measures its children as an implicit `Column` and adds
`2*pad + scrollbar_w` to the width, but its height asks for `content_h + 2*pad` capped at
`row_h * 6 + 2*pad`, so a long list does not push its own height and an `expand` child grows to
the leftover (a scroll area is flexible, like a GTK `ScrolledWindow`). A child's `margin` (when non
negative) is added on all four of its sides; `align` is `Fill` unless set; `expand` divides the
main axis' leftover among the expand children.

**The skip rule.** A patch marks only the node it wrote `dirty`. `measure` recomputes a node
only when its own `dirty` is set or a child reported a changed size, and it returns whether the
node's cached size changed. `arrange` runs on the whole tree only when the root's size changed,
the window was resized, a `Move` patch reordered children, or the tree was never laid out;
otherwise it is skipped entirely. A text change whose new string has the same pixel width
therefore re-measures one node and re-arranges none, which is what the layout-skip test counts.
When `arrange` does run and a node's rectangle differs from the one it had, both rectangles are
added to the damage list, so a real size change repaints the vacated and the occupied area and
nothing else.

### Painting, the theme and the class hook

Every widget paints through the stage-2 primitives with a default `Theme` (background
`0xFF1E1E24`, panel `0xFF2A2A33`, foreground `0xFFE6E6EA`, muted `0xFF9AA0A6`, border
`0xFF3A3A44`, accent `0xFF4C8DFF`, caret `0xFFFFFFFF`). A `Text` draws with
`font.draw_text`; a `Button` and an `Entry` are a rounded fill, a rounded stroke and centered
text; a `Checkbox` is a stroked box with an accent inner fill when checked plus its label; a
`Progress` is a panel track, an accent fill and a stroke; a `Separator` is a one-pixel rule; a
`Scrolled` is a panel fill with a border and a right-hand scrollbar thumb; a focused interactive
widget gets a one-pixel accent stroke.

`class("name")` resolves through a small override table: each entry names a class and the fields
it sets (`bg`, `fg`, `border`, `radius`, `pad`), and the classes on a node are applied in order
over the node's per-kind base style. A field a class does not set stays inherited, so
`todo-done`'s muted foreground reaches the `Text` children of the row it names. The table is code
comparisons, not data, because Kelp has no static `String`; the three shipped names are
`todo-app`, `todo-done` and `todo-pending`, and a program adds one by editing that function.

Painting is clipped to the damage list. `paint(fb)` takes the backend's accumulated `Damage`,
and for each rectangle builds a `ClipStack` at the intersection of that rectangle and
`fb.bounds()` and walks the tree, drawing only nodes whose rectangle intersects. It returns the
list it painted, which is the list the platform is handed. No damage means no walk and no
`present`: an idle frame presents nothing.

### Input

`soft/event.kp` is the only vocabulary a platform needs:

```text
enum Event {
    PointerMove(x, y)  PointerDown(x, y, button)  PointerUp(x, y, button)
    Scroll(dx, dy)     Key(code, mods)            Char(codepoint)
}
```

The backend hit-tests by walking the tree front to back with the scroll clip applied, and returns
the `slot` of the deepest interactive widget under the point. `PointerMove` changes the hover
slot and damages the old and the new widget's rectangle; `PointerDown` sets `pressed` and damages
it; `PointerUp` clears `pressed`, damages it, and posts the widget's id when the press and the
release named the same widget. A click focuses the widget it names. `Tab` and `Shift-Tab` walk
the interactive widgets in tree order and wrap; `set_focus` damages the old and the new focus
rect. `Scroll` adjusts the `Scrolled` under the pointer, clamps it to `[0, content_h - view_h]`,
and damages that area; `dx` is ignored (no horizontal scrolling).

A focused `Entry` keeps a byte caret. `Char` inserts the UTF-8 encoding of the codepoint at the
caret, `Key(BACKSPACE)` deletes the byte before it, `DELETE` the byte at it, `LEFT`/`RIGHT` move
by one, `HOME`/`END` go to the ends; each edit damages the entry's rectangle and posts the
entry's id, so the loop runs the program's `update` through the same path a GTK `changed` signal
uses. `Space` on a focused checkbox or button activates it, and on a focused entry types a space.
Every handler that changes a pixel adds damage and every handler that does not adds none.

The backend also keeps the programmatic channel the other two backends have: `schedule(event)`
queues one `core.Event` and `pump` releases at most one per frame, which is how the smoke
scenario drives the same `Focus`/`Edit`/`Post` sequence the GTK and headless runs use.

### The platform, and `frame`

```text
trait Platform {
    fun mut Self.present(fb: fb.Fb, dirty: &damage.Damage)
    fun Self.now_ms().R: u64
    fun mut Self.poll_event().R: option.Option<Event>
    fun Self.framebuffer().R: fb.Fb
}
```

`present` is handed the framebuffer and the exact rectangles that changed; the platform may
composite just those, or ignore the list. `now_ms` is the platform's monotonic clock in
milliseconds, used only for scripted input in the headless platform. `poll_event` drains input
oldest first and answers `None` when the queue is empty. `framebuffer` returns the `Fb` the
platform owns: pixels are `0xAARRGGBB` `u32`, `stride_px` is in pixels and may exceed the width,
and the backend writes only inside `width x height`.

```text
pub fun frame<P: Platform, S, M>(
    backend: mut SoftBackend, platform: mut P,
    tree: move core.Node<M>, state: mut S,
    view: fun (S).R: core.Node<M>, update: fun (mut S, M),
).R: core.Tick<M>
```

`frame` is the one function an OS calls per tick. It takes the platform's framebuffer, resizes
the backend's layout to it, lays out, drains every pending platform `Event` into the backend,
runs one `core.frame_step` (which drains the queued `core.Event`s and runs
`update`/`view`/`diff`/`apply`), lays out again for the patches, paints the accumulated damage
and hands it to `present`. It returns `core.Tick` (`Idle`/`Changed`/`Quit`, each carrying the
new tree) so the caller knows whether anything changed and when the program asked to quit.

`HeadlessPlatform` owns a `Ptr.alloc<u32>` buffer, records the presented rectangles and a
checksum after each `present`, and plays a scripted `(time_ms, Event)` list through `poll_event`
as `now_ms` advances; it is what the input and damage tests and the todo-soft run use.
`PpmPlatform` is the same plus `soft/debug.dump_ppm` to `<POLYP_DUMP_DIR>/frame_<n>.ppm` on every
present; with the variable unset it is a headless platform. `examples/polyp/PLATFORM.md` is the
plain-terms contract for a custom OS.

### The integer permille

`Props.fraction` is an `f64` because v2's `progress` takes one, and Kelp deliberately has no
float-to-integer conversion. A pixel fill width is an integer, so `soft/frac.kp` reads the
`f64`'s eight bytes through a `Ptr<u64>` and decodes the IEEE sign, exponent and mantissa with
integer shifts into a permille in `0..=1000`. It is ~20 lines, allocates nothing, and never
performs a float operation, which is what keeps a freestanding riscv64 image from needing a
soft-float libcall for the progress bar. The read is a type-punning load, legal only in the
module a manifest lists; `plan/kernel-ub-notes.md` records it. *Stage 4 removed this module:
`Props.fraction` is an integer permille and `Node.Progress`/`Children.progress` carry it, so the
core performs no `f64` comparison and nothing has to decode a float. See stage 4's "What this
stage found".*

### Programs and tests

`examples/polyp-todo-soft/` links `examples/polyp-todo/src/app.kp` unchanged (the same `view` and
`update` as the GTK and headless runs), imports `soft_backend`, schedules `app.script()` on the
backend, and drives `frame` in a loop, printing `app.report`'s summary plus the framebuffer
checksum after every frame. It must end with the same model and the same created/updated/removed/
live counters as the GTK and headless runs, because it runs the same `core.frame_step` body.

`examples/polyp-ui-test/` is the input and layout test program: hit-test and focus-order checks,
the entry editing sequence, a Space and a click activation, the wheel's move and clamp, the
damage assertions ("changing one label's text damages its own
rectangle and no other, and the pixels outside it are identical before and after"; "a hover
change damages only the hovered widget"; "an idle frame presents nothing"), the layout skip
counted with a measure/arrange counter, and the golden todo checksums at 400x300 and 800x600.
The Rust test builds it once and runs it on the backend under test, like stage 1's, with the
bounded runner stage 2 added.

### What this stage did not do

- No horizontal scrolling, no scrollbar dragging, no smooth scrolling: the wheel moves by a
  fixed step and clamps.
- No text selection, no clipboard, no IME, no multi-line entry: the caret is a byte offset and
  the font is byte-oriented, so a non-ASCII codepoint is one replacement box per UTF-8 byte.
- No bold face: `Text`'s `bold` flag is carried and ignored, because the embedded font has one
  weight.
- No incremental damage from a `Move` patch finer than the moved node's old and new rectangles,
  and no partial repaint of a widget whose own pixels changed without a size change beyond that
  widget's rectangle (a `Text` change damages exactly its own box).
- No `f32`/`f64` anywhere in the paint path, per stage 2; the one `f64` the core hands over is
  decoded as bits.

### Language shapes this stage worked around

Four, all recorded in `plan/user-ffi-gap-analysis.md`; none is a compiler change:

- A `mut` parameter of an enum type cannot be reassigned in place, so `core.frame_step` takes the
  tree by value and returns it inside a `Tick` variant rather than writing through `mut
  Node<M>`.
- A borrow taken in an `if`/`while` condition is live for the whole statement, and a method
  receiver is borrowed before its arguments are evaluated, so `self.damage.add(rect_of(self...))`
  is refused; the code binds the rectangle to a local first.
- A `mut S` parameter passed to `view(s: S)` binds the view's parameter to `&S` when `S` is a
  `Copy` scalar, so a generic frame driver needs an owned state type. Every Polyp program's
  `Model` is one; `examples/polyp-ui-test/src/main.kp` wraps its counter in a `datac` for this
  reason.
- A non-`unit` value in statement position is an error when the enclosing `if` has no `else`;
  the paint code discards the primitives' pixel counts with `val _ = ...`.

## Stage 4: the freestanding proof

*Stage 4 of 5. `examples/polyp-fb-riscv64` and `examples/polyp-fb-x86_64` are the two projects
this section designs; the equivalence test they feed is in `kelpc/tests/compile_and_run.rs`.
This stage is LLVM-only, as every freestanding build is.*

The claim to prove is narrow and total: the same `core.kp`, the same `soft/` raster, the same
`soft_backend.kp` and the same application program, built with no libc for two bare-metal
targets, draw the same pixels as the hosted run. "Same pixels" is the FNV-1a checksum
`soft/fb.kp` already computes, printed after every step by all three runs.

### The two kernels

`examples/polyp-fb-riscv64` boots on QEMU's riscv64 `virt` machine (`-bios none`, UART at
`0x10000000`) and `examples/polyp-fb-x86_64` on `-M q35` through the PVH stub, with COM1
(`0x3F8`) for output and a `hlt` halt. Each is the `kernel0` shape (`plan/freestanding.md`,
`plan/x86-64.md`) plus four things:

- a `static mut FB: [u32; FB_W * FB_H]` at 320x200 (256 KiB of `.bss`, zeroed by the `_start`
  stub like every other static) that the project's platform hands to the backend as an `fb.Fb`
  with `stride_px = FB_W`;
- a bump arena in a second `static mut [u8; N]` named by `[target] alloc`/`dealloc`, so
  `List`, `String` and `Box` allocate through the project and the image links no libc;
- a platform (`FbPlatform`) that implements `soft/platform.Platform`: `framebuffer` returns the
  static surface, `present` counts the rectangles it was handed and advances a fake clock by
  100 ms, `now_ms` returns that clock and `poll_event` answers from a timed script that is
  empty here, because the scenario drives the programmatic `core.Event` channel exactly as the
  hosted run does;
- a UART writer for the step lines, and a `[target] panic` handler that prints
  `panic: <msg>` and halts.

The manifest grants `mmio` for the UART window only (`0x10000000+0x100`, `0x3F8+0x8`), lists
the modules that carry a pointer (`uart`, `fb_platform`, `deps/polyp/soft/fb`,
`deps/polyp/soft/draw`) and claims nothing else.

### The shared scenario

`examples/polyp-todo/src/scenario.kp` is the driver all three runs use:
`drive(backend, platform, emit)` builds `app.initial()`, applies `core.create_all`, paints and
presents the first frame, schedules every `app.script()` event on the backend, then calls
`soft_backend.frame` until the program quits, emitting one line per step:

```text
step <n> checksum 0x<16 hex digits> dirty <rectangles painted>
```

`n` is 0 for the first frame and 1.. for one scripted event each, because `SoftBackend.pump`
releases exactly one queued `core.Event` per frame. `dirty` is a new `SoftBackend` diagnostic
(`last_painted`), the count `paint` returned for the frame the driver just ran, so an idle
frame reports 0. `emit` is a plain `fun(str)`: the hosted entry prints, the kernels write the
bytes to their UART. The line is built in the scenario module, not by the platform, so the two
sides cannot format the same checksum differently.

`examples/polyp-todo/src/report.kp` is the same program's human-readable summary, moved out of
`app.kp` because its body calls `print`, which a freestanding build refuses wherever it
appears. `app.kp` is the model, the view, the update and the script, and is the module both
hosted and bare-metal runs link.

### What the core had to change

Four Polyp source fixes, all forced by the freestanding allowed set, none in the compiler:

1. **The progress fraction is an integer permille, not an `f64`.** `Node.Progress`,
   `Props.fraction` and `Children.progress` carry an `i32` in `0..=1000`. Two f64 operations
   were in the freestanding path: `node_props_eq`'s `fa == fb` (a `__eqdf2` soft-float call on
   a riscv64 target with no FPU) and `app.fraction`'s `done.to_f64() / total.to_f64()`. The
   view now computes `done * 1000 / total` with integers. `soft/frac.kp`, which decoded an
   `f64`'s bits into a permille for the paint path, is deleted: nothing decodes a float any
   more. `gtk_backend.kp` and `headless_backend.kp` convert the permille to the `f64` GTK and
   the text dump want (`permille.to_f64() / 1000.0`), hosted float arithmetic that no
   bare-metal image ever emits.
2. **`core.kp`'s passdowns are statics.** A `pd` is refused in a freestanding build because
   its initializer runs through `kelp_init_pds`, which only the hosted entry point calls. The
   counters become `static ... : AtomicI32 = AtomicI32.new(0)` (an `Atomic` static is the safe
   shared mutable, so the core still contains no `unsafe`).
3. **The entry-text snapshot moved to `examples/polyp/diag.kp`.** It is the one counter that
   is a `List<String>`, and a heap type cannot be a static. `diag.capture(backend)` is called
   by the hosted entries after the run and `diag.entry_text(0)` is what `report.kp` and
   `lib.kp`'s `entry_text` read; the freestanding projects do not import the module, so the
   `pd` it carries never reaches them.
4. **`core.kp` no longer imports `std/fmt`**, only for `fmt.int` in a duplicate-key panic
   message; `str.from_i64` is the same text and is part of the freestanding `std/str` subset.

### The equivalence test

`polyp_todo_soft` prints the same `step` lines (its entry prints them through `emit` and then
the summary), so the hosted run at the kernel's resolution is the expected output for both
kernels. The test runs the hosted binary with `POLYP_SOFT_SIZE=kernel` (320x200), builds each
kernel, boots it under `timeout 20 qemu-system-<arch>`, and compares the two runs line by line
from the first `step ` line to the report's `end`, dropping the hosted entry's `-- todo --`
separator; a single pixel difference anywhere fails it. The report is compared too, on purpose:
the same comparison that proves the pixels also proves the whole state machine (both `update`s,
the keyed diff, the counters and the entry text) ran identically with no libc. It also asserts
`llvm-nm -u` reports no undefined symbol for each image, which is the proof that nothing reached
libc or a soft-float helper. Both tests skip where `ld.lld` and the emulator are absent.

The two example `run.sh` scripts are the manual version, and `examples/polyp/PLATFORM.md` now
lists what a bare-metal project has to write down, with `main.kp` as the worked example.

### What this stage found

- **A keyable user type with a `String` field could not link freestanding.** The compiler emits
  the derived `kelp_hash_<T>`/`kelp_eq_<T>` pair for every keyable type in the program, whether
  or not anything uses it as a `Map` key, and the generated equality compares a `String` field by
  calling `kelp_map_key_eq`, a hosted runtime symbol with no freestanding body. The project's own
  types exposed it first (`core.Event`, `core.Key`, `core.Seg`, `app.Msg`, `app.Todo`,
  `Option<String>`), and every keyable `String`-holding datac in any future bare-metal program
  would hit it. Fixed in the Polyp source, with the language's own escape hatch: `core.Event` and
  `core.Key` declare a real `hash`/`eq` pair (`app.Todo` and `app.Msg` too), which is what makes
  the compiler skip the derived pair for them, and `soft_backend.find_text` writes into a caller's
  `String` instead of returning an `Option<String>`, so no keyable `Option<String>` is
  instantiated. The pairs are hand-written with `str.len`/`str.byte` (the freestanding lowering),
  so `Map` still works on both backends. Recorded as a gap in `plan/kernel-ub-notes.md`'s stage-4
  note; the freestanding compiler half (emit a body for `kelp_map_key_eq`, or emit the derived
  pair only for types a `Map` really keys) is not this stage's to make.
- **The float in the core.** `Props.fraction` was an `f64`, which cost two soft-float calls in
  the renderer: `node_props_eq`'s `fa == fb` and the view's `done.to_f64() / total.to_f64()`. The
  core carries an integer permille now and `soft/frac.kp`, which decoded an `f64`'s bits, is
  deleted. The pixels did not move: the hosted 400x300 and 800x600 checksums are byte-identical
  before and after the change to a permille, because `frac.of` already truncated `1000 * v`.
- **`pd` in the core.** A passdown is refused bare-metal, and `core.kp` used five of them for its
  counters plus one for the entry-text snapshot. The counters are `Atomic` statics now (no
  `unsafe`, because an `Atomic`'s methods take a shared borrow), and the snapshot is
  `examples/polyp/diag.kp`, a hosted-only module no freestanding project imports.
- **Two shapes the language made us write around**, both already in the gap list: a borrow of a
  temporary cannot be an argument (`emit(step_line(...).as_str())` is refused; bind it first), and
  a trait method's implementation must spell its parameters exactly as the trait does, unused name
  included.
- **`fn` types have unnamed parameters.** `fun (line: str)` is a parse error; a function type is
  `fun (str)`, and a parameter name would have to come from a trait declaration.

### The stretch: a real display (done)

On riscv64 QEMU's `virt`, `-device ramfb` puts the framebuffer on a real screen, and the project
registers it through fw_cfg: `src/ramfb.kp` reads the file directory (selector `0x0019`) with one
DMA transfer, finds `etc/ramfb`, and writes its 28-byte config (address, `XR24`, flags, width,
height, stride) with the DMA write direction. Every field of the descriptor and of the config is
big-endian, which is what QEMU's `fw_cfg_dma_transfer` and `ramfb.c` read. The test then boots the
image with `-device ramfb -display none` and a monitor on a pipe, asks for a `screendump`, and
compares the PPM with the kernel's own numbers.

One thing that had to change for it: the framebuffer's alpha byte carries glyph coverage, and a
display has no alpha, so the stage's `checksum` can never match a screen. `soft/fb.kp` gained
`checksum_rgb` (the same FNV-1a over blue, green, red and a constant `0xFF`), the shared scenario
emits it once as `display 0x<16 hex digits>` after the last step, and both kernels and the hosted
run print that line, so the display comparison is a checksum comparison and not a golden image.
`plan/polyp-ramfb-status.md` is the record, including what is deliberately not there.

## Stage 5: the latency model, work budgets and a benchmark

*Stage 5 of 5. "Without lag" is a claim about bounded, small work per frame, so this stage turns it
into a latency model, invariants the tests enforce, and numbers a reader can check. The implementation
is `examples/polyp/soft_backend.kp`, `examples/polyp/soft/platform.kp`, the work-budget program
`examples/polyp-budget-test/` and the benchmark `examples/polyp-bench/`.*

### What runs per frame

`soft_backend.frame_budgeted` is the whole per-frame cost. In order:

1. `platform.framebuffer()`, `backend.resize`, `backend.layout()`. Layout is a no-op unless the
   surface size or a widget changed.
2. Drain `platform.poll_event()` to `None`, handling each event in the backend: hit test, hover and
   focus changes, entry edits, wheel scrolling. Each is a tree walk and a small state write, and each
   that changes pixels adds damage.
3. `core.frame_step_into` with the backend's own reusable patch list: pump one scripted event, and
   for every posted message run `update`, `view`, `diff_into`, `apply`.
4. `backend.layout()` again, for whatever the patches changed.
5. `backend.paint_until`: one clip and one tree walk per damage rectangle, then `platform.present`
   with the rectangles painted.

What each step costs, in widgets and pixels:

- **Layout.** `measure` visits the dirty subtree plus the ancestors that must learn a child's size
  changed; `arrange` visits the whole tree only when a size changed, the window was resized, a `Move`
  reordered children, or the tree was never laid out. A same-width text change therefore measures one
  node and arranges none (the budget test pins `1` and `0`). Removing one row of 200 re-measures one
  node and arranges `1002` (the whole tree, because the content height changed), but damages only
  rectangles at or below the removed row.
- **Paint.** One rectangle at a time. Each rectangle's clip is its intersection with the surface,
  and the walk skips any node whose rectangle does not intersect the clip, so rows scrolled out of a
  viewport cost nothing. Every stage-2 primitive returns the pixels it wrote; the frame's total is
  their sum, which is what the budget tests and the benchmark pin.
- **Input.** Hit test is a walk front to back. Hover, focus and caret moves flip a boolean or an
  integer and damage the old and the new widget. An entry edit builds one new `String`. A wheel step
  moves one integer and damages the scroll area.
- **Overdraw.** A repaint fills each damage rectangle with the theme background and then draws the
  widgets over it, so one rectangle's primitive writes exceed its area. The measured factor for a
  full 800x600 repaint of the 200-row todo UI is `1.969x` (945223 pixel writes over 480000 pixels),
  under the factor 2 the budget allows.

### Allocation accounting

The backend exposes three counters, all reset by `reset_counts` and read after a frame:

- `frame_alloc_count()`: the number of allocation sites the backend executed in the last frame.
- `frame_alloc_bytes()`: a lower bound on the bytes those sites built (the allocator's own size is
  not observable from Kelp, so each site charges a documented estimate).
- `arena_high_water()`: the largest patch list any frame has produced.

A site is a place the backend builds a heap value: `make` copies a new widget's `Props`, an entry
edit builds the new text, `set_entry_text` writes one, and `post_id`/`schedule`/`push` append to a
queue. The steady-state paths call none of them, and `set_props_on` clears and refills the node's
own `String`s instead of replacing them with fresh `copy_props` values, so a text, checkbox or
progress patch reuses the retained node's buffers. The work-budget test pins `0` for an idle frame,
a hover, a focus change, a caret move, a wheel step and a steady-state patch frame; typing one
character accounts for the one allocation the new text needs.

The core's `diff` may allocate its patch list, so `core.diff_into(old, new, out)` clears and refills
a caller-owned `List<Patch>` instead. `soft_backend.apply_diff` keeps that list in a `Box` on the
backend; it is cleared and refilled, never freed, and after the largest frame a smaller one reuses
its capacity. `core.frame_step_into` does the same for the loop's own diffs.

This counter is scoped to the backend on purpose: it is what the backend can honestly account for.
It does **not** count the application's `view`, which rebuilds the tree through Kelp's one
allocator (libc `malloc` in a hosted build, the project's `[target] alloc` pair freestanding), nor
the `Props` strings the core's `diff` builds for a changed widget. A whole-frame zero-allocation
claim would need a retained view model or an allocator hook the hosted language does not have; see
"What is not promised".

### Capacities, and what happens past one

- The damage list is a fixed `[Rect; 16]` (`damage.MAX_DAMAGE = 16`). It never exceeds 16: a
  touching or overlapping rectangle is merged, and when the list is full and nothing touches, the
  least-waste pair is merged first. Nothing is silently dropped.
- The clip stack is a fixed `[Rect; 8]`; a ninth push panics `clip stack depth 8 exceeded`.
- The glyph mask is a fixed `560`-byte stack array (`5 * 4 * 7 * 4`, the font's 4x maximum); a
  text scale outside `1..=4` panics `text scale must be in 1..=4`.
- The reusable patch list is a `List<Patch>` that grows through `List.push`. It has no fixed size,
  so the documented cap is `MAX_FRAME_PATCHES = 4096`: a frame that produces more patches panics
  `polyp: frame patch list exceeds MAX_FRAME_PATCHES` rather than growing without bound.
- The retained tree's nodes are owned by the tree's `List`s and grow by one node and one parent
  insertion per `Create`, never shrink, and are freed only when the backend is dropped.

### Work budgets, as exact tests

`examples/polyp-budget-test/` drives the backend directly and prints one number per line. The Rust
test `polyp_soft_work_budgets_hold` pins the whole output, so the numbers are the same on the LLVM
backend and on the C oracle. At 800x600:

| scenario | what it asserts |
| --- | --- |
| idle | an idle frame presents `0` rectangles and writes `0` pixels |
| text | a same-width label change presents exactly `1` rectangle, area `168` (the 24x7 label), and accounts for `0` allocations after warm-up |
| hover | a hover change presents `1` rectangle, the hovered widget |
| focus | a focus change presents `1` rectangle (old focus was none, new is one button) |
| type | typing one character presents `1` rectangle, the entry's `1800`-pixel box, and accounts for `1` allocation |
| remove | removing one row of 200 damages only rectangles at or below it (smallest damage `y` is `70`, the removed row's top) |
| append | appending a row damages only the new row (off-screen, `0` pixels) |
| repaint | a full repaint writes `945223` pixels for `480000`, a factor of `1.969`, under 2 |
| deadline | a 2500 us budget splits a 5-rectangle frame into `3` + `2`, every rectangle whole |

The layout counter is exact too: the text change measures `1` node and arranges `0`; the row
removal measures `1` and arranges `1002` (the whole tree, because the content height changed).

### The benchmark

`examples/polyp-bench/` runs five scenarios, each five times, and prints per scenario: frames,
pixels, presented rectangles, backend allocations, and median and worst microseconds per frame.
It uses `std/time`'s monotonic clock; the counts are exact and machine-independent, the times are
the host's.

Built and run with the release `kelpc` (the compiler's default is `-O2` for the generated program):

```text
nix-shell /home/juniper/Projects/kelp-native-ui/overnight/dev.nix --run \
  'cd kelpc && ../target/release/kelpc build ../examples/polyp-bench -o ../target/polyp-bench'
( ulimit -v 1048576; ulimit -t 60; timeout 60 ./target/polyp-bench )
```

On the development host (7.8 GiB, LLVM backend, release `kelpc`):

| scenario | frames | pixels | rects | allocs | median us/frame | worst us/frame |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| scroll-200 | 200 | 187536900 | 200 | 0 | 3973 | 4371 |
| type-100 | 500 | 16719500 | 500 | 500 | 4788 | 4859 |
| toggle-50 | 250 | 7226945 | 495 | 0 | 4558 | 4632 |
| resize | 150 | 187531425 | 2400 | 0 | 5846 | 6622 |
| repaint | 150 | 141783450 | 150 | 0 | 3575 | 3657 |

The debug `kelpc` build gives the same counts and times within a few percent (worst: scroll `4020`,
type `4439`, toggle `4154`, resize `6123`, repaint `3355`). No scenario's worst frame exceeds 16 ms
on this host with either build, so v3 does not need the qualifier the stage asked for. The C oracle
is a different matter: its generated code is roughly three times slower here, so the full benchmark
does not fit the 10 s bounded runner the test suite uses. The suite therefore sets
`POLYP_BENCH_QUICK`, which shortens every scenario to one frame per run and keeps the same counts
per frame; the table above is a full run.

### The frame deadline

`frame_budgeted(backend, platform, tree, state, view, update, budget_us)` is `frame` with a
deadline. It reads `platform.now_us()` at the start, then after each damage rectangle, and returns
`Frame.Deferred(tick)` with the remaining rectangles still in the damage list once the elapsed time
passes `budget_us`. `Frame.Done(tick)` means the frame finished. A rectangle is painted whole or not
at all, so a deferred frame presents a subset of whole rectangles and the next frame paints the
rest; nothing is lost and no rectangle is half-painted. `budget_us = 0` means no deadline, which is
what `frame` passes, so every existing caller is unchanged.

The `Platform` trait grew a fifth method, `now_us()`, for this: `now_ms` is the coarse clock the
scripted input uses, and a deadline needs a finer one. A platform with only a millisecond clock
returns `now_ms() * 1000` and gets millisecond granularity; `HeadlessPlatform` has a microsecond
clock, and its test clock advances a settable number of microseconds per read, which is how the
budget test makes a 2500 us budget split a 5-rectangle frame into 3 and 2 deterministically.

An operating system integrates it with a tick: on a 60 Hz display the tick is 16.7 ms, so it passes
`budget_us = 16000` and, when the call returns `Deferred`, calls again in the same tick (or at the
start of the next) with the same tree and state, until it returns `Done`; on a 144 Hz display the
budget is about 6900 us. Because the deferred frame keeps its damage, the second call paints only
what the first could not; the application state is not advanced twice, because `frame_step` already
ran.

### What is not promised

- **No whole-frame zero-allocation claim.** The backend's own frame path allocates nothing in the
  steady state, and the patch list is reused, but `view` rebuilds the application tree through
  Kelp's one allocator every time a message arrives. A view that changed nothing still allocates a
  new tree. Removing that needs a retained view model, which v3 does not have.
- **No animation, no timers.** The backend has no clock of its own and no interpolated state; every
  frame is a response to input or a patch. A future stage may drive `frame` from a vsync tick.
- **No vector text, no sub-pixel positioning.** The embedded 5x7 ASCII face is the only font, at
  integer scales 1 to 4, one pen step of `6 * scale` per byte; a byte outside 32..126 is one
  replacement box, and a multi-byte UTF-8 sequence is one box per byte.
- **No GPU, no display server.** The renderer is the integer software raster in `soft/`; the only
  hardware fact is the platform's pixel pointer.
- **Theming is a `Theme` datac and a class table.** Colours and metrics are one `datac` a platform
  can replace, and class overrides are the `class_style` comparisons in `soft_backend.kp`; there is
  no stylesheet parser and no selector engine.
- **No accessibility.** There is no focus announcement, no role tree and no screen-reader bridge.
- **Timing is the host's.** The benchmark's clock measures the process, not a display's frame
  pacing, and the numbers are one development host's at one moment.

### The final v3 status

v3 is five stages, all green on the LLVM backend and the C oracle except the freestanding one, which
is LLVM-only by design. What exists:

- `core.kp`: the pure tree, the keyed diff (now `diff_into` for a caller-owned patch list), the
  `Backend` trait and the loop.
- `soft/`: the integer software raster (`fb`, `draw`, `font`, `damage`, `debug`), the six-variant
  `Event`, the `Platform` trait and two hosted platforms plus the two kernels' platform.
- `soft_backend.kp`: the retained widget tree, measure/arrange layout with the skip rule, damage-only
  painting, input, the class hook, the allocation and pixel counters, and the deadline hook.
- `gtk_backend.kp` and `headless_backend.kp`: the two stage-1 backends, unchanged in surface.
- The tests: the pure diff tests, the raster's per-pixel references and golden checksums, the UI
  hit/focus/edit/damage/layout tests, the todo run at three sizes, the two bare-metal kernels
  (including the ramfb display), the work-budget test and the benchmark.

The invariants a new backend must keep: one patch walk in `core.apply_patches`; interactive ids
handed out by counting interactive `make` calls; focus checked across every patch; damage rectangles
painted whole; a frame that changes nothing presents nothing; and the counters above.
