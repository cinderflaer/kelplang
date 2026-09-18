// M6.3: the view's borrow ends with its scope, so the owner is movable
// again afterwards (and freed exactly once).
fun view(s: &String).R: &str = s.as_str()

fun main() {
    val s = String.from_str("hi")
    {
        val v = view(s)
        print(v)
    }
    val moved = s
    print(moved)
}
