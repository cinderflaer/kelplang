#!/bin/sh
# M4 and M9 (design section 13). Run as a build queue job, with core dumps off:
#
#   buildq submit -p 1 --cwd <worktree> --name "tidepool M4 M9" -- \
#     prlimit --core=0:0 -- sh bench/gc-clear/run.sh
#
# Builds the collector's test binary (release; KELP_GC_VERIFY and KELP_GC_STRESS
# unset), then RUNS times:
#   M4: one process per heap_max (256 and 1024 MiB): 1000 forced clearings with a
#       1 MiB live set. Median pause per clearing, and the ratio 1024 / 256 against
#       the section 20.2 line (at most 1.5).
#   M9: 256 attached threads, one object each, then unrooted 4 KiB garbage until
#       the automatic trigger clears. Median of unused tail plus overshoot against
#       8 MiB, and the first clearing leaves no block but the trigger's own.
# Every run is under ulimit -v 2 GiB (section 17.5). MALLOC_ARENA_MAX=2 stops glibc
# from reserving one arena per thread, which would fill that limit at 256 threads.
set -u
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
GC="$ROOT/kelpc/gc"
RUNS=${RUNS:-5}
LIMIT_KB=2097152
export MALLOC_ARENA_MAX=2
unset KELP_GC_VERIFY KELP_GC_STRESS KELP_GC_STATS

cd "$GC" || exit 2
BUILD_LOG=$(mktemp)
if ! cargo test --release --no-run >"$BUILD_LOG" 2>&1; then
    cat "$BUILD_LOG"
    echo "build failed"
    exit 2
fi
rm -f "$BUILD_LOG"
BIN=$(ls -t "$GC"/target/release/deps/kelp_gc-* | grep -v '\.d$' | head -1)
echo "host=$(hostname) cpus=$(nproc) load=$(cut -d' ' -f1-3 /proc/loadavg) bin=$BIN"

RAW=$(mktemp)
# One measurement test in its own process. $1 is the test name, the rest are env assignments.
run_one() {
    t=$1
    shift
    ( ulimit -v $LIMIT_KB; ulimit -t 600; env "$@" timeout 600 "$BIN" --exact "tests::bench_clear::$t" --ignored --nocapture --test-threads=1 2>&1 )
}

i=1
while [ "$i" -le "$RUNS" ]; do
    for mib in 256 1024; do
        out=$(run_one m4_clearing_cost_child KELP_GC_M4_MIB=$mib)
        echo "$out" | grep -q 'test result: ok' || { echo "M4 run $i heap_max=$mib FAILED"; echo "$out" | tail -30; }
        echo "$out" | grep -o 'M4 heap_max_mib=.*' >>"$RAW"
    done
    out=$(run_one m9_refill_tail_and_overshoot_256_threads)
    echo "$out" | grep -q 'test result: ok' || { echo "M9 run $i FAILED"; echo "$out" | tail -30; }
    echo "$out" | grep -o 'M9 attached=.*' >>"$RAW"
    i=$((i + 1))
done

echo "== raw lines"
cat "$RAW"
echo "== medians and verdicts"
python3 - "$RAW" "$RUNS" <<'PY'
import re, statistics, sys
raw, runs = sys.argv[1], int(sys.argv[2])
m4 = {256: [], 1024: []}
m9 = []
for line in open(raw):
    if line.startswith("M4 heap_max_mib="):
        mib = int(re.search(r"heap_max_mib=(\d+)", line).group(1))
        m4[mib].append(float(re.search(r"pause_ns_per_clearing=([\d.]+)", line).group(1)))
    elif line.startswith("M9 attached="):
        m9.append({k: v for k, v in re.findall(r"(\w+)=(\S+)", line)})
ok = True
for mib in (256, 1024):
    vals = m4[mib]
    if len(vals) != runs:
        print(f"M4 heap_max={mib} MiB: {len(vals)} of {runs} runs")
        ok = False
        continue
    med = statistics.median(vals)
    spread = (max(vals) - min(vals)) / med
    print(f"M4 heap_max={mib} MiB: median pause per clearing {med:.1f} ns, spread {spread * 100:.1f}%, runs {vals}")
if len(m4[256]) == runs and len(m4[1024]) == runs:
    ratio = statistics.median(m4[1024]) / statistics.median(m4[256])
    verdict = "PASS" if ratio <= 1.5 else "FAIL"
    print(f"M4 ratio 1024/256 = {ratio:.2f} against the line of 1.5: {verdict}")
    ok = ok and ratio <= 1.5
if len(m9) == runs:
    tot = []
    for r in m9:
        tot.append(int(r["unused_tail_bytes"]) + int(r["overshoot_bytes"]))
    med = statistics.median(tot)
    attached = sorted({int(r["attached"]) for r in m9})
    # Design rule (section 20.2 M9 and T31): zero blocks in use after the first
    # clearing, excluding the trigger block. The triggering allocation runs after
    # the clearing and holds one block, so that block is subtracted (the test
    # prints the raw count and the count excluding it).
    raw_after = [int(r["blocks_after_first_clearing"]) for r in m9]
    excl_after = [int(r["blocks_after_first_clearing_excl_trigger"]) for r in m9]
    freed = all(x == 0 for x in excl_after)
    print(f"M9 blocks in use after the first clearing, raw per run {raw_after}, "
          f"excluding the trigger block {excl_after}")
    print(f"M9 gate: zero blocks in use after the first clearing, excluding the trigger block "
          f"(section 20.2 M9 and T31): {freed}")
    print(f"M9 attached threads {attached}, unused tail + overshoot median {med:.0f} bytes "
          f"(runs {tot}), line 8 MiB = {8 << 20}")
    m9_ok = med <= (8 << 20) and freed and attached == [256]
    print(f"M9 verdict: {'PASS' if m9_ok else 'FAIL'}")
    ok = ok and m9_ok
else:
    print(f"M9: {len(m9)} of {runs} runs")
    ok = False
print("ALL PASS" if ok else "NOT ALL PASS")
sys.exit(0 if ok else 1)
PY
rc=$?
rm -f "$RAW"
exit $rc
