#!/bin/sh
# Boot the Polyp framebuffer kernel under QEMU and check the bytes that really
# came out of the serial port. The first argument is the project directory
# (default: this example); every argument after it is a line that must appear
# whole in the captured output (default: `end` and `done`, the report's last line
# and the run's own end marker). The last line is also the marker the script
# waits for before it kills QEMU: the kernel halts in a `hlt` loop, so waiting
# for the `timeout 20` to expire would waste the whole window.
#
# The kernel prints one `step <n> checksum <hex> dirty <count>` line per frame and
# then the todo program's report. `cargo test --test compile_and_run
# polyp_fb_x86_64` is the test that also compares those lines with the hosted
# soft backend's; this script is the manual boot.
#
# It has to run where `kelpc` was built and where `qemu-system-x86_64` and
# `ld.lld` are on PATH, that is the repo's `dev-kernel.nix` shell. Set KELPC to
# point at another binary. Nothing here reads the kernel's intent: the
# assertion is a comparison against QEMU's own stdout.
#
# Usage, from anywhere:  examples/polyp-fb-x86_64/run.sh
set -eu

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
project=${1:-$here}
if [ "$#" -gt 0 ]; then
    shift
fi
if [ "$#" -eq 0 ]; then
    set -- "end" "done"
fi
marker=""
for line in "$@"; do
    marker=$line
done

kelpc=${KELPC:-$repo/target/debug/kelpc}
if [ ! -x "$kelpc" ]; then
    echo "run.sh: no kelpc at $kelpc (build it in dev-kernel.nix, or set KELPC)" >&2
    exit 1
fi
if ! command -v qemu-system-x86_64 >/dev/null 2>&1; then
    echo "run.sh: qemu-system-x86_64 is not on PATH (run me in dev-kernel.nix)" >&2
    exit 1
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
elf=$work/kernel.elf
serial=$work/serial.txt
qemu_stderr=$work/qemu.stderr

"$kelpc" build "$project" -o "$elf"

# QEMU's x86-64 `-kernel` hands the ELF to SeaBIOS' Xen PVH loader, which jumps
# to the 32-bit entry the image's `XEN_ELFNOTE_PHYS32_ENTRY` note names; the
# compiler's `_start` stub leaves protected mode for long mode before it calls
# the entry. `-cpu qemu64` is the core the example's target machine is built
# for. The serial bytes land on stdout (SeaBIOS' own messages included); QEMU's
# diagnostics go to stderr, so the file the assertion reads holds the UART.
timeout 20 qemu-system-x86_64 \
    -M q35 \
    -cpu qemu64 \
    -nographic \
    -serial mon:stdio \
    -no-reboot \
    -kernel "$elf" >"$serial" 2>"$qemu_stderr" </dev/null &
qpid=$!

# Read until the marker shows up, QEMU dies, or the timeout would fire; the
# same `timeout 20` above is the hard stop, so a kernel that never prints is
# killed by it and fails the assertion below rather than hanging the script.
i=0
while [ "$i" -lt 200 ]; do
    if grep -qxF -- "$marker" "$serial"; then
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
for line in "$@"; do
    if ! grep -qxF -- "$line" "$serial"; then
        echo "run.sh: the serial output has no line '$line'" >&2
        status=1
    fi
done
if [ "$status" -ne 0 ]; then
    echo "run.sh: captured serial output:" >&2
    cat "$serial" >&2
    echo "run.sh: qemu stderr:" >&2
    cat "$qemu_stderr" >&2
    exit 1
fi

echo "run.sh: booted $project and the UART printed:"
cat "$serial"
