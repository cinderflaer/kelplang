impl "std/str"

// std/fmt — turning values into text (S1). Ungated: this is arithmetic and
// byte shuffling, and it cannot reach the outside world.
//
// Everything here is a thin X wrapper over `std/str`, except the two places
// where the digit work needs C: `snprintf` for hexadecimal, and a byte-level
// hex dump. `fmt` deliberately has no variadic `format`: that needs generics
// or macros, and neither exists in v1 (see plan.md's non-goals). Callers
// concatenate with `str.concat` or push into a `String`.

fun int(n: i64).R: String = str.from_i64(n)

fun uint(n: u64).R: String = str.from_u64(n)

fun float(x: f64, decimals: i32).R: String = str.from_f64(x, decimals)

extern fun hex_raw(n: u64).R: String

// Lowercase, no `0x` prefix.
fun hex(n: u64).R: String = hex_raw(n)

extern fun bytes_raw(b: &Bytes).R: String

// A byte dump for debugging binary data: "1f 8b 08". Lowercase, one space
// between bytes, no trailing space. This is the honest first step towards
// *seeing* your data — `print` still cannot show a `datac` or a `List`
// (open item 35).
fun bytes(b: Bytes).R: String = bytes_raw(b)

// `s` right-aligned in a field of `width`, padded with `fill` (a byte, not a
// `char`: v1 has no char literal — see plan.md's open item 53). A string at
// least `width` long is copied unchanged.
fun pad_left(s: &str, width: usize, fill: u8).R: String {
    val n = str.len(s)
    if n >= width {
        return String.from_str(s)
    }
    var out = String.with_capacity(width)
    var i = n
    while i < width {
        out.push(fill)
        i = i + 1
    }
    out.push_str(s)
    return out
}

// `s` left-aligned in a field of `width`, padded with `fill`.
fun pad_right(s: &str, width: usize, fill: u8).R: String {
    val n = str.len(s)
    if n >= width {
        return String.from_str(s)
    }
    var out = String.with_capacity(width)
    out.push_str(s)
    var i = n
    while i < width {
        out.push(fill)
        i = i + 1
    }
    return out
}
