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
    kelp_gc_init(0, 0, 0, 0, 20, 0);
    let s = st();
    core::ptr::write_bytes(s.blocks as *mut u8, 0, s.nblocks * size_of::<BlockMeta>());
    core::ptr::write_bytes(s.types as *mut u8, 0, TYPE_TABLE_IDS * 8);
    core::ptr::write_bytes(s.large as *mut u8, 0, s.large_cap * 8);
    core::ptr::write_bytes(THREADS.get() as *mut u8, 0, size_of::<[crate::threads::ThreadRec; MAX_THREADS]>());
    crate::threads::reset_roots();
    st().charged_bytes = 0;
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
                kelp_gc_init(0, 0, 0, 0, 20, 0);
                kelp_gc_thread_attach();
                fill_until_full();
            }
            "hook" => {
                kelp_gc_init(0, 8 << 20, 0, 0, 20, 0);
                kelp_gc_set_panic_hook(Some(hook_exit_3));
                kelp_gc_thread_attach();
                fill_until_full();
            }
            "unattached" => {
                kelp_gc_init(0, 0, 0, 0, 20, 0);
                black_box(kelp_gc_alloc(&NODE));
            }
            "type_over_limit" => {
                kelp_gc_init(0, 0, 0, 0, 20, 0);
                kelp_gc_thread_attach();
                black_box(kelp_gc_alloc(&OVER_ID));
            }
            "stop_timeout" => child_stop_timeout(),
            "verify_mark" => child_verify_mark(),
            "root_full" => child_root_full(),
            "dup_id" => child_dup_id(),
            "array_too_large" => {
                kelp_gc_init(0, 0, 0, 0, 20, 0);
                kelp_gc_thread_attach();
                black_box(kelp_gc_alloc_array(&NODE, u32::MAX as usize + 1));
            }
            "noscan_with_pointers" => tidepool_step1_fixb::child_noscan_with_pointers(),
            "array_overflows" => tidepool_step1_fixb::child_array_overflows(),
            "foreign_thread" => tidepool_step1_fixb::child_foreign_thread(),
            "first_clear" => child_first_clear(),
            "first_clear_block" => child_first_clear_block(),
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
    kelp_gc_init(0, 0, 1 << 20, 2 << 20, 20, 0);
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

mod child_first;
pub(crate) use child_first::*;

mod enum_tag;
pub(crate) use enum_tag::*;

mod large;
pub(crate) use large::*;

mod panic_loc;
pub(crate) use panic_loc::*;

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
//   pub fn next_trigger(rule: Rule, heap_min: usize, live_earlier: usize, live_latest: usize, extra: usize, headroom_pct: usize) -> usize
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
    use crate::trigger::{first_trigger, next_trigger, Rule};

    const MIB: usize = 1 << 20;

    #[test]
    pub(crate) fn t40a_first_trigger_is_3_mib_for_1_and_4_mib_at_headroom_20() {
        assert_eq!(first_trigger(8 * MIB, MIB, 4 * MIB, 20), 3 * MIB);
    }

    #[test]
    pub(crate) fn t40b_first_trigger_is_2_5_mib_at_headroom_0() {
        assert_eq!(first_trigger(8 * MIB, MIB, 4 * MIB, 0), 2_621_440);
    }

    #[test]
    pub(crate) fn t40c_first_trigger_is_1_8_mib_not_8_mib_for_1_and_2_mib_with_heap_min_8() {
        let t = first_trigger(8 * MIB, MIB, 2 * MIB, 20);
        assert_eq!(t, 1_887_436);
        assert_ne!(t, 8 * MIB, "heap_min must not set the first trigger");
    }

    #[test]
    pub(crate) fn t40d_next_trigger_uses_the_larger_of_the_two_latest_live_sizes() {
        // A (earlier) = 10 MiB, B (latest) = 20 MiB: max(8, 20 * 1.2) = 24 MiB.
        assert_eq!(next_trigger(Rule::TwoSize, 8 * MIB, 10 * MIB, 20 * MIB, 0, 20), 24 * MIB);
        // A = 30 MiB, B = 20 MiB: max(8, 30 * 1.2) = 36 MiB.
        assert_eq!(next_trigger(Rule::TwoSize, 8 * MIB, 30 * MIB, 20 * MIB, 0, 20), 36 * MIB);
        // A = 1 MiB, B = 2 MiB: heap_min is the floor, 8 MiB.
        assert_eq!(next_trigger(Rule::TwoSize, 8 * MIB, MIB, 2 * MIB, 0, 20), 8 * MIB);
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

/// Tidepool step 2, T40a at the pool-block level: the 3 MiB trigger of
/// `1 MiB .. 4 MiB` at headroom 20 clears in pool block 97, the block that
/// holds the allocation that takes bytes_since to the trigger.
#[test]
fn t40f_first_clearing_lands_in_the_pool_block_of_the_trigger() {
    let (code, err) = run_child("first_clear_block", &[]);
    assert_eq!(code, Some(0), "first clearing was not in the trigger's pool block: exit {code:?}\n{err}");
}

// ---- Step 3 (Tidepool, runtime of manual blocks): one element per chunk, rooted and charged. ----

/// Stat counters, read under the test lock.
fn stats_now() -> KelpGcStats {
    let mut s = KelpGcStats::default();
    unsafe { kelp_gc_stats(&mut s) };
    s
}

/// A block holds a zeroed element, keeps what it points to alive through a
/// clearing, charges its chunk and releases both on free. A stale free does nothing.
#[test]
fn a_block_is_zeroed_rooted_and_charged_until_freed() {
    let _g = serial();
    unsafe {
        reset();
        kelp_gc_thread_attach();
        let before = stats_now();
        let (p, h) = crate::block::alloc(&ROOT_PTR).unwrap_or_else(|e| panic!("status {e}"));
        assert_eq!(*(p as *const usize), 0, "the element starts zeroed");
        let obj = kelp_gc_alloc(&NODE) as usize;
        *(p as *mut usize) = obj;
        crate::threads::collect();
        assert_eq!(kelp_gc_is_heap_ptr(obj as *const u8), 1, "the root keeps its target alive");
        let mid = stats_now();
        assert_eq!(mid.roots, before.roots + 1);
        assert_eq!(mid.charged_bytes, before.charged_bytes + 8);
        assert!(crate::block::free(h, p, &ROOT_PTR));
        assert!(!crate::block::free(h, p, &ROOT_PTR), "a second free is stale");
        let after = stats_now();
        assert_eq!(after.roots, before.roots);
        assert_eq!(after.charged_bytes, before.charged_bytes);
        kelp_gc_thread_detach();
    }
}

/// A descriptor with no pointer slots, so the verifier does not read the
/// scalar word the test writes as a pointer.
static WORDS: KelpGcType = KelpGcType {
    id: 0x77,
    size: 16,
    align: 8,
    flags: 0,
    n_ptrs: 0,
    ptr_off: core::ptr::null(),
    tag_off: 0,
    n_variants: 0,
    variants: core::ptr::null(),
};

/// The address of the object made by `make_dead_words`. A static is not a
/// root, so holding the address here does not keep the object alive.
static DEAD_AT: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// Make one object and drop every reference to it. Only the address goes to
/// `DEAD_AT`, so no stack slot of the caller holds it.
#[inline(never)]
unsafe fn make_dead_words() {
    let obj = kelp_gc_alloc(&WORDS) as usize;
    *(obj as *mut u64) = 0x1122_3344_5566_7788;
    DEAD_AT.store(obj, SeqCst);
}

/// Verify mode poisons a dead shared-pool object at the sweep, so a read through
/// a dangling reference sees the poison and not the old contents (design T24,
/// rule R21). The sweep frees the object's block but never unmaps it, so the
/// read stays inside mapped memory and the test can name what it reads.
#[test]
fn a_dead_object_is_poisoned_in_verify_mode() {
    let _g = serial();
    unsafe {
        reset();
        kelp_gc_thread_attach();
        make_dead_words();
        // Overwrite the dead helper's stack frame, so no stale copy of the
        // address is left where the collector scans.
        clobber_stack();
        crate::threads::collect();
        let obj = DEAD_AT.load(SeqCst);
        assert_eq!(kelp_gc_is_heap_ptr(obj as *const u8), 0, "nothing roots the object");
        let poison = u64::from_ne_bytes([crate::heap::POISON; 8]);
        assert_eq!(*(obj as *const u64), poison, "a dangling read sees the poison");
        kelp_gc_thread_detach();
    }
}

/// Creation past `heap_max` runs one clearing first, then fails with
/// BLOCK_OVER_BUDGET. Every chunk is 1 MiB and untouched, so no memory is committed.
#[test]
fn a_block_past_heap_max_runs_a_clearing_then_fails() {
    static BIG: KelpGcType = KelpGcType {
        id: 200,
        size: 1 << 20,
        align: 8,
        flags: 0,
        n_ptrs: 0,
        ptr_off: core::ptr::null(),
        tag_off: 0,
        n_variants: 0,
        variants: core::ptr::null(),
    };
    let _g = serial();
    unsafe {
        reset();
        kelp_gc_thread_attach();
        let before = stats_now();
        let mut held = Vec::new();
        let status = loop {
            match crate::block::alloc(&BIG) {
                Ok(b) => held.push(b),
                Err(e) => break e,
            }
            assert!(held.len() <= 256, "more than heap_max of blocks was granted");
        };
        assert_eq!(status, crate::block::BLOCK_OVER_BUDGET);
        let after = stats_now();
        assert!(after.collections > before.collections, "the failure ran a clearing first");
        assert_eq!(after.charged_bytes, before.charged_bytes + held.len() as u64 * (1 << 20));
        for (p, h) in held {
            assert!(crate::block::free(h, p, &BIG));
        }
        assert_eq!(stats_now().charged_bytes, before.charged_bytes);
        kelp_gc_thread_detach();
    }
}

/// R12 (pool side): while live blocks hold the budget, the shared pool may not
/// take a run past heap_max, even after a clearing. Once the blocks are freed,
/// the same request is served.
#[test]
fn a_pool_run_past_heap_max_is_refused_while_blocks_hold_the_budget() {
    static BIG: KelpGcType = KelpGcType {
        id: 201,
        size: 1 << 20,
        align: 8,
        flags: 0,
        n_ptrs: 0,
        ptr_off: core::ptr::null(),
        tag_off: 0,
        n_variants: 0,
        variants: core::ptr::null(),
    };
    let _g = serial();
    unsafe {
        reset();
        kelp_gc_thread_attach();
        crate::threads::collect();
        let mut held = Vec::new();
        loop {
            match crate::block::alloc(&BIG) {
                Ok(b) => held.push(b),
                Err(e) => {
                    assert_eq!(e, crate::block::BLOCK_OVER_BUDGET);
                    break;
                }
            }
            assert!(held.len() <= 256, "more than heap_max of blocks was granted");
        }
        let request = 64usize << 20;
        crate::state::lock_heap();
        let refused = crate::heap::try_alloc(request);
        crate::state::unlock_heap();
        assert!(refused.is_none(), "a pool run past heap_max was granted");
        for (p, h) in held {
            assert!(crate::block::free(h, p, &BIG));
        }
        // The same request goes through the allocator, which writes the header
        // the verifier reads. 64 elements of 1 MiB make the 64 MiB run.
        let served = kelp_gc_alloc_array(&BIG, 64);
        assert!(!served.is_null(), "the freed budget does not serve the same run");
        // The object is unrooted garbage now; one clearing frees it.
        crate::threads::collect();
        kelp_gc_thread_detach();
    }
}


/// M5 (design section 13): the cost of one block create and free, and of the
/// root registration with 1 and 1000 live blocks. Pool off only: the block
/// chunk pool (D43) is not built, so the pool-on row is not measured. Each
/// sample is 100,000 create-and-free pairs of one 8-byte element; the figure is
/// the median of 5 samples. Measurement only: run with `--ignored --nocapture`.
#[test]
#[ignore = "measurement (M5): run with --ignored --nocapture"]
fn m5_block_create_and_free_ns() {
    const SAMPLES: usize = 5;
    const PAIRS: usize = 100_000;
    let _g = serial();
    let mut per_live = Vec::new();
    for live_target in [1usize, 1000] {
        let mut samples = Vec::with_capacity(SAMPLES);
        unsafe {
            reset();
            kelp_gc_thread_attach();
            let mut live = Vec::with_capacity(live_target);
            for _ in 0..live_target {
                live.push(crate::block::alloc(&ROOT_PTR).unwrap_or_else(|e| panic!("status {e}")));
            }
            for _ in 0..SAMPLES {
                let t0 = std::time::Instant::now();
                for _ in 0..PAIRS {
                    let (p, h) = crate::block::alloc(&ROOT_PTR).unwrap_or_else(|e| panic!("status {e}"));
                    assert!(crate::block::free(h, p, &ROOT_PTR));
                }
                samples.push(t0.elapsed().as_nanos() as f64 / PAIRS as f64);
            }
            for (p, h) in live {
                assert!(crate::block::free(h, p, &ROOT_PTR));
            }
            kelp_gc_thread_detach();
        }
        let med = m7_median(&samples);
        eprintln!("M5 live={live_target:>5} create+free ns per pair (median of {SAMPLES}): {med:.1}");
        per_live.push(med);
    }
    eprintln!("M5 ratio live=1000 over live=1: {:.3} (recorded, not gated)", per_live[1] / per_live[0]);
}
