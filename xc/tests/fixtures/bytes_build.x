// S1: growing an owned `Bytes` in place — `push`, `append` (a borrowed
// view of another buffer), `reserve`, `clear`. The same growth path and the
// same NUL-termination invariant as `String`.
fun main() {
    var b = Bytes.empty()
    print(b.len())
    b.push(1)
    b.push(2)
    print(b.len())
    print(b.get(0))
    print(b.get(1))

    val tail = Bytes.from_str("abc")
    b.append(tail)
    print(b.len())
    print(b.get(2))
    print(b.get(3))
    print(b.get(4))

    b.reserve(32)
    b.append(Bytes.from_str("de"))
    print(b.len())
    print(b.is_empty())

    b.clear()
    print(b.len())
    print(b.is_empty())
    b.push(9)
    print(b.len())
    print(b.get(0))
}
