# Pending changes

Scratch file for changes that are not released yet. Gitignored, never committed.

Add a line here when a user-visible change lands in `coral`. **Do not touch
`wiki/tooling/coral-changelog.md` until a version is tagged**: that page carries
releases only.

When a version drops, write its entry in `wiki/tooling/coral-changelog.md` from
this file plus the tag's commit and the Gitea release body, then wipe this file
back to this header. No need to ask first.

Keep each line short: what changed in the manifest, which command appeared or
went away, what the toolchain store does now.

- The Kelp port's manifest parser no longer cuts a `#folder` dependency fragment off as a comment: `#` starts a comment only outside double quotes, as in the Rust parser.
