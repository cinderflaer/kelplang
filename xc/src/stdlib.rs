//! The compiler-shipped standard library (M6.2).
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
        _ => return None,
    })
}

/// The names of every stdlib module the compiler ships, for diagnostics that
/// want to list them.
pub fn shipped() -> &'static [&'static str] {
    &["std/mem", "std/str", "std/path"]
}
