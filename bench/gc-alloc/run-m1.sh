#!/bin/sh
# M1 (design section 13): builds the base and s1 collector and their gc-alloc
# bench binaries, then times them with run-m1.py. Run as a build queue job. The
# job runs in a mirror of the worktree that has no .git, so the job does not use
# git. The base source is prepared before the job, outside the worktree, from
# the base commit:
#
#   mkdir -p <scratch>/base && git archive bf655a60 kelpc/gc | tar -x -C <scratch>/base
#   buildq submit -p 1 --exclusive --cwd <worktree> --name "tidepool m1" -- \
#     nix-shell shell.nix --run 'sh bench/gc-alloc/run-m1.sh <scratch>/base <s1 tree>'
#
# $1 is the scratch directory holding kelpc/gc of the base commit. $2 is the
# tree whose kelpc/gc is the s1 arm (the default is the worktree itself). Both
# bench binaries are built in the scratch directories, so the trees get no new
# build output. The bench sources always come from the worktree. The exit status
# is the status of run-m1.py (see its header), 2 for usage or build errors.
set -u
if [ $# -lt 1 ] || [ $# -gt 2 ]; then
    echo "usage: sh bench/gc-alloc/run-m1.sh BASE_SCRATCH_DIR [S1_TREE]" >&2
    exit 2
fi
unset CARGO_TARGET_DIR KELP_GC_VERIFY KELP_GC_STRESS KELP_GC_STATS
ROOT=$(pwd)
B=$1
S1=${2:-$ROOT}
[ -f "$B/kelpc/gc/Cargo.toml" ] || { echo "no base kelpc/gc under $B"; exit 2; }
[ -f "$S1/kelpc/gc/Cargo.toml" ] || { echo "no s1 kelpc/gc under $S1"; exit 2; }
mkdir -p "$B/bench" "$B/s1/bench" "$B/s1/kelpc/gc/target/release" || exit 2
rm -rf "$B/bench/gc-alloc" "$B/s1/bench/gc-alloc"
cp -r "$ROOT/bench/gc-alloc" "$B/bench/" || exit 2
cp -r "$ROOT/bench/gc-alloc" "$B/s1/bench/" || exit 2
for d in "$B/kelpc/gc" "$S1/kelpc/gc"; do
    echo "== build $d"
    (cd "$d" && cargo build --release 2>&1 | tail -3)
    [ -e "$d/target/release/libkelp_gc.a" ] || { echo "missing runtime build in $d"; exit 2; }
done
cp "$S1/kelpc/gc/target/release/libkelp_gc.a" "$B/s1/kelpc/gc/target/release/" || exit 2
for d in "$B/bench/gc-alloc" "$B/s1/bench/gc-alloc"; do
    echo "== build $d"
    (cd "$d" && cargo build --release 2>&1 | tail -3)
    [ -x "$d/target/release/gc-alloc-bench" ] || { echo "missing bench build in $d"; exit 2; }
done
BASE_BIN="$B/bench/gc-alloc/target/release/gc-alloc-bench"
S1_BIN="$B/s1/bench/gc-alloc/target/release/gc-alloc-bench"
for b in "$BASE_BIN" "$S1_BIN"; do
    n=$(nm "$b" | grep -c ' kelp_gc_')
    echo "kelp_gc_ symbols in $b: $n"
    [ "$n" -gt 0 ] || { echo "no kelp_gc_ symbols"; exit 2; }
done
python3 bench/gc-alloc/run-m1.py "$BASE_BIN" "$S1_BIN"
