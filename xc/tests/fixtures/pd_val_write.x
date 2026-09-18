// M6.4: `pd val` is still read-only inside functions. Its hidden borrow is
// shared, so there is no writable path to it at all.
pd val fixed = 7

fun break_it() {
    fixed = 9
}

fun main() {
    print(fixed)
}
