//! Managed mode, front end: `[target] mode = "managed"` is accepted by
//! `kelpc check`, and a resource that would enter a container is refused by
//! name. The fixtures live in `tests/managed/`.

use std::path::PathBuf;
use std::process::{Command, Output};

fn kelpc() -> Command {
    Command::new(env!("CARGO_BIN_EXE_kelpc"))
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/managed").join(name)
}

fn check(name: &str) -> Output {
    kelpc()
        .arg("check")
        .arg(fixture(name))
        .output()
        .expect("failed to run kelpc check")
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn a_managed_project_with_memory_values_checks_clean() {
    let out = check("memory_only");
    assert!(out.status.success(), "{}", text(&out));
}

#[test]
fn a_thread_in_a_list_is_refused_by_name_in_a_managed_project() {
    let out = check("refuse_thread_in_list");
    assert!(!out.status.success(), "a resource in a List must be refused");
    let msg = text(&out);
    assert!(msg.contains("'Thread<i32>' cannot be"), "{msg}");
}
