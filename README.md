# Kelp

Kelp is a statically typed, compiled systems language whose headline feature is
**capability-based security**. A module declares what it may touch, and that
authority is part of the source and checked at every call site that uses it. A
program cannot read a file, open a socket, spawn a process or read the clock
unless the module making the call was granted that resource, so "what can this
code do?" is answered by the first few lines of the file rather than by auditing
it.

The language itself is deliberately small: records (`datac`) and tagged unions
(`enum`) with exhaustive `when` patterns, ownership without a garbage collector,
`Option`/`Result` with `if let` and `?`, closures, string interpolation, and
modules. `Bytes`, `String`, `List`, `Map` and `Box` are compiler builtins rather
than a runtime library, and there is no `unsafe`, no exceptions and no
undefined-behaviour escape hatch.

`kelpc` is the compiler: a Rust program that takes `.kp` sources through lexing,
preprocessing, parsing, type checking, move/borrow checking and code generation.
The standard library ships inside that binary as Kelp sources, so the text
functions in `std/str` are reviewable Kelp rather than a C blob.

## A program

```kelp
datac Point(var x: i32, var y: i32)

enum Shape {
    Circle(val r: f64)
    Rect(val w: f64, val h: f64)
    Empty
}

fun area(s: &Shape).return: f64 = when s {
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

```
$ kelpc run sample.kp
point 10,4
area 12.566360
area 12.000000
sum 42
```

Authority is declared where a module is imported, and it is the only thing that
grants it:

```kelp
impl "std/http" caps(net = ["127.0.0.1:*"])
impl "std/fs"
```

Those two lines are `examples/download.kp`'s entire network and filesystem
authority. A grant names the resource **as the program spells it**, `net`
matches the host string rather than a resolved address, and `fs` matches a
normalized, symlink-resolved path, so there is no second place where authority
can be acquired.

## What the language has

| | |
| --- | --- |
| Types | `i32` `i64` `u8` `u16` `u32` `u64` `usize` `isize` `f32` `f64` `bool` `char`, `str` (borrowed, NUL-terminated text), `View` (borrowed, length-carrying text) and `[T; N]` (fixed-size, held inline, no allocator) |
| Data | `datac` records with `val`/`var` fields, `enum` tagged unions, exhaustive `when` (with a scrutinee or without), `if let`, type parameters on declarations (`Pair<A, B>`, `Option<T>`), `const` |
| Ownership | owned builtins `Bytes`, `String`, `List<T>`, `Map<K, V>`, `Box<T>`; moves, shared and `mut` borrows, drops at end of scope, no GC |
| Errors | `Option`, `Result`, the `?` operator, `panic`/`assert` |
| Functions | `fun`, named arguments, `fun` literals and closures, receiver methods, argument-type overloading inside the stdlib |
| Modules | `impl "std/fs"`, local-file imports, aliases, `pd` passdowns (file-scope values assigned before `main` runs) |
| Capabilities | `fs`, `net`, `process`, `env`, `time`, `rand` |
| Stdlib | `mem` `str` `path` `math` `fmt` `io` `fs` `process` `net` `http` `time` `rand` `env` `list` `option` `result` |

## Building and running

```sh
cargo build --release -p kelpc
./target/release/kelpc run examples/hello.kp
./target/release/kelpc build examples/download.kp      # artifact in build/download
./target/release/kelpc check examples/hello.kp        # compiles? no artifact written
./target/release/kelpc check --json examples/hello.kp # the same, as one JSON object
./target/release/kelpc fmt -w examples/hello.kp       # format in place (also: --check)
```

With no `-o`, the binary and the backend's intermediate land in `build/` beside
the working directory: the `llvm` backend leaves `<stem>.ll` and `<stem>.o`. A
directory with a
[`coral.toml`](https://homelabone.xenoo.dev/gittea/Juniper/coral) in it, `name`,
`version`, `entry`, builds the same way: `kelpc build myproject`.

`kelpc` builds every program with one backend, **`llvm`**: it lowers the typed IR
to LLVM IR, runs LLVM's own pass pipeline over the module, emits an object and
links it with `ld.lld`. There is no backend to choose.

The C backend was removed on 2026-10-10. It was the project's differential
oracle until then, and its answers for the corpus are kept as the golden set in
`kelpc/tests/golden/`. Sanitizers (`--sanitize=`, `[build] sanitize`) went with
it, so a build that asks for one is refused.

`build` and `run` take an optimization level: `-O0`, `-O1`, `-O2`, `-O3` or `-Os`,
with `-O2` the default. One level is one level for the whole build: it selects the
LLVM pass pipeline and the target machine's codegen level, and it is the `-O` flag
the per-program C object and the final link are given. `-Oz` is refused because it
is a clang-only spelling and the linking `cc` may be gcc; `-Os` is the size level.
`-O0` runs no module pipeline and is the cheap dev build.

The hand-written `asm` backend was removed on 2026-09-21, and the LLVM backend's plan of record is
retired with it: the whole MK7 record is in `plan/done.md`. The
S7 flip landed in code on 2026-09-21, and the user-facing backend selector was
removed with it: `llvm` is the only backend a person building a program can use,
and both backends stay in the suite. The release that announces the flip is still
to be cut.

`kelpc check` runs everything a build runs except code generation: the same
modules, the same type check, the same borrow check. It answers whether a
program compiles without writing an artifact. It exits nonzero only for an
error: an outdated-syntax finding on its own exits zero, the rule `kelpc lint`
follows. `--json` prints the findings on stdout as one document instead of prose
on stderr, which is the form an editor or a build tool reads:

```json
{"schema":1,"ok":false,"diagnostics":[
  {"severity":"error","message":"...","module":"main",
   "file":"src/main.kp","line":3,"column":11,"end_line":3,"end_column":19,
   "notes":[]}
]}
```

`line`/`column` are 1-based and name the first byte of the finding's span;
`end_line`/`end_column` are exclusive. The location keys are absent when there is
no file to point at (a path the compiler could not read), and `module` is always
there. `severity` is `error`, `warning` or `outdated`. The front end stops at the
first error, so `diagnostics` carries one error plus whatever findings came
before it.

`kelpc fmt` formats `.kp` source to the style rules in `plan/syntax.md` §19:
`kelpc fmt FILE` writes to stdout, `-w` rewrites in place, and `--check` exits
nonzero when a file is not already formatted, which is what a CI job wants. It is
idempotent, token-faithful and comment-preserving, a formatted file lexes to the
same tokens and keeps every comment verbatim, and it refuses rather than guesses:
a syntax error, a file whose aliases cannot be resolved, a non-regular file, or a
lone carriage return used as a line terminator is reported with nothing written. A
symlink named on the command line is written *through*, and one found inside a
directory is not followed.

## Repository layout

| path | what is there |
| --- | --- |
| `kelpc/src/` | the compiler: `lexer`, `preprocess`, `parser`, `infer` (types, generics, capabilities), `borrowck` (moves, drops), `codegen_llvm`, `driver` |
| `kelpc/std/` | the standard library, written in Kelp and embedded into the compiler |
| `kelpc/runtime/` | the reviewed C that is still C: the capability glue the LLVM backend emits (`cap_glue.c`), and the C matcher/normalizer the §8 differential tests compare the Kelp versions against |
| `kelpc/tests/` | the fixture suite, the multi-file project fixtures, the matcher/normalizer corpora, the formatter corpus, and the corpus goldens |
| `examples/` | `hello.kp`, `fib.kp`, `download.kp` |
| `bench/speed/` | four workloads written in four languages, with checksums |
| `plan/` | the design, syntax, plan and milestone documents (see Documentation) |

## Requirements

- Rust, 2024 edition (rustc 1.85 or newer), and the LLVM 21 development output,
  to build the compiler: `llvm-sys` is a dependency of every build. `nix-shell
  shell.nix` provides it (`llvm-config`, `include/llvm-c`) and sets
  `LLVM_SYS_211_PREFIX`; `plan/done.md`'s MK7 record is the decision and its
  trade-offs.
- A `kelpc` built this way links `libLLVM-21.so` dynamically and needs it at
  runtime.
- The glibc development files and the gcc runtime files (`crtbeginS.o`,
  `libgcc.a`), which the link reads. No C compiler is run to compile or link a
  Kelp program. Building kelpc from source also needs `cc`: cargo uses it as the
  Rust linker, and `build.rs` queries it.
- Optional: `valgrind`, for the memory gate.

## Tests

```sh
cargo test                                   # unit tests + the fixture suite, on the default `llvm` backend
cargo test --test fmt                        # the formatter's corpus and properties
```

The fixture suite compiles a fixture, runs the result, and compares stdout with a
literal an author wrote. The corpus goldens in `kelpc/tests/golden/` are the
reference for every corpus program. Two heavier checks
exist and are `#[ignore]`d so the default run stays fast:

```sh
cargo test --test memory_gate    -- --ignored                 # valgrind over the built binaries
cargo test --test cap_matcher    -- --ignored                 # the capability matcher against its C reference
```

`bench/speed/run.sh` times the four workloads and checks their checksums.

## Related repositories

Kelp is one of three repositories on the same server. `kelpc` is the language and
the compiler; these two are the tooling around it.

| repository | what it is |
| --- | --- |
| **[Coral](https://homelabone.xenoo.dev/gittea/Juniper/coral)** | the build tool and package manager (`plan/roadmap.md` open item 72). `kelpc` stays the compiler; Coral owns the project, the `coral.toml` manifest, tasks, build cache and dependencies, and calls `kelpc` as a subprocess, never the reverse. |
| **[Lighthouse](https://homelabone.xenoo.dev/gittea/Juniper/lighthouse)** | IDE support: Kelp's LSP/editor tooling, completion, hover, go-to-definition, signature help, symbols, references, rename, folding, inlay hints, auto-import and quick fixes, plus diagnostics from `kelpc` itself. |

## Documentation

| file | what it is |
| --- | --- |
| `plan/syntax.md` | the syntax and style authority |
| `plan/architecture.md` | the design: compiler layout, the capability model, the v1 surface, and the standard library's decisions |
| `plan/roadmap.md` | the plan: the marks, the open items, and the backlog in its last part |
| `plan/done.md` | the milestone record of what has shipped, including the whole MK7 LLVM backend record |
| `bench/speed/README.md` | what the benchmark measures and how |
