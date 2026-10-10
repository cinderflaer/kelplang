//! Mark phase and cycle driver. Runs with the world stopped and the registry
//! lock held (see threads::collect).
#![allow(unsafe_op_in_unsafe_fn)]

use crate::heap::*;
use crate::state::*;
use crate::sys;
use crate::types::*;

/// Mark the object that `w` points into, if any. Conservative for stacks and
/// registers; precise slots use it too, since a slot holds a payload pointer.
pub unsafe fn mark_word(w: usize) {
    if w == 0 {
        return;
    }
    if let Some(h) = find_object(w) {
        mark_obj(h);
    }
}

unsafe fn mark_obj(h: usize) {
    let s = st();
    let hdr = rd(h);
    if (hdr & HDR_MARK) as usize == s.epoch {
        return;
    }
    wr(h, (hdr & !HDR_MARK) | s.epoch as u64);
    let total = object_total(h);
    s.live += total;
    if hdr & HDR_LARGE == 0 {
        // Small objects sit inside one block; mark every line they cover.
        let off = h - s.region;
        let b = off >> BLOCK_SHIFT;
        let first = (off & (BLOCK - 1)) >> LINE_SHIFT;
        let last = ((off & (BLOCK - 1)) + total - 1) >> LINE_SHIFT;
        for l in first..=last {
            (*blk(b)).lines[l] = 1;
        }
    }
    if hdr & HDR_NOSCAN == 0 {
        if s.mark_len == s.mark_cap {
            sys::fatal(b"gc: mark stack overflow\0");
        }
        *(s.mark_stack as *mut usize).add(s.mark_len) = h;
        s.mark_len += 1;
    }
}

/// Scan one element (or fixed object) at `base` using descriptor `d`.
pub unsafe fn scan_root_value(base: usize, d: *const KelpGcType) {
    scan_elem(base, d);
}

unsafe fn scan_elem(base: usize, d: *const KelpGcType) {
    if (*d).flags & DESC_ENUM != 0 {
        let tag = core::ptr::read_unaligned((base + (*d).tag_off as usize) as *const u32);
        if tag < (*d).n_variants {
            let v = &*(*d).variants.add(tag as usize);
            for k in 0..v.n_ptrs as usize {
                let off = core::ptr::read_unaligned(v.ptr_off.add(k));
                mark_word(core::ptr::read_unaligned((base + off as usize) as *const usize));
            }
        }
    } else {
        for k in 0..(*d).n_ptrs as usize {
            let off = core::ptr::read_unaligned((*d).ptr_off.add(k));
            mark_word(core::ptr::read_unaligned((base + off as usize) as *const usize));
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

pub unsafe fn drain() {
    let s = st();
    while s.mark_len > 0 {
        s.mark_len -= 1;
        let h = *(s.mark_stack as *const usize).add(s.mark_len);
        scan_object(h);
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
    for b in 0..s.nblocks {
        (*blk(b)).lines = [0; LINES];
    }
    crate::threads::scan_roots();
    drain();
    lock_heap();
    sweep();
    unlock_heap();
    s.epoch ^= 1;
    s.cycles += 1;
    s.trigger = core::cmp::max(s.heap_min, s.live);
    s.bytes_since = 0;
    s.alloc_block = 0;
    s.alloc_line = 0;
    s.cursor = 0;
    s.limit = 0;
}
