use super::*;

// ---------------------------------------------------------------------------
// Kernel stage 10: the second architecture. `[target] triple =
// "aarch64-unknown-none"` gets an AArch64 target machine and the aarch64
// `_start` stub (load `sp` from `__stack_top`, clear `[__bss_start,
// __bss_end)`, call the entry, spin on `wfe`), and `examples/kernel0-aarch64`
// is the PL011 driver that boots on QEMU's aarch64 `virt` machine. Freestanding
// is LLVM-only by design, so these build through the default backend; they need
// `ld.lld` and the llvm tools, and the boot needs `qemu-system-aarch64`.
// ---------------------------------------------------------------------------

/// The ELF half of the stage: the image is ELF64 AArch64 whose entry is the
/// linker script's 0x40000000, referencing no undefined symbol, and whose
/// `_start` is real AArch64 code that halts on `wfe`. A boot that worked by
/// accident on some other entry point could not pass these.
#[test]
pub(crate) fn kernel0_aarch64_builds_an_aarch64_elf_with_no_undefined_symbols() {
    if !have_ld_lld() {
        println!(
            "skipping the aarch64 elf test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = build_freestanding("kernel0_aarch64", &fixture("tests/fixtures/examples/kernel0-aarch64"));
    let path = bin.to_str().expect("the artifact path is utf-8");

    let header = tool("llvm-readelf", &["-h", path]);
    assert!(header.contains("ELF64"), "not an ELF64 image:\n{header}");
    assert!(header.contains("AArch64"), "not an AArch64 image:\n{header}");
    let entry_line = header
        .lines()
        .find(|line| line.contains("Entry point address"))
        .unwrap_or_default();
    assert!(
        entry_line.contains("0x40000000"),
        "the entry point is not the linker script's 0x40000000:\n{header}"
    );

    // `llvm-objdump` is the disassembler half: the generated `_start` has to be
    // real AArch64 code, not an empty section.
    let disassembly = tool("llvm-objdump", &["-d", path]);
    assert!(
        disassembly.contains("_start"),
        "no _start in the image:\n{disassembly}"
    );
    assert!(
        disassembly.contains("wfe"),
        "the _start halt is missing:\n{disassembly}"
    );

    // `llvm-nm -u` prints what the linker had to leave for someone else: an
    // empty list is the whole claim that no libc symbol and no `kelp_rt` helper
    // is behind the image.
    let undefined = tool("llvm-nm", &["-u", path]);
    assert!(
        undefined.trim().is_empty(),
        "the aarch64 image has undefined symbols:\n{undefined}"
    );
}

/// The boot half: `examples/kernel0-aarch64` is a complete Kelp kernel that
/// boots on QEMU's aarch64 `virt` machine and writes one line to the PL011
/// UART. The assertion is against what QEMU captured, so it proves the boot,
/// the linker script's stack and the driver's polling, not that the source says
/// `hello from kelp (aarch64)`.
#[test]
pub(crate) fn kernel0_aarch64_boots_and_prints_its_greeting_over_the_uart() {
    if !have_ld_lld() || !have_qemu(&AARCH64_QEMU) {
        println!(
            "skipping the aarch64 boot test: 'ld.lld' or 'qemu-system-aarch64' is not on \
             PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    boot_under_qemu(
        &AARCH64_QEMU,
        "kernel0_aarch64_boot",
        &fixture("tests/fixtures/examples/kernel0-aarch64"),
        &["hello from kelp (aarch64)"],
    );
}

// ---------------------------------------------------------------------------
// Kernel stage 20: the second ISA family. `[target] triple =
// "x86_64-unknown-none"` gets an X86 target machine and the x86-64 `_start`
// stub (the Xen PVH note, a GDT, a 1 GiB identity map of 2 MiB pages, PAE and
// `EFER.LME`, the far jump to `.code64`, the bss clear, SSE on, the entry
// call), and `examples/kernel0-x86_64` is the COM1 driver that boots on QEMU's
// `q35` machine. Freestanding is LLVM-only by design, so these build through
// the default backend; they need `ld.lld` and the llvm tools, and the boots
// need `qemu-system-x86_64`. `examples/kernel0-x86_64-lmode` is the second
// boot: it proves EFER.LMA and paging are on and then triple-faults on purpose.
// ---------------------------------------------------------------------------

/// The ELF half of the stage: the image is ELF64 x86-64 whose entry is the
/// linker script's 0x100000, carrying the `Xen` PVH note (type 18) whose
/// description is that same entry, referencing no undefined symbol, and whose
/// `_start` is real x86 code that halts. A boot that worked by accident on some
/// other entry point could not pass these.
#[test]
pub(crate) fn kernel0_x86_64_builds_an_x86_64_elf_with_the_pvh_note() {
    if !have_ld_lld() {
        println!(
            "skipping the x86-64 elf test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = build_freestanding("kernel0_x86_64", &fixture("tests/fixtures/examples/kernel0-x86_64"));
    let path = bin.to_str().expect("the artifact path is utf-8");

    let header = tool("llvm-readelf", &["-h", path]);
    assert!(header.contains("ELF64"), "not an ELF64 image:\n{header}");
    assert!(
        header.contains("X86-64"),
        "not an x86-64 image:\n{header}"
    );
    let entry_line = header
        .lines()
        .find(|line| line.contains("Entry point address"))
        .unwrap_or_default();
    assert!(
        entry_line.contains("0x100000"),
        "the entry point is not the linker script's 0x100000:\n{header}"
    );

    // The PVH note: owner `Xen`, type 18 (printed in hex as 0x12), description
    // the 32-bit entry. Without it SeaBIOS has nothing to jump to.
    let notes = tool("llvm-readelf", &["-n", path]);
    assert!(notes.contains("Xen"), "no Xen PVH note in the image:\n{notes}");
    assert!(
        notes.contains("0x00000012"),
        "the Xen note is not XEN_ELFNOTE_PHYS32_ENTRY (type 18):\n{notes}"
    );
    assert!(
        notes.contains("00 00 10 00"),
        "the PVH note does not name the 0x100000 entry:\n{notes}"
    );

    // `llvm-objdump` is the disassembler half: the generated `_start` has to be
    // real x86 code, not an empty section.
    let disassembly = tool("llvm-objdump", &["-d", path]);
    assert!(
        disassembly.contains("_start"),
        "no _start in the image:\n{disassembly}"
    );
    assert!(
        disassembly.contains("hlt"),
        "the _start halt is missing:\n{disassembly}"
    );

    // `llvm-nm -u` prints what the linker had to leave for someone else: an
    // empty list is the whole claim that no libc symbol and no `kelp_rt` helper
    // is behind the image.
    let undefined = tool("llvm-nm", &["-u", path]);
    assert!(
        undefined.trim().is_empty(),
        "the x86-64 image has undefined symbols:\n{undefined}"
    );
}

/// The boot half: `examples/kernel0-x86_64` is a complete Kelp kernel that
/// boots on QEMU's `q35` machine through the Xen PVH path and writes one line
/// to the COM1 UART. The assertion is against what QEMU captured, so it proves
/// the boot, the linker script's stack and the driver's polling, not that the
/// source says `hello from kelp (x86_64)`.
#[test]
pub(crate) fn kernel0_x86_64_boots_and_prints_its_greeting_over_the_uart() {
    if !have_ld_lld() || !have_qemu(&X86_64_QEMU) {
        println!(
            "skipping the x86-64 boot test: 'ld.lld' or 'qemu-system-x86_64' is not on \
             PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    boot_under_qemu(
        &X86_64_QEMU,
        "kernel0_x86_64_boot",
        &fixture("tests/fixtures/examples/kernel0-x86_64"),
        &["hello from kelp (x86_64)"],
    );
}

/// Kernel stage 23 on x86-64: the same `String`-field equality, `View` slice
/// and `std/regex` match as `examples/kernel0-eq`, built for
/// `x86_64-unknown-none` and booted through the Xen PVH note. Both halves of
/// the pair matter: the three bodies the compiler emits have to be
/// architecture-independent byte loops, not a riscv64-only lowering.
///
/// It needs `ld.lld` and `qemu-system-x86_64` from `dev-kernel.nix`.
#[test]
pub(crate) fn kernel0_eq_x86_64_builds_and_boots_the_same_lines() {
    if !have_ld_lld() {
        println!(
            "skipping the kernel0-eq x86-64 build test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = build_freestanding("kernel0_eq_x86_64", &fixture("tests/fixtures/examples/kernel0-eq-x86_64"));
    let symbols = tool("llvm-nm", &[bin.to_str().unwrap()]);
    let undefined: Vec<&str> = symbols
        .lines()
        .filter(|line| line.split_whitespace().next() == Some("U"))
        .collect();
    assert!(
        undefined.is_empty(),
        "the kernel0-eq x86-64 image has undefined symbols:\n{}",
        undefined.join("\n")
    );

    if !have_qemu(&X86_64_QEMU) {
        println!(
            "skipping the kernel0-eq x86-64 boot: 'qemu-system-x86_64' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    boot_under_qemu(
        &X86_64_QEMU,
        "kernel0_eq_x86_64_boot",
        &fixture("tests/fixtures/examples/kernel0-eq-x86_64"),
        &[
            "eq same: yes",
            "eq differ: yes",
            "map keys: 2",
            "map hit: yes",
            "bytes eq: yes",
            "view slice: kelp",
            "view eq: yes",
            "regex match: hello42",
            "regex group: 42",
            "balanced",
        ],
    );
}

/// The second boot: `examples/kernel0-x86_64-lmode` reads CR0/CR4/EFER through
/// `asm` and prints `long mode on` only when EFER.LMA, CR0.PG and CR4.PAE are
/// all set, so the greeting plus that line proves the stub really entered long
/// mode with paging. It then loads a null IDT and executes `ud2`; with no
/// descriptor the #UD is a triple fault, and `-no-reboot` turns QEMU's reset
/// into an exit the test reads. The assertion is still only the two serial
/// lines: the fault after them is the proof, not a line.
#[test]
pub(crate) fn kernel0_x86_64_lmode_proves_long_mode_then_triple_faults() {
    if !have_ld_lld() || !have_qemu(&X86_64_QEMU) {
        println!(
            "skipping the x86-64 long-mode boot test: 'ld.lld' or \
             'qemu-system-x86_64' is not on PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    boot_under_qemu(
        &X86_64_QEMU,
        "kernel0_x86_64_lmode_boot",
        &fixture("tests/fixtures/examples/kernel0-x86_64-lmode"),
        &["hello from kelp (x86_64)", "long mode on"],
    );
}

// ---------------------------------------------------------------------------
// Kernel stage 22: the x86-64 IDT, the CPU exceptions and the PIC+PIT timer.
// `examples/kernel0-x86_64-idt` is the third x86-64 boot: it loads an IDT whose
// gates are naked functions, resumes from a `ud2` and a divide by zero, takes
// five timer interrupts, and ends in a store through an unmapped page whose
// `#PF` handler prints and halts. Freestanding is LLVM-only by design, so these
// build through the default backend; the boot needs `qemu-system-x86_64` and
// `ld.lld` from `dev-kernel.nix`.
// ---------------------------------------------------------------------------

/// The ELF half of the stage: the five gates are functions, each of them
/// 16-byte aligned (the `align(16)` clause, so an IDT entry never points into
/// the middle of another gate), and the image references no undefined symbol,
/// which is the whole claim that no libc helper sits behind the report or the
/// dispatcher.
#[test]
pub(crate) fn kernel0_x86_64_idt_builds_five_aligned_gates_with_no_undefined_symbols() {
    if !have_ld_lld() {
        println!(
            "skipping the x86-64 idt elf test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = build_freestanding(
        "kernel0_x86_64_idt",
        &fixture("tests/fixtures/examples/kernel0-x86_64-idt"),
    );
    let path = bin.to_str().expect("the artifact path is utf-8");

    let symbols = tool("llvm-nm", &[path]);
    for gate in [
        "kelp_idt__gate_de",
        "kelp_idt__gate_ud",
        "kelp_idt__gate_gp",
        "kelp_idt__gate_pf",
        "kelp_idt__gate_timer",
    ] {
        let address = nm_address(&symbols, gate);
        assert_eq!(
            address % 16,
            0,
            "'{gate}' is at {address:#x}, which is not 16-byte aligned:\n{symbols}"
        );
    }

    let undefined = tool("llvm-nm", &["-u", path]);
    assert!(
        undefined.trim().is_empty(),
        "the x86-64 idt image has undefined symbols:\n{undefined}"
    );
}

/// The boot half: the serial lines have to carry, in this order, the `#UD` and
/// the divide by zero (both reported and both resumed, which the ticks after
/// them prove), the five timer ticks, the boot's own `x86: done`, and the
/// `#PF` on the unmapped store with no repair behind it, so it is the last
/// line. The `rip` in a report is the address the linker chose, so a report is
/// matched by shape; the error code and the `cr2` are pinned, because a
/// supervisor write to a not-present page is error code 2 and the address is
/// the one `main.kp` names.
#[test]
pub(crate) fn kernel0_x86_64_idt_takes_its_faults_and_ticks_the_timer_in_order() {
    if !have_ld_lld() || !have_qemu(&X86_64_QEMU) {
        println!(
            "skipping the x86-64 idt boot test: 'ld.lld' or 'qemu-system-x86_64' \
             is not on PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    boot_under_qemu_in_order(
        &X86_64_QEMU,
        "kernel0_x86_64_idt_boot",
        &fixture("tests/fixtures/examples/kernel0-x86_64-idt"),
        &[
            "exception 6 err 0 rip 0x",
            "exception 0 err 0 rip 0x",
            "tick 1",
            "tick 2",
            "tick 3",
            "tick 4",
            "tick 5",
            "x86: done",
            "exception 14 err 2 rip 0x",
            "cr2 0x0000000200000000",
        ],
    );
}

/// Kernel stage 22's layout contract: the gates hardcode the `TrapFrame`
/// offsets and the offset split an IDT entry wants, and the compiler checks
/// neither end against the other. `tests/projects/x86_idt_layout` copies the
/// example's declarations and prints the numbers: the gate is 16 bytes with
/// its offset at 0/6/8, the `lidt` descriptor is ten bytes with the limit at 0
/// and the base at 2, and the frame is 160 bytes with the vector at 0, the
/// registers from 8 to 120, the error code at 128 and `RIP`/`CS`/`RFLAGS` at
/// 136/144/152.
#[test]
pub(crate) fn the_x86_idt_gate_and_frame_offsets_are_what_the_gates_hardcode() {
    assert_project_runs(
        "x86_idt_layout",
        &project("x86_idt_layout"),
        "16\n0\n2\n4\n5\n6\n8\n12\n10\n0\n2\n160\n0\n8\n120\n128\n136\n144\n152\n",
    );
}

/// Builds `source` with an `ld.lld` shim named by `KELPC_LLD` that records every
/// invocation's arguments and then runs the real `ld.lld`. Returns the recorded
/// lines, one per link.
pub(crate) fn build_recording_ld_lld_lines(source: &str) -> Vec<String> {
    use std::os::unix::fs::PermissionsExt;

    let real_lld = String::from_utf8(
        Command::new("sh")
            .args(["-c", "command -v ld.lld"])
            .output()
            .expect("failed to look up ld.lld")
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    assert!(!real_lld.is_empty(), "no ld.lld on PATH");

    let name = Path::new(source).file_stem().unwrap().to_string_lossy().to_string();
    let bin = out_path(&name);
    let dir = bin.with_extension("shim");
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("ld.log");
    let _ = std::fs::remove_file(&log);
    let shim = dir.join("ld-lld");
    std::fs::write(
        &shim,
        format!(
            "#!/bin/sh\necho \"$@\" >> '{}'\nexec '{real_lld}' \"$@\"\n",
            log.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();

    let build = kelpc()
        .env("KELPC_LLD", &shim)
        .arg("build")
        .arg(fixture(source))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected '{source}' to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    std::fs::read_to_string(&log)
        .expect("the ld.lld shim was never called")
        .lines()
        .map(str::to_string)
        .collect()
}

/// True when `flag` is one of the whitespace-separated arguments of a recorded
/// ld.lld line.
pub(crate) fn has_arg(line: &str, flag: &str) -> bool {
    line.split_whitespace().any(|a| a == flag)
}

/// BUG-x7Qmd: a program that does not reach `std/tls` neither links OpenSSL nor
/// includes its headers, so it builds on a host with no OpenSSL development
/// files. A program that does reach it links both libraries.
#[test]
pub(crate) fn only_a_tls_program_links_openssl() {
    let plain = build_recording_ld_lld_lines("tests/fixtures/fs_path_family.kp");
    assert!(!plain.is_empty(), "no ld.lld link was recorded");
    for line in &plain {
        assert!(
            !has_arg(line, "-lssl") && !has_arg(line, "-lcrypto"),
            "a program without std/tls linked OpenSSL: {line}"
        );
    }
    // No C is written for a program at all: the `extern fun` trampolines and
    // callback glue are LLVM IR in an object of their own (`extern_object.rs`).
    let name = "fs_path_family";
    let generated = out_path(name);
    assert!(
        !generated.with_extension("externs.c").exists(),
        "a hosted build wrote a C source for its externs"
    );

    let tls = build_recording_ld_lld_lines("tests/fixtures/https_try_dial.kp");
    assert!(!tls.is_empty(), "no ld.lld link was recorded for std/tls");
    assert!(
        tls.iter().any(|l| has_arg(l, "-lssl") && has_arg(l, "-lcrypto")),
        "a program that reaches std/tls did not link OpenSSL: {tls:?}"
    );
}

// ---------------------------------------------------------------------------
// Kernel stage 13 (MK9-S5-M3): the fallible `Ptr.try_alloc` and the shipped
// `std/arena`. The hosted halves run on both backends (each run of the suite is
// pointed at one); the freestanding halves are LLVM-only by design and need
// `ld.lld`, `llvm-nm` and `qemu-system-riscv64` from `dev-kernel.nix`.
// ---------------------------------------------------------------------------

/// The arena's hosted behaviour: the returned addresses are aligned (`base` is a
/// plain array with no alignment of its own, so the arena has to account for
/// where it starts), a request that does not fit and a size whose aligned end
/// would wrap both answer the null pointer, and `reset` makes the region
/// reusable. No `unsafe` is written in the fixture, so both backends build it.
#[test]
pub(crate) fn an_arena_bumps_with_alignment_and_reuses_after_reset() {
    assert_fixture_runs("tests/fixtures/arena_bump.kp");
}

/// A non-power-of-two `align` is refused at the call with the arena's own
/// message rather than handing back a misaligned pointer.
#[test]
pub(crate) fn an_arena_rejects_a_non_power_of_two_alignment() {
    assert_fixture_panics("arena_bad_align", "arena align must be a power of two");
}

/// The arena over real memory: both pointers are written and read back, and
/// `alloc_zeroed` clears a block the previous user wrote. `main` is on the
/// audit list because the writes go through a raw `Ptr<u8>`.
#[test]
pub(crate) fn an_arena_hands_out_writable_aligned_memory_and_zeroes_on_request() {
    assert_project_runs(
        "arena_memory",
        &project("arena_memory"),
        "170\n187\ntrue\ntrue\n0\n0\ntrue\n",
    );
}

/// Refusal (owner decision 2026-10-12): every hosted build collects memory, and a hosted
/// build refuses `Ptr.try_alloc` because its allocation has no collector descriptor. The
/// refusal stays in `heap_alloc`; this test pins it.
#[test]
pub(crate) fn try_alloc_is_refused_in_a_hosted_managed_build() {
    assert_project_rejected(
        "ptrs_try_alloc",
        &project("ptrs_try_alloc"),
        "this container allocation has no GC type descriptor yet",
    );
}

/// `Ptr.try_alloc` carries the same `unsafe` requirement as `Ptr.alloc`, so a
/// call outside any block is refused by name even in a module the manifest
/// lists.
#[test]
pub(crate) fn try_alloc_outside_unsafe_is_refused() {
    assert_project_rejected(
        "ptr_try_alloc_outside_unsafe",
        &project("ptr_try_alloc_outside_unsafe"),
        "'Ptr.try_alloc' is an unsafe operation",
    );
}

/// In a freestanding build the hook behind `Ptr.try_alloc` is the project's own
/// allocator, so without a `[target] alloc`/`dealloc` pair the call is refused
/// with the two keys that would make it available.
#[test]
pub(crate) fn try_alloc_without_a_freestanding_allocator_is_refused() {
    assert_project_rejected(
        "freestanding_try_alloc_no_alloc",
        &project("freestanding_try_alloc_no_alloc"),
        "'[target] alloc' and '[target] dealloc'",
    );
}

/// The freestanding arena's build claim: `std/arena` and its `unsafe` block
/// compile into a riscv64 image that references no undefined symbol, and the
/// project configures no allocator at all, which is what proves the module is
/// pure pointer arithmetic. Needs `ld.lld` and `llvm-nm`.
#[test]
pub(crate) fn freestanding_arena_builds_with_no_undefined_symbols() {
    if !have_ld_lld() {
        println!(
            "skipping the freestanding arena build test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = out_path("freestanding_arena");
    let build = Command::new(kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .arg("build")
        .arg(project("freestanding_arena"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the freestanding arena build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let path = bin.to_str().expect("the artifact path is utf-8");
    let symbols = tool("llvm-nm", &[path]);
    let undefined: Vec<&str> = symbols
        .lines()
        .filter(|line| line.split_whitespace().next() == Some("U"))
        .collect();
    assert!(
        undefined.is_empty(),
        "the freestanding arena image has undefined symbols:\n{}",
        undefined.join("\n")
    );
}

/// The freestanding arena's boot: a `static mut [u8; 4096]` region carved into
/// aligned allocations, written and read through the returned pointers, with
/// the exhaustion and overflow answers, all read from QEMU's own serial output.
#[test]
pub(crate) fn freestanding_arena_boots_and_prints_its_lines() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU) {
        println!(
            "skipping the freestanding arena boot test: 'ld.lld' or 'qemu-system-riscv64' is \
             not on PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    boot_under_qemu(
        &RISCV64_QEMU,
        "freestanding_arena",
        &project("freestanding_arena"),
        &[
            "p0 aligned 1",
            "p1 aligned 1",
            "ordered 1",
            "used seen 1",
            "remain seen 1",
            "read 170",
            "exhaust 1",
            "wrap 1",
            "reset 0",
            "zero 0",
        ],
    );
}

/// The freestanding `try_alloc`'s build claim: with `[target] alloc`/`dealloc`
/// naming the project's own bump allocator, no libc `malloc` is left behind.
#[test]
pub(crate) fn freestanding_try_alloc_builds_with_no_undefined_symbols() {
    if !have_ld_lld() {
        println!(
            "skipping the freestanding try_alloc build test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = out_path("freestanding_try_alloc");
    let build = Command::new(kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .arg("build")
        .arg(project("freestanding_try_alloc"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the freestanding try_alloc build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let path = bin.to_str().expect("the artifact path is utf-8");
    let symbols = tool("llvm-nm", &[path]);
    let undefined: Vec<&str> = symbols
        .lines()
        .filter(|line| line.split_whitespace().next() == Some("U"))
        .collect();
    assert!(
        undefined.is_empty(),
        "the freestanding try_alloc image has undefined symbols:\n{}",
        undefined.join("\n")
    );
}

/// The freestanding `try_alloc`'s boot: a normal request comes back non-null,
/// aligned and writable through the project allocator, and an absurd count comes
/// back null. QEMU's serial output is the assertion, so this proves the hook was
/// actually called.
#[test]
pub(crate) fn freestanding_try_alloc_boots_and_prints_its_lines() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU) {
        println!(
            "skipping the freestanding try_alloc boot test: 'ld.lld' or 'qemu-system-riscv64' \
             is not on PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    boot_under_qemu(
        &RISCV64_QEMU,
        "freestanding_try_alloc",
        &project("freestanding_try_alloc"),
        &["ptr 0", "align 0", "val 1234", "huge 1"],
    );
}

// ---------------------------------------------------------------------------
// Kernel stage 14 (MK9-S7-M2), the stack half: the freestanding `probe-stack`
// attribute and the guard page. The first test is the object-only claim that a
// frame larger than a page carries the inline probe; the second is the boot
// that overflows a small frame and a big one into the same unmapped page.
// ---------------------------------------------------------------------------

/// Parses a `0x`-prefixed hex word.
pub(crate) fn parse_hex_word(text: &str) -> Option<usize> {
    usize::from_str_radix(text.strip_prefix("0x")?, 16).ok()
}

/// Kernel stage 14's object-only test: `examples/kernel0-stack`'s
/// `recurse_big` holds a `[u8; 16384]` local, and the freestanding build's
/// `"probe-stack"="inline-asm"` attribute has to lower its prologue to a page
/// probe rather than one `sub sp, sp, 16400`. The assertion is on
/// `llvm-objdump`: at least three probe stores, each preceded by a
/// page-sized subtraction, which is what lets the guard page catch the frame
/// instead of the frame stepping over it.
///
/// It needs `ld.lld` and `llvm-objdump` from `dev-kernel.nix`.
#[test]
pub(crate) fn freestanding_big_frames_probe_every_page() {
    if !have_ld_lld() {
        println!(
            "skipping the freestanding probe test: 'ld.lld' is not on PATH \
             (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = out_path("kernel0_stack_probe");
    let build = Command::new(kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .arg("build")
        .arg(fixture("tests/fixtures/examples/kernel0-stack"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the kernel0-stack build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let path = bin.to_str().expect("the artifact path is utf-8");

    // The lowering carries the attribute, so the check has two halves: the IR
    // says the function was marked, and the object shows the probe it lowers to.
    let ir = std::fs::read_to_string(bin.with_extension("ll")).expect("the .ll artifact");
    assert!(
        ir.contains("\"probe-stack\"=\"inline-asm\""),
        "the freestanding IR carries no probe-stack attribute"
    );

    let disassembly = tool(
        "llvm-objdump",
        &["-d", "--disassemble-symbols=kelp_recurse_big", path],
    );
    // `llvm-objdump` prints the displacement as `0(sp)` or `0x0(sp)` depending
    // on the build, so the probe is matched by its operands rather than one
    // exact spelling.
    let is_probe = |line: &str| {
        line.contains("\tsd\t") && line.contains("zero, 0") && line.contains("sp)")
    };
    let probes = disassembly.lines().filter(|line| is_probe(line)).count();
    assert!(
        probes >= 3,
        "kelp_recurse_big's probe is missing: its disassembly has {probes} probe stores:\n{disassembly}"
    );
    // A single `sub sp, sp, 16400` with no probe store would mean the 16 KiB
    // frame stepped over the guard in one instruction.
    assert!(
        disassembly.lines().filter(|line| line.contains("sub\tsp, sp,")).count() >= 3,
        "kelp_recurse_big builds its frame in fewer than three page steps:\n{disassembly}"
    );
}

/// Kernel stage 14's acceptance boot: `examples/kernel0-stack` takes three
/// faults over one trap vector. Phase t stores into the trap stack's own guard
/// page. Phase a recurses with a 16-byte frame until the frame that crosses
/// `__stack_bottom` stores into the kernel stack's guard page; phase b is entered
/// by the handler, which restored `sp` to the top of the kernel stack, and its
/// one 16 KiB frame probes down into that same page. The handler prints the
/// `scause` and `stval` of each fault and says so, so the test can assert every
/// address is inside one of the two guard pages the boot printed.
///
/// It needs `ld.lld` and `qemu-system-riscv64` from `dev-kernel.nix`.
#[test]
pub(crate) fn kernel0_stack_overflows_into_the_guard_page() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU) {
        println!(
            "skipping the kernel0-stack boot test: 'ld.lld' or 'qemu-system-riscv64' is not \
             on PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let captured = boot_and_capture(
        &RISCV64_QEMU,
        "kernel0_stack",
        &fixture("tests/fixtures/examples/kernel0-stack"),
        &|got| got.contains("stack: halted"),
    );
    for line in [
        "stack: m-mode",
        "stack: paging on",
        "stack: phase t",
        "stack: stval in guard",
        "stack: phase a",
        "stack: stval in guard",
        "stack: phase b",
        "stack: stval in guard",
        "stack: halted",
    ] {
        assert!(
            captured.lines().any(|got| got == line),
            "the serial output of kernel0-stack has no line '{line}':\n{captured}"
        );
    }

    // The kernel's own `in guard` line is one check; this is the independent
    // one: parse the bounds of both guard pages the boot printed and the three
    // `stval`s, and require each fault address to lie in one of the two holes.
    let bound_pair = |prefix: &str| -> (usize, usize) {
        let bounds: Vec<usize> = captured
            .lines()
            .find(|line| line.starts_with(prefix))
            .and_then(|line| {
                // The two bounds are the last two words of the line.
                let words: Vec<&str> = line.split_whitespace().collect();
                let n = words.len();
                Some(vec![parse_hex_word(words.get(n.checked_sub(2)?)?)?, parse_hex_word(words.get(n - 1)?)?])
            })
            .unwrap_or_else(|| panic!("the boot printed no `{prefix}<lo> <hi>` line"));
        (bounds[0], bounds[1])
    };
    let (lo, hi) = bound_pair("stack: guard ");
    let (tlo, thi) = bound_pair("stack: trap guard ");
    let faults: Vec<usize> = captured
        .lines()
        .filter(|line| line.starts_with("stack overflow: scause "))
        .filter_map(|line| {
            let words: Vec<&str> = line.split_whitespace().collect();
            let at = words.iter().position(|w| *w == "stval")?;
            parse_hex_word(words.get(at + 1)?)
        })
        .collect();
    assert_eq!(
        faults.len(),
        3,
        "expected one fault per phase, got {}:\n{captured}",
        faults.len()
    );
    assert!(thi <= lo, "the trap stack's guard must lie below the kernel stack's guard");
    for fault in &faults {
        let in_kernel = *fault >= lo && *fault < hi;
        let in_trap = *fault >= tlo && *fault < thi;
        assert!(
            in_kernel || in_trap,
            "a fault at {fault:#x} is outside both guard pages \
             [{lo:#x}, {hi:#x}) and [{tlo:#x}, {thi:#x}):\n{captured}"
        );
    }
    // The trap stack's fault is phase t, the first one the handler prints.
    assert!(
        *faults.first().expect("no faults") >= tlo && *faults.first().unwrap() < thi,
        "phase t's fault is not in the trap stack's guard:\n{captured}"
    );
}

/// Kernel stage 14's other half of "hosted builds are unchanged": a hosted
/// function carries no `probe-stack` attribute, so the probe is a freestanding
/// lowering and not a change to every build. The IR is the LLVM backend's
/// artifact, so this check is LLVM-only.
#[test]
pub(crate) fn hosted_functions_carry_no_stack_probe() {
    let bin = out_path("hosted_no_probe");
    let build = Command::new(kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .arg("build")
        .arg(fixture("tests/fixtures/array_basic.kp"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the hosted build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let ir = std::fs::read_to_string(bin.with_extension("ll")).expect("the .ll artifact");
    assert!(
        !ir.contains("probe-stack"),
        "a hosted build carries the freestanding probe-stack attribute"
    );
}
