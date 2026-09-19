//! The compiler-shipped standard library (M6.2) and its capability domains
//! (M7).
//!
//! Stdlib decision 3 in the plan: stdlib modules are `.kp` sources shipped
//! *with the compiler* rather than generated from C snippets, so the API is
//! reviewable Kelp code. They are embedded into the binary with `include_str!`
//! rather than read from a directory next to the executable: `kelpc` is then a
//! single self-contained file with no install-time layout to get wrong, and
//! the stdlib version is pinned to the compiler version for free.
//!
//! `resolve.rs` treats a `std/...` id exactly like a project module — same
//! lexer, same parser, same aliases, same prefixing — with the text coming
//! from here instead of from `src/`.

/// The compiler's own runtime module (MK7 §8). It is not a library a program is
/// meant to import: the resolver injects it for a program that imports a gated
/// module, because that is when generated code calls the capability matcher it
/// holds. The one other importer is the differential corpus, which calls the
/// matcher directly to compare it against its C reference.
pub const RT_MODULE: &str = "std/rt";

/// The function-key prefix of a module id: `std/rt` is `std.rt.`. The two have
/// to agree everywhere a key is built or recognized, so the rule lives here
/// rather than being spelled out at each call site.
pub fn key_prefix(module: &str) -> String {
    format!("{}.", module.replace('/', "."))
}

/// Source for the stdlib module with `impl` id `id` (`std/mem`), or `None`
/// when the compiler does not ship it.
///
/// This is the *configured* form: the shipped text of `std/http` carries a
/// placeholder for the project's connection ceiling, and this substitutes the
/// default so that the text is always lexable. Anything that only wants to
/// know whether a module is shipped can call it and ignore the text.
pub fn source(id: &str) -> Option<String> {
    source_configured(id, DEFAULT_HTTP_POOL)
}

/// The compiler's default when `coral.toml` says nothing: four kept-alive
/// connections per `host:port`. `0` disables pooling entirely, which is what
/// the manifest writes when a program wants the one-socket-per-request
/// behavior back.
pub const DEFAULT_HTTP_POOL: usize = 4;

/// The placeholder `std/http.kp` carries instead of a literal pool ceiling.
/// Substitution happens on the source text *before* lexing, so this spelling
/// never reaches the parser — and it deliberately avoids `%*name*%`, which is
/// the alias sigil and would be expanded by `preprocess.rs` first.
const HTTP_POOL_PLACEHOLDER: &str = "__KELP_HTTP_POOL__";

/// The shipped source for `id` with the project's configuration spliced in
/// (S4). Only `std/http` is configuration-dependent so far; every other
/// module's text is returned unchanged, and an unknown id is `None`.
pub fn source_configured(id: &str, http_pool: usize) -> Option<String> {
    let text = raw_source(id)?;
    Some(text.replace(HTTP_POOL_PLACEHOLDER, &http_pool.to_string()))
}

fn raw_source(id: &str) -> Option<&'static str> {
    Some(match id {
        // The compiler's own runtime (MK7 §8), in Kelp rather than C text. It
        // is shipped like a stdlib module because that is the only way a module
        // reaches the compiler, and it is *not* part of the stdlib surface: the
        // resolver injects it for a program that imports a gated module, and
        // the capability corpus imports it directly to test it against its C
        // reference. See `RT_MODULE`.
        id if id == RT_MODULE => include_str!("../std/rt.kp"),
        "std/mem" => include_str!("../std/mem.kp"),
        "std/str" => include_str!("../std/str.kp"),
        "std/path" => include_str!("../std/path.kp"),
        "std/math" => include_str!("../std/math.kp"),
        "std/fs" => include_str!("../std/fs.kp"),
        "std/time" => include_str!("../std/time.kp"),
        "std/rand" => include_str!("../std/rand.kp"),
        "std/env" => include_str!("../std/env.kp"),
        "std/fmt" => include_str!("../std/fmt.kp"),
        "std/io" => include_str!("../std/io.kp"),
        "std/process" => include_str!("../std/process.kp"),
        "std/net" => include_str!("../std/net.kp"),
        "std/http" => include_str!("../std/http.kp"),
        "std/list" => include_str!("../std/list.kp"),
        "std/option" => include_str!("../std/option.kp"),
        "std/result" => include_str!("../std/result.kp"),
        _ => return None,
    })
}

/// The names of every stdlib module a *program* may import, for diagnostics
/// that want to list them.
///
/// `std/rt` is deliberately absent even though `raw_source` resolves it: this
/// list is the answer to "which `std/...` may I write in an `impl`", and the
/// runtime is not part of that surface. A program that names it gets the module
/// (the differential corpus does exactly that, to call the capability matcher
/// directly), but nothing should be told it is a library to reach for.
pub fn shipped() -> &'static [&'static str] {
    &[
        "std/mem", "std/str", "std/path", "std/math", "std/fs", "std/time", "std/rand", "std/env",
        "std/fmt", "std/io", "std/process", "std/net", "std/http", "std/list", "std/option",
        "std/result",
    ]
}

/// The capability domains of v1 (the plan's "Capability domains"). Each has
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
///
/// `net` is **not** a path domain, and that was open item 32: counting it as
/// one meant `api.example.com:443` was rejected as "not absolute" and, had it
/// been accepted, every host would have been denied at runtime by a path
/// normalizer that refuses anything without a leading `/`. A `net` pattern is
/// `host` or `host:port`, matched as a name, and it goes through the host
/// matcher instead.
pub fn patterns_are_paths(domain: &str) -> bool {
    matches!(domain, "fs" | "process")
}

/// How one gated call's resource arguments are matched against the module's
/// compiled-in allowlist. Which variant applies is a property of the *call*,
/// not of the argument types: `net.send_text(s, text)` has a `str` argument
/// that is not a resource at all, while `net.recv(s, max)` has a `usize` that
/// is not a port. So the call's own name picks the shape, and a function with
/// no resource argument has none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapCheck {
    /// A path argument: lexically normalized, symlinks resolved, then matched.
    Path(usize),
    /// A plain name (`env`'s variable names): matched as written.
    Name(usize),
    /// A host on its own (`net.resolve`): the pattern's port, if it has one,
    /// is ignored, because resolution is not port-specific.
    Host(usize),
    /// A host and a port, matched together as `host:port`.
    HostPort(usize, usize),
    /// A port on its own (`net.listen`). The listener's host is the machine,
    /// and v1 has no `net.listen(host, port)`, so the port is what the grant
    /// governs: a pattern with no port constrains none.
    Port(usize),
}

/// Which of a gated call's arguments are *resource names* to be matched
/// against the module's allowlist. `fs`'s paths are every `str` argument
/// (`rename`, `copy` and `symlink` have two, and `open`'s mode is not one);
/// `env` checks its variable name; `process` checks the program path, which is
/// not argument 0 for every call (see `process_checked_args`); `net` names its
/// resources positionally, because its signatures mix resources with ordinary
/// strings and numbers. `time` and `rand` have no named resource at all, so
/// there is nothing to match.
///
/// `callee` is the bare function name as written at the call site
/// (`connect`, not `std.net.connect`).
pub fn checked_args(domain: &str, callee: &str, arg_is_str: &[bool]) -> Vec<CapCheck> {
    match domain {
        "fs" => fs_checked_args(callee, arg_is_str),
        "env" => first_str_arg(arg_is_str)
            .map(|i| vec![CapCheck::Name(i)])
            .unwrap_or_default(),
        "process" => process_checked_args(callee, arg_is_str),
        "net" => net_checked_args(callee, arg_is_str),
        _ => Vec::new(),
    }
}

/// The first `str`-shaped argument, or `None` when there is none — the shape
/// a call whose resource is simply "its path" needs.
fn first_str_arg(arg_is_str: &[bool]) -> Option<usize> {
    arg_is_str.iter().position(|is_str| *is_str)
}

/// `process`'s resource argument: the *program* path, which is the only
/// argument the grant governs. The arguments are not checked and are not part
/// of the grant (`process = ["/bin/sh"]` lets a module run `/bin/sh`, and what
/// it asks it to do is the module's business), and neither is the working
/// directory an `_in` form takes: a child's `chdir` changes the meaning of its
/// own relative paths only, and every grant pattern is absolute.
///
/// Which position holds the program is a property of the call, the same shape
/// `fs.open`'s mode argument and `net`'s resources needed: the `_in` forms put
/// the directory first. Getting this wrong is not cosmetic — checking
/// argument 0 there would match the *directory* against a program allowlist,
/// which denies every call whose directory is outside the granted programs.
fn process_checked_args(callee: &str, arg_is_str: &[bool]) -> Vec<CapCheck> {
    let program_at = match callee {
        "spawn_in" | "run_in" | "try_run_in" => 1,
        _ => 0,
    };
    match arg_is_str.get(program_at) {
        Some(true) => vec![CapCheck::Path(program_at)],
        _ => Vec::new(),
    }
}

/// `fs`'s resource arguments. Nearly every `str` argument is a path
/// (`rename`, `copy` and `symlink` have two), but `open`'s second argument is a
/// mode (`"r"`/`"w"`/`"a"`/`"rw"`) and must not be normalised and matched as
/// one. This is the same shape `net` needed: the call's own name decides.
///
/// `symlink` is the one call where checking both ends is a *design* choice
/// rather than an obvious one: the link's own path is what a caller creates,
/// and the target could be checked alone, since every later access through the
/// link resolves it first and is denied if it points outside. Checking the
/// target too keeps an `fs` grant **closed under link creation** — a module
/// can only create a link whose target it could also name — which is what
/// stops a tree inside a grant from containing a link out of it (the case
/// `fs.remove_all` refuses to walk, and now says so).
fn fs_checked_args(callee: &str, arg_is_str: &[bool]) -> Vec<CapCheck> {
    if callee == "open" {
        return first_str_arg(arg_is_str)
            .map(|i| vec![CapCheck::Path(i)])
            .unwrap_or_default();
    }
    arg_is_str
        .iter()
        .enumerate()
        .filter(|(_, is_str)| **is_str)
        .map(|(i, _)| CapCheck::Path(i))
        .collect()
}

/// `net`'s resource arguments, by function. The host is argument 0 and the
/// port argument 1 in `connect`; `listen` has only a port; `resolve` only a
/// host. Everything else in the namespace — the `Socket`/`Listener` methods,
/// `send`, `recv` — names no resource the grant governs, and is admitted by
/// the domain gate alone.
fn net_checked_args(callee: &str, _arg_is_str: &[bool]) -> Vec<CapCheck> {
    match callee {
        "connect" => vec![CapCheck::HostPort(0, 1)],
        "listen" => vec![CapCheck::Port(0)],
        "resolve" => vec![CapCheck::Host(0)],
        _ => Vec::new(),
    }
}
