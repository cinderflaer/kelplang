//! Global collector state. Every field is guarded by the registry lock
//! (`threads::G_MUTEX`) or the heap spin lock (`HEAP_LOCK`), as noted per use.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize};

use crate::sys::Opaque;

pub const BLOCK_SHIFT: u32 = 15;
pub const BLOCK: usize = 1 << BLOCK_SHIFT; // 32 KiB
pub const LINE_SHIFT: u32 = 7;
pub const LINE: usize = 1 << LINE_SHIFT; // 128 B
pub const LINES: usize = BLOCK / LINE; // 256
pub const LARGE_MIN: usize = 8192; // total object size above which blocks are taken whole
pub const MAX_THREADS: usize = 256;
pub const MAX_ROOTS: usize = 1024;
pub const DEFAULT_HEAP_MAX: usize = 256 << 20;
pub const DEFAULT_HEAP_MIN: usize = 8 << 20;

/// Per-block metadata, kept outside the block so all 32 KiB are payload.
#[repr(C)]
pub struct BlockMeta {
    /// FREE, ACTIVE, LARGE_HEAD or LARGE_CONT (see heap.rs).
    pub state: u32,
    /// LARGE_HEAD: number of blocks in the run. LARGE_CONT: index of the head.
    pub link: u32,
    /// One byte per line; nonzero means the line holds a live object (set by mark).
    pub lines: [u8; LINES],
    /// One bit per 8-byte granule: a set bit marks an object header.
    pub starts: [u64; 64],
}

pub struct State {
    pub region: usize,
    pub region_bytes: usize,
    pub nblocks: usize,
    pub blocks: usize,      // *mut BlockMeta
    pub types: usize,       // *mut usize, indexed by type id
    pub mark_stack: usize,  // *mut usize
    pub mark_cap: usize,
    pub mark_len: usize,
    pub heap_min: usize,
    pub heap_max: usize,
    pub epoch: usize,       // 0 or 1; a header is live when its mark bit equals this
    pub bytes_since: usize, // bytes allocated since the last cycle
    pub trigger: usize,
    pub live: usize,        // bytes marked in the current cycle
    pub used_blocks: usize,
    pub peak_blocks: usize,
    pub alloc_block: usize,
    pub alloc_line: usize,
    pub cursor: usize,
    pub limit: usize,
    pub stress_n: usize,
    pub stress_count: usize,
    pub oom_hook: usize,    // 0 or an `extern "C" fn(*const u8)` address
    pub collecting: bool,
    pub cycles: usize,
}

pub struct Global<T>(pub UnsafeCell<T>);
unsafe impl<T> Sync for Global<T> {}
impl<T> Global<T> {
    pub const fn get(&self) -> *mut T {
        self.0.get()
    }
}

pub static STATE: Global<State> = Global(UnsafeCell::new(unsafe { core::mem::zeroed() }));

/// Access to the state. Callers hold the registry lock or the heap lock.
#[inline]
pub unsafe fn st() -> &'static mut State {
    unsafe { &mut *STATE.get() }
}

/// 0 = uninitialized, 1 = initializing, 2 = ready.
pub static INIT: AtomicU32 = AtomicU32::new(0);
pub static HEAP_LOCK: AtomicBool = AtomicBool::new(false);
/// Nonzero while a collection has asked every mutator to stop.
pub static REQUEST: AtomicUsize = AtomicUsize::new(0);

pub static G_MUTEX: Global<Opaque> = Global(UnsafeCell::new(unsafe { core::mem::zeroed() }));
pub static G_COND: Global<Opaque> = Global(UnsafeCell::new(unsafe { core::mem::zeroed() }));

pub fn lock_heap() {
    use core::sync::atomic::Ordering::{Acquire, Relaxed};
    while HEAP_LOCK
        .compare_exchange_weak(false, true, Acquire, Relaxed)
        .is_err()
    {
        while HEAP_LOCK.load(Relaxed) {
            core::hint::spin_loop();
        }
    }
}

pub fn unlock_heap() {
    HEAP_LOCK.store(false, core::sync::atomic::Ordering::Release);
}
