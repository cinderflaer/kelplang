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
use std::process::Command;
use std::sync::{Arc, Barrier, Mutex, MutexGuard};

use crate::heap::{blk, desc_of, rd, total_for};
use crate::state::*;
use crate::threads::{ROOTS, THREADS};
use crate::types::*;
use crate::{
    env_usize, kelp_gc_alloc, kelp_gc_alloc_array, kelp_gc_collect, kelp_gc_enter_native,
    kelp_gc_init, kelp_gc_is_heap_ptr, kelp_gc_leave_native, kelp_gc_register_root_region,
    kelp_gc_safepoint, kelp_gc_set_panic_hook, kelp_gc_thread_attach, kelp_gc_thread_detach,
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

/// The Rust constants must equal the macros the header publishes, so the C
/// side (WP3 and the runtime) sees the same limits.
#[test]
fn header_constants_match_the_rust_side() {
    let h = include_str!("../../gc/include/kelp_gc.h");
    let has = |line: &str| h.lines().any(|l| l.trim() == line);
    assert!(has(&format!("#define KELP_GC_TYPE_ID_MAX {}u", TYPE_ID_MAX)), "TYPE_ID_MAX");
    assert!(has(&format!("#define KELP_GC_TYPE_ID_V1_LIMIT {}u", TYPE_TABLE_IDS)), "TYPE_TABLE_IDS");
    assert!(has(&format!("#define KELP_GC_ENUM_TAG_BYTES {}u", KELP_GC_ENUM_TAG_BYTES)), "enum tag width");
    assert!(has(&format!("#define KELP_GC_LARGE_MIN {}u", LARGE_MIN)), "LARGE_MIN");
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
    kelp_gc_init(0, 0);
    let s = st();
    core::ptr::write_bytes(s.blocks as *mut u8, 0, s.nblocks * size_of::<BlockMeta>());
    core::ptr::write_bytes(s.types as *mut u8, 0, TYPE_TABLE_IDS * 8);
    core::ptr::write_bytes(s.large as *mut u8, 0, s.large_cap * 8);
    core::ptr::write_bytes(THREADS.get() as *mut u8, 0, size_of::<[crate::threads::ThreadRec; MAX_THREADS]>());
    core::ptr::write_bytes(ROOTS.get() as *mut u8, 0, size_of::<[crate::threads::RootRec; MAX_ROOTS]>());
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
                kelp_gc_init(0, 0);
                kelp_gc_thread_attach();
                fill_until_full();
            }
            "hook" => {
                kelp_gc_init(0, 8 << 20);
                kelp_gc_set_panic_hook(Some(hook_exit_3));
                kelp_gc_thread_attach();
                fill_until_full();
            }
            "unattached" => {
                kelp_gc_init(0, 0);
                black_box(kelp_gc_alloc(&NODE));
            }
            "type_over_limit" => {
                kelp_gc_init(0, 0);
                kelp_gc_thread_attach();
                black_box(kelp_gc_alloc(&OVER_ID));
            }
            "stop_timeout" => child_stop_timeout(),
            "verify_mark" => child_verify_mark(),
            other => panic!("unknown child mode {other}"),
        }
    }
}

/// Keep every node live until the heap cap is reached.
unsafe fn fill_until_full() {
    kelp_gc_register_root_region(ROOT_SLOT.0.get() as *mut u8, &ROOT_PTR);
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
    kelp_gc_init(0, 0);
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
    kelp_gc_init(0, 0);
    kelp_gc_thread_attach();
    kelp_gc_register_root_region(ROOT_SLOT.0.get() as *mut u8, &ROOT_PTR);
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

/// Item 6: the enum tag is a u32 at `tag_off`. Variant 0 keeps its pointer
/// alive; variant 1 has no pointer slots, so its pointer is not traced.
#[test]
fn enum_tag_is_read_as_u32_and_selects_the_variant() {
    let _g = serial();
    unsafe {
        reset();
        assert_eq!(KELP_GC_ENUM_TAG_BYTES, 4);
        kelp_gc_thread_attach();
        kelp_gc_register_root_region(ENUM_ROOTS.0.get() as *mut u8, &ROOT_PTR);
        kelp_gc_register_root_region((ENUM_ROOTS.0.get() as *mut usize).add(1) as *mut u8, &ROOT_PTR);
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
        kelp_gc_collect();
        assert_eq!(kelp_gc_is_heap_ptr(x as *const u8), 1, "variant 0 pointer traced");
        assert_eq!(kelp_gc_is_heap_ptr(y as *const u8), 0, "variant 1 pointer not traced");
        kelp_gc_unregister_root_region(ENUM_ROOTS.0.get() as *mut u8);
        kelp_gc_unregister_root_region((ENUM_ROOTS.0.get() as *mut usize).add(1) as *mut u8);
        kelp_gc_thread_detach();
    }
}

#[test]
fn heap_cap_of_256_mib_fails_with_out_of_memory() {
    let (code, stderr) = child_result("cap", &[]);
    assert_eq!(code, Some(1), "stderr: {stderr}");
    assert!(stderr.contains("panic: out of memory"), "expected 'panic: out of memory', got: {stderr}");
}

/// Item 7: the installed hook receives the message and its exit status is kept.
#[test]
fn panic_hook_receives_the_out_of_memory_message() {
    let (code, stderr) = child_result("hook", &[]);
    assert_eq!(code, Some(3), "the hook exits 3; stderr: {stderr}");
    assert!(stderr.contains("hooked: out of memory"), "stderr: {stderr}");
    assert!(!stderr.contains("panic: out of memory"), "default text must not print when a hook is set");
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
    let all_ok = handles.into_iter().all(|h| h.join().expect("worker"));
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
                kelp_gc_register_root_region(slot, &ROOT_PTR);
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
    for h in handles {
        h.join().expect("ring worker");
    }
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
        for t in 0..THREADS_N {
            for k in 0..RINGS {
                let slot = (SLOTS[t].0.get() as *mut usize).add(k) as *mut u8;
                kelp_gc_unregister_root_region(slot);
            }
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
    for h in handles {
        h.join().expect("churn worker");
    }
    let secs = start.elapsed().as_secs_f64();
    (secs, st().allocs, CONTENDED.load(SeqCst))
}

/// Four-thread churn against one thread: if the global heap lock were the
/// bottleneck, the four-thread rate would fall far below the single-thread rate.
#[test]
fn measure_heap_lock_contention_four_threads() {
    let _g = serial();
    unsafe {
        kelp_gc_init(0, 0);
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
