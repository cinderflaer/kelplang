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

- Managed GC core hardening: attach check, stop timeout naming the stuck thread, type-id and enum-tag limits, panic hook, heap verifier, stress mode, cyclic-graph live-set test (`kelpc/gc`).
- Managed GC core: measurement tests for large-object waste and heap-lock contention (`kelpc/gc`).
- Managed-mode GC core (`kelpc/gc`, Rust staticlib `libkelp_gc.a`): C ABI header and descriptor layout for WP3; not yet linked by the compiler.
- Managed GC core: mark-region allocator, mark and sweep, safepoints, native state and stack scan (`kelpc/gc`).
- Managed GC core tests: header layout, type-id round trip, 8-byte alignment, 10M-cell churn, cyclic mark, 256 MiB cap, and stop-the-world with threads.
