// A `String` may not be grown while a view of it is alive: `realloc` can
// move the buffer a `str` points into, so this has to be a compile error,
// not a runtime surprise.
fun main() {
    var s = String.from_str("hi")
    val view = s.as_str()
    s.push(33)
    print(view)
}
