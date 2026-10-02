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
fun changed(text: String).R: Msg = Msg.Changed(text = String.from_str(text.as_str()))

fun view(s: Counter).R: core.Node<Msg> {
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
    Entry(val text: String, val on_change: fun (String).R: M)
    Button(val label: String, val on_click: M)
    Checkbox(val label: String, val checked: bool, val on_toggle: M)
    Progress(val fraction: f64)
    Separator
    Scrolled(val children: List<Child<M>>, val spacing: i32)
}

// The builder. `column`/`row` take the trailing block; the methods on
// `Children` are the leaf and nested constructors the block calls.
pub fun column<M>(block: scoped fun(mut Children<M>)).R: Node<M>
pub fun row<M>(block: scoped fun(mut Children<M>)).R: Node<M>
fun mut Children<M>.text(t: str)
fun mut Children<M>.text_styled(t: str, size: i32, bold: bool)
fun mut Children<M>.spacer(height: i32)
fun mut Children<M>.button(label: str, on_click: move M)
fun mut Children<M>.entry(value: str, on_change: fun (String).R: M)
fun mut Children<M>.checkbox(label: str, checked: bool, on_toggle: move M)
fun mut Children<M>.progress(fraction: f64)
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
fun mut Children<M>.spacing(n: i32)
fun mut Children<M>.margin(n: i32)
fun mut Children<M>.expand()
fun mut Children<M>.align(a: Align)
fun mut Children<M>.class(name: str)

// The v0 plain constructors. `text` and `button` keep their names because
// their builder twins are methods (a separate namespace); `column`/`row` are
// the builder's free functions, and Kelp has no overloading, so the plain pair
// carries a `plain_` prefix.
pub fun plain_column<M>(children: move List<Node<M>>).R: Node<M>
pub fun plain_row<M>(children: move List<Node<M>>).R: Node<M>
pub fun text<M>(text: move String).R: Node<M>
pub fun button<M>(label: move String, on_click: move M).R: Node<M>

pub fun run<S, M>(title: str, width: i32, height: i32, init: move S,
                  view: fun(S).R: Node<M>, update: fun(mut S, M)).R: S

pub datac Stats(val created: i32, val updated: i32, val removed: i32, val live: i32)
pub fun stats().R: Stats
pub fun focus_present().R: bool
pub fun focus_survived().R: bool
pub fun entries_built().R: i32
pub fun entry_text(index: i32).R: String
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
  writes `fun view(state: Counter).R: Node<Msg>` with no `&` anywhere. `update`
  is written `fun update(state: mut Counter, msg: Msg)` and matches
  `fun(mut S, M)`.
- `text` is a label. `text_styled(text, size, bold)` sets Pango markup after
  escaping the text, so a `&` or `<` is shown, not parsed; `size` is points and
  `0` means GTK's default. `spacer(height)` reserves vertical pixels.
- `entry(value, on_change)` is a text field. `value` seeds the text the first
  time the widget is built; a patch rewrites it only when the model's text
  differs **and** the entry is not where the focus is, so a user's cursor and
  selection are not fought. `on_change` is a plain `fun(String).R: M` (called
  with a borrowed `String`) that turns the live text into the app's `Msg`.
- `checkbox(label, checked, msg)` posts `msg` on `toggled`; the application reads
  the new state from its own model, exactly as a button does. `progress(fraction)`
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
  `entry_text(index)` reads the index-th entry's text as it was when `run`
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
  hold the focus), progress fraction, spacer height, box spacing, margins,
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
  there is no interpolator and no per-frame hook.
- **No theming beyond CSS.** `polyp.css` is the only hook: no palette, no
  dark-mode switch, no per-widget style object.
- **GTK is the only *drawn* backend.** v3 puts the loop and the diff behind a
  `core.Backend` interface, so a raster or framebuffer backend is a new module
  rather than a rewrite, but the only platform that draws today is GTK.
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
