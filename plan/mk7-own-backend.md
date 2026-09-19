# MK7 working plan: Kelp's own backend

*This is the execution plan for `roadmap.md`'s MK7 (`Own backend`, S1-S5, "very large"). The roadmap says
**why**; this file says **what order** and **what proves each step**. It is written to be worked over days
with two agents in parallel worktrees, and it is the only place the branch's staging is decided. Where this
file and `roadmap.md` disagree on detail, `roadmap.md` still owns the Mark; where they disagree on
sequence, this file owns it.*

**The goal is not speed.** It is that Kelp stops being C with a mask: `kelpc` produces the artifact itself,
and a Kelp binary is a thing we control rather than something `cc` and glibc happen to produce. `roadmap.md`'s
compile-time number (~95% of `kelpc build` is clang's fixed overhead) is a *side effect* that justifies
starting, not the reason to finish.

**The decision this plan encodes:** no LLVM, no Cranelift, no assembler, no linker, no libc. Everything the
current toolchain does, we do. That is the expensive reading of "no LLVM no nothing", and it is the reading
this branch takes.

---

## 1. Ground rules

These hold for every stage. Breaking one is how this becomes a rewrite instead of a replacement.

1. **`codegen_c.rs` is never deleted.** It becomes the permanent differential oracle: every fixture is
   compiled and run by both backends, and their stdout is compared. A backend is correct when it agrees with
   the C backend, not when it agrees with the author's intent.
2. **248 fixtures are the contract.** They are backend-agnostic by construction (compile, run, compare
   stdout), so they validate a second backend with no edits. Any fixture that has to change for the new
   backend is a bug in one of the two, and the diff says which.
3. **One target.** `x86-64`, SysV ABI, Linux. Nothing else is attempted until Tier 3 is done. Cross-compilation
   (`backlog.md` MK13-S4-M3) and a portable encoder are explicitly out of scope on this branch.
4. **Tiered, not simultaneous.** Tier 2 does not start before Tier 1 passes the full suite; Tier 3 does not
   start before Tier 2 does. Each tier is a working compiler.
5. **The cheap oracle stays cheap.** `cargo test` must keep passing on `main` at every commit. Nothing on
   this branch is allowed to make the C backend's build or test time worse.

## 2. Baseline, measured (2026-09-18, this machine)

Everything the plan is judged against. Re-measure before each stage gate.

| item | value |
| --- | --- |
| `kelpc build examples/hello.kp` | 80 ms |
| `kelpc build bench/speed/speed.kp` | 108 ms |
| Kelp's own pipeline (lex → preprocess → parse → infer → borrowck → codegen) | 3-4 ms |
| clang's share, of which | 76-102 ms |
| — cc1 startup + system-header preprocessing | ~38 ms |
| — optimizer | ~25 ms |
| — link (even from a prebuilt `.o`) | ~46 ms |
| generated C for `examples/hello.kp` | 536 lines |
| generated C for `bench/speed/speed.kp` | 532 lines |
| libc functions a built program calls | 11: `exit` `fprintf` `free` `malloc` `memcpy` `memset` `printf` `realloc` `strcmp` `strerror` `strlen` |
| C runtime text emitted into every program | 813 lines (`RUNTIME` 507 + `PATH_RUNTIME` 55 + `CAP_RUNTIME` 251) |
| `externs.rs` (C bodies of the stdlib externs) | 496 lines |
| `extern fun` declarations in `kelpc/std/*.kp` | 90 |
| C emitter being replaced | `codegen_c.rs`, 3745 lines |
| lowering surface the backend consumes | `tir.rs`, 286 lines, ~25 node kinds |
| `Ty` variants the backend must handle exhaustively | 20+ |
| tests at HEAD of `main` | 310 (80 unit + 230 integration), 30 s |

Two corrections to the record while writing this down: `roadmap.md` says "the 118 end-to-end tests" and adds
"~20 one-line externs". Both are stale. The integration suite is **230** tests, and the stdlib declares **90**
`extern fun`s. The larger numbers make Stage 4 bigger than MK7's own text implies, and this plan sizes it as
90.

## 3. Cost model

The trade the roadmap already states, restated because every stage decision follows from it:

| workload | `-O0` | `-O2` | penalty |
| --- | --- | --- | --- |
| `collatz` (branchy int) | 446 ms | 83 ms | 5.4x |
| `float` (div + add loop) | 265 ms | 101 ms | 2.6x |
| `mix` (div-bound loop) | 204 ms | 168 ms | 1.2x |
| `fib` (real calls) | 28.4 ms | 24.0 ms | 1.2x |

A backend with a competent register allocator and **no** optimizer lands between these two columns. Call-heavy
code barely notices; branchy and floating-point code notices a lot. Closing the remainder means inlining, CSE,
LICM and strength reduction, which is the multi-year part and is **not** on this branch. The gate is therefore
"no worse than the `-O0` column", not "as good as `-O2`".

`bench/speed/` already carries the cross-language checksums that make that comparison meaningful, and it is
where every stage gate runs.

---

## 4. Stage 0 — backend boundary

**The one commit that costs nothing and every later stage depends on.** Do it alone, one agent, before any
fan-out. Today `main.rs:72` calls `codegen_c::generate` directly and `driver.rs` (43 lines) is the only code
that shells out; there is no seam for a second backend to exist in, which means two agents would otherwise
collide in `main.rs` and `Cargo.toml` on day one.

Deliverables:

- **A backend seam.** `codegen_c.rs` becomes one implementation behind a trait or a plain enum dispatch.
  Start with the enum; a trait earns its place only when a third backend exists. Keep the interface at
  `(typed program, drop plan, source map) -> artifact`, which is exactly what `codegen_c::generate` already
  takes.
- **`--backend=c|asm|own`** on `kelpc build` and `kelpc run`, plus a `KELPC_BACKEND` environment override.
  `c` stays the default for the whole of this branch. An unknown value is a hard error naming the valid set.
- **A double run of the fixture suite.** `cargo test` runs the 230 integration tests once per backend selected
  by the same env var, so a third CI job (or one job looping over backends) proves both. This is `roadmap.md`
  open item 26, and it is the thing that makes every later stage verifiable at all.
- **The seam documented in `driver.rs`.** `driver.rs` keeps owning "write the artifact and invoke the external
  tool that finishes it"; at Stage 0 that is still `cc`.

Definition of done: `cargo test` green on `main`, `--backend=c` and `KELPC_BACKEND=c` both select the C
backend, an unknown backend is rejected by name, and the integration suite can be run twice in one command
with the backend as the only variable.

## 5. Stage 1 — x86-64, assembly text, `as` + `ld`

The first working own backend. Emits **x86-64 assembly text** and shells out to `as` and `ld` instead of `cc`.
This drops the C compiler from the dependency chain while keeping libc, and it is deliberately the tier with
the best value-per-line: the assembler does instruction encoding, so the hand-written part is lowering,
register allocation and ABI.

Scope, in build order. Each bullet is separately testable against the fixture suite.

1. **Scalars.** `i32`/`i64`/`u32`/`u64`/`u8`/`u16`/`usize`/`isize`/`bool`/`char`, integer arithmetic,
   comparisons, `while`, `if`/`else`, `return`, function calls, and the `-fwrapv` contract expressed as
   explicit wrapping adds. `char` is `uint32_t` (a codepoint, not a byte) and must not be confused with `u8`.
2. **f32/f64.** SSE2 scalar ops in `xmm`, including the SysV rule that a call's vector-register usage is
   independent of the integer argument registers.
3. **A register allocator.** Not necessarily a good one: a linear-scan allocator over the TIR block structure,
   falling back to spilling. This is the piece that decides the `-O0`-vs-`-O2` position, and it is better to
   write it once properly than to ship a stack machine and rewrite it at Stage 2.
4. **Aggregates by value.** `datac`, `enum` (tagged union), `String`, `Bytes`, `View`, `List<T>`, `Map<K, V>`,
   `Box<T>`. This is the ABI work, and it is where the silent miscompiles live: SysV classifies each aggregate
   field by field into `INTEGER`/`SSE`/`MEMORY`, passes eightbyte chunks in registers where it can, and puts
   anything over 16 bytes on the stack. Layout must match `codegen_c.rs`'s `typedef`s exactly or the two
   backends will not agree on a `List`'s field offsets.
5. **Drop and ownership.** `borrowck.rs` already hands over a `DropPlan` keyed by span, so the backend does
   not re-derive where drops go, only what a drop *is* in machine terms: call the generated drop function,
   free the buffer, clear the pointer so a drop is idempotent at runtime as the C runtime already is.
6. **Closures and indirect calls.** `FnRef`, `ClosureLit`, `Capture`, `FnToClosure`, `IndirectCall`. The
   environment pointer is a hidden parameter, so the SysV classification applies to it too, and the generated
   thunk has to be emitted per signature exactly as `codegen_c.rs::write_closure` does.
7. **Capabilities.** `resolve.rs` compiles each grant to constant data and `codegen_c.rs` emits
   `kelp_cap_check_*` at each gated call site. The matcher and path normalizer must be **byte-exact**
   equivalents or every capability fixture changes meaning, and getting this subtly wrong is a security
   regression, not a test failure. Port them as assembly-called libc-backed routines at this stage (they are
   part of the 813 runtime lines), and keep the C versions as the reference for a differential test of the
   matcher itself over a generated pattern corpus.
8. **`print`/`print!`.** One `printf` with one generated format string, as today, including the x86-64 detail
   that a variadic call must set `AL` to the number of vector registers used.

Definition of done: all 248 fixtures pass under `--backend=asm` with byte-identical stdout to `--backend=c`;
`bench/speed/` runs all four workloads with matching checksums and is **no worse than the `-O0` column**;
`kelpc build` on `hello.kp` is comfortably under 40 ms (the `as`+`ld` share only); `cc` is not invoked.

## 6. Stage 2 — machine-code bytes and our own linker

Replaces `as` and `ld` with an x86-64 encoder and an ELF writer. This is the tier that removes the last
external *tool* while still linking libc.

1. **Instruction encoder.** The subset Stage 1 actually emits, as tables rather than as a general x86-64
   model: ModRM/SIB/displacement forms, REX prefixes for the four register widths and the extended registers,
   immediates, `mov`/`lea`/`add`/`sub`/`imul`/`idiv`/`and`/`or`/`xor`/`shl`/`sar`/`cmp`/`test`, `setcc`,
   `jmp`/`jcc`, `call`/`ret`, `movsx`/`movzx`, and the SSE2 scalar forms. A disassembler is worth writing
   beside it: round-tripping the encoder through `objdump` during development is cheaper than debugging
   wrong bytes in `gdb`.
2. **Object emission.** ELF64 relocatable: sections, symbols, relocations. Only the relocation types the
   emitted code needs (`R_X86_64_PC32` for local calls, `R_X86_64_PLT32` for libc calls, `R_X86_64_64` for
   the capability tables and string literals).
3. **Own linker.** Static layout, or dynamic linking against libc, which means `PT_INTERP`, `PT_DYNAMIC`,
   `DT_NEEDED`, a GOT and PLT stubs. Dynamic is the cheaper honest path at this stage because it keeps all
   90 externs working unchanged; static libc means owning `crt1.o`-equivalent startup, which drags Tier 3
   forward. Pick dynamic.
4. **Differential gate.** Same as Stage 1, plus `readelf`/`objdump` inspection of the produced ELF in the test
   suite so a malformed binary fails the build rather than crashing at exec.

Definition of done: all fixtures pass under `--backend=own` with stdout identical to the C backend, produced by
an ELF written by `kelpc` with no assembler and no linker invoked, and `bench/speed/` still no worse than `-O0`.

## 7. Stage 3 — freestanding

Removes libc. This is `backlog.md` B-6 (freestanding/no-runtime) plus B-7 (own allocator) plus `roadmap.md`
MK9-S4, and it is larger than Stages 1 and 2 combined. It is also not a detour: those three items are
prerequisites for kernel, driver, firmware and embedded targets, so the work is bought once for two goals.

Tier 3 is mostly a **runtime** project, not a codegen project, which is exactly why Section 8 makes it a
parallel track.

1. **Raw syscall layer.** `write`, `read`, `open`/`openat`, `close`, `lseek`, `stat`/`fstat`, `mmap`/`munmap`,
   `brk`, `exit`, `clock_gettime`, `nanosleep`, `getpid`, `fork`/`execve`/`wait4`, `pipe2`, `dup2`, `chdir`,
   `mkdir`, `unlink`, `rename`, `symlink`, `access`, `getcwd`, `getdents64`, `socket`/`connect`/`bind`/
   `listen`/`accept4`/`sendto`/`recvfrom`, `poll`. Roughly the ~40 distinct libc calls this repo uses today,
   each with the x86-64 `syscall` instruction and the errno return convention.
2. **Allocator.** `malloc`/`realloc`/`free` over `mmap` or `brk`, with the contract the runtime already
   assumes (a `Bytes` allocation is `cap + 1` bytes with `data[len] == 0`). B-7's fallible allocation is
   **not** on this branch: `try_alloc` changes the language's surface and belongs to MK9-S5-M1. Panic on OOM
   stays.
3. **Formatter.** `printf`'s responsibilities: integer, hex, string, padding, and **float formatting**. Float
   is the expensive part because shortest-round-trip is a real algorithm (Ryū/Grisu-shaped), not a loop.
   Plan for it explicitly; a naive implementation is a correctness bug that the `float` bench checksum will
   catch, and it is also the piece most likely to be underestimated.
4. **Error text.** `strerror`'s table, because `kelp_panic_errno` prints it and fixtures assert on it
   (open item 47).
5. **Path and capability runtime.** The 55-line path normalizer and 251-line capability matcher, now in Kelp
   or hand-written machine code rather than C text. Security-relevant: `..` rejection, `realpath`-with-a-
   missing-tail, glob matching, host/port splitting. This deserves its own pattern corpus and its own
   differential test against the C implementation before the C one is retired.
6. **DNS.** The largest single non-codegen item. `getaddrinfo` is free today and `std/net`/`std/http` depend
   on it, so a libc-free Kelp needs a stub resolver: `/etc/hosts`, `/etc/resolv.conf`, UDP query and response
   parsing, `A`/`AAAA`, name compression, truncation and TCP fallback, timeouts and retries. `https` is also
   heading this way (open item 37), so the resolver is not a dead end — but it is a project, and it is the
   item most likely to be deferred by a week and then by a month.
7. **Own startup and runtime.** `_start`, `atexit`-equivalent for owned `pd` drops, `main(argc, argv)`
   equivalent, `kelp_panic` → `exit(1)`, and a static ELF with no interpreter.
8. **Losses to accept and record.** ASAN goes away with the C toolchain, so every memory-safety claim in the
   verification section must be re-argued against Kelp's own semantics rather than demonstrated by clang
   (`backlog.md`'s cross-cutting note). glibc's hand-tuned SIMD `memcpy`/`strlen`/`memmove` are gone until B-9
   SIMD work exists, so text-heavy code will be slower. Debug info (DWARF, open item A-16) is absent unless
   built, which means `gdb` against a generated binary is not useful until then.

Definition of done: all 248 fixtures pass with no libc linked and no interpreter in the ELF; `ldd` reports
nothing or is not needed; `bench/speed/` checksums match and the runtime is no worse than `-O0`;
`kelpc build` is under 15 ms; and a capability fixture denied by a grant is denied with the same message.

---

## 8. Parallel track: runtime decoupling

The important structural observation of this plan: **most of Tier 3 is backend-independent.** The allocator,
the formatter, the error table, the path normalizer, the capability matcher and the DNS resolver are the same
code whether the backend emits assembly text, machine code, or nothing at all. They can be built and tested
against the C backend, where a working reference implementation already exists.

So the runtime track starts **now**, in parallel with Stage 1, and it has its own contract:

- Build each runtime piece against the C backend first, replacing the corresponding C text in
  `codegen_c.rs`/`externs.rs` with a Kelp-expressible or hand-written implementation, and prove it with the
  existing fixtures plus a differential test against the C version kept alongside.
- Order by risk, not by tidiness: capability matcher and path normalizer first (security-relevant, byte-exact,
  own corpus), then the allocator, then the formatter (float last, because it is the hardest), then DNS last.
- Keep `-fwrapv`'s promise visible: integer wrap is a Kelp semantic at every layer, not a flag.

This changes the shape of the project: Tier 3 stops being "finish the backend, then start over on the
runtime" and becomes "the runtime is already libc-free by the time the backend can emit a syscall".

---

## 9. Two-agent split

Two agents work on this in parallel, in **separate git worktrees**. Same working tree is not an option: both
would edit `main.rs`, `Cargo.toml` and `infer.rs` inside the first hour, and Rust's exhaustiveness checks make
a shared `types.rs` a hard compile break rather than a merge conflict.

**Agent A — surface and libraries** (the existing language track, unchanged):
owns `kelpc/std/*.kp`, `lexer.rs`, `parser.rs`, `plan/syntax.md`, and new `plan/*.md` files.

**Agent B — backend and runtime** (this branch):
owns `tir.rs`, `types.rs`, `driver.rs`, `main.rs`, `Cargo.toml`, `codegen_c.rs`, and the new backend and
runtime directories.

**The one interface rule that makes the split safe.** `types.rs` and `tir.rs` have exactly one writer: B.
When A's work needs a new `Ty` variant or TIR node, A does **not** add it. A files the request, B adds it in a
single small commit (variant + C backend arm + new backend arm), and both worktrees rebase onto it. This is
the whole reason the split holds: the exhaustively-matched files are single-owner, and the additions that
would break them are serialized through one agent.

**Sequencing.** Stage 0 must land first, alone, one agent, before the fan-out — it is the only commit both
tracks must touch (`main.rs`, `Cargo.toml`). After that the tracks barely intersect.

**Merge order**, once parallel work exists: Stage 0 → A's stdlib-only work (no new `Ty`) → B's Stage 1 →
A's type-adding syntax → B's Stage 2 → B's Stage 3.

**Docs.** One writer per file, always. `roadmap.md` and `backlog.md` belong to B on this branch; A writes
`syntax.md` and new files only. Long single-line bullets in these files conflict on every rebase otherwise.

## 10. Risk register

| risk | why it bites | mitigation |
| --- | --- | --- |
| Aggregate ABI mismatch | Kelp passes `datac`/`enum`/`String`/`List`/`Map`/`Box` by value; a wrong eightbyte split is a silent miscompile | differential stdout on all 248 fixtures; assert struct offsets against the C typedefs in a test |
| Capability matcher drift | byte-exact security behavior; a lenient port widens every grant | own pattern corpus + differential test against the C implementation before retiring it |
| Float formatting | shortest-round-trip is an algorithm, not a loop | last in the runtime order; `bench/speed` `float` checksum is the gate |
| DNS underestimation | `getaddrinfo` looks like a call and is a subsystem | schedule it explicitly, expect TCP fallback and compression to be most of it |
| No optimizer, permanently | 5.4x on branchy int code is the accepted cost, not a bug to fix mid-branch | gate on "no worse than `-O0`"; optimizer stays off this branch |
| Loss of ASAN | safety claims were demonstrated through clang | re-argue against Kelp semantics; keep the C backend for differential runs |
| Two agents, one tree | guaranteed conflicts in `main.rs`/`Cargo.toml`/`infer.rs` | worktrees; single-owner rule for `types.rs`/`tir.rs` |
| Tier 2 linker scope creep | dynamic-linking details multiply; static libc drags Tier 3 in early | pick dynamic linking explicitly; static ELF is Tier 3's endpoint, not Tier 2's |

## 11. Not on this branch

Recorded so the branch stays honest about its own size.

- LLVM, Cranelift, or any codegen library. `roadmap.md` MK7 stage 2 is **replaced** by a hand-written
  x86-64 backend; that is the whole cost of the "no LLVM no nothing" reading.
- An optimizer: inlining, CSE, LICM, strength reduction. Between `-O0` and `-O2` is the target.
- Any target other than x86-64 Linux, and any cross-compilation.
- DWARF and source mapping (`backlog.md` A-16), though the `#line` gap this branch inherits stops being
  unfixable the moment a real backend exists.
- `unsafe`, raw pointers, user FFI, `repr(C)` (B-1/B-2/B-3), and threads (MK6-S1-M1). All are systems work
  that a real backend *enables*; none of them are on the path to a real backend.
- Fallible allocation (B-7's `try_alloc`): `malloc` failing stays a panic on this branch.
