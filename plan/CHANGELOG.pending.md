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

- A tuple can be a `const` value: `const ORIGIN: (i32, i32) = (1, 2)`, read with `ORIGIN.0` or a destructuring binding.
- A block inside a block can capture the outer block's exclusive capture; the inner call's other arguments are checked against it like a local's.
- `std/iter`: `into_iter(list)` and `cloned(&list)` walk a list, and `map`, `filter`, `take`, `skip`, `enumerate`, `zip`, `chain` are iterator adapters that nest. A generic type whose template declares `fun mut Name<...>.next().return: option.Option<Item>` is an `Iterator` by shape, so it satisfies `I: Iterator` and names `I::Item`.
- `impl<A: Describe, B: Describe> Describe for Pair<A, B>`: a trait can be implemented for a generic type under bounds on its parameters (template methods, associated types over the parameters, e.g. `impl<I: Iterator> Iterator for Twice<I>`), and `where T: A, U: B` is a second spelling of a bound on a function or an impl. Blanket impls and impls for one instantiation are still refused.
- `std/iter` consumers: `any`, `all`, `find`, `last` and `fold`.
