# Pending changes

Scratch file for changes that are not released yet. It is tracked on origin/main (it is not gitignored), so an edit to it is committed and pushed like any other file.

Add a line here when a user-visible change lands in `kelp`. **Do not touch
`wiki/changelog.md` until a version is released**: the changelog page carries
releases only.

When a version drops, write that release's entry in `wiki/changelog.md` from
this file plus the tag's commits and the Gitea release body, then wipe this file
back to this header. No need to ask first.

Keep each line short and factual: what changed, and why it matters to someone
writing Kelp. Group under the headings below; delete the headings that stay
empty on release.

## added

- `kelpc caps <project>` prints each module's capabilities and the caps each dependency reaches, sorted for CI diffs. A `[dependencies]` entry whose modules reach a domain its `coral.toml` list does not name is refused at build time.
- `[target] heap_max = N` sets a hosted program's heap cap in MiB (1 to 65536, default 256). A program that needs more stops with `out of memory`. A freestanding build refuses the key.
- `KELP_GC_STATS=1` prints one counter line to stderr at exit and before a fatal exit: collections, pause totals and maxima, live roots and charged bytes.
- `[build] emit = "html"` writes a Polyp view as a static HTML file. `[build] emit_entry` names the program that builds it. A Polyp HTML build needs no env grant.
- `let Some(user) = find(id) else { return Err("missing") }` (let-else) binds an enum variant's payload for the rest of the block when it matches. The else block must end in return, break, continue or panic.
- `errdefer EXPR` (6.5) runs its action only when the function leaves through its `Err` (or `None`), by `return` or `?`. A function that does not return `Result` or `Option` refuses it.
- `derive(eq, hash)` on a `datac` or `enum` writes its `eq` and `hash` methods field by field, so it is a `Map` key with no hand-written pair, and a field that has no `eq` or `hash` is refused by name (12.2).
- `kelpc doc --format json` writes the documentation as JSON: declarations, spans, imports, trait impls and builtin members.
- `kelpc stdlib --json --dir <dir>` writes the shipped std sources to `<dir>` and prints the module list with a SHA-256 per file as JSON.
- `[tidepool]` in `coral.toml` sets the hosted collector's first-clearing guesses (`start_low_mib`, `start_high_mib`), the headroom percent (`headroom_pct`) and the trigger rule (`trigger_rule = "two-size"`, `"last-size"` or `"gogc-style"`, default `"two-size"`). A project that sets `[tidepool]` needs the release that reads it, which the release notes name as the minimum.
- `[build] obfuscate = "names"` ("off" by default) keeps a hosted build's IR and objects in a temporary directory and links the binary with `ld.lld -s`, which drops its symbol table and debug sections. Names mode is not security: a backtrace shows no function names and the binary has no line information. A freestanding project that sets it is refused by name.

## changed

- `=>` is accepted as the same arrow as `->` in `when` arms and in a trailing block's parameter header (`{ x => ... }`). Both spellings are one token, so no existing program changes; the arrow's error messages now name both.
- The `managed` target mode is removed: a `[target] mode = "managed"` line is refused with the replacement named. Every hosted build links the tracing collector, so `Box`, `List`, `Map`, `String` and `Bytes` values are freed by the collector, not by drops. Resources (files, sockets, processes, threads, locks, channel ends, pools) still drop at scope end. Delete the mode line; every hosted file may use `block` regions beside ordinary code, and `heap_max` is read in every hosted build.
- `kelpc` builds the collector archive itself from `kelpc/gc`, so a hosted build needs no `KELP_GC_LIB`. The variable, when set, names a different archive to link.
- A resource (file, socket, process, channel end, pool, TLS stream or capability receipt) cannot go in a `Box`, `List`, `Map`, array or closure environment, or be a thread's argument: the checker refuses it by name in every hosted build.
- A hosted build accepts channels that close when their senders or receiver drop, `select` over channels where one is ready, a `Mutex` shared through one handle, a `Mutex` block value that is returned or consumed by `into_inner`, and a `Mutex` guarding a `List` pushed from several workers. A zero-capacity channel still panics (`sync.bounded: the capacity must be at least 1`).
- A hosted project whose `coral.toml` sets `[build] incremental = true` is built whole-program with a warning that names the reason (the collector's descriptor ids are per unit). The `--incremental` flag is still refused for a hosted build.
- A `String` argument is accepted where a `str` parameter is expected: it is borrowed for the call exactly as `.as_str()` would borrow it (11.2). A generic function or trait method's `str` parameter still needs `.as_str()`, and the compiler says so by name.
- The borrow checker refuses passing one `mut` parameter twice to one call, and reading a struct whole after one of its nested owned fields moved out.
- `std.http` keeps only plain connections in its pool: a TLS connection is not reused after a request, so a TLS request always dials a new connection.
- `Box<T>` blocks of up to 64 bytes are recycled through per-thread free lists instead of malloc and free (binary-trees runs about 1.9x faster). Programs with their own allocator are unchanged.
- A pooled `Box<T>` allocation or release calls `kelp_box_pool` once for the calling thread's free lists, then pops or pushes the block inline; an empty size class goes to `kelp_box_alloc`. Results are unchanged.
- When a thread cannot start, `thread.spawn` panics with the `pthread_create` return code (`errno N: ...`) and frees what it built, where it used to leak it.
- The `header` and `include_dirs` keys of `[extern."<module id>"]` are refused by name, since no C is compiled and they did nothing (`library_dirs` is unchanged). The refusals that said "in managed mode" now say "in a hosted build", and `build/install.sh` checks for `ld.lld` instead of `cc`.
- The Polyp HTML emit (`[build] emit = "html"`) imports `deps/polyp/backends/headless/polyp/lib`, so an HTML build needs no GTK binding and never builds the GTK widget tree.
- The Polyp GTK backend no longer keeps a `gtk.Widget` in its node tree, so a GTK Polyp build compiles in hosted mode (the resource refusal is unchanged). Each node holds scalars only, and its widget is found by its position among its parent's children. A new child appended to the end of a container now lands last, not first.
- The Polyp examples use the Polyp v5 impl ids (`deps/polyp/core/core`, `deps/polyp/backends/gtk/polyp/lib`, `deps/polyp/backends/gtk/gtk`, `deps/polyp/backends/gtk/queue`, `deps/polyp/backends/soft/...`). The `polyp` namespace is unchanged. The v0 examples are frozen.
- The Polyp examples' message queue is Kelp code now, so their `coral.toml` files name no C header.
- A user `extern fun` that takes or returns a `datac`, `Bytes` or `String` by value is lowered to LLVM IR with the System V register or memory classes, not to a C trampoline. A `List`, `Box`, `Map` or array passed or returned by value is still refused by name.
- The `Box` free lists read their per-thread state with the initial-exec TLS model, so each allocation skips a `__tls_get_addr` call.
- In the hosted collector bench (`kelpc/gc`, binary-trees, medians of 5 runs), total stop-the-world time falls from 217 ms to 131 ms (-39%); a 1.5 M-node live set falls from 880 ms to 633 ms (-28%), and the longest pause from 42 ms to 30 ms. Wall time is 11-12% lower on both.

- A hosted build accepts `thread.spawn`
- A hosted build refuses the 257th
- A shared-pool array above 4294967295 elements is named in its own message, and so is a count times size that overflows.
- A `KELP_GC_` variable whose value is not a whole number prints one notice and is ignored. `KELP_GC_STOP_TIMEOUT_MS=0` is refused the same way.
- The collector's root table
- The collector clears
- A hosted program's collector starts with 10 percent headroom. Its first collection starts at the midpoint of the start guesses (1 and 4 MiB) plus the headroom, not at 8 MiB, and later collections follow the two latest live sizes (the default `two-size` trigger) plus the headroom. A program with a small live set collects more often than before. `[tidepool]` in `coral.toml` changes the guesses, the headroom and the trigger rule.

## compiler

- The link step uses `ld.lld`, not `cc`: a build with no `cc` can link, and `KELPC_LLD` selects the `ld.lld` binary (otherwise the first one on PATH).
- A TLS program finds OpenSSL's `libssl.so` and `libcrypto.so` through `pkg-config openssl` (then the build-time record, then the system directories), so a Nix build links without a hand-set library path.
- The C runtime is removed: `kelp-rt/`, a Rust `no_std` crate built by `kelpc/build.rs`, is linked into every LLVM-built program. Its symbol names, signatures and struct layouts are unchanged, so a program that calls a runtime helper links as before.
- The runtime's panic, I/O flush, env cache, child table, dial and capability checks are Rust (`kelp-rt/src/`), not C text spliced into each program; behaviour and denial messages are unchanged.
- The hosted build no longer compiles any C: the `extern fun` trampolines, the native-callback glue and the capability allowlists are LLVM IR in `<out>.externs.o`, written by the program's own target machine.
- `kelpc doc --text` reads one file's source from stdin in place of the file, for editor tools.
- `kelpc doc` output has no em or en dashes: dashes copied from doc comments become hyphens.
- Compiler messages and comments use commas, colons and plain hyphens, not em or en dashes. Only the text of a message that had a dash changes.
- The LLVM backend checks a `List` index of `x.to_usize()` (signed `x`) with one bounds branch instead of two; the same panics and messages, faster `List.get` and `List.set` in tight loops.
- The C backend is removed (2026-10-10): `kelpc` builds every program with the `llvm` backend. The `KELPC_ORACLE` switch no longer exists, and a set `KELPC_ORACLE` is refused by name, the same way `KELPC_BACKEND` and `--backend` already are.
- `--sanitize=` and `[build] sanitize` are refused: the sanitizers were built by the C backend, so no build can be sanitized now.
- The TLS and network externs (the `std/tls` handshake, server context, accept and ALPN calls, and `std/net` listen) no longer need a C compiler: they are part of the Rust runtime.
- The retired `--backend`, `KELPC_BACKEND` and `KELPC_ORACLE` refusals and the `--sanitize=` refusal no longer name the removed C backend or its date. The README and the code comments that described the removed backend are rewritten to say what the build does now.

## stdlib

- `str.split` checks the first byte of the separator before comparing the whole separator, so splitting a long string does less work per position.

## fixes

- The `std/fs` tests write their scratch files under a directory unique to each test in each run, and remove it afterwards, so two kelpc suites running at once no longer delete each other's files.
- A hosted build describes an `Atomic`
- A hosted program whose worker panics no longer stalls the next collection until the stop timeout. The worker leaves the collector in its cleanup handler.
- A thread blocked in `join`, a `Mutex` or an `RwLock` wait no longer stalls a collection on another thread, and a long runtime string scan polls the collector every 64 KiB.
- The Rust runtime leaves the collector when a worker ends (also on a panic), waits in join, locks and condition waits as native, polls the collector in long text and map scans, frees a spawn whose thread cannot start and reports its errno, and the collector's personality routine lets a worker's forced unwind finish.
- A freestanding build refuses the `f32` overloads of `std` and `math` as it refuses the `f64` ones, so no soft-float compare routine is linked (`ld.lld` failed with an undefined `__ltsf2` on riscv).
- A `Mutex` or `RwLock` may hold a `List`, `String`, `Box`, `Map`, `Bytes` or a struct with a `List` field in a hosted build; the collector keeps the value alive while the lock holds it.
- The Polyp GTK backend reads a scrolled area's content box through the viewport GTK puts around it, so the todo example no longer prints GTK CRITICAL warnings; a keyed reorder emits the moves that reach the new order (old [a,b,c,d] to new [c,b,d,a] used to end [c,d,b,a]); a mount that disagrees with the view stops the program instead of writing into nothing; and `examples/polyp-todo/smoke.sh` fails on any CRITICAL line.
- A hosted TLS server no longer crashes when a request string is released.

## limits

- A hosted build refuses an enum stored inside a `datac`: the collector does not describe that layout yet, so such a program is a compile error.
- A-11 (full borrow checking) is not finished.
