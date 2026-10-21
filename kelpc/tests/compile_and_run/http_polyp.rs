use super::*;

/// Polyp v3 stage 3: the todo program on the soft backend, at both golden sizes.
///
/// `examples/polyp-todo-soft` links `examples/polyp-todo/src/app.kp` (the program) and
/// `examples/polyp-todo/src/scenario.kp` (the driver the framebuffer kernels share with it), so its
/// final model and the library's counters are the ones `examples/polyp-todo/smoke.sh` and
/// `examples/polyp-core-test` produce. It prints one `step <n> checksum <hex> dirty <count>` line
/// per frame and then the report, and every line here is the golden value; `POLYP_SOFT_SIZE=big`
/// reruns the same binary at 800x600 for the second golden set, and `POLYP_SOFT_SIZE=kernel` at the
/// 320x200 the framebuffer kernels use, which is the expected output the two kernels below compare
/// against.
#[test]
pub(crate) fn polyp_todo_runs_on_the_soft_backend_with_the_same_counters() {
    let bin = out_path("polyp_todo_soft");
    let build = kelpc()
        .arg("build")
        .arg(fixture("tests/fixtures/examples/polyp-todo-soft"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected the polyp todo soft project to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = run_bounded(&bin, &[]);
    assert!(
        run.status.success(),
        "expected the 400x300 run to exit successfully, got:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        POLYP_TODO_SOFT_400,
        "unexpected output from the 400x300 todo-soft run"
    );
    let run = run_bounded(&bin, &[("POLYP_SOFT_SIZE", "big")]);
    assert!(
        run.status.success(),
        "expected the 800x600 run to exit successfully, got:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        POLYP_TODO_SOFT_800,
        "unexpected output from the 800x600 todo-soft run"
    );
}

pub(crate) const POLYP_TODO_SOFT_400: &str = "step 0 checksum 0x3fe9bf4f4c216435 dirty 1
step 1 checksum 0x0b75e67efe727df5 dirty 1
step 2 checksum 0x30a73f0b7686ae98 dirty 1
step 3 checksum 0x5ea79458670ced5c dirty 2
step 4 checksum 0xd765bd454cf40665 dirty 1
step 5 checksum 0x6e014703e561504c dirty 2
step 6 checksum 0x133d1ca6e8e276a5 dirty 1
step 7 checksum 0x8e17675a724b10fd dirty 2
step 8 checksum 0xfa3d5e46fb8b2113 dirty 2
step 9 checksum 0x89b8f7d6bd68e19b dirty 2
step 10 checksum 0x12d8cf35c1606b27 dirty 3
step 11 checksum 0x12d8cf35c1606b27 dirty 0
display 0xc33211c98befabd7
-- todo --
2
alpha
false
beta
true
7
12
17
22
22
22
22
10
5
17
true
true
1
gamma

end
";

pub(crate) const POLYP_TODO_SOFT_800: &str = "step 0 checksum 0xf0430e4ebe066935 dirty 1
step 1 checksum 0xf4497c1d3a354775 dirty 1
step 2 checksum 0xf93c79c54dcb06f8 dirty 1
step 3 checksum 0x3f9118f05cc2f7fc dirty 2
step 4 checksum 0xd539e0e8a372d425 dirty 1
step 5 checksum 0x8f9684b6e753a46c dirty 2
step 6 checksum 0xebde2f6e1f6fe7e5 dirty 1
step 7 checksum 0x5d3fbf9923562efd dirty 2
step 8 checksum 0xe451f4fb536873bf dirty 2
step 9 checksum 0xe4bf0c25d129d687 dirty 2
step 10 checksum 0xf4b409b661bcc047 dirty 3
step 11 checksum 0xf4b409b661bcc047 dirty 0
display 0x33651316579eafb7
-- todo --
2
alpha
false
beta
true
7
12
17
22
22
22
22
10
5
17
true
true
1
gamma

end
";

/// Polyp v3 stage 4: the hosted run at the framebuffer kernels' resolution, which is the expected
/// output for both kernels.
///
/// It builds `examples/polyp-todo-soft` and runs it with `POLYP_SOFT_SIZE=kernel` (320x200, the
/// kernels' `static mut` surface size), through the bounded runner every compiled program in this
/// file goes through.
pub(crate) fn hosted_fb_run() -> Vec<String> {
    let bin = out_path("polyp_fb_hosted");
    let build = kelpc()
        .arg("build")
        .arg(fixture("tests/fixtures/examples/polyp-todo-soft"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected the hosted soft project to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = run_bounded(&bin, &[("POLYP_SOFT_SIZE", "kernel")]);
    assert!(
        run.status.success(),
        "expected the hosted 320x200 run to exit successfully, got:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    run_slice(&String::from_utf8_lossy(&run.stdout))
}

/// The lines that make up one run of the shared scenario: from the first `step ` line to the first
/// `end` line, which is the report's last line. The hosted entry prints a `-- todo --` separator
/// before the report and the kernels do not, so it is dropped; everything else has to be equal.
/// The same function reads a kernel's serial output, so the two sides are sliced identically and
/// only the run itself can differ.
pub(crate) fn run_slice(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines
        .iter()
        .position(|line| line.starts_with("step "))
        .unwrap_or_else(|| panic!("no step line in the run's output:\n{text}"));
    let end = lines
        .iter()
        .position(|line| *line == "end")
        .unwrap_or_else(|| panic!("no `end` line in the run's output:\n{text}"));
    let out: Vec<String> = lines[start..=end]
        .iter()
        .filter(|line| **line != "-- todo --")
        .map(|line| line.to_string())
        .collect();
    // Twelve `step` lines, one per frame, the `display` line the run ends with, and the report's
    // twenty-one. A run whose shape changed would otherwise compare equal only by accident.
    assert_eq!(out.len(), 12 + 1 + 21, "the run's shape changed:\n{out:?}");
    out
}

/// `llvm-nm -u`'s output for one linked image, which is the proof that nothing in the Polyp run
/// needed libc or a soft-float helper: a freestanding image must name no undefined symbol at all.
pub(crate) fn undefined_symbols(image: &Path) -> String {
    tool("llvm-nm", &["-u", image.to_str().expect("a utf-8 image path")])
}

/// Polyp v3 stage 4: the riscv64 framebuffer kernel draws the hosted run's own pixels.
///
/// `examples/polyp-fb-riscv64` links the Polyp core, the stage-2 raster, the soft backend and the
/// shared todo scenario into a bare-metal riscv64 image with no libc: its own
/// `static mut [u32; 320 * 200]` surface, a bump arena behind `[target] alloc`/`dealloc`, the
/// scripted platform and the NS16550 driver. The expected output is the hosted soft backend's: this
/// test runs `examples/polyp-todo-soft` at `POLYP_SOFT_SIZE=kernel` and requires the kernel's
/// serial lines, both the `step <n> checksum ... dirty ...` lines and the report, to be identical.
/// One pixel difference anywhere fails it. `llvm-nm -u` on the image is the other half: the image
/// names no undefined symbol, so nothing reached libc or a soft-float helper. Both tests skip where
/// `ld.lld` or the emulator is absent (`dev.nix`), which is why they run in `dev-kernel.nix`.
#[test]
pub(crate) fn polyp_fb_riscv64_draws_the_hosted_runs_pixels() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU) {
        eprintln!(
            "skipping the riscv64 framebuffer kernel: 'ld.lld' or 'qemu-system-riscv64' is not on \
             PATH (run this test in dev-kernel.nix)"
        );
        return;
    }
    let expected = hosted_fb_run();
    let name = "polyp_fb_riscv64";
    let captured = boot_and_capture(&RISCV64_QEMU, name, &fixture("tests/fixtures/examples/polyp-fb-riscv64"), &|got| {
        got.lines().any(|line| line == "end")
    });
    let undefined = undefined_symbols(&out_path(name));
    assert!(
        undefined.trim().is_empty(),
        "the riscv64 Polyp image names undefined symbols, so something reached libc or soft float:\n{undefined}"
    );
    assert_eq!(
        run_slice(&captured),
        expected,
        "the riscv64 kernel's serial output differs from the hosted 320x200 run; captured:\n{captured}"
    );
}

/// Polyp v3 stage 4: the x86-64 framebuffer kernel draws the same pixels as the hosted run and as
/// the riscv64 kernel.
///
/// `examples/polyp-fb-x86_64` is the same program with the COM1 driver in place of the NS16550 and
/// the same `fb_platform.kp` source file (the two projects share it through a symlink). Born from
/// SeaBIOS' Xen PVH loader, it goes through the compiler's 32-bit stub into long mode before any
/// Kelp runs, so this test also covers the whole renderer on the second ISA family.
#[test]
pub(crate) fn polyp_fb_x86_64_draws_the_hosted_runs_pixels() {
    if !have_ld_lld() || !have_qemu(&X86_64_QEMU) {
        eprintln!(
            "skipping the x86-64 framebuffer kernel: 'ld.lld' or 'qemu-system-x86_64' is not on \
             PATH (run this test in dev-kernel.nix)"
        );
        return;
    }
    let expected = hosted_fb_run();
    let name = "polyp_fb_x86_64";
    let captured = boot_and_capture(&X86_64_QEMU, name, &fixture("tests/fixtures/examples/polyp-fb-x86_64"), &|got| {
        got.lines().any(|line| line == "end")
    });
    let undefined = undefined_symbols(&out_path(name));
    assert!(
        undefined.trim().is_empty(),
        "the x86-64 Polyp image names undefined symbols, so something reached libc or soft float:\n{undefined}"
    );
    assert_eq!(
        run_slice(&captured),
        expected,
        "the x86-64 kernel's serial output differs from the hosted 320x200 run; captured:\n{captured}"
    );
}

/// Polyp v3 stage 4's stretch: QEMU's `ramfb` device shows the kernel's framebuffer.
///
/// The riscv64 project registers its `static mut` surface with the `ramfb` device through fw_cfg (a
/// DMA write of `etc/ramfb`: address, `XR24`, flags, width, height, stride, all big-endian, which is
/// what `src/ramfb.kp` implements). This test boots the image with `-device ramfb` and a monitor on
/// a pipe, waits for the run's last line, asks the monitor for a `screendump`, and checks the PPM it
/// wrote against the kernel's own `display` line: FNV-1a 64 over each pixel's blue, green and red
/// bytes and a constant alpha, because a display shows only the colour (the framebuffer's alpha byte
/// carries glyph coverage, so `checksum` would not match a screen). The same function is what the
/// raster's `checksum_rgb` computes, so a single differing pixel fails it. Skips where `ld.lld` or
/// the emulator is absent.
#[test]
pub(crate) fn polyp_fb_riscv64_shows_its_framebuffer_on_a_real_display() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU) {
        eprintln!(
            "skipping the riscv64 ramfb boot: 'ld.lld' or 'qemu-system-riscv64' is not on PATH \
             (run this test in dev-kernel.nix)"
        );
        return;
    }
    let name = "polyp_fb_ramfb";
    let bin = out_path(name);
    let build = kelpc()
        .env_remove("KELPC_BACKEND")
        .arg("build")
        .arg(fixture("tests/fixtures/examples/polyp-fb-riscv64"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the freestanding ramfb build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let serial = out_path("polyp_fb_ramfb_serial").with_extension("txt");
    let shot = out_path("polyp_fb_ramfb_shot").with_extension("ppm");
    let _ = std::fs::remove_file(&serial);
    let _ = std::fs::remove_file(&shot);

    // `-display none` still creates the console `ramfb` attaches to, which is what makes a
    // `screendump` of the device possible in a headless test.
    let mut child = Command::new("qemu-system-riscv64")
        .args([
            "-machine",
            "virt",
            "-bios",
            "none",
            "-no-reboot",
            "-display",
            "none",
            "-device",
            "ramfb",
        ])
        .arg("-serial")
        .arg(format!("file:{}", serial.display()))
        .args(["-monitor", "stdio"])
        .arg("-kernel")
        .arg(&bin)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn qemu-system-riscv64");

    // Wait for the run to finish, then ask the monitor for the screen. Both waits are bounded by
    // the same 20 s the other boot tests use.
    let deadline = Instant::now() + ACCEPT_DEADLINE;
    let serial_text = wait_for_file_text(&serial, &deadline, |text| text.contains("\nend\n"));
    let mut stdin = child.stdin.take().expect("qemu's monitor pipe");
    writeln!(stdin, "screendump {}", shot.display()).expect("failed to ask for a screendump");
    stdin.flush().expect("failed to flush the monitor pipe");
    let _ = wait_for_file_text(&shot, &deadline, |_| true);
    writeln!(stdin, "quit").expect("failed to ask qemu to quit");
    let _ = stdin.flush();
    let _ = child.kill();
    let _ = child.wait();

    let ppm = std::fs::read(&shot).expect("the PPM the monitor wrote");
    let (width, height, body) = parse_ppm(&ppm);
    assert_eq!((width, height), (320, 200), "the screen the ramfb device reported");
    assert_eq!(body.len(), 320 * 200 * 3, "the PPM body length");
    let shown = ppm_rgb_checksum(width, height, body);

    // The kernel's own line, from its serial output: `display 0x<16 hex digits>`.
    let line = serial_text
        .lines()
        .find(|line| line.starts_with("display "))
        .unwrap_or_else(|| panic!("no `display` line in the kernel's serial output:\n{serial_text}"));
    let expected = u64::from_str_radix(line.trim_start_matches("display 0x"), 16)
        .unwrap_or_else(|_| panic!("unreadable display line '{line}'"));
    assert_eq!(
        shown, expected,
        "the display device's pixels differ from the kernel's framebuffer ({line})"
    );
}

/// Reads `path` until `done` accepts its text, or the deadline passes, and answers the text it last
/// saw (empty when the file never appeared). The bounded wait is what keeps a kernel that never
/// prints, or a monitor that never writes, from hanging the suite.
pub(crate) fn wait_for_file_text(path: &Path, deadline: &Instant, done: impl Fn(&str) -> bool) -> String {
    let mut last = String::new();
    while Instant::now() < *deadline {
        if let Ok(text) = std::fs::read_to_string(path) {
            if done(&text) {
                return text;
            }
            last = text;
        }
        thread::sleep(Duration::from_millis(50));
    }
    last
}
