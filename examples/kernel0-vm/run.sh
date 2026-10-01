#!/bin/sh
# Boot the Sv39 kernel and check every step it printed: the machine-mode
# setup, the S-mode entry after `mret`, the alias between the high virtual name
# and the identity-mapped physical name, and that unmapping clears the leaf.
# The harness lives in `../kernel0/run.sh`; this only names the project and the
# lines.
#
# Usage, from anywhere:  examples/kernel0-vm/run.sh
set -eu
here=$(cd "$(dirname "$0")" && pwd)
exec "$here/../kernel0/run.sh" "$here" \
    "vm: m-mode" \
    "vm: root" \
    "vm: identity" \
    "vm: uart page" \
    "vm: high page" \
    "vm: s-mode" \
    "vm: paging on" \
    "vm: alias ok" \
    "vm: unmap ok"
