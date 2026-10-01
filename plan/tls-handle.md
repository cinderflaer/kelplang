# A typed handle for `std/tls`

*Release goal 1 (`plan/release-goals.md`, "Follow-ups"). The last item of "stable TLS": `TlsStream` stops
carrying the OpenSSL `SSL*` as a `usize` now that `extern type` exists.*

## What changes

1. **`extern type` in a compiler-shipped std module.** `resolve.rs` refused an `extern type` unless the
   module had its own `[extern."<module id>"]` section, which a shipped std module cannot have. One
   condition: a module is admitted when `stdlib::source(id)` resolves it, the same table that already tells
   a shipped module from a project one (and the table a project id cannot collide with, because the loader
   reads a shipped id's text from it before any project path). A project module still needs its section;
   `tests/projects/ffi_handle_no_section` keeps that refusal.
2. **`std/tls`.** `pub extern type TlsHandle`, `TlsStream(val fd: i32, val handle: TlsHandle)`, and the
   six `*_raw` externs take/return `TlsHandle` in place of `usize`. The bodies in `externs.rs` cast with
   `(SSL*)kelp_handle` and hand the `SSL*` back as the handle; a failed handshake hands back the null
   handle, which is what the `0` usize used to be.
3. **`std/http` / `std/ws`.** `tls_handle` becomes `Option<tls.TlsHandle>`; `Some` is a TLS connection and
   `None` is a plain one, so no integer sentinel (`0`) is needed and no handle has to be forged. `Conn`,
   `Lease`, `Cursor` and `Pool` keep their shapes and every public signature is unchanged. `Option` and its
   payload are handled by value, never as an integer, so a plain connection and a TLS one are still told
   apart by a value the type system knows about.

   `Option<TlsHandle>` is the design the existing `extern type` rules allow: a shipped std module cannot
   write `TlsHandle.null()` (that operation is legal only where an `[extern.*]` section exists, and a std
   module's handles are admitted by `stdlib.rs`, not a manifest), and it does not need to, because `None`
   *is* the "no handle" value and needs no pointer. The null handle stays what it always was: the value a
   failing `handshake_try_raw` returns, which no caller of `std/tls` ever sees.

## What does not change

Server-side TLS, client certificates, session resumption and ALPN stay out of scope, and the plain-family
panics and the `try_` family's errors are untouched. `std/tls`'s "nothing is freed on drop" contract is
unchanged: `TlsHandle` is `Copy` and owns nothing, and `try_shutdown` is still the call that frees the
`SSL*`. Only the *spelling* of the handle changed, from an integer to a type.
