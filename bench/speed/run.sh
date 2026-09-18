#!/usr/bin/env bash
# Compares the working speed of the same three CPU-bound workloads in Rust, Kelp,
# C++ and Python.
#
# Every language runs the *same* algorithm with the *same* op counts, so the
# checksums have to agree — that is what makes the timings comparable. Each
# program times its own workloads; this harness compiles them, runs each
# program three times, keeps the fastest measurement per workload, checks that
# every language produced the same checksums, and prints ns/op.
set -u

DIR="$(cd "$(dirname "$0")" && pwd)"
BUILD="$DIR/build"
TSV="$BUILD/results.tsv"
REPORT="$DIR/results.txt"
TRIALS=3
mkdir -p "$BUILD"
rm -f "$BUILD"/wall.*.tsv "$BUILD"/*.out "$BUILD"/results.tsv

now() { date +%s%N; }

echo "building kelpc (not counted)..." >&2
( cd "$DIR/../../" && cargo build -q --release -p kelpc ) >/dev/null 2>&1 || exit 1
KELPC="$DIR/../../target/release/kelpc"

: > "$TSV"
: > "$REPORT"

compile_one() {
    local lang="$1" cmd="$2" c0 c1
    if [ -z "$cmd" ]; then
        # No compile step at all: Python parses and compiles its source at
        # startup, so that cost shows up under startup instead.
        echo "$lang -1" >> "$BUILD/compile.tsv"
        return 0
    fi
    c0=$(now)
    if [ -n "$cmd" ]; then
        if ! eval "$cmd" >"$BUILD/$lang.compile.log" 2>&1; then
            echo "$lang: compile FAILED"; tail -5 "$BUILD/$lang.compile.log"; return 1
        fi
    fi
    c1=$(now)
    echo "$lang $((c1 - c0))" >> "$BUILD/compile.tsv"
}

: > "$BUILD/compile.tsv"
compile_one rust "rustc -O -o $BUILD/speed_rust $DIR/speed.rs" || exit 1
compile_one cpp  "clang++ -O2 -o $BUILD/speed_cpp $DIR/speed.cpp" || exit 1
compile_one kelp    "$KELPC build $DIR/speed.kp -o $BUILD/speed_kelp" || exit 1
compile_one py   "" || exit 1

# What a bare interpreter start costs, for context on Python's startup line.
{ s=$(now); python3 -c pass; e=$(now); printf "baseline: python3 -c pass (interpreter startup only) %.1f ms\n\n" "$(( (e - s) / 1000 ))e-3"; } | tee -a "$REPORT"

for trial in $(seq 1 "$TRIALS"); do
    for lang in rust cpp kelp py; do
        case "$lang" in
            rust) exe="$BUILD/speed_rust" ;;
            cpp)  exe="$BUILD/speed_cpp" ;;
            kelp) exe="$BUILD/speed_kelp" ;;
            py)   exe="python3 $DIR/speed.py" ;;
        esac
        w0=$(now)
        eval "$exe" >"$BUILD/$lang.$trial.out" 2>&1
        status=$?
        w1=$(now)
        if [ $status -ne 0 ]; then
            echo "$lang: run FAILED (trial $trial, exit $status)"; cat "$BUILD/$lang.$trial.out"; exit 1
        fi
        work_ns=$(awk '/^RESULT /{ s += $4 } END { print s + 0 }' "$BUILD/$lang.$trial.out")
        echo "$lang $((w1 - w0)) $work_ns" >> "$BUILD/wall.$trial.tsv"
        while read -r _ workload ops ns checksum; do
            echo "$lang $workload $ops $ns $checksum" >> "$TSV"
        done < <(grep '^RESULT ' "$BUILD/$lang.$trial.out")
    done
done

# Fastest measurement per (language, workload), then the comparison table.
awk '
{
    key = $1 " " $2
    if (!(key in best) || $4 < best[key]) { best[key] = $4; ops[key] = $3 }
    if (!(key in cks)) { cks[key] = $5 } else if (cks[key] != $5) { mismatch[key] = 1 }
    langs[$1] = 1
    workloads[$2] = 1
    if (!(($2) in wlorder)) { wlorder[$2] = ++nwl; wlnames[nwl] = $2 }
}
END {
    printf "%-9s %14s %14s %12s %10s %10s\n", "workload", "language", "ops", "ns/op", "Mops/s", "vs C++"
    printf "%s\n", "------------------------------------------------------------------------------"
    for (w = 1; w <= nwl; w++) {
        wl = wlnames[w]
        cpp_nsop = 0
        split("rust kelp cpp py", order, " ")
        for (i = 1; i <= 4; i++) {
            k = order[i] " " wl
            if (k in best) { cpp_nsop = best["cpp " wl] / ops["cpp " wl]; break }
        }
        for (i = 1; i <= 4; i++) {
            lang = order[i]
            k = lang " " wl
            if (!(k in best)) { continue }
            nsop = best[k] / ops[k]
            ratio = (cpp_nsop > 0) ? nsop / cpp_nsop : 0
            printf "%-9s %14s %14d %12.3f %10.1f %9.1fx%s\n", wl, lang, ops[k], nsop,
                   1000.0 / nsop, ratio, (k in mismatch) ? "  CHECKSUM MISMATCH" : ""
        }
    }
    printf "\n"
    for (w = 1; w <= nwl; w++) {
        wl = wlnames[w]
        printf "checksum %-8s ", wl
        split("rust kelp cpp py", order, " ")
        for (i = 1; i <= 4; i++) {
            k = order[i] " " wl
            printf "%s=%s ", order[i], (k in cks) ? cks[k] : "?"
        }
        printf "%s\n", (wl in mismatch) ? "<-- LANGUAGES DISAGREE" : "(all agree)"
    }
}
' "$TSV" | tee "$REPORT"

echo >> "$REPORT"
echo "compile (s) / wall per trial (s) / startup = wall - work (ms):" >> "$REPORT"
for lang in rust x cpp py; do
    cns=$(awk -v l="$lang" '$1 == l { print $2 }' "$BUILD/compile.tsv")
    # Startup is measured per trial (wall minus that trial's three workload
    # times, so the two are never from different runs) and the smallest is
    # reported, consistent with taking the fastest workload times.
    stats=$(awk -v l="$lang" '$1 == l { if (!(seen) || $2 < w) w = $2; s = $2 - $3; if (!(seen) || s < st) st = s; seen = 1 } END { printf "%.3f %.1f", w / 1e9, st / 1e6 }' "$BUILD"/wall.*.tsv)
    awk -v l="$lang" -v c="$cns" -v st="$stats" \
        'BEGIN { split(st, a, " ");
                 cs = (c < 0) ? "n/a (interpreted)" : sprintf("%.3f", c / 1e9);
                 printf "  %-5s compile %-17s wall %6.3f   startup %6.1f\n", l, cs, a[1], a[2] }' | tee -a "$REPORT"
done
