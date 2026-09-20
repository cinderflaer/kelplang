# The asm backend: retired plans

Everything in this directory belongs to the **retired hand-written backend track**
(the `asm` backend): x86-64 assembly through `as` and `ld`, then an instruction
encoder and ELF writer, then freestanding, then a hand-written optimizer. It is
kept whole because the measurements and the inventories are still useful history.
Nothing here is a plan of record, nothing here is maintained, and no file here is
re-measured when the compiler moves.

The plan of record for the compiler's next backend is `../mk7-llvm-backend.md`.
The `asm` backend's source (`kelpc/src/codegen_asm.rs` and its tests) is removed
once the LLVM backend passes the corpus, in one commit.

## Why the track was retired

On 2026-09-20 the backend decision changed: `kelpc` will emit **LLVM IR** and let
LLVM optimize and generate code, the way `rustc` does. The hand-written track's
remaining work was mostly an optimizer (inlining, CSE, LICM, strength reduction)
and it was the multi-year part; LLVM also supplies DWARF, sanitizers and multiple
targets, and the language surface does not change. The retired track's headline
payoff, a ~10x compile-time win from dropping the C toolchain, is given up with it:
LLVM *is* the C toolchain's backend, so compile time stays where it is. The C
backend stays as the differential oracle and the escape hatch.

## Files

| File | What it was | What is still worth reading |
| --- | --- | --- |
| `plan.md` | The track's working plan and live state: the backend seam, the tier order, the gates, the risk register, the per-fixture refusal ranking, the status tables. | The refusal ranking and the gate definitions: that is the coverage method the LLVM backend reuses. |
| `default-switch.md` | The sequence for moving `Backend::DEFAULT` off `C`. | §1.5 survives whole and is now a rule in `../mk7-llvm-backend.md` §2: `--backend=c` and `KELPC_BACKEND=c` stay forever, and the C backend is never deleted. |
| `call-overhead.md` | The per-workload and per-call measurement: `bench/speed` at `-O0`, at `-O2`, and under `asm`, plus where `fib`'s per-call cost goes. | The `-O0` versus `-O2` columns are the size of the optimizer work that moved to LLVM. This is the measurement the decision rests on. |
| `encoder-inventory.md` | What a hand-written encoder and ELF writer owe: the observed mnemonic set, the relocation and section surface. | The inventory itself: the honest measure of how much machine-code surface a hand-written backend owes. |
| `next-slice.md` | The reconnaissance brief for the next slice of the track: per-fixture refusal categories and the differential-sweep method. | The per-fixture refusals and the sweep method. |
| `closures.md` | How the backend would have lowered function values and closures: fat pointers, an environment struct, the SysV ABI details. | The frontend-side inventory of where a function value can appear, which is frontend truth and not backend-specific. |
| `capability-check.md` | The per-call-site capability check as assembly: which half was C, which half was already Kelp in `std/rt`, and the exact deny-path text. | All of it: the check keeps its shape under LLVM, so this is still the brief for that work. |
| `extern-bodies.md` | The syscall surface the backend had no routine for, by extern name. | The extern inventory, including which of the 97 declarations each stdlib module contributes. |
| `extern-gating.md` | The raw-primitive and gate-table analysis behind `ISSUES.md`'s `BUG-wQ9tL`, with the three naming sites. | The gate-table analysis. The bug itself is a `codegen_asm.rs` defect: it closes when that file is removed, not before. |

## Reading note

The `MK7 §n` numbering inside these files is the retired track's own plan
numbering (`plan.md` §0 to §13). It is not the roadmap's MK7, which now names the
LLVM backend. The `plan/mk7-*.md` path references inside them were rewritten to
`plan/asm-backend/*.md` when the files moved; nothing else in them was edited, so
their line numbers and quoted measurements are still the ones that were measured.
