// A `std/...` module the compiler does not ship is reported as "not
// implemented yet" (naming what is shipped) rather than as a missing file, so
// the reservation stays visible. `std/http` is the next stdlib module in the
// roadmap (S4) and is deliberately absent.
impl "std/http"

fun main() {}
