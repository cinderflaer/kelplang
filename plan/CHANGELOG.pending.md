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

## compiler
- Managed-mode lowering (WP3, WIP): GC descriptors, typed Box allocation, drop gating, safepoint polls at calls and loop back edges; golden IR and hosted identity pass, link smoke blocked until the collector defines rust_eh_personality.
- `[target] mode = "managed"` is accepted by the manifest, with `heap_max = N` (a whole number of MiB, default 256) for the managed heap cap. Hosted and freestanding are unchanged. The memory or resource classifier is in `kelpc/src/managed.rs`; the heap refusal is not yet wired into the checker.
- A managed build (`[target] mode = "managed"`) is lowered through the managed path: `kelp_gc_init` gets the 8 MiB minimum and the manifest `heap_max` as its maximum, and the collector archive named by `KELP_GC_LIB` is linked in. Hosted and freestanding builds are unchanged. The link still fails until the collector defines `rust_eh_personality`, so a managed program does not run yet.
