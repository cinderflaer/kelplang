//! The example folders share one copy of each `.kp` file they all use.
//!
//! An example's `src/` holds either its own file or a symbolic link into
//! `tests/fixtures/examples/shared/`, never a second copy of a shared file.
//! The compiler reads a link as the file it names, so the module id and every
//! diagnostic path stay the same (issue #305).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

fn examples() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/examples")
}

/// Every `.kp` under an example's `src/`, except the fetched `deps/` tree.
fn example_sources() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in fs::read_dir(examples()).expect("reading tests/fixtures/examples") {
        let dir = entry.expect("reading an examples entry").path();
        if !dir.is_dir() || dir.file_name() == Some("shared".as_ref()) {
            continue;
        }
        let src = dir.join("src");
        if src.is_dir() {
            collect(&src, &mut out);
        }
    }
    out.sort();
    out
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("reading an example's src") {
        let path = entry.expect("reading a src entry").path();
        if path.file_name() == Some("deps".as_ref()) {
            continue;
        }
        if path.is_dir() && !path.is_symlink() {
            collect(&path, out);
        } else if path.extension() == Some("kp".as_ref()) {
            out.push(path);
        }
    }
}

fn is_link(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .expect("stat an example source")
        .file_type()
        .is_symlink()
}

fn relative(path: &Path) -> String {
    path.strip_prefix(examples())
        .unwrap_or(path)
        .display()
        .to_string()
}

#[test]
fn no_example_source_is_a_copy_of_another() {
    let mut by_bytes: BTreeMap<Vec<u8>, Vec<String>> = BTreeMap::new();
    for path in example_sources() {
        if is_link(&path) {
            continue;
        }
        let bytes = fs::read(&path).expect("reading an example source");
        by_bytes.entry(bytes).or_default().push(relative(&path));
    }
    let copies: Vec<String> = by_bytes
        .values()
        .filter(|group| group.len() > 1)
        .map(|group| group.join(", "))
        .collect();
    assert!(
        copies.is_empty(),
        "{} identical copies of a shared file; link each one to examples/shared instead:\n{}",
        copies.len(),
        copies.join("\n")
    );
}

#[test]
fn every_shared_file_is_linked_and_every_link_points_into_shared() {
    let shared = examples().join("shared");
    assert!(shared.is_dir(), "missing {}", relative(&shared));
    let shared_canon = fs::canonicalize(&shared).expect("canonicalizing examples/shared");

    let mut problems = Vec::new();
    let mut linked: BTreeSet<PathBuf> = BTreeSet::new();
    for path in example_sources() {
        if !is_link(&path) {
            continue;
        }
        match fs::canonicalize(&path) {
            Ok(target) if target.starts_with(&shared_canon) => {
                linked.insert(target);
            }
            Ok(target) => problems.push(format!(
                "{} links to {}, which is outside examples/shared",
                relative(&path),
                target.display()
            )),
            Err(e) => problems.push(format!("{} does not resolve: {e}", relative(&path))),
        }
    }

    let mut unlinked = Vec::new();
    for entry in fs::read_dir(&shared).expect("reading examples/shared") {
        let path = entry.expect("reading a shared entry").path();
        if path.extension() != Some("kp".as_ref()) {
            continue;
        }
        let canon = fs::canonicalize(&path).expect("canonicalizing a shared file");
        if !linked.contains(&canon) {
            unlinked.push(relative(&path));
        }
    }

    assert!(
        problems.is_empty() && unlinked.is_empty(),
        "shared copies are not wired up:\n{}\nshared files with no link:\n{}",
        problems.join("\n"),
        unlinked.join("\n")
    );
}
