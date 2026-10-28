//! Phase 2 of the camelCase naming change (issue #376, after phase 1 in #375).
//!
//! Phase 1 renamed the standard library's functions and methods to camelCase
//! and kept each snake_case name as an outdated alias. Phase 2 moves the test
//! programs and the examples to the camelCase calls, so that a program under
//! `kelpc/tests` never calls a renamed name in its snake_case spelling, and
//! `kelpc check` prints no `outdated:` finding for one of the examples.
//!
//! The set of renamed names is read from the register in
//! `kelpc/src/deprecated.rs`: every row whose `instead` names issue #375.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const REGISTER: &str = include_str!("../src/deprecated.rs");

fn manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The snake_case names that phase 1 renamed: the `what` of each register row
/// whose `instead` says "(issue #375)", without the dot and the parenthesis.
fn renamed_names() -> Vec<String> {
    let mut names = Vec::new();
    for block in REGISTER.split("Outdated {").skip(1) {
        // Only the std rows: the unit tests of the module repeat the marker
        // in a sample entry, which is not part of the register.
        if !block.trim_start().starts_with("id: \"std-") || !block.contains("(issue #375)") {
            continue;
        }
        let Some(rest) = block.split("what: \"").nth(1) else {
            continue;
        };
        let what = rest.split('"').next().unwrap_or("");
        let name = what
            .trim_matches('`')
            .trim_start_matches('.')
            .trim_end_matches('(');
        if !name.is_empty() {
            names.push(name.to_string());
        }
    }
    names.sort();
    names.dedup();
    names
}

fn kp_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(dir).unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()));
    for entry in entries {
        let path = entry.expect("directory entry").path();
        if path.is_dir() {
            kp_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "kp") {
            out.push(path);
        }
    }
}

#[test]
fn the_register_names_the_phase_one_set() {
    let names = renamed_names();
    assert_eq!(names.len(), 182, "the phase 1 set is 182 names: {names:?}");
    assert!(names.iter().any(|n| n == "add_field"));
    assert!(!names.iter().any(|n| n == "is_empty"), "is_empty is a builtin follow-up");
    assert!(!names.iter().any(|n| n == "from_u64"), "str.from_u64 is the #262 path");
}

/// `String.char_count()` is a builtin method of the compiler, not a std name,
/// and it has no camelCase spelling yet. These programs keep that call, and
/// only that name, in snake_case until the builtin is renamed (a follow-up of
/// #376).
const BUILTIN_STRING_CALLS: &[&str] = &[
    "tests/fixtures/string_basic.kp",
    "tests/managed/text_poll/src/main.kp",
    "tests/projects/freestanding_alloc_string/src/main.kp",
];

#[test]
fn no_test_program_calls_a_renamed_name_in_snake_case() {
    let names = renamed_names();
    let mut files = Vec::new();
    kp_files(&manifest().join("tests"), &mut files);
    files.sort();
    let mut found = Vec::new();
    for file in &files {
        let text = fs::read_to_string(file).unwrap_or_default();
        for (index, line) in text.lines().enumerate() {
            let rel = file.strip_prefix(manifest()).unwrap_or(file);
            let rel_text = rel.display().to_string();
            for name in &names {
                if name == "char_count" && BUILTIN_STRING_CALLS.iter().any(|f| rel_text == *f) {
                    continue;
                }
                let call = format!(".{name}(");
                if line.contains(&call) {
                    found.push(format!("{rel_text}:{}: {call}", index + 1));
                }
            }
        }
    }
    assert!(
        found.is_empty(),
        "{} call(s) still use a renamed snake_case name:\n{}",
        found.len(),
        found.join("\n")
    );
}

#[test]
fn no_example_prints_an_outdated_finding() {
    let mut files = Vec::new();
    kp_files(&manifest().join("tests/fixtures/examples"), &mut files);
    files.sort();
    assert!(!files.is_empty(), "the examples directory has programs");
    let mut found = Vec::new();
    for file in &files {
        let out = Command::new(env!("CARGO_BIN_EXE_kelpc"))
            .arg("check")
            .arg(file)
            .output()
            .expect("failed to spawn kelpc");
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        // Only the findings for a renamed name: the register also holds older
        // retirements, which are not part of this phase.
        for line in text
            .lines()
            .filter(|l| l.starts_with("outdated: ") && l.contains("(issue #375)"))
        {
            let rel = file.strip_prefix(manifest()).unwrap_or(file).display();
            found.push(format!("{rel}: {line}"));
        }
    }
    assert!(
        found.is_empty(),
        "{} outdated finding(s) on the examples:\n{}",
        found.len(),
        found.join("\n")
    );
}
