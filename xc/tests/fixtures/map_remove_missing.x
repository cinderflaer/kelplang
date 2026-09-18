// MK5-S5: `remove` on a missing key panics, like `get`.
fun main() {
    var m: Map<i64> = Map.new()
    m.insert("present", 1)
    print(m.remove("absent"))
}
