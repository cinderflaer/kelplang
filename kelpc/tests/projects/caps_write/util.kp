impl "std/fs"

fun exercise() {
    val text = String.from_str("hello")
    fs.write_text("/tmp/xc_caps_write/a.txt", text)
    print(fs.exists("/tmp/xc_caps_write/a.txt"))
    print(fs.size("/tmp/xc_caps_write/a.txt"))

    fs.rename("/tmp/xc_caps_write/a.txt", "/tmp/xc_caps_write/b.txt")
    print(fs.exists("/tmp/xc_caps_write/a.txt"))
    val back = fs.read("/tmp/xc_caps_write/b.txt")
    print(back)

    fs.remove("/tmp/xc_caps_write/b.txt")
    print(fs.exists("/tmp/xc_caps_write/b.txt"))

    fs.mkdir("/tmp/xc_caps_write/sub")
    print(fs.is_dir("/tmp/xc_caps_write/sub"))

    val bytes = Bytes.from_str("raw")
    fs.write("/tmp/xc_caps_write/c.bin", bytes)
    print(fs.size("/tmp/xc_caps_write/c.bin"))
}
