# Polyp

A declarative native UI library for Kelp, over GTK4. The application describes
what the screen should be for the current state; Polyp owns the loop and builds
the widgets. It wraps GTK's own widgets rather than drawing, so a Polyp window
is a GTK window.

v1 adds the builder syntax:

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

There is no published version number; "v1" is the builder syntax plus a small
reconciliation rule and three new widgets. `examples/polyp-v0/` and
`examples/polyp-counter-v0/` are a frozen copy of v0 (see [Versions](#versions)).

## What a program looks like

```kelp
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

fun view(s: Counter).R: polyp.Node<Msg> {
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
style of the same view is still available:

```kelp
var kids: List<polyp.Node<Msg>> = List.empty()
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
  integer, finds the `Msg` stored at that position in the tree it built, calls
  `update`, re-runs `view` and rebuilds the widgets.

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
pub enum Node<M> {
    Column(val children: List<Node<M>>)
    Row(val children: List<Node<M>>)
    Text(val text: String, val size: i32, val bold: bool)
    Spacer(val height: i32)
    Entry(val text: String, val on_change: fun (String).R: M)
    Button(val label: String, val on_click: M)
}

// The builder. `column`/`row` take the trailing block; the methods on
// `Children` are the leaf and nested constructors the block calls.
pub datac Children<M>(var items: List<Node<M>>)
pub fun column<M>(block: scoped fun(mut Children<M>)).R: Node<M>
pub fun row<M>(block: scoped fun(mut Children<M>)).R: Node<M>
fun mut Children<M>.text(t: str)
fun mut Children<M>.text_styled(t: str, size: i32, bold: bool)
fun mut Children<M>.spacer(height: i32)
fun mut Children<M>.button(label: str, on_click: move M)
fun mut Children<M>.entry(value: str, on_change: fun (String).R: M)
fun mut Children<M>.column(block: scoped fun(mut Children<M>))
fun mut Children<M>.row(block: scoped fun(mut Children<M>))

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

pub const QUIT_ID: i32
pub fun schedule_post(delay_ms: u32, id: i32)
pub fun schedule_quit(delay_ms: u32)
pub fun schedule_set_entry(delay_ms: u32, index: i32, new_text: str)
pub fun entries_built().R: i32
```

- `run` opens one `GtkWindow` with the given title and default size, renders the
  first tree, and iterates the default GLib main context until a quit is posted.
  It **returns the final state** (`init` is moved in, so returning it is how a
  caller reads what the run left behind).
- `fun(S)` in `view`'s type is the "view borrows the state" shape: a parameter of
  an owned type is a shared borrow (`plan/syntax.md` 7.1), so an application
  writes `fun view(state: Counter).R: Node<Msg>` with no `&` anywhere. `update`
  is written `fun update(state: mut Counter, msg: Msg)` and matches
  `fun(mut S, M)`.
- `text` is a label. `text_styled(text, size, bold)` sets Pango markup after
  escaping the text, so a `&` or `<` is shown, not parsed; `size` is points and
  `0` means GTK's default. `spacer(height)` reserves vertical pixels.
- `entry(value, on_change)` is a text field. `value` seeds the text the first
  time the widget is built; `on_change` is a plain `fun(String).R: M` (called
  with a borrowed `String`) that turns the live text into the app's `Msg`.
- Button ids are handed out in tree order: the `i`-th `Button` in a depth-first
  walk is id `i`. `QUIT_ID` is `-2`, because `polyp_poll` answers `-1` for an
  empty queue. Entry ids are a separate space, so inserting an entry does not
  shift button ids.
- `schedule_post` / `schedule_quit` post an id after a delay;
  `schedule_set_entry` asks the loop to set an entry's text after a delay, which
  fires that entry's real `changed` signal. They exist for programs that have to
  run with no user (the smoke test is one), and they are the only reason
  `g_timeout_add_full` is bound.
- `entries_built()` is a diagnostic: the number of `Entry` widgets the process
  has built. A run that reuses its entries has a smaller count than one that
  rebuilds them, which is how the smoke test proves reconciliation.

`Node<M>` is a generic enum that recurses through `List<Node<M>>`, which the
compiler accepts as written: a `List` element lives in the list's own heap
buffer, so the type has a finite size and no `Box` is needed. `Node<M>` itself
carries application data (`M`, `String`) and is moved apart as it is built, so
nothing keeps it after the widgets exist.

## Reconciliation, exactly

v1 rebuilds the whole tree on every state change, with exactly one identity
rule:

- **An `Entry` at ordinal position `i` in the new walk reuses the `i`-th entry
  widget of the previous walk.** The previous entries are released from the old
  box first (`gtk_widget_unparent`), each keeps its own GTK reference
  (`g_object_ref_sink`), and the reused widget is appended into the new box. Its
  live text, cursor and selection survive, so **focus survives a rebuild** and
  the user's typing is not lost. The new tree's `value` is ignored for a reused
  entry; it only seeds a freshly built one.

Everything else is rebuilt, and these are the honest limits:

- `Column`, `Row`, `Text`, `Spacer` and `Button` widgets are always built
  fresh; **nothing about them is diffed**, and a widget's state (scroll,
  selection, animation) is not preserved.
- Entry identity is **ordinal**, not structural: inserting or removing an
  earlier entry shifts the ordinals, so an entry after the change is rebuilt and
  loses its focus. The one rule is not a tree diff and does not claim to be.
- Because a reused entry keeps its text, another `Msg` that changes the model's
  text does not move the widget. Keep the entry's model field in sync with what
  the entry's `Msg` reports, which is the normal Elm shape.

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

   The library's own `impl "deps/polyp/gtk"` / `"deps/polyp/queue"` lines already
   spell that location, so the copy has to land exactly there.

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
   symbols = ["gtk_init", "gtk_window_new", "gtk_window_set_title", "gtk_window_set_default_size", "gtk_window_set_child", "gtk_window_present", "gtk_box_new", "gtk_box_append", "gtk_label_new", "gtk_label_set_markup", "gtk_widget_set_size_request", "gtk_entry_new", "gtk_editable_set_text", "gtk_editable_get_text", "gtk_widget_unparent", "gtk_button_new_with_label", "g_object_unref", "g_object_ref_sink", "g_main_context_iteration", "g_timeout_add_full", "connect=g_signal_connect_data", "connect_ret=g_signal_connect_data"]

   [extern."deps/polyp/queue"]
   domain = "ui"
   library = "c"
   header = "polyp_queue.h"
   include_dirs = ["src/deps/polyp/include"]
   symbols = ["polyp_post", "polyp_poll", "polyp_post_edit", "polyp_poll_edit", "polyp_poll_edit_text"]
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

`examples/polyp/` and `examples/polyp-counter/` are v1. `examples/polyp-v0/` is
a frozen copy of the v0 library and `examples/polyp-counter-v0/` is the v0
counter, whose `src/deps/polyp` link points at `polyp-v0`, so the two examples
do not fight over the `column`/`row` names and both build:

```sh
examples/polyp-counter/smoke.sh
examples/polyp-counter-v0/smoke.sh
```

## Build and smoke test

```sh
examples/polyp-counter/smoke.sh
```

The script re-enters `dev-gtk.nix` (the shell beside this repository; override
with `POLYP_DEV_GTK`), builds `kelpc`, compiles `examples/polyp-counter/` with
the **default** (`llvm`) backend, and runs the binary under `xvfb-run -a` with
`GSK_RENDERER=cairo GDK_BACKEND=x11` and `POLYP_SMOKE=1`. In that mode the
program schedules its own input: post id 0 (Inc), post id 0 (Inc), post id 1
(Dec), set the entry's text to `typed` at 300 ms (which fires the entry's real
`changed` signal), then quit after 400 ms. stdout must be exactly
`1\ntyped\n1`: the count, the text the entry's constructor produced, and one
entry built across all the rebuilds. A crash, a hang (`timeout 60`) or anything
else on stdout fails the script.

The C backend (`KELPC_ORACLE=c`) produces the same three lines; both backends
are supported, LLVM is the default.

## Known limits

- **Whole-tree rebuild with one retention rule.** Every state change builds a
  new tree and replaces the window's child. Only an `Entry` at the same ordinal
  is reused (see [Reconciliation](#reconciliation-exactly)); there is no general
  diffing, so no other widget identity is preserved.
- **Focus retention is entry-only and ordinal.** A focused button or a focused
  entry whose ordinal changed does not keep focus.
- **No lazy or keyed lists**, no styling beyond `text_styled`, and no layout
  beyond `Column`/`Row`, GTK's default spacing (6 px) and `Spacer`.
- **Ids are positional.** They are re-derived from tree order on every render,
  so an application that leans on raw ids (rather than letting a button carry a
  `Msg`) is leaning on an implementation detail. `polyp-counter`'s smoke run
  does, deliberately, because the smoke run has no user to click anything.
- **No exclusive captures.** A builder block may read the borrowed state but
  `state.count += 1` inside a block is still a language error
  (`plan/scoped-blocks.md`); a mutation goes through a `Msg`.
- **No library manifest.** See "Using the library" above: the consumer pastes
  the `[extern.*]` sections and places the sources under `src/deps/polyp/`.
  This is the one thing that makes Polyp awkward to consume, and it is the same
  for every Kelp library.
- **GTK is required to build and run.** There is no pure-Kelp proof of the loop
  without a display; the smoke test runs a real GTK window under Xvfb, which is
  what this machine can do.
