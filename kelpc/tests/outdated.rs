//! The deprecation contract for `kelpc build` and `kelpc run` (roadmap open
//! item 74, `kelpc/src/deprecated.rs`).
//!
//! The register itself is unit-tested where it lives. What only a real
//! invocation can settle is the contract around it:
//!
//!   1. outdated syntax **compiles** — the exit status is zero, the program
//!      runs, and the finding is a warning on stderr;
//!   2. the warning names the removal and the fix, and points at the clause
//!      rather than at the line;
//!   3. `--deny-outdated` makes the same finding an error, and stops before
//!      anything is generated;
//!   4. a program with no outdated syntax prints nothing at all, so the flag
//!      and the warning do not become background noise.
//!
//! The register holds one entry, `impl "..." with caps(...)`, so the fixtures
//! here import with the keyword. The other entry, `impl "..." as name`, was
//! retired rather than migrated: the clause is gone from the parser, and the
//! last case below is the refusal a program still carrying it meets. A
//! dependency path reached by the name under `deps/` is the spelling the
//! register pointed at while the clause existed.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn kelpc() -> Command {
    Command::new(env!("CARGO_BIN_EXE_kelpc"))
}

/// A fresh directory per test: the tests write programs and build them, and two
/// of them running at once must not see each other's artifacts.
fn scratch(name: &str) -> PathBuf {
    let root = match option_env!("CARGO_TARGET_TMPDIR") {
        Some(dir) => PathBuf::from(dir),
        None => std::env::temp_dir().join("kelpc-outdated-tests"),
    };
    let dir = root.join("outdated").join(name);
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

fn build(path: &Path, out: &Path, deny: bool) -> Output {
    let mut cmd = kelpc();
    cmd.arg("build").arg(path).arg("-o").arg(out);
    if deny {
        cmd.arg("--deny-outdated");
    }
    cmd.output().expect("failed to spawn kelpc")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

/// A two-module project whose import uses the outdated spelling: a real `fs`
/// delegation written with the `with` keyword, so the case is about the
/// keyword rather than about a clause the resolver skips.
fn caps_with_project(dir: &Path) -> PathBuf {
    write(dir, "src/util.kp", "fun answer().R: i32 = 42\n");
    write(
        dir,
        "src/main.kp",
        "impl \"util\" with caps(fs = [\"/tmp/**\"])\n\nfun main() {\n    print(util.answer())\n}\n",
    )
}

#[test]
fn the_with_caps_spelling_warns_and_the_build_succeeds() {
    let dir = scratch("caps-with");
    let source = caps_with_project(&dir);
    let out = dir.join("prog");

    let result = build(&source, &out, false);
    assert!(
        result.status.success(),
        "outdated syntax must still compile, got {:?}:\n{}",
        result.status.code(),
        stderr(&result)
    );
    let text = stderr(&result);
    assert!(
        text.contains("outdated: `impl \"...\" with caps(...)` is outdated"),
        "the finding is not labelled `outdated:`:\n{text}"
    );
    assert!(
        text.contains("will be removed in the next development cycle"),
        "the warning does not name the removal:\n{text}"
    );
    assert!(
        text.contains("drop the `with`"),
        "the warning does not name the fix:\n{text}"
    );
    assert!(
        text.contains(&format!("{}:1:13", source.display())),
        "the warning does not point at the `with` token:\n{text}"
    );
    assert_eq!(
        text.matches("outdated:").count(),
        1,
        "the finding is printed once, not once per load pass:\n{text}"
    );

    // The grant behind the keyword still works: the program builds and runs.
    let ran = Command::new(&out).output().expect("run the built program");
    assert!(ran.status.success());
    assert_eq!(String::from_utf8_lossy(&ran.stdout), "42\n");
}

#[test]
fn deny_outdated_turns_the_finding_into_an_error_and_writes_nothing() {
    let dir = scratch("deny");
    let source = caps_with_project(&dir);
    let out = dir.join("prog");

    let result = build(&source, &out, true);
    assert!(
        !result.status.success(),
        "--deny-outdated must fail the build:\n{}",
        stderr(&result)
    );
    let text = stderr(&result);
    assert!(
        text.contains("error: `impl \"...\" with caps(...)` is outdated"),
        "the finding is not reported as an error:\n{text}"
    );
    assert!(
        !out.exists(),
        "a denied build must stop before it generates an artifact"
    );
}

#[test]
fn run_deny_outdated_refuses_to_run_the_program() {
    let dir = scratch("run-deny");
    let source = caps_with_project(&dir);

    let result = kelpc()
        .arg("run")
        .arg(&source)
        .arg("--deny-outdated")
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        !result.status.success(),
        "the flag has to stop `run` before the program runs"
    );
    assert!(
        String::from_utf8_lossy(&result.stdout).is_empty(),
        "the program ran anyway:\n{}",
        String::from_utf8_lossy(&result.stdout)
    );
}

#[test]
fn a_program_with_no_outdated_syntax_prints_nothing() {
    let dir = scratch("clean");
    write(&dir, "src/util.kp", "fun answer().R: i32 = 42\n");
    let source = write(
        &dir,
        "src/main.kp",
        "impl \"util\"\n\nfun main() {\n    print(util.answer())\n}\n",
    );
    let out = dir.join("prog");

    let result = build(&source, &out, false);
    assert!(result.status.success(), "{}", stderr(&result));
    assert!(
        stderr(&result).is_empty(),
        "a clean build should print nothing, got:\n{}",
        stderr(&result)
    );
    // The same source under the flag: nothing to deny.
    let denied = build(&source, &out, true);
    assert!(denied.status.success(), "{}", stderr(&denied));
}

#[test]
fn the_impl_as_clause_is_gone() {
    // The retired entry: not a warning, a refusal that names the fix, because
    // the register only holds spellings that still compile.
    let dir = scratch("impl-as");
    write(&dir, "src/util.kp", "fun answer().R: i32 = 42\n");
    let source = write(
        &dir,
        "src/main.kp",
        "impl \"util\" as u\n\nfun main() {\n    print(u.answer())\n}\n",
    );
    let out = dir.join("prog");

    let result = build(&source, &out, false);
    assert!(
        !result.status.success(),
        "the clause is removed, so the build must fail:\n{}",
        stderr(&result)
    );
    let text = stderr(&result);
    assert!(
        text.contains("`impl \"...\" as name` is gone: reach the module by its own name"),
        "the refusal does not name the fix:\n{text}"
    );
    assert!(
        !text.contains("outdated:"),
        "a removed spelling is not a deprecation:\n{text}"
    );
    assert!(
        !out.exists(),
        "a refused build must not generate an artifact"
    );
}

#[test]
fn a_dependency_is_reached_by_the_name_under_deps() {
    // The spelling the register points at: no `as`, and the namespace comes
    // from the path Coral copied the dependency to (`src/deps/<key>/`), which
    // is the key the `[dependencies]` line gave it.
    let dir = scratch("deps");
    write(&dir, "coral.toml", "name = \"depsdemo\"\n");
    write(
        &dir,
        "src/deps/argparse/lib.kp",
        "fun parse().R: i32 = 7\n",
    );
    let source = write(
        &dir,
        "src/main.kp",
        "impl \"deps/argparse/lib\"\n\nfun main() {\n    print(argparse.parse())\n}\n",
    );
    let out = dir.join("prog");

    let result = build(&source, &out, false);
    assert!(
        result.status.success(),
        "a dependency path should be reached by its own name, got:\n{}",
        stderr(&result)
    );
    assert!(
        stderr(&result).is_empty(),
        "nothing here is outdated, so nothing should be printed:\n{}",
        stderr(&result)
    );
    let ran = Command::new(&out).output().expect("run the built program");
    assert_eq!(String::from_utf8_lossy(&ran.stdout), "7\n");
}
