#!/bin/sh
# Boot the fault variant and check the two lines the S-mode handler prints: a
# load page fault (`scause` 13, `0xd`) at the unmapped address, then a store
# page fault (15, `0xf`) at the read-only page. The harness lives in
# `../kernel0/run.sh`; this only names the project and the lines.
#
# Usage, from anywhere:  examples/kernel0-vm-fault/run.sh
set -eu
here=$(cd "$(dirname "$0")" && pwd)
exec "$here/../kernel0/run.sh" "$here" \
    "fault: m-mode" \
    "fault: paging on" \
    "fault: scause 0xd stval 0x40000000" \
    "fault: scause 0xf stval 0x3ffffff000"
