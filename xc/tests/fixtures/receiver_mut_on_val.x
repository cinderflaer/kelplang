datac Circle(var r: f64)

fun mut Circle.grow(by: f64) {
    r += by
}

fun main() {
    val c = Circle(r = 2.0)
    c.grow(by = 1.0)
}
