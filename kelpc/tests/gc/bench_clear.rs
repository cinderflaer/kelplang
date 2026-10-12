//! Measurement tests for M4 and M9 (design section 13). Both are ignored by
//! default and run through `bench/gc-clear/run.sh`, which builds the crate's
//! test binary, runs each test in its own process, and takes medians. Nothing
//! here checks a gate: the printed line is the measurement, and the pass line
//! is in section 20.2 of the design.
//!
//! The collector reads `heap_max` once per process, so M4 runs one child
//! process per `heap_max`. Both tests skip `reset()`, so the verifier stays off
//! (KELP_GC_VERIFY unset), as section 13 asks.

use super::*;

/// The 1 MiB live set of M4 is rooted through this slot.
static M4_LIVE: Slot<usize> = Slot(UnsafeCell::new(0));

/// A byte array with no pointers, for the 1 MiB live set.
static M4_BYTES: KelpGcType = KelpGcType {
    id: 900,
    size: 1,
    align: 1,
    flags: DESC_NOSCAN,
    n_ptrs: 0,
    ptr_off: core::ptr::null(),
    tag_off: 0,
    n_variants: 0,
    variants: core::ptr::null(),
};

/// M4 child: one `heap_max` (MiB from KELP_GC_M4_MIB), a 1 MiB live set, and
/// 1000 forced clearings. Prints the pause per clearing from the collector's
/// own timer (threads.rs `collect`). Mark and sweep are not split: the timers
/// that would split them are not in this tree.
#[test]
#[ignore = "measurement (M4): run by bench/gc-clear/run.sh"]
fn m4_clearing_cost_child() {
    let Some(mib) = (unsafe { env_usize(b"KELP_GC_M4_MIB\0") }) else {
        return;
    };
    unsafe {
        kelp_gc_init(0, mib << 20, 0, 0, 20, 0);
        kelp_gc_thread_attach();
        let live = kelp_gc_alloc_array(&M4_BYTES, 1 << 20) as usize;
        *M4_LIVE.0.get() = live;
        let h = root(M4_LIVE.0.get() as *mut u8, &ROOT_PTR);
        const CLEARINGS: usize = 1000;
        for _ in 0..CLEARINGS {
            kelp_gc_collect();
        }
        let s = st();
        assert_eq!(s.cycles, CLEARINGS, "every forced clearing ran");
        eprintln!(
            "M4 heap_max_mib={mib} clearings={} pause_ns_per_clearing={:.1} pause_max_ns={} live_bytes={}",
            s.cycles,
            s.pause_total_ns as f64 / CLEARINGS as f64,
            s.pause_max_ns,
            s.live
        );
        assert_eq!(kelp_gc_unregister_root_region(h), KELP_GC_ROOT_OK);
        kelp_gc_thread_detach();
    }
}

/// M9 (design section 13): 256 attached threads (main plus 255 workers with
/// 256 KiB stacks), each allocating one object and then stopping in native
/// state. Reports the bytes of block space not holding an object, the
/// overshoot past the trigger, and whether the first clearing frees every
/// block. Ignored by default; run one process per repeat.
#[test]
#[ignore = "measurement (M9): run by bench/gc-clear/run.sh"]
fn m9_refill_tail_and_overshoot_256_threads() {
    const WORKERS: usize = 255;
    unsafe {
        kelp_gc_init(0, 0, 0, 0, 20, 0);
        kelp_gc_thread_attach();
        let objects = Arc::new(Mutex::new(Vec::<usize>::new()));
        let ready = Arc::new(Barrier::new(WORKERS + 1));
        let done = Arc::new(Barrier::new(WORKERS + 1));
        let mut workers = Vec::new();
        for _ in 0..WORKERS {
            let (objects, ready, done) = (objects.clone(), ready.clone(), done.clone());
            workers.push(
                std::thread::Builder::new()
                    .stack_size(256 << 10)
                    .spawn(move || unsafe {
                        kelp_gc_thread_attach();
                        let p = kelp_gc_alloc(&LINK) as usize;
                        objects.lock().unwrap().push(p);
                        // Stop in native state while the main thread measures
                        // and clears; the objects are not rooted.
                        kelp_gc_enter_native();
                        ready.wait();
                        done.wait();
                        kelp_gc_leave_native();
                        kelp_gc_thread_detach();
                    })
                    .expect("spawn worker"),
            );
        }
        ready.wait();

        let attached = (0..MAX_THREADS)
            .filter(|&i| (*(THREADS.get() as *const crate::threads::ThreadRec).add(i)).used)
            .count();
        let objs = objects.lock().unwrap().clone();
        let mut object_bytes: usize = objs.iter().map(|&p| crate::heap::object_total(p - 8)).sum();
        assert_eq!(st().cycles, 0, "no clearing ran before the measurement");

        // Fill the pool with unrooted 4 KiB garbage until the automatic trigger
        // clears the heap. The trigger check runs at the start of an allocation
        // (lib.rs alloc_common), so the bytes_since read before the triggering
        // call is the overshoot's base, and the blocks in use before that call
        // are the ones the clearing returns.
        let trigger = st().trigger;
        let mut at_trigger = 0usize;
        let mut blocks_at_trigger = 0usize;
        let mut calls = 0usize;
        loop {
            let before = st().bytes_since;
            let blocks_before = st().used_blocks;
            let c0 = st().cycles;
            let g = kelp_gc_alloc_array(&M4_BYTES, 4096) as usize;
            calls += 1;
            if st().cycles > c0 {
                at_trigger = before;
                blocks_at_trigger = blocks_before;
                // The triggering call's own object is allocated after the clearing.
                break;
            }
            object_bytes += crate::heap::object_total(g - 8);
            assert!(calls < 1_000_000, "the automatic trigger never ran");
        }
        let block_bytes = blocks_at_trigger * BLOCK;
        let unused_tail = block_bytes.saturating_sub(object_bytes);
        let overshoot = at_trigger.saturating_sub(trigger);
        // The triggering call allocated its own object after the clearing, so one
        // block in use belongs to that object (the trigger block). The gate is
        // zero blocks in use after the first clearing, excluding the trigger
        // block: both the raw count and the count minus that one block are printed.
        let used_after = st().used_blocks;
        let used_after_excl = used_after.saturating_sub(1);
        let s = st();
        eprintln!(
            "M9 attached={attached} objects={} object_bytes={object_bytes} blocks_at_trigger={blocks_at_trigger} \
             block_bytes={block_bytes} unused_tail_bytes={unused_tail} bytes_since_at_trigger={at_trigger} \
             trigger={trigger} overshoot_bytes={overshoot} first_clearing_ran=true \
             blocks_after_first_clearing={used_after} blocks_after_first_clearing_excl_trigger={used_after_excl} \
             calls={calls} cycles={}",
            objs.len(),
            s.cycles
        );
        assert_eq!(objs.len(), WORKERS, "every worker allocated its object");
        done.wait();
        for w in workers {
            w.join().expect("worker joins");
        }
        kelp_gc_thread_detach();
    }
}
