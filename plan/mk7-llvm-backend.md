# MK7: the LLVM backend

*The plan of record for the compiler's next backend.* `kelpc` will lower its typed
IR (`tir`) to **LLVM IR** and let LLVM optimize and generate the object, the way
`rustc` does. The hand-written backend track is retired, and its plans and
measurements are in `asm-backend/`.

This file replaces four archived documents: `asm-backend/plan.md` (the stage order
and gates), `asm-backend/default-switch.md` (the flip criteria; §1.5 of it is
carried over whole in §2 below), `asm-backend/call-overhead.md` (the measurement
the decision rests on) and `asm-backend/encoder-inventory.md` (the encoder and
ELF-writer inventory that no longer has to be written).

## 0. Why LLVM, and what it costs

### What it gives us

- **The optimizer, which was the multi-year part.** The retired track was gated at
  "no worse than the `-O0` column", and closing the rest of the gap meant writing
  inlining, CSE, LICM and strength reduction by hand. The size of that gap is
  measured: generated C on `bench/speed` builds at `-O0` and `-O2` as

  | workload | `-O0` | `-O2` | what the gap is |
  | --- | --- | --- | --- |
  | `collatz` (branchy int) | 446.5 ms | 81.5 ms | 5.5x |
  | `float` (div + add loop) | 254.8 ms | 100.4 ms | 2.5x |
  | `mix` (div-bound loop) | 203.1 ms | 165.2 ms | 1.2x |
  | `fib` (real calls) | 26.7 ms | 23.3 ms | 1.15x |

  (`asm-backend/call-overhead.md` §1, medians of 8 interleaved rounds, load 2.5 to
  2.7. The same table in the roadmap's older wording reads 5.4x, 2.6x, 1.2x, 1.2x.)
  Under LLVM both columns are a pass-pipeline flag, not a project.
- **A defined IR.** LLVM's arithmetic has a defined result for every operation we
  emit, provided we emit the non-flagged forms; §3.4 is the rule that turns today's
  `-fwrapv` flag into a property of the IR we write.
- **DWARF and debug locations**, from `DILocation`/`DIBuilder`, instead of the
  `#line`-per-statement trick the C backend uses for `cc` diagnostics (item 20).
- **Multiple targets.** `x86-64` was the only target the retired track could ever
  reach without a second backend, an ELF writer and a freestanding runtime. LLVM
  gives `aarch64`, `riscv64`, `wasm32` and the rest from one lowering, gated only
  by the runtime and the linker. A target is a triple, not a project.

### What it costs, stated plainly

- **The compile-time payoff is given up.** The retired track's headline number was
  a ~10x compile time win from replacing `cc` with `as` + `ld`, because about 95%
  of `kelpc build` is the C toolchain's fixed overhead and Kelp's own pipeline is
  3 to 4 ms. LLVM *is* that toolchain's backend, so compile time stays where it is
  or rises: emitting IR, running the pass pipeline and running `LLVMCodeGen` is the
  work `cc -c` was already doing. The honest expectation is "at or above today's",
  and `bench/speed`'s compile-time column is where that gets measured.
- **A large external dependency.** LLVM is a C++ library of hundreds of megabytes.
  §1 is the decision about how `kelpc` links it.
- **`cc` stays in the chain for linking**, at least at first (see §1.3).
- **No formal semantics.** §3.4 says what LLVM *does* give us and what it does not.

### How long this is, and why it is weeks rather than years

Not a weekend, and not years. The reason is that only the **IR emission** layer is
new. Everything the retired plan listed as the expensive part is either already
ours or already LLVM's:

- **Already ours, and backend-independent:** the lexer, the preprocessor, the
  parser, the type checker, the move and borrow checker, the typed IR, and the
  meaning of every AST construct. "What is a call, what is a `datac`, what does
  this pattern bind" does not change with the target.
- **Already LLVM's:** the optimizer, register allocation, instruction selection,
  the encoder, the ELF writer, the debug-info *format*, sanitizers and the target
  list.

So the work is one module, `codegen_llvm.rs`, as a peer of `codegen_c.rs`: the same
walk over `tir`, emitting a different target. The estimate, as an estimate:

| Phase | What it is | Effort |
| --- | --- | --- |
| Minimal running | base types, functions, control flow (`if`/`while`/`for`), arithmetic, calls, `print` | days to 1 to 2 weeks |
| Feature parity | `datac`/`enum` layout, containers, drops, closures and function values, the extern bodies. The capability check is mostly frontend: resolution already refuses an ungranted name, and codegen emits one runtime call per gated site | 2 to 4 more weeks |
| Differential testing | run the fixture suite (358 tests) against the LLVM backend | runs alongside the two rows above, almost no extra time: the harness and the C oracle already exist |
| Debug info | attach LLVM's DWARF metadata and locations | days, because the format ships with the library |
| Polish and edge cases | whatever the corpus and the fuzzer turn up | ongoing, never really "finished", usable much earlier |

In other words: roughly feature-complete with the fixture suite green in about
**3 to 6 weeks of work**, not months and definitely not years, and it is usable well
before that. The one estimate that does not shrink is the compile-time column: LLVM
run in the driver is the same backend `cc` already is.

**What the retired track does not waste.** Its stage 1 answers questions that do not
change with the target: which AST constructs actually occur, where the container
emission order bites, and where our own marshalling pitfalls were (the call-cost and
ABI-classifier work in `asm-backend/call-overhead.md` and
`asm-backend/closures.md`). Those are frontend and IR facts. Only the emission
target changes.

**The stages the old plan listed that are simply gone.** Reading
`asm-backend/default-switch.md`'s S0 to S7: S0 (the seam) is done and stays; S1
(semantics and checkers as a flip gate) is superseded by "emit only the defined IR
forms", §3.4; S3 (debug info) is a metadata call; S4 (own encoder and ELF writer) is
not written; S5 (the performance decision) is a pass-pipeline flag, §4.3. S2
(fuzzing against the corpus) survives and is still required to *believe* a flip.

## 1. Crate and linking

### 1.1 The three options

| Option | What it is | Cost |
| --- | --- | --- |
| `inkwell` | A Rust wrapper over the LLVM C API: safe handles, a builder, typed values. | One more dependency, and its API tracks LLVM majors, so an LLVM bump is a wrapper bump. Convenient up to the point where the wrapper refuses to expose a C API call we need. |
| `llvm-sys` | The raw C API bindings. The crate version encodes the LLVM version (`llvm-sys` 211 is LLVM 21.1, 231 is LLVM 23). Everything is `unsafe`. | Roughly sixty functions to wrap by hand, and every one is an `unsafe` extern. Nothing between us and the library. |
| Hand-written `extern "C"` bindings, no crate | Declare the C API subset ourselves and find/link `libLLVM` from `build.rs`. | Zero dependencies, and we own the declarations and the version check. |

### 1.2 Decision

**`llvm-sys` plus a thin wrapper module of our own** (`kelpc/src/llvm_ffi.rs`), over
`codegen_llvm.rs` proper. Reasons:

- The wrapper we write is small and it is the part we would have to understand
  anyway. It is the same reasoning `backend.rs` already records for the backend
  seam: name the interface when the second implementation exists, not before.
- `inkwell` is the faster start and the worse long run: an LLVM major bump forces a
  wrapper bump in lockstep, and when the wrapper does not expose an enum or a
  builder call the answer is to drop to `llvm-sys` anyway, which means the wrapper
  is in the tree either way.
- Hand-written bindings are one dependency fewer but one more thing to maintain at
  LLVM's pace, and LLVM's C API does not change often enough to justify owning it.

This is a decision to revisit at most once: if wrapping the C API turns out to be
the bottleneck rather than the lowering, switch to `inkwell` without touching
`codegen_llvm.rs`, because the wrapper is its only client.

### 1.3 Linking model

**Dynamic, against the host's `libLLVM`**, found through `llvm-config` or
`LLVM_SYS_211_PREFIX`. On this machine the library is
`/nix/store/0z4wpkyl46ibsjg74x833qicn84ll5il-llvm-21.1.8-lib/lib/libLLVM-21.so`,
alongside `clang 21.1.8`.

- Static linking is rejected: an LLVM static build is hundreds of megabytes and
  minutes of link time on every `cargo build`, which is a worse cost than the one
  the retired track was trying to remove.
- The consequence must be documented, not discovered: a `kelpc` built this way
  needs `libLLVM-21.so` at runtime. The release asset (`kelpc-<version>-linux-x86_64`)
  therefore carries a runtime dependency the C-only `kelpc` did not have, and the
  release notes must say which LLVM major.
- The LLVM major is part of `kelpc`'s build contract. `llvm-sys`'s version pins it,
  so bumping it is a deliberate commit with a `cargo test` run, never a side effect.
- **Linking the produced binary stays `cc`.** `LLVMTargetMachineEmitToFile` writes
  an object; the driver then invokes `cc` on it exactly as it invokes `cc` on the
  generated C today (`driver.rs::compile_c`). That keeps `glibc_lib_dir()` and the
  `crt1.o` lookup unchanged. Linking with `ld` directly is an optional driver slice
  later, not part of this plan.

## 2. What stays exactly the same

- **`Backend::C` is the oracle and the escape hatch, permanently.** Carried over
  whole from `asm-backend/default-switch.md` §1.5:
  - `--backend=c` and `KELPC_BACKEND=c` stay documented and stay selectable forever.
  - `codegen_c.rs` is never deleted and never stops being maintained.
  - The suite keeps running both backends after the flip. That is the whole reason
    the C backend survives: with one backend, "correct" means "matches the author's
    intent"; with two, it is a measurement over the corpus.
  - A flip with no way back is how a second backend becomes unmaintained.
- **The backend seam.** `backend.rs` keeps its shape: the selector, the names, the
  `--backend=`/`KELPC_BACKEND` resolution, `Backend::DEFAULT`. The new name is
  `llvm`. `asm` was removed from `Backend::IMPLEMENTED` on 2026-09-21, with its
  source and its rows in the tests.
- **The fixture suite stays backend-agnostic** (compile, run, compare stdout), so it
  validates the new backend by being pointed at it.
- **The language does not change.** The surface, the capability model, the move and
  borrow rules and the meaning of a `.kp` file are identical. This is a backend
  changing, not a language changing.
- **The runtime's observable behaviour.** The capability deny path's text, the
  panic prefix and the `errno` second line are pinned by tests and stay as they are.
  `asm-backend/capability-check.md` is still the brief for the check's shape.

## 3. What is genuinely our work

### 3.1 The lowering

`codegen_llvm.rs` walks `tir` and builds an `LLVMModuleRef`. `tir` is already the
surface both existing backends consume, and the retired track proved it is
backend-complete: `codegen_asm.rs` lowered the whole language except the rows its
refusal ranking named. So the front half of the compiler is unchanged and the new
module is a peer of `codegen_c.rs`, not a new pipeline.

| Kelp | LLVM IR |
| --- | --- |
| `i8`/`i16`/`i32`/`i64`/`usize`, `bool`, `char`, `f32`/`f64` | `iN`, `i1`, `i32`, `float`/`double` |
| `datac` | an identified struct type with the same field order and natural alignment |
| `enum` | a `{ tag, union }` struct, the same layout `codegen_c.rs` emits today, with one `switch` per drop |
| `List<T>`, `Map<K, V>`, `Box<T>` | a pointer plus the existing monomorphized function set, one set per element or value type |
| `str`, `View`, `Bytes`, `String` | a pointer and a length; the runtime calls take the pair, never the header by value |
| `if`/`while`/`for`, `when`, `if let`, `?` | `cond_br`, `br`, a `switch` for a scrutinee |
| the drop plan | the same set of drops, emitted as calls at the same points |
| `pd` storage | module globals, plus the `atexit` registration an owned `pd` already needs |
| `print` and templates | calls into the runtime's `%.*s`/integer path, as the C backend does |

### 3.2 The runtime: keep it C, compile it once

The `Bytes`/`String` runtime (allocation, bounds checks, `free`), the 251 lines of
extern bodies in `externs.rs`, and the capability matcher and path normalizer
(`CAP_RUNTIME`/`PATH_RUNTIME`: `kelp_glob_match`, `kelp_cap_check_path`,
`kelp_host_match`, the `..`-rejecting normalizer) are C text today.

They stay C. `build.rs` compiles `runtime/kelp_rt.c` with `cc` into an object that
ships beside `kelpc`, and every binary links it. Reasons:

- The capability matcher's *text* is security-relevant and is pinned by the tests.
  Re-deriving it in IR buys nothing and risks a divergence in exactly the code that
  decides whether a path is inside a grant.
- `cc` is already required to build `kelpc`, so this adds no new tool requirement.
- The crossing surface stays pointers and integers, so IR and C never have to agree
  on an aggregate layout.

The exception is the **monomorphized container code**, which `codegen_c.rs`
generates per element or value type and therefore cannot live in a fixed C file.
That is generated into the module: as IR, with one function set per instantiation,
exactly as the C backend writes one C function set per instantiation.

Later, and independent of this plan, the runtime can move into Kelp as `std/rt` one
piece at a time (the retired track's §8 and `runtime/tests.md`).

### 3.3 Layout agreement

Where an aggregate must cross into the C runtime, pass a pointer, not a value. The
one place layout has to match is the container header (`cap`, `len`, `data`) that
Kelp types and the allocation functions both touch; it is declared once in
`runtime/kelp_rt.c` and mirrored in `codegen_llvm.rs`, with a `static_assert` on
the C side and a size/offset check in a test on the IR side
(`the_builtin_heap_layouts_match_the_c_runtime`).

**Measured 2026-09-21, and "a pointer, not a value" is not only about layout**
(BUG-vN4tR). Clang lowers a 24-byte struct argument and return to SysV itself
(`byval`, `sret`), and LLVM 21.1.8's backend on `x86_64-unknown-linux-gnu` does not
derive that lowering for an IR call that passes the aggregate by value: against
clang's object the `sret`-shaped call dies with SIGSEGV and the `byval` one returns
a wrong value. Both sides of an IR-to-IR call agree, so the verifier and the
fixture suite are blind to it. An aggregate over 16 bytes therefore crosses to C
with the pointer spelled out (`sret` is argument 0, so `void @f(ptr)` matches it)
and never by value.

### 3.4 Move and borrow semantics on LLVM, and the UB surface

This is the part LLVM does not do for us, and it is where the new backend can be
*wrong* rather than incomplete.

- **A move is a load and a store.** A non-`Copy` value moves by assignment in Kelp
  and the C backend gets that free from C struct assignment. In IR it is an
  aggregate `load` plus a `store`, and the drop plan must not leave a second drop
  behind the moved-from binding. The frontend's move marking already guarantees
  this; the backend's job is to keep it, and there is a fixture for it.
- **No `nsw`/`nuw`, no `inbounds`, no poisoning flags.** Today signed overflow is
  pinned by passing `-fwrapv` to `cc`. Under LLVM there is no such flag: we write
  `add`/`sub`/`mul` *without* the `nsw`/`nuw` flags, so the result wraps by
  definition, and we do not emit `inbounds` on a GEP unless the bounds are proven,
  nor `nuw` on an index computation. A `udiv`/`sdiv` by zero is UB in LLVM as it is
  in C, so the existing guard where Kelp's division can see a zero operand stays.
- **`noalias` only where the checker proves it.** The first pass emits `noalias`
  on nothing but a `&mut` parameter, which the borrow checker already guarantees is
  unique for the call's duration. Widening that needs an argument, not a habit.
- **What LLVM does not give us, and what D-1/D-2 still mean.** LLVM supplies a
  defined semantics for the IR *we* emit. It does not supply a formal operational
  semantics for Kelp, an aliasing model for `&mut`, or a UB catalogue. Those remain
  the duty for `unsafe`, atomics and threads (`roadmap.md` D-1 to D-3, MK11). What
  is gone is their role as a *flip prerequisite* for a hand-written backend that
  had to pin by hand the semantics it emitted.

## 4. What must be true before the default moves

Replaces `asm-backend/default-switch.md` §1. Grouped the same way, because each item
exists for a different reason.

### 4.1 Correctness evidence

- **The corpus differential gate green.** Every fixture the C backend builds is
  built by the LLVM backend too, with byte-identical stdout and exit status.
- **The refusal ranking empty.** Nothing is refused by name once the flip is
  announced. A refusal is a user-visible build failure with no workaround, which is
  acceptable while the backend is opt-in and not acceptable as a default.
- **Differential fuzzing and a conformance corpus** (`roadmap.md` D-4). Still the
  highest-value item, and still true of this backend: the corpus has been blind
  three times inside one recorded session (a float `List` element only a hand-written
  probe could see, an enum slice's wrong-width load, and a "byte-identical" claim a
  later measurement found false over 6 of 235 targets). A differential suite is a
  strong regression instrument and a weak assurance instrument.
- **A memory gate.** valgrind extends to LLVM binaries unchanged. AddressSanitizer
  *returns*: ASAN is an LLVM pass plus a runtime, so it can instrument the emitted
  IR. That is a gain over the retired track, where ASAN could only see the C
  backend because it instruments generated C.
- **A race gate, before threads.** Nothing in the harness is helgrind, DRD or TSan
  today. Unchanged by this plan and still owed before MK6-S1-M1.
- **Debug locations.** A crash in an LLVM-built binary names a `.kp` line (§4.3).

### 4.2 Coverage

- Every fixture the C backend builds, built by LLVM.
- A new feature lands in both backends or neither. Otherwise the flip is a trapdoor:
  the surface a user can write stops matching the surface the compiler can build.
- A standing rule for the blind spot: every defect the corpus could not see adds a
  fixture in the same commit that fixes it.

### 4.3 Performance and optimization level

The question is no longer "write an optimizer", it is "which pipeline, by default".
The C backend runs `cc -O2`, so today's default is `-O2`-equivalent and the LLVM
backend should not regress from it.

- **Recommendation: LLVM's `default<O2>` as the default**, with an `-O0`-equivalent
  fast path added as a knob (a flag, and later a Coral build kind: `asm` speed was
  the point of a dev build, and `-O0` IR emission is much cheaper than a pass
  pipeline). The three answers the retired plan listed (accept `-O0` and document
  it, write an optimizer, or split by build kind) collapse to the third, with no
  optimizer to write.
- The number to state at the flip is the compile-time column, since that is what
  changes: `bench/speed`'s build time and the four workloads' runtime, both
  backends, on an idle machine.

### 4.4 Toolchain and artifacts

- `cc` still links the object, and `libLLVM-21.so` is required to run `kelpc`. Both
  go in the release notes.
- **The artifact contract changes.** The C path writes `<out>.c` next to the binary;
  the LLVM path writes `<out>.ll` (textual IR) at `-O0`/`--emit-ir`, and optionally
  `<out>.o`. Anything that reads the generated C breaks at the flip; the new
  contract is decided and documented in the same change.
- **Debuggability.** DWARF is emitted, and it is a stated exit criterion rather than
  a nice-to-have: for a default backend, a crash report that points into generated
  IR instead of a Kelp source line is the difference between actionable and not.
  This was the item most likely to be underestimated on the retired track, and under
  LLVM it is a `DIBuilder` call rather than a debug-info format.

### 4.5 Process

- `--backend=c` and `KELPC_BACKEND=c` stay forever, documented as the escape hatch
  and the oracle switch.
- The suite keeps running both backends after the flip.
- **A release that wants both backends in one binary cannot statically link LLVM**;
  the asset carries the `libLLVM` requirement, and the release notes name it.
- Rollout: the flip lands in a release that announces it, names the minimum version,
  names the `libLLVM` requirement, and states the one flag that goes back.

## 5. Stages

Ordered by dependency. Each stage names what proves it and the corpus gate is
green at every stage.

| stage | what | effort | exit criterion |
| --- | --- | --- | --- |
| **S0** | the seam | done | `Backend`, `--backend=`, `KELPC_BACKEND` |
| **S1** | IR for the scalar subset, `llvm` selectable | done 2026-09-21 | arithmetic, comparisons, `if`/`while`, calls, `print` of the scalars: byte-identical stdout on the fixtures that use only those (`kelpc/tests/run-llvm-scalar-subset.sh`) |
| **S2** | aggregates, containers and drops | 2 to 4 weeks | `datac`/`enum` layout, the container function sets, the drop plan: the whole fixture corpus minus the runtime rows |
| **S3** | the runtime, the externs and the capability check | done 2026-09-21 | `librt` linked, every `extern fun` a declared call, every gated call site checked, the deny path text identical, `pd` globals with their `atexit` drops |
| **S4** | debug locations to DWARF | days | a fixture that crashes on purpose names a `.kp` line |
| **S5** | fuzzing and conformance (D-4) | runs alongside S2 and S3 | N generated programs, both backends, zero mismatches |
| **S6** | the optimization-level decision | a day, plus a bench run | one answer from §4.3, written down, with an idle-machine `bench/speed` table |
| **S7** | the flip | a release | `llvm` is the default in a release, `c` the oracle and the escape hatch, both suites green |

Stage 2 is one stage in the plan and was landed in slices, and the backend's
refusals name those slices, so the mapping belongs here. `S2D` to `S2F` are the
labels the refusals already carry.

| slice | what it is | state |
| --- | --- | --- |
| **S2A** | plain records: a `datac` whose fields are all dropless, with the C backend's field order and alignment, construction, field read and write, by-value parameters and returns | done 2026-09-21 |
| **S2B** | the heap: the C runtime compiled once by `build.rs` instead of emitted per program, `str`/`String`/`Bytes`, an owned field, the drop plan, `panic`/`assert` | done 2026-09-21 |
| **S2C** | tagged unions: `enum` at the C backend's `{ tag, union }` layout, `when`, `if let`, `?`, and one `switch` per drop | done 2026-09-21 |
| **S2D** | containers: `List`, `Map`, `Box`, `[T; N]`, one monomorphized function set per instantiation | done 2026-09-21 |
| **S2E** | closures and function values | done 2026-09-21 |
| **S2F** | references that are not a `datac`: `&T` to a scalar, a borrow of a field, and the dereference of one | done 2026-09-21 |
| **S3A** | the `extern fun` bodies: the C text the C backend emits, compiled per program and linked | done 2026-09-21 |
| **S3** | the rest: `pd` globals with their `atexit` drops, the capability check (the allowlist arrays, the matcher and the deny path), and the last three refusals (`String#into_bytes`, a field of an owned temporary) | done 2026-09-21 |

Two notes on the labels. The stale `S2B` label on the borrow of a non-`datac`
is gone: S2F lowered the borrow of a scalar and of a field, its dereference and
its store, the `&T` parameter and the `&T` return, and S2E lowered the last
borrow it named, the one of a function value or a closure. With S3 landed there
are no refusals left: the `llvm` backend builds every one of the 221 corpus
targets the C backend builds, the refusal ranking is empty, and the differential
sweep records no disagreement. Two shapes that once refused are lowered rather
than named: `String#into_bytes` crosses through the runtime object's
pointer-spelled adapter, and an owned temporary reached outside a call argument
is staged and dropped at the expression that read through it. The string literal
containing NUL (`BUG-f5Kvq`) stays refused, because the C backend does not carry
it either: `printf("%s", "a\0b")` prints `a`, so the truncation is a C-backend
defect and not an LLVM gap.

The minimum for S7 is S1 to S4 plus the S6 decision. S5 is not required to flip but
is required to believe the flip, and saying so is part of the flip's notes.

## 6. The test suite: repurpose, do not retire

| Gate | What happens to it |
| --- | --- |
| `kelpc/tests/compile_and_run.rs` | **Unchanged in kind.** It is backend-agnostic by construction, so the LLVM backend is added by putting `llvm` in `Backend::IMPLEMENTED`. The `asm`-specific assertions left with the backend on 2026-09-21: the `.s`-level SysV classifier test, the assembly-text capability-check assertions and the two-backend differential test are gone. |
| `kelpc/tests/backend_coverage.rs` | **Rebuilt for LLVM, 2026-09-21.** The differential sweep builds every fixture the C backend builds with `llvm` too, runs both and compares stdout and the exit status byte for byte, ratchets `LLVM_BUILT_BASELINE` (221 of the 221 the C backend builds at the MK7 S3 tip, so the refusal ranking is empty), and prints the refusals ranked by message. Its first run found a silent miscompile (`BUG-q7Whn`) and two internal errors (`BUG-b2Ldp`), which is the instrument doing its job. The heavy tests are `#[ignore]`d so the default suite stays cheap: run them with `-- --ignored --nocapture`. |
| `kelpc/tests/memory_gate.rs` | **Repurpose and extend.** The valgrind half covers LLVM binaries unchanged; its LLVM sweeps now cover the closures, the containers, the extern bodies, and (S3) the passdown and capability binaries, whose `atexit` drop and check glue are generated code the C sweep cannot see. The ASAN half, which today can only instrument the C backend, comes back for LLVM (§4.1) once the ASAN pass and runtime are wired. |
| `kelpc/tests/run-all-backends.sh` | **Repurpose.** `BACKENDS` gains `llvm`; `asm` left with the source on 2026-09-21. |
| `codegen_asm.rs` and the `asm` rows of the tests | **Done early, on 2026-09-21.** The module, the `asm` baseline, the `.s`-level tests, the `Backend::Asm` variant and the fixture that existed only for its assertion are gone, and the C-only suite is green. The deletion moved ahead of LLVM parity by decision: the retired track's second-backend differential was already partial, and keeping it cost a hotspot test file plus permanently refused fixtures. |
| `ISSUES.md` entries about the `asm` backend | **Closed on 2026-09-21**, with the removal, in the same session. The entries against the archived plan text were wiped with them. |

## 7. What would make the flip unsafe

- **Emitting an attribute the checker does not justify.** `nsw`, `nuw`, `inbounds`,
  or a `noalias` wider than a `&mut` parameter tells LLVM that a program has
  behaviour Kelp does not guarantee, and LLVM will optimize on that claim. This is
  the one new way to miscompile a correct Kelp program, and it is the risk this plan
  exists to name.
- **A green corpus with a blind spot.**
- **No debug locations**, so every crash report is unreadable.
- **Flipping while a construct exists in one backend only.**
- **Flipping without keeping the C backend green**, which destroys the evidence the
  flip rests on.
- **Shipping a `kelpc` that needs `libLLVM` without saying so.**

## 8. Out of scope, permanently

- **Rewriting `kelpc` in Kelp is out of scope, permanently.** There is no `stage1`,
  no `stage2` and no fixpoint milestone, none will be added, and this backend change
  does not reopen the question. The compiler is and stays a Rust program. The
  roadmap's MK8 bootstrap stages are cancelled (see `roadmap.md`, MK8).
- **A hand-written optimizer** for a hand-written backend: retired with the track.
- **`no_std`/freestanding as a backend deliverable** (`roadmap.md` B-6, MK9-S4):
  still wanted for kernels and firmware, and now an LLVM triple plus a runtime
  question rather than tier 3 of a backend.
- **The encoder and the ELF writer**: not written.

## 9. Where the numbers come from

- The optimizer gap, per workload: `asm-backend/call-overhead.md` §1 (this machine,
  2026-09-20, medians of 8 interleaved rounds).
- The compile-time share of `cc`: `roadmap.md`'s MK7 section, measured 2026-09-18.
- The retired track's stage order, gates and refusal ranking:
  `asm-backend/plan.md` §0 and §5.
- The flip criteria this file rewrites: `asm-backend/default-switch.md` §1, with
  §1.5 carried over whole into §2 above.
- The capability check's shape and text: `asm-backend/capability-check.md`.
- The facts this file relies on, with their sources: `Backend::DEFAULT` and
  `Backend::IMPLEMENTED` (`kelpc/src/backend.rs`); `compile_c` writing the `.c`,
  passing `-fwrapv` and looking up `crt1.o` through `cc`
  (`kelpc/src/driver.rs`); the heap preamble and the per-statement `#line`
  (`kelpc/src/codegen_c.rs`); the extern bodies (`kelpc/src/externs.rs`); the typed
  IR's size and node kinds (`kelpc/src/tir.rs`); the ASAN gate's own limits
  (`kelpc/tests/memory_gate.rs`). The LLVM version on this machine is 21.1.8
  (`clang --version`, `libLLVM-21.so` under `/nix/store`).
