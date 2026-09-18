datac Circle(var r: f64)

fun Circle.area().R: f64 = 3.14159 * r * r

fun mut Circle.grow(by: f64) {
    r += by
}

fun main() {
    var c = Circle(r = 2.0)
    val before = c.area()
    c.grow(by = 1.0)
    print(before)
    print(c.area())
}
