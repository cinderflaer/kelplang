impl "std/net"

// `poll` readiness: one thread can serve many sockets by asking which of them
// has something to say instead of blocking on one. Every wait here is bounded,
// which is the property that makes a loop possible at all.
fun main() {
    val l = net.listen(0)
    val port = l.port()
    val c = net.connect("127.0.0.1", port)
    val s = net.accept(l)

    // Nothing has been sent yet, so a short wait reports false. This is a
    // timeout rather than a hang — the whole point of `wait_readable`.
    print(s.wait_readable(10))
    // A freshly connected socket has room to write.
    print(c.wait_writable(1000))

    net.send_text(c, "now")
    print(s.wait_readable(2000))
    val got = net.recv(s, 8)
    print(got.as_str())

    // After the peer closes its writing half the socket is readable again, and
    // the read reports end of file — a readiness a loop must not spin on.
    net.shutdown_write(c)
    print(s.wait_readable(2000))
    val eof = net.recv(s, 8)
    print(eof.is_empty())

    net.close(c)
    net.close(s)
    l.close()
}
