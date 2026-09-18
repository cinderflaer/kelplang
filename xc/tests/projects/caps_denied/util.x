impl "std/fs"

fun outside().R: String = fs.read_text("/etc/passwd")
