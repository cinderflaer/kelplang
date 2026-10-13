# Server-side TLS, client certificates and ALPN in `std/tls` (standard library lane, task st3)

*Same OpenSSL as the client (`stdlib/tls.md` (kelp wiki), `stdlib/tls.md` (kelp wiki)): the system library, linked only by a
program that reaches `std/tls` (`uses_tls`), hosted only. The server bodies (`handshake_try_raw`, `server_ctx_try_raw`,
`reload_try_raw`, `accept_try_raw`, `ctx_free_raw`, `alpn_raw`, `peer_subject_raw`, `pending_raw`) and the listener
body `net.listen_on_raw` are Rust, in `kelp-rt/src/tls.rs`, and their rows are in the `PROTOTYPES` table of
`kelp-rt/src/abi.rs`. The rest of `std/tls` (the stream bodies) is still inline C in the `EXTERNS` table of
`kelpc/src/externs.rs`, with the classifiers in `OPENSSL_HEADERS` (`kelpc/src/cdecl.rs`). The remaining inline C bodies
are listed in `kelpc/src/externs.rs`; the cc-removal lane is moving them to Rust, so that list will change.*

## API

```
// std/tls (PEM bytes; no std/fs, see "Files" below)
tls.server_config_pem(cert_chain_pem: Bytes, key_pem: Bytes).return: Result<ServerConfig, TlsError>
tls.server_config_pem_with(cert, key, opts: &ServerOptions).return: Result<ServerConfig, TlsError>

tls.server_options().return: ServerOptions                       // TLS 1.2 floor, no ALPN, no client certs, 10 s handshake
ServerOptions.min_version(v: TlsVersion)     -> ServerOptions   // TlsVersion.Tls12 (default) | Tls13
ServerOptions.alpn(protocols: &List<String>) -> ServerOptions   // the server's preference order
ServerOptions.require_client_cert_pem(ca_pem: Bytes) -> ServerOptions   // mTLS
ServerOptions.handshake_timeout_ms(ms: u64) -> ServerOptions
ServerConfig.free()

tls.accept(config: &ServerConfig, s: net.Socket).return: Result<TlsStream, TlsError>
TlsStream.peer_cert_subject().return: Option<String>             // RFC 2253 subject of the peer's certificate (mTLS server side)
TlsStream.alpn().return: Option<String>                          // negotiated protocol, server and client streams

tls.client_options().return: TlsClientOptions                    // system store, no timeout, no ALPN, no client cert
TlsOptions.alpn(&List<String>) / TlsOptions.client_cert_pem(cert, key)   -> TlsClientOptions
TlsClientOptions.alpn / .client_cert_pem / .with_ca_file / .with_timeout_ms
tls.try_handshake_opts(s, host, &TlsClientOptions).return: Result<TlsStream, TlsError>

// std/tlsfile (reads through the fs gate)
tlsfile.read_pem(path: &str, what: &str).return: Result<Bytes, TlsError>

// std/http
http.serve_tls(port, config, handler) / serve_tls_n / serve_tls_on     // HTTPS with the same limits as `serve`
http.try_get_tls(url, &TlsClientOptions) / http.get_tls / Client.try_send_tls(req, &opts)   // private CA, mTLS, ALPN
```

Choices the spec left open:

- **`ServerConfig` is immutable because the options are a value given at build time** (`ServerOptions`), not mutators
  on a built config. It wraps one `SSL_CTX*` (`TlsCtx`, an opaque `extern type`); OpenSSL 1.1+ allows one `SSL_CTX` to
  make `SSL`s on many threads, so a config is shared by reference or by copying its handle. `ServerConfig.free()` ends
  it; like every handle in `std/tls`, nothing is freed on drop.
- **`TlsOptions` keeps its three fields.** Fixtures build it positionally (`tls_ca_file.kp`), and a datac has no field
  defaults, so ALPN and a client certificate live in the new `TlsClientOptions`, which `TlsOptions.alpn(...)` and
  `TlsOptions.client_cert(...)` convert to. `try_handshake_with` is unchanged for callers.
- **`TlsError` gets no new variant** (existing fixtures match it exhaustively). A file that cannot be read is
  `Io("...")`; a malformed PEM, a key that does not match the certificate, a CA file with no certificate, an ALPN
  mismatch and a handshake an mTLS server refuses are `HandshakeFailed("...")`; a client certificate from another CA or
  an expired one is `Untrusted`/`Expired` (the verify code), exactly as on the client.
- **`net.Socket`, not `net.Stream`**: `std/net` has no `Stream` type; `accept` takes the `Socket` `net.accept` returns.
- **`serve_tls` takes a port, not an address**: `net.listen` binds `0.0.0.0` and takes only a port (open item 55).
  The listener is therefore on every interface; the tests use `127.0.0.1` connections only.
- **Files (`std/tlsfile`).** The spec asks for `tls.server_config(cert_path, key_path)` reading through `fs`. A
  module that names `fs.try_read` needs the `fs` grant from every program that imports it, and every `https` client
  imports `std/tls`; the first attempt (an `fs` import in `std/tls`) broke every existing TLS and HTTP program at
  compile time. So `std/tls` takes PEM *bytes* (`server_config_pem`) and `std/tlsfile` holds the one gated read,
  `tlsfile.read_pem(path, what)`, which a program grants on its own line: `impl "std/tlsfile" caps(fs = [...])`.
  `std/tlsfile` cannot call `std/tls` functions itself (they are `net`-gated and would need that grant too), so the
  program composes: `tls.server_config_pem(tlsfile.read_pem(c, "certificate chain")?, tlsfile.read_pem(k, "private key")?)`.

## Rules (same as the client)

- Verification cannot be turned off on either side: the client keeps `SSL_VERIFY_PEER` and the hostname check, the
  server only ever adds `SSL_VERIFY_PEER | SSL_VERIFY_FAIL_IF_NO_PEER_CERT` when a client CA is given.
- A peer that ends without `close_notify` is `PeerClosedUncleanly` on a server stream too (same read paths).
- A private key is never logged or put in an error text: the Rust port reports fixed sentences for key problems
  ("the private key is malformed", "the private key does not match the certificate"), never OpenSSL's own text for
  those calls, and the OpenSSL error queue is cleared after them.
- **Key file permissions are not checked by the library.** A key readable by other users is the operator's problem;
  keep it `0600` and owned by the serving user. The PEM bytes are read into ordinary memory and are not zeroed.
- ALPN: the server selects the first of *its* protocols the client also offered; no overlap is a fatal
  `no_application_protocol` alert, a `HandshakeFailed`. A client that offers none (or a server with none configured)
  negotiates none, and `alpn()` is `None`.
- Handshake deadline: `SO_RCVTIMEO`/`SO_SNDTIMEO` as on the client; default 10 s on the server. The timeouts stay on
  the socket afterwards, as they do for the client.

## `std/http`

`ServerConn` carries `tls_handle: Option<TlsHandle>`; its read, write and close go through the same `Conn` dispatch
the client uses. The one extra rule is in `fill`: OpenSSL may hold decrypted bytes the socket no longer shows, so
`fill` asks `SSL_pending` first and only then waits for readability. `ServerConn.close` ends the TLS session
(`close_notify`) before the half-close and drain. A failed handshake in `serve_tls_on` closes that socket and the loop
goes on; the limits, status table and per-connection rules are those of `stdlib/http.md` (kelp wiki).

## Tests

`kelpc/tests/tls_server.rs` generates a throwaway CA, a `localhost` server certificate, a client certificate, a
certificate from a second CA and an expired certificate with the `openssl` CLI into a scratch directory (it prints
`skipped: no openssl` and passes when the CLI is missing). The programs are in `kelpc/tests/hazards/tls_server_matrix.kp`
and `kelpc/tests/hazards/tls_reload_matrix.kp` (they need paths and wait on sockets, so not fixtures): one program runs
server and client on two threads over `127.0.0.1` on an ephemeral port. The programs are built by the LLVM backend,
the only backend; setting `KELPC_ORACLE` is an error, since the C oracle is gone.
