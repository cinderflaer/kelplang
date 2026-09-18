// S1: a program can fail deliberately. `panic` writes "panic: <msg>" to
// stderr and exits 1; the golden-text check lives in `compile_and_run.rs`.
fun main() {
    print("before")
    panic("deliberate failure")
}
