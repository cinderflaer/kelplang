# Kelp

Kelp is a statically typed, compiled systems language whose headline feature is
**capability-based security**. A module declares what it may touch, and that
authority is part of the source and checked at every call site that uses it. A
program cannot read a file, open a socket, spawn a process or read the clock
unless the module making the call was granted that resource — so "what can this
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

fun area(s: &Shape).R: f64 = when s {
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
impl "std/http" with caps(net = ["127.0.0.1:*"])
impl "std/fs"
```

Those two lines are `examples/download.kp`'s entire network and filesystem
authority. A grant names the resource **as the program spells it** — `net`
matches the host string rather than a resolved address, and `fs` matches a
normalized, symlink-resolved path — so there is no second place where authority
can be acquired.

## What the language has

| | |
| --- | --- |
| Types | `i32` `i64` `u8` `u16` `u32` `u64` `usize` `isize` `f32` `f64` `bool` `char`, `str` (borrowed, NUL-terminated text) and `View` (borrowed, length-carrying text) |
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
./target/release/kelpc fmt -w examples/hello.kp       # format in place (also: --check)
```

With no `-o`, the binary and the generated C (or assembly) land in `build/`
beside the working directory. A directory with a
[`coral.toml`](https://homelabone.xenoo.dev/gittea/Juniper/coral) in it — `name`,
`version`, `entry` — builds the same way: `kelpc build myproject`.

Two code generators are selectable, with `--backend=` or `KELPC_BACKEND`:

- **`c`** (the default) transpiles to C and hands the file to `cc -O2 -fwrapv -lm`.
- **`asm`** emits x86-64 assembly itself and finishes with `as` and `ld`. No C
  compiler is in that chain.

The C backend is the reference: every fixture is built by both and the two
binaries' stdout is compared, so a construct the `asm` backend does not lower
yet is refused by name rather than silently miscompiled.

`kelpc fmt` formats `.kp` source to the style rules in `plan/syntax.md` §19:
`kelpc fmt FILE` writes to stdout, `-w` rewrites in place, and `--check` exits
nonzero when a file is not already formatted, which is what a CI job wants. It is
idempotent, token-faithful and comment-preserving — a formatted file lexes to the
same tokens and keeps every comment verbatim — and it refuses rather than guesses:
a syntax error, a file whose aliases cannot be resolved, a non-regular file, or a
lone carriage return used as a line terminator is reported with nothing written. A
symlink named on the command line is written *through*, and one found inside a
directory is not followed.

## Repository layout

| path | what is there |
| --- | --- |
| `kelpc/src/` | the compiler: `lexer`, `preprocess`, `parser`, `infer` (types, generics, capabilities), `borrowck` (moves, drops), `codegen_c`, `codegen_asm`, `driver` |
| `kelpc/std/` | the standard library, written in Kelp and embedded into the compiler |
| `kelpc/runtime/` | the reviewed C that is still C: the capability glue `codegen_c.rs` emits (`cap_glue.c`), and the C matcher/normalizer the §8 differential tests compare the Kelp versions against |
| `kelpc/tests/` | the fixture suite, the multi-file project fixtures, the matcher/normalizer corpora, the formatter corpus, and `run-all-backends.sh` |
| `examples/` | `hello.kp`, `fib.kp`, `download.kp` |
| `bench/speed/` | four workloads written in four languages, with checksums |
| `plan/` | the design, syntax, plan and milestone documents (see Documentation) |

## Requirements

- Rust, 2024 edition (rustc 1.85 or newer), to build the compiler. There are no
  third-party dependencies.
- The `c` backend: a C compiler and glibc.
- The `asm` backend: `as` and `ld` (binutils) and the glibc it links against. If
  the library directory is not found automatically, set `KELPC_GLIBC_DIR` to the
  directory holding `crt1.o` and `libc.so`.
- Optional: `valgrind`, for the memory gate.

## Tests

```sh
cargo test                                   # unit tests + the fixture suite
cargo test --test fmt                        # the formatter's corpus and properties
KELPC_BACKEND=asm cargo test --test compile_and_run
BACKENDS="c asm" ./kelpc/tests/run-all-backends.sh
```

The fixture suite is backend-agnostic by construction — it compiles a fixture,
runs the result, and compares stdout — so pointing `KELPC_BACKEND` at a backend
validates it with no test edits. Three heavier checks exist and are `#[ignore]`d
so the default run stays fast:

```sh
cargo test --test backend_coverage -- --ignored --nocapture   # differential sweep + refusal ranking
cargo test --test memory_gate    -- --ignored                 # valgrind over both backends' binaries
cargo test --test cap_matcher    -- --ignored                 # the capability matcher against its C reference
```

`bench/speed/run.sh` times the four workloads and checks their checksums.

## Related repositories

Kelp is one of three repositories on the same server. `kelpc` is the language and
the compiler; these two are the tooling around it.

| repository | what it is |
| --- | --- |
| **[Coral](https://homelabone.xenoo.dev/gittea/Juniper/coral)** | the build tool and package manager (`plan/roadmap.md` open item 72). `kelpc` stays the compiler; Coral owns the project — the `coral.toml` manifest, tasks, build cache and dependencies — and calls `kelpc` as a subprocess, never the reverse. |
| **[Lighthouse](https://homelabone.xenoo.dev/gittea/Juniper/lighthouse)** | IDE support: Kelp's LSP/editor tooling — completion, hover, go-to-definition, signature help, symbols, references, rename, folding, inlay hints, auto-import and quick fixes, plus diagnostics from `kelpc` itself. |

## Documentation

| file | what it is |
| --- | --- |
| `plan/syntax.md` | the syntax and style authority |
| `plan/architecture.md` | the design: compiler layout, the capability model, the v1 surface, and the standard library's decisions |
| `plan/roadmap.md` | the plan: the marks, the open items, and the backlog in its last part |
| `plan/done.md` | the milestone record of what has shipped |
| `plan/mk7-own-backend.md` | the working plan for the compiler's own backend |
| `plan/mk7-next-slice.md` | the measured brief for the next own-backend slice, with the fixture list each candidate could reach |
| `bench/speed/README.md` | what the benchmark measures and how |
