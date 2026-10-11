#!/usr/bin/env bash
# The MK7 S6 measurement: what each optimization level costs to compile and to
# run, on `speed.kp`'s four workloads.
#
# `run.sh` compares four languages at one level. This compares the LLVM backend
# across the levels `-O` accepts (MK7 section 4.3), which is the table the S6
# decision is written from.
#
# Method:
#
#   * Rounds are the outer loop and cells are inner, so machine drift (a
#     background job, thermal state) spreads across every cell rather than landing
#     on the last one. The reported number is the *median* of the rounds, not one
#     run.
#   * A cell is (level, backend): one `kelpc build` (timed, including the `cc`
#     link and, for `llvm`, the pass pipeline and object write), then one run of
#     the built binary, which times its own four workloads with a monotonic clock.
#   * Checksums must agree across every round of a cell, and across every cell.
#     Equal checksums are what make the times comparable: they prove every build
#     did the same work.
#
# Usage, on an idle machine, from anywhere:
#
#   ./bench/speed/optlevels.sh                 # 9 rounds
#   BENCH_ROUNDS=15 ./bench/speed/optlevels.sh
#
# Writes `bench/speed/results-optlevels.txt` and a TSV under
# `bench/speed/build/optlevels/`.
set -u

DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$DIR/../.."
OUT="$DIR/build/optlevels"
REPORT="$DIR/results-optlevels.txt"
ROUNDS="${BENCH_ROUNDS:-9}"
LEVELS="${BENCH_LEVELS:--O0 -O1 -O2 -O3 -Os}"
BACKENDS="${BENCH_BACKENDS:-llvm}"

# The retired `KELPC_BACKEND` and `KELPC_ORACLE` are stripped, so a stale export
# cannot refuse a build.
build_backend() {
    env -u KELPC_BACKEND -u KELPC_ORACLE "$KELPC" build "$DIR/speed.kp" "$level" -o "$bin"
}

mkdir -p "$OUT"
rm -f "$OUT"/*.tsv "$OUT"/*.out "$OUT"/*.log

now() { date +%s%N; }

echo "building kelpc (not counted)..." >&2
if [ -n "${KELPC:-}" ]; then
    # A prebuilt binary, so the measurement can run where LLVM's build half is
    # not present (outside `nix-shell`). The C compiler used at link time is
    # whatever `cc` is on that PATH, and the report says which it was.
    echo "using KELPC=$KELPC" >&2
else
    ( cd "$ROOT" && cargo build -q --release -p kelpc ) >/dev/null 2>&1 || {
        echo "kelpc did not build" >&2
        exit 1
    }
    KELPC="$ROOT/target/release/kelpc"
fi
CC_USED="$(command -v cc || echo 'cc (not found)')"
CC_VERSION="$(cc --version 2>/dev/null | head -1 || echo unknown)"

: > "$OUT/cells.tsv"

for round in $(seq 1 "$ROUNDS"); do
    # The cell order is the same every round; the rounds are what interleave.
    for level in $LEVELS; do
        for backend in $BACKENDS; do
            bin="$OUT/speed_${backend}${level}"
            c0=$(now)
            if ! build_backend \
                >"$OUT/${backend}${level}.build.log" 2>&1; then
                echo "build FAILED: $backend $level" >&2
                tail -5 "$OUT/${backend}${level}.build.log" >&2
                exit 1
            fi
            c1=$(now)

            if ! "$bin" >"$OUT/${backend}${level}.out" 2>&1; then
                echo "run FAILED: $backend $level" >&2
                tail -5 "$OUT/${backend}${level}.out" >&2
                exit 1
            fi

            # C is one compile row; R is one row per workload. Keeping them
            # apart means the aggregator reads the same field for a time either
            # way and never has to know the program's output layout.
            echo "C $round $level $backend $((c1 - c0))" >> "$OUT/cells.tsv"
            while read -r _ workload ops ns checksum; do
                echo "R $round $level $backend $workload $ops $ns $checksum" >> "$OUT/cells.tsv"
            done < <(grep '^RESULT ' "$OUT/${backend}${level}.out")
        done
    done
    echo "round $round/$ROUNDS done" >&2
done

awk '
function median(a, n,   i, j, t) {
    for (i = 2; i <= n; i++) {
        t = a[i]
        for (j = i - 1; j >= 1 && a[j] > t; j--) a[j + 1] = a[j]
        a[j + 1] = t
    }
    if (n % 2) return a[(n + 1) / 2]
    return (a[n / 2] + a[n / 2 + 1]) / 2
}
function cellmedian(key,   i, n, tmp) {
    n = count[key]
    for (i in tmp) delete tmp[i]
    for (i = 1; i <= n; i++) tmp[i] = val[key, i]
    return median(tmp, n)
}
function note_cell(cellkey) {
    if (!(cellkey in seen)) { seen[cellkey] = 1; order[++ncells] = cellkey }
}
$1 == "C" {
    cellkey = $3 " " $4
    note_cell(cellkey)
    key = cellkey " build_ns"
    val[key, ++count[key]] = $5
    next
}
$1 == "R" {
    cellkey = $3 " " $4
    note_cell(cellkey)
    key = cellkey " " $5
    val[key, ++count[key]] = $7
    ops[key] = $6
    cksum[key] = $8
    # Every round of one cell must produce one checksum, and every cell must
    # agree with the first cell seen *for that workload*: a difference is two
    # builds doing different work, which makes the times incomparable.
    if (!($5 in firstck)) firstck[$5] = $8
    if ($8 != firstck[$5]) bad = 1
    next
}
END {
    printf "%-5s %-5s %10s %10s %10s %10s %10s\n", "level", "back", "build_ms", "collatz", "float", "mix", "fib"
    printf "%s\n", "---------------------------------------------------------------------"
    for (c = 1; c <= ncells; c++) {
        split(order[c], k, " ")
        cellkey = order[c]
        printf "%-5s %-5s %10.1f %10.1f %10.1f %10.1f %10.1f%s\n",
            k[1], k[2],
            cellmedian(cellkey " build_ns") / 1e6,
            cellmedian(cellkey " collatz") / 1e6,
            cellmedian(cellkey " float") / 1e6,
            cellmedian(cellkey " mix") / 1e6,
            cellmedian(cellkey " fib") / 1e6,
            (bad ? "   CHECKSUM MISMATCH" : "")
    }
    printf "\nchecksums (all levels and backends must agree):\n"
    for (c = 1; c <= ncells; c++) {
        split(order[c], k, " ")
        printf "  %-5s %-5s collatz=%s float=%s mix=%s fib=%s\n",
            k[1], k[2],
            cksum[order[c] " collatz"], cksum[order[c] " float"],
            cksum[order[c] " mix"], cksum[order[c] " fib"]
    }
    printf "%s\n", (bad ? "MISMATCH: the checksums are not all equal" : "all checksums agree")
}
' "$OUT/cells.tsv" | tee "$REPORT"

echo >> "$REPORT"
echo "method: medians of $ROUNDS interleaved rounds; one kelpc build and one run per round and cell." >> "$REPORT"
echo "built by $KELPC on $(uname -n) at $(date -u +%Y-%m-%dT%H:%M:%SZ)." >> "$REPORT"
echo "the C backend's compiler and both backends' linker: $CC_VERSION ($CC_USED)" >> "$REPORT"
