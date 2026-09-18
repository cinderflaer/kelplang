// Growing mutates, so it needs an exclusive borrow, so the binding has to
// be a `var`.
fun main() {
    val s = String.from_str("hi")
    s.push(33)
    print(s)
}
