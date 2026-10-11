#!/bin/sh
# Managed-mode GC benchmark runner (bench/gc). For each program in bench/gc/progs it builds a
# managed project (coral.toml with [target] mode = "managed" and heap_max) and an unmanaged
# project (no mode line: the malloc / Box free-list path). Each build runs once as a discarded
# warm-up and then RUNS times (default 3). It writes:
#   bench/gc/raw.csv      one row per run (the spread is read from here)
#   bench/gc/results.csv  one row per bench/variant with the median of the runs (gate.sh reads it)
#   bench/gc/RESULTS.md   a short table: medians, spread, managed/unmanaged ratio
#
# Run it through the build queue, inside the dev shell, at priority 30:
#   python3 .../tools/buildq/buildq.py submit -p 30 --cwd <worktree> --name "gc bench" \
#     -- nix-shell shell.nix --run 'sh bench/gc/run.sh'
#
# Every run is ( ulimit -v 2097152; ulimit -t 60; timeout 120 ./prog ): at most 2 GiB of address
# space, 60 CPU seconds and 120 s of wall time.
#
# Collector counters: when KELP_GC_STATS=1 is set in the environment, the collector is expected to
# print one line to stderr at exit:
#   kelp-gc-stats: collections=N pause_total_us=T pause_max_us=M
# A managed row with no such line leaves its counter columns empty, and gate.sh fails it. Missing
# pauses are never read as zero.
#
# A managed row counts as managed only when the linked binary has kelp_gc_* symbols. Otherwise it
# is marked NO-GC. At the commit this was measured on (4dcdc5b) the manifest accepts
# mode = "managed" and the checker applies its managed rules, but the production driver calls the
# code generator with managed = false (codegen_llvm.rs compile_ir: "the managed build's driver is
# WP2's"), and no collector is linked. A NO-GC row is the hosted binary: its numbers are not
# overhead, and its ratio is not printed.
#
# Knobs (environment): RUNS=3, KELPC=/path/to/kelpc (default target/release/kelpc, built if
# missing), BENCHES="binary-trees live-set" (subset), HEAP_MAX_MIB="16 64 256", BUILD=<dir>
# (default bench/gc/build, not committed), ARGS_<bench with _ for -> size argument,
# REPORT_ONLY=1 (regenerate results.csv and RESULTS.md from an existing raw.csv; no build, no run).
set -u

HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
PROGS="$HERE/progs"
BUILD=${BUILD:-$HERE/build}
RUNS=${RUNS:-3}
HEAP_MAX_MIB=${HEAP_MAX_MIB:-"16 64 256"}
ALL_BENCHES="binary-trees live-set strings mixed-pool heap-pressure"
BENCHES=${BENCHES:-$ALL_BENCHES}
CSV="$HERE/results.csv"
RAW="$HERE/raw.csv"
MD="$HERE/RESULTS.md"

TIME_BIN=""
for t in /usr/bin/time /run/current-system/sw/bin/time $(IFS=:; for p in $PATH; do echo "$p/time"; done); do
    if [ -x "$t" ] && [ ! -d "$t" ]; then TIME_BIN=$t; break; fi
done
[ -n "$TIME_BIN" ] || { echo "gc/run.sh: GNU time not found" >&2; exit 1; }

REPORT_ONLY=${REPORT_ONLY:-}
if [ -z "${KELPC:-}" ]; then
    KELPC="${CARGO_TARGET_DIR:-$ROOT/target}/release/kelpc"
    if [ -z "$REPORT_ONLY" ]; then
        echo "gc/run.sh: building kelpc (release)..." >&2
        ( cd "$ROOT" && cargo build -q --release -p kelpc ) || { echo "gc/run.sh: cargo build failed" >&2; exit 1; }
    fi
fi
[ -x "$KELPC" ] || { echo "gc/run.sh: no kelpc at $KELPC" >&2; exit 1; }

if [ -z "$REPORT_ONLY" ]; then
mkdir -p "$BUILD"
printf 'bench,variant,run,wall_s,user_s,sys_s,rss_kb,exit,gc_linked,collections,pause_total_ms,pause_max_ms,output\n' > "$RAW"

# make_project NAME VARIANT MODE HEAP_MAX -> prints the project directory.
# MODE is "managed", or empty for the unmanaged build. HEAP_MAX is in MiB.
make_project() {
    name=$1; variant=$2; mode=$3; heap=$4
    dir="$BUILD/$name-$variant.proj"
    rm -rf "${dir:?}/src"
    mkdir -p "${dir:?}/src"
    cp -r "$PROGS/$name/src/." "$dir/src/"
    {
        echo "name = \"$name\""
        echo "version = \"0.1.0\""
        echo "entry = \"src/main.kp\""
        echo
        echo "[target]"
        if [ -n "$mode" ]; then
            echo "heap_max = $heap"
        else
            echo "mode = \"hosted\""
        fi
    } > "$dir/coral.toml"
    echo "$dir"
}

# run_one BIN NAME VARIANT IDX. IDX 0 is the warm-up: it is run but not recorded.
run_one() {
    bin=$1; name=$2; variant=$3; idx=$4
    tf="$BUILD/$name-$variant.time.$idx"
    of="$BUILD/$name-$variant.out.$idx"
    ef="$BUILD/$name-$variant.err.$idx"
    key=$(echo "$name" | tr '-' '_')
    eval "arg=\${ARGS_$key:-}"
    ( ulimit -v 2097152; ulimit -t 60; KELP_GC_STATS=1 exec timeout 120 "$TIME_BIN" -f '%e %U %S %M %x' -o "$tf" "$bin" $arg ) > "$of" 2> "$ef"
    set -- $(tail -n 1 "$tf" 2>/dev/null)
    [ "$idx" -gt 0 ] || return 0
    stats=$(grep '^kelp-gc-stats:' "$ef" | tail -n 1)
    col=$(echo "$stats" | sed -n 's/.*collections=\([0-9]*\).*/\1/p')
    ptot=$(echo "$stats" | sed -n 's/.*pause_total_us=\([0-9]*\).*/\1/p')
    pmax=$(echo "$stats" | sed -n 's/.*pause_max_us=\([0-9]*\).*/\1/p')
    ptot_ms=$(awk -v u="$ptot" 'BEGIN { if (u != "") printf "%.3f", u / 1000 }')
    pmax_ms=$(awk -v u="$pmax" 'BEGIN { if (u != "") printf "%.3f", u / 1000 }')
    out=$(tr '\n' ' ' < "$of" | tr ',' ' ' | sed 's/ *$//')
    linked=$(cat "$BUILD/$name-$variant.linked" 2>/dev/null)
    printf '%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s\n' "$name" "$variant" "$idx" "${1:-}" "${2:-}" "${3:-}" \
        "${4:-}" "${5:-}" "$linked" "$col" "$ptot_ms" "$pmax_ms" "$out" >> "$RAW"
}

# The rows: name|variant|mode|heap_max
rows() {
    for b in $BENCHES; do
        case "$b" in
            heap-pressure)
                for h in $HEAP_MAX_MIB; do echo "$b|managed-$h|managed|$h"; done
                echo "$b|unmanaged||" ;;
            *)
                echo "$b|managed|managed|256"
                echo "$b|unmanaged||" ;;
        esac
    done
}

rows > "$BUILD/rows.txt"
while IFS='|' read -r name variant mode heap; do
    dir=$(make_project "$name" "$variant" "$mode" "$heap")
    bin="$BUILD/$name-$variant.bin"
    rm -f "$bin"
    if ! "$KELPC" build -O2 "$dir" -o "$bin" > "$bin.build.log" 2>&1 || [ ! -x "$bin" ]; then
        echo "gc/run.sh: $name/$variant: BUILD FAILED (see $bin.build.log)" >&2
        why=$(grep -m1 -i 'error' "$bin.build.log" | tr ',' ';' | cut -c1-200)
        printf '%s,%s,BUILD-FAILED,,,,,,,,,,build: %s\n' "$name" "$variant" "$why" >> "$RAW"
        continue
    fi
    # The collector's symbols in the linked binary (grep the strings: nm is not in every shell).
    if grep -a -q 'kelp_gc_alloc' "$bin"; then linked=yes; else linked=no; fi
    echo "$linked" > "$BUILD/$name-$variant.linked"
    i=0
    while [ "$i" -le "$RUNS" ]; do
        run_one "$bin" "$name" "$variant" "$i"
        i=$((i + 1))
    done
    echo "gc/run.sh: $name/$variant done (collector linked: $linked)" >&2
done < "$BUILD/rows.txt"
fi

# ------------------------------------------------- medians -> results.csv
awk -F, '
function sortv(a, n,    i, j, t) {
    for (i = 2; i <= n; i++) { t = a[i]; for (j = i - 1; j >= 1 && a[j] + 0 > t + 0; j--) a[j + 1] = a[j]; a[j + 1] = t }
}
function med(a, n) { sortv(a, n); return a[int((n + 1) / 2)] }
function mn(a, n,    i, m) { m = a[1]; for (i = 2; i <= n; i++) if (a[i] + 0 < m + 0) m = a[i]; return m }
function mx(a, n,    i, m) { m = a[1]; for (i = 2; i <= n; i++) if (a[i] + 0 > m + 0) m = a[i]; return m }
NR == 1 { next }
$3 == "BUILD-FAILED" { key = $1 "," $2; if (!(key in seen)) { seen[key] = 1; order[++nk] = key }; failed[key] = 1; failmsg[key] = $13; next }
{
    key = $1 "," $2
    if (!(key in seen)) { seen[key] = 1; order[++nk] = key }
    r = ++cnt[key]
    W[key, r] = $4; U[key, r] = $5; S[key, r] = $6; R[key, r] = $7
    if ($8 != 0) bad[key] = 1
    linked[key] = $9
    if ($10 != "") col[key, r] = $10
    if ($11 != "") ptot[key, r] = $11
    if ($12 != "") pmax[key, r] = $12
    if (!(key in outp)) outp[key] = $13
    else if (outp[key] != $13) mism[key] = 1
    if ($2 == "unmanaged") ref[$1] = $13
}
END {
    print "bench,variant,status,gc_linked,runs,wall_s_median,wall_s_min,wall_s_max,wall_spread_pct,user_s_median,sys_s_median,rss_kb_median,collections_median,pause_total_ms_median,pause_max_ms_median,output"
    for (q = 1; q <= nk; q++) {
        key = order[q]; split(key, p, ","); name = p[1]; variant = p[2]
        if (key in failed) { printf "%s,%s,BUILD-FAILED,,0,,,,,,,,,,,%s\n", name, variant, failmsg[key]; continue }
        n = cnt[key] + 0
        for (r = 1; r <= n; r++) { A[r] = W[key, r]; B[r] = U[key, r]; C[r] = S[key, r]; D[r] = R[key, r]; E[r] = ptot[key, r]; F[r] = pmax[key, r]; G[r] = col[key, r] }
        status = "ok"
        if (bad[key]) status = "FAILED-EXIT"
        else if (mism[key]) status = "OUTPUT-MISMATCH"
        else if (variant ~ /^managed/ && (name in ref) && outp[key] != ref[name]) status = "OUTPUT-MISMATCH"
        else if (variant ~ /^managed/ && linked[key] == "no") status = "NO-GC"
        mw = med(A, n); lo = mn(A, n); hi = mx(A, n)
        spread = (mw + 0 > 0) ? sprintf("%.1f", (hi - lo) / mw * 100) : ""
        cmed = (G[1] != "") ? sprintf("%.0f", med(G, n)) : ""
        tmed = (E[1] != "") ? sprintf("%.3f", med(E, n)) : ""
        pmed = (F[1] != "") ? sprintf("%.3f", med(F, n)) : ""
        printf "%s,%s,%s,%s,%d,%.3f,%.3f,%.3f,%s,%.3f,%.3f,%.0f,%s,%s,%s,%s\n", name, variant, status, linked[key], n, mw, lo, hi, spread, med(B, n), med(C, n), med(D, n), cmed, tmed, pmed, outp[key]
    }
}' "$RAW" > "$CSV"
echo "gc/run.sh: wrote $RAW and $CSV" >&2

# ------------------------------------------------- RESULTS.md from results.csv
awk -F, -v host="$(uname -srm)" -v cpu="$(grep -m1 'model name' /proc/cpuinfo | sed 's/.*: //')" \
    -v mem="$(awk '/MemTotal/ { printf "%.1f GB", $2 / 1048576 }' /proc/meminfo)" \
    -v kelpc="$("$KELPC" version 2>&1 | head -1)" -v runs="$RUNS" -v when="$(date -u '+%Y-%m-%d %H:%M UTC')" '
NR == 1 { next }
{
    k = $1 SUBSEP $2; rows[++n] = $0; med[k] = $6; wmin[k] = $7; wmax[k] = $8; spr[k] = $9
    usr[k] = $10; sy[k] = $11; rss[k] = $12; col[k] = $13; pt[k] = $14; pm[k] = $15; st[k] = $3; lk[k] = $4; out[k] = $16
    if ($2 == "unmanaged") base[$1] = $6
}
END {
    print "# GC benchmark results (bench/gc)"
    print ""
    print "Generated by `bench/gc/run.sh` on " when ". Do not edit by hand: regenerate it. Medians of " runs " timed runs per row (one discarded warm-up first). The spread is (max - min) / median of the wall time, in percent."
    print ""
    print "- Machine: " cpu ", " mem " RAM, " host
    print "- kelpc: " kelpc
    print "- Build: `kelpc build -O2` on the default LLVM backend. Managed rows set `heap_max` (MiB). Unmanaged rows set `mode = \"hosted\"`. Since the managed mode was removed, both kinds of row build the same shared-pool program."
    print "- Run wrapper: `( ulimit -v 2097152; ulimit -t 60; timeout 120 ... )`."
    print ""
    print "| bench | variant | status | wall s (median) | spread % | user s | sys s | peak RSS MiB | collections | pause total ms | pause max ms | managed / unmanaged wall |"
    print "|---|---|---|---|---|---|---|---|---|---|---|---|"
    for (i = 1; i <= n; i++) {
        split(rows[i], f, ",")
        k = f[1] SUBSEP f[2]
        if (st[k] == "BUILD-FAILED") { print "| " f[1] " | " f[2] " | BUILD-FAILED | | | | | | | | | | " substr(f[16], 1, 200) " |"; continue }
        ratio = ""
        if (f[2] ~ /^managed/ && st[k] == "ok" && (f[1] in base) && base[f[1]] + 0 > 0) ratio = sprintf("%.2fx", med[k] / base[f[1]])
        else if (f[2] ~ /^managed/) ratio = "n/a (" st[k] ")"
        printf "| %s | %s | %s | %s | %s | %s | %s | %.1f | %s | %s | %s | %s |\n", f[1], f[2], st[k], med[k], spr[k], usr[k], sy[k], rss[k] / 1024, col[k], pt[k], pm[k], ratio
    }
    print ""
    print "Status NO-GC: the managed build did not link the collector (no `kelp_gc_*` symbol in the binary). The production driver generates code with managed = false, so the binary is the hosted one and its numbers are not overhead. No ratio is printed for these rows. A blank counter means no `kelp-gc-stats:` line was printed; no collector prints one at this commit."
    print ""
    print "Spread is the run-to-run noise of three timed runs. Differences smaller than the spread are not signal."
}' "$CSV" > "$MD"
echo "gc/run.sh: wrote $MD" >&2
