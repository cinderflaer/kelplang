// Reaching into an empty box panics: v1 has no Option, so there is nothing
// else `get` could return. `is_empty()` is the predicate to check first.
fun main() {
    var b: Box<i32> = Box.empty()
    print(b.is_empty())
    print(b.get())
}
