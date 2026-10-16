#!/bin/sh
# Boot the kernel 0 panic example under QEMU and check that the fault really
# printed over the serial port. Unlike `examples/kernel0/run.sh`, which compares
# whole lines, the assertion here is a *shape*: the compiler writes the check's
# message, so the test can only require the line the handler produced to start
# with `panic: ` and to contain `index out of bounds`.
#
# It has to run where `kelpc` was built and where `qemu-system-riscv64` and
# `ld.lld` are on PATH, that is the repo's `dev-kernel.nix` shell. Set KELPC to
# point at another binary. Nothing here reads the kernel's intent: the assertion
# is against QEMU's own stdout.
#
# Usage, from anywhere:  examples/kernel0-panic/run.sh [project-dir]
set -eu

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../../../.." && pwd)
project=${1:-$here}

kelpc=${KELPC:-$repo/target/debug/kelpc}
if [ ! -x "$kelpc" ]; then
    echo "run.sh: no kelpc at $kelpc (build it in dev-kernel.nix, or set KELPC)" >&2
    exit 1
fi
if ! command -v qemu-system-riscv64 >/dev/null 2>&1; then
    echo "run.sh: qemu-system-riscv64 is not on PATH (run me in dev-kernel.nix)" >&2
    exit 1
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
elf=$work/kernel.elf
serial=$work/serial.txt
qemu_stderr=$work/qemu.stderr

"$kelpc" build "$project" -o "$elf"

# `-bios none` makes QEMU start the image from 0x80000000 with no firmware in
# front of it, which is why the linker script has to put `_start` there. The
# serial bytes land on stdout; QEMU's own diagnostics go to stderr, so the file
# the assertion reads holds only what came out of the UART.
timeout 20 qemu-system-riscv64 \
    -machine virt \
    -bios none \
    -nographic \
    -no-reboot \
    -serial mon:stdio \
    -kernel "$elf" >"$serial" 2>"$qemu_stderr" </dev/null &
qpid=$!

# Read until the handler's line shows up, QEMU dies, or the timeout would fire;
# the same `timeout 20` above is the hard stop, so a kernel that never prints is
# killed by it and fails the assertion below rather than hanging the script.
i=0
while [ "$i" -lt 200 ]; do
    if grep -q '^panic: ' "$serial" 2>/dev/null; then
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

status=0
if ! grep -q '^panic: ' "$serial"; then
    echo "run.sh: the serial output has no line starting with 'panic: '" >&2
    status=1
fi
if ! grep -q 'index out of bounds' "$serial"; then
    echo "run.sh: the panic line does not name the check that fired" >&2
    status=1
fi
if [ "$status" -ne 0 ]; then
    echo "run.sh: captured serial output:" >&2
    cat "$serial" >&2
    echo "run.sh: qemu stderr:" >&2
    cat "$qemu_stderr" >&2
    exit 1
fi

echo "run.sh: booted $project and the UART printed:"
cat "$serial"
