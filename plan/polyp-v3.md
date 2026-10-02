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
module a manifest lists; `plan/kernel-ub-notes.md` records it.

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
