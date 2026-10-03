#!/bin/sh
# Installs a downloaded kelpc release binary (linux x86_64 only).
#
#   install.sh --from <file> (--sha256 <hex> | --sums <SHA256SUMS>) [--prefix <dir>]
#   install.sh --from <file> --insecure-skip-verify [--prefix <dir>]
#   install.sh --uninstall [--prefix <dir>]
#
# This script never downloads anything and never uses sudo. Fetch the binary
# (kelpc-<version>-linux-x86_64) and SHA256SUMS yourself from the Kelp release
# page, then point --from and --sums at them: that way the script cannot be
# tricked into fetching and running unverified code.
#
# The default prefix is ~/.local; the binary goes to <prefix>/bin/kelpc and a
# manifest of what was installed to <prefix>/share/kelpc/manifest, which is all
# --uninstall removes.
#
# Test-only environment overrides (the release tests use them, nothing else
# should): KELP_INSTALL_PLATFORM="<uname -s> <uname -m>" fakes the platform;
# KELP_INSTALL_SKIP_CHECKS=1 skips the cc/libLLVM check and the hello-world run.
set -eu

prog=install.sh
die() { echo "$prog: error: $*" >&2; exit 1; }

usage() {
    cat >&2 <<USAGE
usage: $prog --from <file> (--sha256 <hex> | --sums <SHA256SUMS>) [--prefix <dir>]
       $prog --from <file> --insecure-skip-verify [--prefix <dir>]
       $prog --uninstall [--prefix <dir>]
Nothing is downloaded: fetch kelpc-<version>-linux-x86_64 and SHA256SUMS from
the Kelp release page first.
USAGE
    exit 2
}

prefix="${HOME:-}/.local"
from=""
sha=""
sums=""
insecure=0
uninstall=0
while [ $# -gt 0 ]; do
    case "$1" in
        --prefix) [ $# -ge 2 ] || usage; prefix=$2; shift 2 ;;
        --from) [ $# -ge 2 ] || usage; from=$2; shift 2 ;;
        --sha256) [ $# -ge 2 ] || usage; sha=$2; shift 2 ;;
        --sums) [ $# -ge 2 ] || usage; sums=$2; shift 2 ;;
        --insecure-skip-verify) insecure=1; shift ;;
        --uninstall) uninstall=1; shift ;;
        -h|--help) usage ;;
        *) echo "$prog: unknown argument: $1" >&2; usage ;;
    esac
done
[ -n "$prefix" ] || die "no --prefix given and HOME is not set"

# (a) platform
platform=${KELP_INSTALL_PLATFORM:-"$(uname -s) $(uname -m)"}
if [ "$platform" != "Linux x86_64" ]; then
    die "unsupported platform '$platform': kelpc releases exist only for Linux x86_64. Build from source instead (see the repository README)."
fi

bindir="$prefix/bin"
sharedir="$prefix/share/kelpc"
manifest="$sharedir/manifest"

# --uninstall removes only what the manifest lists.
if [ "$uninstall" = 1 ]; then
    if [ ! -f "$manifest" ]; then
        die "nothing to uninstall: $manifest does not exist (was kelpc installed with this script into this prefix?)"
    fi
    while IFS= read -r f; do
        case "$f" in
            "$prefix"/*) rm -f -- "$f"; echo "removed $f" ;;
            *) echo "$prog: ignoring manifest entry outside the prefix: $f" >&2 ;;
        esac
    done < "$manifest"
    rm -f -- "$manifest"
    rmdir "$sharedir" 2>/dev/null || true
    rmdir "$bindir" "$prefix/share" 2>/dev/null || true
    echo "kelpc uninstalled from $prefix"
    exit 0
fi

[ -n "$from" ] || { echo "$prog: no --from <file> given; this script does not download. Fetch kelpc-<version>-linux-x86_64 and SHA256SUMS from the Kelp release page first." >&2; usage; }
[ -f "$from" ] || die "no such file: $from"

# (b) runtime dependencies
if [ "${KELP_INSTALL_SKIP_CHECKS:-0}" != 1 ]; then
    missing=""
    command -v cc >/dev/null 2>&1 || missing="$missing cc"
    have_llvm=0
    if command -v ldconfig >/dev/null 2>&1 && ldconfig -p 2>/dev/null | grep -q 'libLLVM-21\.so'; then
        have_llvm=1
    else
        for d in /usr/lib /usr/lib64 /usr/lib/x86_64-linux-gnu /lib/x86_64-linux-gnu /usr/lib/llvm-21/lib /usr/lib64/llvm21/lib \
                 /run/current-system/sw/lib /nix/store/*llvm-21*-lib/lib /nix/store/*llvm-21*/lib $(echo "${LD_LIBRARY_PATH:-}" | tr ':' ' '); do
            for f in "$d"/libLLVM-21.so*; do
                if [ -e "$f" ]; then have_llvm=1; fi
            done
        done
    fi
    [ "$have_llvm" = 1 ] || missing="$missing libLLVM-21"
    if [ -n "$missing" ]; then
        cat >&2 <<MSG
$prog: error: missing runtime dependencies:$missing
kelpc needs a C compiler (cc) and the LLVM 21 shared library (libLLVM-21.so). Get them with:
  Debian/Ubuntu: sudo apt install build-essential llvm-21      (llvm-21 is in apt.llvm.org if your release lacks it)
  Fedora:        sudo dnf install gcc llvm21-libs
  NixOS:         nix-shell -p gcc llvmPackages_21.libllvm       (or add both to environment.systemPackages)
Programs that use std/tls also need the OpenSSL runtime libraries (libssl, libcrypto).
MSG
        exit 1
    fi
fi

# (c) verification
actual=$(sha256sum "$from" | cut -d' ' -f1)
if [ -n "$sha" ] && [ -n "$sums" ]; then
    die "give either --sha256 or --sums, not both"
fi
if [ -n "$sums" ]; then
    [ -f "$sums" ] || die "no such file: $sums"
    name=$(basename "$from")
    sha=$(awk -v n="$name" '{ f=$2; sub(/^\*/, "", f); if (f == n) { print $1; exit } }' "$sums")
    [ -n "$sha" ] || die "$sums has no entry for $name (rename the file back to its release name or use --sha256)"
fi
if [ -n "$sha" ]; then
    expected=$(echo "$sha" | tr 'A-F' 'a-f')
    if [ "$expected" != "$actual" ]; then
        die "checksum mismatch for $from: expected $expected, got $actual. Nothing was installed."
    fi
    echo "checksum ok ($actual)"
elif [ "$insecure" = 1 ]; then
    echo "$prog: WARNING: installing $from without verification (sha256 $actual)" >&2
else
    die "refusing to install without verification: pass --sha256 <hex> or --sums <SHA256SUMS> (or, at your own risk, --insecure-skip-verify)."
fi

# (d) install
mkdir -p "$bindir" "$sharedir"
tmp="$bindir/.kelpc.install.$$"
cp "$from" "$tmp"
chmod 755 "$tmp"
mv -f "$tmp" "$bindir/kelpc"
printf '%s\n' "$bindir/kelpc" "$manifest" > "$manifest"
echo "installed $bindir/kelpc"

case ":$PATH:" in
    *":$bindir:"*) ;;
    *) echo "note: $bindir is not on your PATH" ;;
esac

if [ "${KELP_INSTALL_SKIP_CHECKS:-0}" = 1 ]; then
    exit 0
fi
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cat > "$work/hello.kp" <<'KP'
fun main() {
    print("hello from kelp")
}
KP
if out=$("$bindir/kelpc" run "$work/hello.kp" 2>&1) && [ "$out" = "hello from kelp" ]; then
    echo "hello-world check passed: $out"
else
    echo "$prog: error: the installed kelpc could not compile and run a hello-world program:" >&2
    echo "$out" >&2
    exit 1
fi
