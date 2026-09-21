# The development shell `kelpc` is built in.
#
# The system already carries LLVM's runtime output, which is what a built `kelpc`
# links against (`libLLVM-21.so`). `llvm-sys` also needs the *dev* output: the
# `llvm-config` program, which reports the version and the library paths, and
# `include/llvm-c`, the C API headers the wrapper at `kelpc/src/llvm_ffi.rs`
# declares against. The system config installs the runtime output only, so this
# shell is where the build half comes from.
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
  packages = [ llvm.dev llvm.lib ];

  LLVM_SYS_211_PREFIX = llvmPrefix;
}
