//! Allocation, interior-pointer lookup, and sweep. Heap lock held by callers.
//!
//! Two spaces share the region:
//! - the line-hole space (objects up to LARGE_MIN bytes): bump allocation in
//!   the free lines of ACTIVE blocks, each object inside one block;
//! - the run space (objects above LARGE_MIN): a contiguous bump through runs of
//!   whole free blocks, so an object may cross a block boundary. A block in
//!   this space is state LARGE; its live objects are listed in `large`.
#![allow(unsafe_op_in_unsafe_fn)]

use crate::state::*;
use crate::sys::Msg;
use crate::types::*;

pub const FREE: u32 = 0;
pub const ACTIVE: u32 = 1;
pub const LARGE: u32 = 2;

/// A run of free blocks taken for the run space is at most this many blocks
/// (or the object's size, if larger), so a run's unused tail stays small.
pub const RUN_BLOCKS: usize = 16;

#[inline]
pub unsafe fn blk(b: usize) -> *mut BlockMeta {
    (st().blocks as *mut BlockMeta).add(b)
}

#[inline]
pub unsafe fn base_of(b: usize) -> usize {
    st().region + (b << BLOCK_SHIFT)
}

#[inline]
pub unsafe fn rd(a: usize) -> u64 {
    core::ptr::read(a as *const u64)
}

#[inline]
pub unsafe fn wr(a: usize, v: u64) {
    core::ptr::write(a as *mut u64, v)
}

#[inline]
pub unsafe fn in_region(a: usize) -> bool {
    let s = st();
    a >= s.region && a < s.region + s.region_bytes
}

/// Descriptor for a type id. A header always names a registered id.
pub unsafe fn desc_of(id: u32) -> *const KelpGcType {
    let p = *(st().types as *const usize).add(id as usize) as *const KelpGcType;
    if p.is_null() {
        crate::sys::fatal(b"gc: object header names an unregistered type id\0");
    }
    p
}

/// Register a descriptor under its id. Caller holds the heap lock.
pub unsafe fn register_type(d: *const KelpGcType) {
    let id = (*d).id;
    if id > TYPE_ID_MAX {
        Msg::new()
            .s(b"gc: type id ")
            .dec(id as usize)
            .s(b" does not fit the 24-bit header field (limit ")
            .dec(TYPE_ID_MAX as usize)
            .s(b")")
            .die();
    }
    if id as usize >= TYPE_TABLE_IDS {
        Msg::new()
            .s(b"gc: type id ")
            .dec(id as usize)
            .s(b" is over the v1 type table limit (ids 0 to ")
            .dec(TYPE_TABLE_IDS - 1)
            .s(b" are accepted)")
            .die();
    }
    let slot = (st().types as *mut usize).add(id as usize);
    if *slot == 0 {
        *slot = d as usize;
    } else if *slot != d as usize {
        crate::sys::fatal(b"gc: two type descriptors share one id\0");
    }
}

/// Payload bytes of an object from its header (before rounding).
pub unsafe fn payload_bytes(hdr: u64, d: *const KelpGcType) -> usize {
    let size = (*d).size as usize;
    if hdr & HDR_ARRAY != 0 {
        ((hdr >> HDR_COUNT_SHIFT) as usize) * size
    } else {
        size
    }
}

/// Total bytes of an object including its header. Minimum payload is 8.
pub fn total_for(payload: usize) -> usize {
    8 + core::cmp::max(8, (payload + 7) & !7)
}

pub unsafe fn object_total(h: usize) -> usize {
    let hdr = rd(h);
    let d = desc_of(((hdr >> HDR_TYPE_SHIFT) & 0xFF_FFFF) as u32);
    total_for(payload_bytes(hdr, d))
}

unsafe fn set_start_bit(h: usize, set: bool) {
    let off = h - st().region;
    let b = off >> BLOCK_SHIFT;
    let g = (off & (BLOCK - 1)) >> 3;
    let w = &mut (*blk(b)).starts[g >> 6];
    let bit = 1u64 << (g & 63);
    if set {
        *w |= bit;
    } else {
        *w &= !bit;
    }
}

/// Advance the bump cursor to the next hole of at least one line, or a free
/// block. Holes are the runs of lines with no mark, computed at the last cycle.
unsafe fn next_hole() -> bool {
    let s = st();
    loop {
        if s.alloc_block >= s.nblocks {
            return false;
        }
        let b = s.alloc_block;
        let m = blk(b);
        match (*m).state {
            FREE => {
                (*m).state = ACTIVE;
                s.used_blocks += 1;
                s.peak_blocks = core::cmp::max(s.peak_blocks, s.used_blocks);
                s.cursor = base_of(b);
                s.limit = base_of(b) + BLOCK;
                s.alloc_block = b + 1;
                s.alloc_line = 0;
                return true;
            }
            ACTIVE => {
                let mut l = s.alloc_line;
                while l < LINES && (*m).lines[l] != 0 {
                    l += 1;
                }
                if l == LINES {
                    s.alloc_block = b + 1;
                    s.alloc_line = 0;
                    continue;
                }
                let mut r = l;
                while r < LINES && (*m).lines[r] == 0 {
                    r += 1;
                }
                s.cursor = base_of(b) + l * LINE;
                s.limit = base_of(b) + r * LINE;
                s.alloc_line = r;
                return true;
            }
            _ => {
                s.alloc_block = b + 1;
                s.alloc_line = 0;
            }
        }
    }
}

unsafe fn try_small(total: usize) -> Option<usize> {
    let s = st();
    loop {
        if s.cursor + total <= s.limit {
            let h = s.cursor;
            s.cursor += total;
            return Some(h);
        }
        if !next_hole() {
            return None;
        }
    }
}

/// Take the next run of free blocks for the run space. A run is a maximal
/// stretch of FREE blocks, cut to RUN_BLOCKS or to the object's own block
/// count, whichever is larger. Returns false if no run can hold `total`.
unsafe fn take_run(total: usize) -> bool {
    let s = st();
    let want = core::cmp::max(RUN_BLOCKS, total.div_ceil(BLOCK));
    let mut b = s.run_scan;
    while b < s.nblocks {
        if (*blk(b)).state != FREE {
            b += 1;
            continue;
        }
        let start = b;
        while b < s.nblocks && b - start < want && (*blk(b)).state == FREE {
            b += 1;
        }
        let n = b - start;
        if n * BLOCK >= total {
            for i in start..b {
                (*blk(i)).state = LARGE;
            }
            s.used_blocks += n;
            s.peak_blocks = core::cmp::max(s.peak_blocks, s.used_blocks);
            s.run_cur = base_of(start);
            s.run_end = base_of(b);
            s.run_scan = b;
            return true;
        }
    }
    false
}

/// Bump through the run space. Returns the header address.
unsafe fn try_run(total: usize) -> Option<usize> {
    let s = st();
    loop {
        if s.run_cur != 0 && s.run_cur + total <= s.run_end {
            let h = s.run_cur;
            s.run_cur += total;
            return Some(h);
        }
        if !take_run(total) {
            return None;
        }
    }
}

/// Reserve `total` bytes; returns the header address. Heap lock held.
/// Run-space objects are appended to the large table.
pub unsafe fn try_alloc(total: usize) -> Option<usize> {
    if total > LARGE_MIN {
        let h = try_run(total)?;
        let s = st();
        if s.large_n == s.large_cap {
            crate::sys::fatal(b"gc: run-space object table is full\0");
        }
        *(s.large as *mut usize).add(s.large_n) = h;
        s.large_n += 1;
        s.large_sorted = false;
        Some(h)
    } else {
        let h = try_small(total)?;
        set_start_bit(h, true);
        Some(h)
    }
}

/// Sort the run-space table by address (heap lock or world stopped).
pub unsafe fn sort_large() {
    let s = st();
    if s.large_sorted {
        return;
    }
    let a = core::slice::from_raw_parts_mut(s.large as *mut usize, s.large_n);
    a.sort_unstable();
    s.large_sorted = true;
}

/// Header address of the object containing `addr`, or None. Interior pointers
/// resolve to their object. Valid between cycles and during marking; objects
/// that are dead but not yet swept still resolve.
pub unsafe fn find_object(addr: usize) -> Option<usize> {
    if !in_region(addr) {
        return None;
    }
    let s = st();
    let b = (addr - s.region) >> BLOCK_SHIFT;
    let m = blk(b);
    let contains = |h: usize| addr >= h + 8 && addr < h + object_total(h);
    match (*m).state {
        FREE => None,
        LARGE => {
            sort_large();
            let a = core::slice::from_raw_parts(s.large as *const usize, s.large_n);
            let idx = a.partition_point(|&h| h <= addr);
            if idx == 0 {
                return None;
            }
            let h = a[idx - 1];
            if contains(h) { Some(h) } else { None }
        }
        _ => {
            let g = (addr - base_of(b)) >> 3;
            let mut w = g >> 6;
            let bit = g & 63;
            let mask = if bit == 63 { !0u64 } else { (1u64 << (bit + 1)) - 1 };
            let mut bits = (*m).starts[w] & mask;
            loop {
                if bits != 0 {
                    let idx = w * 64 + 63 - bits.leading_zeros() as usize;
                    let h = base_of(b) + idx * 8;
                    return if contains(h) { Some(h) } else { None };
                }
                if w == 0 {
                    return None;
                }
                w -= 1;
                bits = (*m).starts[w];
            }
        }
    }
}

/// Free dead objects and blocks. Runs before the epoch flips, so an object is
/// live when its mark bit equals the current epoch. Mark has set the lines of
/// every live object, so a block with no marked line holds no live object.
pub unsafe fn sweep() {
    let s = st();
    sort_large();
    let a = s.large as *mut usize;
    let mut keep = 0usize;
    for i in 0..s.large_n {
        let h = *a.add(i);
        if (rd(h) & HDR_MARK) as usize == s.epoch {
            *a.add(keep) = h;
            keep += 1;
        }
    }
    s.large_n = keep;
    for b in 0..s.nblocks {
        let m = blk(b);
        match (*m).state {
            LARGE => {
                if (*m).lines.iter().all(|&x| x == 0) {
                    (*m).state = FREE;
                    s.used_blocks -= 1;
                }
            }
            ACTIVE => {
                if (*m).lines.iter().all(|&x| x == 0) {
                    // No marked line: every object here is dead. Free the block
                    // without reading a single header.
                    (*m).state = FREE;
                    (*m).starts = [0; 64];
                    s.used_blocks -= 1;
                } else {
                    let allocated: u32 = (*m).starts.iter().map(|w| w.count_ones()).sum();
                    // Every allocated object marked: nothing to clear.
                    if (*m).live != allocated {
                        for w in 0..64 {
                            let mut bits = (*m).starts[w];
                            while bits != 0 {
                                let bit = bits.trailing_zeros() as usize;
                                bits &= bits - 1;
                                let h = base_of(b) + (w * 64 + bit) * 8;
                                if (rd(h) & HDR_MARK) as usize != s.epoch {
                                    (*m).starts[w] &= !(1u64 << bit);
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
}
