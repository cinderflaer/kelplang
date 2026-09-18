impl "std/mem"
impl "std/net"
impl "std/str"
impl "std/fmt"

// std/http — an HTTP/1.1 client (S4). Ungated itself, and it *holds* `net`:
// the grant that governs its connections is the one its importer wrote
// (`impl "std/http" with caps(net = ["api.example.com:443"])`), and the
// runtime check that follows uses those patterns, not any of its own. Because
// naming `net.connect` needs the `net` domain, a project that imports this
// module without delegating gets a compile error naming the module, not a
// client that fails at runtime.
//
// What this module owns, beyond request/response text:
//
//  * **Keep-alive.** HTTP/1.1 is persistent by default, so the client does not
//    send `Connection: close`; a connection goes back into the pool when the
//    body was completely framed and the server did not ask to close.
//  * **Framing, all of it.** `Content-Length`, `Transfer-Encoding: chunked`, a
//    bodyless response, and the read-to-end-of-file fallback — the only case
//    that costs a connection.
//  * **The pool.** `http_pool` in `x.toml` is how many kept-alive connections
//    per `host:port` a program keeps (default 4; `0` disables pooling). The
//    ceiling is spliced in by the compiler at the `const` below, and the pool
//    is lazy: nothing connects until a request needs it.
//
// A `Lease` is a *ticket*, not an owner. The pool holds every descriptor; a
// lease is permission to use one for one exchange, and `release` consumes it.
// Since a `datac` is never `Copy`, releasing twice or using a lease after
// release is a compile error. That is the reason to have the pool in X rather
// than in C: the ownership check is the borrow checker's, and it is free.
//
// Not here, deliberately: gzip, HTTP/2, cookies, proxies,
// `Expect: 100-continue`, idle reaping, and `https` (TLS is S4b, and
// `parse_url` refuses it by name rather than pretending).

const DEFAULT_SCHEME: str = "http"
const DEFAULT_PORT: u16 = 80
const DEFAULT_TIMEOUT_MS: u64 = 30000
const DEFAULT_MAX_REDIRECTS: i32 = 5

// The project's connection ceiling, from `x.toml`'s `http_pool` key. The
// compiler substitutes the number here *before* this text is lexed, so a
// project with no manifest gets the compiler default. `0` means "no pooling":
// every exchange gets a fresh connection and nothing is kept.
const pool_size: usize = __X_HTTP_POOL__

datac Url(val scheme: String, val host: String, val port: u16, val path: String)

datac Header(val name: String, val value: String)

datac Response(val status: i32, val headers: List<Header>, val body: Bytes, val reusable: bool)

datac Request(val method: String, val url: String, val headers: List<Header>, val body: Bytes)

// `timeout_ms = 0` is "no timeout" (that is what `SO_RCVTIMEO` of zero means),
// which is deliberate: a program that wants to wait forever can say so.
datac Client(val timeout_ms: u64, val max_redirects: i32, val follow_redirects: bool)

// A set of kept-alive connections to one `host:port`, keyed on the host **as
// written** — never on a resolved address, because the capability check
// matches the string the author wrote (S3's rule), and two spellings of one
// address can fall under different grants.
//
// `idle` holds the descriptors of connections that are open and not in use, and
// `size` is how many of those the pool agrees to *keep*. That is the standard
// "max idle" reading rather than a hard cap on open connections: a request that
// arrives while every kept connection is busy gets a fresh one, and `release`
// decides afterwards whether there is room to keep it. A program therefore
// never fails because its pool was small, which is worth more than a strict
// ceiling.
datac Pool(
    val host: String,
    val port: u16,
    val size: usize,
    val timeout_ms: u64,
    var idle: List<i32>,
)

// Permission to use one pooled connection for one exchange. The descriptor *is*
// the number the pool hands out (`fd`), and `reused` says whether the
// connection was already open. Nothing here is owned, and `release` takes the
// lease by value, so the value is gone afterwards: releasing twice, or using a
// lease after release, does not compile. That check is the reason to have the
// pool in X rather than in C — it is the borrow checker's, and it is free.
datac Lease(val fd: i32, val reused: bool)

// The read cursor for one response: bytes already received, and how far into
// them we are. It refills from the socket when it runs dry. It holds an fd
// rather than a `Socket` because a `Socket` is owned, and a parameter is
// always a borrow — there is no way to move one into a struct field.
datac Cursor(val fd: i32, var buf: Bytes, var start: usize)

// ----- URLs -----

// Parses an absolute `http://` URL. Panics: if there is no scheme, no host, or
// the scheme is anything but `http` (`https` gets its own message naming S4b).
//
// The port is split at the *first* `:`, so an IPv6 literal is not expressible
// here either — the same rule `net`'s capability patterns follow, for the same
// reason: a host that contains the port separator would be ambiguous.
fun parse_url(s: &str).R: Url {
    val sep = str.index_of(s, "://")
    if !sep.ok {
        panic("http.parse_url: URL must start with 'http://' (no scheme found)")
    }
    val scheme = str.prefix(s, sep.at)
    if !str.eq(scheme.as_str(), "http") {
        if str.eq(scheme.as_str(), "https") {
            panic("http.parse_url: 'https' is not supported yet (TLS is S4b) — use 'http://'")
        }
        panic("http.parse_url: only 'http' is supported")
    }
    val rest = str.substring(s, sep.at + 3)
    return split_authority(rest)
}

// Splits `host[:port][/path]` — the part after `://`. The path is everything
// from the first `/`, so a query string without a path (`host?a=b`) parses as
// part of the authority and fails there rather than silently losing the query.
fun split_authority(rest: &str).R: Url {
    if str.is_empty(rest) {
        panic("http.parse_url: URL has no host")
    }
    val slash = str.index_of(rest, "/")
    if slash.ok {
        val authority = str.prefix(rest, slash.at)
        val path = str.substring(rest, slash.at)
        return make_url(authority, path)
    }
    val whole = str.prefix(rest, str.len(rest))
    return make_url(whole, "/")
}

fun make_url(authority: String, path: &str).R: Url {
    if str.is_empty(authority.as_str()) {
        panic("http.parse_url: URL has no host")
    }
    val colon = str.index_of(authority.as_str(), ":")
    if colon.ok {
        // Bound to a place first: `str.substring` hands back a borrow of its
        // argument, and a borrow-returning call needs a variable or a field,
        // not a call result.
        val text = authority.as_str()
        val host = str.prefix(text, colon.at)
        val port_text = str.substring(text, colon.at + 1)
        return Url(
            scheme = String.from_str(DEFAULT_SCHEME),
            host = host,
            port = parse_port(port_text),
            path = String.from_str(path),
        )
    }
    return Url(
        scheme = String.from_str(DEFAULT_SCHEME),
        host = authority.clone(),
        port = DEFAULT_PORT,
        path = String.from_str(path),
    )
}

fun parse_port(text: &str).R: u16 {
    if str.is_empty(text) {
        panic("http.parse_url: URL has an empty port")
    }
    val p = str.parse_u64(text)
    if !p.ok {
        panic("http.parse_url: URL port is not a number")
    }
    if p.value > 65535 {
        panic("http.parse_url: URL port is out of range")
    }
    return p.value.to_u16()
}

// ----- headers -----

// ASCII case-insensitive comparison. Header names are case-insensitive, the
// values here are ASCII by construction, and `str.to_lower` is ASCII-only
// (open item 48), so this is exactly as correct as the case operations
// underneath it.
fun name_eq(a: &str, b: &str).R: bool {
    if str.len(a) != str.len(b) {
        return false
    }
    val la = str.to_lower(a)
    val lb = str.to_lower(b)
    return str.eq(la.as_str(), lb.as_str())
}

fun empty_headers().R: List<Header> {
    var hs: List<Header> = List.empty()
    return hs
}

fun Response.has_header(name: &str).R: bool {
    var i: usize = 0
    while i < headers.len() {
        val h = headers.get(i)
        if name_eq(h.name.as_str(), name) {
            return true
        }
        i = i + 1
    }
    return false
}

// The first matching header's value, or an empty `String` when absent — v1
// has no `Option`, so absence is the empty string and `has_header` is the
// honest test.
//
// The result is *owned*, which is a deviation from the API sketch that had it
// return `&str`, and it is forced by two v1 facts rather than chosen: `&str`
// return elision roots at a parameter or a local, so a borrow reached through
// `List.get` cannot be handed back (`return headers.get(i).value.as_str()`
// fails the place rule, and binding the element first makes the root that
// binding instead of `self`), and a `list`-less view type does not exist yet.
// One allocation per lookup, until the length-carrying view (open item 44)
// makes the borrow expressible.
fun Response.header(name: &str).R: String {
    var i: usize = 0
    while i < headers.len() {
        val h = headers.get(i)
        if name_eq(h.name.as_str(), name) {
            return String.from_str(h.value.as_str())
        }
        i = i + 1
    }
    return String.empty()
}

fun Response.is_ok().R: bool = status >= 200 && status < 300

// The body as owned text. Panics: if the bytes are not valid utf-8
// (`String.from_bytes` validates). The body is cloned first because the
// response is borrowed here, and v1 has no partial moves to take a field out.
fun Response.text().R: String = String.from_bytes(body.clone())

// ----- request text -----

// The whole request head, ending with the blank line. `Content-Length` is sent
// only when there is a body: `Content-Length: 0` on a `GET` is legal but
// unusual, and some servers object to it.
//
// No `Connection` header is sent on purpose. HTTP/1.1 is persistent unless the
// client asks otherwise, and staying silent is what lets the server's own
// `Connection: close` (if it sends one) be the single source of truth.
fun build_head(method: &str, u: Url, headers: List<Header>, body_len: usize).R: String {
    var out = String.with_capacity(256)
    out.push_str(method)
    out.push_str(" ")
    out.push_str(u.path.as_str())
    out.push_str(" HTTP/1.1\r\nHost: ")
    out.push_str(u.host.as_str())
    if u.port != DEFAULT_PORT {
        out.push_str(":")
        // A formatted number is an owned temporary, so its `&str` view has to
        // be bound before it can be used: `fmt.uint(n).as_str()` taken as an
        // argument is the "temporary would be freed" error.
        val port_text = fmt.uint(u.port.to_u64())
        out.push_str(port_text.as_str())
    }
    out.push_str("\r\n")
    var i: usize = 0
    while i < headers.len() {
        out.push_str(headers.get(i).name.as_str())
        out.push_str(": ")
        out.push_str(headers.get(i).value.as_str())
        out.push_str("\r\n")
        i = i + 1
    }
    if body_len > 0 {
        out.push_str("Content-Length: ")
        val len_text = fmt.uint(body_len.to_u64())
        out.push_str(len_text.as_str())
        out.push_str("\r\n")
    }
    out.push_str("\r\n")
    return out
}

// ----- the response cursor -----

// Makes at least one byte available, refilling from the socket if needed.
//
// Panics: if the peer closed instead of sending the rest of the body.
fun mut Cursor.ensure() {
    if start >= buf.len() {
        val chunk = net.recv(net.Socket(fd = fd), 4096)
        if chunk.is_empty() {
            panic("http: the connection closed inside the response body")
        }
        buf.clear()
        start = 0
        buf.append(chunk)
    }
}

// One line, up to and including the `LF`; the `LF` is consumed and a `CR` is
// dropped. Dropping every `CR` rather than only a trailing one is safe here
// because neither a chunk size nor a header value may contain one — and it
// avoids a `String` truncation that does not exist yet.
//
// Panics: as `ensure`.
fun mut Cursor.read_line().R: String {
    var out = String.empty()
    var going = true
    while going {
        self.ensure()
        val c = buf.get(start)
        start = start + 1
        if c == 10 {
            going = false
        } else {
            if c != 13 {
                out.push(c)
            }
        }
    }
    return out
}

// Consumes one line and discards it — the `CRLF` after a chunk's data. This
// exists because discarding an owned `String` in statement position is
// rejected ("a non-tail statement that produces an owned value"), and there is
// no `_ =` to say "drop this".
//
// Panics: as `ensure`.
fun mut Cursor.skip_line() {
    var going = true
    while going {
        self.ensure()
        val c = buf.get(start)
        start = start + 1
        if c == 10 {
            going = false
        }
    }
}

// Exactly `n` bytes: whatever the cursor already holds, then direct reads into
// a buffer sized for the whole thing. Reading straight into the destination
// means the socket can never over-read past the body, which is what makes a
// pooled connection safe to reuse afterwards.
//
// Panics: if the peer closes before `n` bytes have arrived.
fun mut Cursor.read_exact(n: usize).R: Bytes {
    var out = mem.alloc(n)
    var got: usize = 0
    while got < n && start < buf.len() {
        out.set(got, buf.get(start))
        start = start + 1
        got = got + 1
    }
    if start >= buf.len() {
        buf.clear()
        start = 0
    }
    while got < n {
        val k = net.recv_into(net.Socket(fd = fd), out, got)
        if k == 0 {
            panic("http: the connection closed before the whole body arrived")
        }
        got = got + k
    }
    return out
}

// Everything up to end of file. Only for a response with no framing at all,
// which is also the case that cannot be pooled: the connection is spent
// finding the end of the body. With a timeout set, a peer that keeps the
// connection open turns this into `net.recv`'s "timed out" panic rather than a
// hang, which is the documented cost of an unframed response.
//
// Panics: if a read fails.
fun mut Cursor.read_to_end().R: Bytes {
    var out = Bytes.empty()
    while start < buf.len() {
        out.push(buf.get(start))
        start = start + 1
    }
    buf.clear()
    start = 0
    var going = true
    while going {
        val chunk = net.recv(net.Socket(fd = fd), 4096)
        if chunk.is_empty() {
            going = false
        } else {
            out.append(chunk)
        }
    }
    return out
}

// One hex digit. Panics: if the byte is not one.
fun hex_digit(c: u8).R: u32 {
    if c >= 48 && c <= 57 {
        return (c - 48).to_u32()
    }
    if c >= 97 && c <= 102 {
        return (c - 87).to_u32()
    }
    if c >= 65 && c <= 70 {
        return (c - 55).to_u32()
    }
    panic("http: chunk size is not a hexadecimal number")
    // Unreachable, but a `.R: u32` function has to end with a `return`:
    // `panic` is typed `Unit`, not "never".
    return 0
}

// A chunk size: hex digits, optionally followed by `;extension`, which is
// ignored. Panics: if there are no digits, or a digit is not hexadecimal.
fun parse_hex(text: &str).R: usize {
    if str.is_empty(text) {
        panic("http: empty chunk size")
    }
    var value: usize = 0
    var digits: usize = 0
    val n = str.len(text)
    var i: usize = 0
    while i < n {
        val c = str.byte(text, i)
        if c == 59 {
            i = n
        } else {
            value = value * 16 + hex_digit(c).to_usize()
            digits = digits + 1
            i = i + 1
        }
    }
    if digits == 0 {
        panic("http: chunk size has no digits")
    }
    return value
}

// Decodes a chunked body: a hex size line, that many bytes, a `CRLF`, repeated
// until a zero size, then trailers until a blank line. This is what keep-alive
// makes mandatory — with a persistent connection there is no other way to find
// where a chunked body stops, and reading to end of file would block until the
// server closed a connection it intends to keep.
//
// Panics: on a malformed chunk or a peer that closes early.
fun mut Cursor.read_chunked().R: Bytes {
    var out = Bytes.empty()
    var going = true
    while going {
        val size_text = self.read_line()
        val size = parse_hex(size_text.as_str())
        if size == 0 {
            going = false
        } else {
            val piece = self.read_exact(size)
            out.append(piece)
            self.skip_line()
        }
    }
    var trailing = true
    while trailing {
        val line = self.read_line()
        if str.is_empty(line.as_str()) {
            trailing = false
        }
    }
    return out
}

// ----- response reading -----

// The offset just past the blank line that ends the header block, or 0 when it
// has not arrived yet. 0 is a usable sentinel because a header block can never
// end at offset 0 — the terminator alone is four bytes.
//
// This scans **bytes**, and that is not an optimization. `Bytes.as_str()`
// validates utf-8, and the buffer holds whatever arrived alongside the
// headers — so the first response with a binary body would panic right here,
// before the headers had even been located. Every synthetic test had an
// all-ASCII body, which is why it took `examples/download.x` (a 200 KB image)
// to find this.
fun find_head_end(b: Bytes).R: usize {
    val n = b.len()
    var i: usize = 0
    while i + 4 <= n {
        if b.get(i) == 13 && b.get(i + 1) == 10 && b.get(i + 2) == 13 && b.get(i + 3) == 10 {
            return i + 4
        }
        i = i + 1
    }
    return 0
}

// The header block as text: the bytes before the terminator, copied and
// validated in one step, since `String.from_bytes` *adopts* the buffer instead
// of copying it. A header block that is not valid utf-8 is a malformed
// response, and this is where that becomes loud rather than producing a
// `String` that lies about its own invariant.
fun head_text(b: Bytes, end: usize).R: String {
    val n = end - 4
    var raw = mem.alloc(n)
    var i: usize = 0
    while i < n {
        raw.set(i, b.get(i))
        i = i + 1
    }
    return String.from_bytes(raw)
}

// The status line: `HTTP/1.1 200 OK`. Only the code matters here; the reason
// phrase may be absent or anything at all.
//
// Panics: if the line has no code, or the code is not a number.
fun parse_status_line(line: &str).R: i32 {
    val sp = str.index_of(line, " ")
    if !sp.ok {
        panic("http: malformed status line (no status code)")
    }
    val rest = str.substring(line, sp.at + 1)
    val sp2 = str.index_of(rest, " ")
    if sp2.ok {
        val code_text = str.prefix(rest, sp2.at)
        return status_code(code_text.as_str())
    }
    return status_code(rest)
}

fun status_code(text: &str).R: i32 {
    val p = str.parse_u64(text)
    if !p.ok {
        panic("http: the status code is not a number")
    }
    if p.value > 999 {
        panic("http: the status code is out of range")
    }
    return p.value.to_i32()
}

// Every header line after the status line, name and value split at the first
// `:`, both trimmed. A line with no `:` is skipped rather than treated as an
// error: HTTP allows a recipient to ignore what it does not understand.
fun parse_headers(lines: List<String>).R: List<Header> {
    var hs: List<Header> = List.empty()
    var i: usize = 1
    while i < lines.len() {
        val text = lines.get(i).as_str()
        if !str.is_empty(text) {
            val colon = str.index_of(text, ":")
            if colon.ok {
                val name = str.prefix(text, colon.at)
                val value = str.trim(str.substring(text, colon.at + 1))
                hs.push(Header(name = name, value = value))
            }
        }
        i = i + 1
    }
    return hs
}

fun header_value(hs: List<Header>, name: &str).R: String {
    var i: usize = 0
    while i < hs.len() {
        if name_eq(hs.get(i).name.as_str(), name) {
            return String.from_str(hs.get(i).value.as_str())
        }
        i = i + 1
    }
    return String.empty()
}

// `-1` for absent, which is not a legal `Content-Length` — the same
// ok-flag-free convention `str.Match` avoids by carrying a flag, kept here
// because the value is a count and every non-negative count is meaningful.
//
// Panics: if the header is present and not a number.
fun content_length_of(hs: List<Header>).R: i64 {
    val text = header_value(hs, "content-length")
    if str.is_empty(text.as_str()) {
        return -1
    }
    val p = str.parse_u64(text.as_str())
    if !p.ok {
        panic("http: Content-Length is not a number")
    }
    return p.value.to_i64()
}

fun is_chunked(hs: List<Header>).R: bool {
    val raw = header_value(hs, "transfer-encoding")
    val text = str.to_lower(raw.as_str())
    return str.contains(text.as_str(), "chunked")
}

fun wants_close(hs: List<Header>).R: bool {
    val raw = header_value(hs, "connection")
    val text = str.to_lower(raw.as_str())
    return str.contains(text.as_str(), "close")
}

// One whole response off one socket: header block, framing decision, body.
//
// Everything is a local because v1 has no partial moves — a helper that
// returned `(status, headers, cursor)` would have to be destructured to be
// used, and there is no destructuring. So the framing lives in `Cursor`'s
// methods and the parsing lives in the helpers above, while the assembly of
// the `Response` happens here, where every piece is still a plain binding.
//
// Panics: on a peer that closes early, a malformed status line, a
// non-numeric `Content-Length`, a malformed chunk, or a read failure.
fun read_response(s: net.Socket).R: Response {
    var buf = Bytes.empty()
    var end: usize = 0
    var eof = false
    while end == 0 && !eof {
        val chunk = net.recv(s, 4096)
        if chunk.is_empty() {
            eof = true
        } else {
            buf.append(chunk)
            end = find_head_end(buf)
        }
    }
    if end == 0 {
        panic("http: the connection closed before the response headers arrived")
    }
    var status: i32 = 0
    var hs: List<Header> = List.empty()
    {
        val head_only = head_text(buf, end)
        val lines = str.split(head_only.as_str(), "\r\n")
        status = parse_status_line(lines.get(0).as_str())
        hs = parse_headers(lines)
    }
    val chunked = is_chunked(hs)
    val len = content_length_of(hs)
    val close = wants_close(hs)
    var c = Cursor(fd = s.fd, buf = buf, start = end)
    var body = Bytes.empty()
    if chunked {
        body = c.read_chunked()
    } else {
        if len >= 0 {
            body = c.read_exact(len.to_usize())
        } else {
            body = c.read_to_end()
        }
    }
    // Reusable means: the body was completely framed (so the next byte on the
    // wire is the start of the next response) and the server did not ask to
    // close. Anything else and the connection is spent.
    val reusable = !close && (chunked || len >= 0)
    return Response(status = status, headers = hs, body = body, reusable = reusable)
}

// ----- one exchange, and redirects -----

// A full round trip on one socket: send the head and body, read the response.
// The socket's timeout is set by whoever opened it, not here, because a pooled
// connection is configured once on connect rather than on every request.
//
// Panics: if the send or the response read fails.
fun one_exchange(s: net.Socket, method: &str, u: Url, headers: List<Header>, body: Bytes).R: Response {
    val head = build_head(method, u, headers, body.len())
    net.send_all(s, head.into_bytes())
    // Unconditional: `send_all` of an empty buffer writes nothing, and this
    // avoids an `if` without an `else` whose body is a `usize` expression.
    net.send_all(s, body)
    return read_response(s)
}

// 301/302/303 become `GET` with no body (the historical behavior every browser
// implements); 307/308 preserve the method and body, which is exactly why they
// were standardized.
fun keep_body_on(status: i32).R: bool = status == 307 || status == 308

fun is_redirect(status: i32).R: bool {
    return status == 301 || status == 302 || status == 303 || status == 307 || status == 308
}

// A `Location` that is not absolute is resolved against the URL that produced
// the redirect. Only the host and port are inherited; the pool key and the
// capability check both use the resulting URL's own host, so a redirect to
// another name is a *different* connection needing its own grant.
fun absolute_url(base: Url, loc: &str).R: String {
    if str.starts_with(loc, "http://") || str.starts_with(loc, "https://") {
        return String.from_str(loc)
    }
    var out = String.with_capacity(64)
    out.push_str(DEFAULT_SCHEME)
    out.push_str("://")
    out.push_str(base.host.as_str())
    out.push_str(":")
    val port_text = fmt.uint(base.port.to_u64())
    out.push_str(port_text.as_str())
    if str.starts_with(loc, "/") {
        out.push_str(loc)
    } else {
        out.push_str("/")
        out.push_str(loc)
    }
    return out
}

// Whether to follow this response, and where to. Panics: if the redirect chain
// is longer than the client allows — a loop is a malformed server, and v1 has
// no `Result` to report it with.
// Where a redirect chain goes next, or an empty string for "do not follow".
//
// The URL is *returned by a call* (`absolute_url`) rather than read out of a
// `datac` field, and that is deliberate: v1 has no partial moves, so a field of
// a local struct cannot be moved out — `Next(url = target.url)` and
// `return target.url` are both errors. A helper that returns a freshly built
// value sidesteps the question entirely.
//
// Panics: if the chain is longer than the client allows.
fun follow_url(client: Client, base: Url, resp: Response, hops: i32).R: String {
    if !client.follow_redirects {
        return String.empty()
    }
    if !is_redirect(resp.status) {
        return String.empty()
    }
    val loc = resp.header("location")
    if str.is_empty(loc.as_str()) {
        return String.empty()
    }
    if hops <= 0 {
        panic("http: too many redirects")
    }
    return absolute_url(base, loc.as_str())
}

fun default_client().R: Client = Client(
    timeout_ms = DEFAULT_TIMEOUT_MS,
    max_redirects = DEFAULT_MAX_REDIRECTS,
    follow_redirects = true,
)

// One request on a fresh connection, following redirects. Every hop connects
// again, which is the correct behavior for a redirect that changes host and
// the only option without a pool to consult.
//
// Panics: if the connection fails, the capability denies the host, the
// response is malformed, or the redirect chain is too long.
fun Client.send(req: Request).R: Response = send_fresh(
    self,
    req.method.clone(),
    req.url.clone(),
    req.headers,
    req.body.clone(),
    self.max_redirects,
)

fun send_fresh(
    client: Client,
    method: String,
    url_text: String,
    headers: List<Header>,
    body: Bytes,
    hops: i32,
).R: Response {
    val u = parse_url(url_text.as_str())
    val s = connect_configured(u.host.as_str(), u.port, client.timeout_ms)
    val resp = one_exchange(s, method.as_str(), u, headers, body)
    net.close(s)
    // The redirect decision is one helper; the *tail* is written here because
    // `resp` is an owned local here and a returned borrow cannot leave. With no
    // function values in v1, that tail is written at each call site.
    val next = follow_url(client, u, resp, hops)
    if str.is_empty(next.as_str()) {
        return resp
    }
    if keep_body_on(resp.status) {
        return send_fresh(client, method, next, headers, body, hops - 1)
    }
    return send_fresh(client, String.from_str("GET"), next, headers, Bytes.empty(), hops - 1)
}

// A single exchange on a socket the caller already holds: parse the URL, send,
// read. This is the concurrency-ready primitive — no pool and no client are
// named while it runs — and `Client.send`/`Pool.send` are the redirect loops
// built on top of it.
//
// Panics: as `one_exchange`, plus a malformed URL.
fun exchange(s: net.Socket, req: Request).R: Response {
    val u = parse_url(req.url.as_str())
    return one_exchange(s, req.method.as_str(), u, req.headers, req.body)
}

// ----- the pool -----

// The pool for one `host:port`. The ceiling and the timeout come from the
// program's configuration (`http_pool`) and the default client; nothing is
// connected here, so creating a pool costs nothing.
fun pool(host: &str, port: u16).R: Pool = Pool(
    host = String.from_str(host),
    port = port,
    size = pool_size,
    timeout_ms = DEFAULT_TIMEOUT_MS,
    idle = empty_idle(),
)

fun empty_idle().R: List<i32> {
    var l: List<i32> = List.empty()
    return l
}

// A configured connection: the one place the timeout is applied, so a pooled
// socket and a fresh one behave the same. It takes parameters instead of a pool
// because reading `self.host` as a `&str` would leave a borrow of the pool
// alive to the end of the calling block, which would then conflict with the
// `idle.push` that follows.
//
// Panics: if the connection fails or the grant denies the host.
fun connect_configured(host: &str, port: u16, timeout_ms: u64).R: net.Socket {
    var s = net.connect(host, port)
    if timeout_ms > 0 {
        s.set_timeout(timeout_ms)
    }
    return s
}

fun Pool.size().R: usize = size

// How many connections the pool is keeping right now.
fun Pool.len().R: usize = idle.len()

fun Lease.socket().R: net.Socket = net.Socket(fd = fd)

// Opens up to `size` connections now, so the first request does not pay for
// them. Optional by design: eager connecting is what makes a pool hurt startup,
// so it is a call a program makes rather than the default.
//
// The pool's fields are copied into locals first and the idle count is read
// inside a block. That is not decoration: a shared borrow created in a *binding
// initializer* stays live to the end of the block (the lexical lifetimes open
// item 30 records), and `idle.push` needs the pool exclusively a line later.
//
// Panics: if a connection fails.
fun mut Pool.warm() {
    val h = host.clone()
    val p = port
    val t = timeout_ms
    val cap = size
    var i: usize = 0
    {
        i = idle.len()
    }
    while i < cap {
        val s = connect_configured(h.as_str(), p, t)
        idle.push(s.fd)
        i = i + 1
    }
}

// Hands out a connection: an idle one if there is one, a fresh one if there is
// not. The pool never refuses — the ceiling governs how many connections are
// *kept*, not how many may be in flight — so a caller holding two leases simply
// gets two connections, and only one of them is kept on release.
//
// Panics: if connecting fails.
fun mut Pool.checkout().R: Lease {
    if idle.is_empty() {
        val h = host.clone()
        val p = port
        val t = timeout_ms
        val s = connect_configured(h.as_str(), p, t)
        return Lease(fd = s.fd, reused = false)
    }
    val fd = idle.pop()
    return Lease(fd = fd, reused = true)
}

// Gives a connection back. `release` consumes the lease, so this cannot happen
// twice. A connection that is not reusable — an unframed body, or a server that
// said `Connection: close` — is closed instead of kept, and so is one beyond
// the ceiling: the pool keeps at most `size`.
//
// Panics: if closing a connection fails.
fun mut Pool.release(lease: Lease, reusable: bool) {
    if reusable && idle.len() < size {
        idle.push(lease.fd)
    } else {
        val s = lease.socket()
        net.close(s)
    }
}

// Panics: if closing a connection fails.
fun mut Pool.close_all() {
    var going = true
    while going {
        if idle.is_empty() {
            going = false
        } else {
            val fd = idle.pop()
            net.close(net.Socket(fd = fd))
        }
    }
}

fun pool_matches(p: Pool, u: Url).R: bool {
    if p.size == 0 {
        return false
    }
    return str.eq(p.host.as_str(), u.host.as_str()) && p.port == u.port
}

fun mut Pool.send(req: Request).R: Response {
    val c = default_client()
    return pooled_hop(
        self,
        c,
        req.method.clone(),
        req.url.clone(),
        req.headers,
        req.body.clone(),
        c.max_redirects,
    )
}

fun pooled_hop(
    p: mut Pool,
    client: Client,
    method: String,
    url_text: String,
    headers: List<Header>,
    body: Bytes,
    hops: i32,
).R: Response {
    val u = parse_url(url_text.as_str())
    if pool_matches(p, u) {
        val lease = p.checkout()
        val sock = lease.socket()
        val resp = one_exchange(sock, method.as_str(), u, headers, body)
        p.release(lease, resp.reusable)
        val next = follow_url(client, u, resp, hops)
        if str.is_empty(next.as_str()) {
            return resp
        }
        if keep_body_on(resp.status) {
            return pooled_hop(p, client, method, next, headers, body, hops - 1)
        }
        return pooled_hop(
            p,
            client,
            String.from_str("GET"),
            next,
            headers,
            Bytes.empty(),
            hops - 1,
        )
    }
    val s = connect_configured(u.host.as_str(), u.port, client.timeout_ms)
    val resp = one_exchange(s, method.as_str(), u, headers, body)
    net.close(s)
    val next = follow_url(client, u, resp, hops)
    if str.is_empty(next.as_str()) {
        return resp
    }
    if keep_body_on(resp.status) {
        return pooled_hop(p, client, method, next, headers, body, hops - 1)
    }
    return pooled_hop(
        p,
        client,
        String.from_str("GET"),
        next,
        headers,
        Bytes.empty(),
        hops - 1,
    )
}

// ----- the one-liners -----

// A `GET` on a fresh connection, no pool. Pooling is opt-in through an
// explicit `Pool` because a process-wide default pool would be shared mutable
// state, and threads (T1) would then need a lock around every request — see
// the plan's open item 61.
//
// Panics: if the connection fails, the capability denies the host, or the
// response is malformed.
fun get(url: &str).R: Response {
    val c = default_client()
    return c.send(make_get(url))
}

fun make_get(url: &str).R: Request = Request(
    method = String.from_str("GET"),
    url = String.from_str(url),
    headers = empty_headers(),
    body = Bytes.empty(),
)

// Panics: as `get`.
fun post_text(url: &str, body: &str, content_type: &str).R: Response {
    var hs: List<Header> = List.empty()
    hs.push(Header(name = String.from_str("Content-Type"), value = String.from_str(content_type)))
    val req = Request(
        method = String.from_str("POST"),
        url = String.from_str(url),
        headers = hs,
        body = Bytes.from_str(body),
    )
    val c = default_client()
    return c.send(req)
}
