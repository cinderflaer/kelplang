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
        "fun f() => i32 {\n    return true\n}\n\nfun main() {\n    print(f())\n}\n",
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

// A word another language spells differently gets the Kelp word in the help
// line. The first line is the message the site always printed; only the help
// text is new. The help strings below are the table in `src/foreign.rs`.

const FN_HELP: &str = "fn is not Kelp: write fun name(params) => type { ... }, or fun name(params) { ... } when nothing is returned";
const DEF_HELP: &str = "def is not Kelp: write fun name(params) => type { ... }, or fun name(params) { ... } when nothing is returned";
const STRUCT_HELP: &str = "struct is not Kelp: a record is datac Name(val x: i32, val y: i32)";
const LET_HELP: &str = "let is not Kelp: write val x = 5, or var x = 5 if the value changes";
const MATCH_HELP: &str = "there is no match: use when x { ... } with one arm for each case";
const NULL_HELP: &str = "Kelp has no null: an absent value is option.Option.None, after impl \"std/option\"";
const NONE_HELP: &str = "None is not a Kelp value: an absent value is option.Option.None, after impl \"std/option\"";
const TRUE_HELP: &str = "True is not Kelp: write true";
const PRINTLN_HELP: &str = "did you mean print? print adds the line end, so print(\"x\") needs no newline argument";
const AS_HELP: &str = "there is no as: convert with a method, such as x.to_i64()";
const INT_HELP: &str = "int is not a Kelp type: write i64, or i32 for a 32-bit integer";
const STRING_HELP: &str = "string is not a Kelp type: write String, the text type";
const ELSEIF_HELP: &str = "elseif is not Kelp: write else if";

#[test]
fn let_as_a_statement_gets_the_kelp_word() {
    let dir = scratch("foreign-let");
    let file = write(&dir, "main.kp", "fun main() {\n    let x = 5\n    print(x)\n}\n");
    assert_site(
        &check(&file, &[]),
        "let-else: 'x' is a plain name, which always matches",
        LET_HELP,
    );
}

#[test]
fn fn_at_the_top_level_gets_the_kelp_word() {
    let dir = scratch("foreign-fn");
    let file = write(&dir, "main.kp", "fn main() {\n    print(1)\n}\n");
    assert_site(&check(&file, &[]), "expected 'fun', found ", FN_HELP);
}

#[test]
fn def_at_the_top_level_gets_the_kelp_word() {
    let dir = scratch("foreign-def");
    let file = write(&dir, "main.kp", "def main() {\n    print(1)\n}\n");
    assert_site(&check(&file, &[]), "expected 'fun', found ", DEF_HELP);
}

#[test]
fn struct_at_the_top_level_gets_the_kelp_word() {
    let dir = scratch("foreign-struct");
    let file = write(
        &dir,
        "main.kp",
        "struct P {\n    x: i32\n}\n\nfun main() {\n    print(1)\n}\n",
    );
    assert_site(&check(&file, &[]), "expected 'fun', found ", STRUCT_HELP);
}

#[test]
fn match_as_a_statement_gets_the_kelp_word() {
    let dir = scratch("foreign-match");
    let file = write(
        &dir,
        "main.kp",
        "fun main() {\n    val x = 1\n    match x {\n    }\n}\n",
    );
    assert_site(
        &check(&file, &[]),
        "expected a newline or '}' after a statement, found ",
        MATCH_HELP,
    );
}

#[test]
fn null_as_a_name_gets_the_kelp_word() {
    let dir = scratch("foreign-null");
    let file = write(&dir, "main.kp", "fun main() {\n    val x = null\n    print(x)\n}\n");
    assert_site(&check(&file, &[]), "undefined variable 'null'", NULL_HELP);
}

#[test]
fn none_as_a_name_gets_the_kelp_word() {
    let dir = scratch("foreign-none");
    let file = write(&dir, "main.kp", "fun main() {\n    val x = None\n    print(x)\n}\n");
    assert_site(&check(&file, &[]), "undefined variable 'None'", NONE_HELP);
}

#[test]
fn true_with_a_capital_gets_the_kelp_word() {
    let dir = scratch("foreign-true");
    let file = write(&dir, "main.kp", "fun main() {\n    val x = True\n    print(x)\n}\n");
    assert_site(&check(&file, &[]), "undefined variable 'True'", TRUE_HELP);
}

#[test]
fn println_gets_print_as_the_did_you_mean() {
    let dir = scratch("foreign-println");
    let file = write(&dir, "main.kp", "fun main() {\n    println(\"x\")\n}\n");
    assert_site(
        &check(&file, &[]),
        "call to undefined function 'println'",
        PRINTLN_HELP,
    );
}

#[test]
fn as_in_a_call_gets_the_kelp_word() {
    let dir = scratch("foreign-as");
    let file = write(
        &dir,
        "main.kp",
        "fun main() {\n    val x = 1\n    print(x as i64)\n}\n",
    );
    assert_site(&check(&file, &[]), "expected ')', found ", AS_HELP);
}

#[test]
fn int_as_a_type_gets_the_kelp_type() {
    let dir = scratch("foreign-int");
    let file = write(&dir, "main.kp", "fun main() {\n    val n: int = 5\n    print(n)\n}\n");
    assert_site(&check(&file, &[]), "unknown type 'int'", INT_HELP);
}

#[test]
fn string_as_a_type_gets_the_kelp_type() {
    let dir = scratch("foreign-string");
    let file = write(
        &dir,
        "main.kp",
        "fun main() {\n    val s: string = \"a\"\n    print(s)\n}\n",
    );
    assert_site(&check(&file, &[]), "unknown type 'string'", STRING_HELP);
}

#[test]
fn elseif_gets_the_else_if_spelling() {
    let dir = scratch("foreign-elseif");
    let file = write(
        &dir,
        "main.kp",
        "fun main() {\n    val x = 1\n    if x > 1 {\n        print(1)\n    } elseif x > 0 {\n        print(2)\n    }\n}\n",
    );
    assert_site(
        &check(&file, &[]),
        "expected a newline or '}' after a statement, found ",
        ELSEIF_HELP,
    );
}
