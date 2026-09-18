impl "std/mem"
impl "std/str"

// `/` is the separator on every platform in the API; a `\` translation
// belongs at the syscall boundary, not here (stdlib plan, `path` section).
fun is_absolute(s: &str).R: bool = str.len(s) > 0 && str.byte(s, 0) == 47

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
