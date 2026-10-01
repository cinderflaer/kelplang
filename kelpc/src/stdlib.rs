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

/// The module that declares `Option<T>`. A compiler builtin that hands back an
/// `Option` (`try_to_<T>`, `MISSING-RbhdW`) names the instance through
/// `key_prefix` of this id, because a builtin has no declaration site for the
/// resolver to qualify the type at.
pub const OPTION_MODULE: &str = "std/option";

/// The module that declares `Ordering`, the one type in the atomics surface
/// that is a plain Kelp `enum` rather than a compiler builtin (MK9-S3-M1 stage
/// A1). A builtin signature has no declaration site for the resolver to qualify
/// a type at, so the `Atomic<T>` method signatures name `Ordering` by this id's
/// `key_prefix`, exactly as `try_to_<T>` names `Option` through `OPTION_MODULE`.
pub const ATOMIC_MODULE: &str = "std/atomic";

/// The module that declares `ThreadErr`, and the module `thread.spawn` and
/// `thread.join` are reached through (MK9-S3-M1 stage B1). `ThreadErr` is a
/// plain Kelp `datac` in this module, and the builtin `thread.join` signature
/// names it by this id's `key_prefix`, exactly as an `Atomic<T>` signature
/// names `Ordering` through `ATOMIC_MODULE`: a builtin signature has no
/// declaration site for the resolver to qualify a type at.
pub const THREAD_MODULE: &str = "std/thread";

/// The call key of `thread.fence`, the one function the shipped `std/thread`
/// module holds (MK9-S3-M1 stage A2): the module id with `/` replaced by `.`
/// plus the function's bare name, which is what `resolve.rs` builds for a
/// shipped module's item. Both backends compare a call's key against it and
/// lower the call as the one instruction a fence is (`__atomic_thread_fence`
/// and `fence seq_cst`), so the key is named here rather than spelled at those
/// two comparisons.
pub const FENCE_FN: &str = "std.thread.fence";

/// The call keys of the two thread calls the checker handles structurally
/// (MK9-S3-M1 stage B1). Neither has a fixed signature: `thread.spawn`'s `T`
/// comes from its entry point's single parameter, and `thread.join`'s from the
/// handle it is given, so the two live beside `FENCE_FN` rather than in the
/// builtin signature table, whose entries are all concrete. Stage C1 lowers
/// both; stage B1 refuses every call that passes the checker's rules.
pub const SPAWN_FN: &str = "std.thread.spawn";
pub const JOIN_FN: &str = "std.thread.join";

/// The declaration `ThreadErr`'s name and the one field it carries, in the
/// shipped `std/thread` module. Both backends build the `Err` arm of
/// `thread.join`'s result at the call site, and they spell the `datac` and its
/// field through these, so a rename in the module cannot leave one of them
/// constructing a struct the declaration no longer has.
pub const THREAD_ERR_NAME: &str = "ThreadErr";
pub const THREAD_ERR_MESSAGE_FIELD: &str = "message";

/// Whether `key` is one of the two thread calls above. One predicate rather
/// than a `matches!` at each of the two sites that recognize them, because the
/// module item path and the call checker have to agree on which keys they are.
pub fn is_thread_fn(key: &str) -> bool {
    key == SPAWN_FN || key == JOIN_FN
}

/// The module that declares `Result<T, E>`. A builtin whose result *is* a
/// `Result` names the instance through `key_prefix` of this id, for the same
/// reason `OPTION_MODULE` exists: `Atomic.compare_exchange` hands back
/// `Result<T, T>` (MK9 4) and has no declaration site to name it at.
pub const RESULT_MODULE: &str = "std/result";

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
        "std/tls" => include_str!("../std/tls.kp"),
        "std/http" => include_str!("../std/http.kp"),
        "std/ws" => include_str!("../std/ws.kp"),
        "std/encoding" => include_str!("../std/encoding.kp"),
        "std/list" => include_str!("../std/list.kp"),
        "std/atomic" => include_str!("../std/atomic.kp"),
        "std/thread" => include_str!("../std/thread.kp"),
        "std/option" => include_str!("../std/option.kp"),
        "std/result" => include_str!("../std/result.kp"),
        "std/arena" => include_str!("../std/arena.kp"),
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
        "std/fmt", "std/io", "std/process", "std/net", "std/tls", "std/http", "std/ws",
        "std/encoding", "std/list", "std/option", "std/result", "std/atomic", "std/thread",
        "std/arena",
    ]
}

/// The shipped modules the compiler's own audit list admits to `unsafe`
/// (kernel stage 13), the stdlib's `[unsafe] modules`. A project's manifest is
/// the list for its own modules; a module that ships *with* the compiler cannot
/// ask every project that imports it to write its id, so the compiler keeps the
/// list itself, and `resolve.rs` admits a module that is on either list. Only
/// the compiler's source can add an entry here, so a dependency still cannot
/// grant itself the power (kernel-path rule 1); the list stays short and is
/// read beside the module it names.
pub const UNSAFE_MODULES: &[&str] = &["std/arena"];

/// Whether `id` is a shipped module the compiler lists for `unsafe`.
pub fn may_use_unsafe(id: &str) -> bool {
    UNSAFE_MODULES.contains(&id)
}

/// The capability domains of v1 (the plan's "Capability domains"). Each has
/// a corresponding bare built-in namespace, and the *only* way to reach the
/// underlying resource is through it.
///
/// `ui` is the one domain whose patterns are not enforced: a display call
/// (`gtk_window_new`) carries no argument that names the display server, so
/// the only grant that means anything is the whole-resource `*`, and
/// `resolve.rs` refuses every other list. `plan/ffi-display-domain.md` owns
/// that decision and the per-display vocabulary it waits on.
///
/// `mmio` is the one domain whose patterns are address *ranges* rather than
/// names or paths (`"0xBASE+0xLEN"`), and the one the entry module does not
/// hold by default: a device's address is a fact about the machine, not a
/// resource the root of trust can be assumed to own, so a grant has to be
/// written down (`plan/device-caps.md`). `caps(*)` hands over every other
/// domain's whole resource and deliberately leaves `mmio` out, because the
/// whole-address-space grant is the escape hatch a reviewer greps for.
pub const DOMAINS: &[&str] =
    &["fs", "net", "process", "env", "time", "rand", "ui", "mmio"];

pub fn is_domain(name: &str) -> bool {
    DOMAINS.contains(&name)
}

/// The `mmio` domain's wildcard: the whole address space. It is the one grant
/// that admits a `Ptr.from_addr` whose address the compiler cannot fold, and
/// the line a reviewer greps for to find every module that can reach any
/// address at all.
pub const MMIO_ANY: &str = "*";

/// Parses one `mmio` grant pattern, `"0xBASE+0xLEN"`: `BASE` and `LEN` in hex
/// (`0x`) or decimal, `+` the separator, `LEN` in bytes. Returns `(base, len)`
/// at `u128` so `base + len` can never wrap: a range that reaches the top of
/// the address space has to answer for its last byte, not for zero.
///
/// A pattern is either the wildcard (checked by the caller, which needs the
/// whole list to tell the two forms apart) or a range; a bare address, a
/// missing or empty length, a non-numeric part and a zero length are all
/// errors, each naming the pattern it read.
pub fn parse_mmio_range(pattern: &str) -> Result<(u128, u128), String> {
    let (base, len) = pattern.split_once('+').ok_or_else(|| {
        format!(
            "'{pattern}' is not an mmio range - write 'BASE+LEN' ('0x10000000+0x100'), both in hex or decimal"
        )
    })?;
    let base = parse_mmio_number(base)
        .ok_or_else(|| format!("the base in the mmio range '{pattern}' is not a number"))?;
    let len = parse_mmio_number(len)
        .ok_or_else(|| format!("the length in the mmio range '{pattern}' is not a number"))?;
    if len == 0 {
        return Err(format!(
            "the mmio range '{pattern}' has zero length - LEN counts bytes and must be at least 1"
        ));
    }
    Ok((base, len))
}

/// One side of an `mmio` range: `0x`/`0X` for hex, otherwise decimal digits.
/// No sign, no whitespace, no underscores: the grammar is small on purpose so
/// a range cannot mean one thing to a reviewer and another to the compiler.
fn parse_mmio_number(text: &str) -> Option<u128> {
    if text.is_empty() {
        return None;
    }
    let (digits, radix) = match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => (hex, 16),
        None => (text, 10),
    };
    if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
        return None;
    }
    u128::from_str_radix(digits, radix).ok()
}

/// Whether two parsed ranges share any byte. `len` is non-zero by
/// construction, so an empty intersection means disjoint.
pub fn mmio_ranges_overlap(a: (u128, u128), b: (u128, u128)) -> bool {
    a.0 < b.0 + b.1 && b.0 < a.0 + a.1
}

/// Whether a module's `mmio` patterns admit `addr`. A `*` admits every
/// address; otherwise `addr` has to fall inside one half-open range,
/// `[base, base + len)`, so `base + len - 1` is inside and `base + len` is
/// not. Order does not matter and ranges are disjoint (the grant is validated
/// before it is stored), so the first match wins.
pub fn mmio_covers(patterns: &[String], addr: u128) -> bool {
    patterns.iter().any(|pattern| {
        if pattern == MMIO_ANY {
            return true;
        }
        parse_mmio_range(pattern)
            .map(|(base, len)| addr >= base && addr < base + len)
            .unwrap_or(false)
    })
}

/// The domain a stdlib module's items are gated on, if it is a gated module.
/// `std/mem`, `std/str`, `std/path` and `std/math` are ungated: they are
/// arithmetic and byte shuffling, and cannot reach the outside world.
///
/// `std/ws` is gated **and** needs a second domain, which is unusual: its
/// items are reached through `net` (so a caller must hold it, as `std/net`'s
/// own items require), and its text also names `std/rand` for the handshake's
/// key and the masking key of every frame. A module holds what its importer
/// grants, so a program that imports it writes both domains on that `impl`
/// line: `caps(net = ["host:port"], rand = [])`.
pub fn domain_of(module: &str) -> Option<&'static str> {
    Some(match module {
        "std/fs" => "fs",
        "std/net" => "net",
        "std/tls" => "net",
        "std/ws" => "net",
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
///
/// `ui` is not a path domain either, and its pattern is never matched against
/// anything: see `DOMAINS`.
pub fn patterns_are_paths(domain: &str) -> bool {
    matches!(domain, "fs" | "process")
}

/// The pattern that names a domain's whole resource: `/**` for a path domain,
/// `*` for a name domain (`net`, `env`). It is what the entry module holds
/// implicitly for every domain, and what `caps(*)` delegates to an import
/// (`syntax.md` 15.2), so both spell it through this one function rather than
/// repeating the vocabulary.
///
/// `time`, `rand` and `ui` have no named resource at all, so their
/// whole-resource pattern is never matched against anything: it is the domain
/// gate alone. For `ui` it is also the only pattern a grant may name.
pub fn whole_resource(domain: &str) -> &'static str {
    if patterns_are_paths(domain) {
        "/**"
    } else {
        "*"
    }
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

/// Whether a type is `str` or a borrow of one: the arguments a capability
/// check treats as resource names, and so the input `checked_args` reads. It
/// lives here rather than beside its caller because the question is about the
/// language's types, not about one backend's code generation.
///
/// `Ty` is named in full rather than imported: this module is otherwise about
/// the shipped sources and the domains, not about the type checker.
pub fn is_str_like_ty(ty: &crate::types::Ty) -> bool {
    match ty {
        crate::types::Ty::Str => true,
        crate::types::Ty::Ref { inner, .. } => matches!(**inner, crate::types::Ty::Str),
        _ => false,
    }
}

/// Which of a gated call's arguments are *resource names* to be matched
/// against the module's allowlist. `fs`'s paths are every `str` argument
/// (`rename`, `copy` and `symlink` have two, and `open`'s mode is not one);
/// `env` checks its variable name; `process` checks the program path, which is
/// not argument 0 for every call (see `process_checked_args`); `net` names its
/// resources positionally, because its signatures mix resources with ordinary
/// strings and numbers. `time`, `rand` and `ui` have no named resource at all,
/// so there is nothing to match.
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
/// the directory first, and so does the primitive `spawn_raw(dir, program,
/// ...)` they wrap. Getting this wrong is not cosmetic: checking argument 0
/// there would match the *directory* against a program allowlist, which denies
/// every call whose directory is outside the granted programs, while leaving
/// the program itself unchecked.
fn process_checked_args(callee: &str, arg_is_str: &[bool]) -> Vec<CapCheck> {
    let program_at = match callee {
        "spawn_in" | "run_in" | "try_run_in" | "spawn_raw" => 1,
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
/// one, and `try_open` has the same signature. This is the same shape `net`
/// needed: the call's own name decides.
///
/// `symlink` is the one call where checking both ends is a *design* choice
/// rather than an obvious one: the link's own path is what a caller creates,
/// and the target could be checked alone, since every later access through the
/// link resolves it first and is denied if it points outside. Checking the
/// target too keeps an `fs` grant **closed under link creation** (a module
/// can only create a link whose target it could also name), which is what
/// stops a tree inside a grant from containing a link out of it (the case
/// `fs.remove_all` refuses to walk, and now says so).
fn fs_checked_args(callee: &str, arg_is_str: &[bool]) -> Vec<CapCheck> {
    // The primitives carry the same signature as the wrappers they back, so
    // `open_raw`'s mode argument is not a path for the same reason and must
    // not be normalized and matched as one.
    if matches!(callee, "open" | "try_open" | "open_raw" | "try_open_raw") {
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
/// host. The primitives are listed under their own `_raw` names because this
/// table is keyed on the name the author wrote, and a raw call must not be
/// left with *no* check while the wrapper it backs is checked. Everything
/// else in the namespace (the `Socket`/`Listener` methods, `send`, `recv`)
/// names no resource the grant governs, and is admitted by the domain gate
/// alone.
///
/// The fallible dial is here too, under both of its names: `try_connect` is
/// checked exactly as `connect` is, so a caller cannot reach a host its grant
/// does not name by asking for the fallible form.
fn net_checked_args(callee: &str, _arg_is_str: &[bool]) -> Vec<CapCheck> {
    match callee {
        "connect" | "connect_raw" | "try_connect" | "connect_try_raw" => {
            vec![CapCheck::HostPort(0, 1)]
        }
        "listen" | "listen_raw" => vec![CapCheck::Port(0)],
        "resolve" | "resolve_raw" => vec![CapCheck::Host(0)],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_domain_list_names_every_domain_once() {
        assert_eq!(
            DOMAINS,
            &["fs", "net", "process", "env", "time", "rand", "ui", "mmio"]
        );
        let mut sorted = DOMAINS.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), DOMAINS.len(), "a domain name appears twice");
    }

    #[test]
    fn ui_is_a_name_domain_with_the_whole_resource_wildcard() {
        assert!(is_domain("ui"));
        assert!(!patterns_are_paths("ui"));
        assert_eq!(whole_resource("ui"), "*");
    }

    #[test]
    fn mmio_is_a_name_domain_with_the_whole_resource_wildcard() {
        assert!(is_domain("mmio"));
        assert!(!patterns_are_paths("mmio"));
        assert_eq!(whole_resource("mmio"), "*");
    }

    #[test]
    fn an_mmio_range_reads_hex_or_decimal() {
        assert_eq!(parse_mmio_range("0x10000000+0x100"), Ok((0x10000000, 0x100)));
        assert_eq!(parse_mmio_range("0X10000000+0X100"), Ok((0x10000000, 0x100)));
        assert_eq!(parse_mmio_range("4096+16"), Ok((4096, 16)));
        assert_eq!(
            parse_mmio_range("0x3FFFFFF000+0x1000"),
            Ok((0x3FFFFFF000, 0x1000))
        );
    }

    #[test]
    fn an_mmio_range_refuses_a_bare_address_or_a_zero_length() {
        assert!(parse_mmio_range("0x10000000").is_err());
        assert!(parse_mmio_range("0x10000000+0x0").is_err());
        assert!(parse_mmio_range("+0x100").is_err());
        assert!(parse_mmio_range("0x10000000+").is_err());
        assert!(parse_mmio_range("0x10000000+zz").is_err());
        assert!(parse_mmio_range("0x+0x1").is_err());
        assert!(parse_mmio_range("1_000+16").is_err());
        assert!(parse_mmio_range("0x10000000+0x100+0x10").is_err());
    }

    #[test]
    fn mmio_ranges_overlap_only_when_they_share_a_byte() {
        assert!(!mmio_ranges_overlap((0x1000, 0x100), (0x1100, 0x100)));
        assert!(mmio_ranges_overlap((0x1000, 0x101), (0x1100, 0x100)));
        assert!(mmio_ranges_overlap((0x1000, 0x100), (0x1080, 0x100)));
        assert!(mmio_ranges_overlap((0x1000, 0x100), (0x1000, 0x100)));
    }

    #[test]
    fn mmio_covers_the_half_open_range_and_the_wildcard() {
        let granted = vec!["0x10000000+0x100".to_string()];
        assert!(mmio_covers(&granted, 0x10000000));
        assert!(mmio_covers(&granted, 0x100000FF));
        assert!(!mmio_covers(&granted, 0x10000100));
        assert!(!mmio_covers(&granted, 0x0FFFFFFF));
        assert!(mmio_covers(&[MMIO_ANY.to_string()], 0xDEADBEEF));
    }

    #[test]
    fn a_ui_call_names_no_resource_to_match() {
        // No argument at a display call names the display server, which is
        // why `ui` is granted as the wildcard only (plan/ffi-display-domain.md).
        assert!(checked_args("ui", "gtk_window_new", &[false, false]).is_empty());
        assert!(checked_args("ui", "gtk_window_set_title", &[false, true]).is_empty());
    }

    #[test]
    fn the_tls_handle_of_a_connection_is_an_option_not_a_usize() {
        // Release goal 1: `std/http` and `std/ws` used a `tls_handle: usize`
        // whose `0` meant "not TLS"; an opaque `extern type` has no integer
        // value, so both carry `Option<tls.TlsHandle>` and `None` is that
        // value. `Conn` is internal to the two modules, so the field's type
        // is checked where it is written rather than through a program.
        for module in ["std/http", "std/ws"] {
            let text = source(module).expect("a shipped module has source");
            assert!(
                text.contains("tls_handle: option.Option<tls.TlsHandle>"),
                "{module} no longer carries an Option<tls.TlsHandle>"
            );
            assert!(
                !text.contains("tls_handle: usize"),
                "{module} still carries the raw usize handle"
            );
        }
    }
}
