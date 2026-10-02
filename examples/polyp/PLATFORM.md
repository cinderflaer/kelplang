# Polyp's platform contract

*The plain-terms description of what an operating system has to provide to run a Polyp program on
`examples/polyp/soft_backend.kp`: the four functions and the buffer the soft backend draws into
(stage 3), and the extra facts a **bare-metal** project has to write down (stage 4, proved by
`examples/polyp-fb-riscv64/` and `examples/polyp-fb-x86_64/`). The design is `plan/polyp-v3.md`.*

Polyp's soft backend owns no display. It owns a retained widget tree, lays it out with integers,
and paints into a framebuffer the platform hands it. Everything the platform has to do is four
functions and one buffer, and this file says exactly what each has to be.

## The four functions

The interface is `trait Platform` in `examples/polyp/soft/platform.kp`. A platform implements it
for its own type.

### `framebuffer() -> Fb`

Returns the framebuffer the platform owns. `Fb` is `fb.new(pixels, width, height, stride_px)`:

- `pixels` is a `Ptr<u32>` to the first pixel.
- A pixel is one `u32`, `0xAARRGGBB`: blue in the low 8 bits, then green, then red, then alpha in
  the high 8 bits. The backend always writes an opaque `0xFF` alpha.
- `width` and `height` are the visible size in pixels.
- `stride_px` is the distance between the starts of two rows, **in pixels, not bytes**. It must be
  at least `width`. A platform whose buffer has padding (a pitch wider than the image) sets
  `stride_px` to that pitch and the backend accounts for it; a tightly packed buffer sets
  `stride_px == width`. The byte offset of `(x, y)` is `pixels + (y * stride_px + x) * 4`.

`framebuffer()` is called once at the start of every frame, so a platform that resizes or swaps
its buffer does so between frames. The backend reads the size every frame and re-lays-out when it
changes.

### `present(fb, dirty_rects)`

Called at the end of a frame **only when at least one pixel changed**. `fb` is the framebuffer
just painted; `dirty_rects` is the exact list of rectangles that changed, in no particular order
and with overlaps already merged by the damage tracker. A platform may:

- copy just those rectangles to the screen (the cheap choice), or
- present the whole framebuffer and ignore the list, or
- record them for a test.

`present` is never called with an empty list, so an idle frame is a frame that does no work and
calls nothing. A platform must not assume a rectangle is a multiple of anything: the backend
writes whole pixels inside each rectangle and nothing outside it.

### `now_ms() -> u64`

A monotonic clock in milliseconds. The requested soft backend uses it only for one thing: deciding
which scripted input events a `HeadlessPlatform` has reached. A real platform returns its own
clock; it may return 0 forever and nothing breaks, because the backend does not time anything
itself.

### `poll_event() -> Option<Event>`

The next pending input event, oldest first, or `None` when the queue is empty. The backend calls
it in a loop until it answers `None`, so a platform may return events one per call and does not
have to count them. `Event` is the six-variant enum in `examples/polyp/soft/event.kp`:

- `PointerMove(x, y)`, `PointerDown(x, y, button)`, `PointerUp(x, y, button)`: a mouse or touch
  pointer, in framebuffer pixels. Only `BUTTON_LEFT` activates a widget.
- `Scroll(dx, dy)`: a wheel step in the scrolled area under the last pointer position.
- `Key(code, mods)`: a key from the small code table at the top of `event.kp`, with `MOD_SHIFT` /
  `MOD_CTRL` / `MOD_ALT` bits.
- `Char(codepoint)`: a text codepoint the platform decoded.

`poll_event` **must not block**: a frame polls what is queued and returns. A platform that must
wait for input does so before it calls `frame`, not inside `poll_event`.

## The tick

An operating system calls one function per tick:

```text
soft_backend.frame(backend, platform, tree, state, view, update)
```

It takes the framebuffer, lays out the tree, drains every pending platform event into the
backend, runs one turn of the application loop (`update`/`view`/`diff`/`apply`), lays out again
for whatever the patches changed, paints the accumulated damage, and calls `present` if anything
changed. It returns a `core.Tick` carrying the new tree and whether the program is `Idle`,
`Changed` or asked to `Quit`. A loop is therefore:

```text
var running = true
while running {
    when soft_backend.frame(backend, platform, tree, state, view, update) {
        Idle(next) -> tree = next
        Changed(next) -> tree = next
        Quit(next) -> { tree = next; running = false }
    }
}
```

A frame with no input and no change paints nothing, presents nothing, and returns `Idle`.

## Allocation contract

- **The platform owns the framebuffer.** It allocates it (or points at a memory-mapped device)
  before the first frame and keeps it valid for as long as the backend uses it. The backend never
  allocates or frees pixels.
- **The backend allocates only when a patch creates a widget.** The retained tree grows by one
  node, and by one list insertion in its parent, for every `Create` patch. It never shrinks its
  capacity. An application whose tree stops changing allocates nothing.
- **A frame allocates nothing.** Damage is a fixed 16-rectangle list, clips are values, and the
  only scratch is a 560-byte glyph mask on the stack. There is no per-frame `malloc`, `List.push`
  or `String` build in the paint or input path.
- **The application state is owned by the caller**, not by the platform or the backend. The
  backend holds only widgets and geometry.

## Timing contract

- `frame` is synchronous and non-blocking. It does the work of one tick and returns.
- A platform calls `frame` once per tick. Two calls in one tick are two frames; a tick with no
  call is a tick that did not repaint. The backend has no internal clock and no timer.
- Input is not lost if events arrive between frames: `poll_event` keeps them queued until a frame
  drains them.
- `present` is called from inside `frame`, on the same thread, before `frame` returns. A platform
  that wants double buffering swaps buffers after copying the dirty rectangles and before the
  next `framebuffer()` call.
- The return value is the only thing a caller has to look at each tick; nothing is delivered by a
  callback and the platform's own threads are not used.

## What the platform does not have to do

- No event loop, no window, no display server.
- No text shaping or font loading: the font is embedded in `examples/polyp/soft/font.kp`.
- No input decoding beyond its own system: the platform turns its keys, pointer and text into the
  six `Event` variants and passes them on.
- No clipping arithmetic: the backend clips every primitive to the framebuffer and to each damage
  rectangle.

## What a minimal implementation looks like

`examples/polyp/soft/headless_platform.kp` is a complete platform in about a hundred lines: it
allocates the buffer with `Ptr.alloc<u32>`, records the presented rectangles and the frame
checksum, and plays a scripted event list. `examples/polyp/soft/ppm_platform.kp` adds a PPM dump.
`examples/polyp-todo-soft/` drives the todo program through `frame` and pins the checksums.

## What a freestanding OS must provide

There is no libc and no runtime behind a bare-metal Polyp image, so a project writes a few more
facts down in its `coral.toml`. `examples/polyp-fb-riscv64/` is the worked example; everything
below is there in about two hundred lines of project code, and `examples/polyp-fb-x86_64/` is the
same program on the second ISA family (its `fb_platform.kp` is literally the same file).

1. **A framebuffer.** The platform owns it. A `static mut [u32; W * H] = [0; W * H]` in `.bss`
is enough, and the compiler's `_start` stub zeroes `.bss` before the entry function runs, so the
surface starts black the way `HeadlessPlatform` zeroes its own buffer. A device framebuffer
works too; that path needs the `mmio` grant below, and this stage compared pixels only through a
static surface.
2. **The platform itself**, in a module of the project: `framebuffer` returns the `Fb` over the
static (bind the pointer with `Ptr.from_mut(FRAME)`, then `cast<u32>()`), `present` counts the
rectangles it was handed, `now_ms` returns the platform's own clock or a fake one, and
`poll_event` returns `None` or the next queued event. `examples/polyp-fb-riscv64/src/fb_platform.kp`
is the whole of it.
3. **`[target] alloc` and `dealloc`.** The retained tree, the scenario's patches and every
`String` the view builds allocate through these, so a freestanding image needs a pair of
functions with the fixed signatures (`fun alloc(size: usize, align: usize).R: Ptr<u8>`, `fun
dealloc(p: Ptr<u8>, size: usize, align: usize)`). `main.kp`'s bump arena over a second
`static mut [u8; N]` is the smallest one that works; a project that wants a real allocator
implements a better bump or a free list against the same two names.
4. **`[target] panic`.** A handler that never returns, so a bounds or overflow check (and a
failed allocation: the compiler calls it with `"out of memory"`) prints something instead of
wedging. The examples print `panic: <message>` over the UART and halt.
5. **A way to write bytes out.** The examples' `emit_line(line: str)` walks the line with
`str.len`/`str.byte` and writes the UART one byte at a time. Nothing in Polyp needs a formatted
write: the scenario builds its own lines with `str.from_i64` and a hex table, so a byte writer is
the whole requirement.
6. **`[unsafe] modules`.** The modules that take a pointer to the surface or write a pixel
(`fb_platform`, `soft/fb`, `soft/draw`) and the device driver. `core.kp`, `soft_backend.kp`,
`font.kp`, `damage.kp`, `event.kp`, `platform.kp` and the scenario contain no `unsafe`.
7. **`[caps] mmio`, only for a memory-mapped device.** A UART at a fixed address needs the grant
and an `impl "uart" caps(mmio = [...])` on the line that imports the driver. A port-mapped device
(a COM1 byte through `out`) needs no grant at all, which is why the x86-64 manifest has no
`[caps]` section.
8. **Optional: a display device.** The surface starts in memory, and putting it on a real screen is
one more device: the riscv64 example registers its static with QEMU's `ramfb` through fw_cfg, and
QEMU scans out that same memory, so what the renderer drew is what a screen shows. That is
`src/ramfb.kp`, its own `mmio` window and one more `[unsafe]` entry, and
`plan/polyp-ramfb-status.md` records it; a platform with a real framebuffer device does the
underlying job directly instead of through firmware.

What a freestanding project **cannot** do is the same list `plan/freestanding.md` gives: no
`print`, no libc, no `std/*` outside the admitted subset, no passdowns, no `String`/`Bytes`
without an allocator. The whole Polyp program the examples run needed four source changes to fit
(the progress fraction is an integer, the core's counters are `static`s, the entry-text snapshot
moved to a hosted-only module, and two types declare their own `Map` key pair); they are recorded
in `plan/polyp-v3.md`, stage 4.
