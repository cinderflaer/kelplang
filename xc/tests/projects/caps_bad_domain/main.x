// `disk` is not a capability domain of v1.
impl "std/fs" with caps(disk = ["/tmp/**"])

fun main() {
    print("unreachable")
}
