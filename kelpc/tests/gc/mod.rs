//! Collector tests. Included from `src/lib.rs` under `cfg(test)`; run by
//! `cargo test` inside `kelpc/gc`. The collector is process-global, so tests
//! take a lock and reset the heap first. Every test runs the heap verifier
//! after each collection (`reset` turns it on). `KELP_GC_STRESS=N` makes the
//! whole suite collect every N allocations.
//!
//! Failure paths (out of memory, the panic hook, an unattached allocation, a
//! thread that never stops, a type id over the limit, a corrupt heap) exit the
//! process, so each one runs in a child process: `child_entry` reads the mode
//! from KELP_GC_CHILD and the parent checks the exit status and the message.
#![allow(unsafe_op_in_unsafe_fn)]

use core::cell::UnsafeCell;
use core::hint::black_box;
use core::mem::{align_of, offset_of, size_of};
use core::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Barrier, Mutex, MutexGuard};

use crate::heap::{blk, desc_of, find_object, object_total, rd, total_for, FREE};
use crate::state::*;
use crate::threads::THREADS;
use crate::types::*;
use crate::{
    env_usize, kelp_gc_alloc, kelp_gc_alloc_array, kelp_gc_collect, kelp_gc_enter_native,
    kelp_gc_init, kelp_gc_is_heap_ptr, kelp_gc_leave_native, kelp_gc_register_root_region,
    kelp_gc_safepoint, kelp_gc_set_panic_hook, kelp_gc_thread_attach, kelp_gc_thread_detach,
    kelp_gc_unregister_root_region, kelp_gc_stats, KelpGcStats,
};

// ---- Layout (the ABI contract with WP3). ----

#[test]
fn c_layout_matches_the_abi() {
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

/// The limits the C side reads, pinned as literals. A change
/// to any of these is a change to the managed ABI and must be made here on purpose.
#[test]
fn abi_limits_are_pinned() {
    assert_eq!(TYPE_ID_MAX, 16_777_215, "type ids are 24 bits");
    assert_eq!(TYPE_TABLE_IDS, 1_048_576, "the v1 type table limit");
    assert_eq!(KELP_GC_ENUM_TAG_BYTES, 4, "enum tag width");
    assert_eq!(LARGE_MIN, 8192, "the large-object threshold");
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

/// The largest id the v1 table accepts. Used by the type-id limit tests.
static TOP_ID: KelpGcType = KelpGcType {
    id: (TYPE_TABLE_IDS - 1) as u32,
    size: 16,
    align: 8,
    flags: 0,
    n_ptrs: 1,
    ptr_off: LINK_OFF.as_ptr(),
    tag_off: 0,
    n_variants: 0,
    variants: core::ptr::null(),
};

/// One id past the v1 table: the header can hold it, the table cannot.
static OVER_ID: KelpGcType = KelpGcType {
    id: TYPE_TABLE_IDS as u32,
    size: 16,
    align: 8,
    flags: 0,
    n_ptrs: 1,
    ptr_off: LINK_OFF.as_ptr(),
    tag_off: 0,
    n_variants: 0,
    variants: core::ptr::null(),
};

/// An enum with two variants. Variant 0 holds one pointer at offset 0; variant
/// 1 holds none. The tag is a u32 at offset 8 (KELP_GC_ENUM_TAG_BYTES).
static ENUM_OFF: [u32; 1] = [0];
static ENUM_VARIANTS: [KelpGcVariant; 2] = [
    KelpGcVariant { n_ptrs: 1, ptr_off: ENUM_OFF.as_ptr() },
    KelpGcVariant { n_ptrs: 0, ptr_off: core::ptr::null() },
];
static ENUM: KelpGcType = KelpGcType {
    id: 5,
    size: 16,
    align: 8,
    flags: DESC_ENUM,
    n_ptrs: 0,
    ptr_off: core::ptr::null(),
    tag_off: 8,
    n_variants: 2,
    variants: ENUM_VARIANTS.as_ptr(),
};

struct Slot<T>(UnsafeCell<T>);
unsafe impl<T> Sync for Slot<T> {}

/// Stores garbage addresses for the cycle test. Not a root: it is a static.
static GARBAGE: Slot<[usize; 2]> = Slot(UnsafeCell::new([0; 2]));
/// Root slot for the cycle test.
static ROOT_SLOT: Slot<usize> = Slot(UnsafeCell::new(0));
/// Root slots for the enum test.
static ENUM_ROOTS: Slot<[usize; 2]> = Slot(UnsafeCell::new([0; 2]));

// ---- Harness. ----

static LOCK: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Clear all heap, type, thread, root and large-table state. Caller holds the
/// test lock. The stress setting comes from the environment, so the whole
/// suite can run under KELP_GC_STRESS; the verifier is always on.
unsafe fn reset() {
    kelp_gc_init(0, 0, 0, 0, 20);
    let s = st();
    core::ptr::write_bytes(s.blocks as *mut u8, 0, s.nblocks * size_of::<BlockMeta>());
    core::ptr::write_bytes(s.types as *mut u8, 0, TYPE_TABLE_IDS * 8);
    core::ptr::write_bytes(s.large as *mut u8, 0, s.large_cap * 8);
    core::ptr::write_bytes(THREADS.get() as *mut u8, 0, size_of::<[crate::threads::ThreadRec; MAX_THREADS]>());
    crate::threads::reset_roots();
    REGIONS.lock().unwrap().clear();
    s.large_n = 0;
    s.large_sorted = true;
    s.used_blocks = 0;
    s.peak_blocks = 0;
    s.bytes_since = 0;
    s.trigger = s.heap_min;
    s.live = 0;
    s.alloc_block = 0;
    s.alloc_line = 0;
    s.cursor = 0;
    s.limit = 0;
    s.run_cur = 0;
    s.run_end = 0;
    s.run_scan = 0;
    s.stress_n = env_usize(b"KELP_GC_STRESS\0").unwrap_or(0);
    s.stress_count = 0;
    s.verify = true;
    s.collecting = false;
    s.epoch = 0;
    s.mark_len = 0;
    s.cycles = 0;
    s.allocs = 0;
    s.pause_total_ns = 0;
    s.pause_max_ns = 0;
    s.mark_ns_total = 0;
    s.sweep_ns_total = 0;
    s.scan_stacks = true;
    REQUEST.store(0, SeqCst);
}

#[inline(never)]
fn clobber_stack() {
    let mut buf = [0usize; 1024];
    black_box(&mut buf);
}

/// Run this test binary again, filtered to `child_entry`, in the given mode.
/// Returns the exit code and the child's stderr.
fn run_child(mode: &str, env: &[(&str, &str)]) -> (Option<i32>, String) {
    let exe = std::env::current_exe().expect("test binary path");
    let mut cmd = Command::new(exe);
    cmd.args(["--exact", "tests::child_entry", "--nocapture", "--test-threads=1"])
        .env("KELP_GC_CHILD", mode);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("spawn child");
    (out.status.code(), String::from_utf8_lossy(&out.stderr).into_owned())
}

/// Child-process bodies. Each mode ends in a fatal exit, except the ones that
/// return normally. Runs only when the parent sets KELP_GC_CHILD.
#[test]
fn child_entry() {
    let Some(mode) = std::env::var_os("KELP_GC_CHILD") else {
        return;
    };
    let mode = mode.to_string_lossy().into_owned();
    unsafe {
        match mode.as_str() {
            "cap" => {
                kelp_gc_init(0, 0, 0, 0, 20);
                kelp_gc_thread_attach();
                fill_until_full();
            }
            "hook" => {
                kelp_gc_init(0, 8 << 20, 0, 0, 20);
                kelp_gc_set_panic_hook(Some(hook_exit_3));
                kelp_gc_thread_attach();
                fill_until_full();
            }
            "unattached" => {
                kelp_gc_init(0, 0, 0, 0, 20);
                black_box(kelp_gc_alloc(&NODE));
            }
            "type_over_limit" => {
                kelp_gc_init(0, 0, 0, 0, 20);
                kelp_gc_thread_attach();
                black_box(kelp_gc_alloc(&OVER_ID));
            }
            "stop_timeout" => child_stop_timeout(),
            "verify_mark" => child_verify_mark(),
            "root_full" => child_root_full(),
            "dup_id" => child_dup_id(),
            "array_too_large" => {
                kelp_gc_init(0, 0, 0, 0, 20);
                kelp_gc_thread_attach();
                black_box(kelp_gc_alloc_array(&NODE, u32::MAX as usize + 1));
            }
            "noscan_with_pointers" => tidepool_step1_fixb::child_noscan_with_pointers(),
            "array_overflows" => tidepool_step1_fixb::child_array_overflows(),
            "foreign_thread" => tidepool_step1_fixb::child_foreign_thread(),
            "first_clear" => child_first_clear(),
            "panic_location" => crate::sys::report_and_abort("heap.rs", 42),
            other => panic!("unknown child mode {other}"),
        }
    }
}

/// Tidepool step 2, T40e: the first clearing happens at the trigger the
/// manifest sets. The start guesses are 1 and 2 MiB at headroom 20, so the
/// first trigger is 1.8 MiB (not heap_min's 8 MiB). The clearing must come at
/// the allocation where `bytes_since` first reaches the trigger, so the bytes
/// before the previous allocation are below it and the bytes before this one
/// are at or above it. Exit 0 when that holds, 5 when it does not.
unsafe fn child_first_clear() {
    const TRIGGER: usize = 1_887_436;
    kelp_gc_init(0, 0, 1 << 20, 2 << 20, 20);
    kelp_gc_thread_attach();
    let mut prev = 0usize;
    for _ in 0..10_000_000u32 {
        let before = crate::state::st().bytes_since;
        let cycles = crate::state::st().cycles;
        black_box(kelp_gc_alloc(&NODE));
        if crate::state::st().cycles > cycles {
            if before >= TRIGGER && prev < TRIGGER {
                std::process::exit(0);
            }
            std::process::exit(5);
        }
        prev = before;
    }
    std::process::exit(6);
}

/// Keep every node live until the heap cap is reached.
unsafe fn fill_until_full() {
    root(ROOT_SLOT.0.get() as *mut u8, &ROOT_PTR);
    loop {
        let n = kelp_gc_alloc(&LINK) as usize;
        core::ptr::write(n as *mut usize, *ROOT_SLOT.0.get());
        *ROOT_SLOT.0.get() = n;
    }
}

unsafe extern "C" fn hook_exit_3(msg: *const u8) {
    let text = std::ffi::CStr::from_ptr(msg as *const core::ffi::c_char);
    eprintln!("hooked: {}", text.to_string_lossy());
    std::process::exit(3);
}

static CHILD_READY: AtomicBool = AtomicBool::new(false);

/// A worker attaches and spins without polling. The collector must give up
/// after the timeout and name the thread.
unsafe fn child_stop_timeout() {
    kelp_gc_init(0, 0, 0, 0, 20);
    kelp_gc_thread_attach();
    std::thread::spawn(|| unsafe {
        kelp_gc_thread_attach();
        CHILD_READY.store(true, SeqCst);
        loop {
            black_box(0usize);
        }
    });
    while !CHILD_READY.load(SeqCst) {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    kelp_gc_collect();
}

/// A survivor whose mark bit is set after its cycle must fail the verifier.
unsafe fn child_verify_mark() {
    kelp_gc_init(0, 0, 0, 0, 20);
    kelp_gc_thread_attach();
    root(ROOT_SLOT.0.get() as *mut u8, &ROOT_PTR);
    let p = kelp_gc_alloc(&LINK) as usize;
    *ROOT_SLOT.0.get() = p;
    kelp_gc_collect();
    let s = st();
    let hdr = p - 8;
    crate::heap::wr(hdr, rd(hdr) & !HDR_MARK | s.epoch as u64);
    crate::verify::heap_check();
}

/// Spawn a child in `mode` with a stop timeout and return its status and stderr.
fn child_result(mode: &str, env: &[(&str, &str)]) -> (Option<i32>, String) {
    run_child(mode, env)
}

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

/// Item 5: ids up to the v1 limit work; the limit is the table size.
#[test]
fn type_ids_up_to_the_v1_limit_round_trip() {
    let _g = serial();
    unsafe {
        reset();
        assert_eq!(TYPE_TABLE_IDS, 1 << 20, "v1 table holds ids below 2^20");
        assert!(TYPE_ID_MAX >= (TYPE_TABLE_IDS as u32) - 1);
        kelp_gc_thread_attach();
        let p = kelp_gc_alloc(&TOP_ID) as usize;
        let hdr = rd(p - 8);
        assert_eq!(((hdr >> HDR_TYPE_SHIFT) & 0xFF_FFFF) as u32, TOP_ID.id);
        assert_eq!(desc_of(TOP_ID.id), &TOP_ID as *const KelpGcType);
        kelp_gc_thread_detach();
    }
}

/// Item 5: one id past the v1 table fails with a message that names the limit.
#[test]
fn type_id_over_the_v1_limit_fails_loudly() {
    let (code, stderr) = child_result("type_over_limit", &[]);
    assert_eq!(code, Some(1), "stderr: {stderr}");
    assert!(
        stderr.contains("panic: gc: type id 1048576 is over the v1 type table limit (ids 0 to 1048575 are accepted)"),
        "stderr: {stderr}"
    );
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
        // Run-space objects must align too.
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
            // LINK, not NODE: wr_scalar writes offset 8, which is a pointer slot of NODE.
            let p = kelp_gc_alloc(&LINK) as usize;
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
        let h = root(ROOT_SLOT.0.get() as *mut u8, &ROOT_PTR);
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
        assert_eq!(kelp_gc_unregister_root_region(h), KELP_GC_ROOT_OK);
        kelp_gc_thread_detach();
    }
}

/// Two enum roots: `a` (variant 0, pointing at `x`) and `b` (variant 1, with a
/// stale pointer `y` in its payload that must not be traced). Nothing is
/// returned, so the caller's frame holds no pointer to `x` or `y`.
#[inline(never)]
unsafe fn build_enum_roots() {
    let x = kelp_gc_alloc(&LINK) as usize;
    let y = kelp_gc_alloc(&LINK) as usize;
    let a = kelp_gc_alloc(&ENUM) as usize;
    let b = kelp_gc_alloc(&ENUM) as usize;
    core::ptr::write(a as *mut usize, x);
    core::ptr::write((a + 8) as *mut u32, 0);
    core::ptr::write(b as *mut usize, y);
    core::ptr::write((b + 8) as *mut u32, 1);
    (*ENUM_ROOTS.0.get())[0] = a;
    (*ENUM_ROOTS.0.get())[1] = b;
}

/// Item 6: the enum tag is a u32 at `tag_off`. Variant 0 keeps its pointer
/// alive; variant 1 has no pointer slots, so its pointer is not traced.
/// Addresses the enum test expects to be live or dead after a collection. They
/// live in statics, which are not scanned, so the test frame holds no copy.
static ENUM_TRACED: Slot<usize> = Slot(UnsafeCell::new(0));
static ENUM_UNTRACED: Slot<usize> = Slot(UnsafeCell::new(0));

/// Builds the enum fixture in its own frame, so no pointer local outlives it.
#[inline(never)]
unsafe fn enum_fixture() {
    let x = kelp_gc_alloc(&LINK) as usize;
    let y = kelp_gc_alloc(&LINK) as usize;
    let a = kelp_gc_alloc(&ENUM) as usize;
    let b = kelp_gc_alloc(&ENUM) as usize;
    core::ptr::write(a as *mut usize, x);
    core::ptr::write((a + 8) as *mut u32, 0);
    core::ptr::write(b as *mut usize, y);
    core::ptr::write((b + 8) as *mut u32, 1);
    (*ENUM_ROOTS.0.get())[0] = a;
    (*ENUM_ROOTS.0.get())[1] = b;
    *ENUM_TRACED.0.get() = x;
    *ENUM_UNTRACED.0.get() = y;
}

/// Every shared object is marked once: a DAG where each node is named by two
/// parents must count each node's bytes exactly once in the cycle's live total.
#[inline(never)]
unsafe fn build_shared_chain(n: usize) -> usize {
    let mut next = 0usize;
    for _ in 0..n {
        let node = kelp_gc_alloc(&NODE) as usize;
        core::ptr::write(node as *mut usize, next);
        core::ptr::write((node + 8) as *mut usize, next);
        next = node;
    }
    next
}

#[test]
fn mark_counts_each_shared_object_once() {
    let _g = serial();
    unsafe {
        reset();
        kelp_gc_thread_attach();
        const N: usize = 4000;
        register_region(ROOT_SLOT.0.get() as *mut u8, &ROOT_PTR);
        *ROOT_SLOT.0.get() = build_shared_chain(N);
        clobber_stack();
        kelp_gc_collect();
        let live = st().live;
        assert_eq!(live, N * total_for(16), "each shared node marked and counted once");
        let mut p = *ROOT_SLOT.0.get();
        let mut count = 0usize;
        while p != 0 {
            assert_eq!(kelp_gc_is_heap_ptr(p as *const u8), 1, "chain node survives");
            count += 1;
            p = core::ptr::read(p as *const usize);
        }
        assert_eq!(count, N, "whole chain intact");
        unregister_region(ROOT_SLOT.0.get() as *mut u8);
        kelp_gc_thread_detach();
    }
}

/// Two garbage nodes, kept in statics so no test-frame local roots them.
static DEAD_NODES: Slot<[usize; 2]> = Slot(UnsafeCell::new([0; 2]));

#[inline(never)]
unsafe fn build_interleaved(n: usize) -> usize {
    let mut all = Vec::with_capacity(n);
    for _ in 0..n {
        all.push(kelp_gc_alloc(&LINK) as usize);
    }
    // Live chain through the even nodes only.
    let mut head = 0usize;
    let mut i = n;
    while i > 0 {
        i -= 1;
        if i % 2 == 0 {
            core::ptr::write(all[i] as *mut usize, head);
            head = all[i];
        }
    }
    *DEAD_NODES.0.get() = [all[1], all[n - 1]];
    head
}

/// Sweep invariant: in a mixed block only the dead objects lose their start
/// bits; live neighbours keep theirs. Half of a 512-node list is rooted, the
/// other half is garbage interleaved with it.
#[test]
fn sweep_clears_only_dead_starts_in_mixed_blocks() {
    let _g = serial();
    unsafe {
        reset();
        kelp_gc_thread_attach();
        register_region(ROOT_SLOT.0.get() as *mut u8, &ROOT_PTR);
        const N: usize = 512;
        let head = build_interleaved(N);
        *ROOT_SLOT.0.get() = head;
        clobber_stack();
        kelp_gc_collect();
        // Live: every even node reachable from the head.
        let mut p = head;
        let mut live = 0usize;
        while p != 0 {
            assert_eq!(kelp_gc_is_heap_ptr(p as *const u8), 1, "live node keeps its start");
            live += 1;
            p = core::ptr::read(p as *const usize);
        }
        assert_eq!(live, N / 2, "every even node is on the chain");
        for d in *DEAD_NODES.0.get() {
            assert_eq!(kelp_gc_is_heap_ptr(d as *const u8), 0, "dead node loses its start");
        }
        unregister_region(ROOT_SLOT.0.get() as *mut u8);
        kelp_gc_thread_detach();
    }
}

/// Sweep invariant: when nothing is reachable, every block is freed; no block
/// stays ACTIVE with stale start bits or live lines.
#[inline(never)]
unsafe fn churn_garbage(n: usize) {
    for _ in 0..n {
        black_box(kelp_gc_alloc(&LINK));
    }
}

#[test]
fn sweep_frees_every_empty_block_without_residue() {
    let _g = serial();
    unsafe {
        reset();
        kelp_gc_thread_attach();
        churn_garbage(200_000);
        clobber_stack();
        kelp_gc_collect();
        let s = st();
        assert_eq!(s.used_blocks, 0, "no block stays in use with nothing reachable");
        assert_eq!(s.live, 0, "nothing marked");
        for b in 0..s.nblocks {
            let m = blk(b);
            assert_eq!((*m).state, FREE, "block {b} is free");
            assert!((*m).starts.iter().all(|&w| w == 0), "block {b} has no start bits");
        }
        kelp_gc_thread_detach();
    }
}

#[test]
fn enum_tag_is_read_as_u32_and_selects_the_variant() {
    let _g = serial();
    unsafe {
        reset();
        assert_eq!(KELP_GC_ENUM_TAG_BYTES, 4);
        kelp_gc_thread_attach();
        let h0 = root(ENUM_ROOTS.0.get() as *mut u8, &ROOT_PTR);
        let h1 = root((ENUM_ROOTS.0.get() as *mut usize).add(1) as *mut u8, &ROOT_PTR);
        build_enum_roots();
        clobber_stack();
        kelp_gc_collect();
        let a = (*ENUM_ROOTS.0.get())[0];
        let b = (*ENUM_ROOTS.0.get())[1];
        let x = core::ptr::read(a as *const usize);
        let y = core::ptr::read(b as *const usize);
        assert_eq!(kelp_gc_is_heap_ptr(x as *const u8), 1, "variant 0 pointer traced");
        assert_eq!(kelp_gc_is_heap_ptr(y as *const u8), 0, "variant 1 pointer not traced");
        assert_eq!(kelp_gc_unregister_root_region(h0), KELP_GC_ROOT_OK);
        assert_eq!(kelp_gc_unregister_root_region(h1), KELP_GC_ROOT_OK);
        kelp_gc_thread_detach();
    }
}

#[test]
fn heap_cap_of_256_mib_fails_with_out_of_memory() {
    let (code, stderr) = child_result("cap", &[]);
    assert_eq!(code, Some(1), "stderr: {stderr}");
    assert!(stderr.contains("panic: out of shared-pool memory: heap_max is 256 MiB, "), "got: {stderr}");
}

/// Item 7: the installed hook receives the message and its exit status is kept.
#[test]
fn panic_hook_receives_the_out_of_memory_message() {
    let (code, stderr) = child_result("hook", &[]);
    assert_eq!(code, Some(3), "the hook exits 3; stderr: {stderr}");
    assert!(stderr.contains("hooked: out of shared-pool memory: heap_max is 8 MiB, "), "stderr: {stderr}");
    assert!(!stderr.contains("panic: out of shared-pool memory"), "default text must not print when a hook is set");
}

/// Item 1: an allocation on a thread that never attached fails loudly.
#[test]
fn allocation_without_attach_fails_loudly() {
    let (code, stderr) = child_result("unattached", &[]);
    assert_eq!(code, Some(1), "stderr: {stderr}");
    assert!(
        stderr.contains("allocation on a thread that is not attached"),
        "stderr: {stderr}"
    );
}

/// Item 2: a thread that never reaches a safepoint is named after the timeout.
#[test]
fn stop_timeout_names_the_thread_that_did_not_stop() {
    let (code, stderr) = child_result("stop_timeout", &[("KELP_GC_STOP_TIMEOUT_MS", "300")]);
    assert_eq!(code, Some(1), "stderr: {stderr}");
    assert!(
        stderr.contains("gc: thread 0x") && stderr.contains("did not reach a safepoint within 300 ms"),
        "stderr: {stderr}"
    );
}

/// Item 8: the verifier fails when a survivor carries a mark bit into the next cycle.
#[test]
fn verifier_rejects_a_mark_bit_left_set() {
    let (code, stderr) = child_result("verify_mark", &[]);
    assert_eq!(code, Some(1), "stderr: {stderr}");
    assert!(stderr.contains("gc: heap verify failed: mark bit set after the sweep"), "stderr: {stderr}");
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

/// Item 1: a thread's only reference to an object is a local on its stack
/// (and a register) while it sits in native state. The collector must find it.
#[test]
fn stack_only_object_of_a_native_thread_survives_collections() {
    let _g = serial();
    unsafe {
        reset();
        kelp_gc_thread_attach();
    }
    static READY: AtomicBool = AtomicBool::new(false);
    static DONE: AtomicBool = AtomicBool::new(false);
    let worker = std::thread::spawn(|| unsafe {
        kelp_gc_thread_attach();
        // LINK: offset 8 is a scalar there, not a pointer slot.
        let p = kelp_gc_alloc(&LINK) as usize;
        core::ptr::write((p + 8) as *mut usize, 4242);
        READY.store(true, SeqCst);
        kelp_gc_enter_native();
        while !DONE.load(SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        kelp_gc_leave_native();
        let ok = core::ptr::read((p + 8) as *const usize) == 4242;
        kelp_gc_thread_detach();
        ok
    });
    while !READY.load(SeqCst) {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    unsafe {
        for _ in 0..3 {
            clobber_stack();
            kelp_gc_collect();
        }
        assert!(st().live >= 24, "the native thread's object is live");
    }
    DONE.store(true, SeqCst);
    assert!(worker.join().expect("worker"), "stack-only object was lost");
    unsafe { kelp_gc_thread_detach() };
}

/// Exit the test binary if `what` has not finished within `secs`. A stop-the-
/// world deadlock would otherwise hang the build queue until a person kills it.
fn watchdog(what: &'static str, secs: u64) {
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(secs));
        eprintln!("deadlock: {what} did not finish within {secs} s");
        std::process::exit(2);
    });
}

/// Threads allocate and keep a short live list on their own stacks, poll
/// safepoints, and enter native state. The main thread collects repeatedly.
/// A worker that detaches while a collection waits for it must wake the
/// collector (the lost-wakeup deadlock this test caught in gc-core).
#[test]
fn threads_stop_the_world_and_keep_their_roots() {
    let _g = serial();
    watchdog("threads_stop_the_world_and_keep_their_roots", 120);
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
                    kelp_gc_enter_native();
                    std::thread::sleep(std::time::Duration::from_millis(1));
                    kelp_gc_leave_native();
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
    // The main thread is attached, so it must be in native state while it
    // blocks on the workers; otherwise a worker's collection waits for it forever.
    unsafe { kelp_gc_enter_native() };
    let all_ok = handles.into_iter().all(|h| h.join().expect("worker"));
    unsafe { kelp_gc_leave_native() };
    unsafe {
        let s = st();
        eprintln!("threads: cycles {}", s.cycles);
        assert!(s.cycles >= 30);
        kelp_gc_thread_detach();
    }
    assert!(all_ok, "a thread lost or corrupted a rooted value across a collection");
}

/// Item 10: four threads build rings of cyclic nodes into precise root slots,
/// churn unrooted garbage (which triggers collections on the workers
/// themselves), and meet the collecting thread at a barrier while parked in
/// native state. After each explicit collection the live set must equal the
/// rings exactly, byte for byte, and every ring must still be intact.
#[test]
fn cyclic_graphs_across_four_threads_have_an_exact_live_set() {
    const THREADS_N: usize = 4;
    const RINGS: usize = 8;
    const RING_LEN: usize = 32;
    const ROUNDS: usize = 60;
    const GARBAGE: usize = 20_000;
    const NODE_TOTAL: usize = 24; // LINK: 16-byte payload plus the 8-byte header

    static HANDLES: Slot<[u64; THREADS_N * RINGS]> = Slot(UnsafeCell::new([0; THREADS_N * RINGS]));
    static SLOTS: [Slot<[usize; RINGS]>; THREADS_N] = [
        Slot(UnsafeCell::new([0; RINGS])),
        Slot(UnsafeCell::new([0; RINGS])),
        Slot(UnsafeCell::new([0; RINGS])),
        Slot(UnsafeCell::new([0; RINGS])),
    ];

    let _g = serial();
    unsafe {
        reset();
        for t in 0..THREADS_N {
            for k in 0..RINGS {
                let slot = (SLOTS[t].0.get() as *mut usize).add(k) as *mut u8;
                let h = root(slot, &ROOT_PTR);
                (*HANDLES.0.get())[t * RINGS + k] = h;
            }
        }
        kelp_gc_thread_attach();
    }

    let barrier = Arc::new(Barrier::new(THREADS_N + 1));
    let mut handles = Vec::new();
    for t in 0..THREADS_N {
        let barrier = Arc::clone(&barrier);
        handles.push(std::thread::spawn(move || unsafe {
            kelp_gc_thread_attach();
            for _ in 0..ROUNDS {
                let slots = SLOTS[t].0.get() as *mut usize;
                for k in 0..RINGS {
                    *slots.add(k) = build_ring(RING_LEN);
                }
                churn(GARBAGE);
                kelp_gc_enter_native();
                barrier.wait(); // all workers parked: the collector checks
                barrier.wait(); // the collector is done
                kelp_gc_leave_native();
            }
            kelp_gc_thread_detach();
        }));
    }

    let expected = THREADS_N * RINGS * RING_LEN * NODE_TOTAL;
    let mut checked = 0usize;
    unsafe {
        for _ in 0..ROUNDS {
            kelp_gc_enter_native();
            barrier.wait();
            kelp_gc_leave_native();
            // The rings are rooted precisely. Stale stack and register words
            // are not roots here, so the live set must equal the rings exactly.
            st().scan_stacks = false;
            kelp_gc_collect();
            st().scan_stacks = true;
            checked += check_round(&SLOTS.iter().map(|s| s.0.get() as *const usize).collect::<Vec<_>>(), RINGS, RING_LEN, expected);
            kelp_gc_enter_native();
            barrier.wait();
            kelp_gc_leave_native();
        }
    }
    unsafe { kelp_gc_enter_native() };
    for h in handles {
        h.join().expect("ring worker");
    }
    unsafe { kelp_gc_leave_native() };
    unsafe {
        eprintln!(
            "rings: {} collections checked, {} ring heads, {} cycles in total, {} allocations",
            ROUNDS,
            checked,
            st().cycles,
            st().allocs
        );
        assert_eq!(checked, ROUNDS * THREADS_N * RINGS);
        kelp_gc_thread_detach();
        for h in (*HANDLES.0.get()).iter().copied() {
            assert_eq!(kelp_gc_unregister_root_region(h), KELP_GC_ROOT_OK);
        }
    }
}

/// After an explicit collection: the live bytes must equal the rings, and every
/// ring head must be live and its ring intact. Returns the heads checked.
#[inline(never)]
unsafe fn check_round(slots: &[*const usize], rings: usize, len: usize, expected: usize) -> usize {
    assert_eq!(
        st().live,
        expected,
        "live set after collection must be exactly the {} rings",
        slots.len() * rings
    );
    let mut n = 0;
    for &row in slots {
        for k in 0..rings {
            let head = *row.add(k);
            assert_eq!(kelp_gc_is_heap_ptr(head as *const u8), 1, "ring head survives");
            check_ring(head, len);
            n += 1;
        }
    }
    n
}

/// A ring of `len` LINK nodes whose tags are 0..len. Only the first node's
/// address leaves this function, and only in the caller's root slot.
#[inline(never)]
unsafe fn build_ring(len: usize) -> usize {
    let first = kelp_gc_alloc(&LINK) as usize;
    core::ptr::write((first + 8) as *mut usize, 0);
    let mut prev = first;
    for i in 1..len {
        let n = kelp_gc_alloc(&LINK) as usize;
        core::ptr::write((n + 8) as *mut usize, i);
        core::ptr::write(prev as *mut usize, n);
        prev = n;
    }
    core::ptr::write(prev as *mut usize, first);
    first
}

/// Walk a ring from its head: tags must run 0..len and close back on the head.
#[inline(never)]
unsafe fn check_ring(head: usize, len: usize) {
    let mut p = head;
    for i in 0..len {
        assert_eq!(core::ptr::read((p + 8) as *const usize), i, "ring tag");
        p = core::ptr::read(p as *const usize);
    }
    assert_eq!(p, head, "ring closes on its head");
}

#[inline(never)]
unsafe fn churn(n: usize) {
    for _ in 0..n {
        black_box(kelp_gc_alloc(&LINK));
    }
}

// ---- Measurements (hardening items 3 and 4). They print numbers and assert
// only that the runs stay correct. ----

/// Churn `total_bytes` of `count`-byte arrays, all garbage, and report the
/// footprint waste: 1 - (bytes placed per cycle) / (peak blocks in use).
#[test]
fn measure_large_object_waste_4k_and_16k() {
    let _g = serial();
    for (label, count) in [("4 KiB", 4096usize), ("16 KiB", 16384usize)] {
        unsafe {
            reset();
            kelp_gc_thread_attach();
            let per = total_for(count);
            let n = (64usize << 20) / per;
            for _ in 0..n {
                black_box(kelp_gc_alloc_array(&BYTES, count));
            }
            let s = st();
            let cycles = s.cycles.max(1);
            let placed_per_cycle = (n * per) as f64 / cycles as f64;
            let footprint = (s.peak_blocks * BLOCK) as f64;
            let waste = 100.0 * (1.0 - placed_per_cycle / footprint);
            eprintln!(
                "waste {label}: {n} objects of {per} bytes; cycles {cycles}; peak {} blocks; waste {waste:.1} per cent",
                s.peak_blocks
            );
            kelp_gc_thread_detach();
        }
    }
}

/// Run `threads` workers that each allocate `per` nodes. Returns the wall time
/// in seconds, the allocation count and the contended heap-lock acquisitions.
unsafe fn churn_run(threads: usize, per: usize) -> (f64, usize, usize) {
    reset();
    CONTENDED.store(0, SeqCst);
    let start = std::time::Instant::now();
    let mut handles = Vec::new();
    for _ in 0..threads {
        handles.push(std::thread::spawn(move || {
            unsafe { kelp_gc_thread_attach() };
            for i in 0..per {
                let p = unsafe { kelp_gc_alloc(&LINK) } as usize;
                unsafe { wr_scalar(p, i) };
            }
            unsafe { kelp_gc_thread_detach() };
        }));
    }
    unsafe { kelp_gc_enter_native() };
    for h in handles {
        h.join().expect("churn worker");
    }
    unsafe { kelp_gc_leave_native() };
    let secs = start.elapsed().as_secs_f64();
    (secs, st().allocs, CONTENDED.load(SeqCst))
}

/// Four-thread churn against one thread: if the global heap lock were the
/// bottleneck, the four-thread rate would fall far below the single-thread rate.
#[test]
fn measure_heap_lock_contention_four_threads() {
    let _g = serial();
    unsafe {
        kelp_gc_init(0, 0, 0, 0, 20);
        let (t1, a1, c1) = churn_run(1, 1_000_000);
        let (t4, a4, c4) = churn_run(4, 1_000_000);
        let rate1 = a1 as f64 / t1 / 1e6;
        let rate4 = a4 as f64 / t4 / 1e6;
        eprintln!(
            "lock: 1 thread {rate1:.2} M alloc/s ({:.1} ns each), contended {c1} of {a1}; \
             4 threads {rate4:.2} M alloc/s aggregate ({:.1} ns each), contended {c4} of {a4} ({:.2} per cent); scaling {:.2}x",
            1000.0 / rate1,
            1000.0 / rate4,
            100.0 * c4 as f64 / a4 as f64,
            rate4 / rate1
        );
        assert_eq!(a4, 4_000_000, "every allocation counted");
    }
}

// ---- Allocation invariants (zeroing, reachability, no overlap). ----

/// Payload sizes for the mixed rounds. 16 to 3000 bytes use the line-hole
/// space; 9000 bytes (with its header) is above LARGE_MIN and uses the run space.
const MIX_BYTES: [usize; 6] = [16, 24, 100, 1000, 3000, 9000];

/// A root region of `n` pointer slots, zeroed and registered. The slots and
/// layout are leaked; the caller unregisters with `kelp_gc_unregister_root_region`.
unsafe fn register_pointer_roots(n: usize) -> *mut usize {
    let slots: &'static mut [usize] = Box::leak(vec![0usize; n].into_boxed_slice());
    let offs: &'static [u32] = Box::leak((0..n as u32).map(|i| i * 8).collect::<Vec<_>>().into_boxed_slice());
    let layout: &'static KelpGcType = Box::leak(Box::new(KelpGcType {
        id: 0,
        size: (n * 8) as u32,
        align: 8,
        flags: 0,
        n_ptrs: n as u32,
        ptr_off: offs.as_ptr(),
        tag_off: 0,
        n_variants: 0,
        variants: core::ptr::null(),
    }));
    let base = slots.as_mut_ptr();
    register_region(base as *mut u8, layout);
    base
}

/// Allocate `n` byte arrays, one per MIX_BYTES size class, and store each in
/// root slot `i` before filling its payload with `fill`. Byte arrays are
/// NOSCAN, so the fill cannot forge a pointer slot. Returns the number of
/// payload bytes that were nonzero when handed out.
unsafe fn fill_round(slots: *mut usize, n: usize, fill: u8) -> usize {
    let mut dirty = 0;
    for i in 0..n {
        let len = MIX_BYTES[i % MIX_BYTES.len()];
        let p = kelp_gc_alloc_array(&BYTES, len) as usize;
        for k in 0..len {
            if core::ptr::read((p + k) as *const u8) != 0 {
                dirty += 1;
            }
        }
        core::ptr::write_bytes(p as *mut u8, fill, len);
        *slots.add(i) = p;
    }
    dirty
}

/// Memory that the collector freed is cleared before the next object takes
/// it, in both the line-hole space and the run space. The second round must
/// also land on the first round's addresses, so the test does exercise reuse.
#[test]
fn reused_memory_reads_as_zero_in_both_spaces() {
    let _g = serial();
    unsafe {
        reset();
        kelp_gc_thread_attach();
        const N: usize = 20_000;
        let slots = register_pointer_roots(N);
        let first = fill_round(slots, N, 0xAB);
        assert_eq!(first, 0, "fresh memory handed out nonzero");
        let round1: Vec<usize> = (0..N).map(|i| *slots.add(i)).collect();
        for i in 0..N {
            *slots.add(i) = 0;
        }
        kelp_gc_collect();
        kelp_gc_collect();
        let dirty = fill_round(slots, N, 0x5C);
        assert_eq!(dirty, 0, "a reused object was not zeroed");
        let reused = (0..N).filter(|&i| *slots.add(i) == round1[i]).count();
        assert!(reused >= N / 2, "expected reuse of freed memory, got {reused} of {N} at the same address");
        unregister_region(slots as *mut u8);
        kelp_gc_thread_detach();
    }
}

// ---- Collection-path benchmarks (ignored by default). Run one at a time in
// their own process with a fixed heap cap, e.g.
//   cargo test --release --no-run; ( ulimit -v 1048576; ... ) bench_binary_trees --ignored --exact
// They print cycles, total and max stop-the-world pause, and wall time.

fn report_pauses(label: &str, wall: f64) {
    unsafe {
        let s = st();
        eprintln!(
            "BENCH {label}: wall {wall:.3} s; cycles {}; pause total {:.2} ms (mark {:.2}, sweep {:.2}); pause max {:.3} ms; peak blocks {}",
            s.cycles,
            s.pause_total_ns as f64 / 1e6,
            s.mark_ns_total as f64 / 1e6,
            s.sweep_ns_total as f64 / 1e6,
            s.pause_max_ns as f64 / 1e6,
            s.peak_blocks
        );
    }
}

/// Binary-trees: a long-lived tree plus repeated short-lived trees of each depth.
#[inline(never)]
unsafe fn bt_make(d: u32) -> usize {
    let n = kelp_gc_alloc(&NODE) as usize;
    if d > 0 {
        let l = bt_make(d - 1);
        let r = bt_make(d - 1);
        core::ptr::write(n as *mut usize, l);
        core::ptr::write((n + 8) as *mut usize, r);
    }
    n
}

#[inline(never)]
unsafe fn bt_check(n: usize) -> usize {
    let l = core::ptr::read(n as *const usize);
    if l == 0 {
        return 1;
    }
    1 + bt_check(l) + bt_check(core::ptr::read((n + 8) as *const usize))
}

#[test]
#[ignore]
fn bench_binary_trees() {
    const MAX_DEPTH: u32 = 16;
    const MIN_DEPTH: u32 = 4;
    unsafe {
        kelp_gc_init(8 << 20, 128 << 20);
        kelp_gc_thread_attach();
        let start = std::time::Instant::now();
        let long = bt_make(MAX_DEPTH);
        let mut d = MIN_DEPTH;
        let mut total = 0usize;
        while d <= MAX_DEPTH {
            let iters = 1usize << (MAX_DEPTH - d + MIN_DEPTH);
            let mut check = 0;
            for _ in 0..iters {
                let t = bt_make(d);
                check += bt_check(t);
            }
            total += check;
            eprintln!("depth {d}: {iters} trees, {check} nodes");
            d += 2;
        }
        assert_eq!(bt_check(long), (1usize << (MAX_DEPTH + 1)) - 1, "long-lived tree intact");
        black_box(total);
        report_pauses("binary-trees", start.elapsed().as_secs_f64());
        kelp_gc_thread_detach();
    }
}

/// Every object that is rooted survives collections with its header and
/// payload intact, the collector finds it by its payload address, and no two
/// objects overlap. Garbage bursts between the roots force many cycles.
#[test]
fn every_allocation_is_reachable_and_no_two_overlap() {
    let _g = serial();
    unsafe {
        reset();
        kelp_gc_thread_attach();
        const N: usize = 6000;
        let slots = register_pointer_roots(N);
        for i in 0..N {
            let p = kelp_gc_alloc_array(&BYTES, MIX_BYTES[i % MIX_BYTES.len()]) as usize;
            core::ptr::write(p as *mut usize, i + 1);
            *slots.add(i) = p;
            for _ in 0..8 {
                black_box(kelp_gc_alloc_array(&BYTES, 1500));
            }
        }
        for _ in 0..3 {
            kelp_gc_collect();
        }
        assert!(st().cycles >= 10, "expected many cycles, got {}", st().cycles);
        let mut ranges = Vec::with_capacity(N);
        for i in 0..N {
            let p = *slots.add(i);
            let h = p - 8;
            assert_eq!(find_object(p), Some(h), "rooted object {i} is not found by the collector");
            assert_eq!(core::ptr::read(p as *const usize), i + 1, "rooted object {i} changed");
            ranges.push((h, h + object_total(h)));
        }
        ranges.sort();
        for w in ranges.windows(2) {
            assert!(w[0].1 <= w[1].0, "objects overlap: {:x?} and {:x?}", w[0], w[1]);
        }
        unregister_region(slots as *mut u8);
        kelp_gc_thread_detach();
    }
}

static LIVE_HEAD: Slot<usize> = Slot(UnsafeCell::new(0));

/// Large live set: a 1.5 M-node list rooted through a root region, churned
/// with garbage so that each cycle marks the whole list.
#[test]
#[ignore]
fn bench_large_live_set() {
    const LIVE: usize = 1_500_000;
    const CHURN: usize = 30_000_000;
    unsafe {
        kelp_gc_init(8 << 20, 128 << 20);
        kelp_gc_thread_attach();
        register_region(LIVE_HEAD.0.get() as *mut u8, &ROOT_PTR);
        let start = std::time::Instant::now();
        for i in 0..LIVE {
            let n = kelp_gc_alloc(&LINK) as usize;
            core::ptr::write(n as *mut usize, *LIVE_HEAD.0.get());
            core::ptr::write((n + 8) as *mut usize, i);
            *LIVE_HEAD.0.get() = n;
        }
        for _ in 0..CHURN {
            black_box(kelp_gc_alloc(&BYTES_16));
        }
        let mut p = *LIVE_HEAD.0.get();
        let mut count = 0usize;
        while p != 0 {
            count += 1;
            p = core::ptr::read(p as *const usize);
        }
        assert_eq!(count, LIVE, "live list intact");
        unregister_region(LIVE_HEAD.0.get() as *mut u8);
        report_pauses("large-live-set", start.elapsed().as_secs_f64());
        kelp_gc_thread_detach();
    }
}

/// 16-byte payload with no pointers: churn filler.
static BYTES_16: KelpGcType = KelpGcType {
    id: 6,
    size: 16,
    align: 8,
    flags: DESC_NOSCAN,
    n_ptrs: 0,
    ptr_off: core::ptr::null(),
    tag_off: 0,
    n_variants: 0,
    variants: core::ptr::null(),
};

/// Regions registered through `register_region`, so `unregister_region` can
/// find the handle of a base address.
static REGIONS: Mutex<Vec<(usize, u64)>> = Mutex::new(Vec::new());

/// Registers `base` with `layout` and remembers its handle under the base address.
unsafe fn register_region(base: *mut u8, layout: *const KelpGcType) {
    let h = root(base, layout);
    REGIONS.lock().unwrap().push((base as usize, h));
}

/// Unregisters the region registered at `base` and checks that it was live.
unsafe fn unregister_region(base: *mut u8) {
    let h = {
        let mut g = REGIONS.lock().unwrap();
        let i = g.iter().rposition(|e| e.0 == base as usize).expect("a registered region");
        g.remove(i).1
    };
    assert_eq!(kelp_gc_unregister_root_region(h), KELP_GC_ROOT_OK);
}

/// Registers `base` as a root with `layout` and returns its handle.
unsafe fn root(base: *mut u8, layout: *const KelpGcType) -> u64 {
    let mut h = 0u64;
    assert_eq!(kelp_gc_register_root_region(base, layout, &mut h), KELP_GC_ROOT_OK);
    assert_ne!(h, 0);
    h
}

// ---- Step 1 (Tidepool): the growable root table, the counters and the messages. ----

/// A descriptor whose noscan flag is set and whose one slot holds a pointer.
/// The allocator refuses it (T51, a child-process test in tidepool_step1_fixb.rs).
static NOSCAN_OFF: [u32; 1] = [0];
static NOSCAN_PTR: KelpGcType = KelpGcType {
    id: 6,
    size: 8,
    align: 8,
    flags: DESC_NOSCAN,
    n_ptrs: 1,
    ptr_off: NOSCAN_OFF.as_ptr(),
    tag_off: 0,
    n_variants: 0,
    variants: core::ptr::null(),
};

/// A second descriptor for id 1, the id of NODE (T52): a different layout under the same id.
static DUP_ID: KelpGcType = KelpGcType {
    id: 1,
    size: 24,
    align: 8,
    flags: 0,
    n_ptrs: 0,
    ptr_off: core::ptr::null(),
    tag_off: 0,
    n_variants: 0,
    variants: core::ptr::null(),
};

/// T2 (R2): a handle to an earlier entry does nothing once its slot is cleared
/// and reused. Only the entry the handle names is cleared.
#[test]
fn a_stale_root_handle_does_nothing() {
    let _g = serial();
    unsafe {
        reset();
        kelp_gc_thread_attach();
        let mut a = 0usize;
        let mut b = 0usize;
        let ha = root(&mut a as *mut usize as *mut u8, &ROOT_PTR);
        assert_eq!(kelp_gc_unregister_root_region(ha), KELP_GC_ROOT_OK);
        assert_eq!(kelp_gc_unregister_root_region(ha), KELP_GC_ROOT_STALE, "a second clear is stale");
        let hb = root(&mut b as *mut usize as *mut u8, &ROOT_PTR);
        assert_eq!((hb & 0xFFFF_FFFF), (ha & 0xFFFF_FFFF), "the freed slot is reused");
        assert_ne!(hb, ha, "the reused slot carries a new generation");
        assert_eq!(kelp_gc_unregister_root_region(ha), KELP_GC_ROOT_STALE, "the old handle names nothing");
        let mut stats = KelpGcStats::default();
        kelp_gc_stats(&mut stats);
        assert_eq!(stats.roots, 1, "the stale clear did not touch the live entry");
        assert_eq!(kelp_gc_unregister_root_region(hb), KELP_GC_ROOT_OK);
        kelp_gc_stats(&mut stats);
        assert_eq!(stats.roots, 0);
        kelp_gc_thread_detach();
    }
}

/// R10 and T14: when the table cannot grow, registration returns the named
/// status, nothing is registered and no charge is taken. The child runs under an
/// address-space limit that the next growth cannot fit.
#[test]
fn the_root_table_that_cannot_grow_says_so() {
    let (code, stderr) = child_result("root_full", &[]);
    assert_eq!(code, Some(0), "stderr: {stderr}");
    assert!(stderr.contains("root table full at 1024 slots, charged 0"), "stderr: {stderr}");
}

#[repr(C)]
struct RLimit {
    cur: u64,
    max: u64,
}
unsafe extern "C" {
    fn setrlimit(resource: i32, rlim: *const RLimit) -> i32;
}
const RLIMIT_AS: i32 = 9;

unsafe fn child_root_full() {
    kelp_gc_init(0, 0, 0, 0, 20);
    kelp_gc_thread_attach();
    // The first registration maps the first table (ROOT_CHUNK slots); the table
    // then cannot grow, because the limit leaves less room than its next size.
    // Every cell the loop needs is allocated before the limit is set.
    let mut cells = vec![0usize; 4096];
    let _first = root(cells.as_mut_ptr() as *mut u8, &ROOT_PTR);
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let vm_kb: u64 = status
        .lines()
        .find_map(|l| l.strip_prefix("VmSize:"))
        .and_then(|v| v.trim().trim_end_matches("kB").trim().parse().ok())
        .expect("VmSize");
    let lim = RLimit { cur: vm_kb * 1024 + 16 * 1024, max: u64::MAX };
    assert_eq!(setrlimit(RLIMIT_AS, &lim), 0);
    let mut count = 1usize;
    let mut h = 0u64;
    loop {
        let base = cells.as_mut_ptr().add(count) as *mut u8;
        if kelp_gc_register_root_region(base, &ROOT_PTR, &mut h) != KELP_GC_ROOT_OK {
            break;
        }
        count += 1;
    }
    let mut stats = KelpGcStats::default();
    kelp_gc_stats(&mut stats);
    assert_eq!(stats.roots as usize, count, "no slot was registered by the failed call");
    assert_eq!(stats.charged_bytes, 0);
    eprintln!("root table full at {count} slots, charged {}", stats.charged_bytes);
}

/// T52: two descriptors that share one id are still a fatal error.
#[test]
fn t52_two_descriptors_for_one_id_are_fatal() {
    let (code, stderr) = child_result("dup_id", &[]);
    assert_eq!(code, Some(1), "stderr: {stderr}");
    assert_eq!(stderr, "panic: gc: two type descriptors share one id\n");
}

unsafe fn child_dup_id() {
    kelp_gc_init(0, 0, 0, 0, 20);
    kelp_gc_thread_attach();
    // NODE is registered by the first two allocations, so the duplicate meets
    // the registered (fast) path as well as the first registration.
    black_box(kelp_gc_alloc(&NODE));
    black_box(kelp_gc_alloc(&NODE));
    black_box(kelp_gc_alloc(&DUP_ID));
}

/// T64: an array above the element limit is refused with its own message.
#[test]
fn t64_an_array_above_the_element_limit_is_named() {
    let (code, stderr) = child_result("array_too_large", &[]);
    assert_eq!(code, Some(1), "stderr: {stderr}");
    assert_eq!(
        stderr,
        "panic: array of 4294967296 elements is too large: a shared-pool array holds at most 4294967295 elements\n"
    );
}

/// T74: a Rust panic in the collector names its location, then aborts.
#[test]
fn t74_a_rust_panic_in_the_collector_names_its_location() {
    let (code, stderr) = child_result("panic_location", &[]);
    assert_eq!(code, None, "the child must abort, not exit: {stderr}");
    assert_eq!(stderr, "panic: gc: internal error at heap.rs:42\n");
}

/// The counter hook reports the live roots and the high-water index.
#[test]
fn the_stats_hook_reports_roots() {
    let _g = serial();
    unsafe {
        reset();
        kelp_gc_thread_attach();
        let mut a = [0usize; 1];
        let mut b = [0usize; 1];
        let ha = root(a.as_mut_ptr() as *mut u8, &ROOT_PTR);
        let hb = root(b.as_mut_ptr() as *mut u8, &ROOT_PTR);
        let mut stats = KelpGcStats::default();
        kelp_gc_stats(&mut stats);
        assert_eq!(stats.roots, 2);
        assert_eq!(stats.root_slots, 2);
        assert_eq!(stats.charged_bytes, 0, "no block is charged in step 1");
        assert_eq!(kelp_gc_unregister_root_region(ha), KELP_GC_ROOT_OK);
        kelp_gc_stats(&mut stats);
        assert_eq!(stats.roots, 1);
        assert_eq!(kelp_gc_unregister_root_region(hb), KELP_GC_ROOT_OK);
        kelp_gc_thread_detach();
    }
}

/// C15 and T78: the Rust structs have the sizes and offsets the header and the
/// metadata total assume, and the C compiler agrees with the header.

/// M7, amended row (b) (design section 20.7, amendment 1). With one live root,
/// `scan_roots` must take at most 2 times as long when the root table's
/// high-water index is 1024 (1024 registrations, then 1023 clearings) as when
/// it is 1 (one registration in a fresh table). The two tables are measured in
/// alternation, one pair per sample, and the gate is the ratio of the medians.
#[test]
#[ignore = "measurement (M7): run with --ignored --nocapture"]
fn m7_scan_roots_hw_1_and_hw_1024() {
    const SAMPLES: usize = 15;
    const ROUNDS: usize = 100_000;
    const GATE: f64 = 2.0;
    let _g = serial();
    let mut cells = vec![0usize; 1024];
    let mut time_a = Vec::with_capacity(SAMPLES);
    let mut time_b = Vec::with_capacity(SAMPLES);
    let mut ratios = Vec::with_capacity(SAMPLES);
    unsafe {
        for _ in 0..SAMPLES {
            m7_table_a(cells.as_mut_ptr());
            let sa = m7_stats();
            assert_eq!((sa.roots, sa.root_slots), (1, 1), "M7 table A: high-water index must be 1");
            let a = m7_time_scans(ROUNDS);
            kelp_gc_thread_detach();

            m7_table_b(cells.as_mut_ptr());
            let sb = m7_stats();
            assert_eq!((sb.roots, sb.root_slots), (1, 1024), "M7 table B: high-water index must be 1024");
            let b = m7_time_scans(ROUNDS);
            kelp_gc_thread_detach();

            time_a.push(a);
            time_b.push(b);
            ratios.push(b / a);
        }
    }
    let med_a = m7_median(&time_a);
    let med_b = m7_median(&time_b);
    let ratio = med_b / med_a;
    let rmin = ratios.iter().cloned().fold(f64::INFINITY, f64::min);
    let rmax = ratios.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let verdict = if ratio <= GATE { "PASS" } else { "FAIL" };
    eprintln!(
        "M7 RESULT hw1_median_ns={med_a:.1} hw1024_median_ns={med_b:.1} ratio={ratio:.3} \
         pair_ratio_min={rmin:.3} pair_ratio_max={rmax:.3} samples={SAMPLES} rounds={ROUNDS} \
         gate_max={GATE:.1} verdict={verdict}"
    );
    assert!(ratio <= GATE, "M7 gate: ratio {ratio:.3} is above {GATE}");
}

/// Fresh table with one registration (high-water index 1). Stacks are not scanned.
unsafe fn m7_table_a(cells: *mut usize) {
    reset();
    kelp_gc_thread_attach();
    st().scan_stacks = false;
    root(cells as *mut u8, &ROOT_PTR);
}

/// Table with 1024 registrations and 1023 clearings, one root left live
/// (high-water index 1024). Stacks are not scanned.
unsafe fn m7_table_b(cells: *mut usize) {
    reset();
    kelp_gc_thread_attach();
    st().scan_stacks = false;
    let mut handles = Vec::with_capacity(1024);
    for i in 0..1024 {
        handles.push(root(cells.add(i) as *mut u8, &ROOT_PTR));
    }
    for &h in &handles[..1023] {
        assert_eq!(kelp_gc_unregister_root_region(h), KELP_GC_ROOT_OK);
    }
}

unsafe fn m7_stats() -> KelpGcStats {
    let mut s = KelpGcStats::default();
    kelp_gc_stats(&mut s);
    s
}

/// Mean nanoseconds per `scan_roots` call over `rounds` calls.
unsafe fn m7_time_scans(rounds: usize) -> f64 {
    let t0 = std::time::Instant::now();
    for _ in 0..rounds {
        crate::threads::scan_roots();
    }
    t0.elapsed().as_nanos() as f64 / rounds as f64
}

fn m7_median(v: &[f64]) -> f64 {
    let mut s = v.to_vec();
    s.sort_by(|x, y| x.partial_cmp(y).unwrap());
    s[s.len() / 2]
}

// ---- Tidepool step 1 tests added by fixer B (T7, T25a, T25b, T30, T31, T39, T51, T70, T78). ----

#[path = "tidepool_step1_fixb.rs"]
mod tidepool_step1_fixb;
mod bench_clear;
// Tidepool step 2, T40 (R36): trigger arithmetic, in bytes, as a pure function.
// The function does not exist yet. The signatures this module expects, in
// `crate::trigger` (kelpc/gc/src/trigger.rs, not written):
//
//   pub fn first_trigger(heap_min: usize, start_low: usize, start_high: usize, headroom_pct: usize) -> usize
//       // midpoint(start_low, start_high) * (100 + headroom_pct) / 100; heap_min is not used
//   pub fn next_trigger(heap_min: usize, live_earlier: usize, live_latest: usize, headroom_pct: usize) -> usize
//       // max(heap_min, max(live_earlier, live_latest) * (100 + headroom_pct) / 100)
//
// Integer division rounds down. The spec gives 1.8 MiB for case (c), which is
// 1887436.8 bytes; the value 1_887_436 assumes the rounding is down.
//
// Red result: this module does not compile (E0432, unresolved import
// `crate::trigger`), so the whole `kelp_gc` test target fails to build.
// RED (job 59c3aa60): the four T40 tests below do not compile (E0432, unresolved import
// `crate::trigger`), so the kelp_gc test target fails to build.
mod trigger_t40 {
    use crate::trigger::{first_trigger, next_trigger};

    const MIB: usize = 1 << 20;

    #[test]
    fn t40a_first_trigger_is_3_mib_for_1_and_4_mib_at_headroom_20() {
        assert_eq!(first_trigger(8 * MIB, MIB, 4 * MIB, 20), 3 * MIB);
    }

    #[test]
    fn t40b_first_trigger_is_2_5_mib_at_headroom_0() {
        assert_eq!(first_trigger(8 * MIB, MIB, 4 * MIB, 0), 2_621_440);
    }

    #[test]
    fn t40c_first_trigger_is_1_8_mib_not_8_mib_for_1_and_2_mib_with_heap_min_8() {
        let t = first_trigger(8 * MIB, MIB, 2 * MIB, 20);
        assert_eq!(t, 1_887_436);
        assert_ne!(t, 8 * MIB, "heap_min must not set the first trigger");
    }

    #[test]
    fn t40d_next_trigger_uses_the_larger_of_the_two_latest_live_sizes() {
        // A (earlier) = 10 MiB, B (latest) = 20 MiB: max(8, 20 * 1.2) = 24 MiB.
        assert_eq!(next_trigger(8 * MIB, 10 * MIB, 20 * MIB, 20), 24 * MIB);
        // A = 30 MiB, B = 20 MiB: max(8, 30 * 1.2) = 36 MiB.
        assert_eq!(next_trigger(8 * MIB, 30 * MIB, 20 * MIB, 20), 36 * MIB);
        // A = 1 MiB, B = 2 MiB: heap_min is the floor, 8 MiB.
        assert_eq!(next_trigger(8 * MIB, MIB, 2 * MIB, 20), 8 * MIB);
    }
}

/// Tidepool step 2, T40e: the runtime's first clearing is at the manifest trigger
/// (see `child_first_clear`). Runs in a child, because the collector starts once
/// per process.
#[test]
fn t40e_first_clearing_happens_at_the_manifest_trigger() {
    let (code, err) = run_child("first_clear", &[]);
    assert_eq!(code, Some(0), "first clearing was not at the trigger: exit {code:?}\n{err}");
}
