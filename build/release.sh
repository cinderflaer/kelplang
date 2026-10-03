#!/bin/sh
# Reproducible release build of kelpc.
#
#   build/release.sh            builds dist/kelpc-<version>-linux-x86_64,
#                               dist/SHA256SUMS and dist/BUILDINFO
#
# Run it inside the development shell (cargo, rustc, llvm-config, cc, git). The
# same commit built with the same toolchain gives the same bytes no matter which
# directory it is built in; `build/verify-reproducible.sh` checks exactly that.
# See plan/reproducible-builds.md for what is pinned and why.
#
# Environment (all optional):
#   SOURCE_DATE_EPOCH   build timestamp; default: the last commit's time
#   KELPC_COMMIT        commit hash to bake in; default: git rev-parse HEAD
#   CARGO_TARGET_DIR    where cargo builds; default: <repo>/target-release
#   DIST_DIR            where the artifacts go; default: <repo>/dist
set -eu

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/.." && pwd)
cd "$root"

if [ -z "${SOURCE_DATE_EPOCH:-}" ]; then
    SOURCE_DATE_EPOCH=$(git log -1 --format=%ct) || {
        echo "release.sh: no git history and SOURCE_DATE_EPOCH is not set" >&2
        exit 1
    }
fi
if [ -z "${KELPC_COMMIT:-}" ]; then
    KELPC_COMMIT=$(git rev-parse HEAD) || {
        echo "release.sh: no git history and KELPC_COMMIT is not set" >&2
        exit 1
    }
fi
export SOURCE_DATE_EPOCH KELPC_COMMIT

crate_version=$(sed -n 's/^version = "\(.*\)"$/\1/p' kelpc/Cargo.toml | head -n 1)
# 26.9.5 -> 26.09.5, the padded release name `kelpc version` prints.
version=$(echo "$crate_version" | awk -F. '{ printf "%s.%02d.%s", $1, $2, $3 }')

cargo_home=${CARGO_HOME:-$HOME/.cargo}
target=${CARGO_TARGET_DIR:-$root/target-release}
dist=${DIST_DIR:-$root/dist}
export CARGO_TARGET_DIR="$target"
export CARGO_INCREMENTAL=0
# Cargo reads the profile from the manifest; strip file-system debris and keep
# the optimiser deterministic with a single codegen unit.
export CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1
export CARGO_PROFILE_RELEASE_STRIP=debuginfo

sysroot=$(rustc --print sysroot)
rustflags="--remap-path-prefix=$root=/kelp-src"
rustflags="$rustflags --remap-path-prefix=$cargo_home=/cargo-home"
rustflags="$rustflags --remap-path-prefix=$sysroot=/rust-sysroot"
rustflags="$rustflags -C link-arg=-Wl,--build-id=none"
# RUSTFLAGS is split on spaces by cargo, so none of the paths may contain one.
case "$root$cargo_home$sysroot" in
    *" "*) echo "release.sh: paths with spaces are not supported" >&2; exit 1 ;;
esac
export RUSTFLAGS="$rustflags"

locked="--locked"
if [ -d "$cargo_home/registry/cache" ] || [ -d "$root/vendor" ]; then
    locked="--locked --frozen"
fi

echo "release.sh: building kelpc $version ($KELPC_COMMIT) epoch $SOURCE_DATE_EPOCH" >&2
# shellcheck disable=SC2086
cargo build --release $locked --manifest-path kelpc/Cargo.toml

mkdir -p "$dist"
asset="kelpc-$version-linux-x86_64"
cp "$target/release/kelpc" "$dist/$asset"
chmod 755 "$dist/$asset"
(cd "$dist" && sha256sum "$asset" > SHA256SUMS)

llvm_config=llvm-config
if [ -n "${LLVM_SYS_211_PREFIX:-}" ] && [ -x "$LLVM_SYS_211_PREFIX/bin/llvm-config" ]; then
    llvm_config="$LLVM_SYS_211_PREFIX/bin/llvm-config"
fi
llvm_version=$("$llvm_config" --version)
{
    echo "version: $version"
    echo "commit: $KELPC_COMMIT"
    echo "source_date_epoch: $SOURCE_DATE_EPOCH"
    echo "rustc: $(rustc -V)"
    echo "cargo: $(cargo -V)"
    echo "llvm_version: $llvm_version"
    echo "llvm_major: ${llvm_version%%.*}"
    echo "profile: release (codegen-units=1, strip=debuginfo, incremental off)"
    echo "cargo_args: build --release $locked --manifest-path kelpc/Cargo.toml"
    echo "RUSTFLAGS: $RUSTFLAGS"
    echo "CARGO_INCREMENTAL: $CARGO_INCREMENTAL"
    echo "CARGO_PROFILE_RELEASE_CODEGEN_UNITS: $CARGO_PROFILE_RELEASE_CODEGEN_UNITS"
    echo "CARGO_PROFILE_RELEASE_STRIP: $CARGO_PROFILE_RELEASE_STRIP"
    echo "sha256: $(cut -d' ' -f1 "$dist/SHA256SUMS")"
} > "$dist/BUILDINFO"
echo "release.sh: wrote $dist/$asset" >&2
