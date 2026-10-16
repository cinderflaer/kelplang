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
pub mod sys;
pub mod sync;
pub mod thread;
pub mod time;

/// Replaces `*reason` with a fresh Kelp string holding `text`, freeing the old
/// buffer first, as the failing `try_*` bodies do.
///
/// # Safety
/// `reason` must point to a live `String`, and `text` must be NUL-terminated.
pub(crate) unsafe fn set_reason(reason: *mut crate::abi::String, text: *const core::ffi::c_char) {
    unsafe {
        libc::free((*reason).data.cast());
        *reason = crate::text::kelp_String__from_str(text);
    }
}
