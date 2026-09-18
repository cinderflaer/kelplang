// MK5-S5 path polish: `stem` (owned, because a prefix cannot be a view),
// `has_extension`, `split` (non-empty components only), and `join_all`
// (`join`'s absolute-part rule applied repeatedly).
impl "std/path"

fun main() {
    print(path.stem("/a/b/archive.tar.gz"))
    print(path.stem("Makefile"))
    print(path.stem("/a/.bashrc"))
    print(path.has_extension("/a/b/note.txt", "txt"))
    print(path.has_extension("/a/b/note.txt", "md"))

    val parts = path.split("/a//b/")
    print(parts.len())
    print(parts.get(0).as_str())
    print(parts.get(1).as_str())

    var all: List<String> = List.empty()
    all.push(String.from_str("/a"))
    all.push(String.from_str("b"))
    all.push(String.from_str("c"))
    print(path.join_all(all))

    var abs: List<String> = List.empty()
    abs.push(String.from_str("/a"))
    abs.push(String.from_str("/etc"))
    abs.push(String.from_str("x"))
    print(path.join_all(abs))
}
