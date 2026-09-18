// A narrowing that would change the value panics with the call's own name,
// rather than truncating: `0 - 1` is not representable in `u8`.
fun main() {
    val neg: i64 = 0 - 1
    print(neg.to_u8())
}
