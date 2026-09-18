impl "std/fs"

fun follow().R: String = fs.read_text("/tmp/xc_caps_symlink/escape")
