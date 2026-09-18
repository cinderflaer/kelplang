// M6.2: comparing owned values has no generated meaning. `String` has no
// equality yet, so this is refused at type-check time rather than emitted as
// an invalid C `==` on two structs (or, for `str`, as an address comparison
// that would be silently wrong).
fun main() {
    val a = String.from_str("x")
    val b = String.from_str("x")
    print(a == b)
}
