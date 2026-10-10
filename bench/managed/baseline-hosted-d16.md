Recorded hosted baseline (WP4). Produced by bench/managed/gate.sh, BT_DEPTH=16, RUNS=7, WARMUP=1, OUT_DIR outside the repo.
Machine load: 1-min load average 0.85 to 0.98 during the runs (4 logical CPUs, another kelpc build running at about one core).
Managed side not available (WP2/WP3 not landed). Peak RSS is the GNU time maximum resident size of each run.


Hosted: /home/juniper/Projects/kelp-work/release-assets/kelpc-26.10.5-linux-x86_64. Managed: not available. binary-trees depth 16.
Gate G-A: managed median / hosted median <= 1.00 PASS, <= 1.10 FALLBACK (owner must accept), else FAIL.

| benchmark | variant | median s | min s | max s | spread % | peak RSS MiB | checksum | G-A | load 1 min (before -> after) |
| --- | --- | ---: | ---: | ---: | ---: | ---: | --- | --- | --- |
| binary-trees | hosted | 0.3600 | 0.3581 | 0.3730 | 4.1 | 18.3 | ok | baseline | 0.85 -> 0.86 |
| binary-trees | c | 0.3189 | 0.3171 | 0.3241 | 2.2 | 10.1 | ok | reference | 0.85 -> 0.87 |
| strings | hosted | 0.2101 | 0.2080 | 0.2119 | 1.9 | 63.1 | ok | baseline | 0.87 -> 0.87 |
| strings | c | 0.1098 | 0.1090 | 0.1131 | 3.7 | 47.7 | ok | reference | 0.87 -> 0.88 |
| hashmap | hosted | 0.3385 | 0.3292 | 0.3406 | 3.4 | 80.1 | ok | baseline | 0.88 -> 0.97 |
| hashmap | c | 0.1821 | 0.1770 | 0.1892 | 6.7 | 66.1 | ok | reference | 0.88 -> 0.97 |
| json-tokenizer | hosted | 0.0963 | 0.0949 | 0.1037 | 9.1 | 21.3 | ok | baseline | 0.97 -> 0.98 |
| json-tokenizer | c | 0.0765 | 0.0755 | 0.0797 | 5.5 | 21.2 | ok | reference | 0.97 -> 0.98 |
