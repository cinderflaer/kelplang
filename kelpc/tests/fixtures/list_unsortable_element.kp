// A `datac` element has no order and no `==` in v1, so `sort`/`contains`/
// `slice` are not defined for it — the diagnostic says which and why.
datac Pair(val a: i32, val b: i32)

fun main() {
    var ps: List<Pair> = List.empty()
    ps.push(Pair(a = 1, b = 2))
    ps.sort()
}
