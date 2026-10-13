//! `kelpc doc` (`kelp wiki page tooling/kelpc-doc.md`): the pages for a small module that holds
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

// ---------------------------------------------------------------------------
// `kelpc doc --format json`, `--text`, and `kelpc stdlib --json` (K1, K2, K3,
// K5). The JSON is compared as text with a checked-in document, because the
// tests carry no JSON parser; the fixture is read from its own directory so
// every path in the output is relative and the same on every machine.
// ---------------------------------------------------------------------------

/// Runs `kelpc doc ...` with the fixture directory as the working directory,
/// so file paths in the output are relative.
fn run_in_json_fixture(args: &[&str], stdin: Option<&str>) -> Output {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = kelpc()
        .arg("doc")
        .args(args)
        .current_dir(docs().join("json"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn kelpc");
    {
        let mut pipe = child.stdin.take().expect("stdin pipe");
        // An empty stdin is written too, so a command that reads it sees EOF.
        let _ = pipe.write_all(stdin.unwrap_or("").as_bytes());
    }
    child.wait_with_output().expect("wait for kelpc")
}

#[test]
fn json_document_matches_the_expected_file() {
    let r = run_in_json_fixture(&["src", "--format", "json"], None);
    assert!(r.status.success(), "{}", stderr(&r));
    let got = String::from_utf8(r.stdout).expect("utf-8 output");
    let want = read(&docs().join("expected").join("json").join("project.json"));
    assert_eq!(want, got, "the JSON document differs from tests/docs/expected/json/project.json");
}

#[test]
fn json_document_is_deterministic() {
    let a = run_in_json_fixture(&["src", "--format", "json"], None);
    let b = run_in_json_fixture(&["src", "--format", "json"], None);
    assert!(a.status.success() && b.status.success());
    assert_eq!(a.stdout, b.stdout);
}

#[test]
fn json_document_has_the_trait_impl_alias_and_nested_module() {
    let r = run_in_json_fixture(&["src", "--format", "json"], None);
    let text = String::from_utf8(r.stdout).expect("utf-8 output");
    // Modules come sorted by id: the dependency, the entry, the nested file.
    let ids = ["\"id\":\"deps/argparse/lib\"", "\"id\":\"main\"", "\"id\":\"shapes/circle\""];
    let mut last = 0;
    for needle in ids {
        let at = text.find(needle).unwrap_or_else(|| panic!("missing {needle}"));
        assert!(at >= last, "modules are not sorted by id");
        last = at;
    }
    // The trait impl names its trait and its target type.
    assert!(
        text.contains("\"trait_impls\":[{\"trait\":\"Shape\",\"target\":\"Circle\""),
        "trait impl missing: {text}"
    );
    // A dependency import is aliased by the compiler's own rule.
    assert!(
        text.contains("{\"path\":\"deps/argparse/lib\",\"alias\":\"argparse\""),
        "dependency alias missing: {text}"
    );
    assert!(text.contains("{\"path\":\"shapes/circle\",\"alias\":\"circle\""));
    // The compiler's builtin table comes with the document.
    assert!(text.contains("\"builtins\":[{\"key\":\"Bytes#append\""));
    assert!(text.contains("{\"key\":\"String#len\",\"type\":\"String\",\"name\":\"len\""));
}

#[test]
fn json_name_spans_point_at_the_name_in_the_file() {
    let r = run_in_json_fixture(&["src", "--format", "json"], None);
    let text = String::from_utf8(r.stdout).expect("utf-8 output");
    let circle = read(&docs().join("json").join("src").join("shapes").join("circle.kp"));
    // (name, the text that comes right before it in circle.kp): the span is
    // the bytes of the name, found at a known place rather than by search.
    let cases = [
        ("Shape", "trait "),
        ("area", "Self."),
        ("Circle", "datac "),
        ("r", "val "),
    ];
    for (name, before) in cases {
        let at = circle.find(&format!("{before}{name}")).unwrap_or_else(|| panic!("{name} in circle.kp"));
        let start = at + before.len();
        let span = format!("[{start},{}]", start + name.len());
        assert_eq!(&circle[start..start + name.len()], name);
        assert!(text.contains(&format!("\"name\":\"{name}\",")), "{name} missing");
        assert!(text.contains(&format!("\"name_span\":{span}")), "no span {span} for {name}");
    }
}

#[test]
fn json_from_stdin_documents_the_buffer() {
    let buffer = "/// Doubles.\npub fun twice(n: i32).return: i32 = n * 2\n";
    let r = run_in_json_fixture(&["--text", "src/buffer.kp", "--format", "json"], Some(buffer));
    assert!(r.status.success(), "{}", stderr(&r));
    let text = String::from_utf8(r.stdout).expect("utf-8 output");
    assert!(text.contains("\"id\":\"buffer\",\"file\":\"src/buffer.kp\""), "{text}");
    let start = buffer.find("twice").unwrap();
    let span = format!("\"name_span\":[{start},{}]", start + "twice".len());
    assert!(text.contains(&span), "{span} missing from {text}");
    assert!(text.contains("\"doc\":\"Doubles.\""));
    // The buffer is the only module: the project's files are not read.
    assert!(!text.contains("shapes/circle"));
}

#[test]
fn text_needs_exactly_one_file_name() {
    let r = run_in_json_fixture(&["--text", "src", "--format", "json"], Some(""));
    assert!(!r.status.success());
    assert!(stderr(&r).contains("'--text' needs exactly one"), "{}", stderr(&r));
}

#[test]
fn json_format_writes_to_stdout_not_a_directory() {
    let out = scratch("json_no_dir");
    let r = run_in_json_fixture(&["src", "--format", "json", "-o", out.to_str().unwrap()], None);
    assert!(!r.status.success());
    assert!(stderr(&r).contains("writes to stdout"), "{}", stderr(&r));
}

/// The stdlib listing writes every shipped module under `<dir>/<build>/` and
/// names the synthetic `std/thread` calls.
#[test]
fn stdlib_listing_writes_the_shipped_sources_and_lists_them() {
    let dir = scratch("stdlib_list");
    let r = kelpc()
        .args(["stdlib", "--json", "--dir", dir.to_str().unwrap()])
        .output()
        .expect("spawn kelpc");
    assert!(r.status.success(), "{}", stderr(&r));
    let text = String::from_utf8(r.stdout).expect("utf-8 output");
    for id in ["std/iter", "std/sync", "std/ops", "std/convert"] {
        assert!(text.contains(&format!("\"id\":\"{id}\"")), "{id} not listed: {text}");
    }
    for name in ["fence", "spawn", "join"] {
        assert!(text.contains(&format!("{{\"name\":\"{name}\",\"kind\":\"fun\"")), "{name} not listed");
    }
    // The build id names the directory, and it is a 64-digit SHA-256.
    let build = text
        .split("\"build\":\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("a build id");
    assert_eq!(build.len(), 64, "build id {build}");
    assert!(build.bytes().all(|b| b.is_ascii_hexdigit()));
    assert!(dir.join(build).is_dir());
    // The file written for a module is the shipped source, byte for byte.
    let shipped = Path::new(env!("CARGO_MANIFEST_DIR")).join("std");
    for module in ["iter", "sync", "ops", "convert"] {
        let written = read(&dir.join(build).join("std").join(format!("{module}.kp")));
        assert_eq!(written, read(&shipped.join(format!("{module}.kp"))), "std/{module}");
    }
    // A second run finds the same bytes and prints the same listing.
    let again = kelpc()
        .args(["stdlib", "--json", "--dir", dir.to_str().unwrap()])
        .output()
        .expect("spawn kelpc");
    assert_eq!(text.as_bytes(), again.stdout.as_slice());
}

#[test]
fn stdlib_needs_json_and_a_directory() {
    let plain = kelpc().args(["stdlib"]).output().expect("spawn kelpc");
    assert!(!plain.status.success());
    let no_dir = kelpc().args(["stdlib", "--json"]).output().expect("spawn kelpc");
    assert!(!no_dir.status.success());
    assert!(stderr(&no_dir).contains("--dir"), "{}", stderr(&no_dir));
}
