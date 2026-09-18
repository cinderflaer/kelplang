use std::path::Path;
use std::process::Command;

pub struct CompileError {
    pub message: String,
}

/// Writes `c_source` to `<out_path>.c` (kept alongside the binary, not
/// deleted, so a generated-C debugging session is a simple `gdb ./foo`
/// against a file that's actually there) and shells out to `cc -O2` to
/// produce the native binary at `out_path`.
pub fn compile_c(c_source: &str, out_path: &Path) -> Result<(), CompileError> {
    let c_path = out_path.with_extension("c");
    std::fs::write(&c_path, c_source).map_err(|e| CompileError {
        message: format!("could not write generated C to '{}': {e}", c_path.display()),
    })?;

    let output = Command::new("cc")
        .arg("-O2")
        .arg("-o")
        .arg(out_path)
        .arg(&c_path)
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
