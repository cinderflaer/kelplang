//! The deprecation contract for `kelpc build` and `kelpc run` (roadmap open
//! item 74, `kelpc/src/deprecated.rs`).
//!
//! The register holds one entry, `.R:` (see the last test); the two it held
//! before, `impl "..." as name` and `impl "..." with caps(...)`, were removed in
//! the one cycle their shared deadline sentence gave them. What only a real
//! invocation can settle is the contract around that machinery:
//!
//!   1. a spelling that was on the register is a parse error that names its fix,
//!      which is what a program still carrying it meets (the two cases below);
//!   2. `--deny-outdated` leaves a program with nothing on the list exactly as
//!      it was: it builds, it runs, and it prints nothing, which is what makes
//!      the flag usable over a tree that is already migrated;
//!   3. a dependency path is reached by the name under `deps/`, the spelling the
//!      first entry pointed at while it existed.
//!
//! The wording an entry buys, and the id lookup behind it, are unit-tested where
//! the register lives (`deprecated.rs`): with an empty register no source program
//! can reach them, so an end-to-end case here would have to invent a permanent
//! deprecation to have something to report.

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

/// A two-module project whose import still writes the removed `with` keyword.
/// The grant under it is a real `fs` delegation, so the case is about the
/// keyword rather than about a clause the resolver skips.
fn caps_with_project(dir: &Path) -> PathBuf {
    write(dir, "src/util.kp", "pub fun answer().return: i32 = 42\n");
    write(
        dir,
        "src/main.kp",
        "impl \"util\" with caps(fs = [\"/tmp/**\"])\n\nfun main() {\n    print(util.answer())\n}\n",
    )
}

#[test]
fn the_impl_as_clause_is_gone() {
    // Retired with the first entry: not a warning, a refusal that names the fix,
    // because the register only holds spellings that still compile.
    let dir = scratch("impl-as");
    write(&dir, "src/util.kp", "pub fun answer().return: i32 = 42\n");
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
fn the_with_caps_clause_is_gone() {
    // Retired with the second and last entry, on the deadline its own finding
    // stated. The refusal is the parser's, and it names the surviving spelling.
    let dir = scratch("caps-with");
    let source = caps_with_project(&dir);
    let out = dir.join("prog");

    let result = build(&source, &out, false);
    assert!(
        !result.status.success(),
        "the clause is removed, so the build must fail:\n{}",
        stderr(&result)
    );
    let text = stderr(&result);
    assert!(
        text.contains("`impl \"...\" with caps(...)` is gone: drop the `with`"),
        "the refusal does not name the surviving spelling:\n{text}"
    );
    assert!(
        text.contains("caps(fs = [\"/var/log/**\"])"),
        "the refusal does not show the grant it means:\n{text}"
    );
    assert!(
        text.contains(&format!("{}:1:13", source.display())),
        "the refusal does not point at the `with` token:\n{text}"
    );
    assert!(
        !text.contains("outdated:"),
        "a removed spelling is not a deprecation:\n{text}"
    );
    assert!(
        !out.exists(),
        "a refused build must not generate an artifact"
    );

    // The surviving spelling is the same clause: it compiles and runs, so the
    // refusal above is about the keyword and nothing else.
    write(
        &dir,
        "src/main.kp",
        "impl \"util\" caps(fs = [\"/tmp/**\"])\n\nfun main() {\n    print(util.answer())\n}\n",
    );
    let fixed = build(&source, &out, false);
    assert!(
        fixed.status.success(),
        "the surviving spelling must build:\n{}",
        stderr(&fixed)
    );
    let ran = Command::new(&out).output().expect("run the built program");
    assert_eq!(String::from_utf8_lossy(&ran.stdout), "42\n");
}

#[test]
fn a_program_with_no_outdated_syntax_prints_nothing() {
    let dir = scratch("clean");
    write(&dir, "src/util.kp", "pub fun answer().return: i32 = 42\n");
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
    // The same source under the flag: nothing on the list to deny, so the build
    // is the build it was, and `run` runs the program rather than stopping at a
    // finding. That is the whole of what `--deny-outdated` does while the
    // register is empty.
    let denied = build(&source, &out, true);
    assert!(denied.status.success(), "{}", stderr(&denied));
    assert!(stderr(&denied).is_empty(), "{}", stderr(&denied));

    let ran = kelpc()
        // `run` writes `build/<stem>` relative to the working directory when no
        // `-o` is given, so the scratch directory is where it runs: the artifact
        // belongs beside the program it was built from, not in the package root.
        .current_dir(&dir)
        .arg("run")
        .arg(&source)
        .arg("--deny-outdated")
        .output()
        .expect("failed to spawn kelpc");
    assert!(ran.status.success(), "{}", stderr(&ran));
    assert_eq!(String::from_utf8_lossy(&ran.stdout), "42\n");
}

#[test]
fn a_dependency_is_reached_by_the_name_under_deps() {
    // The spelling the first entry pointed at: no `as`, and the namespace comes
    // from the path Coral copied the dependency to (`src/deps/<key>/`), which
    // is the key the `[dependencies]` line gave it.
    let dir = scratch("deps");
    write(&dir, "coral.toml", "name = \"depsdemo\"\n");
    write(&dir, "src/deps/argparse/lib.kp", "pub fun parse().return: i32 = 7\n");
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

#[test]
fn the_r_return_marker_still_builds_and_says_what_to_write() {
    // `.R:` is the one entry on the register (id `return-r`): it compiles, the
    // build names the marker that replaces it, and `--deny-outdated` turns the
    // same finding into the error that stops the build.
    let dir = scratch("return_r");
    let source = write(
        &dir,
        "main.kp",
        "fun old(x: i32).R: i32 = x\nfun new(x: i32).return: i32 = x\n\nfun main() {\n    print(old(1) + new(2))\n}\n",
    );
    let out = dir.join("prog");

    let warned = build(&source, &out, false);
    assert!(warned.status.success(), "{}", stderr(&warned));
    let text = stderr(&warned);
    assert!(text.contains("outdated: the return marker `.R:`"), "{text}");
    assert!(text.contains("write `.return:`"), "{text}");
    assert_eq!(text.matches("outdated:").count(), 1, "only the `.R:` is reported: {text}");
    let ran = Command::new(&out).output().expect("run the built program");
    assert_eq!(String::from_utf8_lossy(&ran.stdout), "3\n");

    let denied = build(&source, &out, true);
    assert!(!denied.status.success());
    assert!(stderr(&denied).contains("the return marker `.R:` is outdated"), "{}", stderr(&denied));
}
