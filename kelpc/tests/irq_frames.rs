//! The `irq` and `frames` capability domains (kernel stage 18, MK10-S1's
//! second half): an interrupt line and a physical frame are values the gated
//! `irq.claim` / `frame.claim` build, checked against the calling module's
//! grant (`plan/irq-frame-caps.md`). Every refusal runs `kelpc check` (front
//! end only, so no `ld.lld` and no QEMU); the accepting cases build and run as
//! ordinary hosted programs, built by the LLVM backend.
//!
//! The cases are throwaway projects under the workspace's
//! `target/irq-frame-tests` rather than `tests/projects/*` directories, for the
//! same reason `mmio.rs` writes its own: each one differs from its neighbours by
//! a single grant line or a single argument, and a shared directory would copy
//! the project a dozen times. The range grammar's own unit tests live in
//! `stdlib.rs`.
//!
//! An accepting case asserts the project checks and that the program's stdout
//! is the claimed value; a refusing case asserts the message names the value,
//! the grant or the reason, so neither can pass for the wrong cause.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn kelpc() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_kelpc"));
    cmd.env_remove("KELPC_BACKEND");
    cmd.env_remove("KELPC_ORACLE");
    cmd
}

/// A fresh directory per case: the cases run in parallel and must not see each
/// other's files. It lives under the workspace's `target/`, which is inside the
/// checkout and gitignored.
fn scratch(name: &str) -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crate is a workspace member, so it has a parent")
        .join("target")
        .join("irq-frame-tests");
    let dir = root.join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("src")).expect("create the scratch project");
    dir
}

/// A hosted project whose manifest body is `caps` and whose `src/` holds
/// `files`. Hosted is enough: the check is a front-end one and applies in both
/// modes, and a hosted program needs no linker script or QEMU to be run.
fn project(name: &str, caps: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = scratch(name);
    fs::write(
        dir.join("coral.toml"),
        format!("name = \"irq_frame_case\"\n\n{caps}"),
    )
    .expect("write the manifest");
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

/// Build `dir` into this case's own binary and return its path.
fn build(dir: &Path) -> PathBuf {
    let bin = std::env::var("CARGO_TARGET_TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir())
        .join(format!(
            "irq_frames_{}",
            std::thread::current().name().unwrap_or("main").replace("::", "_")
        ));
    let out = kelpc()
        .arg("build")
        .arg(dir)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        out.status.success(),
        "expected '{}' to build, but it failed:\n{}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    bin
}

/// The compiled program run under hard limits (1 GiB of address space, 10
/// seconds of CPU and of wall clock), so a case that goes wrong is a failure,
/// not a runaway.
fn run_bounded(bin: &Path) -> Output {
    Command::new("sh")
        .arg("-c")
        .arg("ulimit -v 1048576; ulimit -t 10; timeout 10 \"$1\"")
        .arg("sh")
        .arg(bin)
        .output()
        .expect("failed to run the compiled program")
}

/// Build `dir` and assert the program's stdout is `expected`.
fn runs(dir: &Path, expected: &str) {
    let bin = build(dir);
    let run = run_bounded(&bin);
    assert!(
        run.status.success(),
        "expected '{}' to run, but it exited with {:?}:\n{}",
        dir.display(),
        run.status,
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        expected,
        "the stdout of '{}' is not what the claims say",
        dir.display()
    );
}

/// Build `dir` and assert the program panics, naming `needle`. The one runtime
/// check this stage adds is `Frame.claim`'s alignment test, which fires for a
/// computed address under the `["*"]` grant.
fn panics(dir: &Path, needle: &str) {
    let bin = build(dir);
    let run = run_bounded(&bin);
    assert!(
        !run.status.success(),
        "expected '{}' to panic, but it exited cleanly",
        dir.display()
    );
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.contains(needle),
        "expected the panic from '{}' to mention '{needle}', got:\n{stderr}",
        dir.display()
    );
}

/// An entry module that claims one line and prints its number.
fn main_line(line: &str) -> String {
    format!(
        "impl \"std/irq\"\n\nfun main() {{\n    val line: irq.Line = irq.claim({line})\n    print(line.number())\n}}\n"
    )
}

/// An entry module that claims one frame and prints its address and its offset.
fn main_frame(frame: &str) -> String {
    format!(
        "impl \"std/frame\"\n\nfun main() {{\n    val frame: frame.Frame = frame.claim({frame})\n    print(frame.addr())\n    print(frame.offset(16))\n}}\n"
    )
}

/// The entry module that takes a line the compiler cannot fold: the value comes
/// from a parameter rather than a literal.
const MAIN_COMPUTED_LINE: &str = "\
impl \"std/irq\"

fun claim_it(n: u32).return: irq.Line = irq.claim(n)

fun main() {
    print(claim_it(10).number())
}
";

/// The entry module that takes a frame address the compiler cannot fold.
const MAIN_COMPUTED_FRAME: &str = "\
impl \"std/frame\"

fun claim_it(addr: usize).return: frame.Frame = frame.claim(addr)

fun main() {
    print(claim_it(0x80100000).addr())
}
";

/// The entry module whose frame address comes through a parameter, so the
/// compiler cannot fold it and the `["*"]` grant admits it: the alignment test
/// is then the runtime one in `Frame.claim`.
const MAIN_RUNTIME_MISALIGNED: &str = "\
impl \"std/frame\"

fun claim_it(addr: usize).return: frame.Frame = frame.claim(addr)

fun main() {
    print(claim_it(0x80100001).addr())
}
";

/// The entry module whose `Frame.offset` byte comes through a parameter, so the
/// bounds test is the runtime one.
const MAIN_RUNTIME_OFFSET: &str = "\
impl \"std/frame\"

fun off(f: frame.Frame, byte: usize).return: usize = f.offset(byte)

fun main() {
    val f: frame.Frame = frame.claim(0x80100000)
    print(off(f, 4096))
}
";

const FRAME_RANGE: &str = "0x80100000+0x100000";

// ---------------------------------------------------------------------------
// Accepting cases: the value is inside the grant, and the program prints it.
// ---------------------------------------------------------------------------

/// A line inside the grant is the authority the call needs, and the value it
/// carries is the number the compiler admitted.
#[test]
fn a_line_inside_the_grant_is_accepted_and_runs() {
    let dir = project(
        "line_inside",
        "[caps]\nirq = [\"10\"]\n",
        &[("main.kp", &main_line("10"))],
    );
    runs(&dir, "10\n");
}

/// The grant is half-open at the top: `32-47` admits 47 (the last line) and
/// refuses 48, which is the first line it does not.
#[test]
fn the_last_granted_line_is_accepted_and_the_next_is_refused() {
    let caps = "[caps]\nirq = [\"32-47\"]\n";
    let dir = project("line_last", caps, &[("main.kp", &main_line("47"))]);
    runs(&dir, "47\n");

    let refused_dir = project("line_next", caps, &[("main.kp", &main_line("48"))]);
    refused(&refused_dir, "would take the line 48");
    refused(&refused_dir, "granted: 32-47");
}

/// A frame inside the grant is accepted, and its address and offset are the
/// ones the module computes.
#[test]
fn a_frame_inside_the_grant_is_accepted_and_runs() {
    let caps = format!("[caps]\nframes = [\"{FRAME_RANGE}\"]\n");
    let dir = project("frame_inside", &caps, &[("main.kp", &main_frame("0x80100000"))]);
    runs(&dir, "2148532224\n2148532240\n");
}

/// The frame range's end is exclusive at the address a frame can hold: the last
/// aligned frame is `base + len - 4096`, and `base + len` is refused.
#[test]
fn the_frame_range_end_is_exclusive() {
    let caps = format!("[caps]\nframes = [\"{FRAME_RANGE}\"]\n");
    let last = project("frame_last", &caps, &[("main.kp", &main_frame("0x801FF000"))]);
    accepted(&last);

    let past = project("frame_past", &caps, &[("main.kp", &main_frame("0x80200000"))]);
    refused(&past, "would take the address 0x80200000");
    refused(&past, FRAME_RANGE);
}

/// A computed misaligned frame is admitted at compile time under `["*"]` and
/// caught by `Frame.claim`'s runtime test, which is the one runtime check this
/// stage adds.
#[test]
fn a_runtime_misaligned_frame_panics() {
    let dir = project(
        "runtime_misaligned",
        "[caps]\nframes = [\"*\"]\n",
        &[("main.kp", MAIN_RUNTIME_MISALIGNED)],
    );
    panics(&dir, "Frame.claim: address is not 4096-aligned");
}

/// `Frame.offset` is bounds-checked against the frame's own 4096 bytes.
#[test]
fn an_offset_past_the_frame_panics() {
    let dir = project(
        "runtime_offset",
        "[caps]\nframes = [\"*\"]\n",
        &[("main.kp", MAIN_RUNTIME_OFFSET)],
    );
    panics(&dir, "Frame.offset: byte is outside the frame");
}

/// The wildcards are the escape hatch: a line and a frame that no range names
/// are admitted when the module holds the whole resource.
#[test]
fn the_wildcards_admit_a_claimed_line_and_frame() {
    let line = project(
        "wildcard_line",
        "[caps]\nirq = [\"*\"]\n",
        &[("main.kp", &main_line("999"))],
    );
    runs(&line, "999\n");

    let frame = project(
        "wildcard_frame",
        "[caps]\nframes = [\"*\"]\n",
        &[("main.kp", &main_frame("0x90000000"))],
    );
    runs(&frame, "2415919104\n2415919120\n");
}

// ---------------------------------------------------------------------------
// Refusing cases: the grant, the value or the constructor is wrong.
// ---------------------------------------------------------------------------

/// A computed line against a range grant is refused, and the message says which
/// grant would admit it: the compiler cannot prove a runtime value lands in the
/// range.
#[test]
fn a_computed_line_needs_the_whole_resource_grant() {
    let dir = project(
        "computed_line",
        "[caps]\nirq = [\"10\"]\n",
        &[("main.kp", MAIN_COMPUTED_LINE)],
    );
    refused(&dir, "cannot fold");
    refused(&dir, "caps(irq = [\"*\"])");
}

/// A computed frame against a range grant is refused the same way.
#[test]
fn a_computed_frame_needs_the_whole_resource_grant() {
    let caps = format!("[caps]\nframes = [\"{FRAME_RANGE}\"]\n");
    let dir = project("computed_frame", &caps, &[("main.kp", MAIN_COMPUTED_FRAME)]);
    refused(&dir, "cannot fold");
    refused(&dir, "caps(frames = [\"*\"])");
}

/// A frame is 4 KiB-aligned, so a misaligned constant is refused whatever the
/// grant, including the wildcard.
#[test]
fn a_misaligned_constant_frame_is_refused_whatever_the_grant() {
    let wildcard = project(
        "misaligned_wild",
        "[caps]\nframes = [\"*\"]\n",
        &[("main.kp", &main_frame("0x80100001"))],
    );
    refused(&wildcard, "is not 4096-aligned");
    refused(&wildcard, "0x80100001");

    let caps = format!("[caps]\nframes = [\"{FRAME_RANGE}\"]\n");
    let ranged = project("misaligned_ranged", &caps, &[("main.kp", &main_frame("0x80100001"))]);
    refused(&ranged, "is not 4096-aligned");
}

/// A `Line` cannot be built by a literal from another module: the constructor is
/// sealed to `std/irq`, and `claim` is the one way in.
#[test]
fn a_forged_line_is_refused() {
    let main = "\
impl \"std/irq\"

fun main() {
    val line: irq.Line = irq.Line(n = 3)
    print(line.number())
}
";
    let dir = project("forged_line", "", &[("main.kp", main)]);
    refused(&dir, "is a capability value");
    refused(&dir, "'std/irq'");
}

/// A `Frame` cannot be built by a literal either.
#[test]
fn a_forged_frame_is_refused() {
    let main = "\
impl \"std/frame\"

fun main() {
    val frame: frame.Frame = frame.Frame(addr = 0x80100000)
    print(frame.addr())
}
";
    let dir = project("forged_frame", "", &[("main.kp", main)]);
    refused(&dir, "is a capability value");
    refused(&dir, "'std/frame'");
}

/// A module that holds no grant and calls the constructor is refused by the
/// domain gate, before any value question is asked.
#[test]
fn a_module_with_no_grant_is_refused() {
    let main = "impl \"drv\"\n\nfun main() { drv.go() }\n";
    let drv = "\
impl \"std/irq\"

pub fun go() {
    val line: irq.Line = irq.claim(10)
    print(line.number())
}
";
    let dir = project("no_grant", "", &[("main.kp", main), ("drv.kp", drv)]);
    refused(&dir, "needs the 'irq' capability");
    refused(&dir, "module 'drv'");
}

/// The `frames` domain has the same gate: an ungranted module cannot claim one.
#[test]
fn an_ungranted_frames_module_is_refused() {
    let main = "impl \"drv\"\n\nfun main() { drv.go() }\n";
    let drv = "\
impl \"std/frame\"

pub fun go() {
    val frame: frame.Frame = frame.claim(0x80100000)
    print(frame.addr())
}
";
    let dir = project("no_frames_grant", "", &[("main.kp", main), ("drv.kp", drv)]);
    refused(&dir, "needs the 'frames' capability");
    refused(&dir, "module 'drv'");
}

/// A line pattern is a decimal line number or an inclusive range; `ten` is
/// neither, and the refusal says what was expected.
#[test]
fn a_bad_line_pattern_is_refused() {
    let dir = project(
        "bad_line",
        "[caps]\nirq = [\"ten\"]\n",
        &[("main.kp", &main_line("10"))],
    );
    refused(&dir, "is not an irq line or range");
}

/// An inverted `LO-HI` names the empty range it is, rather than granting
/// nothing silently.
#[test]
fn an_empty_line_range_is_refused() {
    let dir = project(
        "empty_line",
        "[caps]\nirq = [\"47-32\"]\n",
        &[("main.kp", &main_line("10"))],
    );
    refused(&dir, "is empty");
}

/// A zero-length frame range grants nothing and is a mistake, so it is refused
/// by name.
#[test]
fn a_zero_length_frame_range_is_refused() {
    let dir = project(
        "zero_frame",
        "[caps]\nframes = [\"0x80100000+0x0\"]\n",
        &[("main.kp", &main_frame("0x80100000"))],
    );
    refused(&dir, "zero length");
}

/// Two line ranges that share a line are ambiguous, so the grant is refused
/// rather than unioned.
#[test]
fn overlapping_line_ranges_are_refused() {
    let dir = project(
        "overlap_lines",
        "[caps]\nirq = [\"10\", \"10-12\"]\n",
        &[("main.kp", &main_line("10"))],
    );
    refused(&dir, "overlap");
}

/// Two frame ranges that share a byte are refused for the same reason.
#[test]
fn overlapping_frame_ranges_are_refused() {
    let dir = project(
        "overlap_frames",
        "[caps]\nframes = [\"0x80100000+0x1000\", \"0x80100800+0x1000\"]\n",
        &[("main.kp", &main_frame("0x80100000"))],
    );
    refused(&dir, "overlap");
}

/// The wildcard is the whole resource, not a pattern that can be listed beside
/// a range, in `irq` as in `mmio`.
#[test]
fn the_wildcard_cannot_be_mixed_with_a_range() {
    let dir = project(
        "wildcard_and_range",
        "[caps]\nirq = [\"*\", \"10\"]\n",
        &[("main.kp", &main_line("10"))],
    );
    refused(&dir, "cannot be written beside a range");
}

/// A second module granted exactly its own line cannot reach another one, which
/// is the point of scoping a grant per module.
#[test]
fn a_second_module_cannot_reach_another_line() {
    let caps = "[caps]\nirq = [\"10\", \"20\"]\n";
    let main = "impl \"drv\" caps(irq = [\"10\"])\n\nfun main() { drv.go() }\n";
    let drv = "\
impl \"std/irq\"

pub fun go() {
    val line: irq.Line = irq.claim(20)
    print(line.number())
}
";
    let dir = project("second_module", caps, &[("main.kp", main), ("drv.kp", drv)]);
    refused(&dir, "would take the line 20");
    refused(&dir, "module 'drv'");
    refused(&dir, "granted: 10");
}

/// The compile-error fixture that lives in the example directory is the same
/// refusal: the entry grants `ungranted` line 10 and the module reaches for 11.
/// It is a file, not part of the booting image, so no boot compiles it until a
/// case copies it in.
#[test]
fn the_examples_ungranted_fixture_is_refused() {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/kernel0-caps/src/ungranted.kp");
    let source =
        fs::read_to_string(&fixture).unwrap_or_else(|e| panic!("read '{}': {e}", fixture.display()));
    let main = "\
impl \"ungranted\" caps(irq = [\"10\"])

fun main() {
    print(ungranted.claim_an_ungranted_line().number())
}
";
    let dir = project("example_fixture", "[caps]\nirq = [\"10\"]\n", &[("main.kp", main)]);
    fs::write(dir.join("src").join("ungranted.kp"), source).expect("copy the fixture");
    refused(&dir, "would take the line 11");
    refused(&dir, "module 'ungranted'");
}

/// Constant folding is at the operand's own width, as the emitted code
/// computes: a `u32` line wraps at 32 bits, so `0xFFFFFFFF + 11` is line 10
/// (what the program really claims), not the 64-bit 4294967306 the folder used
/// to see. A shift by the width or more is not a constant at all.
#[test]
fn a_line_folds_at_its_own_width() {
    let caps = "[caps]\nirq = [\"10\"]\n";
    let dir = project(
        "line_wraps_u32",
        caps,
        &[("main.kp", &main_line("0xFFFFFFFF + 11"))],
    );
    runs(&dir, "10\n");

    let wide = project(
        "line_wide_shift",
        caps,
        &[("main.kp", &main_line("10 + (1 << 31) + (1 << 31)"))],
    );
    runs(&wide, "10\n");

    let outside = project(
        "line_wraps_outside",
        caps,
        &[("main.kp", &main_line("0xFFFFFFFF + 12"))],
    );
    refused(&outside, "would take the line 11");
}
