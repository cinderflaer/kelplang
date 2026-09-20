//! The CLI contract for `kelpc lint` (roadmap open item 23).
//!
//! The rule itself lives in `kelpc/src/lint.rs` and is unit-tested there. What
//! only a real invocation can settle is the command's contract, which is where
//! `lint` differs from every other `kelpc` command:
//!
//!   1. **a warning is not a failure** — findings go to stderr and the exit
//!      stays zero, so a script tells "clean" from "has findings" by the output
//!      and "could not check" by the exit code;
//!   2. **the file is never written** — lint has no `-w` and no `--check`,
//!      because there is no state to fix and nothing to rewrite;
//!   3. **only a read failure or a lex failure is an error**, reported with the
//!      same shape `kelpc build` uses for the same file;
//!   4. **paths are collected the way `fmt` collects them** (a file, or a
//!      directory walked recursively), so a script that lints a tree names the
//!      tree once.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn kelpc() -> Command {
    Command::new(env!("CARGO_BIN_EXE_kelpc"))
}

/// A fresh directory per test: the tests write `.kp` files, and two of them
/// running at once must not see each other's.
fn scratch(name: &str) -> PathBuf {
    let root = match option_env!("CARGO_TARGET_TMPDIR") {
        Some(dir) => PathBuf::from(dir),
        None => std::env::temp_dir().join("kelpc-lint-tests"),
    };
    let dir = root.join("lint").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create the scratch directory");
    dir
}

fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, text).expect("write the fixture");
    path
}

fn lint(path: &Path) -> Output {
    kelpc()
        .arg("lint")
        .arg(path)
        .output()
        .expect("failed to spawn kelpc")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

#[test]
fn a_run_of_line_comments_is_reported_as_a_warning_and_the_exit_stays_zero() {
    let dir = scratch("run");
    let file = write(
        &dir,
        "run.kp",
        "// one\n// two\n// three\nfun main() {}\n",
    );

    let out = lint(&file);
    assert!(
        out.status.success(),
        "'lint' exits zero for a warning, got {:?}:\n{}",
        out.status.code(),
        stderr(&out)
    );
    let text = stderr(&out);
    assert!(
        text.contains("warning: 3 consecutive '//' lines suggest one '/* ... */' comment"),
        "the warning does not name the run:\n{text}"
    );
    assert!(
        text.contains(&format!("{}:1:1", file.display())),
        "the warning does not locate the first comment of the run:\n{text}"
    );
}

#[test]
fn a_file_with_no_run_is_silent() {
    let dir = scratch("clean");
    let file = write(
        &dir,
        "clean.kp",
        "// one line is not a run\nfun main() {\n    // inside the body, still one\n}\n",
    );

    let out = lint(&file);
    assert!(out.status.success(), "a clean file exits zero");
    assert!(
        stderr(&out).is_empty(),
        "a file with no run should print nothing, got:\n{}",
        stderr(&out)
    );
}

#[test]
fn a_doc_comment_run_is_not_reported() {
    let dir = scratch("doc");
    let file = write(
        &dir,
        "doc.kp",
        "/// Documentation for main, which the tooling reads as documentation\n/// and the block-comment suggestion does not apply to.\nfun main() {}\n",
    );

    let out = lint(&file);
    assert!(out.status.success());
    assert!(
        stderr(&out).is_empty(),
        "a doc comment run should print nothing, got:\n{}",
        stderr(&out)
    );
}

#[test]
fn a_directory_is_walked_and_each_finding_names_its_own_file() {
    let dir = scratch("tree");
    write(&dir, "quiet.kp", "fun main() {}\n");
    let nested = dir.join("nested");
    fs::create_dir_all(&nested).expect("create the nested directory");
    let loud = write(&nested, "loud.kp", "// one\n// two\nfun main() {}\n");

    let out = lint(&dir);
    assert!(out.status.success(), "a warning exits zero");
    let text = stderr(&out);
    assert_eq!(
        text.matches("warning:").count(),
        1,
        "expected exactly one finding in the tree, got:\n{text}"
    );
    assert!(
        text.contains(&format!("{}:1:1", loud.display())),
        "the finding does not name the file it is in:\n{text}"
    );
}

#[test]
fn lint_writes_nothing() {
    let dir = scratch("readonly");
    let text = "// one\n// two\nfun main() {}\n";
    let file = write(&dir, "untouched.kp", text);

    let out = lint(&file);
    assert!(out.status.success());
    assert_eq!(
        fs::read_to_string(&file).expect("read the fixture back"),
        text,
        "'lint' must not rewrite the file it reads"
    );
}

#[test]
fn a_file_that_does_not_lex_is_an_error_with_the_lexers_own_message() {
    let dir = scratch("lex-error");
    let file = write(&dir, "broken.kp", "// one\n/* unterminated\n");

    let out = lint(&file);
    assert!(
        !out.status.success(),
        "a file that does not lex exits nonzero"
    );
    let text = stderr(&out);
    assert!(
        text.contains("error: unterminated block comment"),
        "the lexer's message is not reported:\n{text}"
    );
    assert!(
        text.contains(&format!("{}:2:1", file.display())),
        "the error is not located in the file:\n{text}"
    );
}

#[test]
fn a_path_that_names_nothing_is_an_error() {
    let dir = scratch("missing");
    let out = lint(&dir.join("nothing-here"));

    assert!(
        !out.status.success(),
        "a path that names nothing exits nonzero"
    );
    assert!(
        stderr(&out).contains("error:"),
        "expected an error for a path that names nothing, got:\n{}",
        stderr(&out)
    );
}

#[test]
fn no_paths_at_all_is_an_error() {
    let out = kelpc().arg("lint").output().expect("failed to spawn kelpc");
    assert!(
        !out.status.success(),
        "'lint' with no path exits nonzero rather than waiting on input"
    );
    assert!(
        stderr(&out).contains("error: 'lint' needs at least one path"),
        "expected the usage error, got:\n{}",
        stderr(&out)
    );
}

#[test]
fn an_unknown_flag_is_refused() {
    let dir = scratch("flag");
    let file = write(&dir, "run.kp", "// one\n// two\nfun main() {}\n");
    let out = kelpc()
        .args(["lint", "--check"])
        .arg(&file)
        .output()
        .expect("failed to spawn kelpc");
    assert!(!out.status.success(), "'lint' has no '--check' to offer");
    assert!(
        stderr(&out).contains("error: unknown flag '--check'"),
        "expected the flag refusal, got:\n{}",
        stderr(&out)
    );
}
