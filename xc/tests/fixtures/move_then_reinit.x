datac Point(val x: i32, val y: i32)

fun consume(p: Point).R: i32 = p.x + p.y

fun main() {
    var p = Point(x = 1, y = 2)
    print(consume(p))
    p = Point(x = 3, y = 4)
    print(p.x)
    print(consume(p))
}
