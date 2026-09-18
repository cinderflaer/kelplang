# The "X" language — compiler project plan

## Context

The user wants a new statically-typed, systems-level programming language (files end in `.x`), aiming for Rust/C/C++-class runtime performance, with Kotlin/Rust-style braces syntax and a Rust-style ownership + borrowing memory model (no GC, no ARC). This is a from-scratch project (the working directory is empty, not yet a git repo) built purely as a personal/for-fun engineering project — no external users or deadlines, but the user wants a real foundation, not a toy that cuts corners on the core mechanics (ownership/borrowing) they specifically asked for.

Decisions locked in via discussion:
- **Memory model**: full ownership + borrowing (moves, `&T`, `&mut T`, borrow checker) — attempted from the start, not deferred to "moves only."
- **Backend**: transpile to C, then shell out to `cc`/`clang` to produce a native binary. Chosen over LLVM for implementation simplicity while still getting native `-O2` codegen.
- **Type system**: fully static with local type inference (Kotlin/Rust-style — annotate signatures, infer locals).
- **Syntax**: brace-delimited, `fn` keyword, Rust/Kotlin-flavored.
- **Implementation language**: Rust (good fit for tree-walking, pattern matching over ASTs, and enums for typed IR).

Because a full borrow checker is the hardest single component here, the plan is phased so that each milestone produces a compiler that actually builds and runs `.x` programs, with borrowing arriving once the simpler layers (lexer → parser → types → C codegen) are solid and tested. This avoids the failure mode of building a borrow checker against a language/compiler that doesn't exist yet.

**Distinguishing feature — resolved: capability-based security as a language feature, not an OS layer.** Instead of every import getting implicit access to the filesystem, network, process spawning, and environment, each `use` is granted an explicit, delegated subset of capabilities, and code without a grant has no *name* to reach that resource with — not a runtime-guarded call that happens to reject it, but a symbol that plain doesn't resolve. See "Capability-based security" section below for the mechanism. (Other candidates considered and shelved: Zig-style comptime instead of generics/traits/macros; arena/region allocation as an ownership alternative; Go-style structural interfaces — worth revisiting as *additional* features later, but capabilities is the headline one.)

## Compiler crate layout

Cargo workspace at repo root:

```
x/
  Cargo.toml                # workspace
  xc/                        # the compiler binary crate
    src/
      main.rs                # CLI: `xc build foo.x`, `xc run foo.x`
      lexer.rs                # source -> tokens
      ast.rs                  # parsed syntax tree types
      parser.rs               # tokens -> AST (recursive descent + Pratt for expressions)
      resolve.rs              # name resolution / scopes, also resolves `use` imports (see Imports below)
      types.rs                # type representations
      infer.rs                # type inference / checking
      borrowck.rs              # ownership + borrow checker (added in phase 4)
      codegen_c.rs             # AST/typed-IR -> C source text
      driver.rs                # orchestrates pipeline, invokes `cc`
    tests/
      compile_and_run.rs       # end-to-end: compile .x fixtures, run binary, assert stdout
      fixtures/*.x
  examples/
    hello.x
    fib.x
    ownership_move.x
    borrow_basic.x
```

Single crate is fine at this scale (a compiler is inherently one pipeline); no need to over-modularize into multiple crates.

## Imports & project structure for `.x` projects (i.e. what a user's `myproject/` looks like)

Explicit reaction against Java/Kotlin's convention (package must mirror deeply-nested directory path, plus a Gradle/Maven build-tool layer with its own DSL files, wrapper scripts, and cache dirs). Instead:

- **One manifest file**, `x.toml`, at the project root — name, version, entry point. No separate build-tool layer; `xc` itself is the build tool.
- **`src/` holds everything**, flat by default, with subfolders only when the user actually wants to group files — a file's path is never required to match any declared namespace.
- **Imports are path-based**, not namespace-based: `use "shapes/circle"` pulls in `src/shapes/circle.x` by its file path relative to `src/`, and you refer to its items as `circle.Circle` (last path segment becomes the default alias). No `mod` declarations needed to "announce" a file into a tree — the file existing under `src/` is enough.
- **v1 is local-files-only**: `use` can only reach files inside the current project's `src/`. No external/third-party dependency support yet (no registry, no git-url deps, no lockfile) — that is a deliberately separate, later feature.

Example:
```
myproject/
  x.toml
  src/
    main.x          # use "shapes/circle"; fn main() { ... }
    shapes/
      circle.x
      square.x
```

`resolve.rs` handles turning a `use "shapes/circle"` path into a loaded/parsed file plus a name binding for its exported items; this is pure file-path resolution, no separate package-registry concept to build.

**General alias/expansion system (`alias.xconf` + inline `alias` declarations)** — generalized beyond just import paths, per later discussion: a flat, global, purely-textual substitution mechanism, deliberately simple (no parameters, no hygiene, no scoping).

- **Reference syntax**: `%*name*%` used anywhere in `.x` source (including inside a `use "..."` path string). The sigil exists specifically so it can never be confused with a real identifier/variable — the lexer/preprocessor recognizes it unambiguously and substitutes before real parsing happens.
- **Definition site 1 — `alias.xconf`** (project-root config file, zsh-alias-flavored): `name="replacement text"` per line. e.g. `shared="../../shared/utils"` or `chttps="http_client.new(timeout: 30, tls: true, retries: 3)"`.
- **Definition site 2 — inline in `.x` source**: a dedicated `alias` declaration keyword, distinct from `let`, so the parser stores the RHS as a raw token stream (macro text) instead of type-checking/evaluating it as an expression: `alias %*chttps*% = http_client.new(timeout: 30, tls: true, retries: 3);`. Needing a separate keyword (rather than reusing `let`) is exactly the "extra stuff" needed so it isn't mistaken for a normal variable.
- **Mechanics**: a preprocessing pass runs on the token stream *before* the real parser: (1) collect all alias definitions across `alias.xconf` and every source file's `alias` declarations into one flat table; (2) expand every `%*name*%` occurrence with its registered text; (3) feed the fully-expanded token stream into the existing lexer/parser pipeline. Cyclic aliases (an alias whose expansion transitively references itself) are rejected the same way import cycles are.
- **Known tradeoff, stated explicitly**: because this is pure text substitution, a bad alias can produce nonsense that only surfaces as a type error at the (already-expanded) use site — accepted as the cost of keeping this "pretty simple" rather than a hygienic macro system. Error messages should still point back to both the alias's definition span and its use span so this stays debuggable.
- Import-path aliasing (the earlier, narrower idea) is just this same mechanism applied to `use "..."` strings — no separate implementation needed once general expansion exists.

Lands as part of M5 alongside the rest of the import system, since both are about the token/file layer before real parsing.

## Capability-based security (the headline feature)

Every `use` can carry a `with caps(...)` clause granting access to privileged resource domains. Without a grant, the corresponding built-in namespace simply isn't a resolvable name inside that module — enforced by the same mechanism as any other undefined-identifier error, not by a runtime permission check that could be forgotten or bypassed.

Syntax (as proposed by the user):
```
use "http_client" with caps(net: ["api.example.com:443"])
use "logger" with caps(fs: ["/var/log/myapp/*"])
```

**Capability domains (v1)**: `fs` (path/glob allowlist), `net` (host:port allowlist), `process` (spawn allowlist), `env` (variable-name allowlist). Each domain has a corresponding built-in namespace (`x.fs`, `x.net`, `x.process`, `x.env`) exposing gated wrapper functions — there is no other way to reach the underlying syscalls; user code never gets raw `open`/`connect`/`fork`/`getenv`.

**Enforcement mechanism, split by what's actually checkable at compile time:**
- *Reachability (structural, 100% compile-time)*: `resolve.rs` tracks a capability-domain set per module (computed from its incoming `with caps(...)` grant). Resolving `x.net.connect` when the current module's set doesn't include `net` is a resolution error — identical in kind to referencing an undefined variable. This is what makes "not reachable through reflection or a workaround" true: there's no reflection in this language, and the symbol genuinely isn't in scope.
- *Fine-grained matching (host/path allowlist patterns, compiler-inserted runtime check)*: the specific argument to `x.net.connect("api.example.com", 443)` is often a dynamic string, so the compiler embeds the granted allowlist as constant data for that module and inserts a check at each gated call site, generated as part of `codegen_c.rs`. The module itself never holds a capability *value* it could pass around or forge — only the compiler-generated call sites know the allowlist, so there's no way for the module's own code to construct an unchecked path to the resource.

**Delegation/no-escalation rule**: a module's capability set is *received* from whoever imports it, never self-declared. When module A does `use "B" with caps(net: [...])`, the resolver requires that grant to be a subset of A's own capability set — a module can delegate less than it has, never more. The root/entry module (`main`) implicitly holds the full set granted by `x.toml` (or by default, everything, until a later milestone adds a way to restrict `main` itself).

**Scope note**: for the first working version of this feature, only `fs` needs a real underlying implementation (simple path/glob allowlist checked against actual file operations) to prove the mechanism end-to-end; `net`/`process`/`env` can be structurally gated (resolution enforcement works) with stub/unimplemented bodies until there's a reason to flesh them out.

This depends on the multi-file `use` import system existing, so imports move back into v1 scope (see revised milestones below) rather than being deferred.

## Language v1 feature set (scope for milestones M0–M4)

- Primitive types: `i32`, `i64`, `u32`, `u64`, `f32`, `f64`, `bool`, `char`, `unit` (void-equivalent)
- `&str` string literal type only (borrowed, points at static data) — owned/heap `String` deferred to a later milestone once ownership machinery exists to make it meaningful
- `let` (immutable by default) / `let mut`
- `fn name(param: T, ...) -> T { ... }`, explicit param/return types, local type inference for `let`
- `struct` with named fields
- Control flow: `if`/`else` (as statement and expression), `while`, `return`
- Operators: arithmetic, comparison, logical (`&&`, `||`), unary `-`/`!`
- References: `&expr`, `&mut expr`, deref `*expr`
- Moves: assigning/passing a non-`Copy` value transfers ownership; using it after is a compile error
- Borrowing: `&T` / `&mut T` params and bindings, enforced aliasing (many immutable XOR one mutable), enforced "borrow can't outlive owner" using lexical (scope-based) lifetimes — not full non-lexical-lifetime inference; that's a stretch goal, not v1
- `println!`-equivalent (probably `print(expr)` / `println(expr)` builtin) for output, implemented as a thin wrapper emitted directly to C `printf`

Deferred beyond v1 (explicitly out of scope for now, revisit later): generics, traits/interfaces, enums/pattern matching, closures, heap collections (`Vec`, `String`), explicit lifetime annotation syntax (`'a`), non-lexical lifetimes, async, unsafe blocks, external/third-party dependencies (local-file `use` imports are in scope — see Imports & Capabilities sections).

## Compiler pipeline (per milestone)

**M0 — Scaffold + lexer/parser for a minimal subset**
- Cargo workspace, `xc` binary with `xc build <file.x>` CLI (using only `std`, no need for external CLI-parsing crate at this size).
- Lexer: hand-written, produces tokens with source spans (spans matter later for good borrow-checker error messages).
- AST + recursive-descent/Pratt parser for: functions, `let`, arithmetic/comparison expressions, `if`, `return`, `println`.
- Deliverable: `xc` parses `examples/hello.x` and `examples/fib.x` into an AST, pretty-prints it (no codegen yet). Unit tests over the parser.

**M1 — Types + C codegen + first running binary**
- `types.rs`: type representations for primitives + function signatures.
- `infer.rs`: check explicit signatures, infer `let` types from initializer, verify expression types, report type errors with spans.
- `codegen_c.rs`: emit a `.c` file (map `.x` types to C types, functions to C functions, expressions/statements structurally).
- `driver.rs`: write generated C to a temp file, invoke `cc -O2 -o <out> <gen.c>`, run/report.
- Deliverable: `xc build examples/hello.x && ./hello` prints correctly. `xc build examples/fib.x` computes fibonacci correctly. End-to-end test harness in `tests/compile_and_run.rs` that compiles each fixture, runs it, and checks stdout — this becomes the regression suite for every later milestone.

**M2 — Structs, mutability, loops**
- Add `struct` definitions → C `struct`s; field access; `while` loops; `let mut` + assignment.
- Extend type checker for struct field types and mutability tracking (which bindings are declared `mut`).
- New fixtures + tests (a small struct-based program, a mutable accumulator loop).

**M3 — Ownership: moves**
- Track, per binding, whether a value has been moved out (per-scope move-state table in `borrowck.rs`, introduced now even though full borrowing lands in M4).
- Rule: passing/assigning a non-`Copy` value (structs are non-`Copy` by default; primitives are `Copy`) moves it; using a moved-from binding afterward is a compile error with a "value moved here" / "used here" span pair (Rust-style diagnostics).
- No heap allocation yet — moves are checked at compile time only; codegen doesn't need to change yet since nothing is actually freed. This isolates move-checking logic from allocation/drop logic.
- Fixture: `ownership_move.x` demonstrating a legal move and a caught illegal use-after-move (as a `tests/` case that asserts the compiler *rejects* it — the test suite needs both "compiles and runs correctly" and "correctly rejected with expected error" cases from this point on).

**M4 — Borrowing**
- Extend `borrowck.rs`: track active borrows per binding within a scope tree derived from the AST (blocks = scopes).
- Enforce: at most one active `&mut` borrow, or any number of active `&` borrows, never both, for a given owner, within its lexical scope; a borrow can't be used after its scope ends.
- Extend type checker/codegen: `&T`/`&mut T` become C pointers; `*expr` becomes C dereference.
- Fixtures: `borrow_basic.x` (legal shared borrows, legal exclusive borrow, a function taking `&mut T` mutating through it) plus rejection cases (aliasing violation, use of dangling borrow).

**M5 — Multi-file imports**
- `resolve.rs`: parse `use "path/to/file"` at the top of a file, load/parse the referenced file relative to `src/`, bind its exported top-level items under the last path segment as an alias (`circle.Circle`), detect and reject import cycles.
- `x.toml` manifest parsing (name, version, entry point) so `xc build` knows the project root and `src/` layout instead of taking a single loose file.
- Fixtures: a small multi-file project (`src/main.x` using `src/shapes/circle.x`).

**M6 — Capability-based security**
- Parse `with caps(domain: [patterns...])` on `use` clauses.
- `resolve.rs`: maintain a per-module capability-domain set; gate resolution of `x.fs`/`x.net`/`x.process`/`x.env` identifiers on that set; enforce the subset/no-escalation rule when a module delegates capabilities to something it imports.
- Implement `x.fs` for real (path/glob allowlist matching against actual file reads/writes); leave `x.net`/`x.process`/`x.env` structurally gated with stub bodies.
- `codegen_c.rs`: emit the compile-time-known allowlist as constant data and insert the runtime pattern check at each gated call site.
- Fixtures: a module granted `fs` access to a specific path glob that successfully reads a file; a module *not* granted `fs` where referencing `x.fs.*` is a resolution error; a module granted a narrower glob than it was given attempting to over-delegate to a sub-import (rejected).

**M7 (stretch, only after M0–M6 are solid)** — introduce one owned heap type (e.g. `Box<T>`) to give ownership/moves actual runtime consequence (malloc on construction, free on scope exit/drop), proving the ownership tracking isn't just a paper exercise. Explicitly a "come back to this later" milestone, not part of the initial delivery.

## Key implementation notes

- **Error reporting**: every AST node carries a source span from day one (even though M0 has no borrow checker yet) — retrofitting spans later is much more painful than carrying them from the lexer onward.
- **Move/borrow checking as a separate pass, not entangled with type inference**: `infer.rs` produces a typed AST; `borrowck.rs` walks the typed AST separately. Keeps each pass comprehensible and testable in isolation, which matters a lot once borrow-checker bugs need debugging.
- **C codegen strategy**: generate readable, `#line`-annotated C (mapping generated lines back to `.x` source lines) so that `gdb`/`printf`-debugging the *generated* C during compiler development is tractable, and so a future "debug the .x program itself" story is easier to bolt on.
- **No generics/traits in v1**: keeps type inference and borrow checking tractable; monomorphization is a large added dimension best tackled once the non-generic core works end-to-end.

## Verification

- `cargo test` in `xc/` runs the end-to-end fixture suite (`tests/compile_and_run.rs`): for each `examples/*.x` / `tests/fixtures/*.x`, either (a) compile, run the resulting binary, and diff stdout against an expected value, or (b) compile and assert a specific compiler error is produced (for move/borrow violation fixtures).
- Manual smoke test after each milestone: `cargo run -p xc -- build examples/hello.x && ./hello` (and similarly for `fib.x`, etc.) to confirm the CLI path works, not just the test harness.
- After M4, deliberately write a handful of "should fail" `.x` snippets (double move, two simultaneous `&mut` borrows, use of a borrow after its owner's scope ends) and confirm the compiler rejects each with a sensible error — this is the actual proof that the borrow checker works, not just that "well-behaved" programs compile.
