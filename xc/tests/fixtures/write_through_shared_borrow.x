datac Counter(val id: i32, var count: i32)

fun bad_bump(c: Counter) {
    c.count += 1
}

fun main() {
    var c = Counter(id = 1, count = 0)
    bad_bump(c)
}
