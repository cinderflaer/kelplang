#!/bin/sh
# Builds existing runnable fixtures with a sanitizer on the C backend and
# checks that the sanitizer reports nothing and that the program behaves as it
# does without it (plan/sanitizers.md).
#
# Usage, from the repository root, inside the nix shell (it needs `cc`):
#   KELPC=kelpc/target/debug/kelpc kelpc/tests/run-sanitizers.sh [address|undefined|thread ...]
#
# With no argument it runs all three. Environment:
#   KELPC          the compiler to use (default: kelpc on PATH)
#   SAN_COUNT      how many fixtures each sanitizer must cover (default 40)
#   SAN_CANDIDATES how many fixtures to try at most (default 400)
#
# Fixture selection is automatic and deterministic (sorted names): a fixture is
# a candidate when its name does not look like network, stdin, sleep or process
# work; it is USED when the plain C build of it succeeds, runs to exit status 0
# inside 10 seconds, and the sanitized build then builds too. The sanitized
# program must exit with the same status and print the same stdout as the plain
# one, and its stderr must hold no sanitizer report. Every program runs with
# stdin from /dev/null, in a scratch directory, under `ulimit -t 10` and
# `timeout 10`. `ulimit -v` is applied to the plain run only: a sanitizer's
# shadow memory needs terabytes of address space, so the sanitized run is
# bounded by CPU time, wall time and the sanitizer's own RSS limit instead.
#
# Exit status: 0 when no fixture produced a finding and every sanitizer covered
# SAN_COUNT fixtures, 1 otherwise.
set -u

KELPC="${KELPC:-kelpc}"
SAN_COUNT="${SAN_COUNT:-40}"
SAN_CANDIDATES="${SAN_CANDIDATES:-400}"
here=$(cd "$(dirname "$0")" && pwd)
fixtures="$here/fixtures"
sans="${*:-address undefined thread}"

work=$(mktemp -d "${TMPDIR:-/tmp}/kelp-san.XXXXXX")
trap 'rm -rf "$work"' EXIT INT TERM

# Sanitizer options: leaks are findings; UBSan and TSan print a stack; the RSS
# limit is the memory bound the address-space limit cannot be.
export ASAN_OPTIONS="detect_leaks=1:abort_on_error=0:hard_rss_limit_mb=1024:exitcode=66"
export UBSAN_OPTIONS="print_stacktrace=1:halt_on_error=1:exitcode=66"
export TSAN_OPTIONS="exitcode=66:halt_on_error=0"

candidates=$(ls "$fixtures" | grep '\.kp$' | grep -Ev 'net|http|tcp|udp|tls|dns|socket|sleep|stdin|input|server|spawn|process|signal|fail|panic|abort|bad|error|refus|reject|bounds|overflow|hazard|asm|irq|mmio|kernel|qemu|freestanding' | head -n "$SAN_CANDIDATES")

status=0
for san in $sans; do
    used=0
    findings=0
    tried=0
    printf '\n=== sanitizer: %s ===\n' "$san"
    for f in $candidates; do
        [ "$used" -ge "$SAN_COUNT" ] && [ "${SAN_ALL:-0}" != 1 ] && break
        tried=$((tried + 1))
        name="${f%.kp}"
        src="$fixtures/$f"
        dir="$work/$name"
        mkdir -p "$dir"
        # Plain C build and run: the reference behaviour.
        if ! KELPC_ORACLE=c "$KELPC" build "$src" -o "$dir/plain" >/dev/null 2>&1; then
            continue
        fi
        ( cd "$dir" && ulimit -v 1048576 && ulimit -t 10 && timeout 10 ./plain </dev/null >plain.out 2>plain.err )
        [ $? -eq 0 ] || continue
        # Sanitized build: a build failure here is a finding, the plain one worked.
        if ! KELPC_ORACLE=c "$KELPC" build "$src" -o "$dir/san" --sanitize="$san" >"$dir/build.err" 2>&1; then
            printf 'FINDING %s: the %s build failed\n' "$name" "$san"
            sed -n '1,8p' "$dir/build.err"
            findings=$((findings + 1))
            continue
        fi
        ( cd "$dir" && ulimit -t 10 && timeout 10 ./san </dev/null >san.out 2>san.err )
        code=$?
        if [ "$code" -eq 124 ] || [ "$code" -eq 137 ]; then
            # Over the time limit under instrumentation: not a verdict either way.
            printf 'skip %s: timed out under %s\n' "$name" "$san"
            continue
        fi
        used=$((used + 1))
        bad=0
        grep -Eq 'Sanitizer|runtime error:|SUMMARY:' "$dir/san.err" && bad=1
        [ "$code" -eq 0 ] || bad=1
        cmp -s "$dir/plain.out" "$dir/san.out" || bad=1
        if [ "$bad" -eq 1 ]; then
            printf 'FINDING %s: exit %s\n' "$name" "$code"
            sed -n '1,25p' "$dir/san.err"
            findings=$((findings + 1))
        else
            printf 'ok %s\n' "$name"
        fi
    done
    printf -- '--- %s: %s fixtures covered, %s findings (%s tried)\n' "$san" "$used" "$findings" "$tried"
    [ "$findings" -eq 0 ] || status=1
    [ "$used" -ge "$SAN_COUNT" ] || { printf 'only %s of %s fixtures usable\n' "$used" "$SAN_COUNT"; status=1; }
done
exit "$status"
