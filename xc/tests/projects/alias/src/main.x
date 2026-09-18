impl "util" as u

fun describe(t: u.Tally).R: i32 = u.bumped(t, 1)

fun main() {
    val t = u.Tally(label = 3)
    print(u.bumped(t, 4))
    print(describe(t))
}
