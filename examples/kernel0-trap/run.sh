#!/bin/sh
# Boot the trap variant and check the two lines the UART really emitted: the
# one the trap handler writes and the one `kmain` writes after `mret` returns
# to it. The harness lives in `../kernel0/run.sh`; this only names the project
# and the lines.
#
# Usage, from anywhere:  examples/kernel0-trap/run.sh
set -eu
here=$(cd "$(dirname "$0")" && pwd)
exec "$here/../kernel0/run.sh" "$here" "trap: ecall" "done"
