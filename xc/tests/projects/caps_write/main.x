// M7: the write half of the filesystem surface, all inside the grant.
impl "util" with caps(fs = ["/tmp/xc_caps_write/**"])

fun main() {
    util.exercise()
}
