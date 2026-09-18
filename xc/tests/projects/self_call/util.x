// A module's own items are reachable *bare* inside that file, even though
// `resolve.rs` stores them with their module prefix. Both a function call
// and a `datac` construction exercise that lookup.
fun doubled(n: i32).R: i32 = n * 2

fun quadrupled(n: i32).R: i32 = doubled(n = doubled(n = n))

datac Pair(val a: i32, val b: i32)

fun pair_sum(p: Pair).R: i32 = p.a + p.b

fun make_pair().R: Pair = Pair(a = 1, b = 2)
