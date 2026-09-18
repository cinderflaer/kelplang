// M6.3: `&str` return elision. `trim_start`, `substring` and `file_name` are
// views of their argument (no allocation); `trim` and `trim_end` truncate,
// which a `&str` cannot express, so they return an owned `String`.
//
// A `String` is not a `str`: passing one where a view is expected needs an
// explicit `.as_str()` (there is no deref coercion in v1), which is why the
// calls below go through it.
impl "std/str"

fun main() {
    val s = String.from_str("  hello  ")
    val v = s.as_str()
    val t = str.trim(v)
    print(t)
    print(str.len(t.as_str()))

    val lead = str.trim_start(v)
    print(lead)
    print(str.len(lead))

    val tail = str.substring(t.as_str(), 1)
    print(tail)
    print(str.starts_with(t.as_str(), prefix = "hello"))
    print(str.ends_with(t.as_str(), suffix = "lo"))
    print(str.contains(t.as_str(), needle = "ell"))
    print(str.eq(t.as_str(), b = "hello"))
    print(str.eq(t.as_str(), b = "hellO"))
    val m = str.index_of(t.as_str(), needle = "l")
    print(m.ok)
    print(m.at)

    val b = Bytes.from_str("raw")
    print(b.as_str())
    print(str.char_count("héllo"))
    print(str.trim("  own  "))
}
