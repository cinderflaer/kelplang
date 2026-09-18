fun sum_to(n: i32).R: i32 {
    var total = 0
    var i = 1
    while i <= n {
        total += i
        i += 1
    }
    return total
}

fun main() {
    print(sum_to(10))
}
