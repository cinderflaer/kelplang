// S1: sort/slice/insert/remove on a list of *owned* elements. The shifts are
// moves of the element struct, so this is the fixture that proves a growable
// list can be reordered without leaking or double-freeing a `String` buffer
// (it is in the AddressSanitizer pass).
fun main() {
    var words: List<String> = List.empty()
    words.push(String.from_str("pear"))
    words.push(String.from_str("apple"))
    words.push(String.from_str("fig"))
    // Byte order, length-aware: "apple" < "fig" < "pear".
    words.sort()
    print(words.get(0))
    print(words.get(1))
    print(words.get(2))
    print(words.contains(String.from_str("fig")))
    print(words.contains(String.from_str("kiwi")))
    // `contains` borrows the needle for an owned element type, so the
    // temporary above is never moved into the call.
    var middle = words.slice(1, 3)
    print(middle.len())
    print(middle.get(0))
    middle.clear()

    words.insert(0, String.from_str("fig"))
    print(words.len())
    print(words.get(0))
    print(words.get(1))
    val moved = words.remove(0)
    print(moved)
    print(words.len())
    words.reverse()
    print(words.get(0))
}
