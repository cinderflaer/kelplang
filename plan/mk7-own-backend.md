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

## 0. Where the branch is

*Updated as work lands. This table is the answer to "what is done and what is left"; every section below is
the detail behind one row. Branch `mk7/own-backend`, and the numbers are measured on this machine, not
projected.*

| stage | state | evidence |
| --- | --- | --- |
| **0. Backend boundary** | **done** | `backend.rs` + `--backend=`/`KELPC_BACKEND` + per-backend suite + `run-all-backends.sh`; commit `68746a6` |
| **1. x86-64, assembly text, `as`+`ld`** | **in progress — scalars, f32/f64, a register allocator, four slices of aggregates, then item 8's `str`/float printing and the first hand-written runtime routine** | `codegen_asm.rs`; **126 of 247** integration tests pass under `--backend=asm`; printing a `str` and an `f32`/`f64`, and owning `kelp_panic` instead of calling the C runtime's, is the newest slice |
| **2. Machine code + own linker** | not started | tier 1 must pass the suite first (§1.4) |
| **3. Freestanding (no libc)** | not started | blocked on tier 2 |
| **8. Runtime decoupling (parallel)** | **in progress — the matcher and the normalizer are Kelp** | `std/rt.kp` + two differential corpora; commit `1071ead`; the allocator is blocked on raw pointers (§8's note below) |
| **9. Two-agent split** | not started | Stage 0 landed, so the tracks can now diverge |

The one number that matters most: **126 of 247 tests pass on the asm backend, and the C backend passes all
247.** The C backend is the oracle, so that ratio is a measurement of coverage rather than a claim about
correctness — every fixture the asm backend builds is compared against what the C backend builds from the same
source (§1.1). The register allocator (item 3) moved this by exactly one: it is a performance change, not a
coverage one, and the one new fixture it added (`nested_while_registers.kp`) is a regression test for the
allocator's own loop-safety correction, not a previously-uncovered language feature. Item 4's first slice moved
it by six: `datac_basic`, `borrow_basic` (both shared and exclusive borrows of one, plus mutation through
the exclusive one), `field_read_does_not_move`, `move_then_reinit`, and two module-resolution fixtures that
happen to declare a `datac` on the way to what they are actually testing. Item 4's second slice moved it by
exactly one more: `a_datac_can_embed_an_earlier_datac`, over `datac_embeds_datac.kp`. Item 4's third slice
(below) added two new fixtures and unblocked both of them: `a_two_eightbyte_datac_passes_and_returns_in_a_register_pair`
and `a_memory_class_datac_returns_through_a_hidden_pointer`, over `datac_two_eightbytes.kp` and
`datac_memory_class.kp` — which is also why the denominator moved from 242 to 244 rather than staying fixed:
these are new fixtures this slice's own scope needed to exist at all, not previously-uncovered ones an existing
fixture already exercised. Item 4's fourth slice moved it by four: one new fixture for each of its two halves
(`each_borrowed_temporary_gets_a_slot_of_its_own` over `borrow_temp_slots.kp`, and
`a_memory_class_datac_passes_by_value_in_the_argument_area` over `datac_memory_class_by_value.kp`), plus two
fixtures it unblocked — `a_shared_borrow_of_a_temporary_is_allowed_for_a_call` (`borrow_temp_arg.kp`, refused
since item 4's first slice) and `import_alias_reference_runs`, whose `bumped(t: Tally, by: i32)` had two live
parameters silently sharing one register (see that slice's blockquote for both bugs). Item 8's first slice moved
it by eleven: one new fixture (`printing_a_string_or_a_float_uses_the_right_register_file`, over
`asm_float_print.kp` — the only place the *mixed* register assignment a variadic `printf` needs is pinned) and ten
fixtures it unblocked: `aliases`, `an_alias_from_an_imported_module_is_visible` and `hello_prints_greeting`
(printing a `str`), `const_declarations_fold_at_compile_time`,
`integer_to_float_conversions_widen_and_round` and `a_non_finite_float_literal_emits_a_math_h_constant`
(printing a float), `when_desugars_to_if_else_chain` (a function returning `&str`), and
`integer_conversions_widen_exactly_and_narrow_with_a_check`,
`a_panicking_tail_satisfies_any_return_type` and `panic_reports_its_message_and_assert_reports_failure`
(the numeric-conversion range check, `panic` and `assert` — all three through the hand-written `kelp_panic`).

**How much of the whole plan is done, stated the same way the rest of this file measures things — in what
landed, not a percentage that would imply more precision than the plan has:**

- **Stage 0** (backend boundary): done.
- **Stage 1** (x86-64 assembly): 3 of its own 8 build-order items (§5) are done outright — scalars, `f32`/`f64`,
  the register allocator — plus a 4th (aggregates) with its first four slices landed and the first half of item 8
  (`print`/`print!` of `str` and the floats). That is where the 126/247 fixture number comes from. The remaining
  part of item 4 (`enum`, plus a `datac` holding a float or heap field — see the fourth slice's blockquote for
  what "by value" now covers end to end), the second half of item 8 (printing an owned buffer, which needs the
  heap types rather than the printing path), plus items 5-7 (drop/ownership, closures, capabilities), are what
  stands between here and Stage 1's own definition of done: all 247 fixtures passing, `bench/speed` no worse
  than `-O0`, and a memory gate for the asm backend.
- **Stage 2** (machine code + own linker) and **Stage 3** (freestanding): not started; both are blocked on
  Stage 1 finishing first, by this plan's own tiering rule (§1.4).
- **§8** (the parallel runtime track): 2 of its pieces are done (the capability matcher, the path normalizer);
  the allocator is blocked on raw pointers (not on this branch — see §8's note below) and the formatter and DNS
  resolver have not been started.
- **§9** (the two-agent split): not started — a decision to make, not a prerequisite, since Stage 0 already
  unblocked it.

So: **one of four backend stages done, the second about halfway through its own item list, the third and
fourth not begun**, plus the runtime track's two lowest-risk pieces landed and its costliest piece (DNS) not
touched. Read the fixture ratio (126/247) as the honest single number if one number is wanted; it already
accounts for every one of these except the two things it structurally cannot measure — `bench/speed` and the
memory gate — which is why both are called out separately above and in §5's definition of done.

**What is next, in the order the refusals rank themselves** (aggregated across the fixtures the C backend
builds, and detailed in §5):

1. **The runtime** — 40 fixtures are blocked by an `extern fun` at all, 38 of them by `std.mem.alloc_raw`
   alone. This is the §8 track arriving on schedule: the biggest single unlock is not the code generator. Its
   first two pieces — the capability matcher and the path normalizer — landed in `1071ead` and unblock no
   fixture by themselves. **Correction (2026-09-19): the allocator does not unblock these 40 either, on its
   own.** `codegen_asm.rs::generate` refuses any program containing *any* `extern fun` before codegen starts —
   it is not specific to `alloc_raw` — so unblocking these fixtures needs the asm backend to call a hand-written
   assembly routine for allocation (PLT-called, the way it presumably will for other libc calls), which in
   turn needs aggregate-by-value return (§5 item 4, `Bytes` is a struct) landed first. That prerequisite is now
   met: item 4's third and fourth slices cover a `datac` returned through the hidden pointer, and one passed or
   received by value in the argument area. The §8 allocator item is further blocked on its own terms — see §8's
   note below — so it is **not** next; see the corrected order there.
2. **Aggregates, the rest of them** — the four slices of item 4 have landed eleven fixtures (six from the
   first slice, one from the second — a `datac` nested by value inside another qualifying `datac` — two new
   fixtures the third slice added and unblocked itself, and two more the fourth did). The rest are refused
   either for `enum` (16 of them — tagged unions, not attempted at all) or for a `datac` holding a float or
   heap field anywhere in its fields (2 float, and the heap ones, which §5's table counts under its heap row
   because a build stops at the first refusal it hits) — refused regardless of size, since the third slice's
   eightbyte/`MEMORY` classification covers *size*, not a float-classified eightbyte or a heap-typed field.
   The two gaps the third slice named as its own deliberate narrowings — a `Mem`-class `datac` passed or
   received *by value*, and a borrow of a temporary — are both closed; see the fourth slice's blockquote. The
   ABI work the risk register calls the place silent miscompiles live is now complete for every shape this
   backend admits at all.
3. **A passdown (`pd`)** — 8 fixtures, and a row the earlier versions of this ranking did not name: the build
   refuses a program containing a `pd` outright, before codegen, the same way it refuses an `extern fun`. It is
   not in §5's item list at all, so it is not "item 9" so much as a self-contained piece of codegen. Item 8's
   first slice made this row *cheaper without unblocking it*: `passdowns`, `pd_shadowed` and `pd_var_write` were
   each also waiting on printing a `str` passdown, so with `str` printing landed they now need only the `pd`
   machinery — which is a file-scope global plus, in the C backend, a hidden argument. That makes this the
   cheapest remaining row by some way: five or six fixtures for one self-contained mechanism, none of which needs
   the heap.
4. **`List`/`Map`** — 9 fixtures, and they need both the runtime and aggregates.
5. **String values** — now a heap row rather than a printing one: `str` values themselves work, and what remains
   is the owned `String`/`Bytes`/`View` types and their drops.
6. **Capability checks** — 7 fixtures, currently *refused* rather than emitted unchecked (§5, and the only
   row here where a gap would be a security hole rather than a wrong number).
7. **The long tail** — numeric conversions, printing `str`/`f64` and closures: the first two are gone as of item
   8's first slice, and closures are what is left (~1 fixture plus the `FnRef`/`ClosureLit` machinery §5 item 6
   describes).

Those rows are re-measured against the current backend and rank rather than partition: a fixture counts once,
under the *first* refusal its build hits, which is why the heap-type row below is larger than the container
feature it names (`str` is counted separately, but a `String` that a `str` argument is converted into is
counted with the heap row). The totals are in §5's table.

Two things §5's definition of done still requires that no amount of the above delivers: `bench/speed` no worse
than the measured `-O0` column (it has not been run on the asm backend yet), and a memory gate that works for
the asm backend, since the AddressSanitizer gate instruments the generated C and is therefore C-backend-only.

**Environmental prerequisite:** `driver.rs` invokes `as` and `ld` by name, so `binutils` is a real dependency
now rather than something clang pulls in. Debugging hits (gdb, strace, valgrind) are in `/etc/nixos/tools.nix`.

---

## 1. Ground rules

These hold for every stage. Breaking one is how this becomes a rewrite instead of a replacement.

1. **`codegen_c.rs` is never deleted.** It becomes the permanent differential oracle: every fixture is
   compiled and run by both backends, and their stdout is compared. A backend is correct when it agrees with
   the C backend, not when it agrees with the author's intent.
2. **The 260 fixture files are the contract.** They are backend-agnostic by construction (compile, run,
   compare stdout), so they validate a second backend with no edits. Any fixture that has to change for the
   new backend is a bug in one of the two, and the diff says which. The suite is 247 test functions over
   those files — several tests cover more than one fixture, and a fixture that must be *rejected* is a file
   with no test of its own — so "the fixtures pass" is measured as **247 integration tests**, which is the
   number the stage gates below use.
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
| tests at HEAD of `main` | 323 (88 unit + 235 integration), 29 s |

Two corrections to the record while writing this down: `roadmap.md` says "the 118 end-to-end tests" and adds
"~20 one-line externs". Both are stale. The integration suite is **247** tests over **260** fixture files
(the table's 235/248 are this baseline's own date — 2026-09-18 — and every slice since has added fixtures a
new backend needed rather than found an existing one that already covered them; one test can cover more than
one fixture, and rejected fixtures are files without a test of their own), and
the stdlib declares **90**
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

> **Status: done** (commit `68746a6`). `backend.rs` owns the selector, `--backend=` and `KELPC_BACKEND` both
> work with the flag winning, the planned names are refused as planned rather than as unknown, the suite runs
> once per backend via `kelpc/tests/run-all-backends.sh`, and the AddressSanitizer gate reports itself as
> C-backend-only instead of silently skipping. Verified both ways: `BACKENDS="c asm"` passes 235/235 on `c`,
> and pointing the suite at a backend that does not exist fails 174 of its tests, which is what makes the
> mechanism a gate rather than a variable.

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
- **A double run of the fixture suite.** `cargo test` runs the 247 integration tests once per backend selected
  by the same env var, so a third CI job (or one job looping over backends) proves both. This is `roadmap.md`
  open item 26, and it is the thing that makes every later stage verifiable at all.
- **The seam documented in `driver.rs`.** `driver.rs` keeps owning "write the artifact and invoke the external
  tool that finishes it"; at Stage 0 that is still `cc`.

Definition of done: `cargo test` green on `main`, `--backend=c` and `KELPC_BACKEND=c` both select the C
backend, an unknown backend is rejected by name, and the integration suite can be run twice in one command
with the backend as the only variable.

## 5. Stage 1 — x86-64, assembly text, `as` + `ld`

> **Status: first slice landed, then f32/f64, then a register allocator (item
> 3), then four slices of item 4's aggregates, then the first half of item 8.**
> `kelpc build --backend=asm` emits x86-64 assembly itself and finishes with `as`
> and `ld`, with no C compiler in the chain. Measured: **126 of 247 end-to-end
> tests pass**, and the C backend passes all 247 — so the differential gate is
> live (`the_two_backends_agree_on_the_scalar_fixtures`) and the coverage number
> can only rise from here.
>
> Covered: every integer width, `bool`, `char`, `f32`/`f64` arithmetic and
> comparisons (`ucomiss`/`ucomisd`, including the unordered/NaN case — see
> `tests/fixtures/asm_float_scalars.kp`), all unary and binary operators
> including short-circuiting `&&`/`||`, `if`/`else` in statement and value
> position, `while`, assignment, calls with up to six integer *and* eight float
> arguments classified independently per SysV, recursion, and `print`/`print!`
> of integers, characters, booleans, strings and floats — the last two as of
> item 8's first slice, whose blockquote records what printing a float turned
> out to actually require.
>
> **Item 3, the register allocator, is done for integer/`bool`/`char` locals
> (2026-09-19).** A linear-scan pass (`allocate_registers` in
> `codegen_asm.rs`) runs once per function, before a single instruction of its
> body is emitted, over five callee-saved registers (`rbx`, `r12`-`r15`): a
> local whose live range fits gets pinned to one of them for the whole
> function instead of a frame slot, turning every read and write of it into a
> register-to-register `mov` with no memory traffic, and — because the
> registers are callee-saved — no reloading across a call either. `tir.rs` has
> no CFG or SSA form, so this is not general liveness analysis: it assigns
> every statement and expression a strictly increasing position in one
> preorder walk and takes each local's naive interval as
> `[first touch, last touch]` in that numbering, which is already correct for
> anything whose scope does not cross a loop (the loop's positions sit inside
> the interval by construction). The one place that is not automatically
> correct is a local touched *inside* a `while` and needed again next
> iteration — its last textual touch can sit early in the loop body, and a
> naive interval would let the register be handed to something else for the
> rest of the body. `LiveAnalysis` corrects for exactly this: any local
> declared **outside** a loop whose interval already overlaps the loop's
> `[condition, body]` span is widened to cover the span in full, applied
> smallest-span-first (innermost first, for a properly nested AST) so a
> widened inner interval can trigger an enclosing loop's own widening in the
> same pass. A local declared *inside* the loop is exempt — codegen visits its
> `Let` once regardless of how many times the loop runs at runtime, so it gets
> one physical location reused every iteration either way. Nothing not
> confident about this gets a register: an allocation gap is a performance bug,
> an unsafe one would be a correctness bug, so the scan falls back to a spill
> (the same stack slot every local used before this item) whenever the pool of
> five is exhausted. `tests/fixtures/nested_while_registers.kp` pins the
> loop-widening correction directly (an accumulator live across an outer loop,
> read and written inside an inner one nested in it).
>
> Everything else about item 3's scope stays deliberately narrow: `f32`/`f64`
> locals are excluded entirely (xmm live-range allocation is separate work,
> not attempted here — they keep item 2's memory-backed, `xmm0`-only
> discipline), and expression evaluation itself is untouched — `rax`/`rcx` and
> `xmm0`/`xmm1` still spill the left operand across the right's evaluation
> exactly as before. Measured on a memory-traffic-bound accumulator loop
> (`total`/`i` both register-allocated, no division), this item is an 18%
> wall-clock improvement (1990ms → 1630ms over 2×10⁹ iterations) with zero
> fixture regressions; on an idiv-bound workload (`collatz`-shaped, division
> latency dominating each step) the improvement is unmeasurable, which is
> division latency hiding a real effect rather than the allocator doing
> nothing — the generated assembly confirms all five pool registers hold
> live locals with zero spills in both cases. One correctness bug the
> allocator's own regression pass caught before either benchmark ran: the
> call-site `rsp` save (`SP_SLOT`) was a fixed `[rbp-8]`, which used to be
> safe because nothing else lived there — once a function's used pool
> registers are pushed *between* `mov rbp, rsp` and the frame slots, they land
> at exactly `[rbp-8]`, `[rbp-16]`, and so on, so the fixed save aliased the
> first pushed register and a call inside a function using one silently
> overwrote a live callee-saved register's saved value with a stack pointer
> (caught by `fib_computes_correctly`, a segfault, because `fib`'s parameter
> lives in a pushed register across its own recursive calls). The slot is now
> computed per function as `-8 * (pushed registers + 1)`, below every pushed
> register, and threaded through `Frame` instead of being a global constant.
>
> **Item 4's first slice — `datac` by value, ≤8 bytes, all-scalar fields — is
> done (2026-09-19).** The scope is deliberately narrow: a `datac` qualifies
> only if every field is `int_ty`-representable (no float field, no nested
> `datac`, no `str`/`String`/`List`/`Map`/`Box` field) and its C-equivalent
> layout — sequential fields at natural alignment, exactly what `codegen_c.rs`'s
> typedef gets from clang — fits in eight bytes (`build_layouts` in
> `codegen_asm.rs`; every fixture in the sample set, `Pair`/`Point`/`Counter`,
> is two `i32`s). A qualifying value needs no SysV eightbyte classification at
> all: it *is* one eightbyte, so it moves through exactly the paths a `u64`
> already used — a `Let`, an `Assign`, a call argument, a return value, even a
> pool register (`value_kind` widens `int_ty`'s notion of "a value this slice
> moves whole" to include one). Only a *field* read or write needs the layout,
> to find the byte offset and width inside those eight bytes — `struct_lit`
> builds one field by field into a single reused scratch slot (`Frame::agg_slot`)
> then loads the whole eightbyte into `rax`, and `field_read`/`field_assign` go
> through `addr_of_struct`, which computes the object's address into `r11` (a
> register otherwise unused in this module, and — unlike its brief, buggy stint
> as the call-site stack save — never live across a call here, only from one
> instruction to the next) for three shapes: a local `datac`, a local
> `&datac`/`&mut datac` (the common case — a `datac` parameter that is not
> `move` desugars to a reference, so `total(c)`/`bump(c)` inside `fun total(c:
> Counter)` are this, not a direct value), and an explicit `(*r).field`.
> `ExprKind::Ref` is supported for exactly one shape: a named `datac` local —
> `&c` and the implicit borrow `infer.rs` inserts at a call site both compile
> to a `lea` of that local's own frame slot.
>
> **Deliberately still refused, and named so it stays a visible gap rather
> than a silent one:** a `datac` over eight bytes, or holding a float or heap
> field (real SysV eightbyte/`MEMORY`-class classification is unwritten); every
> `enum` (not attempted); and a borrow of a *temporary* — `sum(Pair(a = 1, b =
> 2))` still fails, because `struct_lit` builds into one reused scratch slot,
> and a pointer into it would dangle the moment a second struct literal in the
> same call reused it before the callee read the first. `tests/fixtures/borrow_temp_arg.kp`
> is exactly this shape and stays uncovered until a temporary gets its own
> slot rather than a shared one.
>
> **Item 4's second slice — a `datac` nested by value inside another
> qualifying `datac` — is done (2026-09-19).** `Outer(inner: Inner, n: i32)`
> where `Inner` is itself a qualifying (all-scalar, ≤8-byte) `datac` now
> builds and reads back correctly, as long as `Outer`'s own total size is
> still ≤8 bytes. `build_layouts` runs to a fixed point over the whole struct
> list (bounded by `structs.len()` passes) rather than in one pass, since a
> struct's own qualification can now depend on another struct's — declared
> before or after it in source, either way — and `StructLayout` gained a
> second kind of field entry (`FieldEntry::Nested`, alongside the existing
> `FieldEntry::Scalar`) recording a nested field's own byte offset and struct
> name rather than a width. A nested struct is only accepted if its own size
> is *exactly* one of the four widths this backend can `mov` in one
> instruction (1/2/4/8 bytes, `width_for_size`) — not merely ≤8 — because
> unlike a top-level qualifying struct (always moved whole, as one opaque
> `QWORD`, regardless of its real size) a nested one is copied into its
> parent's slot at its own precise width, and a struct whose fields leave it,
> say, 3 bytes wide would either under-copy or spill into the next field's
> bytes. `addr_of_struct` gained a fourth shape to resolve a `base` expression
> through: `ExprKind::FieldAccess` itself, so a chain like `o.inner.v`
> recurses — computing `o`'s own address first, then stepping `r11` forward by
> `inner`'s offset within `Outer` (an `add`, not a second `lea`, since the
> nested value lives inside the outer one rather than behind its own
> pointer) — before `field_read`/`field_assign` resolve the final scalar leaf
> exactly as they already did. `tests/fixtures/datac_embeds_datac.kp` is the
> fixture this unblocks: `Outer(inner = Inner(v = 7), n = 1)`, printing
> `o.inner.v + o.n`.
>
> **Item 4's third slice — real SysV eightbyte/`MEMORY` classification for a
> `datac` over eight bytes — is done (2026-09-19).** The first two slices
> treated every qualifying `datac` as one opaque eightbyte, moved through
> `rax` exactly like a `u64`; this slice adds the two SysV-mandated shapes a
> larger *top-level* value actually needs, both derived purely from the
> struct's own size (`AggClass`, `agg_class`) — sound only because a field
> this module accepts at all is always `int_ty`-representable or itself a
> qualifying nested `datac`, so unlike a general classifier there is no SSE
> eightbyte to consider; every eightbyte this module ever moves is `INTEGER`
> class. **9-16 bytes (`AggClass::Two`)**: two `INTEGER` eightbytes, passed
> and returned in a register pair — `rax:rdx` on return, two consecutive
> `ARG64` slots as a by-value parameter or call argument. **Over 16 bytes
> (`AggClass::Mem`)**: SysV `MEMORY` class, returned through the hidden
> first-argument-pointer convention (the caller passes `rdi` = the address to
> write the result into, before any real argument; the callee writes through
> it and the convention is that `rax` also holds that same pointer on return,
> though nothing here reads it back out).
>
> Two deliberate narrowings, both because the alternative was real added
> bookkeeping rather than a size-driven derivation like the above, and both
> named so they stay visible gaps: **a `Mem`-class value is not supported as
> a by-value parameter or call argument this slice** — only as a return value,
> a local, or by reference (`&big`, which already worked and costs nothing
> extra, since it is just a pointer regardless of what it points to). Passing
> one by value needs a caller-owned temporary copy whose address survives
> until the call executes — unlike a `Two`-class value, which needs no such
> copy, since it goes straight from wherever it is evaluated into a register
> pair. **No float eightbyte classification** — already true by construction
> (see above), so this is not a scope cut so much as a fact worth stating:
> nothing about this slice's classifier would extend to a `datac` with a
> float field even in principle, because `try_build_layout` never builds a
> layout for one in the first place (unchanged since the first slice).
>
> The internal model: every struct-typed value, regardless of class, still
> lives inline in its owner's stack frame (a local's own slot, sized
> `align_up(layout.size, 8)` by `Frame::declare` regardless of class) — only
> the boundary between one function and another (a parameter's prologue, a
> call's argument marshaling, a `return`) needs to know the class at all.
> `field_read`/`field_assign`/`addr_of_struct`/`Gen`'s `Ref` handling are
> **completely unchanged** by this slice: they already worked from a base
> address computed once, and a base address does not care how the value
> arrived at that address. What is new is `Dest` (a destination that is
> either a fixed frame address or a pointer stashed in a frame slot,
> reloaded fresh before each use rather than cached in a register across
> arbitrary codegen — a register is never guaranteed to survive a nested
> call, a frame slot always is), `emit_copy` (an arbitrary-byte-count copy
> through `rax`, in falling chunk widths, for moving a `Two`/`Mem`-class
> value between two named locations with no single instruction that could do
> it in one move), and `eval_struct_into` (the class-aware counterpart of
> `expr` for the four contexts that can hold a value wider than one
> eightbyte: a `let` initializer, an ident-targeted assignment, a
> `return`/tail value, and — from `call` — a `Two`-class outgoing argument).
> A struct-returning call reached through the *generic* `expr()` path (i.e.
> not through one of those four) is refused with a named message rather than
> silently keeping only the first eightbyte, the same refusal-over-silence
> discipline as everywhere else in this module.
>
> **A real bug this slice's own fixtures found, not something the slice
> introduces:** `LiveAnalysis::expr` (the register allocator's liveness pass,
> item 3) never walked into `ExprKind::StructLit` — a struct literal's field
> initializers are ordinary expressions and can read a plain scalar local or
> parameter (`Pair3(a = a, b = b)`), but nothing before this slice had ever
> written a fixture where a *variable* (as opposed to a literal) filled a
> struct-literal field, so the gap went uncaught since item 4's first slice.
> Without the fix, such a local looks dead immediately after its own
> declaration to `allocate_registers` (nothing after the `Let`/parameter
> binding ever calls `touch`), and the linear scan happily hands two
> genuinely-live locals the *same* physical register, believing their
> intervals never overlap — caught as a wrong first `print` (`4` instead of
> `3`) on the very first hand-written fixture for this slice, fixed by adding
> the missing match arm (`fields` walked, the literal's own name has nothing
> to `touch`).
>
> `tests/fixtures/datac_two_eightbytes.kp` (`Pair3`, two `i64` fields — 16
> bytes) exercises the `Two` path both ways: returned from `make3`, mutated
> through a field write, and passed by `move` to `sum3`, including a nested
> call (`sum3(make3(5, 6))`) used directly as a `Two`-class argument.
> `tests/fixtures/datac_memory_class.kp` (`Quad`, three `i64` fields — 24
> bytes) exercises the `Mem` path: returned from `make_quad`, field-read and
> field-written, reassigned wholesale, and passed *by reference* to
> `sum_quad` (the by-value-argument narrowing above is why `sum_quad` takes
> `Quad`, not `move Quad`).
>
> **Item 4's fourth slice — caller-owned storage for an aggregate temporary —
> is done (2026-09-19).** It closes both of the narrowings the third slice
> named, and it closes them with one mechanism, because they were the same
> missing thing: a `datac` whose *address* has to outlive the expression that
> produced it needs storage the code generator owns and does not reuse. The
> third slice said as much in its own refusal — passing a `Mem`-class value by
> value "needs a caller-owned temporary copy whose address survives until the
> call executes" — and the borrow of a temporary was refused for the mirror
> reason, that `struct_lit` built every literal into one shared `agg_slot` that
> the next literal in the same call would overwrite.
>
> `Frame::scratch(bytes)` is that storage: it reserves bytes below every
> declared local and returns the offset, consuming no declaration id, so it
> cannot desync codegen's declaration order from `LiveAnalysis`'s. A slot
> reserved this way is **never reused**, which is the point — the frame is a few
> bytes larger per syntactic occurrence in exchange for an address that is
> stable for the whole enclosing statement, and `Frame::size()` is computed
> after the body is generated, so those bytes are accounted for like any other.
>
> The first half is `ExprKind::Ref`. A borrow of a named `datac` local is still
> a `lea` of its own slot (no copy — the local already *is* the object). A
> borrow of anything else that is a `datac` — `sum(Pair(a = 1, b = 2))`, or the
> same shape over a call that returns a `datac` — now gets a fresh slot, is
> built into it through the same `eval_struct_into` a `let` initializer uses,
> and is borrowed by `lea`-ing that slot. `tests/fixtures/borrow_temp_slots.kp`
> is the shape this has to be right for and the one a shared slot cannot do: two
> temporaries as two arguments of one call (`pair_sum(Pair(..), Pair(..))`), a
> temporary that is itself a call's result (`sum(make(7, 8))`), a borrow inside
> a loop (one slot, re-initialized every iteration), a nested literal, and a
> `MEMORY`-class temporary borrowed rather than passed by value.
>
> The second half is the SysV `MEMORY`-class **argument**: `int_regs_for` now
> answers `Some(0)` for one instead of `None`, so it takes no integer register
> and — because SysV numbers the register file by how many registers each
> earlier argument consumed — the arguments around it keep exactly the registers
> they would have had. The value goes into the caller's outgoing argument area
> on the stack, which *is* the caller-owned copy; `memory_arg_area` computes the
> offsets (declaration order, each eight-byte aligned, area rounded up to
> sixteen) and is called by both `Gen::call` and `Gen::function`'s prologue, so
> the two ends of the convention cannot drift apart. At the call site the area
> is subtracted from `rsp` *after* `call_align_begin` has rounded it down, so
> `[rsp]` is the area's first byte and `rsp` is still sixteen-byte aligned at
> the `call`; each value is copied out of the slot it was staged in once every
> register argument is already loaded, and the hidden return pointer for a
> `MEMORY`-class *return* is still set last. In the callee the prologue copies
> the bytes from `[rbp+16+offset]` into the parameter's own frame slot, so a
> `MEMORY`-class parameter behaves exactly like any other `datac` local
> afterwards. `tests/fixtures/datac_memory_class_by_value.kp` covers a local, a
> call result and a literal passed by value; a `MEMORY`-class argument between
> two register arguments; two of them in one call; one after all six integer
> registers are taken; one alongside a `Two`-class argument (a register pair, a
> different mechanism sharing the same call) and alongside a float argument
> (whose register file SysV numbers independently); one on a call whose *return*
> is also `MEMORY` class; and a moved parameter copied into a mutable local
> (which is only a copy if the parameter really is the value rather than a
> pointer to the caller's copy).
>
> **Two real bugs this slice's own fixture found, both of the same family the
> third slice's `StructLit` bug came from** — a liveness pass that never walked
> a node kind, so two genuinely-live locals were handed the same register.
> `LiveAnalysis::expr` had no arm for `ExprKind::FieldAccess`, `Ref` or `Deref`,
> so the *inner* expression of each — normally a name — was never `touch`ed.
> That is invisible for a plain `datac` local (`is_agg` keeps those off
> registers entirely) but not for a `&datac`/`&mut datac` parameter, which is
> one register's worth of pointer and *is* a candidate: in
> `datac_memory_class.kp`'s `sum_quad(q: Quad)` there was only one such
> parameter, so nothing collided, and the gap went uncaught from item 4's first
> slice until `borrow_temp_slots.kp`'s `pair_sum(first: Pair, second: Pair)`
> put two of them in one function — both landed in `r15`, the prologue's second
> `mov r15, rsi` overwriting the first `mov r15, rdi`, and the wrong answer was
> `22` instead of `18`. The same gap was a real miscompile in
> `tests/projects/alias`, whose `bumped(t: Tally, by: i32)` mixes a reference
> parameter with a scalar one; fixing it unblocked that fixture too. The second
> bug is on the `Dest` side: `Gen::call` honoured a caller-supplied destination
> for the two wider aggregate classes only (the third slice's scope), so a
> `One`-class call result reached through `eval_struct_into` — newly reachable,
> because a call can now *be* a borrowed temporary's initializer
> (`sum(make(7, 8))`) — was silently left unwritten: that print read `0`
> instead of `15`. `AggClass::One` now honours `Dest` too, with the same
> opaque-eightbyte rule `struct_lit` already used, and every slot an aggregate
> can be written to is `align_up(size, 8)` bytes, so the eight-byte store is
> always contained in its own slot.
>
> **Item 8's first slice — `str` and `f32`/`f64` printing, plus the first
> hand-written runtime routine — is done (2026-09-19).** It is the first slice
> that is not aggregates, and it is the first piece of the *runtime* this
> backend owns rather than calls. Two things it closes and one it adds.
>
> **A `str`/`&str` is a value now.** `value_kind` and `int_regs_for` accept both
> spellings as `Width::W8` — they are the same C value, `const char*`, which is
> also why `types.rs` spells `&str` without a second `*` — so a `str` literal
> (`ExprKind::StringLit`, interned as the same `.byte` run a format string uses,
> so no second round of escaping), a `str` parameter, a `str` call argument and
> a `str` return all move through the paths a pointer already used. A returned
> `str` needed one line: `RetClass` had no case for it, because `Scalar` was
> being read as "an integer" rather than "a value in `rax`". A `&str` borrow
> evaluates its inner expression rather than taking an address, which is the same
> exception `codegen_c.rs` makes for a `str` view.
>
> **Printing a float is a register file, not an algorithm.** The plan's §5 item 2
> note says printing a float stays refused "until the formatter (§8) exists",
> and that is right about `fmt`'s `Display` machinery — shortest-round-trip is a
> real algorithm — but wrong about `print` of an `f64`: the C backend prints one
> with plain `printf("%f", (double)x)`, so the whole of the work here is the
> SysV detail the module header already names. A float argument goes in `xmm0`,
> an `f32` is promoted with `cvtss2sd` (what the C backend's `(double)` cast
> does), and a variadic call sets `AL` to the number of vector registers used —
> one — instead of zero. Where that becomes subtle is a *template*: SysV numbers
> the vector and integer argument files independently, so `"n=$n half=$half"`
> puts `n` in `rsi` and `half` in `xmm0` while `al` says one, and a second float
> goes to `xmm1` while the next integer still goes to `rdx`. `Gen::template`
> computes each interpolation's destination from how many *of its own class* came
> before it — the same split a call's argument marshalling makes — and a float's
> bits are pushed and popped rather than left in `xmm0`, because `xmm0` is the
> stack machine's working register and a later interpolation's evaluation is free
> to overwrite it. Getting this wrong is silent rather than a crash, which is why
> `tests/fixtures/asm_float_print.kp` pins the mixed case (one integer, one
> float, a bool and a `str` in a single template; two floats; both spellings of a
> string) and why it is in the differential gate.
>
> **`kelp_panic` is ours now.** `panic` and `assert` are builtins, not functions,
> and the C backend lowers them to runtime routines called `kelp_panic` and
> `kelp_assert`. This backend emits its own `kelp_panic` — the one piece of
> runtime it owns so far — for the same reason the differential suite exists:
> the routine's *observable* behaviour is what fixtures assert on, so it has to be
> `panic: <msg>` on stderr then `exit(1)`, which is `fprintf(stderr, ...)` with
> `stderr` reached through its GOT entry (a dynamically linked program has no
> fixed address for a libc data symbol) and `and rsp, -16` on entry, because this
> is called from mid-expression, where the stack machine's spills may have left
> `rsp` eight bytes off and `fprintf` is entitled to assume SysV alignment. It is
> emitted only for a program that can panic at all. `assert` inlines its single
> branch, which is all the runtime's one-line body does, and the numeric
> conversions became `Gen::conversion`: a cast plus the same range check
> `codegen_c.rs::numeric_conversion` emits, with the same panic message
> (`i64.to_u8: value out of range`), the same rule for *which* conversions need a
> check (`s_min >= t_min && s_max <= t_max` — a conversion the C backend emits as
> a bare cast has to be a bare move here), and the same signedness for the
> comparison an unsigned source needs (`u64` -> `u32` with a huge value must not
> read as a negative number). The one part that is not a transliteration is the
> unsigned 64-bit to float conversion, where `cvtsi2sd` alone would read bit 63 as
> a sign: the `(x >> 1) | (x & 1)` then double sequence is clang's, and
> `asm_float_print.kp` pins all three edges of it (2^64-1, 2^63, 2^63-1 — the
> first two of which no literal can spell).
>
> Eleven tests moved. Ten are existing tests that were waiting on one of these
> three things and now pass: printing a `str` (`aliases`, `hello_prints_greeting`,
> and the imported-alias project), printing a float (`const_declarations_fold`,
> `integer_to_float_conversions`, `a_non_finite_float_literal_emits_a_math_h_constant`),
> a function returning `&str` (`when_desugars_to_if_else_chain`), and the
> conversion/panic path (`integer_conversions_widen_exactly_and_narrow_with_a_check`
> — which also covers `int_conversion_overflow` — `panic_reports_its_message_and_assert_reports_failure`
> and `a_panicking_tail_satisfies_any_return_type`). The eleventh is the new
> `asm_float_print` fixture, which is the only place the mixed register
> assignment is pinned. The re-measured refusal table below is where the effect
> on the *ranking* shows: three whole rows are gone.
>
> Deliberate choices carried over unchanged from the first slice, each a
> thing later work still replaces: **assembly text** rather than machine code
> (tier 2), and **refusal over silence** — every uncovered construct is
> reported by name, so the suite measures coverage instead of hiding
> miscompiles. Capability-gated calls are refused rather than emitted
> unchecked: that is the one place an unimplemented feature would be a
> security hole rather than a wrong number.
>
> **Where the next 121 failures rank**, measured by aggregating what the backend
> refuses across the fixture files the C backend builds. The refused-fixture
> count is 98; the failing-test count is larger because several tests build a
> whole project rather than one fixture, and the differential gate compares two
> backends on a fixture both already build:
>
> | blocker (the backend's first refusal) | fixture files |
> | --- | --- |
> | an `extern fun`, any of them — 38 `std.mem.alloc_raw`, 2 `std.math.sqrt_raw` | 40 |
> | a heap type — `Bytes`/`String`/`View`/`List`/`Map`/`Box` as a type, a parameter, an argument, a return, a field or a printed value | 29 |
> | `enum` | 16 |
> | a passdown (`pd`) — a row this table did not name until the fourth slice re-measured it | 8 |
> | a `datac` holding a float field | 2 |
> | a `datac` that recurses through a `Box` | 2 |
> | a function value | 1 |
>
> Three corrections the re-measurements have made to the table this replaces.
> The first two are from the fourth slice: an `extern fun` blocks 40 fixtures
> rather than 36, because the refusal is of the *declaration* rather than of
> `alloc_raw` specifically; and `Mem`-class by-value passing is gone from it
> entirely, since the fourth slice closed it. The third is from item 8's first
> slice, which removed three whole rows — `str` printing, `f64` printing and the
> numeric conversions are each at zero refused fixtures now — and left the
> passdown row standing on its own feet, where before it was partly hidden
> behind `str` printing. The capability-gated row (7 fixtures) is unchanged and
> still real, but it does not rank on its own here: every one of those fixtures
> imports a gated module, so its build hits the `extern fun` refusal first, and
> the runtime is what has to land before the capability check is the next thing
> in the way.
>
> The first row is the argument for §8 being a *parallel* track: the single
> biggest unlock is the runtime, not the code generator, and the runtime is the
> half of this work that is backend-independent — though as the correction
> above (§0 item 1, and §8 below) records, the runtime landing in Kelp does not
> by itself clear that row; the asm backend's blanket extern refusal has to move
> too.
>
> Two bugs the differential suite found, kept as evidence the gate works:
> comparisons took their operand width from the `bool` result type (so every
> `i32` comparison compared only its low bytes), and a load used `mov rax, DWORD
> PTR [...]`, which is not an instruction. One bug the design caused and the
> suite caught as a segfault: the call-site stack save used `r11`, which is
> caller-saved, so `printf` clobbered it — invisible in a function that printed
> once, because the epilogue reloads `rsp` from `rbp`.

The first working own backend. Emits **x86-64 assembly text** and shells out to `as` and `ld` instead of `cc`.
This drops the C compiler from the dependency chain while keeping libc, and it is deliberately the tier with
the best value-per-line: the assembler does instruction encoding, so the hand-written part is lowering,
register allocation and ABI.

Scope, in build order. Each bullet is separately testable against the fixture suite.

1. **Scalars.** `i32`/`i64`/`u32`/`u64`/`u8`/`u16`/`usize`/`isize`/`bool`/`char`, integer arithmetic,
   comparisons, `while`, `if`/`else`, `return`, function calls, and the `-fwrapv` contract expressed as
   explicit wrapping adds. `char` is `uint32_t` (a codepoint, not a byte) and must not be confused with `u8`.
2. **f32/f64. Done** (2026-09-19). SSE2 scalar ops in `xmm` — arithmetic, comparisons (`ucomiss`/`ucomisd`,
   correct on the unordered/NaN case), unary negation (bit-flip through the integer file, since there is no
   `neg` for `xmm` and a memory-operand `xorps`/`xorpd` mask would need 16-byte alignment) — and the SysV rule
   that a call's vector-register usage is independent of the integer argument registers, verified both
   directions (an `f64` argument in the middle of an integer arg list, and vice versa). Printing a float turned
   out not to be part of this item's problem at all: `print` of an `f64` is one `printf` with the C backend's own
   `%f`, so it is item 8's, and it landed there — see that slice's blockquote. Only `fmt`'s `Display` machinery
   (shortest round-trip) is §8's.
3. **A register allocator. Done** (2026-09-19). Linear-scan over five callee-saved integer registers, for
   `i32`/`i64`/`bool`/`char` locals only — see item 3's blockquote above for the loop-widening correction and
   the bug it caught.
4. **Aggregates by value. First four slices done** (2026-09-19): a `datac` whose fields are all scalar and whose
   layout fits in eight bytes needs no SysV eightbyte classification at all — it is one eightbyte, so it moves
   through the same paths a `u64` already does, and only a field read or write needs the layout (see item 4's
   blockquote above). The second slice extends this to a `datac` nested by value inside another qualifying
   `datac`, as long as the outer struct's own size is still ≤8 bytes and the nested struct's own size is one of
   the four move widths (1/2/4/8 bytes). The third slice extends the *top-level* size limit past eight bytes:
   9-16 bytes passes/returns as a register pair (`AggClass::Two`), and over sixteen bytes uses the
   `MEMORY`-class conventions — the hidden-pointer one as a return value, and the caller's outgoing argument
   area as a by-value parameter or argument, which is the fourth slice (`Frame::scratch` is the storage both
   that and a borrowed temporary need) — sound with no SSE eightbyte case to consider, since every field this
   module accepts is `INTEGER`-classifiable by construction (see the third and fourth slices' blockquotes
   above). What is left of this item is: `enum` (tagged union) entirely; a `datac` holding a float or heap field
   anywhere in its fields, at any size; `String`, `Bytes`, `View`, `List<T>`, `Map<K, V>`, `Box<T>`; and a
   borrow of anything whose address is not a slotted `datac` (a scalar, a field path, a `str`). Layout must
   match `codegen_c.rs`'s `typedef`s exactly or the two backends will not agree on a `List`'s field offsets.
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
8. **`print`/`print!`. First half done** (2026-09-19): `str`/`&str` and `f32`/`f64` are printed, including the
   x86-64 details — a float argument goes in the `xmm` file rather than an integer register, `f32` is promoted to
   `double`, and a variadic call sets `AL` to the number of vector registers used, so a template interpolating
   one of each numbers the two files independently (see the fifth slice's blockquote). What is left of this item
   is printing an owned `Bytes`/`String`/`View`, which is not a printing gap at all: `%.*s` needs the heap
   types, so it lands with them.

Definition of done: all 247 integration tests pass under `--backend=asm` with byte-identical stdout to
`--backend=c`;
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

Definition of done: all 247 integration tests pass with no libc linked and no interpreter in the ELF; `ldd` reports
nothing or is not needed; `bench/speed/` checksums match and the runtime is no worse than `-O0`;
`kelpc build` is under 15 ms; and a capability fixture denied by a grant is denied with the same message.

---

## 8. Parallel track: runtime decoupling

> **Status: in progress.** The first two pieces §8 puts first — the capability
> matcher and the path normalizer — are Kelp as of `1071ead`, with the C kept as
> the reference each one is differentially tested against. The allocator is next,
> and it is also the top row of §5's ranking above (36 fixtures blocked by
> `std.mem.alloc_raw`).
>
> What landed: `kelpc/std/rt.kp`, a shipped module the resolver injects for any
> program that imports a gated module or `std/path` — the same conditions that
> make generated C call into it. `glob_match`, `host_glob`, `host_match` and
> `path_normalize` are there; the C that remains in `kelpc/runtime/cap_glue.c` is
> the deny path, the allowlist loops, the buffered plumbing, and
> `kelp_resolve_path` (symlink resolution, which moves with the filesystem half of
> the runtime). The reference implementations are files — `cap_matcher_ref.c`,
> `path_normalize_ref.c` — and are no longer emitted into programs; only
> `tests/cap_matcher.rs` and `tests/path_normalize.rs` compile them, which is what
> a `const &str` in `codegen_c.rs` could not have allowed.
>
> Two interfaces were decided here, and both are worth re-reading before the
> allocator moves. The matcher is `&str` in, `bool` out, so the C glue needed only
> generated prototypes for the Kelp symbols. The normalizer writes through a
> `mut Bytes` the *caller* owns and uses `out.len()` as its capacity, so it
> allocates nothing and keeps the C rule (`o + len >= cap`) exactly — the buffers
> are declared one byte past `PATH_MAX` because `Bytes` keeps its NUL in the slot
> at `len`, and the two C callers construct that borrowed view and deliberately
> never drop it.
>
> The corpora are the proof, not the fixtures: ~5000 matcher cases and ~250
> normalizer paths, the latter compared byte by byte in the buffer as well as by
> verdict. Both were mutation-tested — dropping the port vocabulary from
> `host_match`, removing the `PATH_MAX` bound, accepting a leading `..`, loosening
> the capacity rule — and each mutation is caught with the exact case named.
>
> The asm backend skips emitting the runtime module and refuses calls into it by
> name; its pass count and refusal ranking are unchanged by this, because it
> refuses every gated call rather than emitting one unchecked.
>
> Remaining in §8's order: **the allocator** (next, and the biggest single
> unlock), then the formatter with float last, then DNS. Each lands the same way —
> Kelp first, the C kept as the reference, a corpus that compares them — and the
> pieces that stay C longest by design are the ones that are I/O (`kelp_cap_deny`,
> `printf`), because §7 owns that boundary.
>
> **The allocator does not fit that pattern, and this was checked before writing
> any code (2026-09-19).** The matcher and normalizer moved because both operate
> purely over `&str`/`Bytes` at the safe level — no case they handle needs a
> pointer Kelp cannot already express. An allocator's actual bookkeeping (an
> intrusive free list, a header written into raw memory, following a link to the
> next free block) needs raw pointer manipulation, and `unsafe`/raw pointers are
> listed in §11 as **not on this branch** (deferred to backlog B-1/B-2/B-3). So
> "the allocator, in Kelp" is blocked on a language capability this branch has
> explicitly ruled out, not just unwritten.
>
> There is a second gap underneath that one: `std/mem.kp` declares exactly one
> extern, `alloc_raw`, and says so on purpose ("There is deliberately no
> `mem.free`") — drop-generated `free()` calls and `String`/`List` growth's
> `realloc()` calls are emitted directly by `codegen_c.rs` today and are invisible
> to Kelp source entirely. Swapping `alloc_raw`'s body alone would not even be
> safe in isolation: a buffer it hands out could still be freed or grown by a
> mismatched allocator on the C side.
>
> **What is actually available now, and is real progress without pretending to
> be a from-scratch allocator:** expose `realloc_raw`/`free_raw` as externs
> alongside `alloc_raw`, and move the *policy* around them — the zero-fill
> contract, the `cap + 1`/NUL invariant, the OOM-panics-not-fails contract — into
> reviewable Kelp functions that call thin C-backed terminal primitives (the same
> shape `alloc_raw` already has). That is a real, testable, byte-exact-with-C
> step; it is not the allocator Stage 3 §7.2 describes ("`malloc`/`realloc`/`free`
> over `mmap` or `brk`"), which stays blocked on raw pointers until B-1/B-2/B-3
> land. Until then, §8's order should treat the allocator as **blocked**, not
> next, and pull the formatter forward in its place.

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

> **Status: not started, and no longer blocked.** Stage 0 has landed, which was the one commit both tracks
> had to touch, so this is now a decision to make rather than a prerequisite to satisfy. Everything in this
> section is unexecuted detail — treat it as a proposal, not as a record.

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
| Aggregate ABI mismatch | Kelp passes `datac`/`enum`/`String`/`List`/`Map`/`Box` by value; a wrong eightbyte split is a silent miscompile | differential stdout on all 260 fixtures; assert struct offsets against the C typedefs in a test |
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
