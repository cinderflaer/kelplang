#!/bin/sh
# Builds the release binary twice from the committed HEAD, in two source
# directories with different absolute paths (and two target directories), and
# compares the SHA-256 of the results. Exit 0 only when they are identical.
#
#   build/verify-reproducible.sh [work-dir]
#
# The work directory (default: a fresh mktemp -d) must not exist yet or must be
# empty; it is removed afterwards unless KEEP=1. Uncommitted changes are NOT
# part of the check: it builds `git archive HEAD`, which is what a release is.
# Memory: two full release builds, one after the other, never in parallel.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/.." && pwd)
work=${1:-$(mktemp -d)}
mkdir -p "$work"
commit=$(cd "$root" && git rev-parse HEAD)
epoch=$(cd "$root" && git log -1 --format=%ct)
export KELPC_COMMIT="$commit" RELEASE_SOURCE_DATE_EPOCH="$epoch"

hashes=""
for name in alpha beta-longer-path; do
    src="$work/$name/source-tree"
    mkdir -p "$src"
    (cd "$root" && git archive HEAD) | tar -x -C "$src"
    echo "verify: building in $src" >&2
    CARGO_TARGET_DIR="$work/$name/target" DIST_DIR="$work/$name/dist" \
        sh "$src/build/release.sh"
    h=$(cut -d' ' -f1 "$work/$name/dist/SHA256SUMS")
    echo "verify: $name $h" >&2
    hashes="$hashes $h"
done

set -- $hashes
status=0
if [ "$1" = "$2" ]; then
    echo "IDENTICAL: $1"
else
    echo "DIFFERENT: $1 vs $2" >&2
    status=1
fi
if [ "${KEEP:-0}" != 1 ]; then rm -rf "$work"; fi
exit $status
