# GTK4 spike notes (`examples/polyp-spike/`)

Compile-only spike. Built on the **default (LLVM) backend** with
`nix-shell overnight/dev-gtk.nix --run 'kelpc build examples/polyp-spike -o spike'`, GTK 4.22.4. The binary links
`libgtk-4`, `libgobject-2.0` and `libglib-2.0`. It was not run: this machine has no display.

## What builds

One program exercising `pkg_config = "gtk4"`, `header = "gtk/gtk.h"`, six `extern type` handles with a parent
chain (`Window: Widget: Object`), a symbol rename (`connect=g_signal_connect_data`), `str` in both directions, a
`move closure(gtk.Widget)` bound **directly** to `g_signal_connect_data`, and a null `MainContext` handle passed
to `g_main_context_iteration`: the closure expands to `(c_handler, data, destroy)`, GLib's own order, and the
generated call is cast to the expansion's types, so no hand-written C seam is needed. The first version of this
spike needed a `static inline` seam and the C backend; both limits were the compiler's and are gone.

## Remaining blockers, most important first

1. **The display domain (gap 6).** `process` is a stand-in; options in `libraries/polyp.md` (kelp wiki).
2. **Callback arity.** The closure's parameter list is checked against scalars, `str` and handles only; GTK handlers
   with struct arguments cannot be expressed.
3. **No variadics (gap 7).** Not hit here; `g_object_set` would.
4. **No `repr(C)` (gap 9).** Not hit: every GTK value crossing is an opaque handle, a scalar or a `str`.
