# User FFI (B-1 / MK9-S1-M1): what is built and what a GTK binding still needs

*Gap analysis read from the code at `a58da81` (kelpc 26.09.5). Not a Mark; `roadmap.md` owns the schedule.*

## What already exists

`roadmap.md` B-1 still opens with "`extern fun` exists but is compiler-only". That is out of date:
the first slice of MK9-S1-M1 is in the tree.

- A project's `coral.toml` may carry `[extern."<module id>"]` with `domain` (one of the six
  capability domains), `library`, `symbols` (`resolve.rs`, `ExternModuleDecl`), and an optional
  `header` (a C header; see gap 3 below).
- `infer.rs` accepts a user `extern fun` only when its bare name is in that module's `symbols`.
- `codegen_c.rs::write_user_extern` emits a trampoline that forwards every parameter to the C
  symbol of the same name. `driver.rs` adds `-l<library>` for every `[extern.*]` section.
- The module is gated like a stdlib one: an importer needs `caps(<domain> = [...])`.
- Tests: `tests/projects/ffi_getpid`, `ffi_denied`, `ffi_bad_domain`, `ffi_bad_symbol`.

## Gaps, in the order a window would hit them

| # | Gap | Where | Why it blocks GTK |
|---|-----|-------|-------------------|
| 1 | **No pointer or opaque-handle type.** `Ty` has scalars, `str`, `Bytes`, `String`, `View`, `Fn`, `Closure`, arrays and user types. | `types.rs` | `gtk_window_new` returns a `GtkWidget*`. `usize` would carry it at the ABI level but gives no type safety between a window and a button. |
| 2 | **Symbol name is the Kelp name.** "1:1 name mapping" is stated as deliberate. | `resolve.rs`, `codegen_c.rs` | Every call reads `gtk.gtk_window_new`, and two libraries that share a symbol name cannot both be bound. |
| 3 | **No declaration of the native symbol.** ~~The trampoline calls it with no prototype and no `#include`; the fixtures only use libc symbols the runtime preamble already declares.~~ **Include half done:** a `header = "gtk/gtk.h"` key in `[extern.*]` is validated and emitted as `#include <...>` after the preamble, deduplicated and sorted (`resolve.rs`, `codegen_c.rs`, `codegen_llvm.rs`). The prototype-generation half stays open: A GTK call still relies on the header for its signature, which cannot express `const char*` versus `char*` or `gpointer`. | `resolve.rs`, `codegen_c.rs`, `codegen_llvm.rs` | GTK needs the header; a generated prototype needs the pointer types of gap 1. |
| 4 | **Callbacks.** `Ty::Fn` is a plain function pointer with Kelp's signature; `Ty::Closure` is a `{fn, env, drop}` fat pointer. | `types.rs`, `codegen_c.rs::write_closure` | GTK wants a C function pointer plus a `void* user_data`. A closure would need a generated C-ABI thunk that unpacks `env`. `g_signal_connect_data`'s destroy-notify argument maps well onto the closure's `drop`. Nothing generates such a thunk. |
| 5 | **Link flags are only `-l<name>`.** No `-L`, no `-I`, no `pkg-config`. `driver.rs` also always links `-lssl -lcrypto`. | `driver.rs` | GTK needs many include and library paths that `pkg-config gtk4` supplies. On NixOS they also have to be in `shell.nix`. `gtk4` is not present on this machine (`pkg-config --modversion gtk4` fails). |
| 6 | **Capability domain.** Only `fs`, `net`, `process`, `env`, `time`, `rand`; the design pass explicitly forbids new names in this pass. | `stdlib.rs::DOMAINS`, roadmap B-1 | A window is authority over a display server (X11/Wayland socket), which fits none of them. Using `net` would work mechanically but would mislabel what the module can do. This needs a decision, not code (roadmap C-1/C-3). |
| 7 | **Variadics.** Not supported. | `codegen_c.rs`, roadmap B-1 | `g_signal_connect` is a macro over the non-variadic `g_signal_connect_data`, and `gtk_init` and `g_application_run` are not variadic, so a minimal window avoids this; `g_object_set` and `printf` do not. |
| 8 | **Test coverage is one scalar fixture.** `ffi_getpid` takes no arguments and returns `i32`. | `tests/projects/` | No test passes a string, a pointer or a callback, and I found no fixture that exercises a user extern on the LLVM backend specifically (C is covered). |
| 9 | **Layout (B-2).** No `repr(C)`. | roadmap B-2 | Not needed for a minimal window (handles are opaque), but needed as soon as a struct crosses by value (`GdkRGBA`, `GtkAllocation`). |

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
2. **Symbol rename + `header` key** in `[extern.*]` (items 2, 3). Small, contained in
   `resolve.rs` and `codegen_c.rs`.
3. **Opaque handle type** (item 1): `extern type Widget`, a non-`Copy` or `Copy` decision to be
   made, no arithmetic, no null.
4. **C-ABI callback with `user_data`** (item 4): the real design work.
5. **`pkg-config` / `-L` / `-I` in the manifest** (item 5).
6. **Domain decision** (item 6), taken with the owner of C-1/C-3, before anything ships.

Trailing blocks (`plan/trailing-closure.md`) are independent of all six.
