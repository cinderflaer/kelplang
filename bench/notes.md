## Analysis (hand-written, from the runs recorded in this file)

Two full runs are recorded. The **first run** (kept verbatim at the bottom) was made before any
compiler or library change and is the baseline. The **second run** (the tables above) was made after
one library change (see "What was fixed"). Both used the same sources, the same machine and
`bench/run.sh` unchanged, 7 timed runs plus a discarded warm-up per cell.

### Reading the numbers honestly

- **Ratios are against C built with gcc -O2 and no bounds checks.** Kelp and Rust check bounds; the
  C programs do not. That favours C by construction. There is no clang on this machine, so there
  are no clang rows, and no `perf`, so causes below come from reading emitted C/IR, from objdump,
  or from small timed experiments, each said where used.
- **The runs are short** (0.08 s to 0.9 s) because the sizes are the ones the task fixed. The
  measurement wrapper adds about 7 ms to every wall time (7% of the fastest cells). A 5% difference
  between rows is inside what this host does to a repeated measurement: benchmarks the library
  change cannot touch moved between the two runs by up to 9% (`fib` kelp-llvm 0.154 s to 0.140 s,
  `sort` kelp-llvm 0.700 s to 0.661 s, `mandelbrot` 0.115 s both times). Within one run the 7
  samples of a cell agree to 1-3%.
- **The host was never idle.** The 1-minute load average was between 2.3 and 3.7 for the whole of
  both runs (other agents' work and the Minecraft server, none of it stopped or throttled). No cell's
  load changed by more than 1.0 across its block of runs, so nothing is flagged NOISY, but the
  absolute level means all numbers are for a shared 4-core machine.
- Peak RSS stayed small: the largest cell in either run is 104 MB (Rust `hashmap`), far below the
  1.5 GB bound, so no size was scaled down. Every row's checksum equals the C version's on all runs.
- The C `hashmap` is a hand-written open-addressing table in the benchmark file (C has no hash map);
  Kelp uses its builtin `Map<u64, u64>` and Rust its std `HashMap` (SipHash). `sort` is the same
  hand-written quicksort in all three languages; `sort-builtin` compares each language's own sort
  (Kelp `List.sort`, C `qsort` with a comparator callback, Rust `sort_unstable`).

### Where Kelp wins, ties and loses (second run, kelp-llvm, the default backend)

Ties or beats C (0.87-1.14): nbody, spectral-norm, mandelbrot, binary-trees, sieve, matmul. Slower
than C by 1.24-1.32: fib, fannkuch-redux, sort, json-tokenizer. Slower by more than 1.5: strings
(1.84), hashmap (1.88). Faster than C by a wide margin: sort-builtin (0.35), only because C's
`qsort` calls its comparator through a function pointer. Against Rust (kelp-llvm vs rust): Kelp is
faster in fib, binary-trees, matmul and hashmap, level in spectral-norm and mandelbrot, and slower
in nbody, fannkuch-redux, sieve (slightly), strings, sort, sort-builtin and json-tokenizer.

### Every ratio above 1.5, with its cause

| cell (second run) | ratio | cause |
| --- | ---: | --- |
| fib, rust | 1.51 | Not Kelp. Rust and Kelp both go through LLVM and both get ~1.2-1.5x on this loop-ified recursion that gcc compiles best (objdump: each binary keeps one recursive call and turns the other into a loop, so the difference is loop code generation). kelp-c is 1.01 because it is gcc. Not investigated further. |
| spectral-norm, kelp-c | 1.71 | Found and verified: the C oracle backend emits every Kelp function with external linkage, so `eval_a` stays an out-of-line call per matrix element under gcc -O2. Making it `static` in the emitted C gives 0.07 s, the same as the hand-written C; removing the bounds/`to_usize` checks changes nothing (0.13 s both). The default LLVM backend is at parity (0.98). Not fixed: the oracle is not the product backend and `static` is not sound under `--incremental` (cross-unit calls). |
| strings, kelp-llvm / kelp-c | 1.84 / 1.89 | Timed split of the 0.22 s (experiments, not in the table): building the 7.9 MB text 0.07 s (C about 0.03 s; `str.from_i64` plus `push_str` per piece), `str.split` into 1M owned `String`s +0.09 s, the byte hash loop +0.06 s. The hash loop uses `str.byte(p, i)`, which does a `memchr` over `i + 1` bytes per call; reading the same pieces through a `View` makes that +0.01 s and the whole program 0.17 s (1.4x). The benchmark keeps the `str.byte` spelling because it is the first one a user reaches for. |
| hashmap, kelp-llvm / kelp-c | 1.88 / 2.29 | `Map` is two arrays (a hash index and dense insertion-ordered entries, so a lookup is two dependent cache misses) and `Map.new()` grows by rehashing; the C table is one array sized up front. Measured: `Map.with_capacity(2n)` takes the program from 0.32 s to 0.24 s (1.4x C, experiment only); the remaining gap is the double indirection. kelp-c is slower than kelp-llvm here too, probably for the external-linkage reason under spectral-norm (not checked). |
| hashmap, rust | 2.66 | The default std hasher (SipHash-1-3) and a table grown from empty with `HashMap::new()`. Not measured separately. |
| sort, kelp-c | 1.51 | kelp-llvm is 1.32. Both pay for the per-access `List.get`/`set` bounds check and the `List` header (`len`, `data`) reloaded around stores to `data`; Rust pays the bounds checks too but is at 1.06, so the extra is Kelp's: reasoned from the emitted code, not profiled. gcc on the C oracle's emitted C does worse than LLVM on the same program for the external-linkage reason above. |

Ratios between 1.2 and 1.5 that are not in the table above: fib 1.24, fannkuch-redux 1.28, sort
1.32 and json-tokenizer 1.27 on kelp-llvm; matmul 1.28 and strings 1.25 on Rust. The Kelp ones are
the same `List.get`/`set` bounds checks that C does not have (fannkuch-redux, sort) and, for
json-tokenizer, the generation half (see below); none was profiled.

### What was fixed, with before and after

The benchmarks exposed two real defects in `std/str`, both fixed in one commit (`kelpc/std/str.kp`):

1. `str.split` was quadratic. It called `starts_with_at` and `copy_range`, each of which calls
   `s.view()` and so `strlen`s the whole subject once per position and once per piece. Splitting
   the 7.9 MB string of the `strings` benchmark did not finish in 300 s on either backend. It now
   takes each argument's length once and slices views.
2. `str.from_i64` / `from_u64` allocated a scratch `String` (and ran `strlen` on it) for every
   number, two allocations per call. They now collect the digits in a 20-byte stack array, so the
   result is the only allocation.

Before and after, median of 7 (seconds). `fib`, `mandelbrot` and `sort` are controls the change
cannot touch; they show the run-to-run drift of this host.

| benchmark / variant | before (first run) | after (second run) |
| --- | ---: | ---: |
| strings kelp-llvm | timeout at 300 s | 0.224 |
| strings kelp-c | timeout at 300 s | 0.230 |
| json-tokenizer kelp-llvm | 0.126 | 0.098 |
| json-tokenizer kelp-c | 0.126 | 0.110 |
| json-tokenizer c-gcc (control for host drift) | 0.079 | 0.077 |
| fib kelp-llvm (control) | 0.154 | 0.140 |
| mandelbrot kelp-llvm (control) | 0.115 | 0.114 |
| sort kelp-llvm (control) | 0.700 | 0.661 |

The json-tokenizer gain is the generation half (about a fifth of its time was `from_i64`); its tokenizing
half already matched C (about 0.047 s both). Output of both functions was checked against the
previous behaviour on 0, `i64::MIN`, `i64::MAX`, negatives and `u64::MAX`, and on several splits
(empty pieces, trailing separator, multi-byte separator, empty separator), on both backends.

### Findings not fixed

- `str.byte(s, i)` costs O(i): the bounds check is a `memchr` over `i + 1` bytes because a `str` has
  no stored length. A byte loop over a long `str` is therefore quadratic (a 100 KB tokenizer loop took
  0.10 s where the `View` version took under 5 ms). The `json-tokenizer` Kelp program reads through a
  `View` for that reason, and says so in its header comment; the idiomatic fix for users is
  `s.view()` once, outside the loop.
- `Map` growth without `Map.with_capacity` and the double indirection (above).
- The C oracle backend's external linkage (above).
- No `noalias`/TBAA information reaches LLVM for `List` accesses, which is the likely reason `sort`
  and `fannkuch-redux` are 1.3x C where Rust, with the same bounds checks, is at 1.0-1.06. This is a
  hypothesis from the emitted IR, not a measurement, and it is not a cheap or obviously sound change,
  so it was left alone.
