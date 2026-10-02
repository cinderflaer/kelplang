#!/bin/sh
# Boot the x86-64 IDT kernel under QEMU and check the bytes that really came out
# of the serial port, in the order they must have come out
# (`plan/x86-idt.md`):
#
#   exception 6 ...   the `ud2`, repaired: the handler advanced `rip`
#   exception 0 ...   the divide by zero, repaired the same way
#   tick 1 .. tick 5  the 8254 timer through the remapped 8259
#   x86: done         the boot's line after it masked the line
#   exception 14 ...  the store through the unmapped page, printed and halted
#
# The exception lines are matched by prefix, because the `rip` they report is
# whatever the linker chose; their error codes and the `cr2` are pinned, because
# a write to a not-present supervisor page is error code 2 and the address is
# the one `main.kp` names.
#
# It has to run where `kelpc` was built and where `qemu-system-x86_64` and
# `ld.lld` are on PATH, that is the repo's `dev-kernel.nix` shell. Set KELPC to
# point at another binary. Nothing here reads the kernel's intent: the
# assertion is a comparison against QEMU's own stdout.
#
# Usage, from anywhere:  examples/kernel0-x86_64-idt/run.sh
set -eu

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
project=$here

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

cat >"$work/expected" <<'EOF'
exception 6 err 0 rip 0x
exception 0 err 0 rip 0x
tick 1
tick 2
tick 3
tick 4
tick 5
x86: done
exception 14 err 2 rip 0x
EOF

# QEMU's x86-64 `-kernel` hands the ELF to SeaBIOS' Xen PVH loader, which jumps
# to the 32-bit entry the image's `XEN_ELFNOTE_PHYS32_ENTRY` note names; the
# compiler's `_start` stub leaves protected mode for long mode before it calls
# the entry. The serial bytes land on stdout; QEMU's diagnostics go to stderr.
timeout 20 qemu-system-x86_64 \
    -M q35 \
    -cpu qemu64 \
    -nographic \
    -serial mon:stdio \
    -no-reboot \
    -kernel "$elf" >"$serial" 2>"$qemu_stderr" </dev/null &
qpid=$!

# The last thing the kernel prints is the page-fault report, so that is the
# marker; the same `timeout 20` above is the hard stop, so a kernel that never
# gets there is killed by it and fails the assertions below rather than hanging
# the script.
i=0
while [ "$i" -lt 200 ]; do
    if grep -q 'exception 14 ' "$serial"; then
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

# Walk the captured output once, requiring each expected line to appear after
# the one that carried the previous pattern. The match is a substring rather
# than a prefix because SeaBIOS' "Booting from ROM.." has no newline of its own
# and is glued to whichever kernel line comes first.
if ! awk '
    NR == FNR { pat[++n] = $0; next }
    i < n && index($0, pat[i + 1]) > 0 { i++ }
    END {
        if (i < n) {
            printf "run.sh: the serial output has no line with \"%s\" after the previous ones\n", pat[i + 1] > "/dev/stderr"
            exit 1
        }
    }
' "$work/expected" "$serial"; then
    status=1
fi

if ! grep -qE 'exception 14 err 2 rip 0x[0-9a-f]{16} cr2 0x0000000200000000' "$serial"; then
    echo "run.sh: the #PF line does not report error code 2 and cr2 0x200000000" >&2
    status=1
fi
if grep -q 'tick 6' "$serial"; then
    echo "run.sh: the timer was not masked after the fifth tick" >&2
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
