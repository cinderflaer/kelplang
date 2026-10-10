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
- `[target] mode = "managed"` is accepted by the manifest, with `heap_max = N` (a whole number of MiB, default 256) for the managed heap cap. Hosted and freestanding are unchanged. The memory or resource classifier is in `kelpc/src/managed.rs`; the heap refusal is not yet wired into the checker, and a managed build still takes the hosted path.

- Managed-mode lowering (WP3, WIP): GC descriptors, typed Box allocation, drop gating; not yet compiled or verified.
- Managed mode: the checker now refuses a resource (a `Thread`, `Mutex`, `datac` holding one) in a `Box`, `List`, `Map`, array or closure environment, by name. The heap-slot hook runs after the pass; hosted builds are unchanged.
- Managed mode: GC type descriptors for `Map` headers and entries, closure environments and closure values are defined in `kelpc/src/gc_descr.rs`. Nothing emits them yet; the managed lowering still allocates through the stub path.
- Managed builds (`-DKELP_GC_MANAGED`) allocate String and Bytes buffers from the collector and free nothing; hosted runtime output is byte-identical.
