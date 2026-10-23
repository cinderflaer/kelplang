//! Tidepool step 0: the runtime and checker fixes that the block design
//! depends on (`plan/` design, sections 9, 18.9, 18.11, 18.12, 18.13, 18.14 and
//! 20.5). The refusals of a thread's argument, result, environment and lock
//! values were lifted in step 1; their run tests are in `tidepool_s1.rs`. The
//! refusal fixtures run on every `cargo test`. The run fixtures build a hosted
//! program with the shared pool and run it, so they are `#[ignore]`d and run with
//! `--ignored`. `KELP_GC_LIB`, when set, names the `libkelp_gc.a` to link; unset,
//! the build uses the collector kelpc carries.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn kelpc() -> Command {
    Command::new(env!("CARGO_BIN_EXE_kelpc"))
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/managed/tidepool_s0").join(name)
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// Copies a fixture into a scratch directory and builds it as a hosted program.
fn build(name: &str) -> PathBuf {
    let root = fixture(name);
    let dir = std::env::temp_dir().join(format!("kelpc-tidepool-s0-{name}-{}", std::process::id()));
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

/// Runs a built program under the same limits as the other run tests, with
/// a 2000 ms stop timeout, and returns its output.
fn run(bin: &Path, prefix: &str) -> Output {
    // `timeout` is outside the shell and the program is `exec`ed, so a process
    // limit in `prefix` (T60's `ulimit -u 1`) applies to the program alone:
    // `timeout` forks before the limit is set, and a forked child would fail.
    let script = format!(
        "{prefix}ulimit -v 1048576; ulimit -t 10; KELP_GC_STOP_TIMEOUT_MS=2000 exec '{}'",
        bin.display()
    );
    Command::new("timeout")
        .arg("10")
        .arg("sh")
        .arg("-c")
        .arg(&script)
        .output()
        .expect("run managed program")
}

fn expected(name: &str) -> String {
    std::fs::read_to_string(fixture(name).join("expected.txt")).expect("read expected.txt")
}

fn check_refused(name: &str) {
    let out = kelpc()
        .arg("check")
        .arg(fixture(name))
        .output()
        .expect("failed to run kelpc check");
    assert!(!out.status.success(), "{name}: the value must be refused:\n{}", text(&out));
    let want = std::fs::read_to_string(fixture(name).join("refusal.txt")).expect("read refusal.txt");
    let msg = text(&out);
    assert!(msg.contains(want.trim_end()), "{name}: expected '{}' in:\n{msg}", want.trim_end());
}

#[test]
fn a_static_mut_of_a_heap_type_is_refused_by_name() {
    check_refused("refuse_static_mut_string");
}

#[test]
fn a_mutex_holding_a_struct_of_scalars_checks_clean() {
    // The classifier walks the fields: a scalar-only struct (the shape of
    // std.sync.ChanMeta) is memory, so the lock accepts it.
    let out = kelpc()
        .arg("check")
        .arg(fixture("accept_scalar_struct_lock"))
        .output()
        .expect("failed to run kelpc check");
    assert!(out.status.success(), "{}", text(&out));
}

#[test]
fn a_mutex_holding_a_struct_with_a_list_field_checks_clean() {
    // Step 1 roots the lock's value, so a struct with a List field is accepted here too.
    let out = kelpc()
        .arg("check")
        .arg(fixture("accept_list_struct_lock"))
        .output()
        .expect("failed to run kelpc check");
    assert!(out.status.success(), "{}", text(&out));
}

#[test]
fn scalar_thread_and_lock_values_still_check_clean() {
    let out = kelpc()
        .arg("check")
        .arg(fixture("accept_scalar_records"))
        .output()
        .expect("failed to run kelpc check");
    assert!(out.status.success(), "{}", text(&out));
}

fn run_and_compare(name: &str) {
    let bin = build(name);
    let out = run(&bin, "");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "{name}: exit {:?}, stderr: {stderr}", out.status);
    assert_eq!(stdout, expected(name), "{name}: stdout");
}

#[test]
#[ignore = "a managed run: run with --include-ignored"]
fn t58_a_worker_that_panics_leaves_the_collector() {
    run_and_compare("t58_worker_panic_detaches");
}

#[test]
#[ignore = "a managed run: run with --include-ignored"]
fn t61_a_join_does_not_stop_a_collection() {
    run_and_compare("t61_join_blocked");
}

#[test]
#[ignore = "a managed run: run with --include-ignored"]
fn t61_a_mutex_wait_does_not_stop_a_collection() {
    run_and_compare("t61_mutex_blocked");
}

#[test]
#[ignore = "a managed run: run with --include-ignored"]
fn t61_an_rwlock_wait_does_not_stop_a_collection() {
    run_and_compare("t61_rwlock_blocked");
}

#[test]
#[ignore = "a managed run: run with --include-ignored"]
fn t62_a_loop_over_caller_data_polls_the_collector() {
    run_and_compare("t62_caller_data_poll");
}

#[test]
#[ignore = "a managed run: run with --include-ignored"]
fn t69_a_main_thread_panic_runs_no_drop() {
    let bin = build("t69_main_panic_no_drop");
    let out = run(&bin, "");
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "", "a drop ran");
    assert_eq!(String::from_utf8_lossy(&out.stderr), "panic: stop\n");
}

#[test]
#[ignore = "a managed run: run with --include-ignored"]
fn t60_a_failed_pthread_create_names_the_return_code() {
    let bin = build("t60_spawn_fails");
    let out = run(&bin, "ulimit -u 1; ");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert_eq!(
        stderr,
        "panic: thread.spawn: pthread_create failed\n  errno 11: Resource temporarily unavailable\n"
    );
}
