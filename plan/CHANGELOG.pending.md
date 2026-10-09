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

## Compiler

- `for x in xs.iter()` and `for (k, v) in m.iter()` run the same walk as `for x in xs` and `for (k, v) in m`; no iterator value is built, and the body may not mutate the walked list.

## Added

- `sync.rendezvous<T>()` makes a zero-capacity channel: a send completes only once a receive takes the value. `sync.bounded(0)` still panics.
- `sync.select2` and `sync.select3` wait on receive cases over one element type and return `Pick.Got(index, value)` or `Pick.Closed(index)`; a closed, empty channel counts as ready.

## Standard library

- `str.to_upper` and `str.to_lower` apply Unicode 16.0.0 simple (one-to-one) case mappings to every codepoint; multi-character mappings such as U+00DF are not applied.
- `str.slice(s, from, to)` takes byte offsets, which must start a codepoint or be the end of the string; an offset inside a codepoint panics.
- `str.index_of` and `str.replace` measure the haystack once; a search over a 4000-piece haystack runs about 85 times faster.
- `path.split` slices each component from the view it already holds; a 20000-component split runs about 20 times faster.
- `base64` and `base32` map each character by range instead of a `str.byte` lookup; about 2x and 1.7x faster on 2 MB buffers.
- `str.from_f64` (and so `fmt.float`) formats once into a stack buffer when the text fits in 128 bytes; about 2x faster on 400000 two-decimal values.
