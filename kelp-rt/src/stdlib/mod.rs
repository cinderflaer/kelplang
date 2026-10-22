//! The bodies of the compiler's `std/*` `extern fun` declarations, in Rust.
//!
//! Each export is named `kelp_` plus the mangled qualified name, so
//! `std.math.sqrt_raw` is `kelp_std__math__sqrt_raw`. The behaviour follows the
//! C text these bodies replaced: the same return values, the same panic
//! messages, and the same errno detail lines. Capability checks stay in the
//! Kelp and compiler layers, as before.

pub mod env;
pub mod fs;
pub mod io;
pub mod process;
pub mod fmt;
pub mod math;
pub mod net;
pub mod mem;
pub mod path;
pub mod rand;
pub mod source;
pub mod str;
pub mod os;
pub mod sync;
pub mod thread;
pub mod time;
