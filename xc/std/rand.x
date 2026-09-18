// std/rand — the gated entropy namespace (M7). Reading entropy is a
// capability (WASI's `wasi:random`), so a program that cannot observe it is
// reproducible by construction.
impl "std/mem"

extern fun u64_raw().R: u64
extern fun fill_raw(buf: mut Bytes)

fun u64().R: u64 = u64_raw()

// A value in `0..n`. Returns 0 for `n == 0` rather than dividing by zero.
fun below(n: u64).R: u64 {
    if n == 0 {
        return 0
    }
    return u64_raw() % n
}

// `n` fresh random bytes.
fun bytes(n: usize).R: Bytes {
    var out = mem.alloc(n)
    fill_raw(out)
    return out
}
