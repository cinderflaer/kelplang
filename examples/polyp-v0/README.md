<!-- Frozen v0: the v0 library and example, kept so both APIs build. The live library is examples/polyp/. -->

# Polyp v0

A declarative native UI library for Kelp, over GTK4. The application describes
what the screen should be for the current state; Polyp owns the loop and builds
the widgets. It wraps GTK's own widgets rather than drawing, so a Polyp window
is a GTK window.

This is v0. It is small on purpose and the [limits](#known-limits) below are
real.

## What a program looks like

```kelp
impl "deps/polyp/lib" caps(ui = ["*"])
impl "std/fmt"

datac Counter(var count: i32)

enum Msg {
    Inc
    Dec
    Reset
}

fun view(state: Counter).R: polyp.Node<Msg> {
    var kids: List<polyp.Node<Msg>> = List.empty()
    kids.push(polyp.text("a counter"))
    kids.push(polyp.button("+1", Msg.Inc))
    kids.push(polyp.button("-1", Msg.Dec))
    kids.push(polyp.button("reset", Msg.Reset))
    return polyp.column(kids)
}

fun update(state: mut Counter, msg: Msg) {
    when msg {
        Inc -> { state.count += 1 }
        Dec -> { state.count -= 1 }
        Reset -> { state.count = 0 }
    }
}

fun main() {
    val final = polyp.run(
        title = "counter",
        width = 320,
        height = 200,
        init = Counter(count = 0),
        view = view,
        update = update,
    )
    print(final.count)
}
```

`examples/polyp-counter/` is that program, complete and buildable.

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

That makes the two declarative claims hold literally:

- **`view` is a pure function of the state.** It takes the state and returns a
  `Node<Msg>`; it is called on every state change and can do nothing else.
- **`update` is an exhaustive `when` over the `Msg` enum.** Adding a variant and
  forgetting to handle it is a compile error.

The proposal (`plan/polyp-ui.md`) sold this as "borrow-checked callbacks", where
the checker would prove a captured borrow is still valid when the callback runs.
A callback cannot capture a borrow at all, so that feature has no v0 shape; what
v0 has instead is the stronger, simpler statement above, and no stale-closure bug
is expressible in the first place.

## API

```kelp
pub enum Node<M> {
    Column(val children: List<Node<M>>)
    Row(val children: List<Node<M>>)
    Text(val text: String)
    Button(val label: String, val on_click: M)
}

pub fun column<M>(children: move List<Node<M>>).R: Node<M>
pub fun row<M>(children: move List<Node<M>>).R: Node<M>
pub fun text<M>(text: move String).R: Node<M>
pub fun button<M>(label: move String, on_click: move M).R: Node<M>

pub fun run<S, M>(title: str, width: i32, height: i32, init: move S,
                  view: fun(S).R: Node<M>, update: fun(mut S, M)).R: S

pub const QUIT_ID: i32
pub fun schedule_post(delay_ms: u32, id: i32)
pub fun schedule_quit(delay_ms: u32)
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
- Ids are handed out in tree order: the `i`-th `Button` in a depth-first walk is
  id `i`. `QUIT_ID` is `-2`, because `polyp_poll` answers `-1` for an empty queue.
- `schedule_post` / `schedule_quit` post an id after a delay. They exist for
  programs that have to run with no user (the smoke test is one), and they are
  the only reason `g_timeout_add_full` is bound.

`Node<M>` is a generic enum that recurses through `List<Node<M>>`, which the
compiler accepts as written: a `List` element lives in the list's own heap
buffer, so the type has a finite size and no `Box` is needed. `Node<M>` itself
carries application data (`M`, `String`) and is moved apart as it is built, so
nothing keeps it after the widgets exist.

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
   symbols = ["gtk_init", "gtk_window_new", "gtk_window_set_title", "gtk_window_set_default_size", "gtk_window_set_child", "gtk_window_present", "gtk_box_new", "gtk_box_append", "gtk_label_new", "gtk_button_new_with_label", "g_object_unref", "g_main_context_iteration", "g_timeout_add_full", "connect=g_signal_connect_data", "connect_ret=g_signal_connect_data"]

   [extern."deps/polyp/queue"]
   domain = "ui"
   library = "c"
   header = "polyp_queue.h"
   include_dirs = ["src/deps/polyp/include"]
   symbols = ["polyp_post", "polyp_poll"]
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

## Build and smoke test

```sh
examples/polyp-counter/smoke.sh
```

The script re-enters `dev-gtk.nix` (the shell beside this repository; override
with `POLYP_DEV_GTK`), builds `kelpc`, compiles `examples/polyp-counter/` with
the **default** (`llvm`) backend, and runs the binary under `xvfb-run -a` with
`GSK_RENDERER=cairo GDK_BACKEND=x11` and `POLYP_SMOKE=1`. In that mode the
program schedules its own input: post id 0 (Inc), post id 0 (Inc), post id 1
(Dec), then quit after 250 ms. The only line on stdout must be the final count,
`1`. A crash, a hang (`timeout 60`) or anything else on stdout fails the script.

The C backend (`KELPC_ORACLE=c`) produces the same count; both backends are
supported, LLVM is the default.

## Known limits

- **Whole-child rebuild, no reconciliation.** Every state change builds a new
  tree, rebuilds every widget from scratch and replaces the window's child.
  There is no diffing, so widget identity is not preserved.
- **No focus retention.** A rebuild replaces the child, so the widget that had
  focus does not have it afterwards.
- **No text input.** `Text` is a label; there is no entry widget and no way for
  a message to carry text the user typed.
- **No lazy or keyed lists**, no styling, no layout beyond `Column`/`Row` and
  GTK's default spacing (6 px).
- **Ids are positional.** They are re-derived from tree order on every render,
  so an application that leans on raw ids (rather than letting a button carry a
  `Msg`) is leaning on an implementation detail. `polyp-counter`'s smoke run
  does, deliberately, because the smoke run has no user to click anything.
- **No library manifest.** See "Using the library" above: the consumer pastes
  the `[extern.*]` sections and places the sources under `src/deps/polyp/`.
  This is the one thing that makes Polyp v0 awkward to consume, and it is the
  same for every Kelp library.
- **GTK is required to build and run.** There is no pure-Kelp proof of the loop
  without a display; the smoke test runs a real GTK window under Xvfb, which is
  what this machine can do.
