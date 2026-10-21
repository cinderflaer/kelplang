use super::*;

/// A binary P6 PPM as `(width, height, body)`. The three header lines are `P6`, the size and the
/// maximum value, each terminated by a newline, which is what QEMU's `screendump` and
/// `soft/debug.dump_ppm` both write.
pub(crate) fn parse_ppm(bytes: &[u8]) -> (usize, usize, &[u8]) {
    let mut parts = bytes.splitn(4, |b| *b == b'\n');
    assert_eq!(parts.next(), Some(b"P6".as_slice()), "the PPM magic");
    let size = parts.next().expect("the PPM size line");
    let mut numbers = size
        .split(|b| *b == b' ')
        .map(|n| String::from_utf8_lossy(n).parse::<usize>().expect("a PPM dimension"));
    let width = numbers.next().expect("the PPM width");
    let height = numbers.next().expect("the PPM height");
    assert_eq!(parts.next(), Some(b"255".as_slice()), "the PPM maximum value");
    (width, height, parts.next().expect("the PPM body"))
}

/// The raster's `checksum_rgb` over a P6 body: FNV-1a 64 with each pixel's blue, green and red bytes
/// and a constant `0xFF` for the fourth. The kernel prints the same number on its `display` line, so
/// a display device that shows the framebuffer produces the same value.
pub(crate) fn ppm_rgb_checksum(width: usize, height: usize, body: &[u8]) -> u64 {
    let mut h: u64 = 14695981039346656037;
    for i in 0..width * height {
        let (r, g, b) = (body[3 * i], body[3 * i + 1], body[3 * i + 2]);
        for byte in [b, g, r, 255u8] {
            h ^= u64::from(byte);
            h = h.wrapping_mul(1099511628211);
        }
    }
    h
}

/// Polyp v3 stage 3: `PpmPlatform` writes each presented frame when `POLYP_DUMP_DIR` is set.
#[test]
pub(crate) fn polyp_soft_ppm_platform_writes_a_frame() {
    let bin = out_path("polyp_ui_test_ppm");
    let build = kelpc()
        .arg("build")
        .arg(fixture("tests/fixtures/examples/polyp-ui-test"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected the polyp ui test project to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let dir = out_path("polyp_ui_ppm");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("the ppm dump directory");
    let run = run_bounded(
        &bin,
        &[
            ("POLYP_UI_TEST", "ppm"),
            ("POLYP_DUMP_DIR", dir.to_str().unwrap()),
        ],
    );
    assert!(
        run.status.success(),
        "expected the ppm run to exit successfully, got:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout), "ppm\n1\n1\n");
    let ppm = dir.join("frame_0.ppm");
    let bytes = std::fs::read(&ppm).expect("the PPM the platform wrote");
    assert_eq!(&bytes[..13], b"P6\n64 48\n255\n", "the PPM header");
    assert_eq!(bytes.len(), 13 + 64 * 48 * 3, "the PPM body length");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Polyp v3 stage 5: the soft backend's work budgets, pinned as exact machine-independent counts.
///
/// `examples/polyp-budget-test` drives the backend directly and prints one number per line:
///
/// - `idle`: an idle frame writes 0 pixels, presents 0 rectangles and allocates nothing.
/// - `text`: a same-width label change damages one rectangle (area 168, the 24x7 label) and, after
///   the patch buffer has seen that shape, accounts for 0 allocations.
/// - `hover`/`focus`: a hover or focus change damages exactly the affected widget(s), and allocates
///   nothing. The primitive pixel count is higher than the rectangle area because a repaint fills
///   the rectangle with the background and then draws the widget over it (the overdraw `repaint`
///   measures).
/// - `type`: typing one character damages at most the entry's rectangle (area 1800) and accounts
///   for the one allocation the new text needs.
/// - `checkbox`, `progress`, `caret`, `scroll`: a checkbox flip, a progress change, a caret move
///   and a wheel step each damage exactly one rectangle (areas 504, 720, 1800 and 600000) and
///   account for 0 allocations. The caret move leaves the text alone and the caret at byte 4.
/// - `remove`: dropping one row of 200 re-measures one node, re-arranges the tree, and damages only
///   rectangles at or below the removed row (`70` is the smallest damage `y`); nothing above it is
///   touched.
/// - `append`: adding a row at the end damages the new (off-screen) row and nothing else.
/// - `repaint`: a full 800x600 repaint overdraws 1.969x, under the factor 2 the budget allows.
/// - `deadline`: a 2500 us budget and a clock that charges 1000 us per read stop the first frame
///   after three rectangles (`presented 3`, `damage 2`) and leave the last two for the next frame
///   (`presented 2`, `damage 0`); the second frame is never deferred.
/// - `allocs`: an idle frame and a steady-state patch frame both account for 0 allocations; the
///   patch buffer's high-water is 1 patch.
#[test]
pub(crate) fn polyp_soft_work_budgets_hold() {
    let bin = out_path("polyp_budget_test");
    let build = kelpc()
        .arg("build")
        .arg(fixture("tests/fixtures/examples/polyp-budget-test"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected the polyp budget test project to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = run_bounded(&bin, &[]);
    assert!(
        run.status.success(),
        "expected the budget test to exit successfully, got:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        POLYP_BUDGET_EXPECTED,
        "unexpected output from the budget test"
    );
}

pub(crate) const POLYP_BUDGET_EXPECTED: &str = r"idle
0
0
0
0
text
1
168
212
0
1
0
hover
1
440
988
0
focus
1
440
1064
0
type
1
1800
4114
1
Xabcd
remove
16
3712864
832351
1
1002
70
row
append
1
15680
0
6
1012
checkbox
1
504
591
0
progress
1
720
1764
0
caret
1
1800
4101
0
4
scroll
1
600000
967616
0
repaint
1
945223
480000
1969
deadline
deferred
3
2
done
2
0
allocs
0
0
1
";

/// Polyp v3 stage 5: the benchmark builds and prints a table.
///
/// `examples/polyp-bench` runs the five scenarios in `design/polyp-v3.md (kelp wiki)`'s table. The test sets
/// `POLYP_BENCH_QUICK`, which shortens every scenario to one frame per run, so the whole run fits
/// in the bounded runner; the counts it pins are the same numbers the full
/// run's table carries. The wall-clock columns are the host's and are only checked to be numbers.
#[test]
pub(crate) fn polyp_bench_prints_a_bounded_table() {
    let bin = out_path("polyp_bench");
    let build = kelpc()
        .arg("build")
        .arg(fixture("tests/fixtures/examples/polyp-bench"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected the polyp bench project to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = run_bounded(&bin, &[("POLYP_BENCH_QUICK", "1")]);
    assert!(
        run.status.success(),
        "expected the benchmark to exit successfully, got:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let text = String::from_utf8_lossy(&run.stdout);
    let lines: Vec<&str> = text.lines().collect();
    // Five scenarios, seven lines each: name, frames, pixels, rects, allocs, median, worst.
    assert_eq!(lines.len(), 35, "the benchmark's table shape changed:\n{text}");
    let expected = [
        ("scroll-200", 5, 4393660, 5, 0),
        ("type-100", 5, 163730, 5, 5),
        ("toggle-50", 5, 231965, 10, 0),
        ("resize", 5, 7775980, 80, 0),
        ("repaint", 5, 4726115, 5, 0),
    ];
    for (i, (name, frames, pixels, rects, allocs)) in expected.iter().enumerate() {
        let base = i * 7;
        assert_eq!(lines[base], *name, "scenario {i}'s name");
        assert_eq!(lines[base + 1], frames.to_string(), "{name} frames");
        assert_eq!(lines[base + 2], pixels.to_string(), "{name} pixels");
        assert_eq!(lines[base + 3], rects.to_string(), "{name} rects");
        assert_eq!(lines[base + 4], allocs.to_string(), "{name} allocations");
        for k in 5..7 {
            let us: i64 = lines[base + k].parse().unwrap_or_else(|_| {
                panic!("{name}'s time column is not a number: {}", lines[base + k])
            });
            assert!(us >= 0, "{name}'s time column is negative");
        }
    }
}

/// Build one Polyp v4 example project, run it bounded and return its stdout. Every v4 test below is
/// a project in `examples/` that prints a deterministic transcript; the file under
/// `tests/polyp_v4/` is the pinned copy, so a changed number is a visible diff.
pub(crate) fn polyp_v4_transcript(project: &str, tag: &str, envs: &[(&str, &str)]) -> String {
    polyp_v4_transcript_for(project, tag, envs, 10)
}

/// `polyp_v4_transcript` with a `secs`-second limit on the run instead of 10.
pub(crate) fn polyp_v4_transcript_for(project: &str, tag: &str, envs: &[(&str, &str)], secs: u32) -> String {
    let bin = out_path(tag);
    let build = kelpc()
        .arg("build")
        .arg(fixture(&format!("tests/fixtures/examples/{project}")))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected {project} to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = run_bounded_for(&bin, envs, secs);
    assert!(
        run.status.success(),
        "expected {project} to exit successfully, got:\n{}\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    String::from_utf8_lossy(&run.stdout).into_owned()
}

/// Polyp v4 stage 1: timers and animation on the headless soft backend, driven by the test clock.
///
/// `examples/polyp-anim-test` pins the integer easings, a progress bar animating 0..1000 permille over
/// 500 ms (exact values at exact ticks, damage only over the bar, `wants_frames` true while it moves
/// and false after), a restart on a changed `Anim`, a spacer animation that re-lays-out what is below
/// it, opacity and offset animations, a repeating timer that skips the periods it missed, a one-shot
/// that fires once and re-arms when the state asks again, and a UI at rest.
#[test]
pub(crate) fn polyp_v4_timers_and_animation_are_exact_on_the_test_clock() {
    let got = polyp_v4_transcript("polyp-anim-test", "polyp_anim_test", &[]);
    assert_eq!(got, include_str!("../polyp_v4/anim.expected"), "unexpected output from the animation test");
}

/// Polyp v4 stage 2: UTF-8 text in the soft backend.
///
/// `examples/polyp-text-test` pins the decoder (well-formed 1- to 4-byte sequences, stray and
/// truncated bytes, overlong forms, surrogates, values above U+10FFFF: each malformed run is one
/// unit), measure by code point ("héllo" is 5 cells), which blocks have drawings, that a missing
/// glyph and a malformed run draw the same single box and a truncated tail reads nothing out of
/// bounds, and entry editing by code point (caret moves, backspace on `é`, delete on a 3-byte
/// scalar, a malformed byte, a caret stranded inside a sequence, an invalid scalar).
#[test]
pub(crate) fn polyp_v4_text_is_utf8_by_code_point() {
    let got = polyp_v4_transcript("polyp-text-test", "polyp_text_test", &[]);
    assert_eq!(got, include_str!("../polyp_v4/text.expected"), "unexpected output from the text test");
}

/// Polyp v4 stage 3: the accessibility tree.
///
/// `examples/polyp-a11y-test` pins the todo app's tree dump, that pressing "add" by node id ends in the
/// same model and focus as a click at its bounds, focus by id, `set_value` on an entry, a checkbox
/// toggled by id (named after the label beside it), and that a disabled button and entry reject every
/// action, ignore clicks and are skipped by Tab.
#[test]
pub(crate) fn polyp_v4_a11y_tree_drives_the_ui() {
    let got = polyp_v4_transcript("polyp-a11y-test", "polyp_a11y_test", &[]);
    assert_eq!(got, include_str!("../polyp_v4/a11y.expected"), "unexpected output from the a11y test");
}

/// Polyp v4 stage 4: memoised subtrees.
///
/// `examples/polyp-memo-test` runs a scrolled list of memoised rows beside the same list built with
/// plain keyed rows and requires the same framebuffer checksum after every change; it pins the
/// counters: 200 rows are 200 misses the first time, one changed row is 1 view call and 199 hits,
/// no change is 200 hits and nothing painted, a deps change rebuilds exactly that row, a reorder
/// keeps every memo a hit, a removed row's record is dropped and its key built again when it comes
/// back, a nested memo hits when only the outer deps changed, a press inside a memo row reaches the
/// model, and the plain loop with no store draws the same screen.
#[test]
pub(crate) fn polyp_v4_memo_skips_unchanged_subtrees_and_never_changes_a_pixel() {
    let got = polyp_v4_transcript("polyp-memo-test", "polyp_memo_test", &[]);
    assert_eq!(got, include_str!("../polyp_v4/memo.expected"), "unexpected output from the memo test");
}

/// Polyp v4 stage 4: the memoisation benchmark builds and its counters are exact. The times
/// (columns 7 to 9) are the host's; `POLYP_BENCH_QUICK` shortens each run to 3 frames.
#[test]
pub(crate) fn polyp_v4_memo_bench_counts_view_calls_and_hits() {
    // The bench runs five timed passes per variant, so it can exceed the 10 s limit on a loaded host.
    // The counts and pixels do not depend on timing, so only the limit changes here.
    let got = polyp_v4_transcript_for("polyp-memo-bench", "polyp_memo_bench", &[("POLYP_BENCH_QUICK", "1")], 60);
    let lines: Vec<&str> = got.lines().collect();
    // Four runs, nine lines each: label, rows, frames, view calls, hits, pixels, median, view, diff.
    assert_eq!(lines.len(), 36, "the memo benchmark's shape changed:\n{got}");
    let expect = [("plain", 200, 0, 0), ("memo", 200, 3, 3 * 199), ("plain", 1000, 0, 0), ("memo", 1000, 3, 3 * 999)];
    for (i, (label, rows, calls, hits)) in expect.iter().enumerate() {
        let b = i * 9;
        assert_eq!(lines[b], *label);
        assert_eq!(lines[b + 1], rows.to_string(), "{label} rows");
        assert_eq!(lines[b + 2], "3", "{label} frames");
        assert_eq!(lines[b + 3], calls.to_string(), "{label} view calls");
        assert_eq!(lines[b + 4], hits.to_string(), "{label} hits");
        for k in 6..9 {
            lines[b + k].parse::<i64>().unwrap_or_else(|_| panic!("{label}: column {k} is not a number"));
        }
    }
    // Memo and plain write the same pixels.
    assert_eq!(lines[5], lines[9 + 5], "200 rows: pixels written");
    assert_eq!(lines[18 + 5], lines[27 + 5], "1000 rows: pixels written");
}

/// The lines of one Polyp v4 demo run: from the first `step ` line to the first `end` line, without
/// the hosted entry's `-- demo --` separator. Used on the hosted transcript and on a kernel's serial
/// output alike, so the two are sliced identically and only the run itself can differ.
pub(crate) fn v4_run_slice(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines
        .iter()
        .position(|line| line.starts_with("step "))
        .unwrap_or_else(|| panic!("no step line in the run's output:\n{text}"));
    let end = lines
        .iter()
        .position(|line| *line == "end")
        .unwrap_or_else(|| panic!("no `end` line in the run's output:\n{text}"));
    lines[start..=end]
        .iter()
        .filter(|line| **line != "-- demo --")
        .map(|line| line.to_string())
        .collect()
}

/// Polyp v4: the demo program on the riscv64 kernel prints the hosted run's own transcript.
///
/// `examples/polyp-v4-riscv64` is the same `app.kp`/`scenario.kp` as `examples/polyp-v4-demo`, with a
/// platform clock the kernel advances and a `Tick` event it queues for it: every step line (clock,
/// framebuffer checksum, rectangles presented, `wants_frames`, timer count) and the report must equal
/// the hosted line. Skipped under `dev.nix`; run it in `dev-kernel.nix`.
#[test]
pub(crate) fn polyp_v4_riscv64_prints_the_hosted_transcript() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU) {
        eprintln!("skipping the riscv64 Polyp v4 kernel: 'ld.lld' or 'qemu-system-riscv64' is not on PATH");
        return;
    }
    let expected = v4_run_slice(&polyp_v4_transcript("polyp-v4-demo", "polyp_v4_demo_hosted_rv", &[]));
    let name = "polyp_v4_riscv64";
    let captured = boot_and_capture(&RISCV64_QEMU, name, &fixture("tests/fixtures/examples/polyp-v4-riscv64"), &|got| {
        got.lines().any(|line| line == "end")
    });
    let undefined = undefined_symbols(&out_path(name));
    assert!(
        undefined.trim().is_empty(),
        "the riscv64 Polyp v4 image names undefined symbols:\n{undefined}"
    );
    assert_eq!(v4_run_slice(&captured), expected, "the riscv64 kernel differs from the hosted run; captured:\n{captured}");
}

/// The same on the x86-64 kernel (`examples/polyp-v4-x86_64`).
#[test]
pub(crate) fn polyp_v4_x86_64_prints_the_hosted_transcript() {
    if !have_ld_lld() || !have_qemu(&X86_64_QEMU) {
        eprintln!("skipping the x86-64 Polyp v4 kernel: 'ld.lld' or 'qemu-system-x86_64' is not on PATH");
        return;
    }
    let expected = v4_run_slice(&polyp_v4_transcript("polyp-v4-demo", "polyp_v4_demo_hosted_x86", &[]));
    let name = "polyp_v4_x86_64";
    let captured = boot_and_capture(&X86_64_QEMU, name, &fixture("tests/fixtures/examples/polyp-v4-x86_64"), &|got| {
        got.lines().any(|line| line == "end")
    });
    let undefined = undefined_symbols(&out_path(name));
    assert!(
        undefined.trim().is_empty(),
        "the x86-64 Polyp v4 image names undefined symbols:\n{undefined}"
    );
    assert_eq!(v4_run_slice(&captured), expected, "the x86-64 kernel differs from the hosted run; captured:\n{captured}");
}

/// Polyp v4: the hosted demo transcript is pinned, so a changed number is a visible diff.
#[test]
pub(crate) fn polyp_v4_demo_prints_its_pinned_transcript() {
    let got = polyp_v4_transcript("polyp-v4-demo", "polyp_v4_demo", &[]);
    assert_eq!(got, include_str!("../polyp_v4/demo.expected"), "unexpected output from the v4 demo");
}

#[test]
pub(crate) fn a_while_true_loop_that_breaks_continues_after_the_loop() {
    assert_compiles_and_runs("tests/fixtures/while_true_break.kp", "4\n5\n30\nxxx\n");
}

// Language-completeness 8: the interaction matrix of the traits, iterators, const
// generics, operator and conversion features (`design/traits.md (kelp wiki)` 14). Each cell is a
// fixture; a refused cell pins its message, an accepted one its output.
#[test]
pub(crate) fn matrix_associated_types_cannot_be_boxed_as_a_trait_object() {
    assert_rejected_with(
        "tests/fixtures/matrix_assoc_boxdyn.kp",
        "'Box<dyn Source>' is not allowed: the trait declares the associated type 'Item'",
    );
}

#[test]
pub(crate) fn matrix_an_operator_trait_takes_no_type_argument() {
    assert_rejected_with(
        "tests/fixtures/matrix_generic_trait_operator.kp",
        "takes no type arguments",
    );
}

#[test]
pub(crate) fn matrix_a_user_iterator_runs_defers_per_iteration_and_on_return() {
    assert_compiles_and_runs(
        "tests/fixtures/matrix_iter_defer_return.kp",
        "iter\niter\nleave\n4\niter\nleave\n-1\n",
    );
}

#[test]
pub(crate) fn matrix_a_const_generic_type_is_not_an_iterator_but_a_cursor_walks_it() {
    assert_rejected_with(
        "tests/fixtures/matrix_iter_const_generic.kp",
        "implement the trait for a non-generic wrapper",
    );
    assert_compiles_and_runs(
        "tests/fixtures/matrix_iter_const_generic_ok.kp",
        "15\n30\n4\n5\n6\n",
    );
}

#[test]
pub(crate) fn matrix_question_mark_inside_next_must_match_the_option_shape() {
    assert_rejected_with(
        "tests/fixtures/matrix_from_try_in_next.kp",
        "match the value instead",
    );
    assert_compiles_and_runs("tests/fixtures/matrix_from_try_ok.kp", "11\n21\n");
}

#[test]
pub(crate) fn matrix_display_works_through_a_generic_bound_and_a_template() {
    assert_compiles_and_runs(
        "tests/fixtures/matrix_display_generic.kp",
        "<1,2>\n#9\n<1,2> <1,2>\n",
    );
}

#[test]
pub(crate) fn matrix_ord_never_decides_map_key_identity() {
    assert_compiles_and_runs("tests/fixtures/matrix_ord_map_key.kp", "2\n11\ntrue\n");
    assert_compiles_and_runs(
        "tests/fixtures/matrix_ord_only_key.kp",
        "2\nfalse\ntrue\n",
    );
}

/// Builds `fixture` with the default backend and returns the build and the run.
pub(crate) fn build_and_run(fixture_name: &str) -> (std::process::Output, Option<std::process::Output>) {
    let bin = out_path(fixture_name);
    let mut cmd = Command::new(kelpc_bin());
    cmd.env_remove("KELPC_BACKEND");
    let build = cmd
        .arg("build")
        .arg(fixture(&format!("tests/fixtures/{fixture_name}.kp")))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    if !build.status.success() {
        return (build, None);
    }
    let run = Command::new(&bin).output().expect("failed to run the binary");
    (build, Some(run))
}

/// Shifts (`tl7`): an in-range shift and the `wrapping_shl`/`wrapping_shr`
/// escapes print the same on both backends.
#[test]
pub(crate) fn shifts_in_range_and_wrapping_shifts_are_checked() {
    let expected = "128\n1\n240\n32768\n1\n2147483648\n1\n-2147483648\n-1\n-4\n9223372036854775808\n1\n4611686018427387904\n-1\n48\n5\n2\n1\n2147483648\n268435456\n2\n2\n-8\n-2147483648\n";
    let (build, run) = build_and_run("shift_checked");
    let run = run.unwrap_or_else(|| {
        panic!("shift_checked failed to build:\n{}", String::from_utf8_lossy(&build.stderr))
    });
    assert!(run.status.success(), "shift_checked failed:\n{}", String::from_utf8_lossy(&run.stderr));
    assert_eq!(String::from_utf8_lossy(&run.stdout), expected);
}

/// Shifts (`tl7`): a shift by the width or more, or a negative amount, panics
/// with the same message on both backends, at every width, and the legal shift
/// printed before it still appears.
#[test]
pub(crate) fn a_shift_by_the_width_or_more_panics() {
    for (name, first) in [
        ("shift_panic_u32", "2147483648\n"),
        ("shift_panic_u8_right", "1\n"),
        ("shift_panic_negative", "-8\n"),
        ("shift_panic_u64", "9223372036854775808\n"),
    ] {
        let (build, run) = build_and_run(name);
        let run = run.unwrap_or_else(|| {
            panic!("{name} failed to build:\n{}", String::from_utf8_lossy(&build.stderr))
        });
        assert!(!run.status.success(), "{name} should panic");
        assert_eq!(String::from_utf8_lossy(&run.stdout), first, "{name}");
        let stderr = String::from_utf8_lossy(&run.stderr);
        assert!(
            stderr.contains("shift amount out of range"),
            "{name} panicked with:\n{stderr}"
        );
    }
}

/// Shifts (`tl7`): a shift by a constant amount at or past the width is a
/// compile error that names the type and the amount, on both backends.
#[test]
pub(crate) fn a_shift_by_a_constant_at_or_past_the_width_is_a_compile_error() {
    for (name, needle) in [
        ("shift_const_too_wide", "a shift by 33 is not defined for a 'u32'"),
        ("shift_const_named_too_wide", "a shift by 8 is not defined for a 'u8'"),
    ] {
        let (build, run) = build_and_run(name);
        assert!(run.is_none(), "{name} should be rejected");
        let stderr = String::from_utf8_lossy(&build.stderr);
        assert!(stderr.contains(needle), "{name}: expected '{needle}', got:\n{stderr}");
    }
}

/// Shifts (`tl7`): the freestanding half. The check reaches the project's own
/// panic handler, the image has no undefined symbol, and it boots to the
/// message. Needs the `dev-kernel.nix` tools; a shell without them skips.
#[test]
pub(crate) fn a_freestanding_shift_out_of_range_reaches_the_projects_panic_handler() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU) {
        println!(
            "skipping the freestanding shift boot: 'ld.lld' or 'qemu-system-riscv64' is not on \
             PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = out_path("freestanding_shift");
    let build = Command::new(kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .arg("build")
        .arg(project("freestanding_shift"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the freestanding shift build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let symbols = tool("llvm-nm", &[bin.to_str().expect("the artifact path is utf-8")]);
    let undefined: Vec<&str> = symbols
        .lines()
        .filter(|line| line.split_whitespace().next() == Some("U"))
        .collect();
    assert!(undefined.is_empty(), "undefined symbols:\n{}", undefined.join("\n"));
    boot_under_qemu(
        &RISCV64_QEMU,
        "freestanding_shift",
        &project("freestanding_shift"),
        &["8", "panic: shift amount out of range"],
    );
}
