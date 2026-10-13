//! User externs that take or return an aggregate by value, lowered in LLVM IR.
//!
//! The fixture `projects/ffi_aggregate` declares externs over a 16-byte datac
//! (two eightbytes, INTEGER and SSE), a 16-byte datac whose first eightbyte
//! holds two floats, a 24-byte datac, `Bytes` and `String`. Each native symbol
//! is a Rust `extern "C"` function in `WITNESS` below, compiled by `rustc` into
//! a static library and linked through the manifest's `library_dirs`. Rust's
//! C-ABI lowering is the reference for the System V classes, so the program's
//! output is checked against the arithmetic the witness performs, not against
//! the compiler's own shape.
//!
//! No C source is compiled. The witness is Rust, and `cc` is the link driver
//! only, as for every program.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The native side of the fixture's externs, in the ABI `rustc` gives
/// `extern "C"`. Every function is a no_std leaf that cannot panic, and it
/// allocates only through `malloc`, which the link already provides. It has no
/// panic handler of its own, so it links beside the runtime's without a second
/// `rust_begin_unwind`.
const WITNESS: &str = r#"#![no_std]

unsafe extern "C" {
    fn malloc(n: usize) -> *mut u8;
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Pair {
    a: i64,
    b: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Mixed {
    x: f32,
    y: f32,
    n: i64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Wide {
    a: i64,
    b: i64,
    c: i64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Bytes {
    data: *mut u8,
    len: usize,
    cap: usize,
}

#[unsafe(no_mangle)]
pub extern "C" fn agg_pair_bump(p: Pair) -> Pair {
    Pair { a: p.a + 1, b: p.b * 2.0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn agg_mixed_bump(m: Mixed) -> Mixed {
    Mixed { x: m.x + 1.0, y: m.y * 2.0, n: m.n + 10 }
}

#[unsafe(no_mangle)]
pub extern "C" fn agg_wide_bump(w: Wide) -> Wide {
    Wide { a: w.a + 1, b: w.b + 2, c: w.c + 3 }
}

#[unsafe(no_mangle)]
pub extern "C" fn agg_wide_total(w: Wide) -> i64 {
    w.a + w.b + w.c
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Trio {
    x: f32,
    y: f32,
    z: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Tag {
    kind: u8,
    flag: bool,
    n: u16,
}

#[unsafe(no_mangle)]
pub extern "C" fn agg_trio_scale(t: Trio) -> Trio {
    Trio { x: t.x * 2.0, y: t.y * 2.0, z: t.z * 2.0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn agg_tag_next(t: Tag) -> Tag {
    Tag { kind: t.kind.wrapping_add(1), flag: !t.flag, n: t.n * 2 }
}

#[unsafe(no_mangle)]
pub extern "C" fn agg_bytes_len(b: Bytes) -> usize {
    b.len
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn agg_bytes_make(n: usize) -> Bytes {
    unsafe {
        let data = malloc(n + 1);
        let mut i = 0;
        while i < n {
            *data.add(i) = b'Q';
            i += 1;
        }
        *data.add(n) = 0;
        Bytes { data, len: n, cap: n }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn agg_string_len(s: Bytes) -> usize {
    s.len
}
"#;

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

/// Copies the fixture project into `dst`, so the native library can be written
/// beside its manifest without touching the checked-in tree.
fn copy_dir(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).expect("create the project copy");
    for entry in fs::read_dir(src).expect("read the fixture directory") {
        let entry = entry.expect("read a fixture entry");
        let target = dst.join(entry.file_name());
        if entry.file_type().expect("the entry type").is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).expect("copy a fixture file");
        }
    }
}

/// Builds the fixture `fixture` against the Rust witness, with `flags` on the
/// command line, and runs it, returning stdout.
fn build_and_run(fixture: &str, name: &str, flags: &[&str]) -> String {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&root);
    let project = root.join(fixture);
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/projects").join(fixture),
        &project,
    );

    // The witness, as an archive the manifest's `library_dirs` names. An rlib is
    // an ar archive of the object, which the linker reads like a static library.
    let native = project.join("native");
    fs::create_dir_all(&native).expect("create the native directory");
    let witness = root.join("witness.rs");
    fs::write(&witness, WITNESS).expect("write the Rust witness");
    let lib = native.join("libkelpaggwit.a");
    let rustc = Command::new("rustc")
        .args(["--edition", "2024", "--crate-type", "rlib", "-C", "panic=abort", "-O", "-o"])
        .arg(&lib)
        .arg(&witness)
        .output()
        .expect("failed to spawn rustc");
    assert!(
        rustc.status.success(),
        "the Rust witness did not compile:\n{}",
        String::from_utf8_lossy(&rustc.stderr)
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
