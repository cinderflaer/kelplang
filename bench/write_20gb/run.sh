#!/usr/bin/env bash
# Compiles and runs four 20 GiB writers (Rust, Kelp, C++, Python) and reports
# compile time, wall clock, and the phases each program timed for itself.
#
# Each program writes a fresh file, syncs it to storage, verifies the size and
# deletes it, so peak disk usage is one 20 GiB file, not four.
#
# Usage: ./run.sh [target-dir]      (default /mnt/bigdrive)
set -u

DIR="$(cd "$(dirname "$0")" && pwd)"
TARGET_DIR="${1:-/mnt/bigdrive}"
BUILD="$DIR/build"
RESULTS="$DIR/results.txt"
mkdir -p "$BUILD"

TOTAL_BYTES=$((20 * 1024 * 1024 * 1024))

now() { date +%s%N; }

# Rebuild the compiler outside the measured region: the language's compile time
# should not include building its own compiler.
echo "building kelpc (not counted)..." >&2
( cd "$DIR/../../" && cargo build -q --release -p kelpc ) || exit 1
KELPC="$DIR/../../target/release/kelpc"

printf '%-7s %12s %12s %12s %12s %12s %14s %10s\n' \
    lang compile_s wall_s work_s flush_s delete_s startup_s MiB/s | tee "$RESULTS"

for lang in rust kelp cpp py; do
    case "$lang" in
        rust) src="$DIR/write20gb.rs";  out="$BUILD/write20gb_rust";  path="$TARGET_DIR/kelp_bench_write20gb_rust.bin"
              compile_cmd="rustc -O -o $out $src"; run_cmd="$out $path" ;;
        cpp)  src="$DIR/write20gb.cpp"; out="$BUILD/write20gb_cpp";   path="$TARGET_DIR/kelp_bench_write20gb_cpp.bin"
              compile_cmd="clang++ -O2 -o $out $src"; run_cmd="$out $path" ;;
        kelp) src="$DIR/write20gb.kp";   out="$BUILD/write20gb_kelp";     path="$TARGET_DIR/kelp_bench_write20gb_kelp.bin"
              compile_cmd="$KELPC build $src -o $out"; run_cmd="$out" ;;
        py)   src="$DIR/write20gb.py";  out="";                       path="$TARGET_DIR/kelp_bench_write20gb_py.bin"
              compile_cmd=""; run_cmd="python3 $src $path" ;;
    esac

    c0=$(now)
    if [ -n "$compile_cmd" ]; then
        if ! eval "$compile_cmd" >"$BUILD/$lang.compile.log" 2>&1; then
            echo "$lang: compile FAILED"; cat "$BUILD/$lang.compile.log"; continue
        fi
    fi
    c1=$(now)

    r0=$(now)
    eval "$run_cmd" >"$BUILD/$lang.out" 2>&1
    status=$?
    r1=$(now)

    if [ $status -ne 0 ]; then
        echo "$lang: run FAILED (exit $status)"; cat "$BUILD/$lang.out"; continue
    fi

    bytes=$(awk '/^BYTES /{print $2}' "$BUILD/$lang.out")
    work=$(awk '/^WORK_NS /{print $2}' "$BUILD/$lang.out")
    flush=$(awk '/^FLUSH_NS /{print $2}' "$BUILD/$lang.out")
    delete=$(awk '/^DELETE_NS /{print $2}' "$BUILD/$lang.out")
    compile_ns=$((c1 - c0))
    wall_ns=$((r1 - r0))
    phases=$((work + flush + delete))
    startup_ns=$((wall_ns - phases))

    awk -v lang="$lang" -v c="$compile_ns" -v w="$wall_ns" -v work="$work" \
        -v flush="$flush" -v del="$delete" -v st="$startup_ns" -v bytes="$bytes" -v total="$TOTAL_BYTES" '
        BEGIN {
            if (bytes != total) {
                printf("%-7s %12s %12s %12s %12s %12s %14s %10s  <-- SHORT WRITE (got %d bytes)\n",
                       lang, "n/a", "n/a", "n/a", "n/a", "n/a", "n/a", "n/a", bytes)
                exit 1
            }
            mib = (bytes / 1048576.0) / ((work + flush) / 1e9)
            printf("%-7s %12.3f %12.3f %12.3f %12.3f %12.3f %14.3f %10.0f\n",
                   lang, c / 1e9, w / 1e9, work / 1e9, flush / 1e9, del / 1e9, st / 1e9, mib)
        }' | tee -a "$RESULTS"
done

echo                                             >> "$RESULTS"
echo "python3 bare startup: $( { s=$(now); python3 -c pass; e=$(now); echo $((e - s)); } ) ns" >> "$RESULTS"
tail -1 "$RESULTS"
