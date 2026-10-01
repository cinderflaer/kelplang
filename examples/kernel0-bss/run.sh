#!/bin/sh
# Boot the `.bss` clear example under QEMU. `bss clear` is the whole claim: the
# 64-byte marker this image loads non-zero is zero by the time the entry runs,
# so this is kernel0's harness with the expected line pinned.
#
# Usage, from anywhere:  examples/kernel0-bss/run.sh
set -eu

here=$(cd "$(dirname "$0")" && pwd)
exec "$here/../kernel0/run.sh" "$here" "bss clear"
