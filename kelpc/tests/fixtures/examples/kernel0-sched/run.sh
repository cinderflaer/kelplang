#!/bin/sh
# Boot the preemptive scheduler and check the bytes that really came out of the
# serial port: every task printed its letter, the letters changed from one task
# to another at least three times, no line is torn (each is exactly `<letter>
# <count>`), and the kernel's own stop line is whole (`language/kernel.md` (kelp wiki)).
#
# The tasks never yield, so overlapping letters can only come from the machine
# timer interrupting them, and a torn line would mean two tasks reached the UART
# at once. The kernel stops itself after 40 ticks, which is what the marker line
# waits for; QEMU is then killed rather than waited out.
#
# It has to run where `kelpc` was built and where `qemu-system-riscv64` and
# `ld.lld` are on PATH, that is the repo's `dev-kernel.nix` shell. Set KELPC to
# point at another binary. The assertion is against QEMU's own stdout, not
# against the source's intent.
#
# Usage, from anywhere:  examples/kernel0-sched/run.sh
set -eu

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../../../.." && pwd)
project=$here

kelpc=${KELPC:-$repo/target/debug/kelpc}
if [ ! -x "$kelpc" ]; then
    echo "run.sh: no kelpc at $kelpc (build it in dev-kernel.nix, or set KELPC)" >&2
    exit 1
fi
for tool in qemu-system-riscv64 ld.lld; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "run.sh: $tool is not on PATH (run me in dev-kernel.nix)" >&2
        exit 1
    fi
done

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
elf=$work/kernel.elf
serial=$work/serial.txt

"$kelpc" build "$project" -o "$elf"

# The serial bytes land on stdout; QEMU's own diagnostics go to stderr, so the
# file the assertion reads holds only what came out of the UART.
timeout 20 qemu-system-riscv64 \
    -machine virt \
    -bios none \
    -nographic \
    -no-reboot \
    -serial mon:stdio \
    -kernel "$elf" >"$serial" 2>/dev/null </dev/null &
qpid=$!

marker="sched: done ticks=40"
i=0
while [ "$i" -lt 200 ]; do
    if grep -qxF -- "$marker" "$serial" 2>/dev/null; then
        break
    fi
    if ! kill -0 "$qpid" 2>/dev/null; then
        break
    fi
    i=$((i + 1))
    sleep 0.1
done
kill "$qpid" 2>/dev/null || true
wait "$qpid" 2>/dev/null || true

if ! grep -qxF -- "$marker" "$serial"; then
    echo "run.sh: the serial output has no line '$marker'" >&2
    echo "run.sh: captured serial output:" >&2
    cat "$serial" >&2
    exit 1
fi

# Every other line has to be one whole task line. The transition count is over
# consecutive lines, so it is exactly the number of times the CPU changed hands.
awk -v marker="$marker" '
    $0 == marker { done = 1; next }
    /^[ABC] [0-9]+$/ {
        letter = substr($0, 1, 1)
        seen[letter] = 1
        if (prev != "" && letter != prev) transitions++
        prev = letter
        lines++
        next
    }
    { bad = 1; printf "run.sh: torn line: %s\n", $0 > "/dev/stderr" }
    END {
        if (!done) {
            print "run.sh: the done line never appeared" > "/dev/stderr"
            bad = 1
        }
        if (!(seen["A"] && seen["B"] && seen["C"])) {
            print "run.sh: not every task printed its letter" > "/dev/stderr"
            bad = 1
        }
        if (transitions < 3) {
            printf "run.sh: only %d letter transition(s), want at least 3\n", transitions > "/dev/stderr"
            bad = 1
        }
        if (bad) exit 1
        printf "%d lines, %d transitions, no torn lines\n", lines, transitions
    }
' "$serial"
