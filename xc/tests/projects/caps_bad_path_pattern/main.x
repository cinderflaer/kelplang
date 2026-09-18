// M7: a grant pattern for a path domain must be absolute — a relative pattern
// cannot be matched against a normalized path, so it is rejected at the
// `impl` line.
impl "std/fs" with caps(fs = ["tmp/**"])

fun main() {
    print("unreachable")
}
