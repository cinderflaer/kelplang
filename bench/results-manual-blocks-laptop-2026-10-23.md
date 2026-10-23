# Manual blocks (M1) allocation benchmark, laptop, 2026-10-23 (single run)

**This is one run of `bench/gc-alloc/run-m1.sh` on one machine (the laptop). It is one M1 measurement: nine interleaved blocks, each arm measured once per block, not repeated runs.** The verdict is the one-sided bound from those nine paired blocks. The per-arm medians and spreads are diagnostics only. Do not compare these numbers with the committed `bench/RESULTS.md`, which was measured on another machine.

- Script: `bench/gc-alloc/run-m1.sh` with `run-m1.py` (M1, design section 13). Base arm: kelp commit `bf655a60` (`kelpc/gc`, extracted with `git archive`). S1 arm: kelp commit `a7bb7585` (origin/main, the worktree's `kelpc/gc`).
- Job: buildq `125709d1`, priority 1, exclusive, laptop only. Started 2026-10-23 12:06:11, finished 12:11:38 (laptop clock as logged).
- CPU: Intel(R) Core(TM) i5-10310U CPU @ 1.70GHz (laptop), 8 logical CPUs, pinned to one core per run (`core=2`).
- The job waited through the quiet gate (1-minute load below 1.0, no other process above 10 percent CPU) before most runs. The gate retried 9 times and then let each run proceed; the gate-failed lines are in the log below.

## Result

| arm | median ns per allocation (cell16, 24 B, gate row) | spread (diagnostic) |
| --- | ---: | ---: |
| base (`bf655a60`) | 16.66 | 3.72% |
| s1 (`a7bb7585`) | 16.87 | 2.79% |

- Ratio s1 / base, per block: mean 1.0127, sd 0.0162, mean difference +1.27%.
- One-sided 95% upper bound on the mean ratio: 1.0227 against the gate of 1.05.
- **Verdict: PASS** (s1 is at most 5 percent slower than base, to the 95% bound). This is one run.

Other rows (reported, not gated; the script's own diagnostic means):

| row | mean ratio s1 / base |
| --- | ---: |
| array8x8 (72 B) | 1.025 |
| obj200 (208 B) | 1.063 |

The obj200 row mean is above the 1.05 gate value. The gate applies only to the cell16 row, so this is not a fail, but it is a single run and it is worth a repeat before it is read as a regression.

## Failed rows

None. All 18 timed runs (9 blocks, 2 arms) passed the work check (same allocation count, checksum and values on every run). No row failed.

## Raw output (buildq job log, verbatim)

```text
# 2026-10-23 12:06:11 gc-alloc m1 manual blocks in /home/juniper/Projects/kelp-work/wt-bench-run
buildq: running on laptop, paths mirrored under /home/lilly/mirror
== build /home/lilly/mirror/home-juniper/Projects/kelp-work/bench-m1-base-scratch/base/kelpc/gc
   Compiling kelp_gc v0.1.0 (/home/lilly/mirror/home-juniper/Projects/kelp-work/bench-m1-base-scratch/base/kelpc/gc)
    Finished `release` profile [optimized] target(s) in 0.56s
== build /home/lilly/mirror/home-juniper/Projects/kelp-work/wt-bench-run/kelpc/gc
   Compiling kelp_gc v0.1.0 (/home/lilly/mirror/home-juniper/Projects/kelp-work/wt-bench-run/kelpc/gc)
    Finished `release` profile [optimized] target(s) in 0.67s
== build /home/lilly/mirror/home-juniper/Projects/kelp-work/bench-m1-base-scratch/base/bench/gc-alloc
   Compiling gc-alloc-bench v0.1.0 (/home/lilly/mirror/home-juniper/Projects/kelp-work/bench-m1-base-scratch/base/bench/gc-alloc)
    Finished `release` profile [optimized] target(s) in 0.77s
== build /home/lilly/mirror/home-juniper/Projects/kelp-work/bench-m1-base-scratch/base/s1/bench/gc-alloc
   Compiling gc-alloc-bench v0.1.0 (/home/lilly/mirror/home-juniper/Projects/kelp-work/bench-m1-base-scratch/base/s1/bench/gc-alloc)
    Finished `release` profile [optimized] target(s) in 0.78s
kelp_gc_ symbols in /home/lilly/mirror/home-juniper/Projects/kelp-work/bench-m1-base-scratch/base/bench/gc-alloc/target/release/gc-alloc-bench: 5
kelp_gc_ symbols in /home/lilly/mirror/home-juniper/Projects/kelp-work/bench-m1-base-scratch/base/s1/bench/gc-alloc/target/release/gc-alloc-bench: 6
12:06:20 preflight: no buildq slot locks (/home/lilly/Projects/kelp-work/.build.lock.*) on this host; exclusive scheduling is the guard
12:06:20 preflight: host=nixos cpus=8 core=2
12:06:20 9 interleaved blocks, one run per arm per block
12:06:21 quiet gate failed: load1=1.33 top_cpu=4.0 avail_gib=10.87; waiting
12:06:53 block 1 base: 16.28 ns/alloc (load 0.80 -> 0.90); checks ok=True
12:06:55 block 1 s1: 16.90 ns/alloc (load 0.90 -> 0.90); checks ok=True
12:06:58 block 2 s1: 16.62 ns/alloc (load 0.90 -> 0.90); checks ok=True
12:06:59 quiet gate failed: load1=0.90 top_cpu=26.0 avail_gib=10.82; waiting
12:07:31 block 2 base: 16.74 ns/alloc (load 0.61 -> 0.61); checks ok=True
12:07:33 block 3 base: 16.66 ns/alloc (load 0.61 -> 0.64); checks ok=True
12:07:35 block 3 s1: 16.73 ns/alloc (load 0.64 -> 0.64); checks ok=True
12:07:37 block 4 s1: 16.55 ns/alloc (load 0.64 -> 0.64); checks ok=True
12:07:39 block 4 base: 16.56 ns/alloc (load 0.64 -> 0.59); checks ok=True
12:07:41 block 5 base: 16.73 ns/alloc (load 0.59 -> 0.59); checks ok=True
12:07:44 block 5 s1: 16.87 ns/alloc (load 0.59 -> 0.62); checks ok=True
12:07:46 block 6 s1: 16.91 ns/alloc (load 0.62 -> 0.62); checks ok=True
12:07:48 block 6 base: 16.73 ns/alloc (load 0.62 -> 0.65); checks ok=True
12:07:50 block 7 base: 16.90 ns/alloc (load 0.65 -> 0.65); checks ok=True
12:07:52 block 7 s1: 16.94 ns/alloc (load 0.65 -> 0.65); checks ok=True
12:07:54 block 8 s1: 16.66 ns/alloc (load 0.65 -> 0.60); checks ok=True
12:07:56 block 8 base: 16.33 ns/alloc (load 0.60 -> 0.60); checks ok=True
12:07:58 block 9 base: 16.40 ns/alloc (load 0.60 -> 0.63); checks ok=True
12:07:59 quiet gate failed: load1=0.63 top_cpu=21.0 avail_gib=10.82; waiting
12:08:30 quiet gate failed: load1=0.38 top_cpu=22.0 avail_gib=11.00; waiting
12:09:02 quiet gate failed: load1=0.31 top_cpu=11.0 avail_gib=10.99; waiting
12:09:33 quiet gate failed: load1=0.19 top_cpu=25.0 avail_gib=11.01; waiting
12:10:04 quiet gate failed: load1=0.32 top_cpu=27.0 avail_gib=10.99; waiting
12:10:35 quiet gate failed: load1=0.27 top_cpu=12.0 avail_gib=10.98; waiting
12:11:06 quiet gate failed: load1=0.16 top_cpu=28.0 avail_gib=10.99; waiting
12:11:38 block 9 s1: 17.02 ns/alloc (load 0.10 -> 0.17); checks ok=True
== M1 result (cell16, nogc, ns per allocation)
{
 "base_ns": [
  16.28,
  16.74,
  16.66,
  16.56,
  16.73,
  16.73,
  16.9,
  16.33,
  16.4
 ],
 "s1_ns": [
  16.9,
  16.62,
  16.73,
  16.55,
  16.87,
  16.91,
  16.94,
  16.66,
  17.02
 ],
 "ratios": [
  1.0380835380835378,
  0.9928315412186381,
  1.004201680672269,
  0.9993961352657006,
  1.0083682008368202,
  1.0107591153616258,
  1.0023668639053256,
  1.020208205756277,
  1.0378048780487805
 ],
 "base_median": 16.66,
 "base_spread_diag": 0.0372148859543816,
 "s1_median": 16.87,
 "s1_spread_diag": 0.027860106698280904,
 "ratio_mean": 1.0126689065721084,
 "ratio_sd": 0.01621811672668018,
 "ratio_upper_bound_95_one_sided": 1.0227214359231958,
 "gate_ratio": 1.05,
 "other_rows_mean_ratio": {
  "array8x8 (72 B)": 1.0250391984449616,
  "obj200 (208 B)": 1.062956330472066
 }
}
M1 ratio s1/base mean 1.0127 (sd 0.0162, mean difference +1.27%), one-sided 95% upper bound 1.0227 against 1.05: PASS
diagnostics: base median 16.66 ns (spread 3.72%), s1 median 16.87 ns (spread 2.79%); spreads do not decide
```
