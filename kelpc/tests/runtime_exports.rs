//! The export set of the Rust runtime archive (stage 1 WP0 completed by WP6).
//!
//! The archive must export exactly the names `kelp-rt/src/abi.rs` declares in
//! `PROTOTYPES`, plus the two exported data symbols `kelp_argc` and `kelp_argv`.
//! Nothing else with a `kelp_` prefix may be a global symbol: the private
//! helpers of the C runtime (the env cache, the child table, the thread key)
//! must stay private (plan section 1, Q11). The C runtime's two thread-local
//! and file-scope statics are named in the forbidden list too, so a rename
//! that exports one of them again fails by name.
//!
//! The check reads `nm -g --defined-only` over the archive that WP0's
//! `build.rs` writes to `OUT_DIR`. It is ignored until WP0 lands; run it with
//! `cargo test --test runtime_exports -- --ignored`.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Data symbols the archive exports. They are not in `PROTOTYPES`, which lists
/// functions only.
const EXPORTED_DATA: &[&str] = &["kelp_argc", "kelp_argv"];

/// Names that must never be global symbols of the archive. The first group is
/// the private helpers the C runtime has today, one per plan section 1 row
/// marked `internal`, and the file-scope statics `build.rs` makes global
/// today (plan section 0, fact 5 and Q11).
const FORBIDDEN: &[&str] = &[
    "kelp_thread_panic_text",
    "kelp_env_hash",
    "kelp_env_build",
    "kelp_env_ensure",
    "kelp_env_publish",
    "kelp_thread_release",
    "kelp_thread_tramp",
    "kelp_mutex_guard_release",
    "kelp_rwlock_read_end",
    "kelp_rwlock_write_end",
    "kelp_rwlock_write_poison",
    "kelp_collapse",
    "kelp_resolve_path",
    "kelp_host_pattern_is_numeric",
    "kelp_pattern_host_len",
    "kelp_numeric_pattern_admits",
    "kelp_thread_self",
    "kelp_env_table",
    "kelp_env_epoch",
    "kelp_env_readers",
    "kelp_env_mutex",
    "kelp_child_mutex",
    "kelp_child_pids",
    "kelp_child_len",
    "kelp_child_cap",
];

fn abi_source() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../kelp-rt/src/abi.rs")
}

fn archive() -> PathBuf {
    match option_env!("OUT_DIR") {
        Some(dir) => PathBuf::from(dir).join("libkelp_rt.a"),
        None => panic!("OUT_DIR is not set for this test build"),
    }
}

/// The names in the `PROTOTYPES` table: every first string literal of a pair.
fn declared_functions() -> BTreeSet<String> {
    let src = fs::read_to_string(abi_source())
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", abi_source().display()));
    let start = src
        .find("pub const PROTOTYPES")
        .expect("kelp-rt/src/abi.rs has no `pub const PROTOTYPES` table");
    let body = &src[start..];
    let end = body.find("];").expect("PROTOTYPES table is not closed with `];`");
    let literals: Vec<String> = body[..end]
        .split('"')
        .enumerate()
        .filter(|(i, _)| i % 2 == 1)
        .map(|(_, s)| s.to_string())
        .collect();
    literals.chunks(2).map(|pair| pair[0].clone()).collect()
}

/// The global symbols the archive defines whose names start with `kelp_`.
fn exported_kelp_symbols() -> BTreeSet<String> {
    let out = Command::new("nm")
        .args(["-g", "--defined-only"])
        .arg(archive())
        .output()
        .expect("failed to run nm");
    assert!(
        out.status.success(),
        "nm failed on {}:\n{}",
        archive().display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| line.split_whitespace().last())
        .filter(|name| name.starts_with("kelp_"))
        .map(|name| name.to_string())
        .collect()
}

#[test]
#[ignore = "stage1 WP0: needs kelp-rt/src/abi.rs PROTOTYPES and the libkelp_rt.a archive"]
fn the_archive_exports_exactly_the_prototypes_and_the_two_data_symbols() {
    let mut expected = declared_functions();
    for data in EXPORTED_DATA {
        expected.insert(data.to_string());
    }
    let actual = exported_kelp_symbols();

    let missing: Vec<&String> = expected.difference(&actual).collect();
    let extra: Vec<&String> = actual.difference(&expected).collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "exported kelp_* symbols differ from PROTOTYPES plus {EXPORTED_DATA:?}\n\
         missing from the archive: {missing:?}\nexported but not declared: {extra:?}"
    );
}

#[test]
#[ignore = "stage1 WP0: needs the libkelp_rt.a archive"]
fn no_private_helper_is_exported() {
    let actual = exported_kelp_symbols();
    let leaked: Vec<&&str> = FORBIDDEN.iter().filter(|name| actual.contains(**name)).collect();
    assert!(leaked.is_empty(), "private runtime helpers are exported: {leaked:?}");
}

#[test]
#[ignore = "stage1 WP0: needs the libkelp_rt.a archive"]
fn the_thread_local_loopback_flag_is_gone_and_its_accessors_are_exported() {
    // Stable Rust cannot export a thread-local symbol (plan section 3.4). The
    // flag is read and written through two functions instead.
    let actual = exported_kelp_symbols();
    assert!(!actual.contains("kelp_listen_loopback"), "the TLS flag is still exported");
    assert!(actual.contains("kelp_listen_loopback_set"), "kelp_listen_loopback_set is not exported");
    assert!(actual.contains("kelp_listen_loopback_take"), "kelp_listen_loopback_take is not exported");
}
