// M6.3: while a `&str` view is alive, its owner cannot be moved — the view
// is exactly as long-lived as the owner's borrow.
fun view(s: &String).R: &str = s.as_str()

fun main() {
    val s = String.from_str("hi")
    val v = view(s)
    val moved = s
    print(v)
    print(moved)
}
