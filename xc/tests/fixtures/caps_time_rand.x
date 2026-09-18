// M7: `time` and `rand` are capabilities too (WASI's clocks and random), and
// main holds every domain implicitly. Neither has a named resource, so there
// is nothing to match — the gate alone is the whole check.
impl "std/time"
impl "std/rand"

fun main() {
    print(time.now_unix() > 0)
    print(time.now_mono_ns() > 0)
    print(rand.below(n = 10) < 10)
    val b = rand.bytes(n = 4)
    print(b.len())
    time.sleep_ms(ms = 1)
    print("slept")
}
