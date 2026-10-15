//! KelpScript Tidepool garbage collector core (WP1, phase 1).
//!
//! Non-moving mark-region (Immix-style) tracing collector with a precise heap
//! and conservatively scanned thread stacks. See `kelpscript-gc-paper.md`
//! sections 3 and 4. The C ABI is the `kelp_gc_*` exports below.
#![cfg_attr(not(test), no_std)]
#![allow(unsafe_op_in_unsafe_fn)]

pub mod heap;
pub mod mark;
pub mod state;
pub mod sys;
pub mod threads;
pub mod trigger;
pub mod tls;
pub mod types;
pub mod verify;

use core::sync::atomic::Ordering::{Acquire, Release};

use heap::*;
use state::*;
use sys::Msg;
use types::*;

pub use types::{KelpGcStats, KelpGcType, KelpGcVariant, KELP_GC_ROOT_FULL, KELP_GC_ROOT_OK, KELP_GC_ROOT_STALE};

#[cfg(not(test))]
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    // No unwinder: name the location and abort (section 18.20, T74).
    match info.location() {
        Some(loc) => sys::report_and_abort(loc.file(), loc.line()),
        None => sys::report_and_abort("?", 0),
    }
}

/// The personality routine the unwind tables name. This crate builds with
/// `panic = "abort"`, so there is nothing to catch, but a worker's `pthread_exit`
/// is a forced unwind that calls the personality of every frame with unwind
/// info. Each frame must answer `_URC_CONTINUE_UNWIND` (8): any other answer,
/// such as the empty return this stub used to give, makes glibc abort the thread
/// before the collector's detach can finish the process cleanly. The signature
/// matches `kelp-rt`'s personality.
#[cfg(not(test))]
#[unsafe(no_mangle)]
pub extern "C" fn rust_eh_personality(
    _version: i32,
    _actions: i32,
    _class: u64,
    _exception: *mut core::ffi::c_void,
    _context: *mut core::ffi::c_void,
) -> i32 {
    /// `_URC_CONTINUE_UNWIND`.
    const CONTINUE_UNWIND: i32 = 8;
    CONTINUE_UNWIND
}

#[cfg(test)]
#[path = "../../tests/gc/mod.rs"]
mod tests;

/// Map the heap region and metadata. Called once, under INIT == 1.
unsafe fn do_init(
    heap_min: usize,
    heap_max: usize,
    start_low: usize,
    start_high: usize,
    headroom_pct: usize,
    trigger_rule: usize,
) {
    let s = st();
    let max = if heap_max == 0 { DEFAULT_HEAP_MAX } else { heap_max };
    let nblocks = core::cmp::max(4, max.div_ceil(BLOCK));
    let region_bytes = nblocks * BLOCK;
    let raw = map(region_bytes + BLOCK);
    let meta_bytes = nblocks * core::mem::size_of::<BlockMeta>();
    let meta = map(meta_bytes);
    let types_bytes = TYPE_TABLE_IDS * 8;
    let types = map(types_bytes);
    // Mark stack entries: one per pointer slot of a live object (at most one
    // word per 8 bytes of heap), plus room for conservative root words.
    let mark_cap = region_bytes / 8 + (1 << 21);
    let mark = map(mark_cap * 8);
    s.region = (raw + BLOCK - 1) & !(BLOCK - 1);
    s.region_bytes = region_bytes;
    s.nblocks = nblocks;
    s.blocks = meta;
    s.types = types;
    s.mark_stack = mark;
    s.mark_cap = mark_cap;
    s.heap_max = region_bytes;
    s.heap_min = if heap_min == 0 { DEFAULT_HEAP_MIN } else { heap_min };
    s.headroom_pct = headroom_pct;
    s.trigger_rule = match trigger::Rule::from_code(trigger_rule) {
        Some(rule) => rule.code(),
        None => {
            Msg::new().s(b"gc: unknown trigger rule, using two-size").emit_line();
            trigger::Rule::TwoSize.code()
        }
    };
    let low = if start_low == 0 { DEFAULT_START_LOW } else { start_low };
    let high = if start_high == 0 { DEFAULT_START_HIGH } else { start_high };
    // The first clearing is the start guesses' midpoint plus headroom (design
    // section 4). A trigger above the shared-pool space would never be reached.
    s.trigger = core::cmp::min(trigger::first_trigger(s.heap_min, low, high, headroom_pct), region_bytes);
    s.pool_hw = 0;
    s.unused_tail = 0;
    s.epoch = 0;
    s.stress_n = env_usize(b"KELP_GC_STRESS\0").unwrap_or(0);
    s.verify = env_usize(b"KELP_GC_VERIFY\0").unwrap_or(0) != 0;
    s.scan_stacks = true;
    s.stop_timeout_ms = match env_usize(b"KELP_GC_STOP_TIMEOUT_MS\0") {
        Some(0) => {
            Msg::new().s(b"gc: KELP_GC_STOP_TIMEOUT_MS must be at least 1").emit_line();
            DEFAULT_STOP_TIMEOUT_MS
        }
        Some(ms) => ms,
        None => DEFAULT_STOP_TIMEOUT_MS,
    };
    s.stats_on = env_usize(b"KELP_GC_STATS\0").unwrap_or(0) != 0;
    if s.stats_on {
        sys::atexit(print_stats_at_exit);
    }
    s.spawn_pause = env_usize(b"KELP_GC_TEST_SPAWN_PAUSE\0").unwrap_or(0) != 0;
    s.large_cap = region_bytes / LARGE_MIN + 1;
    s.large = map(s.large_cap * 8);
    threads::init_locks();
}

/// Decimal value of an environment variable. None when it is unset. A value
/// that is not a whole number prints one notice and is ignored (section 18.24).
pub unsafe fn env_usize(name: &[u8]) -> Option<usize> {
    let p = sys::getenv(name.as_ptr() as *const _);
    if p.is_null() {
        return None;
    }
    let mut n = 0usize;
    let mut q = p as *const u8;
    let mut ok = *q != 0;
    while ok && *q >= b'0' && *q <= b'9' {
        match n.checked_mul(10).and_then(|v| v.checked_add((*q - b'0') as usize)) {
            Some(v) => n = v,
            None => ok = false,
        }
        q = q.add(1);
    }
    if ok && *q == 0 {
        return Some(n);
    }
    let var = &name[..name.len() - 1];
    let mut value = Msg::new().s(b"gc: ignored ").s(var).s(b"='");
    let mut v = p as *const u8;
    while *v != 0 {
        value = value.s(core::slice::from_raw_parts(v, 1));
        v = v.add(1);
    }
    value.s(b"': expected a whole number").emit_line();
    None
}

/// The counter line of section 9 (`KELP_GC_STATS=1`): the harness parses it, so
/// the first field keeps its name. Printed once, at exit or before a fatal exit.
pub unsafe fn print_stats_once() {
    let s = st();
    if !s.stats_on || s.stats_printed {
        return;
    }
    s.stats_printed = true;
    let (roots, _) = threads::root_counts();
    Msg::new()
        .s(b"kelp-gc-stats: collections=")
        .dec(s.cycles)
        .s(b" pause_total_us=")
        .dec((s.pause_total_ns / 1000) as usize)
        .s(b" pause_max_us=")
        .dec((s.pause_max_ns / 1000) as usize)
        .s(b" roots=")
        .dec(roots)
        .s(b" charged_bytes=")
        .dec(s.charged_bytes)
        .emit_line();
}

unsafe extern "C" fn print_stats_at_exit() {
    print_stats_once();
}

/// Anonymous, zeroed, lazily committed mapping. Aborts the process on failure.
unsafe fn map(bytes: usize) -> usize {
    let p = sys::mmap(
        core::ptr::null_mut(),
        bytes,
        sys::PROT_READ | sys::PROT_WRITE,
        sys::MAP_PRIVATE | sys::MAP_ANONYMOUS | sys::MAP_NORESERVE,
        -1,
        0,
    ) as usize;
    if p == usize::MAX || p == 0 {
        sys::fatal(b"out of memory\0");
    }
    p
}

unsafe fn init_once(
    heap_min: usize,
    heap_max: usize,
    start_low: usize,
    start_high: usize,
    headroom_pct: usize,
    trigger_rule: usize,
) {
    use core::sync::atomic::Ordering::Relaxed;
    match INIT.compare_exchange(0, 1, Acquire, Relaxed) {
        Ok(_) => {
            do_init(heap_min, heap_max, start_low, start_high, headroom_pct, trigger_rule);
            INIT.store(2, Release);
        }
        Err(1) => {
            while INIT.load(Acquire) != 2 {
                core::hint::spin_loop();
            }
        }
        Err(_) => {}
    }
}

#[inline]
unsafe fn ensure_init() {
    if INIT.load(Acquire) != 2 {
        init_once(0, 0, 0, 0, DEFAULT_HEADROOM_PCT, trigger::Rule::TwoSize.code());
    }
}

/// The shared pool is full after a clearing (section 18.2, T63): the three
/// numbers a programmer needs, then the note that names the change.
#[cold]
#[inline(never)]
unsafe fn out_of_shared_pool() -> ! {
    let s = st();
    Msg::new()
        .s(b"out of shared-pool memory: heap_max is ")
        .dec(s.heap_max >> 20)
        .s(b" MiB, ")
        .dec(s.live >> 20)
        .s(b" MiB was live at the last clearing, and ")
        .dec(s.charged_bytes >> 20)
        .s(b" MiB is charged to live blocks\n  note: keep less data live, or raise heap_max in coral.toml's [target] section")
        .die()
}

/// The array count is above the element limit (section 18.3). Kept out of
/// `alloc_common` so its message buffer does not change that function's frame.
#[cold]
#[inline(never)]
unsafe fn array_too_large(count: usize) -> ! {
    Msg::new()
        .s(b"array of ")
        .dec(count)
        .s(b" elements is too large: a shared-pool array holds at most 4294967295 elements")
        .die()
}

/// The count times the element size overflows (section 18.3).
#[cold]
#[inline(never)]
unsafe fn array_overflows(count: usize, size: usize) -> ! {
    Msg::new()
        .s(b"array size overflows: ")
        .dec(count)
        .s(b" elements of ")
        .dec(size)
        .s(b" bytes each")
        .die()
}

/// Allocate one object or array. Returns the payload pointer.
unsafe fn alloc_common(d: *const KelpGcType, count: usize, is_array: bool) -> *mut u8 {
    ensure_init();
    if threads::my_index().is_none() {
        sys::fatal(b"gc: allocation on a thread that is not attached (call kelp_gc_thread_attach before the first allocation)\0");
    }
    let desc = &*d;
    if desc.align > 8 {
        sys::fatal(b"gc: type alignment above 8 is not supported in v1\0");
    }
    if desc.flags & DESC_NOSCAN != 0 && desc.n_ptrs != 0 {
        sys::fatal(b"gc: noscan type descriptor has pointer slots\0");
    }
    let payload = if is_array {
        // Both checks run before any clearing: no clearing makes such a request fit (section 18.3).
        // The product is checked first, so a count whose byte size overflows is named as such.
        let bytes = match count.checked_mul(desc.size as usize) {
            Some(p) => p,
            None => array_overflows(count, desc.size as usize),
        };
        if count > u32::MAX as usize {
            array_too_large(count);
        }
        bytes
    } else {
        desc.size as usize
    };
    let total = total_for(payload);
    threads::safepoint();
    let s = st();
    if s.stress_n != 0 {
        s.stress_count += 1;
        if s.stress_count % s.stress_n == 0 {
            threads::collect();
        }
    }
    if st().bytes_since >= st().trigger {
        threads::collect();
    }
    let mut collected_here = false;
    loop {
        lock_heap();
        register_type(d);
        let got = try_alloc(total);
        if let Some(h) = got {
            let s = st();
            let mut hdr = (desc.id as u64) << HDR_TYPE_SHIFT;
            if is_array {
                hdr |= HDR_ARRAY | ((count as u64) << HDR_COUNT_SHIFT);
            }
            if desc.flags & DESC_NOSCAN != 0 {
                hdr |= HDR_NOSCAN;
            }
            if total > LARGE_MIN {
                hdr |= HDR_LARGE;
            }

            // New objects carry the unmarked value: the epoch's complement.
            hdr |= (s.epoch ^ 1) as u64;
            wr(h, hdr);
            core::ptr::write_bytes((h + 8) as *mut u8, 0, total - 8);
            s.bytes_since += total;
            s.allocs += 1;
            unlock_heap();
            return (h + 8) as *mut u8;
        }
        unlock_heap();
        if collected_here {
            out_of_shared_pool();
        }
        collected_here = threads::collect();
    }
}

// ---- Exported C ABI. The layouts are in types.rs. ----

/// Phase 1 API. heap_max of 0 selects the default 256 MiB; heap_min of 0
/// selects 8 MiB (the collection trigger floor). start_low and start_high are
/// the first clearing's two guesses in bytes (0 selects 1 MiB and 4 MiB), and
/// headroom_pct is the percent added to the measured live size (manifest
/// [tidepool]). The first trigger is the midpoint of the guesses plus headroom.
/// trigger_rule picks how each later trigger follows the live sizes: 0 is
/// two-size (the default), 1 is last-size, 2 is gogc-style (manifest
/// trigger_rule). Call before any allocation. A later call is ignored.
/// Arity guard: every Rust caller of `kelp_gc_init` is checked against this
/// six-argument type, because a caller compiled against a shorter signature
/// links and runs wrong.
const _: unsafe extern "C" fn(usize, usize, usize, usize, usize, usize) = kelp_gc_init;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_gc_init(
    heap_min: usize,
    heap_max: usize,
    start_low: usize,
    start_high: usize,
    headroom_pct: usize,
    trigger_rule: usize,
) {
    init_once(heap_min, heap_max, start_low, start_high, headroom_pct, trigger_rule);
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_gc_alloc(t: *const KelpGcType) -> *mut u8 {
    alloc_common(t, 1, false)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_gc_alloc_array(elem: *const KelpGcType, count: usize) -> *mut u8 {
    alloc_common(elem, count, true)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_gc_collect() {
    ensure_init();
    threads::collect();
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_gc_safepoint() {
    ensure_init();
    threads::safepoint();
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_gc_enter_native() {
    ensure_init();
    threads::enter_native();
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_gc_leave_native() {
    ensure_init();
    threads::leave_native();
}

/// Register a root. Returns KELP_GC_ROOT_OK and writes the
/// handle to `out`, or KELP_GC_ROOT_FULL and writes nothing.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_gc_register_root_region(base: *mut u8, layout: *const KelpGcType, out: *mut u64) -> i32 {
    ensure_init();
    match threads::register_root(base as usize, layout as usize) {
        Ok(h) => {
            if !out.is_null() {
                *out = h;
            }
            KELP_GC_ROOT_OK
        }
        Err(threads::RootError::Full) => KELP_GC_ROOT_FULL,
    }
}

/// Clear the root `h` names. KELP_GC_ROOT_STALE when `h` is not a live entry.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_gc_unregister_root_region(h: u64) -> i32 {
    if threads::unregister_root(h) { KELP_GC_ROOT_OK } else { KELP_GC_ROOT_STALE }
}

/// Fill `out` with the collector's counters. The counts are read
/// under the registry lock.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_gc_stats(out: *mut KelpGcStats) {
    ensure_init();
    threads::lock_for_stats();
    let s = st();
    let (roots, root_slots) = threads::root_counts();
    *out = KelpGcStats {
        collections: s.cycles as u64,
        pause_total_us: s.pause_total_ns / 1000,
        pause_max_us: s.pause_max_ns / 1000,
        roots: roots as u64,
        root_slots: root_slots as u64,
        charged_bytes: s.charged_bytes as u64,
    };
    threads::unlock_for_stats();
}

/// The test hook of section 9 (`KELP_GC_TEST_SPAWN_PAUSE`): nonzero when a spawn
/// must run a clearing while its worker is held before it attaches.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_gc_test_spawn_pause() -> i32 {
    ensure_init();
    st().spawn_pause as i32
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_gc_is_heap_ptr(p: *const u8) -> i32 {
    ensure_init();
    lock_heap();
    let r = find_object(p as usize).is_some();
    unlock_heap();
    r as i32
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_gc_thread_attach() {
    ensure_init();
    threads::attach();
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_gc_thread_detach() {
    threads::detach();
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_gc_set_panic_hook(hook: Option<unsafe extern "C" fn(*const u8)>) {
    st().oom_hook = match hook {
        Some(f) => f as usize,
        None => 0,
    };
}
