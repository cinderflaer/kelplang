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
| 1 | ~~**No pointer or opaque-handle type.**~~ **Closed.** `extern type Widget` is an opaque, pointer-sized `Copy` value: `Ty::Handle`, legal only in a module with an `[extern."<module id>"]` section (`resolve.rs`), exported and named across modules like a `pub datac`. No dereference, no arithmetic, no null, no conversion to or from an integer; `==`/`!=` compare two handles of the same type and `print` refuses one by name. C is `typedef void* <mangled>` with trampolines passing it through; LLVM is `ptr`. Fixtures `ffi_handle`, `ffi_handle_no_section`, `ffi_handle_print`. | `types.rs`, `infer.rs`, `codegen_c.rs`, `codegen_llvm.rs` | `gtk_window_new` returns a `GtkWidget*`; `usize` would carry it at the ABI level but gives no type safety between a window and a button. |
| 2 | ~~**Symbol name is the Kelp name.** "1:1 name mapping" is stated as deliberate.~~ **Closed.** A `symbols` entry may be `"kelp_name=c_symbol"`; `infer.rs` still matches the allowlist on the Kelp name and maps it to the C symbol for `codegen_c.rs` (`resolve.rs::ExternModuleDecl::symbol_of`, tests `ffi_rename`/`ffi_bad_rename`). | `resolve.rs`, `infer.rs` | Every call reads `gtk.gtk_window_new`, and two libraries that share a symbol name cannot both be bound. |
| 3 | **No declaration of the native symbol.** ~~The trampoline calls it with no prototype and no `#include`; the fixtures only use libc symbols the runtime preamble already declares.~~ **Include half done:** a `header = "gtk/gtk.h"` key in `[extern.*]` is validated and emitted as `#include <...>` after the preamble, deduplicated and sorted (`resolve.rs`, `codegen_c.rs`, `codegen_llvm.rs`). The prototype-generation half stays open: A GTK call still relies on the header for its signature, which cannot express `const char*` versus `char*` or `gpointer`. | `resolve.rs`, `codegen_c.rs`, `codegen_llvm.rs` | GTK needs the header; a generated prototype needs the pointer types of gap 1. |
| 4 | ~~**Callbacks.**~~ **Closed on both backends.** A user `extern fun` parameter written `move closure(P0, P1, ...).R: T` expands in place to three C arguments `(thunk, env, destroy)`, GLib's `(func, user_data, destroy_notify)`. `thunk` is a generated `T thunk(P0, ..., void* user_data)` that calls `c->fn(c->env, ...)`; `env` is a heap box holding the closure value; `destroy` runs the closure's `drop` on that environment and frees the box. The closure is moved into the native side, so `destroy` is the only thing that frees the environment, and a native side that never calls it leaks. The closure's parameters and return must be scalars, `bool`, `str`, an opaque handle or `unit`, with no `move`/`mut` parameter, and an extern takes at most one. The native call is made through a function pointer of the expansion's own type, so `GCallback`/`GClosureNotify` parameters need no cast in Kelp. On the LLVM backend the closure crosses as a pointer to the caller's stack slot (24 bytes, like any aggregate past two eightbytes) and the per-program C object's trampoline boxes it. | `infer.rs`, `codegen_c.rs::write_user_extern`, `codegen_llvm.rs::program_c_source` | `g_signal_connect_data`'s destroy-notify argument maps onto the closure's `drop`. |
| 5 | **Link flags are only `-l<name>`.** ~~No `-L`, no `-I`~~ **Closed:** `include_dirs`/`library_dirs` in `[extern.*]` are validated, resolved against the `coral.toml` directory, deduplicated, sorted, and emitted as `-I`/`-L` on both backends (`resolve.rs`, `driver.rs`, fixture `ffi_include_dir`). A `pkg_config = "<package>"` key runs one `pkg-config --cflags --libs` over every package the sections name (sorted, deduplicated) and puts the flags on the compile and the link line of both backends (`resolve.rs`, `driver.rs`, fixture `ffi_pkg_config`; an unresolvable package fails the build). `driver.rs` also always links `-lssl -lcrypto`. | `resolve.rs`, `driver.rs` | GTK needs many include and library paths that `pkg-config gtk4` supplies. On NixOS they also have to be in `shell.nix`. `gtk4` is not present on this machine (`pkg-config --modversion gtk4` fails). |
| 6 | ~~**Capability domain.** Only `fs`, `net`, `process`, `env`, `time`, `rand`; the design pass explicitly forbids new names in this pass.~~ **Closed.** `ui` is the seventh domain in `stdlib.rs::DOMAINS`, decided as option 2 of `plan/ffi-display-domain.md`. A `ui`-assigned module is gated like any other; no display call carries an argument that names the display, so the only accepted grant is `ui = ["*"]` and `resolve.rs::validate_cap_grant` refuses every other pattern list. Fixtures `ffi_ui_domain`, `ffi_ui_pattern`, `ffi_ui_denied`. | `stdlib.rs::DOMAINS`, `resolve.rs`, roadmap B-1 | A window is authority over a display server (X11/Wayland socket), which fits none of the other six. Using `net` would work mechanically but would mislabel what the module can do, and a per-display name needs runtime-derived authority (roadmap C-1), which is not built. |
| 7 | **Variadics.** Not supported. | `codegen_c.rs`, roadmap B-1 | `g_signal_connect` is a macro over the non-variadic `g_signal_connect_data`, and `gtk_init` and `g_application_run` are not variadic, so a minimal window avoids this; `g_object_set` and `printf` do not. |
| 8 | **Test coverage is one scalar fixture.** `ffi_getpid` takes no arguments and returns `i32`; `ffi_handle` now passes a pointer-shaped opaque handle. | `tests/projects/` | No test passes a string or a callback, and I found no fixture that exercises a user extern on the LLVM backend specifically (C is covered). |
| 9 | ~~**Layout (B-2).** No `repr(C)`.~~ **Closed (kernel stage 2).** `repr(c)` (the default, now guaranteed), `repr(packed)` and `repr(align(N))` on a `datac`, the compile-time queries `size_of`/`align_of`/`offset_of`, and explicit discriminants on a field-less enum now work on both backends. A `datac` in an `extern fun` signature still has to be reached by name a header can spell: a user type's C name is the module-mangled `struct_<...>`, so `ffi_layout` passes the address as `Ptr<Packed>` and the header takes it as `const void*`. `plan/layout-control.md` is the design. | roadmap B-2 | A struct crossing by value (`GdkRGBA`, `GtkAllocation`) now has a defined layout; the remaining work is the header spelling above, which is gap 3's generated prototype, still deliberately not planned. |

## Two consequences for Polyp

- **Callback ownership.** A closure owns its environment and captures by move or copy, never by
  borrow (`plan/syntax.md` 7.4). `plan/polyp-ui.md` sells "borrow-checked callbacks", but a
  callback cannot borrow `state`; it can only own a copy or a moved value. Several callbacks that
  all update one piece of state need a sharing mechanism I did not find in the language.
- **Event loop.** `g_application_run` blocks and calls back on the main thread. The callback
  thunk must therefore run on that thread, and the closure must stay alive until GTK's
  destroy-notify fires.

## Suggested order

1. **Spike (no compiler change):** a `[extern."gtk"]` project that opens a window through
   `usize` handles and `str` titles, with `gtk4` added to `shell.nix`. This shows how far items
   1-3 and 5 can be worked around before any language change.
2. ~~**Symbol rename + `header` key** in `[extern.*]` (items 2, 3).~~ **Both done:** item 2 in
   this pass, item 3's include half earlier.
3. ~~**Opaque handle type** (item 1)~~ **Done:** `extern type Widget`, `Copy`, no arithmetic, no null.
4. ~~**C-ABI callback with `user_data`** (item 4)~~ **Done on both backends:** the `move closure(...)` expansion above.
5. ~~**`pkg-config` in the manifest** (item 5)~~ **Done.**
6. ~~**Domain decision** (item 6), taken with the owner of C-1/C-3, before anything ships.~~
   **Done:** option 2, the `ui` domain, in `plan/ffi-display-domain.md`.

Trailing blocks (`plan/trailing-closure.md`) are independent of all of them.

## Status update (2026-09-30)

Closed on `main`: gap 1 (the opaque handle), gap 2 (symbol rename), gap 3 (the `header` key; see below), gap 5 (`include_dirs`/`library_dirs` and `pkg_config`).
- **Gap 1 (opaque handle):** `extern type Widget`, an opaque pointer-sized `Copy` value with `==`/`!=` and no
  `print`. It is legal only under an `[extern."<module id>"]` section, exported like a `pub datac`; C spells
  it `void*` behind a per-type typedef, LLVM spells it `ptr`.
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

## What stage n (MK9-S2) changed for this list

Nothing here is closed by it, and one gap is now reachable from a different angle. `Ptr<T>` (MK9-S2,
`plan/unsafe-and-pointers.md`) is a raw pointer type a binding could use in place of the opaque handle
of gap 1, but it is not a substitute: a `Ptr<T>` is usable only in a module `coral.toml` lists under
`[unsafe] modules`, has no type safety between two handle kinds, and `Ptr<Widget>` needs an
`extern type Widget` to exist first. Gap 1 stays the right answer for a GObject pointer, and gap 4's
callback machinery is unchanged.

## What kernel stage 3 (MK9-S8) changed for this list

Nothing is closed. The byte-order primitives (`swap_bytes`/`to_be`/`to_le` and
`Bytes.read_*_le/be`) and `Ptr.read_unaligned`/`write_unaligned` make a binding
that has to decode or build a fixed-layout native structure expressible without
hand-shifted bits, which is where gap 9 (a `datac` crossing an `extern fun`
boundary) would otherwise force a C helper. `Ptr<T>` unaligned access is also
the honest way to read a packed native field once gap 3's generated prototype
lands; today it is still only reachable from a module in `[unsafe] modules`.

## What kernel stage 4 (MK9-S4) changed for this list

Nothing is closed, and a freestanding project's binding story is narrower. A
user `extern fun` is still *accepted* in freestanding mode, but the per-program
C object that carries its trampoline is libc-backed and is not compiled there,
so the symbol is left for `ld.lld` to resolve against whatever the linker script
or the build puts beside the object. `library`, `header`, `include_dirs`,
`library_dirs` and `pkg_config` have no effect in that mode: there is no `cc`
link line to put them on. Gap 4's callback machinery is likewise hosted-only
for now. A kernel binding should therefore be a bare `extern fun` whose symbol
is defined by code linked into the image, not by a system library.

## What kernel stage 5 (MK9-S7-M1/M3) changed for this list

Nothing is closed. Inline `asm` and `naked fun` are not a binding mechanism: an
`asm` is only legal in a module the manifest lists under `[unsafe] modules`, it
names registers rather than semantics, and it is per-target, so it is the wrong
tool for calling a library and the right one for an instruction no language has
a spelling for. The one thing it changes for a binding is that a native
instruction a library's header wraps in a macro (a fence, a `pause`, a system
call register setup) is now expressible without a C helper, in the same module
that already had to be on the audit list to use `Ptr<T>`.
