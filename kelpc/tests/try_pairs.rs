//! One Result name per fallible operation, and the retirement of the `try_`
//! twins (issue #264).
//!
//! The plain name of a fallible std operation returns the `Result` (or, for a
//! unit operation, `Result<unit, io.IoErr>`) its `try_` twin used to return, and
//! a caller who wants a panic writes `result.expect(call, "...")`. The `try_` names still
//! build for one cycle and each call prints one `outdated:` finding that names
//! the plain name (the register in `kelpc/src/deprecated.rs`). The cases here
//! are the shapes a real invocation reaches: the plain name matched as a
//! `Result`, the `result.expect` that panics with its message, and the findings.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn kelpc() -> Command {
    Command::new(env!("CARGO_BIN_EXE_kelpc"))
}

/// A fresh directory per test, so two tests running at once never share one.
fn scratch(name: &str) -> PathBuf {
    let root = match option_env!("CARGO_TARGET_TMPDIR") {
        Some(dir) => PathBuf::from(dir),
        None => std::env::temp_dir().join("kelpc-try-pairs-tests"),
    };
    let dir = root.join("try_pairs").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create the scratch directory");
    dir
}

fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create the program's directory");
    }
    fs::write(&path, text).expect("write the program");
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

#[test]
fn the_try_names_build_for_one_cycle_and_each_prints_a_finding() {
    let dir = scratch("retired");
    let program = write(
        &dir,
        "src/old.kp",
        "impl \"std/fs\"\nimpl \"std/process\"\n\nfun main() {\n    \
         val a = fs.try_read(\"/nonexistent\")\n    \
         val b = fs.try_list(\"/nonexistent\")\n    \
         val c = fs.try_write(\"/nonexistent/x\", Bytes.empty())\n    \
         val d = process.try_run(\"true\", List.empty())\n}\n",
    );
    let out = dir.join("old");
    let result = build(&program, &out, false);
    let text = stderr(&result);
    assert!(result.status.success(), "{text}");
    assert_eq!(text.matches("outdated: ").count(), 4, "{text}");
    for line in text.lines().filter(|l| l.starts_with("outdated: ")) {
        assert!(line.contains("will be removed in the next development cycle"), "{line}");
        assert!(line.contains("write `fs.") || line.contains("write `process."), "a finding does not name the plain name: {line}");
    }
    for plain in ["fs.read(", "fs.list(", "fs.write(", "process.run("] {
        assert!(text.contains(plain), "no finding names {plain}:\n{text}");
    }
    let denied = build(&program, &out, true);
    assert!(!denied.status.success(), "--deny-outdated must refuse the retired names");
}

#[test]
fn a_plain_fallible_name_is_matched_as_a_result() {
    let dir = scratch("match");
    let program = write(
        &dir,
        "src/a.kp",
        "impl \"std/fs\"\n\nfun main() {\n    \
         when fs.read(\"/nonexistent/config\") {\n        Ok(_) -> print(\"read\")\n        Err(_) -> print(\"no file\")\n    }\n    \
         when fs.write(\"/nonexistent/dir/x\", Bytes.empty()) {\n        Ok(_) -> print(\"wrote\")\n        Err(e) -> print(\"write refused: ${e.errno > 0}\")\n    }\n}\n",
    );
    let out = dir.join("a");
    let result = build(&program, &out, true);
    assert!(result.status.success(), "{}", stderr(&result));
    let ran = Command::new(&out).output().expect("run the program");
    assert!(ran.status.success(), "{}", stderr(&ran));
    assert_eq!(String::from_utf8_lossy(&ran.stdout), "no file\nwrite refused: true\n");
}

#[test]
fn expect_panics_with_its_message_on_err() {
    let dir = scratch("expect");
    let program = write(
        &dir,
        "src/a.kp",
        "impl \"std/fs\"\nimpl \"std/result\"\n\nfun main() {\n    print(\"before\")\n    val text = result.expect(fs.read(\"/nonexistent/config\"), \"cannot read the config\")\n    print(\"after\")\n}\n",
    );
    let out = dir.join("a");
    let result = build(&program, &out, true);
    assert!(result.status.success(), "{}", stderr(&result));
    let ran = Command::new(&out).output().expect("run the program");
    assert!(!ran.status.success(), "expect on Err must panic");
    let text = stderr(&ran);
    assert!(text.contains("cannot read the config"), "{text}");
    assert_eq!(String::from_utf8_lossy(&ran.stdout), "before\n", "nothing runs after the panic");
}

#[test]
fn expect_returns_the_value_on_ok() {
    let dir = scratch("expect-ok");
    let name = dir.join("ok.txt");
    let program = write(
        &dir,
        "src/a.kp",
        &format!(
            "impl \"std/fs\"\nimpl \"std/result\"\n\nfun main() {{\n    val name = \"{}\"\n    result.expect(fs.write(name, Bytes.empty()), \"write\")\n    print(result.expect(fs.read(name), \"read\").len())\n    result.expect(fs.remove(name), \"remove\")\n}}\n",
            name.display()
        ),
    );
    let out = dir.join("a");
    let result = build(&program, &out, true);
    assert!(result.status.success(), "{}", stderr(&result));
    let ran = Command::new(&out).output().expect("run the program");
    assert!(ran.status.success(), "{}", stderr(&ran));
    assert_eq!(String::from_utf8_lossy(&ran.stdout), "0\n");
}

