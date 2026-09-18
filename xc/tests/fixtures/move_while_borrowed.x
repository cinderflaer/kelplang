datac Point(val x: i32, val y: i32)

fun main() {
    val p = Point(x = 1, y = 2)
    val r = &p
    val q = p
    print((*r).x)
    print(q.x)
}
