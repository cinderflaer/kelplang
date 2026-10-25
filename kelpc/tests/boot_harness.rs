//! The boot harness that replaced the per-example `run.sh` scripts (issue #303).
//!
//! Every example whose coral.toml carries a `[run]` table is booted by
//! `kelpc boot` and must pass: each `expect` line appears whole in the serial
//! output of its QEMU machine. The examples here are the 21 that used to be
//! run by a `run.sh`; the four that kept one (`kernel0-panic`, `kernel0-panic2`,
//! `kernel0-sched`, `kernel0-x86_64-idt`) have checks a table cannot state.
//! Needs `qemu-system-*` and `ld.lld` on PATH, which the gate's nix shell provides.

use std::path::PathBuf;
use std::process::Command;

const BOOTED: &[&str] = &[
    "kernel0",
    "kernel0-aarch64",
    "kernel0-bss",
    "kernel0-caps",
    "kernel0-eq",
    "kernel0-eq-x86_64",
    "kernel0-heap",
    "kernel0-percpu",
    "kernel0-stack",
    "kernel0-statics",
    "kernel0-string",
    "kernel0-trap",
    "kernel0-user",
    "kernel0-vm",
    "kernel0-vm-fault",
    "kernel0-x86_64",
    "kernel0-x86_64-lmode",
    "polyp-fb-riscv64",
    "polyp-fb-x86_64",
    "polyp-v4-riscv64",
    "polyp-v4-x86_64",
];

#[test]
fn every_migrated_example_boots_under_kelpc_boot() {
    let examples = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/examples");
    let mut failures = Vec::new();
    for name in BOOTED {
        let dir = examples.join(name);
        let out = Command::new(env!("CARGO_BIN_EXE_kelpc"))
            .arg("boot")
            .arg(&dir)
            .output()
            .expect("kelpc boot should start");
        if !out.status.success() {
            failures.push(format!(
                "{name}: exit {:?}\nstdout:\n{}\nstderr:\n{}",
                out.status.code(),
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            ));
        }
    }
    assert!(failures.is_empty(), "{} example(s) failed to boot:\n{}", failures.len(), failures.join("\n---\n"));
}

#[test]
fn kelpc_boot_refuses_a_project_without_a_run_table() {
    let examples = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/examples");
    let out = Command::new(env!("CARGO_BIN_EXE_kelpc"))
        .arg("boot")
        .arg(examples.join("kernel0-panic2"))
        .output()
        .expect("kelpc boot should start");
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("has no [run] table"), "{err}");
}
