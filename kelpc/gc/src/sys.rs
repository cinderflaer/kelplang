//! Raw libc and pthread declarations. Linked from the final program's libc;
//! no crate dependency.

use core::ffi::{c_char, c_int, c_long, c_ulong, c_void};

pub const PROT_READ: c_int = 1;
pub const PROT_WRITE: c_int = 2;
pub const MAP_PRIVATE: c_int = 0x02;
pub const MAP_ANONYMOUS: c_int = 0x20;
pub const MAP_NORESERVE: c_int = 0x4000;
pub const CLOCK_REALTIME: c_int = 0;
pub const CLOCK_MONOTONIC: c_int = 1;
/// glibc's ETIMEDOUT on Linux.
pub const ETIMEDOUT: c_int = 110;

/// glibc `pthread_attr_t` is 56 bytes; 64 is a safe upper bound.
#[repr(C, align(16))]
pub struct PthreadAttr(pub [u64; 8]);

/// Storage for `pthread_mutex_t` (40 bytes on glibc) and `pthread_cond_t` (48).
#[repr(C, align(16))]
pub struct Opaque(pub [u64; 8]);

#[repr(C)]
pub struct Timespec {
    pub tv_sec: c_long,
    pub tv_nsec: c_long,
}

unsafe extern "C" {
    pub fn mmap(addr: *mut c_void, len: usize, prot: c_int, flags: c_int, fd: c_int, off: c_long) -> *mut c_void;
    pub fn write(fd: c_int, buf: *const c_void, n: usize) -> isize;
    pub fn exit(status: c_int) -> !;
    pub fn abort() -> !;
    pub fn getenv(name: *const c_char) -> *const c_char;
    pub fn clock_gettime(clk: c_int, ts: *mut Timespec) -> c_int;
    pub fn munmap(addr: *mut c_void, len: usize) -> c_int;
    pub fn atexit(f: unsafe extern "C" fn()) -> c_int;

    pub fn pthread_self() -> c_ulong;
    pub fn pthread_getattr_np(thread: c_ulong, attr: *mut PthreadAttr) -> c_int;
    pub fn pthread_attr_getstack(attr: *const PthreadAttr, addr: *mut *mut c_void, size: *mut usize) -> c_int;
    pub fn pthread_attr_destroy(attr: *mut PthreadAttr) -> c_int;

    pub fn pthread_key_create(key: *mut u32, dtor: *const c_void) -> c_int;
    pub fn pthread_getspecific(key: u32) -> *mut c_void;
    pub fn pthread_setspecific(key: u32, value: *const c_void) -> c_int;

    pub fn pthread_mutex_init(m: *mut Opaque, attr: *const c_void) -> c_int;
    pub fn pthread_mutex_lock(m: *mut Opaque) -> c_int;
    pub fn pthread_mutex_unlock(m: *mut Opaque) -> c_int;
    pub fn pthread_cond_init(c: *mut Opaque, attr: *const c_void) -> c_int;
    pub fn pthread_cond_wait(c: *mut Opaque, m: *mut Opaque) -> c_int;
    pub fn pthread_cond_timedwait(c: *mut Opaque, m: *mut Opaque, abs: *const Timespec) -> c_int;
    pub fn pthread_cond_broadcast(c: *mut Opaque) -> c_int;
}

/// Map `bytes` of anonymous memory, or None when the kernel refuses (for
/// example under `RLIMIT_AS`). The growable tables use this, so a full address
/// space is an error the caller returns, not a fatal exit.
pub unsafe fn try_map(bytes: usize) -> Option<usize> {
    let p = mmap(
        core::ptr::null_mut(),
        bytes,
        PROT_READ | PROT_WRITE,
        MAP_PRIVATE | MAP_ANONYMOUS | MAP_NORESERVE,
        -1,
        0,
    ) as usize;
    if p == usize::MAX || p == 0 { None } else { Some(p) }
}

/// Nanoseconds on the monotonic clock, for the pause counters.
pub fn now_ns() -> u64 {
    let mut t = Timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { clock_gettime(CLOCK_MONOTONIC, &mut t) };
    t.tv_sec as u64 * 1_000_000_000 + t.tv_nsec as u64
}

/// A deadline `ms` milliseconds from now on the clock `pthread_cond_timedwait` uses.
pub fn deadline_after_ms(ms: usize) -> Timespec {
    let mut now = Timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { clock_gettime(CLOCK_REALTIME, &mut now) };
    let total = now.tv_nsec as u64 + (ms as u64 % 1000) * 1_000_000;
    Timespec {
        tv_sec: now.tv_sec + (ms / 1000) as c_long + (total / 1_000_000_000) as c_long,
        tv_nsec: (total % 1_000_000_000) as c_long,
    }
}

/// Fixed-size message builder for fatal errors. Numbers are formatted here
/// because the runtime has no formatting machinery.
pub struct Msg {
    buf: [u8; 256],
    n: usize,
}

impl Msg {
    pub const fn new() -> Self {
        Msg { buf: [0; 256], n: 0 }
    }
    fn put(&mut self, b: u8) {
        if self.n < self.buf.len() - 1 {
            self.buf[self.n] = b;
            self.n += 1;
        }
    }
    pub fn s(mut self, text: &[u8]) -> Self {
        for &b in text {
            self.put(b);
        }
        self
    }
    pub fn dec(mut self, mut v: usize) -> Self {
        let mut digits = [0u8; 20];
        let mut k = 0;
        loop {
            digits[k] = b'0' + (v % 10) as u8;
            k += 1;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        while k > 0 {
            k -= 1;
            self.put(digits[k]);
        }
        self
    }
    pub fn hex(mut self, v: usize) -> Self {
        self = self.s(b"0x");
        let mut shift = 60i32;
        while shift >= 0 {
            let d = ((v >> shift) & 0xf) as u8;
            self.put(if d < 10 { b'0' + d } else { b'a' + d - 10 });
            shift -= 4;
        }
        self
    }
    /// Print `panic: <message>` to stderr and exit 1, or call the runtime hook.
    pub fn die(mut self) -> ! {
        self.buf[self.n] = 0;
        fatal(&self.buf[..self.n + 1])
    }
    /// Write the message to stderr with no prefix and no newline added: the
    /// caller puts those in the text. Used for the one-line notices of the
    /// environment and the counter line.
    pub fn emit(self) {
        unsafe { write(2, self.buf.as_ptr() as *const c_void, self.n) };
    }
    /// Write the message to stderr followed by a newline.
    pub fn emit_line(mut self) {
        self = self.s(b"\n");
        self.emit();
    }
    /// The message bytes so far.
    pub fn bytes(&self) -> &[u8] {
        &self.buf[..self.n]
    }
}

/// Out-of-memory and internal-error exit. Calls the runtime's hook if one is
/// installed (kelp_gc_set_panic_hook); otherwise prints `panic: <msg>` and exits
/// with status 1, the same shape kelp_rt prints on the main thread.
/// `msg` must be NUL-terminated.
pub fn fatal(msg: &[u8]) -> ! {
    // The counter line comes first, so a failed run still shows its counts
    // (section 18.28). It is printed at most once per process.
    unsafe { crate::print_stats_once() };
    let hook = unsafe { crate::state::st().oom_hook };
    if hook != 0 {
        let f: unsafe extern "C" fn(*const u8) = unsafe { core::mem::transmute(hook) };
        unsafe { f(msg.as_ptr()) };
    }
    let prefix = b"panic: ";
    unsafe {
        write(2, prefix.as_ptr() as *const c_void, prefix.len());
        write(2, msg.as_ptr() as *const c_void, msg.len() - 1);
        write(2, b"\n".as_ptr() as *const c_void, 1);
        exit(1);
    }
}

/// The Rust panic path of the collector. The `no_std` build has no unwinder, so
/// it names the location and aborts (section 18.20).
pub fn report_and_abort(file: &str, line: u32) -> ! {
    Msg::new()
        .s(b"panic: gc: internal error at ")
        .s(file.as_bytes())
        .s(b":")
        .dec(line as usize)
        .s(b"\n")
        .emit();
    unsafe { abort() }
}
