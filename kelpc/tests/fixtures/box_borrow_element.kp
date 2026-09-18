// A borrow cannot live in a box: the box owns its value, and there is no
// lifetime syntax to say how long the borrow would have to stay valid.
fun main() {
    val b: Box<&str> = Box.of("x")
    print(b.get())
}
