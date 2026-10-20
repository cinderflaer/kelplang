#!/bin/sh
# Builds kelpc, then runs the examples from the README's "Building and running" section in order.
#
#   build/run-examples.sh
#
# Run it inside the development shell (cargo, rustc, llvm-config, cc). Nothing is written into the
# repository: the built program goes to a temporary directory, and `fmt --check` leaves the sources alone.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/.." && pwd)
cd "$root"

examples=kelpc/tests/fixtures/examples
out=$(mktemp -d)
trap 'rm -rf "$out"' EXIT

cargo build --release -p kelpc
kelpc=./target/release/kelpc

"$kelpc" run "$examples/hello.kp"
"$kelpc" build "$examples/download.kp" -o "$out/download"
"$kelpc" check "$examples/hello.kp"
"$kelpc" check --json "$examples/hello.kp"
"$kelpc" fmt --check "$examples/hello.kp"
"$kelpc" lint "$examples/hello.kp"
echo "run-examples: all steps passed"
