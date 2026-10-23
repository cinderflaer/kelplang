//! The Rust witness for the `ffi_aggregate` projects, shared by the extern test
//! (`extern_aggregate.rs`) and the golden sweep (`golden_corpus.rs`). Both build
//! the fixture projects the same way, so both need the same static library.
//!
//! The projects carry no `native/` directory. `copy_project_with_witness` copies a
//! project and writes the witness into a `native/` beside the copy, where the
//! manifest's `library_dirs` finds it.

use std::fs;
use std::path::Path;
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

/// Copies the fixture project `src` into `dst` and writes the witness as the static
/// library `dst/native/libkelpaggwit.a`. `scratch` holds the witness source while
/// `rustc` compiles it.
pub fn copy_project_with_witness(src: &Path, dst: &Path, scratch: &Path) {
    copy_dir(src, dst);

    // The witness, as an archive the manifest's `library_dirs` names. An rlib is
    // an ar archive of the object, which the linker reads like a static library.
    let native = dst.join("native");
    fs::create_dir_all(&native).expect("create the native directory");
    fs::create_dir_all(scratch).expect("create the witness scratch directory");
    let witness = scratch.join("witness.rs");
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
}
