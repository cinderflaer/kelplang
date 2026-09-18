impl "std/net"

// A single-file program: the entry module implicitly holds every domain with a
// `*` pattern (open item 18), so `net` resolves here with no `with caps(...)`
// clause at all — and the runtime check is the whole-resource wildcard `*`,
// which is what the `net` vocabulary had to gain in S3 alongside `host:port`.
//
// One process, loopback, no fixed port: bind 0, ask which port the kernel
// gave, connect to it.
fun main() {
    val l = net.listen(0)
    val port = l.port()
    val c = net.connect("127.0.0.1", port)
    val s = net.accept(l)

    net.send_text(c, "hello")
    val got = net.recv(s, 32)
    print(got.as_str())

    net.close(c)
    net.close(s)
    l.close()
}
