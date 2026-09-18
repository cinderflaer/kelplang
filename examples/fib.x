fun fib(n: i32).R: i32 {
    if n < 2 {
        return n
    }
    return fib(n - 1) + fib(n - 2)
}

fun main() {
    print(fib(10))
}
