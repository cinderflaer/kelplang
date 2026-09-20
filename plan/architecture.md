# Kelp architecture

The design behind the language: why it exists, how the compiler is laid out, the
capability model, the v1 feature set and standard library, and the decisions
(naming, failure, verification) that are not syntax. This file is the authority
for everything here that is not surface syntax.

The other documents:

- `syntax.md` the syntax and style authority.
- `roadmap.md` the plan: the critical path, current state, the MK/S/M naming scheme, the open items, and its Backlog part for what is named but unscheduled.
- `done.md` the milestone record of what has shipped, milestone by milestone.
- `mk7-own-backend.md` the working plan for the compiler's own backend.
- `../README.md` what the project is, and how to build, run and test it.

**Precedence rule: `syntax.md` wins on syntax and style.** Where any file here and `syntax.md` disagree about syntax or style including where another file restates that syntax `syntax.md` is correct and the other file is wrong. Syntax restated outside `syntax.md` is a copy kept for reading convenience, not a second authority. The rest of the plan is the authority for everything else: the language's non-syntax decisions, the milestone record, capabilities, and the standard library.

## Context

The user wants a new statically-typed, systems-level programming language (files end in `.kp`), aiming for Rust/C/C++-class runtime performance, with Kotlin-flavored braces syntax and a Rust-style ownership + borrowing memory model (no GC, no ARC). This is a from-scratch project built purely as a personal/for-fun engineering project no external users or deadlines, but the user wants a real foundation, not a toy that cuts corners on the core mechanics (ownership/borrowing) they specifically asked for.

Decisions locked in via discussion:
- **Memory model**: full ownership + borrowing (moves, borrows, borrow checker) attempted from the start, not deferred to "moves only."
- **Backend**: transpile to C, then shell out to `cc`/`clang` to produce a native binary. Chosen over LLVM for implementation simplicity while still getting native `-O2` codegen. **This choice is deliberate and re-measured, not accidental** see MK7 for the numbers and for the far-future plan to stop being "C with a mask".
- **Type system**: fully static with local type inference (Kotlin/Rust-style annotate signatures, infer locals).
- **Syntax**: brace-delimited, Kotlin-shaped surface (`fun`, `val`/`var`, `datac`, `.R: T`, no semicolons, receivers instead of `self`/`impl` blocks) over a Rust-shaped memory model. Full spec in the "Syntax & style" section below.
- **Implementation language**: Rust (good fit for tree-walking, pattern matching over ASTs, and enums for typed IR).

Because a full borrow checker is the hardest single component here, the plan is phased so that each milestone produces a compiler that actually builds and runs `.kp` programs, with borrowing arriving once the simpler layers (lexer → parser → types → C codegen) are solid and tested. This avoids the failure mode of building a borrow checker against a language/compiler that doesn't exist yet.

**Distinguishing feature resolved: capability-based security as a language feature, not an OS layer.** Instead of every import getting implicit access to the filesystem, network, process spawning, and environment, each `impl` is granted an explicit, delegated subset of capabilities, and code without a grant has no *name* to reach that resource with not a runtime-guarded call that happens to reject it, but a symbol that plain doesn't resolve. See "Capability-based security" below. (Other candidates considered and shelved: Zig-style comptime instead of generics/traits/macros; arena/region allocation as an ownership alternative; Go-style structural interfaces worth revisiting as *additional* features later, but capabilities is the headline one.)

## Compiler crate layout

What is on disk today, plus the files later milestones add. `main.rs`, `lexer.rs`, `preprocess.rs`, `ast.rs`, `parser.rs`, `pretty.rs`, `types.rs`, `infer.rs`, `tir.rs`, `borrowck.rs`, `codegen_c.rs`, `driver.rs`, `resolve.rs`, `diag.rs`, `externs.rs`, `stdlib.rs`, `fmt.rs`, `lint.rs`, `deprecated.rs`, `tests/`, and `std/` all exist now. The `Bytes`/`String` runtime (allocation, bounds checks, `free`) lives in `codegen_c.rs` as a fixed C preamble; the three stdlib primitives that cannot be written in Kelp live in `externs.rs`.

Cargo workspace at repo root:

```
x/
  Cargo.toml                # workspace
  kelpc/                        # the compiler binary crate
    src/
      main.rs                # CLI: `kelpc build <file.kp | project-dir>`, `kelpc run <same>`, `kelpc version`, later `kelpc fmt`
      lexer.rs                # source -> tokens (span-carrying, newline-sensitive, utf-8 aware in literals)
      ast.rs                  # parsed syntax tree types
      parser.rs               # tokens -> AST (recursive descent + Pratt for expressions)
      pretty.rs               # AST pretty-printer (the MK1-S1-M1 deliverable)
      types.rs                # type representations
      infer.rs                # type inference / checking, stdlib overload resolution
      tir.rs                  # typed IR; every node carries its resolved Ty and Span
      borrowck.rs              # move checker (MK1-S3-M1), borrow checker (MK1-S4-M1), drop (MK3)
      codegen_c.rs             # typed IR -> C source text
      driver.rs                # orchestrates pipeline, invokes `cc`
      resolve.rs              # project/manifest loading, `impl` imports, module naming (MK2-S1-M1), capability gating (MK4-S1-M1)
      deprecated.rs           # the register of outdated syntax, and the `outdated:` finding (open item 74)
      diag.rs                 # cross-file diagnostics: module id -> file/line/col (MK2-S1-M1)
      preprocess.rs            # `%*alias*%` expansion, runs before the parser (MK2-S1-M3, done)
      externs.rs              # compiler-supplied C bodies for `extern fun` (MK3-S1-M2, done)
      stdlib.rs               # compiler-shipped stdlib sources + capability domains (MK3-S1-M2/MK4-S1-M1, done)
    std/                       # stdlib, written in Kelp (MK3-S1-M2–MK5-S3)
      mem.kp  str.kp  path.kp  math.kp  fmt.kp  io.kp    # ungated: arithmetic, text, streams
      fs.kp  time.kp  rand.kp  env.kp  net.kp  process.kp # gated: one capability domain each
                                                   # (`env`'s bodies are still stubs)
    tests/
      compile_and_run.rs       # end-to-end: compile .kp fixtures, run binary, assert stdout
      stdlib/                  # per-module stdlib fixtures (MK3+; they live in fixtures/ today)
      fixtures/*.kp             # 111 today one loose file per single-file test
      projects/*/              # 39 multi-file projects today (MK2-S1-M1–MK5-S4: coral.toml or manifest-less)
  examples/
    hello.kp
    fib.kp
    download.kp               # fetch a URL and write it to a file (MK5-S4) the
                             # first example that needs a capability, and the
                             # only one the suite runs against a live server
```

Fixtures live in `kelpc/tests/fixtures/*.kp` (287 today), in three groups, because the harness does three different things with them. **Compiled and run, with an exact expected stdout** (68): `aliases`, `borrow_basic`, `borrow_temp_arg`, `box_basic`, `box_field_take`, `box_recursive`, `bytes_build`, `caps_time_rand`, `consts`, `container_element_from_use`, `datac_basic`, `datac_embeds_datac`, `field_read_does_not_move`, `heap_branches`, `heap_bytes`, `heap_drops_in_control_flow`, `heap_move_then_rebind`, `heap_param_borrow`, `heap_reassign`, `heap_returned`, `heap_scope_drop`, `heap_temporary_print`, `if_expr`, `int_conversions`, `io_stdio`, `io_streams`, `list_basic`, `list_datac_element`, `list_field`, `list_owned`, `list_slice_insert`, `move_then_reinit`, `named_args`, `net_loopback`, `net_poll`, `net_resolve`, `passdown_datac`, `passdowns`, `path_normalize`, `path_parts`, `pd_owned`, `pd_shadowed`, `pd_var_copy_read_twice`, `pd_var_datac_reassign`, `pd_var_mut_argument`, `pd_var_read_only_datac`, `pd_var_read_only_shared`, `pd_var_write`, `pd_write_reaches_callers`, `process_spawn`, `receiver_methods`, `recursive_datac`, `stdlib_build`, `stdlib_fmt`, `stdlib_list`, `stdlib_math`, `stdlib_mem`, `stdlib_parse`, `stdlib_path`, `stdlib_sort`, `stdlib_str`, `string_basic`, `string_build`, `string_field`, `string_views`, `templates`, `view_released_at_scope_end`, `when_expr`, `while_accumulator`. **Compiled and run, then expected to panic with a specific message** (7): `assert_fail`, `box_empty_panic`, `int_conversion_overflow`, `list_pop_empty`, `list_slice_bounds`, `net_timeout`, `panic_msg`. **Rejected by the compiler** (41): `alias_cycle`, `alias_unknown`, `assign_to_immutable`, `borrow_conflict`, `borrow_temp_returned`, `box_borrow_element`, `compare_owned`, `const_not_constant`, `datac_forward_reference`, `datac_self_value`, `datac_str_field`, `double_move_in_loop`, `elision_not_from_param`, `elision_wrong_param`, `escapes`, `extern_no_body`, `extern_receiver`, `grow_while_borrowed`, `heap_use_after_move`, `list_borrow_element`, `list_container_element`, `list_moved_element`, `list_no_element_type`, `list_unsortable_element`, `move_while_borrowed`, `owned_field_read`, `owned_statement`, `pd_forward_read`, `pd_val_write`, `pd_var_double_borrow`, `push_to_val_string`, `receiver_mut_on_val`, `same_call_aliasing`, `str_comparison`, `template_not_printed`, `type_mismatch`, `unknown_escape`, `use_after_move`, `val_field_immutable`, `view_pins_owner`, `write_through_shared_borrow`. `kelpc/tests/compile_and_run.rs` compiles every one with the real `kelpc` binary and checks stdout, the expected rejection, or the expected panic text no fixture asserts on generated C, and only the owning ones are rebuilt under AddressSanitizer (the list is in the Verification section).

**The three group counts above count the names listed, and they are a stale
snapshot of what the directory holds**: it now has 347 `.kp` fixtures, of which
the C backend builds 193 and rejects 154 (measured by the backend sweep and the
valgrind gate, which discover fixtures from the directory rather than from a
list). The names are the record of what the three groups are *for*; re-record
the split on the next docs pass instead of trusting the counts.

Multi-file work lives in `kelpc/tests/projects/<name>/` instead, because a module graph needs a directory (39 today). **Working projects**: `shapes` (manifest + nested imports + methods), `namespace` (`impl "util"`, a qualified type in a signature, reached by the module's own name), `same_name` (two modules both declaring `Item` + `describe`), `loose` (manifest-less sibling import), `self_call` (a module calling and constructing its own items bare), `alias_kpconf` (an `alias.kpconf` substitution used as an `impl` path), `alias_cross_file` (an inline `alias` from an imported module used in the importer), `alias_kpconf_cycle` (a cycle spanning `alias.kpconf` and `main.kp`, the two-file diagnostic case), the MK4-S1-M1 grants `caps_granted`, `caps_write`, `caps_denied` (a path outside the grant), `caps_symlink` (a link inside the grant pointing outside it), `caps_env` (the gated stub) MK5-S2-M1's `process_run`, `process_argv_only` (an empty grant: the module's own-process functions resolve, nothing executable does), MK5-S3's `caps_net` (a delegated `127.0.0.1:*` grant, loopback round trip), `caps_net_denied` (the same machine under another name), `caps_net_resolve_denied`, MK3-S2-M1's `caps_list` (a granted directory listed and cleared recursively through `fs.list`), and MK5-S4's four HTTP projects `http_get` (five requests on one pooled connection), `http_redirect` (a relative redirect followed on the pooled connection, then a POST on a fresh one), `http_pool_off` (`http_pool = 0`, two requests, two connections) and `http_redirect_denied` (a redirect to a host the grant does not name), and MK5-S2-M2's `caps_link` (`fs.symlink` inside a grant, read through, then dangled and removed), `caps_link_denied` (a link whose *target* is outside the grant) and `process_in` (a working directory that the child enters, a directory it cannot enter as code 126, and the three `ProcErr` reasons). **Rejected projects**, one per rule: `cycle`, `unknown_module`, `duplicate_alias`, `dotdot`, `bad_path`, `unknown_member`, `unqualified_import`, `stdlib_import` (now `std/tls`, "module 'tls' is not implemented yet" `std/http` shipped in MK5-S4 and moved off this test), `pd_module_scope`, `caps_ungranted`, `caps_escalation`, `caps_bad_pattern`, `caps_bad_path_pattern`, `caps_bad_domain`, `caps_bad_net_pattern`, `process_denied`.

Single crate is fine at this scale (a compiler is inherently one pipeline); no need to over-modularize into multiple crates.

## Imports & project structure for `.kp` projects

Explicit reaction against Java/Kotlin's convention (package must mirror deeply-nested directory path, plus a Gradle/Maven build-tool layer with its own DSL files, wrapper scripts, and cache dirs). Instead:

- **One manifest file**, `coral.toml`, at the project root name, version, entry point. No separate build-tool layer; `kelpc` itself is the build tool. **As shipped (MK2-S1-M1): the manifest is optional.** `kelpc build <dir>` needs one; `kelpc build <file.kp>` walks up looking for one and, finding none, treats that file's own directory as the module root. Everything in `kelpc/tests/fixtures/` and `examples/` relies on the manifest-less path a single loose file is still a complete program.
- **`src/` holds everything**, flat by default, with subfolders only when the user actually wants to group files a file's path is never required to match any declared namespace.
- **Imports are path-based**, not namespace-based: `impl "shapes/circle"` pulls in `src/shapes/circle.kp` by its file path relative to `src/`, and you refer to its items as `circle.Circle` (the namespace is the module's own name: the last path segment, or the `[dependencies]` key for a `deps/<key>/...` path). No `mod` declarations needed to "announce" a file into a tree the file existing under `src/` is enough.
- **File names are therefore part of the API**: the last path segment becomes the namespace, so file names must be valid identifiers `snake_case`, no dashes, no dots.
- **v1 is local-files-only**: `impl` can only reach files inside the current project's `src/`. No external/third-party dependency support yet (no registry, no git-url deps, no lockfile). The stdlib is the one exception: `impl "std/fs"` reaches compiler-shipped modules, not project files. **As shipped (MK2-S1-M1): `std/...` is reserved but unimplemented** it is rejected with an explicit MK3/MK4-S1-M1 message rather than reported as a missing file, so the reservation is visible in the error.

Example:
```
myproject/
  coral.toml
  src/
    main.kp          # impl "shapes/circle"  ...  fun main() { ... }
    shapes/
      circle.kp
      square.kp
```

`resolve.rs` handles turning an `impl "shapes/circle"` path into a loaded/parsed file plus a name binding for its exported items; this is pure file-path resolution, no separate package-registry concept to build.

**How MK2-S1-M1 implements the namespace (as shipped).** Strict module-qualified, not a flat merge: after loading, every module's top-level names are prefixed with the module's dotted path (`Circle` in `shapes/circle` → `shapes.circle.Circle`), and the *entry* module is left unprefixed so a file's own items stay bare inside that file, and imported items are reachable only as `alias.Item`. Two modules may therefore both declare `Item` and `describe` with no collision (fixture: `same_name`). A module alias is shadowed by a local of the same name, exactly like any other name lookup. Referencing an imported item unqualified is a compile error that names the qualified form (`did you mean 'prov.Thing'?`). See the MK2-S1-M1 milestone entry for the rest, including the one thing this rules out in v1: extension methods on an imported type (`fun circle.Circle.helper()`) are not expressible.

**General alias/expansion system (`alias.kpconf` + inline `alias` declarations)**: a flat, global, purely-textual substitution mechanism, deliberately simple (no parameters, no hygiene, no scoping).

- **Reference syntax**: `%*name*%` used anywhere in `.kp` source (including inside an `impl "..."` path string). The sigil exists specifically so it can never be confused with a real identifier/variable the lexer/preprocessor recognizes it unambiguously and substitutes before real parsing happens.
- **Definition site 1 `alias.kpconf`** (project-root config file, zsh-alias-flavored): `name="replacement text"` per line. e.g. `shared="../../shared/utils"` or `chttps="http_client.new(timeout = 30, tls = true, retries = 3)"`.
- **Definition site 2 inline in `.kp` source**: a dedicated `alias` declaration keyword, distinct from `val`, so the parser stores the RHS as a raw token stream (macro text) instead of type-checking/evaluating it as an expression: `alias %*chttps*% = http_client.new(timeout = 30, retries = 3)`. Needing a separate keyword (rather than reusing `val`) is exactly the "extra stuff" needed so it isn't mistaken for a normal variable.
- **Mechanics**: a preprocessing pass (`preprocess.rs`) runs on the token stream *before* the real parser: (1) collect all alias definitions across `alias.kpconf` and every source file's `alias` declarations into one flat table; (2) expand every `%*name*%` occurrence with its registered text; (3) hand the fully-expanded token stream to the existing parser (the lexer already ran, once, on the raw source as shipped there is no second lexing step). Cyclic aliases (an alias whose expansion transitively references itself) are rejected the same way import cycles are, and eagerly: an unused cycle still fails.
- **Known tradeoff, stated explicitly**: because this is pure text substitution, a bad alias can produce nonsense that only surfaces as a type error at the (already-expanded) use site accepted as the cost of keeping this "pretty simple" rather than a hygienic macro system. **As shipped (MK2-S1-M3)**, errors point back to both ends: the alias's definition span and its use span, the latter as a `Diagnostic` note that may live in a different file.
- Import-path aliasing is just this same mechanism applied to `impl "..."` strings no separate implementation needed once general expansion exists.

Shipped as MK2-S1-M3 (`preprocess.rs`), on the token/file layer before real parsing it was originally pencilled in alongside the import work, but the import layer shipped first as MK2-S1-M1 with no dependency on it. It was the last piece of the original MK2. It is the one MK2 item that is *textual*, not name-resolution: `resolve.rs` never sees an unexpanded `%*name*%`. See the MK2-S1-M3 milestone entry for how the reference is recognized, how the two-span diagnostics work, and why `resolve.rs` grew a discovery phase (an `impl` path may itself be an alias, so the file graph and the alias table each need the other).

## Capability-based security (the headline feature)

Every `impl` can carry a `caps(...)` clause granting access to privileged resource domains. Without a grant, the corresponding built-in namespace simply isn't a resolvable name inside that module enforced by the same mechanism as any other undefined-identifier error, not by a runtime permission check that could be forgotten or bypassed.

```
impl "std/fs" caps(fs = ["/var/log/myapp/**"])
impl "std/net" caps(net = ["api.example.com:443"])
impl "std/rand"
```

**Capability domains (v1)**: `fs` (path/glob allowlist), `net` (host:port allowlist), `process` (spawn allowlist), `env` (variable-name allowlist). Two more are treated as capabilities rather than ambient services, matching what WASI does with `wasi:clocks` and `wasi:random`: `time` (clock reads) and `rand` (entropy).

Each domain has a corresponding bare built-in namespace (`fs`, `net`, `process`, `env`, `time`, `rand`) exposing gated wrapper functions there is no other way to reach the underlying syscalls; user code never gets raw `open`/`connect`/`fork`/`getenv`. Namespaces are bare, not `x.`-prefixed: `x.fs` reads as field access on a variable named `x`.

**Enforcement mechanism, split by what's actually checkable at compile time:**
- *Reachability (structural, 100% compile-time)*: `resolve.rs` tracks a capability-domain set per module (computed from its incoming `caps(...)` grant). Resolving `net.connect` when the current module's set doesn't include `net` is a resolution error identical in kind to referencing an undefined variable. This is what makes "not reachable through reflection or a workaround" true: there's no reflection in this language, and the symbol genuinely isn't in scope.
- *Fine-grained matching (host/path allowlist patterns, compiler-inserted runtime check)*: the specific argument to `net.connect("api.example.com", 443)` is often a dynamic string, so the compiler embeds the granted allowlist as constant data for that module and inserts a check at each gated call site, generated as part of `codegen_c.rs`. The module itself never holds a capability *value* it could pass around or forge only the compiler-generated call sites know the allowlist.

**Grant semantics** (the part that must be right):
- Patterns are absolute paths. `*` matches within one path segment, `**` crosses segments and is legal only as a full segment. No other metacharacters, no regex, no brace expansion.
- A pattern must not contain a `..` segment; rejected at the `impl` line, at compile time.
- At each gated call site: (1) lexically normalize the argument, (2) reject if it escapes the pattern root via `..`, (3) resolve symlinks (`realpath`), (4) match the resolved absolute path against the module's compiled-in allowlist, (5) only then perform the syscall. Steps 3 and 4 must not be reordered resolving symlinks first is the reason a link pointing outside the grant is denied.
- Denied calls panic with the call-site span, the attempted path, and the module's allowlist.

**A pool key is a host string, and a pooled socket is only ever reused for the string it was opened with.** `net`'s check matches the host as written, so `Pool` is keyed on `(host, port)` exactly as the caller spelled it: a connection opened for `127.0.0.1` is never handed to a request for `localhost`, even though both resolve to the same address. That is not conservatism, it is the only sound rule the two spellings can fall under different grant patterns (`caps_net_denied` exists precisely because they do), and the grant is what authorizes the connection, not the address.

**A library module does not re-grant, and the check uses the patterns it *received*** (verified while planning MK5-S4, and the mechanism the HTTP client relies on): a module with a plain `impl "std/net"` and no clause of its own is governed entirely by whatever its importer wrote on the `impl` line that reached it, and the runtime check uses *those* patterns. So `std/http` carries no policy of its own, and the no-escalation rule only constrains clauses a module actually writes. One consequence for tests: a denial prints the *resolved* path (`/etc/hosts` → `/nix/store/...`), so a fixture must assert on the module name or the granted pattern, never on the path as written.

**Delegation/no-escalation rule**: a module's capability set is *received* from whoever imports it, never self-declared. When module A does `impl "B" caps(net = [...])`, the resolver requires that grant to be a subset of A's own capability set a module can delegate less than it has, never more. The root/entry module (`main`) implicitly holds the full set granted by `coral.toml` (or by default, everything, until a later milestone adds a way to restrict `main` itself).

**Scope note**: every domain now has a real implementation except `env`, whose domain, grant and variable-name check are real and whose bodies are the remaining stub. `fs`, `process` and `net` match paths, program paths and hosts respectively; `time` and `rand` have no named resource to match at all, so the check is the domain gate alone.

This depends on the multi-file `impl` import system and on the heap (`fs.read` cannot return contents without an owned buffer), so both precede it see the renumbered milestones below.

## Language v1 feature set

- Primitive types: `i32`, `i64`, `u32`, `u64`, `f32`, `f64`, `bool`, `char`, `u8`, `u16`, `usize`, `isize`, `Unit` (void-equivalent, implicit) all present; `u8`/`u16`/`usize`/`isize` landed with `Bytes` (MK3-S1-M1), since a byte buffer cannot be described without them
- Builtin owned heap type `Bytes` (non-`Copy`, moves, drops at end of scope) the byte-level owned type, and the substrate for `String` **done** (MK3-S1-M1), including the builtin methods `empty`/`from_str`/`len`/`is_empty`/`get`/`set`/`clone`
- Owned `String` (non-`Copy`, moves, drops at end of scope, utf-8 validated) **done** (MK3-S1-M2), with `empty`/`from_str`/`from_bytes`/`len`/`char_count`/`is_empty`/`clone`/`into_bytes`. It is required in v1 because `syntax.md` uses it as a `datac` field and as a parameter type, and a `datac` may only hold owned fields
- `extern fun` declarations whose bodies the compiler supplies **done** (MK3-S1-M2). Only compiler-shipped stdlib modules can have one; user code gets a compile error naming the reason
- `&str` string literal / borrowed-view type (points at static data or a substring of another `&str`)
- `val` (immutable) / `var` (mutable)
- `fun name(param: T, ...).R: T { ... }`, explicit param/return types, expression bodies, local type inference for `val`/`var`
- Receivers: `fun T.m()`, `fun mut T.m()` (sugar over an implicit first parameter)
- `datac` with named fields, each `val` or `var`
- Control flow: `if`/`else` (as statement and expression), `while`, `when` (boolean/`else` form), `return`
- Operators: arithmetic, comparison, logical (`&&`, `||`), unary `-`/`!`, compound assignment (`+=`, `-=`, `*=`, `/=`, `%=`). Operands must match exactly there is no implicit numeric conversion so every integer type has `to_<T>` methods (`to_u8`, `to_i64`, `to_usize`, …), where widening is exact and a narrowing that changes the value panics **done** (MK5-S1-M2)
- References: `&expr`, `&mut expr`, deref `*expr`; parameter/receiver positions use the bare/marked forms instead
- Moves: assigning/passing a non-`Copy` value transfers ownership; using it after is a compile error
- Borrowing: shared by default, exclusive marked `mut`, enforced aliasing (many immutable XOR one mutable), enforced "borrow can't outlive owner" using lexical (scope-based) lifetimes not full non-lexical-lifetime inference; that's a stretch goal. Two refinements arrived with MK3-S2-M1: a borrow produced by a *statement's* temporary ends with that statement rather than with the block, and a shared borrow may be taken of a temporary that lives for the enclosing expression (a call argument); a call that *returns* a borrow still needs a place to borrow, because the result outlives the statement
- `print(expr)` / `print!(expr)` builtins for output, emitted directly to C `printf`
- String templates `"$name"` / `"${expr}"` desugaring to `print`/`printf` formatting **done** (MK2-S1-M2), as a formatted `print`, not a string builder
- Named arguments at call sites with 2+ params
- `pd` passdowns **done** (MK2-S1-M2), module-scoped and writable from any function that references them; the hidden parameter is a shared borrow unless the function writes the passdown, directly or through a callee (item 17)
- `impl` imports **done** (MK2-S1-M1); receiver-form methods **done** (MK1-S4-M2)
- `alias` substitution **done** (MK2-S1-M3)
- `caps(...)` grants **done** (MK4-S1-M1): per-module domains received from the importer, no-escalation enforced, ungranted namespaces unresolvable, and the allowlist compiled in and checked at each call site
- `const NAME: T = expr` at top level, folded to a literal and substituted **done** (MK2-S2-M1)
- Argument-type overloading for `math.abs`/`min`/`max`/`clamp`, stdlib-only **done** (MK2-S2-M1)
- `&str`/`&T` return elision (a returned borrow is the first function's first reference parameter) **done** (MK2-S2-M1)
- Builtin owning pointer `Box<T>` (non-`Copy`, one value or empty, drops its value and frees the cell) **done** (MK3-S2-M2), with `Box.of(x)`/`Box.empty()` and `is_empty`/`get`/`set`/`take`. It exists so a `datac` can be recursive: a type may name itself, or a later type, through a container, and only through one
- Builtin owned sequence `List<T>` (non-`Copy`, grows, drops its elements and its buffer) **done** (MK3-S2-M1), with `List.of(x)`/`List.empty()` construction and `len`/`is_empty`/`push`/`get`/`set`/`pop`/`clear`. One type argument, in type positions only; elements are the scalars, `String`, `Bytes`, or a `datac` not a borrow or `str` view, since an element has to outlive the list

Everything listed above as v1 scope now exists except what is named under "Not shipped, and known" in "Current state": the `env` bodies, `fs.mkdir_all`, and the length-carrying view that a few `str`/`path` functions are waiting for. See "Current state".

Deferred beyond v1 (explicitly out of scope for now, revisit later): traits/interfaces, closures, explicit lifetime annotation syntax (`'a`), non-lexical lifetimes, async, unsafe blocks, exceptions, external/third-party dependencies (local-file `impl` imports are in scope see Imports & Capabilities sections), and the language's own machine-code backend (MK7). Pattern matching over a tagged union moved *into* v1: `enum` (roadmap's A-1 / MK8-S1-M1 part 1) shipped, so that item is off this list — and generics shipped in MK8-S1-M1 part 2, so `enum Option<T>`, `datac Pair<A, B>`, `fun f<T>` and a `Map` with a scalar key type are all expressible now. Heap collections are the other item that moved: `List<T>` shipped as MK3-S2-M1, growing an owned buffer in place (`String.push`, `Bytes.append` in memory) shipped as MK5-S1-M1, and the `for` loop over a sequence shipped as MK8-S1-M3 (open items 27 and 46 are resolved; the `iter`/`next` protocol is not scheduled). This paragraph lists what v1 deliberately leaves out; the full post-v1 backlog, including everything a *self-hosted* `kelpc` and a *deep-systems* Kelp would each still need, is the Backlog part of `roadmap.md`.

## Standard library

Merged from the former `stdlib.md` (now a pointer stub outside the project). What ships *as* the stdlib: namespaces, API sketches, capability wiring, and the language features the stdlib forces.

### Decisions locked

1. **`Bytes` before `fs`.** The builtin owned byte buffer lands *before* the fs milestone, not after. Rationale: `fs` with no content transfer (`exists`/`size`/`mkdir`/`remove` only) is a fake feature that would have to be rewritten, and the heap milestone is needed regardless to make ownership/moves have runtime consequence. Doing the heap once, before `fs`, means `fs` is implemented once. Cost is contained by scope: one builtin type with one fixed drop rule, no user-defined destructors, no generics, no growable-buffer machinery.
2. **Failure model: panic + error `datac`.** Panic for programmer errors and unrecoverable IO; a small error `datac` for recoverable cases people actually branch on. No `-1` sentinels, no out-param `try_` family. Both are replaced by `Option`/`Result` when generics arrive.
3. **Stdlib implemented as `.kp` sources shipped with the compiler**, with an `extern` declaration form whose body the compiler supplies. The raw syscall exists only inside the gated module (`std/fs.kp`), and the compiler inserts the allowlist check at that boundary which keeps the capability story structurally honest rather than a wrapper convention.
4. **Stdlib-only argument-type overloading.** `math.abs`/`min`/`max`/`clamp` resolve on argument type. The rule lives in `infer.rs` and is available only to compiler-shipped modules, so user code gains no new complexity and no user-defined overloading is implied.
5. **Buffers are fixed-size in v1** *superseded in part.* This decision said no reallocation and no length/capacity distinction; MK3-S2-M1 broke it for `List<T>`, and MK5-S1-M1 broke it for `String`/`Bytes`, which now carry a `cap` and grow in place (`push`/`push_str`/`append`/`reserve`/`with_capacity`). What is left of the original intent: `mem.alloc(n)` returns a fixed-size buffer, and a *borrow* of a buffer that then grows is a compile error, so growth is always visible in the types (`&mut`). The old text is kept below because it shaped everything built before MK5-S1 "a `List` is how you build *N* items" was true for four milestones. ~~In-place buffer growth (`String.push`, `String.concat`, `Bytes.append`) is still deferred; a `List` is how you build *N* items for now.~~

### Constraints the language puts on the stdlib

These are not preferences, they are walls. Everything below is shaped by them.

| Constraint | Consequence for stdlib |
| --- | --- |
| `Box<T>` exists (MK3-S2-M2): one value or empty, and the reason a `datac` can be recursive | Anything that needs *N* items and indirection both (`fs.walk` returning a tree, a graph) can be built from `List` + `Box` by hand; nothing in the language makes it cheaper. A recursive walk is written recursively, and both it and the recursive drop recurse on the C stack (open item 33). |
| No slices, and `List<T>` is a fixed *type* per element | *N* items are expressible (`fs.list`, `str.split` return `List<String>`), and a function *can* now be generic over the element type (`fun first<T>(xs: &List<T>)`, A-2) — but `List<i32>` and `List<String>` are still different types, and a list cannot hold a borrow or another list. `argv` and file contents are `List<String>`/`Bytes` (MK5-S2-M1); a list is walked with `for x in l { }`, which the parser desugars to that `while i < l.len()` shape (MK8-S1-M3; open item 27 is resolved — the `iter`/`next` protocol is not scheduled). |
| Owned `String`/`Bytes` grow in place (MK5-S1-M1) | Text and binary data can be built incrementally with `push`/`push_str`/`append`/`reserve`; a `&str`/`&Bytes` view of a buffer cannot outlive a growth of it, because growth takes `&mut` (so the borrow checker refuses it). `mem.alloc(n)` is still fixed-size. |
| `&str` is a borrowed view only | String functions return substrings of their input, or an owned `String`/`Bytes`. |
| `Option`/`Result` in the stdlib (MK5-S6-M1, combinators in MK8-S1-M2 phase D) | `if let` and `?` branch on and propagate them, `std/option`/`std/result` ship the two enums and their helpers, every native that used to return an `ok` flag returns one of them, and the combinators (`map`, `and_then`, `or_else`, `each`, `map_err`) take a closure, so a callback may capture. What is still missing is error *conversion* (traits, A-12) and the helpers that hand a payload out by value for an owned payload (partial moves, A-11): both are now the recorded reason those are `Copy`-only. |
| Function values, `fun` literals, and closures (MK8-S1-M2 phases B/C/E/F) | A callback is expressible: a declared `fun` is a value of type `fun(T).R: U`, a `fun` literal is written where an expression is expected, a literal that names an enclosing binding is a **closure** (`closure(T).R: U`, which owns the heap environment its captures were moved into and drops it where it dies), a plain function value is widened into a closure where one is expected, and `std/list`/`std/option`/`std/result` take closures — so a callback can carry a binding, and a `for_each_entry(path) { }`-shaped escape hatch is expressible. What a `sort_by`-shaped comparator over an owned element type still needs is A-11's partial moves, not a closure. |
| No exceptions | Fatal failures panic; recoverable ones use the error `datac` convention. |
| Capability gating is the headline feature | Gated namespaces must be unreachable without a grant. `fs` cannot be a thin shim over ambient syscalls. |
| `const` exists (MK2-S2-M1, general evaluation in A-19) | `math.PI`/`E`/`TAU` are folded literals. An initializer may use earlier constants, the operators, `if` and an integer conversion, and since A-19 it may also call a plain `fun` whose own body stays inside that set: the fold evaluates the call itself (`infer.rs`'s const evaluator), with a step and a call-depth budget. A builtin, an `extern fun` and a method are still out of reach. |

**The single decision that unblocked the most:** one compiler-builtin owned byte buffer. It unlocks `fs.read`, `fs.write`, `path.join`, `str.to_upper`, `env.get`, and it is the substrate `String` is built on. Locked as decision 1: it ships before `fs`, at MK3.

### Namespace model

Bare lowercase namespaces, no `x.` prefix (the `x.fs` spelling reads as field access on a variable named `x`).

Two tiers:

- **Prelude, always in scope, ungated** `print`, `print!`, `panic`, `assert`, and the pure namespaces `math`, `str`, `path`, `mem`, `io`. No import needed, no capability. These cannot reach the outside world; they are arithmetic and byte shuffling.
- **Gated, requires an explicit grant** `fs`, `env`, `process`, `net`, `time`, `rand`. Reached only through an `impl` that carries `caps(...)`:

```kelp
impl "std/fs" caps(fs = ["/var/log/myapp/**"])
impl "std/rand"
impl "std/fs"          // legal but useless: fs.* will not resolve
impl "std/fs" caps(fs = ["/tmp/**"], env = ["*"])   // several clauses: legal, verified
```

Two things this sketch used to imply that are not true. **"Prelude" means ungated, not in scope**: every namespace including `std/str` needs an explicit `impl` (the repo's own `caps_list` project needed `impl "std/str"` before `str.join` resolved), and `panic`/`assert` are not builtins at all (open item 35). And **`impl "std/rand"` with no clause resolves only because it is written in the entry module**, which holds every domain; the same line in a library module would fail unless its importer delegated `rand` to it (open item 18). The usable distinction is *ungated* vs *gated*.

Reading a clock and reading entropy are **capabilities**, not ambient services the same classification WASI uses (`wasi:clocks`, `wasi:random`). A program that cannot observe time or randomness gets reproducibility for free.

Cheap conveniences stay ungated on purpose: `io.print`/`io.read_line` (stdin/stdout/stderr), `time.sleep`, `process.exit`, `process.getpid`. Sleeping, exiting, and writing to your own stdout are not authority over anything shared.

### `Bytes` and `String` the two builtin owned types

Compiler-provided, not user-definable via `datac` (a `datac` cannot hold a pointer in v1, and these must).

```kelp
// compiler builtins
datac Bytes      // opaque: owned heap byte buffer, freed at end of scope (drop)
datac String     // opaque: owned heap utf-8 text, same move/drop rules

fun Bytes.len().R: usize                   // shipped (MK3-S1-M1)
fun Bytes.get(i: usize).R: u8              // shipped; panics on out-of-bounds
fun Bytes.set(i: usize, b: u8)             // shipped
fun Bytes.slice(from: usize, to: usize).R: &Bytes   // needs &-return elision (MK2-S2-M1)
fun Bytes.as_str().R: &str                 // needs &-return elision (MK2-S2-M1)
fun Bytes.clone().R: Bytes                 // shipped
fun Bytes.from_str(s: &str).R: Bytes       // shipped
fun Bytes.empty().R: Bytes                 // shipped

fun String.len().R: usize                  // shipped (MK3-S1-M2); bytes, not chars
fun String.char_count().R: usize           // shipped
fun String.is_empty().R: bool              // shipped
fun String.as_str().R: &str                // needs &-return elision (MK2-S2-M1)
fun String.as_bytes().R: &Bytes            // needs &-return elision (MK2-S2-M1)
fun String.clone().R: String               // shipped
fun String.from_str(s: &str).R: String     // shipped
fun String.from_bytes(b: Bytes).R: String  // shipped; panics on invalid utf-8
fun String.into_bytes().R: Bytes           // shipped; move, no copy
fun String.slice(from: usize, to: usize).R: &str   // needs &-return elision (MK2-S2-M1)
fun String.empty().R: String               // shipped
```

`mem.alloc(n)` (shipped in MK3-S1-M2) is the allocation primitive the fixed-size-buffer functions are built on: an owned, zero-filled `Bytes` of `n` bytes, so `str.to_upper` allocates once and fills it through `Bytes.set` rather than growing anything.

Rules:
- Both are non-`Copy`. Assigning or passing one moves it. Pass `&Bytes`/`&String` to read, `mut String` to write.
- Moves and drops are fully checked. This is the first place ownership has runtime consequence, so MK3 is also where MK1-S3-M1's move logic and MK1-S4-M1's borrow logic surface their bugs.
- Drop happens at end of scope, after any borrows of it end. A value moved out of a scope is not dropped by that scope. A `datac` holding one drops its fields recursively (MK3-S1-M2), and an owned `pd` is freed by an `atexit` handler (MK3-S1-M2).
- Size is fixed at construction (decision 5). Growable ops are deferred, which is why `String.push`-style functions do not appear in the API yet.
- `String` is not a wrapper *keyword* over `Bytes` in the syntax sense it is a second builtin type with its own move/drop identity. `into_bytes`/`from_bytes` convert with an ownership transfer.
- `&str` is the borrowed view type: static literals, substrings of another `&str`, or (once `as_str` exists) `String.as_str()`.
- **A function can return a borrow** (MK2-S2-M1), under the elision rule: the result borrows the first reference parameter. That is what makes `String.as_str`, `str.trim_start`, `str.substring` and `path.file_name` views rather than copies.
- **Printing an owned value needs no place** (MK2-S2-M1): `print(str.to_upper(s))` hoists the temporary into a C variable and frees it right after the `printf`, so the allocation happens once and is not leaked (`heap_temporary_print` runs under AddressSanitizer). What *does* need a place is a call that **returns** a borrow of a temporary see MK3-S2-M1 and open item 30.

### `Box<T>` the builtin owning pointer (MK3-S2-M2)

```kelp
datac Node(val value: i32, val next: Box<Node>)   // the reason it exists

val empty: Box<i32> = Box.empty()     // a cell holding nothing (NULL)
val one = Box.of(1)                   // allocates, moves the value in

fun Box.is_empty().R: bool
fun Box.get().R: T                    // a copy, when T is Copy
fun Box.get().R: &T                   // a borrow, when T owns memory
fun Box.set(v: T)                     // frees the old value, reuses the cell
fun Box.take().R: T                   // moves the value out; panics when empty
```

- Elements may be any sized type, *including* another container and any `datac`; a borrow or a `str` view is refused, and a `List<Box<T>>` is refused (a list's buffer holds elements inline use `Box<List<T>>`).
- Empty is a real state, not an error: `is_empty()` is the predicate, `take()` is how a value comes back out, and `get()`/`take()` on an empty box panic because v1 has no `Option`.
- Its real purpose is recursion: a `datac` may name itself or a later `datac` **through** a container, which is what makes a linked list, a tree, and a mutually recursive pair expressible. Directly by value the type would have no finite size, and that is a compile error naming the fix.
- Same monomorphization as `List`: one C struct and one function set per element type, emitted with the ordering that lets a container point at a struct whose body comes later.

### `List<T>` the builtin owned sequence (MK3-S2-M1)

```kelp
val names: List<String> = List.empty()     // element type from the written type
var nums = List.of(1)                      // element type from the argument

fun List.len().R: usize
fun List.is_empty().R: bool
fun List.push(v: T)                        // moves an owned element in
fun List.get(i: usize).R: T                // a copy, when T is Copy (a scalar)
fun List.get(i: usize).R: &T               // a borrow, when T owns memory
fun List.set(i: usize, v: T)               // frees the element it replaces
fun List.pop().R: T                        // moves one out; panics when empty
fun List.clear()
// MK5-S1: the ordering surface. Each is defined only for an element type that
// has the capability it needs (`datac` elements are refused with a reason).
fun List.sort()                            // value order; String/Bytes byte order
fun List.reverse()
fun List.contains(v: T).R: bool            // &T for an owned element type
fun List.slice(from: usize, to: usize).R: List<T>   // an owned copy
fun List.insert(i: usize, v: T)
fun List.remove(i: usize).R: T             // moves one out; panics out of range
// `List.index_of` is deliberately absent: its `-1` sentinel is the convention
// decision 2 rejects. It arrives with `Option` (MK5-S6-M1) open item 42.
```

Rules:
- One type argument, in type positions only. `List` is a compiler builtin, not a user-definable generic: the parser accepts `List<T>` and rejects type arguments on every other name.
- Elements are the scalars, `String`, `Bytes`, or a `datac`. A borrow (`&T`) or a `str` view is refused (an element outlives the list, and v1 has no lifetime syntax to say so), and so is a nested `List`.
- Non-`Copy`, so it moves and drops like `Bytes`/`String`: `push`/`pop`/`set` are where a move in or out happens, and the list frees its elements and then its buffer at the end of the owning scope. That includes a `datac` field and an owned `pd`.
- `get` is the one signature the element type changes by value for a `Copy` element, by pointer for an owned one. There is no `Option`, so an out-of-range index panics, and `pop` on an empty list panics.
- **Codegen is monomorphized**: one C struct and one function set per element type the program mentions. `List<i32>` and `List<String>` are different C types, so a function cannot be written *over* an element type (no `fun sum(xs: List<T>)`) that needs type parameters and real generics, which is MK3-S2-M2's neighbourhood, not MK3-S2-M1's.
- Iteration is a `for` over a `List` (`for x in xs { ... }`, desugared by the parser to an index and a `while`), or that index-and-`while` walk written out when the loop is not a front-to-back pass (MK8-S1-M3).


Blocks that stayed blocked on these two types, and no longer are: returning *N* items. `fs.list` and `str.split` landed with the sequence type (MK3-S2-M1), and `List<T>` was the substrate `process.args`/`process.run(argv)` needed both shipped in MK5-S2-M1.

### Module map by milestone

| Tier | Ships | Milestone |
| --- | --- | --- |
| 0 | `print`/`print!`; `str.len`/`byte`/`is_empty`/`to_upper`/`to_lower`/`repeat` and `path.is_absolute`/`join` (**shipped in MK3-S1-M2**); `math`, `panic`, `assert`, and the borrowing `str`/`path` functions | MK3-S1-M2 (partial), MK2-S2-M1 for the rest |
| 1 | `Bytes` + `String` + `mem` (**shipped in MK3-S1-M1/MK3-S1-M2**); allocation-returning `path` and `str` functions (**MK3-S1-M2** for `str.to_upper`/`to_lower`/`repeat` and `path.join`) | MK3 |
| 2 | `fs` **shipped** (MK4-S1-M1); `env` gated with stub bodies (MK4-S1-M1); `io` line reads and `process` **shipped** (MK5-S2-M1); `net` **shipped** (MK5-S3) | MK4-S1-M1, MK5-S2-M1, MK5-S3 |
| 3 | `time`, `rand` (both gated, both real) **shipped** (MK4-S1-M1) | MK4-S1-M1 |
| 4 | ~~collections~~ (**shipped as MK3-S2-M1**: `List<T>`, `str.split`, `str.join`, `fs.list`) | MK3-S2-M1 (done) |
| 5 (MK5-S1) | growable `String`/`Bytes` + `str.concat`/`from_i64`/`from_f64`/`parse_*` + `replace`/`char_at`; `std/fmt`; `List` sort/reverse/slice/insert/remove/contains; `panic`/`assert` | **MK5-S1 done** (parts 1, 2a, 2b, 3) `index_of` deferred to MK5-S6-M1, open item 42 |
| 6 (MK5-S2-M1) | `io` (`File`, `Line`, `BufReader`, streams, `read_line`/`read_all`/`read_lines`); `process` (`run`/`spawn`/`args`/`exit`/`getpid`) | **MK5-S2-M1 done** plus the `main(argc, argv)` codegen change argv needed, open item 50 |
| 7 (MK5-S3) | `net` for real (sockets, listener, DNS, `poll` readiness, timeouts) + the capability vocabulary fix | **MK5-S3 done** `127.0.0.1:*` is now a grant a program can write |
| 8 (MK5-S4) | `http`: keep-alive, response framing (Content-Length, chunked, read-to-EOF), the connection `Pool`/`Lease`, redirects, timeouts, header lookup; `https` is MK5-S4b-M1 | **MK5-S4 done** plus three compiler changes: `coral.toml`'s numeric `http_pool` key spliced into the shipped source, `\r` as a string escape, and CR escaping in the C string-literal emitter |
| 9 (MK5-S5) | `Map<V>`; `fs` breadth (`mkdir_all`/`remove_all`/`walk`/`copy`/`open`/`metadata`/`temp_path`/`read_lines`); `path` polish; real `env` bodies | **MK5-S5 done** (parts 1, 2, 3) plus two capability-matcher gaps: `open`'s mode string was matched as a path, and a trailing `/**` did not admit the granted directory itself |
| 10 (MK5-S6-M1+) | `Option`/`Result` (language-gated), generic containers, TLS, a length-carrying view, `for`, threads | blocked on language work |
| 11 (MK6-S1-M1) | **Process threads** a language milestone: function references, a move-taking parameter, a per-thread panic and cleanup story, atomics/`Mutex`, a `Send`-shaped rule | after MK5-S6-M1; MK5-S7-M1 records every prerequisite |
| 12 (MK6-S2-M1) | **Multithreaded downloading** over MK6-S1-M1 | after MK6-S1-M1 |

Tier 1 strictly precedes tier 2. A compiler that can `fs.read` must already have working move-and-drop for `Bytes` which it now does. In the S tiers, MK5-S1 → MK5-S2-M1 → MK5-S3 → MK5-S4 was a strict chain (text you can build, then streams and programs, then sockets, then HTTP) and it is complete; MK5-S5's `Map` was independent of all of them, and the fs half of MK5-S5 only wanted MK5-S1 and MK5-S2-M1 for convenience; both shipped. MK6-S1-M1 and MK6-S2-M1 are a second chain off the end: neither is reachable from stdlib work, because threads need language surface that does not exist (MK5-S7-M1).

### Failure model

No `Option`, no `Result`, no exceptions. Available conventions:

1. **Panic** `fs.read` on a missing file panics with the call-site span and the granted allowlist printed. Simple, correct, unusable for expected failures.
2. **Sentinel** `str.index_of` returns `i64` with `-1` for absent. Cheap, breaks down for non-numeric returns.
3. **Out-param + bool** `fun path.try_join(a: &str, b: &str, out: mut Bytes).R: bool`. Composable, ugly, C-flavored.
4. **Error `datac`** — *superseded by MK5-S6-M1*: `datac Match(val ok: bool, val at: usize)` then `if m.ok { }`. It was the failure model until `Option`/`Result` shipped; every such native now returns one of those (see the stdlib sections).

Locked (decision 2): panic for programmer errors and unrecoverable IO; convention 4 for the recoverable cases people actually branch on (`str.index_of`, `str.parse_i64` on bad input). Conventions 2 and 3 are rejected: no `-1` sentinels, no out-param `try_` family. Document `Panics:` in the doc comment of every panicking function.

When generics arrive, replace every error `datac` with `Option<T>` / `Result<T, E>` and keep the old names as deprecated wrappers.

### API sketches

#### math (pure, ungated)

```kelp
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
- Alternative (later, if desired): debug builds panic on overflow, `-O2` builds wrap. Requires codegen to emit checked arithmetic in debug mode a real cost, so not v1.

#### str (borrowed `&str` in; `&str`, `String`, or `Bytes` out)

**Shipped in MK3-S1-M2** (`kelpc/std/str.kp`): `str.len`, `str.byte` (the bounds-checked one the section below does not name, but which `to_upper`/`join` are built on), `str.is_empty`, `str.to_upper`, `str.to_lower`, `str.repeat`; MK2-S2-M1 added the `&str`-returning half and `path`; MK3-S2-M1 added `split`/`join`/`copy_range`/`copy_into`, which are the `List<String>` entry and exit. Still unwritten and needing no new rule: `replace`, `from_i64`, `from_f64`, `concat` (see open item 25 concatenation still has no spelling, which is why a path is built with `path.join` or `str.join`).

```kelp
fun str.len(s: &str).R: usize              // bytes, not chars
fun str.is_empty(s: &str).R: bool
fun str.char_count(s: &str).R: usize
fun str.trim(s: &str).R: String   // shipped: truncation cannot be a view
fun str.trim_start(s: &str).R: &str
fun str.trim_end(s: &str).R: &str
fun str.starts_with(s: &str, prefix: &str).R: bool
fun str.ends_with(s: &str, suffix: &str).R: bool
fun str.contains(s: &str, needle: &str).R: bool
fun str.index_of(s: &str, needle: &str).R: Option<usize>   // `None` when absent
fun str.eq_ignore_case(a: &str, b: &str).R: bool
fun str.char_at(s: &str, i: usize).R: u32    // MK5-S1 shipped `u32`, not `char` (item 53)
fun str.byte_index(s: &str, i: usize).R: usize
fun str.repeat(s: &str, n: usize).R: String
fun str.split(s: &str, sep: &str).R: List<String>   // shipped (MK3-S2-M1)
fun str.join(parts: List<String>, sep: &str).R: String   // shipped (MK3-S2-M1)
fun str.copy_range(s: &str, from: usize, to: usize).R: String   // shipped (MK3-S2-M1); the piece-copier split is built on
fun str.replace(s: &str, from: &str, to: &str).R: String
fun str.to_upper(s: &str).R: String
fun str.to_lower(s: &str).R: String
fun str.from_i64(n: i64).R: String
fun str.from_f64(x: f64).R: String
fun str.parse_i64(s: &str).R: i64                  // panics on garbage
fun str.parse_f64(s: &str).R: f64
```

Text-returning functions return `String`, not `Bytes` that is the fix forced by `String` being v1. Byte-level functions (`fs.read`, `net.recv`, `rand.bytes`) still return `Bytes`.

Receiver form is available and preferred in user code: `name.trim()`, `s.contains("x")` extension functions on `&str`.

**Borrow rule this forces:** a function returning `&str` without an explicit lifetime must borrow from its first reference parameter (Rust's elision rule). `fun str.trim(s: &str).R: &str` returns a slice of `s`. Without this rule the type checker cannot accept the `&str`-returning half of `str`/`path` at all, so it is a hard prerequisite, not a nicety and MK3-S1-M2 shipped only the half that does not need it. Iteration over chars waits for collections.

#### path (pure; allocation-returning parts need `Bytes`/`String`)

**Shipped in MK3-S1-M2** (`kelpc/std/path.kp`): `path.is_absolute` and `path.join`. The predicate/slicing family below (`file_name`, `parent`, `extension`, `stem`) returns `&str` and waits on return elision with the rest of `str`. `path.normalize` is deferred *on purpose* it is the same lexical normalization the MK4-S1-M1 capability matcher needs for its `..` rejection, so it should be written once, against the grant matcher, rather than written here and then re-derived there. `path.join` does not normalize (`a/` + `b` collapses the duplicate separator, which is what users mean, but `..` is left alone).

```kelp
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

**Shipped**, with the grant/gate/check machinery described under MK4-S1-M1 and the test suite that goes with it: `read`, `read_text`, `write`, `write_text`, `exists`, `is_file`, `is_dir`, `size`, `remove`, `rename`, `mkdir`, plus two that were not in the sketch: `append(path, data)` (the streaming primitive it needs one buffer's worth of data at a time, not a growable one, which is why streaming a file needed no collections work) and `sync(path)` (an fsync, so a timing of a write is a timing of storage rather than of the page cache). `mkdir_all` is the one function from the sketch below that is *not* here (it needs to walk prefixes of its path).

```kelp
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
fun fs.symlink(target: &str, link: &str)       // shipped in MK5-S2-M2; both paths checked
```

`fs.symlink` arrived with MK5-S2-M2 (the MK5-S5 continuation) and checks **both** of its paths, which is the `fs` default and the safer reading: a grant is closed under link creation, so a tree inside a grant cannot come to hold a link out of it. `target` must be absolute, since the check normalizes every path it matches, and there is deliberately no `read_link` — the check resolves links, and nothing has needed to resolve one back to its target text.

No `try_` variants the out-param family was rejected in decision 2. A recoverable read is expressed by checking first (`fs.exists`) or by accepting the panic. `fs.list` shipped in MK3-S2-M1, as soon as there was a sequence type to return: entries are *names*, sorted, and each one is joined to a directory with `path.join`. `fs.remove` for a directory tree is `fs.remove_all`, deferred. `fs.append` **shipped**, contrary to the plan's own note that it belonged with the growable-buffer work (decision 5): appending a chunk at a time needs no growable buffer, only a chunk, so the streaming case never depended on collections. What is still missing is `Bytes.append`/`String.push` growing a *buffer* in memory, which is a different operation from appending it to a file.

**Grant semantics** (the part that must be right):

- Patterns are absolute, glob only. `*` matches within one path segment, `**` crosses segments and is only legal as a full segment. No other metacharacters, no regex, no brace expansion.
- A pattern must not contain a `..` segment. Rejected at the `impl` line, at compile time.
- At each gated call site: (1) lexically normalize the argument via the same `path.normalize`, (2) reject if it escapes the pattern root via `..`, (3) resolve symlinks with `realpath`, (4) match the resolved absolute path against the module's compiled-in allowlist, (5) only then perform the syscall.
- Symlinks are resolved **before** matching, so a link pointing outside the grant is denied. This is the whole reason step 3 exists and it must not be reordered.
- Denied call panics with the call-site span, the attempted path, and the module's allowlist. `Panics: if path is outside the granted capability` in the doc comment.
- The allowlist is emitted as constant data per module. No API can widen it at runtime, and no stdlib function accepts a capability as a value, so a grant cannot be smuggled through a function argument.

Glob metacharacters inside a granted path *argument* are literal `fs.read("*.txt")` reads a file named `*.txt`.

#### io (ungated; the full sketch is MK5-S2-M1's)

```kelp
fun io.print(s: &str)
fun io.print_bare(s: &str)               // `print`/`print!` already do these two
fun io.eprint(s: &str)
fun io.eprint_bare(s: &str)
fun io.stdin().R: File                   // one stream type over an fd; see MK5-S2-M1
fun io.stdout().R: File
fun io.stderr().R: File
fun read_line().R: Option<String>   // `None` at end of file
fun io.read_line().R: Line                 // needs String, and a buffer: MK5-S2-M1
fun io.read_all().R: Bytes
fun io.read_lines().R: List<String>
datac BufReader(val file: File, var buf: Bytes, var start: usize, var end: usize)
```

`io` is ungated on purpose (open item 38): writing to your own stdout, sleeping and exiting are not authority over anything shared, and the WASI-flavoured alternative gating stdin can arrive with `coral.toml [caps]` when `main` itself becomes restrictable (open item 18). **Shipped in MK5-S2-M1**, with the sketch's `File` surface (`read`/`read_into`/`read_all`/`write`/`write_text`/`write_str`/`close`/`flush`/`set_nonblocking`/`wait_readable`/`wait_writable`), `Line` + `io.read_line`/`read_lines`/`io.read_all`, a `BufReader` with `read_line_into`/`read_line`/`read_all`, and the three standard descriptors as ordinary `File`s. The old "stdin is unreadable" note is obsolete: `io` is a streaming API over a descriptor, which is exactly what made `fs.read("/dev/stdin")` the wrong tool.

#### fmt (ungated, pure)

```kelp
fun fmt.int(n: i64).R: String
fun fmt.uint(n: u64).R: String
fun fmt.float(x: f64, decimals: i32).R: String    // one extern: snprintf("%.*f")
fun fmt.hex(n: u64).R: String                     // lowercase, no prefix
fun fmt.pad_left(s: &str, width: usize, fill: u8).R: String    // fill: u8, not char (item 53)
fun fmt.pad_right(s: &str, width: usize, fill: u8).R: String
fun fmt.bytes(b: Bytes).R: String                 // "1f 8b 08" the first debugging printer
```

`fmt` is MK5-S1 and has no gate: it is arithmetic and byte shuffling. Variadic formatting is explicitly not in scope it needs generics or macros, neither of which exists (open item 43 for concatenation, and the non-goals below). **Shipped in MK5-S1-M3** (`kelpc/std/fmt.kp`), on top of `std/str`; the only C in it is `snprintf("%llx")` for `hex` and the byte-dump loop for `bytes`.

#### Map (a compiler builtin, `Map<K, V>`)

```kelp
fun Map.new().R: Map<K, V>
fun Map.len().R: usize
fun Map.is_empty().R: bool
fun Map.has(key: K).R: bool
fun Map.get(key: K).R: V             // a copy when V is Copy...
fun Map.get(key: K).R: &V            // ...a borrow when V owns memory; panics when absent
fun Map.get_or(key: K, fallback: V).R: V
fun mut Map.insert(key: K, v: V)
fun mut Map.remove(key: K).R: V
fun mut Map.clear()
fun Map.keys().R: List<K>            // insertion order, not hash order
fun Map.values().R: List<V>
```

**Shipped in MK5-S5**, with `String` keys only and a monomorphized value type: FNV-1a over the key's bytes, entries in insertion order in a dense array with a separate open-addressed index, because `keys()` must return insertion order; `get_or` and `values` are offered only for a clonable value type, since a `datac` value cannot be copied back out. `Map<Map<i32>>` is as impossible as `List<List<i32>>`, and `Map.of` does not exist because a map starts empty. There is no `Set`: `Map<bool>` covers it, and a third monomorphized generic is not worth it (open item 41). **The key type became generic in A-2** (`Map<K, V>`, with `Map<V>` kept as the `String`-key shorthand): the key's hash and its equality are generated per instantiation, and the key set is closed to `String`, the integers and `bool` because the compiler, not a trait, supplies those two operations.

#### http (MK5-S4 the payoff)

```kelp
// ungated module that holds `net`; the importer's grant governs it
impl "std/net"
impl "std/str"
impl "std/fmt"
impl "std/mem"

const pool_size: usize = __KELP_HTTP_POOL__      // from coral.toml's http_pool, spliced in by the compiler

datac Url(val scheme: String, val host: String, val port: u16, val path: String)
fun http.parse_url(s: &str).R: Url               // Panics: no scheme/host, or unsupported scheme

datac Header(val name: String, val value: String)
datac Response(val status: i32, val headers: List<Header>, val body: Bytes, val reusable: bool)
fun Response.header(name: &str).R: &str          // "" when absent (no Option)
fun Response.has_header(name: &str).R: bool
fun Response.is_ok().R: bool                     // 200..299
fun Response.text().R: String                    // Panics: invalid utf-8

datac Request(val method: String, val url: String, val headers: List<Header>, val body: Bytes)
datac Client(val timeout_ms: u64, val max_redirects: i32, val follow_redirects: bool)

// A pooled connection, owned by the caller for the length of one exchange.
datac Lease(val handle: i32, val socket: Socket, val reused: bool)
// Kept-alive connections to one host:port, keyed on the host as written.
datac Pool(val host: String, val port: u16, val size: usize, var slots: List<Socket>, var free: List<i32>)

fun http.pool(host: &str, port: u16).R: Pool      // size from coral.toml, not from an argument
fun Pool.size().R: usize                         // the configured ceiling
fun Pool.len().R: usize                          // sockets currently open
fun mut Pool.warm()                              // opens up to `size` now (optional)
fun mut Pool.checkout().R: Lease                 // lazy: opens one when none is free
fun mut Pool.release(lease: Lease, reusable: bool)
fun mut Pool.close_all()
fun Client.send(req: Request).R: Response        // one fresh connection, follows redirects
fun mut Pool.send(req: Request).R: Response      // checkout + exchange + release
fun http.exchange(client: Client, lease: Lease, req: Request).R: Response   // the concurrency-ready shape
fun http.get(url: &str).R: Response              // the one-liner, default client, no pool
fun http.post_text(url: &str, body: &str, content_type: &str).R: Response
fun http.default_client().R: Client
```

Four body-length cases, all framed without ambiguity: `Content-Length` (a `mem.alloc`-sized buffer plus `recv_into`), `Transfer-Encoding: chunked` (hex size line, that many bytes, CRLF, repeat until a zero size, skip trailers **shipped in MK5-S4**, not deferred, because keep-alive leaves no other way to find a chunked body's end), a bodyless response (`204`/`304`, and any `Content-Length: 0`), and neither header (**read to EOF, and the connection is *not* reusable** the one case that costs a connection). A socket returns to the pool only when the body was completely framed, the server did not send `Connection: close`, and no parse error occurred; a pooled socket that fails on first use is retried exactly once on a fresh connection, because a peer may close an idle connection at any moment. Redirects re-run the connect path (and therefore the capability check), so a redirect to an ungranted host is denied: the capability model working, not a limitation. No gzip, no HTTP/2, no cookies, no proxy, no idle reaping. `https` is MK5-S4b-M1 and is a dependency decision (open item 37).

#### process (gated: process) **shipped in MK5-S2-M1, extended in MK5-S2-M2**

```kelp
// `program` is separate from `args` because that is what makes the grant
// matchable: one absolute path, and argv stays a list.
fun process.run(program: &str, args: List<String>).R: RunResult
// MK5-S2-M2: the same, with the *child's* working directory. An empty `dir`
// means the parent's, which is what `run`/`spawn` now delegate with.
fun process.run_in(dir: &str, program: &str, args: List<String>).R: RunResult
datac RunResult(val code: i32, val out: Bytes, val err: Bytes)
fun process.spawn(program: &str, args: List<String>).R: Child
fun process.spawn_in(dir: &str, program: &str, args: List<String>).R: Child
// MK5-S2-M2: the fallible shape, so a missing program is a value rather than a
// panic. Every reason is decided by a `stat`/`access` pre-check *before* the
// fork. A program outside the grant is not one of them, since that refusal is
// the compiler's and it panics (open item 73).
fun process.try_run(program: &str, args: List<String>).R: result.Result<RunResult, ProcErr>
fun process.try_run_in(dir: &str, program: &str, args: List<String>).R: result.Result<RunResult, ProcErr>
enum ProcErr {
    NotFound        // nothing at that path
    NotExecutable   // there, but not a regular executable file
    DirNotFound     // `dir` is not a directory
}
fun process.executable(program: &str).R: bool  // the pre-check `try_run` uses
// SHIPPED with the wait state added, so `wait` and `is_running` agree:
datac Child(val pid: i32, val stdin: File, val stdout: File, val stderr: File, var exited: bool, var code: i32)
fun mut Child.wait().R: i32                  // exit code, or 128+signal
fun Child.kill(signal: i32)
fun mut Child.is_running().R: bool
fun process.args().R: List<String>           // your own invocation (open item 39)
fun process.arg_count().R: usize
fun process.exit(code: i32)
fun process.getpid().R: i64
```

There is **no `run_shell`**: the string-handed-to-a-shell form the earlier sketch pencilled in is gone, because a shell string cannot be checked against a pattern and is injection-prone by construction. `run`/`spawn` take the program and its argv separately. **A per-call working directory shipped in MK5-S2-M2** as `spawn_in`/`run_in`: `chdir` happens in the *child*, between `fork` and `exec`, so the parent's directory is unchanged and no grant pattern's meaning moves — which is what made the form expressible at all, given that a process-wide `chdir` is still out. The working directory is **not** a resource and is not checked against the grant; a `dir` the child cannot enter is exit code **126** (127 stays "cannot exec"). The fallible shape shipped with it, and the compiler's own refusal to call a program outside the grant is deliberately still a panic — it is a manifest error, and making it a value needs a non-panicking check mode in codegen (open item 73). `process.args()` reads argv the runtime stashed at the top of `main` (open item 50); `env.args`/`env.arg` are dropped from the sketch — argv is your own invocation, not the environment's authority. It still lives behind the `process` gate like the rest of the module, but a grant with an **empty** pattern list is enough to reach it, so reading your own argv never carries the authority to execute anything (open item 39).

#### net (gated: net, `host[:port]` allowlist)

```kelp
datac Socket(val fd: i32)
fun net.connect(host: &str, port: u16).R: Socket   // Panics: resolution, connect, or the grant
fun net.listen(port: u16).R: Listener
datac Listener(val fd: i32)
fun net.accept(l: Listener).R: Socket
fun net.close(s: Socket)
fun Listener.close()                               // shipped in MK5-S3; `net.close` takes a Socket
fun net.shutdown_write(s: Socket)
fun net.send(s: Socket, data: Bytes).R: usize      // may be short; send_all is the common case
fun net.send_all(s: Socket, data: Bytes).R: usize  // the short-write loop, in the extern
fun net.send_text(s: Socket, text: &str)
fun net.recv(s: Socket, max: usize).R: Bytes       // one read, empty at EOF
fun net.recv_into(s: Socket, buf: mut Bytes, at: usize).R: usize
fun Socket.wait_readable(timeout_ms: u64).R: bool  // one thread can serve many sockets
fun Socket.wait_writable(timeout_ms: u64).R: bool
fun mut Socket.set_timeout(ms: u64)                // SO_RCVTIMEO/SO_SNDTIMEO
fun Listener.port().R: u16                         // bind 0, learn the port (testability)
fun Socket.peer_addr().R: String
fun net.resolve(host: &str).R: List<String>        // deduplicated, resolver order
```

All shipped in MK5-S3. The pattern vocabulary is `host`, `host:port`, `*.host`, `*:port`, `*` **not** the absolute-path form the plan used to print, which the compiler rejects outright (open item 32, resolved). The check matches the host **exactly as the code wrote it**, with no DNS before matching, so a grant for `example.com` does not cover `net.connect("93.184.216.34", 80)`. Two calls are checked differently, and both deviations are deliberate: `listen` matches the **port alone**, because a listener binds every interface and there is no `net.listen(host, port)` in v1 (the consequence is open item 55), and `resolve` matches the **host alone**, ignoring any port in the pattern. `recv` returning fresh `Bytes` cannot express "read exactly N bytes" without growth, which is why `recv_into` is right next to it. TLS variants are deferred with `https` (open item 37).

#### time / rand (gated: time, rand)

```kelp
fun time.now_unix().R: i64
fun time.now_mono_ns().R: i64
fun time.sleep_ms(ms: u64)          // lives in gated std/time today, so sleeping needs a grant (item 45)
fun rand.u64().R: u64
fun rand.below(n: u64).R: u64
fun rand.bytes(n: usize).R: Bytes
```

Both are real and both are gated; sleeping is ungated "in spirit" per the plan's classification (sleeping is not authority over anything shared) but it lives in `std/time`, so a *library* module needs the grant to reach it recorded as open item 45 rather than silently fixed, since moving it means a new module boundary.

#### env (gated: env, variable-name allowlist)

```kelp
fun env.has(name: &str).R: bool
fun env.get(name: &str).R: String      // "" when unset (no Option)
fun env.set(name: &str, value: &str)
fun env.unset(name: &str)
```

**Shipped in MK5-S5**: the bodies are real (`getenv`/`setenv`/`unsetenv` behind four externs), and the check is on the *name*, before the call, so a grant of `["PATH"]` does not let the holder ask whether `HOME` exists. `env.names()` remains deliberately absent: listing the environment reads *all* of it, which a grant of one name should not permit.

### What would make Kelp genuinely useful for real programs

Ranked by how much the absence hurts, with the shortest path. "Pain" is the question "could I write a program I would actually want to run?"

1. **Text you can build** (`String.push`/`concat`, `str.from_i64`, `fmt.float`) every program that produces output hits it, and the only workaround is a byte loop (`str.to_upper` is one). *High, constant.* MK5-S1.
2. ~~**stdin/stdout/stderr as streams** (`io`)~~ **shipped in MK5-S2-M1**: one `File` over a descriptor, buffered line reading, and the three standard descriptors.
3. ~~**Running programs** (`process.run`/`spawn` + argv)~~ **shipped in MK5-S2-M1**, including the `main(argc, argv)` codegen change behind `process.args`.
4. **`panic`/`assert`** a program that cannot fail deliberately cannot be tested or guard an invariant. *Moderate, embarrassing.* MK5-S1, an afternoon (shipped).
5. **The `net` capability fix** not a feature gap but a headline-feature gap: the documented pattern form is rejected and every host would be denied. A domain that cannot be used is worse than one that does not exist. *High for credibility, zero for today's fixtures.* MK5-S3, first task.
6. **TCP** (`net.connect`/`send`/`recv`) client networking unlocks APIs, sockets, and the HTTP client. *High, blocked on 5.* MK5-S3.
7. ~~**HTTP client** the stated goal, and the payoff.~~ **Shipped in MK5-S4**, with keep-alive and the pool rather than the one-shot client the sketch described: parsing, all four body-length cases, redirects, and a connection pool whose leases are ownership-checked. What is left of the original gap is `https` (MK5-S4b-M1, TLS) and chunked *request* bodies.
8. **A keyed container** (`Map<K, V>`) today a lookup is a linear scan over `List<String>`. *Moderate.* MK5-S5; the key type became generic in A-2.
9. **Being able to *see* your data** `print` rejects a `datac` and a `List` (both probed), so there is no debug printer and nothing in the stdlib substitutes; `fmt.bytes` is the first step, and a generated per-`datac` debug function the honest fix. *Moderate, and it makes debugging everything else worse.* MK5-S1 if it has room.
10. **`fs` breadth** (`mkdir_all`, `remove_all`, `walk`, metadata, temp files) cheap now that `List` exists; mostly an afternoon of Kelp, and the first three could ship today. *Low pain, high frequency.* MK5-S5.
11. **`env`** the shape is proven; the bodies are one extern each. *Low.* MK5-S5.
12. **`Option`/`Result`, views, threads** real, deferred, not blocking anything. *Low today, rising.* MK5-S6-M1/MK5-S7-M1 and open items 44/46.

Bluntly: **real** gaps are 1, 2, 3 and 6 (and 5 as a precondition for 6) without them Kelp cannot replace a shell script, which is the lowest bar there is. **Real but not blocking**: 8, 9, 10, 11. **Tolerable**: 12 the error-`datac` convention genuinely works, and the `for`-loop gap is ugly (`while` + index), not impossible. **Toy gaps left alone on purpose**: a big stdlib `json`, `regex`, `log`, `cli`, a test framework are all downstream of views/generics/collections and would each be rewritten once those land. Write them after MK5-S5, not before.

### Non-goals for the MK5 stages

- **TLS/`https` inside MK5-S4.** MK5-S4b-M1, because the dependency decision is the owner's (open item 37). Hand-rolled TLS is never in scope.
- **`async`/`await`, green threads, thread pools, `Select`-style combinators.** `async` needs function values, a state-machine transform and an executor (MK7-shaped work); OS threads are MK6-S1-M1's subject, and they are a *language* milestone, not a stdlib one. Note the ordering the owner set: MK6-S1-M1 (threads) precedes MK6-S2-M1 (concurrent downloads) but threads are the *third* right answer for downloading, behind poll multiplexing and behind nothing at all. See MK6-S1-M1 and MK6-S2-M1.
- **JSON, `regex`, `log`, `cli` parsing, a test framework, variadic `fmt`, `kelpc fmt`.** All downstream of MK5-S1–MK5-S5, and each would be rewritten once views/generics/collections land.
- **HTTP servers beyond a minimal one, HTTP/2, WebSockets, gzip/brotli, cookies, proxies.** The minimal server exists to test the client and to prove `poll`. Connection pooling is **no longer** on this list: it is MK5-S4, and so is keep-alive.
- **`Option`/`Result`.** The remaining language gap here; generics and tagged unions both shipped in MK8-S1-M1 (A-2, A-1), so what is left is `?`/`if let` and the stdlib migration (MK5-S6-M1).
- **Grapheme-aware Unicode, case folding, normalization.** Tables and algorithms out of proportion to the value here; the scope is codepoint-level (`str.char_at`, `str.byte_index`), ASCII-only case ops, documented (open item 48).
- **An arena/bump allocator, `defer`, custom allocators.** Arena without views is a copying allocator; `defer` needs syntax `syntax.md` does not have, and the drop plan already covers scope exit.
- **Third-party dependencies, a package registry, a separately versioned stdlib.** Decision 3 stands; TLS is the one place "system library" has to be distinguished from "third-party package" (open item 37).

### Distribution and implementation strategy

Stdlib as `.kp` sources shipped with the compiler (`std/fs.kp`, `std/math.kp`, ...), with a small intrinsic escape hatch for the actual syscalls. Dogfoods the language, keeps the API reviewable as Kelp code, and lets the capability wrapper live in the same file as the gated namespace.

```kelp
// std/fs.kp
extern fun fs_read_raw(path: &str).R: Bytes     // compiler-provided body

fun fs.read(path: &str).R: Bytes {
    return fs_read_raw(path)      // compiler inserts the allowlist check at this call site
}
```

Locked (decision 3), and **as of MK4-S1-M1 the whole sentence holds**: `extern fun` parses, bodies come from `externs.rs`, `kelpc/std/*.kp` ships inside the compiler binary, and the compiler-inserted allowlist check runs at every gated call site. The raw syscall lives only inside the gated module (`std/fs.kp`'s externs), the check lives at the boundary where a *user* module names the public function, and the allowlist is compiled in as constant data that no API can widen.

The rejected alternative: emit stdlib bodies directly from `codegen_c.rs` as C snippets. No new syntax, but the API becomes unreviewable non-Kelp code and every doc/example is divorced from the body.

Details:
- Stdlib version is pinned to the compiler version. No `[stdlib]` section in `coral.toml`, no registry, no third-party deps in v1.
- Externals are linked through `cc`; `math` needs `-lm`.
- Generated C for stdlib is cached per compiler build, not per user build.

### Naming and style rules for the stdlib

- Namespaces and functions `snake_case`. No `get_` prefix on getters (`fs.size`, not `fs.get_size`). `is_`/`has_` for predicates.
- Verb-first for effectful functions (`fs.read`, `process.run`); noun-first reads as data access.
- `try_` prefix is **not** used: the out-param family was rejected (decision 2). Recoverable functions return an error `datac` instead, and their doc comment names the datac.
- **The error-`datac` convention — used, then retired (MK5-S6-M1).** It said: `ok: bool` **first**; the success payload named for its type (`value` for a number, `text` for a `String`, `bytes` for a `Bytes`, `at` for an index or offset); a `message: String` when the failure is textual. That convention carried the stdlib from MK3 to MK5-S5, and it is what made the migration mechanical: every native that returned one now returns `Option` or `Result`, and each call site became an `if let`/`?` instead of an `ok` flag. The rule applied in the migration is the one `std/option`/`std/result` state: **a failure that carries no reason is an `Option`, one that carries a reason is a `Result`** — so `Line(ok, text)` became `Option<String>` (end of file is not a reason), `Match(ok, at)` became `Option<usize>` (ditto), and `ParseInt`/`ParseUint`/`ParseFloat(ok, value)` became `Result<T, ParseErr>` with `enum ParseErr { Empty Invalid }`, which is a reason two variants wide. No wrapper names survive: keeping them would have kept the old call shape compiling and hidden the point of the milestone.
- A **panic on a failed syscall** reports `errno` and `strerror` on a **second line** below the unchanged `panic: <msg>` prefix (`kelp_panic_errno`), so a golden assertion on the message still holds; a `getaddrinfo` failure reports `gai_strerror` instead, because it is a return code and not `errno`. Panics raised by argument validation, and the ambiguous short read/short write cases, carry no errno line.
- `_all` suffix for recursive variants (`fs.mkdir_all`, later `fs.remove_all`).
- Receiver form for the fluent cases (`s.trim()`, `p.parent()`), plain namespace call for the rest.
- Every panicking function documents `Panics:` in its doc comment.
- Every gated function documents the capability domain and the matched pattern form.
- No function in the stdlib takes a capability as a parameter. Grants are per-module and compile-time only.
- `read`/`write` are byte-level (`Bytes`); `read_text`/`write_text` are text-level (`String`).

### Language features the stdlib forces

| Feature | Needed by | Cost | Milestone |
| --- | --- | --- | --- |
| `enum` tagged unions + pattern `when` | `Option`/`Result` (A-3), the error-`datac` migration, any branch-on-shape code; a `datac` cannot express "one of these, with a tag" | Large: a declaration form, construction, a pattern form of `when`, exhaustiveness, a `tag` + `union` layout, and a per-variant drop switch shared with `datac` | **done in MK8-S1-M1 part 1** (`Ty::Struct` names an enum too, `EnumDef`/`VariantDef`, `tir::ExprKind::EnumLit`/`EnumTag`/`EnumField`, `codegen_c::write_enum`; generics for the payload are A-2, still open) |
| Builtin owned `Bytes` + drop | `fs`, allocation-returning `str`/`path` | Real: heap, drop insertion, move checking with runtime consequence | MK3-S1-M1 (done) |
| A capability check inserted at a gated call site | the headline feature | Medium: codegen emits the allowlist as constant data and wraps each gated call | MK4-S1-M1 (done) |
| Builtin owned `String` | `datac` fields, `str`/`env` text results | Small once `Bytes` exists | MK3-S1-M2 (done) |
| Builtin owning pointer `Box<T>` | recursive `datac`s (a tree, a list, a graph node), any owned value that needs one owner and a null state | Medium: a second monomorphized container, plus an emission order that lets a container precede the structs it points at | MK3-S2-M2 (done) `Ty::Box`, `infer::box_sig`, `codegen_c::write_box`/`write_containers` |
| Builtin owned sequence `List<T>` | `fs.list`, `str.split`, argv, anything returning *N* items | Medium: a monomorphized type, one C struct and function set per element type, and a drop that frees elements before the buffer | MK3-S2-M1 (done) `Ty::List`, `infer::list_sig`, `codegen_c::write_list`/`collect_list_elems` |
| `&str` return elision (borrow from first ref param) | the `&str`-returning half of `str`, `path` | Small, but unblocks a whole namespace | MK2-S2-M1 (done) `resolve_return` produces a `Ty::Ref`, and `borrowck.rs` enforces the rooting rule at both ends |
| `const NAME: T = expr` at top level, compile-time folded | `math.PI`, `math.E` | Small parser + const-fold in `infer.rs` | MK2-S2-M1 (done) |
| `extern` declarations with compiler-supplied bodies | stdlib written in Kelp | Small parser surface, new codegen path | MK3-S1-M2 (done) |
| Argument-type overloading, stdlib-only | `math.abs/min/max/clamp` across numeric types | Medium: overload resolution in `infer.rs`; kept stdlib-only to avoid user-facing complexity | MK2-S2-M1 (done) |
| `u8`, `u16`, `usize`, `isize` | byte buffers, indices, `Bytes.len` | Small; `usize`/`isize` needed regardless for indexing | MK3-S1-M1 (done) |
| Integer conversions `t.to_<T>()` | MK5-S1 (number formatting/parsing, `str.char_at`, `fmt.hex`/`bytes`), every byte-level algorithm | small: a generated `BUILTINS` block, an inline checked cast in `codegen_c.rs`; no syntax | MK5-S1-M2 (done) `builtin_type_name`, `integer_conversion`/`int_bounds` |
| Growable `Bytes`/`String` (`cap` + `push`/`append`/`reserve`) | MK5-S1, MK5-S4, MK5-S5; every program that builds text | compiler builtin: a `cap` field in `RUNTIME` (and the 3 extern bodies that build those structs field-by-field), ~8 new builtins | MK5-S1-M1 (done) `kelp_buf_reserve`, `cap` on both structs |
| `panic(msg)` / `assert(cond)` | MK5-S1; any program with a deliberate failure | compiler builtin: the C exists (`kelp_panic`), only the names were missing | MK5-S1-M1 (done) `kelp_assert` + two bare-name `BUILTINS` entries |
| `List#sort`/`reverse`/`slice`/`insert`/`remove` | MK5-S1, MK5-S5; deterministic output | compiler builtin, one comparator per element kind (monomorphization makes this nearly free) | small–medium MK5-S1 |
| `main(argc, argv)` + saved argv globals | MK5-S2-M1 (`process.args`) | compiler codegen: change the emitted signature, stash the two values, one extern to read them | **done in MK5-S2-M1** `process.arg_count`/`arg_at`, `process.args` in Kelp |
| Closure type `closure(T).R: U`, captures, and the widening from a function value | MK8-S1-M2 phase E/F (A-4) | language: parser/types/infer/borrowck/codegen — a `{ fn, env, drop }` fat pointer monomorphized per signature, one environment struct and drop per capturing literal, a generated thunk (or a shim, which allocates nothing) for the widening, and a capture read that is a place | large A-4's last piece; the environment's layout stays out of the type system, which is what keeps two closures of one signature the same C type |
| `Map<K, V>` | MK5-S5 (as `Map<V>`, `String` keys); a scalar key type in A-2 | compiler builtin generic the second one, ≈ MK3-S2-M1's cost (`Ty::Map`, `map_sig`, `write_map`, drop) | medium MK5-S5, extended in A-2 |
| `net` capability vocabulary (`host[:port]`, matcher, checked args) | MK5-S3, MK5-S4 | compiler: `stdlib.rs` (`CapCheck`), `resolve.rs::validate_net_pattern`, an emitted `kelp_cap_check_hostport`/`kelp_cap_check_host`/`kelp_cap_check_port` | **done in MK5-S3** `validate_net_pattern` + `kelp_host_glob`, and the `Vec<CapCheck>` redesign the sketch did not foresee (open item 32) |
| `poll` readiness | MK5-S3, MK5-S5 (single-threaded servers) | extern only | **done in MK5-S3** `Socket.wait_readable`/`wait_writable` reach `io`'s existing `poll` through a `File` over the socket's descriptor, so no new extern |
| errno + `strerror` in panic messages | MK5-S5 diagnostics | runtime + builtin (`kelp_panic_errno`) | small **done in MK5-S6-M1's cheap half** `kelp_panic_at`/`kelp_panic_errno`/`kelp_panic_gai` in `codegen_c`'s runtime, and the syscall call sites in `externs.rs` that clean up before panicking capture the code first (open item 47) |
| TLS for `https` | MK5-S4b-M1 | a dependency decision *then* externs (`-lssl`/`-lcrypto`, or the system's `libtls`) | large-ish: the first real link dependency open item 37 |
| A length-carrying borrowed view (`&[u8]`-shaped) | zero-copy parsing (MK5-S4/MK5-S5), `String.slice`, arena allocation; collapses open items 22 and 29 | language + *syntax*: a new type with pointer+length, allowed as a field/param/return; needs a spelling agreed with `syntax.md` §1/§13 | large open item 44 |
| `for` loop / iteration | every list walk in MK5-S1–MK5-S5 | shipped as MK8-S1-M3: parser desugar to `while`, with `syntax.md` §6.2 gained first (§10 before the renumber) open item 27 is resolved |
| Function values + move-taking parameters, per-thread panic policy, atomics/`Mutex`, a `Send`-shaped rule | MK6-S1-M1 (threads), MK6-S2-M1 | language: parser/types/borrowck/codegen + runtime | large MK5-S7-M1 records why threads wait for exactly these; the first two are shipped (MK8-S1-M2 phases A-F, closures included), and MK6-S1-M1's recommended narrow shape — function *references* rather than closures — is now a smaller milestone than it read: a closure is a value a thread could own, and what remains is the panic policy, the per-thread frame story, and `Send` |
| String escapes that mean what they say (`\r`, and an error for anything unknown) | MK5-S4 (an HTTP request is CRLF-framed); any program writing a protocol | compiler: one arm in the lexer's escape match, and a catch-all that returns a diagnostic instead of the bare character | **done in MK5-S4** the fallback used to make `"\q"` mean `"q"`, so `"\r"` meant `"r"` and every request was malformed with no diagnostic |
| A C string-literal emitter that escapes every control byte | MK5-S4's `\r`; any literal with a byte below 0x20 | compiler: `codegen_c::c_string_literal` (+ `printf_text` for template text), CR as `\r` and the rest as three-digit octal | **done in MK5-S4** a raw CR inside a generated C string literal is not a string literal |
| A numeric `coral.toml` key spliced into a shipped stdlib source (`http_pool`) | MK5-S4's pool ceiling | compiler: a manifest parser that accepts a bare integer, a `__KELP_HTTP_POOL__` substitution in `stdlib::source_configured`, one new field on `Manifest`/`Loader` | small the first *config-driven* stdlib constant; also the prerequisite for `coral.toml [caps]` (open item 18) |
| `Transfer-Encoding: chunked` decoding | MK5-S4 (keep-alive makes it mandatory) | stdlib only: a hex size-line decoder in Kelp over `str.byte`/`str.parse_u64`, plus exact-length reads | small, and it replaces a panic |
| A connection pool with ownership-checked leases | MK5-S4's payoff | stdlib only: `Pool`/`Lease` as plain `datac`s, moves in and out, no lock, no refcount | small the language already has everything it needs (this is the point of doing it in Kelp) |
| Multi-fd `poll` + a would-block signal from `send`/`recv` | MK5-S4c (the no-language-work route to concurrent downloads) | stdlib only: `Socket.set_nonblocking`, a `would_block`-carrying send/recv result, and `poll(fds[])` instead of one fd per call (open item 41) | small, and it is the honest answer for I/O-bound concurrency |
| Type parameters (a function over any `List<T>`) | `process.run(argv)`, `env.args`, generic containers, `List` *of* `List` | Large the whole generics dimension | MK3-S2-M2's neighbourhood, post-v1 |
| `Option`/`Result` + generics | replaces error `datac`s | Large | post-v1 |

### Testing the stdlib

- Fixture suite per module, same compile-run-diff-stdout harness as the language fixtures. **As shipped** they live in `kelpc/tests/fixtures/` (single-file: `stdlib_mem`, `stdlib_str`, `stdlib_path`, `stdlib_math`, `stdlib_list`, MK5-S1's `stdlib_build`/`stdlib_parse`/`stdlib_fmt`/`stdlib_sort`/`list_slice_insert`, MK5-S2-M1's `io_stdio`/`io_streams`/`process_spawn`, and MK5-S3's `net_loopback`/`net_poll`/`net_resolve`/`net_timeout`) and `kelpc/tests/projects/` (anything that needs a *grant*: the MK4-S1-M1 `caps_*` projects including `caps_list`, MK5-S2-M1's `process_run`/`process_argv_only`/`process_denied`, and MK5-S3's `caps_net`/`caps_net_denied`/`caps_net_resolve_denied`/`caps_bad_net_pattern`), because the harness takes one path and a granted module needs a project with an importer. `tests/stdlib/` still has not earned its own directory. The stdlib fixtures that complete normally are in the AddressSanitizer pass, which is what makes "the stdlib's allocations are freed" a test result rather than a claim; a fixture that *panics on purpose* is not, because it is expected to leave its buffers behind.
- A stdlib module is imported exactly like a project module (`impl "std/mem"`), so `std/str` importing `std/mem`, and `std/path` importing both, are the ordinary `impl` rules no special case in the loader beyond where the text comes from.
- Capability tests are the ones that matter, and **as shipped (MK4-S1-M1, extended in MK5-S2-M1 and MK5-S3)** every one of them exists: a granted path reads a file (`caps_granted`); an ungranted reference is a resolution error (`caps_ungranted`); an in-grant path writes, renames, sizes and removes (`caps_write`); an out-of-grant path panics at runtime (`caps_denied`); a symlink inside the grant pointing outside it is denied after resolution (`caps_symlink`, and MK5-S2-M1's `process` grant exercises the same rule on a *program* path); a narrower grant cannot be over-delegated to a sub-import (`caps_escalation`); a `..` pattern is rejected at the `impl` line (`caps_bad_pattern`), as is a `net` pattern that is not a host (`caps_bad_net_pattern`); a granted module runs a program inside its grant (`process_run`) and is denied one outside it (`process_denied`, asserted through the module name *and* the resolved path, per the determinism rule below); an **empty** grant admits the module's own-process functions while authorizing nothing executable (`process_argv_only`); the three `net` checks each have a negative case the host as written (`caps_net_denied`: a grant for `127.0.0.1`, a call to `localhost`), the no-port host form (`caps_net_resolve_denied`), and the port-only form, which `caps_net`'s own grant exercises positively since nothing can call `listen` outside its grant without a second binary; and the entry module's `*` is pinned by `net_loopback`, which has no `caps(...)` clause at all; MK5-S2-M2 added the two the link primitive made testable, a link created inside the grant (`caps_link`) and one whose target is outside it (`caps_link_denied`, denied before `symlink(2)` runs), plus `process_in`, which pins that a *working directory* is not a grant question (`/tmp` as the cwd under a program-only grant). Still missing: a table of *which* of `fs.rename`'s two paths is checked the code checks both by construction (`checked_args` returns every `str` argument for `fs`), but nothing exercises the negative case with only one path granted.
- Glob table-driven test: patterns × paths × expected verdict, as data, so the matcher is tested without touching a filesystem.
- Golden files for every `Panics:` message, so error text does not drift.
- **Harness additions the MK5 stages need, in order of cost**: (1) ~~`assert_fixture_runs_with_stdin`~~ **shipped in MK5-S2-M1** (`stdin(Stdio::piped())`, plus a second form taking a distinct binary name, because one fixture now has two stdin cases), along with `assert_project_runs_with_args` for the argv test; (2) reuse of `assert_project_denied_at_runtime` for `process`/`net`/`env` denials **used by MK5-S2-M1** for the `process` grant denial, and the helper needed no change; (3) **a loopback-server helper needed in MK5-S4, reversing this bullet's earlier claim.** MK5-S3's fixtures could be client and server in one process because `net` calls do not block on each other; an HTTP client blocks inside `send` until a response arrives, so a `Pool` cannot be exercised single-process. The helper binds `127.0.0.1:0` in Rust, passes the port to the compiled program through argv, serves canned byte responses on the accepted connection, and can assert how many connections were accepted which is how "the pool reused the socket" becomes a direct assertion instead of a timing inference; (4) golden panic text as `needle` assertions (already the shape of `assert_fixture_panics`), done as messages are written rather than later.
- **Determinism rules per domain**, so no test flakes: `str`/`fmt`/`List` pure fixtures with exact stdout and ASAN for the allocating ones, never locale-dependent float text; `io` harness-fed stdin, including a last line with no trailing newline, never the real terminal; `process` `/bin/sh` with argv, never `/bin/echo`/`/usr/bin/*` (absent on NixOS, verified), with the no-shell argv case done as `sh -c 'printf %s-%s "$1" "$2"' sh one two` instead of a helper binary the harness would have to build; `net`/`http` one-process loopback, `listen(0)` + `Listener.port()`, never a fixed port, never the internet, the redirect denial asserted through the module name or granted pattern (**not** the path as written: a denial prints the resolved path MK5-S3 added the same rule for hosts, so a denial is asserted on the module name or the granted pattern, and `caps_net_denied` asserts `127.0.0.1:*` rather than `localhost:1`); DNS only through `127.0.0.1` and `localhost`, never a real name; `fs` a scratch directory created and removed by the harness and by the program; `env` assert non-empty on a granted name and deny another, never mutate what the harness needs; `Map` assert insertion order of `keys()`, never hash order.

## Key implementation notes

- **Error reporting**: every AST node carries a source span from day one retrofitting spans later is much more painful than carrying them from the lexer onward, and `7932cab` did exactly that threading into `tir`. `%*alias*%` expansion keeps both the definition span and the use span so alias-induced type errors stay debuggable.
- **Move/borrow checking as a separate pass, not entangled with type inference**: `infer.rs` produces a typed AST; `borrowck.rs` walks the typed AST separately. Keeps each pass comprehensible and testable in isolation, which matters a lot once borrow-checker bugs need debugging.
- **C codegen strategy**: generate readable, `#line`-annotated C (mapping generated lines back to `.kp` source lines) so that `gdb`/`printf`-debugging the *generated* C during compiler development is tractable, and so a future "debug the .kp program itself" story is easier to bolt on.
- **Signed overflow must be defined**, since C leaves it UB: build with `-fwrapv` so `i32`/`i64` wrap deterministically, and expose `math.checked_*` / `math.saturating_*` for callers who want to know.
- **`mut` does two jobs** (`var` binding vs exclusive borrow) and error messages must distinguish them clearly, since a wrong guess produces a confusing diagnostic.
- **No generics/traits in v1**: keeps type inference and borrow checking tractable; monomorphization is a large added dimension best tackled once the non-generic core works end-to-end.

## Verification

- `cargo test` in `kelpc/` runs 65 unit tests plus the end-to-end suite (`tests/compile_and_run.rs`, 144 tests as of MK5-S3): for each `examples/*.kp` / `tests/fixtures/*.kp`, either (a) compile, run the resulting binary, and diff stdout against an expected value, (b) compile, run, and assert the program panics with a specific message, or (c) compile and assert a specific compiler error is produced (for move/borrow violation fixtures). A multi-file project is built the same way, through its `main.kp`.
- Manual smoke test after each milestone: `cargo run -p kelpc -- build examples/hello.kp && ./hello` (and similarly for `fib.kp`, etc.) to confirm the CLI path works, not just the test harness.
- After MK1-S4-M1, deliberately write a handful of "should fail" `.kp` snippets (double move, two simultaneous exclusive borrows, use of a borrow after its owner's scope ends) and confirm the compiler rejects each with a sensible error this is the actual proof that the borrow checker works, not just that "well-behaved" programs compile. MK1-S3-M1's fixtures (`use_after_move.kp`, `double_move_in_loop.kp`) are the first two entries in that set. MK3-S1-M2 added four more ("an `extern` needs a compiler body", "an `extern` takes no receiver", "comparing owned values has no meaning", "an owned value in statement position would never be freed").
- **Heap fixtures are run under AddressSanitizer** (MK3-S1-M1, extended in MK3-S1-M2, MK3-S2-M1, MK5-S2-M1 and MK5-S3): `heap_fixtures_are_double_free_and_leak_clean` builds every fixture that owns memory the `Bytes` ones, the `String` ones, the owned-`pd` ones, the stdlib ones, the `List` ones, MK3-S2-M2's `box_basic`/`box_recursive`/`recursive_datac`/`box_field_take`, MK5-S2-M1's `io_stdio`/`io_streams`/`process_spawn`, and MK5-S3's `net_loopback`/`net_poll`/`net_resolve` rebuilds the generated C with `-fsanitize=address -O1`, and runs it. (A fixture that *panics on purpose* is excluded: `net_timeout` is expected to leave its buffers behind, so instrumenting it would assert the opposite of what it tests.) A double free, a use-after-free, or a leak fails the suite, which is what turns "the `atexit` drop works" and "a struct drops its fields" into test results rather than claims. This is the only test that depends on a toolchain feature, so it skips itself if `cc` has no AddressSanitizer which means the memory-safety claim is verified on every machine that can verify it, and the rest still pass. Note what the sanitizer does *not* cover: the MK3-S1-M2 field-assignment leak (open item 15) is real and would be caught by ASAN if a fixture exercised it, so no fixture does.
- **Two benchmarks under `bench/`**, both with a recorded `results.txt`:
  `write_20gb/` writes 20 GiB in four languages (Rust, Kelp, C++, Python) and
  shows the workload is entirely device-bound all four within 2% at
  ~1650-1700 MiB/s and `speed/` runs four CPU-bound workloads with
  cross-language checksums, which is where a language difference is actually
  visible (Kelp, C++ and Rust level to within ~1%; Python 27-87x slower
  depending on the workload). The `speed/` README records the method and the
  caveats, including the one worth remembering: an unconstrained naive `fib`
  is rewritten into a loop by LLVM at 0.5 ns/op, so the call-cost workload
  needs a side-effecting counter to measure anything.
- Capability tests are compile-time-first: an ungranted `fs.*`/`net.*` reference must fail at resolution, never reach a runtime check. Runtime-denial tests cover the dynamic cases (out-of-grant path, `..` escape, symlink escape, `process` program path, `net` host as written, `net.resolve`'s host, and `fs.rename` with only one path granted the last one still unwritten).

## Divergence from Kotlin (kept intentionally)

- No exceptions. Error handling stays Rust-shaped (panic + error `datac` for now).
- No GC. Ownership and borrows remain in the model.
- No collections in v1 beyond the builtin sequence: `List<T>` shipped as MK3-S2-M1, and `syntax.md` still says "no collections in v1" (open item 27 that sentence needs `syntax.md`'s update, not this one's).
- No `self`, no `impl` method blocks receivers instead.
- References do not appear in `datac` field types in v1.
- `val`/`var` and the Kotlin look are surface only; moves and borrows are still visible in the type system.
