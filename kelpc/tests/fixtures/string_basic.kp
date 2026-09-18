// M6.2: the builtin owned `String`. Construct, measure, copy, print, and
// convert to/from `Bytes` with an ownership transfer.
fun main() {
    val greeting = String.from_str("héllo")
    print(greeting.len())
    print(greeting.char_count())
    print(greeting.is_empty())
    print(greeting)

    val copy = greeting.clone()
    print(copy)

    // `into_bytes` moves the receiver out; `from_bytes` moves the buffer
    // back. The buffer is never copied, so the round trip is free.
    val b = copy.into_bytes()
    print(b.len())
    val again = String.from_bytes(b)
    print(again)

    val e = String.empty()
    print(e.is_empty())
    print(e.len())
}
