//! Mark phase and cycle driver. Runs with the world stopped and the registry
//! lock held (see threads::collect).
#![allow(unsafe_op_in_unsafe_fn)]

use crate::heap::*;
use crate::state::*;
use crate::sys;
use crate::types::*;

/// Entries between the mark stack and marking. A candidate's header is
/// prefetched when it enters the ring and read only when it leaves, so the
/// misses of up to RING objects overlap.
const RING: usize = 8;

/// Queue a candidate object header. Its header is not read here.
#[inline(always)]
unsafe fn push_candidate(h: usize) {
    let s = st();
    if s.mark_len == s.mark_cap {
        sys::fatal(b"gc: mark stack overflow\0");
    }
    *(s.mark_stack as *mut usize).add(s.mark_len) = h;
    s.mark_len += 1;
}

/// Queue the object that `w` points into, if any. Conservative for stacks and
/// registers; precise slots use it too, since a slot holds a payload pointer.
pub unsafe fn mark_word(w: usize) {
    if w == 0 {
        return;
    }
    if let Some(h) = find_object(w) {
        push_candidate(h);
    }
}

/// Mark a candidate that left the ring. Returns true if it is live, was not
/// marked before, and has pointer slots to scan.
#[inline(always)]
unsafe fn mark_obj(h: usize) -> bool {
    let s = st();
    let hdr = rd(h);
    if (hdr & HDR_MARK) as usize == s.epoch {
        return false;
    }
    wr(h, (hdr & !HDR_MARK) | s.epoch as u64);
    let total = object_total(h);
    s.live += total;
    let off = h - s.region;
    if hdr & HDR_LARGE == 0 {
        // Small objects sit inside one block; mark every line they cover.
        let b = off >> BLOCK_SHIFT;
        let first = (off & (BLOCK - 1)) >> LINE_SHIFT;
        let last = ((off & (BLOCK - 1)) + total - 1) >> LINE_SHIFT;
        for l in first..=last {
            (*blk(b)).lines[l] = 1;
        }
        (*blk(b)).live += 1;
    } else {
        // A run-space object may cross blocks. Its blocks stay in use whole.
        for b in (off >> BLOCK_SHIFT)..=((off + total - 1) >> BLOCK_SHIFT) {
            (*blk(b)).lines = [1; LINES];
        }
    }
    hdr & HDR_NOSCAN == 0
}

/// Scan one element (or fixed object) at `base` using descriptor `d`.
pub unsafe fn scan_root_value(base: usize, d: *const KelpGcType) {
    scan_elem(base, d);
}

/// Mark the object a precise pointer slot names. A slot holds a payload start,
/// so when `p - 8` is an object start in an ACTIVE block the start bit answers
/// directly; anything else (run space, interior or stray words) takes find_object.
#[inline(always)]
unsafe fn mark_slot(p: usize) {
    if p == 0 {
        return;
    }
    let s = st();
    if p >= s.region + 8 && p < s.region + s.region_bytes && p & 7 == 0 {
        let off = p - 8 - s.region;
        let b = off >> BLOCK_SHIFT;
        let m = blk(b);
        if (*m).state == ACTIVE {
            let g = (off & (BLOCK - 1)) >> 3;
            if (*m).starts[g >> 6] & (1u64 << (g & 63)) != 0 {
                push_candidate(p - 8);
                return;
            }
        }
    }
    mark_word(p);
}

/// Prefetch the header at `h` (no-op off x86_64).
#[inline(always)]
unsafe fn prefetch_header(h: usize) {
    #[cfg(target_arch = "x86_64")]
    core::arch::x86_64::_mm_prefetch(h as *const i8, core::arch::x86_64::_MM_HINT_T0);
    #[cfg(not(target_arch = "x86_64"))]
    let _ = h;
}

unsafe fn scan_elem(base: usize, d: *const KelpGcType) {
    if (*d).flags & DESC_ENUM != 0 {
        let tag = core::ptr::read_unaligned((base + (*d).tag_off as usize) as *const u32);
        if tag < (*d).n_variants {
            let v = &*(*d).variants.add(tag as usize);
            for k in 0..v.n_ptrs as usize {
                let off = core::ptr::read_unaligned(v.ptr_off.add(k));
                mark_slot(core::ptr::read_unaligned((base + off as usize) as *const usize));
            }
        }
    } else {
        for k in 0..(*d).n_ptrs as usize {
            let off = core::ptr::read_unaligned((*d).ptr_off.add(k));
            mark_slot(core::ptr::read_unaligned((base + off as usize) as *const usize));
        }
    }
}

unsafe fn scan_object(h: usize) {
    let hdr = rd(h);
    let d = desc_of(((hdr >> HDR_TYPE_SHIFT) & 0xFF_FFFF) as u32);
    let payload = h + 8;
    if hdr & HDR_ARRAY != 0 {
        let count = (hdr >> HDR_COUNT_SHIFT) as usize;
        let size = (*d).size as usize;
        for i in 0..count {
            scan_elem(payload + i * size, d);
        }
    } else {
        scan_elem(payload, d);
    }
}

/// Process every queued candidate. Entries leave the stack into a ring of
/// RING slots (prefetching their headers on the way in); the oldest entry in
/// the ring is marked and, if needed, scanned, which queues its children.
pub unsafe fn drain() {
    let s = st();
    let mut ring = [0usize; RING];
    let mut head = 0usize;
    let mut n = 0usize;
    loop {
        while n < RING && s.mark_len > 0 {
            s.mark_len -= 1;
            let h = *(s.mark_stack as *const usize).add(s.mark_len);
            prefetch_header(h);
            ring[(head + n) % RING] = h;
            n += 1;
        }
        if n == 0 {
            break;
        }
        let h = ring[head];
        head = (head + 1) % RING;
        n -= 1;
        if mark_obj(h) {
            scan_object(h);
        }
    }
}

/// Scan `[lo, hi)` one aligned word at a time, conservatively.
pub unsafe fn scan_range_conservative(lo: usize, hi: usize) {
    let mut p = lo & !7;
    while p + 8 <= hi {
        mark_word(core::ptr::read_volatile(p as *const usize));
        p += 8;
    }
}

/// Full cycle. Caller holds the registry lock and every other mutator is
/// stopped at a safepoint or in native state, with its stack range recorded.
pub unsafe fn collect_locked() {
    let s = st();
    s.live = 0;
    s.mark_len = 0;
    for b in 0..s.pool_hw {
        (*blk(b)).lines = [0; LINES];
        (*blk(b)).live = 0;
    }
    sort_large();
    let t0 = sys::now_ns();
    crate::threads::scan_roots();
    drain();
    let t1 = sys::now_ns();
    lock_heap();
    sweep();
    unlock_heap();
    let t2 = sys::now_ns();
    s.mark_ns_total += t1 - t0;
    s.sweep_ns_total += t2 - t1;
    s.epoch ^= 1;
    s.cycles += 1;
    s.live_prev = s.live_last;
    s.live_last = s.live;
    s.trigger = core::cmp::min(
        crate::trigger::next_trigger(s.heap_min, s.live_prev, s.live_last, s.headroom_pct),
        s.region_bytes,
    );
    s.bytes_since = 0;
    s.alloc_block = 0;
    s.alloc_line = 0;
    s.cursor = 0;
    s.limit = 0;
    s.run_cur = 0;
    s.run_end = 0;
    s.run_scan = 0;
    if s.verify {
        crate::verify::heap_check();
    }
}
