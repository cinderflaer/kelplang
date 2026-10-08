# Polyp

A declarative UI library for Kelp. Your code describes the screen for the
current state. Polyp runs the loop and patches the live widget tree.

It has one backend-neutral core and three backends. The GTK backend wraps
GTK's own widgets, so a Polyp window is a GTK window. The headless and soft
backends draw nothing to a display, so the same program can be tested with
no display at all.

- **Declarative:** `view` is a function of the state. You never edit widgets.
- **Elm-shaped:** `update` is an exhaustive `when` over your `Msg` enum.
- **Patched, not rebuilt:** keyed children keep their widgets, focus and scroll.
- **Testable:** the loop and whole programs run on the headless backend.
- **Small core:** `core/core.kp` names no GTK symbol and uses no `unsafe`.

## Build

Build and run the examples from the Kelp repository. The smoke scripts
re-enter `dev-gtk.nix` (the shell beside this repository; override with
`POLYP_DEV_GTK`), build `kelpc`, compile with the default `llvm` backend,
and run under `xvfb-run`:

```sh
examples/polyp-todo/smoke.sh      # GTK, with GSK_RENDERER=cairo GDK_BACKEND=x11 and POLYP_SMOKE=1
examples/polyp-counter/smoke.sh
```

Display-free tests run in the plain development shell, on both compiler
backends:

```sh
nix-shell /path/to/dev.nix --run 'cd kelpc && cargo test --test compile_and_run polyp'
nix-shell /path/to/dev.nix --run 'cd kelpc && KELPC_ORACLE=c cargo test --test compile_and_run polyp'
```

Smoke runs schedule their own input and print a deterministic summary.
The counter must print exactly `1\ntyped\n1`. The todo smoke checks the
model, the creation marks, `removed == 5` and the focus. Both smoke runs
also build with `KELPC_ORACLE=c` and check the same output.

### Add Polyp to a project

Kelp has no manifest for a library, so a consumer does three things by hand.

1. **Put the sources where `impl` can reach them.** Dependencies live in
   `src/deps/<name>/`. Copy the repository root there, or link it:

   ```sh
   mkdir -p src/deps
   ln -s ../../../polyp src/deps/polyp      # the examples in the Kelp repo do this
   ```

   The library's own `impl "deps/polyp/core/core"`, `"deps/polyp/backends/gtk/gtk"`
   and `"deps/polyp/backends/gtk/queue"` lines spell that location, so the copy
   must land exactly there.

2. **Paste the `[extern.*]` sections into the consumer's `coral.toml`.** The
   native symbols, the header, the `pkg-config` package and the capability
   domain live in the consumer's manifest:

   ```toml
   [extern."deps/polyp/backends/gtk/gtk"]
   domain = "ui"
   library = "gtk-4"
   header = "gtk/gtk.h"
   pkg_config = "gtk4"
   symbols = ["gtk_init", "gtk_window_new", "gtk_window_set_title", "gtk_window_set_default_size", "gtk_window_set_child", "gtk_window_present", "gtk_window_get_focus", "gtk_window_set_focus", "gtk_box_new", "gtk_box_append", "gtk_box_remove", "gtk_box_insert_child_after", "gtk_box_reorder_child_after", "gtk_box_set_spacing", "gtk_label_new", "gtk_label_set_markup", "gtk_widget_set_size_request", "gtk_entry_new", "gtk_editable_set_text", "gtk_editable_get_text", "gtk_button_new_with_label", "gtk_button_set_label", "gtk_check_button_new_with_label", "gtk_check_button_set_label", "gtk_check_button_set_active", "gtk_progress_bar_new", "gtk_progress_bar_set_fraction", "gtk_separator_new", "gtk_scrolled_window_new", "gtk_scrolled_window_set_child", "gtk_widget_set_margin_start", "gtk_widget_set_margin_end", "gtk_widget_set_margin_top", "gtk_widget_set_margin_bottom", "gtk_widget_set_hexpand", "gtk_widget_set_vexpand", "gtk_widget_set_halign", "gtk_widget_set_valign", "gtk_widget_add_css_class", "gtk_widget_remove_css_class", "gtk_css_provider_new", "gtk_css_provider_load_from_string", "gdk_display_get_default", "gtk_style_context_add_provider_for_display", "gtk_widget_is_ancestor", "g_object_unref", "g_main_context_iteration", "g_timeout_add_full", "connect=g_signal_connect_data", "connect_ret=g_signal_connect_data", "g_signal_handler_block", "g_signal_handler_unblock"]

   [extern."deps/polyp/backends/gtk/queue"]
   domain = "ui"
   library = "c"
   header = "polyp_queue.h"
   include_dirs = ["src/deps/polyp/backends/gtk/include"]
   symbols = ["polyp_post", "polyp_poll", "polyp_post_edit", "polyp_poll_edit", "polyp_poll_edit_text", "polyp_post_focus", "polyp_poll_focus"]
   ```

   `header` takes one key per section, which is why the queue is its own
   module: `polyp_queue.h` belongs to the library, `gtk/gtk.h` to GTK.

3. **Grant the `ui` capability.** Every GTK and queue call is in the `ui`
   domain, and the only accepted grant is the wildcard. The application
   imports the facade and delegates the capability to it:

   ```kelp
   impl "deps/polyp/backends/gtk/polyp/lib" caps(ui = ["*"])
   ```

   The facade holds `ui`, so `main` never names a GTK call. A program writes
   `polyp.column { ... }`, not `gtk.column`. The namespace is the facade's
   directory, `polyp`.

The core and the headless backend are pure Kelp. They need no `[extern.*]`
section. Only `gtk_backend.kp` calls the GTK binding.

Kelp needs a `scoped fun(...)` parameter to type a trailing block header from
the call's expected return type. That landed with fixture `scoped_block_generic`.
On an older compiler, name the block parameters on a `fun(...)` literal instead.

## Quick example

```kelp
impl "deps/polyp/core/core"
impl "deps/polyp/backends/gtk/polyp/lib" caps(ui = ["*"])
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

The full program is `examples/polyp-counter/` in the Kelp repository.
`run` returns the final state, so `main` can read what the run left behind.

The plain style of the same view still compiles. `core.Node<M>` is the tree type:

```kelp
var kids: List<core.Node<Msg>> = List.empty()
kids.push(polyp.text(String.from_str("hi")))
kids.push(polyp.button(String.from_str("+1"), Msg.Inc))
return polyp.plain_column(kids)
```

## Syntax

The snippets below are sketches of the API, not compiled programs.
Names are the ones in `core/core.kp` and the facade.

### Layout

`column` and `row` take a block. Inside it, `k` is the `Children<M>` builder.

```kelp
polyp.column { k ->
    k.row { r ->
        r.text("Name")
        r.entry(s.name.as_str(), changed)
    }
    k.spacing(12)              // spacing of the box being built
}
```

Options apply to the next child you push: `margin(n)`, `expand()`,
`align(Align.Center)` and `class("name")`.

```kelp
k.expand()
k.column { c -> c.text("fills the free space") }
```

### Widgets

| Call | Widget |
|---|---|
| `text(t)`, `text_styled(t, size, bold)` | a label; `size` in points, `0` for GTK's default |
| `spacer(h)` | reserves `h` vertical pixels |
| `button(label, msg)` | a button that posts `msg` |
| `entry(value, on_change)` | a text field |
| `checkbox(label, checked, msg)` | a check box that posts `msg` on toggle |
| `progress(permille)` | a progress bar |
| `separator()` | a horizontal rule |
| `scrolled { ... }` | a scroll window around its children |

`text_styled` escapes the text before it sets Pango markup, so `&` and `<`
show as typed.

### Events and Msg

A callback never holds the state. A button carries a `Msg`, and `update`
turns the `Msg` into a change.

```kelp
k.button("+1", Msg.Inc)
k.entry(s.draft.as_str(), changed)   // changed: fun(String).return: Msg
```

For an entry, `value` seeds the text on first build. A later patch rewrites
the text only when it differs and the entry does not hold focus, so the
user's cursor and selection are kept.

Under the hood, a button's handler captures one small integer. It posts that
integer to a fixed-size queue, and the loop maps it back to a `Msg`.

### Keys and reconciliation

Give a child a key when it is a row in a list. Keys must be unique among the
children of one container. A duplicate panics and names the key.

```kelp
k.keyed(item.id_str()) { r -> r.checkbox(item.title.as_str(), item.done, Msg.Toggle(item.id)) }
k.keyed_int(item.id) { r -> r.text(item.title.as_str()) }
```

Each rebuild is a diff. A matched widget is updated in place. A keyed child
that moved is reordered, not rebuilt. A removed child is destroyed.

### Memo

`memo` skips rebuilding a subtree whose dependencies have not changed.
Dependencies are plain values, pushed in order.

```kelp
var d = core.deps()
d.push_int(item.id)
d.push_bool(item.done)
k.memo("row", d, build_row)          // build_row: fun(&Deps).return: List<Child<Msg>>
```

The memo-aware entry points (`frame_memo` in the soft backend,
`frame_step_memo` in the core) keep a `core.MemoStore` alongside the tree.
`memo_int(key, deps, build)` is the integer-keyed form.

### Timers

The application lists the timers it wants for each state. The loop calls
`subscriptions(state)` after every turn.

```kelp
fun subscriptions(s: Counter).return: List<core.Sub<Msg>> {
    var out: List<core.Sub<Msg>> = List.empty()
    out.push(core.every(1000, Msg.Tick))     // every 1000 ms
    out.push(core.after(250, Msg.Flash))     // once, 250 ms after it first appears
    return out
}
```

A timer's identity is its position, kind and period. A timer that leaves the
list stops. Timers run on the platform's clock, which the soft backend reads
in microseconds.

### Animation

An `Anim` is plain data: a start value, an end value, a duration and an
easing. It carries no clock. The soft backend starts it the first time it
sees the value, then reads the platform clock.

```kelp
k.opacity_anim(anim.lerp(0, 1000, 300, anim.Easing.EaseOut))
k.offset_x_anim(anim.lerp(-40, 0, 200, anim.Easing.EaseInOut))
```

Opacity is in permille. The arithmetic is integer, so both backends agree
to the last unit. Animation is in the soft backend (`core/anim.kp`, `soft_backend.kp`).

### Text

The soft backend has its own text layer, with no toolkit.

- `soft/font.kp` is a hand-drawn 5x7 face for ASCII 32 to 126. It draws at
  integer scales 1 to 4.
- `soft/utf8.kp` decodes UTF-8 for the soft backend (unverified how it meets
  the 5x7 face; the v3 note says a byte outside 32 to 126 is one replacement box).
- `text_width` and `text_height` measure a string before layout.

GTK draws text through Pango, not through this layer.

### Accessibility

The soft backend builds an accessibility tree on demand. It exists only while
a caller holds it, so the frame path allocates nothing extra. Nodes have a
role, a name, a value, state flags and bounds.

The roles are `button`, `checkbox`, `entry`, `label`, `progress`, `list`,
`list_item` and `region`. Spacers and separators are not exposed.
`soft/a11y.kp` holds the module. The functions that drive a node (`a11y_act`,
`a11y_set_value`) are named in its header (unverified in the soft backend source).

I found no accessibility code in `gtk_backend.kp`, so GTK accessibility is unverified here.

### Diagnostics

`stats()` counts widgets created, patched, removed and alive. A run that
patches keeps `created` flat across state changes.

```kelp
val s = polyp.stats()     // created, updated, removed, live, memo_hits, memo_misses, view_calls
```

`focus_present()` and `focus_survived()` report whether a patch found and kept
the focus. `entry_text(index)` reads an entry's text after the run.

## Backends

| Backend | Path | Needs | Shows |
|---|---|---|---|
| **gtk** | `backends/gtk/` | the `ui` capability and a display | a real GTK 4 window |
| **headless** | `backends/headless/` | nothing | an in-memory tree, no display |
| **soft** | `backends/soft/` | nothing, no display server | a framebuffer, hosted or bare metal |

**gtk** is the desktop backend. `gtk.kp` and `queue.kp` bind GTK and the
message ring. `polyp/lib.kp` is the facade programs import. It needs the
`[extern.*]` sections and the `ui` grant from [Add Polyp to a project](#add-polyp-to-a-project).

**headless** applies the same patches to an in-memory tree. Programs run
their whole loop with no display. `examples/polyp-core-test` runs the todo
program this way.

**soft** is a framebuffer backend. Its raster is integer-only: no `f32` or
`f64`, no libc, no OS, and no allocation in the frame path. The only external
fact is a pixel pointer and a stride. `PLATFORM.md` is the contract a
platform implements.

```kelp
impl "deps/polyp/core/core"
impl "deps/polyp/backends/soft/soft/headless_platform"
impl "deps/polyp/backends/soft/soft_backend"

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

The soft modules that dereference a pixel go on the consumer's `[unsafe] modules`
list. The table below marks them. `font` and `damage` are safe.

| Module | What it holds | `unsafe` |
|---|---|---|
| `soft/fb.kp` | `Fb(pixels, width, height, stride_px)`, `Rect`, pixel store and load, `checksum` | yes |
| `soft/draw.kp` | `ClipStack` (depth 8) and the primitives: fills, strokes, rounded rectangles, lines, blends, `blit_mask8` | yes |
| `soft/font.kp` | the 5x7 ASCII face, `text_width`, `text_height`, `draw_text` | no |
| `soft/damage.kp` | `Damage`, at most 16 dirty rectangles | no |
| `soft/debug.kp` | `dump_ppm`, hosted only | yes |
| `soft/event.kp` | `Event`: pointer, scroll, key, char and `Tick` | no |
| `soft/platform.kp` | `trait Platform`: `present`, `now_ms`, `now_us`, `poll_event`, `framebuffer` | no |
| `soft/headless_platform.kp` | an in-memory framebuffer, scripted input, recorded presents | yes |
| `soft/ppm_platform.kp` | the headless platform plus a PPM dump when `POLYP_DUMP_DIR` is set | yes |
| `soft/a11y.kp` | the on-demand accessibility tree | no |
| `soft/utf8.kp` | UTF-8 decoding | no |

Layout is two passes, measure and arrange. A patch marks only the node it
wrote dirty. Painting walks the tree once per damaged rectangle. A frame
that changes nothing presents nothing.

`soft_backend.frame_budgeted(..., budget_us)` stops early when the clock passes
the budget and returns the remaining damage for the next frame. A slow frame
becomes two frames, not a stall.

The benchmark and the work-budget tests are in the Kelp repository
(`examples/polyp-bench/`, `examples/polyp-budget-test/`). They pin exact
counts: an idle frame presents 0 rectangles, and a same-width label change
presents one rectangle of 168 pixels. A full 800 by 600 repaint writes
945223 pixels against 480000, a factor of 1.969 (the budget allows 2). The
worst frame of any benchmark scenario was 6.6 ms on the development host
with the release `kelpc`.

### Bare metal

`examples/polyp-fb-riscv64/` and `examples/polyp-fb-x86_64/` (in the Kelp
repository) are `[target]` projects with no libc. They draw into a static
framebuffer and print one line per step. A test requires each image's serial
output to match the hosted run, pixel for pixel.

```sh
nix-shell /path/to/dev-kernel.nix --run 'cd kelpc && cargo test --test compile_and_run polyp_fb'
nix-shell /path/to/dev-kernel.nix --run 'examples/polyp-fb-riscv64/run.sh'
```

On riscv64 the project also registers its surface with QEMU's `ramfb` device,
so the same pixels reach a real screen. `PLATFORM.md` lists what an OS must
provide: a framebuffer, an allocator, a panic handler and a way to write a line.

## Layout

```
core/
  core.kp              the loop, tree, keyed diff, memo, timers, Backend trait
  anim.kp              integer animation values (no clock)
  diag.kp              entry-text snapshot a hosted program reads after a run
backends/
  gtk/
    gtk_backend.kp     the GTK backend (needs `ui`)
    gtk.kp             GTK bindings
    queue.kp           the message ring
    polyp/lib.kp       the facade programs import as `polyp`
    include/           polyp_queue.h
  headless/
    headless_backend.kp  in-memory tree, no display
  soft/
    soft_backend.kp    the soft backend: layout, paint, input, frame
    soft/              the raster and platform (fb, draw, font, damage, ...)
web/                   WebKelp, the browser front end (see web/README.md)
tools/
  font_gen.py          writes the glyph tables in backends/soft/soft/font.kp
docs/plan/             design notes for each version
tests/README.md        how the test projects are wired
PLATFORM.md            the contract a soft-backend platform implements
```

The test projects are Kelp projects, so their sources are in the Kelp
repository. [tests/README.md](tests/README.md) lists them and how to run them.
Design notes are in [docs/plan/](docs/plan/).

## WebKelp

[web/](web/) is WebKelp, a markdown-style language that compiles to HTML and
CSS. It lives in this repository now, with its history. The old standalone
repository is archived. Its demo and playground sites were removed, so the
`demo/` and `playground/` folders are source only. See
[web/README.md](web/README.md).

## Versions

- **v0:** the plain constructors (`plain_column`, `text`, `button`) and the
  first `run` loop. They still compile, as `examples/polyp-v0/` shows.
- **v1:** the builder syntax, `polyp.column { k -> ... }`. The block is a
  `scoped fun`, so its captures are borrows released with the call.
- **v2:** the live widget tree is patched, not rebuilt. Children carry keys.
  Added `scrolled`, `checkbox`, `progress`, `separator`, layout options and
  `polyp.css`. Design note: `docs/plan/polyp-v2.md`.
- **v3:** the library splits into `core/` and one module per backend. The
  headless and soft backends run with no display. The soft raster, the
  bare-metal proof and the work-budget benchmark are v3. Design note:
  `docs/plan/polyp-v3.md`.
- **v4:** a platform clock with timers, integer animations, UTF-8 soft text
  and editing, an on-demand accessibility tree, and memoised subtrees.
  Design note: `docs/plan/polyp-v4.md`.
- **v5 (in progress):** the layout in this README, with WebKelp merged into
  `web/`. Design note: [docs/plan/polyp-v5.md](docs/plan/polyp-v5.md).

## Limits and what v5 does not do

- **No lazy lists.** Every child is a live widget. A list of 10,000 rows
  builds 10,000 widgets. A keyed patch makes updates cheap, not the first build.
- **No whole-frame zero-allocation claim.** The soft backend's frame path
  allocates nothing, and the patch list is reused. But `view` rebuilds the
  application tree through Kelp's allocator on every message. The backend
  counters measure the backend, not the application.
- **Text is the embedded 5x7 ASCII face.** One weight, scales 1 to 4, no
  vector text, no sub-pixel positioning, no shaping.
- **No GPU.** The renderer is the integer software raster in `backends/soft/soft/`.
- **Theming on the soft backend is a `Theme` value and a class table.**
  `polyp.css` is GTK's hook. The soft backend has no stylesheet parser.
- **Accessibility and animation are in the soft backend.** The code has
  both there. The GTK backend shows neither in its source (unverified).
- **No exclusive captures.** A builder block may read borrowed state, but
  `state.count += 1` inside a block is still a language error
  (`plan/scoped-blocks.md`). A mutation goes through a `Msg`.
- **An id leans on creation order.** A program that posts a raw id, not a
  `Msg`, depends on the order widgets are created in. The smoke runs do
  this on purpose, because they have no user to click.
- **No library manifest.** The consumer pastes the `[extern.*]` sections and
  places the sources under `src/deps/polyp/`. Every Kelp library has this limit.
- **A display is needed to draw GTK.** The GTK smoke tests run under Xvfb.
  The focus tests read the toplevel's focus widget, because global input focus
  needs a window manager. The loop itself needs no display.
- **v5 is not finished.** Kelp does not yet emit HTML from raw Polyp
  (`[build] emit = "html"`; Kelp 26.10.4, lane A). The examples in the Kelp
  repository still use the old module ids. The test sources are still in the
  Kelp repository. `layout-v5` is not merged into main and not tagged.
