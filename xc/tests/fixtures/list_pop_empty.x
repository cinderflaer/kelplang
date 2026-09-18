// `List.pop` on an empty list panics (v1's failure model: panic for
// programmer errors, no Option to return).
fun main() {
    var xs: List<i32> = List.empty()
    print(xs.len())
    print(xs.pop())
}
