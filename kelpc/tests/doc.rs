//! `kelpc doc` (`plan/doc-tool.md`): the pages for a small module that holds
//! every kind of documented item, compared with checked-in expected files; the
//! `--private`, `--check` and `--std` modes; determinism; the lint warning for a
//! doc comment attached to nothing; and the formatter and the front end leaving
//! doc comments alone.
//!
//! To regenerate an expected file after an intended change:
//! `kelpc doc tests/docs/shapes.kp [--format html] [--private] -o tests/docs/expected/<dir>`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn kelpc() -> Command {
    Command::new(env!("CARGO_BIN_EXE_kelpc"))
}

fn docs() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("docs")
}

fn scratch(name: &str) -> PathBuf {
    let root = match option_env!("CARGO_TARGET_TMPDIR") {
        Some(dir) => PathBuf::from(dir),
        None => std::env::temp_dir().join("kelpc-doc-tests"),
    };
    let dir = root.join("doc").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create the scratch directory");
    dir
}

fn run(args: &[&str]) -> Output {
    kelpc().arg("doc").args(args).output().expect("spawn kelpc")
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

/// Generates into a fresh directory and compares every page with the
/// checked-in directory `expected`.
fn matches_expected(test: &str, extra: &[&str], expected: &str) {
    let out = scratch(test);
    let shapes = docs().join("shapes.kp");
    let mut args = vec![shapes.to_str().unwrap()];
    args.extend_from_slice(extra);
    args.push("-o");
    args.push(out.to_str().unwrap());
    let result = run(&args);
    assert!(result.status.success(), "{}", stderr(&result));
    let want = docs().join("expected").join(expected);
    let mut names: Vec<String> = fs::read_dir(&want)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    names.sort();
    let mut got: Vec<String> = fs::read_dir(&out)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    got.sort();
    assert_eq!(names, got, "the set of pages differs");
    for name in names {
        assert_eq!(read(&want.join(&name)), read(&out.join(&name)), "{name} differs");
    }
}

#[test]
fn markdown_pages_match_the_expected_files() {
    matches_expected("md", &[], "md");
}

#[test]
fn html_pages_match_the_expected_files() {
    matches_expected("html", &["--format", "html"], "html");
}

#[test]
fn private_items_appear_only_with_the_private_flag() {
    matches_expected("private", &["--private"], "private");
    let public = read(&docs().join("expected/md/shapes.md"));
    let private = read(&docs().join("expected/private/shapes.md"));
    assert!(!public.contains("hidden") && !public.contains("SECRET"));
    assert!(private.contains("hidden") && private.contains("SECRET"));
}

#[test]
fn html_is_self_contained() {
    let html = read(&docs().join("expected/html/shapes.html"));
    assert!(!html.contains("src=") && !html.contains("http://") && !html.contains("https://"));
    assert!(html.contains("<style>"));
}

#[test]
fn output_is_deterministic() {
    let a = scratch("det_a");
    let b = scratch("det_b");
    let project = docs().join("project");
    for dir in [&a, &b] {
        let r = run(&[project.to_str().unwrap(), "-o", dir.to_str().unwrap()]);
        assert!(r.status.success(), "{}", stderr(&r));
    }
    let mut names: Vec<_> = fs::read_dir(&a).unwrap().map(|e| e.unwrap().file_name()).collect();
    names.sort();
    assert!(names.len() >= 3);
    for n in names {
        assert_eq!(read(&a.join(&n)), read(&b.join(&n)));
    }
}

#[test]
fn a_project_directory_gets_one_page_per_module_and_an_index() {
    let out = scratch("project");
    let project = docs().join("project");
    let r = run(&[project.to_str().unwrap(), "-o", out.to_str().unwrap()]);
    assert!(r.status.success(), "{}", stderr(&r));
    let text = read(&out.join("util.text.md"));
    assert!(text.contains("Text helpers."));
    assert!(text.contains("Upper-cases `s`."));
    assert!(read(&out.join("index.md")).contains("[`util/text`](util.text.md)"));
}

#[test]
fn check_passes_when_every_public_item_is_documented() {
    let shapes = docs().join("shapes.kp");
    let r = run(&[shapes.to_str().unwrap(), "--check"]);
    assert!(r.status.success(), "{}", stderr(&r));
    let project = docs().join("project");
    assert!(run(&[project.to_str().unwrap(), "--check"]).status.success());
}

#[test]
fn check_fails_naming_the_undocumented_public_item() {
    let file = docs().join("undocumented.kp");
    let r = run(&[file.to_str().unwrap(), "--check"]);
    assert!(!r.status.success());
    let err = stderr(&r);
    assert!(err.contains("undocumented.kp:5") && err.contains("`bad`"), "{err}");
    assert!(!err.contains("`good`"), "{err}");
}

#[test]
fn check_with_private_also_wants_private_items_documented() {
    let shapes = docs().join("shapes.kp");
    let r = run(&[shapes.to_str().unwrap(), "--check", "--private"]);
    assert!(!r.status.success());
    assert!(stderr(&r).contains("`hidden`"));
}

#[test]
fn check_writes_nothing() {
    let out = scratch("check_nothing").join("never");
    let file = docs().join("undocumented.kp");
    run(&[file.to_str().unwrap(), "--check", "-o", out.to_str().unwrap()]);
    assert!(!out.exists());
}

#[test]
fn a_doc_comment_before_nothing_is_a_lint_warning_not_an_error() {
    let file = docs().join("orphan.kp");
    let r = kelpc().arg("lint").arg(&file).output().unwrap();
    assert!(r.status.success());
    let err = stderr(&r);
    assert_eq!(err.matches("documents nothing").count(), 2, "{err}");
    assert!(err.contains("orphan.kp:1:1") && err.contains("orphan.kp:4:5"), "{err}");
}

#[test]
fn an_attached_doc_comment_does_not_warn() {
    let file = docs().join("shapes.kp");
    let r = kelpc().arg("lint").arg(&file).output().unwrap();
    assert!(r.status.success());
    assert!(!stderr(&r).contains("documents nothing"), "{}", stderr(&r));
}

#[test]
fn the_formatter_leaves_doc_comments_untouched() {
    let file = docs().join("shapes.kp");
    let src = read(&file);
    let r = kelpc().arg("fmt").arg(&file).output().unwrap();
    assert!(r.status.success(), "{}", stderr(&r));
    let out = String::from_utf8_lossy(&r.stdout).to_string();
    let docs_of = |s: &str| -> Vec<String> {
        s.lines()
            .filter(|l| l.trim_start().starts_with("///") || l.trim_start().starts_with("//!"))
            .map(str::to_string)
            .collect()
    };
    assert_eq!(docs_of(&src), docs_of(&out));
}

#[test]
fn lint_and_doc_still_accept_every_fixture() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures");
    let mut files: Vec<PathBuf> = fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "kp"))
        .collect();
    files.sort();
    assert!(!files.is_empty());
    for f in files {
        // `lint` and `doc` lex a file the same way, so one refuses it exactly
        // when the other does; and neither ever crashes on a fixture.
        let lint = kelpc().arg("lint").arg(&f).output().unwrap();
        let doc = run(&[f.to_str().unwrap(), "--check"]);
        assert!(doc.status.code().is_some(), "{}", f.display());
        assert_eq!(
            lint.status.success(),
            !stderr(&doc).contains("error: "),
            "{}: lint {} doc {}",
            f.display(),
            stderr(&lint),
            stderr(&doc)
        );
    }
}

#[test]
fn std_is_documented_from_its_own_sources() {
    let out = scratch("std");
    let r = run(&["--std", "-o", out.to_str().unwrap()]);
    assert!(r.status.success(), "{}", stderr(&r));
    let option = read(&out.join("std.option.md"));
    assert!(option.contains("# Module `std/option`") && option.contains("### `unwrap_or`"));
    assert!(read(&out.join("index.md")).contains("std.fs.md"));
    // The twelve most used modules have a doc on every public function.
    let missing = run(&["--std", "--check"]);
    let err = stderr(&missing);
    for m in ["str", "list", "option", "result", "fs", "io", "net", "http", "sync", "time", "math", "env"] {
        let bad: Vec<&str> = err
            .lines()
            .filter(|l| l.starts_with(&format!("std/{m}.kp:")) && l.contains("public fun"))
            .collect();
        assert!(bad.is_empty(), "undocumented in std/{m}: {bad:?}");
    }
}

/// A `///` above a `datac` documents the datac only: a field has its own doc
/// comment or none, and fields written on the `datac` line (or sharing a line
/// with another field) are undocumented.
#[test]
fn a_datac_doc_comment_is_not_inherited_by_its_fields() {
    let out = scratch("datac_fields");
    let source = docs().join("datac_fields.kp");
    let result = run(&[source.to_str().unwrap(), "-o", out.to_str().unwrap()]);
    assert!(result.status.success(), "{}", stderr(&result));
    let page = read(&out.join("datac_fields.md"));
    assert_eq!(page.matches("A point on the plane.").count(), 1, "{page}");
    assert_eq!(page.matches("A segment.").count(), 1, "{page}");
    assert_eq!(page.matches("A pair that starts on the opening line.").count(), 1, "{page}");
    assert_eq!(page.matches("Where it starts.").count(), 1, "{page}");
    assert_eq!(page.matches("The second.").count(), 1, "{page}");
    // `check` agrees: the undocumented fields are named, the documented ones are not.
    let check = run(&[source.to_str().unwrap(), "--check"]);
    let msg = stderr(&check);
    for undocumented in ["Point.x", "Point.y", "Segment.to", "Segment.len", "Pair.a"] {
        assert!(msg.contains(undocumented), "{undocumented} should be reported:\n{msg}");
    }
    for documented in ["Segment.from", "Pair.b"] {
        assert!(!msg.contains(documented), "{documented} must not be reported:\n{msg}");
    }
}
