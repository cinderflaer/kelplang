//! The `help:` line under a compiler diagnostic (error-help lane).
//!
//! Each site that has a next step to name gets one test here. The test
//! compiles a small program with `kelpc check`, then asserts three things about
//! the prose form: the first line is the message the site has always printed,
//! the second line is the `-->` location, and the third line is the `help:` text
//! the site carries. The JSON form gets its own test, which pins that the
//! `help` key is there when a site has one and absent when it has none.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn kelpc() -> Command {
    Command::new(env!("CARGO_BIN_EXE_kelpc"))
}

/// A fresh directory per test, so two tests never see each other's files.
fn scratch(name: &str) -> PathBuf {
    let root = match option_env!("CARGO_TARGET_TMPDIR") {
        Some(dir) => PathBuf::from(dir),
        None => std::env::temp_dir().join("kelpc-error-help-tests"),
    };
    let dir = root.join("error_help").join(name);
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

/// Asserts the prose shape of one finding: the message on line one (a prefix,
/// for the messages whose tail names a token the test does not need to pin),
/// the location on line two, and the help text on line three.
fn assert_site(out: &Output, message: &str, help: &str) {
    assert!(!out.status.success(), "the program must not compile");
    let stderr = stderr_of(out);
    let lines: Vec<&str> = stderr.lines().collect();
    assert!(lines.len() >= 3, "expected message, location and help:\n{stderr}");
    assert!(
        lines[0].starts_with(&format!("error: {message}")),
        "first line was {:?}, wanted it to start with {message:?}",
        lines[0]
    );
    assert!(lines[1].starts_with("  --> "), "{stderr}");
    assert_eq!(lines[2], format!("  help: {help}"), "{stderr}");
}

#[test]
fn an_undefined_variable_gets_a_help_line() {
    let dir = scratch("undefined-variable");
    let file = write(
        &dir,
        "main.kp",
        "fun main() {\n    val total = 1\n    print(totl)\n}\n",
    );
    assert_site(
        &check(&file, &[]),
        "undefined variable 'totl'",
        "declare it with val or var before this line, or check the spelling.",
    );
}

// A plain `total = 2` in a body is reported as an undefined variable (the
// site above). The "undefined local" and "declared with 'val'" messages are
// the compile-time evaluator's, so these two fixtures put the assignment in a
// `const` initializer block, which is the code path that prints them.
#[test]
fn an_undefined_local_on_assignment_gets_a_help_line() {
    let dir = scratch("undefined-local");
    let file = write(
        &dir,
        "main.kp",
        "const N: i32 = {\n    total = 2\n    total\n}\n\nfun main() {\n    print(N)\n}\n",
    );
    assert_site(
        &check(&file, &[]),
        "undefined local 'total'",
        "declare it with val or var before the assignment.",
    );
}

#[test]
fn assigning_to_a_val_gets_a_help_line() {
    let dir = scratch("assign-val");
    let file = write(
        &dir,
        "main.kp",
        "const N: i32 = {\n    val n = 1\n    n = 2\n    n\n}\n\nfun main() {\n    print(N)\n}\n",
    );
    assert_site(
        &check(&file, &[]),
        "cannot assign to 'n': it was declared with 'val'",
        "write var n = ... instead if the value has to change.",
    );
}

#[test]
fn a_return_type_mismatch_gets_a_help_line() {
    let dir = scratch("return-mismatch");
    let file = write(
        &dir,
        "main.kp",
        "fun f().return: i32 {\n    return true\n}\n\nfun main() {\n    print(f())\n}\n",
    );
    assert_site(
        &check(&file, &[]),
        "return type mismatch: expected 'i32', found 'bool'",
        "return a value of type 'i32', or change the return type to 'bool'.",
    );
}

#[test]
fn a_program_with_no_main_gets_a_help_line() {
    let dir = scratch("no-main");
    let file = write(&dir, "main.kp", "fun helper() {\n    print(1)\n}\n");
    assert_site(
        &check(&file, &[]),
        "no 'main' function found",
        "add fun main() to the entry file. It takes no arguments and returns nothing.",
    );
}

#[test]
fn a_use_of_a_moved_value_gets_a_help_line() {
    let out = check(Path::new(&format!("{}/tests/fixtures/owned_field_read.kp", env!("CARGO_MANIFEST_DIR"))), &[]);
    assert_site(
        &out,
        "use of moved value 'bag.xs'",
        "pass &bag.xs to keep the value here, or do not use it after the move.",
    );
}

#[test]
fn two_statements_on_one_line_get_a_help_line() {
    let dir = scratch("newline-after-statement");
    let file = write(&dir, "main.kp", "fun main() {\n    print(1) print(2)\n}\n");
    assert_site(
        &check(&file, &[]),
        "expected a newline or '}' after a statement, found ",
        "Kelp has no ';' between statements. Put the next statement on its own line.",
    );
}

#[test]
fn a_module_that_cannot_be_found_gets_a_help_line() {
    let dir = scratch("module-not-found");
    write(
        &dir,
        "coral.toml",
        "name = \"demo\"\nversion = \"0.1.0\"\nentry = \"src/main.kp\"\n",
    );
    write(&dir, "src/main.kp", "impl \"nope\"\n\nfun main() {\n}\n");
    assert_site(
        &check(&dir, &[]),
        "cannot find module 'nope': expected '",
        "create the file at that path, under src/, or correct the import path.",
    );
}

#[test]
fn a_call_to_an_undefined_function_gets_a_help_line() {
    let dir = scratch("undefined-function");
    let file = write(&dir, "main.kp", "fun main() {\n    nope(1)\n}\n");
    assert_site(
        &check(&file, &[]),
        "call to undefined function 'nope'",
        "declare fun nope before this call, or import the module that declares it.",
    );
}

#[test]
fn a_site_without_a_help_line_prints_none() {
    // A finding that has no next step to name keeps its two-line shape.
    let dir = scratch("no-help");
    let file = write(&dir, "main.kp", "fun main() {\n    val x: i32 = \"a\"\n}\n");
    let out = check(&file, &[]);
    assert!(!out.status.success());
    let stderr = stderr_of(&out);
    assert!(!stderr.lines().any(|l| l.starts_with("  help: ")), "{stderr}");
}

#[test]
fn json_carries_the_help_key_only_where_a_site_has_one() {
    let dir = scratch("json-help");
    let file = write(
        &dir,
        "main.kp",
        "fun main() {\n    val total = 1\n    print(totl)\n}\n",
    );
    let out = check(&file, &["--json"]);
    let json = stdout_of(&out);
    assert!(
        json.contains(
            "\"help\":\"declare it with val or var before this line, or check the spelling.\""
        ),
        "{json}"
    );
    // The key comes after the location and before the notes.
    let help_at = json.find("\"help\"").expect("help key");
    assert!(json.find("\"column\"").expect("column key") < help_at, "{json}");
    assert!(help_at < json.find("\"notes\"").expect("notes key"), "{json}");

    let dir = scratch("json-no-help");
    let file = write(&dir, "main.kp", "fun main() {\n    val x: i32 = \"a\"\n}\n");
    let json = stdout_of(&check(&file, &["--json"]));
    assert!(!json.contains("\"help\""), "{json}");
}
