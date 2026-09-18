impl "std/fs"

fun read_note().R: String = fs.read_text("/tmp/xc_caps_granted/note.txt")
