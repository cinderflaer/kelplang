//! The optimization-level differential of the `llvm` backend (stage 2 WP-2,
//! option B of `stage2-oracle-plan.md`). No C backend is involved.
//!
//! A pass pipeline is the first thing that can exploit IR Kelp emitted without
//! a licence for it, and it was the cause of two recorded bugs: a module with no
//! `target datalayout` (`heap_narrow_fields.kp` read 42 as `42 << 32` at `-O2`,
//! and `list_datac_clone_sort.kp` lost clone elements) and optimizer-visible
//! undefined behaviour. `-O0` runs no pass, so it cannot show either. This test
//! builds each corpus target at `-O0` and at `-O1`, `-O2`, `-O3` and `-Os`, runs
//! every build, and requires the same exit status, stdout and first stderr line
//! at every level. `-O0` is the reference.
//!
//! The corpus is the one `backend_coverage.rs` and `golden_corpus.rs` use. A
//! target the `llvm` backend refuses at `-O0` is recorded and skipped, because a
//! named refusal is not a level disagreement; a target it builds at `-O0` and
//! refuses at another level is a failure. A signal death at any level is a
//! failure.
//!
//! The targets in [`EXCLUDED`] are left out for the reason the golden set gives
//! (`golden_corpus.rs`). Keep the two lists the same.
//!
//! Tests:
//!
//! * `the_levels_agree_on_one_fixture` (default suite): one fixture at `-O0` and
//!   `-O2`, so the plumbing is exercised without the sweep's cost.
//! * `every_target_agrees_at_every_level` (`#[ignore]`): the sweep. Five builds
//!   and five runs per target. `LEVEL_FILTER=<substring>` restricts it to the
//!   targets whose tag contains the substring, for a partial run.
//!
//! ```text
//! cargo test --release -p kelpc --test level_differential -- --ignored --nocapture
//! ./kelpc/tests/run-llvm-levels.sh
//! ```

use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// The same exclusions as `golden_corpus.rs`, for the same reason.
const EXCLUDED: &[(&str, &str)] = &[
    ("fixture_float_format_edges", "prints the formatting of 0.0 / 0.0, whose sign bit the language leaves unspecified (plan/bugfix-pass.md:107-120, the NaN sign)"),
    ("fixture_trace", "its stderr carries a clock reading and the source path, which change from run to run"),
    ("fixture_http_try_send", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("fixture_https_try_dial", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("fixture_tls_bad_record", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("fixture_tls_big_body", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("fixture_tls_ca_file", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("fixture_tls_concurrent", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("fixture_tls_error", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("fixture_tls_handshake_no_timeout", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("fixture_tls_handshake_socket_timeout", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("fixture_tls_partial", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("fixture_tls_pool_close", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("fixture_tls_reset_handshake", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("fixture_tls_reset_read", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("fixture_tls_sni", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("fixture_tls_timeout_handshake", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("fixture_tls_timeout_read", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("fixture_tls_truncated", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("fixture_tls_try_ok", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("fixture_ws_gate", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("fixture_ws_try_connect", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("fixture_ws_wss", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("project_http_close_vs_keepalive", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("project_http_get", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("project_http_pool_off", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("project_http_rate_limit", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("project_http_redirect", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("project_http_redirect_denied", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("project_https_big_headers", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("project_https_get", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("project_https_get_denied", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
    ("project_https_redirect", "needs command-line arguments from a harness; run with none it records a deterministic index panic"),
];

/// The reference level, and the levels compared against it.
const REFERENCE: &str = "-O0";
const LEVELS: &[&str] = &["-O1", "-O2", "-O3", "-Os"];

/// The fixture the default-suite plumbing check builds. It is the `u8`/`u16`
/// narrowing regression, which is cheap and has a known answer at every level.
const PLUMBING_FIXTURE: &str = "int_narrow_width_arithmetic.kp";

// ---------------------------------------------------------------------------
// The corpus, as backend_coverage.rs enumerates it.
// ---------------------------------------------------------------------------

struct Target {
    tag: String,
    source: PathBuf,
}

fn targets() -> Vec<Target> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut targets = Vec::new();

    for (stem, source) in kp_files(&manifest.join("tests").join("fixtures")) {
        targets.push(Target {
            tag: format!("fixture_{stem}"),
            source,
        });
    }

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
            .unwrap_or_default()
            .to_string();
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

/// Where one build of one target at one level is written. The level label is
/// part of the path so two levels of one target cannot race for one file.
fn scratch_bin(level: &str, tag: &str) -> PathBuf {
    let mut dir = PathBuf::from(
        std::env::var("CARGO_TARGET_TMPDIR")
            .unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().to_string()),
    );
    dir.push("level_differential");
    dir.push(level.trim_start_matches('-'));
    std::fs::create_dir_all(&dir)
        .unwrap_or_else(|e| panic!("could not create '{}': {e}", dir.display()));
    dir.join(tag)
}

/// Builds one target with the `llvm` backend at one level. Both the retired
/// `KELPC_BACKEND` and the internal oracle switch are removed, so nothing
/// inherited from the environment can select the C backend.
fn build(source: &Path, bin: &Path, level: &str) -> Output {
    Command::new(kelpc_bin())
        .arg("build")
        .env_remove("KELPC_BACKEND")
        .env_remove("KELPC_ORACLE")
        .arg(source)
        .arg("-o")
        .arg(bin)
        .arg(level)
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn kelpc to build '{}': {e}", source.display()))
}

/// One run: how it ended, stdout, and the first stderr line. stdin is closed.
struct Observed {
    code: Option<i32>,
    signal: Option<i32>,
    stdout: Vec<u8>,
    stderr_first: String,
}

fn run(bin: &Path) -> Observed {
    let out = Command::new(bin)
        .stdin(Stdio::null())
        .output()
        .unwrap_or_else(|e| panic!("failed to run '{}': {e}", bin.display()));
    Observed {
        code: out.status.code(),
        signal: out.status.signal(),
        stdout: out.stdout,
        stderr_first: first_line(&out.stderr),
    }
}

impl Observed {
    fn ending(&self) -> String {
        match (self.code, self.signal) {
            (Some(code), _) => format!("exit {code}"),
            (None, Some(signal)) => format!("killed by signal {signal}"),
            (None, None) => "an unknown termination".to_string(),
        }
    }

    fn show(&self) -> String {
        format!(
            "{} stdout {:?} stderr {:?}",
            self.ending(),
            String::from_utf8_lossy(&self.stdout),
            self.stderr_first,
        )
    }
}

fn first_line(stderr: &[u8]) -> String {
    String::from_utf8_lossy(stderr)
        .lines()
        .next()
        .unwrap_or_default()
        .trim_end()
        .to_string()
}

fn excluded_reason(tag: &str) -> Option<&'static str> {
    EXCLUDED
        .iter()
        .find(|(t, _)| *t == tag)
        .map(|(_, reason)| *reason)
}

// ---------------------------------------------------------------------------
// The checks.
// ---------------------------------------------------------------------------

/// The cheap default-suite check: one fixture, built at `-O0` and `-O2` with
/// `llvm`, must behave the same. It proves the level plumbing and the reference
/// comparison without the sweep.
#[test]
fn the_levels_agree_on_one_fixture() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(PLUMBING_FIXTURE);
    let tag = "plumbing_int_narrow_width_arithmetic";

    let reference_bin = scratch_bin(REFERENCE, tag);
    let built = build(&source, &reference_bin, REFERENCE);
    assert!(
        built.status.success(),
        "llvm refused '{}' at {REFERENCE}:\n{}",
        source.display(),
        String::from_utf8_lossy(&built.stderr).trim_end(),
    );
    let reference = run(&reference_bin);

    let level = "-O2";
    let bin = scratch_bin(level, tag);
    let built = build(&source, &bin, level);
    assert!(
        built.status.success(),
        "llvm refused '{}' at {level}:\n{}",
        source.display(),
        String::from_utf8_lossy(&built.stderr).trim_end(),
    );
    let observed = run(&bin);

    assert!(
        reference.signal.is_none() && observed.signal.is_none(),
        "a binary was killed by a signal: {REFERENCE} {}; {level} {}",
        reference.ending(),
        observed.ending(),
    );
    assert!(
        reference.code == observed.code
            && reference.stdout == observed.stdout
            && reference.stderr_first == observed.stderr_first,
        "'{}' differs between {REFERENCE} and {level}:\n  {REFERENCE}: {}\n  {level}: {}",
        source.display(),
        reference.show(),
        observed.show(),
    );
}

/// The sweep: every corpus target, at every level, against `-O0`.
#[test]
#[ignore = "builds and runs the corpus at five optimization levels; run with --ignored"]
fn every_target_agrees_at_every_level() {
    let filter = std::env::var("LEVEL_FILTER").unwrap_or_default();
    let all = targets();
    let mut considered = 0usize;
    let mut reference_built = 0usize;
    let mut refused_at_reference: Vec<String> = Vec::new();
    let mut levels_checked = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for target in &all {
        let tag = &target.tag;
        if !filter.is_empty() && !tag.contains(&filter) {
            continue;
        }
        if let Some(reason) = excluded_reason(tag) {
            println!("skipped {tag}: {reason}");
            continue;
        }
        considered += 1;

        let reference_bin = scratch_bin(REFERENCE, tag);
        let built = build(&target.source, &reference_bin, REFERENCE);
        if !built.status.success() {
            if built.status.code().is_none() {
                failures.push(format!(
                    "'{tag}': the llvm build at {REFERENCE} was {}",
                    built.status.signal().map_or("killed".to_string(), |s| format!("killed by signal {s}")),
                ));
                continue;
            }
            // A named refusal at the reference level is the backend's known
            // partial surface, not a level disagreement.
            refused_at_reference.push(format!(
                "{tag}: {}",
                String::from_utf8_lossy(&built.stderr).lines().next().unwrap_or_default().trim_end()
            ));
            continue;
        }
        reference_built += 1;
        let reference = run(&reference_bin);
        if reference.signal.is_some() {
            failures.push(format!("'{tag}' at {REFERENCE} was {}", reference.ending()));
            continue;
        }

        for level in LEVELS {
            let bin = scratch_bin(level, tag);
            let built = build(&target.source, &bin, level);
            if !built.status.success() {
                failures.push(format!(
                    "'{tag}': llvm built it at {REFERENCE} but refused it at {level} ({}):\n{}",
                    built.status.code().map_or("signal".to_string(), |c| format!("exit {c}")),
                    String::from_utf8_lossy(&built.stderr).trim_end(),
                ));
                continue;
            }
            let observed = run(&bin);
            levels_checked += 1;

            if observed.signal.is_some() {
                failures.push(format!("'{tag}' at {level} was {}", observed.ending()));
                continue;
            }
            if observed.code != reference.code
                || observed.stdout != reference.stdout
                || observed.stderr_first != reference.stderr_first
            {
                failures.push(format!(
                    "'{tag}' differs between {REFERENCE} and {level}:\n  {REFERENCE}: {}\n  {level}: {}",
                    reference.show(),
                    observed.show(),
                ));
            }
        }
    }

    for line in &refused_at_reference {
        println!("refused at {REFERENCE} by llvm, skipped: {line}");
    }
    println!(
        "level differential: {considered} target(s) considered; {reference_built} built at \
         {REFERENCE}; {} refused at {REFERENCE} (skipped); {levels_checked} level build(s) \
         compared against {REFERENCE} ({}); {} failure(s)",
        refused_at_reference.len(),
        LEVELS.join(" "),
        failures.len(),
    );
    assert!(
        failures.is_empty(),
        "{} level disagreement(s) or failure(s):\n\n{}",
        failures.len(),
        failures.join("\n\n"),
    );
}
