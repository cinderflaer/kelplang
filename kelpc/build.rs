//! Builds the Rust runtime `kelp-rt/` into two static archives that ship inside
//! `kelpc` and are linked into every LLVM-built program (MK7 section 3.2):
//! `libkelp_rt.a`, the hosted runtime with the panic handler, and
//! `kelp_rt_managed.o`, the same runtime as one object with its byte buffers taken from
//! the collector. `driver.rs` embeds both with `include_bytes!`, so `kelpc`
//! needs nothing beside its own binary.
//!
//! The C runtime that stood here until the C removal (step C) is gone. The
//! `kelp_String__*_ptr` adapters the IR calls are defined in `kelp-rt`, next
//! to the by-value forms they wrap.

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

/// The directory holding OpenSSL's `libssl.so`: the `libdir` that `pkg-config`
/// reports for `openssl`, when it holds the library. `cc -print-file-name` is the
/// fallback, but on Nix the cc wrapper echoes the bare name for `libssl.so`
/// (it does not search `NIX_LDFLAGS`), so `pkg-config` is the route that works there.
fn tls_lib_dir() -> String {
    let from_pkg_config = first_line(
        Command::new("pkg-config")
            .args(["--variable=libdir", "openssl"])
            .output()
            .ok(),
    )
    .map(PathBuf::from)
    .filter(|d| d.join("libssl.so").is_file())
    .map(|d| d.display().to_string());
    from_pkg_config.unwrap_or_else(|| cc_file_dir("libssl.so"))
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

/// Records the glibc crt, libc, gcc library and libgcc_s directories for `sysroot.rs`
/// (`KELPC_CRT_DIR`, `KELPC_LIBC_DIR`, `KELPC_GCC_LIB_DIR`, `KELPC_LIBGCC_S_DIR`).
/// Build time only: kelpc itself does not run `cc` to find them.
fn bake_sysroot_dirs() {
    println!("cargo:rerun-if-env-changed=CC");
    println!("cargo:rerun-if-env-changed=PKG_CONFIG_PATH");
    let crt = cc_file_dir("crti.o");
    let libc = cc_file_dir("libc.so");
    let gcc = cc_file_dir("crtbeginS.o");
    let libgcc_s = cc_file_dir("libgcc_s.so");
    let tls = tls_lib_dir();
    println!("cargo:rustc-env=KELPC_CRT_DIR={crt}");
    println!("cargo:rustc-env=KELPC_LIBC_DIR={libc}");
    println!("cargo:rustc-env=KELPC_GCC_LIB_DIR={gcc}");
    println!("cargo:rustc-env=KELPC_LIBGCC_S_DIR={libgcc_s}");
    println!("cargo:rustc-env=KELPC_TLS_LIB_DIR={tls}");
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

/// What `build.rs` makes from `kelp-rt/`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Output {
    /// A static library with the `#[panic_handler]`: the hosted runtime, which
    /// links against nothing but libc.
    Staticlib,
    /// One relocatable object with no panic handler: the managed runtime. Its
    /// `core` references are resolved by the collector archive it links with,
    /// which is a staticlib and so carries the one panic handler a managed
    /// program needs. A staticlib here would carry a second one.
    Object,
}

/// One runtime variant `build.rs` builds from `kelp-rt/`.
struct RuntimeVariant {
    /// The nested build's target directory name under `$OUT_DIR`.
    dir: &'static str,
    /// The file name under `$OUT_DIR`, which `driver.rs` embeds.
    file: &'static str,
    /// The `kelp-rt` cargo feature the variant turns on.
    feature: &'static str,
    output: Output,
}

const HOSTED: RuntimeVariant = RuntimeVariant {
    dir: "kelp-rt",
    file: "libkelp_rt.a",
    feature: "panic-handler",
    output: Output::Staticlib,
};

const MANAGED: RuntimeVariant = RuntimeVariant {
    dir: "kelp-rt-managed",
    file: "kelp_rt_managed.o",
    feature: "managed",
    output: Output::Object,
};

/// Builds one runtime variant (`kelp-rt/`) into `$OUT_DIR/<file>`.
///
/// The primary path is a nested `cargo rustc --crate-type staticlib --locked
/// --offline` with its own target directory, so `libc` resolves from the root
/// `Cargo.lock` and the nested build never waits on the outer build's target
/// lock. If that fails, the fallback calls `rustc --crate-type staticlib`
/// directly with the `libc` rlib the nested build left behind.
fn build_rust_runtime(manifest_dir: &Path, out_dir: &Path, variant: &RuntimeVariant) {
    let repo = manifest_dir
        .parent()
        .expect("kelpc sits in the repository root's child directory");
    let rt_dir = repo.join("kelp-rt");
    let target = std::env::var("TARGET").expect("TARGET is set by cargo");
    let nested_dir = out_dir.join(variant.dir);
    let archive_out = out_dir.join(variant.file);
    let feature_cfg = format!("feature=\"{}\"", variant.feature);

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
        .args(["--features", variant.feature, "--target", &target, "--target-dir"])
        .arg(&nested_dir);
    match variant.output {
        Output::Staticlib => {
            cmd.args(["--crate-type", "staticlib"]);
        }
        Output::Object => {
            let mut emit = std::ffi::OsString::from("--emit=obj=");
            emit.push(&archive_out);
            cmd.arg("--").arg(emit);
        }
    }
    cmd.env("CARGO_PROFILE_RELEASE_PANIC", "abort")
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

    // A stale output must not pass for this build's, so it goes first.
    let _ = std::fs::remove_file(&archive_out);
    let nested_archive = nested_dir.join(&target).join("release").join("libkelp_rt.a");
    let nested_ok = match cmd.output() {
        Ok(out) if out.status.success() => match variant.output {
            Output::Staticlib => {
                let found = nested_archive.exists();
                if found {
                    std::fs::copy(&nested_archive, &archive_out)
                        .expect("could not copy the nested kelp-rt archive into OUT_DIR");
                }
                found
            }
            Output::Object => archive_out.exists(),
        },
        Ok(out) => {
            println!(
                "cargo:warning=nested cargo build of kelp-rt ({}) failed, trying rustc: {}",
                variant.feature,
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
    let crate_type = match variant.output {
        Output::Staticlib => "staticlib",
        Output::Object => "rlib",
    };
    fb.args(["--edition", "2024", "--crate-type", crate_type, "--crate-name", "kelp_rt"])
        .args(["--cfg", &feature_cfg, "--target", &target])
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
    if variant.output == Output::Object {
        fb.arg("--emit=obj");
    }
    for r in &remaps {
        fb.arg(r);
    }
    let status = fb
        .status()
        .expect("could not invoke rustc for the kelp-rt fallback");
    assert!(status.success(), "rustc failed to build the kelp-rt fallback archive");
}

/// Builds the collector archive `kelpc/gc` (crate `kelp_gc`, a staticlib with
/// its own workspace) into `$OUT_DIR/kelp-gc/release/libkelp_gc.a`, and bakes
/// that path in as `KELPC_GC_ARCHIVE`. A managed build links this archive when
/// `KELP_GC_LIB` is not set, so no build environment needs the variable.
fn build_collector(manifest_dir: &Path, out_dir: &Path) {
    let gc_dir = manifest_dir.join("gc");
    let target_dir = out_dir.join("kelp-gc");
    let archive = target_dir.join("release").join("libkelp_gc.a");

    for path in ["src", "include", "Cargo.toml", "Cargo.lock"] {
        println!("cargo:rerun-if-changed={}", gc_dir.join(path).display());
    }

    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let mut cmd = Command::new(cargo);
    cmd.current_dir(&gc_dir)
        .args(["build", "--release", "--locked", "--offline", "--target-dir"])
        .arg(&target_dir);
    for (name, _) in std::env::vars_os() {
        if let Some(name) = name.to_str() {
            if is_outer_build_variable(name) {
                cmd.env_remove(name);
            }
        }
    }
    let out = cmd
        .output()
        .expect("could not run cargo for the collector archive kelpc/gc");
    // A global `[build] target` (Nix's cargo setup writes one) moves the output to
    // `<target-dir>/<triple>/release`, so look there too before giving up.
    let triple_archive = target_dir
        .join(std::env::var("TARGET").unwrap_or_default())
        .join("release")
        .join("libkelp_gc.a");
    let archive = if archive.is_file() { archive } else { triple_archive };
    assert!(
        out.status.success() && archive.is_file(),
        "kelpc/gc (libkelp_gc.a) failed to build: {}",
        String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or("")
    );
    println!("cargo:rustc-env=KELPC_GC_ARCHIVE={}", archive.display());
}

fn main() {
    bake_build_info();
    bake_sysroot_dirs();
    let manifest_dir = PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by cargo"),
    );
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR is set by cargo"));
    build_rust_runtime(&manifest_dir, &out_dir, &HOSTED);
    build_rust_runtime(&manifest_dir, &out_dir, &MANAGED);
    build_collector(&manifest_dir, &out_dir);
    println!("cargo:rerun-if-changed=build.rs");
}
