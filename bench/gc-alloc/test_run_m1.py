"""Unit tests for the M1 bound in run-m1.py. Run: cd bench/gc-alloc && python3 -m unittest test_run_m1"""
import importlib.util
import os
import unittest

# run-m1.py has a hyphen in its name, so it is loaded from its path.
_path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "run-m1.py")
_spec = importlib.util.spec_from_file_location("run_m1", _path)
run_m1 = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(run_m1)

# Paired (base, s1) ns per allocation for the nine blocks of build queue job 4ed5dc39
# (cell16 row), copied from its log. Attempt 1 was refused by the old spread rule
# (base spread 6.12 percent); attempt 2 passed it.
ATTEMPT1 = [(13.08, 9.93), (12.30, 10.06), (13.09, 9.87), (12.86, 10.16), (13.08, 9.93),
            (13.10, 9.93), (13.09, 10.23), (12.92, 9.92), (13.09, 9.91)]
ATTEMPT2 = [(12.98, 10.04), (12.86, 9.92), (12.87, 10.03), (13.09, 9.80), (13.10, 9.86),
            (13.07, 9.99), (12.87, 10.09), (12.89, 9.83), (12.93, 10.05)]


def ratios_of(pairs):
    return [s1 / base for base, s1 in pairs]


class BoundTest(unittest.TestCase):
    def test_t_constant_is_one_sided_95_df8(self):
        self.assertAlmostEqual(run_m1.T_ONE_SIDED_95_DF8, 1.8595, places=4)

    def test_attempt2_passes_with_bound_far_under_the_line(self):
        mean, sd, bound = run_m1.upper_bound(ratios_of(ATTEMPT2))
        self.assertAlmostEqual(mean, 0.76820, places=4)
        self.assertAlmostEqual(bound, 0.77567, places=4)
        self.assertLess(bound, run_m1.GATE_RATIO)
        self.assertLess(bound, 0.80)

    def test_attempt1_refused_by_old_spread_rule_passes_the_bound(self):
        mean, sd, bound = run_m1.upper_bound(ratios_of(ATTEMPT1))
        self.assertLessEqual(bound, run_m1.GATE_RATIO)

    def test_bound_formula_matches_hand_calculation(self):
        ratios = [1.00, 1.02, 0.98, 1.04, 0.96, 1.00, 1.02, 0.98, 1.00]
        mean, sd, bound = run_m1.upper_bound(ratios)
        self.assertAlmostEqual(mean, 1.0, places=12)
        expected_sd = (sum((r - 1.0) ** 2 for r in ratios) / 8) ** 0.5
        self.assertAlmostEqual(sd, expected_sd, places=12)
        self.assertAlmostEqual(bound, 1.0 + 1.8595 * expected_sd / 3, places=12)

    def test_synthetic_slow_s1_fails(self):
        ratios = [1.00, 1.20, 1.05, 1.10, 1.15, 1.02, 1.08, 1.12, 1.07]
        mean, sd, bound = run_m1.upper_bound(ratios)
        self.assertGreater(mean, 1.05)
        self.assertGreater(bound, run_m1.GATE_RATIO)

    def test_mean_under_the_line_but_noisy_bound_over_it_fails(self):
        # Mean 1.03 would pass a point comparison; the bound is above 1.05 because the spread is wide.
        ratios = [0.80, 1.30, 0.95, 1.20, 0.85, 1.25, 1.00, 1.05, 1.03]
        mean, sd, bound = run_m1.upper_bound(ratios)
        self.assertLess(mean, 1.05)
        self.assertGreater(bound, run_m1.GATE_RATIO)

    def test_needs_nine_ratios(self):
        with self.assertRaises(ValueError):
            run_m1.upper_bound([1.0] * 8)


class ParseTest(unittest.TestCase):
    OUT = (
        "mode nogc (no cycle in the timed loop)\n"
        "cell16 (24 B)             8000000 allocs     41.30 M allocs/s    24.21 ns/alloc\n"
        "array8x8 (72 B)           1500000 allocs     37.53 M allocs/s    26.65 ns/alloc\n"
        "obj200 (208 B)            1500000 allocs     25.61 M allocs/s    39.05 ns/alloc\n"
        "check cell16 (24 B)     n=8000000 values_ok=1 distinct=1 checksum=0x00000000deadbeef\n"
        "check array8x8 (72 B)   n=1500000 values_ok=1 distinct=1 checksum=0x0000000000000001\n"
        "check obj200 (208 B)    n=1500000 values_ok=1 distinct=1 checksum=0x0000000000000002\n"
    )

    def test_parses_rows_and_checks(self):
        out = run_m1.parse_output(self.OUT)
        self.assertEqual(out["rows"]["cell16 (24 B)"], {"allocs": 8000000, "ns": 24.21})
        self.assertEqual(out["checks"]["cell16 (24 B)"]["checksum"], "0x00000000deadbeef")
        self.assertTrue(out["checks"]["obj200 (208 B)"]["values_ok"])

    def test_missing_check_line_is_not_measured(self):
        text = "\n".join(l for l in self.OUT.splitlines() if not l.startswith("check obj200"))
        with self.assertRaises(run_m1.RunError):
            run_m1.parse_output(text)

    def test_work_signature_detects_a_different_checksum(self):
        a = run_m1.parse_output(self.OUT)
        b = run_m1.parse_output(self.OUT.replace("0x00000000deadbeef", "0x00000000deadbeee"))
        self.assertNotEqual(run_m1.signature(a), run_m1.signature(b))

    def test_row_and_check_counts_must_agree(self):
        with self.assertRaises(run_m1.RunError):
            run_m1.parse_output(self.OUT.replace("n=8000000", "n=7000000"))


if __name__ == "__main__":
    unittest.main()
