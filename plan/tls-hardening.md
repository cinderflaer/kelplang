# Stable TLS, part 2: the hostile-conditions matrix

*Release goal 1 (`plan/release-goals.md`). Part 1 shipped the typed `try_` API, truncation detection,
timeouts, the private CA and `wss` (`plan/done.md`). This file is part 2: a local hostile-conditions matrix
against `std/tls`, `std/http` and `std/ws`, and every bug it found, fixed in the module or the compiler.*

Every case dials **127.0.0.1 only**. The Rust test server mints a throwaway self-signed certificate per
server (`openssl` crate, not a CLI), and the compiled program is pointed at it through `SSL_CERT_FILE`, so no
machine trust store is touched. The fixtures and the servers live in `kelpc/tests/compile_and_run.rs`; every
one of them runs on the default `llvm` backend and on the `c` oracle (`KELPC_ORACLE=c`), because
`compile_and_run.rs` is backend-agnostic by construction.

The finding table starts below the matrix. Each row names the case, the observable failure, the cause and the
fix (or why it was left).

## The matrix

| Case | Fixture / project | Server condition | Assertion |
|------|-------------------|------------------|-----------|
| (a) 10 MiB body, small and large reads | `tls_big_body.kp` | `CannedTlsServer`, 10 MiB body, then clean `close_notify` | length and SHA-1 match the bytes the server sent, both chunk sizes |
| (b) partial reads and writes | `tls_partial.kp` | server reads 1 MiB one byte at a time, writes its own body one byte at a time | client's `write_all` sent 1 MiB; both SHA-1s match |
| (c) alert mid-stream | `tls_bad_record.kp`, `https_get` project | partial body then a `close_notify`, and (separately) a record that fails authentication | the short body is refused by `std/http`; the bad record is an error, never a clean EOF |
| (d) TCP reset during the handshake | `tls_reset_handshake.kp` | accept, no read, close with the ClientHello unread | `try_handshake` is an error, not a hang |
| (d) TCP reset during a read | `tls_reset_read.kp` | handshake, no read of the request, close | the read is an error |
| (e) 64 concurrent clients | `tls_concurrent.kp` | one TLS echo connection per client, a thread each | every echo is its own request, no fd delta |
| (f) silent peer, with `set_timeout` | `tls_handshake_socket_timeout.kp` | never answers the ClientHello | `Timeout` |
| (f) silent peer, without | `tls_handshake_no_timeout.kp` | never answers, then closes after a hold | `PeerClosedUncleanly` (proves no silent default deadline) |
| (g) SNI | `tls_sni.kp` | `DnsLocalhost` certificate, server records `SSL_get_servername` | the recorded name equals the host given to `handshake` |
| (h) `https` redirect to another local port | `https_redirect` project | port A `301` -> port B, both TLS | both hops, body from B |
| (i) `Connection: close` vs keep-alive | `http_close_vs_keepalive` project | first connection says close, the next is kept | exactly two accepted connections, pool counts |
| (j) 100 KiB header block | `https_big_headers` project | one `X-Big` header of 100 000 bytes | header length and body survive `std/http` |

The handshake-leak gate is `tls_handshakes_free_everything_under_address_sanitizer`, which rebuilds the C
backend's generated `.c` with `cc -fsanitize=address` (there is no valgrind on this machine; `memory_gate.rs`
is the same idea for a machine that has it) and runs one success and every failure path against the servers,
including the pooled path drained with `Pool.close_all`. It is the one place a fixture is built through
`KELPC_ORACLE=c` explicitly, because AddressSanitizer only sees the backend that emits C.

## Findings

### F1. A TLS write to a peer that had reset the connection killed the process with SIGPIPE

*Case (d), the reset during a read.* `tls_reset_read` died with `unix_wait_status(13)` (SIGPIPE)
and printed nothing on the C backend, while the LLVM build usually won the race and reported the
error the test expected. That difference was timing: whichever direction the RST reached the client
first.

**Cause.** `net.send_raw`/`net.send_all_raw` pass `MSG_NOSIGNAL` so a plain socket write to a dead
peer becomes `EPIPE`. OpenSSL's socket BIO writes with a plain `write()`, which a library call cannot
ask to change, so a TLS write to a socket the peer has reset raised SIGPIPE and the default action
took the process down before `SSL_write` could return the error the typed path already handles.

**Fix, first version (rejected).** `runtime/kelp_rt.c` ignored SIGPIPE once, process-wide, in a constructor.
That fixed TLS and broke every other program: with the signal ignored, a Kelp program piped into `head` kept writing
to a closed pipe forever instead of being ended like any Unix tool (`yes`-style producers never stopped; checked by
running an endless `print` loop through `| head -1`: exit 141 before, a run to the 5 s cap after).

**Fix, as shipped.** SIGPIPE is blocked on the calling thread only for the duration of each OpenSSL call that can
write (`SSL_connect`, `SSL_write`, `SSL_read`, `SSL_shutdown`), through the `KELP_NO_SIGPIPE(call)` helper in
`codegen_c::C_HEADERS` (shared by both backends). A SIGPIPE that the call itself raised is consumed before the mask is
restored, one that was already pending is left alone, and `errno` is preserved for `SSL_get_error`. The default
action stays in force everywhere else. Regressions: `a_tcp_reset_during_a_read_is_a_typed_error` (TLS) and
`a_program_is_ended_by_sigpipe_when_its_reader_closes_the_pipe` (the process-wide behaviour).

### F2. The truncation and read-timeout fixtures leaked the `SSL*`, and the whole trust store with it

*The AddressSanitizer gate.* `tls_truncated` leaked 92 428 bytes in 466 allocations. The roots were
the `SSL*` a successful handshake had allocated; the "indirect" leaks included the certificate store
`SSL_CTX_set_default_verify_paths` had loaded, because `SSL_new` keeps its own reference to the
`SSL_CTX`, so losing the `SSL*` loses the context too.

**Cause.** After a failed read those fixtures closed the socket without ending the session. `std/tls`
documents that nothing is freed on drop and that `try_shutdown` is the only call that frees the
handle, so this was the fixture's omission, not a leak inside the module: the handshake's own error
paths free the `SSL_CTX` and the `SSL*` on every branch, which the ASan gate now exercises for the
untrusted, timeout, reset and bad-record handshakes, and which the 64-thread fixture exercises for
success.

**Fix.** `tls_truncated.kp`, `tls_timeout_read.kp` and every new fixture call `try_shutdown` on every
path where a `TlsStream` exists, including after a read error. The typed-handle follow-up in
`plan/release-goals.md` (an owned handle whose drop is the free) is the change that would make the
omission impossible; until then the contract stays documented in `std/tls`'s header and is the thing
the leak gate checks.

### F3. The helper cannot send a fatal alert, so case (c) is split in two

Not an implementation bug. `rust-openssl` exposes no `SSL_send_alert`, and after the handshake every
record is authenticated, so a raw alert written into the socket is read as a failed AEAD record (TLS
1.2) or a bad record type (TLS 1.3) rather than as an alert. Case (c) is therefore two tests:
`a_close_notify_mid_body_is_not_a_complete_response` (the alert a peer can send, checked through
`std/http`'s framing) and
`a_record_that_fails_authentication_mid_stream_is_not_a_clean_end_of_stream` (checked at `std/tls`).
Both assert the same observable rule: a peer that stops mid-body is never a clean end of stream.

### What the matrix did not find

No bug in `std/http` or `std/ws`: the `https` redirect to a second local port, `Connection: close`
against keep-alive, and the 100 KiB header block all behave as documented, and the 10 MiB body, the
1 MiB `write_all` against a dribbling peer and the 64 concurrent clients all preserve every byte with
no descriptor delta. `try_handshake` failing halfway (untrusted, timeout, reset, bad record) frees the
`SSL_CTX` and the `SSL*` on every branch, which is what the AddressSanitizer gate now pins.

### Left as a documented contract, not fixed

- **A pooled connection nobody drains.** Nothing frees a `Conn`'s `SSL*` on drop, so a program that exits
  with idle pooled connections leaves them to process teardown; `Pool.close_all` is the call that ends them,
  and `tls_pool_close.kp` is the leak gate's proof that draining works. This is the same "nothing closes on
  drop" rule `std/tls`, `std/http` and `std/ws` all state, and the typed handle plus `Drop` (release goal 3)
  is what would make it unrepresentable.
