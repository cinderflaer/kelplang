use super::*;

#[test]
pub(crate) fn a_scoped_blocks_captured_borrow_cannot_alias_the_same_call() {
    assert_rejected_with(
        "tests/fixtures/scoped_escaping_borrow.kp",
        "already borrowed exclusively",
    );
}

#[test]
pub(crate) fn a_scoped_blocks_captured_borrow_cannot_outlive_a_move_in_the_same_call() {
    assert_rejected_with(
        "tests/fixtures/scoped_borrow_escapes_move.kp",
        "use of moved value",
    );
}

/// Whether `ld.lld` is on PATH. A freestanding build's last step is a bare
/// `ld.lld` link, so these tests are for the `dev-kernel.nix` shell (which
/// carries `llvmPackages_21.lld` and the llvm tools); under the ordinary
/// `dev.nix` shell the tool is absent and the test prints a skip rather than
/// failing, which is what keeps the normal suite green.
pub(crate) fn have_ld_lld() -> bool {
    Command::new("ld.lld")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// One bare-metal target's emulator: the QEMU binary and the machine arguments
/// the target needs. Kernel stage 6 added the riscv64 one; kernel stage 10, the
/// second architecture, added aarch64, so the boot harness is parameterized
/// instead of naming one emulator.
#[derive(Clone, Copy)]
pub(crate) struct Emulator {
    pub(crate) binary: &'static str,
    pub(crate) args: &'static [&'static str],
}

/// QEMU's riscv64 `virt` machine, the one kernel stage 6 boots. `-bios none`
/// makes QEMU start the image from 0x80000000 with no firmware in front of it.
pub(crate) const RISCV64_QEMU: Emulator = Emulator {
    binary: "qemu-system-riscv64",
    args: &[
        "-machine",
        "virt",
        "-bios",
        "none",
        "-nographic",
        "-no-reboot",
        "-serial",
        "mon:stdio",
    ],
};

/// The same riscv64 machine with a second hart (kernel stage 8). `-smp 2`
/// makes QEMU start both harts at the entry with no firmware to park the
/// second, which is what `examples/kernel0-percpu` proves the compiler's
/// `_start` stub handles.
pub(crate) const RISCV64_QEMU_SMP2: Emulator = Emulator {
    binary: "qemu-system-riscv64",
    args: &[
        "-machine",
        "virt",
        "-bios",
        "none",
        "-smp",
        "2",
        "-nographic",
        "-no-reboot",
        "-serial",
        "mon:stdio",
    ],
};

/// QEMU's aarch64 `virt` machine, the one kernel stage 10 boots. `-cpu
/// cortex-a57` is the core its target machine is built for; QEMU loads the
/// `-kernel` ELF at 0x40000000 and starts it at the ELF entry, so no `-bios`
/// argument is needed.
pub(crate) const AARCH64_QEMU: Emulator = Emulator {
    binary: "qemu-system-aarch64",
    args: &[
        "-M",
        "virt",
        "-cpu",
        "cortex-a57",
        "-nographic",
        "-serial",
        "mon:stdio",
    ],
};

/// QEMU's x86-64 `q35` machine, the one kernel stage 20 boots. `-kernel` loads
/// an ELF64 image through the Xen PVH note and SeaBIOS enters the 32-bit entry
/// it names, so the target machine's CPU is the one `-cpu qemu64` is: no AVX,
/// no AVX-512. `-no-reboot` makes the long-mode example's triple fault exit
/// QEMU instead of rebooting into the boot loader again.
pub(crate) const X86_64_QEMU: Emulator = Emulator {
    binary: "qemu-system-x86_64",
    args: &[
        "-M",
        "q35",
        "-cpu",
        "qemu64",
        "-nographic",
        "-serial",
        "mon:stdio",
        "-no-reboot",
    ],
};

/// Whether `emu`'s QEMU binary is on PATH. Kernel stage 6's milestone is a
/// real boot, so its acceptance test needs the emulator as well as `ld.lld`;
/// like the freestanding tests, a shell without it prints a skip.
pub(crate) fn have_qemu(emu: &Emulator) -> bool {
    Command::new(emu.binary)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Kernel stage 6's acceptance harness: build a freestanding project and boot
/// the ELF under `emu` until the last `expected` line has come out of the UART,
/// then kill QEMU and check every expected line against what was captured. The
/// kernel halts in a wait loop, so nothing but reading the marker and killing
/// the emulator keeps the test from waiting out the whole window.
///
/// The expected lines are compared whole, and they are QEMU's own stdout, not
/// anything the project says it will print.
pub(crate) fn boot_under_qemu(emu: &Emulator, name: &str, target: &Path, expected: &[&str]) {
    let marker = expected.last().expect("at least one expected line");
    let captured = boot_and_capture(emu, name, target, &|got| {
        got.lines().any(|line| line == *marker)
    });
    for line in expected {
        assert!(
            captured.lines().any(|got| got == *line),
            "the serial output of '{name}' has no line '{line}':\n{captured}"
        );
    }
}

/// The same boot, for an assertion that is a shape rather than a whole line.
/// Kernel 0's panic handler prints the compiler's own message, which the test
/// cannot spell out in full, so the wait and the check are a set of substrings:
/// `index out of bounds` is in the text and some line starts with `panic: `.
pub(crate) fn boot_under_qemu_containing(
    emu: &Emulator,
    name: &str,
    target: &Path,
    needle: &str,
    line_prefix: &str,
) {
    let captured = boot_and_capture(emu, name, target, &|got| {
        got.contains(needle) && got.lines().any(|line| line.starts_with(line_prefix))
    });
    assert!(
        captured.contains(needle),
        "the serial output of '{name}' does not contain '{needle}':\n{captured}"
    );
    assert!(
        captured.lines().any(|line| line.starts_with(line_prefix)),
        "the serial output of '{name}' has no line starting with '{line_prefix}':\n{captured}"
    );
}

/// The same boot, for a run whose interesting output is a sequence rather than
/// a set of whole lines. Kernel stage 22's exception reports carry a `rip` the
/// linker chose, so each `expected` entry is a substring that has to appear
/// after the one before it; SeaBIOS' "Booting from ROM.." is glued to the
/// first kernel line, which is the other reason the match is not anchored.
pub(crate) fn boot_under_qemu_in_order(emu: &Emulator, name: &str, target: &Path, expected: &[&str]) {
    let marker = expected.last().expect("at least one expected line");
    let captured = boot_and_capture(emu, name, target, &|got| got.contains(marker));
    let mut from = 0usize;
    for want in expected {
        match captured[from..].find(want) {
            Some(at) => from += at + want.len(),
            None => panic!(
                "the serial output of '{name}' has no '{want}' after the earlier lines:\n{captured}"
            ),
        }
    }
}

/// Builds a freestanding project, boots it under `emu`, and reads the serial
/// output until `done` says the interesting part has arrived (or the deadline
/// fires), then kills the emulator and returns everything captured.
pub(crate) fn boot_and_capture(
    emu: &Emulator,
    name: &str,
    target: &Path,
    done: &dyn Fn(&str) -> bool,
) -> String {
    let bin = out_path(name);
    let build = Command::new(kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .arg("build")
        .arg(target)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the freestanding build of '{name}' failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let mut child = Command::new(emu.binary)
        .args(emu.args)
        .arg("-kernel")
        .arg(&bin)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap_or_else(|e| panic!("failed to spawn {}: {e}", emu.binary));

    // A reader thread turns the pipe into a channel so the wait below has a
    // deadline: without one a kernel that never prints would hang the suite.
    let stdout = child.stdout.take().expect("qemu stdout was piped");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            match line {
                Ok(line) => {
                    if tx.send(line).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let deadline = Instant::now() + ACCEPT_DEADLINE;
    let mut captured = String::new();
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(remaining) {
            Ok(line) => {
                captured.push_str(&line);
                captured.push('\n');
                if done(&captured) {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    captured
}

/// Runs an external tool and returns its stdout, failing the test with the
/// tool's stderr when it exits nonzero.
pub(crate) fn tool(name: &str, args: &[&str]) -> String {
    let out = Command::new(name)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("could not run '{name}': {e}"));
    assert!(
        out.status.success(),
        "'{name}' failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Builds one freestanding project with the LLVM backend and returns its object
/// file (the `.o` the driver wrote beside the ELF), so a test can read the
/// layout the object writer produced rather than the linked image.
pub(crate) fn build_freestanding_object(name: &str, project_dir: &str) -> PathBuf {
    let bin = out_path(name);
    let build = Command::new(kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .arg("build")
        .arg(project(project_dir))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the freestanding build of '{name}' failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    bin.with_extension("o")
}

/// One symbol's size from `llvm-nm -S` output (`addr size type name`, all hex
/// except undefined symbols, which have no address).
pub(crate) fn symbol_size(nm: &str, name: &str) -> u64 {
    for line in nm.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() == 4 && fields[3] == name {
            return u64::from_str_radix(fields[1], 16)
                .unwrap_or_else(|_| panic!("unreadable size in '{line}'"));
        }
    }
    panic!("symbol '{name}' is not in llvm-nm's output:\n{nm}");
}

/// The size column of one `llvm-readelf -S` line. The columns end
/// `... ES Flg Lk Inf Al`, so the size is six fields from the end.
pub(crate) fn percpu_section_size(line: &str) -> u64 {
    let fields: Vec<&str> = line.split_whitespace().collect();
    u64::from_str_radix(fields[fields.len() - 6], 16)
        .unwrap_or_else(|_| panic!("unreadable size in '{line}'"))
}

/// The last (`Al`) column of one `llvm-readelf -S` line, in decimal.
pub(crate) fn percpu_section_alignment(line: &str) -> u64 {
    let fields: Vec<&str> = line.split_whitespace().collect();
    fields[fields.len() - 1]
        .parse()
        .unwrap_or_else(|_| panic!("unreadable alignment in '{line}'"))
}

/// Kernel stage 4: `section("...")` on a `const` and on a `fun` changes where
/// the linker may place them, never what the program does. Both backends run
/// the same program to the same output.
#[test]
pub(crate) fn a_section_clause_places_code_and_data_without_changing_the_result() {
    assert_compiles_and_runs("tests/fixtures/section_attribute.kp", "41\n");
}

/// `print` is `printf`, and a freestanding build has no libc: the front end
/// refuses the construct by name. The check runs before either backend is
/// named, so every build refuses for the same reason and with the same message.
#[test]
pub(crate) fn print_is_rejected_in_a_freestanding_project() {
    assert_rejected_with(
        "tests/projects/freestanding_print",
        "freestanding build",
    );
}

/// The manifest's `[target] entry` must name a `fun` that exists in the entry
/// module: a missing one is named rather than left as an ELF whose `_start`
/// calls an undefined symbol.
#[test]
pub(crate) fn a_missing_freestanding_entry_is_rejected() {
    assert_rejected_with(
        "tests/projects/freestanding_no_entry",
        "no entry function 'kmain' found",
    );
}

/// `Ptr.alloc` and `Ptr.free` are the hosted `malloc`/`free`, so they are
/// refused even though `Ptr<T>` itself is allowed.
#[test]
pub(crate) fn ptr_alloc_is_rejected_in_a_freestanding_project() {
    assert_rejected_with(
        "tests/projects/freestanding_ptr_alloc",
        "'Ptr#alloc' is unavailable in a freestanding build",
    );
}

/// The host entry point has no runtime to call it in a freestanding image, so
/// a defined `main` is refused and the manifest's entry is the entry point.
#[test]
pub(crate) fn main_is_rejected_in_a_freestanding_project() {
    assert_rejected_with("tests/projects/freestanding_main", "'main' is unavailable");
}

/// The acceptance test for kernel stage 4: the `freestanding_min` project,
/// whose entry loops forever, builds through LLVM and `ld.lld` into an ELF64
/// RISC-V image based at 0x80000000 that references no undefined symbol, which
/// is to say no libc symbol and no `kelp_rt` helper.
///
/// It needs `ld.lld`, `llvm-readelf`, `llvm-objdump` and `llvm-nm`, all from
/// `dev-kernel.nix`; a shell without them prints a skip.
#[test]
pub(crate) fn freestanding_min_builds_a_riscv_elf_with_no_undefined_symbols() {
    if !have_ld_lld() {
        println!(
            "skipping the freestanding acceptance test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = out_path("freestanding_min");
    let build = Command::new(kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .arg("build")
        .arg(project("freestanding_min"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the freestanding build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let path = bin.to_str().expect("the artifact path is utf-8");
    let header = tool("llvm-readelf", &["-h", path]);
    assert!(header.contains("ELF64"), "not an ELF64 image:\n{header}");
    assert!(header.contains("RISC-V"), "not a RISC-V image:\n{header}");
    let entry_line = header
        .lines()
        .find(|line| line.contains("Entry point address"))
        .unwrap_or_default();
    assert!(
        entry_line.contains("0x80000000"),
        "the entry point is not the linker script's 0x80000000:\n{header}"
    );

    // `llvm-objdump` is the disassembler half of the acceptance check: the
    // generated `_start` has to be real RISC-V code, not an empty section.
    let disassembly = tool("llvm-objdump", &["-d", path]);
    assert!(disassembly.contains("_start"), "no _start in the image:\n{disassembly}");
    assert!(disassembly.contains("wfi"), "the _start halt is missing:\n{disassembly}");

    // No symbol the linker had to leave for someone else: no `printf`, no
    // `malloc`, no `kelp_panic`.
    let symbols = tool("llvm-nm", &[path]);
    let undefined: Vec<&str> = symbols
        .lines()
        .filter(|line| line.split_whitespace().next() == Some("U"))
        .collect();
    assert!(
        undefined.is_empty(),
        "the freestanding image has undefined symbols:\n{}",
        undefined.join("\n")
    );
}

/// `kelp wiki page language/iterators.md`'s freestanding half: a kernel-style loop over
/// `iter.range` lowers to an ordinary `while` over `Range.next`, so the image
/// links with no undefined symbol. This is the same acceptance test as
/// `freestanding_min`, run on the project that walks a range, and it needs the
/// same `dev-kernel.nix` tools.
#[test]
pub(crate) fn a_freestanding_project_can_walk_a_range() {
    if !have_ld_lld() {
        println!(
            "skipping the freestanding iterator test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = out_path("freestanding_iter");
    let build = Command::new(kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .arg("build")
        .arg(project("freestanding_iter"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the freestanding iterator build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let path = bin.to_str().expect("the artifact path is utf-8");
    let header = tool("llvm-readelf", &["-h", path]);
    assert!(header.contains("ELF64"), "not an ELF64 image:\n{header}");
    assert!(header.contains("RISC-V"), "not a RISC-V image:\n{header}");

    let symbols = tool("llvm-nm", &[path]);
    let undefined: Vec<&str> = symbols
        .lines()
        .filter(|line| line.split_whitespace().next() == Some("U"))
        .collect();
    assert!(
        undefined.is_empty(),
        "the freestanding iterator image has undefined symbols:\n{}",
        undefined.join("\n")
    );
}

/// Language-completeness 5's freestanding half: a `datac` with `Add` and `Ord`
/// from `std/ops`, used by `+` and `<`. The traits are pure language, so the
/// image links with no undefined symbol, exactly as the iterator one does.
/// Needs `dev-kernel.nix` tools.
#[test]
pub(crate) fn a_freestanding_project_can_use_the_operator_traits() {
    if !have_ld_lld() {
        println!(
            "skipping the freestanding operator-trait test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = out_path("freestanding_ops");
    let build = Command::new(kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .arg("build")
        .arg(project("freestanding_ops"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the freestanding operator-trait build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let path = bin.to_str().expect("the artifact path is utf-8");
    let header = tool("llvm-readelf", &["-h", path]);
    assert!(header.contains("ELF64"), "not an ELF64 image:\n{header}");
    assert!(header.contains("RISC-V"), "not a RISC-V image:\n{header}");

    let symbols = tool("llvm-nm", &[path]);
    let undefined: Vec<&str> = symbols
        .lines()
        .filter(|line| line.split_whitespace().next() == Some("U"))
        .collect();
    assert!(
        undefined.is_empty(),
        "the freestanding operator-trait image has undefined symbols:\n{}",
        undefined.join("\n")
    );
}
