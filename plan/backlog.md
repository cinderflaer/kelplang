# The "Kelp" backlog

Everything the plan has named and not scheduled. A backlog, not a milestone list: no numbers, no order, no commitment. `roadmap.md` is the record and the plan; this file is where the plan puts what it is not yet willing to commit to.

- **The `A`-`F` item lists** the roadmap used to carry inline: the language surface a self-hosted `kelpc` needs (A), the substrate deep-systems work needs (B), the capability-model gaps (C), the verification and semantics duty (D), a suggested order with the plan's cost calibration (E), and dependency and precedence notes (F). MK8, MK9, MK10 and MK11 promote parts of A-D to Marks; the items keep their `A-n`/`B-n`/`C-n`/`D-n` labels here as the detail.
- **Coral**, the project-layer build tool and package manager, if roadmap open item 72 is ever answered yes.
- **Regulated and financial domains**, if roadmap open item 71 is ever answered yes.

## Far future additions

A backlog, not a milestone list. No numbers, no order, and no commitment: the milestone records in `done.md` stay the authority for what has shipped, and `roadmap.md`'s Mark tables are the authority for what is next, and everything here waits until something needs it. It exists for two reasons, both of them goals the plan states and neither of them reachable with v1 plus the MK5 stages:

- **Write `kelpc` itself in Kelp.** The compiler is 16 files whose central types are enums walked by pattern matching, over hash maps, with `Result` at every fallible edge, closures for traversal, and `Vec`/`HashMap`/`Box` throughout. Almost none of that is expressible today see block A.
- **Deep systems work** (promoted to MK9; block B below is kept as the item detail). Kernels, drivers, firmware, allocators, hypervisors, embedded. Measured against the plan's own framing in "Context" ("a new statically-typed, systems-level programming language ... aiming for Rust/C/C++-class runtime performance"), v1 earns the *performance* half and the *memory model* half; it does not earn the target-list half. Rust reaches kernels only through the five things excluded here `unsafe`, `no_std`, `repr(C)`, atomics, and user FFI see block B.

The three structural blockers and the three reversals are stated in `roadmap.md`'s **Far future additions**; every block below is read against them. Block C exists for the runtime-authority gap in particular.

Items are labelled `A-n`/`B-n`/`C-n`/`D-n` to keep them out of the open-item numbering. Nothing here is a task list yet; several items are reversals of deliberate decisions, and block E says which ones.

### A. Language surface required to write `kelpc` in Kelp

*A-1 to A-13 are promoted to MK8-S1 and MK8-S2; A-14 to A-16 and C-1/C-2 to MK8-S3; A-17 to MK8-S4 through MK8-S6. The items are kept here as the detail.*

- **A-1. Tagged unions and exhaustive pattern matching.** **Part 1 shipped as MK8-S1-M1 (part 1)**, recorded in `done.md`: the spelling is `enum` with payload-carrying variants, chosen over a `sum` keyword and over variants on `datac` because Rust's meaning of `enum` is exactly the meaning wanted here, while `datac` stays a record with one layout and one set of rules. What landed: `enum Name { Variant(val f: T) ... }` (variants newline-separated, the payload list shared with `datac`), construction as `Shape.Circle(r = 1.0)` / `Shape.Empty`, the `when <place> { Variant(bind) -> value else -> value }` pattern form (the boolean `when` has no scrutinee, so the two forms never collide), exhaustiveness checking that names a missing variant, payload binding by copy for `Copy` and by shared borrow otherwise, a C `tag` + `union` layout with a per-variant drop switch, enums as container elements, and one merged `datac`/`enum` declaration order. `syntax.md` §2.3/§3/§6.3/§9.2/§17 carry the syntax. What is left of A-1: nested/literal patterns and guards, which nothing needs yet. `Option<T>`/`Result<T, E>` still wait on A-2 (generics for the payload).
- **A-2. User-declarable generics.** **Shipped as MK8-S1-M1 part 2**, and open item 34 is resolved. Type parameters on `datac`/`enum` declarations, every use site monomorphizing into an ordinary named type; generic *functions* (`fun first<T>(xs: &List<T>).R: T`) and receiver methods on a generic type (`fun Pair<A, B>.first().R: A`); type arguments inferred from the arguments and from the expected type, with an explicit spelling at the expression when inference has nothing to go on (`Pair<i32, String>(...)`); inference through a nested generic constructor; and the builtin `Map` taking a key type (`Map<K, V>`, with `Map<V>` kept as the `String`-key shorthand). The key set is closed to `String`, the integers and `bool`, because the compiler generates the key's hash and its equality — a key of the user's own type would need a trait (A-12) or a comparator function (A-4), neither of which exists, and that is the one piece of "a `Map` with a real key type" that generics alone could not reach. `Box<T>`/`List<T>`/`Map<K, V>` remain compiler builtins.
- **A-3. `Option<T>`/`Result<T, E>` with propagation.** No `?`, no `if let`, no combinators, no null. The error-`datac` convention (stdlib decision 2) carries the stdlib and does not scale to a compiler: nested fallible calls become nested `if`s and every failure shape wants its own datac. This is MK5-S6-M1's decision point, and it is compiler ergonomics as much as stdlib surface. `?`/`if let` are syntax, so `syntax.md` gains them first.
- **A-4. Function values, closures, and move-taking parameters.** **Shipped as MK8-S1-M2 phases A-F, which close A-4.** `name: move T` takes ownership of its argument (open item 1's spelling), a declared `fun` is a value of type `fun(T).R: U`, a `fun` literal is written where an expression is expected, a literal that names a binding from the enclosing scope is a **closure** typed `closure(T).R: U` (a function value plus the heap environment its captures were moved into, dropped where the closure dies), a plain function value is widened into a closure where one is expected (through a generated shim when it names a function, so nothing is allocated), and `std/option`/`std/result`/`std/list` take callbacks that may capture. Every callback's payload is a borrow, so a call never moves an element or a payload out. **Nothing in A-4 is left**: the two pieces an earlier record listed as remaining — a `sort_by`-shaped comparator over an owned element type, and the helpers that hand an owned payload out by value (`option.unwrap` on an `Option<String>`, `result.map` over a `Result<T, MyErr>`, `list.filter` over a `List<String>`) — need **A-11's partial moves**, not closures, and are recorded there and in the modules' headers. A compiler is pass-structured `walk(node, |n| ...)`, comparators, diagnostic sinks, per-node callbacks — all of that is expressible now. MK5-S7-M1's two prerequisites (function values and a move-taking parameter) are met, and open item 60's (a)/(b) now cover closures as well, so MK6-S1-M1's language surface is no longer the blocker.
- **A-5. Iteration: a `for` loop, then possibly a protocol.** Open items 27, 36a and 46. **The cheap half shipped as MK8-S1-M3**: `for x in list { }` is a parser desugar to an index and a `while`, so a list walk no longer reads like C (the element binding is `List.get`'s — a copy for a `Copy` element, a shared borrow for an owned one). The reusable traversal (`iter`/`next`) is the expensive version and is **not** scheduled: it is what an abstraction over heterogeneous traversal would need, and nothing has asked for one. A-5 is therefore closed at the cheap form; the protocol stays an open question with no milestone.
- **A-6. A keyed container with non-`String` keys, and `Set`** (open item 41). A compiler's tables are keyed by interned symbol id, type id, node id, and `(module, name)` stringifying keys to reuse `Map<V>` is the wrong shape at the wrong cost, and `Map<bool>`-as-`Set` is a recorded stopgap rather than a substitute at compiler scale.
- **A-7. Shared ownership of immutable data, and allocator control.** No `Rc`-shape, no arena/region, and no custom allocator (all explicit non-goals today), while a `datac` field must be owned so interned strings, type tables and symbol tables can only be shared as *indices* into a `List`. Indexing is the standard compiler design and will work; the language gives no help, and with no allocator hook the allocation-heavy phase of compilation stays slow. An arena for AST nodes and a shared-immutable table type are the two things a compiler actually reaches for.
- **A-8. A length-carrying borrowed view, and borrows in fields** (open items 22, 29, 44). A node cannot hold a `&str` slice of the source (`syntax.md` §1/§13), so every token and identifier copies its text; a `View` (or `&[u8]`) collapses those copies in the lexer, the parser and every `split`-shaped routine at once, and it also lets `path.parent`/`str.trim` return views instead of allocations and `List<String>.get(i)` drop its `.as_str()` chain.
- **A-9. The bit surface.** v1 has *no bitwise operators at all* (the plan's own note: `str.char_at` does UTF-8 masks by subtraction), no char literal (open item 53), and no sized/hex/binary literal story. A lexer, a codegen and any byte-format work need shifts, masks and testable bits and `&` is already taken by borrows, so the spelling is itself a decision.
- **A-10. Loosened container element rules** (open item 28). No `List<Box<T>>`, no `List<List<T>>`, no borrow or view elements, and a `datac` may only name earlier `datac`s (open item 12) a compiler's children lists are exactly the forbidden shapes, so today each one needs a wrapper `datac`.
- **A-11. Borrow-checker expressiveness.** Deliberately conservative, which is over-strict rather than unsound and a compiler is where the strictness lands hardest: no provenance, so a pointer local cannot be reused as a cursor (item 33); lexical lifetimes, so a bound borrow lives to the end of its block (item 30, workaround is an extra `{ }`); no NLL (a stretch goal only); no partial moves (the `Line` text case); no field-granular drop tracking and no flow-sensitive branch merge, which are also what the three deliberate leaks wait on (item 15). Eventually explicit lifetime syntax (`'a`), so a borrow can be a field and a type can say what it is tied to.
- **A-12. Traits/interfaces and dynamic dispatch.** Substitutable by enums for dispatch *if* A-1 exists, but not for destructors (a user `Drop`) and not for a pass abstraction ("a stage is a thing with a `run`").
- **A-13. Visibility/privacy.** Every top-level item of a module is reachable as `alias.Item` (MK2-S1-M1) and nothing in the plan adds `pub`. A ~100k-line self-hosted compiler with a fully public surface is a structural problem, not a style one.
- **A-14. Compilation at scale.** Separate compilation, incremental/query-based compilation, a cross-build cache, and a reserved internal-library location (today `impl` is local-files-only and `std/...` is the *compiler's* namespace). The measured baseline makes this a requirement rather than a nicety: ~95% of `kelpc build` is `cc`'s fixed overhead and Kelp's own pipeline is 3–4 ms (MK7's table), while a self-hosted `kelpc` is roughly an order of magnitude more Kelp source so MK7's own backend is a precondition for a tolerable self-hosted toolchain, not just a performance project.
- **A-15. Tooling.** A test framework (an explicit non-goal today), `kelpc fmt` (unscheduled, item 13), a lint pass with the configurable naming option (item 23), and a documentation generator with doc-comment extraction.
- **A-16. Diagnostics and debuggability.** `#line` was never emitted (item 20), so generated code carries no source mapping; a real backend needs DWARF and a location story. Also an own tracing/profiling hook set, so a self-hosted compiler can be profiled without the C toolchain's help.
- **A-17. A bootstrap milestone, which this plan does not have at all.** Build `kelpc` in Kelp, compare stage0 (Rust `kelpc`) against stage1 (Kelp `kelpc`) on the whole fixture suite, then check the fixpoint (stage1 builds stage2, behaviourally or byte-identical). Without that test, "the compiler is written in Kelp" is a claim rather than a result the same standard the verification section already holds memory safety to.

### B. Substrate required for deep systems

*Promoted to MK9 (see `roadmap.md`'s MK9 tables). The items are kept here as the detail.*

What separates a systems-*dialect* application language from a language that can host a kernel, a driver, a firmware image, an allocator or a runtime.

- **B-1. User FFI.** `extern fun` exists but is compiler-only: user code gets "only compiler-shipped stdlib modules may declare 'extern fun'" (MK3-S1-M2). Needs user-declarable externs, `extern "C"` blocks, variadic calls (MK7 stage 4 already flags the x86-64 SysV `AL`-register detail for `printf`), function-pointer/callback types, symbol import/export and linkage attributes, link flags and library paths, and system-library declarations (TLS, open item 37, is the first instance). Without this Kelp cannot call libc or any OS API no device library, no database client, no GUI toolkit.
- **B-2. ABI and layout control.** No `repr(C)`, no alignment, no packing, no unions, no bitfields, no explicit size/offset, no enum discriminants, no layout intrinsics (`size_of`/`align_of`/`offset_of`). So no MMIO register block, no wire or on-disk format, no packet header, no kernel struct, no C-interop struct the artefacts deep systems work is made of. MK7 stage 3 lists `datac` layout, calling convention, and `extern "C"` calls as *backend* work for the compiler's own ~20 externs; this item is the user-facing version of the same thing.
- **B-3. `unsafe`, raw pointers, and the memory surface.** No raw pointers, no pointer arithmetic, no casts/`transmute`, no `volatile`, no address-space or provenance control, no uninitialised allocation. Needed for MMIO, DMA, intrusive lists, allocators, page tables, kernel objects and lock-free structures. The plan's own evidence that Kelp cannot express this today is that `Bytes`/`String`'s runtime is a fixed C preamble in `codegen_c.rs` and the stdlib's primitives are 251 lines of C in `externs.rs`. With A-11's checker still conservative and no `unsafe`, there is also no escape valve the single structural reason a kernel is out of reach.
- **B-4. Atomics, ordering, and a published memory model.** No atomics, no fences, no ordering annotations, no `Send`/`Sync`-shaped discipline, and no definition of a data race because there are no threads. A systems language needs the model *before* the threads: what a `&mut` guarantees, what concurrent access to one location does, what a capability-protected region may be shared across, and what the compiler may assume. This is also the missing half of block D.
- **B-5. Threads, and then a concurrency story.** MK5-S7-M1's conclusion stands: no threads until function values *and* move-taking parameters exist (A-4), plus a rule for `&mut` crossing a thread boundary, owned data handed to a thread, per-thread stacks and TLS, a per-thread frame/cleanup story (drops are keyed to lexical blocks in one function), and what a panic in one thread does to the others. `async`/`await` is a further step and needs a state-machine transform in codegen. The intermediate that MK5-S7-M1 already ships is multi-process plus `poll`.
- **B-6. Freestanding / no-runtime mode.** Kelp assumes an ambient runtime: libc stdio, `malloc`/`realloc`/`free` from the preamble, `atexit` for owned `pd` drops, `main(argc, argv)`, and `kelp_panic` → `exit(1)`. Needs a `no_std`-shaped mode with no libc, no stdio and no implicit heap, a user-supplied panic handler and entry point, raw syscall access, target selection, linker-script/section control, static and shared library output, cross-compilation, and C-free static-ELF output. This is the requirement for kernel, bootloader, firmware, hypervisor, RTOS and embedded targets and MK7 stage 5's "static ELF with raw syscalls and our own runtime" is the same item approached from the toolchain side.
- **B-7. Allocator control.** A custom global allocator, per-arena/per-subsystem allocators, alignment and huge-page requests, DMA-safe allocation, and *fallible* allocation (`try_alloc`) today allocation failure panics and `mem.alloc(n)` is a fixed zero-filled `Bytes`. Arena/bump allocation and custom allocators are explicit non-goals in the current plan, so this item reverses that non-goal for the same reason A-7 wants an arena for the compiler.
- **B-8. Destructors and deterministic cleanup.** No user `Drop` (needs A-12), no `defer`, drops only at lexical scope exit and `atexit`, and a panic path that calls `exit(1)`. Systems code needs release points it can point at, a `Drop`-equivalent for resource handles, and a cleanup story for the panic/abort path which also means rethinking "a panic takes the process".
- **B-9. Hardware and ISA surface.** SIMD vectors, intrinsics, CPU-feature detection and dispatch, inline assembly, naked/interrupt-handler functions, calling-convention control (syscall, interrupt, fastcall), stack alignment and prologue control, atomics-as-instructions (with B-4), and cache/prefetch hints. Without these there is no lock-free primitive, no context switch, no device ISR and no vectorised kernel.
- **B-10. Data-format primitives.** Endianness conversion intrinsics, unaligned load/store, and byte-order-aware reads and writes, so a wire or on-disk format is expressible without hand-rolled shifting (which A-9 is also needed for).

### C. Capability-model gaps (this language's headline, extended)

*C-1 is promoted to MK10-S2, C-2 to MK8-S3, and C-3/C-4 to MK10-S1 and MK10-S3. The items are kept here as the detail.*

The capability system is the differentiator, and it is currently a compile-time constant allowlist plus an unrestricted entry module. Two consequences matter for anything beyond one sandboxed program.

- **C-1. Runtime-derived authority.** A real `kelpc` takes its authority from runtime data: source paths on `argv`, the project root, `PATH`/`CC`, a temp directory, an arbitrary `cc` binary. Kelp cannot express "this authority came from my input": grants are compiled-in patterns, no module ever holds a capability *value* it could pass or attenuate, and there is no runtime delegation. Needs capability values/handles as a first-class (or explicitly restricted) thing plus a rule for deriving a pattern from runtime input safely otherwise `main` stays unrestricted and the headline feature is off for the compiler, which is the state today.
- **C-2. Restricting the entry module** (open item 18): `coral.toml [caps]`, so `main` can hold less than everything. The capability fixtures already work around this by putting their interesting module behind one level of delegation.
- **C-3. Device and kernel vocabulary.** Grants for device nodes, MMIO regions, IRQ lines, kernel object handles, memory regions/pages, and file descriptors *as capabilities* rather than as names the object-capability reading the model is already shaped for (seL4/Fuchsia-like), and the direction that would make capabilities a systems feature rather than a sandbox one.
- **C-4. Attenuation and per-request grants.** Handing a narrower grant to a plugin or worker at runtime (the multi-process story MK5-S7-M1 ships), revoking one, and bounding a grant's lifetime and holder count.

### D. Verification and semantics duty

*Promoted to MK11. The items are kept here as the detail.*

Cross-cutting, and the reason the current claims cannot simply be extended to unsafe or concurrent code: every memory-safety and capability claim in the verification section is verified *through clang* (ASAN, `-fwrapv`). Once there is `unsafe`, raw pointers, atomics, or an own backend, the obligation transfers to Kelp, and the argument has to be about Kelp's semantics rather than C's.

- **D-1. A formal operational semantics, and an aliasing model for `&mut`** a Stacked-Borrows/Tree-Borrows equivalent, or a documented decision not to have one. The borrow checker's rules are implemented across `infer.rs`/`borrowck.rs`; the model they implement is not written down anywhere.
- **D-2. A UB catalogue.** Defined behaviour for overflow (currently `-fwrapv` from the C toolchain), uninitialised memory, out-of-bounds, provenance, the FFI boundary, and data races (once B-4 exists). "No UB" cannot be claimed against an undefined model.
- **D-3. A Miri-equivalent dynamic checker** (or a proving argument) for the move/borrow/aliasing rules, so soundness is a test result rather than a reading of the checker's source.
- **D-4. Conformance, differential testing and fuzzing.** A grammar/conformance corpus, differential runs of two backends over the same fixtures (MK7 stage 1's `--backend=` flag is the seed of it), and fuzzing of the lexer, parser and checker against the semantics.
- **D-5. Own sanitizers and debug info.** DWARF emission, source mapping (A-16), and a sanitizer story that does not depend on clang.

### E. Suggested order, and cost

*Block E calls A-7/B-7 (allocators), B-6 (freestanding) and B-4 (concurrency model) decisions to make rather than tasks to schedule. Those three are MK9-S5, MK9-S4 and MK9-S3 the decision is now made inside a tracked Mark.*

Nothing here is scheduled. If it ever is, the cheapest useful sequencing is:

1. A-1 to A-5 and A-11 unions/patterns, generics, `Option`/`Result`, function values, iteration, and checker expressiveness. Without them a self-hosted front end is not writable at all, and A-1 is the largest single language change since borrow checking. (A-1 to A-5 have shipped; what is left of this step is A-11.)
2. A-6 to A-13 containers, shared data/arenas, views, bits, element rules, traits, privacy. The ergonomics that make a large body of Kelp code tractable.
3. A-14 to A-17 with C-1 and C-2 scale, tooling, bootstrap, capability derivation. The difference between "self-hosted in principle" and "self-hosted and usable".
4. B-1 and B-2 FFI and layout. The first *systems* unlocks, and partly free from the C backend if only they were exposed.
5. B-3 to B-10 unsafe, atomics/model, threads, freestanding, allocators, destructors, ISA. Each one a subsystem; together, the difference between a systems-dialect language and a systems language.
6. Block D runs alongside from the first `unsafe`, and B-4 (the memory model) is a precondition for honest claims about B-3, B-5 and B-9.

For calibration: A-1, A-2, A-4, B-3, B-6 and B-9 are each larger than the entire MK5-S1–MK5-S7 stdlib program, and A-14 compounded with MK7 is a project of its own. Three items are reversals of deliberate decisions rather than extensions A-7 and B-7 reverse "no custom allocators, no arena", B-6 reverses "assume an ambient libc runtime", and B-4 creates the concurrency model v1 avoids on purpose. Those should be treated as decisions to make, not tasks to schedule.

### F. Dependency and precedence notes

- **The precedence rule still applies: `syntax.md` wins on syntax and style.** Any syntax these blocks need a sum type, `unsafe`, `repr`, `'a`, `View`, `defer`, `?`, `if let`, `trait` must be added to `syntax.md` first, because that file is the authority on syntax and this one is not. (`for` was on this list and is the one entry that has since shipped, as MK8-S1-M3, with `syntax.md` §6.2 edited first.)
- **Open items these blocks close or supersede**: 4 (`when` value patterns), 11 (line continuation, if syntax work starts), 12 (a datac naming only earlier datacs), 13 and 23 (`kelpc fmt`/lint), 15 (the three deliberate leaks, via field-granular drop plus a flow-sensitive merge), 18 (`main` unrestricted), 20 (`#line`/debug), 28 (element rules), 29, 30 and 33 (borrow lifetimes, provenance, bound-borrow scoping), 34 (generics), 36c (`Option`/`Result`), 37 (TLS, as the first FFI/link case), 41 (`Map` keys, `Set`), 44 (a length-carrying view), 53 (`char`). Three that were on this list — 27, 36a and 46 (`for`) — closed early, as **MK8-S1-M3**: the cheap loop shipped as a parser desugar, and only the `iter`/`next` protocol is still a question.
- **The structural blockers, restated**: see `roadmap.md`'s **Far future additions** for the three, and why the first two are also the reason A-2 sits on the critical path.
- **What is not in this backlog, on purpose**: GC or exceptions (both permanent divergences, `syntax.md` §13), and a REPL/LSP/IDE story (unscoped in this plan entirely, and downstream of A-1 to A-17). A package registry and third-party dependencies were on that list; they are now the Coral design's decision (roadmap open item 72) rather than a settled non-goal, with path and git dependencies paying off whether or not an index ever exists.

## Coral: the build tool and package manager (roadmap open item 72)

*The `MK13-Sn-Mn` identifiers below are the ones this design carried when it was a Mark. They are kept so the detail stays addressable. Nothing here is scheduled.*

*Scope: the project layer above the compiler. `kelpc` stays the compiler. Coral owns the manifest, the project layout, the task graph, dependency resolution, the build cache, artifacts, the test runner, formatting and publishing. Modeled on Gradle: a project model, tasks with declared inputs and dependencies, a build cache, and a development/release split. Named Coral.*

Why this exists, in this repository's own evidence: `kelpc build <path>` compiles one `.kp` file or one directory containing `coral.toml`, and stops there. Everything beyond that is currently someone's shell script or Rust test code. `bench/speed/run.sh` is a 120-line shell program that assembles flags, runs each workload five times, and formats a table. `kelpc/tests/compile_and_run.rs` reimplements discovery, per-fixture flags, expected output, and an AddressSanitizer pass. `examples/download.kp` documents its own build command in a comment. Each of those is a build-tool task that has been hand-written once and will be hand-written again by the next project.

**Division of labor.**

- **`kelpc` keeps the compiler jobs**: load the module graph, type check, move/borrow check, emit C, invoke `cc`, and from MK7 emit machine code. `kelpc build` and `kelpc run` stay documented low-level commands.
- **Coral owns the project**: manifest, layout, tasks, profiles, dependencies, cache, test runner, artifact naming, publishing, toolchain pinning.
- **Coral calls `kelpc`, never the reverse.** Everything Coral needs from the compiler is a command-line contract, not a link against compiler internals.

**Tasks are Kelp programs, and that is the design.** Gradle needed a scripting language, then a DSL, then a plugin API, because a task is arbitrary code. Kelp already has arbitrary code: a task is an `.kp` file with `fun main()`, run by Coral, and its authority is whatever its own `impl` lines grant. No scripting DSL, no dynamic plugin API, no build logic inside a string. A task that reads data and writes a report declares exactly that, and the capability checker enforces it:

```kelp
// tasks/report.kp
impl "std/fs" with caps(fs = ["/project/out/**", "/project/data/**"])
impl "std/str"
impl "std/fmt"

fun main() {
    val size = fmt.uint(fs.size("/project/data/input.csv"))
    fs.write_text("/project/out/report.txt", str.concat("input bytes: ", size.as_str()))
}
```

The manifest names it, and Coral runs it:

```toml
# The shape Coral grows coral.toml into. Today's manifest is flat, has four keys
# (name, version, entry, http_pool), rejects every section, and rejects an
# unknown key, so this is a format change with a compatibility story to decide
# (open item 66).
[package]
name = "myproject"
version = "0.1.0"
entry = "src/main.kp"

[tasks]
report = { file = "tasks/report.kp" }
docs   = { file = "tasks/docs.kp" }
```

```sh
coral init myproject       # coral.toml, src/main.kp, tests/, .gitignore
coral build                # dev profile, artifacts in build/
coral build --release
coral run -- data.csv      # argv and exit code pass through
coral task report
coral test
coral fmt --check
coral clean
```

| Stage | Milestone | Content | Payoff |
| --- | --- | --- | --- |
| **MK13-S1 Project model and tasks** | MK13-S1-M1 | `coral init` scaffolding and layout conventions (`src/`, `tests/`, `tasks/`, `build/`), manifest sections, `coral build`/`run`/`check`/`clean`, the task graph, `coral task <name>` | One command per project instead of one shell script per project |
| | MK13-S1-M2 | Profiles (`dev`, `release`), per-profile flags, artifact naming and placement, argv and exit-code passing for `coral run` | The same project builds two ways without editing files |
| | MK13-S1-M3 | The compiler interface Coral depends on: `kelpc check --json` diagnostics, a per-module dependency list, a documented CLI contract | A build tool that reports and caches instead of scraping stderr |
| **MK13-S2 Incremental builds and caching** | MK13-S2-M1 | Fingerprints (source bytes, flags, profile, compiler version), module-granular recompilation, `build/cache` layout, the invalidation rule written down | A one-file change rebuilds in seconds |
| | MK13-S2-M2 | Cache reuse across branches and worktrees, `coral build --explain` naming the fingerprint that invalidated, cache export/import for CI | CI reuses what a laptop already built |
| | MK13-S2-M3 | Separate compile and link steps (`kelpc --emit-obj`, `kelpc link`), which is also the base for libraries and cross-compilation | Link-only rebuilds; one pipeline serving three later needs |
| **MK13-S3 Dependencies and resolution** | MK13-S3-M1 | Path and git dependencies, a lockfile, checksums, `coral add`/`update`/`vendor` | Kelp code shared between repositories without copy-paste |
| | MK13-S3-M2 | Offline and hermetic builds: vendored trees only, no network, reproducible dependency identity | Builds that work with no network and produce identical bytes |
| | MK13-S3-M3 | The registry decision: what a version constraint means for Kelp, what publishing publishes, who runs the index | Third-party dependencies, if the plan takes that decision |
| **MK13-S4 Workspaces and artifacts** | MK13-S4-M1 | Multiple packages in one workspace, intra-workspace dependencies by path, per-package targets | A compiler, a stdlib and a tool in one repository |
| | MK13-S4-M2 | Library targets: static and shared output, symbol and prefix rules, link flags (shared with MK9-S4-M2) | Kelp code usable from another Kelp project without shipping source |
| | MK13-S4-M3 | Target selection and cross-compilation: target triples, per-target caches and artifacts, toolchain and linker selection | Building for another machine from one host |
| **MK13-S5 Test, format, lint, bench** | MK13-S5-M1 | `coral test`: discovery, per-fixture flags, expected-failure fixtures, sanitizer profiles, plain and machine-readable output (absorbs what `compile_and_run.rs` and `bench/speed/run.sh` do today) | One command runs the suite this repository already has |
| | MK13-S5-M2 | `coral fmt` and `coral lint` over the formatter and lint surface (open items 13 and 23), `--check` mode for CI, settings in the manifest | Style stops being a review argument |
| | MK13-S5-M3 | `coral bench`: fixture workloads, repeat counts, result tables, comparison against a stored baseline | Regressions found by a command instead of by memory |
| **MK13-S6 Toolchains, plugins, publishing** | MK13-S6-M1 | Compiler version pinning (`coral toolchain`), per-project toolchain selection | A project builds with the compiler it was written against |
| | MK13-S6-M2 | Extension surface: tasks contributed by a dependency, generated sources, task outputs as task inputs, with no scripting DSL | Build logic that travels with a package |
| | MK13-S6-M3 | Publishing and CI recipes: artifact packaging, checksums, signing (shared with MK12-S3-M1), cache keys that survive a container restart | Releases that are one command |

**Deliberately not in Coral**: a scripting language (tasks are Kelp), a dynamic plugin runtime (extensions are manifests and Kelp programs), and a general version-range solver at first (exact versions plus a lockfile; ranges are MK13-S3-M3's problem).

**What Coral needs from elsewhere.**

- **From the compiler**: machine-readable diagnostics, a module dependency list, separate compile and link steps, and a stable CLI contract (MK13-S1-M3). Today `kelpc build`/`kelpc run`, `kelpc version` (added with the 0.1.1 release) and prose diagnostics are all a build tool has to work with.
- **From MK5-S2-M1 (done)**: `process.spawn`, argv, captured output and exit codes, which Coral and every non-trivial task need.
- **From MK8-S1/S2, only if Coral is written in Kelp**: generics, `Option`/`Result`, function values and containers — **all four shipped** (A-2 as MK8-S1-M1 part 2, `Option`/`Result` as MK5-S6-M1, containers as MK3, function values and closures as MK8-S1-M2 phases A-F), so the language half of open item 67 is met. Until Coral is ported it is a Rust program that drives `kelpc`.
- **From MK9-S4-M2**: cross-compilation and library output, which MK13-S4-M2/M3 extend rather than invent.
- **From the capability model (MK4-S1-M1, done)**: task grants are checked the same way module grants are, which is what lets a build task hold less authority than the user running it.

### What the port hit: three stdlib-sized gaps, all three now shipped

Writing the parts of Coral's Rust that shell out (`deps.rs`, `toolchain.rs`) against this stdlib ran into four things. Three were stdlib additions and **all three shipped as MK5-S2-M2** (the `fs.symlink` half continuing MK5-S5). None of them needed a language feature: the surface all of them would be written against — generics, `Option`/`Result`, containers, function values — was already shipped. The fourth was a language gap, and MK8-S1-M2 phases E-F closed it. The rows are kept as they were written, with what shipped recorded against each.

| Gap | Evidence | Workaround today | Fix | Size |
| --- | --- | --- | --- | --- |
| ~~**No `chdir`, and no per-call working directory.**~~ | *Done — **MK5-S2-M2**.* As written: `std/process.kp`'s `run` said it outright ("There is deliberately no `chdir` in v1: a process-wide working directory would make every grant pattern's meaning depend on mutable global state"), and Coral runs `git`/`cargo` inside specific target directories constantly, with `git -C`/`cargo --manifest-path` covering two tools and nothing else. | — | Shipped exactly as the sketch had it: `spawn_in(dir, program, args)` / `run_in(dir, program, args)`, `chdir` in the **child** between `fork` and `exec` (one branch in `spawn_raw`'s child path, plus the `dir` parameter), with `spawn`/`run` reduced to delegates passing `""`. Two things the estimate missed: `checked_args` for `process` had to stop being "the first `str` argument", because the directory is argument 0 in the `_in` forms, and a child that cannot enter `dir` exits **126**, which needed a reserved code and a doc line. | done |
| ~~**No `fs.symlink`** — and no `fs.read_link`~~ | *Done — **MK5-S2-M2** (the MK5-S5 continuation).* As written: `std/fs.kp` had no link primitive, symlinks appeared only where the *capability check* resolves them, and `toolchain.rs`'s `use_version` — the `~/.coral/bin/coral` link into the store — had nothing to call. | — | Shipped as `fs.symlink(target, link)` with **both** paths checked, which is the `fs` default and the safer reading: a grant stays closed under link creation. The sketch's "only if a caller needs to resolve one back" held for `read_link` — it is still absent. One consequence to know: the target must be absolute, since the check normalizes every path it matches. | done |
| ~~**`process.spawn`/`run` panic instead of returning a `Result`.**~~ | *Done — **MK5-S2-M2**, with one variant dropped.* As written: Coral treats a missing `git`/`cargo` as a reportable error, and ported as-is one missing tool ended the whole process. | — | Shipped as `try_run(...)`/`try_run_in(...)` returning `Result<RunResult, ProcErr>`, with `run` kept for callers that treat failure as fatal, and `process.executable` exposed as the pre-check in predicate form. Two precisions the sketch asked for and got: the failures are decided **before the fork** by `stat`/`access` (so `NotFound` means the path names nothing and `NotExecutable` means something is there that `exec` would refuse, and neither can be confused with a child exiting with a matching code), and `run("git", ...)` still has no `PATH` search — the check refuses a relative program, so resolving through `PATH` is the caller's job and stays out of the stdlib. **`NotAbsolute` and `Denied` did not ship**: both are refused by the compiler's call-site check, which panics, so neither is reachable from Kelp (open item 73 records the codegen change a value-returning denial would need). The sketch's "an error out-parameter on the extern" would not have reached either one. | done |
| ~~Closures could not capture, so `.filter`/`.find`/`.position` callbacks would have to become loops.~~ | *Closed by **MK8-S1-M2 phases A-F**.* A `fun` literal that names an enclosing binding is a `closure(T).R: U` that owns the environment it captured and drops it where it dies, and the `std/list`/`std/option`/`std/result` callbacks take closures — so `.filter`/`.find`/`.position`-shaped code ports as written. None of Coral's uses closed over meaningful state anyway, so this was never a blocker, only verbosity. | — | — | done |

The pattern in all three gaps was the same: the primitive was missing, the *shape* of it was already decided by the module that would gain it (`spawn_in` by `run`'s own doc comment, `try_run` by the `Option`/`Result` convention MK5-S6-M1 established for every other fallible stdlib call), and each was one extern plus one wrapper. To that the implementation added one place the estimate was wrong: the *capability* table, not the C, is what the `_in` forms moved — `checked_args` is keyed on the call's name now, so a working directory is never matched against a program allowlist. The one thing the compiler had to refuse outright is a value-returning grant denial (open item 73).

## Regulated and financial domains (roadmap open item 71)

*The `MK12-Sn-Mn` identifiers below are the ones this design carried when it was a Mark. Nothing here is scheduled.*

*Scope: decimal arithmetic, transactions, audit/traceability, auditor-usable verification tooling, legacy interop, and certification. Proposed because the domain was named; not endorsed by the plan, which has no section for it.*

| Stage | Milestone | Content | Was |
| --- | --- | --- | --- |
| **MK12-S1 Numeric correctness** | MK12-S1-M1 | `Decimal` / fixed-point numeric type, arithmetic, rounding modes, overflow behavior | Not in the plan before this entry |
| **MK12-S2 Transactional primitives** | MK12-S2-M1 | Transactional memory or a transactional primitive at the type level | Not in the plan before this entry |
| **MK12-S3 Audit and traceability** | MK12-S3-M1 | Source-to-binary provenance, reproducible builds, signed builds | Not in the plan before this entry |
| | MK12-S3-M2 | Structured audit-log primitive | Not in the plan before this entry |
| **MK12-S4 Formal tooling** | MK12-S4-M1 | Auditor-usable verification tooling built on MK11 | Not in the plan before this entry |
| **MK12-S5 Interop** | MK12-S5-M1 | Legacy-system integration: COBOL, DB2, MQ, mainframe protocols | B-1 is the primitive; nothing else in the plan |
| **MK12-S6 Certification** | MK12-S6-M1 | Certification path, conformance-test publication, standards-body engagement | Not in the plan before this entry |

**Caveat on MK12.** The plan deliberately rejects an ecosystem as a goal (no registry, no third-party dependencies in v1) and has no certification section. Banking is largely an integration domain, so several MK12 stages run against the grain of v1's deliberate non-goals. MK12 is recorded because the domain was named; whether MK12 exists is a **decision to make**, in the sense of block E, not a task to schedule.
