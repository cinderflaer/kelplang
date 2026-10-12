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
  (no tonos forms); Cyrillic А-я with Ё/ё; €; en dash and em dash, ‘ ’ ‚ “ ” „ † ‡ • …; ← ↑ → ↓ ↔ ↕; box drawing
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

## Stage 3: the accessibility tree

`soft/a11y.kp` defines `a11y.Node(id, role, name, value, state, x, y, w, h, actions, kids)`, the role,
state and action constants, and `a11y.dump(&tree)`; `SoftBackend.a11y_tree()` builds a fresh tree from
the retained widgets when asked (nothing is built in the frame path, so v3's allocation budgets are
untouched).

- Roles: button, checkbox, entry, label (`Text`), progress, list (`Scrolled`), list_item (a container
  directly inside a list), region (other `Column`/`Row`). Spacers and separators are not exposed.
- `name` is the label or text; an unlabeled checkbox takes the next sibling `Text` (the todo rows).
  `value` is the entry's text, `"true"`/`"false"` for a checkbox, the permille for a progress bar.
  `state` flags: focused, disabled, checked. `bounds` are layout rectangles (no animated offset).
  `actions`: press and focus on buttons/checkboxes, focus and set_value on entries, none when disabled.
- Ids are the backend's widget uids, stable for the life of the widget.
- `a11y_act(id, ACT_PRESS | ACT_FOCUS)` and `a11y_set_value(id, text)` drive the UI. Press focuses the
  widget and posts its message exactly as a click does (so model, counters and focus are equal; the
  hover colour a real pointer leaves is not reproduced). A missing node, a wrong action or a disabled
  node answers false and changes nothing.
- New option `k.disabled()` (a `Props`/`Opts` field): the soft backend skips the widget in hit
  testing, Tab and keyboard handling, draws it muted, and the tree reports it disabled. It applies to
  interactive widgets only (a disabled container does not disable its children). GTK ignores it.
- `a11y.dump` format: `role "name" [value="v"] state=[..] bounds=x,y,w,h actions=[..] id=N`, two spaces
  of indent per level.

Test: `examples/polyp-a11y-test` pinned in `kelpc/tests/polyp_v4/a11y.expected`, equal on both backends.

## Stage 4: memoised subtrees

`core.Memo` lets a view leave a large, pure subtree as a cheap placeholder. The application gives a
memo a sibling-unique key, a `Deps` value and a plain `fun (&Deps).return: List<Child<M>>`; the function
must return exactly one child through `core.single`. `Deps` is an ordered list of integers and
strings, built with `core.deps()` and `push_int`, `push_bool` and `push_str`. Its declared equality
is the cache contract: include every value the memo view reads. `k.memo(key, deps, build)` places a
memo in a builder, while `core.memo(deps, build)` makes one root node.

The caller owns a `MemoStore<M>` for the life of the displayed tree. First render uses
`create_all_memo(tree, store)`; later turns use `frame_step_memo` or `soft_backend.frame_memo` (or
`apply_diff_memo` for a driver that already owns its frame loop). A record is identified by its path,
with keyed path segments compared by key rather than index, so a keyed reorder is a hit. An equal
dependency value leaves the retained subtree untouched; a miss calls the view once and diffs the
replacement body. Removing a memo drops its record and every nested record. A memo nested in a
changed outer memo still hits when its own dependencies did not change.

The transparent wrapper is a `Column` in every backend. This keeps the ordinary keyed-patch and
layout rules intact, and gives a memo's options to its wrapper. Event dispatch rebuilds the memo
body only to recover the message under a posted widget; that reconstruction is not a cache miss and
is not counted as one.

`MemoStore` exposes `hits`, `misses` and `view_calls`; `core.stats()` publishes those three fields
after a memo frame. `examples/polyp-memo-test`, pinned in `kelpc/tests/polyp_v4/memo.expected`,
checks first render, one changed row, no change, dependency changes, reorder, removal and reinsertion,
nested memoisation, event dispatch, and equality of every memoised framebuffer with its non-memoised
counterpart. `examples/polyp-memo-bench` runs a 200- and 1,000-row scrolled list. On this development
host's 40-frame run, the 1,000-row memo case made 40 row-view calls and 39,960 hits, versus rebuilding
the plain view each frame; both variants wrote 118,517 pixels. Its timings are diagnostic only and
the fast test pins counters and equal pixels, not timings.

The hosted v4 demo now combines all four stages: animation and timers, UTF-8 labels, the final
accessibility dump and three memoised rows. The two freestanding v4 kernel tests compare their demo
run slice against the hosted transcript when `ld.lld` and the corresponding QEMU are available.

### What v4 still does not do

Memo dependency values are explicit data rather than automatically tracked reads; omitting a value
from `Deps` is an application bug and can leave stale pixels. The store is linear and tuned for
small, stable keyed lists, not a general-purpose cache. There is no eviction policy beyond removing
the memo from the tree, and GTK currently accepts the memo wrapper through the shared core path but
has no performance measurement attached to it.
