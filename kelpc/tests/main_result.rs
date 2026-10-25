//! `main` may return `Result<unit, E>` (issue #298). On `Err` the program
//! prints the error's text on stderr and exits with status 1; on `Ok` it exits
//! with status 0. The rewrite that makes this work runs in the parser, so these
//! tests build real binaries with the `kelpc` the suite was compiled with.

use std::path::{Path, PathBuf};
use std::process::Command;

fn kelpc() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_kelpc"));
    cmd.env_remove("KELPC_BACKEND");
    cmd.env_remove("KELPC_ORACLE");
    cmd
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

fn out_path(name: &str) -> PathBuf {
    let dir = std::env::var("CARGO_TARGET_TMPDIR").unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().to_string());
    let test = std::thread::current().name().unwrap_or("main").to_string();
    PathBuf::from(dir).join(format!("kelpc_main_result_{test}_{name}"))
}

/// Builds `source` (a fixture file name) and runs it, returning the run's
/// status code, stdout and stderr.
fn build_and_run(source: &str) -> (Option<i32>, String, String) {
    let name = Path::new(source).file_stem().unwrap().to_string_lossy().to_string();
    let bin = out_path(&name);
    let build = kelpc()
        .arg("build")
        .arg(fixture(source))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected '{source}' to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&bin).output().expect("failed to run compiled binary");
    (
        run.status.code(),
        String::from_utf8_lossy(&run.stdout).to_string(),
        String::from_utf8_lossy(&run.stderr).to_string(),
    )
}

/// Writes `source` to a scratch file and returns the build's stderr. The build
/// must fail.
fn build_refused(name: &str, source: &str) -> String {
    let dir = out_path(name);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    let file = dir.join("main.kp");
    std::fs::write(&file, source).expect("write source");
    let build = kelpc()
        .arg("build")
        .arg(&file)
        .arg("-o")
        .arg(dir.join("bin"))
        .output()
        .expect("failed to spawn kelpc");
    assert!(!build.status.success(), "expected '{name}' to be refused, but it built");
    String::from_utf8_lossy(&build.stderr).to_string()
}

#[test]
fn an_ok_from_main_exits_zero_after_its_output() {
    let (code, stdout, stderr) = build_and_run("main_result_ok.kp");
    assert_eq!(code, Some(0), "stderr: {stderr}");
    assert_eq!(stdout, "start\n");
    assert!(stderr.is_empty(), "unexpected stderr: {stderr}");
}

#[test]
fn an_err_from_main_prints_its_text_and_exits_one() {
    let (code, stdout, stderr) = build_and_run("main_result_err.kp");
    assert_eq!(code, Some(1));
    assert_eq!(stdout, "before\n");
    assert_eq!(stderr, "boom\n");
}

#[test]
fn a_question_mark_in_main_ends_the_program_with_the_error() {
    let (code, stdout, stderr) = build_and_run("main_result_question.kp");
    assert_eq!(code, Some(1));
    assert_eq!(stdout, "a = 1\n");
    assert_eq!(stderr, "step 5 is too large\n");
}

#[test]
fn an_integer_error_prints_its_digits() {
    let (code, stdout, stderr) = build_and_run("main_result_int_err.kp");
    assert_eq!(code, Some(1));
    assert_eq!(stdout, "");
    assert_eq!(stderr, "7\n");
}

#[test]
fn main_may_not_return_a_result_with_a_success_type_other_than_unit() {
    let stderr = build_refused(
        "ok_type",
        "impl \"std/result\"\n\nfun main() => result.Result<i32, String> {\n    return result.Result.Err(error = \"x\")\n}\n",
    );
    assert!(
        stderr.contains("Result<unit, E>"),
        "expected the refusal to name Result<unit, E>, got:\n{stderr}"
    );
}

#[test]
fn a_user_declaration_of_the_hidden_entry_name_is_refused() {
    let stderr = build_refused(
        "reserved_name",
        "impl \"std/result\"\n\nfun kelp_main_result() => result.Result<unit, String> {\n    return result.Result.Ok(value = ())\n}\n\nfun main() => result.Result<unit, String> {\n    return kelp_main_result()\n}\n",
    );
    assert!(
        stderr.contains("kelp_main_result"),
        "expected the refusal to name kelp_main_result, got:\n{stderr}"
    );
}
