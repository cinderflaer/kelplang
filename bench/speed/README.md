# speed — working speed of the same four workloads, in four languages

    ./run.sh          # compiles, runs three trials of each, prints the table

## Method

Four CPU-bound workloads, the **same algorithm and the same op counts** in
Rust, Kelp, C++ and Python. Every program times its own workloads with a
monotonic clock and prints one line per workload:

    RESULT <workload> <ops> <work_ns> <checksum>

The harness compiles each program, runs it three times, keeps the *fastest*
measurement per workload, verifies that all four languages produced the same
checksum for each workload, and reports ns/op. Equal checksums are what make
the timings comparable: they prove the four programs did the same work.

## The workloads

| workload | ops | what it stresses |
| --- | --- | --- |
| `collatz` | 1,000,000 start values | branchy integer work, ~100 dependent steps each: division by 2, multiply-add, unpredictable branch |
| `float` | 100,000,000 steps | one FP divide and one FP add per step (a `1/(1+x²)` sum), plus a threshold count that consumes every value |
| `mix` | 50,000,000 iterations | a small integer function that clang inlines in all three compiled languages: dominated by two 64-bit modulos, i.e. hardware division |
| `fib` | 18,454,929 calls | naive exponential recursion — real call/return cost. The counter parameter is load-bearing: without a side effect LLVM rewrites the recursion into a loop (measured at 0.5 ns/op, i.e. clearly optimised away), which would make this workload a duplicate of `mix` |

## Caveats worth knowing

- **Kelp compiles to C** and shells out to `clang -O2`, so Kelp and C++ share a
  backend and a single C++ file's `-O2` flags. Kelp being level with C++ is the
  expected result, not a surprise — what the numbers can show is that Kelp's own
  front end adds nothing (no bounds checks, no hidden allocation, no slow ABI)
  on these workloads.
- **Rust is built with `rustc -O`** (no cargo, no LTO), C++ with
  `clang++ -O2`, Kelp with `kelpc build` (its whole pipeline plus the `cc` call).
  Python has no compile step: it parses and compiles at import, which is why
  its compile column says n/a and its startup column is ~13 ms.
- **`collatz` is measured per start value**, not per inner step, so its
  "80 ns/op" is ~100 steps' worth of work. The ns/op column is only
  comparable *within* a row.
- Checksums are integers on purpose. The float workload's `VALUE` is printed
  too and agrees across all four to six decimals, which is a stronger check
  than the integer count, but the harness asserts on the integers because FP
  summation order and FMA contraction can differ in the last bits.
- The compiled languages' measurements are ~0.1–0.4 s per workload; the
  numbers are stable to about 1% across trials on this machine.
