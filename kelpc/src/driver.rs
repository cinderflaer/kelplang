use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::guard::GuardedOutput;
use crate::opt::OptLevel;

/// The C runtime object, compiled from `runtime/kelp_rt.c` by `build.rs` and
/// embedded in the compiler binary. The driver writes it to a temporary file at
/// link time, so `kelpc` needs nothing beside its own binary and does not depend
/// on its install location (MK7 §3.2).
static RUNTIME_OBJECT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/kelp_rt.o"));
/// The managed runtime object (`KELP_GC_MANAGED`): the same runtime, with its
/// byte buffers allocated from the collector. A managed link takes it.
static RUNTIME_OBJECT_MANAGED: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/kelp_rt_managed.o"));
/// The Rust runtime archive, built from `kelp-rt/` by `build.rs`. Embedded but not
/// linked yet: the link switches to it in stage 1 WP7.
#[allow(dead_code)]
static RUNTIME_ARCHIVE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/libkelp_rt.a"));

/// Makes the temporary runtime object's name unique within one process: several
/// compilations can run at once (the test suite does), and two of them must not
/// write the same path.
static RUNTIME_TMP: AtomicU64 = AtomicU64::new(0);

/// A sanitizer named by `[build] sanitize`. The C backend that built sanitized
/// programs was removed on 2026-10-10, and the LLVM backend links no sanitizer
/// passes, so the manifest key is refused at build time. The type stays so the
/// manifest still parses and the refusal can name the kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sanitizer {
    Address,
    Undefined,
    Thread,
}

impl Sanitizer {
    pub fn parse(name: &str) -> Option<Sanitizer> {
        match name {
            "address" => Some(Sanitizer::Address),
            "undefined" => Some(Sanitizer::Undefined),
            "thread" => Some(Sanitizer::Thread),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Sanitizer::Address => "address",
            Sanitizer::Undefined => "undefined",
            Sanitizer::Thread => "thread",
        }
    }
}

pub struct CompileError {
    pub message: String,
}

/// Compiles one generated C translation unit into an object. The LLVM backend
/// needs it for the `extern fun` bodies, which stay C (MK7 §3.2) but whose object
/// is emitted per program: see `codegen_llvm::externs_c_source` for why a fixed
/// object cannot carry them.
///
/// The flags are `-O<n> -fwrapv`, so a body that does arithmetic at the edge of a
/// signed range wraps the way the rest of the program does.
/// `-Werror=implicit-function-declaration` is the one extra: the bodies call the
/// runtime helpers through prototypes written out beside them, and C99 makes an
/// undeclared call a warning, so a helper whose prototype went missing would be
/// called through an invented `int` signature instead of failing the build.
pub fn compile_c_object(
    c_source: &str,
    c_path: &Path,
    obj_path: &Path,
    opt: OptLevel,
    include_dirs: &[String],
    pkg_config_flags: &[String],
) -> Result<(), CompileError> {
    std::fs::write(c_path, c_source).map_err(|e| CompileError {
        message: format!("could not write generated C to '{}': {e}", c_path.display()),
    })?;
    let mut command = Command::new("cc");
    command
        .arg("-c")
        .arg(opt.cc_flag())
        .arg("-fwrapv")
        // The same reason the runtime object's own build passes it: an `extern
        // fun` body may reach the thread runtime's helpers (MK9-S3-M1 stage C1).
        .arg("-pthread")
        .arg("-Werror=implicit-function-declaration");
    for flag in dir_flags(include_dirs, &[]) {
        command.arg(flag);
    }
    // The LLVM backend compiles the generated C for a program's `extern fun`
    // bodies here, so a package's `-I` has to reach this invocation as well
    // (MK9-S1-M1).
    for flag in pkg_config_flags {
        command.arg(flag);
    }
    let output = command
        .arg("-o")
        .arg(obj_path)
        .arg(c_path)
        .guarded_output()
        .map_err(|e| CompileError {
            message: format!("could not invoke 'cc': {e}"),
        })?;
    if !output.status.success() {
        return Err(CompileError {
            message: format!(
                "cc failed to build '{}':\n{}",
                c_path.display(),
                String::from_utf8_lossy(&output.stderr)
            ),
        });
    }
    Ok(())
}

/// Links the object `codegen_llvm` wrote into the final binary: the LLVM
/// backend's *finishing* step. The C linker is kept (MK7 §1.3), so
/// `glibc_lib_dir()` and the `crt1.o` lookup stay unchanged. The runtime object
/// the compiler embeds is written to a temporary file and linked alongside.
/// There is no `-fwrapv` here: the LLVM backend emits plain `add`/`sub`/`mul`
/// IR, which wraps by definition. `-lm` is for `std/math`'s externs and is free
/// whether or not they are used.
///
/// `externs_object` is the object [`compile_c_object`] built from the program's
/// `extern fun` bodies, or `None` when the program reaches none of them. It is
/// an ordinary object on the link line, after the IR object and before `-lm`, so
/// a body that calls a runtime helper resolves against the runtime object above
/// it and one that calls `sqrt` resolves against the library below.
///
/// `opt` is the selected level's `-O` flag on the link line. At link time it is
/// inert without LTO, but it is the same vocabulary the per-program object's `cc` call takes
/// (MK7 S6).
pub fn link_llvm_object(
    object_path: &Path,
    externs_object: Option<&Path>,
    out_path: &Path,
    opt: OptLevel,
    link_libs: &[String],
    library_dirs: &[String],
    pkg_config_flags: &[String],
    tls: bool,
) -> Result<(), CompileError> {
    link_llvm_objects(
        std::slice::from_ref(&object_path.to_path_buf()),
        externs_object,
        out_path,
        opt,
        link_libs,
        library_dirs,
        pkg_config_flags,
        tls,
    )
}

/// Links a managed build's object (WP3): the program's object, the embedded
/// runtime, the program's `extern fun` object when there is one, then the
/// collector archive `gc_archive` (`kelpc/gc`'s static library, which defines
/// `kelp_gc_*`), then the libraries the manifest names and `-ldl` for the Rust
/// side of that archive. Only a managed build calls this; hosted and
/// freestanding builds link exactly as before.
///
/// The archive comes after the program's objects so their `kelp_gc_*`
/// references resolve against it. `managed_archive` finds the archive.
pub fn link_managed_object(
    object_path: &Path,
    externs_object: Option<&Path>,
    gc_archive: &Path,
    out_path: &Path,
    opt: OptLevel,
    link_libs: &[String],
    library_dirs: &[String],
    pkg_config_flags: &[String],
    tls: bool,
) -> Result<(), CompileError> {
    let mut libs = link_libs.to_vec();
    libs.push("dl".to_string());
    link_llvm_objects_with_archive(
        std::slice::from_ref(&object_path.to_path_buf()),
        externs_object,
        Some(gc_archive),
        out_path,
        opt,
        &libs,
        library_dirs,
        pkg_config_flags,
        tls,
    )
}

/// The collector archive a managed build links: the path in `KELP_GC_LIB`,
/// which must name an existing file. A managed build with no archive is an
/// error that names the variable, never a link that fails on `kelp_gc_init`.
pub fn managed_archive() -> Result<PathBuf, CompileError> {
    let path = std::env::var_os("KELP_GC_LIB").map(PathBuf::from).ok_or_else(|| CompileError {
        message: "a managed build links the collector archive, and KELP_GC_LIB does not name it: set KELP_GC_LIB to the libkelp_gc.a built from kelpc/gc (cargo build --release)".to_string(),
    })?;
    if !path.is_file() {
        return Err(CompileError {
            message: format!(
                "KELP_GC_LIB names '{}', which is not a file: build the collector archive from kelpc/gc",
                path.display()
            ),
        });
    }
    Ok(path)
}

/// Links several objects into the final binary: the incremental mode's
/// finishing step (roadmap A-14 slice 1), where each compilation unit is its
/// own object and the link line carries all of them.
///
/// The invocation is [`link_llvm_object`]'s, in the same argument order: every
/// unit object first (in the order the units were emitted, which is a function
/// of the program's own module list), then the runtime object, then the
/// program's `extern fun` object when there is one, then the libraries.
///
/// `objects` must not be empty: a build that produced no object has nothing to
/// link, and the caller reports that as its own error rather than letting `cc`
/// invent a linker message for it.
pub fn link_llvm_objects(
    objects: &[PathBuf],
    externs_object: Option<&Path>,
    out_path: &Path,
    opt: OptLevel,
    link_libs: &[String],
    library_dirs: &[String],
    pkg_config_flags: &[String],
    tls: bool,
) -> Result<(), CompileError> {
    link_llvm_objects_with_archive(
        objects,
        externs_object,
        None,
        out_path,
        opt,
        link_libs,
        library_dirs,
        pkg_config_flags,
        tls,
    )
}

/// [`link_llvm_objects`] with the managed build's collector archive, which
/// goes after the program's `extern fun` object and before `-lm`.
fn link_llvm_objects_with_archive(
    objects: &[PathBuf],
    externs_object: Option<&Path>,
    gc_archive: Option<&Path>,
    out_path: &Path,
    opt: OptLevel,
    link_libs: &[String],
    library_dirs: &[String],
    pkg_config_flags: &[String],
    tls: bool,
) -> Result<(), CompileError> {
    let runtime_path = std::env::temp_dir().join(format!(
        "kelp_rt_{}_{}.o",
        std::process::id(),
        RUNTIME_TMP.fetch_add(1, Ordering::Relaxed)
    ));
    // A managed link (the collector archive is named) takes the managed runtime.
    let runtime_bytes = if gc_archive.is_some() { RUNTIME_OBJECT_MANAGED } else { RUNTIME_OBJECT };
    std::fs::write(&runtime_path, runtime_bytes).map_err(|e| CompileError {
        message: format!(
            "could not write the runtime object to '{}': {e}",
            runtime_path.display()
        ),
    })?;
    let mut command = Command::new("cc");
    command
        .arg(opt.cc_flag())
        .arg("-o")
        .arg(out_path);
    for object in objects {
        command.arg(object);
    }
    command
        .arg(&runtime_path)
        // The runtime object holds `pthread_create`/`pthread_join` now (MK9-S3-M1
        // stage C1), so the link has to name the threading library.
        .arg("-pthread");
    if let Some(externs) = externs_object {
        command.arg(externs);
    }
    if let Some(archive) = gc_archive {
        command.arg(archive);
    }
    command.arg("-lm");
    for flag in dir_flags(&[], library_dirs) {
        command.arg(flag);
    }
    // A package's `-L`/`-l` has to be on the link line too (MK9-S1-M1).
    for flag in pkg_config_flags {
        command.arg(flag);
    }
    // `std/tls`'s externs need the system OpenSSL.
    if tls {
        command.arg("-lssl").arg("-lcrypto");
    }
    // A `coral.toml` `[extern."<module id>"]` section's `library` (MK9-S1-M1)
    // has to be on this link line too, or the build would fail with an
    // undefined symbol.
    for lib in link_libs {
        command.arg(format!("-l{lib}"));
    }
    let output = command.guarded_output().map_err(|e| CompileError {
        message: format!("could not invoke 'cc': {e}"),
    })?;
    // The object is embedded in `kelpc`; the file was only a place for `cc` to
    // read from, and leaving one per compile behind would be litter.
    let _ = std::fs::remove_file(&runtime_path);

    if !output.status.success() {
        return Err(CompileError {
            message: format!(
                "cc failed:\n{}",
                String::from_utf8_lossy(&output.stderr)
            ),
        });
    }

    Ok(())
}

/// Links a freestanding object into an ELF with `ld.lld` (kernel stage 4).
///
/// This is the LLVM backend's *other* finishing step: no `cc`, no libc, no
/// `-lm`, no runtime object. The linker script places the sections and defines
/// the symbols the generated `_start` reads, and `ld.lld` is the linker the
/// bare-metal LLVM toolchains supply. A missing `ld.lld` is an error naming it,
/// never a fallback to `cc`: `cc` would link the hosted startup files back in,
/// which is the one thing a freestanding image must not have.
///
/// `linker_script` is the manifest's `[target] linker_script`, already resolved
/// against the directory holding `coral.toml`.
pub fn link_freestanding_object(
    object_path: &Path,
    linker_script: &Path,
    out_path: &Path,
) -> Result<(), CompileError> {
    let output = Command::new("ld.lld")
        .arg("-T")
        .arg(linker_script)
        .arg("-o")
        .arg(out_path)
        .arg("--static")
        .arg(object_path)
        .guarded_output()
        .map_err(|e| CompileError {
            message: format!(
                "could not invoke 'ld.lld': {e}; a freestanding build needs 'ld.lld' on PATH"
            ),
        })?;
    if !output.status.success() {
        return Err(CompileError {
            message: format!(
                "ld.lld failed:\n{}",
                String::from_utf8_lossy(&output.stderr).trim_end()
            ),
        });
    }
    Ok(())
}

/// The `-I<dir>` and `-L<dir>` flags for a project's `[extern.*]` include and
/// library directories, in that order. Pure, so the flag shape is tested
/// without invoking `cc`.
pub fn dir_flags(include_dirs: &[String], library_dirs: &[String]) -> Vec<String> {
    include_dirs
        .iter()
        .map(|dir| format!("-I{dir}"))
        .chain(library_dirs.iter().map(|dir| format!("-L{dir}")))
        .collect()
}

/// The argument list for one `pkg-config` invocation over `packages`: the
/// `--cflags --libs` request first, then the packages themselves, sorted and
/// deduplicated so the same set of manifests always runs the same command.
/// Pure, so the flag shape is tested without running `pkg-config`.
pub fn pkg_config_args(packages: &[String]) -> Vec<String> {
    let mut sorted: Vec<&String> = packages.iter().collect();
    sorted.sort();
    sorted.dedup();
    let mut args = vec!["--cflags".to_string(), "--libs".to_string()];
    args.extend(sorted.into_iter().cloned());
    args
}

/// Splits one `pkg-config` stdout into individual flags, in the order it
/// printed them. `pkg-config` separates them with whitespace (`-I/x -L/y
/// -lfoo -pthread -DBAR=1`), so whitespace is the separator. Pure.
pub fn split_pkg_config_output(output: &str) -> Vec<String> {
    output.split_whitespace().map(str::to_string).collect()
}

/// Runs one `pkg-config --cflags --libs` over `packages` and returns the flags
/// split from its stdout (MK9-S1-M1). A missing `pkg-config` or a non-zero
/// exit is an error naming the packages and carrying `pkg-config`'s stderr;
/// the build never silently links without the requested packages.
pub fn pkg_config_flags(packages: &[String]) -> Result<Vec<String>, CompileError> {
    if packages.is_empty() {
        return Ok(Vec::new());
    }
    let output = Command::new("pkg-config")
        .args(pkg_config_args(packages))
        .guarded_output()
        .map_err(|e| CompileError {
            message: format!(
                "could not invoke 'pkg-config' for {}: {e}",
                packages.join(", ")
            ),
        })?;
    if !output.status.success() {
        return Err(CompileError {
            message: format!(
                "pkg-config failed for {}:\n{}",
                packages.join(", "),
                String::from_utf8_lossy(&output.stderr).trim_end()
            ),
        });
    }
    Ok(split_pkg_config_output(&String::from_utf8_lossy(&output.stdout)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dir_flags_name_includes_then_libraries() {
        let flags = dir_flags(
            &["/a".to_string(), "/b".to_string()],
            &["/c".to_string()],
        );
        assert_eq!(flags, ["-I/a", "-I/b", "-L/c"]);
    }

    #[test]
    fn no_dirs_yield_no_flags() {
        assert!(dir_flags(&[], &[]).is_empty());
    }

    #[test]
    fn pkg_config_args_request_flags_then_sorted_packages() {
        let args = pkg_config_args(&["zlib".to_string(), "gtk4".to_string(), "zlib".to_string()]);
        assert_eq!(args, ["--cflags", "--libs", "gtk4", "zlib"]);
    }

    #[test]
    fn pkg_config_output_splits_on_whitespace_in_order() {
        assert_eq!(
            split_pkg_config_output("-I/x -L/y -lfoo -pthread -DBAR=1\n"),
            ["-I/x", "-L/y", "-lfoo", "-pthread", "-DBAR=1"]
        );
        assert!(split_pkg_config_output("").is_empty());
    }

    #[test]
    fn no_packages_never_runs_pkg_config() {
        let flags = pkg_config_flags(&[])
            .unwrap_or_else(|e| panic!("no invocation needed: {}", e.message));
        assert!(flags.is_empty());
    }
}
