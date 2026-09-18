// M6: the builtin owned heap byte buffer. Construct, measure, index, mutate,
// copy, and print — each of those is a compiler-provided builtin whose body
// lives in the generated C runtime.
fun main() {
    var b = Bytes.from_str("hello")
    print(b.len())
    print(b.get(i = 0))
    b.set(i = 0, b = 72)
    print(b)
    val c = b.clone()
    print(c)
    print(b.is_empty())
    val e = Bytes.empty()
    print(e.len())
}
