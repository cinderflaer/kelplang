//! Collector tests. Included from `src/lib.rs` under `cfg(test)`; run by
//! `cargo test` inside `kelpc/gc`. The collector is process-global, so tests
//! take a lock and reset the heap first.
#![allow(unsafe_op_in_unsafe_fn)]

use core::cell::UnsafeCell;
use core::hint::black_box;
use core::mem::{align_of, offset_of, size_of};
use std::sync::{Mutex, MutexGuard};

use crate::heap::{blk, desc_of, rd};
use crate::state::*;
use crate::threads::{ROOTS, THREADS};
use crate::types::*;
use crate::{
    kelp_gc_alloc, kelp_gc_alloc_array, kelp_gc_collect, kelp_gc_init, kelp_gc_is_heap_ptr,
    kelp_gc_register_root_region, kelp_gc_safepoint, kelp_gc_thread_attach, kelp_gc_thread_detach,
    kelp_gc_unregister_root_region,
};

// ---- Layout (the header contract with WP3). ----

#[test]
fn c_layout_matches_header() {
    assert_eq!(offset_of!(KelpGcType, id), 0);
    assert_eq!(offset_of!(KelpGcType, size), 4);
    assert_eq!(offset_of!(KelpGcType, align), 8);
    assert_eq!(offset_of!(KelpGcType, flags), 12);
    assert_eq!(offset_of!(KelpGcType, n_ptrs), 16);
    assert_eq!(offset_of!(KelpGcType, ptr_off), 24);
    assert_eq!(offset_of!(KelpGcType, tag_off), 32);
    assert_eq!(offset_of!(KelpGcType, n_variants), 36);
    assert_eq!(offset_of!(KelpGcType, variants), 40);
    assert_eq!(size_of::<KelpGcType>(), 48);
    assert_eq!(align_of::<KelpGcType>(), 8);
    assert_eq!(offset_of!(KelpGcVariant, n_ptrs), 0);
    assert_eq!(offset_of!(KelpGcVariant, ptr_off), 8);
    assert_eq!(size_of::<KelpGcVariant>(), 16);
}

// ---- Shared descriptors. ----

/// Two pointer slots at offsets 0 and 8, payload 16 bytes (a `datac Node` with two Box fields).
static NODE_OFF: [u32; 2] = [0, 8];
static NODE: KelpGcType = KelpGcType {
    id: 1,
    size: 16,
    align: 8,
    flags: 0,
    n_ptrs: 2,
    ptr_off: NODE_OFF.as_ptr(),
    tag_off: 0,
    n_variants: 0,
    variants: core::ptr::null(),
};

/// One pointer slot at offset 0, payload 16 bytes: next pointer plus a scalar at 8.
static LINK_OFF: [u32; 1] = [0];
static LINK: KelpGcType = KelpGcType {
    id: 2,
    size: 16,
    align: 8,
    flags: 0,
    n_ptrs: 1,
    ptr_off: LINK_OFF.as_ptr(),
    tag_off: 0,
    n_variants: 0,
    variants: core::ptr::null(),
};

/// A single pointer, used as a root region layout.
static ROOT_PTR: KelpGcType = KelpGcType {
    id: 3,
    size: 8,
    align: 8,
    flags: 0,
    n_ptrs: 1,
    ptr_off: LINK_OFF.as_ptr(),
    tag_off: 0,
    n_variants: 0,
    variants: core::ptr::null(),
};

/// Byte buffer: no pointer slots, never scanned.
static BYTES: KelpGcType = KelpGcType {
    id: 4,
    size: 1,
    align: 1,
    flags: DESC_NOSCAN,
    n_ptrs: 0,
    ptr_off: core::ptr::null(),
    tag_off: 0,
    n_variants: 0,
    variants: core::ptr::null(),
};

struct Slot<T>(UnsafeCell<T>);
unsafe impl<T> Sync for Slot<T> {}

/// Stores garbage addresses for the cycle test. Not a root: it is a static.
static GARBAGE: Slot<[usize; 2]> = Slot(UnsafeCell::new([0; 2]));
/// Root slot for the cycle test.
static ROOT_SLOT: Slot<usize> = Slot(UnsafeCell::new(0));

// ---- Harness. ----

static LOCK: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Clear all heap, type, thread and root state. Caller holds the test lock.
unsafe fn reset() {
    kelp_gc_init(0, 0);
    let s = st();
    core::ptr::write_bytes(s.blocks as *mut u8, 0, s.nblocks * size_of::<crate::state::BlockMeta>());
    core::ptr::write_bytes(s.types as *mut u8, 0, TYPE_TABLE_IDS * 8);
    core::ptr::write_bytes(THREADS.get() as *mut u8, 0, size_of::<[crate::threads::ThreadRec; MAX_THREADS]>());
    core::ptr::write_bytes(ROOTS.get() as *mut u8, 0, size_of::<[crate::threads::RootRec; MAX_ROOTS]>());
    s.used_blocks = 0;
    s.peak_blocks = 0;
    s.bytes_since = 0;
    s.trigger = s.heap_min;
    s.live = 0;
    s.alloc_block = 0;
    s.alloc_line = 0;
    s.cursor = 0;
    s.limit = 0;
    s.stress_n = 0;
    s.collecting = false;
    s.epoch = 0;
    s.mark_len = 0;
    s.cycles = 0;
    REQUEST.store(0, core::sync::atomic::Ordering::SeqCst);
}

#[inline(never)]
fn clobber_stack() {
    let mut buf = [0usize; 1024];
    black_box(&mut buf);
}

// ---- Tests. ----

#[test]
fn type_id_round_trips_through_header_and_descriptor() {
    let _g = serial();
    unsafe {
        reset();
        kelp_gc_thread_attach();
        let p = kelp_gc_alloc(&NODE) as usize;
        let hdr = rd(p - 8);
        let id = ((hdr >> HDR_TYPE_SHIFT) & 0xFF_FFFF) as u32;
        assert_eq!(id, NODE.id, "header type id must equal the descriptor id");
        assert_eq!(desc_of(id), &NODE as *const KelpGcType, "id must map back to the descriptor");
        assert_eq!(hdr & HDR_ARRAY, 0);
        assert_eq!(hdr & HDR_LARGE, 0);
        // Zeroed payload.
        assert_eq!(rd(p), 0);
        assert_eq!(rd(p + 8), 0);
        let a = kelp_gc_alloc_array(&BYTES, 37) as usize;
        let ahdr = rd(a - 8);
        assert_ne!(ahdr & HDR_ARRAY, 0);
        assert_ne!(ahdr & HDR_NOSCAN, 0, "byte arrays carry the noscan bit");
        assert_eq!(ahdr >> HDR_COUNT_SHIFT, 37);
        kelp_gc_thread_detach();
    }
}

#[test]
fn every_payload_is_eight_byte_aligned() {
    let _g = serial();
    unsafe {
        reset();
        kelp_gc_thread_attach();
        let mut count = 0usize;
        for round in 0..200 {
            let p = kelp_gc_alloc_array(&BYTES, round % 97) as usize;
            assert_eq!(p % 8, 0, "array payload misaligned");
            let q = kelp_gc_alloc(&NODE) as usize;
            assert_eq!(q % 8, 0, "fixed payload misaligned");
            count += 2;
        }
        // Large objects take whole blocks; their payloads must align too.
        for n in [3000usize, 9000, 40_000, 70_000] {
            let big = kelp_gc_alloc_array(&BYTES, n) as usize;
            assert_eq!(big % 8, 0, "large payload misaligned");
            count += 1;
        }
        assert!(count > 0);
        kelp_gc_thread_detach();
    }
}

#[test]
fn churn_ten_million_sixteen_byte_cells_keeps_heap_bounded() {
    let _g = serial();
    unsafe {
        reset();
        kelp_gc_thread_attach();
        const N: usize = 10_000_000;
        for i in 0..N {
            let p = kelp_gc_alloc(&NODE) as usize;
            assert_eq!(p % 8, 0);
            wr_scalar(p, i);
        }
        let s = st();
        let peak_bytes = s.peak_blocks * BLOCK;
        eprintln!(
            "churn: {N} allocations of a 16-byte payload (24 bytes with header); cycles {}; peak heap {} KiB ({} blocks)",
            s.cycles,
            peak_bytes / 1024,
            s.peak_blocks
        );
        assert!(s.cycles >= 10, "expected many cycles, got {}", s.cycles);
        assert!(
            peak_bytes <= 32 << 20,
            "peak heap {} bytes exceeds the 32 MiB bound",
            peak_bytes
        );
        kelp_gc_thread_detach();
    }
}

unsafe fn wr_scalar(p: usize, v: usize) {
    core::ptr::write((p + 8) as *mut usize, v);
}

#[inline(never)]
unsafe fn build_cycles() {
    // Live cycle a -> b -> c -> a, rooted through ROOT_SLOT.
    let a = kelp_gc_alloc(&NODE) as usize;
    let b = kelp_gc_alloc(&NODE) as usize;
    let c = kelp_gc_alloc(&NODE) as usize;
    core::ptr::write(a as *mut usize, b);
    core::ptr::write(b as *mut usize, c);
    core::ptr::write(c as *mut usize, a);
    *ROOT_SLOT.0.get() = a;

    // Garbage cycle d -> e -> d, unreachable.
    let d = kelp_gc_alloc(&NODE) as usize;
    let e = kelp_gc_alloc(&NODE) as usize;
    core::ptr::write(d as *mut usize, e);
    core::ptr::write(e as *mut usize, d);
    (*GARBAGE.0.get())[0] = d;
    (*GARBAGE.0.get())[1] = e;
}

#[test]
fn mark_keeps_rooted_cycle_and_frees_garbage_cycle() {
    let _g = serial();
    unsafe {
        reset();
        kelp_gc_thread_attach();
        kelp_gc_register_root_region(ROOT_SLOT.0.get() as *mut u8, &ROOT_PTR);
        build_cycles();
        clobber_stack();
        kelp_gc_collect();
        let a = *ROOT_SLOT.0.get();
        let b = core::ptr::read(a as *const usize);
        let c = core::ptr::read(b as *const usize);
        assert_eq!(core::ptr::read(c as *const usize), a, "rooted cycle intact");
        assert_eq!(kelp_gc_is_heap_ptr(a as *const u8), 1);
        assert_eq!(kelp_gc_is_heap_ptr(b as *const u8), 1);
        assert_eq!(kelp_gc_is_heap_ptr(c as *const u8), 1);
        // Interior pointers resolve to their object too.
        assert_eq!(kelp_gc_is_heap_ptr((a + 8) as *const u8), 1);
        let [d, e] = *GARBAGE.0.get();
        assert_eq!(kelp_gc_is_heap_ptr(d as *const u8), 0, "garbage cycle node d freed");
        assert_eq!(kelp_gc_is_heap_ptr(e as *const u8), 0, "garbage cycle node e freed");
        assert_eq!(st().live, 3 * 24, "exactly the three rooted nodes survive");
        kelp_gc_unregister_root_region(ROOT_SLOT.0.get() as *mut u8);
        kelp_gc_thread_detach();
    }
}

/// Child half of the cap test. Runs only when the parent sets the variable.
/// Keeps every node live, so the heap must hit its 256 MiB cap.
#[test]
fn cap_child() {
    if std::env::var_os("KELP_GC_CAP_CHILD").is_none() {
        return;
    }
    unsafe {
        kelp_gc_init(0, 0);
        kelp_gc_thread_attach();
        kelp_gc_register_root_region(ROOT_SLOT.0.get() as *mut u8, &ROOT_PTR);
        loop {
            let n = kelp_gc_alloc(&LINK) as usize;
            core::ptr::write(n as *mut usize, *ROOT_SLOT.0.get());
            *ROOT_SLOT.0.get() = n;
        }
    }
}

#[test]
fn heap_cap_of_256_mib_fails_with_out_of_memory() {
    if std::env::var_os("KELP_GC_CAP_CHILD").is_some() {
        return;
    }
    let exe = std::env::current_exe().expect("test binary path");
    let out = std::process::Command::new(exe)
        .args(["--exact", "tests::cap_child", "--nocapture", "--test-threads=1"])
        .env("KELP_GC_CAP_CHILD", "1")
        .output()
        .expect("spawn child");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "child must fail at the cap");
    assert!(
        stderr.contains("panic: out of memory"),
        "expected 'panic: out of memory' in stderr, got: {stderr}"
    );
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn heap_region_is_exactly_the_default_cap() {
    let _g = serial();
    unsafe {
        reset();
        assert_eq!(st().region_bytes, DEFAULT_HEAP_MAX);
        assert_eq!(st().nblocks, DEFAULT_HEAP_MAX / BLOCK);
        assert_eq!((*blk(0)).state, 0);
    }
}

/// Threads allocate and keep a short live list on their own stacks, poll
/// safepoints, and enter native state. The main thread collects repeatedly.
#[test]
fn threads_stop_the_world_and_keep_their_roots() {
    let _g = serial();
    unsafe {
        reset();
        kelp_gc_thread_attach();
    }
    let mut handles = Vec::new();
    for t in 0..4usize {
        handles.push(std::thread::spawn(move || unsafe {
            kelp_gc_thread_attach();
            let mut head: usize = 0;
            let mut ok = true;
            for i in 0..150_000usize {
                kelp_gc_safepoint();
                let n = kelp_gc_alloc(&LINK) as usize;
                core::ptr::write((n + 8) as *mut usize, i);
                core::ptr::write(n as *mut usize, head);
                head = n;
                if i % 1000 == 999 {
                    // Walk the list: values must be consecutive and decreasing.
                    let mut p = head;
                    let mut expect = i;
                    let mut seen = 0;
                    while p != 0 && seen < 1000 {
                        let v = core::ptr::read((p + 8) as *const usize);
                        if v != expect {
                            ok = false;
                            break;
                        }
                        if expect == 0 {
                            break;
                        }
                        expect -= 1;
                        p = core::ptr::read(p as *const usize);
                        seen += 1;
                    }
                    head = 0;
                }
                if i % 5000 == 0 {
                    kelp_gc_enter_native_test();
                    std::thread::sleep(std::time::Duration::from_millis(1));
                    kelp_gc_leave_native_test();
                }
            }
            black_box(t);
            kelp_gc_thread_detach();
            ok
        }));
    }
    unsafe {
        for _ in 0..30 {
            kelp_gc_collect();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
    let all_ok = handles.into_iter().all(|h| h.join().expect("worker"));
    unsafe {
        let s = st();
        eprintln!("threads: cycles {}", s.cycles);
        assert!(s.cycles >= 30);
        kelp_gc_thread_detach();
    }
    assert!(all_ok, "a thread lost or corrupted a rooted value across a collection");
}

unsafe fn kelp_gc_enter_native_test() {
    crate::kelp_gc_enter_native();
}
unsafe fn kelp_gc_leave_native_test() {
    crate::kelp_gc_leave_native();
}
