// Pushing an owned element moves it into the list: the binding that held it
// is not a second owner afterwards (M3's move check, reused unchanged).
fun main() {
    var names: List<String> = List.empty()
    val s = String.from_str("x")
    names.push(s)
    print(s)
}
