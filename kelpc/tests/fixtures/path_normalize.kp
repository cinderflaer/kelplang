// M7: `path.normalize` is the *same* C function the capability checker uses,
// so there is exactly one normalization in the compiler.
impl "std/path"

fun main() {
    print(path.normalize("/a//b/./c"))
    print(path.normalize("/a/b/../c"))
    print(path.normalize("/a"))
    print(path.normalize("/tmp/scratch/"))
}
