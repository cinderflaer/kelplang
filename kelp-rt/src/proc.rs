//! Port of kelp_rt.c:398-452 (the child-process table) and kelp_rt.c:179-180
//! (`kelp_argc`, `kelp_argv`, written by the emitted `main`).
//!
//! `kelp_argc` and `kelp_argv` are exported data symbols, so the IR and the C
//! prelude can keep reading them by name.
use core::ffi::{c_char, c_int};
use core::ptr;

#[unsafe(no_mangle)]
pub static mut kelp_argc: c_int = 0;

#[unsafe(no_mangle)]
pub static mut kelp_argv: *mut *mut c_char = ptr::null_mut();

/// The children `process.spawn` started and nobody has reaped. Guarded by
/// `CHILD_MUTEX`; a pid leaves the table when it is reaped.
static mut CHILD_MUTEX: libc::pthread_mutex_t = libc::PTHREAD_MUTEX_INITIALIZER;
static mut CHILD_PIDS: *mut c_int = ptr::null_mut();
static mut CHILD_LEN: usize = 0;
static mut CHILD_CAP: usize = 0;

fn child_lock() {
    unsafe {
        libc::pthread_mutex_lock(&raw mut CHILD_MUTEX);
    }
}

fn child_unlock() {
    unsafe {
        libc::pthread_mutex_unlock(&raw mut CHILD_MUTEX);
    }
}

/// Records a spawned child's pid. On allocation failure the pid is dropped,
/// as in the C text.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_child_add(pid: c_int) {
    child_lock();
    unsafe {
        if CHILD_LEN == CHILD_CAP {
            let cap = if CHILD_CAP == 0 { 16 } else { CHILD_CAP.wrapping_mul(2) };
            let grown = libc::realloc(CHILD_PIDS.cast(), cap.wrapping_mul(core::mem::size_of::<c_int>()))
                as *mut c_int;
            if grown.is_null() {
                child_unlock();
                return;
            }
            CHILD_PIDS = grown;
            CHILD_CAP = cap;
        }
        *CHILD_PIDS.add(CHILD_LEN) = pid;
        CHILD_LEN += 1;
    }
    child_unlock();
}

/// Whether `pid` is in the child table.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_child_has(pid: c_int) -> bool {
    let mut found = false;
    child_lock();
    unsafe {
        for i in 0..CHILD_LEN {
            if *CHILD_PIDS.add(i) == pid {
                found = true;
                break;
            }
        }
    }
    child_unlock();
    found
}

/// Removes `pid` from the child table, if present (swap with the last entry).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_child_remove(pid: c_int) {
    child_lock();
    unsafe {
        for i in 0..CHILD_LEN {
            if *CHILD_PIDS.add(i) == pid {
                CHILD_LEN -= 1;
                *CHILD_PIDS.add(i) = *CHILD_PIDS.add(CHILD_LEN);
                break;
            }
        }
    }
    child_unlock();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_add_then_has() {
        unsafe {
            kelp_child_add(1_000_001);
            kelp_child_add(1_000_002);
            assert!(kelp_child_has(1_000_001));
            assert!(kelp_child_has(1_000_002));
            assert!(!kelp_child_has(1_000_003));
            kelp_child_remove(1_000_001);
            kelp_child_remove(1_000_002);
        }
    }

    #[test]
    fn child_remove_drops_only_that_pid() {
        unsafe {
            kelp_child_add(2_000_001);
            kelp_child_add(2_000_002);
            kelp_child_add(2_000_003);
            kelp_child_remove(2_000_002);
            assert!(!kelp_child_has(2_000_002));
            assert!(kelp_child_has(2_000_001));
            assert!(kelp_child_has(2_000_003));
            kelp_child_remove(9_999_999);
            assert!(kelp_child_has(2_000_001));
            kelp_child_remove(2_000_001);
            kelp_child_remove(2_000_003);
        }
    }

    #[test]
    fn child_table_grows_past_initial_capacity() {
        unsafe {
            let base = 3_000_000;
            for i in 0..40 {
                kelp_child_add(base + i);
            }
            for i in 0..40 {
                assert!(kelp_child_has(base + i), "pid {} lost after growth", base + i);
            }
            for i in 0..40 {
                kelp_child_remove(base + i);
            }
            for i in 0..40 {
                assert!(!kelp_child_has(base + i));
            }
        }
    }
}
