// M6.2: an `extern` takes no receiver — a method would need a body the
// compiler cannot supply for an arbitrary user type.
extern fun mut Bytes.nope()

fun main() {
    print("unreachable")
}
