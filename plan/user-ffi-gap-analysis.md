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
| 9 | ~~**Layout (B-2).** No `repr(C)`.~~ **Closed (kernel stage 2).** `repr(c)` (the default, now guaranteed), `repr(packed)` and `repr(align(N))` on a `datac`, the compile-time queries `size_of`/`align_of`/`offset_of`, and explicit discriminants on a field-less enum now work on both backends. A `datac` in an `extern fun` signature still has to be reached by name a header can spell: a user type's C name is the module-mangled `struct_<...>`, so `ffi_layout` passes the address as `Ptr<Packed>` and the header takes it as `const void*`. `plan/layout-control.md` is the design. | roadmap B-2 | A struct crossing by value (`GdkRGBA`, `GtkAllocation`) now has a defined layout; the remaining work is the header spelling above, which is gap 3's generated prototype, still deliberately not planned. |

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
  describes. **Polyp v1 landed on that shape:** `examples/polyp/lib.kp` has a `Children` builder
  with block-taking `column`/`row`, a sized/bold `Text`, `Spacer` and a text `Entry` whose
  `fun(String).R: M` constructor is a plain function value, with ordinal focus retention for the
  entry; the v0 library is frozen in `examples/polyp-v0/` and both smoke tests pass on both
  backends.
- **Event loop.** `g_application_run` blocks and calls back on the main thread. The callback
  thunk must therefore run on that thread, and the closure must stay alive until GTK's
  destroy-notify fires.

**Both are answered by Polyp v0** (`examples/polyp/`, built on this tree): the sharing mechanism is a
ring of ints in a C header (`include/polyp_queue.h`) whose two `static inline` functions are bound
through an `[extern.*]` section exactly like any other symbol (`library = "c"`, so nothing to link),
and the loop drains it after each `g_main_context_iteration(null, TRUE)`, on the same thread that a
GTK callback runs on. No callback captures state at all; a callback carries an int. Both backends
run it: `examples/polyp-counter/smoke.sh` opens a real GTK window under Xvfb and asserts the count.

**Polyp v2 closes nothing in this list; it works around two language limits.** An owned field read
by value out of a `mut` receiver is a move out of a borrow, so the builder's one-shot pending key and
options live in `Box`es and are `take()`n; and a `pd var` counter written by a call chain conflicts
with itself (two functions cannot hold one passdown exclusively), so the patch counters are
`pd val AtomicI32` cells bumped through their shared-borrow operations, the shape `syntax.md` 12.5
recommends. A programmatic widget write feeding back as the signal that produced it is handled with
`g_signal_handler_block` around the write. `plan/polyp-v2.md` is the note.

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
  A compiler-shipped std module could not declare one when this gap closed (it has no manifest); that is now
  lifted too (`plan/tls-handle.md`): `resolve.rs` admits an `extern type` in a module `stdlib.rs` ships, and
  `std/tls` uses it for `TlsHandle` in place of the `usize` it used to carry.
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

## What kernel stage 7 (the freestanding `str` surface) changed for this list

Nothing is closed, and one gap gets smaller. A freestanding program can now
call `str.len`, `str.byte`, `str.eq`, `str.starts_with` and `str.ptr` with no
libc behind them, so a binding whose native side hands back a `const char*` (a
`str` parameter or return, which stage 4 already accepts) can be *inspected* in
the kernel instead of only passed around. Nothing about the binding mechanism
itself changed: `str.ptr` is the address of the first byte, so it is the same
`Ptr<u8>` a driver already writes out, and reading through it still needs the
module on the `[unsafe] modules` list. The other `std/str` functions stay out
of reach in that mode, so anything that allocates or formats a string from a
native value still needs a different shape.

## What kernel gap 2 (`Ptr.from_fn`) changed for this list

One shape a native binding reaches for is now expressible: the address of a
Kelp `fun` as a `Ptr<u8>`, which is what a callback table, an interrupt vector
or a native API taking a function pointer wants to hand over. It is safe
(`Ptr.from_fn` needs no `unsafe`), the address is the linker's real symbol, and
`align(N)` on the `fun` sets the alignment the native side needs. It does not
close gap 4: a `Ptr<u8>` is one machine word with no signature and no
environment, so it is not a `move closure(...)` and the native side still has
to know the ABI itself.

## What `defer` (MK9-S6-M1) changed for this list

Nothing is closed. A `defer` action is an ordinary expression, so a deferred
release of a bound native resource (`defer widget.destroy()`) is expressible the
day gap 1's handle has such a method, and the same action is duplicated at every
exit of its block. What it does not give a binding is a *drop*: a `defer` is
written where the resource is taken, and a panic skips it, so the leak a native
handle has when a frame panics is unchanged (`plan/defer.md`).

## What user `Drop` (MK9-S6-M1) changed for this list

One thing is closed for a *wrapper*: a `datac` holding a native handle can now
declare `fun mut T.drop()` and release the handle there, so the release runs at
every ordinary drop point the value reaches (end of scope, an early `return`, a
container element, a closure environment), which a `defer` could not do because
it is written at the acquisition site. What it does not change is the *binding*
side: a raw `extern type` handle is `Copy` and has no destructor, so the leak a
handle has when a frame panics is unchanged, and an `extern fun` that hands out
an owned pointer still needs a wrapper type before `Drop` can describe it.

## What kernel gap 4 (the freestanding runtime essentials) changed for this list

Nothing is closed, and one freestanding obstacle is gone. A binding whose
signature or wrapping code makes LLVM emit an aggregate `memcpy`/`memmove`/
`memset`/`memcmp` call no longer leaves an undefined symbol for `ld.lld` to
report: the compiler defines all five weakly and as loops in every freestanding
object, and a kernel that wants its own implementation only has to put a strong
definition in the image (`plan/freestanding.md`). The binding mechanism itself
is unchanged: a freestanding `extern fun` still has no trampoline, and the
symbol is still the linker's or the script's to resolve.

## Status update (release goal 1 follow-up): a shipped std module may declare a handle

Gap 1's last restriction is gone. `resolve.rs` admits an `extern type` in a module `stdlib.rs` ships
(`stdlib::source`), not only in a module with its own `[extern."<module id>"]` section; a project module
still needs that section, and `tests/projects/ffi_handle_no_section` pins the refusal. `std/tls` uses it
for `pub extern type TlsHandle`, `TlsStream.handle` is now a `TlsHandle` rather than a `usize`, and
`std/http`/`std/ws` carry `option.Option<tls.TlsHandle>` so `None`, not `0`, is "not TLS". Fixtures
`tls_handle_not_an_integer.kp` and `tls_handle_not_forged.kp` show the two integer directions are refused.
`plan/tls-handle.md` is the record.

## What kernel stage 19 (the freestanding standard subset) changed for this list

Nothing is closed, and one thing a binding could use is now reachable: a freestanding program may
call `std/math`'s integer `abs`/`min`/`max`/`clamp` and, with the allocator pair, `std/list` and
the `std/option`/`std/result` callback combinators, where before the whole module was refused. A
binding that has to normalise a length or clamp an index in a kernel can now write that in Kelp
rather than in an `asm` block. The rest of the list is unchanged: a freestanding `extern fun`
still has no trampoline, `library`/`header`/`pkg_config` still have no effect in that mode, and
`std/mem` (whose `alloc_raw` is libc `malloc`) is still refused by name.

## Status update (2026-10-03, Polyp v3 stage 1)

- **No gap here is closed, and none is newly blocked.** Polyp v3 split its library into a
  backend-neutral `core.kp`, a `gtk_backend.kp` and a `headless_backend.kp`, which is a library
  answer to "no platform but GTK" rather than a compiler one: the core holds no `extern` call at
  all, and the GTK binding is unchanged from v2 (the same `[extern."deps/polyp/gtk"]` and
  `[extern."deps/polyp/queue"]` sections, the same symbols). `plan/polyp-v3.md` is the design and
  `examples/polyp-todo/smoke.sh` still runs both backends.
- One consequence worth recording for a *future* backend: a backend interface that passes plain
  data (`Props`, `Patch`) and integers is expressible with what gap 1 (the opaque handle), gap 4
  (the C-ABI callback) and gap 6 (the `ui` domain) already provide, which is why the split needed
  no language change. A raster or soft backend that needs a `datac` to cross an `extern fun`
  boundary would still hit gap 9.

## Status update (2026-10-02, Polyp v3 stage 2)

- **No FFI gap is closed and none is newly blocked.** The software rasterizer
  (`examples/polyp/soft/`) is pure Kelp with no `extern` call at all: the only external facts a
  drawing primitive has are a `Ptr<u32>` and a stride, which is the shape gap 1 already admits,
  and a caller with a `static mut [u32; N]` reaches the pixels through `Ptr.from_mut` and
  `cast<u32>()`. `debug.dump_ppm` uses `std/fs`, which is the ordinary gated namespace, not an
  FFI gap. `plan/polyp-v3.md` is the design.
- One Kelp *language* shape the stage hit and worked around, worth recording for any I/O-heavy
  binding: a method call with explicit type arguments cannot be written directly on a call
  expression. `Ptr.from_ref(&buf).cast<u32>()` is refused ("'cast' does not take type arguments:
  a method's type parameters come from its receiver"), while
  `val b: Ptr<[u32; N]> = Ptr.from_ref(&buf)` followed by `b.cast<u32>()` is accepted. Bind the
  receiver first. Nothing about FFI depends on this, but it will bite a generated binding.
- The per-program C object's trampoline is untouched, so an application still pastes the same
  `[extern.*]` sections as v2/v3 stage 1.

## Status update (2026-10-03, Polyp v3 stage 3)

- **No FFI gap is closed and none is newly blocked.** The soft backend, its layout, its input
  and both platform implementations (`examples/polyp/soft_backend.kp`,
  `examples/polyp/soft/`) contain no `extern` call at all: the only external facts are a
  `Ptr<u32>` framebuffer and a stride, which is gap 1's shape, and a platform is an ordinary
  trait implemented in Kelp. `soft/ppm_platform.kp` uses `std/fs`, which is the ordinary gated
  namespace. `plan/polyp-v3.md` stage 3 is the design and `examples/polyp/PLATFORM.md` the
  contract a custom OS implements.
- Four Kelp shapes the stage worked around, worth recording for a binding or any generic
  driver, none of them FFI:
  - A `mut` parameter of an enum type cannot be reassigned in place (`t = value` is refused);
    the loop passes the tree in by value and returns it in a `Tick` variant instead.
  - A borrow taken in an `if`/`while` condition is live for the whole statement, and a method
    receiver is borrowed before its arguments are evaluated, so `self.damage.add(rect_of(self.root...))`
    is refused. A local binding between the two reads keeps both sound without weakening a check.
  - A `mut S` parameter passed to `view(s: S)` binds `S` to `mut S` when the state is a `Copy`
    scalar, so a generic frame driver wants an owned `datac` state, which is what every Polyp
    program's `Model` already is.
  - A discarded non-`unit` return value in statement position is an error (`if` without an
    `else` must be `unit`); `val _ = call()` is the discard the paint code uses.

## Status update (2026-10-04, Polyp v3 stage 4)

- **No FFI gap is closed and none is newly blocked.** The two freestanding framebuffer projects
  (`examples/polyp-fb-riscv64`, `examples/polyp-fb-x86_64`) declare no `extern fun` at all: a UART
  is a `Ptr<u8>` under an `mmio` grant on riscv64 and an `in`/`out` pair on x86-64, and
  `plan/kernel-ub-notes.md`'s stage-4 note carries what that costs. The one new gap is a
  *freestanding* one, not an FFI one, and it is recorded there: a keyable user type with a `String`
  field pulls in the hosted `kelp_map_key_eq`.
- Three Kelp shapes the stage worked around, none of them FFI:
  - A function type's parameters are unnamed: `emit: fun (line: str)` is a parse error, `emit:
    fun (str)` is the spelling, so a trait is the only place a callback parameter gets a name.
  - A trait method's implementation has to spell its parameters exactly as the trait does, unused
    name included (`present(fb: ...)` cannot be written `present(_surface: ...)`).
  - An argument that borrows a temporary cannot be a call argument (`emit(step_line(...).as_str())`
    is refused); bind it to a local first. The same shape the stage-2 note records for
    `Ptr.from_ref(&buf).cast<u32>()`.

## Status update (2026-10-04, Polyp v3 stage 5)

- **No FFI gap is closed and none is newly blocked.** Stage 5 adds no `extern fun`: the deadline is
  an ordinary trait method (`Platform.now_us`) implemented in Kelp, and the benchmark's clock is
  `std/time`, the ordinary gated namespace. The work-budget and benchmark programs live in
  `examples/polyp-budget-test` and `examples/polyp-bench`.
- Four Kelp shapes the stage worked around, none of them FFI:
  - A method cannot declare its own type parameters: `fun mut SoftBackend.apply_diff<M>(...)` is a
    parse error at the `<`, because a method's type parameters come from its receiver. The
    reusable-patch entry points are free generic functions (`soft_backend.apply_diff(backend, old,
    new)`) that take the backend as a `mut` parameter, which is the same shape the raster's tests
    already use.
  - A free function cannot be `mut` (`fun mut name(...)` is refused outside a receiver), which is
    correct but easy to reach for: `apply_diff` is `fun apply_diff(...)` with a `mut SoftBackend`
    parameter.
  - Consuming an enum by value in a `when` needs a `move` parameter. `fun tick_tree<M>(t:
    core.Tick<M>)` matched on `t` yields a `&Node`, because a non-`move` parameter is borrowed;
    `t: move core.Tick<M>` is the spelling that binds the payload by value. The `frame` loop avoids
    it by matching on the call's temporary directly.
  - A `mut` parameter of a `Copy` array (`[i64; N]`) is passed by value, not borrowed, so a helper
    that sorts a caller's samples cannot write back. The benchmark sorts a local copy inside the
    reporting function, the same workaround the stage-2 note records for mutable scratch images.
