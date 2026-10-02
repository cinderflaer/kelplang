# An HTTP/1.1 server in `std/http` (standard library lane, task st2)

*Pure Kelp over `std/net`, no new compiler surface. The server lives at the end of `kelpc/std/http.kp`, beside the
client, because it shares `Header`, `name_eq`, `header_value`, `head_text` and `hex_value`. `std/http` now also holds
`std/encoding` (percent-decoding) and `std/thread` (`serve_threaded`, `sleep_ms`).*

## API

Types: `Limits`, `ServerRequest`, `ServerResponse`, `Server`, `ServerConn`, `Worker` (internal).

```
datac ServerRequest(val method: String, val target: String, val path: String, val query: String,
                    val headers: List<Header>, val body: Bytes)
fun ServerRequest.header(name)         -> String          // first match, "" when absent (as Response.header)
fun ServerRequest.has_header(name)     -> bool
fun ServerRequest.query_param(name)    -> Option<String>  // form-decoded: `+` is a space, `%XX` is a byte
fun ServerRequest.path_segments()      -> List<String>    // non-empty pieces, each percent-decoded
fun ServerRequest.text()               -> String          // panics on invalid utf-8, as Response.text

datac ServerResponse(val status: i32, val headers: List<Header>, val body: Bytes)
http.ok_text(&str)  http.json(&str)  http.html(&str)  http.status(code)  http.not_found()  http.bad_request(&str)
fun ServerResponse.with_header(name, value) -> ServerResponse   // a copy
fun mut ServerResponse.add_header(name, value)                  // in place

http.listen(port: u16) -> Server        Server.port()  Server.accept()  Server.try_accept()  Server.close()
                                        Server.set_accept_timeout(ms)
ServerConn.read_request(&Limits) -> Result<ServerRequest, HttpErr>
ServerConn.write_response(&ServerResponse) -> Result<usize, HttpErr>
ServerConn.write_error(&HttpErr) -> Result<usize, HttpErr>      ServerConn.close()
http.http_error_status(&HttpErr) -> i32

http.serve(port, handler)                          // forever, one connection at a time
http.serve_n(port, handler, max_requests) -> u64   // returns after max_requests answered, listener closed
http.serve_on(&Server, handler, &Limits, max_requests) -> u64      // the same on a listener you hold
http.serve_threaded(port, handler, workers)        // forever, `workers` threads on one listener
http.serve_threaded_on(&Server, handler, &Limits, workers, idle_ms) -> u64
```

`handler` is a plain `fun(&ServerRequest).R: ServerResponse` function value, so nothing borrowed crosses a thread.

Choices the spec left open, taken as the smallest consistent option:

- **`write_response` returns a `Result`**, not unit: a peer that has gone is the normal end of a connection, and it must
  not panic the server. `serve*` ignore the `Err`, close, and go on. `read_request` and `write_error` follow.
- **The status of a failed read travels in the `HttpErr`.** `HttpErr` is a three-variant enum users match on, so a new
  variant would break them. A refusal is `HttpErr.Exchange` whose reason starts with its status code and a space
  (`408 the request did not arrive in time`); `http_error_status` reads it back. A reason with no leading code (the peer
  reset, closed between requests, or sat idle) has status 0 and is answered with nothing.
- **A request counts** once the server has produced a response for it: a handler that ran (even if the write then
  failed) or an error response written. A connection that closes with nothing, or mid-body, counts for nothing.
  `serve_n`/`serve_on` stop at `max_requests`; the last answer carries `Connection: close`.
- **`serve_threaded_on` takes `idle_ms`** and `serve_threaded` passes 0: with `idle_ms > 0` each worker stops once an
  `accept` has waited that long with nobody connecting, the call returns the total, and a test ends. The workers are
  spawned one inside the next (a `Thread<T>` cannot be a list element) and each level joins its own.
- **`serve_on` / `serve_threaded_on` / `Server`** exist because `serve_n(port, ...)` cannot tell a test which port the
  kernel gave for port 0; the program binds, prints `Server.port()`, then serves.
- `Server.try_accept` and `Server.set_accept_timeout` use two `std/net` additions (below).

## Limits

`Limits` is a `datac` of ceilings; `default_limits()` is the table below; `Limits.with_timeouts(head, body, idle)`
returns a copy with other timeouts.

| Limit | Default | Violation, status (every error response carries `Connection: close`) |
|-------|---------|--------------------------------------------------------------------------|
| request line | 8 KiB | 414 |
| header bytes after the request line | 16 KiB | 431 |
| header count | 100 | 431 |
| body (`Content-Length`, or the sum of chunks) | 1 MiB | 413, before the body is read |
| head deadline | 10 s | 408 |
| body deadline | 30 s | 408 |
| keep-alive idle | 5 s | the connection closes with no answer |
| requests per connection | 100 | the 100th answer says `Connection: close` |

Other answers: 400 for a malformed request line, method, target, header, chunk or framing; 411 for a `POST`, `PUT` or
`PATCH` with neither `Content-Length` nor chunking; 417 for an `Expect` other than `100-continue`; 501 for an unknown
transfer coding; 505 for a well-formed `HTTP/d.d` other than 1.0 and 1.1 (a malformed version is 400).

**The timeouts are deadlines, not per-read waits.** A client that sends one byte every 100 ms never trips a
`SO_RCVTIMEO`; the head deadline (from the first byte of the request) ends it. Waiting for the first byte of a request is
the idle wait (`idle_timeout_ms` once the connection has served something, `head_timeout_ms` on a fresh one); a fresh
connection that sends nothing gets a 408, an idle keep-alive connection is closed silently.

## Parsing rules

- A head is CRLF ASCII: a bare LF, a lone CR, a NUL or other control byte (HTAB aside), a byte above 127, or a blank line
  before the request line is a 400. That also makes the head safe to turn into a `String` (no UTF-8 check to panic on).
- Request line: exactly three single-space-separated parts. Method is a token; the target starts with `/` (or is `*` for
  `OPTIONS`); absolute-form targets are not accepted (400). `path` and `query` are the halves around the first `?`, still
  percent-encoded.
- Headers: name is a token with no space before the colon; the value is trimmed; a line starting with a space or tab
  (folding) is 400; duplicates are kept in order. No `Host` requirement is enforced.
- Framing: `Content-Length` must be digits only, repeated headers must agree (a list like `3, 3` is 400);
  `Content-Length` with `Transfer-Encoding` is 400; two `Transfer-Encoding` headers are 400; the value must be exactly
  `chunked` (case-insensitive), any other is 501; chunked in an HTTP/1.0 request is 400. Chunk extensions are ignored,
  trailers are read (bounded by the header limit) and discarded.
- `Expect: 100-continue` on a body-carrying HTTP/1.1 request is answered with `100 Continue` before the body is read,
  unless body bytes already arrived. A body is read only after a 413 check on the declared length.
- Connection: HTTP/1.0 closes unless `Connection: keep-alive` (the answer then says so), `Connection: close` (any token in
  any `Connection` header) closes, a handler that sets `Connection: close` closes, and the request cap closes.
- Pipelined requests are answered in order: bytes read past a request stay in the connection's buffer for the next.

## Responses

`Content-Length` is added automatically (not for 1xx, 204 or 304, which have no body); a handler's own `Content-Length`,
`Transfer-Encoding` and `Connection` headers are dropped, and a header whose name is not a token or whose value holds CR,
LF or NUL is dropped (no response splitting). `HEAD` answers carry the headers, with the `Content-Length` of the body that
is not sent. Reason phrases exist for the common codes (`Unknown` otherwise). Every body is sent with its head in one
`send`.

A closing connection is closed gently: `shutdown(SHUT_WR)`, then whatever the peer still sends is read and discarded for
at most 250 ms and 1 MiB, then `close`. Closing with unread data makes the kernel reset the connection, which can destroy a
413 the client has not read yet.

## Capabilities

`http.listen` is `net.listen`, checked against the **port** of the importer's `net` grant (`caps(net = ["127.0.0.1:8080"])`
or `"127.0.0.1:*"`); `std/http` carries no policy of its own, as for the client. `accept` and the connection calls are not
checked, as in `std/net`. Listening **cannot be restricted by host today**: `net.listen` always binds `0.0.0.0` and a
pattern's host part constrains nothing for it (roadmap open item 55). That is a known gap, not a second mechanism added
here. Projects `caps_http_listen` and `caps_http_listen_denied` pin both sides.

## What `std/net` gained

Three small additions, each one `extern fun` row in `externs.rs` plus a Kelp wrapper in `net.kp`:

- `net.mono_ms()`: a monotonic clock in milliseconds, for deadlines. It sits in `net`, not `time`, because `std/http`
  holds only `net` (its importer delegates `net = [...]`); asking for `time` too would break every existing
  `impl "std/http" caps(net = [...])` line. It reads elapsed time and nothing else.
- `net.try_accept(l)` and `Listener.set_timeout(ms)`: an accept that can time out (`net.accept: timed out`) instead of
  blocking forever or panicking, which is what lets threaded workers stop and a failed accept be a value.
- `net.try_shutdown_write(s)`: `shutdown_write` that answers `false` for a reset socket instead of panicking.

## Not here

TLS (the server speaks plain HTTP), HTTP/2, `Date` and `Server` headers (the module has no clock beyond `mono_ms`),
`Host` enforcement, absolute-form and `CONNECT` targets, ranges, compression, cookies, `Transfer-Encoding` on responses
(bodies are always `Content-Length`-framed, so no streaming), a send deadline (a client that never reads blocks its
worker in `send`), connection limits, and `Expect` on anything but `100-continue`. A panic in a handler ends the process
in `serve`/`serve_n` and one worker thread in `serve_threaded*`.

## Tests

`kelpc/tests/compile_and_run.rs` (`http_server_*`, `http_listen_follows_the_net_grant_port`): the Rust test is the client
and the server is a compiled Kelp program from `kelpc/tests/hazards/` (`http_server.kp`, `http_server_threaded.kp`; a
hazard, not a fixture, because it waits for clients and the sweeps run every fixture unbounded). Each program binds
port 0, reports it on its first stdout line, answers a given number of requests and ends, under `ulimit -v`/`-t` and
`timeout`, with every client socket timed out. Its last line reports the process's descriptor count before and after, and
the test asserts they are equal. Both backends run every case (`KELPC_ORACLE=c` for the oracle). Covered: GET and POST
echo, routing by `path_segments`, query decoding, three requests on one connection, pipelining, a chunked body with
trailers, `HEAD`, `Expect: 100-continue`, every limit violation and framing refusal above with its status and a closed
connection, a slowloris client, an idle keep-alive timeout, a client that disconnects mid-body and one that resets
mid-response (the server answers the next client), `serve_threaded_on` with 4 workers and 8 concurrent clients, and an
AddressSanitizer run of both programs on the C backend's generated source. `tests/fixtures/http_server_lowlevel.kp` covers
the accessors and the low-level API in one process.
