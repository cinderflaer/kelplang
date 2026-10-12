//! Tidepool trigger arithmetic (design section 4, R36). Pure functions in
//! bytes, so the rules can be tested without a heap.
//!
//! The first clearing fires at the midpoint of the two start guesses plus
//! `headroom_pct` percent. Later clearings fire at the larger of `heap_min`
//! and the larger of the last two measured live sizes plus `headroom_pct`
//! percent. Integer division rounds down. The arithmetic is done in 128 bits
//! and clamped to `usize`, so a large manifest value cannot overflow.

/// Scale `value` by `(100 + headroom_pct) / 100`, rounding down, in 128 bits.
fn with_headroom(value: u128, headroom_pct: usize) -> u128 {
    value * (100 + headroom_pct as u128) / 100
}

/// Clamp a 128-bit byte count to `usize`.
fn clamp(value: u128) -> usize {
    if value > usize::MAX as u128 {
        usize::MAX
    } else {
        value as usize
    }
}

/// The first trigger, in bytes: the midpoint of `start_low` and `start_high`
/// plus `headroom_pct` percent. `heap_min` does not set the first trigger
/// (T40c), so it is not used here.
pub fn first_trigger(heap_min: usize, start_low: usize, start_high: usize, headroom_pct: usize) -> usize {
    let _ = heap_min;
    let midpoint = (start_low as u128 + start_high as u128) / 2;
    clamp(with_headroom(midpoint, headroom_pct))
}

/// A later trigger, in bytes: `max(heap_min, max(live_earlier, live_latest) *
/// (100 + headroom_pct) / 100)`. The two arguments are the two latest
/// measured live sizes, in either order.
pub fn next_trigger(heap_min: usize, live_earlier: usize, live_latest: usize, headroom_pct: usize) -> usize {
    let live = core::cmp::max(live_earlier, live_latest) as u128;
    core::cmp::max(heap_min, clamp(with_headroom(live, headroom_pct)))
}
