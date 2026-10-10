//! Allocation, interior-pointer lookup, and sweep. Heap lock held by callers.
#![allow(unsafe_op_in_unsafe_fn)]

use crate::state::*;
use crate::sys;
use crate::types::*;

pub const FREE: u32 = 0;
pub const ACTIVE: u32 = 1;
pub const LARGE_HEAD: u32 = 2;
pub const LARGE_CONT: u32 = 3;

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
        sys::fatal(b"gc: object header names an unregistered type id\0");
    }
    p
}

/// Register a descriptor under its id. Caller holds the heap lock.
pub unsafe fn register_type(d: *const KelpGcType) {
    let id = (*d).id;
    if id > TYPE_ID_MAX || id as usize >= TYPE_TABLE_IDS {
        sys::fatal(b"gc: type id is out of range for the v1 type table\0");
    }
    let slot = (st().types as *mut usize).add(id as usize);
    if *slot == 0 {
        *slot = d as usize;
    } else if *slot != d as usize {
        sys::fatal(b"gc: two type descriptors share one id\0");
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

/// A run of whole free blocks for an object above LARGE_MIN.
unsafe fn try_large(total: usize) -> Option<usize> {
    let s = st();
    let n = (total + BLOCK - 1) >> BLOCK_SHIFT;
    let mut run = 0usize;
    let mut start = 0usize;
    for b in 0..s.nblocks {
        if (*blk(b)).state != FREE {
            run = 0;
            continue;
        }
        if run == 0 {
            start = b;
        }
        run += 1;
        if run == n {
            let head = blk(start);
            (*head).state = LARGE_HEAD;
            (*head).link = n as u32;
            for i in start + 1..start + n {
                (*blk(i)).state = LARGE_CONT;
                (*blk(i)).link = start as u32;
            }
            s.used_blocks += n;
            s.peak_blocks = core::cmp::max(s.peak_blocks, s.used_blocks);
            return Some(base_of(start));
        }
    }
    None
}

/// Reserve `total` bytes; returns the header address. Heap lock held.
pub unsafe fn try_alloc(total: usize) -> Option<usize> {
    let h = if total > LARGE_MIN { try_large(total)? } else { try_small(total)? };
    set_start_bit(h, true);
    Some(h)
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
        LARGE_CONT => {
            let h = base_of((*m).link as usize);
            if contains(h) { Some(h) } else { None }
        }
        LARGE_HEAD => {
            let h = base_of(b);
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
/// live when its mark bit equals the current epoch.
pub unsafe fn sweep() {
    let s = st();
    for b in 0..s.nblocks {
        let m = blk(b);
        match (*m).state {
            LARGE_HEAD => {
                let h = base_of(b);
                if (rd(h) & HDR_MARK) as usize != s.epoch {
                    set_start_bit(h, false);
                    let n = (*m).link as usize;
                    for i in b..b + n {
                        (*blk(i)).state = FREE;
                        (*blk(i)).link = 0;
                    }
                    s.used_blocks -= n;
                }
            }
            ACTIVE => {
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
                if (*m).lines.iter().all(|&x| x == 0) {
                    (*m).state = FREE;
                    (*m).starts = [0; 64];
                    s.used_blocks -= 1;
                }
            }
            _ => {}
        }
    }
}
