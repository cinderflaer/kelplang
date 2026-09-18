// A grant must not be able to name its way out of its own root: the `..`
// segment is rejected at the `impl` line, at compile time.
impl "std/fs" with caps(fs = ["/tmp/../etc/**"])

fun main() {
    print("unreachable")
}
