#!/bin/sh
# Boot the stack-guard kernel and check the three faults it takes on purpose.
# Phase t stores into the trap stack's guard page. Phase a recurses with a small
# frame until it crosses the kernel stack's guard; phase b is entered from the
# trap handler with a fresh `sp` and overflows that guard again with one 16 KiB
# frame. Each fault prints `stack overflow: scause 0xf stval 0x...` and then
# `stack: stval in guard`, which is the handler's own range check against the
# two guard pages. The harness in
# `../kernel0/run.sh` does the build and the QEMU run; this only names the
# project and the lines that must appear.
#
# Usage, from anywhere:  examples/kernel0-stack/run.sh
set -eu
here=$(cd "$(dirname "$0")" && pwd)
exec "$here/../kernel0/run.sh" "$here" \
    "stack: m-mode" \
    "stack: paging on" \
    "stack: phase t" \
    "stack: stval in guard" \
    "stack: phase a" \
    "stack: stval in guard" \
    "stack: phase b" \
    "stack: stval in guard" \
    "stack: halted"
