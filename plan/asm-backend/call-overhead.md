# MK7: where `fib`'s call cost goes, measured

> **ARCHIVED 2026-09-20. Superseded, not deleted.** This is the measurement the LLVM decision rests
> on: the `-O0` versus `-O2` columns below are the size of the optimizer a hand-written backend
> would have had to build, and §1's table is the reason that work moved to LLVM
> (`plan/mk7-llvm-backend.md` §0).

This file is a measurement, not a plan proposal. It answers one question the
gate raised: `bench/speed/speed.kp` builds under `--backend=asm` at HEAD
`4752ebb`, and every workload except `fib` is at or below the `-O0` column while
`fib` is 1.72 times it. Section 3's cost model predicts the opposite. This file
re-measures the gate, then measures where `fib`'s per-call cost actually goes,
one hypothesis at a time, on the emitted assembly rather than by argument.

Date: 2026-09-20. Base commit: `4752ebb`. Machine: `AMD Ryzen 5 7600` (6C/12T,
30 GiB), Linux `6.18.52`, `clang 21.1.8` as `cc`, `cargo 1.95.0`.

Every number below was taken in this session. The `kelpc` under test was built
`--release` from a detached worktree of `4752ebb`, so nothing here is copied from
the earlier lane's table.

## 0. Method

- `kelpc build --backend=c bench/speed/speed.kp -o speed_c` writes `speed_c.c`;
  the two C columns are `cc -O0 -fwrapv -lm` and `cc -O2 -fwrapv -lm` over that
  file. `--backend=asm` builds `speed_asm` through `as` and `ld`.
- One estimator for every column: 9 rounds interleaved round-robin (one run of
  each column per round), round 1 discarded, median of the 8 that remain, spread
  is min to max of those 8. This is the estimator §13's annex asks for.
- The self-reported number is the program's own `time.now_mono_ns()` around each
  workload (`work_ns`). The external number is `date +%s%N` around the whole
  process, so no column times itself.
- All four checksums and the float `VALUE` line are asserted to match
  `bench/speed/results.txt` in every run of every column.

## 1. The gate re-measured

Load average during the sweep: 2.54 to 2.74, from another lane compiling.

| workload | `-O0` | `-O2` | asm | asm/`-O0` |
| --- | --- | --- | --- | --- |
| collatz | 446.5 (444.4-450.6) | 81.5 (80.7-82.8) | 430.5 (427.9-433.0) | 0.964 |
| float | 254.8 (253.0-257.9) | 100.4 (98.7-103.0) | 151.5 (145.2-234.3) | 0.594 |
| mix | 203.1 (199.9-204.2) | 165.2 (164.2-168.4) | 139.0 (138.0-140.8) | 0.684 |
| fib | 26.7 (26.6-28.2) | 23.3 (23.0-24.1) | 46.1 (45.6-47.4) | 1.723 |

Whole-process external wall clock, median of the same 8 rounds: `-O0` 933.0 ms,
`-O2` 374.1 ms, asm 771.9 ms. The asm column's whole-process wall is **0.827** of
`-O0`'s.

Checksums in every column and every round: `collatz 131434424`, `float 57735026`,
`mix 542344`, `fib 18454929` calls and `5702887`. The float `VALUE` line is
`78539816.062926` in every column, so the `float` column's work is the same work.

A second sweep, 15 rounds kept, pinned to one core with `taskset -c 3`, load
2.13 to 3.13, reproduced the reading: collatz 0.969, float 0.696, mix 0.707, fib
1.727, whole-process wall 0.878.

**Two readings of the gate.** §5's definition of done says `bench/speed` "runs
all four workloads with matching checksums and is no worse than the `-O0`
column". Read as one criterion over the artifact, the whole-process wall is
0.827 and the gate is **met**. Read per workload, the gate is **unmet on `fib`**
at 1.723, and met on the other three (0.964, 0.594, 0.684). The plan's own text
already uses the per-workload reading: §5's status blockquote reports the ratio
per workload and calls `fib` above the column at 1.70, and §5's float blockquote
does the same. So the per-workload reading is the operative one, and on it the
gate fails on exactly one workload.

**This is not noise.** This session's three sweeps put `fib` at 1.70, 1.73 and
1.72 of `-O0`, and the lane that landed the artifact put it at 1.710; the two
earlier rounds gave 1.718 and 2.605 against the same `-O0` column. Every other
workload beats `-O0` in every sweep.

## 2. The cost of a call, measured directly

Two microbenchmarks, written outside the repo in `/tmp`, built with the same two
compilers, timed with the external clock. Both use the nesting trick so that the
loop body is byte-identical and only the call count changes: the difference
divided by `N` is the per-call constant with the loop cancelled exactly.

**Flat non-inlined one-argument call** (`fun one(x: i64).R: i64 = x + 1`, loops
with one, two and three nested calls, `N = 100000000`, three runs, pinned to one
core):

| column | ns per call |
| --- | --- |
| `-O0` | 1.47 to 1.61 |
| asm | 1.22 to 1.31 |

The flat call is **cheaper** in the asm backend than at `-O0`. `-O2` inlines
`one` and the numbers collapse to the loop.

**Flat two-argument call** (`fun add(a: i64, b: i64).R: i64 = a + b`, same
shape): `-O0` 1.39 to 1.48 ns per call, asm 1.50 to 1.81 ns per call. The
marginal cost of the second argument is about 0.38 ns in the asm backend and
nothing at `-O0`. That is the first sign that the cost is in the *marshalling*
of each register argument, not in the call itself.

**The recursive shape is not special.** `fib(n, calls)` at `n = 28` through
`n = 34` (the exact `speed.kp` body), per call:

| column | ns per call, n=28 | n=34 |
| --- | --- | --- |
| `-O0` | 1.567 | 1.554 |
| asm | 2.595 | 2.575 |

`fib`'s per-call cost is flat across the depths, so the recursion is the same
constant per call repeated 18454929 times, not a shape-specific penalty.

### Instruction count on `fib`'s inner path

`objdump` of the two binaries. Static instruction count of `kelp_fib`:

| column | instructions in `kelp_fib` |
| --- | --- |
| `-O0` | 33 |
| `-O2` | 32 |
| asm | 71 |

The asm version executes all 71 but one unreachable `jmp` on its two-call path;
`-O0` executes 29 on the same path. The asm backend issues about 2.4 times the
instructions per `fib` call but runs the call in about 1.7 times the time, so its
instructions are individually cheaper (better IPC) than `-O0`'s frame traffic.

The asm call site for `fib(n-1, calls)` is 11 instructions before the alignment
fixup and the `call`:

```
mov rax, r15        # n
push rax
mov rax, 1
mov rcx, rax
pop rax
sub rax, rcx        # n - 1
push rax
mov rax, r14        # calls
push rax
pop rsi
pop rdi
```

`-O0` does the same call in 5: `mov -0x10(rbp),rdi; sub $1,rdi;
mov -0x18(rbp),rsi; call; mov rax,-0x20(rbp)`. A direct lowering is 2 before the
`call`: `lea rdi, [r15-1]; mov rsi, r14`.

## 3. Ranked causes

Each cause was measured by hand-editing the backend's own `kelp_fib` in the
emitted `.s` (outside the repo, never committed) and rebuilding with the same
`as` and `ld`. Each variant changes `fib` only; `collatz`, `float` and `mix` are
untouched, and `collatz` held at 435.0 to 437.9 ms through every variant, which
is the check that the edit is isolated.

| variant | change to `kelp_fib` | fib median ms | fib/`-O0` | ns/call |
| --- | --- | --- | --- | --- |
| asm baseline | none | 46.4 | 1.722 | 2.517 |
| v1 | drop the 3-instruction alignment fixup at both call sites | 42.0 | 1.555 | 2.273 |
| v2 | v1 plus `lea rdi,[r15-1]; mov rsi,r14` instead of the push/pop marshalling | 30.0 | 1.112 | 1.626 |
| v3 | v2 plus the `*calls` update without the stack machine | 24.7 | 0.914 | 1.336 |
| v5 | v3 plus `cmp r15, 2; jge` for the leaf test | 24.6 | 0.910 | 1.330 |
| `-O0` | reference | 27.0 | 1.000 | 1.462 |
| `-O2` | reference | 23.4 | 0.871 | 1.270 |

Instruction counts of the same variants: baseline 71, v1 65, v2 47, v3 39,
v5 30. `-O0` is 33. The gap between baseline and `-O0` is 19.4 ms; the measured
contributions are:

1. **Argument marshalling through the stack: 12.0 ms of the 19.4 (62%).** Every
   register argument is pushed and popped, and the argument's own arithmetic runs
   through the stack machine as well, so one two-argument call site costs 11
   instructions where 2 would do. Evidence: v2, and the flat two-argument
   microbenchmark's 0.38 ns marginal cost per argument against `-O0`'s zero.
2. **Body codegen through the stack machine: 5.3 ms (27%).** The `*calls`
   increment, the `n < 2` comparison and the `n - 1`/`n - 2` all materialize into
   `rax` and spill, so each costs 5 to 12 instructions where a register form
   costs 2 or 3. Evidence: v3.
3. **The per-call alignment fixup: 4.4 ms (23%).** Every call site emits
   `mov QWORD PTR [rbp-24], rsp; and rsp, -16` before the `call` and
   `mov rsp, QWORD PTR [rbp-24]` after it, three instructions unconditionally,
   even at the `fib` call sites where the pending-operand depth is provably zero
   and `rsp` is already 16-aligned at the `call`. Evidence: v1.

The three overlap, so they sum to more than the gap; the ordering by absolute
effect is robust and the fixed build (v5) is below `-O0`.

### Ruled out by measurement

- **Register allocation across the call (hypothesis: every operand reloaded from
  its frame slot).** False for `fib`. `n` stays in `r15` and `calls` in `r14`
  across both recursive calls; the prologue loads them once (`mov r15,rdi;
  mov r14,rsi`) and the call path reads `r15`/`r14` directly afterwards with no
  reload. `allocate_registers` hands integer locals the five callee-saved
  registers with no call restriction; only a float local whose live range
  overlaps a clobber span is forced to a frame slot, and `fib` has no float. The
  problem is not where a live value is kept, it is how each argument is moved.
- **A thunk or the PLT.** False. The emitted call is
  `call 40141f <kelp_fib>`, a direct relative call to the same symbol. Only the
  extern and libc calls (`printf`, `malloc`, `memcpy`, `atexit`) go through
  `@PLT`.
- **A stack-passed argument.** `fib` passes two integer arguments, both in
  `rdi`/`rsi`; nothing is written to an outgoing argument area. The 11-instruction
  call site is marshalling overhead, not spilling.
- **The prologue and epilogue.** Small, not the cause. `fib`'s prologue is 7
  instructions and its epilogue 5 (frame 48 bytes, two pool registers saved);
  `-O0`'s are 5 and 4 (frame 32, none saved). Three instructions of the
  42-instruction excess. Not isolated further (see below).
- **The recursive shape.** False. Per-call cost is flat from `n = 28` to
  `n = 34` in both columns.

## 4. Proposed slices, ordered

The three causes are not independent, so the split below is the one the
cumulative measurement gives, and the per-slice numbers are order-dependent: the
chain measured was baseline 1.722 to v1 1.555 (alignment fixup removed) to v2
1.112 (marshalling) to v3 0.914 (body) to v5 0.910 (leaf test). The order below
reorders the first two by the size of the effect, and the effect of S1 is the one
measured with the alignment fixup already removed. If the order matters for a
later re-measurement, measure each slice on the baseline build, not on the
previous slice's build.

**S1. Marshal arguments without the push/pop round trip.** Evaluate each
argument once, stage it so a nested call cannot clobber an argument register
already loaded, then load the argument registers once, with no push and pop per
argument. Expected: `fib` 1.72 to about 1.11 (v2 measured 1.112 against 1.722),
no measurable effect on `collatz` (0.964) or `mix` (0.684); `float` may improve
slightly where a call's argument is arithmetic. **Risk: correctness.** The
push-then-pop-in-reverse order is what makes a nested call safe today, because an
argument's own expression can contain a call that overwrites an argument register.
A rewrite that loads `rdi` before evaluating a later argument that calls would be
wrong. **What catches it:** the 248-fixture differential suite
(`KELPC_BACKEND=asm cargo test --test compile_and_run` against the C backend),
`cargo test --test backend_coverage -- --ignored`, `cargo test --test memory_gate
-- --ignored`, plus a new fixture with a call in argument position (for example
`f(g(x), h(y))`) so the nested case is exercised on purpose rather than by luck.

**S2. Stop materializing a single binary operation into `rax` and spilling it.**
The counter update, the comparison and the argument arithmetic in `fib` all use
the shape the backend emits for every integer expression: build `rax`, push it,
build the other side, pop, operate. A peephole that recognizes a binary operation
whose right operand is a constant or an already-addressable place and emits one
instruction (`add rax, 1`, `cmp r15, 2`, `lea rdi, [r15-1]`) is a codegen-quality
change. Expected: a further step to about 0.91 (v3 measured 0.914 after S1, v5
0.910 with the leaf test as well). **Risk: low but not zero.** Each peephole is a
rewrite that can compute the wrong value. **What catches it:** the fixture suite
and the differential sweep, which is how the existing integer codegen is checked.

**S3. Emit the alignment fixup only when it is needed.** The pending-operand
depth at a call site is statically known, because the stack machine pushes one
eight-byte operand per pending subexpression and every push and pop is emitted by
this module. Track the depth modulo 16 and emit the round-down only when it is
odd. Expected: a further small step, measured alone as 1.722 to 1.555 on the
baseline. **Risk: purely mechanical, the lowest of the three.** A miscounted
depth means the fixup is skipped where it was needed, so `rsp` is not 16-aligned
at the `call`, which is a crash inside a callee that uses an aligned SSE access
rather than a wrong number. **What catches it:** the fixtures, plus a unit test
on the depth tracker at a call site with an odd number of pending operands (for
example a call as the right operand of `+`). This is the one slice of the three
with no new correctness argument to write, so it is the safe one to bank first if
a low-risk win is wanted before S1.

The fully fixed `fib` (v5, 30 instructions) is 0.91 of `-O0`, so the three slices
together turn the one failing workload into one that beats `-O0`, with no
optimizer and no inlining involved.

## 5. What could not be determined

- **The prologue and epilogue's own share, isolated.** The difference is three
  instructions (12 against 9), too small to see in a 19 ms gap, and no variant
  isolated it. The command that would settle it: a variant whose frame is 16
  bytes instead of 48 and whose callee-saved savings are kept, so the frame size
  changes and nothing else, measured against the baseline in the same interleaved
  sweep.
- **The uncontended `float` ratio.** The asm `float` column is bimodal on this
  machine: 145.2 to 234.3 in the gate sweep, and 137.3 to 502.8 in an earlier one
  in this session, while `-O0` and `-O2` stayed inside about 2% of their medians
  through the same window. So 0.594 is a median that includes contended runs, and
  the uncontended number is nearer 0.55. The command that would settle it: a
  sweep on an idle machine, or a longer sweep reporting the two modes separately.
- **Whether removing the alignment fixup is safe at every call site, not only
  `fib`'s.** I hand-edited only `fib`, whose pending depth is even at both call
  sites. A general fix needs the depth tracker, and the settling artifact is a
  fixture with a call at an odd pending depth.
- **A cycle-level attribution.** No `perf` run was made; the instruction counts
  and the wall-clock deltas are the evidence, and they agree, but the split
  between front-end, store-forwarding stalls and port pressure is not measured.
