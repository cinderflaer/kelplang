// The grant names PATH only, so asking about HOME is denied before the body
// runs — the name itself is the governed resource.
impl "util" with caps(env = ["PATH"])

fun main() {
    print(util.peek())
}
