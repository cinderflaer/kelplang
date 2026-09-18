// Reading an owned field *by value* is refused: v1 has no partial moves, and
// the read would be a shallow copy, so the binding and the field would both
// own one buffer. Mutating the field afterwards then left the copy dangling —
// a genuine heap-use-after-free, which is why the rule exists (M8).
datac Bag(val xs: List<i32>)

fun main() {
    var bag = Bag(xs = List.of(1))
    var stolen = bag.xs
    bag.xs.push(2)
    print(stolen.get(0))
}
