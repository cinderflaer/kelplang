# The "X" language — compiler project plan

Merged document: language plan + compiler milestones + standard library. The stdlib is no longer a separate file; it is the "Standard library" section below.

**Precedence rule: `syntax&shii.md` always wins.** Where this file and `syntax&shii.md` disagree about syntax or style — including where this file restates that syntax — `syntax&shii.md` is correct and this file is wrong. The syntax content here is a copy kept for reading convenience, not a second authority. This file is the authority for everything else: milestones, capabilities, and the standard library.

## Current state

The compiler exists and runs. `xc/` implements the new syntax end-to-end (lexer → preprocess → parser → type check → move/borrow check → C codegen), `cargo test` is green (65 unit tests + 97 end-to-end fixture/project tests, of which the heap ones are also re-run under AddressSanitizer), and the milestone record below reflects commits, not hopes. This file is now the sole living status doc (`PASSDOWN.md` was deleted once this file absorbed it — do not recreate it).

- **Done**: M0 (scaffold + parser for the full new syntax), M1 (types + C codegen + first running binary), M2 (`datac`, per-field `val`/`var`, while loops), M3 (move checking), M4 (borrowing — free-function parameters and local `&`/`&mut`/`*`), M4.5 (receiver-form methods, the piece M4 cut out — see below), M5.1 (multi-file `impl` imports, `x.toml`, strict module-qualified names), M5.2 (`pd` passdowns and string templates), M5.3 (`%*alias*%` substitution — `alias.xconf` + inline `alias` declarations), M6.1 (the builtin owned `Bytes`), M6.2 (owned `String`, `extern fun`, the first stdlib modules in `xc/std/`, owned `datac` fields, owned passdowns), M6.3 (`const`, stdlib-only argument-type overloading, `&str` return elision, the rest of the `str`/`path` surface), M6.4 (a `pd var` is writable from any function), M7 (**capability-based security** — `with caps(...)`, a real `fs`, real `time`/`rand`, and `env` as the gated stub).
- **Next**: M8 (the stretch milestone: user-definable owned types) or the post-v1 list — collections first, since `fs.list`/`str.split`/argv all wait on a growable sequence type. Nothing is scheduled between here and those.
- **Far future, and wanted**: M9 — the language's **own backend**, so `xc` emits machine code instead of C. The measured case for it is compile time (~95% of `xc build` is the C toolchain's fixed overhead, not X's compiler) and the measured cost is runtime (`-O0` vs `-O2`, 1.2–5.4x on the `bench/speed` workloads). It is on the list because "my own language, not C with a mask" is a project goal in its own right, not because it would make X faster.
- **Deferred despite being in `syntax&shii.md`'s deltas**: `caps(...)` is the last one. Those deltas were aspirational scheduling and lost to implementation order; the milestone list below records where each one is now targeted (`impl` imports shipped as M5.1, `pd` and templates as M5.2, `alias` substitution as M5.3, `caps(...)` at M7).
- **The heap exists, and ownership is checkable at runtime.** `Bytes` and `String` are non-`Copy`, so assigning or returning one moves it; it is freed at the end of the block that owns it, and a `datac` with an owned field drops its fields recursively. A passdown of an owned type is freed by an `atexit` handler, which is v1's only drop point at process exit. The suite proves all of that rather than asserting it: every heap fixture is rebuilt with `-fsanitize=address` and run, so a double free, a use-after-free, or a leak fails `cargo test`. That caught two real bugs during M6.1 (a branch merge that dropped the drop candidates, and a reassignment that leaked) — which is the point of the milestone.
- **The stdlib is no longer hypothetical.** `xc/std/` ships eight modules inside the compiler binary (`stdlib.rs`, `include_str!`), all written in X and reachable as `impl "std/fs"` etc.: `mem`, `str`, `path`, `math` (ungated), `fs`, `time`, `rand`, `env` (gated). The only non-X pieces are `extern fun` bodies supplied by `externs.rs` — allocation, `strlen`, three `&str` primitives, the libm calls, the syscalls behind `fs`/`time`/`rand`, and `env`'s three stub panics. That is stdlib decision 3 in practice: the policy is X source, the unportable primitive is C.
- **Capabilities are real, and the gate is compile-time.** A module's capability set is received from its importer (the entry module holds everything), a delegation must be covered by what the delegator holds, and referencing an ungranted namespace is a *resolution* error — `fs.read` does not resolve, rather than resolving and being rejected at runtime. The runtime check that does exist is the fine-grained one: each call site's path or name argument is matched against the *calling module's* compiled-in allowlist, after lexical normalization and symlink resolution, in that order. `caps_denied` (a path outside the grant) and `caps_symlink` (a link inside the grant pointing outside it) both panic with the `.x` call site, the attempted path, and the granted patterns.
- **Alias substitution is textual, and that shows.** `%*name*%` is spliced in as raw text before parsing, so `%*doubled*% = %*answer*% * 2` with `%*answer*% = 40 + 2` prints 44, not 84 — precedence is whatever the expansion produces. That is the accepted cost of the plan's "pretty simple" choice over a hygienic macro system; `xc/tests/fixtures/aliases.x` pins it so the behavior is documented rather than discovered.
- **`pd var` semantics, resolved and shipped** (was `syntax&shii.md` §9's open question): a `pd var` is **read-only inside functions**. It may only be assigned at its own top-level declaration, since v1 has no top-level statement form beyond `pd` itself. Functions that reference a passdown receive it as a **shared borrow** (for non-`Copy` values) or by value (for `Copy` ones), so no function can write through it and no cross-function exclusivity rule is needed. `syntax&shii.md` §9's list of options is answered by that sentence; the syntax authority is unchanged (semantics are this file's call, not the syntax file's). Implemented in M5.2.
- **Passdown visibility is module-scoped.** This was not specified anywhere; M5.2 answers it the way M5.1 answered imports — only functions in the module that declares a passdown can name it, and there is no `alias.name` spelling for another module's passdown. Recorded as an open item below, since it is a v1 restriction rather than a considered design.
- **Not shipped, and known**: `net` and `process` (the gate mechanism is ready — `stdlib::domain_of` and the resolver need an entry each — but their modules are not written, so `impl "std/net"` still reports "not implemented yet"); `io` (stdin/stdout/stderr; `io.print` is what `print` already does, and `read_line` needs a shipped module); `env` beyond its gated stubs; `fs.mkdir_all` (prefix walking); and every function whose result is a *view* of something that would have to be truncated (`path.parent` returns an owned `String` instead) or that returns *N* items.
- **Known doc bug, not a compiler bug**: `syntax&shii.md`'s (and this file's) own "Full example" mutates a `val` field (`datac Circle(val r: f64)` then `fun mut Circle.grow(by: f64) { r += by }`), which directly contradicts the `Counter` example's own stated rule just above it ("no binding mutability can fix" a `val` field). The compiler correctly rejects this — `xc/tests/fixtures/receiver_methods.x` uses `var r: f64` instead. Left un-edited in `syntax&shii.md` itself since that file is locked as the syntax authority and nobody asked for that edit; flagging it here so it doesn't get "fixed" by loosening the compiler instead.
- **Also deliberately left as-is in `syntax&shii.md`**: §8's `impl "logger" with caps(fs = ["/var/log/myapp/*"])` and §11's full example both use `with caps(...)`, which the parser rejects with an explicit "not implemented until M7" message (M5.1 lexes `[`/`]` for exactly this). `caps` is scheduled, not dropped.

## Context

The user wants a new statically-typed, systems-level programming language (files end in `.x`), aiming for Rust/C/C++-class runtime performance, with Kotlin-flavored braces syntax and a Rust-style ownership + borrowing memory model (no GC, no ARC). This is a from-scratch project built purely as a personal/for-fun engineering project — no external users or deadlines, but the user wants a real foundation, not a toy that cuts corners on the core mechanics (ownership/borrowing) they specifically asked for.

Decisions locked in via discussion:
- **Memory model**: full ownership + borrowing (moves, borrows, borrow checker) — attempted from the start, not deferred to "moves only."
- **Backend**: transpile to C, then shell out to `cc`/`clang` to produce a native binary. Chosen over LLVM for implementation simplicity while still getting native `-O2` codegen. **This choice is deliberate and re-measured, not accidental** — see M9 for the numbers and for the far-future plan to stop being "C with a mask".
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

**Resolved (was: Unresolved):** `pd var` is mutable global state reachable from any function, and v1 has no closures and no `unsafe` to make that safe. The decision, recorded in full under "Current state": **`pd var` is read-only inside functions** — it can only be assigned at its own top-level declaration, so no function ever holds an exclusive borrow of a passdown and no cross-function exclusivity rule is needed. That is closer to option (b) than to (a) or (c): the feature is kept, but writability is confined to the top level rather than exposed inside functions. Implemented in M5.2 — see that milestone entry for the wiring mechanism (hidden parameters) and for the module-scoping answer.

### Retained Kotlin-look rules

- No semicolons. Newline ends a statement. *(`syntax&shii.md` §10 says exactly this. A later revision of this copy added "a line ending in a binary operator (`.`, `+`, `,`, `&&`, `=`) continues onto the next line" — that is **not implemented**: the lexer emits a statement-terminating newline at paren depth 0 regardless of a trailing operator, so a wrapped expression must be parenthesized. Kept here as an aspirational rule rather than deleted, but see open item 11.)*
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

What is on disk today, plus the files later milestones add. `main.rs`, `lexer.rs`, `preprocess.rs`, `ast.rs`, `parser.rs`, `pretty.rs`, `types.rs`, `infer.rs`, `tir.rs`, `borrowck.rs`, `codegen_c.rs`, `driver.rs`, `resolve.rs`, `diag.rs`, `externs.rs`, `stdlib.rs`, `tests/`, and `std/` all exist now. The `Bytes`/`String` runtime (allocation, bounds checks, `free`) lives in `codegen_c.rs` as a fixed C preamble; the three stdlib primitives that cannot be written in X live in `externs.rs`.

Cargo workspace at repo root:

```
x/
  Cargo.toml                # workspace
  xc/                        # the compiler binary crate
    src/
      main.rs                # CLI: `xc build <file.x | project-dir>`, `xc run <same>`, later `xc fmt`
      lexer.rs                # source -> tokens (span-carrying, newline-sensitive, utf-8 aware in literals)
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
      preprocess.rs            # `%*alias*%` expansion, runs before the parser (M5.3, done)
      externs.rs              # compiler-supplied C bodies for `extern fun` (M6.2, done)
      stdlib.rs               # compiler-shipped stdlib sources + capability domains (M6.2/M7, done)
    std/                       # stdlib, written in X (M6.2/M6.3/M7)
      mem.x  str.x  path.x  math.x       # ungated: arithmetic and byte shuffling
      fs.x  time.x  rand.x  env.x        # gated: one capability domain each
    tests/
      compile_and_run.rs       # end-to-end: compile .x fixtures, run binary, assert stdout
      stdlib/                  # per-module stdlib fixtures (M6+; they live in fixtures/ today)
      fixtures/*.x             # 67 today — one loose file per single-file test
      projects/*/              # 27 multi-file projects today (M5.1–M7: x.toml or manifest-less)
  examples/
    hello.x
    fib.x
```

Fixtures live in `xc/tests/fixtures/*.x` (67 today: `datac_basic`, `datac_embeds_datac`, `datac_forward_reference`, `while_accumulator`, `named_args`, `when_expr`, `if_expr`, `assign_to_immutable`, `val_field_immutable`, `type_mismatch`, `use_after_move`, `double_move_in_loop`, `move_then_reinit`, `field_read_does_not_move`, `borrow_basic`, `borrow_conflict`, `same_call_aliasing`, `write_through_shared_borrow`, `move_while_borrowed`, `receiver_methods`, `receiver_mut_on_val`, `passdowns`, `passdown_datac`, `pd_shadowed`, `templates`, `aliases`, `heap_bytes`, `heap_param_borrow`, `heap_move_then_rebind`, `heap_returned`, `heap_scope_drop`, `heap_drops_in_control_flow`, `heap_reassign`, `heap_branches`, `string_basic`, `string_field`, `pd_owned`, `stdlib_mem`, `stdlib_str`, `stdlib_path`, plus the rejected `pd_var_write`, `pd_forward_read`, `template_not_printed`, `alias_cycle`, `alias_unknown`, `heap_use_after_move`, `heap_temporary_print`, `datac_str_field`, `extern_no_body`, `extern_receiver`, `compare_owned`, `owned_statement`, `consts`, `const_not_constant`, `stdlib_math`, `string_views`, `path_parts`, `elision_wrong_param`, `elision_not_from_param`, `view_pins_owner`, `view_released_at_scope_end`, `str_comparison`, `pd_val_write`, `pd_var_double_borrow`, `pd_var_copy_read_twice`, `caps_time_rand`, `path_normalize`). `xc/tests/compile_and_run.rs` compiles each one with the real `xc` binary and checks stdout or the expected rejection.

Multi-file work lives in `xc/tests/projects/<name>/` instead, because a module graph needs a directory (27 today): `shapes` (manifest + nested imports + methods), `alias` (`impl "." as u`, qualified type in a signature), `same_name` (two modules both declaring `Item` + `describe`), `loose` (manifest-less sibling import), `self_call` (a module calling and constructing its own items bare), `alias_xconf` (an `alias.xconf` substitution used as an `impl` path), `alias_cross_file` (an inline `alias` from an imported module used in the importer), plus one rejected project per import rule — `cycle`, `unknown_module`, `duplicate_alias`, `dotdot`, `bad_path`, `unknown_member`, `unqualified_import`, `stdlib_import` (now "module 'net' is not implemented yet"), (M5.2) `pd_module_scope`, (M5.3) `alias_xconf_cycle` (a cycle spanning `alias.xconf` and `main.x`, which is the two-file diagnostic case), and (M7) `caps_granted`, `caps_write`, `caps_denied` (a path outside the grant), `caps_symlink` (a link inside the grant pointing outside it), `caps_ungranted`, `caps_escalation`, `caps_bad_pattern`, `caps_bad_path_pattern`, `caps_bad_domain`, `caps_env`.

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
- **Mechanics**: a preprocessing pass (`preprocess.rs`) runs on the token stream *before* the real parser: (1) collect all alias definitions across `alias.xconf` and every source file's `alias` declarations into one flat table; (2) expand every `%*name*%` occurrence with its registered text; (3) hand the fully-expanded token stream to the existing parser (the lexer already ran, once, on the raw source — as shipped there is no second lexing step). Cyclic aliases (an alias whose expansion transitively references itself) are rejected the same way import cycles are, and eagerly: an unused cycle still fails.
- **Known tradeoff, stated explicitly**: because this is pure text substitution, a bad alias can produce nonsense that only surfaces as a type error at the (already-expanded) use site — accepted as the cost of keeping this "pretty simple" rather than a hygienic macro system. **As shipped (M5.3)**, errors point back to both ends: the alias's definition span and its use span, the latter as a `Diagnostic` note that may live in a different file.
- Import-path aliasing is just this same mechanism applied to `impl "..."` strings — no separate implementation needed once general expansion exists.

Shipped as M5.3 (`preprocess.rs`), on the token/file layer before real parsing — it was originally pencilled in alongside the import work, but the import layer shipped first as M5.1 with no dependency on it. It was the last piece of the original M5. It is the one M5 item that is *textual*, not name-resolution: `resolve.rs` never sees an unexpanded `%*name*%`. See the M5.3 milestone entry for how the reference is recognized, how the two-span diagnostics work, and why `resolve.rs` grew a discovery phase (an `impl` path may itself be an alias, so the file graph and the alias table each need the other).

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

- Primitive types: `i32`, `i64`, `u32`, `u64`, `f32`, `f64`, `bool`, `char`, `u8`, `u16`, `usize`, `isize`, `Unit` (void-equivalent, implicit) — all present; `u8`/`u16`/`usize`/`isize` landed with `Bytes` (M6.1), since a byte buffer cannot be described without them
- Builtin owned heap type `Bytes` (non-`Copy`, moves, drops at end of scope) — the byte-level owned type, and the substrate for `String` — **done** (M6.1), including the builtin methods `empty`/`from_str`/`len`/`is_empty`/`get`/`set`/`clone`
- Owned `String` (non-`Copy`, moves, drops at end of scope, utf-8 validated) — **done** (M6.2), with `empty`/`from_str`/`from_bytes`/`len`/`char_count`/`is_empty`/`clone`/`into_bytes`. It is required in v1 because `syntax&shii.md` uses it as a `datac` field and as a parameter type, and a `datac` may only hold owned fields
- `extern fun` declarations whose bodies the compiler supplies — **done** (M6.2). Only compiler-shipped stdlib modules can have one; user code gets a compile error naming the reason
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
- String templates `"$name"` / `"${expr}"` desugaring to `print`/`printf` formatting — **done** (M5.2), as a formatted `print`, not a string builder
- Named arguments at call sites with 2+ params
- `pd` passdowns — **done** (M5.2), module-scoped and read-only inside functions
- `impl` imports — **done** (M5.1); receiver-form methods — **done** (M4.5)
- `alias` substitution — **done** (M5.3)
- `with caps(...)` grants — **done** (M7): per-module domains received from the importer, no-escalation enforced, ungranted namespaces unresolvable, and the allowlist compiled in and checked at each call site
- `const NAME: T = expr` at top level, folded to a literal and substituted — **done** (M6.3)
- Argument-type overloading for `math.abs`/`min`/`max`/`clamp`, stdlib-only — **done** (M6.3)
- `&str`/`&T` return elision (a returned borrow is the first function's first reference parameter) — **done** (M6.3)

Everything listed above as v1 scope now exists except the modules named under "Not shipped, and known" in "Current state": `net`, `process`, `io`, the `env` bodies, and `fs.mkdir_all`. See "Current state".

Deferred beyond v1 (explicitly out of scope for now, revisit later): generics, traits/interfaces, enums/pattern matching, closures, heap collections (`Vec`), explicit lifetime annotation syntax (`'a`), non-lexical lifetimes, async, unsafe blocks, exceptions, external/third-party dependencies (local-file `impl` imports are in scope — see Imports & Capabilities sections), and the language's own machine-code backend (M9).

## Compiler pipeline (per milestone)

This section is the record of what actually happened, and it overrides `syntax&shii.md`'s own milestone deltas wherever those said a feature would land earlier. Milestone numbers here match the commits in `PASSDOWN.md`. Done: M0–M5.3. Next: M6.

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

**M5.2 — `pd` passdowns and string templates (DONE)**

Shipped together because they meet in exactly one place: the syntax file's own example is a template that interpolates passdowns. Neither depends on the other otherwise, and each got its own fixtures.

**Passdowns**
- Syntax: `pd val name = expr`, `pd var name = expr`, `pd val name: T = expr`, in canonical position (after `impl`, before `datac`/`fun` — enforced at parse time with a message naming that order). `pd` is now a keyword and the lexer emits `TokenKind::Pd`.
- **Wiring is real hidden parameters, not globals read in place.** A scope-aware AST scan (`PdScan`, run before any body is checked) records which passdowns each function actually names, honoring shadowing exactly as the checker does — a parameter, receiver field, or local `val`/`var` of the same name hides the passdown, and a field name / method name / callee name is never a passdown reference. Only referencing functions get parameters, so `fun unrelated(n: i32).R: i32 = n * 2` is emitted unchanged.
- `infer.rs` resolves every passdown to `PdInfo { qualified, name, hidden, value_ty, param_ty, mutable, index }`, where `hidden` is `pd#<module-qualified name>` — a spelling no user identifier can produce, which is what lets the generated C global and the hidden parameter share one symbol. `param_ty` is `implicit_borrow_wrap(value_ty, false)`: by value for `Copy` types, a **shared borrow** for a `datac` (reading a `datac` passdown by value would "move" it out of the global on first use).
- Call sites pass each hidden argument as `&<global>` (non-`Copy`) or `<global>` (`Copy`) — see `PdInfo::arg`. Ordering is fixed by the callee's declaration order, so the generated C signature and the arg list agree. Receiver methods are wired the same way through their `Type#method` key.
- `codegen_c.rs`: one `static <ty> x_pd__...;` per passdown plus a generated `x_init_pds()` that assigns them in declaration order, called as the first statement of `main`. `main` itself is the one function that cannot take hidden parameters — it is emitted as `int main(void)` — so inside `main` a passdown reference takes the address of the global instead of naming a parameter (`Ctx::pd_global_borrow`). Since a call site also reads the global, this is the same value either way; the parameter is what makes the dependency visible in the callee's own signature.
- Diagnostics: writing a passdown inside a function is rejected with wording that distinguishes `pd val` from `pd var` ("read-only inside a function"); a `pd` initializer reading a passdown declared later says so (adding "a `pd` initializer can only use passdowns declared before it" to the generic undefined-name message); a `pd` initializer *calling* a function whose passdowns are not initialized yet is rejected rather than silently reading a zero-initialized global; naming another module's passdown says that passdowns are module-scoped; a name used for both a passdown and another top-level item in one module is rejected.
- `collect_pds` resolves declared types directly and infers unannotated ones from a type-only probe of the initializer, which is then re-checked for real once all passdown types and the wiring table exist — the same "check twice, keep the second" pattern `check_method_call` already used for receivers.
- **Deliberately restricted in v1:** a `pd` initializer cannot read a *non-`Copy`* passdown (a passdown has to hold a value, and a `datac` one is handed around as a borrow), and `main` cannot be called from `.x` code (it takes no arguments, so it could not receive its passdowns). Both are errors with messages rather than silent behavior.
- Borrow checking needed no new rules: hidden parameters are ordinary parameters to `borrowck.rs`, and a passdown parameter is `Copy` (a `Ref` is always `Copy`), so referencing one repeatedly can't look like a move. `pd` initializers are not walked by `borrowck.rs` at all — they have no locals and no borrows (nothing to borrow with no top-level statement form).

**String templates**
- `"a$b c${expr}"` is lexed into a flat token run (`TemplateStart StrChunk TemplateExprStart <expr> TemplateExprEnd ... TemplateEnd`) by the same lexer pass, using a `next_token()` refactor plus a small pending-token queue. Splitting them in the lexer rather than re-lexing the literal text is what keeps each interpolated expression's spans **absolute** — so `file:line:col` is right with no offset arithmetic — and lets a template nest inside its own `${...}`. `$` followed by anything else (a digit, a space, `}` ) stays literal text, so `"cost $5"` needs no escaping; a string with no `$` is still the single `StringLit` token it always was, which is why no existing fixture changed.
- `infer.rs` accepts a template only as the single argument of `print`/`print!` and rejects it anywhere else: v1 has no owned string type, so a template cannot *produce* a value before M6. Interpolated values obey exactly `print`'s existing printable rule (numeric, `bool`, `char`, `str`).
- `codegen_c.rs` emits **one** `printf` per template with a generated format string: literal chunks are escaped into it (`%` doubled, quotes/backslashes/newlines/tabs escaped), each value contributes its `printf_piece` specifier and argument, and `print` appends the newline to the format rather than making a second call. `print!` vs `print` is a flag on the `tir` node.
- Fixtures: `passdowns.x` (read by two functions and by neither), `passdown_datac.x` (a `datac` passdown read through its shared borrow, including a method call and `${...}` field access), `pd_shadowed.x` (a local shadows a passdown), `templates.x` (literal `%` and braces, `${...}`, `print!` without a newline), plus rejected `pd_var_write.x`, `pd_forward_read.x`, `template_not_printed.x`, and the `pd_module_scope` project.

**M5.3 — `%*alias*%` substitution (`preprocess.rs`) (DONE)**

Shipped as one token-level pass, run once per file before the parser. This was the last, deliberately-textual piece of the original M5.

- **Detecting a reference.** `%*name*%` lexes as `Percent Star Ident Star Percent`, and `preprocess.rs` recognizes that run only when its five tokens are *adjacent* in the source, so `a % * b * %` stays arithmetic. `alias` is not a lexer keyword — a declaration is recognized structurally as `alias %*name*% = <raw text>` at a statement start (start of file, or after `newline`/`{`/`}`), which is what keeps `val alias = 5` working. The right-hand side is sliced out of the *source text* verbatim, which is why the RHS needs no separate expression grammar at all.
- **Expansion.** Replacement text is lexed, expanded recursively (a chain `%*a*%` → `%*b*%` works), then every produced token's span is remapped onto the `%*name*%` occurrence the user wrote — a span is only meaningful inside its own file. Inside a string literal (or template chunk) the substitution is purely textual, which is the case `impl "%*shared*%"` needs; nothing is re-lexed there, so a `$` or a `%` inside a replacement stays literal.
- **The full example from the plan's tradeoff note:** `%*doubled*% = %*answer*% * 2` with `%*answer*% = 40 + 2` prints **44**, not 84. Pure text substitution does not add parens. Pinned by `aliases.x` so it reads as documented behavior rather than a bug.
- **Two definition sites, one flat table.** `alias.xconf` at the project root (one `name="replacement"` per line, `#` comment lines, no sections — deliberately the zsh-alias-flavored format, not a config format) plus every inline `alias` declaration in every file the walk reaches. A second definition of the same name is an error naming both sites, since one global namespace is the point.
- **Cycles are rejected eagerly, used or not** — the same rule imports follow. `AliasTable::check_cycles` walks the table after loading, so a cycle nothing references still fails.
- **Two spans, in two files where that is where they live.** `diag::Diagnostic` gained `notes` (`with_note`), so an error can carry a secondary location in a different module: an unknown alias or a cycle reports the `%*name*%` that failed as the primary span and `'%*name*%' is defined here` as the note. `alias_xconf_cycle` is the fixture that proves both files print.
- **Loading had to change shape, and this is the part worth remembering.** An `impl` path may itself be an alias, so the file graph is only knowable *after* expansion, while the table is only complete *after* every file is known. `resolve.rs` now runs discovery rounds before the real load: discovery reads `impl` paths straight off the raw token stream (no expansion, no parse), so a file whose body cannot expand yet still contributes its own aliases and its unaliased imports; then a strict round expands and parses everything with the table built so far. If the strict round loads a file it reached through an *aliased* path, the whole thing repeats with a bigger table (bounded by `MAX_ALIAS_ROUNDS = 8`, which is a guard, not an expected limit); once a strict round loads nothing new, its errors are final. **That is what makes the table genuinely flat**: `alias_cross_file` uses an `alias` declared in an imported module, and the entry file's own alias may appear on a later line than its use.
- **Not implemented, deliberately:** no aliases inside comments (the lexer drops comments before preprocessing, so there is nothing to expand); no whitespace inside the sigil; no re-lexing of a string-bodied replacement; and a *path* alias whose target file is the same file that declares it cannot bootstrap discovery (a genuine chicken-and-egg, and not worth more machinery for a case with an obvious literal-path workaround).
- Tests: 12 new unit tests in `preprocess.rs` (reference detection, chains, adjacency, string expansion, stripping, unknown/duplicate/cycle rejection, `alias.xconf` parsing) plus 6 end-to-end tests. `aliases.x` (inline declarations, a chain, a reference inside a string), `alias_cycle.x` and `alias_unknown.x` (rejected), the `alias_xconf` project (an `alias.xconf` entry used as an `impl` path), `alias_cross_file` (cross-module inline alias), and `alias_xconf_cycle` (the two-file cycle diagnostic). 62 unit tests, 50 fixture/project tests.

**M6 — Heap: builtin `Bytes` and owned `String`**

Split three ways, on the same reasoning that split M4 and M5: the byte buffer plus its drop rules is an independently testable increment, and everything else about the heap (the second builtin type, `extern`, the stdlib modules) builds on it. M6.1 and M6.2 shipped; M6.3 is what remains of the original entry.

**M6.1 — builtin owned `Bytes`, drop, and moves with runtime consequence (DONE)**

- **The type.** `Ty::Bytes` is a compiler builtin (not a `datac`): `x_Bytes { unsigned char* data; size_t len; }` in the generated C, so it is the one type that may hold a pointer. It is non-`Copy` (`Ty::is_copy`), which is what makes M3's move machinery apply to it with no new move logic at all: `val b = a` moves, using `a` afterwards is an error, and a `Bytes` parameter is a *borrow* (M4's rule, now spelled out in `implicit_borrow_wrap`, which had to learn about owned heap types as well as `datac`s).
- **The builtins.** `Bytes.empty()`, `Bytes.from_str(s)` (associated, written on the type name), and `Bytes.len()`, `is_empty()`, `get(i)`, `set(i, v)`, `clone()`. `infer.rs` keeps them in a single table keyed exactly like a receiver method (`Bytes#len`), so call resolution, argument matching, and passdown wiring are unchanged; `codegen_c.rs` emits the bodies from a fixed runtime preamble, and a builtin the preamble does not define fails at `cc` rather than silently. `fun Bytes.helper()` is rejected with a message saying builtin methods are compiler-provided, and `datac Bytes` is rejected as a redeclaration of a builtin type.
- **Drops.** `borrowck.rs` already tracked moves and scopes; it now also returns a `DropPlan` — a side table keyed by *span*, saying which owned locals die at the end of each block and which die before each `return`. Keying on spans rather than position is what lets `codegen_c.rs` walk the tree independently: the two passes genuinely differ in one place (`while` bodies are checked twice for M3's double-move check but emitted once). A moved-out local is never dropped by the scope that declared it; a `return` drops the frame's live locals *after* evaluating the returned value (which may read through one of them); reassigning an owned `var` frees the old value after reading the new one and before storing it.
- **New `u8`, `u16`, `usize`, `isize`**, because a byte buffer cannot be described without them: `len` is `usize`, `get` is `u8`, indexing is `usize`. All four are ordinary numeric types for arithmetic and `print`.
- **Printing a `Bytes`** goes through `%.*s` with the buffer's length and pointer as two arguments, which is why it requires a *place*: a temporary has no address to take, and evaluating it twice would allocate twice. The diagnostic says so and suggests binding it to a `val` first. This is the one restriction M6.1 adds to `print`.
- **The gate, met and mechanized.** The original gate said "a fixture that moves a `Bytes` into a function and uses it after the move is rejected" — that spelling no longer exists, because M4 removed move-taking parameters. The implemented equivalent is a *local* rebinding (`heap_use_after_move.x` rejected, `heap_move_then_rebind.x` accepted). And "no double free is expressible" is no longer just an argument: every heap fixture is rebuilt with `-fsanitize=address` and run as part of `cargo test`, so a double free, a use-after-free, or a **leak** fails the suite. That is what caught two real bugs during this milestone — a branch merge that silently dropped the drop candidates, and a reassignment that leaked the overwritten buffer — which is exactly what the gate was for.
- **Deliberately not in M6.1** (all of them with an explicit diagnostic rather than silent behavior): owned values as `datac` fields, owned passdowns, owned values in growable buffers, and `String`/`str`/`path`/`fs`. A `datac` field of type `Bytes` is refused because dropping a struct means generating a recursive drop for its fields; a `pd` of type `Bytes` is refused because there is no drop point at process exit yet; and an owned `Bytes` returned from a function is fine, but a borrow *of* one that outlives it is not expressible either — which is the `&str` return-elision question again, deferred with it.
- **Known remaining leaks, both safe.** `if c { b = a }` marks `a` moved for the code after the `if` (M3's conservative branch merge), so the branch that did not move it leaks rather than risking a double free; and an assignment *through* a dereferenced borrow (`*p = value`) leaks the value it overwrote, since the drop plan only records plain bindings. Both are leaks, never a double free, and both are in the plan's ledger rather than hidden.
- **Two `datac` field rules tightened while the checker was open**: a field of an owned type (`Bytes`) is refused (dropping a struct is not implemented), and so is a *borrowed* field (`str`/reference) — the latter is `syntax&shii.md`'s own "a datac holds owned data only" rule, which nothing had been enforcing because v1 had no owned alternative (`datac_str_field.x`).
- Fixtures: `heap_bytes`, `heap_param_borrow` (a `Bytes` parameter is a borrow: read through shared, write through `mut`), `heap_move_then_rebind`, `heap_returned`, `heap_scope_drop`, `heap_drops_in_control_flow` (loop body, `return` from inside a branch with another owned local live, block-as-value), `heap_reassign`, `heap_branches`, plus rejected `heap_use_after_move`, `heap_temporary_print`, and `datac_str_field`. 11 fixtures, 12 tests, all heap ones additionally run under AddressSanitizer.

**M6.2 — owned `String`, `extern`, and the first stdlib modules (DONE)**

Shipped as one increment because the four pieces meet in one place: `String` is what makes the stdlib worth writing in X, `extern` is what makes it possible, and owned `datac` fields plus an owned `pd` are the two places M6.1's "dropping a struct is not implemented" wall actually stood.

**`String`**
- `Ty::String` is the second builtin owned type: the same `{ unsigned char* data; size_t len; }` shape as `x_Bytes`, kept a distinct type so text and raw bytes are not interchangeable. Non-`Copy`, dropped at the end of its scope, printed through the same `%.*s` path, and refused as a `datac` name next to `Bytes`.
- Builtins: `empty`, `from_str`, `from_bytes`, `len`, `char_count`, `is_empty`, `clone`, `into_bytes`. `char_count` is the reason `String` is not just a `Bytes` alias — it counts non-continuation bytes, which is a real operation and not a cast.
- **`into_bytes`/`from_bytes` are ownership transfers, and that needed no new machinery.** Declaring the builtin's `self` parameter as plain `Ty::String` (not the `Ty::Ref` `implicit_borrow_wrap` would produce) makes the existing call-argument path read the receiver as a whole value, which M3's move tracking already understands — so the receiver local is marked moved and the buffer changes owner without a copy. `String.from_str("x").into_bytes()` therefore works and leaks nothing: the buffer moves pointer-wise into the result.
- `as_str`, `as_bytes`, and `slice` are *not* in the surface yet. Each returns a borrow of the receiver, and a function cannot return a `Ty::Ref` until M6.3's `&str` return-elision rule exists (see the M4 note on why `resolve_type` never produces one).

**`extern fun` and the stdlib**
- `extern fun name(params).R: T` parses as a fourth `FunctionBody` variant with no body at all (writing one is a parse error, and an `extern` receiver method is rejected — a method body is exactly what the compiler cannot supply for an arbitrary type). `infer.rs` accepts the declaration only if `externs.rs` has a body for it, so user code gets "only compiler-shipped stdlib modules may declare 'extern fun'" instead of a missing symbol at `cc`.
- Bodies live in `externs.rs`, keyed by `(module id, bare name)` — `("std/mem", "alloc_raw")`, `("std/str", "byte_at_raw")`. A body's C text has to use the mangled parameter spelling (`n` -> `x_n`), so `codegen_c.rs` writes a comment listing that mapping above each body rather than leaving it to be deduced.
- `resolve.rs` serves `std/...` ids from sources embedded in the binary (`stdlib.rs`, `include_str!`) instead of from disk, so `xc` stays one self-contained file and the stdlib version is pinned to the compiler version for free. Everything downstream — lexing, aliases, parsing, prefixing, `impl`-cycle checks — is identical to a project module, which is why `std/str.x` can `impl "std/mem"` and `std/path.x` can import both. A `std/...` module the compiler does not ship is still rejected, now naming what does ship.
- Shipped: `std/mem` (`alloc`, over one extern), `std/str` (`len`, `byte`, `is_empty`, `to_upper`, `to_lower`, `repeat`), `std/path` (`is_absolute`, `join`). Only three C bodies exist in total: a `malloc`-and-zero wrapper, a `strlen`, and a bounds-checked byte read. That is stdlib decision 3 in practice — the policy is X source, the unportable primitive is one line of C.
- Deliberately deferred with a note in the file: `str.replace`/`from_i64`/`from_f64` and `path.normalize`. `normalize` is the *same* function the M7 capability matcher needs for its `..` rejection, so it should be written once, against the grant matcher, rather than twice — that is the reason it is not here.

**Owned `datac` fields, and the recursive drop**
- A field of `Bytes`/`String`/an owned `datac` is now allowed. `Ty::needs_drop()` could only answer for a builtin (a `Ty::Struct` cannot know its own fields without the struct table), so it became the free functions `types::needs_drop(ty, owned_structs)` and `types::owned_structs(structs)`; because a `datac` may only name `datac`s declared before it, one pass in declaration order is already a valid fixpoint.
- `codegen_c.rs` emits `x_drop_<Type>` next to each struct's typedef when it owns memory, recursing into the fields that need it. Ordering is free: a struct's drop function is always defined before the struct that calls it.
- **Still a leak, and worth recording:** assigning a *field* of an owned type (`h.name = s`) overwrites a live buffer with nothing freeing it, because the drop plan only records plain bindings (the same shape as the `*p = value` leak from M6.1). Also new in M6.2: a non-tail expression statement that evaluates to an owned value (`String.from_str("x")` on its own line) is *rejected* rather than silently leaked, with a message telling the author to bind it.

**Owned `pd`**
- A `pd` of an owned type is allowed: the generated file-scope global stays, and `write_pd_globals` emits an `x_drop_pds()` that `x_init_pds()` registers with `atexit` — v1's only drop point at process exit, which is what open item 3 was waiting on. A referencing function still receives it as a shared borrow, so `pd` remains read-only inside a function.
- An owned `pd` still cannot be read inside *another* initializer ("its value is not Copy"), which is unchanged from M5.2 and now covers `String` as well as `datac`.

**A comparison rule the heap made necessary**
- `==`/`!=`/`<`/… on an owned value, a `datac`, or a `str` used to type-check and then fail at `cc` (`x_a == x_b` on two structs), or — for `str` — quietly compare *addresses* instead of text. `infer.rs::comparable` now restricts comparisons to the scalars C can compare directly plus `Ty::Ref` (comparing two borrows compares addresses, which is what the author asked for). Rejecting is the honest v1 answer; byte equality for `String`/`Bytes` is the obvious M6.3 addition. Pinned by `compare_owned.x`.

**Two bugs found while building it**
- **A module could not call its own items.** `resolve.rs` stores a non-entry module's top-level names with their module prefix, and `infer.rs` had always looked callees up by the bare name the source wrote — so `fun alloc(n) = alloc_raw(n)` inside `std/mem.x` failed with "call to undefined function 'alloc_raw'". Nothing had ever exercised a module's self-reference, because every existing project called its own items only through receiver methods (which resolve through the receiver's already-prefixed type name) or from the entry module (whose prefix is empty). Fixed with `Ctx::qualify_own`, which retries the lookup with the module prefix; `datac` construction goes through the same helper. Pinned by the `self_call` project.
- **Multi-byte UTF-8 in a string literal was mangled.** The lexer is byte-oriented and pushed each byte of a multi-byte character as its own Latin-1 scalar, so `"héllo"` became `"hÃ©llo"` — `String.len` reported 8 bytes for 6, and `char_count` was meaningless on exactly the input it exists for. Such a bug is invisible until a type can *measure* a string, which is why it surfaced here and not in M0. The literal path now reassembles the character from the (already validated) `&str`.
- Fixtures: `string_basic`, `string_field` (owned field, dropped with the struct, moved whole), `pd_owned`, `stdlib_mem`, `stdlib_str`, `stdlib_path`, plus rejected `extern_no_body`, `extern_receiver`, `compare_owned` (comparing owned values has no generated meaning), and `owned_statement`; the `self_call` project; and two new lexer unit tests. `string_basic`, `string_field`, `pd_owned`, and all three stdlib fixtures joined the AddressSanitizer pass, which is what proves the `atexit` drop and the recursive struct drop rather than asserting them. The harness now builds the instrumented programs to their own output paths instead of racing the individually-run fixtures for the same file. 63 unit tests, 73 fixture/project tests.

**M6.3 — `const`, stdlib-only overloading, and `&str` return elision (DONE)**

Three things that were stdlib prerequisites, shipped together because `str`, `path` and `math` all needed at least one of them and none needed the others to land first.

**`const`**
- `const NAME: T = expr` at top level (type mandatory: a `const` is a declared contract, not an inferred binding), folded at its declaration and substituted at every use. `math.PI` is the literal `3.141592653589793` in the generated C — no global, no initializer function, no runtime cost.
- An initializer may be a literal, an earlier `const` (same module or `alias.NAME`), and arithmetic/comparison/logic on those. That is a deliberate limit: folding is a single pass, so a `const` can only use constants declared before it, and a call in an initializer is a compile error rather than a deferred evaluation.
- `const` shares its module's top-level namespace with `datac`s, `pd`s and functions, so a clash is reported at the declaration. Canonical order became `impl`, `const`/`pd`, `datac`/`fun`.

**Argument-type overloading, stdlib-only**
- An overloadable name (`abs`, `min`, `max`, `clamp`) declared in a compiler-shipped module gets a *parameter-typed* signature key (`std.math.abs@f64`), so several declarations coexist; a call site probes its first argument's type and picks the single matching overload, with "no overload takes X — it is declared for …" otherwise.
- The mechanism is gated on `is_std_module`, which is what stdlib decision 4 asked for: user code cannot declare overloads, and cannot tell that the compiler has the machinery.
- The consequence to remember: `scan_all` (passdown wiring) and the body checker need the *same* key the call sites will use, so `check_program` now computes a `decl_keys` vector once and threads it, instead of every pass re-deriving `sig_key`.

**`&str` return elision**
- `&T` in a *return* position is a `Ty::Ref` whose lifetime is the function's first reference parameter (`str` counts as one: a `str` parameter *is* `&str`, modelled as a flat borrowed view). M4's claim that "no surface syntax produces a returned borrow" is now false by exactly this much, and `borrowck.rs` enforces the rule at both ends:
  - a returned borrow must be rooted at that parameter — or be a `str` literal, whose storage is static, which is what `fun classify(n: i32).R: &str = "pos"` needs, and what keeps the syntax file's own `when` example compiling;
  - a caller of a borrow-returning function keeps the rooted argument borrowed for the enclosing scope (`register_scope_borrow`), so moving or freeing the owner while the view lives is an error.
- Rooting is transitive through calls (`trim_start(s)` roots at `s`) and through `if`/`when` in value position (all branches must agree), which is why a `when` of literals works and a branch mixing a parameter with a literal does not.
- Representation: a `&str` is the `const char*` it already was, so a view and a `str` are the same value at runtime and interchangeable in argument matching and `print`. Only the type system and the borrow checker distinguish them.
- **The one design consequence worth stating**: a `&str` is a NUL-terminated C string, so a *suffix* can be a view but a *prefix* cannot (there is nowhere to put a terminator). `str.substring`/`trim_start`, `path.file_name`/`extension` are views; `str.trim`/`trim_end`/`prefix` and `path.parent` allocate and return `String`. That is a deviation from the plan's own `str` sketch, and the honest one — the alternative is a length-carrying `&str`, which is a different type.

**The surface that shipped**: `str.len`/`byte`/`char_count`/`is_empty`/`substring`/`trim_start`/`trim_end`/`trim`/`prefix`/`starts_with`/`ends_with`/`contains`/`eq`/`index_of` (returning the `Match` error `datac`)/`to_upper`/`to_lower`/`repeat`; `String.as_str`/`Bytes.as_str` as builtins returning views; `path.is_absolute`/`join`/`normalize`/`file_name`/`extension`/`parent`; `math` with `PI`/`E`/`TAU`, the libm transcendentals, and the four overloaded families. Only three further C bodies were needed (a bounds-checked byte read, a slice-from, and a `strlen`).

**Also fixed here, because the stdlib made it bite**: printing or interpolating a *temporary* owned buffer (`print(str.to_upper(s))`) used to be a diagnostic telling the author to bind a `val` first (open item 16). Codegen now hoists the temporary into a C variable and drops it right after the `printf`, so the allocation is freed exactly once — `heap_temporary_print` is in the AddressSanitizer pass to prove it.

**M6.4 — a `pd var` is writable (DONE)**

M5.2 resolved the `pd var` question by making a passdown read-only inside every function, which left a `pd var` with no way to be written at all — `main` included. This replaces that with a rule that *keeps* the write path checked instead of removing it:

- A `pd` reference inside a function is a hidden **borrow** of the global, typed by the declaration: `pd var` → exclusive, `pd val` → shared. A read is `*borrow`, so reads and writes go through one pointer and `count = count + 1` is an ordinary assignment through an exclusive borrow.
- `main` and `pd` initializers have no hidden parameters, so they reach the generated global directly (`*(&x_pd__count)`), and `borrowck.rs` registers the globals as bindings so those accesses are tracked too — otherwise `combine(tally, tally)` in `main` would alias two borrows of one global unnoticed.
- Diagnostics name the passdown rather than its hidden parameter (`cannot borrow passdown 'tally': already borrowed exclusively`), and writing a `pd val` keeps its own wording.
- **The cost, pinned by a fixture**: every reference to a `pd var` is exclusive, so one call cannot take it both exclusively and shared (`pd_var_double_borrow.x`). Reading a *Copy* passdown by value is unaffected — that is a read of the global, not a borrow of it (`pd_var_copy_read_twice.x`).
- The alternative that would remove that cost — propagate exclusivity transitively over the call graph so a read-only function keeps a shared borrow — is recorded as an open item rather than built, because it needs a fixpoint over call edges that nothing else in v1 needs yet.

**M7 — Capability-based security (DONE)**

**Grants**
- `impl "target" with caps(domain = ["pattern", ...])` parses (the M5-era "not yet" error is gone), one clause per domain, empty pattern lists allowed.
- A capability set is **received, never self-declared**: the entry module holds everything (path domains `/**`, the name domain `*`), every other module holds exactly what its importers granted it, and a delegation must be covered by what the delegating module holds — `pattern_covered` is deliberately syntactic and conservative (identical, or inside a `/**` root), because glob-to-glob inclusion is undecidable and being conservative can only *under*-grant.
- Patterns are validated at the `impl` line: `fs`/`net`/`process` patterns are absolute, `**` is legal only as a whole segment, and a `..` segment is refused outright — a grant must not be able to name its way out of its own root. `env` patterns are variable names instead (`HOME`, `LANG*`, `*`), and `time`/`rand` have no named resource at all, so their pattern list is unused.

**Enforcement, split the way the plan asks**
- *Reachability* is a resolution error: `infer.rs::check_capability` refuses to resolve an item of a gated module for a module that does not hold the domain. No runtime check is reached — the name simply does not resolve, which is the whole point of the feature.
- *Matching* is a compiler-inserted runtime check at each gated **call site**, using the **calling module's** compiled-in allowlist, emitted as a statement expression so it works in value and statement position alike. Paths are lexically normalized, then symlink-resolved, then glob-matched — in that order, and the order is the reason a link inside the grant that points outside it is denied. `env` matches names as written (no path structure to normalize). A denial panics with the `.x` call site, the attempted value, and the granted patterns; the location comes from `diag::SourceMap`, threaded into codegen for exactly this.
- `extern` bodies inside a gated module are the *primitive* and are not themselves gated — the check belongs at the boundary where a user module names the public function, which is also why `std/fs` needs no grant to call its own syscall wrappers.
- `path.normalize` and the capability check call the **same** C normalizer, so the security-relevant normalization exists once.

**What shipped behind the gate**
- `std/fs`: `read`, `read_text`, `write`, `write_text`, `exists`, `is_file`, `is_dir`, `size`, `remove`, `rename`, `mkdir`, `append`, `sync`, over twelve compiler-supplied syscall externs. `mkdir_all` needs prefix walking and is deferred. `append`/`sync` were added when a 20 GiB streaming write turned out to be the thing that needed them (see the note under `fs` in the stdlib section); `append` is the streaming primitive and needed no growable buffer, which is the part the plan had wrong when it pencilled `fs.append` in with the collections milestone.
- `std/time` (`now_unix`, `now_mono_ns`, `sleep_ms`) and `std/rand` (`u64`, `below`, `bytes`) are real, both gated. Sleep is gated only by accident of where it lives: the plan's classification is that sleeping is not a capability.
- `std/env` is the worked example of a *structurally gated stub*: the domain, the grant and the variable-name check are real, the bodies panic. `std/net` and `std/process` are not shipped at all — the gate mechanism is ready (one entry each in `stdlib::domain_of`), and half-shipping them would have been worse than saying so.
- **The entry module is still unrestricted.** The plan says main holds everything "until a later milestone adds a way to restrict main itself", and that is exactly what the `caps_granted`/`caps_denied` fixtures show: the restriction that bites is the one a *library* module received. Worth remembering when reading a fixture whose `main` reads `/etc` without complaint.

**Fixtures**: `caps_granted` (read + `exists` inside the grant), `caps_write` (write, size, rename, remove, mkdir), `caps_denied` (a path outside the grant, denied at runtime), `caps_symlink` (a link inside the grant pointing outside it, denied after resolution), `caps_ungranted` (resolution error), `caps_escalation` (over-delegation refused), `caps_bad_pattern` (`..`), `caps_bad_path_pattern` (relative), `caps_bad_domain` (`disk`), `caps_env` (granted stub: the name check passes, the body panics), `caps_time_rand`, `path_normalize`.

**M8 (stretch, only after M0–M7 are solid)** — user-definable owned types, i.e. `Box<T>`-style owners beyond the two builtins. This wants generics, so it is where the first generic machinery gets scoped, or a monomorphic single-parameter special case is hand-rolled. Explicitly a "come back to this later" milestone.

**M9 (far future) — own backend: compile X to machine code, not to C**

Stated as a goal in its own right: *X should stop being C with a mask*. Nothing about the language's identity lives in the backend — the differentiators are the capability system and the move/borrow model — so this is not about making X faster. It is about X not depending on a C compiler to exist, and about fixing the one thing that is actually bad today: **compile time is ~95% somebody else's fixed overhead.**

**Why (measured 2026-09-18 on this machine, best-of-5, using the C that `xc build` already keeps next to the binary):**

| program | `xc build` | clang + link | X's own pipeline |
| --- | --- | --- | --- |
| `examples/hello.x` (230 lines of C) | 80 ms | 76 ms | **~3 ms** |
| `bench/speed/speed.x` (532 lines of C) | 108 ms | 102 ms | **~4 ms** |

And inside clang's share: cc1 startup plus system-header preprocessing ~38 ms (that is the *entire* cost of `-O0 -c` on the trivial file), optimizer ~25 ms on the bigger one, link ~46 ms even for a prebuilt `.o`. So ~76–102 ms of every `xc build` is fixed toolchain cost, and our whole lexer → preprocess → parser → infer → borrowck → codegen pipeline is 3–4 ms. A backend that emits an object and calls `ld` directly could plausibly reach ~10 ms compiles — a 10x compile-time win and no `cc` dependency.

**What it costs, measured rather than guessed:** the *same* generated C compiled `-O0` vs `-O2`, on the four `bench/speed` workloads:

| workload | `-O0` | `-O2` | penalty |
| --- | --- | --- | --- |
| `collatz` (branchy int) | 446 ms | 83 ms | 5.4x |
| `float` (div + add loop) | 265 ms | 101 ms | 2.6x |
| `mix` (div-bound loop) | 204 ms | 168 ms | 1.2x |
| `fib` (real calls) | 28.4 ms | 24.0 ms | 1.2x |

`-O0` is the worst realistic case (everything spilled, nothing inlined). A hand-written backend with a decent register allocator lands between `-O0` and `-O2`: call-heavy code barely notices, branchy and FP-heavy code notices a lot. Closing the rest of the gap means writing an optimizer — inlining, CSE, LICM, strength reduction — which is the multi-year part (LLVM is millions of lines; Cranelift is six figures and lands around `-O1`). **That is the trade: ~10x faster compiles and no toolchain, for ~1.5–3x runtime on numeric code until an optimizer exists.**

**Staged path** (each stage is testable with what already exists):

1. **Backend boundary.** `codegen_c.rs` becomes one implementation of a backend, `driver.rs` (43 lines, and the only place that shells out) picks one, and `xc build --backend=…` selects it — with an env-var override so the 97 end-to-end tests, which are backend-agnostic by construction (compile, run, compare stdout), can validate both backends in one `cargo test`.
2. **Cranelift** (`cranelift-codegen` + `cranelift-frontend` + `object`) for the scalar subset first: no `Bytes`/`String`/`datac`. This is where the compile-time win lands and where the runtime gap is smallest. The `tir` surface is small enough for this to be a real option: 177 lines, ~16 node kinds, four scalar widths.
3. **`datac` layout**, calling convention, and `extern "C"` calls so the ~20 one-line externs (`sqrt`, `clock_gettime`, `stat`, …) keep working as direct calls.
4. **Move the non-trivial runtime out of C**: `externs.rs` (251 lines of C bodies) plus `PATH_RUNTIME`/`CAP_RUNTIME` (the glob matcher, the `..`-rejecting path normalizer, `realpath`-with-a-missing-tail). Two fiddly pieces to plan for: `print` currently goes through C's `printf`, and on x86-64 SysV calling a variadic function requires setting `AL` to the number of vector registers used; and the capability checker's matcher/normalizer are X-visible behaviour that is currently C text.
5. **Then decide.** Keep Cranelift, hand-write x86-64 (own register allocator, no optimizer), or go all the way to a static ELF with raw syscalls and our own runtime — allocator, formatter, and `fs` — which is the only path to a genuinely self-contained binary.

Things that get *better* rather than worse on the way: GNU statement expressions and compound literals leave the strategy (a real backend handles value-position blocks natively), `-fwrapv` becomes explicit wrap semantics we control, and the `#line` gap (open item 20) stops being something we cannot do.

Gate it on `bench/speed/` (runtime parity, four workloads with cross-language checksums) plus a compile-time column — that pair is the whole argument, and it is already in the repo.

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

fun Bytes.len().R: usize                   // shipped (M6.1)
fun Bytes.get(i: usize).R: u8              // shipped; panics on out-of-bounds
fun Bytes.set(i: usize, b: u8)             // shipped
fun Bytes.slice(from: usize, to: usize).R: &Bytes   // needs &-return elision (M6.3)
fun Bytes.as_str().R: &str                 // needs &-return elision (M6.3)
fun Bytes.clone().R: Bytes                 // shipped
fun Bytes.from_str(s: &str).R: Bytes       // shipped
fun Bytes.empty().R: Bytes                 // shipped

fun String.len().R: usize                  // shipped (M6.2); bytes, not chars
fun String.char_count().R: usize           // shipped
fun String.is_empty().R: bool              // shipped
fun String.as_str().R: &str                // needs &-return elision (M6.3)
fun String.as_bytes().R: &Bytes            // needs &-return elision (M6.3)
fun String.clone().R: String               // shipped
fun String.from_str(s: &str).R: String     // shipped
fun String.from_bytes(b: Bytes).R: String  // shipped; panics on invalid utf-8
fun String.into_bytes().R: Bytes           // shipped; move, no copy
fun String.slice(from: usize, to: usize).R: &str   // needs &-return elision (M6.3)
fun String.empty().R: String               // shipped
```

`mem.alloc(n)` (shipped in M6.2) is the allocation primitive the fixed-size-buffer functions are built on: an owned, zero-filled `Bytes` of `n` bytes, so `str.to_upper` allocates once and fills it through `Bytes.set` rather than growing anything.

Rules:
- Both are non-`Copy`. Assigning or passing one moves it. Pass `&Bytes`/`&String` to read, `mut String` to write.
- Moves and drops are fully checked. This is the first place ownership has runtime consequence, so M6 is also where M3's move logic and M4's borrow logic surface their bugs.
- Drop happens at end of scope, after any borrows of it end. A value moved out of a scope is not dropped by that scope. A `datac` holding one drops its fields recursively (M6.2), and an owned `pd` is freed by an `atexit` handler (M6.2).
- Size is fixed at construction (decision 5). Growable ops are deferred, which is why `String.push`-style functions do not appear in the API yet.
- `String` is not a wrapper *keyword* over `Bytes` in the syntax sense — it is a second builtin type with its own move/drop identity. `into_bytes`/`from_bytes` convert with an ownership transfer.
- `&str` is the borrowed view type: static literals, substrings of another `&str`, or (once `as_str` exists) `String.as_str()`.
- **A function cannot yet return a borrow**, which is what the `&-return elision (M6.3)` marks above mean: `resolve_type` never produces a `Ty::Ref` for a `.R: T` clause, so any function whose result is a view of its input is not typeable until that rule lands.
- **Printing an owned value needs a place to name it**: `print(up)` works, `print(str.to_upper(s))` does not, because `%.*s` reads the length and the pointer separately and evaluating a call twice would allocate twice. M6.1 introduced the rule for `Bytes`; M6.2's stdlib returns owned `String`s, so it now bites in ordinary code (open item 16).

Blocks that stay blocked even with these types: anything returning *N* items. `fs.list`, `str.split`, `env.args` need a growable sequence type, which is the collections milestone.

### Module map by milestone

| Tier | Ships | Milestone |
| --- | --- | --- |
| 0 | `print`/`print!`; `str.len`/`byte`/`is_empty`/`to_upper`/`to_lower`/`repeat` and `path.is_absolute`/`join` (**shipped in M6.2**); `math`, `panic`, `assert`, and the borrowing `str`/`path` functions | M6.2 (partial), M6.3 for the rest |
| 1 | `Bytes` + `String` + `mem` (**shipped in M6.1/M6.2**); allocation-returning `path` and `str` functions (**M6.2** for `str.to_upper`/`to_lower`/`repeat` and `path.join`) | M6 |
| 2 | `fs` **shipped** (M7); `env` gated with stub bodies (M7); `io` line reads and `process`/`net` not shipped | M7 |
| 3 | `time`, `rand` (both gated, both real) — **shipped** (M7) | M7 |
| 4 | collections, `fmt`-style formatting, `fs.list`, `process.run(argv)`, `env.args`, growable strings | post-v1 |

Tier 1 strictly precedes tier 2. A compiler that can `fs.read` must already have working move-and-drop for `Bytes` — which it now does.

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

**Shipped in M6.2** (`xc/std/str.x`): `str.len`, `str.byte` (the bounds-checked one the section below does not name, but which `to_upper`/`join` are built on), `str.is_empty`, `str.to_upper`, `str.to_lower`, `str.repeat`. Everything below that returns `&str` — `trim`, `trim_start`, `trim_end`, `file_name`-style slicing, `char_at` — is waiting on the `&str` return-elision rule, because a function's `.R: T` cannot be a borrow yet. `replace`, `from_i64`, and `from_f64` need no new rule and are simply not written yet.

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

**Borrow rule this forces:** a function returning `&str` without an explicit lifetime must borrow from its first reference parameter (Rust's elision rule). `fun str.trim(s: &str).R: &str` returns a slice of `s`. Without this rule the type checker cannot accept the `&str`-returning half of `str`/`path` at all, so it is a hard prerequisite, not a nicety — and M6.2 shipped only the half that does not need it. Iteration over chars waits for collections.

#### path (pure; allocation-returning parts need `Bytes`/`String`)

**Shipped in M6.2** (`xc/std/path.x`): `path.is_absolute` and `path.join`. The predicate/slicing family below (`file_name`, `parent`, `extension`, `stem`) returns `&str` and waits on return elision with the rest of `str`. `path.normalize` is deferred *on purpose* — it is the same lexical normalization the M7 capability matcher needs for its `..` rejection, so it should be written once, against the grant matcher, rather than written here and then re-derived there. `path.join` does not normalize (`a/` + `b` collapses the duplicate separator, which is what users mean, but `..` is left alone).

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

**Shipped**, with the grant/gate/check machinery described under M7 and the test suite that goes with it: `read`, `read_text`, `write`, `write_text`, `exists`, `is_file`, `is_dir`, `size`, `remove`, `rename`, `mkdir`, plus two that were not in the sketch: `append(path, data)` (the streaming primitive — it needs one buffer's worth of data at a time, not a growable one, which is why streaming a file needed no collections work) and `sync(path)` (an fsync, so a timing of a write is a timing of storage rather than of the page cache). `mkdir_all` is the one function from the sketch below that is *not* here (it needs to walk prefixes of its path).

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
fun fs.append(path: &str, data: Bytes)         // shipped, and not in the original sketch
fun fs.sync(path: &str)                        // shipped: fsync, so a timing is of storage
```

No `try_` variants — the out-param family was rejected in decision 2. A recoverable read is expressed by checking first (`fs.exists`) or by accepting the panic. `fs.list` is absent: no sequence type. `fs.remove` for a directory tree is `fs.remove_all`, deferred. `fs.append` **shipped**, contrary to the plan's own note that it belonged with the growable-buffer work (decision 5): appending a chunk at a time needs no growable buffer, only a chunk, so the streaming case never depended on collections. What *is* still blocked on collections is anything that returns or accepts *N* items, and `Bytes.append`/`String.push` — growing a buffer in memory — which is a different operation from appending it to a file.

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

Locked (decision 3), and **as of M7 the whole sentence holds**: `extern fun` parses, bodies come from `externs.rs`, `xc/std/*.x` ships inside the compiler binary, and the compiler-inserted allowlist check runs at every gated call site. The raw syscall lives only inside the gated module (`std/fs.x`'s externs), the check lives at the boundary where a *user* module names the public function, and the allowlist is compiled in as constant data that no API can widen.

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
| Builtin owned `Bytes` + drop | `fs`, allocation-returning `str`/`path` | Real: heap, drop insertion, move checking with runtime consequence | M6.1 (done) |
| A capability check inserted at a gated call site | the headline feature | Medium: codegen emits the allowlist as constant data and wraps each gated call | M7 (done) |
| Builtin owned `String` | `datac` fields, `str`/`env` text results | Small once `Bytes` exists | M6.2 (done) |
| `&str` return elision (borrow from first ref param) | the `&str`-returning half of `str`, `path` | Small, but unblocks a whole namespace | M6.3 (done) — `resolve_return` produces a `Ty::Ref`, and `borrowck.rs` enforces the rooting rule at both ends |
| `const NAME: T = expr` at top level, compile-time folded | `math.PI`, `math.E` | Small parser + const-fold in `infer.rs` | M6.3 (done) |
| `extern` declarations with compiler-supplied bodies | stdlib written in X | Small parser surface, new codegen path | M6.2 (done) |
| Argument-type overloading, stdlib-only | `math.abs/min/max/clamp` across numeric types | Medium: overload resolution in `infer.rs`; kept stdlib-only to avoid user-facing complexity | M6.3 (done) |
| `u8`, `u16`, `usize`, `isize` | byte buffers, indices, `Bytes.len` | Small; `usize`/`isize` needed regardless for indexing | M6.1 (done) |
| Slice/array type | `fs.list`, `process.run(argv)`, `env.args`, `str.split` | Large — collections milestone | post-v1 |
| `Option`/`Result` + generics | replaces error `datac`s | Large | post-v1 |

### Testing the stdlib

- Fixture suite per module, same compile-run-diff-stdout harness as the language fixtures. **As shipped** they live in `xc/tests/fixtures/` (single-file: `stdlib_mem`, `stdlib_str`, `stdlib_path`, `stdlib_math`) and `xc/tests/projects/` (anything that needs a *grant*: the ten `caps_*` projects), because the harness takes one path and a granted module needs a project with an importer. `tests/stdlib/` still has not earned its own directory. The stdlib fixtures are in the AddressSanitizer pass, which is what makes "the stdlib's allocations are freed" a test result rather than a claim.
- A stdlib module is imported exactly like a project module (`impl "std/mem"`), so `std/str` importing `std/mem`, and `std/path` importing both, are the ordinary `impl` rules — no special case in the loader beyond where the text comes from.
- Capability tests are the ones that matter, and **as shipped (M7)** every one of them exists: a granted path reads a file (`caps_granted`); an ungranted reference is a resolution error (`caps_ungranted`); an in-grant path writes, renames, sizes and removes (`caps_write`); an out-of-grant path panics at runtime (`caps_denied`); a symlink inside the grant pointing outside it is denied after resolution (`caps_symlink`); a narrower grant cannot be over-delegated to a sub-import (`caps_escalation`); a `..` pattern is rejected at the `impl` line (`caps_bad_pattern`). Still missing: a table of *which* of `fs.rename`'s two paths is checked — the code checks both by construction (`checked_args` returns every `str` argument for `fs`), but nothing exercises the negative case with only one path granted.
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
- After M4, deliberately write a handful of "should fail" `.x` snippets (double move, two simultaneous exclusive borrows, use of a borrow after its owner's scope ends) and confirm the compiler rejects each with a sensible error — this is the actual proof that the borrow checker works, not just that "well-behaved" programs compile. M3's fixtures (`use_after_move.x`, `double_move_in_loop.x`) are the first two entries in that set. M6.2 added four more ("an `extern` needs a compiler body", "an `extern` takes no receiver", "comparing owned values has no meaning", "an owned value in statement position would never be freed").
- **Heap fixtures are run under AddressSanitizer** (M6.1, extended in M6.2): `heap_fixtures_are_double_free_and_leak_clean` builds every fixture that owns memory — the `Bytes` ones, the two `String` ones, the owned-`pd` one, and the three stdlib ones — rebuilds the generated C with `-fsanitize=address -O1`, and runs it. A double free, a use-after-free, or a leak fails the suite, which is what turns "the `atexit` drop works" and "a struct drops its fields" into test results rather than claims. This is the only test that depends on a toolchain feature, so it skips itself if `cc` has no AddressSanitizer — which means the memory-safety claim is verified on every machine that can verify it, and the rest still pass. Note what the sanitizer does *not* cover: the M6.2 field-assignment leak (open item 15) is real and would be caught by ASAN if a fixture exercised it, so no fixture does.
- **Two benchmarks under `bench/`**, both with a recorded `results.txt`:
  `write_20gb/` writes 20 GiB in four languages (Rust, X, C++, Python) and
  shows the workload is entirely device-bound — all four within 2% at
  ~1650-1700 MiB/s — and `speed/` runs four CPU-bound workloads with
  cross-language checksums, which is where a language difference is actually
  visible (X, C++ and Rust level to within ~1%; Python 27-87x slower
  depending on the workload). The `speed/` README records the method and the
  caveats, including the one worth remembering: an unconstrained naive `fib`
  is rewritten into a loop by LLVM at 0.5 ns/op, so the call-cost workload
  needs a side-effecting counter to measure anything.
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
2. ~~**`pd var` semantics and the `pd` wiring pass.**~~ Resolved twice: M5.2 made a passdown read-only inside functions, and M6.4 replaced that with a hidden *borrow* typed by the declaration (`pd var` exclusive, `pd val` shared), so a `pd var` is writable from any function that references it while every access stays borrow-checked.
3. ~~**`pd` and `Bytes`/`String`.**~~ Resolved in M6.2. An owned passdown is allowed and its drop point at process exit is an `atexit` handler registered alongside the generated global's initializer, so `pd val banner = String.from_str("...")` works and is freed. Reading an owned passdown inside *another* passdown's initializer is still rejected ("its value is not Copy") — unchanged from M5.2, now enforced by `is_copy` rather than by naming `datac` specifically.
4. **`when` is boolean-only in v1.** Exhaustiveness checking and value patterns wait for enums; say so in diagnostics rather than letting users expect Kotlin's `when`.
5. **`process.run_shell`** — ship it gated in v1, or wait for argv slices and ship nothing that shells out?
6. **Is `io` (stdin/stdout/stderr) really ungated?** WASI gates stdin as an input capability. Interactive CLI usability says ungated; purity says gated. Recommend ungated in v1, revisit when `x.toml` gains a way to restrict `main`.
7. **`math` on `f32`.** Either add `f32` overloads or accept implicit `f32 -> f64` promotion, which interacts badly with inference. With overloading locked in (stdlib decision 4), the clean answer is `f32` overloads.
8. **`String` API surface.** The section above sketches `String` largely in parallel with `Bytes`, and decides that text-returning `str.*` functions return `String` rather than `Bytes`. That decision is this document's, not `syntax&shii.md`'s — confirm it, especially whether `path.join`/`path.normalize` should return `String`.
9. **Canonical top-of-file order** is `impl`, then `pd`, then `datac`/`fun`, and `%*alias*%` may appear in any of those. Enforced for `impl` (M5.1) and `pd` (M5.2) — each is a parse error after any `datac`/`fun`. M5.3 answered the `%*alias*%` half by taking it out of the parser's hands entirely: `alias` declarations are stripped by `preprocess.rs` before the parser runs, so a declaration is legal anywhere a statement could start and the canonical order does not apply to them. References keep their slot.
10. **Passdowns are module-scoped, with no cross-module spelling.** M5.2's answer to a question `syntax&shii.md` §9 leaves open. If a passdown should be reachable from another module, the syntax needs a spelling (`util.shared`? a parameter?) and the wiring has to reach across the module boundary — today naming another module's passdown is an error that says so.
11. **Trailing-operator line continuation is claimed by this file, not by the compiler.** This document's style list was revised to say that a line ending in `.`, `+`, `,`, `&&`, or `=` continues onto the next line. `syntax&shii.md` §10 says only "newline ends a statement", the M0 record says continuation was never attempted (a wrapped expression must be parenthesized), and the lexer behaves the M0 way. The style bullet now carries that as an explicit aspirational note instead of a claim. Decide whether the *language* should support it, and if so schedule it as its own small lexer change (`syntax&shii.md` would need the sentence too, since it is the authority).
12. **A `datac` may only name `datac`s declared before it.** Not a documented rule anywhere, but it is what the declaration-order struct table means in practice, and it is now enforced with a message that says so (M5.2's fix for struct emission order). Mutual/recursive `datac`s would need `Box`-style indirection, so this is expected to stay a v1 restriction.
13. **`xc fmt`** is the enforcement story for the style rules, but is not scheduled; until it exists the rules are conventions only.
14. **`syntax&shii.md`'s milestone deltas are stale, and it still contains them.** The implemented order is M0–M6.2 done (borrowing M4, receivers M4.5, `impl` imports M5.1, `pd` + templates M5.2, aliases M5.3, `Bytes` M6.1, `String`/`extern`/stdlib M6.2), with M6.3 next. Either annotate that file's §12 deltas or leave it as the syntax authority only and ignore its scheduling. The precedence rule covers syntax and style, not scheduling — but the file does not say that yet.
15. **Three known leaks, all deliberate, all safe.** From M6.1: `if c { b = a }` marks `a` moved for everything after the `if` — M3's conservative branch merge — so the branch that did not move it leaks instead of risking a double free; and an assignment through a dereferenced borrow (`*p = value`) leaks the value it overwrote, because the drop plan only records plain bindings. Added in M6.2: assigning an owned *field* (`h.name = s`) leaks the field's old buffer for the same reason. All three are leaks, never a double free, and all three would disappear with a flow-sensitive merge plus field-granular drop tracking (a bigger change than v1 wants). A fourth candidate — a non-tail statement that produces an owned value — is *rejected* rather than leaked, so it is not on this list.
16. ~~**An owned buffer can only be printed from a place.**~~ Lifted in M6.3: codegen hoists a temporary owned buffer into a C variable and drops it right after the `printf`, so `print(str.to_upper(s))` allocates once, prints, and frees. `heap_temporary_print` runs under AddressSanitizer to keep that honest.
17. **`pd var` cost, and how to remove it.** Every reference to a `pd var` is an exclusive borrow (M6.4), so one call cannot take it both exclusively and shared. Removing that needs the exclusivity to propagate transitively: a function that only reads the passdown should keep a shared hidden borrow, and a function that calls a writer should be *upgraded* to exclusive — a fixpoint over the call graph induced by the `wired` table. Not built, because nothing needs the ergonomics yet and the fixpoint is the kind of machinery that should be added when a real program complains.

18. **The entry module is unrestricted.** `main` implicitly holds every domain with `/**`/`*`, so the grants a `main` writes on its own `impl` lines restrict the *imported* module, not `main` itself. The plan anticipated this ("until a later milestone adds a way to restrict `main` itself") and it is the reason the capability fixtures put the interesting module behind one level of delegation. The natural home for the fix is `x.toml`: a `[caps]` section granting `main` less than everything.

19. **`net`, `process`, `io`, and the rest of `env`.** The gate mechanism is domain-generic and ready (`stdlib::domain_of` + one `with caps` spelling per domain), but only `fs`, `time`, `rand` and the `env` stub ship. `net` needs host:port pattern validation and a socket type; `process` needs argv (slices); `io` needs `read_line` (a `String` and a stdin extern). Half-shipping them would have been worse than saying so.

20. **`#line`-annotated C was never actually emitted.** M1's record claims it shipped "as planned", and this file's Key-implementation-notes still recommends it for debugging the generated C — but `codegen_c.rs` emits no `#line` directive at all, which is why M7's capability denial passes a `file:line:col` string through `diag::SourceMap` instead of using `__FILE__`/`__LINE__`. Either implement the directive per statement (the span is on every `tir` node, so it is mechanical) or drop the claim; the honest state today is that a `panic:` in the generated C names a line of *generated* C unless the caller the compiler generated passed a source location explicitly.

21. **A `String` is not a `str`.** Passing an owned `String` where a `&str` is expected needs an explicit `.as_str()` (M6.3), which the stdlib's own fixtures show repeatedly (`str.len(t.as_str())`). Rust's deref coercion would remove it, and it is the most visible ergonomic wart left in the string layer.

22. **`&str` return elision is first-parameter-only, and views cannot truncate.** Deliberate on both counts (the elision rule is what makes the caller-side borrow sound; a `&str` is a NUL-terminated C string), but it means `path.parent`/`str.trim`/`str.prefix` allocate where the plan's sketches say `&str`, and `fun f(a: &str, b: &str).R: &str` can only return a view of `a`. Both would fall out of a length-carrying string view type.

23. **User-code naming case is a configurable lint** (see "Retained Kotlin-look rules" above): a numbered option, `0` = `camelCase` (default), `1` = `snake_case`, room to append more — far-future, lint-only (a beauty warning, never a compile error; any case keeps compiling regardless of the configured preference). Not scheduled to a milestone. Open sub-questions for whenever `xc fmt`/a lint pass is actually built: where the setting lives (`x.toml`? an `xc fmt` flag?), and whether the stdlib's locked `snake_case` convention (naming decision, "Naming and style rules for the stdlib" below) stays an intentional exception or should track the configured case too.
24. **No extension methods on a builtin or imported type.** M5.1 recorded that `fun circle.Circle.helper()` cannot be parsed (the receiver grammar takes one bare type name), and the same wall is why the stdlib plan's "receiver form is available and preferred in user code" (`name.trim()`) is not true: `fun str.trim(s: &str)` is a *free* function in `std/str`, so callers write `str.trim(s)`. `&str` return elision (M6.3) removed the *other* blocker, so what is left is purely the extension-method spelling. Recorded here because the stdlib's own naming section promises the receiver form.
25. **`String` still is not a full peer of the plan's sketch.** `as_str` shipped in M6.3 (as a builtin returning a view); `as_bytes` and `slice` are still missing, the first because nothing needs a `&Bytes` and the second because slicing by *length* needs the length-carrying view type of item 22. `String.char_count` counts non-continuation bytes, which is the cheap right answer but not grapheme-aware. **Equality is settled by refusal**: M6.2 made `==`/`!=` on an owned value, a `datac`, or a `str` a type error (`compare_owned.x`, `str_comparison.x`), because the alternative was an invalid C `==` on two structs — and for `str`, a comparison of addresses rather than text, which is silently wrong; `str.eq` is the text comparison. Byte equality for `Bytes` is still absent, and `String.concat` does not exist (whether concatenation should be an operator or `str.concat(a, b)` remains open).

26. **The backend is a C compiler, and the test suite cannot yet tell.** M9 records why that is deliberate and what replacing it would take; the immediate, small prerequisite is that `xc build` has no way to select a backend, so a second one could not be validated by the suite that already exists. Two things to do when M9 starts, both small: an `--backend=` flag (plus an environment override) and a `cargo test` that runs the fixture suite twice. Worth its own item because it is the one gap this file's verification section cannot currently cover: every memory-safety and capability claim in it is verified *through clang*, so an own backend inherits the obligation to re-verify all of that, not just the performance numbers.
