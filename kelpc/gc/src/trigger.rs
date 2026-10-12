//! Tidepool trigger arithmetic (design section 4, R36). Pure functions in
//! bytes, so the rules can be tested without a heap.
//!
//! The first clearing fires at the midpoint of the two start guesses plus
//! `headroom_pct` percent. Later clearings fire at the larger of `heap_min`
//! and a live size plus `headroom_pct` percent. Which live size is chosen is
//! the trigger rule (design section 4, Challenge 2, and M3):
//!
//! - `TwoSize` (the default, D4): the larger of the last two measured live sizes.
//! - `LastSize`: the last measured live size alone.
//! - `GogcStyle`: the last measured live size plus the stack and global bytes
//!   scanned with it, as Go's GOGC counts them.
//!
//! Integer division rounds down. The arithmetic is done in 128 bits and
//! clamped to `usize`, so a large manifest value cannot overflow.

/// The rule that sets every later trigger. The codes are the ABI value
/// `kelp_gc_init` receives, and the manifest names them by `NAME`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rule {
    TwoSize,
    LastSize,
    GogcStyle,
}

impl Rule {
    /// The ABI code of the rule: 0 is the default.
    pub const fn code(self) -> usize {
        match self {
            Rule::TwoSize => 0,
            Rule::LastSize => 1,
            Rule::GogcStyle => 2,
        }
    }

    /// The rule for an ABI code, or `None` for a code no rule has.
    pub const fn from_code(code: usize) -> Option<Rule> {
        match code {
            0 => Some(Rule::TwoSize),
            1 => Some(Rule::LastSize),
            2 => Some(Rule::GogcStyle),
            _ => None,
        }
    }
}

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

/// A later trigger, in bytes: `max(heap_min, live * (100 + headroom_pct) / 100)`,
/// where `live` is chosen by `rule`. `live_earlier` and `live_latest` are the
/// two latest measured live sizes, in either order. `extra` is the stack and
/// global bytes scanned in the cycle that measured `live_latest`; only
/// `GogcStyle` reads it.
pub fn next_trigger(
    rule: Rule,
    heap_min: usize,
    live_earlier: usize,
    live_latest: usize,
    extra: usize,
    headroom_pct: usize,
) -> usize {
    let live: u128 = match rule {
        Rule::TwoSize => core::cmp::max(live_earlier, live_latest) as u128,
        Rule::LastSize => live_latest as u128,
        Rule::GogcStyle => live_latest as u128 + extra as u128,
    };
    core::cmp::max(heap_min, clamp(with_headroom(live, headroom_pct)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: usize = 1 << 20;
    const HEAP_MIN: usize = 8 * MIB;

    /// A fixed sequence of measured live sizes, one per clearing. The 40 MiB
    /// spike is the third measurement and has died by the fifth. `EXTRA` is
    /// the stack and global bytes of every cycle.
    const LIVE: [usize; 5] = [10 * MIB, 10 * MIB, 40 * MIB, 12 * MIB, 12 * MIB];
    const EXTRA: usize = MIB;

    /// The trigger each cycle after the first sets, for `rule` at `headroom`
    /// percent. Entry k uses the measurements of clearings k+1 and k+2.
    fn triggers(rule: Rule, headroom: usize) -> Vec<usize> {
        (1..LIVE.len())
            .map(|i| next_trigger(rule, HEAP_MIN, LIVE[i - 1], LIVE[i], EXTRA, headroom))
            .collect()
    }

    #[test]
    fn two_size_keeps_a_spike_one_clearing_longer_than_last_size() {
        // 12 MiB * 1.2, 40 MiB * 1.2, max(40, 12) * 1.2, max(12, 12) * 1.2.
        assert_eq!(triggers(Rule::TwoSize, 20), vec![12_582_912, 50_331_648, 50_331_648, 15_099_494]);
    }

    #[test]
    fn last_size_follows_the_latest_live_size_only() {
        // 12 MiB * 1.2, 40 MiB * 1.2, 12 MiB * 1.2, 12 MiB * 1.2.
        assert_eq!(triggers(Rule::LastSize, 20), vec![12_582_912, 50_331_648, 15_099_494, 15_099_494]);
    }

    #[test]
    fn gogc_style_adds_the_stack_and_global_bytes_to_the_latest_live_size() {
        // (live + 1 MiB) * 1.2 for each live size.
        assert_eq!(triggers(Rule::GogcStyle, 20), vec![13_841_203, 51_589_939, 16_357_785, 16_357_785]);
    }

    #[test]
    fn the_rules_differ_on_the_same_measurements() {
        let two = triggers(Rule::TwoSize, 20);
        let last = triggers(Rule::LastSize, 20);
        let gogc = triggers(Rule::GogcStyle, 20);
        assert_ne!(two, last);
        assert_ne!(last, gogc);
        assert_ne!(two, gogc);
    }

    #[test]
    fn heap_min_is_the_floor_for_every_rule() {
        for rule in [Rule::TwoSize, Rule::LastSize, Rule::GogcStyle] {
            assert_eq!(next_trigger(rule, HEAP_MIN, MIB, MIB, 0, 0), HEAP_MIN);
        }
    }

    #[test]
    fn zero_headroom_gives_the_live_size_plus_any_extra() {
        assert_eq!(next_trigger(Rule::LastSize, HEAP_MIN, 10 * MIB, 12 * MIB, EXTRA, 0), 12 * MIB);
        assert_eq!(next_trigger(Rule::GogcStyle, HEAP_MIN, 10 * MIB, 12 * MIB, EXTRA, 0), 13 * MIB);
    }

    #[test]
    fn rule_codes_round_trip_and_unknown_codes_are_refused() {
        for rule in [Rule::TwoSize, Rule::LastSize, Rule::GogcStyle] {
            assert_eq!(Rule::from_code(rule.code()), Some(rule));
        }
        assert_eq!(Rule::TwoSize.code(), 0);
        assert_eq!(Rule::from_code(3), None);
    }
}
