//! `kelpc check` (MK13-S1-M3): the front end with no code generation, and the
//! machine-readable findings `lighthouse` and Coral read.
//!
//! The prose form is the one `build` already prints, from the same
//! `SourceMap`, so what only a real invocation settles is the contract around
//! it:
//!
//!   1. **it answers and writes nothing** — no artifact, no generated C;
//!   2. **the exit status separates an error from a warning** — an outdated
//!      finding on its own exits zero, the rule `lint` follows;
//!   3. **`--json` is a document a consumer reads without parsing a message** —
//!      severity, message, and a file with a 1-based range, on stdout, with
//!      nothing on stderr to interleave with it.
//!
//! The JSON is asserted by substring rather than by a parser because this crate
//! carries no dependencies on purpose. What the assertions pin is the shape a
//! consumer depends on: the keys, the severity vocabulary, the range, and the
//! two documents that are short enough to write out.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn kelpc() -> Command {
    Command::new(env!("CARGO_BIN_EXE_kelpc"))
}

/// A fresh directory per test: the tests write projects and check them, and two
/// of them running at once must not see each other's files.
fn scratch(name: &str) -> PathBuf {
    let root = match option_env!("CARGO_TARGET_TMPDIR") {
        Some(dir) => PathBuf::from(dir),
        None => std::env::temp_dir().join("kelpc-check-tests"),
    };
    let dir = root.join("check").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create the scratch directory");
    dir
}

fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create the file's directory");
    }
    fs::write(&path, text).expect("write the fixture");
    path
}

fn check(path: &Path, flags: &[&str]) -> Output {
    let mut cmd = kelpc();
    cmd.arg("check").arg(path);
    for flag in flags {
        cmd.arg(flag);
    }
    cmd.output().expect("failed to spawn kelpc")
}

fn stderr_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

fn stdout_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

const GOOD: &str = "\
fun main() {
    var xs = List.empty()
    xs.push(1)
    print(\"${xs.len()}\")
}
";

const BAD: &str = "\
fun main() {
    var ys = List.empty()
    print(ys.len())
}
";

#[test]
fn a_clean_program_checks_and_writes_nothing() {
    let dir = scratch("clean");
    let file = write(&dir, "main.kp", GOOD);
    let out = check(&file, &[]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    // No artifact and no generated C: `check` stops before the backend, and
    // the working directory it ran in is the one a build would litter.
    let left: Vec<String> = fs::read_dir(&dir)
        .expect("read the scratch directory")
        .map(|e| e.expect("entry").file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(left, vec!["main.kp".to_string()], "check wrote something");
}

#[test]
fn an_error_is_reported_in_prose_and_exits_nonzero() {
    let dir = scratch("prose-error");
    let file = write(&dir, "main.kp", BAD);
    let out = check(&file, &[]);
    assert!(!out.status.success());
    let stderr = stderr_of(&out);
    // The point, not the range: the range form is what `--json` is for.
    assert!(stderr.contains("error: "), "{stderr}");
    assert!(stderr.contains("--> "), "{stderr}");
    assert!(stderr.contains("main.kp:3:11"), "{stderr}");
    assert!(stdout_of(&out).is_empty(), "prose findings go to stderr");
}

#[test]
fn json_reports_a_clean_program_as_one_document() {
    let dir = scratch("json-clean");
    let file = write(&dir, "main.kp", GOOD);
    let out = check(&file, &["--json"]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    assert_eq!(stdout_of(&out), "{\"schema\":1,\"ok\":true,\"diagnostics\":[]}\n");
    assert!(stderr_of(&out).is_empty(), "a consumer reads stdout alone");
}

#[test]
fn json_names_the_severity_the_file_and_the_range() {
    let dir = scratch("json-error");
    let file = write(&dir, "main.kp", BAD);
    let out = check(&file, &["--json"]);
    assert!(!out.status.success());
    assert!(stderr_of(&out).is_empty());
    let json = stdout_of(&out);
    assert!(json.starts_with("{\"schema\":1,\"ok\":false,\"diagnostics\":["), "{json}");
    assert!(json.contains("\"severity\":\"error\""), "{json}");
    assert!(json.contains("\"file\":\""), "{json}");
    // The range points at `ys.len()`, and the end is after the start: a
    // diagnostic that carried a point instead of a span would report the same
    // two numbers, which is what this pins.
    assert!(json.contains("\"line\":3,\"column\":11,"), "{json}");
    assert!(json.contains("\"end_line\":3,\"end_column\":19"), "{json}");
    assert!(json.contains("\"notes\":[]"), "{json}");
}

#[test]
fn json_hands_back_a_finding_that_has_no_file_at_all() {
    // A path the compiler cannot read is a finding before any module exists,
    // so there is no file to name. The document still has to be well formed,
    // which is why `module` is always there and the location keys are not.
    let dir = scratch("json-no-file");
    let out = check(&dir.join("absent.kp"), &["--json"]);
    assert!(!out.status.success());
    let json = stdout_of(&out);
    assert!(json.contains("\"ok\":false"), "{json}");
    assert!(json.contains("\"module\":\"\""), "{json}");
    assert!(!json.contains("\"file\""), "{json}");
    assert!(!json.contains("\"line\""), "{json}");
}

#[test]
fn outdated_syntax_is_a_finding_rather_than_a_failure() {
    let dir = scratch("outdated");
    let file = write(
        &dir,
        "main.kp",
        "impl \"std/fs\" with caps(fs = [\"/tmp/**\"])\n\nfun main() {\n    print(fs.exists(\"/tmp\"))\n}\n",
    );
    let out = check(&file, &["--json"]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    let json = stdout_of(&out);
    assert!(json.contains("\"ok\":true"), "{json}");
    assert!(json.contains("\"severity\":\"outdated\""), "{json}");

    // `--deny-outdated` is the migration gate: the same finding, as an error.
    let denied = check(&file, &["--json", "--deny-outdated"]);
    assert!(!denied.status.success());
    let json = stdout_of(&denied);
    assert!(json.contains("\"ok\":false"), "{json}");
    assert!(json.contains("\"severity\":\"error\""), "{json}");
}

#[test]
fn a_project_directory_checks_and_names_the_module_that_failed() {
    let dir = scratch("project");
    write(
        &dir,
        "coral.toml",
        "name = \"demo\"\nversion = \"0.1.0\"\nentry = \"src/main.kp\"\n",
    );
    write(&dir, "src/main.kp", "impl \"util\"\n\nfun main() {\n    util.area()\n}\n");
    write(&dir, "src/util.kp", "fun area().R: i32 = \"not a number\"\n");

    let out = check(&dir, &["--json"]);
    assert!(!out.status.success());
    let json = stdout_of(&out);
    // The finding lands on the module that has the defect, not on the entry
    // file that imports it: this is the grouping `lighthouse` needs.
    assert!(json.contains("util.kp"), "{json}");
    assert!(json.contains("\"module\":\"util\""), "{json}");
}

#[test]
fn a_bad_flag_and_a_missing_path_are_errors_of_their_own() {
    let dir = scratch("flags");
    let file = write(&dir, "main.kp", GOOD);
    let bad_flag = check(&file, &["--nope"]);
    assert!(!bad_flag.status.success());
    assert!(stderr_of(&bad_flag).contains("unknown flag '--nope'"));
    assert!(stdout_of(&bad_flag).is_empty());

    let no_path = {
        let mut cmd = kelpc();
        cmd.arg("check");
        cmd.output().expect("failed to spawn kelpc")
    };
    assert!(!no_path.status.success());
    assert!(stderr_of(&no_path).contains("needs a .kp file"), "{}", stderr_of(&no_path));
}
