// MK5-S5: a `datac` value type, and a `datac` that owns a `Map` field. The
// map typedef is emitted before every `datac` (the value is behind a
// pointer), so the field type is complete; the drop walks keys and values,
// which ASAN proves on the `Bag`.
impl "std/str"

datac Pair(val a: i32, val b: i32)

datac Bag(val counts: Map<i64>)

fun total(b: Bag).R: i64 {
    val ks = b.counts.keys()
    var sum: i64 = 0
    var i: usize = 0
    while i < ks.len() {
        sum += b.counts.get(ks.get(i).as_str())
        i += 1
    }
    return sum
}

fun main() {
    var counts: Map<i64> = Map.new()
    counts.insert("a", 10)
    counts.insert("b", 20)
    val bag = Bag(counts = counts)
    print(total(bag))

    var pairs: Map<Pair> = Map.new()
    pairs.insert("first", Pair(a = 1, b = 2))
    pairs.insert("second", Pair(a = 3, b = 4))
    {
        val p = pairs.get("first")
        print(p.b)
    }
    val taken = pairs.remove("second")
    print(taken.a)
    print(pairs.len())
}
