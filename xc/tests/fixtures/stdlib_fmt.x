// S1: `std/fmt`. Numbers, hex, padding, and the byte dump that is the only
// way to see binary data today (`print` cannot show a `datac` or a `List`).
impl "std/str"
impl "std/fmt"

fun main() {
    print(fmt.int(0))
    print(fmt.int(0 - 1234))
    print(fmt.uint(4294967295))
    print(fmt.float(3.14159, 3))
    print(fmt.float(1.5, 1))
    print(fmt.hex(0))
    print(fmt.hex(255))
    print(fmt.hex(48879))
    print(fmt.pad_left("7", 3, 48))
    print(fmt.pad_left("long", 3, 32))
    print(fmt.pad_right("7", 3, 46))
    print(fmt.bytes(Bytes.from_str("ab")))
    print(fmt.bytes(Bytes.empty()))
    // A `String` is not a `str` (open item 21): the owned result is bound
    // and viewed before it can be concatenated.
    val padded = fmt.pad_left("7", 3, 48)
    print(str.concat(padded.as_str(), "!"))
}
