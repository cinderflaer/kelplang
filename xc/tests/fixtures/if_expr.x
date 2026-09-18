fun classify(n: i32).R: i32 {
    val result = if n < 0 {
        -1
    } else if n == 0 {
        0
    } else {
        1
    }
    return result
}

fun main() {
    print(classify(-5))
    print(classify(0))
    print(classify(42))
}
