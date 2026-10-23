//! User externs that take or return an aggregate by value, lowered in LLVM IR.
//!
//! The fixture `projects/ffi_aggregate` declares externs over a 16-byte datac
//! (two eightbytes, INTEGER and SSE), a 16-byte datac whose first eightbyte
//! holds two floats, a 24-byte datac, `Bytes` and `String`. Each native symbol
//! is a Rust `extern "C"` function in `WITNESS` (`support/ffi_witness.rs`),
//! compiled by `rustc` into a static library and linked through the manifest's
//! `library_dirs`. Rust's
//! C-ABI lowering is the reference for the System V classes, so the program's
//! output is checked against the arithmetic the witness performs, not against
//! the compiler's own shape.
//!
//! No C source is compiled. The witness is Rust, and `cc` is the link driver
//! only, as for every program.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[path = "support/ffi_witness.rs"]
mod ffi_witness;

/// The lines the datac externs print: the witness's arithmetic on the fixture's
/// inputs, with floats in the `%f` form Kelp prints them in.
fn datac_output() -> String {
    // agg_pair_bump(Pair { a: 3, b: 2.5 })
    let (pa, pb) = (3i64 + 1, 2.5f64 * 2.0);
    // agg_mixed_bump(Mixed { x: 1.5, y: 2.25, n: 7 })
    let (mx, my, mn) = (1.5f32 + 1.0, 2.25f32 * 2.0, 7i64 + 10);
    // agg_wide_bump(Wide { a: 1, b: 2, c: 3 })
    let (wa, wb, wc) = (1i64 + 1, 2i64 + 2, 3i64 + 3);
    // agg_wide_total(Wide { a: 10, b: 20, c: 30 })
    let total = 10i64 + 20 + 30;
    // agg_trio_scale(Trio { x: 1.5, y: 2.0, z: 3.0 })
    let (tx, ty, tz) = (1.5f32 * 2.0, 2.0f32 * 2.0, 3.0f32 * 2.0);
    // agg_tag_next(Tag { kind: 7, flag: true, n: 21 })
    let (gk, gf, gn) = (7u8.wrapping_add(1), !true, 21u16 * 2);
    format!(
        "{pa}\n{pb:.6}\n{mx:.6}\n{my:.6}\n{mn}\n{wa}\n{wb}\n{wc}\n{total}\n\
         {tx:.6}\n{ty:.6}\n{tz:.6}\n{gk}\n{gf}\n{gn}\n"
    )
}

/// The whole fixture's output: the datac lines, then the `Bytes` and `String`
/// lengths.
fn expected_output() -> String {
    // agg_bytes_len("hello"), agg_bytes_make(5).len(), agg_string_len("abcdefg")
    let (hello, made, seven) = (5usize, 5usize, 7usize);
    format!("{}{hello}\n{made}\n{seven}\n", datac_output())
}

/// Builds the fixture `fixture` against the Rust witness, with `flags` on the
/// command line, and runs it, returning stdout.
fn build_and_run(fixture: &str, name: &str, flags: &[&str]) -> String {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&root);
    let project = root.join(fixture);
    ffi_witness::copy_project_with_witness(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/projects").join(fixture),
        &project,
        &root,
    );

    let bin = root.join("ffi_aggregate_bin");
    let build = Command::new(env!("CARGO_BIN_EXE_kelpc"))
        .env_remove("KELPC_BACKEND")
        .env_remove("KELPC_ORACLE")
        .arg("build")
        .arg(&project)
        .arg("-o")
        .arg(&bin)
        .args(flags)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected the aggregate fixture to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let run = Command::new(&bin).output().expect("failed to run the compiled fixture");
    assert!(
        run.status.success(),
        "the aggregate fixture exited with {:?}:\n{}",
        run.status,
        String::from_utf8_lossy(&run.stderr)
    );
    String::from_utf8_lossy(&run.stdout).into_owned()
}

#[test]
fn a_user_extern_passes_and_returns_aggregates_by_value() {
    let stdout = build_and_run("ffi_aggregate", "extern_aggregate_values", &[]);
    assert_eq!(stdout, expected_output());
}

/// The datac shapes alone, with no `Bytes` or `String`: the program the unit split
/// is meant for. A hosted build refuses `--incremental` (the shared pool), so
/// this one is built whole-program.
#[test]
fn a_user_extern_datac_shapes_pass_and_return_by_value() {
    let stdout = build_and_run("ffi_aggregate_datac", "extern_aggregate_datac", &[]);
    assert_eq!(stdout, datac_output());
}
