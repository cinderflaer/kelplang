//! Compiles the C runtime `runtime/kelp_rt.c` into an object that ships inside
//! `kelpc` and is linked into every LLVM-built program (MK7 §3.2).
//!
//! The file is the same text the C backend emits into each generated program,
//! kept verbatim so the two backends cannot diverge on the panic text, the
//! `Bytes`/`String` invariants or the capability matcher's meaning. The C
//! backend still compiles it as part of the generated `.c`; the LLVM backend
//! needs it as a separate object, so it is compiled once here and embedded in
//! the compiler binary with `include_bytes!` (see `driver.rs`). Embedding rather
//! than shipping a file beside the executable is what keeps `kelpc` independent
//! of its own install location.
//!
//! The runtime's functions are `static` in the verbatim text, which is what a
//! single generated translation unit wants. The IR calls them across an object
//! boundary, so this build strips `static` and supplies the standard headers the
//! text relies on the generated program for. One consequence is that
//! `kelp_panic` becomes a single external definition, so the LLVM lowering must
//! declare it rather than define a second copy.

use std::path::{Path, PathBuf};
use std::process::Command;

/// `YYYY-MM-DD` (UTC) for a Unix timestamp, by the civil-from-days algorithm, so
/// the build date is a pure function of `SOURCE_DATE_EPOCH` and needs no clock.
fn date_from_epoch(secs: i64) -> String {
    let z = secs.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02}")
}

fn first_line(out: Option<std::process::Output>) -> Option<String> {
    let out = out.filter(|o| o.status.success())?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// Bakes the version-report fields into the binary as `KELPC_*` env vars. Only
/// data that is fixed for a given commit goes in: the commit hash (`KELPC_COMMIT`
/// if set, else `git rev-parse HEAD`), the date from `SOURCE_DATE_EPOCH`, the
/// compiler's `rustc -V` and the LLVM version from `llvm-config`. No wall clock.
fn bake_build_info() {
    for var in ["KELPC_COMMIT", "SOURCE_DATE_EPOCH", "LLVM_SYS_211_PREFIX"] {
        println!("cargo:rerun-if-env-changed={var}");
    }
    let commit = std::env::var("KELPC_COMMIT")
        .ok()
        .filter(|c| !c.is_empty())
        .or_else(|| {
            first_line(
                Command::new("git")
                    .args(["rev-parse", "HEAD"])
                    .output()
                    .ok(),
            )
        })
        .unwrap_or_else(|| "unknown".to_string());
    let date = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|s| s.trim().parse::<i64>().ok())
        .map(date_from_epoch)
        .unwrap_or_else(|| "unknown".to_string());
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let rustc_version = first_line(Command::new(rustc).arg("-V").output().ok())
        .unwrap_or_else(|| "unknown".to_string());
    let llvm_config = std::env::var("LLVM_SYS_211_PREFIX")
        .map(|p| format!("{p}/bin/llvm-config"))
        .unwrap_or_else(|_| "llvm-config".to_string());
    let llvm = first_line(Command::new(llvm_config).arg("--version").output().ok())
        .or_else(|| first_line(Command::new("llvm-config").arg("--version").output().ok()))
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=KELPC_COMMIT={commit}");
    println!("cargo:rustc-env=KELPC_BUILD_DATE={date}");
    println!("cargo:rustc-env=KELPC_RUSTC_VERSION={rustc_version}");
    println!("cargo:rustc-env=KELPC_LLVM_VERSION={llvm}");
}

/// The directory holding `name` as `cc` sees it (`cc -print-file-name=<name>`),
/// or `None` when `cc` does not know the file. The result is empty, not an
/// error, so a build without a usable `cc` still succeeds: `sysroot.rs` then
/// falls back to the system directories at run time.
fn cc_file_dir(name: &str) -> String {
    let path = first_line(
        Command::new("cc")
            .arg(format!("-print-file-name={name}"))
            .output()
            .ok(),
    )
    .map(PathBuf::from);
    // `cc` echoes the bare name back when it cannot find the file.
    path.filter(|p| p.is_absolute() && p.is_file())
        .and_then(|p| p.parent().map(|d| d.display().to_string()))
        .unwrap_or_default()
}

/// Records the glibc crt, libc and gcc library directories for `sysroot.rs`
/// (`KELPC_CRT_DIR`, `KELPC_LIBC_DIR`, `KELPC_GCC_LIB_DIR`). Build time only:
/// kelpc itself does not run `cc` to find them.
fn bake_sysroot_dirs() {
    println!("cargo:rerun-if-env-changed=CC");
    let crt = cc_file_dir("crti.o");
    let libc = cc_file_dir("libc.so");
    let gcc = cc_file_dir("crtbeginS.o");
    println!("cargo:rustc-env=KELPC_CRT_DIR={crt}");
    println!("cargo:rustc-env=KELPC_LIBC_DIR={libc}");
    println!("cargo:rustc-env=KELPC_GCC_LIB_DIR={gcc}");
}

/// The variables a nested cargo must not inherit from this build script. The
/// outer build's flags and per-package metadata would otherwise leak into the
/// runtime's profile, features and target directory.
fn is_outer_build_variable(name: &str) -> bool {
    const EXACT: &[&str] = &[
        "RUSTFLAGS",
        "CARGO_ENCODED_RUSTFLAGS",
        "CARGO_TARGET_DIR",
        "CARGO_MANIFEST_DIR",
        "CARGO_MANIFEST_LINKS",
        "OUT_DIR",
        "HOST",
        "TARGET",
        "PROFILE",
        "OPT_LEVEL",
        "DEBUG",
        "NUM_JOBS",
    ];
    EXACT.contains(&name)
        || name.starts_with("CARGO_PKG_")
        || name.starts_with("CARGO_CFG_")
        || name.starts_with("CARGO_FEATURE_")
}

/// Builds the Rust runtime (`kelp-rt/`) into `$OUT_DIR/libkelp_rt.a`.
///
/// The primary path is a nested `cargo rustc --crate-type staticlib --locked
/// --offline` with its own
/// target directory, so `libc` resolves from the root `Cargo.lock` and the nested
/// build never waits on the outer build's target lock. If that fails, the
/// fallback calls `rustc --crate-type staticlib` directly with the `libc` rlib
/// the nested build left behind.
fn build_rust_runtime(manifest_dir: &Path, out_dir: &Path) {
    let repo = manifest_dir
        .parent()
        .expect("kelpc sits in the repository root's child directory");
    let rt_dir = repo.join("kelp-rt");
    let target = std::env::var("TARGET").expect("TARGET is set by cargo");
    let nested_dir = out_dir.join("kelp-rt");
    let archive_out = out_dir.join("libkelp_rt.a");

    println!("cargo:rerun-if-changed={}", rt_dir.join("src").display());
    println!("cargo:rerun-if-changed={}", rt_dir.join("Cargo.toml").display());
    println!("cargo:rerun-if-changed={}", repo.join("Cargo.lock").display());

    // Remap the source roots so the archive does not carry this checkout's
    // absolute paths; the same commit then builds the same bytes anywhere.
    let mut remaps = vec![
        format!("--remap-path-prefix={}=.", repo.display()),
        format!("--remap-path-prefix={}=.", out_dir.display()),
    ];
    if let Some(home) = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cargo")))
    {
        remaps.push(format!("--remap-path-prefix={}=/cargo", home.display()));
    }
    let mut rustflags = vec![
        "-C".to_string(),
        "force-unwind-tables=yes".to_string(),
        "-C".to_string(),
        "relocation-model=pic".to_string(),
    ];
    rustflags.extend(remaps.iter().cloned());

    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let mut cmd = Command::new(cargo);
    cmd.current_dir(repo)
        .args(["rustc", "-p", "kelp-rt", "--lib", "--release", "--locked", "--offline"])
        .args(["--crate-type", "staticlib"])
        .args(["--features", "panic-handler", "--target", &target, "--target-dir"])
        .arg(&nested_dir)
        .env("CARGO_PROFILE_RELEASE_PANIC", "abort")
        .env("CARGO_PROFILE_RELEASE_OPT_LEVEL", "2")
        .env("CARGO_PROFILE_RELEASE_CODEGEN_UNITS", "1")
        .env("CARGO_PROFILE_RELEASE_DEBUG", "0")
        .env("CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS", "false")
        .env("CARGO_ENCODED_RUSTFLAGS", rustflags.join("\x1f"));
    for (name, _) in std::env::vars_os() {
        if let Some(name) = name.to_str() {
            if is_outer_build_variable(name) {
                cmd.env_remove(name);
            }
        }
    }

    let nested_archive = nested_dir.join(&target).join("release").join("libkelp_rt.a");
    let nested_ok = match cmd.output() {
        Ok(out) if out.status.success() => nested_archive.exists(),
        Ok(out) => {
            println!(
                "cargo:warning=nested cargo build of kelp-rt failed, trying rustc: {}",
                String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or("")
            );
            false
        }
        Err(e) => {
            println!("cargo:warning=could not run cargo for kelp-rt: {e}");
            false
        }
    };
    if nested_ok {
        std::fs::copy(&nested_archive, &archive_out)
            .expect("could not copy the nested kelp-rt archive into OUT_DIR");
        return;
    }

    // Fallback: rustc directly. It needs the `libc` rlib, which the nested
    // build leaves under its deps directory; a missing rlib means the nested
    // build did not get that far, and the build stops with the reason.
    let deps = nested_dir.join(&target).join("release").join("deps");
    let libc_rlib = std::fs::read_dir(&deps).ok().and_then(|entries| {
        entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .find(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("liblibc-") && n.ends_with(".rlib"))
            })
    });
    let libc_rlib = libc_rlib.expect(
        "kelp-rt could not be built: cargo failed and no libc rlib exists for the rustc fallback",
    );
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let mut fb = Command::new(rustc);
    fb.args(["--edition", "2024", "--crate-type", "staticlib", "--crate-name", "kelp_rt"])
        .args(["--cfg", "feature=\"panic-handler\"", "--target", &target])
        .args(["-C", "opt-level=2", "-C", "panic=abort", "-C", "codegen-units=1"])
        .args(["-C", "debuginfo=0", "-C", "overflow-checks=off"])
        .args(["-C", "force-unwind-tables=yes", "-C", "relocation-model=pic"])
        .arg("--extern")
        .arg(format!("libc={}", libc_rlib.display()))
        .arg("-L")
        .arg(format!("dependency={}", deps.display()))
        .arg("-o")
        .arg(&archive_out)
        .arg(rt_dir.join("src/lib.rs"));
    for r in &remaps {
        fb.arg(r);
    }
    let status = fb
        .status()
        .expect("could not invoke rustc for the kelp-rt fallback");
    assert!(status.success(), "rustc failed to build the kelp-rt fallback archive");
}

fn main() {
    bake_build_info();
    bake_sysroot_dirs();
    build_rust_runtime(
        &PathBuf::from(
            std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by cargo"),
        ),
        &PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR is set by cargo")),
    );
    let manifest_dir = PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by cargo"),
    );
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR is set by cargo"));
    let source = manifest_dir.join("runtime/kelp_rt.c");
    println!("cargo:rerun-if-changed={}", source.display());
    println!("cargo:rerun-if-changed=build.rs");

    // The generated C program supplies these headers before the runtime text;
    // compiled on its own the text has none, so the wrapper is what lets the
    // same file build both ways. `#define static` is the one deliberate
    // difference: it gives the helpers the external linkage the IR needs.
    let wrapper = out_dir.join("kelp_rt_export.c");
    let body = format!(
        "/* Generated by build.rs from runtime/kelp_rt.c. Do not edit. */\n\
         #include <errno.h>\n\
         #include <netdb.h>\n\
         #include <pthread.h>\n\
         #include <stdatomic.h>\n\
         #include <stdbool.h>\n\
         #include <signal.h>\n\
         #include <stddef.h>\n\
         #include <stdint.h>\n\
         #include <stdio.h>\n\
         #include <stdlib.h>\n\
         #include <string.h>\n\
         /* The socket surface the shared dial in `runtime/kelp_rt.c` needs:
\
            `sys/socket.h` and `sys/types.h` for `socket`/`connect`, and
\
            `unistd.h` for `close`. The generated program supplies the same
\
            headers before the same text. */\n\
         #include <sys/socket.h>\n\
         #include <sys/types.h>\n\
         #include <unistd.h>\n\
         #define static\n\
         #include \"{}\"\n\
         \n\
         /* The pointer-spelled entry point for `String.from_bytes`, whose C form\n\
            takes a 24-byte `kelp_Bytes` by value. An IR call cannot cross such an\n\
            argument (MK7 section 3.3, BUG-vN4tR), so the IR declares this adapter\n\
            and hands it a pointer. It is C to C inside this translation unit, where\n\
            both sides are compiled by the same compiler and agree, and the buffer\n\
            is moved into the result exactly as the by-value form moves it. */\n\
         kelp_String kelp_String__from_bytes_ptr(const kelp_Bytes* b) {{\n\
             return kelp_String__from_bytes(*b);\n\
         }}\n\
         \n\
         /* The same for the reverse direction: `kelp_String__into_bytes` takes a
\
            24-byte `kelp_String` by value and returns a 24-byte `kelp_Bytes`, and
\
            an IR call cannot cross either by value (MK7 section 3.3,
\
            BUG-vN4tR). The adapter takes the receiver by pointer, so the IR
\
            stages the `String` into a slot of its own and the buffer moves into
\
            the result exactly as the by-value form moves it. */\n\
         kelp_Bytes kelp_String__into_bytes_ptr(const kelp_String* self) {{\n\
             return kelp_String__into_bytes(*self);\n\
         }}\n",
        source.display()
    );
    std::fs::write(&wrapper, body).expect("could not write the runtime wrapper");

    let object = out_dir.join("kelp_rt.o");
    // `-pthread` because the runtime text now reaches `pthread_create`,
    // `pthread_exit` and `pthread_join` (MK9-S3-M1 stage C1): it makes the
    // thread-safe macros visible and links the library on systems whose libc
    // does not carry them.
    let status = Command::new("cc")
        .args(["-c", "-O2", "-fPIC", "-fwrapv", "-pthread"])
        // Keep the build directory out of the object's file symbol and any
        // `__FILE__`, so the same commit builds the same bytes anywhere.
        .arg(format!("-ffile-prefix-map={}=.", out_dir.display()))
        .arg(format!("-ffile-prefix-map={}=.", manifest_dir.display()))
        .arg("-o")
        .arg(&object)
        .arg(&wrapper)
        .status()
        .expect("could not invoke 'cc' to build the Kelp runtime object");
    assert!(
        status.success(),
        "cc failed to build the Kelp runtime object from runtime/kelp_rt.c"
    );

    // The managed variant (`KELP_GC_MANAGED`): the same text, with its String
    // and Bytes buffers allocated from the collector through `kelp_gc.h`. A
    // managed link takes this object instead of the hosted one.
    let managed_object = out_dir.join("kelp_rt_managed.o");
    let gc_include = manifest_dir.join("gc").join("include");
    let status = Command::new("cc")
        .args(["-c", "-O2", "-fPIC", "-fwrapv", "-pthread", "-DKELP_GC_MANAGED"])
        .arg(format!("-I{}", gc_include.display()))
        .arg(format!("-ffile-prefix-map={}=.", out_dir.display()))
        .arg(format!("-ffile-prefix-map={}=.", manifest_dir.display()))
        .arg("-o")
        .arg(&managed_object)
        .arg(&wrapper)
        .status()
        .expect("could not invoke 'cc' to build the managed Kelp runtime object");
    assert!(
        status.success(),
        "cc failed to build the managed Kelp runtime object from runtime/kelp_rt.c"
    );
    println!("cargo:rerun-if-changed={}", gc_include.join("kelp_gc.h").display());
}
