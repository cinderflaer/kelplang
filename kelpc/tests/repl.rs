//! Transcript tests for `kelpc repl` (stage 1a of `plans/kelp-repl.md`).
//!
//! Each case is `tests/repl/<name>.in`, fed to `kelpc repl` on stdin. The
//! expected stdout, stderr and exit status are `<name>.out`, `<name>.err` and
//! `<name>.status`. `piped_equals_script` also runs the same file through
//! `--script`, and the two transcripts must match.
//!
//! Setting `KELPC_REPL_BLESS=1` writes the three files from the current output
//! instead of comparing; read the result before committing it.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// The output of one run: stdout, stderr and the exit status.
struct Transcript {
    stdout: String,
    stderr: String,
    status: i32,
}

fn cases_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/repl")
}

/// Runs `kelpc repl` with `args`, feeding `stdin` when it is given.
fn run(args: &[&str], stdin: Option<&[u8]>) -> Transcript {
    let mut command = Command::new(env!("CARGO_BIN_EXE_kelpc"));
    command
        .arg("repl")
        .args(args)
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("kelpc starts");
    if let Some(bytes) = stdin {
        child
            .stdin
            .take()
            .expect("stdin is piped")
            .write_all(bytes)
            .expect("the input is written");
    }
    let output = child.wait_with_output().expect("kelpc finishes");
    Transcript {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        status: output.status.code().unwrap_or(-1),
    }
}

/// Runs a case through stdin, the way a terminal or a pipe feeds it.
fn piped(name: &str) -> Transcript {
    let input = fs::read(cases_dir().join(format!("{name}.in"))).expect("the case input exists");
    run(&[], Some(&input))
}

/// Compares a transcript with the case's expected files, or writes them when
/// `KELPC_REPL_BLESS` is set.
fn check(name: &str, got: &Transcript) {
    let dir = cases_dir();
    let out = dir.join(format!("{name}.out"));
    let err = dir.join(format!("{name}.err"));
    let status = dir.join(format!("{name}.status"));
    if std::env::var_os("KELPC_REPL_BLESS").is_some() {
        fs::write(&out, &got.stdout).expect("write stdout");
        fs::write(&err, &got.stderr).expect("write stderr");
        fs::write(&status, format!("{}\n", got.status)).expect("write status");
        return;
    }
    let want_out = fs::read_to_string(&out).expect("the expected stdout exists");
    let want_err = fs::read_to_string(&err).expect("the expected stderr exists");
    let want_status: i32 = fs::read_to_string(&status)
        .expect("the expected status exists")
        .trim()
        .parse()
        .expect("the expected status is a number");
    assert_eq!(got.stdout, want_out, "{name}: stdout");
    assert_eq!(got.stderr, want_err, "{name}: stderr");
    assert_eq!(got.status, want_status, "{name}: exit status");
}

#[test]
fn int_expr() {
    check("int_expr", &piped("int_expr"));
}

#[test]
fn string_expr() {
    check("string_expr", &piped("string_expr"));
}

#[test]
fn unit_line_silent() {
    check("unit_line_silent", &piped("unit_line_silent"));
}

#[test]
fn unknown_name() {
    check("unknown_name", &piped("unknown_name"));
}

#[test]
fn parse_error() {
    check("parse_error", &piped("parse_error"));
}

#[test]
fn eof_without_newline() {
    check("eof_without_newline", &piped("eof_without_newline"));
}

#[test]
fn display_refused() {
    check("display_refused", &piped("display_refused"));
}

#[test]
fn refused_stage() {
    check("refused_stage", &piped("refused_stage"));
}

#[test]
fn script_status() {
    let path = cases_dir().join("script_status.in");
    let got = run(&["--script", path.to_str().expect("UTF-8 path")], None);
    check("script_status", &got);
}

#[test]
fn piped_equals_script() {
    let path = cases_dir().join("piped_equals_script.in");
    let through_script = run(&["--script", path.to_str().expect("UTF-8 path")], None);
    let through_stdin = piped("piped_equals_script");
    assert_eq!(through_stdin.stdout, through_script.stdout, "stdout of stdin and script runs");
    assert_eq!(through_stdin.stderr, through_script.stderr, "stderr of stdin and script runs");
    assert_eq!(through_stdin.status, through_script.status, "status of stdin and script runs");
    check("piped_equals_script", &through_stdin);
}
