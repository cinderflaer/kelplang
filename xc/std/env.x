// std/env — the gated environment namespace (M7).
//
// The gate, the grant and the runtime name check are real: this module's
// items are unreachable from a module that was not granted `env`, and an
// argument is matched against the granted variable names before the call.
// The bodies themselves are stubs — the plan keeps `net`/`process`/`env`
// structurally gated with stub bodies until there is a reason to flesh them
// out, and this is the one shipped as the worked example of that.

extern fun unsupported_bool(what: &str).R: bool
extern fun unsupported_string(what: &str).R: String
extern fun unsupported_unit(what: &str)

// Panics: the namespace is gated but not implemented yet.
fun has(name: &str).R: bool = unsupported_bool("env.has")

// Panics: the namespace is gated but not implemented yet.
fun get(name: &str).R: String = unsupported_string("env.get")

// Panics: the namespace is gated but not implemented yet.
fun set(name: &str, value: &str) {
    unsupported_unit("env.set")
}
