# The "X" language — compiler project plan

Merged document: language plan + compiler milestones + standard library. The stdlib is no longer a separate file; it is the "Standard library" section below.

**Precedence rule: `syntax&shii.md` always wins.** Where this file and `syntax&shii.md` disagree about syntax or style — including where this file restates that syntax — `syntax&shii.md` is correct and this file is wrong. The syntax content here is a copy kept for reading convenience, not a second authority. This file is the authority for everything else: milestones, capabilities, and the standard library.

## Current state

The compiler exists and runs. `xc/` implements the new syntax end-to-end (lexer → parser → type check → move/borrow check → C codegen), `cargo test` is green (38 unit tests + 34 end-to-end fixture/project tests), and the milestone record below reflects commits, not hopes. This file is now the sole living status doc (`PASSDOWN.md` was deleted once this file absorbed it — do not recreate it).

- **Done**: M0 (scaffold + parser for the full new syntax), M1 (types + C codegen + first running binary), M2 (`datac`, per-field `val`/`var`, while loops), M3 (move checking), M4 (borrowing — free-function parameters and local `&`/`&mut`/`*`), M4.5 (receiver-form methods, the piece M4 cut out — see below), M5.1 (multi-file `impl` imports, `x.toml`, strict module-qualified names).
- **Next**: `pd` passdowns plus string templates (M5.2), then the `%*alias*%` text-substitution preprocessor (M5.3); after that everything remaining is stdlib-blocking (see "Deferred" below).
- **Deferred despite being in `syntax&shii.md`'s deltas**: `pd` passdowns, `alias` substitution, string templates, `caps(...)`. Those deltas were aspirational scheduling and lost to implementation order; the milestone list below records where each one is now targeted (`impl` imports have since shipped, as M5.1).
- **`pd var` semantics, resolved** (was `syntax&shii.md` §9's open question): a `pd var` is **read-only inside functions**. It may only be assigned outside a function body — i.e. at its own top-level declaration, since v1 has no top-level statement form beyond `pd` itself. Functions that reference a passdown always get a shared borrow of it and can never write through it. This is the narrowest reading that keeps the feature without introducing reachable mutable global state, and it is what M5.2 will implement. `syntax&shii.md` §9's list of options is answered by that sentence; the syntax authority is unchanged and the question is closed here (semantics are this file's call, not the syntax file's).
- **Not started**: the stdlib (`xc/std/` does not exist yet), so every stdlib API sketch below is a plan, not a description.
- **Known doc bug, not a compiler bug**: `syntax&shii.md`'s (and this file's) own "Full example" mutates a `val` field (`datac Circle(val r: f64)` then `fun mut Circle.grow(by: f64) { r += by }`), which directly contradicts the `Counter` example's own stated rule just above it ("no binding mutability can fix" a `val` field). The compiler correctly rejects this — `xc/tests/fixtures/receiver_methods.x` uses `var r: f64` instead. Left un-edited in `syntax&shii.md` itself since that file is locked as the syntax authority and nobody asked for that edit; flagging it here so it doesn't get "fixed" by loosening the compiler instead.
- **Also deliberately left as-is in `syntax&shii.md`**: §8's `impl "logger" with caps(fs = ["/var/log/myapp/*"])` and §11's full example both use `with caps(...)`, which the parser rejects with an explicit "not implemented until M7" message (M5.1 lexes `[`/`]` for exactly this). `caps` is scheduled, not dropped.

## Context

The user wants a new statically-typed, systems-level programming language (files end in `.x`), aiming for Rust/C/C++-class runtime performance, with Kotlin-flavored braces syntax and a Rust-style ownership + borrowing memory model (no GC, no ARC). This is a from-scratch project built purely as a personal/for-fun engineering project — no external users or deadlines, but the user wants a real foundation, not a toy that cuts corners on the core mechanics (ownership/borrowing) they specifically asked for.

Decisions locked in via discussion:
- **Memory model**: full ownership + borrowing (moves, borrows, borrow checker) — attempted from the start, not deferred to "moves only."
- **Backend**: transpile to C, then shell out to `cc`/`clang` to produce a native binary. Chosen over LLVM for implementation simplicity while still getting native `-O2` codegen.
- **Type system**: fully static with local type inference (Kotlin/Rust-style — annotate signatures, infer locals).
- **Syntax**: brace-delimited, Kotlin-shaped surface (`fun`, `val`/`var`, `datac`, `.R: T`, no semicolons, receivers instead of `self`/`impl` blocks) over a Rust-shaped memory model. Full spec in the "Syntax & style" section below.
- **Implementation language**: Rust (good fit for tree-walking, pattern matching over ASTs, and enums for typed IR).

Because a full borrow checker is the hardest single component here, the plan is phased so that each milestone produces a compiler that actually builds and runs `.x` programs, with borrowing arriving once the simpler layers (lexer → parser → types → C codegen) are solid and tested. This avoids the failure mode of building a borrow checker against a language/compiler that doesn't exist yet.

**Distinguishing feature — resolved: capability-based security as a language feature, not an OS layer.** Instead of every import getting implicit access to the filesystem, network, process spawning, and environment, each `impl` is granted an explicit, delegated subset of capabilities, and code without a grant has no *name* to reach that resource with — not a runtime-guarded call that happens to reject it, but a symbol that plain doesn't resolve. See "Capability-based security" below. (Other candidates considered and shelved: Zig-style comptime instead of generics/traits/macros; arena/region allocation as an ownership alternative; Go-style structural interfaces — worth revisiting as *additional* features later, but capabilities is the headline one.)

## Syntax & style

Theme: Kotlin-shaped surface over a Rust-shaped memory model. Copied from `syntax&shii.md`, which is the authority — on any collision, that file wins.

### Types: `datac`

`struct` is gone. Fields are declared `val` (immutable) or `var` (mutable). A `datac` holds **owned data only** — no reference fields in v1. The only owned heap type in v1 is the builtin `Bytes` (see "Standard library"); `&str` fields are therefore illegal.

```x
datac Circle(val r: f64)
datac Vec2(val x: f64, val y: f64)
datac User(val name: String, var age: i32)
```

`String` in that example is the owned heap string type, so it lands with the heap milestone (M4) — `syntax&shii.md` uses it as a `datac` field, and owned fields are the only kind a `datac` may hold.

Field assignment needs both: the field declared `var` in the `datac`, and the binding holding the `datac` reachable mutably.

```x
datac Counter(val id: i32, var count: i32)

fun bump(c: mut Counter) {
    c.count += 1        // legal: `count` is var in the datac, `c` is an exclusive borrow
}
```

`c.id = 5` is an error at any call site — `id` is `val` in the `datac`, and no binding mutability can fix that.

### Bindings: `val` / `var`

- `val` — immutable binding
- `var` — mutable binding

`let` and `let mut` are dropped entirely.

```x
val before = c.area()
var c = Circle(r = 2.0)
```

### Functions: `fun` + `.R: T`

- Keyword is `fun`, not `fn`.
- Return type is `.R: T`, written after the full signature. `.R` marks "this returns"; `: T` gives the type.
- No `.R` = returns `Unit`.
- `->` is never a return marker.

```x
fun Circle.area().R: f64 = 3.14159 * r * r
fun mut Circle.grow(by: f64) { r += by }
fun main() { ... }
```

### Return: `return expr`

A function with `.R: T` must end with `return expr` producing a `T`. The signature is the contract; `return expr` delivers the value. Expression bodies (`= expr`) remain shorthand for single-expression functions.

```x
fun sum_to(n: i32).R: i32 {
    var acc = 0
    var i = 1
    while i <= n {
        acc += i
        i += 1
    }
    return acc
}

fun Circle.area().R: f64 = 3.14159 * r * r     // shorthand, no return needed

fun classify(n: i32).R: &str = when {
    n > 0 -> "pos"
    n < 0 -> "neg"
    else  -> "zero"
}
```

### Receivers replace `self` / `impl` blocks / explicit `&`

- No `self` keyword. No `impl` method block.
- Receiver is declared in the signature: `fun Circle.area()`.
- Shared borrow is implicit — write nothing.
- Exclusive borrow is marked `mut`: `fun mut Circle.grow(by: f64)`.
- Free-function parameters: write `mut` only when mutation is needed; everything else is a shared borrow by default.

```x
fun Circle.area().R: f64 = 3.14159 * r * r        // shared receiver
fun mut Circle.grow(by: f64) { r += by }          // exclusive receiver

fun len(s: String).R: usize                       // shared borrow, inferred
fun push(s: mut String, ch: char) { ... }         // exclusive, marked
```

`&` / `&mut` survive only outside signatures: taking a borrow of a local (`val p = &c`, `val q = &mut c`) and dereferencing (`*p`). Parameter and receiver positions use the bare/marked forms above.

**Gap, resolved in M4:** parameters are shared borrows by default, so there is no spelling for a parameter that *takes ownership* (a move-in) — and per the M4 milestone entry below, there isn't one in v1. Callers pass a borrow or a local; a `datac` is only ever moved by local rebinding.

### Print

- `print(...)` — prints with a trailing newline.
- `print!(...)` — prints with no trailing newline.

Replaces `println` entirely.

```x
print!("loading")   // loading
print!(".")         // loading.
print("done")       // loading.done\n
```

### `->` is reserved for `when`

`->` appears only in `when` branches, as `condition -> value`. It never means "returns".

```x
fun classify(n: i32).R: &str = when {
    n > 0 -> "pos"
    n < 0 -> "neg"
    else  -> "zero"
}
```

`when` in v1 is the boolean/`else` form only. Exhaustiveness checking, enum subjects, and value patterns wait for enums.

### Module imports: `impl`

Module imports use `impl`, not `use`. `impl` means import; the `impl` method block it used to suggest is gone (receivers replaced it), so the keyword is free.

```x
impl "shapes/circle"
impl "logger" with caps(fs = ["/var/log/myapp/*"])
impl "net/http"
impl "time" as t
```

### Passdowns: `pd`

A passdown is a top-level binding available to every function without being passed explicitly. The compiler wires it in **only in functions that actually reference it** — functions that never mention it pay no cost.

```x
pd val name = expr
pd var name = expr
pd val name: T = expr
```

- Legal only in the file's top block, before any `fun` or `datac`, and **after** the `impl` lines (canonical order: `impl`, then `pd`, then `datac`/`fun`).
- `pd val` immutable, `pd var` mutable — same meaning as local `val`/`var`.
- Type inferred, or written explicitly with `: T`.
- A function referencing a passdown name gets it wired in automatically.
- A local `val`/`var` with the same name shadows the passdown inside that scope.
- Nothing is written at the function side — the function just uses the name.

```x
pd val app_name = "shapes"
pd var request_count = 0

fun log_area(c: Circle) {
    request_count += 1
    print("[$app_name #$request_count] area = ${c.area()}")
}
```

`log_area` uses both passdowns — wired in. `classify` uses neither — no wiring, no cost.

**Resolved (was: Unresolved):** `pd var` is mutable global state reachable from any function, and v1 has no closures and no `unsafe` to make that safe. The decision, recorded in full under "Current state": **`pd var` is read-only inside functions** — it can only be assigned outside a function body (i.e. at its own top-level declaration), so no function ever holds an exclusive borrow of a passdown and no cross-function exclusivity rule is needed. That is closer to option (b) than to (a) or (c): the feature is kept, but writability is confined to the top level rather than exposed inside functions. `pd` wiring itself lands in M5.2.

### Retained Kotlin-look rules

- No semicolons. Newline ends a statement; a line ending in a binary operator (`.`, `+`, `,`, `&&`, `=`) continues onto the next line.
- Types on the right with `:`, e.g. `r: f64`.
- No parens on `if` / `while` conditions. Braces always required.
- String templates: `"x = $before, area = ${c.area()}"`.
- Named arguments at call sites when 2+ params: `Circle(r = 2.0)`, `math.pow(base = 2.0, exp = 8.0)`.
- `Unit` is the void type and the implicit return type (replaces `unit`).
- `when` replaces long `if`/`else if` chains.
- Blank line between functions. `impl` lines grouped at top. 4 spaces. Hard 100-col.
- Format is canonical, so `xc fmt` is the enforcement story (later milestone, not v1).
- **User-code naming case is a configurable lint, not a fixed rule.** A project-level setting (name TBD — e.g. an `x.toml` field or an `xc fmt`/lint flag, whichever ends up hosting `xc fmt`'s config) picks the preferred case for function/variable/param names; whichever case is *not* selected produces a warning, never a compile error. Options, numbered so a config value can just be an index: `0` = `camelCase` (default when unset — `fun computeArea()`, `val itemCount`), `1` = `snake_case` (`fun compute_area()`, `val item_count`). Room to extend the list later (`PascalCase`, `SCREAMING_SNAKE_CASE` for consts once `const` exists, etc.) without renumbering — append, don't reorder. Far-future, lint-only either way: this is a *beauty warning* (`xc fmt`/a future lint pass), and code in any case must keep compiling regardless of the configured preference. Not scheduled to a milestone yet. Tension to resolve whenever this is actually built: the stdlib naming rule below already locks `snake_case` for stdlib functions/namespaces (`fs.read`, `math.checked_add`) — decide then whether the stdlib is simply exempt from the project's configured case (it reads as its own namespace, not user code) or should track it too.

### Full example (`main.x`)

```x
impl "shapes/circle"
impl "std/fs" with caps(fs = ["/var/log/myapp/*"])

pd val app_name = "shapes"
pd var request_count = 0

datac Circle(val r: f64)

fun Circle.area().R: f64 = 3.14159 * r * r

fun mut Circle.grow(by: f64) {
    r += by
}

fun log_area(c: Circle) {
    request_count += 1
    print("[$app_name #$request_count] area = ${c.area()}")
}

fun classify(n: i32).R: &str = when {
    n > 0 -> "pos"
    n < 0 -> "neg"
    else  -> "zero"
}

fun sum_to(n: i32).R: i32 {
    var acc = 0
    var i = 1
    while i <= n {
        acc += i
        i += 1
    }
    return acc
}

fun main() {
    var c = Circle(r = 2.0)
    val before = c.area()
    c.grow(by = 1.0)
    print("before = $before, after = ${c.area()}")
    log_area(c)
    print(classify(n = 0))
}
```

## Compiler crate layout

What is on disk today, plus the files later milestones add. `main.rs`, `lexer.rs`, `ast.rs`, `parser.rs`, `pretty.rs`, `types.rs`, `infer.rs`, `tir.rs`, `borrowck.rs`, `codegen_c.rs`, `driver.rs`, `resolve.rs`, `diag.rs`, and `tests/` exist now; `preprocess.rs` and `std/` do not yet.

Cargo workspace at repo root:

```
x/
  Cargo.toml                # workspace
  xc/                        # the compiler binary crate
    src/
      main.rs                # CLI: `xc build <file.x | project-dir>`, `xc run <same>`, later `xc fmt`
      lexer.rs                # source -> tokens (span-carrying, newline-sensitive)
      ast.rs                  # parsed syntax tree types
      parser.rs               # tokens -> AST (recursive descent + Pratt for expressions)
      pretty.rs               # AST pretty-printer (the M0 deliverable)
      types.rs                # type representations
      infer.rs                # type inference / checking, stdlib overload resolution
      tir.rs                  # typed IR; every node carries its resolved Ty and Span
      borrowck.rs              # move checker (M3), borrow checker (M4), drop (M6)
      codegen_c.rs             # typed IR -> C source text
      driver.rs                # orchestrates pipeline, invokes `cc`
      resolve.rs              # project/manifest loading, `impl` imports, module naming (M5.1), capability gating (M7)
      diag.rs                 # cross-file diagnostics: module id -> file/line/col (M5.1)
      preprocess.rs            # `%*alias*%` expansion, runs before the parser (M5.3)
    std/                       # stdlib, written in X (M6+, does not exist yet)
      fs.x  path.x  math.x  str.x  mem.x  io.x  time.x  rand.x
    tests/
      compile_and_run.rs       # end-to-end: compile .x fixtures, run binary, assert stdout
      stdlib/                  # per-module stdlib fixtures (M6+)
      fixtures/*.x             # 19 today — one loose file per single-file test
      projects/*/              # 13 multi-file projects today (M5.1: x.toml or manifest-less)
  examples/
    hello.x
    fib.x
```

Fixtures live in `xc/tests/fixtures/*.x` (19 today: `datac_basic`, `while_accumulator`, `named_args`, `when_expr`, `if_expr`, `assign_to_immutable`, `val_field_immutable`, `type_mismatch`, `use_after_move`, `double_move_in_loop`, `move_then_reinit`, `field_read_does_not_move`, `borrow_basic`, `borrow_conflict`, `same_call_aliasing`, `write_through_shared_borrow`, `move_while_borrowed`, `receiver_methods`, `receiver_mut_on_val`). `xc/tests/compile_and_run.rs` compiles each one with the real `xc` binary and checks stdout or the expected rejection.

Multi-file work lives in `xc/tests/projects/<name>/` instead, because a module graph needs a directory: `shapes` (manifest + nested imports + methods), `alias` (`impl "." as u`, qualified type in a signature), `same_name` (two modules both declaring `Item` + `describe`), `loose` (manifest-less sibling import), plus one rejected project per import rule — `cycle`, `unknown_module`, `duplicate_alias`, `dotdot`, `bad_path`, `unknown_member`, `unqualified_import`, `stdlib_import`, `caps_clause`.

Single crate is fine at this scale (a compiler is inherently one pipeline); no need to over-modularize into multiple crates.

## Imports & project structure for `.x` projects

Explicit reaction against Java/Kotlin's convention (package must mirror deeply-nested directory path, plus a Gradle/Maven build-tool layer with its own DSL files, wrapper scripts, and cache dirs). Instead:

- **One manifest file**, `x.toml`, at the project root — name, version, entry point. No separate build-tool layer; `xc` itself is the build tool. **As shipped (M5.1): the manifest is optional.** `xc build <dir>` needs one; `xc build <file.x>` walks up looking for one and, finding none, treats that file's own directory as the module root. Everything in `xc/tests/fixtures/` and `examples/` relies on the manifest-less path — a single loose file is still a complete program.
- **`src/` holds everything**, flat by default, with subfolders only when the user actually wants to group files — a file's path is never required to match any declared namespace.
- **Imports are path-based**, not namespace-based: `impl "shapes/circle"` pulls in `src/shapes/circle.x` by its file path relative to `src/`, and you refer to its items as `circle.Circle` (last path segment becomes the default alias). No `mod` declarations needed to "announce" a file into a tree — the file existing under `src/` is enough.
- **File names are therefore part of the API**: the last path segment becomes the alias, so file names must be valid identifiers — `snake_case`, no dashes, no dots.
- **v1 is local-files-only**: `impl` can only reach files inside the current project's `src/`. No external/third-party dependency support yet (no registry, no git-url deps, no lockfile). The stdlib is the one exception: `impl "std/fs"` reaches compiler-shipped modules, not project files. **As shipped (M5.1): `std/...` is reserved but unimplemented** — it is rejected with an explicit M6/M7 message rather than reported as a missing file, so the reservation is visible in the error.

Example:
```
myproject/
  x.toml
  src/
    main.x          # impl "shapes/circle"  ...  fun main() { ... }
    shapes/
      circle.x
      square.x
```

`resolve.rs` handles turning an `impl "shapes/circle"` path into a loaded/parsed file plus a name binding for its exported items; this is pure file-path resolution, no separate package-registry concept to build.

**How M5.1 implements the namespace (as shipped).** Strict module-qualified, not a flat merge: after loading, every module's top-level names are prefixed with the module's dotted path (`Circle` in `shapes/circle` → `shapes.circle.Circle`), and the *entry* module is left unprefixed — so a file's own items stay bare inside that file, and imported items are reachable only as `alias.Item`. Two modules may therefore both declare `Item` and `describe` with no collision (fixture: `same_name`). A module alias is shadowed by a local of the same name, exactly like any other name lookup. Referencing an imported item unqualified is a compile error that names the qualified form (`did you mean 'prov.Thing'?`). See the M5.1 milestone entry for the rest, including the one thing this rules out in v1: extension methods on an imported type (`fun circle.Circle.helper()`) are not expressible.

**General alias/expansion system (`alias.xconf` + inline `alias` declarations)**: a flat, global, purely-textual substitution mechanism, deliberately simple (no parameters, no hygiene, no scoping).

- **Reference syntax**: `%*name*%` used anywhere in `.x` source (including inside an `impl "..."` path string). The sigil exists specifically so it can never be confused with a real identifier/variable — the lexer/preprocessor recognizes it unambiguously and substitutes before real parsing happens.
- **Definition site 1 — `alias.xconf`** (project-root config file, zsh-alias-flavored): `name="replacement text"` per line. e.g. `shared="../../shared/utils"` or `chttps="http_client.new(timeout = 30, tls = true, retries = 3)"`.
- **Definition site 2 — inline in `.x` source**: a dedicated `alias` declaration keyword, distinct from `val`, so the parser stores the RHS as a raw token stream (macro text) instead of type-checking/evaluating it as an expression: `alias %*chttps*% = http_client.new(timeout = 30, retries = 3)`. Needing a separate keyword (rather than reusing `val`) is exactly the "extra stuff" needed so it isn't mistaken for a normal variable.
- **Mechanics**: a preprocessing pass (`preprocess.rs`) runs on the token stream *before* the real parser: (1) collect all alias definitions across `alias.xconf` and every source file's `alias` declarations into one flat table; (2) expand every `%*name*%` occurrence with its registered text; (3) feed the fully-expanded token stream into the existing lexer/parser pipeline. Cyclic aliases (an alias whose expansion transitively references itself) are rejected the same way import cycles are.
- **Known tradeoff, stated explicitly**: because this is pure text substitution, a bad alias can produce nonsense that only surfaces as a type error at the (already-expanded) use site — accepted as the cost of keeping this "pretty simple" rather than a hygienic macro system. Error messages should still point back to both the alias's definition span and its use span so this stays debuggable.
- Import-path aliasing is just this same mechanism applied to `impl "..."` strings — no separate implementation needed once general expansion exists.

Lands as M5.3 (`preprocess.rs`), on the token/file layer before real parsing — it was originally pencilled in alongside the import work, but the import layer shipped first as M5.1 with no dependency on it, and `%*alias*%` remains the last piece of the original M5. Note it is the one M5 item that is *textual*, not name-resolution: `resolve.rs` never sees an unexpanded `%*name*%`.

## Capability-based security (the headline feature)

Every `impl` can carry a `with caps(...)` clause granting access to privileged resource domains. Without a grant, the corresponding built-in namespace simply isn't a resolvable name inside that module — enforced by the same mechanism as any other undefined-identifier error, not by a runtime permission check that could be forgotten or bypassed.

```
impl "std/fs" with caps(fs = ["/var/log/myapp/**"])
impl "std/net" with caps(net = ["api.example.com:443"])
impl "std/rand"
```

**Capability domains (v1)**: `fs` (path/glob allowlist), `net` (host:port allowlist), `process` (spawn allowlist), `env` (variable-name allowlist). Two more are treated as capabilities rather than ambient services, matching what WASI does with `wasi:clocks` and `wasi:random`: `time` (clock reads) and `rand` (entropy).

Each domain has a corresponding bare built-in namespace (`fs`, `net`, `process`, `env`, `time`, `rand`) exposing gated wrapper functions — there is no other way to reach the underlying syscalls; user code never gets raw `open`/`connect`/`fork`/`getenv`. Namespaces are bare, not `x.`-prefixed: `x.fs` reads as field access on a variable named `x`.

**Enforcement mechanism, split by what's actually checkable at compile time:**
- *Reachability (structural, 100% compile-time)*: `resolve.rs` tracks a capability-domain set per module (computed from its incoming `with caps(...)` grant). Resolving `net.connect` when the current module's set doesn't include `net` is a resolution error — identical in kind to referencing an undefined variable. This is what makes "not reachable through reflection or a workaround" true: there's no reflection in this language, and the symbol genuinely isn't in scope.
- *Fine-grained matching (host/path allowlist patterns, compiler-inserted runtime check)*: the specific argument to `net.connect("api.example.com", 443)` is often a dynamic string, so the compiler embeds the granted allowlist as constant data for that module and inserts a check at each gated call site, generated as part of `codegen_c.rs`. The module itself never holds a capability *value* it could pass around or forge — only the compiler-generated call sites know the allowlist.

**Grant semantics** (the part that must be right):
- Patterns are absolute paths. `*` matches within one path segment, `**` crosses segments and is legal only as a full segment. No other metacharacters, no regex, no brace expansion.
- A pattern must not contain a `..` segment; rejected at the `impl` line, at compile time.
- At each gated call site: (1) lexically normalize the argument, (2) reject if it escapes the pattern root via `..`, (3) resolve symlinks (`realpath`), (4) match the resolved absolute path against the module's compiled-in allowlist, (5) only then perform the syscall. Steps 3 and 4 must not be reordered — resolving symlinks first is the reason a link pointing outside the grant is denied.
- Denied calls panic with the call-site span, the attempted path, and the module's allowlist.

**Delegation/no-escalation rule**: a module's capability set is *received* from whoever imports it, never self-declared. When module A does `impl "B" with caps(net = [...])`, the resolver requires that grant to be a subset of A's own capability set — a module can delegate less than it has, never more. The root/entry module (`main`) implicitly holds the full set granted by `x.toml` (or by default, everything, until a later milestone adds a way to restrict `main` itself).

**Scope note**: `fs` is the domain with a real implementation first (path/glob allowlist checked against actual file operations); `net`/`process`/`env` are structurally gated (resolution enforcement works) with stub bodies until there's a reason to flesh them out. `time` and `rand` are small enough to implement for real when they land.

This depends on the multi-file `impl` import system and on the heap (`fs.read` cannot return contents without an owned buffer), so both precede it — see the renumbered milestones below.

## Language v1 feature set

- Primitive types: `i32`, `i64`, `u32`, `u64`, `f32`, `f64`, `bool`, `char`, `u8`, `u16`, `usize`, `isize`, `Unit` (void-equivalent, implicit)
- Builtin owned heap type `Bytes` (non-`Copy`, moves, drops at end of scope) — the byte-level owned type, and the substrate for `String`
- Owned `String` (non-`Copy`, moves, drops, utf-8 validated) — required in v1 because `syntax&shii.md` uses it as a `datac` field and as a parameter type, and a `datac` may only hold owned fields
- `&str` string literal / borrowed-view type (points at static data or a substring of another `&str`)
- `val` (immutable) / `var` (mutable)
- `fun name(param: T, ...).R: T { ... }`, explicit param/return types, expression bodies, local type inference for `val`/`var`
- Receivers: `fun T.m()`, `fun mut T.m()` (sugar over an implicit first parameter)
- `datac` with named fields, each `val` or `var`
- Control flow: `if`/`else` (as statement and expression), `while`, `when` (boolean/`else` form), `return`
- Operators: arithmetic, comparison, logical (`&&`, `||`), unary `-`/`!`, compound assignment (`+=`, `-=`, `*=`, `/=`, `%=`)
- References: `&expr`, `&mut expr`, deref `*expr`; parameter/receiver positions use the bare/marked forms instead
- Moves: assigning/passing a non-`Copy` value transfers ownership; using it after is a compile error
- Borrowing: shared by default, exclusive marked `mut`, enforced aliasing (many immutable XOR one mutable), enforced "borrow can't outlive owner" using lexical (scope-based) lifetimes — not full non-lexical-lifetime inference; that's a stretch goal
- `print(expr)` / `print!(expr)` builtins for output, emitted directly to C `printf`
- String templates `"$name"` / `"${expr}"` desugaring to `print`/`printf` formatting
- Named arguments at call sites with 2+ params
- `pd` passdowns; `alias` substitution; string templates: **not yet** (M5.2/M5.3)
- `impl` imports; receiver-form methods: **done** (M5.1 / M4.5)
- `with caps(...)` grants: **not yet** (M7)

Not yet implemented despite being listed above as v1 scope: `pd`/`alias`/string templates, `with caps(...)`, plus `mem`/`Bytes`/`String` and the entire stdlib. See "Current state".

Deferred beyond v1 (explicitly out of scope for now, revisit later): generics, traits/interfaces, enums/pattern matching, closures, heap collections (`Vec`), explicit lifetime annotation syntax (`'a`), non-lexical lifetimes, async, unsafe blocks, exceptions, external/third-party dependencies (local-file `impl` imports are in scope — see Imports & Capabilities sections).

## Compiler pipeline (per milestone)

This section is the record of what actually happened, and it overrides `syntax&shii.md`'s own milestone deltas wherever those said a feature would land earlier. Milestone numbers here match the commits in `PASSDOWN.md`. Done: M0–M3. Next: M4.

**M0 — Scaffold + lexer/parser for the full syntax surface (DONE, `1b616c5`)**
- Shipped: workspace + `xc build <file.x>` CLI; span-carrying lexer; newline-terminated statements (paren-depth aware, one `Newline` per blank-line run at depth 0 — so a continued expression *must* be wrapped in parens; no trailing-operator continuation was attempted); `val`/`var`; `fun` + `.R: T` + expression bodies; `datac Name(val/var field: T, ...)` with per-field mutability; construction via call syntax `Point(x = 1, y = 2)` (no separate literal grammar, so no struct-literal-vs-block ambiguity exists); named + positional arguments in one call; `print`/`print!` where `print` adds the newline; compound assignment desugared at parse time; `when` desugared to an `if`/`else if`/`else` chain so no later pass ever sees it; `&str` accepted and lowered to plain `str`; AST pretty-printer plus unit tests.
- Not in M0, deferred: receiver methods, `pd`, `impl` imports, string templates, `caps(...)`.

**M1 — Types + C codegen + first running binary (DONE, `ca4af6a`)**
- Shipped: `types.rs` primitives + signatures; `infer.rs` signature checking, `val`/`var` inference, span-carrying type errors; `codegen_c.rs` mapping `.x` types to C; `driver.rs` writing C to a temp file and invoking `cc -O2`.
- Shipped alongside it: `tir.rs`, the typed IR every later pass walks, and `#line`-annotated C as planned.
- Deliverable met: `xc build examples/hello.x && ./hello` prints correctly, `fib.x` computes fibonacci, and `xc/tests/compile_and_run.rs` compiles each fixture with the real binary and checks stdout or the expected rejection (14 end-to-end tests today).

**M2 — `datac`, mutability, loops (DONE, `4c9f914`)**
- Shipped: `datac` → C `struct`, field access, `while` loops, `var` assignment, and **real per-field mutability**: a `val` field is never reassignable, and a `var` field is reassignable only when the root binding is also `var`.

**M3 — Ownership: moves (DONE, `f9d479c`)**
- Shipped: `borrowck.rs`, run between `infer.rs` and `codegen_c.rs`, operating purely on `tir` — Copy-ness is read off each node's resolved `Ty`, so it needs no struct/function tables.
- Copy rule: everything except `Ty::Struct(_)` is Copy. State is a per-scope `HashMap<String, VarState>` holding `is_copy` and `moved_at: Option<Span>`, mirroring `infer.rs`'s scope stack.
- Move points: a bare `Ident` read as a whole value. A `.field` read walks the place chain without moving anything, since v1 has no way to move a single field out. Assigning *to* a binding reinitializes it and clears moved state.
- `if`/`else`: each branch is checked from an independent snapshot; a variable counts as moved afterwards if either branch moved it.
- `while`: the body is checked twice in sequence, so a value double-moved across iterations is caught; the loop's post-state is then merged with the pre-loop snapshot because the body may run zero times.
- Diagnostics are single-span for now (`BorrowError { message, span }`); the two-span "moved here / used here" pair is polish, not a blocker.
- Fixtures: `use_after_move.x` and `double_move_in_loop.x` (rejected), `move_then_reinit.x` and `field_read_does_not_move.x` (accepted).
- Not implemented, and worth remembering: partial/per-field moves. The per-field `val`/`var` bookkeeping from M2 is the natural hook for it.

**M4 — Borrowing (DONE)**
- **Resolved the open prerequisite** ("no spelling for a move-taking parameter") by taking the option `syntax&shii.md`/this file's own "Gap, unresolved" note listed: *no move-taking parameters in v1*. A `datac`-typed parameter is now always a borrow — implicit shared, or exclusive when written `mut T` (the *only* spelling; `&mut T` in parameter position is a parse error, since `parse_type`'s leading-`&` handling expects an identifier next, not the `mut` keyword token). The only way to move a `datac` value is still local rebinding (`val q = p`), which M3 already checked. This is a real behavior change from M3: calling a function with a `datac` argument no longer moves it.
- `types.rs`: new `Ty::Ref { mutable, inner: Box<Ty> }`, always Copy (copying a pointer doesn't move what it points at). `.R: T` never produces one — `resolve_type` for return types is untouched by the new `resolve_param_type` path — so **a borrow can never be returned to a caller in M4**. That's deliberate: it's what makes call-scoped borrow release (see below) sound without real lifetime inference, at the cost of `&str` return elision staying out of scope (moot for now anyway, since `&str` isn't routed through `Ty::Ref` at all — it's still its own flat Copy `Ty::Str`).
- `parser.rs`: `&expr`/`&mut expr`/`*expr` as prefix unary operators (same slot as `-`/`!`); `mut` recognized only in parameter position, right after `:`.
- `infer.rs`: call sites never write `&` — passing a plain place to a `Ty::Ref`-typed parameter auto-borrows it (`check_call_arg` synthesizes the `tir::ExprKind::Ref` node; this is the *checker* desugaring, not codegen inspecting signatures — codegen only ever sees the already-resolved tir). Forwarding an already-borrowed value type-matches directly; a shared parameter accepts an exclusive argument. `.field` access/assignment auto-derefs through a `Ref` base each node decides independently, purely from its own base's type — so a `mut`-borrowed param's fields are writable and a `.field.field` chain through it "just works" with no extra plumbing. `check_target_writable` replaced the old "root must be `var`" assignment rule: writability now comes from either a plain owned `var` root, or a statically-exclusive `Ref` root.
- `borrowck.rs`: adds `BorrowKind::{None, Shared(n), Exclusive}` per owner. Two lifetimes: **call-scoped** (an auto-borrow or explicit `&expr` passed straight into a call — every borrow in one call's argument list is *started before any of them release*, so the existing conflict check catches same-call aliasing like `f(&mut c, &c)` for free, no separate pass needed) and **scope-durable** (`val p = &c` — released when `p`'s enclosing block pops, reusing the move-checker's scope stack). Moving or reassigning a variable while it's actively borrowed is rejected; reading `*p` as a whole value (not just through it) is rejected when the pointee is non-Copy, mirroring "cannot move out of a shared reference."
- `codegen_c.rs`: `FieldAccess`/assignment-target rendering picks `.` vs `->` per-node from the base's `Ty`; `&expr`/`*expr` map to C `&`/`*`.
- **Cut from this milestone, deliberately** (receivers landed right after, as M4.5 below — see that entry): receiver-form methods — a separable parsing/method-resolution feature once the borrow mechanics alone turned out to be the bulk of the work. `&str` return elision (see above — moot until `&str` routes through `Ty::Ref`). A known, narrow soundness gap, exercised by no fixture: `p = &c` *reassigning* an existing binding to a fresh borrow is checked against currently-active borrows at the moment of assignment but doesn't persist scope-durable tracking the way a `val`/`var` *declaration* does (`val p = &c`, which is fully tracked).
- Fixtures: `borrow_basic.x` (shared + exclusive via a `mut` free-function parameter, plus a local `&`/`*` shared borrow), `borrow_conflict.x` (simultaneous shared + exclusive, rejected), `same_call_aliasing.x` (`f(&mut c, &c)` in one call, rejected — the fixture that specifically exercises the call-scoped conflict check), `write_through_shared_borrow.x` (mutating through an unmarked/shared param, rejected), `move_while_borrowed.x` (rejected). `use_after_move.x`/`double_move_in_loop.x` were rewritten to move via rebinding instead of a function call, since a call no longer moves its argument.

**M4.5 — Receiver-form methods (DONE)**
- `ast.rs`: `FunctionDecl.receiver: Option<Receiver>` (`Receiver { type_name, mutable, span }`), kept separate from `params` since the receiver sits outside the parens in the surface syntax. New `Expr::MethodCall { receiver, method, args, span }`.
- `parser.rs`: `fun [mut] Type.name(...)` is disambiguated from a plain `fun name(...)` by one token of lookahead (a `.` right after the first identifier commits to receiver form; `mut` right after `fun` always commits to it, since a free function never writes `fun mut name(...)`). `recv.method(args)` is disambiguated from `recv.field` in `parse_postfix` by whether a `(` immediately follows the name — both share a new `parse_call_args` helper factored out of the plain-call-parsing path.
- `infer.rs` (the substantial part):
  - `sig_key(f)` is the lookup key in `sigs`/`tir::Function.name`: `Type#name` for a method, bare `name` for a free function. `#` can't appear in a real `.x` identifier, so a method can share a bare name with an unrelated free function, or with a same-named method on a *different* receiver type, with no collision — only two methods on the *same* type with the same name collide, which is exactly the duplicate-definition case that should error.
  - The receiver is resolved exactly like a parameter (`resolve_receiver_type`, sharing the `implicit_borrow_wrap` logic `resolve_param_type` was refactored to use) and prepended as a real `("self", Ty)` entry at `params[0]` — both in the `FnSig` and in the `tir::Function` the body actually gets checked against. This means a receiver method is, underneath, indistinguishable from a free function whose first parameter happens to be named `self`; codegen needed zero changes for receivers specifically.
  - **No `self` keyword needed at call sites, and none needed to *read* a field either**: per `syntax&shii.md` §5 ("no `self` keyword"), `fun Circle.area().R: f64 = 3.14159 * r * r` uses the bare field name `r`, not `self.r`. `check_expr`'s `Ident` arm now falls back, when a name isn't a local/parameter, to checking whether the current function has a receiver whose `datac` has a field of that name — if so it synthesizes a `FieldAccess` on an internal `self` binding. (`self` itself remains available as an ordinary identifier for the rare case a method needs the whole receiver value, even though nothing requires writing it.)
  - `check_method_call` resolves `method` against the receiver expression's type-checked (but not yet borrow-producing) probe to find the `Type#method` key, then calls `check_call_arg("self", ...)` on the *actual* receiver expression for the real borrow — reusing M4's auto-borrow/aliasing machinery with zero special-casing, then lowers to a plain `tir::ExprKind::Call`. A `fun mut T.m()` call therefore participates in the same call-scoped conflict checking as any other exclusive borrow (e.g. `combine(c, c)`-style same-call aliasing would be caught here too, though no fixture exercises that specific combination for methods).
- `codegen_c.rs`: `c_name` maps a method key's `#` to `__` (e.g. `Circle#area` → `x_Circle__area`) — not fully collision-proof against a hand-named `Circle__area` free function, but low-risk and simple; noted as a known limitation rather than engineered around.
- Fixtures: `receiver_methods.x` (the spec's own `Circle.area`/`Circle.grow` example, adjusted for the doc bug noted in "Current state" above), `receiver_mut_on_val.x` (calling a `mut` receiver method on a `val`-bound local, rejected — exactly the free-function `write_through_shared_borrow.x` check, now exercised through method-call syntax).

**M5.1 — Multi-file imports (DONE)**

Split out of the original M5 on the same reasoning that split M4/M4.5: the file/import layer is independently testable, and the remaining M5 items (`pd`, templates, aliases) do not depend on it landing together. Shipped:

- **`diag.rs` (new).** A `Span` is only meaningful inside the file it was lexed from, so every diagnostic now carries the canonical id of the module it came from, and `SourceMap` maps that id back to path+text to print `--> file:line:col`. `ast::DatacDecl`/`ast::FunctionDecl` and `tir::Function` carry a `module` field for exactly this; `infer.rs` and `borrowck.rs` tag their errors with it at the function/decl boundary, so their internals are otherwise unchanged. Verified: a borrow error inside an imported module points at the imported file, not the entry file.
- **`resolve.rs` (new).** Loads the entry file, follows `impl` transitively, and returns one *merged* `ast::Program` plus a `Project` naming table. `impl "shapes/circle"` resolves to `src/shapes/circle.x` relative to the project's `src/`; the last path segment is the default alias, `as` overrides it (line-segment validation rejects absolute paths, `..`/`.`, non-identifier segments, a trailing `.x`, and reserved `std/...` with a "not implemented until M6/M7" message). Import cycles are rejected with the chain printed (`import cycle: a -> b -> a`), duplicate aliases within one file are rejected, and a module reached twice (diamond) is loaded once.
- **Merging, not a real module system.** Each module's top-level names are prefixed with the module's dotted path (`Circle` in `shapes/circle` becomes `shapes.circle.Circle`; the *entry* module is unprefixed) and `FunctionDecl.module` records the origin. Nothing else about the pipeline changed shape: infer → borrowck → one C translation unit → one `cc` call. `resolve.rs` rewrites a receiver's type name too, so `sig_key`'s existing `Type#name` scheme keeps working unchanged.
- **Strict module-qualified namespace** (chosen over a flat merge): a module's items are reachable from another module only as `alias.Item`, never bare, and a file's own items are reachable bare inside that file. `circle.Circle(r = 2.0)` and `circle.scale(x)` are parsed as method/field syntax and re-interpreted in `infer.rs::check_expr` when the base identifier is an `impl` alias that no local shadows — so a local `val util = 5` still shadows a `util` module alias, exactly like any other name lookup. Type annotations use the same alias table (`fun describe(t: u.Tally)`). Unqualified references to an imported item get a targeted "did you mean 'prov.Thing'?" hint; so does a module used as a value.
- **`x.toml`** (`name`, `version`, `entry`, hand-parsed — no TOML dependency, unknown keys rejected): `xc build <dir>` uses the manifest's entry point, and `xc build <file>` walks up to find the manifest and requires the file to live under `<root>/src`. **A manifest is optional**: with no `x.toml` found, the entry file's own directory is the module root, which is what keeps all 19 single-file fixtures and both `examples/` compiling with no manifest at all.
- **Codegen mangling**: `types::c_ident` turns `.` and `#` into `__` (`shapes.circle.Circle#area` → `x_shapes__circle__Circle__area`, struct tag `struct_shapes__circle__Circle`). Same low-risk/collision caveat M4.5 recorded for `#`, now covering module prefixes too.
- Tests: 34 fixture/project tests (`cargo test` = 38 unit + 34) — `shapes` (manifest, nested import inside an imported module, receiver methods on an imported type), `same_name` (two modules both declaring `Item` and `describe`, proving the prefixing avoids collisions), `alias` (`impl "util" as u` plus a qualified type in a signature), `loose` (manifest-less sibling import), and rejected `cycle`, `unknown_module`, `duplicate_alias`, `dotdot`, `bad_path`, `unknown_member`, `unqualified_import`, `stdlib_import`, `caps_clause`.
- **Deliberately not in M5.1**: `pd` wiring, string templates, `%*alias*%`/`alias.xconf`. Also worth remembering: **extension methods on an imported type are not expressible** — `fun circle.Circle.helper()` cannot be parsed (the receiver grammar takes one bare type name), so helper methods must live in the module that declares the type. That is a v1 restriction, not a bug, and it is the natural thing to revisit if user-definable-cross-module extension methods ever matter.

**M5.2 — `pd` passdowns and string templates**
- `pd val`/`pd var` at the top of a file, in canonical position (after `impl`, before `datac`/`fun`). Wiring only into functions that reference the name.
- **`pd var` semantics are settled** (see "Current state"): read-only inside functions, assignable only at its own top-level declaration. Concretely the wiring pass hands every referencing function a shared borrow, and an assignment to a passdown name anywhere inside a `fun` is a compile error — no new borrowck state, no reachable mutable global.
- String templates `"$name"` / `"${expr}"` desugaring into the existing `print`/`printf` machinery. Blocked in practice on deciding what a template with a non-`str`, non-numeric value is (today `print` rejects `datac`/`Unit`/references outright), so scope this to the printable types v1 already has unless `String` arrives first (M6).
- Fixtures: a passdown read by two functions and by none, a `pd var` assignment inside a function (rejected), a template with a field access inside `${...}`.

**M5.3 — `%*alias*%` substitution (`preprocess.rs`)**
- `preprocess.rs`: collect alias definitions from `alias.xconf` + inline `alias` declarations, expand every `%*name*%` occurrence (including inside `impl "..."` strings), reject cycles, feed the expanded token stream to the parser. Errors must span both the definition and the use site, per the tradeoff recorded in "Imports & project structure".
- Fixtures: an `alias.xconf` case, an alias inside an `impl` path string, a rejected alias cycle.

**M6 — Heap: builtin `Bytes` and owned `String`**
- `Bytes` and `String` as compiler builtins: allocation on construction, `free` at end of scope, non-`Copy`, moves, no user-defined destructors, no growable buffers (stdlib decision 5).
- Drop insertion in `codegen_c.rs`; drop ordering relative to borrows checked in `borrowck.rs`.
- `extern` declarations with compiler-supplied bodies, so the stdlib can be written in `.x` (stdlib decision 3).
- `mem`, plus the allocation-returning parts of `path`/`str`.
- Gate before M7: a fixture that moves a `Bytes` into a function and uses it after the move is rejected, and no double-free is expressible. This is where ownership stops being a paper exercise, so it is also where M3's move logic and M4's borrow logic surface their bugs.

**M7 — Capability-based security**
- Parse `with caps(domain = [patterns...])` on `impl` lines.
- `resolve.rs`: maintain a per-module capability-domain set; gate resolution of `fs`/`net`/`process`/`env`/`time`/`rand` identifiers on that set; enforce the subset/no-escalation rule when a module delegates capabilities to something it imports.
- Implement `fs` for real (path/glob allowlist, `..` rejection, symlink-resolution-before-match); leave `net`/`process`/`env` structurally gated with stub bodies; implement `time`/`rand` for real.
- `codegen_c.rs`: emit the compile-time-known allowlist as constant data and insert the runtime pattern check at each gated call site.
- Fixtures: a module granted `fs` access to a glob that successfully reads a file; a module *not* granted `fs` where referencing `fs.*` is a resolution error; a module granted a narrower glob than it was given attempting to over-delegate to a sub-import (rejected); a symlink inside the grant pointing outside, denied after resolution.

**M8 (stretch, only after M0–M7 are solid)** — user-definable owned types, i.e. `Box<T>`-style owners beyond the two builtins. This wants generics, so it is where the first generic machinery gets scoped, or a monomorphic single-parameter special case is hand-rolled. Explicitly a "come back to this later" milestone.

Rejected numbering: an earlier revision of this file put moves and borrow checking at M1 and retired M3/M4, on the theory that `syntax&shii.md`'s deltas won. The implementation did not do that — M3 (moves) shipped as M3 — so the record above is the numbering that counts.

## Standard library

Merged from the former `stdlib.md` (now a pointer stub outside the project). What ships *as* the stdlib: namespaces, API sketches, capability wiring, and the language features the stdlib forces.

### Decisions locked

1. **`Bytes` before `fs`.** The builtin owned byte buffer lands *before* the fs milestone, not after. Rationale: `fs` with no content transfer (`exists`/`size`/`mkdir`/`remove` only) is a fake feature that would have to be rewritten, and the heap milestone is needed regardless to make ownership/moves have runtime consequence. Doing the heap once, before `fs`, means `fs` is implemented once. Cost is contained by scope: one builtin type with one fixed drop rule, no user-defined destructors, no generics, no growable-buffer machinery.
2. **Failure model: panic + error `datac`.** Panic for programmer errors and unrecoverable IO; a small error `datac` for recoverable cases people actually branch on. No `-1` sentinels, no out-param `try_` family. Both are replaced by `Option`/`Result` when generics arrive.
3. **Stdlib implemented as `.x` sources shipped with the compiler**, with an `extern` declaration form whose body the compiler supplies. The raw syscall exists only inside the gated module (`std/fs.x`), and the compiler inserts the allowlist check at that boundary — which keeps the capability story structurally honest rather than a wrapper convention.
4. **Stdlib-only argument-type overloading.** `math.abs`/`min`/`max`/`clamp` resolve on argument type. The rule lives in `infer.rs` and is available only to compiler-shipped modules, so user code gains no new complexity and no user-defined overloading is implied.
5. **Buffers are fixed-size in v1.** No reallocation, no length/capacity distinction. Growable ops (`String.push`, `String.concat`, `Bytes.append`) land with the collections milestone.

### Constraints the language puts on the stdlib

These are not preferences, they are walls. Everything below is shaped by them.

| Constraint | Consequence for stdlib |
| --- | --- |
| No arrays, no slices, no `Vec` | Nothing can return *N* items. Directory listing, argv, `split` are blocked. |
| Owned `String` exists but is fixed-size in v1 | Text can be built and returned; it cannot be grown in place. |
| `&str` is a borrowed view only | String functions return substrings of their input, or an owned `String`/`Bytes`. |
| No `Option` / `Result` / generics | No `?`, no null. Failure must be a panic or an error `datac`. |
| No closures | No `for_each_entry(path) { }` callback escape hatch. |
| No exceptions | Fatal failures panic; recoverable ones use the error `datac` convention. |
| Capability gating is the headline feature | Gated namespaces must be unreachable without a grant. `fs` cannot be a thin shim over ambient syscalls. |
| No `const` yet | `math.PI` is not expressible without a new declaration form. |

**The single decision that unblocked the most:** one compiler-builtin owned byte buffer. It unlocks `fs.read`, `fs.write`, `path.join`, `str.to_upper`, `env.get`, and it is the substrate `String` is built on. Locked as decision 1: it ships before `fs`, at M6.

### Namespace model

Bare lowercase namespaces, no `x.` prefix (the `x.fs` spelling reads as field access on a variable named `x`).

Two tiers:

- **Prelude, always in scope, ungated** — `print`, `print!`, `panic`, `assert`, and the pure namespaces `math`, `str`, `path`, `mem`, `io`. No import needed, no capability. These cannot reach the outside world; they are arithmetic and byte shuffling.
- **Gated, requires an explicit grant** — `fs`, `env`, `process`, `net`, `time`, `rand`. Reached only through an `impl` that carries `with caps(...)`:

```x
impl "std/fs" with caps(fs = ["/var/log/myapp/**"])
impl "std/rand"
impl "std/fs"          // legal but useless: fs.* will not resolve
```

Reading a clock and reading entropy are **capabilities**, not ambient services — the same classification WASI uses (`wasi:clocks`, `wasi:random`). A program that cannot observe time or randomness gets reproducibility for free.

Cheap conveniences stay ungated on purpose: `io.print`/`io.read_line` (stdin/stdout/stderr), `time.sleep`, `process.exit`, `process.getpid`. Sleeping, exiting, and writing to your own stdout are not authority over anything shared.

### `Bytes` and `String` — the two builtin owned types

Compiler-provided, not user-definable via `datac` (a `datac` cannot hold a pointer in v1, and these must).

```x
// compiler builtins
datac Bytes      // opaque: owned heap byte buffer, freed at end of scope (drop)
datac String     // opaque: owned heap utf-8 text, same move/drop rules

fun Bytes.len().R: usize
fun Bytes.get(i: usize).R: u8              // panics on out-of-bounds
fun Bytes.set(i: usize, b: u8)
fun Bytes.slice(from: usize, to: usize).R: &Bytes
fun Bytes.as_str().R: &str                 // panics on invalid utf-8
fun Bytes.clone().R: Bytes
fun Bytes.from_str(s: &str).R: Bytes
fun Bytes.empty().R: Bytes

fun String.len().R: usize                  // bytes, not chars
fun String.char_count().R: usize
fun String.is_empty().R: bool
fun String.as_str().R: &str
fun String.as_bytes().R: &Bytes
fun String.clone().R: String
fun String.from_str(s: &str).R: String
fun String.from_bytes(b: Bytes).R: String  // panics on invalid utf-8
fun String.into_bytes().R: Bytes           // move, no copy
fun String.slice(from: usize, to: usize).R: &str
fun String.empty().R: String
```

Rules:
- Both are non-`Copy`. Assigning or passing one moves it. Pass `&Bytes`/`&String` to read, `mut String` to write.
- Moves and drops are fully checked. This is the first place ownership has runtime consequence, so M6 is also where M3's move logic and M4's borrow logic surface their bugs.
- Drop happens at end of scope, after any borrows of it end. A value moved out of a scope is not dropped by that scope.
- Size is fixed at construction (decision 5). Growable ops are deferred, which is why `String.push`-style functions do not appear in the API yet.
- `String` is not a wrapper *keyword* over `Bytes` in the syntax sense — it is a second builtin type with its own move/drop identity. `into_bytes`/`from_bytes` convert with an ownership transfer.
- `&str` is the borrowed view type: static literals, substrings of another `&str`, or `String.as_str()`.

Blocks that stay blocked even with these types: anything returning *N* items. `fs.list`, `str.split`, `env.args` need a growable sequence type, which is the collections milestone.

### Module map by milestone

| Tier | Ships | Milestone |
| --- | --- | --- |
| 0 | `print`/`print!`/`panic`/`assert`; `math` (numeric + consts); `str` (substring-only); `path` (pure predicates) | M1 (partially available) |
| 1 | `Bytes` + `String` + `mem`; allocation-returning `path` and `str` functions | M6 |
| 2 | `fs` real, `env` real, `io` line reads; `process`/`net` structurally gated stubs | M7 |
| 3 | `time`, `rand` (both gated, both real) | M7 |
| 4 | collections, `fmt`-style formatting, `fs.list`, `process.run(argv)`, `env.args`, growable strings | post-v1 |

Tier 1 strictly precedes tier 2. A compiler that can `fs.read` must already have working move-and-drop for `Bytes`.

### Failure model

No `Option`, no `Result`, no exceptions. Available conventions:

1. **Panic** — `fs.read` on a missing file panics with the call-site span and the granted allowlist printed. Simple, correct, unusable for expected failures.
2. **Sentinel** — `str.index_of` returns `i64` with `-1` for absent. Cheap, breaks down for non-numeric returns.
3. **Out-param + bool** — `fun path.try_join(a: &str, b: &str, out: mut Bytes).R: bool`. Composable, ugly, C-flavored.
4. **Error `datac`** — `datac Match(val ok: bool, val at: usize)` then `if m.ok { }`. No pattern matching needed, names the failure, works today.

Locked (decision 2): panic for programmer errors and unrecoverable IO; convention 4 for the recoverable cases people actually branch on (`str.index_of`, `str.parse_i64` on bad input). Conventions 2 and 3 are rejected: no `-1` sentinels, no out-param `try_` family. Document `Panics:` in the doc comment of every panicking function.

When generics arrive, replace every error `datac` with `Option<T>` / `Result<T, E>` and keep the old names as deprecated wrappers.

### API sketches

#### math (pure, ungated)

```x
const PI: f64 = 3.141592653589793
const E: f64 = 2.718281828459045
const TAU: f64 = 6.283185307179586

// overloaded across i32/i64/u32/u64/f32/f64 (decision 4)
fun math.abs(x: f64).R: f64
fun math.min(a: f64, b: f64).R: f64
fun math.max(a: f64, b: f64).R: f64
fun math.clamp(x: f64, lo: f64, hi: f64).R: f64
fun math.sign(x: f64).R: f64

fun math.sqrt(x: f64).R: f64
fun math.pow(base: f64, exp: f64).R: f64
fun math.floor(x: f64).R: f64
fun math.ceil(x: f64).R: f64
fun math.round(x: f64).R: f64
fun math.trunc(x: f64).R: f64
fun math.exp(x: f64).R: f64
fun math.ln(x: f64).R: f64
fun math.log2(x: f64).R: f64
fun math.log10(x: f64).R: f64
fun math.sin(x: f64).R: f64
fun math.cos(x: f64).R: f64
fun math.tan(x: f64).R: f64
fun math.atan2(y: f64, x: f64).R: f64

fun math.is_nan(x: f64).R: bool
fun math.is_finite(x: f64).R: bool

fun math.checked_add(a: i64, b: i64).R: i64   // panics on overflow
fun math.saturating_add(a: i64, b: i64).R: i64
```

Everything else stays `f64` until `f32` overloads are decided (open item 7). Integer `abs`/`min`/`max`/`clamp`/`sign` come from the overloading rule in decision 4, not from suffixed names like `math.iabs`/`math.imin`.

#### Arithmetic overflow semantics (language-level, stdlib-visible)

C signed overflow is UB, so it must be defined:

- Build with `-fwrapv` so `i32`/`i64` overflow wraps deterministically.
- Unsigned already wraps.
- `+` never panics. Overflow is visible only through the explicit `math.checked_*` / `math.saturating_*` family.
- Alternative (later, if desired): debug builds panic on overflow, `-O2` builds wrap. Requires codegen to emit checked arithmetic in debug mode — a real cost, so not v1.

#### str (borrowed `&str` in; `&str`, `String`, or `Bytes` out)

```x
fun str.len(s: &str).R: usize              // bytes, not chars
fun str.is_empty(s: &str).R: bool
fun str.char_count(s: &str).R: usize
fun str.trim(s: &str).R: &str
fun str.trim_start(s: &str).R: &str
fun str.trim_end(s: &str).R: &str
fun str.starts_with(s: &str, prefix: &str).R: bool
fun str.ends_with(s: &str, suffix: &str).R: bool
fun str.contains(s: &str, needle: &str).R: bool
fun str.index_of(s: &str, needle: &str).R: Match   // datac Match(val ok: bool, val at: usize)
fun str.eq_ignore_case(a: &str, b: &str).R: bool
fun str.char_at(s: &str, i: usize).R: char
fun str.byte_index(s: &str, i: usize).R: usize
fun str.repeat(s: &str, n: usize).R: String
fun str.replace(s: &str, from: &str, to: &str).R: String
fun str.to_upper(s: &str).R: String
fun str.to_lower(s: &str).R: String
fun str.from_i64(n: i64).R: String
fun str.from_f64(x: f64).R: String
fun str.parse_i64(s: &str).R: i64                  // panics on garbage
fun str.parse_f64(s: &str).R: f64
```

Text-returning functions return `String`, not `Bytes` — that is the fix forced by `String` being v1. Byte-level functions (`fs.read`, `net.recv`, `rand.bytes`) still return `Bytes`.

Receiver form is available and preferred in user code: `name.trim()`, `s.contains("x")` — extension functions on `&str`.

**Borrow rule this forces:** a function returning `&str` without an explicit lifetime must borrow from its first reference parameter (Rust's elision rule). `fun str.trim(s: &str).R: &str` returns a slice of `s`. Without this rule the type checker cannot accept `str`/`path` at all, so it is a hard prerequisite, not a nicety. Iteration over chars waits for collections.

#### path (pure; allocation-returning parts need `Bytes`/`String`)

```x
fun path.is_absolute(s: &str).R: bool
fun path.file_name(s: &str).R: &str
fun path.parent(s: &str).R: &str
fun path.extension(s: &str).R: &str
fun path.stem(s: &str).R: &str
fun path.has_extension(s: &str, ext: &str).R: bool
fun path.join(a: &str, b: &str).R: String      // needs owned allocation
fun path.normalize(s: &str).R: String          // needs owned allocation
```

Path separator is `/` on every platform; codegen translates to `\` for Windows at the syscall boundary, not in the API. `path.normalize` is purely lexical (`a//b` → `a/b`, drop `.`, reject `..` escaping) and is the *same* function the capability checker uses, so the security-relevant normalization has exactly one implementation.

#### fs (gated: `fs`)

```x
fun fs.read(path: &str).R: Bytes
fun fs.read_text(path: &str).R: String
fun fs.write(path: &str, data: Bytes)
fun fs.write_text(path: &str, text: String)
fun fs.exists(path: &str).R: bool
fun fs.is_file(path: &str).R: bool
fun fs.is_dir(path: &str).R: bool
fun fs.size(path: &str).R: u64
fun fs.remove(path: &str)
fun fs.rename(from: &str, to: &str)            // both paths checked
fun fs.mkdir(path: &str)
fun fs.mkdir_all(path: &str)
```

No `try_` variants — the out-param family was rejected in decision 2. A recoverable read is expressed by checking first (`fs.exists`) or by accepting the panic. `fs.list` is absent: no sequence type. `fs.remove` for a directory tree is `fs.remove_all`, deferred. `fs.append` is deferred with the growable-buffer work (decision 5).

**Grant semantics** (the part that must be right):

- Patterns are absolute, glob only. `*` matches within one path segment, `**` crosses segments and is only legal as a full segment. No other metacharacters, no regex, no brace expansion.
- A pattern must not contain a `..` segment. Rejected at the `impl` line, at compile time.
- At each gated call site: (1) lexically normalize the argument via the same `path.normalize`, (2) reject if it escapes the pattern root via `..`, (3) resolve symlinks with `realpath`, (4) match the resolved absolute path against the module's compiled-in allowlist, (5) only then perform the syscall.
- Symlinks are resolved **before** matching, so a link pointing outside the grant is denied. This is the whole reason step 3 exists and it must not be reordered.
- Denied call panics with the call-site span, the attempted path, and the module's allowlist. `Panics: if path is outside the granted capability` in the doc comment.
- The allowlist is emitted as constant data per module. No API can widen it at runtime, and no stdlib function accepts a capability as a value, so a grant cannot be smuggled through a function argument.

Glob metacharacters inside a granted path *argument* are literal — `fs.read("*.txt")` reads a file named `*.txt`.

#### io / env / process / net / time / rand

```x
// io — ungated
fun io.print(s: &str)
fun io.print_bare(s: &str)
fun io.eprint(s: &str)
fun io.read_line().R: String        // needs String
fun io.flush()

// env — gated: env, variable-name allowlist
fun env.has(name: &str).R: bool
fun env.get(name: &str).R: String          // needs String
fun env.set(name: &str, value: &str)
fun env.arg_count().R: usize               // escape hatch, no slices needed
fun env.arg(i: usize).R: String

// process — gated: process
fun process.exit(code: i32).R: Unit        // ungated in spirit; keep ungated
fun process.getpid().R: i64
fun process.run_shell(cmd: &str).R: i32    // gated; shell string, documented as injection-prone
// process.run(argv) blocked until slices exist

// net — gated: net, host:port allowlist
fun net.connect(host: &str, port: u16).R: Socket
fun net.close(s: mut Socket)
fun net.send(s: Socket, data: Bytes).R: usize
fun net.recv(s: Socket, max: usize).R: Bytes
fun net.listen(port: u16).R: Listener      // needs an accept loop -> deferred
// TLS variants deferred

// time — gated: time for clocks, ungated for sleeping
fun time.now_unix().R: i64
fun time.now_mono_ns().R: i64
fun time.sleep_ms(ms: u64)

// rand — gated: rand
fun rand.u64().R: u64
fun rand.below(n: u64).R: u64
fun rand.bytes(n: usize).R: Bytes
```

`process.run_shell` is the only v1 path to running a program and it is a string handed to a shell. It is gated, it is documented as injection-prone, and it is replaced by the argv form the moment slices exist. Do not add `exec`-style variants that bypass the `process` grant.

### Distribution and implementation strategy

Stdlib as `.x` sources shipped with the compiler (`std/fs.x`, `std/math.x`, ...), with a small intrinsic escape hatch for the actual syscalls. Dogfoods the language, keeps the API reviewable as X code, and lets the capability wrapper live in the same file as the gated namespace.

```x
// std/fs.x
extern fun fs_read_raw(path: &str).R: Bytes     // compiler-provided body

fun fs.read(path: &str).R: Bytes {
    return fs_read_raw(path)      // compiler inserts the allowlist check at this call site
}
```

Locked (decision 3). It needs one addition — an `extern` declaration form whose body is supplied by the compiler — and it makes the capability-gating story structurally honest: the raw syscall exists only inside `std/fs.x`, and the check is compiler-inserted at the boundary.

The rejected alternative: emit stdlib bodies directly from `codegen_c.rs` as C snippets. No new syntax, but the API becomes unreviewable non-X code and every doc/example is divorced from the body.

Details:
- Stdlib version is pinned to the compiler version. No `[stdlib]` section in `x.toml`, no registry, no third-party deps in v1.
- Externals are linked through `cc`; `math` needs `-lm`.
- Generated C for stdlib is cached per compiler build, not per user build.

### Naming and style rules for the stdlib

- Namespaces and functions `snake_case`. No `get_` prefix on getters (`fs.size`, not `fs.get_size`). `is_`/`has_` for predicates.
- Verb-first for effectful functions (`fs.read`, `process.run`); noun-first reads as data access.
- `try_` prefix is **not** used: the out-param family was rejected (decision 2). Recoverable functions return an error `datac` instead, and their doc comment names the datac.
- `_all` suffix for recursive variants (`fs.mkdir_all`, later `fs.remove_all`).
- Receiver form for the fluent cases (`s.trim()`, `p.parent()`), plain namespace call for the rest.
- Every panicking function documents `Panics:` in its doc comment.
- Every gated function documents the capability domain and the matched pattern form.
- No function in the stdlib takes a capability as a parameter. Grants are per-module and compile-time only.
- `read`/`write` are byte-level (`Bytes`); `read_text`/`write_text` are text-level (`String`).

### Language features the stdlib forces

| Feature | Needed by | Cost | Milestone |
| --- | --- | --- | --- |
| Builtin owned `Bytes` + drop | `fs`, allocation-returning `str`/`path` | Real: heap, drop insertion, move checking with runtime consequence | M6 |
| Builtin owned `String` | `datac` fields, `str`/`env` text results | Small once `Bytes` exists | M6 |
| `&str` return elision (borrow from first ref param) | all of `str`, `path` | Small, but unblocks a whole namespace | post-v1 — moot until `&str` routes through `Ty::Ref` |
| `const NAME: T = expr` at top level, compile-time folded | `math.PI`, `math.E` | Small parser + const-fold in `infer.rs` | post-M5.3 (stdlib prereq) |
| `extern` declarations with compiler-supplied bodies | stdlib written in X | Small parser surface, new codegen path | M6 |
| Argument-type overloading, stdlib-only | `math.abs/min/max/clamp` across numeric types | Medium: overload resolution in `infer.rs`; kept stdlib-only to avoid user-facing complexity | post-M5.3 (stdlib prereq) |
| `u8`, `u16`, `usize`, `isize` | byte buffers, indices, `Bytes.len` | Small; `usize`/`isize` needed regardless for indexing | M6 (stdlib prereq) |
| Slice/array type | `fs.list`, `process.run(argv)`, `env.args`, `str.split` | Large — collections milestone | post-v1 |
| `Option`/`Result` + generics | replaces error `datac`s | Large | post-v1 |

### Testing the stdlib

- Fixture suite in `tests/stdlib/` per module, same compile-run-diff-stdout harness as the language fixtures.
- Capability tests are the ones that matter: granted path succeeds; ungranted path is a resolution error (compile-time, no runtime check reached); in-grant path reads a file; out-of-grant path panics at runtime; `..` escapes are normalized and denied; a symlink inside the grant pointing outside is denied after resolution; `fs.rename` where only one of the two paths is granted is denied; a narrower grant cannot be over-delegated to a sub-import (the subset rule).
- Glob table-driven test: patterns × paths × expected verdict, as data, so the matcher is tested without touching a filesystem.
- Golden files for every `Panics:` message, so error text does not drift.

## Key implementation notes

- **Error reporting**: every AST node carries a source span from day one — retrofitting spans later is much more painful than carrying them from the lexer onward, and `7932cab` did exactly that threading into `tir`. `%*alias*%` expansion keeps both the definition span and the use span so alias-induced type errors stay debuggable.
- **Move/borrow checking as a separate pass, not entangled with type inference**: `infer.rs` produces a typed AST; `borrowck.rs` walks the typed AST separately. Keeps each pass comprehensible and testable in isolation, which matters a lot once borrow-checker bugs need debugging.
- **C codegen strategy**: generate readable, `#line`-annotated C (mapping generated lines back to `.x` source lines) so that `gdb`/`printf`-debugging the *generated* C during compiler development is tractable, and so a future "debug the .x program itself" story is easier to bolt on.
- **Signed overflow must be defined**, since C leaves it UB: build with `-fwrapv` so `i32`/`i64` wrap deterministically, and expose `math.checked_*` / `math.saturating_*` for callers who want to know.
- **`mut` does two jobs** (`var` binding vs exclusive borrow) and error messages must distinguish them clearly, since a wrong guess produces a confusing diagnostic.
- **No generics/traits in v1**: keeps type inference and borrow checking tractable; monomorphization is a large added dimension best tackled once the non-generic core works end-to-end.

## Verification

- `cargo test` in `xc/` runs the end-to-end fixture suite (`tests/compile_and_run.rs`): for each `examples/*.x` / `tests/fixtures/*.x`, either (a) compile, run the resulting binary, and diff stdout against an expected value, or (b) compile and assert a specific compiler error is produced (for move/borrow violation fixtures).
- Manual smoke test after each milestone: `cargo run -p xc -- build examples/hello.x && ./hello` (and similarly for `fib.x`, etc.) to confirm the CLI path works, not just the test harness.
- After M4, deliberately write a handful of "should fail" `.x` snippets (double move, two simultaneous exclusive borrows, use of a borrow after its owner's scope ends) and confirm the compiler rejects each with a sensible error — this is the actual proof that the borrow checker works, not just that "well-behaved" programs compile. M3's fixtures (`use_after_move.x`, `double_move_in_loop.x`) are the first two entries in that set.
- Capability tests are compile-time-first: an ungranted `fs.*` reference must fail at resolution, never reach a runtime check. Runtime-denial tests cover the dynamic cases (out-of-grant path, `..` escape, symlink escape, `fs.rename` with only one path granted).

## Divergence from Kotlin (kept intentionally)

- No exceptions. Error handling stays Rust-shaped (panic + error `datac` for now).
- No GC. Ownership and borrows remain in the model.
- No collections in v1.
- No `self`, no `impl` method blocks — receivers instead.
- References do not appear in `datac` field types in v1.
- `val`/`var` and the Kotlin look are surface only; moves and borrows are still visible in the type system.

## Open items

1. ~~**Move-taking parameters have no spelling.**~~ Resolved in M4: there is no move-taking parameter spelling in v1, by design — a `datac`-typed parameter is always a borrow (shared, or exclusive via `mut T`), and a value is only ever moved by local rebinding.
2. ~~**`pd var` semantics and the `pd` wiring pass.**~~ Resolved (see "Current state"): `pd var` is read-only inside functions and assignable only at its own top-level declaration, so the wiring pass hands referencing functions a shared borrow and an assignment inside a `fun` is a compile error. Implementation lands with the rest of `pd` in M5.2.
3. ~~**`pd` and `Bytes`/`String`.**~~ Narrowed by the item 2 decision: since nothing inside a function can *write* a passdown, the drop-point problem for an owned passdown is reduced to "the entry module's scope", which is where `main`'s frame already drops things. The remaining ordering question — whether `pd` wiring must wait for M6's drop insertion — is answered "no": `pd val` of an owned type is a binding that lives as long as the program, so M5.2 can land `pd` for the types v1 has today and extend it when `Bytes`/`String` exist. Re-open only if a `pd`-held owned value turns out to need a drop at a point other than process exit.
4. **`when` is boolean-only in v1.** Exhaustiveness checking and value patterns wait for enums; say so in diagnostics rather than letting users expect Kotlin's `when`.
5. **`process.run_shell`** — ship it gated in v1, or wait for argv slices and ship nothing that shells out?
6. **Is `io` (stdin/stdout/stderr) really ungated?** WASI gates stdin as an input capability. Interactive CLI usability says ungated; purity says gated. Recommend ungated in v1, revisit when `x.toml` gains a way to restrict `main`.
7. **`math` on `f32`.** Either add `f32` overloads or accept implicit `f32 -> f64` promotion, which interacts badly with inference. With overloading locked in (stdlib decision 4), the clean answer is `f32` overloads.
8. **`String` API surface.** The section above sketches `String` largely in parallel with `Bytes`, and decides that text-returning `str.*` functions return `String` rather than `Bytes`. That decision is this document's, not `syntax&shii.md`'s — confirm it, especially whether `path.join`/`path.normalize` should return `String`.
9. **Canonical top-of-file order** is `impl`, then `pd`, then `datac`/`fun`, and `%*alias*%` may appear in any of those. Confirmed and enforced for `impl` in M5.1 (an `impl` after any `datac`/`fun` is a parse error); `pd` and `%*alias*%` keep their slots when M5.2/M5.3 land.
10. **`xc fmt`** is the enforcement story for the style rules, but is not scheduled; until it exists the rules are conventions only.
11. **`syntax&shii.md`'s milestone deltas are stale, and it still contains them.** The implemented order is M0–M3 done, M4 borrowing, receivers at M4.5, `impl` imports at M5.1, with templates/`pd` at M5.2 and aliases at M5.3. Either annotate that file's §12 deltas or leave it as the syntax authority only and ignore its scheduling. The precedence rule covers syntax and style, not scheduling — but the file does not say that yet.
12. **User-code naming case is a configurable lint** (see "Retained Kotlin-look rules" above): a numbered option, `0` = `camelCase` (default), `1` = `snake_case`, room to append more — far-future, lint-only (a beauty warning, never a compile error; any case keeps compiling regardless of the configured preference). Not scheduled to a milestone. Open sub-questions for whenever `xc fmt`/a lint pass is actually built: where the setting lives (`x.toml`? an `xc fmt` flag?), and whether the stdlib's locked `snake_case` convention (naming decision, "Naming and style rules for the stdlib" below) stays an intentional exception or should track the configured case too.
