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
  user's allocator);
- the capability model extended to devices (MMIO regions, IRQ lines, frames) so a driver can only touch what it
  was granted: this is what makes it a *Kelp* OS and not C with nicer syntax;
- stack guards and a defined panic path with no OS underneath;
- a second architecture target (x86-64) next to riscv64, and a test story that boots an image under QEMU.

Not required for the release: a particular kernel design, a filesystem, a network stack, a userland. The
language and toolchain have to make writing them possible.

## Follow-ups

- **A typed `extern type` handle for `std/tls`.** `TlsStream` still carries the `SSL*` as a `usize`. An
  `extern type` is legal only in a module with its own `coral.toml` `[extern."<module id>"]` section
  (`resolve.rs`), and a compiler-shipped std module has no manifest, so `std/tls` cannot declare one. The
  smallest change is one condition in `resolve.rs`: admit a module that is compiler-shipped (`stdlib.rs`'s
  source table already tells a std module from a project one), since the extern bodies such a module may
  declare already come from `externs.rs` and the handle's C (`void*`) and LLVM (`ptr`) lowering is
  module-independent. The second half is in the callers: `std/http` and `std/ws` use `0` in a
  `tls_handle: usize` to mean "not TLS", and an opaque handle has no integer value to compare against, so
  both would carry `Option<tls.TlsHandle>` instead and test that.
