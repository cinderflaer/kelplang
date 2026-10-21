//! `std/os`: target facts, process and machine facts, environment variables,
//! and the temporary directories and files that `TempDir` and `TempFile` own.
//!
//! The temporary paths are registered here when they are made and removed
//! here when their owner is dropped. A panic runs no destructor, so the
//! registry is also drained at process exit (`atexit`, which `exit(3)` and the
//! panic path both run) and, for a worker thread that panics, at the worker's
//! end. Each entry records the process and thread that made it: a forked child
//! that leaves with `exit` never removes its parent's directories.

use core::ffi::{c_char, c_int};
use core::ptr::{addr_of_mut, null_mut};

use crate::abi::String as KString;
use crate::panic::kelp_panic_errno;
use crate::text::{kelp_String__empty, kelp_String__from_str};

const PATH_BUF: usize = libc::PATH_MAX as usize;

/// One registered temporary path. The list is kept in malloc'd nodes, so the
/// runtime needs no allocator of its own.
#[repr(C)]
struct Entry {
    path: *mut c_char,
    pid: libc::pid_t,
    thread: libc::pthread_t,
    next: *mut Entry,
}

static mut HEAD: *mut Entry = null_mut();
static mut LOCK: libc::pthread_mutex_t = libc::PTHREAD_MUTEX_INITIALIZER;
static mut ARMED: bool = false;

/// Takes the registry lock. Every access to `HEAD` is inside one.
unsafe fn lock() {
    unsafe { libc::pthread_mutex_lock(addr_of_mut!(LOCK)) };
}

unsafe fn unlock() {
    unsafe { libc::pthread_mutex_unlock(addr_of_mut!(LOCK)) };
}

/// Adds `path` to the registry, owned by this process and thread, and arms the
/// exit hook the first time.
unsafe fn register(path: *const c_char) {
    unsafe {
        let node = libc::malloc(core::mem::size_of::<Entry>()) as *mut Entry;
        let copy = libc::strdup(path);
        if node.is_null() || copy.is_null() {
            kelp_panic_errno(c"os: cannot record a temporary path".as_ptr());
        }
        lock();
        node.write(Entry {
            path: copy,
            pid: libc::getpid(),
            thread: libc::pthread_self(),
            next: *addr_of_mut!(HEAD),
        });
        *addr_of_mut!(HEAD) = node;
        if !*addr_of_mut!(ARMED) {
            *addr_of_mut!(ARMED) = true;
            libc::atexit(cleanup_at_exit);
        }
        unlock();
    }
}

/// Removes the registered paths made by this process, or, when `thread_only`
/// is set, by the calling thread alone.
unsafe fn drain(thread_only: bool) {
    unsafe {
        let pid = libc::getpid();
        let me = libc::pthread_self();
        lock();
        let mut link = addr_of_mut!(HEAD);
        while !(*link).is_null() {
            let node = *link;
            let mine = (*node).pid == pid && (!thread_only || libc::pthread_equal((*node).thread, me) != 0);
            if mine {
                remove_tree((*node).path);
                *link = (*node).next;
                libc::free((*node).path.cast());
                libc::free(node.cast());
            } else {
                link = addr_of_mut!((*node).next);
            }
        }
        unlock();
    }
}

extern "C" fn cleanup_at_exit() {
    unsafe { drain(false) }
}

/// Called by the panic path of a worker thread, which ends the thread and not
/// the process: the thread's own temporary paths go, the others stay.
///
/// # Safety
/// Called on the thread that is ending.
pub(crate) unsafe fn cleanup_thread() {
    unsafe { drain(true) }
}

/// Removes `path` and everything under it. A symbolic link is removed, never
/// followed. Best effort: an entry that cannot be removed is left behind, and a
/// drop has no way to report it.
unsafe fn remove_tree(path: *const c_char) {
    unsafe {
        let mut st: libc::stat = core::mem::zeroed();
        if libc::lstat(path, &mut st) != 0 {
            return;
        }
        if st.st_mode & libc::S_IFMT != libc::S_IFDIR {
            libc::unlink(path);
            return;
        }
        let dir = libc::opendir(path);
        if !dir.is_null() {
            let mut child = [0 as c_char; PATH_BUF];
            loop {
                let ent = libc::readdir(dir);
                if ent.is_null() {
                    break;
                }
                let name = (*ent).d_name.as_ptr();
                if libc::strcmp(name, c".".as_ptr()) == 0 || libc::strcmp(name, c"..".as_ptr()) == 0 {
                    continue;
                }
                let need = libc::strlen(path) + 1 + libc::strlen(name) + 1;
                if need > PATH_BUF {
                    continue;
                }
                libc::snprintf(child.as_mut_ptr(), PATH_BUF, c"%s/%s".as_ptr(), path, name);
                remove_tree(child.as_ptr());
            }
            libc::closedir(dir);
        }
        libc::rmdir(path);
    }
}

/// The NUL-terminated copy of a Kelp string, for a call that writes into its
/// argument. `None` when it does not fit.
unsafe fn copy_into(buf: &mut [c_char; PATH_BUF], text: *const c_char) -> bool {
    unsafe {
        if libc::strlen(text) >= PATH_BUF {
            return false;
        }
        libc::strcpy(buf.as_mut_ptr(), text);
        true
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn kelp_std__os__os() -> *const c_char {
    #[cfg(target_os = "linux")]
    return c"linux".as_ptr();
    #[cfg(target_os = "macos")]
    return c"macos".as_ptr();
    #[cfg(target_os = "windows")]
    return c"windows".as_ptr();
    #[cfg(target_os = "freebsd")]
    return c"freebsd".as_ptr();
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows", target_os = "freebsd")))]
    return c"unknown".as_ptr();
}

#[unsafe(no_mangle)]
pub extern "C" fn kelp_std__os__arch() -> *const c_char {
    #[cfg(target_arch = "x86_64")]
    return c"x86_64".as_ptr();
    #[cfg(target_arch = "aarch64")]
    return c"aarch64".as_ptr();
    #[cfg(all(target_arch = "riscv64", target_pointer_width = "64"))]
    return c"riscv64".as_ptr();
    #[cfg(target_arch = "arm")]
    return c"arm".as_ptr();
    #[cfg(target_arch = "x86")]
    return c"x86".as_ptr();
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64", target_arch = "riscv64", target_arch = "arm", target_arch = "x86")))]
    return c"unknown".as_ptr();
}

#[unsafe(no_mangle)]
pub extern "C" fn kelp_std__os__pid() -> i64 {
    unsafe { libc::getpid() as i64 }
}

#[unsafe(no_mangle)]
pub extern "C" fn kelp_std__os__parent_pid() -> i64 {
    unsafe { libc::getppid() as i64 }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__os__exe_path() -> KString {
    unsafe {
        let mut buf = [0 as c_char; PATH_BUF];
        let n = libc::readlink(c"/proc/self/exe".as_ptr(), buf.as_mut_ptr(), PATH_BUF - 1);
        if n < 0 {
            kelp_panic_errno(c"os.exe_path: cannot read /proc/self/exe".as_ptr());
        }
        buf[n as usize] = 0;
        kelp_String__from_str(buf.as_ptr())
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__os__current_dir() -> KString {
    unsafe {
        let mut buf = [0 as c_char; PATH_BUF];
        if libc::getcwd(buf.as_mut_ptr(), PATH_BUF).is_null() {
            kelp_panic_errno(c"os.current_dir: cannot read the working directory".as_ptr());
        }
        kelp_String__from_str(buf.as_ptr())
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__os__set_current_dir(path: *const c_char) {
    unsafe {
        if libc::chdir(path) != 0 {
            kelp_panic_errno(c"os.set_current_dir: cannot change directory".as_ptr());
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn kelp_std__os__exit(code: i32) {
    unsafe { libc::exit(code) }
}

#[unsafe(no_mangle)]
pub extern "C" fn kelp_std__os__cpu_count() -> usize {
    let n = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_ONLN) };
    if n < 1 { 1 } else { n as usize }
}

#[unsafe(no_mangle)]
pub extern "C" fn kelp_std__os__total_memory() -> u64 {
    let pages = unsafe { libc::sysconf(libc::_SC_PHYS_PAGES) };
    let size = unsafe { libc::sysconf(libc::_SC_PAGE_SIZE) };
    if pages < 1 || size < 1 {
        return 0;
    }
    (pages as u64).wrapping_mul(size as u64)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__os__has_raw(name: *const c_char) -> bool {
    unsafe { !crate::env::kelp_env_lookup(name).is_null() }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__os__get_raw(name: *const c_char) -> KString {
    unsafe {
        let v = crate::env::kelp_env_lookup(name);
        if v.is_null() {
            return kelp_String__empty();
        }
        kelp_String__from_str(v)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__os__set_raw(name: *const c_char, value: *const c_char) {
    unsafe {
        if crate::env::kelp_env_set(name, value) != 0 {
            kelp_panic_errno(c"os.set_env_var: cannot set variable".as_ptr());
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__os__unset_raw(name: *const c_char) {
    unsafe {
        if crate::env::kelp_env_unset(name) != 0 {
            kelp_panic_errno(c"os.unset_env_var: cannot unset variable".as_ptr());
        }
    }
}

/// The user database entry of the calling user, copied into `buf`, or `None`.
unsafe fn passwd(buf: &mut [c_char; 1024]) -> Option<libc::passwd> {
    unsafe {
        let mut pw: libc::passwd = core::mem::zeroed();
        let mut out: *mut libc::passwd = null_mut();
        let rc: c_int = libc::getpwuid_r(libc::getuid(), &mut pw, buf.as_mut_ptr(), buf.len(), &mut out);
        if rc != 0 || out.is_null() {
            None
        } else {
            Some(pw)
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__os__hostname() -> KString {
    unsafe {
        let mut buf = [0 as c_char; 256];
        if libc::gethostname(buf.as_mut_ptr(), buf.len()) != 0 {
            return kelp_String__empty();
        }
        buf[buf.len() - 1] = 0;
        kelp_String__from_str(buf.as_ptr())
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__os__user_name() -> KString {
    unsafe {
        let mut buf = [0 as c_char; 1024];
        match passwd(&mut buf) {
            Some(pw) => kelp_String__from_str(pw.pw_name),
            None => kelp_String__empty(),
        }
    }
}

/// `$HOME`, or the home directory of the user database when it is unset, or
/// the empty string when neither is known.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__os__home_dir_raw() -> KString {
    unsafe {
        let v = crate::env::kelp_env_lookup(c"HOME".as_ptr());
        if !v.is_null() && *v != 0 {
            return kelp_String__from_str(v);
        }
        let mut buf = [0 as c_char; 1024];
        match passwd(&mut buf) {
            Some(pw) => kelp_String__from_str(pw.pw_dir),
            None => kelp_String__empty(),
        }
    }
}

/// `$TMPDIR`, or `/tmp` when it is unset or empty.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__os__temp_dir() -> KString {
    unsafe {
        let v = crate::env::kelp_env_lookup(c"TMPDIR".as_ptr());
        if !v.is_null() && *v != 0 {
            return kelp_String__from_str(v);
        }
        kelp_String__from_str(c"/tmp".as_ptr())
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn kelp_std__os__uid() -> u32 {
    unsafe { libc::getuid() }
}

#[unsafe(no_mangle)]
pub extern "C" fn kelp_std__os__gid() -> u32 {
    unsafe { libc::getgid() }
}

/// Makes a fresh directory from `template` (`<dir>/<prefix>XXXXXX`), registers
/// it for removal, and returns its path. The empty string means `mkdtemp`
/// failed; the caller turns that into a value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__os__mkdtemp_raw(template: *const c_char) -> KString {
    unsafe {
        let mut buf = [0 as c_char; PATH_BUF];
        if !copy_into(&mut buf, template) {
            return kelp_String__empty();
        }
        if libc::mkdtemp(buf.as_mut_ptr()).is_null() {
            return kelp_String__empty();
        }
        register(buf.as_ptr());
        kelp_String__from_str(buf.as_ptr())
    }
}

/// As `mkdtemp_raw`, for a fresh empty file.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__os__mkstemp_raw(template: *const c_char) -> KString {
    unsafe {
        let mut buf = [0 as c_char; PATH_BUF];
        if !copy_into(&mut buf, template) {
            return kelp_String__empty();
        }
        let fd = libc::mkstemp(buf.as_mut_ptr());
        if fd < 0 {
            return kelp_String__empty();
        }
        libc::close(fd);
        register(buf.as_ptr());
        kelp_String__from_str(buf.as_ptr())
    }
}

/// Removes `path` and everything under it, when this process registered it
/// (`mkdtemp_raw` or `mkstemp_raw`). A path it did not make is left alone: a
/// `TempDir` value can be built from any string, and a drop must not remove a
/// tree the runtime never created. Called by the drop of `TempDir` and
/// `TempFile`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_std__os__remove_tree_raw(path: *const c_char) {
    unsafe {
        lock();
        let mut link = addr_of_mut!(HEAD);
        while !(*link).is_null() {
            let node = *link;
            if libc::strcmp((*node).path, path) == 0 {
                remove_tree((*node).path);
                *link = (*node).next;
                libc::free((*node).path.cast());
                libc::free(node.cast());
                break;
            }
            link = addr_of_mut!((*node).next);
        }
        unlock();
    }
}
