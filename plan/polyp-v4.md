# Polyp v4: timers and animation, UTF-8 text, an accessibility tree, memoised subtrees

v3 (`plan/polyp-v3.md`) ends with a list called "What is not promised". v4 closes the four items on
it that matter most for an OS UI. The design was decided up front; this file records what was built,
stage by stage, the numbers, and what v4 still does not do. Everything is integer math, nothing
allocates in the steady-state frame path that did not allocate before, and every test runs on both
backends (and, where noted, on the two freestanding kernels).

## Stage 1: timers and animation

### The clock

`soft/event.kp`'s `Event` gains `Tick(now_us: u64)`: the platform's monotonic clock in microseconds.
A platform sends one when its clock moved since the last one it sent, before the frame's input
events and at most one per frame. The backend keeps the last value (`clock_us`, never backwards);
timers and animations read only that. A platform that never sends a `Tick` has a clock stuck at 0,
so nothing animates and no timer fires: the clock is a documented platform requirement
(`examples/polyp/PLATFORM.md`).

`HeadlessPlatform.advance(ms)` / `advance_us(us)` set the test clock and queue the `Tick`. The
framebuffer kernels' `FbPlatform` gets `advance_ms` (what a timer interrupt handler does) and
`set_auto_clock(false)` to take the clock over from v3's "a presented frame is 100 ms" rule, which
stays the default so v3's kernel goldens do not move.

### Timers: `subscriptions(state)`, not a `Cmd`

The loop is Elm-shaped and `update` returns nothing, so timers are declared by a pure function of the
state, `subscriptions(state) -> List<Sub<Msg>>`, which the loop asks after every turn:

- `core.every(ms, msg)`: repeating. Fires when the period elapsed, at most once per frame; a frame
  that finds it several periods late fires it once and the next due time is the first period
  boundary after now (missed periods are skipped, not replayed).
- `core.after(ms, msg)`: one shot, `ms` after it first appears.

A `Sub` carries its message by value and the loop moves it out when it fires; because the list is
rebuilt from the state each time every firing gets a fresh message, so `M` needs no `clone`. A
timer's identity is its position in the list, its kind and its period: same on the next turn means
the same timer (its schedule is kept), anything else is a new one that starts at that turn's clock.
A one-shot that fired stays fired while it stays in the list at that position, and re-arms when the
list changes shape (the usual case: `update` clears the state that put it there).

The existing entry points keep their signatures (`core.run`, `core.frame_step`, `frame_step_into`,
`soft_backend.frame`, `frame_budgeted`); the timed ones are new: `core.frame_step_full` and
`soft_backend.frame_timed(backend, platform, tree, state, view, update, subs, timers)` with a
caller-owned `core.Timers` table. `core.next_wake_us(&timers)` is when the next armed timer is due
(-1: none), the time an OS that went to sleep wakes at.

### Animation

`examples/polyp/anim.kp`: `Anim(from, to, ms, ease)` built by `anim.lerp(from, to, ms, easing)`,
easings `Linear`, `EaseIn` (t^2), `EaseOut` (1-(1-t)^2), `EaseInOut`, all in permille with `i64`
intermediates; `anim.none()` is the absence of one. An `Anim` is data with no clock: the soft
backend starts it the first time it sees that exact value on that widget and reads the `Tick` clock
from then on. An unchanged `Anim` on the next view keeps running, a different one restarts at the
current clock; a finished animation keeps showing `to`.

Builder options (they apply to the next child, like `margin`):

| option | animates / sets |
| --- | --- |
| `k.anim(a)` | the widget's own value: a `Progress`'s permille, a `Spacer`'s height (the node's plain value is what it shows without one, and what a backend that ignores animation shows) |
| `k.opacity(permille)`, `k.opacity_anim(a)` | opacity, 1000 = opaque |
| `k.offset(x, y)`, `k.offset_x_anim(a)`, `k.offset_y_anim(a)` | a paint translation of the widget and its descendants; layout is untouched |

`Opts` and `Props` carry the new fields, so the diff updates them like any other property.

The soft backend advances running channels once per frame, after the loop turn
(`advance_anims`): a read-only scan finds the paths down to widgets that are animating and only
those are walked, only the rectangle of a widget whose value changed is damaged (old and new place
for an offset), and a spacer's height change marks layout dirty so what is below it moves with the
normal arrange damage. `wants_frames()` is true while an animation is running or input/damage is
pending, and false at rest and in the frame that finishes the last animation: that is the whole
point, an OS loop is `if backend.wants_frames() { frame } else { sleep until the next event or
core.next_wake_us }`. Timers are deliberately not part of `wants_frames`.

Opacity is the blend of the widget's own colours toward the theme background (`fade`): exact for a
widget on the plain background, an approximation over something else. Offsets and opacity apply to
descendants; hit testing uses the layout rectangles, not the animated ones.

### Tests

`examples/polyp-anim-test` (pinned in `kelpc/tests/polyp_v4/anim.expected`): easings, a progress bar
0..1000 over 500 ms reaching 200/500/800/1000 at exactly 100/250/400/500 ms with one presented
rectangle equal to the bar, restart/keep, a spacer animation moving the button below it, opacity
pixels, offset damage covering both places, a repeating timer (fires at 110, 460, 500 and 1500 ms
against a 100 ms period and skips the missed periods), a one-shot that fires once and re-arms, and a
UI at rest (`wants_frames` false, nothing presented). `examples/polyp-v4-demo` is the shared
program/driver; `polyp-v4-riscv64` and `polyp-v4-x86_64` run the same files on bare metal and their
serial step lines (clock, framebuffer checksum, rectangles, `wants_frames`, timer count) equal the
hosted ones. The riscv64 image build peaks near 1.45 GB, and it grows with the size of the bump
arena: a 4 MiB arena took 3.9 GB, so the kernels stay at 1 MiB.

## Stage 2: UTF-8 text

`soft/utf8.kp` decodes by unit: `decode(s, i)` answers `(len << 32) | codepoint`. A unit is one scalar
value or one *malformed run* (a stray continuation byte or invalid lead, a lead cut short plus the
continuation bytes that did follow it, or a complete sequence that is overlong, a surrogate or above
U+10FFFF); a malformed run is one unit, one replacement box, one caret step and one backspace, is at
least one byte long, and no read goes past the end of the string. (`C0 80` is therefore two units:
`C0` is never a valid lead.) The font, the measure and the entry editor all go through it:

- `font.text_width` counts units (`"héllo"` is 5 cells, a 4-byte scalar is 1), `draw_text` decodes.
- Glyphs: ASCII; Latin-1 Supplement except the three vulgar fractions U+00BC..U+00BE; the Latin
  Extended-A letters used by Polish, Czech, Slovak, Hungarian, Turkish, Baltic and Romanian text
  (not ď ť ľ ĺ ĥ and similar, whose ascenders leave no room for a mark); Greek capitals and lowercase
  (no tonos forms); Cyrillic А-я with Ё/ё; €; – — ‘ ’ ‚ “ ” „ † ‡ • …; ← ↑ → ↓ ↔ ↕; box drawing
  ─ │ ┌ ┐ └ ┘ ├ ┤ ┬ ┴ ┼ ═ ║; ■ □ ▲ ▼ ○ ●. Everything else (all other 3- and 4-byte scalars) is the
  replacement box; `font.has_glyph(cp)` says which. The tables are generated by
  `examples/polyp/tools/font_gen.py` (accents composed onto the ASCII letters, an accented capital is the
  5-row squash of the plain one under a 2-row mark; the rest drawn there) and written into `font.kp`
  as hex-coded columns, same layout as the ASCII table. 343 non-ASCII glyphs.
- Entry: the caret is still a byte offset but always on a unit boundary. Left/Right/Home/End,
  Backspace and Delete move or remove one unit; a caret stranded inside a sequence by a programmatic
  text change snaps to the sequence's start at the next edit; the caret is drawn
  `utf8.count_before(text, caret)` cells from the left; `Char(cp)` with an invalid scalar inserts
  U+FFFD. `caret_of` stays bytes, `caret_chars_of` is the code point index.
- There is no text wrapping in the soft backend (every text is one line), so measure and caret are
  the only layout that needed to learn code points.

Box-drawing glyphs are 5 columns wide with the 6th (pen) column blank, so a horizontal line is dashed.
Test: `examples/polyp-text-test`, pinned in `kelpc/tests/polyp_v4/text.expected`, identical on both
backends. The v3 goldens (ASCII only) did not move.
