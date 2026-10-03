# Reproducible release builds, `kelpc version --verbose`, and the installer

## What exists

- `build/release.sh`: builds `dist/kelpc-<version>-linux-x86_64`, `dist/SHA256SUMS` and
  `dist/BUILDINFO` (version, commit, rustc/cargo version, LLVM version and major, the exact flags).
- `build/verify-reproducible.sh [workdir]`: `git archive HEAD` into two directories with different
  absolute paths (`alpha/source-tree`, `beta-longer-path/source-tree`, each with its own target dir),
  runs `release.sh` in each, one after the other, and compares SHA-256. Exit 0 only if identical.
  Uncommitted changes are not part of the check (commit first). `KEEP=1` keeps the work dir.
- `build/install.sh`: POSIX installer, no downloads, no sudo, verification mandatory (see its header).
- `kelpc version [--verbose]`: the verbose form adds `commit`, `build date`, `rustc`, `llvm`, baked
  in by `kelpc/build.rs` (`KELPC_COMMIT` env else `git rev-parse HEAD`; date from `SOURCE_DATE_EPOCH`
  rendered as a UTC date; `rustc -V`; `llvm-config --version`). No wall clock anywhere.
- Tests: `kelpc/tests/release_tooling.rs`.

## What is pinned

`CARGO_INCREMENTAL=0`; `--locked` (plus `--frozen` when the cargo registry cache exists);
`CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1`; `CARGO_PROFILE_RELEASE_STRIP=debuginfo`;
`RUSTFLAGS=--remap-path-prefix=<src>=/kelp-src --remap-path-prefix=<cargo home>=/cargo-home
--remap-path-prefix=<rust sysroot>=/rust-sysroot -C link-arg=-Wl,--build-id=none`;
`SOURCE_DATE_EPOCH` = last commit time (the nix shell exports a fixed 1980 value, so the script
ignores an ambient one and uses `RELEASE_SOURCE_DATE_EPOCH` or git). The C runtime object compiled in
`build.rs` gets `-ffile-prefix-map` for the manifest dir and `OUT_DIR`.
`codegen-units=1` was set up front rather than found necessary; it was not tested without.

## Result (2026-10-03, rustc 1.95.0, LLVM 21.1.8, nix shell dev.nix)

Two builds of commit `74cf4af2ae920d7cb04da4c716a0be8d19c14fa0` in two directories with different
path lengths and different target dirs: IDENTICAL, both
`beb7f8b7b3a3cb77a9775f4a7cce651fa8ac1772bccba3035e938fcd2531d722`
(`kelpc-26.09.5-linux-x86_64`). An earlier attempt differed only because the nix shell's
`SOURCE_DATE_EPOCH` leaked into one run's baked build date; fixed in `release.sh`.

## What is not controlled

The binary contains no checkout, cargo-home or sysroot path, but it does embed the toolchain's nix
store paths: the dynamic loader/RPATH entries for glibc, gcc-lib and `libLLVM-21` and a
`...-nix-shell` entry from the shell's link flags. Reproducibility therefore holds for the same
toolchain (same nix store paths), not across different nixpkgs revisions. A binary meant for
non-Nix users should be built in an environment where those are the distribution's `/lib` paths
(for example a Debian container with `llvm-21-dev`); the same scripts apply.
