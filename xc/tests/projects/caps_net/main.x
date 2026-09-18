// The `net` grant goes to `peer`, not to `main`: the capability is received by
// the module that names `net.listen`/`net.connect`, and the runtime check that
// follows uses *that* module's patterns. `127.0.0.1:*` is the shape S3 made
// expressible — a host and a wildcard port, matched as a name rather than
// lexically normalized as a path.
impl "peer" with caps(net = ["127.0.0.1:*"])

fun main() {
    print(peer.report())
}
