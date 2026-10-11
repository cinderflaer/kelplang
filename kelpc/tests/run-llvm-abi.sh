#!/bin/sh
# The BUG-vN4tR gate: an aggregate over two eightbytes crosses a C boundary by
# pointer, never by value.
#
# MK7 section 3.3 states the rule and the entry that opened BUG-vN4tR measured
# the trap: clang lowers a 24-byte struct argument to `byval(%struct)` on the
# stack and a 24-byte struct return to `sret(%struct)` through a hidden pointer,
# and LLVM's backend does not derive either lowering from an IR call that passes
# the aggregate by value. The IR verifier cannot see the mismatch, and both sides
# of an IR-to-IR call agree with each other, so the only witness is a real object
# compiled by a C compiler and linked in.
#
# This gate is that witness. It builds `kelpc/runtime/abi_aggregate.c` with `cc`
# and:
#
#   * shows the trap. Two hand-written IR programs call the same C object, one
#     passing the 24-byte struct by value and one through a pointer. The two
#     printed numbers differ, which is the whole defect.
#   * shows the fixed shape end to end. A Kelp program that receives a 24-byte
#     `Bytes` from `mem.alloc` is linked against a C object that defines that
#     extern body with the pointer-spelled ABI the backend declares
#     (`declare void @kelp_std__mem__alloc_raw(ptr, i64)`). The program prints
#     the C object's text `ZY`; the compiler's own body returns a zero-filled
#     buffer, which prints as an empty line.
#   * reads the backend's own IR back and asserts no C-boundary declaration takes
#     or returns a 24-byte aggregate by value. This is the pin the entry asks
#     for, and it is what catches the next crossing (`pd` globals, or an
#     `extern fun` whose body takes a `datac`).
#
# The runtime the relink needs is the archive `build.rs` builds from `kelp-rt/`
# (`target/release/build/kelpc-*/out/libkelp_rt.a`), because the Kelp object
# references `kelp_panic`, `kelp_Bytes__set` and the rest of it.
#
# `cargo build --release` runs first, and every command is bounded by the
# project's own `timeout 2400` when this is run through the shell it needs:
#
#   timeout 2400 nix-shell --run './kelpc/tests/run-llvm-abi.sh'
#
# Usage, from anywhere inside the shell:  ./kelpc/tests/run-llvm-abi.sh
set -e

cd "$(dirname "$0")/../.."

CC="${CC:-cc}"
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT

cargo build --release

# The genuine C object. `cc` is the compiler `kelpc` links with, so its ABI is
# the one the runtime object is built under too.
"$CC" -c -O2 -o "$T/abi.o" kelpc/runtime/abi_aggregate.c

# ---- the trap, measured against a genuine C object -------------------------
#
# One IR call per spelling of the same C function. The C object's `byval`
# parameter and the by-value IR call do not agree, so `byval` prints a number the
# pointer spelling never prints.
cat > "$T/byval.ll" <<'EOF'
%kelp_abi_wide = type { i64, i64, i64 }
declare i32 @printf(ptr, ...)
declare i64 @kelp_abi_take(%kelp_abi_wide)
@fmt = private constant [5 x i8] c"%ld\0A\00"
define i32 @main() {
entry:
  %a = insertvalue %kelp_abi_wide undef, i64 3, 0
  %b = insertvalue %kelp_abi_wide %a, i64 4, 1
  %c = insertvalue %kelp_abi_wide %b, i64 5, 2
  %r = call i64 @kelp_abi_take(%kelp_abi_wide %c)
  call i32 (ptr, ...) @printf(ptr @fmt, i64 %r)
  ret i32 0
}
EOF
cat > "$T/byptr.ll" <<'EOF'
%kelp_abi_wide = type { i64, i64, i64 }
declare i32 @printf(ptr, ...)
declare i64 @kelp_abi_take_ptr(ptr)
@fmt = private constant [5 x i8] c"%ld\0A\00"
define i32 @main() {
entry:
  %slot = alloca %kelp_abi_wide
  store %kelp_abi_wide { i64 3, i64 4, i64 5 }, ptr %slot
  %r = call i64 @kelp_abi_take_ptr(ptr %slot)
  call i32 (ptr, ...) @printf(ptr @fmt, i64 %r)
  ret i32 0
}
EOF

clang -w -o "$T/byval" "$T/byval.ll" "$T/abi.o"
clang -w -o "$T/byptr" "$T/byptr.ll" "$T/abi.o"
BYVAL="$("$T/byval")"
BYPTR="$("$T/byptr")"
echo "the C object, called with the struct by value: $BYVAL"
echo "the C object, called with a pointer to it:     $BYPTR"
if [ "$BYPTR" != "112245" ]; then
    echo "the pointer call returned '$BYPTR' where the C object returns 112245" >&2
    exit 1
fi
if [ "$BYVAL" = "$BYPTR" ]; then
    echo "the by-value call returned '$BYVAL' too: the trap did not reproduce" >&2
    exit 1
fi

# ---- the fixed shape, end to end -------------------------------------------
#
# The Kelp program on its own, against the compiler's own alloc body: the buffer
# is zero-filled, so the program prints an empty line.
env -u KELPC_BACKEND -u KELPC_ORACLE ./target/release/kelpc build \
    kelpc/tests/abi_aggregate.kp -o "$T/abi_prog" >/dev/null
if [ "$("$T/abi_prog")" != "" ]; then
    echo "the compiler's own alloc body printed something where it zero-fills" >&2
    exit 1
fi

RT="$(find target/release/build -name libkelp_rt.a | head -n 1)"
if [ -z "$RT" ]; then
    echo "no libkelp_rt.a under target/release/build: run this inside the project shell" >&2
    exit 1
fi

# Relink the same Kelp object against the genuine C object, which defines the
# extern body with the pointer-spelled ABI. The compiler's generated
# `abi_prog.externs.o` is deliberately left out, so the C object is the only
# definition of the body.
"$CC" -O2 -o "$T/abi_prog_ours" "$T/abi_prog.o" "$T/abi.o" "$RT" -lm -lpthread
GOT="$("$T/abi_prog_ours")"
echo "the C object's 24-byte return, printed by the Kelp program: '$GOT'"
if [ "$GOT" != "ZY" ]; then
    echo "the Kelp program printed '$GOT' where the C object's buffer holds 'ZY'" >&2
    exit 1
fi

# ---- the backend's own declaration, read back ------------------------------
IR="$T/abi_prog.ll"
if ! grep -qxF 'declare void @kelp_std__mem__alloc_raw(ptr, i64)' "$IR"; then
    echo "'$IR' does not spell the extern's 24-byte return as a hidden pointer" >&2
    exit 1
fi
if grep -qE '^declare [^(]*%kelp_(Bytes|String)[^A-Za-z0-9_]' "$IR" \
    || grep -qE '^declare [^(]*\([^)]*%kelp_(Bytes|String)[^A-Za-z0-9_]' "$IR"; then
    echo "'$IR' names a 24-byte aggregate in a C-boundary declaration" >&2
    exit 1
fi

echo "BUG-vN4tR gate passed: a 24-byte aggregate passes the C object through a pointer (the by-value call disagrees), the Kelp program reads back the C object's 'ZY' through the hidden-pointer return, and the emitted declarations spell no 24-byte aggregate by value"
