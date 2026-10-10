//! Heap verifier, enabled by KELP_GC_VERIFY=1 and always on in the test
//! suite. Runs after every collection with every mutator stopped and the
//! registry lock held, so the heap is in the state the next allocation sees.
//!
//! It walks every block and the run-space table, checks every object header
//! and extent, checks that every mark bit is clear for the next cycle (the
//! epoch has flipped, so a survivor's mark equals the previous epoch), and
//! checks every pointer slot of every survivor: a non-null slot names a
//! survivor's payload start.
#![allow(unsafe_op_in_unsafe_fn)]

use crate::heap::*;
use crate::state::*;
use crate::sys::Msg;
use crate::types::*;

unsafe fn fail(what: &[u8], addr: usize) -> ! {
    Msg::new().s(b"gc: heap verify failed: ").s(what).s(b" at ").hex(addr).die()
}

/// Check the header at `h` and return its object's total size.
unsafe fn check_header(h: usize) -> usize {
    let s = st();
    let hdr = rd(h);
    let id = ((hdr >> HDR_TYPE_SHIFT) & 0xFF_FFFF) as usize;
    if id >= TYPE_TABLE_IDS {
        fail(b"type id outside the v1 table", h);
    }
    let d_raw = *(s.types as *const usize).add(id);
    if d_raw == 0 {
        fail(b"header names an unregistered type id", h);
    }
    let d = d_raw as *const KelpGcType;
    if hdr & HDR_STICKY != 0 {
        fail(b"sticky bit set in phase 1", h);
    }
    if (hdr & HDR_NOSCAN != 0) != ((*d).flags & DESC_NOSCAN != 0) {
        fail(b"noscan bit disagrees with the descriptor", h);
    }
    if hdr & HDR_ARRAY == 0 && hdr >> HDR_COUNT_SHIFT != 0 {
        fail(b"element count set on a scalar object", h);
    }
    let payload = if hdr & HDR_ARRAY != 0 {
        match ((hdr >> HDR_COUNT_SHIFT) as usize).checked_mul((*d).size as usize) {
            Some(p) => p,
            None => fail(b"array length overflows", h),
        }
    } else {
        (*d).size as usize
    };
    let total = total_for(payload);
    if (hdr & HDR_LARGE != 0) != (total > LARGE_MIN) {
        fail(b"large bit disagrees with the object size", h);
    }
    if (hdr & HDR_MARK) as usize == s.epoch {
        fail(b"mark bit set after the sweep", h);
    }
    total
}

/// Visit every pointer slot of the object at `h` (its header already checked).
unsafe fn for_each_slot(h: usize, f: &mut dyn FnMut(usize)) {
    let hdr = rd(h);
    let d = desc_of(((hdr >> HDR_TYPE_SHIFT) & 0xFF_FFFF) as u32);
    if hdr & HDR_NOSCAN != 0 {
        return;
    }
    let size = (*d).size as usize;
    let count = if hdr & HDR_ARRAY != 0 { (hdr >> HDR_COUNT_SHIFT) as usize } else { 1 };
    for i in 0..count {
        let base = h + 8 + i * size;
        if (*d).flags & DESC_ENUM != 0 {
            let tag = core::ptr::read_unaligned((base + (*d).tag_off as usize) as *const u32);
            if tag < (*d).n_variants {
                let v = &*(*d).variants.add(tag as usize);
                for k in 0..v.n_ptrs as usize {
                    let off = core::ptr::read_unaligned(v.ptr_off.add(k));
                    f(core::ptr::read_unaligned((base + off as usize) as *const usize));
                }
            }
        } else {
            for k in 0..(*d).n_ptrs as usize {
                let off = core::ptr::read_unaligned((*d).ptr_off.add(k));
                f(core::ptr::read_unaligned((base + off as usize) as *const usize));
            }
        }
    }
}

/// A pointer slot of a survivor must name a survivor's payload start.
unsafe fn check_slot(v: usize, owner: usize) {
    if v == 0 {
        return;
    }
    let s = st();
    if !in_region(v) {
        fail(b"pointer slot outside the heap, owner", owner);
    }
    let Some(h) = find_object(v) else {
        fail(b"pointer slot names no object, owner", owner);
    };
    if v != h + 8 {
        fail(b"pointer slot is not a payload start, owner", owner);
    }
    if (rd(h) & HDR_MARK) as usize != s.epoch ^ 1 {
        fail(b"pointer slot names an unmarked object, owner", owner);
    }
}

/// Full heap check. Caller holds the registry lock with the world stopped.
pub unsafe fn heap_check() {
    let s = st();
    for b in 0..s.nblocks {
        let m = blk(b);
        let base = base_of(b);
        if (*m).lines.iter().any(|&x| x > 1) {
            fail(b"line table entry is not 0 or 1", base);
        }
        match (*m).state {
            FREE => {
                if (*m).starts.iter().any(|&w| w != 0) {
                    fail(b"free block has start bits", base);
                }
                if (*m).lines.iter().any(|&x| x != 0) {
                    fail(b"free block has marked lines", base);
                }
            }
            ACTIVE => {
                let mut prev_end = base;
                for w in 0..64 {
                    let mut bits = (*m).starts[w];
                    while bits != 0 {
                        let bit = bits.trailing_zeros() as usize;
                        bits &= bits - 1;
                        let h = base + (w * 64 + bit) * 8;
                        let total = check_header(h);
                        if h < prev_end {
                            fail(b"objects overlap", h);
                        }
                        if h + total > base + BLOCK {
                            fail(b"object crosses the end of its block", h);
                        }
                        let off = h - base;
                        for l in (off >> LINE_SHIFT)..=((off + total - 1) >> LINE_SHIFT) {
                            if (*m).lines[l] == 0 {
                                fail(b"live object on an unmarked line", h);
                            }
                        }
                        prev_end = h + total;
                    }
                }
            }
            LARGE => {
                if (*m).starts.iter().any(|&w| w != 0) {
                    fail(b"run-space block has start bits", base);
                }
            }
            _ => fail(b"block in an unknown state", base),
        }
    }

    sort_large();
    let table = core::slice::from_raw_parts(s.large as *const usize, s.large_n);
    let region_end = s.region + s.region_bytes;
    let mut prev_end = 0usize;
    for &h in table {
        if !in_region(h) {
            fail(b"run-space object outside the heap", h);
        }
        let total = check_header(h);
        if h < prev_end {
            fail(b"run-space objects overlap", h);
        }
        if h + total > region_end {
            fail(b"run-space object crosses the end of the heap", h);
        }
        for b in ((h - s.region) >> BLOCK_SHIFT)..=((h + total - 1 - s.region) >> BLOCK_SHIFT) {
            if (*blk(b)).state != LARGE {
                fail(b"run-space object over a block outside the run space", h);
            }
        }
        prev_end = h + total;
    }

    // Pointer slots of every survivor, in both spaces.
    for b in 0..s.nblocks {
        let m = blk(b);
        if (*m).state != ACTIVE {
            continue;
        }
        let base = base_of(b);
        for w in 0..64 {
            let mut bits = (*m).starts[w];
            while bits != 0 {
                let bit = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                let h = base + (w * 64 + bit) * 8;
                for_each_slot(h, &mut |v| check_slot(v, h));
            }
        }
    }
    for &h in table {
        for_each_slot(h, &mut |v| check_slot(v, h));
    }
}
