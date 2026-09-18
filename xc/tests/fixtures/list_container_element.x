// A list's buffer holds its elements inline, so a container is not a list
// element; a `Box<List<T>>` (or a plain `List<T>`) covers the same ground.
fun main() {
    var xs: List<Box<i32>> = List.empty()
    print(xs.len())
}
