impl "std/mem"
impl "std/str"

// `/` is the separator on every platform in the API; a `\` translation
// belongs at the syscall boundary, not here (stdlib plan, `path` section).
fun is_absolute(s: &str).R: bool = str.len(s) > 0 && str.byte(s, 0) == 47

// Purely lexical normalization: `//` collapses, `.` is dropped, `..` pops a
// segment. It is *the same C function* the capability checker uses (M7),
// which is the plan's requirement — the security-relevant normalization must
// exist once, not twice.
// Panics: if the path is not absolute, or escapes its own root via `..`.
extern fun normalize_raw(s: &str).R: String

fun normalize(s: &str).R: String = normalize_raw(s)

// The byte index just past the last `/`, or 0 when the path has none. A
// path with no separator has no directory part at all, so `file_name`
// reports the whole string — which is what every other path library does.
fun last_sep(s: &str).R: usize {
    val n = str.len(s)
    var i: usize = n
    while i > 0 {
        if str.byte(s, i - 1) == 47 {
            return i
        }
        i = i - 1
    }
    return 0
}

// The final component: a *view* of the tail, so no allocation.
fun file_name(s: &str).R: &str = str.substring(s, last_sep(s))

// The text after the last `.` in the final component, or the empty string.
fun extension(s: &str).R: &str {
    val start = last_sep(s)
    val n = str.len(s)
    var i: usize = n
    while i > start {
        if str.byte(s, i - 1) == 46 {
            return str.substring(s, i)
        }
        i = i - 1
    }
    return str.substring(s, n)
}

// The directory part, as an owned copy: a prefix cannot be a view (see
// `str.trim_end` on why truncation allocates). The empty string means "no
// directory part", which is the honest answer for a bare file name.
fun parent(s: &str).R: String {
    val end = last_sep(s)
    if end == 0 {
        return String.empty()
    }
    var cut: usize = end - 1
    while cut > 1 && str.byte(s, cut - 1) == 47 {
        cut = cut - 1
    }
    return str.prefix(s, cut)
}

// `a` joined with `b`, with exactly one `/` between them, and without
// copying either side's bytes twice. An absolute `b` replaces `a`, which is
// what every other path library does.
//
// `normalize` (lexical `//`, `.`, and `..` handling) is *not* here yet: it
// is the same function the capability checker will use in M7, so it should
// be written once, against the grant matcher, rather than twice.
fun join(a: &str, b: &str).R: String {
    if is_absolute(b) {
        return String.from_str(b)
    }
    val la = str.len(a)
    val lb = str.len(b)
    val sep: usize = if la > 0 && str.byte(a, la - 1) == 47 { 0 } else { 1 }
    var out = mem.alloc(la + lb + sep)
    var i: usize = 0
    while i < la {
        out.set(i, str.byte(a, i))
        i = i + 1
    }
    if sep == 1 {
        out.set(la, 47)
    }
    var j: usize = 0
    while j < lb {
        out.set(la + sep + j, str.byte(b, j))
        j = j + 1
    }
    return String.from_bytes(out)
}
