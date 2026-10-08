# The development shell `kelpc` is built in.
#
# The system already carries LLVM's runtime output, which is what a built `kelpc`
# links against (`libLLVM-21.so`). `llvm-sys` also needs the *dev* output: the
# `llvm-config` program, which reports the version and the library paths, and
# `include/llvm-c`, the C API headers the wrapper at `kelpc/src/llvm_ffi.rs`
# declares against. The system config installs the runtime output only, so this
# shell is where the build half comes from.
#
# `std/tls`'s externs (MK5-S4b-M1, open item 37) need OpenSSL's headers the
# same way: `driver.rs` already links `-lssl -lcrypto` against whatever the
# system carries at runtime, but compiling a program that reaches
# `openssl/ssl.h` (every program does, unconditionally, per `C_HEADERS`) needs
# the *dev* output's `include/openssl` on the search path, which the bare
# system install does not provide. `pkgs.openssl` in `packages` is enough:
# `mkShell` runs every package's setup hook, the same way it does for `llvm`
# above, so this needs no explicit `NIX_CFLAGS_COMPILE`/`NIX_LDFLAGS`.
{ pkgs ? import <nixpkgs> { } }:

let
  llvm = pkgs.llvmPackages_21.llvm;

  # `llvm-sys` wants one prefix that holds both `include/` and `lib/`. Nix splits
  # those into two store paths, so join them rather than point at either.
  llvmPrefix = pkgs.symlinkJoin {
    name = "llvm-21-kelp-prefix";
    paths = [ llvm.dev llvm.lib ];
  };
in
pkgs.mkShell {
  # `pkg-config` itself, so `openssl-sys` (the `openssl` crate's test
  # dependency for MK5-S4b-M1's TLS test server) can find `openssl.pc` under
  # `pkgs.openssl.dev` the way its build script expects, rather than needing
  # `OPENSSL_DIR` set by hand.
  # `pkgs.gtk4.dev` supplies the `gtk4.pc` that a Polyp consumer's `[extern."deps/polyp/gtk"]`
  # (`pkg_config = "gtk4"`) asks for at compile time. Polyp's `lib.kp` imports the GTK backend
  # unconditionally, so any program that builds against Polyp needs the GTK headers in this shell.
  packages = [ llvm.dev llvm.lib pkgs.openssl pkgs.pkg-config pkgs.gtk4.dev ];

  LLVM_SYS_211_PREFIX = llvmPrefix;
}
