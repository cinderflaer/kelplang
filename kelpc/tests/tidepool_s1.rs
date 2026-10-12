//! Tidepool step 1: the growable root table, the spawn, lock and result roots,
//! the spawn-side thread count, the counter line and the messages of section
//! 18.2, 18.3 and 18.24 (sections 9, 16.1 and 18.32). The fixtures build a
//! managed program and need the collector archive, so they are `#[ignore]`d and
//! run with `KELP_GC_LIB` set to `libkelp_gc.a` and `--ignored`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn kelpc() -> Command {
    Command::new(env!("CARGO_BIN_EXE_kelpc"))
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/managed/tidepool_s1").join(name)
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// Copies a fixture into a scratch directory and builds it as a managed program.
fn build(name: &str) -> PathBuf {
    let root = fixture(name);
    static CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let call = CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("kelpc-tidepool-s1-{name}-{}-{call}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).expect("create scratch dir");
    std::fs::copy(root.join("src/main.kp"), dir.join("src/main.kp")).expect("copy main.kp");
    std::fs::copy(root.join("coral.toml"), dir.join("coral.toml")).expect("copy coral.toml");
    let bin = dir.join(name);
    let out = kelpc()
        .arg("build")
        .arg(&dir)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(out.status.success(), "the managed build of {name} failed:\n{}", text(&out));
    bin
}

/// Runs a built program under the limits of the other managed tests, with a
/// 2000 ms stop timeout and the variables of `env`, and returns its output.
fn run_env(bin: &Path, env: &str) -> Output {
    run_env_limit(bin, env, 1048576)
}

/// As `run_env`, with an address-space limit of `kb` KiB. Each attached thread
/// reserves its default 8 MiB stack, so 256 open threads need about 2 GiB (C16).
fn run_env_limit(bin: &Path, env: &str, kb: u64) -> Output {
    let script = format!(
        "{env}ulimit -v {kb}; ulimit -t 10; KELP_GC_STOP_TIMEOUT_MS=2000 exec '{}'",
        bin.display()
    );
    // Two tests can build the same fixture at once, and a sibling thread's fork
    // can hold the binary open for writing for a moment. The exec then fails
    // with `Text file busy` (the shell exits 126), so retry a few times.
    let mut attempt = 0;
    loop {
        let out = Command::new("timeout")
            .arg("10")
            .arg("sh")
            .arg("-c")
            .arg(&script)
            .output()
            .expect("run managed program");
        let busy = out.status.code() == Some(126) && String::from_utf8_lossy(&out.stderr).contains("Text file busy");
        attempt += 1;
        if !busy || attempt >= 5 {
            return out;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

fn expected(name: &str) -> String {
    std::fs::read_to_string(fixture(name).join("expected.txt")).expect("read expected.txt")
}

fn run_and_compare(name: &str, env: &str) {
    assert!(std::env::var_os("KELP_GC_LIB").is_some(), "set KELP_GC_LIB to libkelp_gc.a");
    let bin = build(name);
    let out = run_env(&bin, env);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "{name}: exit {:?}, stderr: {stderr}", out.status);
    assert_eq!(stdout, expected(name), "{name}: stdout");
}

/// The spawner's pause hook runs a clearing while the worker has not started.
#[test]
#[ignore = "needs KELP_GC_LIB: the collector archive from kelpc/gc"]
fn t20_spawn_environment_and_argument_are_rooted() {
    run_and_compare("t20_spawn_env_and_arg_rooted", "export KELP_GC_TEST_SPAWN_PAUSE=1; ");
}

#[test]
#[ignore = "needs KELP_GC_LIB: the collector archive from kelpc/gc"]
fn t21_spawn_result_is_rooted_until_join() {
    run_and_compare("t21_spawn_result_rooted_until_join", "export KELP_GC_STRESS=20000 KELP_GC_VERIFY=1; ");
}

#[test]
#[ignore = "needs KELP_GC_LIB: the collector archive from kelpc/gc"]
fn t22_a_mutex_value_is_rooted() {
    run_and_compare("t22_mutex_value_rooted", "export KELP_GC_STRESS=20000 KELP_GC_VERIFY=1; ");
}

#[test]
#[ignore = "needs KELP_GC_LIB: the collector archive from kelpc/gc"]
fn t22_an_rwlock_value_is_rooted() {
    run_and_compare("t22_rwlock_value_rooted", "export KELP_GC_STRESS=20000 KELP_GC_VERIFY=1; ");
}

#[test]
#[ignore = "needs KELP_GC_LIB: the collector archive from kelpc/gc"]
fn t68_three_hundred_joined_threads_run_to_the_end() {
    run_and_compare("t68_joined_300", "");
}

/// The 257th open thread is refused by the spawner, with the spawner's message
/// and no thread-side fatal.
#[test]
#[ignore = "needs KELP_GC_LIB: the collector archive from kelpc/gc"]
fn t68_the_257th_open_thread_is_refused_in_the_spawner() {
    assert!(std::env::var_os("KELP_GC_LIB").is_some(), "set KELP_GC_LIB to libkelp_gc.a");
    let bin = build("t68_open_257");
    let out = run_env_limit(&bin, "", 8388608);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "panic: thread.spawn: 256 threads are already running; join or drop a thread before spawning another\n"
    );
}

/// Whether `line` has the form of section 17.7 (T55): the prefix, then three
/// unsigned fields in order. Other fields after them are allowed.
fn is_counter_line(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("kelp-gc-stats: collections=") else {
        return false;
    };
    let field = |rest: &str| -> Option<usize> {
        let end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
        if end == 0 {
            None
        } else {
            Some(end)
        }
    };
    let Some(n) = field(rest) else { return false };
    let Some(rest) = rest[n..].strip_prefix(" pause_total_us=") else { return false };
    let Some(n) = field(rest) else { return false };
    let Some(rest) = rest[n..].strip_prefix(" pause_max_us=") else { return false };
    field(rest).is_some()
}

/// T55 (R54): with KELP_GC_STATS=1 the program prints nothing else, and the
/// counter line is printed once, at exit, in the form of section 17.7.
#[test]
#[ignore = "needs KELP_GC_LIB: the collector archive from kelpc/gc"]
fn t55_the_counter_line_is_printed_at_exit() {
    assert!(std::env::var_os("KELP_GC_LIB").is_some(), "set KELP_GC_LIB to libkelp_gc.a");
    let bin = build("t55_stats_line");
    let out = run_env(&bin, "export KELP_GC_STATS=1; ");
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "true\n");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    let lines: Vec<&str> = stderr.lines().collect();
    assert_eq!(lines.len(), 1, "stderr must be exactly one counter line: {stderr}");
    assert!(is_counter_line(lines[0]), "not in the section 17.7 form: {}", lines[0]);
    assert!(lines[0].contains(" pause_total_us=") && lines[0].contains(" pause_max_us="), "{}", lines[0]);
    assert!(!lines[0].contains("collections=0 "), "no clearing was counted: {}", lines[0]);
}

/// T63 (18.2): the out-of-memory text carries the three numbers in order: the
/// heap_max, the live bytes at the last clearing, and the charged bytes, all in MiB.
#[test]
#[ignore = "needs KELP_GC_LIB: the collector archive from kelpc/gc"]
fn t63_out_of_memory_names_three_numbers() {
    assert!(std::env::var_os("KELP_GC_LIB").is_some(), "set KELP_GC_LIB to libkelp_gc.a");
    let bin = build("t80_fatal_stats_and_oom");
    let out = run_env(&bin, "");
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    let line = stderr
        .lines()
        .find(|l| l.starts_with("panic: out of shared-pool memory: "))
        .unwrap_or_else(|| panic!("no out-of-memory line: {stderr}"));
    let rest = line
        .strip_prefix("panic: out of shared-pool memory: heap_max is 1 MiB, ")
        .unwrap_or_else(|| panic!("heap_max is not named first: {line}"));
    let (live, rest) = rest
        .split_once(" MiB was live at the last clearing, and ")
        .unwrap_or_else(|| panic!("no live figure: {line}"));
    let (charged, tail) = rest
        .split_once(" MiB is charged to live blocks")
        .unwrap_or_else(|| panic!("no charged figure: {line}"));
    assert!(live.parse::<u64>().is_ok(), "live is not a number: {line}");
    assert!(charged.parse::<u64>().is_ok(), "charged is not a number: {line}");
    assert_eq!(tail, "", "the line has text after the charged figure: {line}");
    assert!(
        stderr.contains("  note: keep less data live, or raise heap_max in coral.toml's [target] section\n"),
        "{stderr}"
    );
}

/// T80 (18.28): a fatal exit still prints the counter line, once, before the
/// fatal line, and the process exits with status 1.
#[test]
#[ignore = "needs KELP_GC_LIB: the collector archive from kelpc/gc"]
fn t80_a_fatal_exit_prints_the_counter_line_first() {
    assert!(std::env::var_os("KELP_GC_LIB").is_some(), "set KELP_GC_LIB to libkelp_gc.a");
    let bin = build("t80_fatal_stats_and_oom");
    let out = run_env(&bin, "export KELP_GC_STATS=1; ");
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    let counter: Vec<&str> = stderr.lines().filter(|l| l.starts_with("kelp-gc-stats:")).collect();
    assert_eq!(counter.len(), 1, "the counter line must print once: {stderr}");
    assert!(is_counter_line(counter[0]), "not in the section 17.7 form: {}", counter[0]);
    let at_counter = stderr.find(counter[0]).expect("counter line");
    let at_panic = stderr.find("panic: out of shared-pool memory").expect("out-of-memory line");
    assert!(at_counter < at_panic, "the counter line must come before the fatal line: {stderr}");
}

/// T77: a bad `KELP_GC_` value prints one line and the default applies.
#[test]
#[ignore = "needs KELP_GC_LIB: the collector archive from kelpc/gc"]
fn t77_a_bad_gc_value_prints_one_line_and_the_default_applies() {
    assert!(std::env::var_os("KELP_GC_LIB").is_some(), "set KELP_GC_LIB to libkelp_gc.a");
    let bin = build("t77_bad_gc_env");
    let out = run_env(&bin, "export KELP_GC_VERIFY=yes; ");
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(String::from_utf8_lossy(&out.stdout), expected("t77_bad_gc_env"));
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "gc: ignored KELP_GC_VERIFY='yes': expected a whole number\n"
    );
}

/// T77: a stop timeout of 0 is refused the same way.
#[test]
#[ignore = "needs KELP_GC_LIB: the collector archive from kelpc/gc"]
fn t77_a_zero_stop_timeout_is_refused() {
    assert!(std::env::var_os("KELP_GC_LIB").is_some(), "set KELP_GC_LIB to libkelp_gc.a");
    let bin = build("t77_bad_gc_env");
    let script = format!("export KELP_GC_STOP_TIMEOUT_MS=0; exec '{}'", bin.display());
    let out = Command::new("timeout").arg("10").arg("sh").arg("-c").arg(&script).output().expect("run");
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "gc: KELP_GC_STOP_TIMEOUT_MS must be at least 1\n"
    );
}
