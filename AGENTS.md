# Kelp: contributor and agent guide

This is a general version of the working rules for this repository. It names no
machine, account, host or credential. Where a rule needs one (a remote, a token),
it says so in words and leaves the value to your own setup.

## Hard rules

1. **Delegate with exact scope.** A helper (a subagent or a second person) gets
   exact paths, exact interfaces and acceptance criteria, never the whole
   conversation or the whole tree.
2. **Never delete work you did not make.** Files changed by another session or
   person are off limits: no revert, no restore, no stash, no reset, no clean, no
   rewrite. If keeping them blocks your task, stop and ask.
3. **Credentials stay out of the repository.** Read a token from the environment
   or a private file with restrictive permissions. Never print it, commit it, or
   work around a missing one (extracting it from a service, a database or a
   browser). Missing or rejected: say so and ask for a new one.
4. **Stage by path.** `git add` the paths you touched, never `-A`, so a commit
   does not carry another session's in-flight edit.

## What Kelp is

Kelp is a statically typed, compiled systems language whose headline feature is
capability-based security: a module declares the resources it may touch, and that
authority is checked at every call site. Small language surface, ownership without
a GC, `Option`/`Result`, `datac`/`enum` with exhaustive `when`, compiler builtins
for `Bytes`, `String`, `List`, `Map`, `Box`. No `unsafe` in safe code, no exceptions.

## Layout of this repository

| Path | What it is |
| --- | --- |
| `kelpc/` | The compiler, `kelpc`, in Rust: lexer, preprocessor, parser, `infer.rs`, `borrowck.rs`, the backends, `driver.rs`. `kelpc/std/` ships inside the binary. |
| `kelpc/tests/` | The fixture suite and the other test targets. |
| `plan/` | Unfinished plans and the project's working records: `ISSUES.md`, `CHANGELOG.pending.md`, and the plans still in progress. The language and design docs live in the kelp wiki. |

Related projects live in their own repositories: the wiki, the `fpl` library
collection, `coral` (build tool and package manager) and `lighthouse` (editor
tooling).

## Authorities

- The kelp wiki is the language and design authority: `language/*.md` for the
  language, `compiler/*.md` for the compiler, `meta/roadmap.md` for the open
  items, the marks and the backlog, and `meta/milestones.md` for the shipped
  record. `plan/syntax.md` and `plan/architecture.md` stay in `plan/` until
  their rules are checked against the wiki, and they are not the authority
  for anything the wiki already states.
- `plan/ISSUES.md` records what is open. It is not a plan and does not outrank
  the wiki.
- A README claim is not authority over the wiki or `plan/`.

## Build and test

Check `README.md` for the current commands. The compiler needs LLVM development
files (`llvm-sys`); the repository ships a `shell.nix` that provides them, so on
Nix run `nix-shell shell.nix --run 'cargo test --no-fail-fast'`. Elsewhere, point
`LLVM_SYS_211_PREFIX` at an LLVM 21 install.

## Parallel lanes: one worktree per task

More than one session may work in one checkout. A task that runs beside another
gets its own git worktree and branch, and the shared checkout is only where a lane
lands.

- Branch name: `<area>-<short-description>`, for example `syntax-match-guards`.
- Base and landing branch: `main` for this repository.
- The base is a commit, not a working tree. A lane contains the committed tip
  only; never commit someone else's uncommitted work to make a base carry it.
- One writer per worktree.
- Every worktree has its own `target/`; do not share one `CARGO_TARGET_DIR`.
- When a lane lands, merge it, then immediately remove the worktree and delete the
  branch. Other lanes rebase onto the new tip.

### Where two lanes still collide

- `kelpc/src/ast.rs`, `tir.rs`, `types.rs`, `parser.rs`, `infer.rs`, `borrowck.rs`
  are language shape: one lane at a time when a variant, field or type is added.
- `kelpc/src/stdlib.rs` and `resolve.rs` hold the capability and gate tables read
  by every backend; a change moves everyone's measurements.
- `kelpc/tests/compile_and_run.rs` holds the whole fixture suite, so two lanes
  adding tests nearby conflict textually.
- rustc's exhaustiveness check is the safety net. A `match` that names every
  variant and has no wildcard stops building when another lane adds a variant;
  fill in the arm. Do not add a catch-all, `todo!()` or `unreachable!()` to make it
  build.

## Finishing a lane: merge it yourself

- A lane is finished when its work is on the landing branch (`main`). Merge it in
  the same session, once its tests pass. Do not leave lanes unmerged for a later
  cleanup: unmerged lanes conflict with each other once main has moved on.
- Rebase or merge onto the current landing branch, resolve conflicts keeping the
  intent of both sides, run the suite, then push the landing branch as a
  fast-forward. If the push is rejected, fetch and repeat.
- If the suite fails: fix it in the lane. If the failure is known and recorded
  (for example a documented platform gap), name the failing tests in the merge
  commit body and in your report. Do not land a lane with unexplained failures
  without asking.
- Every lane closes into a landing branch: `main` for kelp, coral, lighthouse and polyp; `unreleased` for
  kelp-wiki and fpl. No lane may leave a branch behind on origin. When a lane has landed, delete its own
  branch. Do not open a new long-lived branch for work that can merge the same day.
- Never force-push. Never delete a remote branch without the owner's approval.
  After a lane lands, remove its worktree and its local branch.

## Builds and tests

- Run heavy builds and tests through the shared build queue when one is set up,
  not directly on the machine you are working on. Normal work uses the default
  priority; the queue's setup notes say how.
- Run builds and tests inside the repository's nix shell (`nix-shell shell.nix
  --run '...'`) on any machine where the system linker is not on the path.
- Tidepool work (any job whose name or working directory mentions tidepool) runs
  at the highest queue priority, always.
- Do not share one `CARGO_TARGET_DIR` between worktrees.

## Changelog and commits

- Every user-visible change gets one line in `plan/CHANGELOG.pending.md`, however
  small the change.
- Commit with the owner's configured git identity. One sentence in the message,
  no trailers, staged by path.

## Machine setup

Machine names, hosts, queue locations and token file locations are not kept in
this repository. They live in the owner's own setup notes, which every session
reads before it builds or pushes.

## Version scheme

Every piece of Kelp software is numbered `YY.MM.N`: two-digit year, two-digit
month, release number within that month starting at `0`. The padded name is the
release name (tag `v26.09.0`, `kelpc version` prints `kelpc 26.09.0`). Cargo
cannot carry a leading zero, so `Cargo.toml` holds `26.9.0` and every human-facing
spelling pads the month back. Never renumber a released version.

## Changelogs

- **Pending**: `plan/CHANGELOG.pending.md`, tracked and always editable. Append a
  short factual line in the same change that makes the user-visible difference.
- **Published**: the wiki's changelog page, written at release only, from the
  pending file plus the tag's commits and release notes. Then wipe the pending file
  back to its header.
- Keep entries short. Do not invent an entry from a commit subject alone.
- Do not use em dashes or en dashes anywhere. Use a colon, comma or semicolon, and a
  plain hyphen in a range.

## Documentation follows the code

Anything a reader of the docs would notice (a construct, a rule, a diagnostic
message, a stdlib name, a limit, a count) is updated in every page that carries it,
not only where it was announced. Search the docs for wording the change makes false
("there is no X", a stale count). Documentation for unreleased behaviour lives on an
`unreleased` branch of the documentation repository and reaches its `main` at
release, so the published docs describe only released versions.

## Issues and findings

- A bug or missing feature goes in `plan/ISSUES.md` in the session you find it: one
  entry with the repository it belongs to, the date and a source (a repro program,
  or the file and section that states it).
- The heading id is `BUG-` or `MISSING-` plus five mixed-case alphanumerics, for
  example `MISSING-djHo0`. It is opaque, not a sequence, new each time, and never
  reused.
- Every `MISSING-` entry carries `effort: S|M|L|XL`, and that section is sorted
  easiest first. `S` is a stdlib function or one small diagnostic, `M` one
  subsystem in one lane, `L` touches a hotspot file or needs a design call, `XL` is
  mark-sized or blocked on a decision. If a finding changes an entry's size, move it
  to the right tier; an `S` that needs a design call is re-tiered, not hacked.
- `ISSUES.md` holds open items only. Resolving one deletes it and adds a line to
  the pending changelog.
- Do not publish defect details (a live bug with a repro) on a public page.

## Picking work

With no task named, take the top `S` entries, one lane each, then work down. Check
an entry's own text for a dependency first. An `XL` entry, or one marked
decided-to-wait, is not started without the maintainer.
