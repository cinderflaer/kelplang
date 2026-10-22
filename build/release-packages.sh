#!/bin/sh
# Release packages for kelpc: one build per distro container, then one native
# package per format, a flake and a PKGBUILD, and a test install of each package
# in a fresh container of its own distro.
#
#   build/release-packages.sh VERSION [OUTDIR]
#
#   VERSION   a release name, YY.MM.N (for example 26.10.6)
#   OUTDIR    where the artifacts go; default: a fresh mktemp -d, printed at the end.
#             OUTDIR must not be inside the repository.
#
# Run it on a machine with docker and git (the build host). It writes nothing into
# the repository: it archives HEAD (git archive) into a scratch directory, sets the
# version in that copy, and builds from the copy. Only OUTDIR is written.
#
# Targets, and why each one:
#   debian-trixie   Debian 13, the stable Debian line. LLVM 21 from apt.llvm.org (the
#                   trixie archive stops at LLVM 19). Rust from the distro (1.85).
#   ubuntu-noble    Ubuntu 24.04 LTS, the most common LTS on servers. LLVM 21 from
#                   apt.llvm.org (noble). Rust from rustup: the distro rustc is 1.75,
#                   too old for edition 2024.
#   fedora          Fedora (latest). LLVM 21 from the distro llvm21 packages. Rust
#                   from the distro. It is also the RPM build host.
#   arch            Arch Linux, rolling. LLVM 21 from the extra repository (llvm21,
#                   lld21). Rust from the distro. It is also the pacman build host.
# A target that cannot build kelpc is reported and skipped; the rest still run.
#
# LLVM pins: the packages named above, recorded in BUILDINFO-<target>.txt together
# with the llvm-config, ld.lld, rustc and gcc versions each container used.
#
# Formats and what each one declares:
#   .deb   one per Debian-family target. Depends: the packages that own the libraries
#          kelpc links (ldd) plus the link tools (lld, the cc driver, crt and libc
#          files) found by the same query. dpkg resolves them on install.
#   .rpm   Fedora. Requires: the same owners as above, plus rpm's own ELF requires.
#   .pkg.tar.zst  Arch. depends=(): the same owners, pacman resolves them on install.
#   flake.nix     NixOS / nix. Builds from the release source tarball
#                 (kelpc-<VERSION>-src.tar.gz, which is also in OUTDIR), with
#                 nixpkgs' llvmPackages_21 and lld. Not in the repository.
# Every package is installed in a fresh container of its own distro with only the
# package (and apt, dnf or pacman), then runs kelpc run on hello.kp and kelpc build on
# download.kp. The nix flake is tested separately with `nix build` (see the release notes).
#
# Environment (all optional):
#   RELEASE_TARGETS   space-separated subset of the targets; default: all four
#   KELPC_SRC_URL     URL the flake fetches the source tarball from; default: the
#                     Gitea release asset for v<VERSION>
#   KELPC_PKG_TEST    0 skips the fresh-container install tests (default 1)
#   DOCKER            docker binary (default: docker)
set -eu

# ---------------------------------------------------------------- common

die() { echo "release-packages.sh: $*" >&2; exit 1; }

# The version the manifest carries: 26.10.6 -> 26.10.6, 26.09.5 -> 26.9.5 (semver
# has no leading zeros). The release name keeps the padded month.
crate_version_of() {
    printf '%s' "$1" | awk -F. '{ printf "%s.%d.%s", $1, $2 + 0, $3 }'
}

# Pads the month back for the release name, the same rule as build/release.sh.
release_name_of() {
    printf '%s' "$1" | awk -F. '{ printf "%s.%02d.%s", $1, $2, $3 }'
}

# An SRI sha256 (sha256-<base64>) for a file, which is what flake.nix expects.
sri_sha256() {
    if command -v openssl >/dev/null 2>&1; then
        printf 'sha256-%s' "$(openssl dgst -sha256 -binary "$1" | openssl base64 -A)"
    else
        printf 'sha256-%s' "$(python3 -c 'import base64,hashlib,sys; print(base64.b64encode(hashlib.sha256(open(sys.argv[1],"rb").read()).digest()).decode())' "$1")"
    fi
}

# ---------------------------------------------------------------- in the container

# Owner package of a file, by the family of the target's package manager.
# Both the path as named and its resolved target are asked, because an alternatives
# link (cc, for one) and the package that ships its target can differ.
owner_of() {
    for q in "$1" "$(readlink -f "$1" 2>/dev/null)"; do
        [ -n "$q" ] || continue
        case $FAMILY in
            # dpkg also prints "diversion by ... from:" lines; only "name[:arch]: path" counts.
            deb) dpkg -S "$q" 2>/dev/null | awk -F': ' '/^[a-z0-9][a-z0-9.+-]*(:[a-z0-9-]+)?: / { sub(/:.*/, "", $1); print $1; exit }' ;;
            rpm) rpm -qf --qf '%{NAME}\n' "$q" 2>/dev/null | head -n 1 ;;
            arch) pacman -Qqo "$q" 2>/dev/null | head -n 1 ;;
        esac
    done | sort -u
}

# The packages kelpc needs at run time: the owners of every library the given
# executables and shared objects link, and of the tools and crt/libc files a hosted
# link reads (ld.lld, cc, crt objects, libc.so, libgcc). Prints one package name per
# line, sorted, without duplicates.
find_runtime_deps() {
    {
        for f in "$@"; do
            ldd "$f" | awk '{ for (i = 1; i <= NF; i++) if ($i ~ /^\//) print $i }'
        done
        for f in crti.o crtbeginS.o libc.so libgcc.a libgcc_s.so; do
            cc -print-file-name="$f"
        done
        command -v cc
        command -v ld.lld
    } | while read -r p; do
        [ -n "$p" ] && [ -e "$p" ] && owner_of "$p"
    done | sort -u | grep -v '^$' || true
}

# Depends for dpkg, rpm and pacman come from the same list.
container_setup_debian() {
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq
    apt-get install -y -qq ca-certificates curl gnupg build-essential pkg-config \
        libssl-dev dpkg-dev binutils file tar gzip >/dev/null
    curl -fsSL https://apt.llvm.org/llvm-snapshot.gpg.key | gpg --dearmor -o /usr/share/keyrings/llvm-snapshot.gpg
    echo "deb [signed-by=/usr/share/keyrings/llvm-snapshot.gpg] https://apt.llvm.org/$1/ llvm-toolchain-$1-21 main" \
        > /etc/apt/sources.list.d/llvm-21.list
    apt-get update -qq
    apt-get install -y -qq llvm-21-dev lld-21 >/dev/null
    LLVM_PREFIX=/usr/lib/llvm-21
    if [ "$2" = rustup ]; then
        install_rustup
    else
        apt-get install -y -qq rustc cargo >/dev/null
    fi
}

install_rustup() {
    curl -fsSL https://sh.rustup.rs | sh -s -- -y -q --profile minimal --default-toolchain stable
    PATH="$HOME/.cargo/bin:$PATH"
    export PATH
}

container_setup_fedora() {
    dnf install -y -q gcc gcc-c++ make pkgconf-pkg-config openssl-devel llvm21-devel \
        lld21 cargo rust tar gzip rpm-build binutils file findutils >/dev/null
    LLVM_PREFIX=/usr/lib64/llvm21
}

container_setup_arch() {
    pacman -Syu --noconfirm --needed base-devel llvm21 lld21 rust openssl pkgconf tar gzip >/dev/null
    id builder >/dev/null 2>&1 || useradd -m builder
    LLVM_PREFIX=/usr/lib/llvm21
}

# Builds kelpc from the source tree in $1 with the target's LLVM 21, runs the hello
# fixture, and writes the binary to $2.
build_kelpc() {
    src=$1
    export LLVM_SYS_211_PREFIX=$LLVM_PREFIX
    export PATH="$LLVM_PREFIX/bin:$PATH"
    export KELPC_COMMIT=${KELPC_COMMIT:?}
    export SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:?}
    export CARGO_TARGET_DIR=/work/target
    export CARGO_INCREMENTAL=0
    v=$(llvm-config --version)
    case $v in 21.*) ;; *) die "llvm-config reports $v, not LLVM 21" ;; esac
    cd "$src"
    cargo build --release --locked --manifest-path kelpc/Cargo.toml
    cp /work/target/release/kelpc "$2"
    chmod 755 "$2"
}

# The hello fixture in a clean scratch directory: kelpc run must print the greeting.
smoke_run() {
    rm -rf /work/run && mkdir -p /work/run && cd /work/run
    "$1" version
    "$1" run "$2" > out.txt
    grep -qx 'hello, world' out.txt || die "hello.kp did not print hello, world"
    echo "smoke: kelpc run printed hello, world"
}

# The installed layout, the same for every format:
#   /usr/lib/kelpc/kelpc          the compiler binary (it embeds the collector archive)
#   /usr/bin/kelpc                a launcher that names this distro's ld.lld
# ld.lld is needed for every hosted link. kelpc looks for it in KELPC_LLD, then on PATH.
# Debian-family targets bundle LLVM 21 (libLLVM.so and ld.lld) under /usr/lib/kelpc,
# because LLVM 21 is only in apt.llvm.org and a package cannot depend on a repository
# the install does not add. The launcher then sets LD_LIBRARY_PATH. Other targets
# depend on the distro's llvm21 packages.
stage_root() {
    llvm_lib=$1
    lld_bin=$2
    rm -rf /work/root && mkdir -p /work/root/usr/bin /work/root/usr/lib/kelpc
    install -m 0755 /work/kelpc-bin /work/root/usr/lib/kelpc/kelpc
    if [ "$BUNDLE_LLVM" = 1 ]; then
        mkdir -p /work/root/usr/lib/kelpc/lib /work/root/usr/lib/kelpc/bin
        cp -L "$llvm_lib" "/work/root/usr/lib/kelpc/lib/$(basename "$llvm_lib")"
        cp -L "$lld_bin" /work/root/usr/lib/kelpc/bin/ld.lld
        chmod 0755 /work/root/usr/lib/kelpc/bin/ld.lld
        lld=/usr/lib/kelpc/bin/ld.lld
        libpath='export LD_LIBRARY_PATH=/usr/lib/kelpc/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}'
    else
        lld=$lld_bin  # keep the symlink: lld picks its flavour from argv[0]
        libpath=''
    fi
    cat > /work/root/usr/bin/kelpc <<WRAP
#!/bin/sh
# kelpc launcher (installed by the kelpc package).
KELPC_LLD=\${KELPC_LLD:-$lld}
export KELPC_LLD
$libpath
exec /usr/lib/kelpc/kelpc "\$@"
WRAP
    chmod 0755 /work/root/usr/bin/kelpc
    tar -C /work/root -czf "/work/kelpc-$VERSION-root.tar.gz" usr
    # Install the same tree into this build container, so the smoke run uses the launcher.
    cp -a /work/root/. /
}

write_buildinfo() {
    {
        echo "target: $TARGET"
        echo "version: $VERSION"
        echo "commit: $KELPC_COMMIT"
        echo "source_date_epoch: $SOURCE_DATE_EPOCH"
        echo "image: $(cat /etc/os-release | sed -n 's/^PRETTY_NAME=//p' | tr -d '"')"
        echo "llvm_version: $(llvm-config --version)"
        echo "llvm_prefix: $LLVM_PREFIX"
        echo "ld.lld: $(ld.lld --version 2>&1 | head -n 1)"
        echo "rustc: $(rustc -V)"
        echo "cargo: $(cargo -V)"
        echo "gcc: $(cc --version | head -n 1)"
        echo "linked_libraries:"
        ldd /usr/bin/kelpc 2>/dev/null || ldd /work/kelpc-bin
    } > "/out/BUILDINFO-$TARGET.txt"
}

# The Depends and Requires lists, one owner per line.
deps_lines() {
    cat "$1"
}

package_deb() {
    codename=$1
    pkg=/work/pkg/kelpc
    rm -rf "$pkg" && mkdir -p "$pkg/DEBIAN" "$pkg/usr/bin"
    cp -a /work/root/. "$pkg/"
    # cc is an alternatives link no package owns, so its driver package is named here.
    depends=$({ echo gcc; deps_lines /work/deps.txt; } | sort -u | paste -sd, - | sed 's/,/, /g')
    deb_version="$VERSION-1+$codename"
    cat > "$pkg/DEBIAN/control" <<CONTROL
Package: kelpc
Version: $deb_version
Architecture: amd64
Maintainer: Lilly <lilly@rift.dev>
Depends: $depends
Section: devel
Priority: optional
Homepage: https://homelabone.xenoo.dev/gittea/Lilly/kelp
License: MIT
Description: Kelp compiler
 kelpc compiles and runs Kelp (.kp) programs. It is built on $codename against
 LLVM 21 (libllvm21) and links with the lld-21 ld.lld.
CONTROL
    dpkg-deb --build --root-owner-group "$pkg" "/out/kelpc_${deb_version}_amd64.deb"
}

package_rpm() {
    top=/work/rpm
    rm -rf "$top" && mkdir -p "$top/BUILD" "$top/RPMS" "$top/SOURCES" "$top/SPECS" "$top/SRPMS" "$top/BUILDROOT"
    cp -a /work/root "$top/SOURCES/root"
    requires=$(deps_lines /work/deps.txt | sed 's/^/Requires: /')
    cat > "$top/SPECS/kelpc.spec" <<SPEC
Name: kelpc
Version: $VERSION
Release: 1%{?dist}
Summary: Kelp compiler
License: MIT
URL: https://homelabone.xenoo.dev/gittea/Lilly/kelp
AutoReqProv: yes
$requires

%description
kelpc compiles and runs Kelp (.kp) programs. It is built against LLVM 21 and
links with the lld21 ld.lld.

%install
cp -a %{_sourcedir}/root/usr %{buildroot}/

%files
%{_bindir}/kelpc
%{_prefix}/lib/kelpc/kelpc
SPEC
    rpmbuild --define "_topdir $top" -bb "$top/SPECS/kelpc.spec" >/dev/null
    cp "$top"/RPMS/x86_64/kelpc-*.rpm /out/
}

package_arch() {
    dir=/work/arch
    rm -rf "$dir" && mkdir -p "$dir"
    cp "/work/kelpc-$VERSION-root.tar.gz" "$dir/"
    depends=$(deps_lines /work/deps.txt | sed "s/.*/'&'/" | paste -sd' ' -)
    sum=$(sha256sum "$dir/kelpc-$VERSION-root.tar.gz" | cut -d' ' -f1)
    cat > "$dir/PKGBUILD" <<PKGBUILD
# Maintainer: Lilly <lilly@rift.dev>
pkgname=kelpc
pkgver=$VERSION
pkgrel=1
pkgdesc='Kelp compiler (kelpc), built against LLVM 21'
arch=('x86_64')
url='https://homelabone.xenoo.dev/gittea/Lilly/kelp'
license=('MIT')
depends=($depends)
options=('!strip' '!debug')
source=("kelpc-\$pkgver-root.tar.gz")
sha256sums=('$sum')

package() {
    tar -xzf "\$srcdir/kelpc-\$pkgver-root.tar.gz" -C "\$pkgdir"
}
PKGBUILD
    chown -R builder "$dir"
    su builder -c "cd $dir && makepkg -f --noconfirm"
    cp "$dir"/kelpc-*.pkg.tar.zst /out/
    cp "$dir/PKGBUILD" /out/PKGBUILD
}

container_build() {
    TARGET=$1
    VERSION=$2
    export KELPC_COMMIT=$3 SOURCE_DATE_EPOCH=$4
    src=/work/kelpc-$VERSION
    BUNDLE_LLVM=0
    case $TARGET in
        debian-trixie) FAMILY=deb; BUNDLE_LLVM=1; container_setup_debian trixie apt ;;
        ubuntu-noble) FAMILY=deb; BUNDLE_LLVM=1; container_setup_debian noble rustup ;;
        fedora) FAMILY=rpm; container_setup_fedora ;;
        arch) FAMILY=arch; container_setup_arch ;;
        *) die "unknown target $TARGET" ;;
    esac
    export PATH="$LLVM_PREFIX/bin:$PATH"
    build_kelpc "$src" /work/kelpc-bin
    llvm_lib=$(ldd /work/kelpc-bin | awk '/libLLVM/ { print $3; exit }')
    [ -f "$llvm_lib" ] || die "libLLVM not found for kelpc"
    lld_bin=$(command -v ld.lld)
    stage_root "$llvm_lib" "$lld_bin"
    smoke_run /usr/bin/kelpc "$src/kelpc/tests/fixtures/examples/hello.kp"
    write_buildinfo
    find_runtime_deps /work/kelpc-bin "$lld_bin" "$llvm_lib" > /work/deps.txt
    if [ "$BUNDLE_LLVM" = 1 ]; then
        # The bundled copies replace the distro packages that would own them.
        { owner_of "$llvm_lib"; owner_of "$lld_bin"; } | grep -v '^$' | sort -u > /work/drop.txt || true
        grep -vxF -f /work/drop.txt /work/deps.txt > /work/deps.new || true
        mv /work/deps.new /work/deps.txt
    fi
    # A program that uses TLS links with -lssl and -lcrypto, which need the unversioned
    # libssl.so and libcrypto.so. Those belong to the development package, not to a
    # runtime library that ldd on kelpc reports, so each family names its owner here.
    case $FAMILY in
        deb) echo libssl-dev ;;
        rpm) echo openssl-devel ;;
        arch) echo openssl ;;
    esac >> /work/deps.txt
    sort -u /work/deps.txt -o /work/deps.txt
    echo "deps for $TARGET:" && cat /work/deps.txt
    case $TARGET in
        debian-trixie) package_deb trixie ;;
        ubuntu-noble) package_deb noble ;;
        fedora) package_rpm ;;
        arch) package_arch ;;
    esac
    echo "container build $TARGET: done"
}

# ---------------------------------------------------------------- the host

usage() {
    cat >&2 <<USAGE
usage: build/release-packages.sh VERSION [OUTDIR]
       VERSION like 26.10.6; OUTDIR must be outside the repository.
USAGE
}

# Sets the crate version in the scratch copy only (kelpc/Cargo.toml and the kelpc
# entry of Cargo.lock), so the build is of VERSION without touching the repository.
stamp_version() {
    tree=$1
    crate=$2
    awk -v v="$crate" '
        !done && /^version = "/ { print "version = \"" v "\""; done = 1; next }
        { print }' "$tree/kelpc/Cargo.toml" > "$tree/kelpc/Cargo.toml.new"
    mv "$tree/kelpc/Cargo.toml.new" "$tree/kelpc/Cargo.toml"
    awk -v v="$crate" '
        /^name = "kelpc"$/ { inpkg = 1; print; next }
        inpkg && /^version = "/ { print "version = \"" v "\""; inpkg = 0; next }
        { print }' "$tree/Cargo.lock" > "$tree/Cargo.lock.new"
    mv "$tree/Cargo.lock.new" "$tree/Cargo.lock"
}

image_for() {
    case $1 in
        debian-trixie) echo debian:trixie-slim ;;
        ubuntu-noble) echo ubuntu:24.04 ;;
        fedora) echo fedora:latest ;;
        arch) echo archlinux:latest ;;
        *) die "unknown target $1 (debian-trixie ubuntu-noble fedora arch)" ;;
    esac
}

# Builds one target in its container. The container gets the source tarball
# read-only and OUTDIR as /out; it writes its packages and BUILDINFO there.
build_target() {
    t=$1
    img=$(image_for "$t")
    uid=$(id -u)
    gid=$(id -g)
    "$DOCKER" run --rm \
        -v "$OUT:/out" \
        -v "$OUT/kelpc-$VERSION-src.tar.gz:/in/src.tar.gz:ro" \
        "$img" sh -c "set -e
            mkdir -p /work && tar -xzf /in/src.tar.gz -C /work
            rc=0
            sh /work/kelpc-$VERSION/build/release-packages.sh --in-container $t $VERSION $COMMIT $EPOCH || rc=\$?
            chown -R $uid:$gid /out
            exit \$rc"
}

# A test: install the package in a fresh container of its own distro, then run
# kelpc on the hello and download fixtures from the source tarball. The install must
# pull in the dependencies by itself, so the command has no extra apt, dnf or pacman steps.
hello_check() {
    cat <<CHECK
mkdir -p /t && cd /t && tar -xzf /out/kelpc-$VERSION-src.tar.gz kelpc-$VERSION/kelpc/tests/fixtures/examples/hello.kp kelpc-$VERSION/kelpc/tests/fixtures/examples/download.kp
kelpc version && kelpc run kelpc-$VERSION/kelpc/tests/fixtures/examples/hello.kp > /t/out.txt
grep -qx 'hello, world' /t/out.txt && echo "PASS: hello.kp printed hello, world"
kelpc build kelpc-$VERSION/kelpc/tests/fixtures/examples/download.kp -o /t/download && test -x /t/download && echo "PASS: download.kp built"
CHECK
}

test_package() {
    f=$1
    case $f in
        *.deb)
            case $f in *+noble*) img=ubuntu:24.04 ;; *) img=debian:trixie-slim ;; esac
            install="export DEBIAN_FRONTEND=noninteractive; apt-get update -qq >/dev/null && apt-get install -y -qq /out/$f >/dev/null" ;;
        *.rpm)
            img=fedora:latest
            install="dnf install -y -q /out/$f >/dev/null" ;;
        *.pkg.tar.zst)
            img=archlinux:latest
            install="pacman -Sy --noconfirm >/dev/null && pacman -U --noconfirm /out/$f >/dev/null" ;;
        *) return 0 ;;
    esac
    echo "test: $f in $img"
    "$DOCKER" run --rm -v "$OUT:/out:ro" "$img" sh -c "set -e; $install; $(hello_check | tr '\n' ';')"
}

# The flake: the source tarball's URL and SRI hash, the version, and the commit
# name that goes into the version report. It lives in OUTDIR, never the repository.
write_flake() {
    url=${KELPC_SRC_URL:-https://homelabone.xenoo.dev/gittea/Lilly/kelp/releases/download/v$VERSION/kelpc-$VERSION-src.tar.gz}
    hash=$(sri_sha256 "$OUT/kelpc-$VERSION-src.tar.gz")
    cat > "$OUT/flake.nix" <<'FLAKE'
{
  description = "kelpc @VERSION@, built from the release source tarball";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};
      llvm = pkgs.llvmPackages_21.llvm;
      lld = pkgs.llvmPackages_21.lld;
      # llvm-sys wants one prefix that holds include/ and lib/ (as shell.nix does).
      llvmPrefix = pkgs.symlinkJoin {
        name = "llvm-21-kelp-prefix";
        paths = [ llvm.dev llvm.lib ];
      };
      # The hash is the tarball's own sha256 (SRI), as fetchurl checks it.
      tarball = pkgs.fetchurl {
        url = "@URL@";
        hash = "@HASH@";
      };
      src = pkgs.runCommand "kelpc-@VERSION@-src" { } ''
        mkdir -p $out
        tar -xzf ${tarball} -C $out --strip-components=1
      '';
      kelpc = pkgs.rustPlatform.buildRustPackage {
        pname = "kelpc";
        version = "@VERSION@";
        inherit src;
        cargoLock.lockFile = "${src}/Cargo.lock";
        cargoBuildFlags = [ "-p" "kelpc" ];
        doCheck = false;
        nativeBuildInputs = [ pkgs.pkg-config pkgs.makeWrapper ];
        buildInputs = [ llvm.lib pkgs.openssl ];
        LLVM_SYS_211_PREFIX = llvmPrefix;
        KELPC_COMMIT = "v@VERSION@";
        # kelpc runs ld.lld and cc for every hosted link.
        postFixup = ''
          wrapProgram $out/bin/kelpc \
            --prefix PATH : ${pkgs.lib.makeBinPath [ lld pkgs.stdenv.cc ]}
        '';
        meta = {
          description = "Kelp compiler (kelpc)";
          homepage = "https://homelabone.xenoo.dev/gittea/Lilly/kelp";
          mainProgram = "kelpc";
          platforms = [ system ];
          license = pkgs.lib.licenses.mit;
        };
      };
    in {
      packages.${system} = {
        inherit kelpc;
        default = kelpc;
      };
    };
}
FLAKE
    sed -i -e "s|@VERSION@|$VERSION|g" -e "s|@URL@|$url|g" -e "s|@HASH@|$hash|g" "$OUT/flake.nix"
}

main() {
    if [ "${1:-}" = --in-container ]; then
        shift
        [ $# -eq 4 ] || die "--in-container wants TARGET VERSION COMMIT EPOCH"
        container_build "$@"
        exit 0
    fi
    [ $# -ge 1 ] && [ $# -le 2 ] || { usage; exit 2; }
    VERSION=$1
    echo "$VERSION" | grep -Eq '^[0-9]{2}\.[0-9]{2}\.[0-9]+$' || die "VERSION must look like 26.10.6, got '$VERSION'"

    here=$(cd "$(dirname "$0")" && pwd)
    root=$(cd "$here/.." && pwd)
    command -v git >/dev/null 2>&1 || die "git is needed (it archives HEAD)"
    DOCKER=${DOCKER:-docker}
    command -v "$DOCKER" >/dev/null 2>&1 || die "$DOCKER is needed"
    KELPC_PKG_TEST=${KELPC_PKG_TEST:-1}
    TARGETS=${RELEASE_TARGETS:-"debian-trixie ubuntu-noble fedora arch"}

    OUT=${2:-}
    if [ -z "$OUT" ]; then OUT=$(mktemp -d); fi
    mkdir -p "$OUT"
    OUT=$(cd "$OUT" && pwd)
    case $OUT/ in
        "$root"/*) die "OUTDIR $OUT is inside the repository; give a directory outside it" ;;
    esac

    STAGE=$(mktemp -d)
    trap 'rm -rf "$STAGE"' EXIT INT TERM
    COMMIT=$(git -C "$root" rev-parse HEAD)
    EPOCH=$(git -C "$root" log -1 --format=%ct)
    git -C "$root" archive --format=tar --prefix="kelpc-$VERSION/" HEAD | tar -x -C "$STAGE"
    tree="$STAGE/kelpc-$VERSION"
    old=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$tree/kelpc/Cargo.toml" | head -n 1)
    crate=$(crate_version_of "$VERSION")
    stamp_version "$tree" "$crate"
    echo "release-packages.sh: $VERSION from $COMMIT (manifest $old stamped as $crate)" >&2
    tar -C "$STAGE" --sort=name --owner=0 --group=0 --numeric-owner --mtime="@$EPOCH" \
        -cf - "kelpc-$VERSION" | gzip -n > "$OUT/kelpc-$VERSION-src.tar.gz"

    built=""
    failed=""
    for t in $TARGETS; do
        echo "release-packages.sh: building $t" >&2
        if build_target "$t"; then
            built="$built $t"
        else
            failed="$failed $t(build)"
            echo "release-packages.sh: $t FAILED to build" >&2
        fi
    done

    if [ "$KELPC_PKG_TEST" = 1 ]; then
        for f in "$OUT"/kelpc_*.deb "$OUT"/kelpc-"$VERSION"-*.rpm "$OUT"/kelpc-"$VERSION"-*.pkg.tar.zst; do
            [ -e "$f" ] || continue
            name=$(basename "$f")
            if ! test_package "$name"; then
                failed="$failed $name(test)"
            fi
        done
    fi

    write_flake
    cd "$OUT"
    rm -f SHA256SUMS
    sha256sum $(ls | grep -v '^SHA256SUMS$') > SHA256SUMS
    echo "release-packages.sh: built:${built:- none}" >&2
    echo "release-packages.sh: failed:${failed:- none}" >&2
    echo "release-packages.sh: output $OUT"
    [ -z "$failed" ]
}

main "$@"
