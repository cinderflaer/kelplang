# Pending changes

Scratch file for changes that are not released yet. Gitignored, never committed.

Add a line here when a user-visible change lands in `kelp`. **Do not touch
`wiki/changelog.md` until a version is released**: the changelog page carries
releases only.

When a version drops, write that release's entry in `wiki/changelog.md` from
this file plus the tag's commits and the Gitea release body, then wipe this file
back to this header. No need to ask first.

Keep each line short and factual: what changed, and why it matters to someone
writing Kelp. Group under the headings below; delete the headings that stay
empty on release.

## Fixed

- Borrow check: one call can no longer pass the same `mut` parameter to a `mut` parameter and a `&` parameter (`f(s, s)`); the shared reborrow was not tracked, so the aliasing was accepted in either order.
- Assigning to something that is not a place (`f -= 1` on a function name) is a diagnostic, not a compiler crash.

## Added

- Tuples: `t.0` reads a position, `for (a, b) in pairs` takes each element apart, and `when (a, b) { (Some(x), _) -> ... }` matches several values at once (variant, name or `_` per position).
- The return marker is `.return:` (`fun f(a: i32).return: i32`); `.R:` still compiles and prints an `outdated:` finding naming the fix, and is removed in the next development cycle. The shipped stdlib, examples and docs now write `.return:`.
- `loop { ... }`, loop labels (`'outer: for ...`) and `break 'outer` / `continue 'outer` leave or continue an outer loop. `break` still carries no value.
