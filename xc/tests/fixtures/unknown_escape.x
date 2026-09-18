// An unknown escape is a diagnostic, not the bare character: `"\q"` used to
// mean `"q"`, which is a silent typo. The lexer now names the supported set.
fun main() {
    print("\q")
}
