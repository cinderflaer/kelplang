impl "std/mem"

// The three operations on a `&str` that X cannot express: its byte length, a
// byte at an index, and a borrow of the suffix starting at an index. All
// three bodies are compiler-supplied; the public wrappers below are ordinary
// X code, and they are what callers see.
//
// The bounds checks live in the compiler-supplied bodies (see `externs.rs`)
// rather than here, so there is exactly one of each and no call site can
// forget it.
extern fun byte_len_raw(s: &str).R: usize
extern fun byte_at_raw(s: &str, i: usize).R: u8
extern fun slice_from_raw(s: &str, from: usize).R: &str

// Byte length, not character count — see `char_count`, which counts
// continuation bytes as parts of the same character.
fun len(s: &str).R: usize = byte_len_raw(s)

// The byte at `i`.
// Panics: if `i` is out of bounds.
fun byte(s: &str, i: usize).R: u8 = byte_at_raw(s, i)

fun is_empty(s: &str).R: bool = len(s) == 0

// The tail of `s` starting at byte `from` — a view, not a copy, which is
// what `&str` return elision makes possible: the result borrows `s`.
// Panics: if `from` is past the end.
fun substring(s: &str, from: usize).R: &str = slice_from_raw(s, from)

/// How many characters a `&str` has: bytes that do not continue a UTF-8
/// sequence.
fun char_count(s: &str).R: usize {
    val n = len(s)
    var count: usize = 0
    var i: usize = 0
    while i < n {
        // A byte in 128..191 is a UTF-8 continuation; everything else starts
        // a character. (No bitwise operators in v1, so this is a range test.)
        val c = byte(s, i)
        if c < 128 || c >= 192 {
            count = count + 1
        }
        i = i + 1
    }
    return count
}

datac Match(val ok: bool, val at: usize)

// The index of `needle` in `s`, or `ok = false`. The failure model is the
// error-`datac` convention from the stdlib decisions — no `-1` sentinel.
fun index_of(s: &str, needle: &str).R: Match {
    val ls = len(s)
    val ln = len(needle)
    if ln == 0 {
        return Match(ok = true, at = 0)
    }
    if ln > ls {
        return Match(ok = false, at = 0)
    }
    var i: usize = 0
    while i + ln <= ls {
        if starts_with_at(s, needle, i) {
            return Match(ok = true, at = i)
        }
        i = i + 1
    }
    return Match(ok = false, at = 0)
}

// Whether `needle` occurs in `s` starting exactly at byte `at`. Private to
// the module in practice (nothing outside needs the offset form).
fun starts_with_at(s: &str, needle: &str, at: usize).R: bool {
    val ln = len(needle)
    var i: usize = 0
    while i < ln {
        if byte(s, at + i) != byte(needle, i) {
            return false
        }
        i = i + 1
    }
    return true
}

fun starts_with(s: &str, prefix: &str).R: bool {
    if len(prefix) > len(s) {
        return false
    }
    return starts_with_at(s, prefix, 0)
}

fun ends_with(s: &str, suffix: &str).R: bool {
    val ls = len(s)
    val lf = len(suffix)
    if lf > ls {
        return false
    }
    return starts_with_at(s, suffix, ls - lf)
}

fun contains(s: &str, needle: &str).R: bool {
    val m = index_of(s, needle)
    return m.ok
}

// Text equality, which `==` deliberately does not provide for `str` (it
// would compare addresses — see `infer.rs::comparable`).
fun eq(a: &str, b: &str).R: bool {
    if len(a) != len(b) {
        return false
    }
    val n = len(a)
    var i: usize = 0
    while i < n {
        if byte(a, i) != byte(b, i) {
            return false
        }
        i = i + 1
    }
    return true
}

fun is_space(c: u8).R: bool = c == 32 || c == 9 || c == 10 || c == 13

// Leading whitespace is skipped by advancing the pointer, so this is a view
// of `s` rather than a copy.
fun trim_start(s: &str).R: &str {
    val n = len(s)
    var i: usize = 0
    while i < n && is_space(byte(s, i)) {
        i = i + 1
    }
    return slice_from_raw(s, i)
}

// Trailing whitespace is *truncation*, which a `&str` (a NUL-terminated C
// string) cannot represent — there is nowhere to put the terminator. So the
// end-trimming functions return an owned `String` and allocate, while the
// start-trimming ones stay views. This is the one place M6.3 diverges from
// the plan's `str` sketch.
fun trim_end(s: &str).R: String {
    val n = len(s)
    var end: usize = n
    while end > 0 && is_space(byte(s, end - 1)) {
        end = end - 1
    }
    return prefix(s, end)
}

// The first `n` bytes of `s`, as an owned copy. Truncation cannot be a view:
// a `&str` is a NUL-terminated C string, and there is nowhere in the original
// buffer to put a terminator.
// Panics: if `n` is past the end of `s`.
fun prefix(s: &str, n: usize).R: String {
    var out = mem.alloc(n)
    var i: usize = 0
    while i < n {
        out.set(i, byte(s, i))
        i = i + 1
    }
    return String.from_bytes(out)
}

fun trim(s: &str).R: String = trim_end(trim_start(s))

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

// `s` repeated `n` times.
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
