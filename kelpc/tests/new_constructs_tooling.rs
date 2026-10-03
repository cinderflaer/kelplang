//! Language-completeness 8: `kelpc fmt`, `kelpc lint` and `kelpc check --json`
//! on every construct the 2026-10 language rounds added (associated types and
//! `Self::Item`/`I::Item` projections, generic traits and `From`, `&dyn` and
//! `Box<dyn>`, operator and `Display` traits, const generics, iterators with
//! `break`/`continue`, `?` conversion, trailing blocks with exclusive captures).
//! The one source is `tests/fixtures/language_tour_all.kp`; the refusals are
//! existing fixtures, so the JSON an editor reads is exercised on the error
//! paths of the same features and must never be a crash.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn kelpc() -> Command {
    Command::new(env!("CARGO_BIN_EXE_kelpc"))
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

fn scratch(name: &str) -> PathBuf {
    let root = match option_env!("CARGO_TARGET_TMPDIR") {
        Some(dir) => PathBuf::from(dir),
        None => std::env::temp_dir().join("kelpc-constructs-tests"),
    };
    let dir = root.join("constructs").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create the scratch directory");
    dir
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).to_string()
}

fn run(args: &[&str], path: &Path) -> Output {
    kelpc().args(args).arg(path).output().expect("failed to spawn kelpc")
}

#[test]
fn fmt_reformats_every_new_construct_idempotently_and_keeps_the_program() {
    let dir = scratch("fmt");
    let file = dir.join("tour.kp");
    // Start from a deliberately ragged copy so the formatter has work to do.
    let source = fs::read_to_string(fixture("language_tour_all.kp")).unwrap();
    let ragged: String = source
        .lines()
        .map(|l| format!("{}\n", l.trim_start()))
        .collect();
    fs::write(&file, ragged).unwrap();
    let out = run(&["fmt", "-w"], &file);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let once = fs::read_to_string(&file).unwrap();
    let check = run(&["fmt", "--check"], &file);
    assert!(check.status.success(), "fmt is not idempotent: {}", text(&check.stderr));
    let again = run(&["fmt", "-w"], &file);
    assert!(again.status.success());
    assert_eq!(once, fs::read_to_string(&file).unwrap());
    // The formatted program is still the same program.
    let checked = run(&["check"], &file);
    assert!(checked.status.success(), "{}", text(&checked.stderr));
}

#[test]
fn lint_accepts_every_new_construct_without_failing() {
    let out = run(&["lint"], &fixture("language_tour_all.kp"));
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(!text(&out.stderr).contains("panicked"));
}

#[test]
fn check_json_is_clean_for_every_new_construct() {
    let out = run(&["check", "--json"], &fixture("language_tour_all.kp"));
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(text(&out.stdout), "{\"schema\":1,\"ok\":true,\"diagnostics\":[]}\n");
}

#[test]
fn check_json_reports_each_new_refusal_as_a_finding_not_a_crash() {
    for name in [
        "traits_missing_method.kp",
        "traits_assoc_missing.kp",
        "for_iter_no_impl.kp",
        "from_no_impl.kp",
        "const_generics_mismatch.kp",
        "traits_dyn_unsafe_self.kp",
        "traits_bound_unimplemented.kp",
        "traits_boxdyn_take.kp",
        "ops_arith_no_add.kp",
        "ops_display_missing.kp",
        "matrix_assoc_boxdyn.kp",
        "matrix_generic_trait_operator.kp",
        "matrix_from_try_in_next.kp",
        "matrix_iter_const_generic.kp",
        "excl_refused_val.kp",
        "excl_refused_nested.kp",
        "excl_refused_same_call_arg.kp",
    ] {
        let out = run(&["check", "--json"], &fixture(name));
        assert!(!out.status.success(), "{name} should be refused");
        let json = text(&out.stdout);
        assert!(
            json.starts_with("{\"schema\":1,\"ok\":false,\"diagnostics\":["),
            "{name}: {json}"
        );
        assert!(json.contains("\"severity\":\"error\""), "{name}: {json}");
        assert!(json.contains("\"line\":"), "{name}: {json}");
        assert!(json.ends_with("}\n"), "{name}: {json}");
        assert!(!text(&out.stderr).contains("panicked"), "{name}: {}", text(&out.stderr));
    }
}

#[test]
fn fmt_and_lint_keep_every_refusal_fixture_lexable() {
    // A refused program still lexes, so the editor can format and lint it while
    // the checker is complaining.
    for name in [
        "matrix_assoc_boxdyn.kp",
        "matrix_iter_const_generic.kp",
        "excl_refused_nested.kp",
        "excl_capture_view.kp",
    ] {
        let lint = run(&["lint"], &fixture(name));
        assert!(lint.status.success(), "{name}: {}", text(&lint.stderr));
        let fmt = run(&["fmt", "--check"], &fixture(name));
        assert!(!text(&fmt.stderr).contains("panicked"), "{name}");
    }
}
