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

**Where the default moves:** `plan/mk7-default-switch.md` is the proposed sequence for making the `asm`
backend the default, and states what has to be true first and in what order. It is a proposal, not a
commitment: this file's §0 stays the live state, and §1's ground rule 4 still decides when stage 2 may
start.

---

## 0. Where the branch is

*Updated as work lands. This table is the answer to "what is done and what is left"; every section below is
the detail behind one row. Branch `mk7/own-backend`, and the numbers are measured on this machine, not
projected.*

> **Re-measured 2026-09-20 at `c3a8759`, the current tip, and this blockquote is the current state.**
> Measured in a detached worktree of this checkout, because other lanes were working in the shared one at
> the time; every number below is that commit's, and `plan/mk7-next-slice.md` is the brief that follows
> from them. The fixture ratchet is **176 built of the 201 fixtures the C backend builds, 25 refused**;
> the asm suite is **285 of 336** with the C backend passing all **336**; valgrind is clean over **377
> runs / 357 fixtures** (`c: 201 checked, 0 findings`; `asm: 176 checked, 25 refused, 0 findings`); and
> the refusal ranking is **a heap type 14, a fixed-size array 0, an `enum` 0, a `datac` holding a float or
> heap field 0, a closure or a function value 10, a gated extern 1**. Three of the six rows are empty, and
> the heap row is not one gap: 12 of its 14 name an `extern fun` body this backend has no routine for, one
> names `List.clone` on a `datac` element (the asm half of this commit's own derived copy and order), and
> one names a borrow of a field chain. The sweep also reports one fixture it builds that the recorded
> baseline does not name yet: `borrow_two_phase`. Everything this section says below about `283 of 329`,
> `240 of 289`, `144 built of 168`, `175 fixtures`, `168 C and 144 asm` or `24 refused` is the measurement
> of the slice it sits beside, kept as the record; the numbers in this blockquote are the tip's.
>
> **Re-measured 2026-09-20 at `caeb98d` (now `8d86d8e`), which supersedes the `c3a8759` blockquote
> above; that one stays as the record of its own commit.** The fixture ratchet is **178 built of the
> 202 fixtures the C backend builds, 24 refused**; the asm suite is **288 of 336** with the C backend
> passing all **336**; valgrind is clean over **380 runs / 358 fixtures** (`c: 202 checked, 0
> findings`; `asm: 178 checked, 24 refused, 0 findings`); and the refusal ranking is **a heap type 13,
> a fixed-size array 0, an `enum` 0, a `datac` holding a float or heap field 0, a closure or a
> function value 10, a gated extern 1**. Three of the six rows are still empty, and the heap row is
> still not one gap: 11 of its 13 name an `extern fun` body this backend has no routine for, one names
> a borrow of a field chain, and one is this round's own `caps_gated_path`, the pin for the gated
> `fs.read` a capability check has to reach. Two slices moved the numbers since `c3a8759`: `fdd3d38`
> (an empty container takes its element from the first use, which recorded `container_element_from_use`,
> 196/175/21) and `caeb98d` (this round: it compiles `std/rt` and lowers `std.path.normalize_raw` and
> `std.fs.cwd_raw`, so `path_normalize` and `path_absolute` build, 202/178/24). The sweep's "builds but
> is unrecorded" note is settled: `borrow_two_phase` is in `ASM_BUILT_BASELINE` now. Everything this
> section says below about `285 of 336`, `176 of 201`, `377 runs over 357 fixtures`, `201 checked` or
> `25 refused` is the measurement of the slice it sits beside, kept as the record; the numbers in this
> blockquote are the tip's. Commands: `cargo test --test backend_coverage -- --ignored --nocapture`,
> `cargo test --test compile_and_run` (and with `KELPC_BACKEND=asm`), and
> `cargo test --test memory_gate -- --ignored --nocapture`.

> **Re-measured 2026-09-20 at `9c89b70`, the current tip, which supersedes the `caeb98d`
> blockquote above; both earlier blockquotes stay as the record of their own commits.** Measured in a
> detached worktree of this checkout (`git worktree add --detach /tmp/plan-refresh 9c89b70`), because
> other lanes were working in the shared one; the worktree was removed afterwards. The fixture ratchet
> is **200 built of the 208 fixtures the C backend builds, 8 refused**; the asm suite is **319 of 344**
> with the C backend passing all **344**; valgrind is clean over **408 runs / 364 fixtures** (`c: 208
> checked, 0 findings`; `asm: 200 checked, 8 refused, 0 findings`); and the refusal ranking is **a heap
> type 8, a fixed-size array 0, an `enum` 0, a `datac` holding a float or heap field 0, a closure or a
> function value 0, a gated extern 0**. Five of the six rows are empty, and the heap row is still not
> one gap: six of its eight name an `extern fun` body this backend has no routine for (`fs_fallible`,
> `process_spawn`, and the four `net` fixtures), one names `List.clone` on a `datac`/`enum` element (the
> asm half of `c3a8759`'s derived copy and order), and one names a borrow of a field chain. Four slices
> moved these numbers since `caeb98d`: the closures slice (`eab7916`, the whole closure row, ten
> fixtures), the clock and entropy externs (`5958794`), the `io` descriptor family (`4752ebb`), and the
> capability check itself (`e65ce61`, `9c89b70`), which built `caps_gated_path`, `fs_errno` and
> `fs_symlink_kind` and took the last two path fixtures with them. The sweep's "builds but is
> unrecorded" note is settled: `borrow_two_phase` is in `ASM_BUILT_BASELINE`. Everything this section
> says below about `288 of 336`, `178 of 202`, `380 runs over 358 fixtures`, `202 checked` or `24
> refused` is the measurement of the slice it sits beside, kept as the record; the numbers in this
> blockquote are the tip's. Commands: `cargo test --release --test backend_coverage -- --ignored
> --nocapture`, `cargo test --release --test compile_and_run` (and with `KELPC_BACKEND=asm`), and
> `cargo test --release --test memory_gate -- --ignored --nocapture`.

> **Re-measured 2026-09-20 at `926e33f`, the current tip, which supersedes the `9c89b70` blockquote
> above; every earlier blockquote stays as the record of its own commit.** Measured in a detached
> worktree of this checkout (`git worktree add --detach /tmp/stop-record/tip3 926e33f`), because other
> lanes were working in the shared one; the worktree was removed afterwards. The fixture ratchet is
> **205 built of the 208 fixtures the C backend builds, 3 refused**; the asm suite is **341 of 358**
> with the C backend passing all **358** (a first run of the same command read 340 of 358, and its one
> extra failure was `the_host_port_denial_names_the_host_as_written_and_its_own_port`, the loopback
> test that fails intermittently in a full run; the re-run is the number quoted); valgrind is clean
> over **413 runs / 364 fixtures** (`c: 208 checked, 0 findings`; `asm: 205 checked, 3 refused,
> 0 findings`); and the refusal ranking is **a heap type 3, a fixed-size array 0, an `enum` 0, a
> `datac` holding a float or heap field 0, a closure or a function value 0, a gated extern 0**. Five of
> the six rows are empty, and the heap row is three mechanisms rather than one: `process_spawn` names
> the `extern fun std.process.arg_count_raw`, `list_datac_clone_sort` names `List.clone` on a `List`
> (the asm half of `c3a8759`'s derived copy and order), and `borrow_chain_through_a_borrow` names a
> borrow of a field chain. Seven slices moved these numbers since `9c89b70`: the S1 clock and entropy
> externs (`5958794`), the S2 `io` descriptor family (`4752ebb`), the S4 `fs` path family (`0053bc2`),
> the four capability-bypass closures (`dadd309`, `5366fff`, `e40add4`, `734131b`), the whole `std/net`
> surface and its three checks (`65c1271`, `3390ee2`), and S7, the fallible `fs` pair (`7dc08d7`). The
> gate artifact builds and runs under both backends, and the three commits after `7dc08d7` fix two
> defects this round's own spec, fuzz and review lanes found (`8be6caa`, the C denial's location,
> module and domain escaped through `diag::c_string_body`; `fccb578`, a gated item refused as a
> function value, granted or not) and restate the sweep's own counts (`926e33f`). The `bench/speed`
> gate is still unmet on `fib` alone: the annex's `9c89b70` table has `fib` at 1.713 against `-O0`
> with the other three workloads at 0.964, 0.594 and 0.684, and this round's two sweeps (9 rounds
> interleaved, round 1 discarded, median of 8, the one estimator the annex fixes; load 20.7 and 11.9
> from other lanes) put `fib` at 1.845 and 1.795, the rest of the asm column between 0.85 and 1.10,
> and the whole-process external wall at 1.078 and 0.994, so the idle-machine run the annex has owed
> since it was written is still owed. Everything this section says below about `200 of 208`, `319 of
> 344`, `408 runs over 364 fixtures`, `208 checked` or `8 refused` is the measurement of the slice it
> sits beside, kept as the record; the numbers in this blockquote are the tip's. Commands: `cargo test
> --release --test backend_coverage -- --ignored --nocapture`, `cargo test --release --test
> compile_and_run` (and with `KELPC_BACKEND=asm`), and `cargo test --release --test memory_gate --
> --ignored --nocapture`. The session that produced this state, commit by commit, with the defects it
> found, is the record in §14 at the end of this file.
>
> **This round's sibling spec is `plan/mk7-extern-gating.md`, and this blockquote points at it rather
> than summarizing it.** It is the specification lane for `KelpLANG/ISSUES.md` `BUG-wQ9tL`, "a `*_raw`
> extern bypasses the capability check": the bypass repro, how the gate table is built in `resolve.rs`,
> and three candidate rules with their costs. A lane is implementing its rule (b) now, which edits
> `resolve.rs` and `stdlib.rs`, so the two numbers of the above that lane can move are the C-built
> denominator (208) and the refusal *messages* of the six gated extern-body fixtures; the two non-gated
> refusals (`list_datac_clone_sort`, `borrow_chain_through_a_borrow`) are not coupled to it.
> `plan/mk7-next-slice.md` is the brief that follows from the numbers above.

> **Correction recorded 2026-09-20: the `81e68e7` message's "byte-identical" claim is false.** Measured
> over the 447 `.kp` targets under `tests/fixtures` and `tests/projects` at `81e68e7^` and `81e68e7`
> (235 of them emit C at both and were compared as sorted line multisets), six differ, five only by the
> `String.as_str()` hoist `checked_arg_needs_temp` adds (the capability fix is behaviourally neutral for
> every other analysed target) and the sixth being the new `caps_double_eval`; `docs` commit `7cb41ec`
> carries the measurement and the method.

| stage | state | evidence |
| --- | --- | --- |
| **0. Backend boundary** | **done** | `backend.rs` + `--backend=`/`KELPC_BACKEND` + per-backend suite + `run-all-backends.sh`; commit `68746a6` |
| **1. x86-64, assembly text, `as`+`ld`** | **in progress — scalars, f32/f64, a register allocator, ten slices of item 4's aggregates (the last five being `datac`s that own a heap field, `Box<T>`, `List<T>` — the first container whose element can itself own — `Map<K, V>`, which closed the container set, `enum`, and then a float field in an aggregate, the real per-eightbyte SysV classifier that closes item 4 outright), item 8's `str`/float printing (the print slice extends it: a `Bytes`/`String`/`View` interpolation is a `%.*s` *pair* of variadic arguments, so the template's register numbering counts arguments per piece rather than per value — see §5) and `kelp_panic`, passdowns' generated storage, extern calls through the PLT, then the owned `Bytes` buffer with drops, then a scalar borrow with the read and store through it, then the three `str` parsers, then the whole closure row and function values, then the capability check (all five kinds since `65c1271`: the path and name checks, the `net` host, hostport and port checks, the deny path and the compiled-in allowlists)** | `codegen_asm.rs`; **341 of 358** integration tests pass under `--backend=asm` (measured at `926e33f`); all 205 fixtures the asm backend builds print byte-identical stdout to the C backend, and six heap types are in: `Bytes` end to end (layout, `malloc`/`realloc`/`free` through the PLT, thirteen owned routines, printing `%.*s`, and item 5's drops), then `String` and `View` (a further 28 routines, and `View` as the first heap type on the `AggClass::Two` register pair, which needed no new marshalling code at all), then `Box<T>` (one layout for every element type, seven routines per instantiation, and the first type whose own layout has to resolve while the `datac` that holds it is still being placed), then `List<T>` (one `$List` key for every element type — the same `{data, len, cap}` header `Bytes`/`String` are — fifteen routines per instantiation plus a generated drop, one growth helper, `kelp_rt$list_reserve`, that is deliberately *not* monomorphized because the element size is an argument, and then S1's element-level three — `sort` behind a per-element-kind comparator, `contains`, and `slice` cloning each owned element — gated on `types.rs`'s `list_sortable_elem`/`list_comparable_elem`/`list_clonable_elem` exactly as the C backend gates them; see §5's sort blockquote), then `Map<K, V>` (one `$Map` key — a 56-byte `MEMORY`-class header, fifteen routines per instantiation, a generated drop and three shared key helpers — and the first thing in this backend whose *entry* layout depends on a type parameter, because the value pointer sits at `align_up(size_of(K), 8)` and the live byte follows it), then `enum` (a tag-and-union layout beside the `datac` ones, `EnumLit`/`EnumTag`/`EnumField`, `?`, and a generated drop that switches on the runtime tag — the first thing in this backend whose *drop* depends on a runtime value), then a float field in an aggregate (a per-eightbyte classifier over `INTEGER`/`SSE`, `xmm` transport through every boundary, and the union merge that makes two same-size enums differ only in register file — see §5's blockquote), and then a scalar borrow (`&mut x` as a `lea` of the local's own slot, with the live-range pass keeping every addressed local out of the register pool, and `*p`/`*p = v` for integer and float pointees) together with the three `str` parsers, which are `strtoll`/`strtoull`/`strtod` behind `externs.rs`'s own whitespace, whole-string and `errno` checks. The memory gate §5's definition of done had been missing is now a test rather than an argument: `kelpc/tests/memory_gate.rs` (`0747195`) runs `valgrind` over both backends' binaries — 168 C and 144 asm, zero findings — at that commit; the sweep was red at and after `e748b06` (one defect in the shared drop plan, §5), and the drop-plan slice's commit is what makes the row true again: **329 runs over 309 fixtures, `c: 174 checked, 0 findings`; `asm: 155 checked, 0 findings`** (at `c3a8759`: **377 runs over 357 fixtures, `c: 201 checked, 0 findings`; `asm: 176 checked, 25 refused, 0 findings`**) — because the AddressSanitizer gate instruments generated C and cannot see this backend. Two bugs this round's own fixtures turned up are fixed in `53ec30d`, and the enum slice's own two — one of them latent since the first slice — in this branch's `enum` commit; the enum slice's *third* defect, a wrong-width aggregate load the differential sweep could not see, is fixed and pinned by `heap_narrow_fields`/`list_narrow_element_pop`/`try_narrow_payload` in this commit, and then the float codegen slice (below) — `f32`/`f64` locals in an `xmm` pool and float operands named directly instead of loaded — which is the first change on this branch that moves a `bench/speed` kernel rather than a fixture count, and the first whose measured effect had to be re-derived after the classifier landed because the two slices share this file: on the merge result `float` goes from **4.16x** the measured `-O0` column to **0.54x**, with `collatz` 0.97, `mix` 0.70 and `fib` 1.66 (see §5's float blockquote for the table, the method and why `fib`'s number is a different slice's). Re-measured 2026-09-20 at `926e33f`, what has landed since `9c89b70` is the whole `std/net` surface and its three check kinds, the S1 clock and entropy, S2 `io` descriptor and S4 `fs` path families, the S7 fallible `fs` pair, and the four capability-bypass closures plus the frontend refusal of a gated function value; what is left of items 5 and 10 is `std.process` alone |
| **2. Machine code + own linker** | not started | tier 1 must pass the suite first (§1.4) |
| **3. Freestanding (no libc)** | not started | blocked on tier 2 |
| **8. Runtime decoupling (parallel)** | **in progress — the matcher and the normalizer are Kelp** | `std/rt.kp` + two differential corpora; commit `1071ead`; the allocator is blocked on raw pointers (§8's note below) |
| **9. Two-agent split** | not started | Stage 0 landed, so the tracks can now diverge |

The one number that matters most, and corrected here because this sentence had gone stale at 195/272 while the
ratchet beside it moved: **226 of 283 tests pass on the asm backend, and the C backend passes all
283.** The C backend is the oracle, so that ratio is a measurement of coverage rather than a claim about
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
The passdown slice moved it by six: one new fixture (`passdown_storage_is_a_global_that_main_initializes`,
over `passdown_globals.kp`, which pins an initializer that reads an earlier passdown and a caller that has no
hidden parameter and so passes the global's own address) and five fixtures it unblocked — `passdowns`,
`passdown_datac`, `pd_shadowed`, `pd_var_copy_read_twice` and `pd_var_write`. The three passdown fixtures it
did **not** unblock (`pd_owned`, `list_field`, `pd_argument_forwarding`) were refused for their passdown's
type, which is `String` or `List<i32>` — the mechanism was there, the heap was not — and the passdown-drop
slice below built all three.
The extern slice (below) moved it by three, and its number is the least interesting thing about it: `escapes`,
`char_literals` and `stdlib_math` now build and agree with the C backend, while the *ranking* moved much
further — the blanket `extern fun` refusal is gone, and with it the mask that made 40 fixtures report an
extern declaration as their first blocker when the heap type or `enum` behind it was what was actually in the
way. Re-measured after that slice, the 90 refused fixtures rank as **heap types 73, `enum` 9, a `datac` with a
float field 3, closures 4, a gated extern 1** — see §5's table for the per-message breakdown.
The heap slice (also below) moved it by ten at the fixture level and by eleven at the test level: the ten
fixtures in §5's heap blockquote, plus a new `int_mod_narrow.kp` that only exists because that slice's own
probe found `%` unassemblable below sixty-four bits. Its other two measurements are the ones worth keeping: 48
fixtures now build (was 37), the refused set is 80 (was 90), and every one of the 48 is `valgrind`-clean, which
is the first memory gate this backend has had. The String/`View` slice (below) then moved it by ten more at the
fixture level and eleven at the test level — 58 fixtures build, 71 are refused, the asm suite is 156 of 251; the two bug fixes that followed took both
counts up by two more, to 60 and 158 —
with valgrind clean over all 131 C-backend and 60 asm-backend binaries, and with two things worth carrying
forward: `View` was the first heap type to use the `AggClass::Two` register pair and needed no new ABI code at
all, and a latent dispatch bug (`View#to_string` matching the `#to_` numeric-conversion prefix) was found by a
heap type whose method name happens to contain that spelling. The heap-field slice (below) then moved it by
three: two fixtures a `datac` holding an owned `String`/`Bytes` field had been keeping out (`string_field`,
and `generic_method_basic`, whose `Pair<String, String>` instantiation was refused only for the field type) plus
the one fixture the slice wrote to pin a struct with three owned shapes in it at once
(`datac_owned_fields`) — 63 fixtures build, 69 are refused, and the asm suite is 161 of 254. Its measurement
worth keeping is that **the ABI needed nothing**: a `String` field is a nested aggregate like any other, so the
generated per-`datac` drop was the only new mechanism, and every fixture the slice moved had been refused for
the field's type rather than for anything about how a value travels. The box slice (also below) moved it by
three: the two fixtures the builtin owning pointer had been keeping out (`box_recursive`, whose recursive
`datac` through a `Box` field is the type's whole reason to exist, and `box_empty_panic`) and
`box_owned_elements`, the fixture that slice wrote for replacing and taking an owned element — 66 fixtures
build, 67 are refused, and the asm suite is 164 of 255. Two things about it are worth more than the count: a
`datac` whose field is a `Box` needed the *recursive* layout to resolve (a box is eight bytes whatever it
points at, so the element's own layout is deliberately not required while the parent's fields are placed), and
the slice's own fixture found a **pre-existing bug in this backend that no earlier fixture had reached** — an
assignment to an aggregate local wrote its first field into the target's own slot before evaluating a field
initializer that reads the target (`list = Node(value = 2, next = Box.of(list))`), which C's
compound-literal-then-assign order gets for free and which this backend has to stage explicitly. The list
slice (below) moved it by nineteen at the fixture level and eighteen at the test level: `List<T>`'s single
`$List` layout key turned every fixture that had been refused for a list type *somewhere* into a build —
fourteen of them, from `list_basic` to `box_basic`, `for_loop`, `move_param`, `path_polish`, `recursive_datac`
and `view` — and the five fixtures the slice's own scope forced into existence (`list_element_widths`,
`list_growth_and_rebuild`, `list_insert_remove_shifts`, `list_insert_bounds`, `list_remove_bounds`) are why the
denominator moved from 255 to 260 rather than staying fixed. 85 fixtures build, 54 are refused, the asm suite
is 182 of 260, and valgrind is clean over all 139 C-backend and 85 asm-backend binaries.

The map slice (below) then moved it by eleven at the test level and twelve at the fixture level. Twelve
fixtures stopped refusing: five of the six the heap row had been carrying for `Map` (`map_basic`,
`map_key_types`, `map_missing_key`, `map_owned`, `map_remove_missing`), `map_datac` from the `datac` row, and
the six the slice wrote (`map_clear_then_reuse`, `map_growth_and_rehash`, `map_key_widths`,
`map_owned_round_trip`, `map_replace_drops_old_value`, `map_tombstones_and_reuse`) — against a denominator
that moved from 260 to 266 for those six. `str_to_string_coercion` is **not** among them, and the acceptance
criterion that named it was wrong: its first refusal is an `if` in value position producing a `MEMORY`-class
value, and then an `enum`, neither of them a `Map`. 97 fixtures build, 48 are refused, the asm suite is 193 of
266, and valgrind is clean over all 145 C-backend and 97 asm-backend binaries. Its measurement worth keeping
is that **the ABI needed nothing**: `$Map` is a `MEMORY`-class aggregate like `Bytes`, so every site that
already handled one handled it, and the one thing that finally depended on a type parameter was an entry's own
offsets — which is the fifth time a heap type has slotted into the aggregate work unchanged.

The enum slice (below) then moved it by nineteen at the test level and nineteen at the fixture level: the asm
suite went 200 of 273 → 219 of 278 and the ratchet 106 built of 153 → 125 of 158, with 33 refused. The
scalar-borrow slice (also below) then moved it by four more — the asm suite 223 of 280, the fixture
ratchet 129 built of 160, 31 refused — and the enum slice's own defect fix (also below) has now moved the
denominators by three more without unblocking a refusal: the asm suite is **226 of 283**, the fixture
ratchet is **132 built of 163, 31 refused**, and valgrind is clean over all 163 C-backend and 132 asm-backend
binaries. The fix's three fixtures are new (`heap_narrow_fields`, `list_narrow_element_pop`,
`try_narrow_payload`), so they move the denominator rather than the ratio. The float-in-aggregate slice
(below) has now moved both counts again: its five new fixtures and the `.s`-level ABI test move the
denominator, all seven fixtures it had been waiting on stop refusing, and the numbers at that slice are the **asm
suite 240 of 289**, the **fixture ratchet 144 built of 168, 24 refused**, valgrind clean over all 168
C-backend and 144 asm-backend binaries, and a refusal ranking of **heap 13, `enum` 0, a `datac` holding a
float or heap field 0, closure 10, a gated extern 1**. Two of the scalar-borrow
slice's four are fixtures the plan had credited *that* slice with and it could finally reach
(`stdlib_build`, `borrow_provenance_and_arms`, both refused first on `std.str.parse_i64_raw` and both reachable
only because the enum slice had already landed behind it), and two are the fixtures the slice wrote
(`scalar_borrow`, `scalar_borrow_f64`). `stdlib_parse`, the third fixture the plan credited it with, was
**refused** at that point: with the parsers lowerable its first refusal was `Result<f64, ParseErr>`, the
float-aggregate row — and the float-in-aggregate slice has now built it, which is the correction recorded in
§0's list rather than quietly fixed.

**How much of the whole plan is done, stated the same way the rest of this file measures things — in what
landed, not a percentage that would imply more precision than the plan has:**

- **Stage 0** (backend boundary): done.
- **Stage 1** (x86-64 assembly): 3 of its original 8 build-order items (§5) are done outright — scalars,
  `f32`/`f64`, the register allocator — plus a 4th (aggregates) **with all of its slices landed** (a `datac` by
  value, a nested `datac`, the wider SysV classes, a `MEMORY`-class argument and a borrowed temporary, an owned
  heap field, `Box<T>`, `List<T>`, `Map<K, V>`, `enum`, and a float field in an aggregate — the per-eightbyte
  classifier that closes item 4 outright), the first
  half of item 8 (`print`/`print!` of `str` and the floats), passdowns (§5's ninth item, added after the fact:
  `pd` is M5.2's language feature, and the asm backend's second self-contained piece of codegen after the
  `datac` work), the extern path (§5's tenth item, added for the same reason): calls to `extern fun`s this
  backend can lower now go through the PLT or to a routine it owns, and functions nothing can reach are not
  compiled, and the owned-passdown drop (§5's eleventh item): an owning `pd` is freed from a generated
  `kelp_drop_pds` that `main` registers with `atexit`, which is what closes the passdown row. That is where the
  float-in-aggregate slice's 240/289 fixture number came from; at `926e33f` the ratio is 205 of 208, and
  the closures slice (`eab7916`), the capability check's five kinds (`e65ce61`, `9c89b70`, `65c1271`),
  the `std/rt` compile with its two path/cwd externs (`8d86d8e`), the S1, S2, S4 and S7 extern families
  (`5958794`, `4752ebb`, `0053bc2`, `7dc08d7`) and the four capability-bypass closures plus the frontend
  refusal of a gated function value (`dadd309`, `5366fff`, `e40add4`, `734131b`, `fccb578`) have landed
  since.
  Item 4 has nothing left to name: `enum` landed with the enum slice below,
  the float field with the float-in-aggregate slice below, and `String`/`Bytes`/`List`/`Map` fields and
  `Box<T>` itself are covered
  (see the heap-field, box, list and map slices' blockquotes, and the fourth slice's, which covers what "by
  value" means end to end), and item 8's rest is done too (the print slice lowered an owned
  `String`/`View` interpolated into a template, which is what that row really was: `print` of an owned
  `String` already built). So what stands between here and Stage 1's own definition of done, measured at
  `926e33f`, is the rest of items 5 and 10 (one extern family, `std.process`: its seven-argument
  `spawn_raw` needs a stack-passed register-class argument this backend does not have yet, which is a
  mechanism rather than a body), item 7 is done (all five check kinds emit, the `net` host, hostport and
  port ones since `65c1271`), the heap row's two leftovers that are neither (`list_datac_clone_sort`'s
  `datac`/`enum` element derived copy and order, and `borrow_chain_through_a_borrow`), and Stage 1's own
  numbers: all 358 integration tests passing (341 do at `926e33f`), `bench/speed` no worse than `-O0`
  (unmet on `fib`, 1.713 in the annex's `9c89b70` table and 1.795 to 1.845 in this round's two contended
  sweeps), and a memory gate for the asm backend, which the heap
  slice supplied as a `valgrind` sweep over every fixture this backend builds (413 runs over 364 fixtures
  at `926e33f`) and which is documented rather than wired into the suite (see that blockquote for why).
- **Stage 2** (machine code + own linker) and **Stage 3** (freestanding): not started; both are blocked on
  Stage 1 finishing first, by this plan's own tiering rule (§1.4).
- **§8** (the parallel runtime track): 4 of its pieces are done (the capability matcher, the path
  normalizer, and this session's `/etc/hosts`/`resolv.conf` parser and DNS message parser, each with a C
  reference and its own corpus);
  the allocator is blocked on raw pointers (not on this branch — see §8's note below) and the formatter and the
  resolver's own transport half have not been started.
- **§9** (the two-agent split): not started — a decision to make, not a prerequisite, since Stage 0 already
  unblocked it.

So: **one of four backend stages done, the second about halfway through its own item list, the third and
fourth not begun**, plus the runtime track's four lowest-risk pieces landed and its costliest piece (DNS
transport) not
touched. Read the fixture ratio (205 of 208 at `926e33f`) as the honest single number if one number is wanted; it
already accounts for every one of these except the two things it structurally cannot measure, `bench/speed`
and the memory gate, which is why both are called out separately above and in §5's definition of done. The
memory gate now exists (see the blockquote at the top of this section) and `bench/speed` now builds under
`--backend=asm` and is measured (§13 and that blockquote; by §5's per-workload reading it is unmet on `fib`
alone).

**What is next, in the order the refusals rank themselves** (aggregated across the fixtures the C backend
builds, re-measured at `c3a8759`, and detailed in `plan/mk7-next-slice.md`):

> **Re-measured 2026-09-20 at `926e33f`: the list below is the `c3a8759` ranking with a `9c89b70`
> correction beside it, and at the tip it is three fixtures and three mechanisms.** Row 1 is
> `process_spawn` alone: `fs_fallible` built at `7dc08d7` and the four `net` fixtures at `65c1271` and
> `3390ee2`, so the only `extern fun` body left is `std.process.arg_count_raw` and the seven bodies
> behind it, and row 1's `List` and borrow members are the other two rows' subjects. Row 4's own
> methods are built and its leftover is `list_datac_clone_sort` (one fixture). Row 6's capability row is
> done rather than 0: the check emits for all five kinds and the refusal ranking's gated row is empty.
> The third refusal, `borrow_chain_through_a_borrow`, is the one no row here names for itself. The
> order the tip's own measurements put the next work in, and what each item needs, is §14's "how to
> resume" at the end of this file.

> **Re-measured 2026-09-20 at `9c89b70`: the list below is the `c3a8759` ranking, and its row 1 is now 8
> fixtures, not 14.** The heap row's twelve externs are six (`fs_fallible`, `process_spawn` and the four
> `net` fixtures): the `fs` path family, the `io` descriptor family and the `time`/`rand` bodies have
> landed since. Rows 2 and 3 are still landed, row 4's `str`-parser fixtures still build, and the
> capability row is 0 rather than 7. `plan/mk7-next-slice.md` is the brief that follows from the `9c89b70`
> measurement.

1. **The heap row, restated, because the measurement moved it** — 14 fixtures at `c3a8759`, and it is not
   one gap. Twelve name an `extern fun` whose body this backend has no routine for: **the `fs`/`io`/`net`/
   `path`/`process` extern bodies** (`fs_errno`, `fs_fallible`, `fs_symlink_kind`, `io_stdio`,
   `io_streams`, `net_loopback`, `net_poll`, `net_resolve`, `net_timeout`, `path_absolute`,
   `path_normalize`, `process_spawn`). One names the `List` method the newest commit added to the frontend
   and the C backend (`list_datac_clone_sort`). One names a borrow of a field chain
   (`borrow_chain_through_a_borrow`). Three of the twelve import `std/io` or `std/path` only, and those two
   modules gate no domain (`stdlib.rs`'s `domain_of`), so they need the bodies alone; the other nine also
   need the capability check, because their first refusal is an extern body only because the live-extern
   pre-pass in `generate` runs before the call-site gate check. **What this row no longer carries**: the
   three `List` S1 methods and the template interpolation both landed (the list and print slices), the `str`
   parsers landed with the scalar-borrow slice (which built `stdlib_build` and
   `borrow_provenance_and_arms`), `str_to_string_coercion` builds as of the enum slice, and the owning
   passdowns built with the passdown-drop slice. `stdlib_parse`, the fixture the flattening of the `str`
   parsers moved to the float row, built when that row landed.
2. **A float payload in an aggregate** — **done** (the float-in-aggregate slice below). All six fixtures build
   and agree with the C backend byte for byte, plus `stdlib_parse`, the fifth this row acquired when the
   scalar-borrow slice lowered the `str` parsers: `receiver_methods`, `templates` (a `datac` holding a float
   field), `enum_basic`, `enum_container`, `if_let_basic`, `if_let_chain_same_scrutinee` (an `enum` whose
   *payload* is a float) and `stdlib_parse` (`Result<f64, ParseErr>`). The slice replaced the size-only
   classifier with a per-eightbyte one — `INTEGER`/`SSE`, `MEMORY` over sixteen bytes, an `enum`'s payload
   classified through its union — and gave every boundary (`xmm` parameters and returns, mixed register pairs,
   and the hand-written `Box`/`List`/`Map` element marshalling) the vector-register arm it needed. The ABI work
   the risk register calls the place silent miscompiles live was **not** complete before this slice, contrary
   to what the previous version of this paragraph asserted: it was complete for every shape the backend
   admitted, and an eightbyte reachable only through float fields was exactly the shape it did not admit. That
   is now true again, and a `.s`-level test asserts the register each row travels in, because a self-consistent
   wrong split is invisible to the differential sweep.
3. **A passdown (`pd`)** — **done**, including the owned ones: an owning passdown is freed from the generated
   `kelp_drop_pds` that `main` registers with `atexit`, the same handler the C backend gets for free, so
   `pd_owned`, `list_field` and `pd_argument_forwarding` build. See §5's passdown blockquotes.
4. **`List`'s own leftovers** — **done** (the sort/contains/slice slice), and the row's one leftover at
   `c3a8759` is the derived copy and order the newest commit gave a `datac`/`enum` element: the C backend
   emits `kelp_clone_<T>`/`kelp_cmp_<T>` beside every user type, and this backend's dispatch still gates on
   the element-only predicates, so `list_datac_clone_sort` refuses `List.clone` on a `datac` element. One
   fixture; see `plan/mk7-next-slice.md` §D.
5. **`Map`** — **done** (the map slice below). Its measurement worth keeping is that the ABI needed nothing:
   `$Map` is a `MEMORY`-class aggregate like `Bytes`, so every site that already handled one handled it, which is
   the fifth time a heap type has slotted into the aggregate work unchanged.
6. **Capability checks** — measured at `c3a8759`: **0** of the 25 refused fixtures name the check, and 10
   of the 201 fixtures the C backend builds import a gated module (`caps_time_rand`, plus the nine gated
   fixtures in row 1). The row is *masked* exactly as this file predicted, because the live-extern pre-pass
   runs before the gate refusal, and the earlier "7 fixtures" was the count when it was written. It stays
   the only row here where a gap would be a security hole rather than a wrong number; the matcher itself
   is measured to lower on this backend already, so what is missing is the call-site check and its glue.
7. **The long tail** — numeric conversions, printing `str`/`f64` and closures: the first two are gone as of item
   8's first slice, and closures are what is left (`FnRef`/`ClosureLit`/`IndirectCall`, 10 fixtures, plus the
   `stdlib_closure_callbacks`/`stdlib_list_callbacks` pair). The row grew by one with the enum slice, and the
   entry is worth naming: `try_drops_frame` was counted as *reachable* by `plan/mk7-next-slice.md`'s brief, and
   measurement says it is not — its last function wraps the same `?` the rest of the fixture pins in a `fun`
   literal, so the closure is now its first refusal. At `c3a8759` the row is exactly ten fixtures:
   `closure_basic`, `closure_datac_borrow_param`, `closure_move`, `fn_literal`, `fn_literal_capture`,
   `fn_value`, `stdlib_closure_callbacks`, `stdlib_combinators`, `stdlib_list_callbacks`, `try_drops_frame`.

Five corrections to this file's own text come from re-measuring it, and they are recorded rather than quietly
fixed, because §9's first lesson is that this plan has now been wrong in the same direction five times. A borrow
of a scalar reaches **0** fixtures on its own, not the three named above, because all three refuse first on an
`extern fun` and then need an `enum` — and with the three `str` parsers landed too it reaches **two** of the
three, not three, because the third needs a float payload in an aggregate (`Result<f64, ParseErr>`), which is
the next row rather than this one. "Printing an owned `String`" is really interpolation into a template, since `print` of
an owned `String` already builds. The acceptance criterion that named `str_to_string_coercion` as must-pass
for the map slice was wrong, and the enum slice is what finally built it — one slice later than the plan
predicted, and not for the reason either of those measurements gave. And the enum slice's own acceptance list
named `try_drops_frame`, which measurement says is a closures row rather than an `enum` one. The measured brief
for the next round, with the reachable fixture list for each candidate slice, is `plan/mk7-next-slice.md`.

*(Re-measured 2026-09-20 at `c3a8759`, and four more corrections come with it, recorded here in the same
place and the same way. The current-tense numbers this section gave (`283 of 329`, `240 of 289`,
`144 built of 168`, `175 fixtures`, `24 refused`, the ranking's `heap 13` and its empty `enum`/`datac` rows
with a closure row of 10) are all the float-aggregate slice's: the tip's are in the blockquote at the top of
this section. The heap row is 14 and has changed composition, which row 1 above now says. The capability
row is not 7 fixtures: 10 of the 201 fixtures the C backend builds import a gated module and 0 of the 25
refusals name the check, which row 6 above now says. And §13's annex names the wrong second prerequisite for
the gate artifact, which its own dated correction now says.)*

> **2026-09-20, this round's three specifications.** The rows above were measured before this
> round's specs and are unchanged; three new files decide them in detail, each at `bb2316e`.
> `plan/mk7-extern-bodies.md` (MK7 §5 item 5 and the heap row's extern-headed members) decides
> which `extern fun` bodies are this backend's own work, each body's shape, the ten slices and
> each slice's acceptance target. `plan/mk7-closures.md` (MK7 §5 item 6) decides the lowering of
> `FnRef`, `ClosureLit`, `Capture`, `FnToClosure` and `IndirectCall`, why the environment stays
> heap, and the four slices that reach the row's ten fixtures. `plan/mk7-capability-check.md`
> (MK7 §5 item 7) decides what a gated call site must emit, which assembly routines this backend
> owns, the deny path's exact text, and that the three `net` checks stay deferred while a `net`
> call keeps being refused.

Those rows are re-measured against the current backend and rank rather than partition: a fixture counts once,
under the *first* refusal its build hits, which is why the heap-type row below is larger than the container
feature it names (`str` is counted separately, but a `String` that a `str` argument is converted into is
counted with the heap row). The totals are in §5's table.

Two things §5's definition of done still requires that no amount of the above delivers: `bench/speed` no worse
than the measured `-O0` column (it still cannot be built for this backend, and §13's dated correction says
what is left in its way), and a memory gate that works for the asm backend, since the AddressSanitizer gate
instruments the generated C and is therefore C-backend-only. The second is delivered: `kelpc/tests/memory_gate.rs`
runs `valgrind` over both backends' binaries, 377 runs over 357 fixtures at `c3a8759`, with zero findings
(its form and its limits are in §5).

*(Corrected 2026-09-20, at `9c89b70`: the first of those two things is delivered too, and this sentence's
"it still cannot be built for this backend" is false. `bench/speed/speed.kp` builds under `--backend=asm`,
runs, and matches every checksum; the memory gate is now 408 runs over 364 fixtures, `c: 208 checked,
0 findings`, `asm: 200 checked, 8 refused, 0 findings`. See the `9c89b70` blockquote at the top of this
section and §13.)*

*(Re-measured 2026-09-20, at `926e33f`: the two delivery claims above hold, and the gate's own reading is
narrower than "delivered". All five check kinds emit and the refusal ranking's gated row is empty, so
nothing the gate refused is still refused for the gate; `bench/speed` builds and runs under both
backends and its checksums match; and the memory gate is 413 runs over 364 fixtures at `926e33f`,
`c: 208 checked, 0 findings`, `asm: 205 checked, 3 refused, 0 findings`. What §5's per-workload reading
still calls unmet is `fib`: 1.713 in §13's `9c89b70` table and 1.795 to 1.845 in this round's two
contended sweeps. The three fixtures still refused are `process_spawn`, `list_datac_clone_sort` and
`borrow_chain_through_a_borrow`.)*

**Environmental prerequisite:** `driver.rs` invokes `as` and `ld` by name, so `binutils` is a real dependency
now rather than something clang pulls in. Debugging hits (gdb, strace, valgrind) are in `/etc/nixos/tools.nix`.

---

## 1. Ground rules

These hold for every stage. Breaking one is how this becomes a rewrite instead of a replacement.

1. **`codegen_c.rs` is never deleted.** It becomes the permanent differential oracle: every fixture is
   compiled and run by both backends, and their stdout is compared. A backend is correct when it agrees with
   the C backend, not when it agrees with the author's intent.
2. **The 261 fixture files are the contract.** They are backend-agnostic by construction (compile, run,
   compare stdout), so they validate a second backend with no edits. Any fixture that has to change for the
   new backend is a bug in one of the two, and the diff says which. The suite is 248 test functions over
   those files — several tests cover more than one fixture, and a fixture that must be *rejected* is a file
   with no test of its own — so "the fixtures pass" is measured as **248 integration tests**, which is the
   number the stage gates below use.
3. **One target.** `x86-64`, SysV ABI, Linux. Nothing else is attempted until Tier 3 is done. Cross-compilation
   (the Backlog part of `roadmap.md` MK13-S4-M3) and a portable encoder are explicitly out of scope on this branch.
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
"~20 one-line externs". Both are stale. The integration suite is **248** tests over **261** fixture files
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
- **A double run of the fixture suite.** `cargo test` runs the 248 integration tests once per backend selected
  by the same env var, so a third CI job (or one job looping over backends) proves both. This is `roadmap.md`
  open item 26, and it is the thing that makes every later stage verifiable at all.
- **The seam documented in `driver.rs`.** `driver.rs` keeps owning "write the artifact and invoke the external
  tool that finishes it"; at Stage 0 that is still `cc`.

Definition of done: `cargo test` green on `main`, `--backend=c` and `KELPC_BACKEND=c` both select the C
backend, an unknown backend is rejected by name, and the integration suite can be run twice in one command
with the backend as the only variable.

## 5. Stage 1 — x86-64, assembly text, `as` + `ld`

> **Status: first slice landed, then f32/f64, then a register allocator (item
> 3), then ten slices of item 4's aggregates (a `datac` by value, nesting, the
> wider SysV classes, a `MEMORY`-class argument, owned heap fields, `Box<T>`,
> `List<T>` — the first container whose element can itself own — `Map<K, V>`,
> which closes the container set, `enum`, which closes the aggregate set
> except for a `datac` holding a float field, and then that float field,
> whose per-eightbyte `INTEGER`/`SSE` classifier closes item 4 outright),
> then the first half of item 8,
> then passdowns, then the extern path, then an owned passdown's `atexit` drop
> (the ninth, tenth and eleventh items — the first two added after the fact,
> because each was a blanket refusal at the top of `generate` that kept the
> feature out of codegen entirely),
> then a scalar borrow with the read and store through it, and the three `str`
> parsers it was the last prerequisite for.**
> `kelpc build --backend=asm` emits x86-64 assembly itself and finishes with `as`
> and `ld`, with no C compiler in the chain. Measured: **226 of 283 end-to-end
> tests pass**, and the C backend passes all 283 — so the differential gate is
> live (`the_two_backends_agree_on_the_scalar_fixtures`) and the coverage number
> can only rise from here. Every fixture the asm backend builds is compared
> against the C backend's own binary — by the differential sweep in
> `kelpc/tests/backend_coverage.rs` since §1.1's sweep landed, and by hand for the
> first slice that had any fixture to compare — which is the check the fixture
> literals cannot make: 132 fixtures, 132 identical stdout at this measurement.
>
> Covered: every integer width, `bool`, `char`, `f32`/`f64` arithmetic and
> comparisons (`ucomiss`/`ucomisd`, including the unordered/NaN case — see
> `tests/fixtures/asm_float_scalars.kp`), all unary and binary operators
> including short-circuiting `&&`/`||`, `if`/`else` in statement and value
> position, `while`, assignment, a borrow of a named scalar local and the read
> and store through it (`*p`/`*p = v` for an integer or float pointee), calls
> with up to six integer *and* eight float
> arguments classified independently per SysV, recursion, and `print`/`print!`
> of integers, characters, booleans, strings and floats — the last two as of
> item 8's first slice, whose blockquote records what printing a float turned
> out to actually require.
>
> *(Re-measured 2026-09-20 at `c3a8759`: the numbers in this status blockquote are the scalar-borrow
> slice's, and the tip's are these. The asm suite is **285 of 336** with the C backend passing all **336**
> end-to-end tests; the fixture ratchet is **176 built of the 201 fixtures the C backend builds, 25
> refused**; every one of the 176 is byte-identical to the C backend's own stdout; and the refusal ranking
> is **a heap type 14, a fixed-size array 0, an `enum` 0, a `datac` holding a float or heap field 0, a
> closure or a function value 10, a gated extern 1**. The recorded baseline in
> `kelpc/tests/backend_coverage.rs` is one fixture behind the sweep, which reports `borrow_two_phase` as
> built and unnamed. §0's blockquote carries the same measurement, and `plan/mk7-next-slice.md` is the
> brief that follows from it.)*
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
> `tests/projects/namespace`, whose `bumped(t: Tally, by: i32)` mixes a reference
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
> **The passdown slice — a `pd` as generated storage — is done (2026-09-19).**
> This is the row the ranking had never named until the fourth slice re-measured
> it, and it needs no runtime at all: a passdown is a file-scope object with one
> initializer, and the C backend's version is a `static` global plus a hidden
> pointer parameter per referencing function. The asm backend gets the same
> behaviour out of the parts it already had, plus two new ones.
>
> **What the TIR actually says**, which is what makes this cheap to mirror:
> `infer::PdInfo` gives every passdown one internal name, `pd#<qualified>`, and
> two spellings of it. Inside a function that references it, the name is a
> *hidden parameter* appended after the real parameters, and its type is always
> a borrow — the parameter's value is already the pointer to the storage — but
> which borrow follows the body, not just the declaration: `&T` when the
> function only reads the passdown, `&mut T` when it writes it, directly or
> through a function it calls (roadmap item 17). In `main` and in a `pd`
> initializer there is no such parameter, so the name is wrapped in a `Ref` and
> means the *global*. A read is `Deref` of that borrow in both cases, and a
> write is an assignment to the same `Deref`. So there are exactly three things
> to implement: a global to point at, a `Deref` that reads and writes through a
> pointer, and a name that resolves to whichever of the two is in scope.
>
> **The storage** is one object per passdown in `.bss`, emitted in
> initialization order under the same symbol the C backend uses
> (`kelp_pd__origin`), sized by `pd_storage_size` — rounded up to eight bytes
> the way a local's slot is, and `None` for a type with no storage at all (which
> is how a type the backend cannot lay out is still refused, now by name with
> its own type instead of by the blanket program-level refusal the earlier
> slices had). An *owned* passdown is stored here the same way and freed at
> process exit — see the drop blockquote below.
> `main` assigns them before its own body runs (`pd_init`, in the same order
> `program.pds` is in), which is the job the C backend's generated
> `kelp_init_pds()` does, and for the same reason: `main` is the one function
> that can have no hidden parameters, so it is the one place a passdown can be
> filled from nothing. An initializer that reads an earlier passdown therefore
> works, and `passdown_globals.kp` pins that order.
>
> **`Dest::Global(index)`** is the piece that makes a `datac` passdown work: a
> passdown initializer is *not* a `Let`, so it cannot write into a frame slot,
> and `eval_struct_into` needed a destination that is neither a frame address
> nor a pointer in a slot. An index into `Gen::pds` rather than the label keeps
> `Dest` `Copy`, which every site that threads it relies on; the label is looked
> up where the store is emitted. Every shape `eval_struct_into` already knew
> works through it — literal, named local and call — so a `datac` passdown
> initializes the same way a local does.
>
> **`Ident` resolution is where the two spellings meet.** `frame.lookup`
> answers for a hidden parameter as it would for any local (that is the whole
> trick: the parameter is an ordinary `&T`-typed local, so nothing about the
> prologue, the argument marshalling or the register allocator had to change),
> and when it finds nothing the name is looked up in `Gen::pds` instead. What
> that fallback *emits* depends on the passdown's own type, and the rule is the
> C backend's: a `str` passdown's storage holds the pointer, so its name loads
> the global (`&str` and `str` are the same `const char*`), while every other
> passdown's name *addresses* it, because that is the borrow `infer.rs` wraps it
> in. Two supporting details fall out: `value_kind`/`int_regs_for` now accept a
> reference to a *scalar* (the hidden parameter of a `pd var count = 0` is
> `&i32`, a one-word pointer like any other), and `ExprKind::Ref` learned that
> `&*p` is `p` (`origin.sum()` is `sum(&(*origin))`) and that a `Ref` of a name
> that is already a borrow is not an address to take.
>
> **Six tests moved**: five fixtures that were waiting on this row
> (`passdowns`, `passdown_datac`, `pd_shadowed`, `pd_var_copy_read_twice`,
> `pd_var_write`) and one new fixture. `passdown_globals.kp` pins what those
> five do not: an initializer reading an earlier passdown, and `bump()` — a
> function that names no passdown, has no hidden parameter, and therefore hands
> its callee the generated global's own address, which is the one shape where
> the two spellings of the same name are both live in one program. The three
> remaining passdown fixtures (`pd_owned`, `list_field`, `pd_argument_forwarding`)
> were refused for their passdown's *type* — `String`, `List<i32>` — and the
> drop blockquote below is what built them.
>
> Two narrowings this slice leaves, both named rather than silent. A `datac`
> passdown read *by value* (`val p = origin`) is refused: reading a struct out
> of a borrow needs a copy, field access through the same borrow already works
> (which is what the fixtures use), and half-doing it would be the silent kind
> of wrong. And a template with more than five non-float interpolations is still
> refused, which is a limit of the printing path rather than of this slice — it
> is in the long tail below, and no fixture is blocked by it.
>
> **The extern slice — the runtime starts arriving — is done (2026-09-19).**
> This is the plan's §0 ranking row 1, in the only form it had left: not an
> allocator in Kelp (still blocked on raw pointers, §8), but the *mechanism* an
> allocator, a formatter or any other libc-backed primitive needs — an `extern
> fun` this backend can call. It also removes the mask that had been distorting
> every ranking table above: `generate` refused any program containing *any*
> `extern fun` *declaration*, and `impl "std/str"` alone declares seven of them,
> so the first refusal 40 fixtures reported named a declaration none of them had
> to call.
>
> **An `extern fun` is now a call-site concern, and the call site knows nothing
> about it.** `extern_target` (`codegen_asm.rs`) maps a canonical key
> (`std.math.sqrt_raw`) to either a libc symbol or one of this backend's own
> routines; `Gen::call` then substitutes that symbol for `Gen::symbol(callee) →
> kelp_...` and everything else about the call is unchanged, because the argument
> marshalling is driven by the extern's *declared* signature — the same contract
> `externs.rs` keeps the C bodies honest about. Three shapes of extern exist in
> `std/*.kp` and two of them need nothing else: a libc function whose signature is
> already the extern's (`strlen`, and all fourteen libm transcendentals that
> `std/math` wraps — calling the same libm the C backend calls is what makes the
> two agree *bit for bit*, not approximately), and a C **macro** with no symbol
> to call (`isnan`/`isfinite`, which are a `ucomisd` parity test and an exponent
> mask here). The third shape — a body with a bounds check in front of the
> library call (`str.byte`, `str.substring`) — is a routine this backend owns,
> `strlen@PLT` plus the same panic message `externs.rs` writes. `kelp_panic`
> is emitted for one exactly as it is for `panic`/`assert`, through the same
> `needs_panic` flag.
>
> **A live extern with no entry here is still a refusal, and it names the
> declaration** — the message is the one the blanket check used to give, naming
> the `extern fun` and its key — so the discipline the
> blanket check was for is intact: the message moved from "this program has an
> extern" to "this program *calls* this extern". The 25 fixtures still refused
> that way are all heap-backed or gated: `alloc_raw` 18, the `str` parsers and
> `from_f64_raw` 5, `path.normalize_raw` 1, `time.now_unix_raw` 1.
>
> **The second half of this slice is `live_functions`: dead-code elimination at
> function granularity.** A Kelp program's function list is not a program's code
> — importing `std/str` brings in all 40-odd of its functions, and every one this
> backend cannot lower (a `String` parameter, a `Bytes` return) used to refuse
> the whole program even when nothing called it. The C backend emits them all and
> lets `cc` discard the unused ones, so refusing them meant refusing programs the
> oracle builds. The roots are `main` and every passdown initializer (and
> `FnRef`/`ClosureLit` are edges like any call); the walk is written as an
> exhaustive `match` on the TIR, so a new node kind cannot be silently missed.
> It is sound by construction — an unreachable function cannot run, print or drop
> — and what it costs is stated plainly: an uncovered shape appearing *only* in
> dead code is no longer a build error. That is the same trade the `datac` layout
> rule already makes (a `datac` that does not qualify is refused where a value of
> it is *moved*, not where it is declared), and it is what `escapes.kp` — which
> imports `std/str` and uses two of its functions — is the proof of.
>
> Three fixtures moved: `escapes` and `char_literals` (the `str` byte primitives,
> through a module whose other functions are all heap-typed) and `stdlib_math`
> (libm). All three are in the differential gate, because the values a
> transcendental produces are exactly the kind of thing two backends can agree on
> as text while disagreeing in the last bit.
>
> A fourth, deliberate change is in the *message*, not the coverage: the
> program-level `enum` refusal is gone, for the same reason the extern one went —
> `std/str` declares `ParseErr`, so every `impl "std/str"` program refused with
> "the enum `std.str.ParseErr`" whether or not it ever named one. `EnumLit`,
> `EnumTag`, `EnumField` and `Try` each refuse where they are reached and name the
> enum (recovered from the base's own type), so a program that genuinely uses one
> still fails with the enum's name.

> **The heap slice — `Bytes`, and the first memory gate — is done (2026-09-19).**
> This is §5 item 4's remainder arriving for a real heap type, plus item 5's first
> slice, plus the rest of item 8, all at once: `Bytes` is the type every other
> heap type is a variation on, and it needed all three at the same time.
>
> **One synthesized layout, and nothing else in the ABI had to change.**
> `codegen_c.rs`'s `kelp_Bytes` is `{data, len, cap}` — 24 bytes, align 8, so
> SysV `MEMORY` class, exactly like a three-`i64` `datac`. `build_layouts`
> inserts that layout under a key no user type can collide with (`$Bytes` is not
> a Kelp identifier), and `struct_layout` resolves `Ty::Bytes` through it. Every
> site that already handled a `Mem`-class `datac` — `value_kind`, `int_regs_for`,
> `Frame::declare`, `RetClass`, `memory_arg_area`, `eval_struct_into`, `emit_copy`
> — then handles an owned `Bytes` with no new special case at all: it is a local,
> a parameter, an argument in the caller's outgoing area, a return through the
> hidden pointer, and a `&Bytes` is a plain pointer. That was the *point* of item
> 4's third and fourth slices, and this slice is the first evidence they were
> enough.
>
> **Thirteen routines this backend owns**, emitted on demand the way the extern
> slice's two already were: `kelp_buf_reserve` (the doubling rule, `realloc`,
> `want + 1`, re-terminate), `kelp_bytes_drop`, `kelp_bytes_alloc_raw`,
> `kelp_Bytes__empty/from_str/len/is_empty/get/set/clone/push/append/clear`.
> Each is a transliteration of the matching C body in `codegen_c.rs`'s RUNTIME,
> panic messages and bounds checks included, and `std.mem.alloc_raw` — the plan's
> named "hand-written assembly routine for allocation, PLT-called" — is the
> malloc/zero-fill pair those routines already needed. `print`/`print!` of an
> owned buffer is `printf("%.*s", (int)len, data)`, with the temporary freed
> after the `printf` exactly as the C backend frees the one it hoists.
>
> **Drops.** `Gen` now carries the `DropPlan` `borrowck.rs` already computed and
> the function whose body is being written, and frees at the same four points
> `codegen_c.rs` does: block end, function body end, a `return`'s span, and the
> old value of an assignment. A value produced *before* a drop is parked in a
> frame slot while the drops run (`park`/`unpark`, the asm side of the C
> backend's `write_temp_then_drops`), a `&Bytes` temporary is hoisted into a
> never-reused `Frame::scratch` slot and freed after the call that borrowed it,
> and a target whose type is not `Bytes` is refused by name rather than skipped —
> a skipped `free` is a leak, and looking handled is exactly what this backend's
> refusals exist to prevent.
>
> **A memory gate exists now, and it is not ASAN.** §5's definition of done has
> asked for one since the plan was written; the AddressSanitizer gate instruments
> *generated C*, so it cannot see this backend, and the plan's standing answer was
> that the claim would have to be argued from Kelp's semantics instead. That is
> true of ASAN and false of the question: `valgrind --leak-check=full` runs on an
> asm-generated binary like any other, and it reports the ten heap fixtures clean
> — no leak, no double free, no invalid access — as it does for all 60 fixtures
> this backend now builds. The four fixtures whose valgrind output is not empty
> are the panic fixtures, whose *programs* print `panic: ...` to stderr; valgrind
> itself says nothing. That is a real gate of the kind the definition of done
> asks for, and it cost a script rather than a week.
>
> It is a test now: `kelpc/tests/memory_gate.rs` (commit `0747195`) sweeps every
> fixture the C backend builds and, where the asm backend accepts it, the asm
> binary too, parses valgrind's log per pid, and asserts on every pid including
> fork children — with the program's own `panic: ...` stderr kept structurally
> separate from valgrind's findings, which is the trap the first hand-run fell
> into. It is `#[ignore]`d, because §1.5 forbids making the default `cargo test`
> slower or needier and valgrind is not a declared dependency of this repository;
> `cargo test --test memory_gate -- --ignored` is the gate, and it skips with a
> printed note when valgrind is absent. It earned its keep before it was
> committed: it found a real leak in the *C* backend's `process.spawn_raw` (the
> forked child's `argv`, never freed on the `execvp` failure path, which ASAN
> cannot see because `_exit` bypasses LeakSanitizer), fixed in `dbaabc2` and
> pinned by a deliberate pre-fix rerun. The layouts it checks are §12.
>
> **`%` did not work below sixty-four bits, and no fixture had ever noticed.**
> A division's remainder lands in the *high* half of the dividend — `ah`, `dx`,
> `edx` — and this backend moved it into a 64-bit destination (`mov rax, edx`),
> which `as` rejects outright. It was a loud failure rather than a silent wrong
> number, but it meant `7 % 3` on an `i32` could not be compiled at all, and the
> only fixtures dividing a narrow value were refused for other reasons first —
> a *fixture* hole as much as a backend one. Fixed in the same commit as
> `int_mod_narrow.kp`, a new fixture covering `i32`/`u32`/`u8`/`u16`/`i64` with
> values chosen so a wrong remainder register or a missing extension prints a
> different number; it is in the differential gate. That is also why the test
> count moved by eleven rather than ten.
>
> **A finding this slice left in the oracle, and the fix that came out of it.**
> Inline literals wider than their own inferred type were folded at 64 bits by
> the *C* backend while this one truncated them to the expression's width first,
> so `print(1000000000007 % 1000000007)` printed `999993014` under
> `--backend=c` and `-727379961` under `--backend=asm`. The expression is `i32`
> on both sides (the C backend printed it with `%d`), so this backend was right
> and the C backend was computing in a width its own emitted format string
> contradicted; a `val x: i32 = 1000000000007;` local truncated identically in
> both, so the trigger was a literal used *inline* as an operand.
>
> It was recorded rather than fixed at the time, because the C backend is §1.1's
> oracle and changing it can move what an existing fixture asserts. The decision
> then taken was to fix it (commit `53ec30d`): `ExprKind::IntLit` now emits a cast to the literal's
> own type (the same narrowing the declaration path always did, and the same one
> `codegen_asm.rs` does), so both backends narrow a literal before the operation.
> **The corpus did not move at all** — 251 fixtures still pass, none of them had
> ever written a literal that did not fit its own type, which is exactly why the
> disagreement survived. `int_literal_width.kp` is the fixture that writes one.
>
> The footgun that fix does not remove, recorded so it is a decision rather than
> an oversight: a literal that does not fit its inferred type is narrowed
> *silently* — `print(1000000000007)` prints `-727379961`, by the same rule that
> always applied to `val x: i32 = 1000000000007`. Rejecting it instead would be a
> language decision (a diagnostic, or widening the literal's type when nothing
> constrains it), not a backend one.

> **The other bug this slice's spec listed is fixed too.** `plan/heap-runtime.md`
> (§12 here) flagged `String.with_capacity(0)` as its first uncertainty: the
> capacity reserve has nothing to do at zero, so `data` stayed NULL while `len`
> and `cap` were 0 — an empty `String` that crashes the first thing treating it
> as a C string. The spec called it "an edge, not a proven bug"; it was a bug,
> and a five-line one: both backends now allocate the single NUL byte that
> `String.empty()` allocates. `string_with_capacity_zero.kp` segfaults under the
> pre-fix C runtime and passes under both now, which is the check that makes it
> a fixture rather than an assertion.

> **The String/`View` slice — the text types, and the containers' prerequisite —
> is done (2026-09-19).** `$String` (24 bytes, `MEMORY` class like `Bytes`) and
> `$View` (16 bytes) join `$Bytes` in the synthesized-layout table, and
> `struct_layout` resolves both, so **the argument marshalling needed nothing
> new**: `View` is the first heap type on `AggClass::Two` (a register pair,
> `rdi`/`rsi` in and `rax`/`rdx` out) and it went through item 4's third slice
> without a line of special case in `Gen::call`. That is the second time a heap
> type has slotted into the aggregate ABI unchanged, which is the strongest
> argument yet that those four slices were the right shape.
>
> Twenty-eight routines this backend now owns, transliterated from `RUNTIME` and
> `externs.rs` with their panic text and bounds checks intact: `String`'s fifteen
> (`empty`, `from_str`, `from_bytes`, `into_bytes`, `len`, `is_empty`,
> `char_count`, `clone`, `push`, `push_str`, `reserve`, `with_capacity`, `clear`,
> `as_str`, `view`), `View`'s seven (`len`, `is_empty`, `get`, `slice`, `eq`,
> `to_string`, `str.view`), `kelp_utf8_valid`, `Bytes.as_str`, `str.from_f64_raw`,
> `fmt.hex_raw` and `fmt.bytes_raw`. `String#reserve` is `kelp_buf_reserve`
> directly — the same growth rule `Bytes` uses. `View` borrows and so has no
> drop; a `View` as a drop target is refused by name rather than skipped.
> Printing an owned `String` or a `View` is the same `%.*s` shape as `Bytes`.
>
> **A latent bug this slice found rather than introduced.** `View#to_string` is a
> method whose callee key contains `#to_`, and the conversion dispatch matched on
> that substring alone — so `view.to_string()` was on its way into the numeric
> conversion path. The guard now requires both halves of the name to be integer
> or float types, which is the same test `codegen_c.rs` makes. Nothing had hit it
> before because no earlier heap type had a method spelled that way.
>
> **Measured** (`046cb14`): the asm suite goes 146 → **156** (the C denominator 249 → 251: one
> test from the `%` fix, one for this slice's new fixture), the fixture-level
> build set 48 → **58**, the refused set 80 → **71**, and valgrind reports nothing
> over all 129 C-backend and 58 asm-backend binaries (131 and 60 once the two
> bug fixes below landed). Ten fixtures moved: nine
> that were refused for a `String` type they no longer mention (`string_basic`,
> `string_build`, `stdlib_str`, `stdlib_path`, `path_parts`, `stdlib_fmt`,
> `literal_operand_typing`, `float_integral_literal`, `view_released_at_scope_end`)
> and `string_view_basic`, a new fixture that pins `View`'s register-pair ABI,
> because a wrong pair is a wrong string rather than a build failure.
>
> **What did not come, recorded as a gap rather than a miss.** Three of the six
> fixtures this slice's brief named — `view`, `string_views` and
> `str_to_string_coercion` — are blocked by `List`, by `Option<usize>` (an `enum`)
> and by a `datac` with a `String` field, all three explicitly outside its scope;
> the agent reported its own criterion *not satisfied* rather than claiming the
> slice complete, which is the right behaviour and the reason the brief was wrong:
> an acceptance criterion for a delegated slice has to name only the fixtures that
> slice's own scope can reach. The `str` parsers were left out deliberately too —
> their out-parameter is a `mut` scalar borrow this backend cannot address yet, so
> mapping them would have moved the refusal from the declaration to the call site
> without unblocking anything, and `stdlib_build`/`stdlib_parse` stay refused for
> that reason.

> **The heap-field slice — a `datac` that owns a heap field, and the first
> generated per-`datac` drop — is done.** This is §5 item 4's remaining
> *non-`enum`, non-container* shape and item 5's second slice, at once, because
> they are the same thing seen from two sides: a `String` field is a nested
> aggregate in the layout, and the only reason it needs machinery a scalar field
> does not is that something has to free it.
>
> **The layout change is one line and the ABI change is none.** `FieldEntry`'s
> `Nested` variant became `Aggregate`, holding the field's own `Ty` as well as
> its offset and layout key, and `try_build_layout` now accepts *any* field whose
> own `struct_layout` exists — a nested `datac` of any size, or a `String`/`Bytes`
> field, whose layout is one of the synthesized `$String`/`$Bytes`/`$View` ones.
> The old nested-field size rule (`width_for_size`: the nested struct had to be
> exactly 1/2/4/8 bytes so one `mov` could copy it) is gone, because the copy is
> now `emit_copy`'s arbitrary-byte-count one either way. Everything downstream —
> `value_kind`, `int_regs_for`, `Frame::declare`, `RetClass`, `memory_arg_area`,
> `eval_struct_into`, `emit_copy` — handled a `String` field with **no new code at
> all**: a struct holding one is just a `MEMORY`-class struct, and the slice is the
> third time a heap type has slotted into item 4's ABI unchanged. That is worth
> stating plainly because it is the strongest evidence yet that the four slices
> were the right shape.
>
> **The generated drop.** `emit_drop` no longer refuses a `datac`; `drop_site_for`
> maps a type to a `DropSite`, which is either one of the fixed routines
> (`kelp_bytes_drop`/`kelp_string_drop`) or a generated `kelp_drop_<c_ident>` —
> the same symbol `types::drop_fn_name` gives the C backend's `write_struct`, so a
> stack trace and a differential run name the same function. The transitive
> closure (a `datac` holding a `datac` holding a `String`) is collected by
> `require_datac_drop` *before* the routine set is snapshotted, because
> `require_routine` after the snapshot links against a symbol nothing emitted —
> the same trap the `String.with_capacity(0)` fix documented. The body is
> `write_struct`'s: each owning field in declaration order, at its own offset,
> with the value's pointer kept in `rbx` across the nested calls.
>
> **One honesty fix the slice forced on itself.** A passdown of an owning type is
> refused by name because nothing here frees one at process exit, and that test
> used to be `matches!(ty, Ty::Bytes | Ty::String)`. A `datac` that holds a
> `String` is an owning type now, so the test became `ty_owns`, which is the same
> recursion the drop generator uses — otherwise a `pd` of such a type would have
> been accepted and leaked at exit, which is exactly the failure mode the refusal
> exists to prevent.
>
> **Measured:** the asm suite goes 158 → **161** of 254 (the C denominator moved
> by one for the fixture this slice added), the fixture-level build set 60 →
> **63**, the refused set 71 → **69**, the `datac` row 11 → **8**, and valgrind
> reports nothing over all 132 C-backend and 63 asm-backend binaries. Three
> fixtures moved: `string_field` (the plan's named smallest instance, whose first
> refusal had been `the return type Holder of make`), `generic_method_basic`
> (refused only for the `String`-instantiated `Pair` it declares — a generic
> `datac` over owned fields moved for free, which no part of this slice's brief
> predicted), and `datac_owned_fields`, the new fixture that pins a struct with a
> `String`, a `Bytes`, an `i32` that is deliberately not last, and a nested owning
> `datac` in it, dropped once each through a borrow, a move and a block scope.
>
> **What did not come, and this is a correction to the plan's own earlier text.**
> §5's heap discussion listed `str_to_string_coercion`, `box_field_take`,
> `map_datac` and `recursive_datac` as "unlocked" by this mechanism. They are not:
> each also needs `List`/`Map`/`Box` or an `enum` (recon confirmed all four by
> running their builds — `str_to_string_coercion` now stops at `List<String>`),
> and they were refused for *that* all along once the `String` field stopped being
> the first thing in the way. What this slice is, honestly stated, is the
> **prerequisite** for owned elements in containers, which is where the next
> movement has to come from. The `View` cannot be a field at all —
> `is_borrowed_field_ty` (`types.rs`) refuses one in a `datac`, pinned by
> `view_field.kp` — so the slice's fixture exercises it as a local borrow printed
> and then dropped unfreed, which is the only shape a zero-owning heap type has.

> **The box slice — `Box<T>`, the builtin owning pointer — is done.** This is §5
> item 4's `Box<T>` row and the last container that fits the ABI without a
> `List`-shaped per-instantiation element walk. It is also the type that makes a
> *recursive* `datac` expressible (`datac Node(val next: Box<Node>)`), so it is
> the first slice whose layout work is circular by construction.
>
> **One layout for every element type, and the circularity falls out of it.**
> `struct_layout(Ty::Box(_))` answers from a single `$Box` key — eight bytes,
> one pointer, `NULL` for empty — because a box is one pointer whatever it
> points at; only the *routines* are per-instantiation
> (`kelp_Box__<mangle>__of`), exactly the split `types::box_c_type` makes for
> the C names. That is what lets `Node`'s own layout resolve while `Node`'s
> fields are still being placed: the box field is eight bytes and its element's
> layout is deliberately **not** required at that point. So a `Box` field is
> placed by the same `FieldEntry::Aggregate` code a `String` field is, and an
> element this backend cannot size (`Box<List<i32>>`) is refused where it
> matters instead — by `Gen::call`'s `Box#` dispatch for a value a program
> builds, and by `emit_datac_drop` for one it only drops. `is_agg` gained
> `Ty::Box` in the same slice, because every method on a box is called through
> `&b`/`&mut b` and a borrowed local has to have an address.
>
> **Seven routines per instantiation**, transliterated from
> `codegen_c.rs::write_box` with its panic text intact: `__empty` (NULL),
> `__of` (`malloc(sizeof(T))`, copy the argument in), `__is_empty`,
> `__get`/`__get_const` (by value for a `Copy` element, by pointer otherwise —
> decided from the call's own result type, the same `matches!(ret, Ty::Ref{..})`
> `codegen_c.rs` uses, so the two backends cannot pick different spellings),
> `__set` (allocate if empty, otherwise free the value the slot held),
> `__take` (move the value out, free the slot, `NULL` it) and
> `kelp_box_drop__<mangle>`. Each is written against the SysV class of its
> element — `rdi` for a register-classed argument, the caller's outgoing area at
> `[rbp+16]` for a `MEMORY`-class one, `rax`/`rdx` or the hidden pointer on the
> way out — and keeps everything in frame slots rather than callee-saved
> registers, because the register allocator pins this program's locals to
> `rbx`/`r12`-`r15` and `malloc`/`free` are exactly the calls a caller-saved
> register cannot survive. `take` parks the value in its frame buffer before
> freeing, because `free` is entitled to clobber `rax`/`rdx`.
>
> **The bug this slice's own fixture found was not in the box code at all.**
> `list = Node(value = 2, next = Box.of(list))` miscompiled: the assignment
> staged the new aggregate straight into `list`'s own slot when the drop plan
> was empty, so the first field written (`value`) overwrote the old `list`
> *before* the field initializer that reads it (`Box.of(list)`) ran. C's
> compound-literal-then-assign order gives `codegen_c.rs` this for free; here it
> has to be explicit, so an aggregate assignment always stages the new value in
> a scratch slot and copies it home afterwards. The same hazard was found and
> closed on the `let` side in the same commit: the local is declared *after* its
> initializer is staged, so a shadowing declaration cannot read its own
> not-yet-initialized slot. Both were latent from item 4's third slice and no
> fixture had ever written a struct literal whose field initializer names the
> binding being assigned.
>
> **Measured:** the asm suite goes 161 → **164** of 255 (the C denominator moved
> by one for the fixture this slice added), the fixture-level build set 63 →
> **66**, the refused set 69 → **67**, the `datac` row 8 → **5**, and valgrind
> reports nothing over all 133 C-backend and 66 asm-backend binaries. Three
> fixtures moved: `box_recursive` (whose recursive walk, `take`/`set` round trip
> and 200-link chain exercise the recursive drop), `box_empty_panic`, and
> `box_owned_elements`, the new fixture that replaces and takes an owned
> `String`, an owned `Bytes` and a `datac` element and then walks a 100-node
> chain. `box_basic` stays refused, and for the honest reason: it boxes a
> `List<i32>`, so it is a `List` fixture wearing a box.

> **The list slice — `List<T>`, the first per-instantiation routine set over
> an element that can itself own — is done.** This is §5 item 4's `List` row,
> and it is the row nearly every remaining heap fixture named: `List<String>`
> alone was the first refusal of `str_to_string_coercion`, `stdlib_list`,
> `list_owned`, `move_param` and most of the rest of it. `Box` went first
> because it was the cheap one and needed no element walk at all; a list is
> where the per-instantiation routine set has to free the elements it owns, one
> at a time.
>
> **One `$List` key for every element type, and the ABI needed nothing.** The C
> typedef is `{data, len, cap}` — `data` at 0, `len` at 8, `cap` at 16, size 24,
> align 8 — which is byte for byte the `Bytes`/`String` header with a different
> pointee, so a list *value* travels on paths this backend already had: a
> `MEMORY`-class local sized `align_up(size, 8)`, a by-value argument in the
> fourth item-4 slice's outgoing argument area, a return through the hidden
> pointer. The struct's own size does not depend on the element at all — only
> the pointee of `data` does — so `list_layout()` answers from one key exactly
> the way `box_layout()` does, and the element's own layout is deliberately
> **not** required to place a list. That is what lets a `List` field sit inside
> a `datac` whose element type is not resolvable at that point, and it is why
> `List<Shape>` over an `enum` is a 24-byte value the ABI handles fine while
> everything a program can actually *do* with one (push it, get from it, drop
> it) is refused where it is done, by name.
>
> **The routine set, all of it per instantiation but one.**
> `kelp_List__<mangle>__{empty, of, len, is_empty, push, bounds, get,
> get_const, set, pop, clear, insert, remove, reverse}` and
> `kelp_list_drop__<mangle>`, named exactly as `codegen_c.rs::write_list` and
> `write_list_s1` name them and with their panic text intact
> (`List.get: index out of bounds`, `List.pop: the list is empty`,
> `List.insert: index out of bounds`, `List.remove: index out of bounds`).
> `get` versus `get_const` is decided from the call's own result type, never by
> guessing — the same `matches!(ret, Ty::Ref { .. })` split the box slice made.
> The one routine that is **not** monomorphized is the growth helper,
> `kelp_rt$list_reserve(self, want, elem_size)`: start at four elements, double
> until the wanted length fits, `realloc`, panic with `out of memory` on
> `NULL`. `codegen_c.rs` writes one of those per element type because the
> element is baked into its C signature; here the element size is a third
> argument, which makes this the one place the two backends' symbol sets
> deliberately differ.
>
> **Ownership, which is the whole reason the element walk exists.** `set` drops
> the element it overwrites before storing the new one, `clear` drops elements
> `0..len` and then sets `len = 0`, and the generated drop is `clear` plus
> `free(data)` plus zeroing `data` and `cap` — `codegen_c.rs`'s three bodies,
> instruction for instruction. The element's own drop is resolved through
> `drop_site_for`, so a `String`, a `Bytes`, a generated `datac` drop, another
> box and another list are all the same call the rest of this module already
> makes. A `List<T>` always owns its buffer — `empty()` is the only list with
> nothing allocated, and it still has a header to free — so `ty_owns(List)` is
> unconditionally true and no path here can skip a drop and call it handled.
> A `pop` or a `remove` whose element is `MEMORY` class hands the element back
> through the hidden return pointer rather than in a register, which is the
> shape `List<String>` has: the receiver then arrives in `rsi` and the index in
> `rdx`, and that shift is the one place in the set where the argument
> registers are not the obvious ones.
>
> **Deliberately refused, and named so it stays a visible gap:**
> `List#sort`, `List#contains` and `List#slice`, each because it needs an
> element-level comparison or clone this backend has no way to express yet —
> which is what keeps `list_slice_bounds`, `list_slice_insert` and
> `stdlib_sort` refused. An owning `pd` passdown is still refused too (nothing
> here frees one at process exit, and the C backend gets that from `atexit`),
> so `list_field` and `pd_owned` stay out even though the type behind them now
> builds. `Map` is untouched.
>
> **Measured:** the asm suite goes 164 → **182** of 260 (the C denominator moved
> from 255 for the five fixtures and tests this slice added), the fixture-level
> build set 66 → **85**, the refused set 67 → **54**, of the 139 fixtures the C
> backend builds (was 133), and valgrind reports **zero findings and zero leak
> bytes** over 139 C-backend and 85 asm-backend binaries. Fourteen fixtures
> stopped refusing, and they were held out by three spellings of the same row: a
> boxed `List` (`box_basic`, `box_field_take`), a `List` in a signature
> (`for_loop`, `generic_function_basic`, `move_param`, `path_polish`,
> `recursive_datac`, `stdlib_list`, `view`), and the list itself (`list_basic`,
> `list_datac_element`, `list_owned`, `list_pop_empty`,
> `list_owned_replacement`). Five more are new, because this slice's own scope
> needed them to exist: `list_element_widths` (a one-byte `bool`, an eight-byte
> `i64` and a twelve-byte `Trio` — not a power of two, so copying one is two
> instructions rather than one), `list_growth_and_rebuild` (three hundred pushes
> across six `realloc`s, a clear-and-refill, then a `List<String>` drained by
> `pop`), `list_insert_remove_shifts` (`insert`/`remove`/`reverse` shifting
> twenty-four-byte owned elements), and `list_insert_bounds` and
> `list_remove_bounds` for the two panic messages nothing else reached.
>
> **One gap item 3's exclusion list had to close for the new type.** `is_agg`
> did not name `Ty::List(_)`, so a list local was a register candidate — and a
> list local is 24 bytes that every method on one takes `&self`/`&mut self` to,
> so a pool register would have been handed a value that needs an address. It
> failed loudly and at once (`agg locals are stack-only`, on the first fixture
> that compiled) rather than miscompiling, which is what makes it a gap in an
> exclusion list rather than a wrong answer.
>
> **Three more things the slice's own work found, recorded as found rather than
> introduced.** `emit_elem_return`'s `MEMORY`-class arm loaded the hidden
> return pointer into `r11` — the same register that held the source address —
> so the copy was `r11` to `r11`, and a `List<String>.pop()` handed back
> twenty-four uninitialized bytes; a two-line hand-written probe
> (`print(names.pop())` printing an empty line where `element` belonged) found
> it, and the destination now goes through `r10`. `drop_site_for` *names* the
> fixed `Bytes`/`String` drops without registering them, and both
> `require_list_fn` and — latently, since the box slice — `require_box_fn`
> assumed it did the registering, so a `List<String>` or `Box<String>` whose
> only drop path is the generated one linked against a `kelp_rt$string_drop`
> nothing emitted; both now register the routine the site names. And
> `addr_of_struct` accepted a `&datac`-typed base but not a by-value aggregate
> temporary, so `xs.pop().a` and `xs.remove(0).b` — unreachable until
> `List#pop`/`List#remove` started handing the element back by value — were
> refused; they now stage the temporary in a fresh `Frame::scratch` slot and
> `lea` it, which is the fourth item-4 slice's mechanism for a borrowed
> temporary applied to the by-value case.

> **The map slice — `Map<K, V>`, the container that closes the set — is done.** This is §5 item 4's `Map`
> row and item 5's third slice, and it was the row that replaced the heap as "what is left": six fixtures named
> a `Map` type as their first refusal, and §12 already had its layouts, invariants, routine set and drop order
> written down against the C source.
>
> **One layout key, and an entry layout that finally depends on a type parameter.** `$Map` is 56 bytes — seven
> eight-byte fields, `entries`, `entry_count`, `entry_cap`, `len`, `index`, `index_cap`, `index_used` — `MEMORY`
> class, and independent of the key and value types, exactly as the C typedef is. So a map *value* travels on
> paths this backend already had, and the ABI needed nothing: **this is the fifth time a heap type has slotted
> into the aggregate work unchanged**, and the fifth piece of evidence that those four slices were the right
> shape. What is new is the entry, because the key is stored inline: the value pointer sits at
> `align_up(size_of(K), 8)` and the live byte follows it, which is 40 bytes for a `String` key and 24 for a
> scalar one. An entry is only ever an array element, never a by-value argument, so its SysV class is never
> exercised — only its size and offsets matter.
>
> **Fifteen routines per instantiation, one generated drop, three shared helpers this backend owns.**
> `kelp_Map__K__V__{new, find, rehash, insert, has, get, get_const, get_or, remove, clear, len, is_empty, keys,
> values}` and `kelp_map_drop__K__V`, named exactly as `write_map` names them, plus `kelp_map_hash` (FNV-1a over
> the key bytes), `kelp_map_key_eq` and `kelp_map_mix`. `get` versus `get_const` is decided from the call's own
> result type rather than guessed, the rule the box and list slices established. `keys()` and `values()` return a
> `List`, so the matching `ListFn` is registered *before* the routine set is snapshotted — the trap that would
> otherwise link against a symbol nothing emitted. A `String` key arrives as `const char*` and is stored as an
> owned copy rather than the caller's pointer; the drop frees each live value, then the key, then both arrays,
> then zeroes the header.
>
> **A `datac` holding a `Map` field came for free, uninvited.** Because `$Map` resolves through `struct_layout`,
> the existing `FieldEntry::Aggregate` path places it, and `map_datac` moved out of the "a `datac` holding a
> float or heap field" row without being part of the brief. The row went from 3 fixtures to 2.
>
> **What it did not reach, and the brief that was wrong.** `str_to_string_coercion` — the sixth fixture the row
> had been credited with, and one the acceptance criterion named as must-pass — is **refused**. Its first refusal
> is an `if` in value position producing a `MEMORY`-class value, and then an `enum`; neither is a `Map`, and the
> backend writer reported it as unreachable rather than claiming the slice complete. That is the third time this
> file's own text has been wrong in this direction, so it is recorded in §0 as a correction rather than quietly
> fixed. Still refused by name after this slice: `List#sort`/`#contains`/`#slice`, the `str` parsers' mutable
> out-parameter, an owning passdown that needs an exit handler, and the `fs`/`io`/`net`/`path` extern bodies.
>
> **Measured:** the asm suite goes 182 → **193** of 266 (the C denominator moved from 260 for the six fixtures
> this slice added), the fixture-level build set 85 → **97** of 145, the refused set 54 → **48** — the heap row
> 25 → 20, the `datac` row 3 → 2, `enum` unchanged at 18, closures at 7, the gated extern at 1 — and valgrind
> reports zero findings and zero leak bytes over all 145 C-backend and 97 asm-backend binaries. Twelve fixtures
> moved: the five the heap row had been carrying for `Map`, `map_datac`, and six the slice wrote for the growth
> and rehash policy, tombstones and their reuse, the entry key widths, a replace that must drop the old value,
> the whole owned round trip, and a clear followed by reuse.
>
> **The measurement worth more than stdout.** Every map fixture's heap allocation profile is identical between
> the two backends — 32 allocations and 7008 bytes for the growth fixture, which crosses two index rebuilds with
> tombstones present — so the rehash count and the index-capacity sequence are pinned, not just the numbers
> printed. All seven silent-miscompile risks this plan lists for the ABI were checked against the C backend, with
> one caveat recorded honestly: a tombstone read as empty makes a lookup stop early, so the observable failure is
> a missing key that panics rather than a resurrected value, and it is the fixture that catches it because `find`
> does check the live byte.
>
> **Unpinned, and recorded as a risk rather than a reassurance:** a 16-byte, a 1-byte and an owning `datac` value
> are hand-probed clean but pinned by **no fixture**, and `get`'s by-value path hard-codes `AggClass::One`, which
> is safe only because every reachable by-value value is a numeric, `bool` or `char`. `Map` with an `f64` value is
> refused by `elem_size`, and a `View` or `str` value by the front end.

> Deliberate choices carried over unchanged from the first slice, each a
> thing later work still replaces: **assembly text** rather than machine code
> (tier 2), and **refusal over silence** — every uncovered construct is
> reported by name, so the suite measures coverage instead of hiding
> miscompiles. Capability-gated calls are refused rather than emitted
> unchecked: that is the one place an unimplemented feature would be a
> security hole rather than a wrong number.
>
> **The enum slice — `enum`, the tagged union, and the generated drop that
> switches on the runtime tag (MK7 §5's ninth slice of item 4) — is done
> (2026-09-19).** It is the last aggregate shape this backend admitted nothing
> of, and the one with the largest reach left: every signature that names an
> `Option`/`Result`, and `std.option`/`std.result` are enums, was refused for
> the `enum` it mentioned. What made the slice small rather than a second ABI is
> that an `enum` **is** an aggregate: `AggClass`/`agg_class`/`Dest`/`emit_copy`/
> `eval_struct_into`/`marshal_elem` place, copy and marshal it with no new code
> once its layout exists, so the new work is the layout, four node kinds, and a
> drop body that reads the tag.
>
> **The layout is `write_enum`'s typedef, spelled out.** An `int32_t` tag at
> offset 0, then a union of one payload struct per variant at
> `align_up(4, union_align)` — **eight**, not four, for any enum whose widest
> payload holds an eight-byte scalar, which is most of them — sized to the
> largest member and rounded up to the enum's own alignment. The tag offset is 0
> for every `enum` whatever the payloads are, the union's offset is not, and
> that asymmetry is the whole of what this slice got wrong twice before its own
> fixture was written (`Narrow`, a union at 4, is one `u8` different from
> `Eight`'s at 8). `build_layouts` runs its fixed point over `program.enums`
> beside `program.structs`, so a payload that is a `datac` or another `enum`
> resolves in either textual order; a `StructLayout` gained
> `variants: Option<Vec<..>>`, `None` for a `datac` and for the synthesized heap
> layouts, so one lookup still answers for both kinds; and every payload offset
> is pre-rebased onto the enum's own base, which is what a reader already holding
> the value's address in `r11` needs. A payload this backend cannot place — a
> `float`, above all — leaves the whole declaration without a layout, which is
> what keeps the four float-payload fixtures refused *by name* rather than
> misplacing a field.
>
> **Four node kinds and a drop.** `EnumLit` writes the tag and then the chosen
> variant's payload, positionally, with the destination-awareness the aggregate
> slice already had; `EnumTag` is one `cmp` against the word at offset 0 of an
> address `addr_of_struct` computes; `EnumField` loads a payload scalar at its
> own offset, and a *borrow* of one — which is what an owned payload binding is,
> `Word(text) -> text.len()` being `&v.kelp_v.kelp_v_Word.kelp_text` — is an
> `add r11, offset`. The generated per-`enum` drop is a compare chain on the
> runtime tag, one case per variant that owns, each case dropping that variant's
> owning payload fields in declaration order, under the same symbol
> `types::drop_fn_name` gives the C backend, and **with no default arm**,
> mirroring the C `switch` on purpose. That body is the one thing an enum's drop
> shares nothing with a datac's — a datac's knows statically which fields own,
> an enum's does not until the tag is read — and it is the first drop in this
> backend that depends on a *runtime* value.
>
> `?` (MK5-S6-M1) is the fourth kind: evaluate the operand once into a slot, test
> the failure tag, and on the failure arm rebuild the error payload in the
> enclosing function's own return type — at whichever place that class says its
> return value lives (the hidden pointer, the register-pair slot, or `rax`) — run
> the frame's `DropPlan::at_try` drops, and jump to the epilogue. The success arm
> moves the payload out of the temporary, which is never dropped, exactly as C's
> statement-expression does. An owned success payload (`Option<String>`) travels
> this way without ever being truncated to a register, which is why the node is
> one of `eval_struct_into`'s shapes rather than only `expr`'s.
>
> **Two bugs, neither of them the enum work, both found by fixtures it made
> reachable — and the first is the most interesting thing in the slice.** The
> epilogue was `add rsp, frame_size` followed by the pops, which is correct only
> if `rsp` is exactly at the frame's bottom when the jump to `return_label`
> happens — true for a `return` statement, false for the first *mid-expression*
> early return this backend has ever had, `1 + half(n)?`, where the left operand
> of `+` is still pushed. The `ret` popped the saved `rbp` instead of the return
> address and jumped into the stack; `try_operator.kp` segfaulted before printing
> anything. The epilogue now rebuilds `rsp` from `rbp` and loads each saved pool
> register from the offset `push` wrote it at, which makes it independent of how
> the return was reached. This is the failure mode §10's risk register is about in
> the shape it had not yet taken: not a wrong eightbyte split but a wrong stack
> pointer, latent since the register allocator's own `rsp`-save bug and
> unreachable until an expression could return.
>
> The second is in the *aggregate* machinery, and it is a staging bug rather than
> a lowering one: every value staged for a `Two`-class call argument or a
> `Two`-class `return` was built in one shared `Frame::agg_slot`, on the reasoning
> that no two uses overlap in time. They do when one nests inside the other — a
> field initializer is an expression, and `Node(value = 2, next =
> Box.of(Node(...)))` stages the inner node into the same slot while the outer one
> is half written. `generic_recursive` printed 7 for a three-node chain's sum of
> 6, and `tests/fixtures/nested_literal_staging.kp` is the A/B: the previous
> compiler prints `7, 10, 10, 20, 13` where the sum is `6, 10, 9, 19, 13`. The
> three sites are `struct_lit`, `enum_lit` and the `Two`-class argument, and each
> now reserves a slot of its own through `Frame::scratch` (never reused). The
> `Two`-class *return* keeps the fixed slot, because the epilogue reads it from a
> fixed location, and it is safe there for the reason the others were not:
> nothing else can stage into it while it is being written.
>
> Two smaller additions came out of the same measurements, and both are
> mechanisms rather than enum features. `eval_struct_into` — the
> destination-aware half of expression lowering — gained `If` and `Block`,
> because a wide value in those positions had no lowering at all: a `when` in
> value position is a `Block` around an `if` chain, so `val w: String = when o {
> ... }` needed it, and so did `enum_owned`'s own `val t = if c { Token.Word(..) }
> else { Token.Blank }`. And `field_read` now loads a *one-eightbyte* aggregate
> field whole rather than refusing it, because one eightbyte is a value `rax` can
> hold — which is what lets an enum at that class be read out of a `datac` field
> by value. As a side effect, `str_to_string_coercion` builds: it had been refused
> since the heap slice for an `if` in value position producing a `MEMORY`-class
> value, which is the first of those two additions.
>
> Fixtures this slice wrote, five, one per mechanism its own scope needed and no
> existing fixture covered: `enum_layout` (all three classes, each with a payload
> that only reads back correctly at the offset clang gives it, passed by `move`
> and returned by value, plus an enum at each class inside a `datac`),
> `enum_in_containers` (the same enum as a `List` element, a `Map` value and a
> `Box` element, with an owning payload, plus an enum nested inside an enum),
> `enum_liveness` (two scalars alive across each of the four new node kinds — the
> regression test for the liveness gap that has now been found twice), and
> `try_frame_drops` (`?`'s early return freeing a frame that owns a `String`, in
> an `if` expression, an inner block, a loop body and a payload-less `Option` —
> `try_drops_frame`'s mechanism without the `fun` literal this backend refuses),
> and `nested_literal_staging` (the staging bug above).
>
> **Deliberately still refused, and each by name:** a `float` payload — the float
> aggregate slice's job, and the four fixtures it leaves are `enum_basic`,
> `enum_container`, `if_let_basic` and `if_let_chain_same_scrutinee`, which the
> next-slice brief had already named as expected-blocked; closures and function
> values, which is where `try_drops_frame` now sits (the brief counted it as one
> of this slice's thirteen reachable fixtures, and measurement disagrees: its last
> function wraps the same `?` in a `fun` literal, so the closure is its first
> refusal); `List.sort`/`contains`/`slice`; a `str` parser's `mut` out-parameter;
> the `fs`/`io`/`net`/`path` extern bodies; and a gated extern. An aggregate
> payload field read *as a plain value* is still refused unless it is one
> eightbyte wide, and a `?` whose success payload is wider than one eightbyte in a
> position that can only hold one is refused rather than truncated —
> `try_expr`'s `dest` is what the four contexts that can hold a wide value pass.
>
> Measured: the C suite is 278; the asm suite is **219 of 278** (was 200 of
> 273), with no test that passed at 200 failing now. The fixture ratchet is
> **125 built of 158, 33 refused** (was 106/153/47), every one of the 125
> compared against the C backend's own binary byte for byte, and the memory gate
> runs `valgrind --leak-check=full` over 283 binaries — 158 C and 125 asm — with
> zero findings. The ranking below moved from `heap 25, enum 18, datac 3, closure
> 7, gated 1` to `heap 16, enum 4, datac 2, closure 10, gated 1`: the `enum` row
> is the four float-payload fixtures and nothing else, the closure row grew by
> one (`try_drops_frame`), and the heap row lost the `str`-coercion fixture the
> `if`-in-value-position addition fixed.
>
> **Where the remaining refusals rank**, re-measured after the scalar-borrow
> slice, and
> reproducible instead of hand-run: `kelpc/tests/backend_coverage.rs` sweeps
> every fixture the C backend builds, refuses to let a refusal go unnamed, keeps
> a ratchet on the set the asm backend builds, and prints this exact table under
> `cargo test --test backend_coverage -- --ignored --nocapture`. The refused
> count is 31 of the 163 fixtures the C backend builds (132 built, all agreeing
> byte for byte; the three fixtures the enum slice's defect fix added are all
> built, so they moved the denominator and left the refusal count alone). The
> categories are the *feature* in the way, read off each
> fixture's first refusal message:
>
> | blocker (the backend's first refusal) | fixture files |
> | --- | --- |
> | a heap type — an `fs`/`io`/`net`/`path` extern whose body needs the heap, a `List` S1 method, or an owned `String` interpolated into a template | 13 |
> | `enum` — the fixtures whose *payload* is a float, which is the float-aggregate row seen from inside a tagged union (`stdlib_parse` joined the four when its `std.str.parse_f64` became reachable) | 5 |
> | a closure or a function value | 10 |
> | a `datac` holding a float field | 2 |
> | a gated extern (`time.now_unix_raw`, refused for the gate rather than the symbol) | 1 |
>
> The table above had gone stale by two slices when this one was measured: it
> still said 54 refused of 139 after the map slice had moved it to 48 of 145 and
> the passdown-drop slice to 47 of 152, both of which §0's narrative recorded
> without this table following. That is the same direction this plan has been
> wrong in four times now — a number written once and quoted forward — so the
> numbers above are the re-measurement, and §0's own leading sentence was
> corrected at the same time (`195 of 272`, from the list-slice era, while the
> ratchet beside it said 200 of 273).
>
> Re-measured after the heap-field slice the movement was refused 71 -> 69,
> built 60 -> 63, the `datac` row 11 -> 8, and the heap row 36 -> 37 because
> `str_to_string_coercion` stopped at its `List<String>` rather than at the
> `String` field of the `Doc` in front of it. After the box slice it is refused
> 69 -> 67, built 63 -> 66, and the `datac` row 8 -> 5 — the three fixtures that
> left it are `box_recursive`, `box_empty_panic` and `box_owned_elements`, whose
> first refusal used to name the `datac` holding the box. The `enum` row moved
> 17 -> 18 for a reason that is an artefact of the classifier rather than of the
> backend: with the `Box` out of the way, one fixture's first refusal is now an
> `enum` two steps further in. After the list slice it is refused 67 -> 54 and
> built 66 -> 85, and every row moved with it, because a `List` type was standing
> in front of all of them in one spelling or another: the heap row 37 -> 25 (the
> fourteen the list slice landed, less `str_to_string_coercion`, which stopped at
> the `Map` behind its list), the `datac` row 5 -> 3, the closure row 6 -> 7 (a
> `List<closure>` fixture and its `Box<closure>` sibling, whose first refusal is
> now the closure rather than the list), and `enum` unchanged at 18 — the one row
> this slice did not touch, because an `enum` with an owning payload has no
> routine set to inherit from a list. After the map slice it is refused 54 -> 48
> and built 85 -> 97; after the enum slice, refused 48 -> 33 and built 97 -> 125;
> and after the scalar-borrow slice, refused 33 -> 31, built 125 -> 129, the heap
> row 16 -> 13 (the two fixtures it lowered, `stdlib_build` and
> `borrow_provenance_and_arms`) and the `enum` row 4 -> 5, because `stdlib_parse`
> now stops at `Result<f64, ParseErr>` instead of at the `str` parser in front
> of it.
>
> The numbers do not match the hand tally two blockquotes up, and the reason is
> the classifier rather than the backend: the tool reads the message a fixture
> was refused with, and one message (`the datac X (it is too large, or holds a
> field this slice cannot lower yet)`) covers both a float field and a heap
> field, so it lands in the `datac` row here where the hand tally split those
> cases; conversely a `&Shape` parameter is an `enum` to the tool and was
> counted as heap by hand. Both are honest readings of the same 71 fixtures;
> the tool's is the one to quote, because it is the one anyone can re-run.
>
> **The heap is no longer the whole of what is left, and the row that replaced it
> is `Map`** — and, since this paragraph was written, `List` itself has landed
> too. What the list slice did **not** do is the fixtures this ranking's earlier
> version credited it with: `str_to_string_coercion` stopped at its
> `Map<String, String>` rather than finishing, `stdlib_sort`/`list_slice_insert`/
> `list_slice_bounds` need `sort`/`contains`/`slice` (refused by name),
> `list_field`/`pd_owned`/`pd_argument_forwarding` need the `atexit` handler, and
> `stdlib_build`/`stdlib_parse` still need the `str` parsers' `mut` out-parameter.
> The honest statement is that a container's layout and routine set is the
> **prerequisite** for the fixtures that name one, not a substitute for the
> mechanism behind it, and that this plan has now been wrong in the same direction
> twice: it credited a slice with the fixtures its successor reaches.
>
> What is left in that row is therefore `Map` — the list landed with the list
> slice above, and the annex's `List` half is now history —
> plus **an `enum` that holds a heap type**, which is the other half of the
> `enum` row too, since `Result<i64, String>` and `Option<String>` are enums whose
> payloads own.
>
> Behind that, in the order the refusals rank themselves, and measured by
> building each of the 25: `Map` itself (six fixtures, `str_to_string_coercion`
> among them — its first refusal is the `Map<String, String>` behind its
> `List<String>`; the layouts are in §12, and `keys`/`values` return a `List`, so
> it lands on top of the list slice rather than beside it); the `fs`/`io`/`net`/
> `path` extern bodies (nine); an owning `pd`, which needs the `atexit` handler
> (`list_field`, `pd_owned`, `pd_argument_forwarding`); the three `List` S1
> methods refused by name (`stdlib_sort`, `list_slice_insert`, `list_slice_bounds`);
> the `str` parsers and `from_f64_raw` as routines, which now
> need only a `mut` scalar borrow out-parameter to be expressible at all
> (`stdlib_build`, `stdlib_parse`, `borrow_provenance_and_arms`); and printing an
> owned `String` as a value (`heap_temporary_print`). Behind the heap row:
> `enum`'s eighteen fixtures; then
> the two things
> `bench/speed` needs before §3's gate can be measured at all — the `std/time`
> externs *and* the capability check that gates them (§5 item 7), plus the scalar
> `mut` borrow that `float`/`fib` take as a counter. None of those last two
> unblocks a single fixture, which is why they keep losing the ranking to the
> heap row despite being on the gate's critical path.
>
> *(That paragraph is the heap-era ordering it was written in, corrected here
> rather than rewritten: `Map` and the owning `pd` it names as still ahead both
> landed in the two slices after it, `str_to_string_coercion` builds as of the
> enum slice — so none of the three remains — and `enum`'s eighteen are four, all
> of them float payloads. The current list is §0's "What is next", and the
> current table is the one above.)*
>
> The passdown row is gone from this table, which is the whole measurement of
> the passdown slice: the three fixtures that still name a `pd` in their refusal
> are refused for the type it holds (and, for the `List` one, for the exit handler
> that type would need), so they are heap rows now. And the
> `extern fun` row is gone as a *row* while 12 of the 25 remaining heap fixtures
> still name an extern in their message: that is the extern slice working, not a
> leftover — the refusal
> is now of a call to an extern this backend cannot supply, which is the same
> statement as "this program needs the heap". Before that, the
> two earlier re-measurements had already removed three rows — `str` printing,
> `f64` printing and the numeric conversions, all at zero refused fixtures — and
> corrected an `extern fun` from 36 to 40, because the refusal was of the
> *declaration* rather than of `alloc_raw` specifically, which is also why the
> extern slice went after the declaration rather than after `alloc_raw`. The
> capability-gated row (7 fixtures) is unchanged and still real, and it is still
> **masked**: every one of those fixtures imports a gated module and calls an
> extern of it, and the live-extern check in `generate` runs before the gate check
> in `call`, so the gate is still not the next thing in any of their way. It will
> stop being masked the moment the extern it calls has a body — which is a
> property of this ordering, not a claim about the gate work.
>
> *(Corrected 2026-09-20 at `c3a8759`, an addendum to the paragraph above: its masking claim measured
> true and its count measured false. The capability check is the first refusal of **0** of the 25 refused
> fixtures, and **10** of the 201 fixtures the C backend builds import a gated module (`caps_time_rand`,
> `fs_errno`, `fs_fallible`, `fs_symlink_kind`, `net_loopback`, `net_poll`, `net_resolve`, `net_timeout`,
> `path_absolute`, `process_spawn`). The ranking's own gated-extern row is **1**, because that heuristic
> keys on `std.time.now_unix_raw` alone, so "7 fixtures" is wrong at this commit in both readings.
> Measured too: the matcher (`std/rt.kp`'s `glob_match`, `host_match` and `path_normalize`) lowers on this
> backend today when copied into an ordinary module, so the missing work is the call-site check and its
> glue (`runtime/cap_glue.c` is 182 lines of C), not the matcher. See `plan/mk7-next-slice.md` §C.)*
>
> The first row is the argument for §8 being a *parallel* track: the single
> biggest unlock is the runtime, not the code generator, and the runtime is the
> half of this work that is backend-independent. The extern slice is what turned
> that observation into a mechanism — the runtime pieces this backend can already
> call (libm, `strlen`, and whatever else a routine of its own fronts) are
> reachable now, and the ones it cannot are the ones that need the heap first.
>
> **The scalar-borrow slice — a borrow of a scalar local, and the three `str`
> parsers — is done (2026-09-19).** It is §5 item 4's last named gap that is not
> a float ("a borrow of anything whose address is not a slotted `datac` ... (a
> scalar `&i32`)") together with item 10's `str` parsers, which were waiting on
> exactly that borrow. The two are one slice because neither is useful alone:
> each parser's `out` is a `mut i64`/`u64`/`f64` parameter, and a `mut` scalar
> parameter is a plain pointer whose caller's half is `&mut local`.
>
> **The borrow is a `lea`, and the one new thing is that the local has to have a
> slot.** `ExprKind::Ref` needed no storage for it: a named scalar local is
> addressed by `lea rax, [rbp+slot]`, the same instruction it already emitted
> for a named `datac` local, and the value that comes back is an ordinary
> integer-typed pointer, so the call marshalling needed no change at all. What
> it did need is the other half of item 3's exclusion list — a pool register has
> no address, so `LiveAnalysis` now records every local whose address is taken
> and `allocate_registers` leaves it in a frame slot, the same treatment
> `f32`/`f64` and the aggregate kinds already had. The store is the genuinely
> new primitive, and it is the small kind of new: `*p = v` for an integer
> pointee (a `datac` passdown already had the wide form), plus an `f64` pointee,
> which needed `int_regs_for`/`value_kind` to accept a reference to a float so
> that a `mut f64` parameter is one pointer like any other, and
> `ExprKind::Deref`/`Assign` to know `movsd` where the integer path writes
> `mov`. The read `*p` for a scalar pointee already existed for the passdown
> slice.
>
> **The three parsers are `externs.rs` transliterated.** `strtoll`, `strtoull`
> and `strtod` through the PLT behind the same empty-and-whitespace scan, the
> same whole-string check and the same single store through the caller's out
> pointer, with `parse_u64_raw`'s leading-`-` rejection and the `errno` pair
> (`__errno_location`, zeroed before the conversion and read after it) that
> makes an out-of-range number a failure instead of a saturated success —
> without it `parse_i64("9223372036854775808")` would have returned
> `Ok(INT64_MAX)` where the C backend returns `Err`. `alloc_raw` and
> `from_f64_raw` were **not** needed: no fixture in this slice's acceptance set
> reaches either, and neither was added.
>
> **Measured.** The C suite is 280; the asm suite is **223 of 280** (was 219 of
> 278), with no test that passed at 219 failing now, and the two fixtures this
> slice wrote are the only reason the denominator moved. The fixture ratchet is
> **129 built of 160, 31 refused** (was 125/158/33), every one of the 129
> compared against the C backend's own binary byte for byte, and
> `cargo test --test memory_gate -- --ignored` is valgrind-clean over all 289
> binaries — 160 C and 129 asm — with zero findings. Two credited fixtures
> moved: `stdlib_build` and `borrow_provenance_and_arms`, both of which refuse
> first on `std.str.parse_i64_raw` and were reachable only because the enum
> slice had already landed behind it. Two are new: `scalar_borrow` (an integer
> out-parameter, a `&mut i64` binding held across a call, and two live locals
> around the store — the live-range probe, since a missed touch there hands the
> pointer's register to the next local and the store writes through a wild
> pointer) and `scalar_borrow_f64` (the same borrow with a float pointee, in the
> differential gate because a wrong register file prints a plausible wrong
> number rather than failing to build). The ranking moved `heap 16, enum 4,
> datac 2, closure 10, gated 1` to `heap 13, enum 5, datac 2, closure 10, gated
> 1`: the heap row lost the two this slice lowered and the `enum` row gained
> `stdlib_parse`.
>
> **The measurement disagrees with the plan, again in the same direction, and
> this one is the most instructive of the five.** `stdlib_parse` is the third
> fixture §0 credited this slice with, and it is **refused**. With the parsers
> lowerable its first refusal is the return type `std.result.Result<f64,
> std.str.ParseErr>` of `std.str.parse_f64` — an `enum` whose `Ok` payload is an
> `f64`, which is the float-aggregate row (§0's ranking 2), not this one. So of
> the three, **two build and one is blocked by a float payload**: the borrow was
> the last thing in any of their way, and it was not the only thing in the way
> of all of them. That is exactly the error §9's first lesson names, and it is
> recorded in §0's correction list rather than quietly fixed.
>
> **Deliberately still refused, and each by name:** a float payload in an
> aggregate, which is what `stdlib_parse`, `enum_basic`, `enum_container`,
> `if_let_basic`, `if_let_chain_same_scrutinee`, `receiver_methods` and
> `templates` are all waiting on — the same row seen from a tagged union or from
> a `datac` field; `alloc_raw` and the `fs`/`io`/`net`/`path`/`time` extern
> bodies; `List.sort`/`contains`/`slice`; a capability-gated call; and closures.
> `std.time.now_mono_ns_raw` is deliberately *not* added either: it is one of
> `bench/speed`'s three prerequisites, but the gate also needs the capability
> check that gates the same module, so the extern alone would only move the
> refusal from the declaration to the gate.
>
> Two bugs the differential suite found, kept as evidence the gate works:
> comparisons took their operand width from the `bool` result type (so every
> `i32` comparison compared only its low bytes), and a load used `mov rax, DWORD
> PTR [...]`, which is not an instruction. One bug the design caused and the
> suite caught as a segfault: the call-site stack save used `r11`, which is
> caller-saved, so `printf` clobbered it — invisible in a function that printed
> once, because the epilogue reloads `rsp` from `rbp`.
>
> **The enum slice's third defect — a wrong-width aggregate load — is fixed, and
> it is a defect fix *in* that slice rather than a new one (2026-09-19).**
> `field_read`'s `AggClass::One` arm — the one the slice added so an `enum` at
> that class could be read out of a `datac` field by value — loaded a whole
> eightbyte (`mov rax, QWORD PTR [base+offset]`) whatever the field's own width
> was. The trigger is a `One`-class aggregate **field** whose width is not 8,
> read out of a base that may be a heap block sized to the enclosing value:
> `Box.of(Outer(...))` allocates `sizeof(Outer)` bytes, `Box#get` returns a
> borrow, and `b.get().field` reaches `field_read` — so two bytes of `Inner`
> behind a three-byte `Outer` read eight bytes starting one byte into a
> three-byte block.
>
> **The differential sweep could not see it; the valgrind gate now can.** Both
> backends printed the same number, because the load's extra bytes lie past the
> field and no consumer reads past the field's own width — a wrong result needs
> a wrong byte *inside* the value, which this defect never produces, so a
> C-vs-asm stdout comparison is blind to it by construction. `valgrind` on the
> asm binary reports `Invalid read of size 8 … 1 bytes inside a block of
> size 3`, and `kelpc/tests/memory_gate.rs` sweeps both backends, so this defect
> is a failing test rather than a hand-read log. The fix is `load_agg_bytes` in
> `codegen_asm.rs`: exactly the field's own width, one `movzx`/`mov` for 1/2/4/8
> and a byte-wise assembly for 3/5/6/7 — the widths no single instruction moves,
> on a path nothing hot reaches, because correctness beats speed here. Every
> consumer treats a `One`-class aggregate as an opaque zero-extended eightbyte,
> so zeroing the high bytes reproduces what the padded slot supplied for free,
> and no construction or transport code changed.
>
> **The same load in three sibling sites, and one that was already right.**
> `emit_elem_return`'s `One` arm — what `List#pop`/`List#remove` hand a by-value
> element — had the identical eight-byte load, and its `Two` arm loaded a full
> second eightbyte out of a nine-to-fifteen-byte element; four pushes to fill a
> list's initial capacity put both pops at the buffer's end, the sharpest shape
> (the probe measured invalid reads for element sizes 1, 2, 4, 5 and 6 — 7 and 8
> round up and are clean — and for a 12-byte `Two`-class element).
> `payload_out`'s `FieldEntry::Aggregate` arm refused *every* aggregate payload
> of a `?` with no destination and called a three-byte `datac` "over eight
> bytes"; the class is the question, so a `One`-class payload now loads into
> `rax` at its own width and only an actually-wider one keeps the refusal, whose
> message now says "wider than eight bytes". Audited and **cleared**:
> `eval_struct_into`'s `FieldAccess` arm and `emit_copy` itself copy an exact
> byte count taken from the layout, so a three-byte field reads three bytes;
> `marshal_elem`'s deliberately wide stores land in a frame buffer `size + 16`
> bytes wide, and `BoxFn::Take` reloads from the same buffer; `emit_map_value_out`'s
> `One` arm already staged the value in a frame slot for exactly this reason,
> and its `Two` arm (and `MapFn::Remove`'s) reads its second eightbyte out of a
> `malloc(sizeof(V))` block, whose usable size is at least sixteen for every
> nine-to-sixteen-byte value — valgrind-clean and left unchanged;
> `List#insert`/`remove` shift through `emit_copy`; and the `List`/`Map` buffers
> are `cap * size` and `malloc(size)`, so no other load is wider than its own
> storage.
>
> **Fixtures, three, none covered by an existing one.** `heap_narrow_fields`
> (field widths 1, 2, 3, 4, 5, 7 and 8, each read whole through `field_read` and
> byte by byte through the recursion that was always correct, plus the
> offset-zero field), `list_narrow_element_pop` (`List<A3>` and `List<S12>`
> popped and removed off the buffer's end), and `try_narrow_payload` (`?` on a
> `Result<D3, u8>` in a `let`, on both paths). All three agree with the C
> backend byte for byte, are `valgrind`-clean, and are in the AddressSanitizer
> list. The A/B: the pre-fix compiler builds all three with the same stdout, and
> only valgrind separates it — six invalid reads for `heap_narrow_fields` and
> five for `list_narrow_element_pop`, against zero each after — while
> `try_narrow_payload` was refused outright with the false message. Measured:
> the asm suite is **226 of 283** (was 223 of 280), the fixture ratchet **132
> built of 163, 31 refused** (was 129/160/31), and valgrind is clean over all 163
> C-backend and 132 asm-backend binaries — zero findings, zero leak bytes.
>
> Three smaller corrections landed with it, all review findings rather than new
> work. The register-save comment in the prologue still argued for `add`/`pop`
> after the epilogue had become `mov rsp, rbp`, and now points at the epilogue's
> own reason. `kelpc/tests/backend_coverage.rs`'s recorded counts said 153/106/47
> beside a 129-entry list; they now say what the sweep prints (163/132/31), and
> the three new fixtures are recorded in `ASM_BUILT_BASELINE`. And
> `enum_liveness` did not reach `if let` or an enum-typed local live across a
> call, though the enum slice's coverage note named both shapes; the fixture now
> holds them (`if_let`, `held`) with the expected stdout extended, so the
> liveness pass is exercised on them rather than argued about. One behaviour recorded
> while measuring, so that no fixture claims the opposite later: an owning `enum`
> is not dropped when a panic unwinds — `kelp_panic` prints and calls `exit(1)`
> in both backends, so no local drop runs and the process exit reclaims the
> memory (valgrind counts it as still-reachable glibc memory, not a finding).
>
> **The float-in-aggregate slice — the last shape item 4 names, and the real
> per-eightbyte classifier (2026-09-20).** A `datac` field or an `enum` payload
> may now be an `f32`/`f64`, and every site that moves such a value derives its
> SysV class per eightbyte rather than from the size. What that replaced is
> worth stating plainly, because the third slice's prose said it out loud and
> was right at the time: `AggClass` was `One`/`Two`/`Mem` *by size*, sound only
> because no layout with a float field existed. The size-only classifier would
> in fact have built all seven fixtures this slice had been waiting on — a
> sixteen-byte enum's union merges its payload eightbyte to `INTEGER`, so
> sizes 8/16/24 happen to map to the right transport — which is exactly the
> silent miscompile §10 warns about, and why this slice did the real
> classification rather than taking the seven as a sign that none was needed.
>
> **The classifier, as it now exists.** `EbClass { kind, size }` is one
> eightbyte: `INTEGER` or `SSE`, and how many of its bytes the value occupies
> (only read for `SSE`, to choose `movss` for a lone `f32` and `movsd`
> otherwise). `AggClass` is `One(EbClass)`, `Two(EbClass, EbClass)` or `Mem`,
> with `int_regs`/`sse_regs` counting each register file independently. The
> class is computed once at layout time from the layout's leaf *atoms* —
> `(offset, size, kind)` — and stored on `StructLayout`, because every call
> site consults it. A field's own atoms are folded in at its offset, so a
> nested `datac` that straddles the parent's eightbyte boundary splits
> correctly instead of being merged as a unit. `INTEGER` wins a tie, an
> eightbyte no atom reaches is `INTEGER`, and a value over sixteen bytes is
> `MEMORY` regardless, as `{f64, String}` (32 bytes) is. `FieldEntry::Float`
> is the one new field kind; `try_build_layout` and `try_build_enum_layout`
> place it with its natural alignment (4 for `f32`, 8 for `f64`).
>
> **The union merge is the subtlety, and it is measured.** An `enum`'s payload
> eightbytes are classified through the union, so a variant with no payload (or
> one with a non-float field) in the same eightbyte merges it back to
> `INTEGER`. `enum_atoms` supplies that missing atom: the four-byte tag, then
> every variant's payload entries, then — for each payload-less variant — the
> one `char` C writes at the union's own offset. Two sixteen-byte enums that
> differ only in the register file are the pin, and a fixture asserts both at
> the `.s` level (see below): `Pure { D(f64) }` is `INTEGER`/`SSE` (`rdi` +
> `xmm0`), `Merge { C(f64), Empty }` is `INTEGER`/`INTEGER` (`rdi` + `rsi`)
> because `Empty`'s `char` shares the payload eightbyte.
>
> **The measured table, every row reproduced from the C backend's own binary
> and cross-checked by `objdump`ing it** (the asm side was built, run and
> disassembled the same way; the shapes below are the fixture's own
> declarations):
>
> | shape | size | eightbytes | parameter | return |
> | --- | --- | --- | --- | --- |
> | `{f64}` | 8 | SSE | `xmm0` | `xmm0` |
> | `{f32}` | 4 | SSE | `xmm0` (`movss`) | `xmm0` |
> | `{f32,f32}` | 8 | SSE | `xmm0` (one eightbyte) | `xmm0` (`movsd`) |
> | `{i32,f32}` | 8 | INTEGER (mixed eightbyte) | `rdi` | `rax` |
> | `{f64,f64}` | 16 | SSE, SSE | `xmm0`,`xmm1` | `xmm0`,`xmm1` |
> | `{f32,f32,f32}` | 12 | SSE, SSE | `xmm0`,`xmm1` | `xmm0` (`movsd`), `xmm1` (`movss`) |
> | `{i64,f64}` | 16 | INTEGER, SSE | `rdi`,`xmm0` | `rax`,`xmm0` |
> | `{f64,i64}` | 16 | SSE, INTEGER | `xmm0`,`rdi` | `xmm0`,`rax` |
> | `{f64,f64,f64}` | 24 | MEMORY | outgoing area `[rbp+16]` | hidden pointer |
> | `{f64, String}` | 32 | MEMORY | area (self too) | hidden pointer (`rdi`) |
> | `enum { D(f64) }` | 16 | INTEGER, SSE | `rdi`,`xmm0` | `rax`,`xmm0` |
> | `enum { C(f64), Empty }` | 16 | INTEGER, INTEGER (union) | `rdi`,`rsi` | `rax`,`rdx` |
> | `enum Shape { Circle(f64), Rect(2×f64), Empty }` | 24 | MEMORY | area | hidden pointer |
>
> **What had to change, by mechanism.** `arg_regs_for` replaced the third
> slice's `int_regs_for`, returning both register counts, so a parameter list
> and a call site number the two files independently. `Gen::function` binds a
> by-value aggregate with `store_arg_ebs`: each eightbyte from the `ARG64` or
> `FARG` register its class names, a `movss`/`movsd` for an `SSE` one. Its
> epilogue loads the return registers from `agg_slot` the same way, and every
> `return`/tail site routes a register-class aggregate there. `Gen::call`
> classifies each argument into an `ArgSlot::Regs` schedule, pushes each
> eightbyte, and pops it into the right file; the return store writes each
> returned eightbyte into its `Dest` at its own width. `value_kind` no longer
> answers for a `One(SSE)` aggregate — it is not a plain value `rax` can hold
> — so a `let`, an assignment, a return and a call argument all stage it
> through `Dest` exactly as a `Two`/`Mem` value already was. `field_read`,
> `field_assign`, `struct_lit_at`, `enum_lit_at`, `enum_field_read`,
> `payload_out` and `move_payload` gained the `Float` arm, each a `movss`/
> `movsd` where the integer arm was a `mov`. `marshal_elem`,
> `emit_elem_return` and `emit_map_value_out` gained the same per-eightbyte
> arms, which is what keeps `Box<Vec2>`, `List<Vec2>` and a `Map<i64, Vec2>`
> correct *by construction* rather than refused: `elem_size` starts returning
> `Some` for a float `datac` the moment it has a layout, and a hand-written
> routine disagreeing with the generated caller about an `SSE` element is the
> one place a wrong split *is* visible in stdout, so the fixtures cross that
> boundary and compare with the C backend's output.
>
> **The refusal sites are gone, not moved.** `try_build_layout` and
> `try_build_enum_layout` (`Ty::F32`/`Ty::F64` no longer fall through to
> `None`), and with them the `struct_lit`/`struct_lit_at`
> "holds a field this backend cannot lower yet" arm, `enum_layout`,
> `require_enum_layout`, `enum_lit`, `enum_field_read` and `Gen::function`'s
> parameter/return checks, none of which can fire on a float field any more.
> All seven fixtures that named them build, run and print the C backend's
> bytes exactly: `enum_basic`, `enum_container`, `if_let_basic`,
> `if_let_chain_same_scrutinee`, `receiver_methods`, `templates` and
> `stdlib_parse`. A shape still out of scope keeps an honest refusal: a
> closure or function value, the `fs`/`io`/`net`/`path` extern bodies,
> `List.sort`/`contains`/`slice`, a capability-gated call, and an owned
> `String` interpolated into a template.
>
> **A self-consistent wrong ABI is invisible to the differential sweep, so the
> slice adds the test kind that pins it.** Both ends of every generated call
> are this backend, so putting a value in the wrong register *consistently*
> still prints the right number. `assemble_and_link` keeps `<out>.s`, and
> `the_classifier_puts_each_aggregate_eightbyte_in_its_own_register_file`
> reads it: one representative of each row above, asserting the exact
> parameter store and the exact return load, including the union-merge pair.
> It is asm-only and reports itself so under the C backend, the way the
> AddressSanitizer gate reports itself as C-only. A scratch build with
> `classify` forced to `INTEGER` makes it fail (`mov QWORD PTR …, rdi` where
> the assertion wants `movsd …, xmm0`), which is how the test proves it can
> fail rather than merely passing.
>
> **Fixtures, five, and the valgrind they are in.** `float_datac_boundary`
> (each row of the table, passed and returned by value, every value printed
> after crossing a call), `float_datac_memory` (a `{f64, String}` passed and
> returned, valgrind-clean), `float_enum_register_file` (the union-merge
> pair), `float_datac_containers` (`Box#of`/`set`/`take`, `List#push`/`get`/
> `pop`, `Map#insert` over an `SSE` element), and `float_field_write` (a
> `mut`-method write and a plain `c.r = v` on a mixed `INTEGER`/`SSE` value).
> Measured: the asm suite is **240 of 289** (was 226 of 283; the failing set
> is unchanged, every name still a closure or a capability row), the fixture
> ratchet is **144 built of 168, 24 refused** (was 132/163/31), and valgrind is
> clean over all 168 C-backend and 144 asm-backend binaries — zero findings.
> The refusal ranking is now **heap 13, `enum` 0, a `datac` holding a float or
> heap field 0, closure 10, a gated extern 1**: the two rows this slice owned
> are empty, and the float-aggregate gap item 4 named is closed.
>
> **One pre-existing bug this slice's probes found, recorded rather than
> fixed.** A *field read of an owned temporary* — `print(mkName().r)` where
> `mkName() -> Named` owns a `String` — leaks the temporary in this backend.
> It is not float-specific: the same program with an `i64` field leaks
> identically, and the pinned pre-slice compiler (`md5
> db2fbf48de9505bd35b606db01845b09`) reproduces it, so it predates this
> slice. The C backend frees the temporary. The new fixtures do not read a
> field of an owned temporary for this reason; the bug is a separate slice
> (the temporary's drop is not keyed at a field-access statement), not a
> half-working shape this one introduced.
>
> **That slice has since landed and the fix is recorded as `e748b06`**, which
> is where it belongs because that commit did not touch this file: a call
> temporary read through a field access is now staged and the *temporary* is
> dropped, and `heap_field_of_temporary`/`heap_field_of_owned_temporary` are
> the regression test — the second is the one fixture in the corpus only the
> asm backend accepts, so `memory_gate.rs` now builds and valgrinds those too
> (`c_rejected`). Measured: valgrind-clean on the merged compiler, and 30
> bytes in 10 blocks definitely lost on the compiler before the fix. It had no
> test under either backend until `01ec7aa`, which is how a non-terminating
> loop got committed with it; that loop is bounded now and the fixture's whole
> stdout is asserted. The ratchet `e748b06` re-records is 170
> C-built / 146 asm-built / 24 refused.
>
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
4. **Aggregates by value. First nine slices done** (2026-09-19): a `datac` whose fields are all scalar and whose
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
   above). The fifth slice extends the *field* rule: a `datac` may now hold a `String` or `Bytes` field (or a
   nested `datac` of any size), which is an `INTEGER`-class aggregate like any other — see the heap-field
   blockquote above, whose whole point is that no ABI code changed. The sixth adds `Box<T>` itself: one pointer,
   one layout for every element type, seven generated routines and a drop — see the box blockquote above, which
   also records the pre-existing assignment-evaluation-order bug the slice's own fixture found. The seventh adds
   `List<T>`: the same `{data, len, cap}` header `Bytes`/`String` are, one `$List` layout key for every element
   type, fifteen routines per instantiation plus a generated drop, and one growth helper that is deliberately
   *not* monomorphized — see the list blockquote above, which also records the register allocator's exclusion
   list that had to learn the new type and the three plumbing bugs the slice's own probes found. The tenth is
   not an aggregate at all but the exclusion list this item's own prose kept naming: a borrow of a named
   *scalar* local, which is a `lea` of that local's own frame slot and needs `LiveAnalysis` to keep every
   addressed local out of the register pool — see the scalar-borrow blockquote below, which also carries the
   `str` parsers' out-parameter, the shape that borrow exists for. **The item is now closed** by the
   float-in-aggregate slice: a `datac` field or `enum` payload may be an `f32`/`f64`, and `AggClass` is the
   real per-eightbyte classifier (`INTEGER`/`SSE`, `MEMORY` over sixteen bytes, an `enum`'s payload through its
   union) rather than the third slice's size-only split — `Map<K, V>` landed as the eighth slice of this item
   and `enum` as the ninth, the float field as the tenth (`Bytes`, `String`, `View`, `Box`, `List`, `Map` and
   every aggregate shape an `enum` or a float field adds are done; see the heap
   blockquotes above and the map, enum and float-in-aggregate blockquotes below, and §12 for the reading of the
   C they mirror —
   layouts, invariants, runtime functions and drop orders). Layout must
   match `codegen_c.rs`'s `typedef`s exactly or the two backends will not agree on a `List`'s field offsets —
   which is why an `enum`'s union offset is `align_up(4, union_align)` and not the four bytes a reader would
   guess.
5. **Drop and ownership. Sixth slice done** (2026-09-19), for `Bytes`, `String`, every `datac` that holds
   one, `Box<T>`, every `List<T>`, every `Map<K, V>` and every `enum`: `borrowck.rs` already hands over a
   `DropPlan` keyed by span, so the backend does not re-derive where drops go, only what a drop *is* in machine
   terms — `kelp_bytes_drop`/`kelp_string_drop` for the two builtins, which free the buffer and clear the
   pointer and length so a second drop frees nothing, and a generated `kelp_drop_<c_ident>` for a `datac`,
   which drops each owning field in declaration order (see the heap-field blockquote above), a generated
   `kelp_box_drop__<mangle>` per instantiation, which frees the element (if it owns) and the box, and a
   generated `kelp_list_drop__<mangle>` per instantiation, which clears the elements and frees the buffer, and a
   generated `kelp_map_drop__<mangle>` per instantiation, which drops each live value, then the key when the key
   type is `String`, then both arrays — and whose element and value drops are whatever `drop_site_for` answers,
   so a `List<String>`, a `List<datac>`, a `List<Box<T>>`, a `List<List<T>>` and a `Map<String, String>` need no
   new mechanism. The points are
   `codegen_c.rs`'s four (block end, function body end, a `return`'s span, the old value
   of an assignment), plus a fifth this backend's own `?` needs (`DropPlan::at_try`, whose early return is a
   `return`), a value produced before a drop is parked while the drops run, and a borrowed temporary is
   freed after the call that borrowed it — the last of which now covers a borrowed *`datac`*, *`Box`* or
   *`List`* or *`Map`* temporary as well, since any of them can own what it holds. The owning `enum` — a tagged
   union that holds one, and the generated drop that switches on the runtime tag and frees only the live
   variant's payload — landed with the enum slice (see that blockquote below), so what is left of this item is
   nothing: the process-exit handler an owning `pd` needs is the passdown-drop slice's, below.
6. **Closures and indirect calls.** `FnRef`, `ClosureLit`, `Capture`, `FnToClosure`, `IndirectCall`. The
   environment pointer is a hidden parameter, so the SysV classification applies to it too, and the generated
   thunk has to be emitted per signature exactly as `codegen_c.rs::write_closure` does. **Done (2026-09-20,
   `eab7916`):** the whole row of ten fixtures built and printed the C backend's bytes, and the closure row
   of the refusal ranking is empty; see the closure slice's blockquote in this section.
7. **Capabilities.** `resolve.rs` compiles each grant to constant data and `codegen_c.rs` emits
   `kelp_cap_check_*` at each gated call site. The matcher and path normalizer must be **byte-exact**
   equivalents or every capability fixture changes meaning, and getting this subtly wrong is a security
   regression, not a test failure. Port them as assembly-called libc-backed routines at this stage (they are
   part of the 813 runtime lines), and keep the C versions as the reference for a differential test of the
   matcher itself over a generated pattern corpus. **Step 1 done (2026-09-20, `e65ce61`/`9c89b70`):** the
   path and name checks, the deny path and the compiled-in allowlists are emitted, and the gated-extern row
   of the refusal ranking is empty. **Step 2 (the `net` host/port kinds) is not done:** `cap_decision`
   refuses them, which is the second prerequisite of the four `net` fixtures; see
   `plan/mk7-next-slice.md` §D.
8. **`print`/`print!`. Second half started** (2026-09-19): `str`/`&str` and `f32`/`f64` are printed, including the
   x86-64 details — a float argument goes in the `xmm` file rather than an integer register, `f32` is promoted to
   `double`, and a variadic call sets `AL` to the number of vector registers used, so a template interpolating
   one of each numbers the two files independently (see the fifth slice's blockquote). An owned `Bytes` is
   printed too, as one `printf("%.*s", (int)len, data)` with the temporary freed after it — the shape
   `codegen_c.rs` uses — and every other owning type follows the same `%.*s` shape once its layout exists. What
   is left of this item is an owned `String`/`View` printed *as a value* (`print(s)`, refused by name — the
   fixture `heap_temporary_print` is exactly this), **`Bytes` interpolated
   into a `print` template** (`print("x=${b}")`, refused by name: only the single-argument call forms are
   implemented), and lifting the five-interpolation limit on a template, which no fixture reaches.
9. **Passdowns.** Done (2026-09-19), and an item this list never had: `pd` is M5.2's language feature, and the
   asm backend refused any program containing one at the top of `generate` — before codegen, the way it refused
   `extern fun` — so it never appeared in the build order §5 lays out. The C backend's shape is one file-scope
   global per passdown plus a hidden pointer parameter on every function that references it, and the asm
   backend's is the same object assigned from `main` with the parameter kept (it costs nothing: an `&T`-typed
   local is a local). See the passdown blockquote above for the three things that had to exist, and for the two
   narrowings it leaves; the owned case is item 11 below.
10. **`extern fun`, and the dead code around it. First slice done** (2026-09-19), and the second item this list
    never had — for the same reason as the ninth: the blanket refusal at the top of `generate` meant no extern
    ever reached codegen, so nothing here was ever *built*. Calls to the externs this backend can lower now
    resolve to `libc@PLT` or to a routine it owns (`strlen`; the fourteen libm transcendentals; `str.byte` and
    `str.substring`, whose C bodies add a bounds check and a panic; `isnan`/`isfinite`, which are macros with
    no symbol to call), and the whole of an `extern fun`'s *declaration* is now irrelevant to whether a program
    builds. The second half is `live_functions`: only functions reachable from `main` (or a passdown
    initializer) are compiled at all, so a module's heap-typed functions stop refusing the programs that only
    import it. See the extern blockquote above. What is left of this item is the externs whose bodies are the
    heap and the syscall layer: `alloc_raw`, the `fs`/`io`/`net`/`path`/`time` families above all, which is why
    the item is one slice of a row rather than a finished one. The `str` parsers joined the list in the
    scalar-borrow slice below — they were the one group whose declared signature, a `mut` scalar out-parameter,
    had no lowering at all.

11. **An owned passdown, and the drop handler. Done** (2026-09-19), and it is the last thing the passdown row
    was carrying: `pd_owned`, `list_field` and `pd_argument_forwarding` needed exactly what the C backend gets
    from its C runtime, a drop point at process exit. `generate` now resolves a **drop site** for every passdown
    whose type owns memory, in a pass that still runs before the routine set is snapshotted — `drop_site_for` is
    what registers the `Bytes`/`String`/`datac`/`Box`/`List`/`Map` routines the drop body calls, and for the two
    fixed routines it only *names* the site, so `require_routine` is called there as well. The body is
    `kelp_drop_pds`: one `lea`/`call` per owning passdown, in declaration order, under the same symbol the C
    backend's `kelp_init_pds` generates, and emitted after every routine it can call. `main` registers it with
    `atexit` right after the passdown initializers, so a `return` from `main`, `std/process.exit` and
    `kelp_panic`'s own `exit(1)` all free the passdowns, which is the C backend's behavior exactly. One detail
    is glibc's rather than ours, and it is the whole of what this took beyond the body: `atexit` lives in
    `libc_nonshared.a` and calls `__cxa_atexit(fn, NULL, __dso_handle)`, and `__dso_handle` is normally defined
    by `crtbegin.o` — which this backend's `ld` line deliberately does not carry (crt1/crti/crtn and `-lc` only).
    The generated object defines it itself (hidden, zero), which is what `crtbegin.o` would have supplied, and
    `exit` runs every registered handler regardless of the value. The three fixtures this moved, plus the two the
    borrow-mode slice moved, `block_comments`, and the fixture this slice wrote for the aggregate store itself
    (`pd_var_datac_reassign.kp`) put the baseline at 106 built of 153. The refusal a program met before the store
    existed was honest; what it took to make the store *correct* was not one thing but two, and only one of them
    announced itself. The codegen half is a staged copy through the pointer, as `Stmt::Let` does for an
    aggregate — a refusal until now, so the silence was not there. The other half is that the
    operand of the `*` is a **use**: `LiveAnalysis`'s `Assign` arm touched an `Ident` target and a
    `FieldAccess` target's base, and for a `Deref` target it only ticked a position, so the operand's live range
    was its declaration and nothing else. The allocator was then free to hand that register to another local —
    in `pd_var_datac_reassign.kp`, to `a` or `b` — so `pair = Pair(...)` wrote through the wrong pointer and the
    program printed `1,2` where the C backend printed `3,6`. That is the failure mode the architecture's risk
    register is about, caught by an A/B build (fix reverted: `3,6` becomes `1,2`) rather than by a refusal, and
    the fixture is the regression test. The valgrind gate covers both: 259 runs, zero findings, and the owned
    passdowns free clean.

**The float codegen slice — `f32`/`f64` locals in the `xmm` pool, and float
operands named instead of loaded — is done (2026-09-21).** It is the first
change on this branch that moves a `bench/speed` kernel's number rather than a
fixture count, and the number it moved was 4.14x. Item 3's blockquote above
named the exclusion — "`f32`/`f64` locals are excluded entirely" — and that
exclusion turned out to be the *smaller* half of the gap. Measured on the four
kernels (`bench/speed/speed.kp`'s bodies and constants verbatim, the clock calls
removed, timed externally), interleaved round-robin so machine load hits every
column equally, 9 runs each, medians in ms:

| workload | `-O0` | `-O2` | asm before | asm after | after/`-O0` |
| --- | --- | --- | --- | --- | --- |
| `collatz` | 452.1 | 85.7 | 438.8 | 434.1 | 0.96 |
| `float` | 260.0 | 114.9 | 1077.2 | 185.4 | **0.71** |
| `mix` | 205.2 | 168.2 | 145.9 | 144.4 | 0.70 |
| `fib` | 544.4 / 20 | 465.9 / 20 | 924.1 / 20 | 923.7 / 20 | 1.70 |

What each half measured: giving every `f32`/`f64` local a `FLOAT_POOL` register
(`xmm8`-`xmm15`) instead of a frame slot is worth 1077.2 → 822.0 — a quarter of
the loop, nowhere near the 4.14x. The other three quarters are
`Gen::binary_float`, which loaded *every* operand into `xmm0`, spilled the left
one to `[rsp]` across the right one's evaluation and reloaded it: ~83
instructions per iteration where clang `-O0` needs 25, which is worse code
generation than `-O0` rather than merely unoptimized code. Two candidates the
diagnosis ruled out by measurement instead of by reading: the left-operand spill
is *not* the cost (hand-replacing all five `[rsp]` spill pairs with
`xmm2`-`xmm7` holds, checksum unchanged, ran 18% **slower**), and the float
constants were never `.rodata` loads (they were `mov rax, imm64; movq xmm0, rax`,
two instructions each).

What replaced them, all of it additive and all of it falling back to the path
above when its guard does not hold: an operand that is already a place or a
constant is named as the instruction's second operand (`mulsd xmm0, [rbp-16]`,
`addsd xmm15, [rip+.LC0]`); a commutative operator whose *left* operand is
addressable evaluates the right one first and uses the left as that operand
(`1.0 + x * x`); a constant that has to be the left operand of a
non-commutative one is held in `xmm2`-`xmm7` across the right side's evaluation
(`1.0 / t`), refused when that side contains a call because every `xmm` register
is caller-saved; and `x = x <op> v` updates the destination in place — one
instruction when the local is in the pool, three when it is not. Float
constants are interned in `.rodata` now, since an `addsd` can take one as an
operand and there is no `mov xmm, imm`.

The allocator's discipline is the part that keeps this correct, and it is
narrower than the integer pool's on purpose: the five integer registers are
callee-saved, so a local that spans a call still holds its value when the call
returns, while all sixteen `xmm` registers are caller-saved. A float local is a
`FLOAT_POOL` candidate only when no *clobber span* overlaps its live range —
`LiveAnalysis::clobbers`, which is every call expression's whole argument list
(an inner call in an earlier argument clobbers the registers before a later
argument reads them), every `print`'s piece list, and every drop site, all of
which the emitter can turn into a `call`. A float local that cannot be allocated
keeps its frame slot, which is what every float local had before this slice. All
226 integration tests the asm backend passed before it still pass, with the same
names, and every fixture either backend builds still prints byte-identical
stdout to the C backend.

**Still excluded, and why the gate is not met everywhere.** The integer path is
untouched, and for `float` it is now the *residual*: of the ~32 instructions the
new loop still spends per iteration, 19 are integer loop control — the
`i < iters` comparison and `i = i + 1` each round-trip their operands through
the stack, and `*above = *above + 1` is twelve instructions taken 57.7M times.
A hand-written version of the same kernel that fixes only that control runs
109 ms, faster than `-O2`, so what is left is a bounded piece of work rather
than a property of the architecture. `fib` is unchanged at 1.70x for the same
reason from the other end: it has no float local at all, and its cost is 69
instructions per call against clang `-O0`'s 34 — the prologue/epilogue around
two pushed registers, the call-site `rsp` save at four instructions per call
site, and the `mut i64` counter's twelve-instruction update. Hand-writing those
three alone gets 30.9 ms against `-O0`'s 28.1, i.e. 1.10x. §3's claim that a
competent allocator with no optimizer lands between the two columns survives
this measurement for `collatz` (0.96), `mix` (0.70) and `float` (0.71), but not
for `fib`, whose gap is call-sequence overhead rather than arithmetic.

One measurement condition decides how a re-run should be read, and §13's annex
already asks for it: `float`'s new loop is throughput-bound, so it is far more
sensitive to co-running work than clang's `-O0` code is. On this machine its
9-run median is 185 ms and its minimum 140 ms, but runs that overlapped another
lane's compilation landed at 459-496 ms while the `-O0` and `-O2` columns stayed
inside 3% of their own medians through the same window. The gate number is the
uncontended one; a run on a loaded machine is not comparable, and the same
caution applies to every slice that makes a kernel *faster*.

**Re-derived on the merge result (`7594b25`, this slice merged into the branch
that already had the per-eightbyte classifier), because the table above was
measured before the classifier landed in the same file (2026-09-21).** Same
method and the same four kernels: `bench/speed/speed.kp`'s bodies and
constants verbatim, the clock calls removed, the C columns from this
compiler's own generated C with `cc -O0 -fwrapv -lm` and `cc -O2 -fwrapv -lm`
(clang 21.1.8), the asm columns built `--backend=asm`, external wall clock
around the whole process so no column times itself, all four columns
interleaved round-robin so machine load hits every column in the same round, 9
runs each, medians in ms. Machine `AMD Ryzen 5 7600` (6C/12T); load average
2.4-2.8 from other lanes' work throughout the sweep (2.13 on the 15-run
re-check below).

| workload | `-O0` | `-O2` | asm at the tip (`e748b06`) | asm on the merge | merge/`-O0` |
| --- | --- | --- | --- | --- | --- |
| `collatz` | 443.5 | 82.4 | 429.5 | 431.4 | 0.97 |
| `float` | 258.6 | 105.9 | 1075.2 | **139.0** | **0.54** |
| `mix` | 201.2 | 164.4 | 141.9 | 140.7 | 0.70 |
| `fib` | 28.5 | 24.1 | 46.9 | 47.2 | 1.66 |

All sixteen columns printed the same checksum as `bench/speed/results.txt` and
as each other — `collatz 131434424`, `float` 57735026 with `VALUE
78539816.062926`, `mix 542344`, `fib` 18454929 calls and 5702887 — so no
column's time is a column that did less. The `collatz`, `mix` and `fib`
binaries the merge builds are **byte-identical** to the ones the tip builds
(`md5sum` equal); only `float` differs, which is the whole of this slice's
effect and is why the other three ratios move only by machine state — this
sweep's `float` "before" column reads 1075.2 against the 1077.2 in the table
above, i.e. the same measurement twice.

**What is still slow, re-derived.** `float`'s residual is the integer loop
control the fast paths do not reach, and `fib` is the same statement from the
other end: it has no float local at all, its binary is byte-identical before
and after this slice, and its 1.66x is therefore entirely the integer path's
call sequence (69 instructions per call against clang `-O0`'s 34, from the
diagnosis above) — a different slice's number, which this one neither causes
nor could move. §3's claim that a competent allocator with no optimizer lands
between the two columns survives this measurement for `collatz` (0.97), `mix`
(0.70) and `float` (0.54), but not for `fib`.

**The bimodality caveat, measured twice and still the condition on every
number above.** `float`'s new loop is throughput-bound, so it is far more
sensitive to co-running work than clang's `-O0` code is, and a contended
median lands a factor of two off the uncontended one. In the sweep above its
nine runs were 133.6-140.4 ms five times and 372.8-493.1 ms four times (median
139.0, min 134.9, max 405.9) while the `-O0` and `-O2` columns stayed inside
1.8% and 10.9% of their own medians through the same window. A second, 15-run
sweep of the same four columns at load 2.13 put 14 of its 15 merge runs between
134.3 and 216.0 ms (median 140.3) with one at 290.9, over a `-O0` median of
258.3 ms — 0.55x. The gate number is the uncontended one, **0.54x to 0.55x**
here; the 185 ms this slice reported at its own base is the same loop with its
median landing in the contended mode (that run's minimum was 140 ms), not a
slower compiler. A third 9-run sweep of the same four columns, on the final
fixture-fix commit (`58ec97f`), with every kernel binary `md5sum`-identical to
the ones above, gave `collatz` 0.96, `float` **0.57**, `mix` 0.70 and `fib`
1.68 with the same checksums — the same binaries and the same fast mode
(134-146 ms), differing only in how many contended runs the median had to
include, which is the spread this paragraph is about.

**What the merge had to leave untouched, re-checked on `7594b25`.**
`KELPC_BACKEND=asm cargo test --release --test compile_and_run` is **241 passed
/ 49 failed of 290**, and the 49 failing *names* are identical to the pre-merge
tip's, so the slice moved no test in either direction. `backend_coverage --
--ignored --nocapture` is **24 fixtures refused of 170 the C backend builds,
146 built**, rows **heap 13, `enum` 0, a `datac` holding a float or heap field
0, a closure or function value 10, a gated extern 1**. `cargo test --release`
is green (C backend, 290 of 290, and the unit tests). The ratchet's own numbers
are therefore unchanged by the merge.

**The one invariant this merge does *not* report green, and the one defect it
leaves behind, recorded because the row above asserts a green gate.**
`cargo test --release --test memory_gate -- --ignored` is **red on both sides
of the merge**, and the merge changes nothing about it:

- At `e748b06` it reports **three findings over 317 runs** (`c: 170 binaries
  checked, 1 finding`; `asm: 147 binaries checked, 24 refused, 1 of the checked
  ones from a fixture the C backend itself rejects, 2 findings`). Two are
  `std_str_order` (below); the third was `heap_field_of_owned_temporary`, whose
  loop never terminated, so valgrind was killed at the gate's 60 s limit and
  that counted as a finding. **That hang is fixed in `01ec7aa`** — the fixture
  now bounds its own loop and still evaluates a temporary inside the condition,
  and `compile_and_run.rs` holds its stdout — and the sweep then reports **two
  findings**: `c: 170 checked, 1 finding`; `asm: 147 checked, 24 refused, 1 asm
  only, 1 finding`; 317 runs.
- Both remaining findings are `std_str_order`, 10 bytes in 2 blocks in each
  backend, and they are **one defect, not two**: an owned local bound inside a
  loop body is **not dropped** on a path through that body which does not move
  it, when another path through the same body does. The drops are simply
  absent from the emitted code — the generated C for the fixture's insertion
  sort has no `kelp_String__drop` for `left_text`/`right_text` anywhere in the
  body — so the two 10-byte losses are the pair the last inner pass abandoned
  (valgrind: `10 bytes in 2 blocks are definitely lost in loss record 1 of 1`,
  both `at … malloc` / `by … kelp_rt$string_from_str`; the C backend's build
  splits them into 4 and 6 bytes, which clang `-O2`'s dead-store elimination
  explains — it can delete an allocation nothing reads, which is why the C
  backend's binary sometimes reads clean for the same program).
- **Minimal repro, written from scratch under `/tmp`** (`v15`), 16 lines:

  ```
  impl "std/str"
  fun main() {
      var words: List<String> = List.empty()
      words.push("a")
      var b: usize = 2
      var moved = true
      while b > 0 && moved {
          val s = String.from_str("hi")
          if b == 2 {
              words.set(0, s)
              b = 1
          } else {
              moved = false
          }
      }
      print(words.get(0))
  }
  ```

  The asm backend leaks `3 bytes in 1 blocks` (the pass that sets
  `moved = false` abandons `s`); the C backend's binary is clean at `-O2`
  because clang deletes the unobservable allocation. Removing the `if`/`else`
  (an abandoned local in the same body) frees it correctly, and so does a body
  with no move at all — the defect needs both paths.
- **It is a compiler defect and it is not this merge's, not this branch's, and
  not the fixture's.** The same 16-line program's *generated C from `main`
  (`061883b`) at `-O0`* — with no branch work in it at all — leaks the same
  `3 bytes in 1 blocks`, so it is in the shared drop plan and predates the
  branch. It reproduces identically at `e748b06` and on the merge, and the
  fixture only makes it visible because a `String` allocated inside the loop
  body has a path that abandons it. It is **not** papered over here and not
  fixed in this lane: `std_str_order.kp` is unchanged, and this is its own
  slice after the release.

> **That slice is done, and the memory gate is green again.** The
> classification above was right and the diagnosis takes one correction: the
> block-end entry is not *missing* — there is one entry per block, and on this
> program it is present and **empty**, which is the same thing for the path that
> needs it. `Scope::merge` propagated a move from *either* path
> (`entry.moved = entry.moved || other.moved`), and `live_drops` then filtered
> the binding out of the only drop set a block has. So the fact being recorded
> was a property of a *path*, and the plan has one entry per *block*: nothing
> downstream could recover it, and both backends consulted exactly the right key
> and found `[]`.
>
> **Evidence, at the broken compiler (`f9fe6a4`).** The 16-line repro above,
> built both ways: `cargo build --release` then
> `kelpc build --backend=c|c repro.kp`. The C build's `main` has **no
> `kelp_string_drop(&kelp_s)` anywhere inside the loop body** — the last line of
> the body is `(kelp_moved = false);` / the `List#set` call, and `kelp_s` is
> never named again — and the asm build's `.s` has no `lea rdi, [rbp-168]` /
> `call kelp_rt$string_drop` in the body either. In the plan's own terms:
> `FnDrops::at_block_end` has an entry for the body block (the block declares
> an owned local), and `DropPlan::at_block_end(function, body_block)` answers
> `&[]`. The value bound by the `tir::Stmt::Let` for `s` is at the block end
> neither dropped nor moved, which is the one state the plan could not spell.
> Valgrind: the asm binary `3 bytes in 1 blocks are definitely lost`, the C
> binary `cc -O0`-built `3 bytes in 1 blocks are definitely lost`, and the `cc`
> default `-O2` build **clean** because clang deletes the allocation nothing
> reads — which is why this needed valgrind over the asm binary, and why the
> C backend's `std_str_order` build splits its 10 bytes into 4 and 6.
>
> **The fix: a move clears its source, so a *conditional* move can still be
> dropped.** `borrowck.rs` now carries three states per owned local
> (`MoveState::Live`/`Moved`/`MaybeMoved`) instead of a bool, and
> `MoveState::merge` answers `MaybeMoved` for any two paths that disagree. A
> `MaybeMoved` binding stays in every drop set it reaches (`live_drops` filters
> `Moved` only, and `at_return`/`at_try`/`at_assign` all read the same set), and
> the plan gained one new query: `FnDrops::at_move`, the *move sites* — the
> `tir::Expr` nodes that move a value out — keyed by identity like
> `at_block_end`. `DropPlan::moves_at(function, expr)` is what codegen asks, and
> both backends answer it the same way: copy the value out, **zero the
> binding**. The drop that runs is then a free of nothing on the path that
> moved and a real free on the path that did not, which is the property the
> one-entry-per-block plan could not state.
>
> Only sites for bindings some merge left `MaybeMoved` are recorded: a binding
> moved on *every* path is dropped nowhere, so clearing its slot would be dead
> code. `at_assign` changed with the same reasoning — the old value of an owned
> binding is freed unless the binding is *definitely* moved
> (`owned_definitely_moved`), which is what `x = f(x)` needs and what a
> conditional move must not inherit.
>
> `codegen_c.rs` renders a move site as one statement expression
> (`({ kelp_String _xtmp = kelp_s; kelp_s = (kelp_String){0}; _xtmp; })`), so a
> move site inside a call argument, a `let`, a `return` or a closure capture is
> cleared wherever it is rendered. `codegen_asm.rs` clears the local's frame
> slot in both `Ident` value paths — `Gen::expr` for a `Width::W8` value and
> `eval_struct_into` for the `Two`/`Mem`-class ones — through a new
> `emit_zero`, and `LiveAnalysis` marks such a local `addr_taken` so the frame
> slot the clear needs is never a pool register the allocator handed out. The
> generated `.s` for the repro shows both halves in the loop body:
> `mov QWORD PTR [rbp-168], 0` ×3 after the value is copied out of `s`, and
> `lea rdi, [rbp-168]` + `call kelp_rt$string_drop` at the body's end.
>
> **Measured before and after** (`cargo test --release --test memory_gate --
> --ignored --nocapture`, same machine, valgrind 3.26.0):
>
> | | before | after |
> | --- | --- | --- |
> | sweep | 317 runs / 305 fixtures | 321 runs / 307 fixtures |
> | `c` | 170 checked, **1 finding**, 10 definitely lost | 172 checked, **0 findings**, 0 |
> | `asm` | 147 checked, 24 refused, 1 asm-only, **1 finding**, 10 definitely lost | 149 checked, 24 refused, 1 asm-only, **0 findings**, 0 |
> | wall clock | 95.9 s | 98.5 s |
>
> Both findings were `std_str_order` and both are now gone; the C backend's
> pair was 4+6 bytes and the asm's one 10-byte record, the same `malloc`/`from_str`
> pair as before. Nothing was excused, no fixture changed its expected output,
> and `std_str_order.kp` itself is untouched.
>
> **Fixtures.** Two, both registered in `compile_and_run.rs` with exact stdout
> and both added to the AddressSanitizer list: `owned_conditional_move`
> (a branch that moves a local on one path, an assignment after the join that
> frees the old value, and a `return` — the same defect at `at_assign` and
> `at_return`) and `owned_move_in_loop` (the minimal repro, the loop-body shape
> whose block-end drop the two `while` passes have to write without losing).
> Both build and print identical bytes under both backends and are valgrind-clean
> in both; `owned_move_in_loop` is the one that leaked `3 bytes in 1 blocks` in
> the asm backend before this commit. The C backend's `-O2` build of the same
> program reads clean either way, so the gate that pins this is valgrind, not
> ASan.
>
> **What this does not change.** No new refusal: both backends build both
> fixtures and the whole differential sweep is unchanged (`24 refused of 172
> the C backend builds, 148 built`, rows heap 13, `enum` 0, a closure or
> function value 10, a gated extern 1 — the denominator moved from 170 to 172
> with the two fixtures, and `ASM_BUILT_BASELINE` gained them). The C backend's
> output for the rest of the corpus is unchanged except where a move site is now
> cleared. `try_drops_frame` and the other closure fixtures stay refused for
> their own reason (a function value), not because of this.

> **The print slice: an owned buffer inside a `print` template.** `
> heap_temporary_print` was refused with `printing a value of type \`String\`
> yet`, and it was not the buffer that was missing: `print(s)` of an owned
> `String` already built, and so did `print(str.to_upper(s))` — the single-value
> `print_buffer` shape. What was missing was the *template*, where an owned
> value is one interpolation among several.
>
> The work was not `%.*s` itself but the numbering around it. `template` numbered
> one variadic register per interpolated **value**; a `%.*s` interpolation is
> two integer arguments (the `int` length, then the `char*` data), so every
> interpolation after a buffer one sat one register too low and printed the
> wrong bytes — silently, since `printf` reads whatever is in `rsi`/`rdx`.
> It now numbers one *argument* per slot: each piece contributes its own list of
> slots and the pops run in reverse over the flattened list, so a buffer piece's
> pointer lands behind its length and everything after it shifts by two. The
> `int_slot > 5` refusal counts arguments too, and now names them
> ("interpolation arguments").
>
> All three buffer shapes are lowered, mirroring `printf_piece_with`'s three:
> an **owned** `Bytes`/`String` (only ever a temporary — `infer.rs` wraps a named
> place, so a local arrives as a borrow) is staged in a `Frame::scratch` slot,
> read out of it, and freed after the `printf`, exactly `print_buffer`'s
> lifetime; a **borrow** (`&Bytes`/`&String`) is the header's address and frees
> nothing; a **`View`** is a `{ptr, len}` value, so its pointer is the value's
> own first eightbyte rather than a field behind the pointer. Two staged
> temporaries in one template are two independent drop sites.
>
> **Measured.** `KELPC_BACKEND=asm cargo test --release --test compile_and_run`
> goes from **244 passed / 49 failed of 293** to **246 passed / 48 failed of
> 294**, and the failing set is the previous one minus exactly
> `printing_a_temporary_owned_buffer_works` — `heap_temporary_print` builds and
> prints byte-identically under both backends now. `backend_coverage --
> --ignored --nocapture` moves from **24 refused of 172, 148 built** (heap 13)
> to **23 refused of 173, 150 built** (heap 12): the heap row lost
> `heap_temporary_print`, and the new `template_buffer_pieces` fixture is built
> by both. `cargo test --release` is green, 294 of 294. `memory_gate --
> --ignored --nocapture` stays green over 324 runs / 308 fixtures (`c: 173
> checked, 0 findings`; `asm: 151 checked, 0 findings`) — the new fixture stages
> two owned buffers and is valgrind-clean in both backends, as is
> `heap_temporary_print`.
>
> `template_buffer_pieces` is the numbering fixture: an owned `String`, a
> borrow of a name, a `View` and an owned `Bytes`, each followed by a scalar, a
> float or another buffer, plus a two-buffer line (`AB` + `cd`). It is in
> `compile_and_run.rs` with exact stdout and in the AddressSanitizer list.
>
> **Still refused in a template:** a `List`/`Map`/`Box`/`datac` interpolation
> (no format for one, and `infer.rs` refuses it), a function value, and — by
> name, not by a wrong register — more than five integer argument slots or more
> than eight floats.

> **The sort slice: `List.sort`, `List.contains`, `List.slice`.** The three
> routines `codegen_asm.rs` had been declining by name since the list slice —
> "each needs an element-level comparison or clone the backend has no way to
> express yet" — are now written, and the three fixtures that were refused for
> them build: `stdlib_sort`, `list_slice_insert` and `list_slice_bounds`.
> `caps_fs_breadth`'s project still does not, and that is the *extern* it needs
> rather than these methods.
>
> The shape is `codegen_c.rs::write_list_s1`'s, one routine per element kind
> rather than one general comparison table:
>
> - `ListFn::SortCmp` — `kelp_List__<mangle>__cmp(a: &T, b: &T) -> i32`,
>   `write_list_s1`'s `{list}__cmp` and the same `-1/0/1` contract. A signed
>   integer compares `jg`/`jl`, an unsigned one `ja`/`jb`, `bool` as one
>   unsigned byte; a float compares through its own `ucomiss`/`ucomisd` with the
>   unordered case answering 0, which is what C's `(*a > *b) - (*a < *b)`
>   answers for a NaN; `String`/`Bytes` compare length-aware through `memcmp`
>   over the *shorter* prefix and then by length — never `strcmp`, because a
>   buffer may hold an embedded NUL, and a prefix must not pass. This is the one
>   comparator that takes a frame, because `memcmp` is a call and every
>   caller-saved register is libc's during it.
> - `ListFn::Sort` — the same stable in-place insertion sort, `v = data[i]` in
>   the routine's own frame buffer, and `data[j] = data[j-1]` for the shift, so
>   an *owned* element is moved rather than copied twice (valgrind checks it).
> - `ListFn::Contains` — the needle by value for a `Copy` element and by
>   *borrow* for an owned one (`infer.rs`'s `contains_param`), compared
>   length-first for `String`/`Bytes` (`memcmp` over the needle's whole length,
>   which the length check has already proved equal) and at the element's own
>   width and signedness otherwise.
> - `ListFn::Slice` — the C bounds check and panic message, `out = empty()` in
>   the caller's hidden return slot, `kelp_rt$list_reserve(out, to - from,
>   size)`, and one clone per element: `kelp_rt$string_clone`/
>   `kelp_rt$bytes_clone` for the two owned kinds, a `size`-byte copy otherwise.
>   The result is `MEMORY` class, so `slice`'s receiver and range shift one
>   register along, exactly as `pop`/`remove` already do for a `List<String>`.
>
> All three are gated on the *same* predicates `infer.rs`'s `list_sig` gates the
> signatures on (`types::list_sortable_elem`, `list_comparable_elem`,
> `list_clonable_elem`), so a method the type checker accepted cannot arrive
> without a lowering and one it refused cannot be lowered by accident. The
> `Sort` → `SortCmp` and `Slice` → `ListReserve`/clone dependencies are declared
> in `require_list_fn` at the call site, because the routine set is snapshotted
> before any body is written and a symbol registered at emission time would
> never be emitted.
>
> **Three defects this slice's own fixtures found, none of them pre-existing:**
>
> 1. the shift in `Sort` copied through `rax` while *also* using `rax` as the
>    destination address, so `data[j] = data[j-1]` wrote the last copied chunk
>    over the element it had just read (`stdlib_sort` segfaulted);
> 2. the `String` comparator returned from a frame it had not unwound, so the
>    `ret` popped the frame instead of the return address (`list_slice_insert`);
> 3. `contains` jumped to the loop's *next* label with the equality result
>    instead of branching on it, and then — once it branched — branched the
>    wrong way, so a mismatch read as a match and a match as a mismatch
>    (`list_slice_insert` printed the two `contains` answers swapped, then both
>    wrong).
>
> None of the three could print a wrong *number*: the first two are a segfault
> and a bad `ret`, and the third is exactly the shape the discipline warns about,
> which is why every fixture here is run under valgrind as well as
> differentially.
>
> **Measured.** `KELPC_BACKEND=asm cargo test --release --test compile_and_run`
> goes from **246 passed / 48 failed of 294** to **250 passed / 45 failed of
> 295**, and the failing set is the previous one minus exactly
> `a_list_sorts_reverses_searches_and_slices`,
> `sorting_and_slicing_a_list_of_owned_elements_moves_them_safely` and
> `list_sort_and_slice_reject_what_they_cannot_do` (which holds both the
> `List.slice` panic fixture and the `datac` element's by-name rejection, and
> still passes). `backend_coverage -- --ignored --nocapture` moves from **23
> refused of 173, 150 built** (heap 12) to **20 refused of 174, 154 built**
> (heap 9): three of the heap row's refusals were these methods, and the new
> fixture is built by both backends. `cargo test --release` is green, 295 of
> 295. `memory_gate -- --ignored --nocapture` stays green over 329 runs / 309
> fixtures (`c: 174 checked, 0 findings`; `asm: 155 checked, 0 findings`), with
> the four sort fixtures valgrind-clean in both backends.
>
> **Fixtures.** The three that were refused needed no edit — their stdout is
> asserted by tests that already existed. `list_sort_kinds` is new: the element
> kinds neither `stdlib_sort` (`i32`, `bool`) nor `list_slice_insert` (`String`)
> reaches — a signed `i32` with a negative value, an unsigned `u8` (`200` sorts
> above `3` only if the compare is unsigned), and `Bytes`, plus a slice of a
> `Copy` element and a slice of an owned one. It is in `compile_and_run.rs` with
> exact stdout and in the AddressSanitizer list.
>
> **Still refused, and why:** a `List<f64>`/`List<f32>` at all — `elem_size`
> answers `None` for a scalar float element, so `List.empty()` refuses it by
> name long before `sort` is reached, which is why `SortCmp`'s float arm has no
> fixture: it is written because `list_sortable_elem` admits a float and the
> C comparator has the same case, and it is the arm that would be *wrong*
> (rather than refused) if a float element list ever landed without it. The
> remaining heap-row refusals are the capability-gated externs
> (`fs_errno`, `io_stdio`, `io_streams`, the four `net_*`, `path_normalize`,
> `process_spawn`), each refused for its extern and none for a list method.

> **The A-18 array slice, and the borrow-return slice before it.** Two named
> refusals in `codegen_asm.rs` are gone. The first is `error: the 'asm' backend
> does not support the return type \`&String\` of \`view\` yet`: the
> return-class classification had a pointer-in-`rax` case that named `str`
> exactly, so a function could hand back a `&str` and nothing else. A borrow of
> *any* type is that same one pointer, and `value_kind` already answered for
> one, so the fix is the arm and the fixture that pins it: `borrow_returned`
> returns a `&Point`, a `&String`, a `&Bytes` and a `&Point` out of a returned
> `if`, and both backends print the same bytes.
>
> The second is the fixed-size array. `[T; N]` is a *value* here now, one
> synthesized layout per element type and length (`array_layout`, keyed
> `$<mangle>$N` so two lengths cannot share one), classified by the same
> per-eightbyte classifier everything else uses: a `[u8; 2]` is one `INTEGER`
> eightbyte and travels in `rax` exactly as a `u16` does, a `[i32; 3]` is a
> two-eightbyte aggregate, and a `[u8; 4096]` is `MEMORY`-class and goes through
> the hidden return pointer and the outgoing argument area like any other wide
> `datac`. The layouts are placed by `build_layouts`' own fixed point rather
> than after it, because the two directions both exist — a `[Gate; 4]` needs
> `Gate`'s layout, and `Idt` needs the array field's — and one pass that places
> nothing anywhere is what leaves the rest rejected by name.
>
> The routine set is four: `__len` and `__is_empty` are constants (the length is
> the type's, so neither reads memory at all), `__get`/`__set` are the index
> arithmetic plus the C backend's own bounds check and panic text, and the drop
> is a per-element loop that exists only for an element that owns something —
> `ty_owns([i32; 3])` is false, so no routine is emitted for one. A literal is
> written element by element into the destination (`array_lit_at`), a repeat
> stages its value **once** and copies it into each slot in a loop, and the four
> places that can hold a value wider than one eightbyte (`let`, an ident
> assignment, a `return`/tail, a call argument) intercept an array expression
> through `eval_struct_into` exactly as they already did for a `datac`.
>
> **Measured.** `backend_coverage -- --ignored --nocapture` moves from **25
> refused of 194, 169 built** to **21 refused of 195, 174 built**, and the
> ranking's array row is **empty**: `array_basic`, `array_owned`,
> `array_bounds` and `array_bounds_set` all build, run and print the C backend's
> bytes (the two bounds fixtures panic with the same message and a non-zero exit
> in both backends), and the slice's own `array_wide_and_float` pins the two
> element kinds those four do not reach — a float element, and an array wide
> enough to be `MEMORY` class as an argument *and* as a return — because a probe
> of the first found a real defect: a scalar float element was classified by the
> heap routines' integer default, so `[f64; 2].get(i)` loaded the element into
> `rax` while the call site read `xmm0`. Nothing in the corpus had a float
> element, so only the differential probe could see it. `KELPC_BACKEND=asm cargo test --test compile_and_run` is
> **282 passed / 46 failed of 328**, and the failing set is byte-identical to
> the one before this slice — no test that passed stopped passing, and the four
> A-18 tests now *run* under `--backend=asm` instead of short-circuiting with a
> note. `cargo test --test compile_and_run` (the C oracle) is green, 328 of 328.
> `memory_gate -- --ignored --nocapture` is green over 349 fixtures (`c: 195
> checked, 0 findings`; `asm: 174 checked, 21 fixture(s) refused by the asm
> backend (skipped), 0 findings`), with the five array fixtures valgrind-clean
> in both backends — which is the check that matters for the new per-element
> drop and for `set` freeing the element it replaces.
>
> **Still refused in this row:** an array of a type with no layout of its own
> (an element this backend cannot place is a refusal by name, never a stride
> guess), a zero-length array, and `Array.get` of a `MEMORY`-class element *by
> value* — the same refusal `List.get` makes for the same shape, and the same
> one `Box<T>::get` had to grow a `MEMORY`-class return path for above.

Definition of done: all 280 integration tests pass under `--backend=asm` with byte-identical stdout to
`--backend=c`;
`bench/speed/` runs all four workloads with matching checksums and is **no worse than the `-O0` column**;
`kelpc build` on `hello.kp` is comfortably under 40 ms (the `as`+`ld` share only); `cc` is not invoked. The
memory gate this list has always asked for is `cargo test --test memory_gate -- --ignored`
(`kelpc/tests/memory_gate.rs`, `0747195`): a `valgrind --leak-check=full` sweep over every fixture the C backend
builds and every one the asm backend accepts — 196 and 175 today, zero findings — run when the heap or drop
machinery changes. ASAN cannot serve, because it instruments generated C.

*(Corrected 2026-09-20, at `9c89b70`: this paragraph's current-tense numbers are the round it was
written in. It says `280 integration tests` and `196 and 175` fixtures; measured, the asm suite is
`319 of 344` with the C backend passing all `344`, and the valgrind sweep is `408 runs over 364
fixtures` (`c: 208 checked`, `asm: 200 checked, 8 refused`, `0 findings`). Its definition of done's
second clause is measured rather than pending: `bench/speed/speed.kp` builds under `--backend=asm` and
runs all four workloads with matching checksums. See §0's `9c89b70` blockquote and §13.)*

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

Removes libc. This is the Backlog part of `roadmap.md` B-6 (freestanding/no-runtime) plus B-7 (own allocator) plus `roadmap.md`
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
   (the Backlog part of `roadmap.md`'s cross-cutting note). glibc's hand-tuned SIMD `memcpy`/`strlen`/`memmove` are gone until B-9
   SIMD work exists, so text-heavy code will be slower. Debug info (DWARF, open item A-16) is absent unless
   built, which means `gdb` against a generated binary is not useful until then.

Definition of done: all 248 integration tests pass with no libc linked and no interpreter in the ELF; `ldd` reports
nothing or is not needed; `bench/speed/` checksums match and the runtime is no worse than `-O0`;
`kelpc build` is under 15 ms; and a capability fixture denied by a grant is denied with the same message.

---

## 8. Parallel track: runtime decoupling

> **Status: in progress.** The first two pieces §8 puts first — the capability
> matcher and the path normalizer — are Kelp as of `1071ead`, with the C kept as
> the reference each one is differentially tested against. The allocator is still
> blocked on its own terms (see the note at the end of this section), and the
> extern path that would let the asm backend *call* one landed separately in
> §5 item 10; 18 fixtures still name `std.mem.alloc_raw` in their refusal, and
> every one of them needs the heap types before the allocator itself matters.
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
>
> **One thing that note was waiting on has landed, on the other track** (2026-09-19):
> the asm backend can now lower an `extern fun` — `libc@PLT`, or a routine it
> owns (see §5 item 10). That is the asm-side half of "thin C-backed terminal
> primitives": `alloc_raw`, `realloc_raw` and `free_raw` are now *writable* as
> routines the asm backend emits, where before this slice no program containing
> them could build at all. What they are not is *useful* yet — a routine
> returning a `Bytes` hands back a value no other part of this backend can
> lower, so the heap row in §5's table is what actually has to move first. The
> allocator's own status is unchanged.

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

> **Status: the worktree half is still not started; the fan-out half has been used, and it is worth reading
> before the next one.** Stage 0 landed, so the prerequisite below is gone. What has actually happened since is
> a series of *short-lived* fan-outs in one shared tree rather than the two long-lived worktrees this section
> proposes: three children at a time, a fresh context each (`context: "fresh"` — an agent created with `fork`
> inherits the whole conversation, which is the expensive default to override), and an edit boundary per child,
> so the single-owner rule holds by construction. The backend still has exactly one writer per round — every
> remaining Stage 1 item is `codegen_asm.rs`, so the parallelism is one writer plus measurement and
> specification lanes, not two writers on the backend. Two process errors, recorded because both cost a
> rejected acceptance verdict rather than a bad commit:
>
> 1. **An acceptance criterion may only name what that slice's own scope can reach.** A brief listing six
>    must-pass fixtures when three of them need `List`/`enum`/`datac` guarantees a rejection, and the agent
>    that honestly reported *not satisfied* was right while the criterion was wrong.
> 2. **Commit the parent's work before fanning out, or isolate the lanes.** The checked-writer gate rejects
>    staged files repo-wide, so a parent that stages its own rename while children run can fail a child's
>    acceptance for something the child never touched.
>
> The rest of this section is still unexecuted detail — a proposal for the long-lived form.

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

**Docs.** One writer per file, always. `roadmap.md` and the Backlog part of `roadmap.md` belong to B on this branch; A writes
`syntax.md` and new files only. Long single-line bullets in these files conflict on every rebase otherwise.

## 10. Risk register

| risk | why it bites | mitigation |
| --- | --- | --- |
| Aggregate ABI mismatch | Kelp passes `datac`/`enum`/`String`/`List`/`Map`/`Box` by value; a wrong eightbyte split is a silent miscompile | differential stdout on all 261 fixtures; assert struct offsets against the C typedefs in a test |
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
- DWARF and source mapping (the Backlog part of `roadmap.md` A-16), though the `#line` gap this branch inherits stops being
  unfixable the moment a real backend exists.
- `unsafe`, raw pointers, user FFI, `repr(C)` (B-1/B-2/B-3), and threads (MK6-S1-M1). All are systems work
  that a real backend *enables*; none of them are on the path to a real backend.
- Fallible allocation (B-7's `try_alloc`): `malloc` failing stays a panic on this branch.

## 12. Annex: the heap runtime, as the C backend defines it

*(This annex was `plan/heap-runtime.md`. It is a reading of `codegen_c.rs` and
`types.rs` at the commit that folded it in: layouts, invariants, the runtime
functions each heap type needs, and drop order, every claim cited to a source
location. It is here rather than beside this file because it exists for the work
below and it expires with it — once the C runtime it describes is replaced, this
annex is history, not reference.)*


*What the asm backend must own to lower the six heap types. Every claim
carries a `file:line` citation. Where code and I disagree, see
[Uncertainties](#uncertainties). No code here is changed; this is a reading of
`codegen_c.rs` and `types.rs`.*

Sources read: `kelpc/src/codegen_c.rs` (`RUNTIME` at :36-542, `write_struct`
:979, `write_enum` :1027, `write_container_typedef` :1089, `write_tags` :1355,
`write_box` :1406, `write_map` :1467, `write_list` :1659, `write_list_s1`
:1734, `call_value` :3019), `kelpc/src/types.rs`, `kelpc/src/externs.rs`.

### 0. Naming rule (how codegen spells a type)

- `Ty::c_type` (`types.rs:222`) names the C type: `Bytes` -> `kelp_Bytes`,
  `String` -> `kelp_String`, `View` -> `kelp_View` (`types.rs:239-241`).
- `List<T>` -> `list_c_type` (`types.rs:572`) = `kelp_List__{ty_mangle(T)}`.
- `Box<T>` -> `box_c_type` (`types.rs:577`) = `kelp_Box__{ty_mangle(T)}`.
- `Map<K,V>` -> `map_c_type` (`types.rs:582`) =
  `kelp_Map__{ty_mangle(K)}__{ty_mangle(V)}`.
- `ty_mangle` (`types.rs:545`): `i32`->`i32`, `String`->`String`,
  `Bytes`->`Bytes`, a `datac` `shapes.circle.Circle` -> `shapes__circle__Circle`
  (via `c_ident`, `types.rs:656`), nested `Box<List<i32>>` -> `Box__List__i32`.
  An integer element is a compact name: `kelp_List__i32` (`types.rs:568-571`
  comments).
- `drop_fn_name` (`types.rs:461`): `Bytes`->`kelp_bytes_drop`,
  `String`->`kelp_string_drop`, `datac`->`kelp_drop_{c_ident(name)}`,
  `List`->`kelp_list_drop__{m}` , `Box`->`kelp_box_drop__{m}`,
  `Map`->`kelp_map_drop__{k}__{v}`. `View` has **no** drop arm (it is `Copy`;
  `is_copy` `types.rs:169`), and reaching `drop_fn_name(View)` is
  `unreachable!`.
- A `datac`/`enum` element inside a container typedef is spelled by its **tag**
  (`container_elem_c_type`, `types.rs:631`): `struct tag_{c_ident(name)}`,
  forward-declared by `write_tags` (`codegen_c.rs:1355-1365`). Every other
  element is `c_type()`.

### 1. Exact C layout, offsets, alignment, SysV class

All offsets/sizes below were also verified empirically with `offsetof` on the
typedefs (`/tmp`, not in the repo); they match the source.

#### kelp_Bytes (`codegen_c.rs:37-44`)
```
typedef struct { unsigned char* data; size_t len; size_t cap; } kelp_Bytes;
```
| field | type | offset | width |
| --- | --- | --- | --- |
| data | `unsigned char*` | 0 | 8 |
| len | `size_t` | 8 | 8 |
| cap | `size_t` | 16 | 8 |

size 24, align 8. 3 INTEGER eightbytes + size > 16 -> **MEMORY** class.
Element-type dependent: no.

#### kelp_String (`codegen_c.rs:316-321`)
Identical layout to `kelp_Bytes` (`unsigned char* data; size_t len; size_t
cap;`, `codegen_c.rs:316-321`). size 24, align 8, **MEMORY**. No.

#### kelp_View (`codegen_c.rs:115-118`)
| field | type | offset | width |
| --- | --- | --- | --- |
| ptr | `const char*` | 0 | 8 |
| len | `size_t` | 8 | 8 |

size 16, align 8, two INTEGER eightbytes -> **register pair** (arg
`rdi`+`rsi`, return `rax`+`rdx`). No.

#### kelp_List__* (`codegen_c.rs:1096-1101`)
```
typedef struct { <T-elem>* data; size_t len; size_t cap; } kelp_List__<m>;
```
| field | type | offset | width |
| --- | --- | --- | --- |
| data | `T*` (`container_elem_c_type(T)`) | 0 | 8 |
| len | `size_t` | 8 | 8 |
| cap | `size_t` | 16 | 8 |

size 24, align 8, **MEMORY**. The struct's size/alignment do **not** depend on
`T`: only the pointee type of `data` changes (`container_elem_c_type`), which is
`struct tag_Foo*` for a `datac`. The *function names* are monomorphized
(`ty_mangle(T)`), the layout is not.

#### kelp_Box__* (`codegen_c.rs:1106-1111`)
```
typedef struct { <T-elem>* value; } kelp_Box__<m>;
```
value at offset 0, width 8. size 8, align 8, one INTEGER eightbyte ->
`rdi`/`rax`. No dependence on `T`.

#### kelp_Map__K__V (`codegen_c.rs:1121-1134`)
```
typedef struct { K key; V* value; uint8_t live; } kelp_Map__K__V_entry;
typedef struct {
    <entry>* entries;
    size_t entry_count;
    size_t entry_cap;
    size_t len;
    size_t* index;
    size_t index_cap;
    size_t index_used;
} kelp_Map__K__V;
```
| field | type | offset | width |
| --- | --- | --- | --- |
| entries | entry* | 0 | 8 |
| entry_count | size_t | 8 | 8 |
| entry_cap | size_t | 16 | 8 |
| len | size_t | 24 | 8 |
| index | size_t* | 32 | 8 |
| index_cap | size_t | 40 | 8 |
| index_used | size_t | 48 | 8 |

size 56, align 8, **MEMORY**. The map header does **not** depend on `K`/`V`.
The **entry** struct does, because the key is inline:

| entry form | key | value | live | size | align |
| --- | --- | --- | --- | --- | --- |
| `K=String` (`key.c_type()` = `kelp_String`, 24) | 0..24 | 24 | 32 | 40 | 8 |
| `K` scalar (`i32`/`i64`/`bool`, `codegen_c.rs:1126`) | 0..8 | 8 | 16 | 24 | 8 |

`value` is always a pointer (a `datac` value is `struct tag_Foo*`); `live` is
`uint8_t` at `align_up(sizeof(K),8)+8`. An entry is only ever an array element
(`entries[e]`), never a by-value argument, so its SysV class is not exercised —
but its size is what `realloc(entries, cap*sizeof(entry))`
(`codegen_c.rs:1583`) and `memset(index, ...)` count on.

**SysV rule used throughout:** <=8 bytes -> one INTEGER eightbyte; 9-16 -> two
INTEGER eightbytes (register pair); >16 -> MEMORY. This mirrors
`codegen_asm.rs::agg_class` (`:807-816`) and `enum AggClass` (`:757`), and is
sound here because every field class is INTEGER (all fields are integers or
pointers; no SSE eightbyte exists — same argument the plan makes for `datac`,
§5 item 4, third slice). So: `Box` = one eightbyte; `View` = a register pair;
`Bytes`/`String`/`List`/`Map` = MEMORY (returned through the hidden first-arg
pointer; by-value parameters go in the caller's outgoing argument area).

### 2. Field invariants and who maintains them

#### kelp_Bytes / kelp_String

| invariant | statement | maintained by (cite) |
| --- | --- | --- |
| cap+1 allocation | the buffer is always `cap + 1` bytes; `cap` counts usable bytes **excluding** the NUL slot | comment `codegen_c.rs:41-43`; `kelp_buf_reserve` `:169` does `realloc(*data, want + 1)` `:180`; constructors set `cap = len` exactly: `from_str` `:229`, `clone` `:265`, `alloc_raw` `:529` |
| `data[len] == 0` | the byte at `len` is always NUL, so a `str` view / `printf("%s")` of the buffer is valid | `kelp_buf_reserve` writes `p[len] = 0` after every `realloc` `:184`; `__push` `:283`, `__append` `:291`, `__push_str` `:422`; `__empty` `:212` (`p[0]=0`); `from_str` copies `n+1` `:233`; `clone` `:270`; `alloc_raw` `memset(p,0,n+1)` `:534` |
| empty value points at a NUL byte, never NULL | `Bytes.empty()`/`String.empty()` = `malloc(1)` with `p[0]=0`, `len=0`, `cap=0` | comments `codegen_c.rs:212-216`, `:324`; bodies `:217-227`, `:324-335` |
| `len` = byte count | number of live bytes; a `str` view of the buffer is `len` bytes | `__len` `:243`, `kelp_String__len` `:373`; `char_count` `:381` counts code points separately |
| text validity | `String` bytes must be valid UTF-8 | `kelp_String__from_bytes` `:353-355`; `kelp_Bytes__as_str` `:482-484`; `kelp_utf8_valid` `:186` |

`kelp_String__with_capacity(n)` (`codegen_c.rs:429`) sets `data=NULL, len=0,
cap=0` then reserves; for `n=0` the reserve early-returns (`len+extra <= *cap`)
and leaves `data==NULL` — see Uncertainties.

#### kelp_View
| invariant | statement | maintained by |
| --- | --- | --- |
| borrow, no ownership | `{ptr,len}`; owns nothing, never dropped | `is_copy` `types.rs:169`; no `drop_fn_name` arm |
| `len` is exact | every op is length-aware; no reliance on a NUL after the slice | comment `codegen_c.rs:103-113`; `__eq` `:147` (memcmp over exact len), `__to_string` `:466` copies `len` |

#### kelp_List__T
| invariant | statement | maintained by |
| --- | --- | --- |
| `len` = live element count | elements `0..len` are initialized | comment `codegen_c.rs:1089`; `__push` `:1695`, `__pop` `:1705`, `__insert` `:1773`, `__remove` `:1783` all keep it |
| `cap` >= `len`; buffer is `cap*sizeof(T)` bytes | `data` may be NULL iff `cap==0` | `__empty` `:1670`, `__reserve` `:1675` |
| owned elements dropped before reuse | `set`/`clear`/`drop` free the element they overwrite/remove | `__set` `:1702`, `__clear` `:1710`, `kelp_list_drop__*` `:1728` |

#### kelp_Box__T
| invariant | statement | maintained by |
| --- | --- | --- |
| `value == NULL` means empty | `empty()` `:1414`, `take()` clears it `:1450` | `write_box` `codegen_c.rs:1406` |
| `value != NULL` -> one live `T` at `malloc(sizeof(T))` | `of`/`set` allocate | `:1417`, `:1436` |
| drop/replace frees the held value | `set` drops the old `T` before overwrite `:1439`; drop drops then frees `:1455` | `write_box` |

#### kelp_Map__K__V
| invariant | statement | maintained by |
| --- | --- | --- |
| entries array in insertion order | `entries[0..entry_count]`; live entries have `live==1` | comment `:1471-1477`, `write_map` `:1467` |
| `len` = live key count | `insert++` `:1598`, `remove--` `:1618`, `clear=0` `:1626` | `write_map` |
| `index` open-addressed, `entry index + 1` | slot holds `e+1`; `0` = empty; `SIZE_MAX` = tombstone | comment `:1471-1477`, `:1123-1127`; probe `__find` `:1540`; `remove` writes `SIZE_MAX` `:1617` |
| `index_cap` a power of two | probe masks by `index_cap - 1` | `__rehash` `:1562-1564` |
| `index_used` = live indexed slots | load factor `(index_used+1)*4 >= index_cap*3` triggers rehash | `__rehash` `:1571`, `__insert` `:1576` |
| `live` flag | `1` live, `0` dead (removed/cleared); the slot's `value` pointer is NULL when dead | `insert` `:1597`, `remove` `:1616`, `clear` `:1623` |
| a `String` key owns its buffer | stored via `kelp_String__from_str`/`__clone`; freed via `kelp_string_drop` | `store_key` `:1521`, `clone_key` `:1528`, `drop_key` `:1536` |

### 3. Runtime functions per type

`L` = a `List` C type, `B` a `Box` type, `M` a `Map` type, `T`/`V`/`K` the
element/value/key C types, `Karg` = `const char*` for a `String` key else `K`
(`codegen_c.rs:1484-1488`). Bodies are emitted by `write_list`/`write_box`/
`write_map` unless marked *(fixed)* = the `RUNTIME` string.

#### Bytes — 13 methods + 1 allocator, 2 shared helpers *(fixed)*
| C name | signature | returns | panics with | libc | does |
| --- | --- | --- | --- | --- | --- |
| `kelp_Bytes__empty` `:212` | `(void)` | `kelp_Bytes` | `out of memory` | malloc | one NUL byte, len=0,cap=0 |
| `kelp_Bytes__from_str` `:229` | `(const char*)` | `kelp_Bytes` | `out of memory` | strlen, malloc, memcpy | copy a C string |
| `kelp_Bytes__len` `:243` | `(const kelp_Bytes*)` | `size_t` | — | — | byte length |
| `kelp_Bytes__is_empty` `:247` | `(const kelp_Bytes*)` | `bool` | — | — | `len==0` |
| `kelp_Bytes__get` `:251` | `(const kelp_Bytes*, size_t)` | `uint8_t` | `Bytes.get: index out of bounds` | — | bounds-checked byte read |
| `kelp_Bytes__set` `:258` | `(kelp_Bytes*, size_t, uint8_t)` | `void` | `Bytes.set: index out of bounds` | — | bounds-checked byte write |
| `kelp_Bytes__clone` `:265` | `(const kelp_Bytes*)` | `kelp_Bytes` | `out of memory` | malloc, memcpy | deep copy |
| `kelp_Bytes__push` `:279` | `(kelp_Bytes*, uint8_t)` | `void` | `out of memory` (via reserve) | realloc | append one byte, re-NUL |
| `kelp_Bytes__append` `:285` | `(kelp_Bytes*, const kelp_Bytes*)` | `void` | `out of memory` | realloc, memcpy | append another buffer |
| `kelp_Bytes__reserve` `:295` | `(kelp_Bytes*, size_t)` | `void` | `out of memory` | realloc | grow to fit `extra` |
| `kelp_Bytes__clear` `:299` | `(kelp_Bytes*)` | `void` | — | — | len=0, `data[0]=0` |
| `kelp_Bytes__as_str` `:482` | `(const kelp_Bytes*)` | `const char*` | `Bytes.as_str: input is not valid utf-8` | — | checked borrowed view |
| `kelp_bytes_drop` `:306` | `(kelp_Bytes*)` | `void` | — | free | free, data=NULL, len=0 |
| `kelp_bytes_alloc_raw` `:529` | `(size_t n)` | `kelp_Bytes` | `out of memory` | malloc, memset | zeroed NUL-terminated buffer of `n` |
| `kelp_buf_reserve` `:169` *(shared)* | `(unsigned char**, size_t, size_t*, size_t)` | `void` | `out of memory` | realloc | doubling growth + re-NUL |
| `kelp_utf8_valid` `:186` *(shared)* | `(const unsigned char*, size_t)` | `bool` | — | — | UTF-8 well-formedness check |

#### String — 16 functions *(fixed)*
| C name | signature | returns | panics with | libc | does |
| --- | --- | --- | --- | --- | --- |
| `kelp_String__empty` `:323` | `(void)` | `kelp_String` | `out of memory` | malloc | one NUL byte |
| `kelp_String__from_str` `:337` | `(const char*)` | `kelp_String` | `out of memory` | strlen, malloc, memcpy | copy text |
| `kelp_String__from_bytes` `:353` | `(kelp_Bytes)` | `kelp_String` | `String.from_bytes: input is not valid utf-8` | kelp_utf8_valid | adopt a valid `Bytes` buffer |
| `kelp_String__into_bytes` `:365` | `(kelp_String)` | `kelp_Bytes` | — | — | hand the buffer out |
| `kelp_String__len` `:373` | `(const kelp_String*)` | `size_t` | — | — | byte length |
| `kelp_String__is_empty` `:377` | `(const kelp_String*)` | `bool` | — | — | `len==0` |
| `kelp_String__char_count` `:381` | `(const kelp_String*)` | `size_t` | — | — | count code points |
| `kelp_String__clone` `:391` | `(const kelp_String*)` | `kelp_String` | `out of memory` | malloc, memcpy | deep copy |
| `kelp_String__push` `:408` | `(kelp_String*, char)` | `void` | `out of memory` | realloc | append one byte |
| `kelp_String__push_str` `:414` | `(kelp_String*, const char*)` | `void` | `out of memory` | strlen, realloc, memcpy | append C text |
| `kelp_String__reserve` `:425` | `(kelp_String*, size_t)` | `void` | `out of memory` | realloc | grow |
| `kelp_String__with_capacity` `:429` | `(size_t)` | `kelp_String` | `out of memory` | realloc | reserve up front |
| `kelp_String__clear` `:438` | `(kelp_String*)` | `void` | — | — | len=0, `data[0]=0` |
| `kelp_String__as_str` `:447` | `(const kelp_String*)` | `const char*` | — | — | borrowed `str` |
| `kelp_String__view` `:454` | `(const kelp_String*)` | `kelp_View` | — | — | `{data,len}` view |
| `kelp_string_drop` `:489` | `(kelp_String*)` | `void` | — | free | free, data=NULL, len=0 |

#### View — 7 functions *(fixed; no drop)*
| C name | signature | returns | panics with | libc | does |
| --- | --- | --- | --- | --- | --- |
| `kelp_View__len` `:120` | `(kelp_View)` | `size_t` | — | — | length |
| `kelp_View__is_empty` `:124` | `(kelp_View)` | `bool` | — | — | `len==0` |
| `kelp_View__get` `:128` | `(kelp_View, size_t)` | `uint8_t` | `View.get: index out of bounds` | — | bounds-checked read |
| `kelp_View__slice` `:135` | `(kelp_View, size_t, size_t)` | `kelp_View` | `View.slice: index out of bounds` | — | sub-view |
| `kelp_View__eq` `:147` | `(kelp_View, kelp_View)` | `bool` | — | memcmp | length-aware equality |
| `kelp_View__to_string` `:466` | `(kelp_View)` | `kelp_String` | `out of memory` | malloc, memcpy | copy `len` bytes + NUL |
| `kelp_str__view` `:158` | `(const char*)` | `kelp_View` | — | strlen | view a whole `str` |

#### List — 18 per instantiation + 1 drop (some conditional)
Conditional set is decided by `list_sortable_elem` / `list_comparable_elem` /
`list_clonable_elem` (`types.rs:430-448`), same predicates `infer.rs` uses.

| C name (per `L`) | signature | returns | panics with | libc | does |
| --- | --- | --- | --- | --- | --- |
| `<L>__empty` `:1670` | `(void)` | `L` | — | — | data=NULL,len=0,cap=0 |
| `<L>__reserve` `:1675` | `(L*, size_t)` | `void` | `out of memory` | realloc | grow to `want` |
| `<L>__of` `:1684` | `(T)` | `L` | `out of memory` | realloc | one-element list |
| `<L>__len` `:1689` | `(const L*)` | `size_t` | — | — | element count |
| `<L>__is_empty` `:1690` | `(const L*)` | `bool` | — | — | `len==0` |
| `<L>__push` `:1695` | `(L*, T)` | `void` | `out of memory` | realloc | append |
| `<L>__bounds` `:1698` | `(const L*, size_t)` | `void` | `List.get: index out of bounds` | — | bounds check |
| `<L>__get` `:1702` | `(const L*, size_t)` | `T` | bounds msg | — | copy out a `Copy` elem |
| `<L>__get_const` `:1705` | `(const L*, size_t)` | `const T*` | bounds msg | — | borrow an owned elem |
| `<L>__set` `:1710` | `(L*, size_t, T)` | `void` | bounds msg | — | overwrite (drops old elem) |
| `<L>__pop` `:1714` | `(L*)` | `T` | `List.pop: the list is empty` | — | remove last |
| `<L>__clear` `:1719` | `(L*)` | `void` | — | — | drop elems `0..len`, len=0 |
| `<L>__cmp` `:1741` | `(const T*, const T*)` | `int` | — | memcmp (String/Bytes) | comparator (if sortable) |
| `<L>__sort` `:1759` | `(L*)` | `void` | — | — | stable insertion sort (if sortable) |
| `<L>__reverse` `:1777` | `(L*)` | `void` | — | — | in-place reverse |
| `<L>__contains` `:1794` | `(const L*, <param>)` | `bool` | — | memcmp (String/Bytes) | membership (if comparable) |
| `<L>__slice` `:1807` | `(const L*, size_t, size_t)` | `L` | `List.slice: range out of bounds` | realloc | clone range (if clonable) |
| `<L>__insert` `:1827` | `(L*, size_t, T)` | `void` | `List.insert: index out of bounds` | realloc | shift + insert |
| `<L>__remove` `:1836` | `(L*, size_t)` | `T` | `List.remove: index out of bounds` | — | shift out, return value |
| `kelp_list_drop__{m}` `:1728` | `(L*)` | `void` | — | free | `__clear` + free buffer |

#### Box — 6 per instantiation + 1 drop
| C name (per `B`) | signature | returns | panics with | libc | does |
| --- | --- | --- | --- | --- | --- |
| `<B>__empty` `:1414` | `(void)` | `B` | — | — | value=NULL |
| `<B>__of` `:1417` | `(T)` | `B` | `out of memory` | malloc | allocate one `T` |
| `<B>__is_empty` `:1427` | `(const B*)` | `bool` | — | — | `value==NULL` |
| `<B>__get` `:1431` | `(const B*)` | `T` | `Box.get: the box is empty` | — | copy out a `Copy` T |
| `<B>__get_const` `:1434` | `(const B*)` | `const T*` | `Box.get: the box is empty` | — | borrow an owned T |
| `<B>__set` `:1438` | `(B*, T)` | `void` | `out of memory` | malloc | allocate-if-empty, drop old, store |
| `<B>__take` `:1450` | `(B*)` | `T` | `Box.take: the box is empty` | free | move value out, free box |
| `kelp_box_drop__{m}` `:1455` | `(B*)` | `void` | — | free | drop value, free box, NULL |

#### Map — 12 methods + 1 drop (+3 shared key helpers)
| C name (per `M`) | signature | returns | panics with | libc | does |
| --- | --- | --- | --- | --- | --- |
| `<M>__new` `:1539` | `(void)` | `M` | — | — | all-NULL/0 header |
| `<M>__find` `:1544` | `(const M*, Karg, bool*)` | `size_t` | — | memcmp/strlen (String key) | probe; slot or insert-at |
| `<M>__rehash` `:1562` | `(M*, size_t want)` | `void` | `out of memory` | free, malloc, memset | rebuild index, drop tombstones |
| `<M>__insert` `:1576` | `(M*, Karg, V)` | `void` | `out of memory` | realloc, malloc | insert/replace; drops old value |
| `<M>__has` `:1600` | `(const M*, Karg)` | `bool` | — | — | key present |
| `<M>__get` `:1605` | `(const M*, Karg)` | `V` | `Map.get: key not found` | — | copy a `Copy` value |
| `<M>__get_const` `:1608` | `(const M*, Karg)` | `const V*` | `Map.get: key not found` | — | borrow an owned value |
| `<M>__get_or` `:1611` | `(const M*, Karg, V)` | `V` | — | clone helpers | value or fallback (if clonable) |
| `<M>__remove` `:1617` | `(M*, Karg)` | `V` | `Map.remove: key not found` | free | mark dead, tombstone, return value |
| `<M>__clear` `:1622` | `(M*)` | `void` | — | free, memset | drop all live, keep buffers |
| `<M>__len` `:1628` | `(const M*)` | `size_t` | — | — | live count |
| `<M>__is_empty` `:1629` | `(const M*)` | `bool` | — | — | `len==0` |
| `<M>__keys` `:1632` | `(const M*)` | `kelp_List__{m(K)}` | `out of memory` | realloc | insertion-order key list (clones keys) |
| `<M>__values` `:1644` | `(const M*)` | `kelp_List__{m(V)}` | `out of memory` | realloc | insertion-order values (if clonable) |
| `kelp_map_drop__{k}__{v}` `:1651` | `(M*)` | `void` | — | free | drop live values/keys, free both arrays |
| `kelp_map_hash` `:502` *(shared)* | `(const char*, size_t)` | `uint64_t` | — | — | FNV-1a over key bytes |
| `kelp_map_key_eq` `:511` *(shared)* | `(const kelp_String*, const char*, size_t)` | `bool` | — | memcmp | length-aware `String` key equality |
| `kelp_map_mix` `:520` *(shared)* | `(uint64_t)` | `uint64_t` | — | — | scalar-key 64-bit finalizer |

Counts: **Bytes 14 (+2 shared), String 16, View 7, List 19 (18 + drop, up to 4
conditional), Box 7, Map 15 (14 + drop, 1 conditional) (+3 shared key helpers).**

### 4. Drop order

The fixed drop entry points are `kelp_bytes_drop` / `kelp_string_drop`; every
other type's drop is **generated per instantiation** and named by
`drop_fn_name` (`types.rs:461`). The caller is `drop_call`
(`codegen_c.rs:3356`), driven by `borrowck.rs`'s `DropPlan`.

| type | what a drop frees, in order | generated? |
| --- | --- | --- |
| `Bytes` | `free(data)`; then `data=NULL`, `len=0` (`cap` left stale) `:306-310` | fixed |
| `String` | `free(data)`; then `data=NULL`, `len=0` (`cap` left stale) `:489-493` | fixed |
| `View` | nothing (Copy borrow) | no drop |
| `List<T>` | `__clear`: drop elements `data[0..len]` in order (only if `needs_drop(T)`), then `free(data)`, `data=NULL`, `cap=0` `:1728-1733` | yes, per `T` |
| `Box<T>` | if `value!=NULL`: drop `*value` (if `needs_drop(T)`), then `free(value)`, `value=NULL` `:1455-1462` | yes, per `T` |
| `Map<K,V>` | for each `e` in `0..entry_count`: skip `!live`; drop `*entries[e].value` (if `needs_drop(V)`), `free(entries[e].value)`; drop the key (`kelp_string_drop`) if `K==String`; then `free(entries)`, `free(index)`, zero all header fields `:1651-1658` | yes, per `K,V` |
| `datac` | drop each owned field in declaration order, calling `drop_fn_name(field.ty)` `:995-1002` | yes, per `datac` |
| `enum` | switch on `v->kelp_tag`; drop the live variant's owned payload fields only `:1027-1072` | yes, per `enum` |

Recursive cases:
- `List<String>`: `elem_drops = needs_drop(String) = true`; `__clear` calls
  `kelp_string_drop(&data[i])` per element, then drops the buffer (`:1710`,
  `:1728`).
- `List<Shape>` where `Shape` is an `enum`: `owned_types` (`types.rs:385`)
  marks `Shape` owned if any variant owns; `__clear` then calls
  `kelp_drop_Shape(&data[i])`, which switches on each element's tag.
- recursive `Box` chain (`Node` holds `Box<Node>`): `kelp_box_drop__Node`
  calls `kelp_drop_Node` on the held value (because `needs_drop(Node)`), and
  `kelp_drop_Node` calls `kelp_box_drop__Node` on its field — one frame per
  link, terminated by the `NULL` (`Box.empty()`).
- `Map` whose values own memory: `kelp_map_drop__K__V` drops each live
  `*value`; dead entries were already freed by `remove`/`clear`.
- `enum`, exactly one variant live: the drop *must* be generated and must
  switch on `kelp_tag`, because the compiler cannot know statically which
  payload owns memory; only the live variant's owned fields are dropped (no
  `default:` arm, by design — `codegen_c.rs:1067-1069`).

Which cases need a generated per-type drop (not a fixed routine): every
`List<T>`, `Box<T>`, `Map<K,V>`, `datac` that owns memory, and every `enum` any
of whose variants owns memory. The switch a generated function has to make is
compile-time for `List`/`Box`/`Map` (does the element/value/key need a drop,
`needs_drop`, `types.rs:451`) and **run-time for `enum`** (which variant's tag
is set).

### 5. Terminal primitives the heap runtime depends on

Not declared in `externs.rs` — the generated C calls them directly and the asm
backend must supply them (libc `@PLT` at first, raw syscalls later). Counted
over the `RUNTIME` text (`codegen_c.rs:36-542`):

`malloc`, `realloc`, `free`, `memcpy`, `memset`, `memcmp`, `strlen`
(allocation/copy/compare). Also on the print/panic path the same text uses
`fprintf`, `printf`, `stderr`, `stdout`, `fflush`, `exit`, `strerror`, `errno`,
and the extern bodies add `snprintf`, `strtoll`, `strtoull`, `strtod`,
`getenv`, `setenv`, `unsetenv`, `fopen`/`fread`/`fwrite`/`fclose`/`fseek`/
`ftell`/`rewind`/`remove`/`rename`/`mkdir`/`stat`/`scandir`, `clock_gettime`,
`nanosleep`, `pipe`/`fork`/`execvp`/`waitpid`/`kill`, and the socket family.

`externs.rs` bodies that **depend on** these heap routines (so the routines
must exist before those externs can be lowered): `std/mem.alloc_raw` ->
`kelp_bytes_alloc_raw`; `std/str.from_f64_raw` -> `kelp_bytes_alloc_raw`,
`kelp_String__from_bytes`; `std/fmt.hex_raw` -> `kelp_String__from_str`;
`std/fmt.bytes_raw` -> `kelp_String__empty`, `kelp_bytes_alloc_raw`,
`kelp_String__from_bytes`; `std/fs.read_raw`/`list_raw` -> `kelp_bytes_alloc_raw`,
`kelp_String__from_bytes`; `std/env.get_raw` -> `kelp_String__empty`,
`kelp_String__from_str`; `std/io.read_raw`/`std/net.recv_raw` ->
`kelp_bytes_alloc_raw`; `std/process.arg_at_raw` -> `kelp_String__from_str`;
`std/path.normalize_raw` -> the `kelp_Bytes` view layout, `kelp_bytes_alloc_raw`,
`kelp_String__from_bytes`. Bodies that read `kelp_Bytes`/`kelp_String` fields
directly depend on the §1 layout: `std/io.read_into_raw`/`write_bytes_raw`,
`std/net.send_raw`/`recv_into_raw`, `std/fs.write_raw`/`write_text_raw`, and
`std/process.spawn_raw` (which names `kelp_List__String` and `kelp_String.data`
literally — `externs.rs:390`).

### Uncertainties

1. **`String.with_capacity(0)`.** ~~`kelp_String__with_capacity` (`:429`) starts
   with `data=NULL` and `reserve` early-returns when `len+extra <= cap`, so a
   zero capacity leaves `data==NULL` — breaking the "an empty value never
   points at NULL" invariant the other constructors keep. I could not find a
   call site that passes 0, and no fixture pins it. Treat as an edge to decide,
   not a guaranteed difference.~~ **Resolved, and it was a bug rather than an
   edge**: both backends now allocate the one NUL byte `String.empty()`
   allocates, and `kelpc/tests/fixtures/string_with_capacity_zero.kp` — which
   segfaulted under the pre-fix runtime, at `str.len(s.as_str())` — pins it.
   See §5's note beside the heap-slice blockquote.
2. **`cap` after a drop.** `kelp_bytes_drop`/`kelp_string_drop` reset `data`
   and `len` but **not** `cap` (`:306-310`, `:489-493`). Harmless (the pointer
   is NULL) but the struct is not fully re-initialized; a backend that re-reads
   `cap` on a dropped value would see the old value.
3. **SysV threshold.** I state `>16 bytes -> MEMORY` following
   `codegen_asm.rs::agg_class` (`:807-816`) and the plan's own §5 item 4
   ("over sixteen bytes ... `MEMORY`"). The literal SysV wording is subtler
   (an aggregate over two eightbytes is MEMORY *unless* its first eightbyte is
   SSE/SSEUP); since no field here is SSE-classified, both readings agree for
   these six types. A rule that every field is INTEGER/pointer is what makes
   the simple threshold sound.
4. **`Map` entry SysV class** is never exercised (entries are only array
   elements), so I did not classify it; only its size/offset matter.
5. **`Map#get_or` / `values` conditions.** Their emission is keyed on
   `list_clonable_elem(value)` (`:1611`, `:1644`), the same predicate
   `infer.rs` uses; if the two ever diverge, the call site would name a symbol
   `codegen_c.rs` did not emit. I did not re-verify `infer.rs` against it.
6. **Conditional `List` methods** (`__cmp`/`__sort`/`__contains`/`__slice`)
   follow `types.rs`'s predicates (`:430-448`); the count "19" assumes the
   maximal element kind (a sortable+comparable+clonable element such as
   `String`). A `List<datac>` emits fewer.


## 13. Annex: the `bench/speed` gate, measured

*(This annex was `plan/speed-asm.md`, measured 2026-09-19 at `d0a485f`. It is the
only measurement of §3's gate this branch has. Its headline is the measurement's
own result: the gate artifact does not build under `--backend=asm` yet, so the gate
is unmet by the letter of §5's definition of done, and what the annex does have is a
probe on the two kernels that can be built plus the fairness conditions a real gate
run would have to satisfy.)*

> **Re-measured 2026-09-20 at `9c89b70`, the current tip: the gate artifact builds, and this annex's
> asm column and its two prerequisites are superseded.** `bench/speed/speed.kp` builds under
> `--backend=asm` and under `--backend=c`, and both columns run. The clock extern landed at `5958794`
> and the scalar `mut` borrow at the scalar-borrow slice; the capability check this annex does not name
> was not a second blocker, because `speed.kp`'s only gated call is `time.now_mono_ns()`, which names
> no resource. Re-measured with the same four columns and one estimator for every column (interleaved
> round-robin, 9 rounds, round 1 discarded, median of the 8 that remain), workload elapsed ms: collatz
> `-O0` 449.8, `-O2` 81.4, asm 429.7 (`asm/-O0` 0.955); float 255.7, 100.1, 149.5 (0.585); mix 201.1,
> 166.7, 139.1 (0.692); fib 26.8, 23.4, 46.0 (1.713). Whole-process external wall `-O0` 933.8 ms,
> `-O2` 373.0 ms, asm 765.3 ms, ratio 0.820. Every column printed the annex's checksums and the float
> `VALUE` line, so the table below is confirmed rather than corrected; by §5's per-workload reading the
> gate is unmet on `fib` alone, as the `4752ebb` blockquote already says. The sweep ran at load 4.08
> from another lane, which is why `float`'s median is reported with its spread (138.8-199.8 ms); a gate
> run on an idle machine is still owed. `plan/mk7-next-slice.md` carries the same table. The one number
> here the concurrent gate-table rewrite is expected to invalidate is none: the fix's own spec measures
> `speed.kp`'s generated C unchanged under rule (b), because its gated call names no resource. A future
> round that changes the estimator, the machine's idle state or a kernel does invalidate every cell,
> which is why each names `9c89b70`.

*(2026-09-20, at `926e33f`: the idle-machine gate run this annex has owed since it was written is still
owed, and `fib` at 1.713 remains the one failing workload. Measured this round, twice, at `7dc08d7` and
at `926e33f` (the artifact builds under both backends at both commits, and every column printed the four
checksums and the float `VALUE` line): with the same estimator the blockquote above fixes, 9 rounds
interleaved round-robin, round 1 discarded, median of 8, `fib` reads 1.845 against `-O0` at load 20.7 and
1.795 at load 11.9; `collatz` 1.044 and 0.999, `float` 0.964 and 0.854, `mix` 1.099 and 0.981, and
whole-process external wall 1.078 and 0.994. Both sweeps ran while another lane compiled and drove
valgrind, which is why every cell sits nearer 1 than the `9c89b70` blockquote's `collatz` 0.955, `float`
0.585 and `mix` 0.692, and why neither is a gate run: what they confirm is the blockquote's per-workload
reading, that the `fib` cell is the only one above 1, not any cell's value. The run this annex asks for,
on an idle machine with one estimator, is still owed, and `float`'s bimodality is still unmeasured with
nothing else running.)*

**Headline: the gate cannot be evaluated as written, because `bench/speed/speed.kp`
does not build under `--backend=asm` at HEAD `d0a485f`.** The C columns measure
cleanly and reproduce §2's baseline. A supplementary probe (exact workload
functions, clock calls removed) shows `collatz` and `mix` beat the `-O0` column
under the asm backend; `float` and `fib` cannot be built at all, because each
passes a `mut i64` counter and the asm backend refuses a borrow of a scalar.

*(Corrected 2026-09-20, at `c3a8759`: one of this annex's two prerequisites has landed, and the other is
not the one it names. The scalar `mut` borrow is in the backend, so `float` and `fib` do build. Measured:
the whole workload file with `impl "std/time"` removed and `time.now_mono_ns()` replaced by `0` builds under
both backends and prints identical stdout, with `collatz 131434424`, `float 57735026`,
`VALUE 78539816.062926`, `mix 542344` and `fib 5702887`, so nothing else in `speed.kp` refuses and the
annex's "whether the asm backend has further refusals behind the two above" is answered: none. The refusal
now printed is still `extern fun std.time.now_mono_ns_raw`, and behind it stands the capability check,
because `std/time` is a gated module and the gate refusal is at the call site (the annex does not name the
check). So what stands between the gate artifact and a build is `std/time`'s three extern bodies and the
capability check. The table, the method and the fairness conditions above are unchanged and remain this
annex's measurement at `d0a485f`.)*

*(Corrected 2026-09-20, at `4752ebb`: the gate artifact builds now, and its asm
column exists. `bench/speed/speed.kp` builds under `--backend=asm` at `4752ebb`,
and it also builds at `5958794`, which is the slice that lowered `std.time`'s
clock extern; measured here by building `kelpc` at both commits and running the
binary, which printed all four checksums and the `VALUE` line. So the headline
above, and the `cannot build speed.kp` cells in the gate table, are no longer
true, and item 1 of "What a follow-up must do" is met by the extern body alone.
The capability check the c3a8759 correction puts behind the refusal was not a
second blocker: `speed.kp` names only `time.now_mono_ns()`, which takes no
resource argument, and `cap_check_refusal` returns `None` for a gated call whose
checked-argument list is empty (`codegen_asm.rs`'s own unit test
`a_gated_domain_with_no_checked_arguments_is_not_refused` asserts exactly this
for `std.time.now_unix`). This is a new measurement, not a correction of the old
one: the table above, the method and the fairness conditions stay as they were
measured at `d0a485f`.)*

*(Measured 2026-09-20 at `4752ebb`, this session, `kelpc` built `--release` from
a detached worktree of that commit. Same four columns as the table above: `cc
-O0 -fwrapv -lm` and `cc -O2 -fwrapv -lm` over the backend's generated C, and
`--backend=asm`. One estimator for every column, the one item 4 above asks for:
9 rounds interleaved round-robin, round 1 discarded, median of the 8 that remain,
spread is min to max of those 8. `work_ns` is the program's own clock,
external wall is `date +%s%N` around the whole process. Load average 2.54 to
2.74 during the sweep, from another lane compiling. Every column printed the
gate's checksums and the float `VALUE` line.)*

| workload | `-O0` | `-O2` | asm | asm/`-O0` |
| --- | --- | --- | --- | --- |
| collatz | 446.5 (444.4-450.6) | 81.5 (80.7-82.8) | 430.5 (427.9-433.0) | 0.964 |
| float | 254.8 (253.0-257.9) | 100.4 (98.7-103.0) | 151.5 (145.2-234.3) | 0.594 |
| mix | 203.1 (199.9-204.2) | 165.2 (164.2-168.4) | 139.0 (138.0-140.8) | 0.684 |
| fib | 26.7 (26.6-28.2) | 23.3 (23.0-24.1) | 46.1 (45.6-47.4) | 1.723 |

*(The float `VALUE` line is `78539816.062926` in every column; the checksums are
`collatz 131434424`, `float 57735026`, `mix 542344`, `fib 18454929` calls and
`5702887`. Whole-process external wall clock, median of the same 8 rounds:
`-O0` 933.0 ms, `-O2` 374.1 ms, asm 771.9 ms, which is 0.827 of `-O0`. A second
sweep of the same columns, 15 rounds kept, pinned to one core with `taskset -c
3`, load 2.13 to 3.13, gave collatz 0.969, float 0.696, mix 0.707, fib 1.727 and
a whole-process wall ratio of 0.878, so the reading is stable.)*

*(The concurrency caveat of item 5 above still holds and now has a number: the
asm `float` column is bimodal. Its 8 kept runs spanned 145.2 to 234.3 ms here and
137.3 to 502.8 ms in an earlier sweep this session, while `-O0` and `-O2` stayed
inside about 2% of their own medians through the same window. So 0.594 is a
median that includes contended runs and the uncontended ratio is nearer 0.55.
Every other asm cell is tight (collatz 1.2% spread, mix 2.0%, fib 3.9%).)*

*(The gate criterion has two readings here and they disagree. Read as one
criterion over the artifact, the definition of done in section 5 is met: the
whole-process external wall is 0.827, so `bench/speed` is no worse than the `-O0`
column. Read per workload, the definition is met on three and unmet on one:
collatz 0.964, float 0.594, mix 0.684, fib 1.723. The plan's own text supports
the per-workload reading: section 5's status blockquote reports the ratio per
workload and says the asm backend is "above it on `fib` (1.70)", and the float
blockquote in section 5 ranks the four workloads the same way. So by the plan's
own usage the gate is **unmet**, and **`fib` is the one failing workload**: it is
the only cell above 1.0, in this sweep and in both earlier rounds (1.718 and
2.605 against the same `-O0` column).)*

*(Section 3's cost model is contradicted, and the measurement inverts it. Section
3 says a backend with a competent register allocator and no optimizer lands
between the two C columns and that "call-heavy code barely notices; branchy and
floating-point code notices a lot". The measured ratios are the other way round:
the call-heavy workload `fib` notices the most (1.723, and it is the worst cell
in the sweep) while the branchy workload `collatz` (0.964) and the floating-point
workload `float` (0.594) notice least and beat the `-O0` column outright. So the
column that moves when call sequence overhead rises is the one section 3 called
immune to it, and `plan/mk7-call-overhead.md` measures where that overhead goes:
most of it is the per-argument push and pop of the marshalling loop, then the
unconditional per-call stack alignment fixup, then the body's stack-machine
traffic.)*

*(Still unanswered from the follow-up list above: items 2 and 3 are satisfied by
the run above (external wall was recorded, and the float `VALUE` line was
asserted), item 4's estimator is the one used here, but item 5's "idle machine"
is not met, which is why the float cell is reported with its spread and the
bimodality above is spelled out rather than hidden behind the median.)*

### Machine, toolchain, and noise

`AMD Ryzen 5 7600` (6C/12T, 30 GiB), Linux `6.18.52`, `clang 21.1.8` as `cc`,
`cargo 1.95.0`, `kelpc` built `--release` from this branch. **Other lanes were
compiling on this machine during the sweep**; the widest spreads below (float
`-O2` 10%, `fib` `-O0` 5%) are consistent with that, not with the workloads.
7 runs each, run 1 discarded, median reported, spread is min–max of runs 2–7.

### Commands

```
kelpc build --backend=c bench/speed/speed.kp -o /tmp/sp/speed_c   # writes speed_c.c beside it
cc -O0 -fwrapv -lm -o speed_o0 speed_c.c                          # the -O0 column
cc -O2 -fwrapv -lm -o speed_o2 speed_c.c                          # the -O2 column
kelpc build --backend=asm bench/speed/speed.kp -o /tmp/sp/speed_asm
./speed_o0 ; ./speed_o2                                            # 7x each, median of 2..7
```

`cc` is clang 21, so the two columns are the plan's own clang `-O0`/`-O2`
columns, not `kelpc`'s hard-coded `-O2`.

### Gate table (workload elapsed ms, self-reported by the program)

| workload | -O0 | -O2 | asm | asm/-O0 | gate |
| --- | --- | --- | --- | --- | --- |
| collatz | 442.0 (439.9–449.4) | 80.2 (79.8–83.2) | cannot build speed.kp | — | **not met / not judgeable** |
| float   | 253.8 (252.5–258.0) | 109.2 (102.7–113.0) | cannot build speed.kp | — | **not met / not judgeable** |
| mix     | 201.4 (199.7–203.6) | 165.4 (164.5–169.6) | cannot build speed.kp | — | **not met / not judgeable** |
| fib     | 27.5 (26.94–28.34) | 23.3 (23.13–23.79) | cannot build speed.kp | — | **not met / not judgeable** |

The C columns reproduce §2's baseline within a few percent (446/83, 265/101,
204/168, 28.4/24.0). The asm column is empty because the **gate artifact itself
does not build**:

```
$ kelpc build --backend=asm bench/speed/speed.kp -o /tmp/sp/speed_asm
error: the 'asm' backend does not support the `extern fun std.time.now_mono_ns_raw` yet
```

Definition of done §5 says "`bench/speed/` runs all four workloads with matching
checksums and is no worse than the `-O0` column". Today it does not run at all,
so by the letter of §5 the gate is **unmet**.

### Supplementary probe: which kernels the asm backend can compile

New scratch files in `/tmp` (no repo file touched): each workload's function
copied verbatim, `impl "std/time"` and the `time.now_mono_ns()` calls removed so
the only difference is the clock. Results:

| workload kernel | asm builds? | blocker when built as the real workload |
| --- | --- | --- |
| collatz | yes | none beyond the clock extern |
| mix | yes | none beyond the clock extern |
| float | yes (counter removed) | `mut i64` counter: scalar borrow refused |
| fib | yes (counter removed) | `mut i64` counter: scalar borrow refused |

The scalar-borrow refusal (from the asm probe, exact text):

```
error: the 'asm' backend does not support a borrow of anything but a named `datac` local or a `datac` temporary yet
```

The `float` and `fib` workloads both take `above: mut i64` / `calls: mut i64`,
so the real workload is refused at that call regardless of whether f64 and
recursion work (both do — the counter-free probes build).

**Partial gate signal, collatz and mix only** (exact workload code, no
self-timing, external wall-clock; medians of 7, run 1 discarded):

| workload | -O0 | -O2 | asm | asm/-O0 | gate |
| --- | --- | --- | --- | --- | --- |
| collatz | 446 (442–448) | 84 (83–88) | **431 (429–437)** | 0.97 | **met** |
| mix     | 204.5 (202–207) | 168.5 (167–171) | **171 (170–173)** | 0.84 | **met** |

Wall here is workload + ~2 ms process startup, which is why `collatz -O2` reads
84 not 80. Both are inside the `-O0` column; the asm backend is roughly at, and
slightly better than, `-O0` on the two kernels it can build. This is **not** the
gate — the gate is the four-workload `bench/speed` binary, which does not exist.

### Checksums

All four integer checksums are byte-identical across `-O0`, `-O2`, every run,
and `bench/speed/results.txt` (where rust/kelp/cpp/py also agree):
`collatz 131434424`, `float 57735026`, `mix 542344`, `fib 5702887`. The float
`VALUE` also matches exactly: `78539816.062926` at both `-O0` and `-O2`. The
asm probes reproduce `collatz 131434424` and `mix 542344` on every run. **No
checksum mismatch was observed anywhere**, so the C columns are valid timings.

### What a follow-up must do before the asm number is trustworthy

1. **Build the gate artifact first.** Two asm-backend gaps block
   `bench/speed/speed.kp`: the `std.time.now_mono_ns_raw` extern (currently
   refused) and the scalar `mut` borrow. Until both lower, no asm column exists.
2. **Do not trust `work_ns` alone.** It is the program timing itself through
   `time.now_mono_ns()`; if the asm backend supplies that extern, record external
   wall-clock too, so a wrong clock cannot pass as a fast number.
3. **`float`'s integer checksum is weak.** `RESULT float ... 57735026` is the
   count above threshold; the FP sum is only the separate `VALUE` line, which
   `run.sh` does not assert on. A backend can get the division loop wrong and
   still match 57735026. Compare `VALUE` (or fold `acc` into the checksum)
   before calling float "the same work".
4. **One timing method across columns.** `run.sh` keeps the fastest of 3; the
   `-O0`/`-O2` columns here use medians of 6. A gate must fix one estimator
   (median, first discarded) for every column.
5. **Pin the machine state.** The 10% spread on float `-O2` came from concurrent
   compilation; a gate run must be on an idle machine or reported with its
   spread, not as a single number.

### What could not be determined

- The asm `-O0` ratio for any of the four workloads **as the gate defines it** —
  `bench/speed/speed.kp` does not build under asm.
- The asm ratio for `float` and `fib` even as a probe — both need the scalar
  `mut` borrow, which the asm backend refuses.
- Whether the asm backend has further refusals behind the two above: codegen
  stops at the first refusal, so the rest of `speed.kp` is unexercised.

---

## 14. Session record: 2026-09-20, `c3a8759` to `926e33f`

*Written at the end of the session that produced it, in one section, so a reader who was not here can
see what changed, what it cost, what went wrong and what is next without reading one report per commit.
`git log --oneline c3a8759..926e33f` is the commit list. Every number below was either measured by the
lane that wrote this section, in a detached worktree of the commit it names, or is labelled with the
report it came from under
`/home/juniper/.pi/agent/sessions/--mnt-bigdrive-projects-KelpLANG--/subagent-artifacts/`; a number
with no label is this section's own measurement, and a number with a label is that lane's, quoted and
not re-taken. The two endpoints and the tip are this section's own.*

### 14.1 What the session moved, measured at both ends

| | `c3a8759` | `926e33f` |
| --- | --- | --- |
| fixtures the C backend builds | 201 | 208 |
| fixtures the `asm` backend builds | 176 | 205 |
| refused by name | 25 | 3 |
| refusal ranking | heap 14, array 0, `enum` 0, float or heap `datac` 0, closure 10, gated extern 1 | heap 3, and the other five rows 0 |
| asm suite | 285 of 336 | 341 of 358 |
| C suite | 336 of 336 | 358 of 358 |
| valgrind | 377 runs over 357 fixtures: `c` 201, `asm` 176, 0 findings | 413 runs over 364 fixtures: `c` 208, `asm` 205, 3 refused, 0 findings |
| `bench/speed` under `--backend=asm` | does not build | builds, runs, every checksum matches; `fib` 1.713 to 1.845 depending on load |
| capability check | not emitted; every gated call refused | all five kinds emit; the four bypass routes and the function-value route closed |
| fixture files / project directories | 357 / 47 | 364 / 61 |

Commands, each in a worktree with its own `CARGO_TARGET_DIR` (`git worktree add --detach
/tmp/stop-record/start c3a8759` and `/tmp/stop-record/tip3 926e33f`, both removed afterwards): `cargo
test --release --test backend_coverage -- --ignored --nocapture`; `cargo test --release --test
compile_and_run` and the same with `KELPC_BACKEND=asm`; `cargo test --release --test memory_gate --
--ignored --nocapture`. The gate sweep: `bench/speed/speed.kp` built `--backend=c` and
`--backend=asm`, the generated C compiled with `cc -O0 -fwrapv -lm` and `cc -O2 -fwrapv -lm`, 9 rounds
interleaved round-robin, round 1 discarded, median of 8, twice, at load 20.7 and 11.9.

### 14.2 The session by slice, commit by commit

**A. The runtime track and the three specifications** (reconnaissance; no backend code).

- `c0f99c2` re-measured this plan and the next-slice brief at `c3a8759`: 176 built of 201, 25 refused,
  asm 285 of 336. Lane `2b0f726f`; this section's own `c3a8759` worktree reads the same.
- `a2a89e1` the DNS message parser in Kelp beside its C reference, over a 1990-case corpus (419
  accepted, 1571 rejected, 11 rejection classes). Lane `1b47d00f`, reproduced by review `2a6a481b`.
- `2505421` the stage 2 encoder and ELF inventory: 71 mnemonics (63 observed in 177 fixture builds),
  165 normalized `(mnemonic, operand form)` pairs, `PLT32` 3865 / `PC32` 3313 / `REX_GOTPCRELX` 127.
  Lane `219f342e`.
- `9da3653` the sweep's own recorded counts, `195/174/21` to `202/178/24`. Lane `bf0834e3`.
- `bb2316e` this plan and the brief at the then-tip: 178 built of 202, 24 refused, asm 288 of 336.
- `17ec217`, `44a881e`, `d4ed113` the three specifications (closures, the capability check, the extern
  bodies) and `1769152` the pointer to them. Their measured content: the extern body union is 46
  unlowerable externs behind the 12 extern-headed programs; the closure row is ten fixtures; the
  capability check's step 1 was predicted to make nothing build. Lanes `c8082723`, `dda15f85`,
  `06c34fe0`, `7ed276f2`.
- `796db95` the `/etc/hosts` and `/etc/resolv.conf` parser beside its C reference, over a 2612-case
  corpus (882 accepted, 1730 rejected, 9 rejection classes), and `3603792` the `dns_parse` note that a
  corpus of agreement is not a proof of the rules. Lanes `259b7194`, `df086b5c`, review `2a6a481b`. Both
  parsers' own pinned cases caught real bugs in them during the lane (a hyphen routed through
  `!is_alnum`, and a `::` scanner that ate the separator colon, so `2001:db8::1` was refused).

**B. S0: `std/rt` compiles, and the two path/cwd externs** (`8d86d8e`).

- before: 176 built of 201, 25 refused, asm 285 of 336, C 336. after: 178 built of 202, 24 refused, asm
  288 of 336, valgrind 202 `c` / 178 `asm`. Lane `d394612b`, review `8f272b13`.
- Three asm tests flipped, not two: `path_absolute_resolves_a_relative_argument_against_a_base`,
  `a_narrow_fs_grant_still_reaches_cwd` and
  `path_normalize_is_the_function_the_capability_checker_uses`. The commit message said 286 to 288; the
  true pre-slice number is 285, which the amended message now records (review `8f272b13`).
- Two defects inside it. The passdown `__dso_handle` emitter left the assembler in `.data`, so
  `kelp_panic` was written into a non-executable RW segment and a panic in a `pd`-bearing program
  exited 139 with no output; after the fix it prints `panic: assertion failed`, exit 1, byte-identical
  to C. And `Gen::call` refused every gated callee while the C backend emits no check when the
  checked-argument set is empty, so `path_absolute` (which calls the gated `fs.cwd`) was refused for a
  check that does not exist; escalated as a decision, closed with a narrow predicate and the
  `caps_gated_path.kp` pin.

**C. Closures and function values, item 6** (`eab7916`).

- before: 178 of 202, 24 refused, asm 288 of 336, C 336. after: 191 of 205, 14 refused, asm 302 of 339,
  valgrind 205 `c` / 191 `asm` over 396 runs and 361 fixtures. The closure row went 10 to 0 and all 13
  fixtures print the C backend's bytes (review `205d8e11` reproduced 13 of 13). Lanes `4acdcc79`,
  `205d8e11`.
- The two fixtures the slice wrote for itself (`closure_two_captures`, `closure_env_wide_ret`) catch the
  wrong-slot class the spec's trap list described and nothing reached: a `+4` on the read path alone
  moved their output in the lane's own mutation probe.
- The spec asked for one layout key per signature; the landing uses one key for all of them, because the
  three-pointer shape cannot depend on the signature (lane disagreement 1, measured equal on all
  thirteen).

**D. The C capability check's double evaluation** (`81e68e7`), and the false claim corrected around it
(`7cb41ec`, `513527e`).

- before: a resource argument was rendered twice, the check first, so `SIDE` printed twice and a helper
  whose first value is in grant and whose second is not had `/etc/passwd` read while the control that
  always returns it was denied. after: one evaluation, the call reads the value the check admitted, the
  control still denied; C suite 339 to 340. Lane `4435a235`, review `b5915e5f`.
- Caught while writing the capability-check specification: its trap list read `codegen_c.rs::cap_checked_call`
  re-rendering `self.expr_value(&args[i])` for the check, and the lane measured it by hand. Filed as
  `BUG-Kf7Rn` by lane `7ed276f2`.
- The fix's review built every `.kp` under `tests/fixtures` and `tests/projects` at both commits (447
  targets) and found the commit message's "byte-identical" claim false: six differ, five only by the
  `String.as_str()` hoist the fix introduces. The claim was corrected by an empty commit carrying the
  measurement (`7cb41ec`) and a dated blockquote (`513527e`), not by rewriting history (lane `25f7607e`).

**E. Where the asm backend's per-call cost goes** (`92d65f3` for `plan/mk7-call-overhead.md`, `7af207e`
for §13's annex).

- Per call, with the loop cancelled by nesting: a flat one-argument call is 1.22 to 1.31 ns in asm
  against 1.47 to 1.61 at `-O0`; `fib` at n=28..34 is 2.58 against 1.55. Hand-edited `kelp_fib` in the
  emitted `.s`: 46.4 ms to 42.0 (alignment fixup removed) to 30.0 (marshalling) to 24.7 (counter hoist),
  against `-O0` 27.0 and `-O2` 23.4. Ranked: argument marshalling 62% of the 19.4 ms, body
  stack-machine traffic 27%, the per-call alignment fixup 23%. Lane `58d5eb96`.
- The annex now carries the four-workload table at `4752ebb` with one estimator, the concurrency caveat
  on `float`, and the two readings of the gate: whole artifact 0.827 met, per workload unmet on `fib`
  at 1.723.

**F. S1 clock and entropy** (`5958794`) **and S2 the `io` descriptor family** (`4752ebb`).

- ratchet 191 to 192 to 194 built of 205, 14 to 13 to 11 refused; asm 303 of 340 to 304 to 307 of 340;
  C 340. The gate artifact built for the first time at `5958794`. Lane `725488ba`.

**G. The extern-gating specification** (`a1b07bb`, `plan/mk7-extern-gating.md`).

- no code. It measured the bypass (a module with a narrowed grant reads `/etc/hostname` through
  `fs.read_raw` on both backends, and a module with no grant at all still compiles), three further
  naming routes (a function value, a receiver method, a `pd` initializer), the blast radius (97 externs
  in 12 modules, 113 stdlib call sites all same-module, 37 cross-module sites in the two fixtures) and
  three candidate rules. Lane `68e9e400`; the primitive route was filed as `BUG-wQ9tL` by lane
  `cb5553a8`.

**H. S4 the `fs` path family** (`0053bc2`).

- ratchet 194 to 196 built of 207, 11 refused; asm 307 of 340 to 309 of 342; C 342. Lane `cb5553a8`,
  review `1b2a1f75`.
- The defect worth the section: `IoWriteBytesRaw` and `IoWriteTextRaw` read the outgoing argument area
  while a by-value `Bytes`/`String` parameter travels as a borrow in a register, so
  `io.stdout().write(Bytes.from_str("hello\n"))` printed nothing and returned 0 where C printed
  `hello\n6`. No fixture reached the two bodies (`git grep` at `4752ebb` finds none), so the
  differential sweep had been silent about them. Caught by the lane writing `io_write_bodies.kp` and
  comparing captures by hand; the review reproduced the pre-fix output (`0\n0\nstr\n4` against
  `bytes\n6\ntext\n5\nstr\n4`) and confirmed no earlier fixture reached it. Fixed and pinned in the
  same commit.
- The lane's own brief was wrong twice: its S4 extern list is not the specification's S4 list, and it
  named `fs_errno` and `fs_symlink_kind` as S4-reachable when both move to the capability check instead.

**I. The capability check, item 7 step 1** (`e65ce61`, `9c89b70`).

- ratchet 196 to 200 built of 208, 11 to 8 refused; asm 309 of 342 to 319 of 344; C 344; valgrind 408
  runs over 364 fixtures. Four fixtures, six projects and ten asm tests moved. Lane `edd00dbd`,
  refreshes `fbbe7474`.
- The lane had to close a second gap the brief did not name: a call whose wide return value is
  discarded was refused, and two of the five target fixtures do exactly that, so `discard_expr` landed
  in the same commit with the `discard_wide_return.kp` pin. The spec's "step 1 unblocks nothing" is
  false by measurement, and the brief said so first.

**J. The plan and the next-slice brief refreshed at `9c89b70`** (`805b263`, `5660f44`, `6cc7bb6`,
`074bf57`).

- docs: 200 of 208, 8 refused, asm 319 of 344, valgrind 408 runs, and the six remaining extern bodies,
  with the extern-body row's built half split correctly and the older blockquotes named as the source
  of the stale numbers. Lane `fbbe7474`.

**K. The four capability bypass routes** (`dadd309`, `5366fff`, `e40add4`, `734131b`, and `2593e3e`,
which corrected the S2 comment and the architecture sentence).

- ratchet unchanged at 208 built of 208 with 8 refused, all four inside the heap and gated rows; C
  suite 344 to 357; asm 319 of 344 to 332 of 357 with the same 25 failure names. Lane `de4ea8aa`,
  review `647b6faa`.
- route 1 (`dadd309`): a gated module's `extern fun` primitives, and its receiver methods with them,
  join the gate table. Before: `kelpc check` exit 0 and both backends printed `6` for `/etc/hostname`
  with no `caps(...)` anywhere. After: `'std.fs.read_raw' needs the 'fs' capability, which module
  'util' was not granted`; with a narrow grant, both backends deny at run time with the C message.
- route 2 (`5366fff`): a function value naming a gated item goes through `check_capability`. Before:
  the ungranted name built and read `6`, and the primitive spelling also died in `ld` under asm.
  After: refused by name on both backends.
- route 3 (`e40add4`): a receiver method of a gated module. Before:
  `process.Child(pid = 1, ...).kill(0)` ran the syscall with no grant (the `EPERM` panic proves it ran).
  After: `'std.process.Child#kill' needs the 'process' capability`.
- route 4 (`734131b`): a `pd` initializer's check. Before: C emitted none
  (`kelp_pd__util__leak = kelp_std__fs__exists(...)`) and asm emitted one with the **entry module's**
  allowlist (`lea rdx, [rip+kelp_caps_main_fs]`), so a narrow grant admitted `/etc/passwd` on both.
  After: both deny with the `pd`'s own module and allowlist. Caught by the specification lane's route 4
  and confirmed against the emitted `.s` at both commits by the review.
- The review also found the shape the four commits do not cover, and it is the fifth route: a module
  that holds the domain hands the value to a module that holds nothing, and `main` calls it (its `p37`
  ran on both backends and printed `6`). That is `BUG-k2Qv7` widened, and slice P closes it.

**L. The wiki's coverage refresh at `2593e3e`** (wiki commit `cf5fb37` on `unreleased`, pushed).

- the counts and refusal examples on `compiler/backends.md`, `compiler/tests.md` and
  `tooling/kelpc-cli.md`, the `asm` artifact note in the CLI page, and two getting-started pages whose
  quoted refusals no longer refused. Lane `3254539f`. It found the page claim that `--backend=asm`
  refuses a gated function value while the C backend builds it: measurement confirms that for the
  wrapper spelling only, which is what slice P closes.

**M. The `std/net` surface and its three checks** (`65c1271`, `3390ee2`).

- ratchet 200 to 204 built of 208, 8 to 4 refused; asm 332 of 357 to 341 of 359; C 359; valgrind 412
  runs. `net_loopback`, `net_poll`, `net_resolve`, `net_timeout` and the three `net` projects move;
  `http_redirect_denied` does not, because its first refusal is `std.process.arg_count_raw`. Lane
  `383fa2ce`.
- The review (`c6d40dce`) found a P1 defect in the new bodies: after `call socket@PLT` the descriptor is
  stored with a 32-bit `mov r14d, eax`, which zero-extends `-1`, while the failure test is 64-bit, so a
  failed `socket` hands `-1` back as a descriptor where C panics with the errno. Filed as `BUG-nQ7Rt`.
  The same review re-derived the escaping fix's neutrality over 242 programs at zero differences, found
  two residual gaps (`BUG-vX4Kd`, `BUG-zR8Wm`), and confirmed the argument-position read for
  `host`/`port`, the `freeaddrinfo` on every path and the errno capture are correct.

**N. S7, the fallible `fs` pair** (`7dc08d7`).

- ratchet 204 to 205 built of 208, 4 to 3 refused; asm 341 of 359 on the committed tree (the shared
  tree read 358 because another lane was deleting a test then); valgrind 413 runs over 364 fixtures.
  `fs_fallible` builds and pins the first lowered body whose failure is a returned value rather than a
  panic, including the `mut i32` errno out-parameter and the short-read path that frees the `Bytes` it
  had allocated. Lane `9acd48aa`.
- S9 was started and taken back out, because the work is a calling-convention mechanism and not the
  nine bodies: `std.process.spawn_raw` takes seven integer arguments and SysV puts the seventh on the
  stack, which this backend cannot do. The unverified work in progress is kept outside the repo at
  `/tmp/s9-stack-args-wip.patch` (723 lines), explicitly a starting point rather than an authority.

**O. The two closing fixes, and the sweep's own counts** (`8be6caa`, `fccb578`, `926e33f`).

- `8be6caa`: the C denial's `at`, module and domain were interpolated raw into a C string literal, so a
  source directory holding a backslash had the byte eaten (the two backends denied with different
  bytes) and one holding a quote broke the build (`cc: expected ')'`) while asm built and denied
  correctly. All three go through `diag::c_string_body` now, the escaper `#line` already used, and the
  new `tests/deny_message_escaping.rs` (2 tests) derives its shape from the directory name and asserts
  that both backends build and print identical denials. Caught by the differential verdict fuzzing lane
  (`049c6f67`), which compared 3908 `fs` cases and found the backslash case, then the quote's build
  failure; the fix's own review (`c6d40dce`) re-derived neutrality over 242 programs at zero differences.
- `fccb578`: a gated item cannot be a function value, granted or not. Before: under a narrow grant,
  `val f: fun(&str).R: Bytes = fs.read` built on both backends and `f("/etc/hostname")` read the file
  with no check at all, while the primitive spelling was refused only by asm and only for want of a
  symbol. After: refused at inference, on both backends, for both spellings, with the message naming the
  item and its domain. Its `tests/cap_function_value.rs` (4 tests) covers the four ways a value leaves
  its naming site plus the ungranted case, keeps an ungated function usable as a value, and keeps a
  function literal over a gated call checked at run time. The project `caps_raw_value_granted` went with
  the test it displaced, so `compile_and_run` is 358 and the project count 61.
- `926e33f`: the sweep's own recorded note and module doc said `207/196/11` and "builds 127 fixtures
  twice and runs 37 pairs"; the tip measures `208/205/3` and takes about a minute. Documentation only,
  and it is also where the three moves the note records are written down.

### 14.3 The defects the session found and closed, and how each was caught

1. **A capability check that read a different path than it used** (the C double evaluation). Caught by
   the specification lane reading `codegen_c.rs::cap_checked_call` for its trap list, then by hand:
   `SIDE` printed twice, and a helper whose first value is in grant and second is not had the file read
   while the control was denied. Fixed `81e68e7`, pinned by `caps_double_eval` with a byte-exact stderr.
2. **A `pd` initializer's check loading the entry module's allowlist** in asm, and no check at all in
   C. Caught by the extern-gating specification lane enumerating naming sites, and confirmed against the
   emitted `.s` at both commits. Fixed `734131b`.
3. **io write bodies that wrote zero bytes, with no fixture reaching them.** Caught by writing the
   first fixture that calls them and comparing captures by hand (`0\n0\nstr\n4` against
   `bytes\n6\ntext\n5\nstr\n4`). Fixed and pinned in `0053bc2`.
4. **Four capability bypass routes, plus the function-value route.** Caught by the specification lane's
   route enumeration (1 to 4) and by the fix's review (the cross-module hand-off, which ran on both
   backends). Fixed `dadd309`, `5366fff`, `e40add4`, `734131b`, `fccb578`.
5. **A C deny message that mangled a backslash and broke the build on a quote.** Caught by differential
   fuzzing of the denial bytes over 3908 `fs` cases in a directory whose name held the byte. Fixed
   `8be6caa`.
6. **A panic written into a non-executable segment**: the passdown `__dso_handle` emitter never
   restored `.text`. Caught by the S0 lane's own probe (`assert(false)` under asm exited 139 with no
   output). Fixed in `8d86d8e`.
7. **A false refusal**: `path_absolute` was refused for the gate because `Gen::call` refused every
   gated callee while the C backend emits no check for a call that names no resource. Caught by the S0
   lane's differential run, escalated as a decision, closed with a narrow predicate and a fixture pin.
8. **A latent refusal for a discarded wide return**, which two of the capability-check fixtures hit.
   Caught by that lane's own acceptance run; closed with `discard_expr` in `e65ce61`.
9. **The net descriptor sign-extension** (`mov r14d, eax` against a 64-bit `js`), which returns `-1` as
   a descriptor where C panics. Caught by the net slice's review. Filed `BUG-nQ7Rt`, not fixed.
10. **A commit message's byte-identical claim** that six of 447 targets falsify. Caught by that fix's
    review, which built every target. Corrected by `7cb41ec` and `513527e`.
11. **Documents that lagged the code**: the wiki's backend counts (four refreshes this session, the
    last `c00026c`), the sweep's own recorded note (`926e33f`), `plan/roadmap.md`'s MK7 row, and this
    file's §0 (re-measured three times). Caught by the lane that owns each file when it re-measured,
    which is the rule that produced this section.

### 14.4 Where this file's own plans and briefs were wrong

Recorded rather than tidied away, because the plan has now been wrong in the same direction many times
and every one of these was caught by measuring:

- §13's annex named the wrong second prerequisite for the gate artifact (the `now_mono_ns_raw` extern
  and the scalar `mut` borrow). The scalar borrow had landed, both kernels built, and the second thing
  it named was not a blocker at all: `speed.kp`'s only gated call names no resource, so the capability
  check never refused it.
- §0's capability row said 7 fixtures; measurement said 0 of 25 refusals named the check, with 10 of the
  201 C-built fixtures importing a gated module.
- `plan/mk7-capability-check.md` predicted that step 1 would make nothing that builds today build.
  Measurement moved 4 fixtures, 6 projects and 10 asm tests.
- `plan/mk7-closures.md` §5.0 asked for one synthesized closure layout per signature; the landing uses
  one key, and the three-pointer shape cannot depend on the signature, so the collect pass it wanted is
  unnecessary.
- `plan/mk7-extern-bodies.md` §3's S2 acceptance predicted `fs_fallible`'s next refusal would become
  `std.fs.write_text_raw`; measurement says `std.fs.write_raw`, and the specification's own ordering
  rule agrees with the measurement.
- The S4 brief's thirteen externs are not the specification's S4 thirteen (the brief's set is S4's
  read, write, write_text, exists, is_file, is_dir and remove plus S6's mkdir, mkdir_all, list, symlink,
  read_link and is_symlink), and the brief's starting count (`11 refused of 208, 197 built`) measured
  205/194/11 at `4752ebb`.
- The capability check's brief named five fixtures as refused only on the check; `caps_gated_path` and
  `fs_errno` had a second refusal behind it (the discarded wide return), and `caps_link_denied` was
  missing from the list.
- `plan/mk7-extern-gating.md`'s recommendation applies the domain half only at a function value, which
  leaves the cross-module route open; the review that found it also found the wiki sentence and the
  `plan/architecture.md` sentence that limit the exposure to the granting module. That review's own
  report is the correction, and the compiler now refuses the value outright.
- The S0 commit message said the asm suite went 286 to 288; the true pre-slice number is 285, and the
  fix's message claimed byte-identical generated C where six of 447 targets differ.
- This section's own predecessors: §0's current-tense numbers were the float-aggregate slice's twice,
  which is what the dated blockquotes are for, and every earlier blockquote is now marked as the record
  of its own commit rather than of the tip.
- `plan/mk7-stage2-encoder.md` pinned its line numbers to an in-flight working tree (`sha256
  ca476256…`, 12339 lines) rather than to a commit; the correction names `c3a8759` as the then-HEAD and
  adds the tip's numbers.

### 14.5 The measured state at `926e33f`

- `cargo test --release --test backend_coverage -- --ignored --nocapture`: `asm backend refusal
  ranking (3 fixtures refused of 208 the C backend builds, 205 built)`; heap 3, five rows 0; no
  re-record note, so `ASM_BUILT_BASELINE`'s 205 entries are the 205 the backend builds.
- The three refusals, by hand: `process_spawn` reads ``the `extern fun std.process.arg_count_raw` ``,
  `list_datac_clone_sort` reads ``the method `List.clone` on a `List` ``, and
  `borrow_chain_through_a_borrow` reads `a borrow of anything but a named local (a scalar, a `datac` or
  a heap type), a `datac` temporary, or a field of one`.
- `cargo test --release --test compile_and_run`: 358 of 358 under `--backend=c`. With
  `KELPC_BACKEND=asm`: 341 of 358, the 17 being the construct refusals plus
  `a_build_with_no_output_path_lands_in_build`, which asserts on the generated C.
- `cargo test --release --test memory_gate -- --ignored --nocapture`: 413 valgrind runs over 364
  fixtures; `c: 208 binaries checked, 0 finding(s)`; `asm: 205 binaries checked, 3 refused, 0
  finding(s)`; the 156 fixtures the compiler itself rejects skipped.
- `cargo test --release --test deny_message_escaping`: 2 of 2. `cargo test --release --test
  cap_function_value`: 4 of 4.
- `bench/speed/speed.kp`: builds under both backends and prints `collatz 131434424`, `float 57735026`,
  `VALUE 78539816.062926`, `mix 542344` and `fib 5702887` in every column, all equal to
  `bench/speed/results.txt`.

What these numbers do and do not prove: the fixture and suite counts are deterministic and reproducible;
the gate cells are not (both of this round's sweeps ran at load 12 to 21, and every cell sits nearer 1
than the `9c89b70` table's); the refusal ranking's rows are a `println!` heuristic over first refusal
messages, so a fixture moves rows when a message's wording changes; and the two runtime corpora prove
agreement between two implementations by one author, not that the rules they encode are right.

### 14.6 Open items

1. **S9 and the stack-passed register-class argument mechanism** (`std.process`, one fixture:
   `process_spawn`). This is the last extern family and the last refusal with a body behind it. The
   diagnosis is in lane `9acd48aa`'s report: `spawn_raw` takes seven integer arguments (`dir`,
   `program`, an `args: List<String>` lowered to a shared borrow, and four `mut i32` out-pointers), and
   SysV puts the seventh on the stack, which `codegen_asm.rs` refuses with `a call to
   'std.process.spawn_raw' with more than six integer argument registers yet`. The change is all in
   `codegen_asm.rs`: generalize `memory_arg_area` into one stack-area rule that carries `MEMORY`-class
   values and an argument whose integer register file has run out, whole and without advancing that
   file; an area-bound argument becomes `ArgSlot::Stack(off)` and is staged into a frame slot and copied
   into the outgoing area instead of pushed, scalars included; a new `ParamSlot::IntStack` (and `AggMem`
   for an area-carried aggregate) reads `[rbp+16+offset]` at the parameter's own width; `emit_trampoline`
   copies area-carried parameters rather than shifting them; then the nine bodies (`arg_count_raw`,
   `arg_at_raw`, `spawn_raw`, `reap_raw`, `kill_raw`, `exists_raw`, `executable_raw`, `is_dir_raw` as
   routines; `exit_raw` and `getpid_raw` as `Extern::Libc`) and the `kelp_argc`/`kelp_argv` globals with
   `main`'s prologue spill. A work in progress implementing all of it built `process_spawn.kp` and then
   panicked at `io.close` (EBADF) and hung, so nothing is verified; the patch is at
   `/tmp/s9-stack-args-wip.patch` (723 lines) and is a starting point, not an authority. It has to land
   as its own commit with the whole suite, the ranking and the valgrind gate as the gate, because the
   shared-area change touches every call site and every function prologue.
2. **`list_datac_clone_sort`** (one fixture): `List.clone`, `sort` and `slice` over a `datac`/`enum`
   element, which this backend still gates on the element-only predicates in `types.rs` where the C
   backend emits `kelp_clone_<T>`/`kelp_cmp_<T>` per user type. It is the asm half of `c3a8759`, and the
   frontend and the C backend already carry the surface.
3. **`borrow_chain_through_a_borrow`** (one fixture): a borrow of a field of a borrow; the refusal is at
   `codegen_asm.rs:9704` (the line the next-slice brief also names).
4. **The `fib` marshalling slice, and the two further slices `plan/mk7-call-overhead.md` orders**: S1
   marshalling (the per-argument push and pop, which is 62% of the gap; predicted 1.72 to about 1.11;
   correctness-sensitive, because the push and pop order is what makes a nested call safe; catch: the
   differential sweep, the ranking, the memory gate and a fixture with `f(g(x), h(y))`), then S2 body
   peepholes (`add rax, 1`, `cmp r15, 2`; about 0.91), and S3, the alignment fixup only when the pending
   depth is odd (1.722 to 1.555 alone, purely mechanical, the safest to bank first; catch: an odd-depth
   call fixture).
5. **The loopback test that fails intermittently**: `the_host_port_denial_names_the_host_as_written_and_its_own_port`
   in `compile_and_run.rs`. Reproduced in this section's own runs (a full C run read 358 of 359 and a
   full asm run 340 of 358 with it in the failure list; both re-ran green, and it passes in isolation).
   Not shown to be asm-specific.
6. **The three follow-up bugs filed this session**: `BUG-nQ7Rt` (the asm `net_connect_raw` and
   `net_listen_raw` return a zero-extended `-1` descriptor when `socket` fails, where C panics; fix is
   `movsxd r14, eax` or a 32-bit test), `BUG-vX4Kd` (`diag::c_string_body` escapes `\` and `"` only, so
   a control byte in a source path breaks the generated C for both the `#line` directive and the
   denial's `at`; asm is unaffected), and `BUG-zR8Wm` (`types::c_ident` leaves `"` and `\` alone, so a
   module name carrying one produces an invalid allowlist symbol and neither backend builds; the entry
   module's id is its file stem, so `kelpc build 'we"ird.kp'` reaches it).
7. **The harness race where two concurrent `cargo test` runs share a temp path**: the suite's scratch
   directory is `CARGO_TARGET_TMPDIR/kelpc_test_<backend>_<name>`, so a second suite running at the same
   time can hit `ExecutableFileBusy` (`ETXTBSY`) at `compile_and_run.rs:1401`. Caught by the net slice's
   review, which saw a spurious 340 of 358 and then 341 of 358 for the same tree. A per-run suffix on
   that path is the fix.
8. **Noted and not filed**: `net.recv(socket, 0)` blocks forever in both backends (Linux `recv(2)` with
   length 0 on a blocking socket blocks; `recv_into` short-circuits and returns 0 itself), and the
   capability check's path resolution recurses once per segment, so a path with about a thousand
   one-character segments dies with SIGSEGV identically on both backends.

### 14.7 How to resume

In the order the tip's own measurements put the work:

1. The stack-argument ABI and S9 (`std.process`): the last extern family, one fixture, and the only
   refusal left that hides anything (item 1 above). Nothing else in the refusal ranking is a body.
2. The `List` derived copy and order (`list_datac_clone_sort`): one fixture, and the surface already
   exists in the frontend and the C backend.
3. The borrow of a field chain (`borrow_chain_through_a_borrow`): one fixture.
4. `plan/mk7-call-overhead.md`'s S3 first (the alignment fixup only when the pending depth is odd), then
   S1 (marshalling) and S2 (body peepholes), which is what moves `fib` under 1.0 and closes §5's gate by
   the per-workload reading the plan uses.
5. An idle-machine gate run with one estimator, which §13 has owed since it was written, as the gate for
   step 4 and as the first asm column that is a measurement rather than an indication.
6. `BUG-nQ7Rt` and the harness race are small and independent of the above; `BUG-vX4Kd` and `BUG-zR8Wm`
   are one function each in `diag.rs` and `types.rs`.

The wiki pages this session moved are `compiler/backends.md`, `compiler/tests.md`,
`tooling/kelpc-cli.md` and `language/capabilities.md`, on branch `unreleased` (this record's own
refresh, commits `c00026c` and `04228b2`, pushed), on top of the refreshes in slices A, K and L; the
second commit is the sweep's own result, because `getting-started/install.md` and
`getting-started/troubleshooting.md` still quoted `std.net.connect_raw` as the refusal example after the
`std/net` slice had lowered it. `main` still
describes the released 26.09.0 and is unchanged. `ISSUES.md` lost `BUG-wQ9tL`, `BUG-Kf7Rn`,
`BUG-k2Qv7` and `BUG-m3Wv9` this session and gained the three follow-ups above, and
`kelp/CHANGELOG.pending.md` carries one bullet per user-visible change, uncommitted as its ignore rule
requires.
