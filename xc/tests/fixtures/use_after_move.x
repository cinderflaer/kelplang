datac Point(val x: i32, val y: i32)

fun consume(p: Point).R: i32 = p.x + p.y

fun main() {
    val p = Point(x = 1, y = 2)
    print(consume(p))
    print(consume(p))
}
