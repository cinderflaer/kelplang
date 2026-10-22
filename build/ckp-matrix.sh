#!/bin/sh
# Portable .ckp matrix: build the test packages in the baseline container, then run them in a fresh
# container of each release-matrix distro.
#
#   build/ckp-matrix.sh KELP_TREE CORAL_TREE OUTDIR
#
#   KELP_TREE    a kelp source tree (the kelpc to build, and the release-packages functions)
#   CORAL_TREE   a coral source tree (the coral that builds and runs the packages)
#   OUTDIR       where the packages and the PASS/FAIL log go; must be outside both trees
#
# Phases (see the portable-ckp plan, section 4):
#   build  ubuntu-noble (the baseline) builds kelpc, then coral with kelpc, then packages hello,
#          tlsapp (std/tls, bundles libssl), extapp ([extern] .so and a resource), and the
#          negative archives. debian-trixie, fedora and arch build nothing but kelpc and
#          coral here; fedora also packages hello, which the "built newer, runs older" check runs.
#   run    each of the four targets runs in a fresh distro image with only the coral binary and the
#          archives in it. The image must have no kelpc, no LLVM and no cc. Each check prints one
#          PASS or FAIL line; the script exits 1 when any check fails.
#
# The script writes nothing into the source trees. Only OUTDIR is written. It uses docker, as
# build/release-packages.sh does (DOCKER selects the binary, default docker).
set -eu

die() { echo "ckp-matrix.sh: $*" >&2; exit 1; }

DOCKER=${DOCKER:-docker}
TARGETS="debian-trixie ubuntu-noble fedora arch"
VERSION=${CKP_VERSION:-26.10.6}

image_for() {
    case $1 in
        debian-trixie) echo debian:trixie-slim ;;
        ubuntu-noble) echo ubuntu:24.04 ;;
        fedora) echo fedora:latest ;;
        arch) echo archlinux:latest ;;
        *) die "unknown target $1" ;;
    esac
}

# ---------------------------------------------------------------- the test programs

# The three projects and the two negative archives are written inside the baseline
# container, so the repository carries no test artifacts.
write_projects() {
    rm -rf /work/proj && mkdir -p /work/proj
    cd /work/proj

    mkdir -p hello/src
    printf 'name = "hello"\nversion = "%s"\nentry = "src/main.kp"\n' "$VERSION" > hello/coral.toml
    printf 'fun main() {\n    print("hello from ckp")\n}\n' > hello/src/main.kp

    mkdir -p tlsapp/src
    printf 'name = "tlsapp"\nversion = "%s"\nentry = "src/main.kp"\n' "$VERSION" > tlsapp/coral.toml
    cp /src/kelp/kelpc/tests/fixtures/tls_ca_file.kp tlsapp/src/main.kp

    mkdir -p extapp/src extapp/native extapp/data
    cat > extapp/coral.toml <<TOML
name = "extapp"
version = "$VERSION"
entry = "src/main.kp"

[package]
resources = ["data/*.txt"]

[extern."meas"]
domain = "process"
library = "meas"
library_dirs = ["native"]
symbols = ["meas_add"]
TOML
    printf 'pub extern fun meas_add(a: i64, b: i64) => i64\n' > extapp/src/meas.kp
    cat > extapp/src/main.kp <<KP
impl "meas"
impl "std/fmt"
impl "std/str"

fun main() {
    val sum = meas.meas_add(2, 3)
    val text = fmt.int(sum)
    print(text.as_str())
}
KP
    printf 'greeting\n' > extapp/data/greeting.txt
    printf 'long meas_add(long a, long b) { return a + b; }\n' > meas.c
    cc -shared -fPIC -o extapp/native/libmeas.so meas.c
}

# Two negative archives, made from the hello archive in $1: a copy whose manifest says
# glibc 2.99 (the manifest is not hashed, so the copy is well formed, with a requirement no
# host meets), written to $2; and a copy with one byte changed, written to $3.
write_negatives() {
    python3 - "$1" "$2" "$3" <<'PY'
import sys, zipfile
src, glibc_out, flipped_out = sys.argv[1], sys.argv[2], sys.argv[3]
with zipfile.ZipFile(src) as zin, zipfile.ZipFile(glibc_out, "w", zipfile.ZIP_STORED) as zout:
    for info in zin.infolist():
        data = zin.read(info.filename)
        if info.filename == "manifest.toml":
            lines = data.decode().split("\n")
            assert any(l.startswith('glibc = "') for l in lines)
            lines = ['glibc = "2.99"' if l.startswith('glibc = "') else l for l in lines]
            data = "\n".join(lines).encode()
        fresh = zipfile.ZipInfo(info.filename, date_time=info.date_time)
        fresh.compress_type = zipfile.ZIP_STORED
        zout.writestr(fresh, data)
b = bytearray(open(src, "rb").read())
b[len(b) // 3] ^= 0x55
open(flipped_out, "wb").write(bytes(b))
PY
}

# ---------------------------------------------------------------- in the baseline container

container_setup() {
    case $1 in
        debian-trixie) FAMILY=deb; container_setup_debian trixie apt ;;
        ubuntu-noble) FAMILY=deb; container_setup_debian noble rustup ;;
        fedora) FAMILY=rpm; container_setup_fedora ;;
        arch) FAMILY=arch; container_setup_arch ;;
        *) die "unknown target $1" ;;
    esac
}

in_build() {
    TARGET=$1
    rm -rf /work && mkdir -p /work/kelp /work/coral /work/out "/out/$TARGET"
    tar -C /src/kelp --exclude=./target --exclude=./.git -cf - . | tar -x -C /work/kelp
    tar -C /src/coral --exclude=./target --exclude=./.git --exclude=./build -cf - . | tar -x -C /work/coral
    sed -n '1,/^# -* the host/p' /work/kelp/build/release-packages.sh > /work/rp-funcs.sh
    . /work/rp-funcs.sh
    export KELPC_COMMIT=ckp-matrix SOURCE_DATE_EPOCH=0
    container_setup "$TARGET"
    case $FAMILY in
        deb) apt-get install -y -qq python3 >/dev/null ;;
        rpm) dnf install -y -q python3 >/dev/null ;;
        arch) pacman -S --noconfirm --needed python >/dev/null ;;
    esac
    export PATH="$LLVM_PREFIX/bin:$PATH"
    export LD_LIBRARY_PATH="$LLVM_PREFIX/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
    build_kelpc /work/kelp /work/kelpc-bin
    install -m 0755 /work/kelpc-bin /usr/local/bin/kelpc
    export KELPC=/usr/local/bin/kelpc
    export KELPC_LLD
    KELPC_LLD=$(command -v ld.lld)

    cd /work/coral
    kelpc build src/main.kp -o /work/coral-bin
    install -m 0755 /work/coral-bin /out/"$TARGET"/coral

    write_projects
    for p in hello tlsapp extapp; do
        (cd "/work/proj/$p" && /work/coral-bin build --package)
        cp /work/proj/"$p"/build/"$p"-"$VERSION"-linux-x86_64.ckp "/out/$TARGET/$p.ckp"
    done
    write_negatives "/out/$TARGET/hello.ckp" "/out/$TARGET/hello-glibc299.ckp" "/out/$TARGET/hello-flipped.ckp"
    kelpc version > "/out/$TARGET/BUILDINFO.txt"
    echo "build $TARGET: done"
}

# ---------------------------------------------------------------- in a fresh run container

PASS_COUNT=0
FAIL_COUNT=0
pass() { PASS_COUNT=$((PASS_COUNT + 1)); echo "PASS: $*"; }
fail() { FAIL_COUNT=$((FAIL_COUNT + 1)); echo "FAIL: $*"; }

# Expect the command to exit with $1 and print the line $2 on stdout (empty $2 means nothing checked).
expect_run() {
    want_code=$1; want_out=$2; shift 2
    set +e
    got=$("$@" 2>/tmp/ckp-stderr.txt)
    code=$?
    set -e
    if [ "$code" = "$want_code" ] && { [ -z "$want_out" ] || printf '%s\n' "$got" | grep -qxF "$want_out"; }; then
        pass "$* -> exit $code, output '$want_out'"
    else
        fail "$* -> exit $code (wanted $want_code), output: $got, stderr: $(cat /tmp/ckp-stderr.txt)"
    fi
}

in_run() {
    TARGET=$1
    if command -v kelpc >/dev/null 2>&1; then fail "kelpc is on PATH in the $TARGET run image"; else pass "no kelpc in the $TARGET run image"; fi
    if command -v cc >/dev/null 2>&1; then fail "cc is on PATH in the $TARGET run image"; else pass "no cc in the $TARGET run image"; fi
    install -m 0755 /pkg/coral /usr/local/bin/coral
    mkdir -p /tmp/ckp-run && cd /tmp/ckp-run
    export XDG_CACHE_HOME=/tmp/ckp-run/cache

    expect_run 0 "hello from ckp" coral run /pkg/hello.ckp
    expect_run 0 "5" coral run /pkg/extapp.ckp
    # tlsapp connects to port 1, which is closed, so its exit code is its own; the check is that libssl loads from the package.
    coral run /pkg/tlsapp.ckp -- 1 /pkg/hello.ckp >/dev/null 2>&1 || true

    entry=$(find "$XDG_CACHE_HOME/coral/ckp" -mindepth 2 -maxdepth 2 -type f -name hello 2>/dev/null | head -n 1)
    if [ -n "$entry" ] && ! ldd "$entry" | grep -q 'not found'; then
        pass "ldd of the unpacked hello has no 'not found' line"
    else
        fail "ldd of the unpacked hello: ${entry:-no unpacked hello}"
    fi
    ext_entry=$(find "$XDG_CACHE_HOME/coral/ckp" -mindepth 2 -maxdepth 2 -type f -name extapp 2>/dev/null | head -n 1)
    if [ -n "$ext_entry" ] && ! ldd "$ext_entry" | grep -q 'not found'; then
        pass "ldd of the unpacked extapp has no 'not found' line"
    else
        fail "ldd of the unpacked extapp: ${ext_entry:-no unpacked extapp}"
    fi

    tls_entry=$(find "$XDG_CACHE_HOME/coral/ckp" -mindepth 2 -maxdepth 2 -type f -name tlsapp 2>/dev/null | head -n 1)
    if [ -n "$tls_entry" ] && ldd "$tls_entry" | grep -q "libssl.so.3 => $XDG_CACHE_HOME" && ! ldd "$tls_entry" | grep -q 'not found'; then
        pass "tlsapp loads libssl.so.3 from its package lib/"
    else
        fail "tlsapp libssl: ${tls_entry:-no unpacked tlsapp}"
    fi

    # The archive built on fedora (glibc 2.42 in its own container) runs here, because its references are old.
    if [ -f /pkg/fedora-hello.ckp ]; then
        expect_run 0 "hello from ckp" coral run /pkg/fedora-hello.ckp
    fi

    # Negative: a requirement no host meets is refused before anything is unpacked.
    rm -rf "$XDG_CACHE_HOME"
    set +e
    coral run /pkg/hello-glibc299.ckp >/tmp/ckp-neg.out 2>/tmp/ckp-neg.err
    code=$?
    set -e
    if [ "$code" = 1 ] && grep -q 'needs glibc 2.99' /tmp/ckp-neg.err && [ ! -e "$XDG_CACHE_HOME" ]; then
        pass "glibc 2.99 archive refused with exit 1, reason on stderr, no cache directory"
    else
        fail "glibc 2.99 archive: exit $code, stderr $(cat /tmp/ckp-neg.err), cache present: $( [ -e "$XDG_CACHE_HOME" ] && echo yes || echo no)"
    fi

    # Negative: a changed byte is refused before anything is unpacked.
    set +e
    coral run /pkg/hello-flipped.ckp >/tmp/ckp-neg.out 2>/tmp/ckp-neg.err
    code=$?
    set -e
    if [ "$code" = 1 ] && [ ! -e "$XDG_CACHE_HOME" ]; then
        pass "changed-byte archive refused with exit 1, no cache directory"
    else
        fail "changed-byte archive: exit $code, cache present: $( [ -e "$XDG_CACHE_HOME" ] && echo yes || echo no)"
    fi
    echo "run $TARGET: $PASS_COUNT passed, $FAIL_COUNT failed"
    [ "$FAIL_COUNT" = 0 ]
}

# ---------------------------------------------------------------- host side

# One container. A build mounts both source trees read-only and OUT as /out. A run mounts
# OUT/pkg (the coral binary and the archives) as /pkg, read-only.
run_in() {
    # $1 target, $2 mode (build|run), $3 image
    img=$3
    if [ "$2" = build ]; then
        "$DOCKER" run --rm \
            -v "$KELP:/src/kelp:ro" -v "$CORAL:/src/coral:ro" -v "$OUT:/out" \
            "$img" sh /src/kelp/build/ckp-matrix.sh --in-container build "$1"
    else
        "$DOCKER" run --rm \
            -v "$KELP:/src/kelp:ro" -v "$OUT/pkg:/pkg:ro" \
            "$img" sh /src/kelp/build/ckp-matrix.sh --in-container run "$1"
    fi
}

in_container_entry() {
    mode=$1; target=$2
    case $mode in
        build) in_build "$target" ;;
        run) in_run "$target" ;;
        *) die "unknown mode $mode" ;;
    esac
}

main() {
    if [ "${1:-}" = --in-container ]; then
        shift
        in_container_entry "$@"
        return
    fi
    [ $# -eq 3 ] || { echo "usage: build/ckp-matrix.sh KELP_TREE CORAL_TREE OUTDIR" >&2; exit 2; }
    KELP=$(cd "$1" && pwd)
    CORAL=$(cd "$2" && pwd)
    mkdir -p "$3"
    OUT=$(cd "$3" && pwd)
    case $OUT in "$KELP"|"$KELP"/*|"$CORAL"|"$CORAL"/*) die "OUTDIR must be outside the source trees" ;; esac
    echo "build baseline ubuntu-noble"
    run_in ubuntu-noble build "$(image_for ubuntu-noble)"
    echo "build fedora (packages hello for the newer-runs-older check)"
    run_in fedora build "$(image_for fedora)"
    rm -rf "$OUT/pkg" && mkdir -p "$OUT/pkg"
    cp "$OUT/ubuntu-noble/coral" "$OUT/ubuntu-noble/"*.ckp "$OUT/pkg/"
    cp "$OUT/fedora/hello.ckp" "$OUT/pkg/fedora-hello.ckp"
    echo "run on each target"
    status=0
    for t in $TARGETS; do
        echo "== $t"
        run_in "$t" run "$(image_for "$t")" || status=1
    done
    if [ "$status" = 0 ]; then echo "MATRIX: all PASS"; else echo "MATRIX: FAIL"; fi
    exit "$status"
}

main "$@"
