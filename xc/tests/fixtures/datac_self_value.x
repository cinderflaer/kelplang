// A datac that holds itself *by value* has no finite size. The fix is the
// M8 spelling, which the diagnostic names: `val next: Box<Node>`.
datac Bad(val next: Bad)

fun main() {
    print(1)
}
