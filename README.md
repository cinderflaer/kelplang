# Kelp

Kelp is a statically typed, compiled systems language built around **capability-based security**. A module declares what it may touch, and that authority is part of the source and checked at every call that uses it. A program cannot read a file, open a socket, spawn a process or read the clock unless the module making the call was granted that resource. "What can this code do?" is answered by the first few lines of the file.

The language is deliberately small: records (`datac`) and tagged unions (`enum`) with exhaustive `when` patterns, ownership without a garbage collector for values you own, `Option` and `Result` with `if let` and `?`, closures, string interpolation, and modules. `Bytes`, `String`, `List`, `Map` and `Box` are compiler builtins. There is no `unsafe`, no exceptions and no undefined-behaviour escape hatch.

`kelpc` is the compiler. It takes `.kp` sources through lexing, preprocessing, parsing, type checking, move and borrow checking, and code generation. The standard library ships inside the binary as Kelp source, so the functions in `std/str` are reviewable Kelp, not opaque native code.

## A program

```kelp
datac Point(var x: i32, var y: i32)

enum Shape {
    Circle(val r: f64)
    Rect(val w: f64, val h: f64)
    Empty
}

fun area(s: &Shape) => f64 = when s {
    Circle(r) -> 3.14159 * r * r
    Rect(w, h) -> w * h
    Empty -> 0.0
}

fun main() {
    var p = Point(x = 3, y = 4)
    p.x = 10
    print("point ${p.x},${p.y}")

    print("area ${area(Shape.Circle(r = 2.0))}")
    print("area ${area(Shape.Rect(w = 3.0, h = 4.0))}")

    var xs: List<i32> = List.empty()
    xs.push(7)
    xs.push(35)
    print("sum ${xs.get(i = 0) + xs.get(i = 1)}")
}
```

Authority is granted where a module is imported, and nowhere else:

```kelp
impl "std/http" caps(net = ["127.0.0.1:*"])
impl "std/fs"
```

A grant names the resource as the program spells it. `net` matches the host string, not a resolved address, and `fs` matches a normalized, symlink-resolved path.

## Language at a glance

| | |
| --- | --- |
| Types | `i32` `i64` `u8` `u16` `u32` `u64` `usize` `isize` `f32` `f64` `bool` `char`, `str` (borrowed, NUL-terminated text), `View` (borrowed text with a length), `[T; N]` (fixed size, held inline) |
| Data | `datac` records with `val` and `var` fields, `enum` tagged unions, exhaustive `when`, `if let`, type parameters (`Pair<A, B>`, `Option<T>`), `const` |
| Ownership | owned builtins `Bytes`, `String`, `List<T>`, `Map<K, V>`, `Box<T>`; moves, shared and `mut` borrows, drops at end of scope |
| Errors | `Option`, `Result`, the `?` operator, `panic` and `assert` |
| Functions | `fun`, named arguments, function literals and closures, receiver methods |
| Modules | `impl "std/fs"`, local file imports, `alias _name = <text>` (references are `_name`), `pd` passdowns (values set before `main` runs) |
| Capabilities | `fs`, `net`, `process`, `env`, `time`, `rand` |
| Standard library | `mem` `str` `path` `math` `fmt` `io` `fs` `process` `net` `http` `time` `rand` `env` `list` `option` `result` |

## Building and running

```sh
cargo build --release -p kelpc
./target/release/kelpc run kelpc/tests/fixtures/examples/hello.kp
./target/release/kelpc build kelpc/tests/fixtures/examples/download.kp   # binary in build/download
./target/release/kelpc check kelpc/tests/fixtures/examples/hello.kp     # compile checks, writes no artifact
./target/release/kelpc check --json kelpc/tests/fixtures/examples/hello.kp
./target/release/kelpc fmt -w kelpc/tests/fixtures/examples/hello.kp    # format in place (also: --check)
./target/release/kelpc lint kelpc/tests/fixtures/examples/hello.kp
```

Without `-o`, the binary and the backend's intermediate files land in `build/` beside the working directory. A directory with a `coral.toml` (fields `name`, `version`, `entry`) builds the same way: `kelpc build myproject`. The build tool and package manager is [coral](https://homelabone.xenoo.dev/gittea/Lilly/coral).

`kelpc` has one backend, **LLVM**. It lowers the typed IR to LLVM IR, runs LLVM's pass pipeline, emits an object file and links it with `ld.lld`. The C backend has been removed.

`build` and `run` take an optimization level, `-O0` to `-O3` or `-Os`, with `-O2` as the default. `-O0` is the cheap development build.

Sanitizers are not available. The LLVM backend links no sanitizer passes, so a build that asks for one is refused.

`kelpc check --json` prints the findings as one JSON document, which is the form an editor or build tool reads:

```json
{"schema":1,"ok":false,"diagnostics":[
  {"severity":"error","message":"...","module":"main",
   "file":"src/main.kp","line":3,"column":11,"end_line":3,"end_column":19,
   "notes":[]}
]}
```

`line` and `column` are 1-based and name the first byte of the finding. `end_line` and `end_column` are exclusive. `severity` is `error`, `warning` or `outdated`.

`kelpc fmt` formats `.kp` source to the style rules. It is idempotent and keeps every comment. `--check` exits nonzero when a file is not already formatted, which is what a CI job wants. It refuses rather than guesses: a file with a syntax error is reported with nothing written.

## Without Nix

Packages are built by `build/release-packages.sh` for Debian 13 (trixie), Ubuntu 24.04 (noble), Fedora and Arch Linux. Each one installs the compiler as `/usr/lib/kelpc/kelpc` and a launcher as `/usr/bin/kelpc`. The package manager resolves the dependencies. The file names below are the ones for version 26.10.6.

```sh
sudo apt install ./kelpc_26.10.6-1+trixie_amd64.deb     # Debian 13
sudo apt install ./kelpc_26.10.6-1+noble_amd64.deb      # Ubuntu 24.04
sudo dnf install ./kelpc-26.10.6-1.fc44.x86_64.rpm      # Fedora
sudo pacman -U kelpc-26.10.6-1-x86_64.pkg.tar.zst       # Arch Linux
```

What each package needs:

- **Debian and Ubuntu (`.deb`).** LLVM 21 is not in the distribution archives, so the package bundles `libLLVM.so.21.1` and `ld.lld` under `/usr/lib/kelpc`. The launcher sets `LD_LIBRARY_PATH` and `KELPC_LLD` to those copies. The package depends on `gcc`, the C library and its development files, `libstdc++6`, and the other libraries kelpc links. `apt show kelpc` lists them.
- **Fedora (`.rpm`).** The distribution's `llvm21` and `lld21` packages, and the libraries kelpc links. `rpm -qpR` on the file lists them.
- **Arch Linux (`.pkg.tar.zst`).** The same owners, from the `extra` repository (`llvm21`, `lld21`). `pacman -Qip` on the file lists them.

Every hosted link runs `ld.lld` and `cc`. `kelpc` finds `ld.lld` in `KELPC_LLD`, then on `PATH`. The collector archive is inside the compiler, so no other variable is needed.

To build from source without Nix you need:

- Rust with the 2024 edition (rustc 1.85 or newer). `rustup` works on every distribution.
- LLVM 21 development files, with `llvm-config` reporting 21.x, and `ld.lld`.
- A C compiler (`cc`), `pkg-config`, and the OpenSSL development files.

| Distribution | LLVM 21 and lld | `LLVM_SYS_211_PREFIX` | Packages |
| --- | --- | --- | --- |
| Debian 13, Ubuntu 24.04 | apt.llvm.org (`llvm-21-dev`, `lld-21`) | `/usr/lib/llvm-21` | `build-essential pkg-config libssl-dev` |
| Fedora | distribution (`llvm21-devel`, `lld21`) | `/usr/lib64/llvm21` | `gcc gcc-c++ make pkgconf-pkg-config openssl-devel` |
| Arch Linux | distribution (`llvm21`, `lld21`) | `/usr/lib/llvm21` | `base-devel pkgconf openssl` |

On Debian and Ubuntu the apt.llvm.org repository for LLVM 21 must be added first. Then, for example on Debian 13:

```sh
export LLVM_SYS_211_PREFIX=/usr/lib/llvm-21
export PATH=/usr/lib/llvm-21/bin:$PATH      # ld.lld, and llvm-config for the build
cargo build --release -p kelpc
./target/release/kelpc run kelpc/tests/fixtures/examples/hello.kp
./target/release/kelpc build kelpc/tests/fixtures/examples/download.kp
```

A `kelpc` built this way links `libLLVM-21.so` from the system, so that library must be present at run time. The distribution packages above provide it. If `ld.lld` is not on `PATH`, set `KELPC_LLD` to its full path instead.

## Repository layout

| Path | Contents |
| --- | --- |
| `kelpc/src/` | the compiler: lexer, preprocessor, parser, type checker, borrow checker, LLVM code generation, driver |
| `kelpc/std/` | the standard library, written in Kelp and embedded in the compiler |
| `kelpc/gc/` | the collector runtime that hosted programs link (Tidepool) |
| `kelpc/runtime/` | Kelp sources for the stub resolver's message parser and configuration reader |
| `kelpc/tests/` | the test suite: fixtures, multi-file projects, formatter and golden corpora |
| `kelpc/tests/fixtures/examples/` | `hello.kp`, `fib.kp`, `download.kp` and the example projects |
| `kelp-rt/` | the runtime library that Kelp programs link against |
| `bench/` | benchmark workloads and their recorded results |
| `build/` | build scripts for the release and the tooling |

## Requirements

- Rust with the 2024 edition, to build the compiler.
- LLVM 21 development output. `llvm-sys` is a dependency of every build. `nix-shell shell.nix` provides it and sets `LLVM_SYS_211_PREFIX`.
- A `kelpc` built this way links `libLLVM-21.so` dynamically and needs it at runtime.
- The glibc development files and the gcc runtime files, which the link reads. Kelp programs are not compiled by a C compiler. Building kelpc still needs `cc`, because cargo uses it as the Rust linker.
- Optional: `valgrind`, for the memory gate.

## Tests

```sh
cargo test                       # unit tests and the fixture suite on the LLVM backend
cargo test --test fmt            # the formatter's corpus and properties
cargo test --test memory_gate -- --ignored   # valgrind over the built binaries (ignored by default)
```

The fixture suite compiles a fixture, runs the result, and compares its output with the literal the author wrote. The golden files in `kelpc/tests/golden/` are the reference for each corpus program.

## Documentation and planning

Design notes, the language reference and the changelog are in the Kelp wiki (the `kelp-wiki` repository). Plans are Gitea issues, labelled `plan`. Pending changes are recorded on the wiki's `changelog-pending` page until a release.

License: MIT, see LICENSE
