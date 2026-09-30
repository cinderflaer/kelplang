# GTK4 spike notes (`examples/polyp-spike/`)

Compile-only spike. Built with
`nix-shell overnight/dev-gtk.nix --run 'KELPC_ORACLE=c kelpc build src/main.kp'`,
GTK 4.22.4, GLib 2.88.3, gcc 15.3.0. The binary was not run: this machine has no display.

## What built

`kelpc build src/main.kp` under `KELPC_ORACLE=c` exits 0 and writes `build/main`, which links
`libgtk-4.so.1`, `libgobject-2.0.so.0` and `libglib-2.0.so.0` with no unresolved symbols.

The spike exercises, in one program: `pkg_config = "gtk4"` against a real system package,
`header = "gtk/gtk.h"`, `include_dirs` for a project header, four `extern type` handles
(`Application`, `Window`, `Widget`, `Argv`) named across module boundaries, two symbol renames
(`application_unref=g_object_unref`, `connect_clicked=kelp_gtk_connect_clicked`), `str` in both
directions of an extern signature, and one `move closure(gtk.Widget)` parameter whose
`(thunk, env, destroy)` expansion reaches `g_signal_connect_data`.

## What did not build, and why

1. **A direct `g_signal_connect_data` binding does not compile.** Its `GCallback` and
   `GClosureNotify` parameters do not match the fixed thunk and destroy signatures the
   `move closure(...)` expansion emits, and the generated call carries no cast:

   ```
   build/main.c:1991:62: error: passing argument 3 of 'g_signal_connect_data' from incompatible pointer type [-Wincompatible-pointer-types]
     expected 'GCallback' {aka 'void (*)(void)'} but argument is of type 'void (*)(void *, void *)'
   build/main.c:1991:129: error: passing argument 5 of 'g_signal_connect_data' from incompatible pointer type [-Wincompatible-pointer-types]
     expected 'GClosureNotify' {aka 'void (*)(void *, struct _GClosure *)'} but argument is of type 'void (*)(void *)'
   ```

   This is gap 4 (`codegen_c.rs::write_user_extern`, `write_callback_glue`) plus gap 3's open
   prototype half: nothing in the manifest or the language can say "this parameter is a
   `GCallback`" or "cast it". GLib's own `g_signal_connect` is a macro that inserts exactly those
   two casts, so the practical workaround is the `include/kelp_gtk_seam.h` `static inline` the
   spike uses, which hardcodes the casts and therefore the closure's arity.

2. **The default (LLVM) backend refuses the program**, which is documented behaviour:

   ```
   error: callbacks in user externs are not supported on the llvm backend yet; build this project with the C backend (KELPC_ORACLE=c), or pass the callback to a Kelp function instead
   ```

   Gap 4, C-backend only.

3. **GTK4 has no `gtk_window_new`** (that is GTK3), so only `gtk_application_window_new` is bound.
   A GTK fact, not a Kelp gap.

4. **`g_object_unref` takes a `gpointer`.** Handles are nominal and do not widen (gap 1), so the one
   C symbol is bound once per handle type, using the symbol-rename form. `g_application_run`'s
   `char**` has the same problem, and solving it needs a `g_strsplit` round trip plus a fifth
   handle type: there is no null handle.

## Remaining blockers, most important first

1. **Callback parameter types / casts (gap 4, gap 3).** Any GLib signal API needs this. Either emit
   the `GCallback` and `GClosureNotify` casts for a `move closure(...)` call, or let the manifest
   declare the native types so the thunk can be written as them.
2. **The display domain (gap 6).** `process` is a stand-in; the choice is `plan/ffi-display-domain.md`.
3. **Callbacks are C-backend only (gap 4).** A GTK program cannot use the default backend.
4. **No null, no handle widening (gap 1).** Costs an API call for every nullable pointer argument.
5. **No variadics (gap 7).** Not hit here; `g_object_set` and `gtk_init`-style calls would.
6. **No `repr(C)` (gap 9).** Not hit here: every GTK value crossing is an opaque handle, a scalar
   or a `str`.
