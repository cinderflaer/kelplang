use super::*;

#[test]
pub(crate) fn http_server_low_level_api_and_accessors_work_in_one_process() {
    // The defaults, the request accessors (`path_segments`, `query_param` with
    // its percent- and plus-decoding, case-insensitive headers) and the
    // low-level accept / read_request / write_response / write_error / close
    // loop, with the client and the server in one process over loopback.
    assert_compiles_and_runs(
        "tests/fixtures/http_server_lowlevel.kp",
        "8192\n16384\n100\n1048576\n10000\n30000\n5000\n100\n3\nb c\nd\n1\nA b\n\n\n%zz\n<none>\n1\ntrue\nfalse\nGET\n/p\nq=1\nPOST\n2\n0\nfalse\nHTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: 3\r\n\r\noneHTTP/1.1 200 OK\r\nContent-Type: application/json\r\nX-Seen: yes\r\nContent-Length: 2\r\n\r\n{}\n400\nHTTP/1.1 400 Bad Request\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: 35\r\nConnection: close\r\n\r\nconflicting Content-Length headers\n\n",
    );
}

/// Polyp v3 stage 1: the pure diff and the whole todo program on the headless backend.
///
/// `examples/polyp-core-test` has no `[extern.*]` section at all, so this runs in the plain
/// development shell with no X server and no GTK, on the backend under test. The first half is the
/// diff: identical trees produce nothing, a text change one Update, a keyed swap one Move and
/// no Create or Remove, an insert two Creates (the keyed row and its Text child), a remove one
/// patch for the whole subtree, a kind change a replace, and a duplicate key a panic naming it.
/// After the diff, the headless tree after each step of a keyed reorder, insert and remove must
/// match a fresh build of the new view (the invariant the GTK backend's mount has to keep too).
///
/// The second half runs the todo example's own program (`examples/polyp-todo/src/app.kp`, linked in
/// as `deps/todo`) on the headless backend with the same scripted input `examples/polyp-todo/smoke.sh`
/// arms the GTK backend with, and prints the same summary. The counters here must be the ones that
/// smoke asserts: created 22, updated 10, removed 5, live 17, focus present and surviving, one
/// entry built, and the entry still showing `gamma` while the model's draft is empty.
#[test]
pub(crate) fn polyp_core_diffs_and_runs_the_todo_program_with_no_display() {
    let bin = out_path("polyp_core_test");
    let build = kelpc()
        .arg("build")
        .arg(fixture("tests/fixtures/examples/polyp-core-test"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected the polyp core test project to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let run = Command::new(&bin).output().expect("failed to run the core test binary");
    assert!(
        run.status.success(),
        "expected the core test to exit successfully, got:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        EXPECTED,
        "unexpected output from the polyp core test"
    );
}

pub(crate) const EXPECTED: &str = "identical\n0\n0\n0\n0\ntext\n0\n1\n0\n0\nreorder\n0\n0\n0\n1\ninsert\n2\n0\n0\n0\nremove\n0\n0\n1\n0\nkind\n1\n0\n1\n0\ninvariant\nstart\nok\ninvariant\nc-b-d-a\nok\ninvariant\nd-c-b-a\nok\ninvariant\nb-d-a-c\nok\ninvariant\ninsert-and-move\nok\ninvariant\nshrink-to-two\nok\ninvariant\ngrow-back\nok\n-- todo --\n2\nalpha\nfalse\nbeta\ntrue\n7\n12\n17\n22\n22\n22\n22\n10\n5\n17\ntrue\ntrue\n1\ngamma\n\nend\n-- tree --\ncolumn\n  row\n    entry id=0 \"gamma\"\n    button id=1 \"add\"\n  separator\n  scrolled\n    row #1\n      checkbox id=2 unchecked\n      text \"alpha\"\n      button id=3 \"^\"\n      button id=4 \"x\"\n    row #2\n      checkbox id=5 checked\n      text \"beta\"\n      button id=6 \"^\"\n      button id=7 \"x\"\n  progress 0.500\n\n";

/// `[build] emit = "html"` (Polyp HTML emit): the app's first view is written as
/// `index.html` in the directory the binary went to. The fixture is a Polyp app with
/// two texts, a spacer and a button, and the page must carry what a browser would show.
#[test]
pub(crate) fn polyp_html_emit_writes_the_first_view_as_index_html() {
    let dir = out_path("polyp_html_emit");
    std::fs::create_dir_all(&dir).expect("could not create the emit directory");
    let index = dir.join("index.html");
    let _ = std::fs::remove_file(&index);
    let build = kelpc()
        .arg("build")
        .arg(fixture("tests/fixtures/polyp_html"))
        .arg("-o")
        .arg(dir.join("app"))
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected the polyp html fixture to emit, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let html = std::fs::read_to_string(&index)
        .unwrap_or_else(|e| panic!("expected index.html beside the binary: {e}"));
    for want in [
        "<!doctype html>",
        "<p class=\"polyp-text\" data-kind=\"text\">Hello from Polyp</p>",
        "the count is 2",
        "<div class=\"polyp-column\" style=\"gap:6px\" data-kind=\"column\">",
        "<div style=\"height:6px\" data-kind=\"spacer\"></div>",
        "<button type=\"button\" data-kind=\"button\">+1</button>",
    ] {
        assert!(html.contains(want), "index.html lacks {want:?}:\n{html}");
    }
}

/// The duplicate-key half, in its own run because a `panic` ends the process.
#[test]
pub(crate) fn polyp_core_panics_on_a_duplicate_key_and_names_it() {
    let bin = out_path("polyp_core_dupkey");
    let build = kelpc()
        .arg("build")
        .arg(fixture("tests/fixtures/examples/polyp-core-test"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected the polyp core test project to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let run = Command::new(&bin)
        .env("POLYP_TEST", "dupkey")
        .output()
        .expect("failed to run the core test binary");
    assert!(
        !run.status.success(),
        "expected a duplicate key to panic, but the program exited successfully"
    );
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.contains("duplicate key 'dup' among the children of one container"),
        "expected the panic to name the key, got:\n{stderr}"
    );
}


/// Polyp v3 stage 2: the software rasterizer's primitives, font, damage list and checksums.
///
/// `examples/polyp-soft-test` carries the tests in Kelp: each primitive runs against an
/// independent per-pixel reference written in the project (full-precision integers, its own
/// geometry), on fixed-seed LCG inputs with negative and zero sizes and clips on every edge.
/// The project also prints golden FNV checksums for three scenes. Three panic contracts
/// (`POLYP_SOFT_TEST=scale`, `clip_depth`, `clip_empty`) and the hosted PPM dump
/// (`POLYP_SOFT_TEST=dump`) are exercised here, because a panic ends the process and the dump
/// writes a file.
///
/// Every run goes through `run_bounded`: `ulimit -v 1048576` (1 GiB), `ulimit -t 10` and
/// `timeout 10`, so a bug cannot take the host down the way an earlier unbounded 6.7 GB run did.
#[test]
pub(crate) fn polyp_soft_raster_matches_its_references_and_golden_checksums() {
    let bin = out_path("polyp_soft_test");
    let build = kelpc()
        .arg("build")
        .arg(fixture("tests/fixtures/examples/polyp-soft-test"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected the polyp soft test project to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let run = run_bounded(&bin, &[]);
    assert!(
        run.status.success(),
        "expected the soft test to exit successfully, got:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        POLYP_SOFT_EXPECTED,
        "unexpected output from the soft test"
    );

    for (mode, needle) in [
        ("scale", "text scale must be in 1..=4"),
        ("clip_depth", "clip stack depth 8 exceeded"),
        ("clip_empty", "clip stack is empty"),
    ] {
        let run = run_bounded(&bin, &[("POLYP_SOFT_TEST", mode)]);
        assert!(
            !run.status.success(),
            "expected POLYP_SOFT_TEST={mode} to panic, but the program exited successfully"
        );
        let stderr = String::from_utf8_lossy(&run.stderr);
        assert!(
            stderr.contains(needle),
            "expected the {mode} panic to mention '{needle}', got:\n{stderr}"
        );
    }

    let ppm = out_path("polyp_soft_frame").with_extension("ppm");
    let run = run_bounded(
        &bin,
        &[
            ("POLYP_SOFT_TEST", "dump"),
            ("POLYP_SOFT_PPM", ppm.to_str().unwrap()),
        ],
    );
    assert!(run.status.success(), "the dump run failed:\n{}", String::from_utf8_lossy(&run.stderr));
    assert_eq!(String::from_utf8_lossy(&run.stdout), "dumped\n");
    let bytes = std::fs::read(&ppm).expect("the PPM the dump run wrote");
    assert_eq!(&bytes[..13], b"P6\n40 28\n255\n", "the PPM header");
    assert_eq!(bytes.len(), 13 + 40 * 28 * 3, "the PPM body length");
    // Pixel (0, 0) of the dumped blend scene: `0x80FF0000` source-over the `0xFF000000`
    // background is `(128 * 255 + 127) / 255 == 128` red, zero green, zero blue.
    assert_eq!(&bytes[13..16], &[128u8, 0, 0], "the dumped pixel at (0, 0)");
    let _ = std::fs::remove_file(&ppm);
}

/// Runs `bin` with `envs` under a 1 GiB address-space limit, a 10 s CPU limit and a 10 s wall
/// timeout, and returns its output. Every compiled program this file runs on behalf of the
/// raster tests goes through here.
pub(crate) fn run_bounded(bin: &Path, envs: &[(&str, &str)]) -> std::process::Output {
    run_bounded_for(bin, envs, 10)
}

/// `run_bounded` with a wall-clock and CPU-time limit of `secs` seconds instead of 10.
/// Used only where a correct run can take longer than 10 s on a loaded host.
pub(crate) fn run_bounded_for(bin: &Path, envs: &[(&str, &str)], secs: u32) -> std::process::Output {
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(format!("( ulimit -v 1048576; ulimit -t {secs}; timeout {secs} \"$1\" )"))
        .arg("sh")
        .arg(bin);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.output().expect("failed to run the soft test binary")
}

pub(crate) const POLYP_SOFT_EXPECTED: &str = "fill_rect
60
0
stroke_rect
60
0
fill_round_rect
60
0
stroke_round_rect
60
0
line
60
0
blend
40
0
blit_mask8
30
0
draw_text
40
0
clip_stack
40
0
edges
13
0
damage
200
8
0
cap
16
true
static_fb
5870808570317741093
scene shapes
14814842095218757869
scene text
15225207939037777876
scene blend
3376705936528111333
failures
0
";

/// Polyp v3 stage 3: the software backend's hit testing, focus order, entry editing, damage and
/// layout skip.
///
/// `examples/polyp-ui-test` runs on `HeadlessPlatform` in the plain development shell. The five
/// sections are deterministic: the hit section clicks each interactive widget by its own centre;
/// the order section Tabs through the tree and wraps; the edit section types, moves the caret,
/// deletes and backspaces; the damage section changes one label's text and asserts the exact
/// rectangle and that a FNV digest of every pixel outside it is unchanged, then hovers and
/// asserts an idle frame presents nothing; the layout section counts re-measures and re-arranges
/// for a same-width text change (one measure, no arrange) and a wider one (both).
#[test]
pub(crate) fn polyp_soft_ui_hits_focuses_edits_and_damages() {
    let bin = out_path("polyp_ui_test");
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
    let run = run_bounded(&bin, &[]);
    assert!(
        run.status.success(),
        "expected the ui test to exit successfully, got:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        POLYP_UI_EXPECTED,
        "unexpected output from the polyp ui test"
    );
}

pub(crate) const POLYP_UI_EXPECTED: &str = "hit\n0\n1\n2\n3\n-1\norder\n0\n1\n2\n3\n0\n1\n2\n3\n0\n3\nedit\nabcd\n0\nXabcd\n1\nXaYbcd\n3\n2\naYbcd\n0\n5\naYbc\n4\ndamage\n1\n0\n0\n24\n7\noutside-same\n1\n0\n0\nhover\n1\n22\n7\n22\n20\n1\n22\n22\ncheckbox\ntrue\n1\nscroll\nscrolled\n1\n0\nlayout\n1\n0\n2\n3\n";
