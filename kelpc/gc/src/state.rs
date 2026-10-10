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
/// Total object size above which an object goes to the run space (contiguous
/// bump through free blocks, see heap.rs) instead of the line-hole allocator.
pub const LARGE_MIN: usize = 8192;
pub const MAX_THREADS: usize = 256;
pub const MAX_ROOTS: usize = 1024;
pub const DEFAULT_HEAP_MAX: usize = 256 << 20;
pub const DEFAULT_HEAP_MIN: usize = 8 << 20;
/// Default time a mutator may take to reach a safepoint before the collector
/// gives up and names it.
pub const DEFAULT_STOP_TIMEOUT_MS: usize = 10_000;

/// Per-block metadata, kept outside the block so all 32 KiB are payload.
#[repr(C)]
pub struct BlockMeta {
    /// FREE, ACTIVE or LARGE (see heap.rs).
    pub state: u32,
    /// Small objects marked in the current cycle (reset at cycle start). Sweep
    /// compares it with the allocated count to skip fully live blocks.
    pub live: u32,
    /// One byte per line; nonzero means the line holds a live object (set by mark).
    pub lines: [u8; LINES],
    /// One bit per 8-byte granule: a set bit marks an object header (ACTIVE only).
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
    /// Run-space object headers (`*mut usize`), sorted after `sort_large`.
    pub large: usize,
    pub large_n: usize,
    pub large_cap: usize,
    pub large_sorted: bool,
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
    /// Run-space bump range and the next block index to search for a run.
    pub run_cur: usize,
    pub run_end: usize,
    pub run_scan: usize,
    pub stress_n: usize,
    pub stress_count: usize,
    pub verify: bool,
    /// Test hook: when false, a collection does not scan thread stacks or
    /// registers (root regions still count). Always true outside tests.
    pub scan_stacks: bool,
    pub stop_timeout_ms: usize,
    pub tls_key: u32,
    pub oom_hook: usize,    // 0 or an `extern "C" fn(*const u8)` address
    pub collecting: bool,
    pub cycles: usize,
    /// Stop-the-world time in ns: total and the longest single pause (measurement).
    pub pause_ns_total: u64,
    pub pause_ns_max: u64,
    /// Time in root scan and mark, and in sweep, summed over cycles (measurement).
    pub mark_ns_total: u64,
    pub sweep_ns_total: u64,
    pub allocs: usize,
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
/// Heap lock acquisitions that found the lock held (measurement only).
pub static CONTENDED: AtomicUsize = AtomicUsize::new(0);
/// Nonzero while a collection has asked every mutator to stop.
pub static REQUEST: AtomicUsize = AtomicUsize::new(0);

pub static G_MUTEX: Global<Opaque> = Global(UnsafeCell::new(unsafe { core::mem::zeroed() }));
pub static G_COND: Global<Opaque> = Global(UnsafeCell::new(unsafe { core::mem::zeroed() }));

pub fn lock_heap() {
    use core::sync::atomic::Ordering::{Acquire, Relaxed};
    if HEAP_LOCK.compare_exchange(false, true, Acquire, Relaxed).is_ok() {
        return;
    }
    CONTENDED.fetch_add(1, Relaxed);
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
