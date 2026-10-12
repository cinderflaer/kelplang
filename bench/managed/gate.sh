#!/bin/sh
# Managed-mode benchmark gate (WP4). Times the GC-relevant benchmarks in hosted mode and, when a
# managed-capable kelpc is named, in managed mode, with the same wrapper as bench/run.sh (ulimit,
# timeout, GNU time for peak RSS, nice -n 0). Reports the ratio managed / hosted of the median wall
# time against gate G-A: 1.00x, with 1.10x as the fallback the owner may accept.
#
#   sh bench/managed/gate.sh                          hosted rows only, managed shows n/a
#   KELPC_MANAGED=/path/kelpc MANAGED_FLAGS=... sh bench/managed/gate.sh
#
# Environment: KELPC_HOSTED (default: 26.10.5 release asset), KELPC_MANAGED, MANAGED_FLAGS (empty
# until WP2/WP3 define the managed build flag), RUNS (timed runs, default 7), WARMUP (default 1),
# BT_DEPTH (binary-trees max depth, default 16; the managed runs use 20 per the design paper, so
# BT_DEPTH=20 measures that), OUT_DIR (default bench/managed/out).
#
# Also checks G-C for each row: every run's stdout must equal the first hosted run's stdout.
# Writes results.csv (bench,variant,run,wall_s,rss_kb) and gate.md (the table) to OUT_DIR.
# The 1-minute load average is read before and after each benchmark and recorded in the table.
set -u

HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
SUITE="$ROOT/bench/suite"
KELPC_HOSTED=${KELPC_HOSTED:-/home/juniper/Projects/kelp-work/release-assets/kelpc-26.10.5-linux-x86_64}
KELPC_MANAGED=${KELPC_MANAGED:-}
MANAGED_FLAGS=${MANAGED_FLAGS:-}
RUNS=${RUNS:-7}
WARMUP=${WARMUP:-1}
BT_DEPTH=${BT_DEPTH:-16}
OUT_DIR=${OUT_DIR:-$HERE/out}
LOCK=${LOCK:-/home/juniper/Projects/kelp-work/.build.lock}
export PATH=/nix/store/79mr0jw3qccq7hhf1hh62knxd88dwazc-gcc-wrapper-15.3.0/bin:$PATH

TIME_BIN=/usr/bin/time
[ -x "$TIME_BIN" ] || TIME_BIN=$(command -v time 2>/dev/null)
[ -n "$TIME_BIN" ] || { echo "gate.sh: GNU time not found" >&2; exit 1; }
[ -x "$KELPC_HOSTED" ] || { echo "gate.sh: no hosted kelpc at $KELPC_HOSTED" >&2; exit 1; }
managed_ok=0
[ -n "$KELPC_MANAGED" ] && [ -x "$KELPC_MANAGED" ] && managed_ok=1

mkdir -p "$OUT_DIR/build"
CSV="$OUT_DIR/results.csv"
TABLE="$OUT_DIR/table.txt"
: > "$CSV"
: > "$TABLE"
now_ns() { date +%s%N; }
load1() { cut -d' ' -f1 /proc/loadavg; }

# run_timed BIN ARG OUTFILE -> RUN_WALL (s), RUN_RSS (KiB), RUN_RC
run_timed() {
    rssf="$OUT_DIR/build/.rss"
    : > "$rssf"
    t0=$(now_ns)
    ( ulimit -v 4194304; ulimit -t 300; exec timeout 300 "$TIME_BIN" -f '%M' -o "$rssf" nice -n 0 "$1" $2 ) > "$3" 2> "$OUT_DIR/build/.err"
    RUN_RC=$?
    t1=$(now_ns)
    RUN_WALL=$(awk -v a="$t0" -v b="$t1" 'BEGIN { printf "%.4f", (b - a) / 1e9 }')
    RUN_RSS=$(tail -n 1 "$rssf" 2>/dev/null | tr -dc '0-9')
    [ -n "$RUN_RSS" ] || RUN_RSS=0
}

# stats FILE: FILE holds name,variant,run,wall,rss,check lines; prints median min max spread%
stats() {
    awk -F, '{ print $4 }' "$1" | sort -n | awk '
        { v[NR] = $1 }
        END {
            if (NR == 0) { print "n/a n/a n/a n/a"; exit }
            if (NR % 2) m = v[(NR + 1) / 2]; else m = (v[NR / 2] + v[NR / 2 + 1]) / 2
            printf "%.4f %.4f %.4f %.1f\n", m, v[1], v[NR], (v[NR] - v[1]) / m * 100
        }'
}

# bench_row NAME ARG SRC_DIR: builds and times one benchmark in every available variant
bench_row() {
    name=$1; arg=$2; d=$3
    src="$d/$name.kp"
    load_before=$(load1)
    flock "$LOCK" timeout 300 "$KELPC_HOSTED" build -O2 "$src" -o "$OUT_DIR/build/$name.hosted" > "$OUT_DIR/build/$name.hosted.build" 2>&1 || { echo "build failed: $name hosted" >&2; return 1; }
    variants="hosted"
    if [ "$managed_ok" = 1 ]; then
        flock "$LOCK" timeout 300 "$KELPC_MANAGED" build $MANAGED_FLAGS -O2 "$src" -o "$OUT_DIR/build/$name.managed" > "$OUT_DIR/build/$name.managed.build" 2>&1 || { echo "build failed: $name managed" >&2; return 1; }
        variants="$variants managed"
    fi
    ref="$OUT_DIR/build/$name.ref.out"
    rm -f "$ref"
    for v in $variants; do
        bin="$OUT_DIR/build/$name.$v"
        i=0
        while [ "$i" -lt "$WARMUP" ]; do run_timed "$bin" "$arg" "$OUT_DIR/build/$name.$v.warm"; i=$((i + 1)); done
        j=1
        while [ "$j" -le "$RUNS" ]; do
            run_timed "$bin" "$arg" "$OUT_DIR/build/$name.$v.out"
            [ -f "$ref" ] || cp "$OUT_DIR/build/$name.$v.out" "$ref"
            g=ok
            cmp -s "$ref" "$OUT_DIR/build/$name.$v.out" || g=MISMATCH
            [ "$RUN_RC" = 0 ] || g="rc=$RUN_RC"
            echo "$name,$v,$j,$RUN_WALL,$RUN_RSS,$g" >> "$OUT_DIR/$name.$v.csv"
            echo "$name,$v,$j,$RUN_WALL,$RUN_RSS" >> "$CSV"
            j=$((j + 1))
        done
        set -- $(stats "$OUT_DIR/$name.$v.csv" | cut -d' ' -f1-4)
        med=$1; mn=$2; mx=$3; sp=$4
        rss=$(awk -F, '{ if ($5 > m) m = $5 } END { printf "%.1f", m / 1024 }' "$OUT_DIR/$name.$v.csv")
        checks=$(awk -F, 'BEGIN { s = "ok" } $6 != "ok" { s = $6 } END { print s }' "$OUT_DIR/$name.$v.csv")
        rm -f "$OUT_DIR/$name.$v.csv"
        echo "$name|$v|$med|$mn|$mx|$sp|$rss|$checks|$load_before -> $(load1)" >> "$TABLE"
    done
}

bench_row binary-trees "$BT_DEPTH" "$SUITE/binary-trees"
bench_row strings "" "$SUITE/strings"
bench_row hashmap "" "$SUITE/hashmap"
bench_row json-tokenizer "" "$SUITE/json-tokenizer"

# Table, in the shape of bench/RESULTS.md. Wall seconds, median of RUNS runs.
{
    echo "# Managed-mode gate table (wall time, median of $RUNS runs after $WARMUP warm-up)"
    echo
    echo "Hosted: $KELPC_HOSTED. Managed: ${KELPC_MANAGED:-not available}. binary-trees depth $BT_DEPTH."
    echo "Gate G-A: managed median / hosted median <= 1.00 PASS, <= 1.10 FALLBACK (owner must accept), else FAIL."
    echo
    echo "| benchmark | variant | median s | min s | max s | spread % | peak RSS MiB | checksum | G-A | load 1 min (before -> after) |"
    echo "| --- | --- | ---: | ---: | ---: | ---: | ---: | --- | --- | --- |"
    while IFS='|' read -r name v med mn mx sp rss checks load; do
        ga="n/a"
        case "$v" in
            hosted) ga="baseline" ;;
            c) ga="reference" ;;
            managed)
                hm=$(grep "^$name|hosted|" "$TABLE" | cut -d'|' -f3)
                ga=$(awk -v m="$med" -v h="$hm" 'BEGIN { r = m / h; if (r <= 1.00) printf "PASS %.3fx", r; else if (r <= 1.10) printf "FALLBACK %.3fx", r; else printf "FAIL %.3fx", r }') ;;
        esac
        echo "| $name | $v | $med | $mn | $mx | $sp | $rss | $checks | $ga | $load |"
    done < "$TABLE"
} > "$OUT_DIR/gate.md"
cat "$OUT_DIR/gate.md"
