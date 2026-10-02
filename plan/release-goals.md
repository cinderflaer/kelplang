# Release goals: the next release

*Set by the maintainer on 2026-09-30. `roadmap.md` still owns the schedule; this file says what the release is
for, so work can be ordered against it.*

1. **Stable TLS.**
2. **Bugfixes.**
3. **The possibility of writing a full kernel and OS stack in Kelp.**

## 1. Stable TLS: what "stable" means here

`std/tls` exists and works for the happy path (client side, OpenSSL linked, system trust store, hostname
verification, TLS 1.2 floor, no insecure flag). Stable means, in addition:

- **Failure is a value, not a panic.** A fallible API (`try_handshake`, `try_read`, `try_write`, ...) returns
  `Result<_, TlsError>` with a typed error (untrusted certificate, expired, wrong host, protocol too old,
  handshake failure, peer closed uncleanly, timeout, I/O). The panicking functions stay as thin wrappers.
- **Truncation is detected.** A connection that ends without a close_notify is an error, never a clean EOF.
- **Timeouts.** A handshake, read and write deadline.
- **Trust configuration without an insecure switch.** An optional CA file or directory, so a private CA works
  on a machine whose system store does not know it; still no way to turn verification off.
- **`wss://`** works over the same stream (the test that says "wss is refused because v1 has no tls" goes).
- **A typed handle.** `TlsStream` stops carrying the `SSL*` as a `usize` now that opaque handles exist.
- **A hardening test matrix** against a local server: expired, wrong host, self-signed, unknown CA, TLS 1.1-only
  server, mid-record close, large bodies, partial reads, many threads at once. Bugs it finds are fixed.

Out of scope for this release: server-side TLS, client certificates, session resumption, ALPN.

## 2. Bugfixes

A systematic pass, not a list: run the tests that are ignored by default, the differential fuzz between the two
backends, and the warnings the compiler emits on its own source; fix what is real, each with a regression
fixture; record what was found and what was left in `plan/bugfix-pass.md`.

## 3. A kernel and OS stack

`plan/kernel-path.md` is the ordered plan and its "kernel 0" is the first proof. "A full OS stack" asks for more
than kernel 0, so the release is done when, on top of stages 1-5, the following also exist:

- cleanup and allocation a kernel can rely on (`defer`/`Drop`, global and arena allocators, fallible allocation);
- per-CPU storage, address-space management, a syscall ABI with untrusted-pointer validation;
- a freestanding standard subset (the parts of `std` that need no libc, plus an `alloc`-style layer on the
  user's allocator): **done** (kernel stage 19, `plan/freestanding-std.md`). A freestanding project may
  implement `std/arena`, `std/atomic`, `std/frame`, `std/irq`, `std/list` and `std/math` (its integer
  functions), `std/option`, `std/result` and `std/str` (its byte-length subset), each narrowed to the items
  that resolve without libc; `std/list`, the `std/option`/`std/result` callback combinators and the `String`
  builders need the `[target] alloc`/`dealloc` pair, and every other module is refused by name with "needs a
  hosted runtime" and the admitted list;
- the capability model extended to devices (MMIO regions, IRQ lines, frames) so a driver can only touch what it
  was granted: this is what makes it a *Kelp* OS and not C with nicer syntax. All three are done. `mmio`
  (`plan/device-caps.md`) is a domain of `"0xBASE+0xLEN"` ranges or `["*"]` and a compile-time refusal of any
  freestanding `Ptr.from_addr` outside them, on the address a pointer is born at rather than on offsets applied
  later. `irq` and `frames` (`plan/irq-frame-caps.md`) are values: `irq.claim(n)` and `frame.claim(addr)` are the
  gated constructors in the shipped `std/irq` and `std/frame`, a constant argument must fall in a granted range
  (a frame must be 4 KiB-aligned), a computed one needs `["*"]`, and the constructor is sealed so another module
  cannot forge a line or a frame. Unlike `mmio`, that check applies to a hosted program and a freestanding image
  alike; `examples/kernel0-caps` boots one image that claims exactly one of each and prints them;
- stack guards and a defined panic path with no OS underneath;
- a second architecture target (x86-64) next to riscv64, and a test story that boots an image under QEMU.

Not required for the release: a particular kernel design, a filesystem, a network stack, a userland. The
language and toolchain have to make writing them possible.

## Follow-ups

- ~~**A typed `extern type` handle for `std/tls`.**~~ **Done** (`plan/tls-handle.md`). `resolve.rs` admits an
  `extern type` in a compiler-shipped module (`stdlib.rs`'s source table; a project module still needs its own
  `[extern."<module id>"]` section), `std/tls` declares `pub extern type TlsHandle` and carries the OpenSSL
  `SSL*` as one, and `std/http`/`std/ws` carry `Option<tls.TlsHandle>` with `None` for a plain connection
  instead of the `0` in a `tls_handle: usize`. Both backends pass the full suite; the project-module refusal
  (`ffi_handle_no_section`) and the opaque-handle fixtures are unchanged.
