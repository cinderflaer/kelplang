# The ramfb stretch: a real display shows the framebuffer (Polyp v3 stage 4)

*Status note for stage 4's stretch. It is not a plan and not a Mark; `plan/polyp-v3.md` owns the
stage. **It works**: QEMU's `ramfb` device scans out the kernel's framebuffer, and
`polyp_fb_riscv64_shows_its_framebuffer_on_a_real_display` in `kelpc/tests/compile_and_run.rs`
compares a `screendump` of that screen with the kernel's own checksum of the same pixels.*

## What is built

`examples/polyp-fb-riscv64/src/ramfb.kp` is the fw_cfg driver, about two hundred lines with the
comments. It uses the DMA interface only, and the three facts it needed were read out of QEMU's
`hw/nvram/fw_cfg.c` rather than guessed (each guess before that read was wrong):

- The MMIO device sits at `0x10100000`: data at +0, selector at +8, DMA address at +16, all three
  big-endian (`DEVICE_BIG_ENDIAN` on the memory-region ops).
- The 16-byte DMA descriptor in guest memory is **big-endian** (`be32_to_cpu`/`be64_to_cpu` on the
  way in, `stl_be_dma` for the control write-back). The control word is
  `select | write | (file_select << 16)` to write a file, and the ready poll is "the control word is
  zero", with the error bit landing in the word's highest byte when it is read natively.
- The *file directory* is selector `0x0019` and the first *file* is `0x0020`; a read past the end of
  a file is zero-filled rather than failed, so one fixed-size read fetches the whole table.

The project finds `etc/ramfb` in that table and writes the 28-byte `RAMFBCfg`:
`addr`, `fourcc` `XR24`, `flags`, `width`, `height`, `stride`, every one of them big-endian
(`hw/display/ramfb.c` reads them with `be32_to_cpu`/`be64_to_cpu`). The address is the physical
address of the project's `static mut [u32; 64000]`, which is the same memory the soft backend
paints; QEMU then scans that memory out, so the screen is live rather than a copy.

Two `[caps]`/`[unsafe]` entries come with it: the manifest grants the fw_cfg window
(`0x10100000+0x20`) beside the UART's, and `ramfb` joins the `[unsafe]` list because it is the
module that builds pointers from those addresses.

## The test

`polyp_fb_riscv64_shows_its_framebuffer_on_a_real_display` boots the image with
`-device ramfb -display none` and a monitor on a pipe, waits for the run's last line in the serial
file, sends `screendump <file>` to the monitor, quits QEMU, and reads the PPM. It asserts the PPM
is 320x200, then compares the FNV-1a 64 of its pixels with the kernel's own `display` line.

That line exists because of one detail the first attempt got wrong: the framebuffer's alpha byte
carries *glyph coverage* (a text pixel's alpha is its mask byte, which is what `soft/draw.kp`'s
`blit_mask8` blends), and a display has no alpha to show. The stage's `checksum` hashes all four
bytes, so it would never match a screen. `soft/fb.kp` therefore gained `checksum_rgb`: the same
FNV-1a over each pixel's blue, green and red bytes and a constant `0xFF` in the alpha's place. The
shared scenario driver emits it once, after the last step, as `display 0x<16 hex digits>`, which is
also what the hosted run and the x86-64 kernel print (so the three runs stay comparable), and the
test's `ppm_rgb_checksum` is the same function over the PPM. The two numbers were equal on the first
run that got the byte order right: the display shows the kernel's pixels, not merely something.

## What is deliberately not here

- Only the riscv64 `virt` machine is covered. QEMU supports `ramfb` on other machines too, and the
  module would need only the fw_cfg base as a parameter, but the x86-64 example is a COM1 boot and
  adding a display to it was not asked for.
- No live reconfiguration: the surface is registered once, from a `static mut` at a fixed window
  size, and never unregistered. There is no second-projector story, no resolution change and no
  format other than `XR24`.
- No `screendump` of an *animated* run: the test captures the finished screen once. The comparison
  is a checksum, not a pixel dump kept as a golden file.
- The fw_cfg driver is written for QEMU's MMIO layout and does not try to be a general fw_cfg
  client: no IO-port path, no `fw_cfg` file writes other than `etc/ramfb`, no DMA skip support.
- `plan/kernel-ub-notes.md`'s stage-4 note carries the undefined behaviour this adds (raw MMIO,
  a DMA descriptor in guest memory, and an address the device is trusted with).
