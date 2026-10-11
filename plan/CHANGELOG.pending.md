# Pending changes

Scratch file for changes that are not released yet. Gitignored, never committed.

Add a line here when a user-visible change lands in `kelp`. **Do not touch
`wiki/changelog.md` until a version is released**: the changelog page carries
releases only.

When a version drops, write that release's entry in `wiki/changelog.md` from
this file plus the tag's commits and the Gitea release body, then wipe this file
back to this header. No need to ask first.

Keep each line short and factual: what changed, and why it matters to someone
writing Kelp. Group under the headings below; delete the headings that stay
empty on release.

## Added

- Managed mode: `[target] mode = "managed"` builds a program whose heap is collected by a GC instead of freed by hand. It needs the LLVM backend, refuses `--incremental`, and reads the collector archive from `KELP_GC_LIB`. `[target] heap_max` sets the heap cap in MiB (1 to 65536, default 256). A resource value (a file, socket or capability) in a heap slot is refused by name (on branch round-gc, not merged).
- `[target] mode = "managed"` builds a program whose heap is collected by a tracing garbage collector, so `Box`, `List`, `Map`, `String` and `Bytes` values are freed by the collector, not by drops. Resources (threads, mutexes, rwlocks, condvars) still drop at scope end, and the checker refuses one in the heap by name. Not yet: an enum held by value inside a `datac` is refused at build time, and incremental (per-unit) builds are refused.
- `[target] heap_max = N` sets a managed program's heap cap in MiB (1 to 65536, default 256). A program that needs more fails with "out of memory".
- `[build] emit = "html"` writes a Polyp view as a static HTML file. `[build] emit_entry` names the program that builds it. A Polyp HTML build needs no env grant.
- `let Some(user) = find(id) else { return Err("missing") }` (let-else) binds an enum variant's payload for the rest of the block when it matches. The else block must end in return, break, continue or panic.

- `errdefer EXPR` (6.5) runs its action only when the function leaves through its `Err` (or `None`), by `return` or `?`. A function that does not return `Result` or `Option` refuses it.

- `derive(eq, hash)` on a `datac` or `enum` writes its `eq` and `hash` methods field by field, so it is a `Map` key with no hand-written pair, and a field that has no `eq` or `hash` is refused by name (12.2).

## Changed

- A `String` argument is accepted where a `str` parameter is expected: it is borrowed for the call exactly as `.as_str()` would borrow it (11.2). A generic function or trait method's `str` parameter still needs `.as_str()`, and the compiler says so by name.

- The Polyp examples use the Polyp v5 impl ids (`deps/polyp/core/core`, `deps/polyp/backends/gtk/polyp/lib`, `deps/polyp/backends/gtk/gtk`, `deps/polyp/backends/gtk/queue`, `deps/polyp/backends/soft/...`). The `polyp` namespace is unchanged. The v0 examples are frozen.

- `Box<T>` blocks of up to 64 bytes are recycled through per-thread free lists instead of malloc and free (binary-trees runs about 1.9x faster). Programs with their own allocator are unchanged.
- The `Box` free lists read their per-thread state with the initial-exec TLS model, so each allocation skips a `__tls_get_addr` call.
- A pooled `Box` allocation or release is inline in the generated code: a call into the runtime only when a size class is empty or a thread is new.
- The `[target]` error for a missing `mode` now also names `managed`, and `heap_max` is refused unless `mode = "managed"` (on branch round-gc, not merged).
- A managed build refuses `thread.spawn` of a closure that captures values, including one bound to a name first. A thread's argument or result, and a `Mutex` or `RwLock` value, that holds a `Box`, `String`, `List`, `Map` or `Bytes` is refused by name, because the collector does not scan those records yet.
- When a thread cannot start, `thread.spawn` panics with the `pthread_create` return code (`errno N: ...`) and frees what it built, where it used to leak it.

## compiler

- The C text the `llvm` backend still emits (runtime headers, `extern fun` declarations, callback glue) moved from `codegen_c.rs` to `cdecl.rs`. Emitted output is unchanged.
- The link step uses `ld.lld`, not `cc`: a build with no `cc` can link, and `KELPC_LLD` selects the `ld.lld` binary (otherwise the first one on PATH). `cc` still compiles the `extern fun` C bodies.
- A TLS program finds OpenSSL's `libssl.so` and `libcrypto.so` through `pkg-config openssl` (then the build-time record, then the system directories), so a Nix build links without a hand-set library path.
- `kelp-rt/`, a Rust `no_std` runtime library, is added and built by `kelpc/build.rs` as the first step of moving the runtime off C. Programs are unchanged: the C runtime is still the one linked.
- `kelpc doc --format json` writes the documentation as JSON: declarations, spans, imports, trait impls and the compiler's builtin members (on branch lighthouse-support, not merged).
- `kelpc doc --text` reads one file's source from stdin in place of the file, for editor tools; it does not print text (on branch lighthouse-support, not merged).
- The corpus has golden outputs frozen from the C oracle at v26.10.5 (`kelpc/tests/golden/`), and the `llvm` backend is checked at five optimization levels against `-O0` with no C in the comparison.

- `kelpc doc --format json` writes the documentation as JSON (on branch lighthouse-support, not merged).
- `kelpc doc --text` prints the documentation as plain text (on branch lighthouse-support, not merged).
- `kelpc` builtin listing is available as JSON (on branch lighthouse-support, not merged).
- The LLVM backend checks a `List` index of `x.to_usize()` (signed `x`) with one bounds branch instead of two; the same panics and messages, faster `List.get` and `List.set` in tight loops.
- The capability checks are ported from C to Rust in `kelp-rt/src/cap.rs` (stage 1, not yet linked into `kelpc`); the checks behave as before.
- The runtime's panic, I/O flush, env cache, child table and dial are ported to Rust in the new `kelp-rt` crate (stage 1, WP2). Not linked by `kelpc` yet; behaviour is unchanged.
- The LLVM backend takes the builtin heap layouts and the runtime prototypes from one table (stage 1 WP5); no language change.

- The C backend is removed (2026-10-10): `kelpc` builds every program with the `llvm` backend. The `KELPC_ORACLE` switch no longer exists, and a set `KELPC_ORACLE` is refused by name, the same way `KELPC_BACKEND` and `--backend` already are.
- `--sanitize=` and `[build] sanitize` are refused: the sanitizers were built by the C backend, so no build can be sanitized now.

## stdlib

- `kelpc stdlib --json --dir <dir>` writes the shipped std sources to `<dir>` and prints the module list with a SHA-256 per file as JSON (on branch lighthouse-support, not merged).
- `kelpc stdlib --json` lists the standard library as JSON (on branch lighthouse-support, not merged).
- The runtime differential `kelpc/tests/run-runtime-diff.sh` and its recorded baseline from origin/main are in place, with runtime link smoke tests (on branch stage1-wp6, not merged).

## fixes

- The roadmap no longer carries a release schedule, and ISSUES no longer has release slots (landed on main, c87fb51).
- The roadmap says channels and Mutex ship now, and the download row says when it was true (landed on main, be51d2e).
- The KelpScript proposal moves to `plan/kelpscript.md` (on branch kelpscript-plan, not merged).
- The dev shell (`shell.nix`) adds `gdb`, so the debug-info test (`llvm_debug_info`) can resolve its fault backtrace.
- A managed program whose worker panics no longer stalls the next collection until the stop timeout. The worker leaves the collector in its cleanup handler.
- A thread blocked in `join`, a `Mutex` or an `RwLock` wait no longer stalls a collection on another thread, and a long runtime string scan polls the collector every 64 KiB.

## limits

- Managed lowering is WIP: its first commit (7b24451) was not compiled or verified; later commits add tests and the enum and safepoint lowering (on branch round-gc, not merged).
- Managed mode does not describe an enum stored inside a `datac` yet; such a build is a compile error (on branch round-gc, not merged).
- A-11 borrow checking is a WIP commit, not finished (on branch a11-checker, not merged).

## Wiki and docs

- Homepage: the current release line is 26.10.5 (fbdb951, merged into main by 02f7db4).
- Coral changelog: a 26.10.5 entry and a table row (9ec46bc, merged into main by c90a24b).
- Polyp changelog: a new page at `libraries/polyp-changelog.md` (9ec46bc, merged into main by c90a24b).
- Owner name changed from Juniper to Lilly (d813b32 on unreleased, merged into main by 82ec21c).
- Finished wiki lane branches deleted on the remote and locally; origin keeps only main and unreleased.
- Wiki: new section "Working without the garbage collector" (`memory.md` and `memory/`, on kelp-wiki branch `wiki-manual-memory`, not merged); its managed comparison is a placeholder until the managed build is measured.
- Wiki reorganised for finding pages: examples named by what they do, changelog pages under `changelog/`, a sidebar order file, and old paths kept as redirect stubs.

## Plans not in the repo

- `/home/juniper/Projects/kelp-work/stage1-runtime-plan.md`
- `/home/juniper/Projects/kelp-work/stage2-oracle-plan.md`
- `/home/juniper/Projects/kelp-work/stage3-lld-plan.md`
- `/home/juniper/Projects/kelp-work/kelpscript-gc-paper.md`
- `/home/juniper/Projects/kelp-work/ckp-plan.md`

## Decisions

- C is removed from the compiler in three stages (see the stage plans above).
- Every change, however small, gets one line in its repo's pending changelog.
