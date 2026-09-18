impl "std/net"

// A peer that never writes. `set_timeout` turns the blocking `recv` into a
// bounded wait, so this panics with "timed out" instead of hanging forever —
// the difference between a broken peer and an unkillable program. Both ends
// are in this process, so nothing external is involved.
fun main() {
    val l = net.listen(0)
    val port = l.port()
    val c = net.connect("127.0.0.1", port)
    var s = net.accept(l)
    s.set_timeout(100)
    print("waiting")
    val got = net.recv(s, 8)
    print(got.as_str())
}
