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

## Compiler

- `std/fs` has fallible twins of its mutators: `try_write`, `try_write_text`, `try_append`, `try_remove`, `try_rename`, `try_mkdir_all`, `try_symlink` and `try_remove_all` return `Option<io.IoErr>` (`None` on success) instead of panicking, and `try_list` returns `Result<List<String>, io.IoErr>`, so a tool can report a failure and carry on.
- `List.with_capacity(n)` builds an empty list with room for at least `n` elements, so a fill that knows its size allocates once. The element type comes from the binding, and the list grows past `n` like any other.
- A string literal accepts `\$` for a literal dollar sign, so `"\$KELPC is unset"` is an ordinary string value instead of a template.
- `kelpc build --incremental --timings` prints a `front-end interfaces: H hit, M miss` line. The generated `.kpi` interface artifacts are validated against the current build (a corrupt or old-format file is a miss and is rewritten atomically); the front end does not reuse them yet. A module that declares a generic item, `const`, `static`, `pd` or `drop` now folds its whole text into its interface hash, because a body edit there can change an importer.

## Standard library

- `std/process`: `process.run_inherit(program, args)` and `run_inherit_in(dir, program, args)` run a child on this process's stdin, stdout and stderr instead of pipes, so its output is live and in order and `isatty` sees the parent's terminal. The `RunResult` carries `code` and `signal` with empty `out` and `err`.
- `std/process`: `RunResult` and `Child` carry a `signal` field, the signal that killed the child (0 when it exited by itself). `code` stays `128 + signal` for a killed child, so `signal` is what tells it from a child that called `exit(143)`. `RunResult` gained the field, so a program that builds one by hand must add it.
- `std/sys`: `sys.os()` and `sys.arch()` name the build target (`"linux"`, `"x86_64"`, ...), ungated, so a program can pick a platform-specific release asset.
- `std/fs`: `fs.set_mode(path, mode)` sets a path's permission bits (the low 12 bits `Metadata.mode` reports), so a downloaded file can be made executable without a `chmod` child process.
- `std/source`: `source.file()` and `source.line()` give an ungated library the path and 1-based line of its Kelp call site, so test reports and diagnostics can name their caller on both backends.
- `std/trace`: opt-in stderr trace events and timed spans use monotonic timestamps and explicit `std/source` call locations, so programs can report profiling hooks without backend-specific tooling.
- `std/tls`: a peer that drops the connection without `close_notify` (a proxy or load balancer resetting TCP) is now end of stream in `tls.read`/`tls.read_into`, so an `https://` response ended that way no longer panics.
- `std/ws`: `wss://` works. `ws.parse_url` accepts it (port 443), and `ws.connect_secure`/`ws.try_connect_secure` run the handshake over `std/tls`; `ws.url_is_secure(u)` reads a URL's scheme.
- `std/http`: `Pool.checkout` only reuses an idle connection of the scheme asked for, so a request is never sent over a connection of the other scheme.
- `std/http`: a fallible send. `Client.try_send`, `Pool.try_send`, `Pool.try_checkout` and `try_get` return `Result<Response, HttpErr>` instead of panicking. `HttpErr` splits on whether the server could have seen the request: `Dial` (nothing sent: an unresolvable or refused dial, or a failed TLS handshake), `Exchange` (a send failed, the peer reset or closed early, a read timed out, or the response was malformed) and `Redirects` (too many). The panicking forms are now wrappers over the same code and panic with the same text. A pooled request that fails mid-exchange closes its connection instead of keeping it.
- `std/net` and `std/tls`: fallible twins of the socket and TLS I/O calls (`net.try_send_all`/`try_recv`/`try_recv_into`, `tls.try_handshake`/`try_write_all`/`try_read`/`try_read_into`), returning `NetErr`/`TlsErr` whose `reason` is the panic text.

## Compiler and runtime

- Moving the same enum payload out of an owned value twice is now refused (`use of moved value 'h.text'`); the checker had no path for a payload, so the second `when h { Two(text, _) -> text }` was accepted and freed the string twice.
- A-11 lifetime owners: direct `as_str()` and `view()` bindings keep their
  temporary owner for the binding's scope, and a `datac` may keep a `str` or
  `View` when its String/Bytes owner is moved into an earlier field of the same
  value. Borrowed containers, raw reference fields and escaping temporary
  borrows remain refused.
- Calling a mutable method through an existing `&mut` binding now reuses that
  borrow for the call. Iterator loops over mutable receivers and the shipped
  freestanding regex module no longer fail with a false exclusive-borrow error.
- Nested partial moves keep sibling fields usable and may be reinitialized on a
  `var` local; forwarding one `mut` parameter to two shared parameters for one
  call is checked as two shared reborrows.
- User `extern fun` declarations accept typed plain `fun(...)` callbacks and
  returns on both backends, so a native API can borrow a function pointer for
  one call; retained callbacks still use `move closure(...)`.
- A program that does not reach `std/tls` (directly, or through `std/http` or `std/ws`) no longer links `-lssl -lcrypto` or includes OpenSSL's headers, so `hello` builds on a host with no OpenSSL development files.
- The environment lookup table behind `std/env` frees the table a `set`/`unset` replaces instead of leaking one per call, without a lock on the read path.
- `str.byte(s, i)` bounds-checks by scanning `i + 1` bytes rather than the whole string. `str.len` is still a `strlen`; scan a long string through `s.view()`, whose `len` and `get` are constant time.
