# User FFI (B-1 / MK9-S1-M1): what is built and what a GTK binding still needs

*Gap analysis read from the code at `a58da81` (kelpc 26.09.5). Not a Mark; `roadmap.md` owns the schedule.*

## What already exists

`roadmap.md` B-1 still opens with "`extern fun` exists but is compiler-only". That is out of date:
the first slice of MK9-S1-M1 is in the tree.

- A project's `coral.toml` may carry `[extern."<module id>"]` with `domain` (one of the
  capability domains), `library`, `symbols` (`resolve.rs`, `ExternModuleDecl`), and an optional
  `header` (a C header; see gap 3 below).
- `infer.rs` accepts a user `extern fun` only when its bare Kelp name is in that module's
  `symbols`, and a `symbols` entry may rename it: `"kelp_name=c_symbol"` (see gap 2).
- `codegen_c.rs::write_user_extern` emits a trampoline that forwards every parameter to the C
  symbol the manifest named (the same name, unless the entry renamed it). `driver.rs` adds
  `-l<library>` for every `[extern.*]` section.
- The module is gated like a stdlib one: an importer needs `caps(<domain> = [...])`.
- Tests: `tests/projects/ffi_getpid`, `ffi_denied`, `ffi_bad_domain`, `ffi_bad_symbol`,
  `ffi_rename`, `ffi_bad_rename`.

## Gaps, in the order a window would hit them

| # | Gap | Where | Why it blocks GTK |
|---|-----|-------|-------------------|
| 1 | ~~**No pointer or opaque-handle type.**~~ **Closed.** `extern type Widget` is an opaque, pointer-sized `Copy` value: `Ty::Handle`, legal only in a module with an `[extern."<module id>"]` section (`resolve.rs`), exported and named across modules like a `pub datac`. No dereference, no arithmetic, no conversion to or from an integer, and `print` refuses one by name. `extern type Window: Widget` gives a handle a parent chain (same module or imported, cycles refused): a child converts implicitly to any ancestor at an argument, a binding and a return, at zero runtime cost, and `==`/`!=` accept an ancestor/descendant pair. `Widget.null()` and `w.is_null()` carry and test the null handle, and `Window.cast(w)` reinterprets an ancestor down to `Window` with no runtime check; all three are legal only in an extern module. C is `typedef void* <mangled>` with `NULL`, LLVM is `ptr` with `ptr null`. Fixtures `ffi_handle`, `ffi_handle_upcast`, `ffi_handle_null`, `ffi_handle_no_section`, `ffi_handle_bad_downcast`, `ffi_handle_null_outside`, `ffi_handle_print`. | `types.rs`, `infer.rs`, `codegen_c.rs`, `codegen_llvm.rs` | `gtk_window_new` returns a `GtkWidget*`; `usize` would carry it at the ABI level but gives no type safety between a window and a button, and GTK's null pointer arguments and subclass upcasts have no spelling. |
| 2 | ~~**Symbol name is the Kelp name.** "1:1 name mapping" is stated as deliberate.~~ **Closed.** A `symbols` entry may be `"kelp_name=c_symbol"`; `infer.rs` still matches the allowlist on the Kelp name and maps it to the C symbol for `codegen_c.rs` (`resolve.rs::ExternModuleDecl::symbol_of`, tests `ffi_rename`/`ffi_bad_rename`). | `resolve.rs`, `infer.rs` | Every call reads `gtk.gtk_window_new`, and two libraries that share a symbol name cannot both be bound. |
| 3 | **No declaration of the native symbol.** ~~The trampoline calls it with no prototype and no `#include`; the fixtures only use libc symbols the runtime preamble already declares.~~ **Include half done:** a `header = "gtk/gtk.h"` key in `[extern.*]` is validated and emitted as `#include <...>` after the preamble, deduplicated and sorted (`resolve.rs`, `codegen_c.rs`, `codegen_llvm.rs`). The prototype-generation half stays open: A GTK call still relies on the header for its signature, which cannot express `const char*` versus `char*` or `gpointer`. | `resolve.rs`, `codegen_c.rs`, `codegen_llvm.rs` | GTK needs the header; a generated prototype needs the pointer types of gap 1. |
| 4 | ~~**Callbacks.**~~ **Closed on both backends.** A user `extern fun` parameter written `move closure(P0, P1, ...).R: T` expands in place to three C arguments `(thunk, env, destroy)`, GLib's `(func, user_data, destroy_notify)`. `thunk` is a generated `T thunk(P0, ..., void* user_data)` that calls `c->fn(c->env, ...)`; `env` is a heap box holding the closure value; `destroy` runs the closure's `drop` on that environment and frees the box. The closure is moved into the native side, so `destroy` is the only thing that frees the environment, and a native side that never calls it leaks. The closure's parameters and return must be scalars, `bool`, `str`, an opaque handle or `unit`, with no `move`/`mut` parameter, and an extern takes at most one. The native call is made through a function pointer of the expansion's own type, so `GCallback`/`GClosureNotify` parameters need no cast in Kelp. On the LLVM backend the closure crosses as a pointer to the caller's stack slot (24 bytes, like any aggregate past two eightbytes) and the per-program C object's trampoline boxes it. | `infer.rs`, `codegen_c.rs::write_user_extern`, `codegen_llvm.rs::program_c_source` | `g_signal_connect_data`'s destroy-notify argument maps onto the closure's `drop`. |
| 5 | **Link flags are only `-l<name>`.** ~~No `-L`, no `-I`~~ **Closed:** `include_dirs`/`library_dirs` in `[extern.*]` are validated, resolved against the `coral.toml` directory, deduplicated, sorted, and emitted as `-I`/`-L` on both backends (`resolve.rs`, `driver.rs`, fixture `ffi_include_dir`). A `pkg_config = "<package>"` key runs one `pkg-config --cflags --libs` over every package the sections name (sorted, deduplicated) and puts the flags on the compile and the link line of both backends (`resolve.rs`, `driver.rs`, fixture `ffi_pkg_config`; an unresolvable package fails the build). `driver.rs` also always links `-lssl -lcrypto`. | `resolve.rs`, `driver.rs` | GTK needs many include and library paths that `pkg-config gtk4` supplies. On NixOS they also have to be in `shell.nix`. `gtk4` is not present on this machine (`pkg-config --modversion gtk4` fails). |
| 6 | ~~**Capability domain.** Only `fs`, `net`, `process`, `env`, `time`, `rand`; the design pass explicitly forbids new names in this pass.~~ **Closed.** `ui` is the seventh domain in `stdlib.rs::DOMAINS`, decided as option 2 of `plan/ffi-display-domain.md`. A `ui`-assigned module is gated like any other; no display call carries an argument that names the display, so the only accepted grant is `ui = ["*"]` and `resolve.rs::validate_cap_grant` refuses every other pattern list. Fixtures `ffi_ui_domain`, `ffi_ui_pattern`, `ffi_ui_denied`. | `stdlib.rs::DOMAINS`, `resolve.rs`, roadmap B-1 | A window is authority over a display server (X11/Wayland socket), which fits none of the other six. Using `net` would work mechanically but would mislabel what the module can do, and a per-display name needs runtime-derived authority (roadmap C-1), which is not built. |
| 7 | ~~**Variadics.**~~ **Closed.** A *user* `extern fun` may write `...` as its last parameter (`extern fun g_object_set(obj: Object, first: str, ...).R: unit`), and a call passes any number of extra arguments after the fixed ones, each of an integer, `bool`, `f32`, `f64`, `str` or opaque-handle type (anything else is a compile error naming the argument). `infer.rs` records the extra types per call and mints one deterministic fixed-arity C wrapper per distinct list (`kelp_va_<mangled fn>_<mangled types>(fixed..., extra...) { return symbol(fixed..., extra...); }`); `tir::ExprKind::VariadicCall` carries the wrapper name and the call site calls it, so the C compiler applies the default promotions (an `i32`/`u8` travels as `int`, an `f32` as `double`) and neither backend emits a variadic call. Fixtures `ffi_variadic` (run on both backends), `ffi_variadic_not_last`, `ffi_variadic_bad_arg`. | `lexer.rs`, `parser.rs`, `infer.rs`, `tir.rs`, `codegen_c.rs`, `codegen_llvm.rs` | `g_object_set` and `printf`-shaped bindings. |
| 8 | **Test coverage is one scalar fixture.** `ffi_getpid` takes no arguments and returns `i32`; `ffi_handle` now passes a pointer-shaped opaque handle. | `tests/projects/` | No test passes a string or a callback, and I found no fixture that exercises a user extern on the LLVM backend specifically (C is covered). |
| 9 | **Layout (B-2).** No `repr(C)`. | roadmap B-2 | Not needed for a minimal window (handles are opaque), but needed as soon as a struct crosses by value (`GdkRGBA`, `GtkAllocation`). |

## Two consequences for Polyp

- **Callback ownership.** A closure owns its environment and captures by move or copy, never by
  borrow (`plan/syntax.md` 7.4). `plan/polyp-ui.md` sells "borrow-checked callbacks", but a
  callback cannot borrow `state`; it can only own a copy or a moved value. Several callbacks that
  all update one piece of state need a sharing mechanism I did not find in the language.
  **Solved for the read-only builder shape** by `plan/scoped-blocks.md`: a `scoped fun(...)`
  parameter takes a literal or trailing block written at the call site, whose captures are shared
  borrows released with the call, so a view callback can read a borrowed `state`. An *exclusive*
  capture (the `state.count += 1` shape) and a callback that outlives the call (the native
  `extern` shape, gap 4) still need a `move` closure or a shared cell, as the Polyp v0 note below
  describes.
- **Event loop.** `g_application_run` blocks and calls back on the main thread. The callback
  thunk must therefore run on that thread, and the closure must stay alive until GTK's
  destroy-notify fires.

**Both are answered by Polyp v0** (`examples/polyp/`, built on this tree): the sharing mechanism is a
ring of ints in a C header (`include/polyp_queue.h`) whose two `static inline` functions are bound
through an `[extern.*]` section exactly like any other symbol (`library = "c"`, so nothing to link),
and the loop drains it after each `g_main_context_iteration(null, TRUE)`, on the same thread that a
GTK callback runs on. No callback captures state at all; a callback carries an int. Both backends
run it: `examples/polyp-counter/smoke.sh` opens a real GTK window under Xvfb and asserts the count.

## Suggested order

1. **Spike (no compiler change):** a `[extern."gtk"]` project that opens a window through
   `usize` handles and `str` titles, with `gtk4` added to `shell.nix`. This shows how far items
   1-3 and 5 can be worked around before any language change.
2. ~~**Symbol rename + `header` key** in `[extern.*]` (items 2, 3).~~ **Both done:** item 2 in
   this pass, item 3's include half earlier.
3. ~~**Opaque handle type** (item 1)~~ **Done:** `extern type Widget`, `Copy`, no arithmetic; `extern type Window: Widget` adds parent-chain upcasting, `null()`/`is_null()` and the unchecked `cast()` down.
4. ~~**C-ABI callback with `user_data`** (item 4)~~ **Done on both backends:** the `move closure(...)` expansion above.
5. ~~**`pkg-config` in the manifest** (item 5)~~ **Done.**
6. ~~**Domain decision** (item 6), taken with the owner of C-1/C-3, before anything ships.~~
   **Done:** option 2, the `ui` domain, in `plan/ffi-display-domain.md`.

Trailing blocks (`plan/trailing-closure.md`) are independent of all of them.

## Status update (2026-09-30)

Closed on `main`: gap 1 (the opaque handle), gap 2 (symbol rename), gap 3 (the `header` key; see below), gap 5 (`include_dirs`/`library_dirs` and `pkg_config`).
- **Gap 1 (opaque handle):** `extern type Widget`, an opaque pointer-sized `Copy` value with `==`/`!=` and no
  `print`. It is legal only under an `[extern."<module id>"]` section, exported like a `pub datac`; C spells
  it `void*` behind a per-type typedef, LLVM spells it `ptr`. `extern type Window: Widget` adds a parent chain
  (declared earlier in the module or imported, cycles are errors); a child converts implicitly to any ancestor
  at an argument, a binding and a return with no runtime cost, and equality accepts an ancestor/descendant pair.
  `Widget.null()`/`w.is_null()`/`Window.cast(w)` are the null handle, the null test and the unchecked downcast,
  legal only in an extern module. Fixtures `ffi_handle_upcast`, `ffi_handle_null`, `ffi_handle_bad_downcast`,
  `ffi_handle_null_outside`, on both backends.
- **Gap 3, prototype generation: not planned.** The `header` key makes the real declaration visible, so generating
  a prototype from the Kelp signature would only risk a second, conflicting declaration.
- **Gap 6 (domain):** **closed.** Option 2 of `plan/ffi-display-domain.md` is implemented: `ui` is the
  seventh domain, the gate is the domain gate alone, and `ui = ["*"]` is the only accepted grant
  because a call in this domain carries no argument to match. No-grant and any other pattern list are
  compile errors. `examples/polyp-spike` is switched to it.
- **Gap 4 (C-ABI callbacks), both backends:** a user `extern fun` parameter `move closure(P0, P1, ...).R: T` expands in place to
  `(thunk, env, destroy)`. `thunk` reorders the native `(P0, ..., void* user_data)` into the closure's own `(void* env, P0, ...)`;
  `env` is a heap box holding the closure value; `destroy` runs the closure's drop on its environment and frees the box. The
  closure is moved to the native side, so `destroy` is the only thing that frees the environment (a native side that never calls
  it leaks) and the Kelp side must not drop it. Restrictions are compile errors: the closure's parameters and return are a scalar,
  `bool`, `str`, an opaque handle or `unit`; no `move`/`mut` parameter; at most one closure parameter. LLVM is the default backend and C the
  oracle, so both must work: the LLVM declaration takes the closure as a pointer to the caller's slot, the trampoline in the
  per-program C object (`write_extern(..., closure_by_ref = true)`) reads through it, and the native call is cast to the
  expansion's types. Fixtures `ffi_callback` (run on both backends), `ffi_callback_move_param`, `ffi_callback_two`.
- **In progress:** nothing on this list.

## Status update (2026-09-30, gap 7)

- **Gap 7 (variadics): closed on both backends.** A user `extern fun` may end its parameter list with
  `...`; a call passes extras of integer, `bool`, `f32`, `f64`, `str` or opaque-handle type, with no
  implicit conversion, and anything else is a compile error naming the argument. The typechecker
  mints one deterministic fixed-arity C wrapper per distinct list of extra types and the call site
  calls it, so the C compiler does the variadic call on both backends. The wrapper is emitted through
  the existing user-extern path (a synthesized `extern fun` in `tir::Program::functions` +
  `extern_ffi_symbols`), so the C backend's `write_extern` and the LLVM backend's per-program C object
  share one emitter. Fixtures `ffi_variadic` (0, 1 and 3 extra `i32`s plus a mixed `f32`/`str` call,
  run on both backends), `ffi_variadic_not_last` (parse error) and `ffi_variadic_bad_arg` (type error).
  A variadic extern cannot be generic: nothing would specialize the wrapper's signature per
  instantiation.

## Status update (2026-09-30, later)

- **Polyp v0** (`examples/polyp/`) is the first real consumer of the `ui` domain and of a
  `header`-only binding: `gtk.kp` binds 15 GTK symbols (two `connect` shapes mapping to one C symbol,
  with `move closure(...)` callbacks for `clicked`, `close-request` and `g_timeout_add_full`), and
  `queue.kp` binds two `static inline` functions, which need no library at all. It builds and runs on
  both backends. Nothing in the compiler changed for it: the gaps this file lists were already
  enough, and gap 7 (variadics) is still not needed for a window.
