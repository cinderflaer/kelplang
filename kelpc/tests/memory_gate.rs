//! The valgrind memory gate (MK7 §5's definition of done).
//!
//! The AddressSanitizer gate in `compile_and_run.rs`
//! (`heap_fixtures_are_double_free_and_leak_clean`) rebuilds the *generated C*
//! with `cc -fsanitize=address`, so it has nothing to instrument once a backend
//! writes its own object instead of C. Valgrind runs any binary, so it is the
//! gate a backend that emits its own code can actually be judged by.
//!
//! What one sweep does, for every `tests/fixtures/*.kp`:
//!
//! - build the fixture through the project's internal oracle switch, which is
//!   the only way to reach the C backend now. A fixture the C backend itself
//!   rejects has no binary and is skipped.
//! - run each binary under valgrind with leak checking and error detection
//!   on: `--leak-check=full --show-leak-kinds=all
//!   --errors-for-leak-kinds=definite,indirect,possible`. The program's stdin
//!   is `/dev/null`, its stdout is discarded, its stderr goes to a file of its
//!   own, and **valgrind's diagnostics go to `--log-file`**. Only the log is
//!   parsed, which is what makes the distinction from a deliberately
//!   panicking fixture structural rather than conventional: such a fixture
//!   writes `panic: …` to its own stderr, and that is the program's output,
//!   never a memory finding. Nothing but valgrind can write to the log.
//! - assert that valgrind reported nothing, in **every process the log
//!   mentions**, not only the one it was launched on. A `fork()`ed child
//!   inherits valgrind's attachment and writes into the same log under its own
//!   pid, so a child that leaks or touches freed memory is as much a finding
//!   as the parent is; the report names the pid, because "the program leaked"
//!   and "a child it forked leaked" are different bugs. Exactly one such
//!   finding is in this tree's history: `process_spawn.kp` ends with a run of
//!   a program that does not exist, and the child reached `execvp`, failed,
//!   and `_exit(127)`ed with the `char*[]` it had built for `execvp` still
//!   allocated. AddressSanitizer could not see it — `_exit` bypasses
//!   LeakSanitizer — and this gate did; it is fixed in `dbaabc2`.
//!
//! **Ground rule 1.5**: nothing may make the
//! default `cargo test` slower or needier. A sweep builds ~130 fixtures and
//! runs each under valgrind, which is about a minute, and valgrind is not a
//! declared dependency of this repository. Every test that runs the sweep is
//! therefore `#[ignore]`d, and the default run only checks that valgrind is
//! discoverable and prints what the sweep would cover. Run the sweep explicitly:
//!
//! ```text
//! cargo test --test memory_gate -- --ignored --nocapture
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The valgrind executable this gate shells out to. Deliberately not a
/// declared dependency: a machine without it skips the sweep with a printed
/// note, so this file adds nothing to install.
const VALGRIND: &str = "valgrind";

/// A run that has not finished in this long is killed and reported as a
/// finding: the sweep has to terminate, and a fixture that hangs only under
/// valgrind (which is 25-50x slower than native) is something to look at
/// rather than to wait for. Every fixture in the corpus finishes each run in
/// well under a second.
const RUN_TIMEOUT: Duration = Duration::from_secs(60);

fn kelpc_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_kelpc"))
}

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

/// Where the sweep's binaries, valgrind logs and program-stderr files are
/// written. Rooted in `CARGO_TARGET_TMPDIR` the way `compile_and_run.rs`'s
/// `out_path` is.
fn scratch_dir() -> PathBuf {
    let mut p = PathBuf::from(std::env::var("CARGO_TARGET_TMPDIR").unwrap_or_else(|_| {
        std::env::temp_dir().to_string_lossy().to_string()
    }));
    p.push("memory_gate");
    p
}

/// `valgrind --version`, or `None` when it is not on PATH.
fn valgrind_version() -> Option<String> {
    let out = Command::new(VALGRIND).arg("--version").output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Every fixture name (file stem) in `tests/fixtures`, sorted.
fn fixture_names() -> Vec<String> {
    let dir = fixture_dir();
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("failed to read '{}': {e}", dir.display()))
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            if path.extension()? != "kp" {
                return None;
            }
            path.file_stem()?.to_str().map(str::to_string)
        })
        .collect();
    names.sort();
    names
}

/// Builds one fixture with one backend, pinned by the oracle switch rather than
/// by the retired `KELPC_BACKEND`: `c` sets the switch and any other name is the
/// default `llvm` backend with the switch removed. The retired variable is
/// stripped in either case, so an inherited value cannot turn a build into its
/// refusal and cannot change what this file measures.
fn build(backend: &str, name: &str, bin: &Path) -> bool {
    let source = fixture_dir().join(format!("{name}.kp"));
    let mut cmd = Command::new(kelpc_bin());
    cmd.arg("build");
    cmd.env_remove("KELPC_BACKEND");
    if backend == "c" {
        cmd.env("KELPC_ORACLE", "c");
    } else {
        cmd.env_remove("KELPC_ORACLE");
    }
    let out = cmd
        .arg(&source)
        .arg("-o")
        .arg(bin)
        .output()
        .unwrap_or_else(|e| {
            panic!(
                "failed to spawn kelpc to build '{}': {e}",
                source.display()
            )
        });
    out.status.success()
}

/// The leak kinds valgrind's `LEAK SUMMARY` reports, in bytes.
#[derive(Clone, Copy, Default)]
struct Leaks {
    definite: u64,
    indirect: u64,
    possible: u64,
    /// Not a leak this program caused: glibc's own one-time allocations
    /// (stdout's buffer, the locale tables) are still reachable at exit in
    /// every C-backend run, and valgrind does not count them as errors.
    still_reachable: u64,
}

impl Leaks {
    /// The kinds this gate refuses to tolerate.
    fn lost(self) -> u64 {
        self.definite + self.indirect + self.possible
    }

    fn accumulate(&mut self, other: Leaks) {
        self.definite += other.definite;
        self.indirect += other.indirect;
        self.possible += other.possible;
        self.still_reachable += other.still_reachable;
    }
}

/// Valgrind prefixes every line of its log with the pid the line belongs to.
/// `None` for a line that carries no such prefix.
fn split_pid(line: &str) -> Option<(u32, &str)> {
    let rest = line.strip_prefix("==")?;
    let (pid, rest) = rest.split_once("==")?;
    Some((pid.parse().ok()?, rest.trim_start()))
}

/// The `N` in `ERROR SUMMARY: N errors from M contexts`.
fn error_count(line: &str) -> Option<u64> {
    let at = line.find("ERROR SUMMARY:")? + "ERROR SUMMARY:".len();
    line[at..].trim_start().split(' ').next()?.parse().ok()
}

/// One `LEAK SUMMARY` field, e.g. `   definitely lost: 0 bytes in 0 blocks`.
fn leak_counts(line: &str) -> Option<(u8, u64)> {
    /// `definite`, `indirect`, `possible`, `still_reachable` — the order
    /// `Leaks::accumulate` reads them in.
    const KINDS: [(u8, &str); 4] = [
        (0, "definitely lost:"),
        (1, "indirectly lost:"),
        (2, "possibly lost:"),
        (3, "still reachable:"),
    ];
    let line = line.trim_start();
    let (kind, rest) = KINDS
        .iter()
        .find_map(|(kind, prefix)| Some((*kind, line.strip_prefix(prefix)?)))?;
    let number = rest.trim().split(' ').next()?.replace(',', "");
    Some((kind, number.parse().ok()?))
}

/// One process valgrind wrote about, with the pid prefix stripped from its
/// lines.
struct Process {
    pid: u32,
    /// The process valgrind was launched on — the artifact under test — as
    /// opposed to a child it forked.
    launched: bool,
    lines: Vec<String>,
}

impl Process {
    /// The pid's error count, or `None` when valgrind wrote no
    /// `ERROR SUMMARY` line for it (a pid whose only lines are a note).
    fn errors(&self) -> Option<u64> {
        self.lines.iter().find_map(|line| error_count(line))
    }

    fn leaks(&self) -> Leaks {
        let mut leaks = Leaks::default();
        for line in &self.lines {
            if let Some((kind, bytes)) = leak_counts(line) {
                match kind {
                    0 => leaks.definite += bytes,
                    1 => leaks.indirect += bytes,
                    2 => leaks.possible += bytes,
                    _ => leaks.still_reachable += bytes,
                }
            }
        }
        leaks
    }

    /// Valgrind's own record headers and summaries, with the stack frames and
    /// the `still reachable` per-record headers dropped: an `at 0x…`/`by 0x…`
    /// line says where a block was allocated or freed, not what happened, and
    /// each `N bytes in M blocks are still reachable in loss record …` line is
    /// one of glibc's own allocations, which the `LEAK SUMMARY` below it
    /// already totals and which no process is ever failed for. What is left is
    /// the fixture name, which process it belongs to, and valgrind's own account
    /// of what that process did wrong.
    fn report(&self) -> Vec<String> {
        self.lines
            .iter()
            .filter(|line| {
                let line = line.trim();
                !line.is_empty()
                    && !line.starts_with("at 0x")
                    && !line.starts_with("by 0x")
                    && !line.contains("are still reachable in loss record")
            })
            .cloned()
            .collect()
    }
}

/// One binary run under valgrind, with everything valgrind said about it.
struct Check {
    fixture: String,
    backend: &'static str,
    /// What the program exited with, if it exited rather than being killed.
    /// Never asserted on: a fixture that panics on purpose is allowed to exit
    /// non-zero, and its own `panic: …` stderr text is its output.
    exit: Option<i32>,
    /// Set when valgrind had to be killed for exceeding [`RUN_TIMEOUT`].
    timed_out: bool,
    /// Valgrind's own diagnostics, verbatim. The only thing parsed.
    log: String,
    /// The program's own stderr, verbatim. Read for the summary (a panicking
    /// fixture is visible here) and never parsed as a finding: `--log-file`
    /// keeps valgrind's diagnostics off this stream entirely.
    program_stderr: String,
}

impl Check {
    /// Valgrind's log split by pid, in the order the pids appear. The first
    /// entry is the process valgrind was launched on; any later one is a child
    /// the artifact forked and valgrind stayed attached to. Every one is
    /// checked, and the report says which one a finding belongs to.
    fn processes(&self) -> Vec<Process> {
        let mut order: Vec<u32> = Vec::new();
        let mut by_pid: BTreeMap<u32, Vec<String>> = BTreeMap::new();
        let mut unattributed: Vec<String> = Vec::new();
        for line in self.log.lines() {
            match split_pid(line) {
                Some((pid, rest)) => {
                    if !by_pid.contains_key(&pid) {
                        order.push(pid);
                    }
                    by_pid.entry(pid).or_default().push(rest.to_string());
                }
                None => unattributed.push(line.to_string()),
            }
        }
        // A line valgrind did not attribute to a pid belongs to the run, so it
        // is reported with the process it was launched on rather than dropped.
        if let Some(first) = order.first().copied() {
            let mut merged = unattributed;
            merged.extend(by_pid.remove(&first).unwrap_or_default());
            by_pid.insert(first, merged);
        }
        order
            .iter()
            .enumerate()
            .map(|(i, pid)| Process {
                pid: *pid,
                launched: i == 0,
                lines: by_pid.remove(pid).unwrap_or_default(),
            })
            .collect()
    }

    /// What valgrind reported, as `(pid, valgrind's own lines)` per offending
    /// process. Empty means it reported nothing anywhere.
    ///
    /// A run is clean when it finished, the log carries at least one
    /// `ERROR SUMMARY`, every summary says zero errors, and no process lost
    /// memory. Checking every summary line rather than only the launched pid's
    /// is what makes a fork child's report a finding too.
    fn findings(&self) -> Vec<(Option<u32>, Vec<String>)> {
        if self.timed_out {
            return vec![(
                None,
                vec![format!(
                    "valgrind was killed after {}s without the program finishing",
                    RUN_TIMEOUT.as_secs()
                )],
            )];
        }
        if !self.log.lines().any(|line| line.contains("ERROR SUMMARY")) {
            return vec![(
                None,
                vec![
                    "valgrind wrote no 'ERROR SUMMARY' line, so the run did not complete"
                        .to_string(),
                ],
            )];
        }
        let mut findings = Vec::new();
        for process in self.processes() {
            let errors = process.errors().unwrap_or(0);
            let leaks = process.leaks();
            if errors == 0 && leaks.lost() == 0 {
                continue;
            }
            let role = if process.launched {
                "the process valgrind was launched on"
            } else {
                "a child it forked"
            };
            findings.push((
                Some(process.pid),
                std::iter::once(format!(
                    "{errors} error(s), {} leak byte(s) in pid {} ({role})",
                    leaks.lost(),
                    process.pid,
                ))
                .chain(process.report())
                .collect(),
            ));
        }
        findings
    }
}

/// One binary run under valgrind: the program's stdin on `/dev/null`, its
/// stdout discarded, its stderr in its own file, and valgrind's diagnostics in
/// its own log file.
fn run_under_valgrind(backend: &'static str, name: &str, bin: &Path, dir: &Path) -> Check {
    let log_path = dir.join(format!("{backend}_{name}.vglog"));
    let stderr_path = dir.join(format!("{backend}_{name}.stderr"));
    let stderr_file = std::fs::File::create(&stderr_path)
        .unwrap_or_else(|e| panic!("failed to create '{}': {e}", stderr_path.display()));

    let mut child = Command::new(VALGRIND)
        .arg("--leak-check=full")
        .arg("--show-leak-kinds=all")
        .arg("--errors-for-leak-kinds=definite,indirect,possible")
        .arg(format!("--log-file={}", log_path.display()))
        .arg(bin)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr_file))
        .spawn()
        .unwrap_or_else(|e| panic!("failed to spawn valgrind on '{}': {e}", bin.display()));

    let deadline = Instant::now() + RUN_TIMEOUT;
    let mut timed_out = false;
    let exit = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    timed_out = true;
                    break None;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => panic!("failed to poll valgrind on '{}': {e}", bin.display()),
        }
    };

    Check {
        fixture: name.to_string(),
        backend,
        exit,
        timed_out,
        log: std::fs::read_to_string(&log_path).unwrap_or_default(),
        program_stderr: std::fs::read_to_string(&stderr_path).unwrap_or_default(),
    }
}

/// Everything one sweep measured.
struct Sweep {
    /// Fixtures the C backend builds, each run under valgrind.
    c_checked: Vec<Check>,
    /// Fixtures the C backend itself rejects: there is no binary to check.
    c_rejected: usize,
    /// How many `*.kp` files the sweep looked at.
    fixtures: usize,
    elapsed: Duration,
}

fn run_sweep() -> Sweep {
    let started = Instant::now();
    let names = fixture_names();
    let dir = scratch_dir();
    std::fs::create_dir_all(&dir)
        .unwrap_or_else(|e| panic!("failed to create '{}': {e}", dir.display()));

    let mut c_checked = Vec::new();
    let mut c_rejected = 0;

    for name in &names {
        let c_bin = dir.join(format!("c_{name}"));
        if !build("c", name, &c_bin) {
            // The C backend is the oracle and the compile-error fixtures are
            // rejected by design, so there is no binary to run.
            c_rejected += 1;
            continue;
        }
        c_checked.push(run_under_valgrind("c", name, &c_bin, &dir));
    }

    Sweep {
        c_checked,
        c_rejected,
        fixtures: names.len(),
        elapsed: started.elapsed(),
    }
}

/// One line per backend: how many binaries were checked, how many fixtures were
/// skipped, and valgrind's own leak totals.
fn backend_summary(label: &str, checks: &[Check], skipped: &str) -> String {
    let mut leaks = Leaks::default();
    for check in checks {
        for process in check.processes() {
            leaks.accumulate(process.leaks());
        }
    }
    let findings: usize = checks.iter().map(|check| check.findings().len()).sum();
    format!(
        "  {label}: {} binaries checked{skipped}, {findings} finding(s); \
         leak bytes: definitely {}, indirectly {}, possibly {} \
         (still reachable {} — glibc's own, not an error)",
        checks.len(),
        leaks.definite,
        leaks.indirect,
        leaks.possible,
        leaks.still_reachable,
    )
}

fn print_summary(sweep: &Sweep, version: &str) {
    println!(
        "memory gate: {version} over kelpc/tests/fixtures ({} .kp files)",
        sweep.fixtures,
    );
    println!("{}", backend_summary("c", &sweep.c_checked, ""));
    println!(
        "  {} fixture(s) the C backend itself rejects were skipped: there is no binary to check",
        sweep.c_rejected,
    );
    for check in &sweep.c_checked {
        let children: Vec<Process> = check
            .processes()
            .into_iter()
            .filter(|process| !process.launched)
            .collect();
        for child in &children {
            println!(
                "  fork child of '{}' ({}): pid {}, {} error(s); checked, not excused",
                check.fixture,
                check.backend,
                child.pid,
                child.errors().unwrap_or(0),
            );
        }
    }
    let runs = sweep.c_checked.len();
    let non_zero = sweep
        .c_checked
        .iter()
        .filter(|check| check.exit != Some(0))
        .count();
    let panics = sweep
        .c_checked
        .iter()
        .filter(|check| check.program_stderr.starts_with("panic:"))
        .count();
    println!(
        "  program exits: {non_zero} of {runs} runs exited non-zero, and {panics} of those wrote \
         `panic: …` to their own stderr — the program's output, never parsed as a finding",
    );
    println!(
        "  wall clock: {:.1}s for {runs} valgrind runs over {} fixtures",
        sweep.elapsed.as_secs_f64(),
        sweep.fixtures,
    );
}

/// The gate itself: every fixture the C backend builds, each run under
/// valgrind. A leak, a double free, an invalid read or an invalid write, in the
/// process valgrind was launched on or in a child it forked, fails this test,
/// and the failure quotes valgrind's own lines for it.
///
/// Run it with `cargo test --test memory_gate -- --ignored --nocapture`; without
/// `--nocapture` the harness swallows the summary.
#[test]
#[ignore = "the sweep builds ~130 fixtures and runs each under valgrind; ground rule 1.5 keeps it out of the default suite"]
fn no_fixture_leaks_or_touches_freed_memory_under_valgrind() {
    let Some(version) = valgrind_version() else {
        println!(
            "memory gate: valgrind is not on PATH, so the sweep is skipped — install valgrind to \
             check the binaries' drops for leaks, double frees and invalid accesses"
        );
        return;
    };

    let sweep = run_sweep();
    print_summary(&sweep, &version);

    assert!(
        !sweep.c_checked.is_empty(),
        "the C backend built no fixture at all, so the sweep measured nothing",
    );

    let mut report = Vec::new();
    let mut findings = 0;
    for check in &sweep.c_checked {
        for (pid, lines) in check.findings() {
            findings += 1;
            match pid {
                Some(pid) => report.push(format!("{} '{}' (pid {pid}):", check.backend, check.fixture)),
                None => report.push(format!("{} '{}':", check.backend, check.fixture)),
            }
            report.extend(lines.into_iter().map(|line| format!("    {line}")));
        }
    }
    assert!(
        report.is_empty(),
        "valgrind reported {findings} finding(s) over {} runs:\n{}",
        sweep.c_checked.len(),
        report.join("\n"),
    );
}

// ---------------------------------------------------------------------------
// The cheap half, so the default (non-`--ignored`) run compiles and exercises
// this file without any of the sweep's cost.
// ---------------------------------------------------------------------------

/// The container fixtures the LLVM backend builds, the ones whose binaries the
/// monomorphized `List`/`Map`/`Box`/`[T; N]` function sets generated into the
/// module (MK7 S2D) own heap memory for. A fixed list, not a sweep: only these
/// fixtures exercise those sets, and a fixture that uses a container but also
/// calls `std/mem` or reads a `pd` is refused by the backend (S3) rather than
/// built, so it would be skipped anyway.
const LLVM_CONTAINER_FIXTURES: &[&str] = &[
    "array_basic",
    "array_bounds",
    "array_bounds_set",
    "array_owned",
    "array_wide_and_float",
    "box_basic",
    "box_empty_panic",
    "box_field_take",
    "box_owned_elements",
    "box_recursive",
    "container_element_from_use",
    "enum_container",
    "enum_in_containers",
    "enum_recursive",
    "float_datac_containers",
    "generic_recursive",
    "list_basic",
    "list_datac_element",
    "list_element_widths",
    "list_float_element",
    "list_growth_and_rebuild",
    "list_insert_bounds",
    "list_insert_remove_shifts",
    "list_narrow_element_pop",
    "list_owned_replacement",
    "list_pop_empty",
    "list_remove_bounds",
    "list_slice_insert",
    "list_sort_kinds",
    "map_clear_then_reuse",
    "map_growth_and_rehash",
    "map_key_types",
    "map_key_widths",
    "map_missing_key",
    "map_remove_missing",
    "map_tombstones_and_reuse",
    "nested_literal_staging",
    "recursive_datac",
    "stdlib_sort",
    // A-11: a partial move clears the field's slot in generated IR, so the
    // binding's own generated drop frees the field that stayed and nothing
    // where the moved value now lives. The C sweep covers the oracle; these
    // three are here because the clear and the drop are IR this backend emits.
    "partial_move_field",
    "partial_move_conditional",
    "partial_move_reassign",
];

/// The S3A extern fixtures: the ones whose heap an `extern fun` body allocates,
/// converts or adopts. `extern_signatures` is this slice's own fixture (a
/// `Bytes` from `mem.alloc`, a `String` from `fmt.hex`/`str.from_f64` and one
/// from a parsed path), `path_normalize` is the body that calls into `std/rt`,
/// and the rest are the stdlib paths that reach `String.from_bytes` (the
/// 24-byte-by-value entry point the runtime object carries a pointer-spelled
/// adapter for, MK7 section 3.3).
const LLVM_EXTERN_FIXTURES: &[&str] = &[
    "extern_signatures",
    "path_normalize",
    "stdlib_build",
    "stdlib_fmt",
    "stdlib_math",
    "stdlib_mem",
    "stdlib_str",
    "string_views",
];

/// The S3A half of the memory gate: every extern-body binary the LLVM backend
/// builds, run under valgrind. The bodies themselves are C and the C sweep
/// covers them, but the *calls* to them, the `String.from_bytes` adapter and the
/// buffers the bodies allocate and adopt move memory across that boundary, so
/// this is where a buffer freed twice or handed over without an owner shows up.
#[test]
#[ignore = "builds the extern fixtures and runs each under valgrind; ground rule 1.5 keeps it out of the default suite"]
fn the_llvm_extern_binaries_are_valgrind_clean() {
    let Some(version) = valgrind_version() else {
        println!(
            "memory gate: valgrind is not on PATH, so the LLVM extern sweep is skipped"
        );
        return;
    };

    let dir = scratch_dir();
    std::fs::create_dir_all(&dir)
        .unwrap_or_else(|e| panic!("failed to create '{}': {e}", dir.display()));

    let mut checked = Vec::new();
    let mut rejected = Vec::new();
    for name in LLVM_EXTERN_FIXTURES {
        let bin = dir.join(format!("llvm_{name}"));
        if build("llvm", name, &bin) {
            checked.push(run_under_valgrind("llvm", name, &bin, &dir));
        } else {
            rejected.push(*name);
        }
    }

    println!(
        "memory gate: {version} over {} LLVM extern fixture(s); {} refused by the backend and skipped",
        checked.len(),
        rejected.len(),
    );
    println!("{}", backend_summary("llvm extern", &checked, ""));

    assert!(
        checked.len() == LLVM_EXTERN_FIXTURES.len(),
        "the LLVM backend refused {:?}, which this slice must build",
        rejected,
    );

    let mut report = Vec::new();
    let mut findings = 0;
    for check in &checked {
        for (pid, lines) in check.findings() {
            findings += 1;
            match pid {
                Some(pid) => report.push(format!("{} '{}' (pid {pid}):", check.backend, check.fixture)),
                None => report.push(format!("{} '{}':", check.backend, check.fixture)),
            }
            report.extend(lines.into_iter().map(|line| format!("    {line}")));
        }
    }
    assert!(
        report.is_empty(),
        "valgrind reported {findings} finding(s) over {} LLVM extern runs:\n{}",
        checked.len(),
        report.join("\n"),
    );
}

/// The S2D half of the memory gate: every container binary the LLVM backend
/// builds, run under valgrind. The C sweep above covers the oracle; this one
/// covers the code the LLVM backend generates *instead* of C, which is the
/// part another backend cannot check for it: a container's drop, its growth
/// (`realloc`), a map's tombstone reuse and an `[T; N]`'s element drops all
/// free memory in generated IR, and only valgrind sees a double free or a leak
/// in it.
#[test]
#[ignore = "builds the container fixtures and runs each under valgrind; ground rule 1.5 keeps it out of the default suite"]
fn the_llvm_container_binaries_are_valgrind_clean() {
    let Some(version) = valgrind_version() else {
        println!(
            "memory gate: valgrind is not on PATH, so the LLVM container sweep is skipped"
        );
        return;
    };

    let dir = scratch_dir();
    std::fs::create_dir_all(&dir)
        .unwrap_or_else(|e| panic!("failed to create '{}': {e}", dir.display()));

    let mut checked = Vec::new();
    let mut rejected = Vec::new();
    for name in LLVM_CONTAINER_FIXTURES {
        let bin = dir.join(format!("llvm_{name}"));
        if build("llvm", name, &bin) {
            checked.push(run_under_valgrind("llvm", name, &bin, &dir));
        } else {
            rejected.push(*name);
        }
    }

    println!(
        "memory gate: {version} over {} LLVM container fixture(s); {} refused by the backend and skipped",
        checked.len(),
        rejected.len(),
    );
    println!("{}", backend_summary("llvm container", &checked, ""));

    assert!(
        !checked.is_empty(),
        "the LLVM backend built none of the container fixtures, so the sweep measured nothing",
    );

    let mut report = Vec::new();
    let mut findings = 0;
    for check in &checked {
        for (pid, lines) in check.findings() {
            findings += 1;
            match pid {
                Some(pid) => report.push(format!("{} '{}' (pid {pid}):", check.backend, check.fixture)),
                None => report.push(format!("{} '{}':", check.backend, check.fixture)),
            }
            report.extend(lines.into_iter().map(|line| format!("    {line}")));
        }
    }
    assert!(
        report.is_empty(),
        "valgrind reported {findings} finding(s) over {} LLVM container runs:\n{}",
        checked.len(),
        report.join("\n"),
    );
}

/// The S2E closure fixtures the LLVM backend builds: the ones a closure or a
/// function value gives heap to own. A capturing literal mallocs its
/// environment and the closure value owns it from then on, so a missed drop is
/// a leak and a second one a double free, and neither prints anything. The
/// widening fixtures add the two other allocations: a shim allocates nothing,
/// and a value whose identity the call knows gets a one-field heap environment
/// (`closure_value_widening` reaches both). `try_drops_frame`'s closure is one
/// the frame holds while `?` leaves it, so its environment is freed on the
/// early-return path.
const LLVM_CLOSURE_FIXTURES: &[&str] = &[
    "closure_datac_borrow_param",
    "closure_env_wide_ret",
    "closure_move",
    "closure_two_captures",
    "closure_value_widening",
    "fn_literal_capture",
    "fn_value_plain",
    "try_drops_frame",
];

/// The S2E half of the memory gate: every closure binary the LLVM backend
/// builds, run under valgrind. The C sweep above covers the oracle; this covers
/// the environment the LLVM backend generates the allocation, the capture stores
/// and the destructor for, which is exactly the code another backend cannot
/// check for it.
#[test]
#[ignore = "builds the closure fixtures and runs each under valgrind; ground rule 1.5 keeps it out of the default suite"]
fn the_llvm_closure_binaries_are_valgrind_clean() {
    let Some(version) = valgrind_version() else {
        println!(
            "memory gate: valgrind is not on PATH, so the LLVM closure sweep is skipped"
        );
        return;
    };

    let dir = scratch_dir();
    std::fs::create_dir_all(&dir)
        .unwrap_or_else(|e| panic!("failed to create '{}': {e}", dir.display()));

    let mut checked = Vec::new();
    let mut rejected = Vec::new();
    for name in LLVM_CLOSURE_FIXTURES {
        let bin = dir.join(format!("llvm_{name}"));
        if build("llvm", name, &bin) {
            checked.push(run_under_valgrind("llvm", name, &bin, &dir));
        } else {
            rejected.push(*name);
        }
    }

    println!(
        "memory gate: {version} over {} LLVM closure fixture(s); {} refused by the backend and skipped",
        checked.len(),
        rejected.len(),
    );
    println!("{}", backend_summary("llvm closure", &checked, ""));

    assert!(
        !checked.is_empty(),
        "the LLVM backend built none of the closure fixtures, so the sweep measured nothing",
    );

    let mut report = Vec::new();
    let mut findings = 0;
    for check in &checked {
        for (pid, lines) in check.findings() {
            findings += 1;
            match pid {
                Some(pid) => report.push(format!("{} '{}' (pid {pid}):", check.backend, check.fixture)),
                None => report.push(format!("{} '{}':", check.backend, check.fixture)),
            }
            report.extend(lines.into_iter().map(|line| format!("    {line}")));
        }
    }
    assert!(
        report.is_empty(),
        "valgrind reported {findings} finding(s) over {} LLVM closure runs:\n{}",
        checked.len(),
        report.join("\n"),
    );
}

/// The S3 `pd` and capability fixtures the LLVM backend builds: the ones that
/// give a module-scope global something to own, and the ones that run the
/// capability glue.
///
/// A `pd` is storage that outlives every frame, so its drop point is the
/// `atexit` handler the generated `kelp_init_pds` registers; a missed handler
/// leaks, and one that frees twice fails here rather than in production. The
/// capability glue allocates: `kelp_cap_check_path` builds a `PATH_MAX` scratch
/// buffer on the stack and the matcher runs over it, and the deny path exits
/// through glibc's `atexit` chain, which is where a `pd` drop and the deny path
/// meet. Everything here also prints nothing about memory, so valgrind is the
/// only instrument that can see it.
const LLVM_PD_CAP_FIXTURES: &[&str] = &[
    // The passdowns, including the owned one and the container-typed one.
    "passdowns",
    "passdown_datac",
    "passdown_globals",
    "pd_owned",
    "pd_owned_container",
    "pd_shadowed",
    "pd_argument_forwarding",
    "pd_var_write",
    "pd_write_reaches_callers",
    "pd_var_datac_reassign",
    "pd_var_copy_read_twice",
    "pd_var_mut_argument",
    "pd_var_read_only_datac",
    "pd_var_read_only_shared",
    "list_field",
    "for_loop",
    // A gated call with a resource argument, checked at the call site.
    "caps_time_rand",
    "fs_errno",
    "fs_fallible",
    "fs_path_family",
    "fs_symlink_kind",
    "io_stdio",
    "io_streams",
    "io_write_bodies",
    "net_loopback",
    "net_poll",
    "net_resolve",
    "net_timeout",
    "path_absolute",
    "process_spawn",
    // The last three refusals: the receiver adapter and the owned-temporary
    // staging.
    "string_basic",
    "heap_field_of_temporary",
    "heap_field_of_owned_temporary",
];

/// The S3 half of the memory gate: every `pd` and capability binary the LLVM
/// backend builds, run under valgrind. `pd` storage and the capability glue are
/// the two places this slice touches memory across a boundary the C sweep
/// cannot see: the `atexit` drop is registered by generated IR, and the check
/// calls into the C glue through the per-program object.
#[test]
#[ignore = "builds the passdown and capability fixtures and runs each under valgrind; ground rule 1.5 keeps it out of the default suite"]
fn the_llvm_passdown_and_capability_binaries_are_valgrind_clean() {
    let Some(version) = valgrind_version() else {
        println!(
            "memory gate: valgrind is not on PATH, so the LLVM passdown and capability sweep is skipped"
        );
        return;
    };

    let dir = scratch_dir();
    std::fs::create_dir_all(&dir)
        .unwrap_or_else(|e| panic!("failed to create '{}': {e}", dir.display()));

    let mut checked = Vec::new();
    let mut rejected = Vec::new();
    for name in LLVM_PD_CAP_FIXTURES {
        let bin = dir.join(format!("llvm_{name}"));
        if build("llvm", name, &bin) {
            checked.push(run_under_valgrind("llvm", name, &bin, &dir));
        } else {
            rejected.push(*name);
        }
    }

    println!(
        "memory gate: {version} over {} LLVM passdown/capability fixture(s); {} refused by the backend and skipped",
        checked.len(),
        rejected.len(),
    );
    println!("{}", backend_summary("llvm passdown/capability", &checked, ""));

    assert!(
        !checked.is_empty(),
        "the LLVM backend built none of the passdown or capability fixtures, so the sweep measured nothing",
    );

    let mut report = Vec::new();
    let mut findings = 0;
    for check in &checked {
        for (pid, lines) in check.findings() {
            findings += 1;
            match pid {
                Some(pid) => report.push(format!("{} '{}' (pid {pid}):", check.backend, check.fixture)),
                None => report.push(format!("{} '{}':", check.backend, check.fixture)),
            }
            report.extend(lines.into_iter().map(|line| format!("    {line}")));
        }
    }
    assert!(
        report.is_empty(),
        "valgrind reported {findings} finding(s) over {} LLVM passdown/capability runs:\n{}",
        checked.len(),
        report.join("\n"),
    );
}

/// Says what the ignored sweep would cover, and whether the tool it needs is
/// here. It costs one `valgrind --version` spawn and never fails: valgrind is
/// not a declared dependency of this repository, so a machine without it must
/// skip the gate rather than fail on it.
#[test]
fn the_sweep_reports_what_it_would_cover() {
    let fixtures = fixture_names().len();
    assert!(
        fixtures > 0,
        "'{}' holds no .kp fixture, so the sweep below would check nothing",
        fixture_dir().display(),
    );
    match valgrind_version() {
        Some(version) => println!(
            "memory gate: {version} is on PATH; the ignored sweep builds every one of these {fixtures} \
             fixtures with `--backend=c`, then runs each binary under `--leak-check=full`. Run it with:\n    \
             cargo test --test memory_gate -- --ignored --nocapture"
        ),
        None => println!(
            "memory gate: valgrind is not on PATH, so the ignored sweep is skipped; install valgrind \
             to check the binaries' drops for leaks, double frees and invalid accesses"
        ),
    }
}
