pd val label = "outer"

fun show() {
    val label = "inner"
    print("local=$label")
}

fun main() {
    show()
    print("pd=$label")
}
