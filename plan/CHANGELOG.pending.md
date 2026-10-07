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
- `loop { ... }`, loop labels (`'outer: for ...`) and `break 'outer` / `continue 'outer` leave or continue an outer loop.
- A method can declare type parameters of its own: `fun Pair<A, B>.map_first<C>(f: fun(A).return: C).return: Pair<C, B>`, on a plain or a generic `datac`, inferred at the call and bound-able.
- A generic `enum` can carry methods (`fun Opt<T>.map<U>(...)`), and `Option`/`Result` now have `is_some`, `is_none`, `map`, `and_then`, `or_else`, `each` and `is_ok`, `is_err`, `and_then`, `or_else`, `each` as methods (the borrowing combinators; `unwrap_or` and friends take `move` and stay free functions).
- An integer literal where a float is expected is that float (`x * 2`, `val w: f64 = 3`), when exactly representable; `f32.to_f64()` and `f64.to_f32()` convert float widths; every `std/math` function has an `f32` overload.
- A line that ends in a binary or assignment operator, `,`, `.` or `->` continues onto the next line.
- Tuple patterns in `when` take literal positions (`(0, y)`, `('a', true)`) and nested tuples (`((a, b), c)`).
- `for (key, value) in map` walks a `Map` (a snapshot of the keys, in insertion order).
- Borrow check: two fields of one owner are different places. `bump(s.count, s.items)` takes both exclusively, a live `&mut s.count` leaves `s.items` usable, and a scoped block that writes `s.count` runs in a call that borrows `s.items`. Overlapping borrows (the whole owner and a field, the same field twice) are still refused.
- `loop` can be a value: `val x = loop { ... break value ... }` (also labelled: `break 'outer value`). `while` and `for` still cannot break with a value.
