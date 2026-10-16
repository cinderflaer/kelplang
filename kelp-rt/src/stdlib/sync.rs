//! `std/sync` channel wakeups: the runtime half of the general `select`
//! (issue 216, plan 236, decision 743).
//!
//! A channel has a number, handed out by [`kelp_std__sync__chan_id_raw`]. A
//! select makes one waiter, registers it on the number of every channel it
//! watches, and waits on it. Every channel operation that changes state calls
//! [`kelp_std__sync__chan_wake_raw`] with that number, which sets the sticky
//! `signaled` flag of each waiter on the channel and wakes it.
//!
//! The protocol that makes this lossless: the select registers first, then
//! polls its cases, then waits. A wake that lands between the poll and the wait
//! sets the flag, and the wait returns at once. The flag is cleared only when a
//! wait consumes it.
//!
//! Lock order is the registry lock first, then a waiter's lock. A waiter is
//! freed only after it is removed from the registry, so no wake can reach it.

use core::ffi::c_int;
use core::ptr;

/// One select's waiter: a pthread mutex and condition variable (on the
/// monotonic clock) and the sticky wake flag.
#[repr(C)]
pub struct Waiter {
    lock: libc::pthread_mutex_t,
    cond: libc::pthread_cond_t,
    signaled: c_int,
}

/// One registration: waiter `waiter` watches channel `chan`.
#[repr(C)]
struct Link {
    chan: u64,
    waiter: *mut Waiter,
    next: *mut Link,
}

static mut REG_LOCK: libc::pthread_mutex_t = libc::PTHREAD_MUTEX_INITIALIZER;
static mut REG_HEAD: *mut Link = ptr::null_mut();
static mut NEXT_CHAN: u64 = 1;

/// Milliseconds on the monotonic clock.
unsafe fn now_ms() -> i64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &raw mut ts) };
    (ts.tv_sec as i64) * 1000 + (ts.tv_nsec as i64) / 1_000_000
}

/// A fresh channel number, never zero.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__sync__chan_id_raw() -> u64 {
    unsafe {
        libc::pthread_mutex_lock(&raw mut REG_LOCK);
        let id = NEXT_CHAN;
        NEXT_CHAN += 1;
        libc::pthread_mutex_unlock(&raw mut REG_LOCK);
        id
    }
}

/// Milliseconds on the monotonic clock, for the deadlines `waiter_wait_raw` takes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__sync__chan_now_ms_raw() -> i64 {
    unsafe { now_ms() }
}

/// A new waiter handle, not yet watching any channel. Free it with
/// `waiter_free_raw` once it has been unwatched.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__sync__waiter_new_raw() -> u64 {
    unsafe {
        let w = libc::malloc(core::mem::size_of::<Waiter>()).cast::<Waiter>();
        if w.is_null() {
            crate::panic::kelp_panic(c"sync: out of memory for a select waiter".as_ptr());
        }
        libc::pthread_mutex_init(&raw mut (*w).lock, ptr::null());
        let mut attr = core::mem::MaybeUninit::<libc::pthread_condattr_t>::uninit();
        libc::pthread_condattr_init(attr.as_mut_ptr());
        libc::pthread_condattr_setclock(attr.as_mut_ptr(), libc::CLOCK_MONOTONIC);
        libc::pthread_cond_init(&raw mut (*w).cond, attr.as_ptr());
        libc::pthread_condattr_destroy(attr.as_mut_ptr());
        (*w).signaled = 0;
        w as usize as u64
    }
}

/// Removes every registration of `w` and frees it. The caller must not use `w` again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__sync__waiter_free_raw(w: u64) {
    unsafe {
        let w = w as usize as *mut Waiter;
        libc::pthread_mutex_lock(&raw mut REG_LOCK);
        let mut prev: *mut *mut Link = &raw mut REG_HEAD;
        while !(*prev).is_null() {
            let link = *prev;
            if (*link).waiter == w {
                *prev = (*link).next;
                libc::free(link.cast());
            } else {
                prev = &raw mut (*link).next;
            }
        }
        libc::pthread_mutex_unlock(&raw mut REG_LOCK);
        libc::pthread_cond_destroy(&raw mut (*w).cond);
        libc::pthread_mutex_destroy(&raw mut (*w).lock);
        libc::free(w.cast());
    }
}

/// Registers `w` on channel `chan`: a wake of `chan` will reach it until it is unwatched.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__sync__waiter_watch_raw(w: u64, chan: u64) {
    unsafe {
        let link = libc::malloc(core::mem::size_of::<Link>()).cast::<Link>();
        if link.is_null() {
            crate::panic::kelp_panic(c"sync: out of memory for a select registration".as_ptr());
        }
        libc::pthread_mutex_lock(&raw mut REG_LOCK);
        (*link).chan = chan;
        (*link).waiter = w as usize as *mut Waiter;
        (*link).next = REG_HEAD;
        REG_HEAD = link;
        libc::pthread_mutex_unlock(&raw mut REG_LOCK);
    }
}

/// Removes every registration of `w` on `chan`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__sync__waiter_unwatch_raw(w: u64, chan: u64) {
    unsafe {
        let w = w as usize as *mut Waiter;
        libc::pthread_mutex_lock(&raw mut REG_LOCK);
        let mut prev: *mut *mut Link = &raw mut REG_HEAD;
        while !(*prev).is_null() {
            let link = *prev;
            if (*link).waiter == w && (*link).chan == chan {
                *prev = (*link).next;
                libc::free(link.cast());
            } else {
                prev = &raw mut (*link).next;
            }
        }
        libc::pthread_mutex_unlock(&raw mut REG_LOCK);
    }
}

/// Wakes every waiter registered on `chan`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__sync__chan_wake_raw(chan: u64) {
    unsafe {
        libc::pthread_mutex_lock(&raw mut REG_LOCK);
        let mut link = REG_HEAD;
        while !link.is_null() {
            if (*link).chan == chan {
                let w = (*link).waiter;
                libc::pthread_mutex_lock(&raw mut (*w).lock);
                (*w).signaled = 1;
                libc::pthread_cond_signal(&raw mut (*w).cond);
                libc::pthread_mutex_unlock(&raw mut (*w).lock);
            }
            link = (*link).next;
        }
        libc::pthread_mutex_unlock(&raw mut REG_LOCK);
    }
}

/// Waits until `w` is woken or the monotonic deadline `deadline_ms` passes
/// (`deadline_ms < 0` waits without a limit). Answers `true` for a wake and
/// `false` for a deadline. A wake that came earlier, with nothing waiting, still
/// answers `true` at once. The wake is consumed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__sync__waiter_wait_raw(w: u64, deadline_ms: i64) -> bool {
    unsafe {
        let w = w as usize as *mut Waiter;
        libc::pthread_mutex_lock(&raw mut (*w).lock);
        while (*w).signaled == 0 {
            if deadline_ms < 0 {
                libc::pthread_cond_wait(&raw mut (*w).cond, &raw mut (*w).lock);
            } else {
                if now_ms() >= deadline_ms {
                    break;
                }
                let abs = libc::timespec {
                    tv_sec: (deadline_ms / 1000) as libc::time_t,
                    tv_nsec: ((deadline_ms % 1000) * 1_000_000) as libc::c_long,
                };
                libc::pthread_cond_timedwait(&raw mut (*w).cond, &raw mut (*w).lock, &abs);
            }
        }
        let woken = (*w).signaled != 0;
        (*w).signaled = 0;
        libc::pthread_mutex_unlock(&raw mut (*w).lock);
        woken
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn wake_before_wait_is_not_lost() {
        unsafe {
            let c = kelp_std__sync__chan_id_raw();
            let w = kelp_std__sync__waiter_new_raw();
            kelp_std__sync__waiter_watch_raw(w, c);
            kelp_std__sync__chan_wake_raw(c);
            let far = kelp_std__sync__chan_now_ms_raw() + 10_000;
            assert!(kelp_std__sync__waiter_wait_raw(w, far));
            kelp_std__sync__waiter_unwatch_raw(w, c);
            kelp_std__sync__waiter_free_raw(w);
        }
    }

    #[test]
    fn deadline_passes_without_wake() {
        unsafe {
            let c = kelp_std__sync__chan_id_raw();
            let w = kelp_std__sync__waiter_new_raw();
            kelp_std__sync__waiter_watch_raw(w, c);
            let start = kelp_std__sync__chan_now_ms_raw();
            assert!(!kelp_std__sync__waiter_wait_raw(w, start + 30));
            assert!(kelp_std__sync__chan_now_ms_raw() - start >= 25);
            kelp_std__sync__waiter_unwatch_raw(w, c);
            kelp_std__sync__waiter_free_raw(w);
        }
    }

    #[test]
    fn wake_from_another_thread_ends_an_unlimited_wait() {
        unsafe {
            let c = kelp_std__sync__chan_id_raw();
            let w = kelp_std__sync__waiter_new_raw();
            kelp_std__sync__waiter_watch_raw(w, c);
            let waker = thread::spawn(move || {
                thread::sleep(Duration::from_millis(20));
                unsafe { kelp_std__sync__chan_wake_raw(c) };
            });
            assert!(kelp_std__sync__waiter_wait_raw(w, -1));
            waker.join().unwrap();
            kelp_std__sync__waiter_unwatch_raw(w, c);
            kelp_std__sync__waiter_free_raw(w);
        }
    }

    #[test]
    fn unwatched_channel_does_not_wake() {
        unsafe {
            let c = kelp_std__sync__chan_id_raw();
            let w = kelp_std__sync__waiter_new_raw();
            kelp_std__sync__waiter_watch_raw(w, c);
            kelp_std__sync__waiter_unwatch_raw(w, c);
            kelp_std__sync__chan_wake_raw(c);
            let far = kelp_std__sync__chan_now_ms_raw() + 20;
            assert!(!kelp_std__sync__waiter_wait_raw(w, far));
            kelp_std__sync__waiter_free_raw(w);
        }
    }

    #[test]
    fn free_removes_registrations() {
        unsafe {
            let c = kelp_std__sync__chan_id_raw();
            let w = kelp_std__sync__waiter_new_raw();
            kelp_std__sync__waiter_watch_raw(w, c);
            kelp_std__sync__waiter_free_raw(w);
            // Must not touch the freed waiter.
            kelp_std__sync__chan_wake_raw(c);
        }
    }
}
