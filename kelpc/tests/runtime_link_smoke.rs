//! Link smoke for the Kelp runtime (stage 1 WP6).
//!
//! Each case builds an existing fixture with the LLVM backend, runs the binary,
//! and checks that its exit status, stdout and stderr equal the text recorded
//! from the C oracle at baseline before that backend was removed (`origin/main`
//! f3c5a0d, see `kelpc/tests/runtime-diff-reference/`). The cases
//! are the runtime paths the stage 1 plan names: strings and maps, threads with
//! mutex, rwlock and condvar, env set/unset/lookup under threads, a loopback
//! listen and connect, a worker panic seen by `thread.join`, poisoning, an
//! uncaught panic, and a denied capability check.
//!
//! The Kelp sources are the fixtures themselves, so this file adds no Kelp code
//! of its own. A failure names the case and the field that differs.
//! Stderr from a capability denial contains the absolute repository path, so that
//! case checks the stable part of the message instead of the whole line.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// One smoke case. `unit` is relative to `kelpc/tests`, a `.kp` file or a
/// project's `main.kp`.
struct Case {
    name: &'static str,
    unit: &'static str,
    status: i32,
    stdout: &'static str,
    stderr: &'static str,
}

const CASES: &[Case] = &[
    Case {
        name: "strings_and_maps",
        unit: "fixtures/map_basic.kp",
        status: 0,
        stdout: "0\ntrue\n3\ntrue\nfalse\n2\n99\n22\n1\n2\ntwo three \n2\n0\ntrue\n",
        stderr: "",
    },
    Case {
        name: "condvar_handshake",
        unit: "fixtures/condvar_handshake.kp",
        status: 0,
        stdout: "42\n",
        stderr: "",
    },
    Case {
        name: "env_under_threads",
        unit: "fixtures/env_thread_stress.kp",
        status: 0,
        stdout: "true\ntrue\ntrue\n",
        stderr: "",
    },
    Case {
        name: "loopback_listen_connect",
        unit: "fixtures/net_loopback.kp",
        status: 0,
        stdout: "hello\n",
        stderr: "",
    },
    Case {
        name: "worker_panic_seen_by_join",
        unit: "fixtures/thread_panic.kp",
        status: 0,
        stdout: "worker exploded\n0\n42\n",
        stderr: "",
    },
    Case {
        name: "mutex_poisoned",
        unit: "fixtures/mutex_poison.kp",
        status: 1,
        stdout: "joined\n",
        stderr: "panic: mutex poisoned\n",
    },
    Case {
        name: "rwlock_poisoned",
        unit: "fixtures/rwlock_poison.kp",
        status: 1,
        stdout: "writer exploded\n",
        stderr: "panic: rwlock poisoned\n",
    },
    Case {
        name: "uncaught_panic",
        unit: "fixtures/panic_msg.kp",
        status: 1,
        stdout: "before\n",
        stderr: "panic: deliberate failure\n",
    },
];

fn kelpc_tests() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests")
}

fn kelpc_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_kelpc"))
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("runtime-link-smoke-{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Builds `unit` into `dir`, and returns the binary's path.
fn build(unit: &str, dir: &Path, tag: &str) -> PathBuf {
    let bin = dir.join(tag);
    let out = Command::new(kelpc_bin())
        .current_dir(kelpc_tests().join(".."))
        .arg("build")
        .arg(kelpc_tests().join(unit))
        .arg("-o")
        .arg(&bin)
        .env_remove("KELPC_BACKEND")
        .env_remove("KELPC_ORACLE")
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        out.status.success(),
        "{tag}: `kelpc build {unit}` failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    bin
}

fn run(bin: &Path) -> Output {
    Command::new(bin)
        .stdin(std::process::Stdio::null())
        .output()
        .expect("failed to run the built binary")
}

fn check(case: &Case, out: &Output) {
    let tag = case.name;
    assert_eq!(out.status.code(), Some(case.status), "{tag}: exit status");
    assert_eq!(String::from_utf8_lossy(&out.stdout), case.stdout, "{tag}: stdout");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(stderr, case.stderr, "{tag}: stderr");
}

fn run_case(case: &Case) {
    let dir = scratch(case.name);
    let bin = build(case.unit, &dir, case.name);
    let out = run(&bin);
    check(case, &out);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn strings_and_maps_match_the_recorded_text() {
    run_case(&CASES[0]);
}

#[test]
fn condvar_handshake_matches_the_recorded_text() {
    run_case(&CASES[1]);
}

#[test]
fn env_under_threads_matches_the_recorded_text() {
    run_case(&CASES[2]);
}

#[test]
fn loopback_listen_connect_matches_the_recorded_text() {
    run_case(&CASES[3]);
}

#[test]
fn worker_panic_is_seen_by_join() {
    run_case(&CASES[4]);
}

#[test]
fn a_poisoned_mutex_panics() {
    run_case(&CASES[5]);
}

#[test]
fn a_poisoned_rwlock_panics() {
    run_case(&CASES[6]);
}

#[test]
fn an_uncaught_panic_exits_one_with_the_panic_line() {
    run_case(&CASES[7]);
}

/// A capability denial is a runtime path too (`kelp_cap_deny`, plan section 1,
/// row 99). The message names the absolute path of the offending file, which
/// depends on where the repository is checked out, so only the stable part is
/// compared.
#[test]
fn a_denied_net_capability_exits_one() {
    let unit = "projects/caps_net_denied/main.kp";
    let dir = scratch("caps-net-denied");
    {
        let bin = build(unit, &dir, "caps-net-denied");
        let out = run(&bin);
        let tag = "caps_net_denied";
        assert_eq!(out.status.code(), Some(1), "{tag}: exit status");
        assert!(out.stdout.is_empty(), "{tag}: stdout should be empty");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("panic: 'localhost:1' is outside the net capability granted to module 'util'"),
            "{tag}: stderr does not carry the denial:\n{stderr}"
        );
        assert!(stderr.contains("granted:\n    127.0.0.1:*"), "{tag}: stderr does not list the grant:\n{stderr}");
    }
    let _ = fs::remove_dir_all(&dir);
}
