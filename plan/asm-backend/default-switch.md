# MK7: moving the default backend off `C`

> **ARCHIVED 2026-09-20. Superseded, not deleted.** The target backend changed to LLVM IR, so the
> flip this file sequences is not the one that will happen; `plan/mk7-llvm-backend.md` carries the
> criteria that replaced §1. Its §1.5 process rule is the part that survives whole: `--backend=c`
> and `KELPC_BACKEND=c` stay forever as the escape hatch and the oracle switch, and the C backend is
> never deleted.

*This is a proposed sequence, not a commitment and not a Mark. `roadmap.md` owns the Mark. This file
exists because `Backend::DEFAULT` is `C` for the whole of MK7 by design (`kelpc/src/backend.rs`), and
"the default stays C" has a reason (the differential oracle) that can be mistaken for "forever". What
follows is what would have to be true before the default moves, and in what order. `mk7-own-backend.md`
§0 stays the live state; this file is only the switch.*

## 0. Three different flips

"Fully switch to asm" is three decisions, and only two of them are wanted.

| flip | what it means | wanted |
| --- | --- | --- |
| **A** | a bare `kelpc build` uses `asm` | yes, this is the target |
| **B** | `asm` is the backend a release is built with | yes |
| **C** | `codegen_c.rs` is deleted, or stops being maintained | **no**, permanently |

The target is A plus B. C is out of scope forever, and everything below assumes the C backend stays
compiled, stays tested, and stays selectable. Ground rule 1 (`mk7-own-backend.md` §1) says it is never
deleted, for a reason that survives the flip: with one backend, "correct" means "matches the author's
intent"; with two, it is a measurement over the corpus.

What does **not** change at any flip: the language surface, the capability model, the move and borrow
rules, and the meaning of a `.kp` file. This is a default changing, not a language changing.

## 1. What must be true

Grouped by the kind of evidence, because each item exists for a different reason.

### 1.1 Correctness evidence

- **The corpus differential gate green.** Stage 1's clause 1: every fixture the C backend builds is also
  built by the asm backend, with byte-identical stdout.
- **The refusal ranking empty.** Not a quality goal once the default moves. The asm backend refuses a
  construct by name rather than emitting it wrongly (its documented design), so every remaining refusal
  becomes a user-visible build failure with no workaround. It is 3 fixtures today.
- **Differential fuzzing and a conformance corpus** (`roadmap.md` D-4). The highest-value item here. The
  corpus has been blind three times inside one recorded session: a float `List` element that only a
  hand-written differential probe could see, an enum slice's wrong-width load that "the differential
  sweep could not see", and a commit's "byte-identical" claim that a later measurement found false over
  6 of 235 compared targets. A differential suite is a strong regression instrument and a weak assurance
  instrument: it cannot see the case nobody wrote.
- **A memory gate that does not need generated C** (`roadmap.md` D-5). The C path gets ASAN over
  generated C plus valgrind. The asm path can only get valgrind, because `cc -fsanitize=address`
  instruments generated C and the asm backend emits none; `kelpc/tests/memory_gate.rs` says so in its own
  header. One memory tool instead of two is acceptable while C is the default and stops being acceptable
  when it is not.
- **A race gate, before threads.** There is no helgrind, DRD or TSan anywhere in the harness today.
- **Semantics pinned** (`roadmap.md` D-1 and D-2): an operational semantics with an aliasing model for
  `&mut`, and a UB catalogue covering overflow, uninitialised memory, out-of-bounds, provenance, the FFI
  boundary and data races. The concrete instance: signed overflow is pinned today by `-fwrapv`, which is
  a flag on the C path (`kelpc/src/driver.rs::compile_c`), and the asm path has no such flag and emits
  the arithmetic itself. That is the verification obligation transferring from C's semantics to Kelp's,
  and it has to be written down before a disagreement can be called a backend bug.

### 1.2 Coverage

- **Every fixture the C backend builds, built by asm.** This is stage 1 and it is gated.
- **A new feature lands in both backends or neither.** Otherwise the flip is a trapdoor: the surface a
  user can write stops matching the surface the compiler can build.
- **A standing rule for the blind spot.** Every defect the corpus could not see adds a fixture in the
  same commit that fixes it.

### 1.3 Performance

Measured, from `mk7-own-backend.md` §3:

| workload | `-O0` | `-O2`, today's default path | penalty |
| --- | --- | --- | --- |
| `collatz` (branchy int) | 446 ms | 83 ms | 5.4x |
| `float` (div + add loop) | 265 ms | 101 ms | 2.6x |
| `mix` (div-bound loop) | 204 ms | 168 ms | 1.2x |
| `fib` (real calls) | 28.4 ms | 24.0 ms | 1.2x |

The asm backend is gated at "no worse than the `-O0` column". A flip today hands every user the left
column for branchy and floating-point code. There are three answers and this is a decision, not a task:

1. **Accept and document it.** Cheap and honest, and bad for anyone doing arithmetic in a loop.
2. **Write an optimizer.** The plan calls inlining, CSE, LICM and strength reduction "the multi-year part
   and is not on this branch". This is the only answer that reaches `-O2`.
3. **Split the default by build kind.** `asm` when build speed is the point (a dev or check build), `C`
   when runtime is the point (a release build). The only option that pays both ways, and it needs a
   build-kind concept that does not exist yet. That is Coral territory (roadmap open item 72).

A flip that picks answer 1 or 3 also needs the compile-time number stated at the same time, since that is
the payoff being bought: at stage 1 the chain is `as` plus `ld`, not `cc`, and the full win arrives at S4.

### 1.4 Toolchain and artifacts

- **Stage 1 still needs `as`, `ld`, libc and `crt1.o`.** A flip removes the C compiler from
  code generation, not the host toolchain. `cc` stays required by `cargo test` and by the oracle path.
- **The artifact contract changes.** The C path writes a `.c` next to the binary
  (`kelpc/src/driver.rs::compile_c`); the asm path writes `.s` and `.o`. Anything that reads the
  generated C, for debugging or for docs, breaks at the flip. Decide and document the new contract in the
  same change.
- **Debuggability.** There is no DWARF and no source mapping (`roadmap.md` A-16, D-5), so a crash in an
  asm-built binary points into generated assembly text rather than a Kelp source line. For a default
  backend that is the difference between a bug report somebody can act on and one they cannot. This is
  the item most likely to be underestimated, because it is invisible until the first crash.
- **Diagnostics parity.** `glibc_lib_dir()` in `kelpc/src/driver.rs` still execs `cc` (or `clang`) for
  `-print-file-name=crt1.o` unless `KELPC_GLIBC_DIR` is set. That is a path lookup rather than a
  compilation, and it is why stage 1's "`cc` is not invoked" clause has to be read as "does not compile".
  S4 removes the question.

### 1.5 Process

- **`--backend=c` and `KELPC_BACKEND=c` stay forever**, documented as the escape hatch and the oracle
  switch. A flip with no way back is how a second backend becomes unmaintained.
- **The suite keeps running both backends after the flip.** That is the whole reason the C backend
  survives.
- **Rollout.** The flip lands in a release that announces it, names the minimum version, and states the
  one flag that goes back.

## 2. Stages

Ordered by dependency, not by size. Each stage names what proves it. `mk7-own-backend.md`'s own stage 2
and stage 3 appear here as S4 and S6: they are inputs to the flip, not alternatives to it.

| stage | what | exit criterion | evidence |
| --- | --- | --- | --- |
| **S0** | stage 1 green | `mk7-own-backend.md` §5's five clauses | asm suite green, refusal ranking empty, `bench/speed` no worse than `-O0`, no compiler in the chain, valgrind clean |
| **S1** | semantics and checkers | D-1 and D-2 written; a race gate exists | the two semantics documents; helgrind or TSan added to `memory_gate` |
| **S2** | fuzzing and conformance | N generated programs, both backends, zero mismatches | the generator, its seed corpus, its coverage statement, and the mismatch list |
| **S3** | debug info and source mapping | a crash in an asm binary names a Kelp source line | A-16 landed, plus a fixture that crashes on purpose |
| **S4** | stage 2: own encoder and ELF writer | `mk7-own-backend.md` §6's definition of done | all fixtures under `--backend=own`, no assembler and no linker invoked |
| **S5** | the performance decision | one of the three answers in 1.3, written down with numbers | an idle-machine `bench/speed` table and the decision it produced |
| **S6** | stage 3: freestanding | `mk7-own-backend.md` §7's definition of done | no libc linked, and the `ldd` reading the definition asks for |
| **S7** | the flip | `asm` is the default in a release, `C` the oracle and the escape hatch | the release notes, both suites green, the wiki pages moved on `unreleased` and then `main` |

**The minimum for S7 is S0, S1, S2, S3 and the S5 decision.** S4 and S6 are not required for the flip:
S4 removes `as` and `ld`, and S6 removes libc, which is the requirement for kernel, driver, firmware and
embedded targets rather than for a default changing. A flip may happen with S4 and S6 still ahead, and
should say so.

## 3. What would make the flip unsafe

- **A green corpus with a blind spot.** Documented three times in one session, which is what S2 is for.
- **Shipping an optimizer-free backend as the default while the old default was `-O2`.** A 5.4x
  regression on branchy code is a real cost, and it needs a stated decision rather than silence.
- **No source mapping**, so every crash report is unreadable.
- **Flipping while a construct exists in one backend only.**
- **Flipping without keeping the C backend green**, which would destroy the evidence the flip rests on.

## 4. Where this plan sits

- Not a Mark and not scheduled. The three reversals `roadmap.md` guards against (B-4, B-6, B-7) are
  separate questions; this file is about a default, not about the language.
- If the owner promotes it, `roadmap.md`'s MK7 row gains a stage and this file becomes the detail behind
  it. Until then it is a proposal.
- Cross-references: `roadmap.md` MK7, and blocks D and E for D-1 to D-5; `mk7-own-backend.md` §0, §1,
  §5, §6 and §7; `mk7-stage2-encoder.md`; `mk7-call-overhead.md` for the performance column.
- The facts this file relies on, with their sources: `Backend::DEFAULT` and `Backend::IMPLEMENTED`
  (`kelpc/src/backend.rs`); `compile_c` writing the `.c` and passing `-fwrapv`
  (`kelpc/src/driver.rs`); the `as` plus `ld` path, the `crt1.o` dependency and the `cc`
  `-print-file-name` lookup (`kelpc/src/driver.rs`); the ASAN gate's own limits
  (`kelpc/tests/memory_gate.rs`); and the stage gates (`mk7-own-backend.md` §5 to §7).
