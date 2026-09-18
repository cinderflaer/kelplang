// MK5-S5: a `Map` has no `Option` yet, so a missing key panics with the call
// name rather than returning a sentinel. `remove` is checked the same way.
fun main() {
    var m: Map<i64> = Map.new()
    m.insert("present", 1)
    print(m.get("absent"))
}
