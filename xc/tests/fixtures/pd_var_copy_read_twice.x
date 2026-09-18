// M6.4: reading a Copy `pd var` twice in one call is fine — that is a read
// of the global, not a borrow of it (see pd_var_double_borrow.x for the case
// that is refused).
pd var counter = 1

fun add(a: i32, b: i32).R: i32 = a + b

fun main() {
    print(add(counter, counter))
    counter = 10
    print(add(counter, counter))
}
