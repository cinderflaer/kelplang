// M6.3: the `path` component functions. `file_name`/`extension` are views of
// their argument; `parent` truncates, so it allocates a `String`.
impl "std/path"

fun main() {
    val f = "/tmp/notes.txt"
    print(path.file_name(f))
    print(path.extension(f))
    val d = path.parent(f)
    print(d)
    print(path.file_name("bare.txt"))
    print(path.extension("bare.txt"))
    print(path.parent("bare.txt"))
    val j = path.join(a = "/var", b = "log")
    print(j)
    print(path.is_absolute(f))
}
