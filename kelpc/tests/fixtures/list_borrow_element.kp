// A borrow cannot be a list element: an element has to outlive the list, and
// v1 has no lifetime syntax to say that a view's owner does.
fun main() {
    val xs: List<&str> = List.of("a")
    print(xs.len())
}
