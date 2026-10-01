//! The `mmio` capability domain (kernel stage 16, MK10-S1): a freestanding
//! project may build a pointer from an address only where the module was
//! granted it (`plan/device-caps.md`). The gate is a front-end one, so every
//! case here runs `kelpc check` and needs neither `ld.lld` nor QEMU; a project
//! that checks is never run, and the accepting cases assert exactly that.
//!
//! The cases are written as throwaway projects under `CARGO_TARGET_TMPDIR`
//! rather than as `tests/projects/*` directories, because each one differs from
//! its neighbours by a single manifest range or a single address and the shared
//! freestanding target section, linker script and UART driver would otherwise be
//! copied a dozen times. The range parser's own unit tests live in
//! `stdlib.rs`.
//!
//! An accepting case asserts the project checks and a refusing case asserts it
//! does not and that the message names the address, the granted ranges or the
//! reason, so neither can pass for the wrong cause.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn kelpc() -> Command {
    Command::new(env!("CARGO_BIN_EXE_kelpc"))
}

/// A fresh directory per case: the cases run in parallel and must not see each
/// other's files. It lives under the workspace's `target/`, which is inside the
/// checkout and gitignored, rather than the platform temp directory the
/// capability tests use: a scratch project for a kernel never belongs in a
/// shared place.
fn scratch(name: &str) -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crate is a workspace member, so it has a parent")
        .join("target")
        .join("mmio-tests");
    let dir = root.join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("src")).expect("create the scratch project");
    dir
}

/// The RISC-V linker script the accepting cases would need if they were ever
/// linked. `kelpc check` stops before the backend, so this is here for the
/// project to be a real one rather than for any link.
const LINKER_SCRIPT: &str = "\
ENTRY(_start)
SECTIONS
{
    . = 0x80000000;
    .text : { *(.text.boot) *(.text .text.*) }
    .rodata : { *(.rodata .rodata.*) }
    .data : { *(.data .data.*) }
    .bss : ALIGN(8) { __bss_start = .; *(.bss .bss.*) *(COMMON); . = ALIGN(8); __bss_end = .; }
    . = ALIGN(16);
    __stack_top = . + 0x10000;
}
";

/// A freestanding riscv64 project whose manifest's `[caps]` section body is
/// `caps` (`""` for none) and whose `src/` holds `files`.
fn project(name: &str, caps: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = scratch(name);
    let manifest = format!(
        "name = \"mmio_case\"\n\n\
         [target]\n\
         mode = \"freestanding\"\n\
         triple = \"riscv64-unknown-none-elf\"\n\
         entry = \"kmain\"\n\
         linker_script = \"kernel.ld\"\n\n\
         [unsafe]\n\
         modules = [\"main\", \"uart\", \"spi\"]\n\n\
         {caps}"
    );
    fs::write(dir.join("coral.toml"), manifest).expect("write the manifest");
    fs::write(dir.join("kernel.ld"), LINKER_SCRIPT).expect("write the linker script");
    for (file, text) in files {
        fs::write(dir.join("src").join(file), text).expect("write a source file");
    }
    dir
}

fn check(dir: &Path) -> Output {
    kelpc()
        .arg("check")
        .arg(dir)
        .output()
        .expect("failed to spawn kelpc")
}

fn accepted(dir: &Path) {
    let out = check(dir);
    assert!(
        out.status.success(),
        "expected '{}' to check, but it failed:\n{}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn refused(dir: &Path, needle: &str) {
    let out = check(dir);
    assert!(
        !out.status.success(),
        "expected '{}' to be refused, but it checked",
        dir.display()
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(needle),
        "expected the error for '{}' to mention '{needle}', got:\n{stderr}",
        dir.display()
    );
}

/// The entry module, which takes its device's address itself, at `addr`.
fn entry_at(addr: &str) -> String {
    format!(
        "const ADDR: usize = {addr}\n\n\
         section(\".text.boot\") fun kmain() {{\n\
         \x20   val p: Ptr<u8> = unsafe {{ Ptr.from_addr(ADDR) }}\n\
         \x20   unsafe {{ p.write_volatile(65) }}\n\
         \x20   while true {{ unsafe {{ asm(\"wfi\") }} }}\n\
         }}\n"
    )
}

/// The entry module, which takes an address the compiler cannot fold: the
/// value comes back from an `asm` as a runtime result rather than a constant.
const ENTRY_COMPUTED: &str = "\
section(\".text.boot\") fun kmain() {
    val n: usize = unsafe { asm(\"li {0}, 0x10000000\", out(\"r\") _) }
    val p: Ptr<u8> = unsafe { Ptr.from_addr(n) }
    unsafe { p.write_volatile(65) }
    while true { unsafe { asm(\"wfi\") } }
}
";

/// The NS16550 driver the accepting cases grant, whose address is a
/// compile-time constant.
const UART_DRIVER: &str = "\
const UART_BASE: usize = 0x10000000

pub fun putc(c: u8) {
    unsafe {
        val thr: Ptr<u8> = Ptr.from_addr(UART_BASE)
        thr.write_volatile(c)
    }
}

pub fun halt() {
    while true {
        unsafe { asm(\"wfi\") }
    }
}
";

const UART_RANGE: &str = "0x10000000+0x100";

/// The entry holds one device's range and builds a pointer at an address inside
/// it: the grant is exactly the authority the call needs.
#[test]
fn an_address_inside_the_granted_range_is_accepted() {
    let caps = format!("[caps]\nmmio = [\"{UART_RANGE}\"]\n");
    let main = entry_at("0x10000000");
    accepted(&project("inside", &caps, &[("main.kp", &main)]));
}

/// The range is half-open: `base + len - 1` is the last byte the grant admits.
#[test]
fn the_last_byte_of_the_range_is_accepted() {
    let caps = format!("[caps]\nmmio = [\"{UART_RANGE}\"]\n");
    let main = entry_at("0x100000FF");
    accepted(&project("last_byte", &caps, &[("main.kp", &main)]));
}

/// ... and `base + len` is the first byte it does not.
#[test]
fn the_first_byte_past_the_range_is_refused() {
    let caps = format!("[caps]\nmmio = [\"{UART_RANGE}\"]\n");
    let main = entry_at("0x10000100");
    let dir = project("past_the_end", &caps, &[("main.kp", &main)]);
    refused(&dir, "0x10000100");
    refused(&dir, UART_RANGE);
}

/// An address outside the granted range names both the address and the ranges
/// the module does hold, so the line to change is obvious.
#[test]
fn an_address_outside_the_granted_range_is_refused() {
    let caps = format!("[caps]\nmmio = [\"{UART_RANGE}\"]\n");
    let main = entry_at("0x20000000");
    let dir = project("outside", &caps, &[("main.kp", &main)]);
    refused(&dir, "would take the address 0x20000000");
    refused(&dir, "granted: 0x10000000+0x100");
}

/// The whole-address-space wildcard is the one grant that admits an address the
/// compiler cannot fold, and it admits every constant too.
#[test]
fn the_whole_address_space_grant_admits_a_computed_address() {
    accepted(&project(
        "wildcard",
        "[caps]\nmmio = [\"*\"]\n",
        &[("main.kp", ENTRY_COMPUTED)],
    ));
}

/// A computed address against a range grant is refused, and the message says
/// which grant would admit it: the compiler cannot prove the address lands
/// inside the range.
#[test]
fn a_computed_address_needs_the_whole_address_space_grant() {
    let caps = format!("[caps]\nmmio = [\"{UART_RANGE}\"]\n");
    let dir = project("computed", &caps, &[("main.kp", ENTRY_COMPUTED)]);
    refused(&dir, "cannot fold");
    refused(&dir, "caps(mmio = [\"*\"])");
}

/// A manifest with no `[caps]` section leaves the entry module ungranted for
/// `mmio`, unlike every other domain: a device's address is a machine fact that
/// has to be written down.
#[test]
fn the_entry_module_needs_an_explicit_grant() {
    let main = entry_at("0x10000000");
    let dir = project("entry_ungranted", "", &[("main.kp", &main)]);
    refused(&dir, "needs an 'mmio' grant");
    refused(&dir, "module 'main' holds none");
}

/// A project module that names an address holds nothing unless its importer
/// delegated the range; a bare `impl` line is not a grant.
#[test]
fn a_project_module_with_no_grant_is_refused() {
    let caps = format!("[caps]\nmmio = [\"{UART_RANGE}\"]\n");
    let main = "impl \"uart\"\n\nsection(\".text.boot\") fun kmain() {\n\
                \x20   uart.putc(65)\n\
                \x20   uart.halt()\n\
                }\n";
    let dir = project(
        "module_ungranted",
        &caps,
        &[("main.kp", main), ("uart.kp", UART_DRIVER)],
    );
    refused(&dir, "needs an 'mmio' grant");
    refused(&dir, "module 'uart' holds none");
}

/// A malformed range is refused where it is written, before any call is
/// checked: a bare address is not a range.
#[test]
fn a_range_with_no_length_is_refused() {
    let main = entry_at("0x10000000");
    let dir = project(
        "bad_syntax",
        "[caps]\nmmio = [\"0x10000000\"]\n",
        &[("main.kp", &main)],
    );
    refused(&dir, "is not an mmio range");
    refused(&dir, "0x10000000+0x100");
}

/// A zero-length range grants nothing and is a mistake, so it is refused by
/// name rather than silently admitting no address.
#[test]
fn a_zero_length_range_is_refused() {
    let main = entry_at("0x10000000");
    let dir = project(
        "zero_length",
        "[caps]\nmmio = [\"0x10000000+0x0\"]\n",
        &[("main.kp", &main)],
    );
    refused(&dir, "zero length");
}

/// Two ranges that share a byte are ambiguous and a sign the grant was meant to
/// be one range, so the grant is refused rather than unioned.
#[test]
fn overlapping_ranges_are_refused() {
    let main = entry_at("0x10000000");
    let dir = project(
        "overlap",
        "[caps]\nmmio = [\"0x10000000+0x100\", \"0x10000080+0x100\"]\n",
        &[("main.kp", &main)],
    );
    refused(&dir, "overlap");
}

/// The wildcard is the whole address space, not a pattern that can be listed
/// beside a range.
#[test]
fn the_wildcard_cannot_be_mixed_with_a_range() {
    let main = entry_at("0x10000000");
    let dir = project(
        "wildcard_and_range",
        "[caps]\nmmio = [\"*\", \"0x10000000+0x100\"]\n",
        &[("main.kp", &main)],
    );
    refused(&dir, "cannot be written beside a range");
}

/// Two drivers, each granted its own device: the one that reaches its own range
/// is accepted, and the one that reaches the other device's address is refused
/// by name, which is the whole point of scoping the grant per module.
#[test]
fn a_second_driver_cannot_reach_another_devices_address() {
    let caps = format!(
        "[caps]\nmmio = [\"{UART_RANGE}\", \"0x20000000+0x100\"]\n"
    );
    let main = "impl \"uart\" caps(mmio = [\"0x10000000+0x100\"])\n\
                impl \"spi\" caps(mmio = [\"0x20000000+0x100\"])\n\n\
                section(\".text.boot\") fun kmain() {\n\
                \x20   uart.putc(65)\n\
                \x20   spi.poke()\n\
                \x20   uart.halt()\n\
                }\n";
    // `spi` was granted the 0x20000000 window and reaches 0x30000000 instead.
    let spi = "const SPI_BASE: usize = 0x30000000\n\n\
               pub fun poke() {\n\
               \x20   unsafe {\n\
               \x20       val p: Ptr<u8> = Ptr.from_addr(SPI_BASE)\n\
               \x20       p.write_volatile(1)\n\
               \x20   }\n\
               }\n";
    let dir = project(
        "second_driver",
        &caps,
        &[
            ("main.kp", main),
            ("uart.kp", UART_DRIVER),
            ("spi.kp", spi),
        ],
    );
    refused(&dir, "would take the address 0x30000000");
    refused(&dir, "module 'spi'");
    refused(&dir, "granted: 0x20000000+0x100");

    // The same two drivers, each inside its own grant, check cleanly: the
    // refusal above is the address, not the shape of the project.
    let spi_ok = spi.replace("0x30000000", "0x20000000");
    accepted(&project(
        "two_drivers_ok",
        &caps,
        &[
            ("main.kp", main),
            ("uart.kp", UART_DRIVER),
            ("spi.kp", &spi_ok),
        ],
    ));
}
