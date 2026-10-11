//! `std/thread`: the sleep and the id of the calling thread.

#[unsafe(no_mangle)]
pub extern "C" fn kelp_std__thread__sleep_ms_raw(ms: u64) {
    super::time::sleep_ms(ms);
}

#[unsafe(no_mangle)]
pub extern "C" fn kelp_std__thread__current_id_raw() -> u64 {
    unsafe { libc::pthread_self() as u64 }
}
