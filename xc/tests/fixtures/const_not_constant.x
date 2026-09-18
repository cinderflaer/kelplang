// M6.3: a `const` has to fold at compile time, so a call is not an
// initializer.
const BAD: i32 = side()

fun side().R: i32 = 1

fun main() {
    print(BAD)
}
