// The `env` bodies are real (MK5-S5): read a granted name, and set/read/unset
// another. The grant covers exactly the two names used.
impl "std/str"
impl "std/env"

fun has_home().R: bool = env.has("HOME")

fun get_home_nonempty().R: bool {
    val v = env.get("HOME")
    return str.len(v.as_str()) > 0
}

fun set_read_unset().R: bool {
    env.set("XC_ENV_PROBE", "value")
    val v = env.get("XC_ENV_PROBE")
    val ok = str.eq(v.as_str(), "value")
    env.unset("XC_ENV_PROBE")
    return ok && !env.has("XC_ENV_PROBE")
}
