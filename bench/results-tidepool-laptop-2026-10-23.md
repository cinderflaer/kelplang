# Tidepool GC benchmark, laptop, 2026-10-23 (single run)

**This is one run of `bench/gc/run.sh` on one machine (the laptop). It is not a median of repeated runs.** The script was run with `RUNS=1`: one discarded warm-up per row, then one recorded run. There is no spread and no noise estimate. Treat each number as one sample, and do not compare it with the committed `bench/RESULTS.md`, which was measured on another machine.

- Script: `bench/gc/run.sh` (`RUNS=1`, run through `nix-shell shell.nix`, as a build queue job at priority 1, exclusive, on the laptop only).
- Job: buildq `1fbd4a76`, started 2026-10-23 12:03:43 (laptop clock as logged).
- CPU: Intel(R) Core(TM) i5-10310U CPU @ 1.70GHz (laptop), 15.3 GB RAM, Linux 7.2.10 x86_64.
- kelp commit: `a7bb7585` (origin/main at the time of the run); kelpc `26.10.6`.
- Build: `kelpc build -O2`, default LLVM backend, `mode = "hosted"`, managed rows add `heap_max` (MiB).
- Run wrapper: `( ulimit -v 2097152; ulimit -t 60; timeout 120 ... )`.
- The script wrote its own `RESULTS.md` stamp as `2026-10-23 10:05 UTC` (12:05 CEST, the time the run finished).

## Result table

Times are the single recorded run (wall seconds, user and sys seconds, peak RSS in KB, collections, pause totals in ms). The `managed / unmanaged wall` ratio is the script's value, computed from the single run times.

| bench | variant | status | wall s | user s | RSS KB | collections | pause total ms | pause max ms |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| binary-trees | managed | ok | 2.920 | 2.880 | 121232 | 39 | 908.692 | 35.073 |
| binary-trees | unmanaged | ok | 2.890 | 2.840 | 121180 | 39 | 881.710 | 34.471 |
| live-set | managed | ok | 0.920 | 0.880 | 128900 | 8 | 258.086 | 40.673 |
| live-set | unmanaged | ok | 0.920 | 0.880 | 129016 | 8 | 243.491 | 39.123 |
| strings | managed | ok | 1.950 | 1.860 | 236592 | 20 | 166.654 | 21.856 |
| strings | unmanaged | ok | 1.950 | 1.860 | 214684 | 21 | 171.470 | 20.866 |
| mixed-pool | managed | ok | 6.370 | 22.870 | 15788 | 137 | 216.908 | 8.920 |
| mixed-pool | unmanaged | ok | 6.390 | 22.930 | 15624 | 137 | 198.205 | 2.099 |
| heap-pressure | managed-16 | FAILED-EXIT | 0.030 | 0.020 | 23128 | 3 | 13.461 | 6.410 |
| heap-pressure | managed-64 | ok | 1.460 | 1.440 | 32484 | 61 | 332.868 | 6.375 |
| heap-pressure | managed-256 | ok | 1.460 | 1.440 | 32484 | 61 | 330.243 | 6.433 |
| heap-pressure | unmanaged | ok | 1.450 | 1.430 | 32380 | 61 | 330.409 | 6.537 |

Managed / unmanaged wall ratio (single run each):

| bench | managed / unmanaged |
| --- | ---: |
| binary-trees | 1.01x |
| live-set | 1.00x |
| strings | 1.00x |
| mixed-pool | 1.00x |
| heap-pressure (managed-64 vs unmanaged) | 1.01x |
| heap-pressure (managed-256 vs unmanaged) | 1.01x |
| heap-pressure (managed-16) | n/a, FAILED-EXIT |

## Failed rows

- **heap-pressure / managed-16: FAILED-EXIT (exit 1), not measured.** The program aborts with the designed panic: `panic: out of shared-pool memory: heap_max is 16 MiB, 11 MiB was live at the last clearing, and 0 MiB is charged to live blocks`. The runtime's advice line is `keep less data live, or raise heap_max in coral.toml's [target] section`. This is a bench size problem, not a script bug: the program's live set (about 11 MiB) is larger than the 16 MiB heap cap. The bench sizes were not changed to make the row pass. Its 0.03 s time is the time to the panic and is not a benchmark number.

## Other notes

- All other rows exited 0, printed the same output in the warm-up and the recorded run, and linked the collector (`kelp_gc_*` symbols present, so no NO-GC status).
- mixed-pool shows user time (about 22.9 s) well above wall time (6.4 s) in both variants, so this program uses more than one thread. Recorded as observed; not analysed.
- The build log has one compiler warning (`thread_detach` is never read in `kelpc/src/gc_descr.rs`). It does not affect the run.

## Raw output (`bench/gc/raw.csv` from the run)

```csv
bench,variant,run,wall_s,user_s,sys_s,rss_kb,exit,gc_linked,collections,pause_total_ms,pause_max_ms,output
binary-trees,managed,1,2.92,2.88,0.03,121232,0,yes,39,908.692,35.073,binary-trees 18 68332206
binary-trees,unmanaged,1,2.89,2.84,0.04,121180,0,yes,39,881.710,34.471,binary-trees 18 68332206
live-set,managed,1,0.92,0.88,0.03,128900,0,yes,8,258.086,40.673,live-set 2000000 2000037000000
live-set,unmanaged,1,0.92,0.88,0.03,129016,0,yes,8,243.491,39.123,live-set 2000000 2000037000000
strings,managed,1,1.95,1.86,0.08,236592,0,yes,20,166.654,21.856,strings 1000000 4520446541216952489
strings,unmanaged,1,1.95,1.86,0.07,214684,0,yes,21,171.470,20.866,strings 1000000 4520446541216952489
mixed-pool,managed,1,6.37,22.87,0.09,15788,0,yes,137,216.908,8.920,mixed-pool 41814680 0
mixed-pool,unmanaged,1,6.39,22.93,0.08,15624,0,yes,137,198.205,2.099,mixed-pool 41814680 0
heap-pressure,managed-16,1,0.03,0.02,0.01,23128,1,yes,3,13.461,6.410,
heap-pressure,managed-64,1,1.46,1.44,0.01,32484,0,yes,61,332.868,6.375,heap-pressure 500000 49812400250000
heap-pressure,managed-256,1,1.46,1.44,0.01,32484,0,yes,61,330.243,6.433,heap-pressure 500000 49812400250000
heap-pressure,unmanaged,1,1.45,1.43,0.01,32380,0,yes,61,330.409,6.537,heap-pressure 500000 49812400250000
```

## Script output (`bench/gc/results.csv`)

```csv
bench,variant,status,gc_linked,runs,wall_s_median,wall_s_min,wall_s_max,wall_spread_pct,user_s_median,sys_s_median,rss_kb_median,collections_median,pause_total_ms_median,pause_max_ms_median,output
binary-trees,managed,ok,yes,1,2.920,2.920,2.920,0.0,2.880,0.030,121232,39,908.692,35.073,binary-trees 18 68332206
binary-trees,unmanaged,ok,yes,1,2.890,2.890,2.890,0.0,2.840,0.040,121180,39,881.710,34.471,binary-trees 18 68332206
live-set,managed,ok,yes,1,0.920,0.920,0.920,0.0,0.880,0.030,128900,8,258.086,40.673,live-set 2000000 2000037000000
live-set,unmanaged,ok,yes,1,0.920,0.920,0.920,0.0,0.880,0.030,129016,8,243.491,39.123,live-set 2000000 2000037000000
strings,managed,ok,yes,1,1.950,1.950,1.950,0.0,1.860,0.080,236592,20,166.654,21.856,strings 1000000 4520446541216952489
strings,unmanaged,ok,yes,1,1.950,1.950,1.950,0.0,1.860,0.070,214684,21,171.470,20.866,strings 1000000 4520446541216952489
mixed-pool,managed,ok,yes,1,6.370,6.370,6.370,0.0,22.870,0.090,15788,137,216.908,8.920,mixed-pool 41814680 0
mixed-pool,unmanaged,ok,yes,1,6.390,6.390,6.390,0.0,22.930,0.080,15624,137,198.205,2.099,mixed-pool 41814680 0
heap-pressure,managed-16,FAILED-EXIT,yes,1,0.030,0.030,0.030,0.0,0.020,0.010,23128,3,13.461,6.410,
heap-pressure,managed-64,ok,yes,1,1.460,1.460,1.460,0.0,1.440,0.010,32484,61,332.868,6.375,heap-pressure 500000 49812400250000
heap-pressure,managed-256,ok,yes,1,1.460,1.460,1.460,0.0,1.440,0.010,32484,61,330.243,6.433,heap-pressure 500000 49812400250000
heap-pressure,unmanaged,ok,yes,1,1.450,1.450,1.450,0.0,1.430,0.010,32380,61,330.409,6.537,heap-pressure 500000 49812400250000
```
