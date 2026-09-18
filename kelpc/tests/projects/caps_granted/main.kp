// M7: main holds every capability implicitly, so it delegates a narrow `fs`
// grant to `util` and can use `fs` itself without restrictions.
impl "util" with caps(fs = ["/tmp/xc_caps_granted/**"])
impl "std/fs"

fun main() {
    print(util.read_note())
    print(fs.exists("/tmp/xc_caps_granted/note.txt"))
    print(fs.is_file("/tmp/xc_caps_granted/note.txt"))
}
