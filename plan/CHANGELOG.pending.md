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

## Added

- `[build] emit = "html"` writes a Polyp view as a static HTML file. `[build] emit_entry` names the program that builds it. A Polyp HTML build needs no env grant.

## Changed

- The Polyp examples use the Polyp v5 impl ids (`deps/polyp/core/core`, `deps/polyp/backends/gtk/polyp/lib`, `deps/polyp/backends/gtk/gtk`, `deps/polyp/backends/gtk/queue`, `deps/polyp/backends/soft/...`). The `polyp` namespace is unchanged. The v0 examples are frozen.

## compiler

- The corpus has golden outputs frozen from the C oracle at v26.10.5 (`kelpc/tests/golden/`), and the `llvm` backend is checked at five optimization levels against `-O0` with no C in the comparison.

- `kelpc doc --format json` writes the documentation as JSON (on branch lighthouse-support, not merged).
- `kelpc doc --text` prints the documentation as plain text (on branch lighthouse-support, not merged).
- `kelpc` builtin listing is available as JSON (on branch lighthouse-support, not merged).

## stdlib

- `kelpc stdlib --json` lists the standard library as JSON (on branch lighthouse-support, not merged).

## fixes

- The roadmap no longer carries a release schedule, and ISSUES no longer has release slots (landed on main, c87fb51).
- The roadmap says channels and Mutex ship now, and the download row says when it was true (landed on main, be51d2e).
- The KelpScript proposal moves to `plan/kelpscript.md` (on branch kelpscript-plan, not merged).

## limits

- Managed lowering is a WIP commit, not compiled or verified (on branch gc-lowering, not merged).
- A-11 borrow checking is a WIP commit, not finished (on branch a11-checker, not merged).

## Wiki and docs

- Homepage: the current release line is 26.10.5 (fbdb951, merged into main by 02f7db4).
- Coral changelog: a 26.10.5 entry and a table row (9ec46bc, merged into main by c90a24b).
- Polyp changelog: a new page at `libraries/polyp-changelog.md` (9ec46bc, merged into main by c90a24b).
- Owner name changed from Juniper to Lilly (d813b32 on unreleased, merged into main by 82ec21c).
- Finished wiki lane branches deleted on the remote and locally; origin keeps only main and unreleased.

## Plans not in the repo

- `/home/juniper/Projects/kelp-work/stage1-runtime-plan.md`
- `/home/juniper/Projects/kelp-work/stage2-oracle-plan.md`
- `/home/juniper/Projects/kelp-work/stage3-lld-plan.md`
- `/home/juniper/Projects/kelp-work/kelpscript-gc-paper.md`
- `/home/juniper/Projects/kelp-work/ckp-plan.md`

## Decisions

- C is removed from the compiler in three stages (see the stage plans above).
- Every change, however small, gets one line in its repo's pending changelog.
