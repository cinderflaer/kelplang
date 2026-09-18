// M6 rejection: `%.*s` needs the buffer's length and pointer as two separate
// arguments, so the receiver has to be a place — a temporary has no address
// to take, and evaluating it twice would allocate twice.
fun main() {
    print(Bytes.from_str("x"))
}
