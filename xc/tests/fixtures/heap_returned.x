// M6: returning an owned value moves it out of the frame, so the callee must
// not free it and the caller must.
fun make(text: &str).R: Bytes {
    val b = Bytes.from_str(text)
    return b
}

fun main() {
    val made = make(text = "hello")
    print(made.len())
    print(made)
}
