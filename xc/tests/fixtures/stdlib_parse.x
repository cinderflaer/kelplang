// S1: parsing. `ok` is false unless the whole string is one number, and an
// out-of-range value is a failure, not a wrapped number.
impl "std/str"

fun main() {
    val a = str.parse_i64("123")
    print(a.ok)
    print(a.value)
    val m = str.parse_i64("-9223372036854775808")
    print(m.ok)
    print(m.value)
    print(str.parse_i64("9223372036854775808").ok)
    print(str.parse_i64("12x").ok)
    print(str.parse_i64("").ok)
    print(str.parse_i64(" 12").ok)
    val u = str.parse_u64("18446744073709551615")
    print(u.ok)
    print(str.from_u64(u.value))
    print(str.parse_u64("-1").ok)
    print(str.parse_u64("18446744073709551616").ok)
    val f = str.parse_f64("2.5e2")
    print(f.ok)
    print(f.value)
    print(str.parse_f64("2.5x").ok)
    // Codepoint-level text: `char_at` is a character index, `byte_index` maps
    // it back, and a multi-byte character reports its codepoint (é = 233).
    print(str.char_at("héllo", 0))
    print(str.char_at("héllo", 1))
    print(str.byte_index("héllo", 2))
    print(str.char_at("abc", 9))
    print(str.char_count("héllo"))
}
