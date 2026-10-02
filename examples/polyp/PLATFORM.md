# Polyp's platform contract

*The plain-terms description of what an operating system has to provide to run a Polyp program on
`examples/polyp/soft_backend.kp`. The design is `plan/polyp-v3.md`, stage 3.*

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

## A minimal implementation

`examples/polyp/soft/headless_platform.kp` is a complete platform in about a hundred lines: it
allocates the buffer with `Ptr.alloc<u32>`, records the presented rectangles and the frame
checksum, and plays a scripted event list. `examples/polyp/soft/ppm_platform.kp` adds a PPM dump.
`examples/polyp-todo-soft/` drives the todo program through `frame` and pins the checksums.
