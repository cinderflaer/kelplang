use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::opt::OptLevel;

/// The C runtime object, compiled from `runtime/kelp_rt.c` by `build.rs` and
/// embedded in the compiler binary. The driver writes it to a temporary file at
/// link time, so `kelpc` needs nothing beside its own binary and does not depend
/// on its install location (MK7 §3.2).
static RUNTIME_OBJECT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/kelp_rt.o"));

/// Makes the temporary runtime object's name unique within one process: several
/// compilations can run at once (the test suite does), and two of them must not
/// write the same path.
static RUNTIME_TMP: AtomicU64 = AtomicU64::new(0);

pub struct CompileError {
    pub message: String,
}

/// Writes `c_source` to `<out_path>.c` (kept alongside the binary, not
/// deleted, so a generated-C debugging session is a simple `gdb ./foo`
/// against a file that's actually there) and shells out to `cc -O<n>` to
/// produce the native binary at `out_path`.
///
/// `opt` is the selected optimization level, and it is the same `-O` the LLVM
/// backend and the final link get: one level means one level for both backends
/// (MK7 S6, the record in `plan/done.md`), so the differential sweep compares
/// them at one level rather than two.
///
/// This is the C backend's *finishing* step, and the only place in the
/// compiler that invokes an external tool. A later backend does not call it: it
/// writes its own object and links it, and this file's whole dependency on a
/// system C compiler ends at this function.
pub fn compile_c(
    c_source: &str,
    out_path: &Path,
    opt: OptLevel,
) -> Result<(), CompileError> {
    let c_path = out_path.with_extension("c");
    std::fs::write(&c_path, c_source).map_err(|e| CompileError {
        message: format!("could not write generated C to '{}': {e}", c_path.display()),
    })?;

    let output = Command::new("cc")
        .arg(opt.cc_flag())
        .arg("-o")
        .arg(out_path)
        .arg(&c_path)
        // `-fwrapv` makes signed overflow wrap instead of being UB, which is
        // what the plan's arithmetic-overflow section promises. `-lm` is for
        // `std/math`'s externs (an unused library is free to link). `-pthread`
        // is for the generated program's `thread.spawn`/`thread.join`, whose
        // runtime half is part of the generated C (MK9-S3-M1 stage C1). `-lssl
        // -lcrypto` are for `std/tls`'s externs (MK5-S4b-M1, open item 37): the
        // system OpenSSL is linked unconditionally, the same "an unused library
        // is free to link" reasoning as `-lm`.
        .arg("-fwrapv")
        .arg("-pthread")
        .arg("-lm")
        .arg("-lssl")
        .arg("-lcrypto")
        .output()
        .map_err(|e| CompileError {
            message: format!("could not invoke 'cc': {e}"),
        })?;

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

/// Compiles one generated C translation unit into an object, the way
/// [`compile_c`] compiles a whole program's generated C. The LLVM backend needs
/// it for the `extern fun` bodies, which stay C (MK7 §3.2) but whose object is
/// emitted per program: see `codegen_llvm::externs_c_source` for why a fixed
/// object cannot carry them.
///
/// The flags are the ones the C backend compiles under at the same level
/// (`-O<n> -fwrapv`), so a body that does arithmetic at the edge of a signed
/// range wraps the way the C backend's `-O<n> -fwrapv` compiled copy of it does.
/// `-Werror=implicit-function-declaration` is the one extra: the bodies call the
/// runtime helpers through prototypes written out beside them, and C99 makes an
/// undeclared call a warning, so a helper whose prototype went missing would be
/// called through an invented `int` signature instead of failing the build.
pub fn compile_c_object(
    c_source: &str,
    c_path: &Path,
    obj_path: &Path,
    opt: OptLevel,
) -> Result<(), CompileError> {
    std::fs::write(c_path, c_source).map_err(|e| CompileError {
        message: format!("could not write generated C to '{}': {e}", c_path.display()),
    })?;
    let output = Command::new("cc")
        .arg("-c")
        .arg(opt.cc_flag())
        .arg("-fwrapv")
        // The same reason the runtime object's own build passes it: an `extern
        // fun` body may reach the thread runtime's helpers (MK9-S3-M1 stage C1).
        .arg("-pthread")
        .arg("-Werror=implicit-function-declaration")
        .arg("-o")
        .arg(obj_path)
        .arg(c_path)
        .output()
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
/// backend's *finishing* step, the sibling of [`compile_c`].
///
/// The same `cc` invocation as `compile_c`, and for the same reason (MK7 §1.3
/// keeps the C linker, so `glibc_lib_dir()` and the `crt1.o` lookup stay
/// unchanged), with two differences: there is no `-fwrapv` here, and the runtime
/// object the compiler embeds is written to a temporary file and linked
/// alongside. `-fwrapv` is a C-frontend flag that pins signed overflow; the LLVM
/// backend emits plain `add`/`sub`/`mul` IR, which wraps by definition, so the
/// guarantee is already in the object and the flag would be inert. `-lm` is for
/// `std/math`'s externs and is free whether or not they are used.
///
/// `externs_object` is the object [`compile_c_object`] built from the program's
/// `extern fun` bodies, or `None` when the program reaches none of them. It is
/// an ordinary object on the link line, after the IR object and before `-lm`, so
/// a body that calls a runtime helper resolves against the runtime object above
/// it and one that calls `sqrt` resolves against the library below.
///
/// `opt` is the selected level's `-O` flag on the link line. At link time it is
/// inert without LTO, but it is the same flag `compile_c` passes, so the two
/// backends' finishing steps stay one vocabulary (MK7 S6).
pub fn link_llvm_object(
    object_path: &Path,
    externs_object: Option<&Path>,
    out_path: &Path,
    opt: OptLevel,
) -> Result<(), CompileError> {
    let runtime_path = std::env::temp_dir().join(format!(
        "kelp_rt_{}_{}.o",
        std::process::id(),
        RUNTIME_TMP.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&runtime_path, RUNTIME_OBJECT).map_err(|e| CompileError {
        message: format!(
            "could not write the runtime object to '{}': {e}",
            runtime_path.display()
        ),
    })?;
    let mut command = Command::new("cc");
    command
        .arg(opt.cc_flag())
        .arg("-o")
        .arg(out_path)
        .arg(object_path)
        .arg(&runtime_path)
        // The runtime object holds `pthread_create`/`pthread_join` now (MK9-S3-M1
        // stage C1), so the link has to name the threading library.
        .arg("-pthread");
    if let Some(externs) = externs_object {
        command.arg(externs);
    }
    let output = command
        .arg("-lm")
        // `std/tls`'s externs need the system OpenSSL (MK5-S4b-M1, open item
        // 37); an unused library is free to link, the same as `-lm` above.
        .arg("-lssl")
        .arg("-lcrypto")
        .output()
        .map_err(|e| CompileError {
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
