# Polyp

A declarative native UI library for Kelp, over GTK4. The application describes
what the screen should be for the current state; Polyp owns the loop and patches
the live widget tree. It wraps GTK's own widgets rather than drawing, so a Polyp
window is a GTK window.

v1 added the builder syntax:

```kelp
polyp.column { k ->
    k.text(s.title.as_str())
    k.button("+1", Msg.Inc)
}
```

The block is a `scoped fun(mut Children<M>)` (plan/scoped-blocks.md), so it is
written at this call site and its captures are borrows released with the call:
`k.text` reads the view's borrowed `s`, and no callback owns the state. The v0
plain constructors are still in the library, so both styles compile.

**v2** keeps the live GTK widget tree and **patches** it: children can carry a
key, a matched widget is updated in place, a moved keyed child is reordered
rather than rebuilt, and focus, selection and scroll survive. It also adds
`scrolled`, `checkbox`, `progress`, `separator`, the layout options `spacing`,
`margin`, `expand`, `align` and `class`, and the app-level `polyp.css`.
`plan/polyp-v2.md` is its design note.

**v3** splits the library into a backend-neutral **core** and one module per
backend. The tree, the keyed diff and the Elm loop are `core.kp`, which names no
GTK symbol, calls no `extern fun` and uses no `unsafe`; the answer to "what
changed" is a `List<Patch>` of plain data. GTK is one implementation of the
`core.Backend` interface (`gtk_backend.kp`) and a **headless** backend
(`headless_backend.kp`) applies the same patches to an in-memory tree, so the
loop and a whole program can be tested with no display at all on either compiler
backend. The application surface is unchanged apart from the tree's type, which
is `core.Node<M>` now that it lives in the core; `plan/polyp-v3.md` is the design
note. Examples `polyp-counter` and `polyp-counter-v0` keep working;
`examples/polyp-todo` is the keyed-list program and `examples/polyp-core-test` is
the display-free test project.

**v3 stages 2 to 4** add a display-free path: `examples/polyp/soft/` is a self-contained integer
rasterizer, `soft_backend.kp` applies the same patches to a retained tree and paints through it,
and `examples/polyp-fb-riscv64`/`examples/polyp-fb-x86_64` prove the whole stack boots and draws
the hosted run's exact pixels with no libc (`plan/polyp-v3.md` stages 2, 3 and 4, and the
`## The freestanding proof` section below).

**v4** adds a platform clock with subscriptions and integer animations, UTF-8 soft text and editing,
an on-demand accessibility tree, and memoised subtrees. A memo is an explicit pure function of a
`core.Deps` value: `k.memo(key, deps, build)` preserves its retained subtree when the dependencies
are equal. The caller keeps `core.MemoStore` with the displayed tree and uses the memo-aware first
render and frame entry points. [`plan/polyp-v4.md`](../../plan/polyp-v4.md) records the contract,
the test coverage and the remaining limits.

There is no published version number; "v3" is the backend split plus what is
listed here. `examples/polyp-v0/` and `examples/polyp-counter-v0/` are a frozen
copy of v0 (see [Versions](#versions)).

## What a program looks like

```kelp
impl "deps/polyp/core"
impl "deps/polyp/lib" caps(ui = ["*"])
impl "std/fmt"
impl "std/str"

datac Counter(var count: i32, var draft: String)

enum Msg {
    Inc
    Dec
    Changed(val text: String)
}

// A plain `fun` value, not a closure: the entry hands it the new text.
fun changed(text: String).return: Msg = Msg.Changed(text = String.from_str(text.as_str()))

fun view(s: Counter).return: core.Node<Msg> {
    return polyp.column { k ->
        k.text_styled("a counter", 0, true)
        val shown = fmt.int(s.count.to_i64())
        k.text(shown.as_str())
        k.spacer(6)
        k.button("+1", Msg.Inc)
        k.button("-1", Msg.Dec)
        k.entry(s.draft.as_str(), changed)
    }
}

fun update(state: mut Counter, msg: Msg) {
    when msg {
        Inc -> { state.count += 1 }
        Dec -> { state.count -= 1 }
        Changed(text) -> { state.draft = String.from_str(text.as_str()) }
    }
}

fun main() {
    val final = polyp.run(
        title = "counter",
        width = 320,
        height = 200,
        init = Counter(count = 0, draft = String.from_str("")),
        view = view,
        update = update,
    )
    print(final.count)
    print(final.draft)
}
```

`examples/polyp-counter/` is that program, complete and buildable. The plain
style of the same view is still available (and `core.Node<M>` is the tree type
v3 moved into the core):

```kelp
var kids: List<core.Node<Msg>> = List.empty()
kids.push(polyp.text(String.from_str("hi")))
kids.push(polyp.button(String.from_str("+1"), Msg.Inc))
return polyp.plain_column(kids)
```

## The architecture, and why it is this one

Polyp is the Elm architecture, and the reason is a property of Kelp rather than
a taste in UI frameworks:

- A Kelp closure **owns** its captures and cannot capture a borrow
  (`plan/syntax.md` 7.4), and Kelp has no shared-ownership type to put the
  application state behind.
- So a callback cannot hold the state. Polyp does not pretend otherwise: **no
  callback captures the state at all.**
- Instead, a button's handler is a closure whose entire environment is one small
  integer. It posts that integer to a fixed-size C queue
  (`include/polyp_queue.h`), and the loop, which *does* own the state, reads the
  integer, finds the node it names, takes that node's `Msg` out of a fresh
  `view`, calls `update`, re-runs `view` and patches the widgets.

That is why the split into a core and a backend is not just tidiness: the only
thing a callback carries is an integer, and the only thing a backend is told is
plain `Props`, so the loop and the diff are expressible, and testable, with no
display at all.

GTK callbacks and the loop run on the same thread (`g_signal_connect` handlers
run inside `g_main_context_iteration`, which the loop calls), so the queue needs
no lock.

The builder block is the one place the state is read from a callback-shaped
literal, and it is safe because the literal never outlives the call: it is a
`scoped fun(...)` value the callee may only call during `column`/`row`, and its
captures are shared borrows the borrow checker releases afterwards. An
*exclusive* capture (the `state.count += 1` shape) is not implemented, so a
button still carries a `Msg`, never a mutation.

That makes the two declarative claims hold literally:

- **`view` is a pure function of the state.** It takes the state and returns a
  `Node<Msg>`; it is called on every state change and can do nothing else.
- **`update` is an exhaustive `when` over the `Msg` enum.** Adding a variant and
  forgetting to handle it is a compile error.

## API

```kelp
pub enum Key { None  Str(val s: String)  Int(val n: i64) }
pub enum Align { Fill  Start  Center  End }

pub datac Opts(var margin: i32, var expand: bool, var align: i32, var classes: List<String>)
pub datac Child<M>(val key: Key, val node: Node<M>, val opts: Opts)

pub enum Node<M> {
    Column(val children: List<Child<M>>, val spacing: i32)
    Row(val children: List<Child<M>>, val spacing: i32)
    Text(val text: String, val size: i32, val bold: bool)
    Spacer(val height: i32)
    Entry(val text: String, val on_change: fun (String).return: M)
    Button(val label: String, val on_click: M)
    Checkbox(val label: String, val checked: bool, val on_toggle: M)
    Progress(val permille: i32)
    Separator
    Scrolled(val children: List<Child<M>>, val spacing: i32)
    Memo(val deps: Deps, val build: fun (&Deps).return: List<Child<M>>)
}

pub datac Deps(var ints: List<i64>, var strs: List<String>)
pub fun deps().return: Deps
fun mut Deps.push_int(n: i64)
fun mut Deps.push_bool(b: bool)
fun mut Deps.push_str(s: str)
pub fun single<M>(block: scoped fun(mut Children<M>)).return: List<Child<M>>

// The builder. `column`/`row` take the trailing block; the methods on
// `Children` are the leaf and nested constructors the block calls.
pub fun column<M>(block: scoped fun(mut Children<M>)).return: Node<M>
pub fun row<M>(block: scoped fun(mut Children<M>)).return: Node<M>
fun mut Children<M>.text(t: str)
fun mut Children<M>.text_styled(t: str, size: i32, bold: bool)
fun mut Children<M>.spacer(height: i32)
fun mut Children<M>.button(label: str, on_click: move M)
fun mut Children<M>.entry(value: str, on_change: fun (String).return: M)
fun mut Children<M>.checkbox(label: str, checked: bool, on_toggle: move M)
fun mut Children<M>.progress(permille: i32)
fun mut Children<M>.separator()
fun mut Children<M>.column(block: scoped fun(mut Children<M>))
fun mut Children<M>.row(block: scoped fun(mut Children<M>))
fun mut Children<M>.scrolled(block: scoped fun(mut Children<M>))
fun mut Children<M>.keyed(key: str, block: scoped fun(mut Children<M>))
fun mut Children<M>.keyed_col(key: str, block: scoped fun(mut Children<M>))
fun mut Children<M>.keyed_int(key: i64, block: scoped fun(mut Children<M>))
fun mut Children<M>.keyed_int_col(key: i64, block: scoped fun(mut Children<M>))
fun mut Children<M>.with_key(key: str)
fun mut Children<M>.with_key_int(key: i64)
fun mut Children<M>.memo(key: str, deps: move Deps, build: fun (&Deps).return: List<Child<M>>)
fun mut Children<M>.memo_int(key: i64, deps: move Deps, build: fun (&Deps).return: List<Child<M>>)
fun mut Children<M>.spacing(n: i32)
fun mut Children<M>.margin(n: i32)
fun mut Children<M>.expand()
fun mut Children<M>.align(a: Align)
fun mut Children<M>.class(name: str)

// The v0 plain constructors. `text` and `button` keep their names because
// their builder twins are methods (a separate namespace); `column`/`row` are
// the builder's free functions, and Kelp has no overloading, so the plain pair
// carries a `plain_` prefix.
pub fun plain_column<M>(children: move List<Node<M>>).return: Node<M>
pub fun plain_row<M>(children: move List<Node<M>>).return: Node<M>
pub fun text<M>(text: move String).return: Node<M>
pub fun button<M>(label: move String, on_click: move M).return: Node<M>

pub fun run<S, M>(title: str, width: i32, height: i32, init: move S,
                  view: fun(S).return: Node<M>, update: fun(mut S, M)).return: S

pub datac Stats(val created: i32, val updated: i32, val removed: i32, val live: i32,
                val memo_hits: i32, val memo_misses: i32, val view_calls: i32)
pub fun stats().return: Stats
pub fun focus_present().return: bool
pub fun focus_survived().return: bool
pub fun entries_built().return: i32
pub fun entry_text(index: i32).return: String
pub fun css(text: str)
pub const QUIT_ID: i32
pub fun schedule_post(delay_ms: u32, id: i32)
pub fun schedule_quit(delay_ms: u32)
pub fun schedule_set_entry(delay_ms: u32, index: i32, new_text: str)
pub fun schedule_focus_entry(delay_ms: u32, index: i32)
```

- `run` opens one `GtkWindow` with the given title and default size, patches the
  first tree into it, and iterates the default GLib main context until a quit is
  posted. It **returns the final state** (`init` is moved in, so returning it is
  how a caller reads what the run left behind).
- `fun(S)` in `view`'s type is the "view borrows the state" shape: a parameter of
  an owned type is a shared borrow (`plan/syntax.md` 7.1), so an application
  writes `fun view(state: Counter).return: Node<Msg>` with no `&` anywhere. `update`
  is written `fun update(state: mut Counter, msg: Msg)` and matches
  `fun(mut S, M)`.
- `text` is a label. `text_styled(text, size, bold)` sets Pango markup after
  escaping the text, so a `&` or `<` is shown, not parsed; `size` is points and
  `0` means GTK's default. `spacer(height)` reserves vertical pixels.
- `entry(value, on_change)` is a text field. `value` seeds the text the first
  time the widget is built; a patch rewrites it only when the model's text
  differs **and** the entry is not where the focus is, so a user's cursor and
  selection are not fought. `on_change` is a plain `fun(String).return: M` (called
  with a borrowed `String`) that turns the live text into the app's `Msg`.
- `checkbox(label, checked, msg)` posts `msg` on `toggled`; the application reads
  the new state from its own model, exactly as a button does. `progress(permille)`
  is a `GtkProgressBar`. `separator()` is a horizontal rule.
- `scrolled(block)` puts the block's children in a `GtkScrolledWindow`, laid out
  in a vertical box inside it. The window is patched in place, so the scroll
  offset survives.
- `keyed(key, block)` is a keyed **row**: the block's children become one `Row`
  child with the given key, which is the shape a list of rows wants.
  `keyed_col` is the same for a column, `keyed_int`/`keyed_int_col` are the
  integer-keyed twins, and `with_key`/`with_key_int` give the next pushed node a
  key of any kind. Keys must be unique among the children of one container; a
  duplicate panics naming it.
- `spacing(n)` is the spacing of the box being built. `margin(n)`, `expand()`,
  `align(a)` and `class(name)` are options of the **next pushed child**, whatever
  its kind: `k.expand() k.column { ... }` expands the column, and
  `k.class("row") k.keyed(...)` classes the row.
- `css(text)` loads a `GtkCssProvider` on the default display at
  `GTK_STYLE_PROVIDER_PRIORITY_APPLICATION`, so ordinary CSS applies to every
  widget that carries a matching class.
- **An id is an interactive widget's.** The core and the backend derive the same
  integer from the same rule: the nth `Entry`, `Button` or `Checkbox` a run
  creates is id n, ids only grow and are never reused, and a handler captures its
  id forever. A surviving widget keeps its id; a removed one's id resolves to no
  path, so a queued id whose widget is gone is dropped instead of reaching
  `update`. `QUIT_ID` is `-2`, because `polyp_poll` answers `-1` for an empty
  queue. An application should still carry a `Msg` rather than post a raw id.
- `schedule_post` / `schedule_quit` post an id after a delay;
  `schedule_set_entry` asks the loop to set an entry's text after a delay, which
  fires that entry's real `changed` signal; `schedule_focus_entry` sets the
  window's focus. They exist for programs that have to run with no user (the
  smoke tests are), and they are the only reason `g_timeout_add_full` is bound.
- `stats()` is a diagnostic: how many widgets the run ever built, how many
  matched widgets it patched in place, how many it destroyed, and how many are
  alive (`created - removed`). `focus_present()` says whether any patch found a
  focus widget, `focus_survived()` whether every patch kept the one it found,
  `entries_built()` how many `Entry` widgets the process built, and
  `entry_text(index)` reads the index-th entry's text as `diag.capture` took it when `run`
  returned. A run that patches keeps `created` flat across state changes; a
  rebuild-everything implementation does not.

`Node<M>` is a generic enum that recurses through `List<Child<M>>`, which the
compiler accepts as written: a `List` element lives in the list's own heap
buffer, so the type has a finite size and no `Box` is needed. `Node<M>` itself
carries application data (`M`, `String`) and is moved apart as it is built, so
nothing keeps it after the widgets exist.

## Reconciliation, exactly

Every state change calls `view` for the whole tree, but the loop keeps the live
widgets and **patches** them. v3 makes the decision a pure function,
`core.diff(old, new) -> List<Patch>`, whose output is one of `Create`,
`Update`, `Remove` or `Move`, each naming a path; the backend applies that list
and nothing else. For each container (and for the window's single child):

- A child is matched to its previous self **by key** when it has one, and by
  **position and widget kind** otherwise. Kinds are part of the match, so a
  `Button` is never patched into a `Text`.
- A matched widget is **updated in place**: label markup, button label, checkbox
  label and active state, entry text (only when it differs and the entry does not
  hold the focus), progress permille, spacer height, box spacing, margins,
  expand, alignment and CSS classes. It is the same GTK object, so its focus,
  text selection and scroll offset survive by construction.
- A child the new tree does not match is created and inserted at its index
  (`gtk_box_insert_child_after`). A keyed child that only moved is reordered
  (`gtk_box_reorder_child_after`), not rebuilt. When the matched sequence is
  still in order the survivors are not touched at all, which is what keeps a
  removal from disturbing the focus.
- A child the new tree does not name is removed from its container, and its
  subtree is counted as destroyed.
- A programmatic write to a checkbox or an entry blocks that widget's signal
  first (`g_signal_handler_block`), so setting the widget's state to match the
  model cannot post the `Msg` that state came from.

The old v1 rule (the `i`-th `Entry` reuses the `i`-th entry widget) is gone; the
patch subsumes it, and it works for any widget, not only an entry. The counter's
smoke run still prints one entry built across all its renders.

## The keyed todo list

`examples/polyp-todo/` is a todo list: an entry and an add button, a scrolled
list of keyed rows (checkbox, title, move-up and remove buttons), and a progress
bar of done over total. Each row's key is the item's own id, never its index,
which is what makes a row the same row after a reorder. Ticking a row patches
its checkbox; removing a row leaves the other rows' widgets untouched.

Its smoke test drives it headless (`POLYP_SMOKE=1`): it focuses the entry, types
three titles through the real `changed` signal, adds them, ticks the second row,
moves the third up, removes the middle one, and quits, printing the final model
and the library's counters. The test asserts those counters, and that the entry
still shows the last text typed even though the model cleared it.

## The software rasterizer (v3 stage 2)

`examples/polyp/soft/` is the layer a **soft** backend draws through with no widget toolkit and
no display, and the first part of Polyp built to survive a bare-metal build. It is integer-only
(no `f32`/`f64`, so no soft-float libcall on a riscv64 target), calls no libc and no OS, never
allocates, and the only external facts it has are a pixel pointer and a stride.

```kelp
val base: Ptr<[u32; N]> = unsafe { Ptr.from_mut(FRAME) }   // or Ptr.from_ref(&buf) for a local
val screen = fb.new(pixels = base.cast<u32>(), width = N.to_i32(), height = 1, stride_px = N.to_i32())
val clip = draw.clip(fb.Rect(x = 0, y = 0, w = 320, h = 200))
val painted = draw.fill_round_rect(screen, clip, 8, 8, 64, 32, 6, 0xFF3050A0)
```

| Module | What it holds | `unsafe` |
| --- | --- | --- |
| `fb.kp` | `Fb(pixels, width, height, stride_px)`, `Rect`, the pixel store/load, `checksum` | yes |
| `draw.kp` | `ClipStack` (fixed depth 8) and every primitive: `fill_rect`, `stroke_rect`, `fill_round_rect`/`stroke_round_rect`, `hline`/`vline`, `line`, `blend_pixel`/`blend_over`, `blit_mask8` | yes |
| `font.kp` | a hand-drawn 5x7 face for ASCII 32..126, `text_width`/`text_height`, `draw_text` at scale 1..4 | no |
| `damage.kp` | `Damage`, at most 16 dirty rectangles, `add`/`clear`/`count`/`at`/`bounds` | no |
| `debug.kp` | `dump_ppm`, hosted only | yes |

Only the three modules that dereference a pixel are on a consumer's `[unsafe] modules` list;
`font` and `damage` are not. `debug.kp` is a separate file so a freestanding project, which does
not import it, never compiles `std/fs`.

Every primitive clips to `clip.top() ∩ fb.bounds()` and returns the number of pixels it wrote, so
a frame's cost can be asserted. The blend is exact source-over per channel,
`(src * a + dst * (255 - a) + 127) / 255` in `u64`. `draw_text` renders any byte outside
32..126 as a replacement box, one box per byte, so a multi-byte UTF-8 scalar is several boxes;
`text_width` counts bytes. `checksum(fb)` is FNV-1a 64 over the pixels row by row.

Its tests live in `examples/polyp-soft-test/` and run in the plain development shell, with no
display, on both compiler backends:

```sh
nix-shell /path/to/dev.nix --run 'cd kelpc && cargo test --test compile_and_run polyp_soft'
nix-shell /path/to/dev.nix --run 'cd kelpc && KELPC_ORACLE=c cargo test --test compile_and_run polyp_soft'
```

Each primitive is compared pixel for pixel against an independent per-pixel reference written in
the test project, on fixed-seed inputs with negative and zero sizes and clips on every edge; three
fixed scenes print golden FNV checksums; the damage list is checked to stay within 16 entries and
to cover every point ever added. Three API contracts panic (`text scale must be in 1..=4`, `clip
stack depth 8 exceeded`, `clip stack is empty`). `plan/polyp-v3.md` stage 2 is the design.

## The soft backend, layout, input and a platform (v3 stage 3)

`examples/polyp/soft_backend.kp` is stage 1's `core.Backend` over the raster instead of a toolkit:
zero display dependency, zero libc, and a per-frame path that allocates nothing. It keeps the
same retained tree `headless_backend.kp` keeps (so interactive ids, counters, focus and entry
text cannot drift between them) and adds integer layout, damage-only painting and input.

```kelp
impl "deps/polyp/core"
impl "deps/polyp/soft/headless_platform"
impl "deps/polyp/soft_backend"

var platform = headless_platform.new(width = 400, height = 300)
var backend = soft_backend.new()
var state = app.initial()
var tree: core.Node<app.Msg> = app.view(state)
backend.resize(400, 300)
backend.apply(core.create_all(&tree))
while running {
    when soft_backend.frame(backend, platform, tree, state, app.view, app.update) {
        Idle(next) -> tree = next
        Changed(next) -> tree = next
        Quit(next) -> { tree = next; running = false }
    }
}
```

| Module | What it holds | `unsafe` |
| --- | --- | --- |
| `soft_backend.kp` | `SoftBackend`: the retained tree, `Theme`, the class override table, measure/arrange, damage-only painting, hit testing, hover/press/focus, entry editing | no |
| `soft/event.kp` | `Event`: pointer, scroll, key and char | no |
| `soft/platform.kp` | `trait Platform`: `present`, `now_ms`, `poll_event`, `framebuffer` | no |
| `soft/headless_platform.kp` | an in-memory framebuffer, scripted input, recorded presents | yes |
| `soft/ppm_platform.kp` | the headless platform plus a PPM dump when `POLYP_DUMP_DIR` is set | yes |
| `diag.kp` | the entry-text snapshot a hosted program reads after a run, in its own module because its one global is a `pd` | no |

Layout is two passes, measure and arrange, over the same `Rect` arithmetic; a patch marks only
the node it wrote dirty, and `arrange` runs only when a size changed, the window was resized, a
`Move` reordered children, or the tree was never laid out. So a text change whose new string has
the same pixel width re-measures one node and re-arranges none. `class("name")` resolves through
a small override table (`bg`, `fg`, `border`, `radius`, `pad`), and a class on a container's
foreground reaches the text inside it. Painting walks the tree once per damaged rectangle, clipped
to it, and after each frame the backend hands the platform the exact rectangles it painted.

Input is the six-variant `Event` a platform decodes its windowing system into. The backend
hit-tests, tracks hover and pressed state, moves focus with Tab and Shift-Tab, activates buttons
on a press and release on the same widget, edits a focused entry with a visible caret, toggles a
checkbox with Space, and scrolls the `Scrolled` under the wheel. Every change produces damage;
a no-op produces none, and an idle frame presents nothing.

A platform is four functions and the framebuffer it owns; `examples/polyp/PLATFORM.md` is the
plain-terms contract (the pixel format, the stride rule, the allocation contract, the timing
contract). `soft_backend.frame` is the one function an OS calls per tick. `examples/polyp-todo-soft/`
runs the same todo program as the GTK and headless runs and prints the framebuffer checksum after
every frame; `examples/polyp-ui-test/` holds the hit, focus, editing, damage and layout-skip
tests. Both run in the plain development shell on both compiler backends.

```sh
nix-shell /path/to/dev.nix --run 'cd kelpc && cargo test --test compile_and_run polyp'
nix-shell /path/to/dev.nix --run 'cd kelpc && KELPC_ORACLE=c cargo test --test compile_and_run polyp'
```

## The freestanding proof (v3 stage 4)

`examples/polyp-fb-riscv64/` and `examples/polyp-fb-x86_64/` are bare-metal `[target]` projects
(`plan/freestanding.md`, `plan/x86-64.md`) that link the core, the raster, the soft backend and the
todo program into an image with no libc and draw into a `static mut [u32; 320 * 200]`. They run the
same script as `examples/polyp-todo-soft`, through the shared driver
`examples/polyp-todo/src/scenario.kp`, and print one line per step:

```text
step 0 checksum 0xf0b341c4a10c78b5 dirty 1
...
step 11 checksum 0x0e6a6aa56d77ac07 dirty 0
```

Everything an OS has to provide on top of the platform contract is in
`examples/polyp-fb-riscv64/src/main.kp`: a framebuffer (a static, or MMIO), a `[target]
alloc`/`dealloc` pair for the tree's allocations, a `[target] panic` handler, and a way to write a
line out. `examples/polyp/PLATFORM.md` has the list. The equivalence test is the point:
`kelpc/tests/compile_and_run.rs` runs `examples/polyp-todo-soft` at `POLYP_SOFT_SIZE=kernel`
(320x200) and requires each kernel's serial output to be byte-identical to it, step lines and
report, so one pixel of difference fails; `llvm-nm -u` on each image shows no undefined symbol,
which is what proves nothing reached libc or a soft-float helper.

```sh
nix-shell /path/to/dev-kernel.nix --run 'cd kelpc && cargo test --test compile_and_run polyp_fb'
nix-shell /path/to/dev-kernel.nix --run 'examples/polyp-fb-riscv64/run.sh'
```

The last line of a run is `display 0x<16 hex digits>`: the same FNV-1a over the colour bytes only,
because the framebuffer's alpha byte carries glyph coverage and a screen has no alpha. On riscv64
the project also registers its surface with QEMU's `ramfb` device through fw_cfg
(`src/ramfb.kp`), so `-device ramfb` puts those exact pixels on a real screen;
`polyp_fb_riscv64_shows_its_framebuffer_on_a_real_display` captures it with a `screendump` and
compares it with that line (`plan/polyp-ramfb-status.md`).

## Work budgets, allocation accounting and a benchmark (v3 stage 5)

"Without lag" is a claim about bounded work per frame, so stage 5 makes it measurable. The design
is `plan/polyp-v3.md`'s "Stage 5: the latency model, work budgets and a benchmark".

**Work budgets.** `examples/polyp-budget-test/` drives the soft backend directly and prints exact,
machine-independent counts. `polyp_soft_work_budgets_hold` pins them on both backends: an idle frame
presents 0 rectangles and writes 0 pixels; a same-width label change presents exactly one rectangle
of area 168 and, with the patch list reused, accounts for 0 allocations; a hover or focus change
damages only the affected widget; typing one character damages at most the entry's 1800-pixel box;
a checkbox flip, a progress change, a caret move and a wheel step each damage exactly one rectangle
and account for 0 allocations; removing one row of 200 damages only rectangles at or below it; and a
full 800x600 repaint writes 945223 pixels over 480000, a factor of 1.969 (under the factor 2 the
budget allows).

**Allocation accounting.** The backend exposes `frame_alloc_count()`, `frame_alloc_bytes()` and
`arena_high_water()`, and `soft_backend.apply_diff`/`core.diff_into` clear and refill a caller-owned
patch list instead of building a new one. The counters are scoped to the backend: the steady-state
backend paths allocate nothing (pinned at 0 by the budget test), but the application's `view`
rebuilds its tree through Kelp's one allocator on every message, and that allocation is not counted.
The documented cap on one frame's patch list is `MAX_FRAME_PATCHES = 4096`.

**The benchmark.** `examples/polyp-bench/` runs five scenarios (200-row scroll, type 100 characters,
toggle 50 checkboxes, resize 800x600 to 1024x768, full repaint) five times each and prints frames,
pixels, presented rectangles, backend allocations and median/worst microseconds per frame. On the
development host with the release `kelpc` the worst frame of any scenario is 6.6 ms, under 16 ms:

```sh
nix-shell /path/to/dev.nix --run 'cd kelpc && ../target/release/kelpc build ../examples/polyp-bench -o ../target/polyp-bench'
( ulimit -v 1048576; ulimit -t 20; timeout 20 ./target/polyp-bench )
```

**The frame deadline.** `soft_backend.frame_budgeted(..., budget_us)` returns
`Frame.Deferred(tick)` with the remaining damage when the platform's microsecond clock passes the
budget, so a slow frame degrades into two frames instead of a stall. Each damage rectangle is
painted whole or left for the next call. `frame(platform, budget_us = 0)` is the unbounded form.
The `Platform` trait gained `now_us()` for it.

## Using the library

Kelp has **no manifest for a library**, and a project's modules resolve only
under its own `src/`, so a consumer has to do two things by hand. Both are the
packaging limitation, not a design choice:

1. **Put the sources where `impl` can reach them.** The convention Coral fetches
   dependencies into is `src/deps/<name>/`, and a path ending in `/lib` under
   `deps/` is named after its directory, so `impl "deps/polyp/lib"` is reached
   as `polyp.*`. Copy `examples/polyp/` to `src/deps/polyp/`, or link it:

   ```sh
   mkdir -p src/deps
   ln -s ../../../polyp src/deps/polyp      # examples/polyp-counter does this
   ```

   The library's own `impl "deps/polyp/core"`, `"deps/polyp/gtk"` and
   `"deps/polyp/queue"` lines already spell that location, so the copy has to
   land exactly there. `core.kp` and `headless_backend.kp` are pure Kelp and
   need no `[extern.*]` section; `gtk_backend.kp` is the one that calls the GTK
   binding.

2. **Paste the `[extern.*]` sections into the consumer's `coral.toml`.** A
   library has no manifest of its own, so the native symbols, the header, the
   `pkg-config` package and the capability domain live in the consumer's
   manifest. The module ids are the same `impl` paths as above:

   ```toml
   [extern."deps/polyp/gtk"]
   domain = "ui"
   library = "gtk-4"
   header = "gtk/gtk.h"
   pkg_config = "gtk4"
   symbols = ["gtk_init", "gtk_window_new", "gtk_window_set_title", "gtk_window_set_default_size", "gtk_window_set_child", "gtk_window_present", "gtk_window_get_focus", "gtk_window_set_focus", "gtk_box_new", "gtk_box_append", "gtk_box_remove", "gtk_box_insert_child_after", "gtk_box_reorder_child_after", "gtk_box_set_spacing", "gtk_label_new", "gtk_label_set_markup", "gtk_widget_set_size_request", "gtk_entry_new", "gtk_editable_set_text", "gtk_editable_get_text", "gtk_button_new_with_label", "gtk_button_set_label", "gtk_check_button_new_with_label", "gtk_check_button_set_label", "gtk_check_button_set_active", "gtk_progress_bar_new", "gtk_progress_bar_set_fraction", "gtk_separator_new", "gtk_scrolled_window_new", "gtk_scrolled_window_set_child", "gtk_widget_set_margin_start", "gtk_widget_set_margin_end", "gtk_widget_set_margin_top", "gtk_widget_set_margin_bottom", "gtk_widget_set_hexpand", "gtk_widget_set_vexpand", "gtk_widget_set_halign", "gtk_widget_set_valign", "gtk_widget_add_css_class", "gtk_widget_remove_css_class", "gtk_css_provider_new", "gtk_css_provider_load_from_string", "gdk_display_get_default", "gtk_style_context_add_provider_for_display", "gtk_widget_is_ancestor", "g_object_unref", "g_main_context_iteration", "g_timeout_add_full", "connect=g_signal_connect_data", "connect_ret=g_signal_connect_data", "g_signal_handler_block", "g_signal_handler_unblock"]

   [extern."deps/polyp/queue"]
   domain = "ui"
   library = "c"
   header = "polyp_queue.h"
   include_dirs = ["src/deps/polyp/include"]
   symbols = ["polyp_post", "polyp_poll", "polyp_post_edit", "polyp_poll_edit", "polyp_poll_edit_text", "polyp_post_focus", "polyp_poll_focus"]
   ```

   `header` is one key per section, which is why the queue is its own module:
   `polyp_queue.h` is a header of the library and `gtk/gtk.h` is GTK's, and a
   section names one.

3. **Grant the domain.** Every GTK and queue call is in the `ui` domain
   (`plan/ffi-display-domain.md`; the only accepted grant is the wildcard), so
   the consumer's import of the library delegates it:

   ```kelp
   impl "deps/polyp/lib" caps(ui = ["*"])
   ```

   The library holds `ui` and the application does not need to: `main` never
   names a GTK call.

The builder syntax needs one compiler property: a generic function's
`scoped fun(...)` parameter has to be able to type a trailing block header from
the call's expected return type. That landed with v1 (fixture
`scoped_block_generic`); on an older compiler the block header is refused with
"no function type at this position", and the same view has to name the block
parameters on a `fun(...)` literal instead.

## Versions

`examples/polyp/` and `examples/polyp-counter/` are v3, and `examples/polyp-todo/`
is the v3 keyed-list program, whose model, view and update live in
`src/app.kp` so `examples/polyp-core-test` can run the same program on the
headless backend. `examples/polyp-v0/` is a frozen copy of the v0
library and `examples/polyp-counter-v0/` is the v0 counter, whose `src/deps/polyp`
link points at `polyp-v0`, so the two examples do not fight over the
`column`/`row` names and both build:

```sh
examples/polyp-counter/smoke.sh
examples/polyp-counter-v0/smoke.sh
examples/polyp-todo/smoke.sh
```

`examples/polyp-budget-test/` and `examples/polyp-bench/` are stage 5's two programs: the first is
the work-budget test the suite pins, the second is the benchmark whose table `plan/polyp-v3.md`
quotes. Both are hosted and headless, and both run in the plain development shell.

## Build and smoke test

```sh
examples/polyp-todo/smoke.sh
```

The scripts re-enter `dev-gtk.nix` (the shell beside this repository; override
with `POLYP_DEV_GTK`), build `kelpc`, compile the example with the **default**
(`llvm`) backend, and run the binary under `xvfb-run -a` with
`GSK_RENDERER=cairo GDK_BACKEND=x11` and `POLYP_SMOKE=1`. In that mode the
program schedules its own input and prints a deterministic summary.

The counter **must print exactly** `1\ntyped\n1`: the count, the text the
entry's constructor produced, and one entry built across all the patches. The
todo smoke parses that summary and asserts the model, the creation marks
(no widget is created after the adds), `removed == 5`, `live == created - removed`,
the focus present and surviving, and the entry's text. **Both** smoke runs build
the todo again with `KELPC_ORACLE=c` and check the same output, so the C backend
and the default one behave identically. A crash, a hang (`timeout 60`) or
anything else on stdout fails the script.

## Known limits

- **No lazy or virtualized lists.** Every child in the model is a live widget;
  a list of 10,000 rows builds 10,000 widgets. A keyed patch makes the updates
  cheap, not the first build.
- **No animation and no transitions.** A patch sets a property in one step;
  there is no interpolator and no per-frame hook. A tick paints a response to
  input, not a curve; the frame deadline bounds how much of one tick that takes,
  it does not add animation.
- **No whole-frame zero-allocation claim.** The soft backend's own steady-state
  frame path allocates nothing and the patch list is reused, but `view`
  rebuilds the application tree through Kelp's one allocator on every message.
  The counters in `soft_backend` measure the backend, not the application, and
  `plan/polyp-v3.md` says so.
- **Text is the embedded 5x7 ASCII face.** One weight, integer scales 1 to 4,
  no vector text, no sub-pixel positioning, no shaping; a byte outside 32..126
  is a replacement box and a UTF-8 sequence is one box per byte.
- **No GPU.** The renderer is the integer software raster in `examples/polyp/soft/`;
  the only hardware fact is the platform's pixel pointer.
- **Theming on the soft backend is a `Theme` datac and a class table.**
  `polyp.css` is GTK's hook; the soft backend's equivalent is `soft_backend`'s
  `Theme` value and its `class_style` comparisons, not a stylesheet parser.
- **No accessibility.** There is no focus announcement, role tree or
  screen-reader bridge.
- **GTK and the soft backend both draw.** `core` is backend-neutral and two
  backends are shipped: GTK for a desktop and the pure-Kelp soft backend for a
  framebuffer, hosted or bare metal.
- **An id still leans on creation order.** An application that posts a raw id
  (rather than carrying a `Msg`) is relying on the order widgets are created in;
  the smoke runs do, deliberately, because they have no user to click anything.
- **No exclusive captures.** A builder block may read the borrowed state but
  `state.count += 1` inside a block is still a language error
  (`plan/scoped-blocks.md`); a mutation goes through a `Msg`.
- **No library manifest.** See "Using the library" above: the consumer pastes
  the `[extern.*]` sections and places the sources under `src/deps/polyp/`.
  This is the one thing that makes Polyp awkward to consume, and it is the same
  for every Kelp library.
- **A display is required to *draw*.** The GTK smoke tests still run a real
  window under Xvfb, and the focus tests read the toplevel's focus widget rather
  than `gtk_widget_has_focus`, which needs a window manager to hand over global
  input focus. The loop itself no longer needs one: `examples/polyp-core-test`
  runs the whole todo program on the headless backend with no display.
