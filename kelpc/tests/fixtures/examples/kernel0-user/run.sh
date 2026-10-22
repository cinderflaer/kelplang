#!/bin/sh
# Boot the user-mode kernel and check every line the serial port really emitted:
# the frame-layout check, the syscall refusals checked before any program runs,
# the first program's two writes and its exit, the bad pointer's `EFAULT`, the
# second program's load page fault, and the kernel's survival after it. The
# harness lives in `../kernel0/run.sh`; this only names the project and the lines.
#
# Usage, from anywhere:  examples/kernel0-user/run.sh
set -eu
here=$(cd "$(dirname "$0")" && pwd)
exec "$here/../kernel0/run.sh" "$here" \
    "user: s-mode" \
    "user: frame ok" \
    "syscall: refusals ok" \
    "user: paging on" \
    "hello from user" \
    "user: efault ok" \
    "bad write -> E" \
    "user exited 7" \
    "user killed: scause 13" \
    "kernel survived"
