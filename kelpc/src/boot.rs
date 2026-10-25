//! `kelpc boot <project dir>`: build a freestanding example, boot it under the
//! QEMU machine its coral.toml `[run]` table names, and check the serial output
//! (issue #303). It replaces the per-example `run.sh` scripts: the same build,
//! the same QEMU flags (`BootMachine::base_args`), the same whole-line check, and
//! the same 20 second limit. Like those scripts it stops early once the last
//! expected line appears, because a halted kernel would otherwise wait out the
//! whole window.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use crate::resolve::{BootMachine, RunDecl};

/// How long a boot may run before QEMU is killed, as `timeout 20` did.
const BOOT_LIMIT: Duration = Duration::from_secs(20);

/// The expected lines that do not appear whole in `serial`, in table order.
pub(crate) fn missing_lines(expect: &[String], serial: &str) -> Vec<String> {
    expect.iter().filter(|want| !serial.lines().any(|line| line == want.as_str())).cloned().collect()
}

/// Whether the serial output already holds the stop marker, so QEMU can be killed.
pub(crate) fn reached_marker(serial: &str, marker: &str) -> bool {
    serial.lines().any(|line| line == marker)
}

/// The QEMU arguments for one boot: the machine's base flags, the table's options,
/// then the image.
pub(crate) fn qemu_args(run: &RunDecl, elf: &Path) -> Vec<String> {
    let mut args: Vec<String> = run.machine.base_args().iter().map(|a| a.to_string()).collect();
    if run.ramfb {
        args.extend(["-device".to_string(), "ramfb".to_string()]);
    }
    if run.smp > 1 {
        args.extend(["-smp".to_string(), run.smp.to_string()]);
    }
    args.extend(["-kernel".to_string(), elf.display().to_string()]);
    args
}

/// A scratch directory removed when the boot ends, whatever its outcome.
struct ScratchDir(PathBuf);

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn scratch_dir() -> Result<ScratchDir, String> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("kelpc-boot-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&dir).map_err(|e| format!("could not create '{}': {e}", dir.display()))?;
    Ok(ScratchDir(dir))
}

/// `kelpc boot <project dir>`: exit 0 when every expected line printed, 1 otherwise.
pub(crate) fn boot_command(args: &[String]) -> ExitCode {
    let [dir] = args else {
        eprintln!("usage: kelpc boot <project dir>");
        return ExitCode::FAILURE;
    };
    let root = PathBuf::from(dir);
    let run = match crate::resolve::manifest_run(&root) {
        Ok(run) => run,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::FAILURE;
        }
    };
    let scratch = match scratch_dir() {
        Ok(scratch) => scratch,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::FAILURE;
        }
    };
    let elf = scratch.0.join("kernel.elf");
    let serial_path = scratch.0.join("serial.txt");
    let qemu_stderr_path = scratch.0.join("qemu.stderr");

    // The same build `kelpc build <dir> -o <elf>` does, run as this binary.
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            eprintln!("error: could not find kelpc itself: {e}");
            return ExitCode::FAILURE;
        }
    };
    match Command::new(&exe).arg("build").arg(&root).arg("-o").arg(&elf).status() {
        Ok(status) if status.success() => {}
        Ok(_) => {
            eprintln!("kelpc boot: the build of '{}' failed", root.display());
            return ExitCode::FAILURE;
        }
        Err(e) => {
            eprintln!("error: could not run the build: {e}");
            return ExitCode::FAILURE;
        }
    }

    let serial_file = match File::create(&serial_path) {
        Ok(file) => file,
        Err(e) => {
            eprintln!("error: could not create the serial capture: {e}");
            return ExitCode::FAILURE;
        }
    };
    let stderr_file = match File::create(&qemu_stderr_path) {
        Ok(file) => file,
        Err(e) => {
            eprintln!("error: could not create the qemu stderr capture: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut child = match Command::new(run.machine.binary())
        .args(qemu_args(&run, &elf))
        .stdin(Stdio::null())
        .stdout(serial_file)
        .stderr(stderr_file)
        .spawn()
    {
        Ok(child) => child,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("kelpc boot: {} is not on PATH (run it in the repository's nix shell)", run.machine.binary());
            return ExitCode::FAILURE;
        }
        Err(e) => {
            eprintln!("error: could not start {}: {e}", run.machine.binary());
            return ExitCode::FAILURE;
        }
    };

    // Read the capture until the marker shows, QEMU exits, or the limit passes.
    let marker = run.expect.last().cloned().unwrap_or_default();
    let deadline = Instant::now() + BOOT_LIMIT;
    loop {
        let serial = fs::read_to_string(&serial_path).unwrap_or_default();
        if reached_marker(&serial, &marker) {
            break;
        }
        match child.try_wait() {
            Ok(None) => {}
            _ => break,
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
    let _ = child.wait();

    let serial = fs::read_to_string(&serial_path).unwrap_or_default();
    let missing = missing_lines(&run.expect, &serial);
    if missing.is_empty() {
        println!("kelpc boot: booted {} and the UART printed:", root.display());
        print!("{serial}");
        return ExitCode::SUCCESS;
    }
    for line in &missing {
        eprintln!("kelpc boot: the serial output has no line '{line}'");
    }
    eprintln!("kelpc boot: captured serial output:");
    eprint!("{serial}");
    eprintln!("kelpc boot: qemu stderr:");
    eprint!("{}", fs::read_to_string(&qemu_stderr_path).unwrap_or_default());
    ExitCode::FAILURE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_must_match_whole() {
        let serial = "hello from kelp\nend\n";
        assert!(missing_lines(&["end".to_string()], serial).is_empty());
        assert_eq!(missing_lines(&["hello".to_string()], serial), ["hello".to_string()]);
        assert_eq!(missing_lines(&["done".to_string()], serial), ["done".to_string()]);
    }

    #[test]
    fn expected_lines_may_come_in_any_order() {
        let serial = "b\na\n";
        assert!(missing_lines(&["a".to_string(), "b".to_string()], serial).is_empty());
    }

    #[test]
    fn the_last_line_is_the_stop_marker() {
        assert!(reached_marker("x\nend\n", "end"));
        assert!(!reached_marker("x\nendless\n", "end"));
        assert!(!reached_marker("", "end"));
    }

    #[test]
    fn qemu_arguments_follow_the_machine() {
        let riscv = RunDecl {
            machine: BootMachine::Riscv64,
            expect: vec!["end".to_string()],
            ramfb: false,
            smp: 1,
        };
        let args = qemu_args(&riscv, Path::new("/tmp/k.elf"));
        assert!(args.windows(2).any(|w| w == ["-machine", "virt"]), "{args:?}");
        assert!(args.windows(2).any(|w| w == ["-bios", "none"]), "{args:?}");
        assert!(!args.iter().any(|a| a == "-device" || a == "-smp"), "{args:?}");
        let fb = RunDecl { ramfb: true, smp: 2, ..riscv };
        let args = qemu_args(&fb, Path::new("/tmp/k.elf"));
        assert!(args.windows(2).any(|w| w == ["-device", "ramfb"]), "{args:?}");
        assert!(args.windows(2).any(|w| w == ["-smp", "2"]), "{args:?}");
        assert_eq!(args.last().map(String::as_str), Some("/tmp/k.elf"));
        let x86 = RunDecl { machine: BootMachine::X86_64, ramfb: false, smp: 1, expect: vec![] };
        let args = qemu_args(&x86, Path::new("/tmp/k.elf"));
        assert!(args.windows(2).any(|w| w == ["-M", "q35"]), "{args:?}");
        assert!(args.windows(2).any(|w| w == ["-cpu", "qemu64"]), "{args:?}");
        assert_eq!(BootMachine::X86_64.binary(), "qemu-system-x86_64");
        assert_eq!(BootMachine::Aarch64.binary(), "qemu-system-aarch64");
    }
}
