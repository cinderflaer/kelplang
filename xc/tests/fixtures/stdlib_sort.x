// S1: the `List` ordering surface — `sort`, `reverse`, `contains`, `slice`.
// Numbers and `bool` sort by value; the whole set is only defined for an
// element type that has an order (see `list_unsortable_element`).
fun main() {
    var xs = List.of(3)
    xs.push(1)
    xs.push(2)
    xs.sort()
    print(xs.get(0))
    print(xs.get(1))
    print(xs.get(2))
    xs.reverse()
    print(xs.get(0))
    print(xs.get(2))
    print(xs.contains(2))
    print(xs.contains(9))
    var head = xs.slice(0, 2)
    print(head.len())
    print(head.get(0))
    // A slice is a copy: mutating it does not touch the original.
    head.clear()
    print(xs.len())

    var flags: List<bool> = List.empty()
    flags.push(true)
    flags.push(false)
    flags.sort()
    print(flags.get(0))
    print(flags.get(1))

    var empty: List<i32> = List.empty()
    empty.sort()
    empty.reverse()
    print(empty.len())
}
