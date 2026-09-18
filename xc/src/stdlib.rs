//! The compiler-shipped standard library (M6.2) and its capability domains
//! (M7).
//!
//! Stdlib decision 3 in `plan.md`: stdlib modules are `.x` sources shipped
//! *with the compiler* rather than generated from C snippets, so the API is
//! reviewable X code. They are embedded into the binary with `include_str!`
//! rather than read from a directory next to the executable: `xc` is then a
//! single self-contained file with no install-time layout to get wrong, and
//! the stdlib version is pinned to the compiler version for free.
//!
//! `resolve.rs` treats a `std/...` id exactly like a project module — same
//! lexer, same parser, same aliases, same prefixing — with the text coming
//! from here instead of from `src/`.

/// Source for the stdlib module with `impl` id `id` (`std/mem`), or `None`
/// when the compiler does not ship it.
pub fn source(id: &str) -> Option<&'static str> {
    Some(match id {
        "std/mem" => include_str!("../std/mem.x"),
        "std/str" => include_str!("../std/str.x"),
        "std/path" => include_str!("../std/path.x"),
        "std/math" => include_str!("../std/math.x"),
        "std/fs" => include_str!("../std/fs.x"),
        "std/time" => include_str!("../std/time.x"),
        "std/rand" => include_str!("../std/rand.x"),
        "std/env" => include_str!("../std/env.x"),
        "std/fmt" => include_str!("../std/fmt.x"),
        _ => return None,
    })
}

/// The names of every stdlib module the compiler ships, for diagnostics that
/// want to list them.
pub fn shipped() -> &'static [&'static str] {
    &[
        "std/mem", "std/str", "std/path", "std/math", "std/fs", "std/time", "std/rand", "std/env",
        "std/fmt",
    ]
}

/// The capability domains of v1 (`plan.md`'s "Capability domains"). Each has
/// a corresponding bare built-in namespace, and the *only* way to reach the
/// underlying resource is through it.
pub const DOMAINS: &[&str] = &["fs", "net", "process", "env", "time", "rand"];

pub fn is_domain(name: &str) -> bool {
    DOMAINS.contains(&name)
}

/// The domain a stdlib module's items are gated on, if it is a gated module.
/// `std/mem`, `std/str`, `std/path` and `std/math` are ungated: they are
/// arithmetic and byte shuffling, and cannot reach the outside world.
pub fn domain_of(module: &str) -> Option<&'static str> {
    Some(match module {
        "std/fs" => "fs",
        "std/net" => "net",
        "std/process" => "process",
        "std/env" => "env",
        "std/time" => "time",
        "std/rand" => "rand",
        _ => return None,
    })
}

/// Whether a domain's patterns name paths (normalized lexically and resolved
/// through symlinks before matching) or plain names (`env`'s variable names,
/// which have no path structure at all).
pub fn patterns_are_paths(domain: &str) -> bool {
    matches!(domain, "fs" | "net" | "process")
}

/// Which of a gated call's arguments are *resource names* to be matched
/// against the module's allowlist. `fs`'s paths are every `str` argument
/// (`rename` checks both ends); a namespace whose allowlist governs the first
/// argument only — `env`'s variable name, `net`'s host — says so; `time` and
/// `rand` have no named resource at all, so there is nothing to match.
pub fn checked_args(domain: &str, arg_is_str: &[bool]) -> Vec<usize> {
    match domain {
        "fs" => arg_is_str
            .iter()
            .enumerate()
            .filter(|(_, is_str)| **is_str)
            .map(|(i, _)| i)
            .collect(),
        "env" | "net" | "process" => match arg_is_str.first() {
            Some(true) => vec![0],
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}
