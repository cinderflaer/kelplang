//! Per-phase wall times for `kelpc build --timings` (roadmap A-14 slice 2).
//!
//! One build collects them into one [`Timings`] and prints them to stderr at
//! the end. The phases are the ones a build actually has: `resolve::load` is
//! split into its parse half (loading, lexing and parsing every module) and its
//! resolve half (merging the modules and qualifying their names), and the
//! backend records the codegen half (lowering each unit to IR), the unit
//! compilation half (LLVM's pass pipeline and the object write) and the link.
//!
//! The per-unit list is what makes the object cache observable: a unit built
//! from an unchanged source is a `hit` and its milliseconds are the module
//! lowering that produced the key, while a changed unit is a `miss`.

use std::time::Duration;

/// One compilation unit's total wall time and whether its object came from the
/// cache.
pub struct UnitTiming {
    pub name: String,
    pub hit: bool,
    pub elapsed: Duration,
}

/// The wall times of one build.
#[derive(Default)]
pub struct Timings {
    /// Loading, lexing and parsing every module.
    pub parse: Duration,
    /// Merging the modules and qualifying their names.
    pub resolve: Duration,
    /// The type check.
    pub infer: Duration,
    /// The move and borrow check.
    pub borrowck: Duration,
    /// Lowering modules to LLVM IR (the whole backend's work when the per-unit
    /// split is off).
    pub codegen: Duration,
    /// The pass pipeline and the object write, summed over the units.
    pub unit_compile: Duration,
    /// The `cc` link.
    pub link: Duration,
    /// Per-unit times, in the units' own order.
    pub units: Vec<UnitTiming>,
}

impl Timings {
    pub fn new() -> Timings {
        Timings::default()
    }

    /// Print the phases, then one line per unit, to stderr. The unit lines
    /// carry `hit` or `miss`, which is how a caller sees the cache's decision
    /// per unit.
    pub fn print(&self) {
        eprintln!("kelpc timings:");
        let phases = [
            ("parse", self.parse),
            ("resolve", self.resolve),
            ("infer", self.infer),
            ("borrowck", self.borrowck),
            ("codegen", self.codegen),
            ("unit compile", self.unit_compile),
            ("link", self.link),
        ];
        for (label, elapsed) in phases {
            eprintln!("  {label}: {}", millis(elapsed));
        }
        for unit in &self.units {
            let verdict = if unit.hit { "hit" } else { "miss" };
            eprintln!("  unit {}: {} {verdict}", unit.name, millis(unit.elapsed));
        }
    }
}

/// One duration as `N.N ms`.
fn millis(elapsed: Duration) -> String {
    format!("{:.1} ms", elapsed.as_secs_f64() * 1000.0)
}
