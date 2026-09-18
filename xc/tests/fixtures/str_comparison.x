// M6.3: `==` on a `str` would compare addresses, not text, so it is refused
// in favour of `str.eq`.
fun main() {
    val a = "x"
    val b = "y"
    print(a == b)
}
