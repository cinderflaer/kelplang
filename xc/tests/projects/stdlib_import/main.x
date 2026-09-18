// A `std/...` module the compiler does not ship is reported as "not
// implemented yet" (naming what is shipped) rather than as a missing file, so
// the reservation stays visible. `std/tls` is the next one in the roadmap
// (S4b, behind the dependency decision in open item 37) and is deliberately
// absent; `std/http` shipped in S4 and moved off this test.
impl "std/tls"

fun main() {}
