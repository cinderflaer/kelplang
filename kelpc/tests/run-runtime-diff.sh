#!/bin/sh
# The runtime differential: builds every corpus unit with the default backend
# and with the C oracle, runs the hosted ones, and records what each one did.
#
# Stage 1 replaces `kelpc/runtime/kelp_rt.c` in the LLVM backend with a Rust
# `no_std` archive. The C oracle keeps its C copy, so the two runtimes are an
# independent pair. This script is the gate that says whether they still agree:
# run it on a tree, then compare that tree's output with the reference recorded
# on `origin/main` (`--reference`).
#
# What is recorded, per unit and per backend, under `<out>/hosted/<backend>/`
# or `<out>/freestanding/<backend>/`:
#
#   * hosted: the build status and both build streams, then the run status and
#     the run's stdout and stderr. The binary runs with stdin from /dev/null and
#     a 10 second timeout.
#   * freestanding (a project whose `coral.toml` sets `mode = "freestanding"`):
#     the build status and `nm -u` of the image. These carry no runtime, so the
#     records must not change at all.
#
# Paths inside the records are made stable: the repository root is written as
# `<root>` and the output directory as `<out>`, so a run from another checkout
# or another scratch directory produces the same bytes.
#
# Units (scope):
#   * every `.kp` under `kelpc/tests/fixtures`, and `kelpc/tests/*.kp`
#   * every project under `kelpc/tests/projects` and `examples`, built from its
#     directory (its `coral.toml` names the entry)
#   * every `.kp` under `examples` that is not inside a project
#
# Threads: a unit whose output order depends on scheduling is listed by its unit
# name in `kelpc/tests/runtime-diff-sorted.txt`. For those units only, the
# comparison sorts both sides' stdout and stderr lines first.
#
# The run is resumable. A record that already exists is kept, so a long run can
# be split with `--budget`: each invocation stops after about that many seconds
# and exits 3, and the next invocation continues where it stopped.
#
# Usage, from anywhere:
#
#   kelpc/tests/run-runtime-diff.sh [--kelpc <binary>] [--out <dir>]
#                                   [--budget <seconds>] [--reference <dir>]
#
#   --kelpc      the compiler to test. Default: `target/debug/kelpc` under the
#                repository root. Copy it somewhere first when another build
#                may overwrite it while the run is in progress.
#   --out        where records go. Default: `target/runtime-diff`.
#   --budget     stop after about this many seconds (exit 3 when incomplete).
#   --reference  after a complete run, compare the records with this directory
#                and exit 1 on any difference (exit 0 when none).
#
# Exit status: 0 complete and (with --reference) identical; 1 differences
# against the reference; 2 usage or setup error; 3 incomplete (budget spent).
set -u

ROOT=$(cd "$(dirname "$0")/../.." && pwd -P) || exit 2
KELPC="$ROOT/target/debug/kelpc"
OUT="$ROOT/target/runtime-diff"
REF=""
BUDGET=0

usage() {
    echo "usage: $0 [--kelpc <binary>] [--out <dir>] [--budget <seconds>] [--reference <dir>]" >&2
    exit 2
}

while [ $# -gt 0 ]; do
    case "$1" in
        --kelpc) [ $# -ge 2 ] || usage; KELPC=$2; shift 2 ;;
        --out) [ $# -ge 2 ] || usage; OUT=$2; shift 2 ;;
        --budget) [ $# -ge 2 ] || usage; BUDGET=$2; shift 2 ;;
        --reference) [ $# -ge 2 ] || usage; REF=$2; shift 2 ;;
        -h|--help) usage ;;
        *) usage ;;
    esac
done

case "$KELPC" in
    /*) ;;
    *) KELPC="$(pwd)/$KELPC" ;;
esac
if [ ! -x "$KELPC" ]; then
    echo "run-runtime-diff: no executable compiler at $KELPC (use --kelpc)" >&2
    exit 2
fi
case "$OUT" in
    /*) ;;
    *) OUT="$(pwd)/$OUT" ;;
esac
mkdir -p "$OUT/work" || exit 2
START=$(date +%s)

# Every unit, as a path relative to the repository root. Projects are named by
# their directory; single files by the file.
list_units() {
    cd "$ROOT" || exit 2
    find kelpc/tests/fixtures -name '*.kp' | sort
    find kelpc/tests -maxdepth 1 -name '*.kp' | sort
    find kelpc/tests/projects examples -name coral.toml | sed 's#/coral.toml$##' | sort
    # Projects without a manifest are built from their main.kp.
    for d in kelpc/tests/projects/*/; do
        d=${d%/}
        if [ ! -f "$d/coral.toml" ] && [ -f "$d/main.kp" ]; then
            echo "$d/main.kp"
        fi
    done | sort
    find examples -name '*.kp' | while read -r f; do
        dir=$(dirname "$f")
        inproj=0
        while [ "$dir" != "examples" ] && [ "$dir" != "." ]; do
            if [ -f "$dir/coral.toml" ]; then inproj=1; break; fi
            dir=$(dirname "$dir")
        done
        [ "$inproj" -eq 0 ] && echo "$f"
    done | sort
}

# The record name of a unit: its path without the `kelpc/tests/` prefix and the
# `.kp` suffix, with `/` written as `__`.
unit_name() {
    printf '%s\n' "$1" | sed -e 's#^kelpc/tests/##' -e 's#\.kp$##' -e 's#/#__#g'
}

is_freestanding() {
    [ -d "$ROOT/$1" ] && grep -q '^mode *= *"freestanding"' "$ROOT/$1/coral.toml" 2>/dev/null
}

# Build and record one unit with one backend. Returns 0 when a record exists
# afterwards (new or kept).
run_unit() {
    unit=$1
    backend=$2
    name=$(unit_name "$unit")
    if is_freestanding "$unit"; then kind=freestanding; else kind=hosted; fi
    rec="$OUT/$kind/$backend/$name.txt"
    if [ -f "$rec" ]; then
        return 0
    fi
    mkdir -p "$OUT/$kind/$backend" "$OUT/bin/$backend" || return 1
    bin="$OUT/bin/$backend/$name"
    work="$OUT/work/$backend.$name"
    rm -f "$bin"
    tmp="$rec.tmp"

    if [ "$backend" = "c" ]; then
        (cd "$ROOT" && env -u KELPC_BACKEND KELPC_ORACLE=c timeout 300 "$KELPC" build "$unit" -o "$bin" >"$work.out" 2>"$work.err" </dev/null)
    else
        (cd "$ROOT" && env -u KELPC_BACKEND -u KELPC_ORACLE timeout 300 "$KELPC" build "$unit" -o "$bin" >"$work.out" 2>"$work.err" </dev/null)
    fi
    bstatus=$?

    {
        echo "unit: $unit"
        echo "backend: $backend"
        echo "build-status: $bstatus"
        echo "--- build stdout"
        sed -e "s#$ROOT#<root>#g" -e "s#$OUT#<out>#g" "$work.out"
        echo "--- build stderr"
        sed -e "s#$ROOT#<root>#g" -e "s#$OUT#<out>#g" "$work.err"
    } >"$tmp"

    if [ "$kind" = "freestanding" ]; then
        echo "--- nm -u" >>"$tmp"
        if [ "$bstatus" -eq 0 ] && [ -f "$bin" ]; then
            nm -u "$bin" 2>&1 | sort >>"$tmp"
        else
            echo "(no image)" >>"$tmp"
        fi
    elif [ "$bstatus" -eq 0 ] && [ -f "$bin" ]; then
        (cd "$OUT/work" && timeout 10 "$bin" </dev/null >"$work.run.out" 2>"$work.run.err")
        rstatus=$?
        {
            echo "run-status: $rstatus"
            echo "--- stdout"
            cat "$work.run.out"
            echo "--- stderr"
            sed -e "s#$ROOT#<root>#g" -e "s#$OUT#<out>#g" "$work.run.err"
        } >>"$tmp"
        rm -f "$work.run.out" "$work.run.err"
    else
        echo "run-status: not run" >>"$tmp"
    fi

    mv "$tmp" "$rec"
    rm -f "$bin" "$work.out" "$work.err"
    return 0
}

# The run itself.
total=0
done_count=0
incomplete=0
list_units >"$OUT/units.txt"
while read -r unit; do
    for backend in c llvm; do
        total=$((total + 1))
        if [ "$BUDGET" -gt 0 ] && [ $(( $(date +%s) - START )) -ge "$BUDGET" ]; then
            incomplete=1
            continue
        fi
        if run_unit "$unit" "$backend"; then
            done_count=$((done_count + 1))
        fi
    done
done <"$OUT/units.txt"

# A record is complete when its file exists. Count what is there, not what this
# invocation did, so a resumed run reports the whole picture.
have=$(find "$OUT/hosted" "$OUT/freestanding" -name '*.txt' 2>/dev/null | wc -l)
want=$(( $(wc -l <"$OUT/units.txt") * 2 ))

if [ "$incomplete" -eq 1 ] || [ "$have" -lt "$want" ]; then
    echo "run-runtime-diff: incomplete, $have of $want records under $OUT"
    exit 3
fi

# Summary: one line per backend and kind, with build and run outcomes.
{
    for kind in hosted freestanding; do
        for backend in c llvm; do
            dir="$OUT/$kind/$backend"
            [ -d "$dir" ] || continue
            ok=$(grep -l '^build-status: 0$' "$dir"/*.txt 2>/dev/null | wc -l)
            failed=$(grep -L '^build-status: 0$' "$dir"/*.txt 2>/dev/null | wc -l)
            echo "$kind $backend: records=$(ls "$dir" | wc -l) built=$ok build-failed=$failed"
        done
    done
} >"$OUT/summary.txt"
cat "$OUT/summary.txt"
rm -rf "$OUT/bin"
mkdir -p "$OUT/work"

if [ -z "$REF" ]; then
    exit 0
fi

# Compare with the reference. A unit listed in runtime-diff-sorted.txt compares
# sorted lines, so scheduling order cannot fail it.
SORTED="$ROOT/kelpc/tests/runtime-diff-sorted.txt"
differences=0
cd "$REF" || { echo "run-runtime-diff: no reference at $REF" >&2; exit 2; }
for rel in $(find hosted freestanding -name '*.txt' 2>/dev/null | sort); do
    if [ ! -f "$OUT/$rel" ]; then
        echo "missing: $rel"
        differences=$((differences + 1))
        continue
    fi
    if cmp -s "$REF/$rel" "$OUT/$rel"; then
        continue
    fi
    name=$(basename "$rel" .txt)
    if [ -f "$SORTED" ] && grep -qx "$name" "$SORTED"; then
        sort "$REF/$rel" >"$OUT/work/ref.sorted"
        sort "$OUT/$rel" >"$OUT/work/out.sorted"
        if cmp -s "$OUT/work/ref.sorted" "$OUT/work/out.sorted"; then
            continue
        fi
    fi
    echo "differs: $rel"
    differences=$((differences + 1))
done
for rel in $(cd "$OUT" && find hosted freestanding -name '*.txt' 2>/dev/null | sort); do
    [ -f "$REF/$rel" ] || { echo "extra: $rel"; differences=$((differences + 1)); }
done

if [ "$differences" -ne 0 ]; then
    echo "run-runtime-diff: $differences difference(s) from the reference"
    exit 1
fi
echo "run-runtime-diff: identical to the reference"
exit 0
