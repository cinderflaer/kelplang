//! Coverage, refusal ranking and the differential sweep for the `llvm` backend
//! (MK7 §4.1 and §6, `plan/mk7-llvm-backend.md`).
//!
//! `kelpc/tests/compile_and_run.rs` is backend-agnostic by construction: it
//! compiles a fixture with whichever backend `KELPC_BACKEND` names and compares
//! the run to a literal an author wrote. This file is the other half. It always
//! builds *both* backends, so the C backend is an oracle rather than a second
//! opinion, and it records how much of the corpus the `llvm` backend accepts so
//! the number cannot drift silently. It carries no expected-output table,
//! because the C backend's output is the expected output.
//!
//! The corpus is the one the suite uses: `kelpc/tests/fixtures/*.kp`, the
//! project directories under `kelpc/tests/projects/` that carry a `coral.toml`
//! (the directory is the source, its manifest names the entry point), and
//! `kelpc/examples/*.kp`. A target the C backend rejects is skipped, because a
//! compile-error fixture is rejected by design and C defines what the corpus is.
//!
//! Four things live here, and a fifth was added at MK7 S6:
//!
//! 1. The differential sweep. Every target the C backend builds is built with
//!    `--backend=llvm` too. For every target both backends build, both binaries
//!    are run and their stdout and exit status must be byte-identical, and a
//!    mismatch fails with the target named and both outputs shown. A target the
//!    `llvm` backend refuses is recorded with its message rather than failed: a
//!    partial backend refuses by name, and every refusal names the MK7 stage
//!    that lifts it.
//! 2. The crash check. A build or a run that dies from a signal (SIGSEGV,
//!    SIGABRT, killed) is a failure, because a partial backend refuses with a
//!    diagnostic and never crashes. A clean nonzero exit that does not begin
//!    with a diagnostic is treated the same way.
//! 3. The ratchet. [`LLVM_BUILT_BASELINE`] is the count of corpus targets the
//!    `llvm` backend built when the number was last recorded. It may only rise;
//!    [`RERECORD_ENV`] re-records it.
//! 4. The ranking report. The refusals grouped by their exact message, so the
//!    slice each one waits on is visible in the output. It is a `println!` and
//!    never an assertion, so a refusal moving between messages cannot break a
//!    run.
//! 5. The optimization-level sweep (S6, §4.3). The corpus is swept again at each
//!    of `-O0`, `-O1`, `-O2`, `-O3` and `-Os`, both backends at that level, and
//!    the same crash and disagreement assertions apply at every one. A level
//!    that changes behavior is a bug, not a tuning choice: a pass pipeline is the
//!    first thing here that can exploit IR Kelp emitted without a licence for it
//!    (§7).
//!
//! The sweep compiles the whole corpus twice, which is close to a minute, so it
//! is `#[ignore]`d and the default suite runs only the cheap plumbing check at
//! the bottom of this file. Run the instrument explicitly, in `--release` as
//! the rest of the suite is:
//!
//! ```text
//! cargo test --release --test backend_coverage -- --ignored --nocapture
//! ```
//!
//! The compiler is invoked with `--backend=` on the command line rather than
//! through `KELPC_BACKEND`, so the environment cannot change what is measured.

use std::collections::BTreeMap;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;

/// The prefix every named refusal carries (`codegen_llvm.rs::refuse` plus the
/// driver's `error:` lead-in). A refusal that does not carry it is an internal
/// error or a crash rather than a named gap, and the ranking report prints those
/// as their own rows so they cannot hide among the named ones.
const REFUSAL_PREFIX: &str = "error: the llvm backend cannot lower";

/// Setting this to any value re-records [`LLVM_BUILT_BASELINE`]: the ratchet
/// then prints the live count and the sorted list of targets behind it instead
/// of asserting. Re-recording is a one-line edit to the constant, in the commit
/// that raises the coverage.
const RERECORD_ENV: &str = "LLVM_BUILT_BASELINE_RERECORD";

/// The corpus targets the `llvm` backend built when this ratchet was recorded:
/// **223 of the 223 targets the C backend builds**. The refusal ranking is
/// empty, which is MK7 §4.1's "the refusal ranking empty" met.
///
/// Recorded at MK7 S5 on 2026-09-21, raised from the 221 the S3 tip built. Two
/// fixtures landed with S5, each the regression fixture for a defect the
/// curated corpus could not see and the differential fuzzer found:
/// `list_float_element.kp` (a plain `List<f64>` element, whose `contains` the
/// `llvm` backend emitted an integer compare for, which is invalid IR) and
/// `pd_owned_container.kp` (an owned `pd` no expression reads, whose container
/// function set `collect_instantiations` omitted, so the backend reported an
/// internal error where the C backend built it).
///
/// The S3 record this supersedes: **221 of the 221 targets the C backend
/// builds**, recorded at MK7 S3 (the rest: `pd` globals, the capability check,
/// the last three refusals) on 2026-09-21. The 183 the S3A tip built plus the 38
/// the rest of S3 added: the 35 `pd` targets (module globals, `kelp_init_pds`,
/// the `atexit` drop and the `argv` saved by `main`), the 9 that reach a gated
/// call site with a resource argument (`fs`, `net` and `process`, checked
/// against the module's compiled-in allowlist exactly as the C backend checks
/// them), and the 3 owned-temporary shapes (`String#into_bytes` through the
/// runtime object's pointer-spelled adapter, and a field or a method receiver
/// on an owned temporary, staged and dropped at the expression that read
/// through it). The sweep records no target that builds and then disagrees
/// with the C backend, and no target that fails without a named refusal.
///
/// This number may only rise. A slice that lowers a construct turns refusals
/// into built targets, so the count changes and the constant is raised in the
/// same commit; a slice that adds a fixture the backend cannot build yet leaves
/// it alone. A drop is a regression in coverage, which is what the assertion
/// exists to catch. `LLVM_BUILT_BASELINE_RERECORD=1` prints the live count and
/// the list behind it.
///
/// The refusal ranking at the same commit, which is what is left before §4.1's
/// "the refusal ranking empty" can be claimed:
///
/// ```text
/// (empty)
/// ```
///
/// No refusal names a construct any more: `pd`, the capability check and the
/// two owned-temporary shapes all lower. The gate for this slice is
/// `kelpc/tests/run-llvm-pd-caps.sh`, which also pins the deny path's text
/// against the C backend's own.
const LLVM_BUILT_BASELINE: usize = 223;

/// One compilable input, named the way the failure messages and the artifact
/// paths name it.
struct Target {
    /// The kind and the name, for example `fixture_if_expr` or `project_shapes`.
    tag: String,
    /// A `.kp` file, or a project directory with a manifest.
    source: PathBuf,
}

/// The corpus, in a deterministic order so the ranking and the re-record list
/// read the same on every run.
fn targets() -> Vec<Target> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut targets = Vec::new();

    for (stem, source) in kp_files(&manifest.join("tests").join("fixtures")) {
        targets.push(Target {
            tag: format!("fixture_{stem}"),
            source,
        });
    }

    // A project directory is one compilable unit and the directory is what
    // `kelpc build` is handed, the way `compile_and_run.rs::assert_project_runs`
    // hands it. A directory without a manifest is a negative or import-only
    // fixture for another test, not a target of this sweep.
    let projects = manifest.join("tests").join("projects");
    let mut project_dirs: Vec<PathBuf> = std::fs::read_dir(&projects)
        .unwrap_or_else(|e| panic!("failed to read '{}': {e}", projects.display()))
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            path.join("coral.toml").is_file().then_some(path)
        })
        .collect();
    project_dirs.sort();
    for source in project_dirs {
        let name = source
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        targets.push(Target {
            tag: format!("project_{name}"),
            source,
        });
    }

    for (stem, source) in kp_files(&manifest.join("..").join("examples")) {
        targets.push(Target {
            tag: format!("example_{stem}"),
            source,
        });
    }

    targets
}

/// Every `.kp` file in `dir`, as `(stem, path)`, sorted by stem.
fn kp_files(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut found: Vec<(String, PathBuf)> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("failed to read '{}': {e}", dir.display()))
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("kp") {
                return None;
            }
            Some((path.file_stem()?.to_str()?.to_string(), path))
        })
        .collect();
    found.sort();
    found
}

fn kelpc_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_kelpc"))
}

/// Where one backend's artifact for one target is written, rooted in
/// `CARGO_TARGET_TMPDIR` the way `compile_and_run.rs::out_path` is, with the
/// backend and the level in the path so the two builds of one target, and two
/// levels' builds of one target, cannot race for one file. `slot` is the level
/// label [`level_slot`] makes.
fn out_path(slot: &str, backend: &str, tag: &str) -> PathBuf {
    let mut dir = PathBuf::from(
        std::env::var("CARGO_TARGET_TMPDIR")
            .unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().to_string()),
    );
    dir.push("kelpc_backend_coverage");
    dir.push(slot);
    dir.push(backend);
    std::fs::create_dir_all(&dir)
        .unwrap_or_else(|e| panic!("could not create '{}': {e}", dir.display()));
    dir.join(tag)
}

/// The directory label for a sweep at one level: the flag without its leading
/// dash, or `default` for the run that passes no flag (which is `-O2` too, but
/// is the run the suite has always made and stays distinguishable).
fn level_slot(level: Option<&str>) -> &'static str {
    match level {
        None => "default",
        Some("-O0") => "O0",
        Some("-O1") => "O1",
        Some("-O2") => "O2",
        Some("-O3") => "O3",
        Some("-Os") => "Os",
        Some(other) => panic!("level_slot has no label for '{other}'"),
    }
}

/// Builds one target with one backend, pinned by flag rather than by
/// environment so the sweep's two halves cannot disagree about which backend
/// produced which binary. `level` is the `-O` flag for a level sweep, or `None`
/// for the default run (which passes no flag, the way the suite always has).
fn build(backend: &str, source: &Path, bin: &Path, level: Option<&str>) -> Output {
    let mut command = Command::new(kelpc_bin());
    command
        .args(["build", &format!("--backend={backend}")])
        .arg(source)
        .arg("-o")
        .arg(bin);
    if let Some(level) = level {
        command.arg(level);
    }
    command
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn kelpc to build '{}': {e}", source.display()))
}

struct Run {
    code: Option<i32>,
    stdout: Vec<u8>,
}

fn run(bin: &Path) -> Run {
    let out = Command::new(bin)
        .output()
        .unwrap_or_else(|e| panic!("failed to run '{}': {e}", bin.display()));
    Run {
        code: out.status.code(),
        stdout: out.stdout,
    }
}

/// How a process ended, for a failure message: a clean exit code, or the signal
/// that killed it. `None` is the signal case (`ExitStatus::code`), which is
/// exactly the "it crashed" this file must not let pass as a refusal.
fn ending(status: std::process::ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("exit {code}"),
        None => match status.signal() {
            Some(signal) => format!("killed by signal {signal}"),
            None => "an unknown termination".to_string(),
        },
    }
}

/// The first line of a stderr, which is where a `kelpc` diagnostic starts. The
/// ranking groups on this and nothing below it, because the lines under a
/// diagnostic name the source position and would split one gap into many rows.
fn first_line(stderr: &[u8]) -> String {
    String::from_utf8_lossy(stderr)
        .lines()
        .next()
        .unwrap_or_default()
        .trim_end()
        .to_string()
}

/// Everything the sweep observes, computed once and shared by the heavy tests
/// (see `sweep`). Nothing here is asserted against on its own, because the same
/// data backs three tests and each one cares about a different field.
struct Sweep {
    /// Targets the C backend builds, sorted.
    c_built: Vec<String>,
    /// Targets the `llvm` backend also builds, sorted.
    llvm_built: Vec<String>,
    /// Targets the C backend rejects, which are outside the corpus definition.
    c_rejected: usize,
    /// `(target, first stderr line)` for every target the C backend builds and
    /// the `llvm` backend refuses.
    refusals: Vec<(String, String)>,
    /// One line per build or run that died from a signal, or that failed without
    /// a diagnostic, on either backend.
    crashes: Vec<String>,
    /// One multi-line entry per target both backends build but disagree on.
    disagreements: Vec<String>,
}

/// The sweep, run at most once per process. The heavy tests all need the same
/// expensive data and libtest runs tests in parallel threads in one process, so
/// this is what keeps `--ignored` from compiling the corpus three times.
///
/// This is the default run, with no `-O` flag: it is what the suite has always
/// measured, and the level sweeps below are separate runs so their additions do
/// not change it.
fn sweep() -> &'static Sweep {
    static SWEEP: OnceLock<Sweep> = OnceLock::new();
    SWEEP.get_or_init(|| run_sweep(None))
}

fn run_sweep(level: Option<&str>) -> Sweep {
    let slot = level_slot(level);
    let mut c_built = Vec::new();
    let mut llvm_built = Vec::new();
    let mut c_rejected = 0;
    let mut refusals = Vec::new();
    let mut crashes = Vec::new();
    let mut disagreements = Vec::new();

    for target in targets() {
        let tag = target.tag;

        let c_bin = out_path(slot, "c", &tag);
        let c_build = build("c", &target.source, &c_bin, level);
        if !c_build.status.success() {
            c_rejected += 1;
            // The C backend is the oracle and the corpus definition, so a crash
            // here would mean the corpus is not defined; it is a failure rather
            // than a skip, because skipping it would quietly shrink the sweep.
            if c_build.status.code().is_none() {
                crashes.push(format!(
                    "the 'c' backend was {} while building '{tag}'",
                    ending(c_build.status)
                ));
            }
            continue;
        }
        c_built.push(tag.clone());

        let llvm_bin = out_path(slot, "llvm", &tag);
        let llvm_build = build("llvm", &target.source, &llvm_bin, level);
        if !llvm_build.status.success() {
            if llvm_build.status.code().is_none() {
                crashes.push(format!(
                    "the 'llvm' backend was {} while building '{tag}':\n{}",
                    ending(llvm_build.status),
                    String::from_utf8_lossy(&llvm_build.stderr).trim_end()
                ));
                continue;
            }
            let message = first_line(&llvm_build.stderr);
            // A failed build whose first line is not a diagnostic is a crash
            // reported as an exit code, and it must be as visible as a signal.
            if !message.starts_with("error:") {
                crashes.push(format!(
                    "the 'llvm' backend failed '{tag}' without a diagnostic ({}):\n{}",
                    ending(llvm_build.status),
                    String::from_utf8_lossy(&llvm_build.stderr).trim_end()
                ));
                continue;
            }
            refusals.push((tag, message));
            continue;
        }
        llvm_built.push(tag.clone());

        let c_run = run(&c_bin);
        let llvm_run = run(&llvm_bin);
        for (backend, finished) in [("c", &c_run), ("llvm", &llvm_run)] {
            if finished.code.is_none() {
                crashes.push(format!(
                    "the '{backend}' binary for '{tag}' was killed by a signal"
                ));
            }
        }
        if c_run.stdout != llvm_run.stdout {
            disagreements.push(format!(
                "'{tag}': the 'c' and 'llvm' backends print different stdout\n  \
                 c    ({}): {:?}\n  llvm ({}): {:?}",
                c_run
                    .code
                    .map_or("signal".to_string(), |c| format!("exit {c}")),
                String::from_utf8_lossy(&c_run.stdout),
                llvm_run
                    .code
                    .map_or("signal".to_string(), |c| format!("exit {c}")),
                String::from_utf8_lossy(&llvm_run.stdout),
            ));
        } else if c_run.code != llvm_run.code {
            disagreements.push(format!(
                "'{tag}': the 'c' and 'llvm' backends exit with status {:?} and {:?}, \
                 and print the same stdout {:?}",
                c_run.code,
                llvm_run.code,
                String::from_utf8_lossy(&c_run.stdout),
            ));
        }
    }

    Sweep {
        c_built,
        llvm_built,
        c_rejected,
        refusals,
        crashes,
        disagreements,
    }
}

/// The numbers §4.1 is measured from, printed by every heavy test so a run
/// always carries the state of the backend with it.
fn report(s: &Sweep) {
    println!(
        "llvm coverage sweep: {} of {} corpus targets built by the 'c' backend \
         ({} rejected by it), {} also built by the 'llvm' backend, {} refused, \
         {} mismatch(es), {} crash(es)",
        s.c_built.len(),
        s.c_built.len() + s.c_rejected,
        s.c_rejected,
        s.llvm_built.len(),
        s.refusals.len(),
        s.disagreements.len(),
        s.crashes.len(),
    );
}

// ---------------------------------------------------------------------------
// The differential sweep and the crash check.
// ---------------------------------------------------------------------------

/// The whole point of a second backend, run rather than argued: for every target
/// the C backend builds, the `llvm` backend either builds it too and agrees with
/// the C binary byte for byte on stdout and exit status, or refuses it with a
/// diagnostic. A mismatch is a failure with both outputs shown, because a wrong
/// literal and a wrong backend agree with each other and the fixture literals in
/// `compile_and_run.rs` cannot see that; the C backend can.
#[test]
#[ignore = "the sweep compiles the whole corpus twice; the default suite runs only the plumbing check"]
fn the_llvm_backend_agrees_with_the_c_backend_or_refuses_by_name() {
    let s = sweep();
    report(s);

    assert!(
        s.crashes.is_empty(),
        "{} build or run(s) crashed instead of failing with a diagnostic:\n{}",
        s.crashes.len(),
        s.crashes.join("\n"),
    );

    assert!(
        s.disagreements.is_empty(),
        "the two backends do not agree on {} of the {} targets both build:\n{}",
        s.disagreements.len(),
        s.llvm_built.len(),
        s.disagreements.join("\n\n"),
    );
}

// ---------------------------------------------------------------------------
// The coverage ratchet.
// ---------------------------------------------------------------------------

/// The recorded count may never shrink. A slice that removes coverage is a
/// regression, and the point of writing the number down is that the removal is
/// what fails a run rather than a number quietly dropping in the plan's tables.
///
/// Growing the count is fine and expected: growth is reported with the command
/// that re-records it. The failure direction is one-sided on purpose.
#[test]
#[ignore = "the sweep compiles the whole corpus twice; the default suite runs only the plumbing check"]
fn the_llvm_backend_builds_at_least_the_recorded_baseline() {
    let s = sweep();
    report(s);
    let built = s.llvm_built.len();

    if std::env::var_os(RERECORD_ENV).is_some() {
        println!("{RERECORD_ENV} is set: re-record LLVM_BUILT_BASELINE = {built}");
        for tag in &s.llvm_built {
            println!("  {tag}");
        }
        return;
    }

    assert!(
        built >= LLVM_BUILT_BASELINE,
        "the 'llvm' backend builds {built} corpus targets, down from the \
         {LLVM_BUILT_BASELINE} recorded at the S3 tip: coverage was removed, and if that \
         was deliberate the constant is the line to change in the same commit",
    );

    if built > LLVM_BUILT_BASELINE {
        println!(
            "note: the 'llvm' backend builds {built} targets, above the recorded \
             {LLVM_BUILT_BASELINE}; raise LLVM_BUILT_BASELINE and say which slice in its \
             comment (set {RERECORD_ENV}=1 to print the list)",
        );
    }
}

// ---------------------------------------------------------------------------
// The optimization-level sweep (MK7 S6, `plan/mk7-llvm-backend.md` §4.3).
// ---------------------------------------------------------------------------

/// Every level `-O` accepts, both backends, swept over the whole corpus.
///
/// A pass pipeline is the first thing in this project that can exploit IR Kelp
/// emitted without a licence for it (MK7 §7), so a level that changes a
/// program's stdout or exit status is a bug rather than a tuning choice, and the
/// same is true of a level whose C backend builds a target its LLVM backend
/// refuses. This is the gate for that claim.
///
/// Each level is its own sweep with the flag pinned on the command line, so the
/// C backend is the oracle *at that level* and the comparison is one level
/// against one level. `-O2` is the default too, but it is passed explicitly here
/// so its row cannot be confused with the no-flag run the tests above make.
#[test]
#[ignore = "the sweep compiles the whole corpus twice per level; the default suite runs only the plumbing check"]
fn every_level_agrees_with_the_c_backend() {
    for level in ["-O0", "-O1", "-O2", "-O3", "-Os"] {
        let s = run_sweep(Some(level));
        report_at(level, &s);
        assert!(
            s.crashes.is_empty(),
            "at {level}, {} build or run(s) crashed instead of failing with a \
             diagnostic:\n{}",
            s.crashes.len(),
            s.crashes.join("\n"),
        );
        assert!(
            s.disagreements.is_empty(),
            "at {level}, the two backends do not agree on {} of the {} targets both \
             build:\n{}",
            s.disagreements.len(),
            s.llvm_built.len(),
            s.disagreements.join("\n\n"),
        );
        assert!(
            s.refusals.is_empty(),
            "at {level}, the 'llvm' backend refused {} target(s) the 'c' backend \
             built:\n{}",
            s.refusals.len(),
            s.refusals
                .iter()
                .map(|(tag, message)| format!("  {tag}: {message}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }
}

/// The same numbers as [`report`], with the level named, so a run of the level
/// sweep reads as a table.
fn report_at(level: &str, s: &Sweep) {
    println!(
        "llvm coverage sweep at {level}: {} of {} corpus targets built by the 'c' \
         backend ({} rejected by it), {} also built by the 'llvm' backend, {} refused, \
         {} mismatch(es), {} crash(es)",
        s.c_built.len(),
        s.c_built.len() + s.c_rejected,
        s.c_rejected,
        s.llvm_built.len(),
        s.refusals.len(),
        s.disagreements.len(),
        s.crashes.len(),
    );
}

// ---------------------------------------------------------------------------
// The ranking report.
// ---------------------------------------------------------------------------

/// Prints the refusals grouped by their exact message, most common first, with
/// the targets behind each row and the slice the message names. This is the
/// report §4.1's "what is next, in the order the refusals rank themselves" is
/// measured from, and it is a print on purpose: a refusal whose wording changes
/// moves rows, and that must not be able to break a run.
///
/// `--nocapture` is what makes this visible; without it the harness swallows
/// stdout and the report never reaches the terminal.
#[test]
#[ignore = "the sweep compiles the whole corpus twice; the default suite runs only the plumbing check"]
fn the_refusals_rank_by_their_message() {
    let s = sweep();
    report(s);

    let mut by_message: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (tag, message) in &s.refusals {
        by_message.entry(message).or_default().push(tag);
    }
    let mut groups: Vec<(&str, Vec<&str>)> = by_message.into_iter().collect();
    // Most refusals first, then the message, so the order is total and stable.
    groups.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(b.0)));

    println!(
        "'llvm' backend refusal ranking ({} refused of the {} targets the 'c' backend builds)",
        s.refusals.len(),
        s.c_built.len(),
    );
    for (message, tags) in &groups {
        println!("  {:>3}  {message}", tags.len());
        for tag in tags {
            println!("         {tag}");
        }
    }

    let unnamed: Vec<&str> = groups
        .iter()
        .filter(|(message, _)| !message.starts_with(REFUSAL_PREFIX))
        .map(|(message, _)| *message)
        .collect();
    if !unnamed.is_empty() {
        println!(
            "note: {} message(s) do not carry the by-name refusal prefix '{REFUSAL_PREFIX}'; \
             they are compiler bugs rather than named gaps, and the rows above name them:",
            unnamed.len(),
        );
        for message in unnamed {
            println!("  {message}");
        }
    }
}

// ---------------------------------------------------------------------------
// The cheap plumbing check, for the default suite.
// ---------------------------------------------------------------------------

/// Builds one pinned fixture with both backends and compares it, so a change to
/// this file's own helpers breaks the default suite instead of the explicit
/// sweep. `if_expr.kp` is the first fixture the S1 subset gate names, so a
/// failure here is a real failure and not a corpus choice.
#[test]
fn the_differential_helpers_agree_on_one_pinned_fixture() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("if_expr.kp");
    let c_bin = out_path("default", "c", "plumbing_if_expr");
    let llvm_bin = out_path("default", "llvm", "plumbing_if_expr");

    let c_build = build("c", &source, &c_bin, None);
    assert!(
        c_build.status.success(),
        "'if_expr.kp' was rejected by the 'c' backend:\n{}",
        String::from_utf8_lossy(&c_build.stderr),
    );
    let llvm_build = build("llvm", &source, &llvm_bin, None);
    assert!(
        llvm_build.status.success(),
        "'if_expr.kp' was refused by the 'llvm' backend:\n{}",
        String::from_utf8_lossy(&llvm_build.stderr),
    );

    let c_run = run(&c_bin);
    let llvm_run = run(&llvm_bin);
    assert_eq!(
        c_run.code, llvm_run.code,
        "'if_expr.kp' exits with different statuses on the two backends",
    );
    assert_eq!(
        c_run.stdout, llvm_run.stdout,
        "'if_expr.kp' prints different stdout on the two backends",
    );
}
