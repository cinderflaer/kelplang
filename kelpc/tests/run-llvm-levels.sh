#!/bin/sh
# The optimization-level differential of the llvm backend (stage 2, WP-2).
#
# Builds every corpus target at -O0 and at -O1, -O2, -O3 and -Os with the llvm
# backend only, and requires the same exit status, stdout and first stderr line
# at every level as at -O0. No C backend is involved; the link still uses cc.
# The sweep is the ignored test in kelpc/tests/level_differential.rs. It runs
# five builds and five runs per target, so run it from a detached queue on a
# small host and not from an interactive shell.
#
# Usage, from the repository root:
#   ./kelpc/tests/run-llvm-levels.sh              the whole corpus
#   LEVEL_FILTER=fixture_int_ ./kelpc/tests/run-llvm-levels.sh   a substring of the tag
#
# Run it through the build lock and the shell the repository uses:
#   flock .build.lock nix-shell shell.nix --run './kelpc/tests/run-llvm-levels.sh'
set -e

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
cd "$ROOT"

cargo test --release -j 2 -p kelpc --test level_differential \
  -- --ignored --nocapture every_target_agrees_at_every_level
