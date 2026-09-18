datac Counter(val id: i32, var count: i32)

fun main() {
    var c = Counter(id = 1, count = 0)
    val r = &c
    val m = &mut c
    print((*m).count)
    print((*r).count)
}
