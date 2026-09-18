// `env` is a gated namespace whose domain works end to end (the grant, the
// gate, and the variable-name check) while its bodies are still stubs.
impl "util" with caps(env = ["HOME"])

fun main() {
    print(util.lookup())
}
