//! Port of kelp_rt.c:263-389 (the environment read cache) and
//! kelp_rt.c:455-471 (`kelp_env_lookup`).
//!
//! The table is open addressing over pointers into `environ`'s own strings.
//! A lookup is lock-free: it registers in one of two reader counters chosen
//! by the parity of the epoch, and a rebuild frees the table it replaces only
//! after the counter of the old parity drains. Builds and every mutation of
//! `environ` happen under `ENV_MUTEX`.
use core::ffi::{c_char, c_int, c_void};
use core::ptr;
use core::sync::atomic::{AtomicIsize, AtomicPtr, AtomicUsize, Ordering};

use crate::panic::kelp_panic;

unsafe extern "C" {
    static mut environ: *mut *mut c_char;
}

#[repr(C)]
struct EnvTable {
    slots: *mut *const c_char,
    cap: usize,
}

static ENV_TABLE: AtomicPtr<EnvTable> = AtomicPtr::new(ptr::null_mut());
static ENV_EPOCH: AtomicUsize = AtomicUsize::new(0);
static ENV_READERS_EVEN: AtomicIsize = AtomicIsize::new(0);
static ENV_READERS_ODD: AtomicIsize = AtomicIsize::new(0);
static mut ENV_MUTEX: libc::pthread_mutex_t = libc::PTHREAD_MUTEX_INITIALIZER;

/// The reader counter for `parity` (the low bit of the epoch).
fn readers(parity: usize) -> &'static AtomicIsize {
    if parity & 1 == 0 {
        &ENV_READERS_EVEN
    } else {
        &ENV_READERS_ODD
    }
}

fn env_lock() {
    unsafe {
        libc::pthread_mutex_lock(&raw mut ENV_MUTEX);
    }
}

fn env_unlock() {
    unsafe {
        libc::pthread_mutex_unlock(&raw mut ENV_MUTEX);
    }
}

/// FNV-1a over `len` bytes, the same hash as the C text.
fn kelp_env_hash(data: *const c_char, len: usize) -> u64 {
    let mut h: u64 = 14695981039346656037;
    for i in 0..len {
        let b = unsafe { *data.add(i) } as u8;
        h ^= b as u64;
        h = h.wrapping_mul(1099511628211);
    }
    h
}

/// One scan of `environ`, building a table at about twice its entry count.
/// Callers hold `ENV_MUTEX`.
unsafe fn kelp_env_build() -> *mut EnvTable {
    unsafe {
        let mut n: usize = 0;
        while !(*environ.add(n)).is_null() {
            n += 1;
        }
        let mut cap: usize = 8;
        while cap < n.wrapping_mul(2).wrapping_add(1) {
            cap = cap.wrapping_mul(2);
        }
        let t = libc::malloc(core::mem::size_of::<EnvTable>()) as *mut EnvTable;
        if t.is_null() {
            kelp_panic(c"env: out of memory building the lookup table".as_ptr());
        }
        let slots = libc::calloc(cap, core::mem::size_of::<*const c_char>()) as *mut *const c_char;
        if slots.is_null() {
            kelp_panic(c"env: out of memory building the lookup table".as_ptr());
        }
        (*t).slots = slots;
        (*t).cap = cap;
        let mask = (cap as u64).wrapping_sub(1);
        for i in 0..n {
            let entry = *environ.add(i);
            let eq = libc::strchr(entry, b'=' as c_int);
            let name_len = if eq.is_null() {
                libc::strlen(entry)
            } else {
                (eq as *const c_char).offset_from(entry) as usize
            };
            let mut idx = (kelp_env_hash(entry, name_len) & mask) as usize;
            while !(*slots.add(idx)).is_null() {
                idx = (idx + 1) & (cap - 1);
            }
            *slots.add(idx) = entry;
        }
        t
    }
}

/// The published table, building the first one under the mutex if needed.
unsafe fn kelp_env_ensure() -> *mut EnvTable {
    let t = ENV_TABLE.load(Ordering::Acquire);
    if !t.is_null() {
        return t;
    }
    env_lock();
    let mut t = ENV_TABLE.load(Ordering::Relaxed);
    if t.is_null() {
        t = unsafe { kelp_env_build() };
        ENV_TABLE.store(t, Ordering::Release);
    }
    env_unlock();
    t
}

/// Rebuilds and publishes the table, then frees the one it replaced once no
/// lookup registered under its parity is still probing it. Callers hold
/// `ENV_MUTEX` and have just changed `environ`.
unsafe fn kelp_env_publish() {
    unsafe {
        let built = kelp_env_build();
        let old = ENV_TABLE.swap(built, Ordering::SeqCst);
        let e = ENV_EPOCH.fetch_add(1, Ordering::SeqCst);
        if old.is_null() {
            return;
        }
        while readers(e).load(Ordering::SeqCst) != 0 {
            libc::sched_yield();
        }
        libc::free((*old).slots.cast::<c_void>());
        libc::free(old.cast::<c_void>());
    }
}

/// `setenv` and the rebuild that follows it, as one critical section.
/// Returns what `setenv` returned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_env_set(name: *const c_char, value: *const c_char) -> c_int {
    env_lock();
    let rc = unsafe { libc::setenv(name, value, 1) };
    if rc == 0 {
        unsafe { kelp_env_publish() };
    }
    env_unlock();
    rc
}

/// `unsetenv` and the rebuild that follows it, as one critical section.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_env_unset(name: *const c_char) -> c_int {
    env_lock();
    let rc = unsafe { libc::unsetenv(name) };
    if rc == 0 {
        unsafe { kelp_env_publish() };
    }
    env_unlock();
    rc
}

/// Null if `name` is not set, else a pointer to its value inside `environ`.
/// Entries match up to their `=`, as `getenv` does.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_env_lookup(name: *const c_char) -> *const c_char {
    unsafe {
        let _ = kelp_env_ensure();
        let e = loop {
            let e = ENV_EPOCH.load(Ordering::SeqCst);
            readers(e).fetch_add(1, Ordering::SeqCst);
            if ENV_EPOCH.load(Ordering::SeqCst) == e {
                break e;
            }
            readers(e).fetch_sub(1, Ordering::SeqCst);
        };
        let t = ENV_TABLE.load(Ordering::SeqCst);
        let cap = (*t).cap;
        let mask = (cap as u64).wrapping_sub(1);
        let mut found: *const c_char = ptr::null();
        let name_len = libc::strlen(name);
        let mut idx = (kelp_env_hash(name, name_len) & mask) as usize;
        for _ in 0..cap {
            let entry = *(*t).slots.add(idx);
            if entry.is_null() {
                break;
            }
            if libc::strncmp(entry, name, name_len) == 0 && *entry.add(name_len) == b'=' as c_char {
                found = entry.add(name_len + 1);
                break;
            }
            idx = (idx + 1) & (cap - 1);
        }
        readers(e).fetch_sub(1, Ordering::SeqCst);
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::{CStr, CString};

    fn cs(s: &str) -> CString {
        CString::new(s).expect("no interior nul")
    }

    /// The value `kelp_env_lookup` returns for `name`, as an owned string.
    fn lookup(name: &str) -> Option<String> {
        let n = cs(name);
        let p = unsafe { kelp_env_lookup(n.as_ptr()) };
        if p.is_null() {
            None
        } else {
            Some(unsafe { CStr::from_ptr(p) }.to_str().expect("utf8").to_owned())
        }
    }

    fn getenv_str(name: &str) -> Option<String> {
        let n = cs(name);
        let p = unsafe { libc::getenv(n.as_ptr()) };
        if p.is_null() {
            None
        } else {
            Some(unsafe { CStr::from_ptr(p) }.to_str().expect("utf8").to_owned())
        }
    }

    #[test]
    fn env_hash_matches_fnv1a_vectors() {
        assert_eq!(kelp_env_hash(c"".as_ptr(), 0), 0xcbf29ce484222325);
        assert_eq!(kelp_env_hash(c"a".as_ptr(), 1), 0xaf63dc4c8601ec8c);
    }

    #[test]
    fn env_set_then_lookup_agrees_with_getenv() {
        let n = cs("KELP_RT_TEST_SET_A");
        let v = cs("first");
        assert_eq!(unsafe { kelp_env_set(n.as_ptr(), v.as_ptr()) }, 0);
        assert_eq!(lookup("KELP_RT_TEST_SET_A").as_deref(), Some("first"));
        assert_eq!(getenv_str("KELP_RT_TEST_SET_A").as_deref(), Some("first"));
    }

    #[test]
    fn env_set_overwrites_existing_value() {
        let n = cs("KELP_RT_TEST_OVER");
        unsafe {
            kelp_env_set(n.as_ptr(), cs("one").as_ptr());
            kelp_env_set(n.as_ptr(), cs("two").as_ptr());
        }
        assert_eq!(lookup("KELP_RT_TEST_OVER").as_deref(), Some("two"));
        assert_eq!(getenv_str("KELP_RT_TEST_OVER").as_deref(), Some("two"));
    }

    #[test]
    fn env_unset_removes_name() {
        let n = cs("KELP_RT_TEST_UNSET");
        unsafe { kelp_env_set(n.as_ptr(), cs("gone").as_ptr()) };
        assert_eq!(unsafe { kelp_env_unset(n.as_ptr()) }, 0);
        assert_eq!(lookup("KELP_RT_TEST_UNSET"), None);
        assert_eq!(getenv_str("KELP_RT_TEST_UNSET"), None);
    }

    #[test]
    fn env_lookup_unset_name_is_null() {
        assert_eq!(lookup("KELP_RT_TEST_NEVER_SET_XYZ"), None);
    }

    #[test]
    fn env_lookup_does_not_match_a_longer_name() {
        let n = cs("KELP_RT_TEST_PFXL");
        unsafe { kelp_env_set(n.as_ptr(), cs("long").as_ptr()) };
        assert_eq!(lookup("KELP_RT_TEST_PFX"), None);
        assert_eq!(lookup("KELP_RT_TEST_PFXL").as_deref(), Some("long"));
        unsafe { kelp_env_unset(n.as_ptr()) };
    }

    #[test]
    fn env_set_rejects_invalid_name_with_setenv_result() {
        let bad = cs("KELP=RT");
        assert_eq!(unsafe { kelp_env_set(bad.as_ptr(), cs("x").as_ptr()) }, -1);
    }
}
