impl "std/mem"

// The two operations on a `&str` that X cannot express: its byte length and
// a byte at an index. Both bodies are compiler-supplied; the public wrappers
// below are ordinary X code, and they are what callers see.
//
// The bounds check for `byte_at_raw` lives in its compiler-supplied body
// (see `externs.rs`) rather than here, so there is exactly one of it and no
// call site can forget it.
extern fun byte_len_raw(s: &str).R: usize
extern fun byte_at_raw(s: &str, i: usize).R: u8

// Byte length, not character count — see `str.char_count` for that.
fun len(s: &str).R: usize = byte_len_raw(s)

// The byte at `i`.
// Panics: if `i` is out of bounds.
fun byte(s: &str, i: usize).R: u8 = byte_at_raw(s, i)

fun is_empty(s: &str).R: bool = len(s) == 0

// Non-ASCII bytes are copied through unchanged: upper-casing arbitrary
// Unicode needs a character table, which v1 has no place to put.
fun to_upper(s: &str).R: String {
    val n = len(s)
    var out = mem.alloc(n)
    var i: usize = 0
    while i < n {
        val c = byte(s, i)
        if c >= 97 && c <= 122 {
            out.set(i, c - 32)
        } else {
            out.set(i, c)
        }
        i = i + 1
    }
    return String.from_bytes(out)
}

fun to_lower(s: &str).R: String {
    val n = len(s)
    var out = mem.alloc(n)
    var i: usize = 0
    while i < n {
        val c = byte(s, i)
        if c >= 65 && c <= 90 {
            out.set(i, c + 32)
        } else {
            out.set(i, c)
        }
        i = i + 1
    }
    return String.from_bytes(out)
}

// `s` repeated `n` times. `replace`, `from_i64`, and `from_f64` are the
// remaining allocation-returning `str` functions and land with M6.3's
// `&str` surface; `repeat` needs nothing those do not already have.
fun repeat(s: &str, n: usize).R: String {
    val ls = len(s)
    var out = mem.alloc(ls * n)
    var i: usize = 0
    while i < n {
        var j: usize = 0
        while j < ls {
            out.set(i * ls + j, byte(s, j))
            j = j + 1
        }
        i = i + 1
    }
    return String.from_bytes(out)
}
