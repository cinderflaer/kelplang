impl "std/process"

fun count().R: usize = process.arg_count()

// Everything after `argv[0]`, joined with commas.
fun after_program_name().R: String {
    val args = process.args()
    val n = args.len()
    var out = String.with_capacity(16)
    var i: usize = 1
    while i < n {
        if i > 1 {
            out.push_str(",")
        }
        out.push_str(args.get(i).as_str())
        i = i + 1
    }
    return out
}
