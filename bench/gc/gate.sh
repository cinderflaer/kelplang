#!/bin/sh
# Regression gate for bench/gc. Compares a new results.csv (from run.sh) with a stored baseline.
#
#   sh bench/gc/gate.sh [NEW=bench/gc/results.csv] [BASE=bench/gc/baseline.csv]
#
# Fails (exit 1) when any of these hold, for a row that is "ok" in the new file:
#   - its wall_s_median or rss_kb_median is more than MAX_REGRESS_PCT (default 5) above the baseline
#     median for the same bench and variant;
#   - its pause_max_ms_median is above MAX_PAUSE_MS (default 50), or it is a managed row with a
#     collector linked and no pause counters at all (missing counters are not a pass);
#   - a row in the baseline has no match in the new file, or the new status is not "ok"
#     (FAILED-EXIT, OUTPUT-MISMATCH, BUILD-FAILED, NO-GC) while the baseline row was "ok";
#   - a managed row in the new file is not "ok" at all (BUILD-FAILED, NO-GC, ...). A managed
#     row that cannot be measured is not a pass. ALLOW_UNMEASURED=1 turns that one rule off,
#     for a run on a branch where the managed mode is known not to build yet.
# A baseline row that is not "ok" (for example BUILD-FAILED) is reported and not compared.
# Exit 0 when nothing fails. Prints one line per row.
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
NEW=${1:-$HERE/results.csv}
BASE=${2:-$HERE/baseline.csv}
MAX_REGRESS_PCT=${MAX_REGRESS_PCT:-5}
MAX_PAUSE_MS=${MAX_PAUSE_MS:-50}
ALLOW_UNMEASURED=${ALLOW_UNMEASURED:-0}

[ -f "$NEW" ] || { echo "gate: no results file $NEW" >&2; exit 2; }
[ -f "$BASE" ] || { echo "gate: no baseline $BASE" >&2; exit 2; }

awk -F, -v maxr="$MAX_REGRESS_PCT" -v maxp="$MAX_PAUSE_MS" -v allowu="$ALLOW_UNMEASURED" '
# column layout (run.sh): 1 bench, 2 variant, 3 status, 4 gc_linked, 5 runs, 6 wall median,
# 7 wall min, 8 wall max, 9 spread, 10 user, 11 sys, 12 rss median, 13 collections median,
# 14 pause total ms median, 15 pause max ms median, 16 output
FNR == 1 { next }
FILENAME == ARGV[1] {
    k = $1 SUBSEP $2; nst[k] = $3; nlk[k] = $4; nw[k] = $6; nr[k] = $12; npm[k] = $15; seen_new[k] = 1; nlist[++nn] = k
    if ($2 ~ /^managed/ && $3 != "ok") {
        if (allowu == "1") printf "NOTE  %s/%s unmeasured (ALLOW_UNMEASURED=1): status %s\n", $1, $2, $3
        else { printf "FAIL  %s/%s unmeasured: status %s %s\n", $1, $2, $3, substr($16, 1, 120); unmeasured++ }
    }
    next
}
{
    k = $1 SUBSEP $2; bst[k] = $3; bw[k] = $6; br[k] = $12; blist[++bn] = k; bkeys[k] = 1
}
END {
    fails = 0
    for (i = 1; i <= bn; i++) {
        k = blist[i]; split(k, p, SUBSEP); label = p[1] "/" p[2]
        if (bst[k] != "ok") { printf "SKIP  %-28s baseline status is %s, not compared\n", label, bst[k]; continue }
        if (!(k in seen_new)) { printf "FAIL  %-28s missing from new results\n", label; fails++; continue }
        if (nst[k] != "ok") { printf "FAIL  %-28s status %s (baseline ok)\n", label, nst[k]; fails++; continue }
        bad = ""
        if (bw[k] + 0 > 0) {
            d = (nw[k] - bw[k]) / bw[k] * 100
            if (d > maxr) bad = bad sprintf(" wall %+.1f%%", d)
        }
        if (br[k] + 0 > 0) {
            d = (nr[k] - br[k]) / br[k] * 100
            if (d > maxr) bad = bad sprintf(" rss %+.1f%%", d)
        }
        if (npm[k] != "") {
            if (npm[k] + 0 > maxp) bad = bad sprintf(" pause_max %s ms > %s", npm[k], maxp)
        } else if (nlk[k] == "yes" && p[2] ~ /^managed/) {
            bad = bad " no pause counters from a linked collector"
        }
        if (bad != "") { printf "FAIL  %-28s%s\n", label, bad; fails++ }
        else printf "PASS  %-28s wall %s s (base %s), rss %s KiB (base %s), pause_max %s ms\n", label, nw[k], bw[k], nr[k], br[k], (npm[k] == "" ? "n/a" : npm[k])
    }
    for (i = 1; i <= nn; i++) if (!(nlist[i] in bkeys)) { split(nlist[i], q, SUBSEP); printf "NOTE  %s/%s not in the baseline, not compared\n", q[1], q[2] }
    if (unmeasured && allowu != "1") fails += unmeasured
    if (fails) { printf "gate: %d failing row(s)\n", fails; exit 1 }
    print "gate: pass"
}' "$NEW" "$BASE"
