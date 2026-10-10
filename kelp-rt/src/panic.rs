//! The panic handler for the `no_std` static library.
//!
//! A Rust panic in the runtime is a runtime bug, so it must not look like a Kelp
//! panic. The handler writes one fixed line to stderr and aborts (exit status
//! 134). Kelp panics never use Rust panics; the `kelp_panic` family arrives in a
//! later work package.

#[cfg(all(not(test), feature = "panic-handler"))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo<'_>) -> ! {
    const MSG: &[u8] = b"kelp-rt: internal error: the runtime panicked\n";
    // SAFETY: writes a constant buffer to stderr; the result is ignored because
    // the process is ending either way.
    unsafe {
        libc::write(2, MSG.as_ptr() as *const libc::c_void, MSG.len());
        libc::abort()
    }
}
