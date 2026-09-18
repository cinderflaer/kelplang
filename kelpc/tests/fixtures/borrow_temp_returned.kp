// The other side of that rule: a call that *returns* a borrow of its
// argument cannot borrow a temporary, because the borrow would outlive the
// temporary. Bind the owned value first ("val up = str.to_upper(s = \"x\")")
// and borrow that.
impl "std/str"

fun main() {
    val s = String.from_str("x").as_str()
    print(s)
}
