fun sub(a: i32, b: i32).R: i32 = a - b

fun main() {
    print(sub(10, 3))
    print(sub(a = 10, b = 3))
    print(sub(b = 3, a = 10))
}
