#!/usr/bin/env python3
"""M1 (design section 13): ns per allocation, paired blocks and a one-sided bound.

Usage (as a build queue job, from run-m1.sh):

    python3 bench/gc-alloc/run-m1.py BASE_BIN S1_BIN

BASE_BIN and S1_BIN are two builds of bench/gc-alloc (gc-alloc-bench), one per
runtime. Each arm is one process per run, `nogc` mode. The cell16 (24 B) row is
the gate measure; the other two rows are checked for equal work and reported.

Method:
  1. Preflight: this job is the only one in the build queue's running set. Waits
     up to 20 minutes, then refuses. Holds every free buildq slot lock visible on
     this host. buildq.py opens the slot locks as LOCK.HOST SLOT, where LOCK is
     BUILDQ_LOCK or buildq's default; the script globs the same names. A remote
     host (the job runs in a mirror) may not see the runner's locks; then the
     exclusive submit is the guard that keeps other jobs off the host.
  2. Each run is pinned to one core (taskset), under ulimit -v 2 GiB and a CPU
     limit of 300 s.
  3. Quiet gate before each run: 1-minute load below 1.0, no other process above
     10 percent CPU (sampled over one second), MemAvailable above 3 GiB. On failure
     it waits 30 s and retries, up to 20 minutes, then aborts.
  4. Nine interleaved blocks: odd blocks run base then s1, even blocks s1 then base.
  5. Work check: every run must print the same allocation count, the same checksum
     and the same values_ok and distinct flags as the first run, for every row, in
     both arms. A mismatch means the binaries did different work: not measured.
  6. Verdict. For each block i, r_i = s1_i / base_i from the same block. With the
     mean m and the sample standard deviation s of the nine ratios, the one-sided
     95 percent upper bound on the mean is m + t * s / sqrt(9), t = 1.8595
     (Student t, 8 degrees of freedom). Pass when the bound is at most 1.05, which
     is the step 1 line: s1 at most 5 percent slower than base. Per-arm medians
     and spreads are printed as diagnostics only; they do not decide.

Exit codes: 0 pass (bound at most 1.05); 1 fail (measured, bound above 1.05);
2 usage error; 3 not measured (a run failed, the output did not parse, or the
work check failed); 4 preflight refused; 5 quiet gate aborted.
"""
import fcntl
import glob
import json
import math
import os
import re
import resource
import statistics
import subprocess
import sys
import time

BLOCKS = 9
T_ONE_SIDED_95_DF8 = 1.8595  # Student t, one-sided 95 percent, BLOCKS - 1 = 8 degrees of freedom
GATE_RATIO = 1.05  # the step 1 line: the upper bound on s1 over base at most 1.05
RUN_TIMEOUT = 300
QUIET_WAIT_S = 20 * 60
RETRY_S = 30
STATE = os.path.expanduser(os.environ.get("BUILDQ_STATE", "~/.local/state/buildq"))
LOCK = os.environ.get("BUILDQ_LOCK") or os.path.expanduser("~/Projects/kelp-work/.build.lock")  # buildq.py's default
LABELS = ["cell16 (24 B)", "array8x8 (72 B)", "obj200 (208 B)"]
GATE_LABEL = "cell16 (24 B)"
ROW_RE = re.compile(r"^(cell16 \(24 B\)|array8x8 \(72 B\)|obj200 \(208 B\))\s+(\d+) allocs\s+[\d.]+ M allocs/s\s+([\d.]+) ns/alloc", re.M)
CHECK_RE = re.compile(r"^check (cell16 \(24 B\)|array8x8 \(72 B\)|obj200 \(208 B\))\s+n=(\d+) values_ok=([01]) distinct=([01]) checksum=(0x[0-9a-f]+)", re.M)
CORE = 2 if (os.cpu_count() or 1) > 2 else 0

held_fds = []


class RunError(Exception):
    """A run produced no usable output, or the two arms did different work."""


def log(msg):
    print(time.strftime("%H:%M:%S ") + msg, flush=True)


def preflight():
    deadline = time.time() + QUIET_WAIT_S
    while True:
        running = glob.glob(os.path.join(STATE, "running", "*.json"))
        if len(running) <= 1:
            break
        if time.time() > deadline:
            log(f"preflight: {len(running)} jobs running after 20 minutes; refusing")
            sys.exit(4)
        log(f"preflight: {len(running)} jobs running, waiting")
        time.sleep(RETRY_S)
    # Hold every free slot lock buildq would take on this host (LOCK.HOST SLOT).
    # Names that do not exist are not created: buildq owns them.
    locks = sorted(glob.glob(glob.escape(LOCK) + ".*"))
    for path in locks:
        try:
            fd = os.open(path, os.O_RDWR)
        except OSError as e:
            log(f"preflight: cannot open {path}: {e}")
            continue
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            held_fds.append(fd)
            log(f"preflight: holding {path}")
        except BlockingIOError:
            os.close(fd)
            log(f"preflight: {path} is held by another process (the running job)")
    if not locks:
        log(f"preflight: no buildq slot locks ({LOCK}.*) on this host; exclusive scheduling is the guard")
    log(f"preflight: host={os.uname().nodename} cpus={os.cpu_count()} core={CORE}")


def meminfo_avail_kb():
    with open("/proc/meminfo") as f:
        for line in f:
            if line.startswith("MemAvailable:"):
                return int(line.split()[1])
    return 0


def _cpu_ticks():
    ticks = {}
    for d in os.listdir("/proc"):
        if not d.isdigit():
            continue
        try:
            with open(f"/proc/{d}/stat") as f:
                fields = f.read().rsplit(")", 1)[1].split()
            ticks[int(d)] = int(fields[11]) + int(fields[12])  # utime + stime
        except (OSError, IndexError, ValueError):
            pass
    return ticks


def busiest_other_cpu():
    """Highest instantaneous %CPU (over one second) of any process other than this runner."""
    skip = {os.getpid(), os.getppid()}
    hz = os.sysconf("SC_CLK_TCK")
    before = _cpu_ticks()
    time.sleep(1.0)
    after = _cpu_ticks()
    best = 0.0
    for pid, t in after.items():
        if pid in skip or pid not in before:
            continue
        best = max(best, 100.0 * (t - before[pid]) / hz)
    return best


def quiet_gate():
    """True when the box is quiet; waits up to 20 minutes."""
    deadline = time.time() + QUIET_WAIT_S
    while True:
        load1 = float(open("/proc/loadavg").read().split()[0])
        top = busiest_other_cpu()
        avail_gib = meminfo_avail_kb() / (1 << 20)
        if load1 < 1.0 and top < 10.0 and avail_gib > 3.0:
            return load1, top, avail_gib
        if time.time() > deadline:
            return None
        log(f"quiet gate failed: load1={load1:.2f} top_cpu={top:.1f} avail_gib={avail_gib:.2f}; waiting")
        time.sleep(RETRY_S)


def limit_child():
    resource.setrlimit(resource.RLIMIT_AS, (2097152 * 1024, 2097152 * 1024))
    resource.setrlimit(resource.RLIMIT_CPU, (RUN_TIMEOUT, RUN_TIMEOUT))


def parse_output(text):
    """Rows and checks of one run. Raises RunError unless all three rows and checks are present."""
    rows = {m.group(1): {"allocs": int(m.group(2)), "ns": float(m.group(3))} for m in ROW_RE.finditer(text)}
    checks = {m.group(1): {"n": int(m.group(2)), "values_ok": m.group(3) == "1",
                           "distinct": m.group(4) == "1", "checksum": m.group(5)}
              for m in CHECK_RE.finditer(text)}
    if set(rows) != set(LABELS) or set(checks) != set(LABELS):
        raise RunError(f"output lacks rows or checks: rows={sorted(rows)} checks={sorted(checks)}")
    for label in LABELS:
        if rows[label]["allocs"] != checks[label]["n"]:
            raise RunError(f"{label}: row allocs {rows[label]['allocs']} differ from check n {checks[label]['n']}")
    return {"rows": rows, "checks": checks}


def signature(out):
    """What must be equal between runs and arms for the work to count as the same."""
    return {label: (out["rows"][label]["allocs"], out["checks"][label]["n"], out["checks"][label]["checksum"],
                    out["checks"][label]["values_ok"], out["checks"][label]["distinct"]) for label in LABELS}


def run_once(binary):
    cmd = ["taskset", "-c", str(CORE), binary, "nogc"]
    proc = subprocess.run(cmd, capture_output=True, text=True, timeout=RUN_TIMEOUT + 60, preexec_fn=limit_child)
    if proc.returncode != 0:
        raise RunError(f"run failed rc={proc.returncode}: {proc.stdout[-300:]} {proc.stderr[-300:]}")
    return parse_output(proc.stdout)


def timed_block(arm_bin, arm, block_no, runs, reference):
    gate = quiet_gate()
    if gate is None:
        log("quiet gate aborted; stopping")
        sys.exit(5)
    load_before = gate[0]
    out = run_once(arm_bin)
    load_after = float(open("/proc/loadavg").read().split()[0])
    sig = signature(out)
    if reference["sig"] is None:
        reference["sig"] = sig
    elif sig != reference["sig"]:
        raise RunError(f"block {block_no} {arm}: work differs from the first run: {sig} vs {reference['sig']}")
    runs[arm].append({label: out["rows"][label]["ns"] for label in LABELS})
    log(f"block {block_no} {arm}: {out['rows'][GATE_LABEL]['ns']:.2f} ns/alloc "
        f"(load {load_before:.2f} -> {load_after:.2f}); checks ok={all(c['values_ok'] and c['distinct'] for c in out['checks'].values())}")


def measure(base_bin, s1_bin):
    runs = {"base": [], "s1": []}
    reference = {"sig": None}
    for block_no in range(1, BLOCKS + 1):
        order = [("base", base_bin), ("s1", s1_bin)] if block_no % 2 == 1 else [("s1", s1_bin), ("base", base_bin)]
        for arm, binary in order:
            timed_block(binary, arm, block_no, runs, reference)
    return runs


def upper_bound(ratios):
    """Mean, sample standard deviation and the one-sided 95 percent upper bound on the mean of the ratios."""
    if len(ratios) != BLOCKS:
        raise ValueError(f"need {BLOCKS} paired ratios, got {len(ratios)}")
    mean = statistics.fmean(ratios)
    sd = statistics.stdev(ratios)
    return mean, sd, mean + T_ONE_SIDED_95_DF8 * sd / math.sqrt(len(ratios))


def median_and_spread(values):
    med = statistics.median(values)
    return med, (max(values) - min(values)) / med


def main():
    if len(sys.argv) != 3:
        print(__doc__)
        sys.exit(2)
    base_bin, s1_bin = sys.argv[1], sys.argv[2]
    for b in (base_bin, s1_bin):
        if not os.access(b, os.X_OK):
            print(f"not executable: {b}")
            sys.exit(2)
    preflight()
    log(f"{BLOCKS} interleaved blocks, one run per arm per block")
    try:
        runs = measure(base_bin, s1_bin)
    except (RunError, subprocess.TimeoutExpired, OSError) as e:
        print(f"M1 NOT MEASURED: {e}")
        sys.exit(3)

    gate_base = [r[GATE_LABEL] for r in runs["base"]]
    gate_s1 = [r[GATE_LABEL] for r in runs["s1"]]
    ratios = [s / b for s, b in zip(gate_s1, gate_base)]
    mean, sd, bound = upper_bound(ratios)
    base_med, base_spread = median_and_spread(gate_base)
    s1_med, s1_spread = median_and_spread(gate_s1)
    other = {label: upper_bound([s / b for s, b in zip([r[label] for r in runs["s1"]], [r[label] for r in runs["base"]])])[0]
             for label in LABELS if label != GATE_LABEL}
    summary = {
        "base_ns": gate_base, "s1_ns": gate_s1, "ratios": ratios,
        "base_median": base_med, "base_spread_diag": base_spread,
        "s1_median": s1_med, "s1_spread_diag": s1_spread,
        "ratio_mean": mean, "ratio_sd": sd, "ratio_upper_bound_95_one_sided": bound,
        "gate_ratio": GATE_RATIO, "other_rows_mean_ratio": other,
    }
    print("== M1 result (cell16, nogc, ns per allocation)")
    print(json.dumps(summary, indent=1))
    delta_pct = (mean - 1.0) * 100.0
    verdict = "PASS" if bound <= GATE_RATIO else "FAIL"
    print(f"M1 ratio s1/base mean {mean:.4f} (sd {sd:.4f}, mean difference {delta_pct:+.2f}%), "
          f"one-sided 95% upper bound {bound:.4f} against {GATE_RATIO:.2f}: {verdict}")
    print(f"diagnostics: base median {base_med:.2f} ns (spread {base_spread * 100:.2f}%), "
          f"s1 median {s1_med:.2f} ns (spread {s1_spread * 100:.2f}%); spreads do not decide")
    sys.exit(0 if verdict == "PASS" else 1)


if __name__ == "__main__":
    main()
