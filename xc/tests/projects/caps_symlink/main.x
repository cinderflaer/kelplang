// The symlink is *inside* the grant; it points outside it. Because symlinks
// are resolved before matching, the read is denied.
impl "util" with caps(fs = ["/tmp/xc_caps_symlink/**"])

fun main() {
    print(util.follow())
}
