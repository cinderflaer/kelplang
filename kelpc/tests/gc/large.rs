use super::*;

/// Churn `total_bytes` of `count`-byte arrays, all garbage, and report the
/// footprint waste: 1 - (bytes placed per cycle) / (peak blocks in use).
#[test]
pub(crate) fn measure_large_object_waste_4k_and_16k() {
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
pub(crate) unsafe fn churn_run(threads: usize, per: usize) -> (f64, usize, usize) {
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
pub(crate) fn measure_heap_lock_contention_four_threads() {
    let _g = serial();
    unsafe {
        kelp_gc_init(0, 0, 0, 0, 20, 0);
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
pub(crate) const MIX_BYTES: [usize; 6] = [16, 24, 100, 1000, 3000, 9000];

/// A root region of `n` pointer slots, zeroed and registered. The slots and
/// layout are leaked; the caller unregisters with `kelp_gc_unregister_root_region`.
pub(crate) unsafe fn register_pointer_roots(n: usize) -> *mut usize {
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
pub(crate) unsafe fn fill_round(slots: *mut usize, n: usize, fill: u8) -> usize {
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
pub(crate) fn reused_memory_reads_as_zero_in_both_spaces() {
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

pub(crate) fn report_pauses(label: &str, wall: f64) {
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
pub(crate) unsafe fn bt_make(d: u32) -> usize {
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
pub(crate) unsafe fn bt_check(n: usize) -> usize {
    let l = core::ptr::read(n as *const usize);
    if l == 0 {
        return 1;
    }
    1 + bt_check(l) + bt_check(core::ptr::read((n + 8) as *const usize))
}

#[test]
#[ignore]
pub(crate) fn bench_binary_trees() {
    const MAX_DEPTH: u32 = 16;
    const MIN_DEPTH: u32 = 4;
    unsafe {
        kelp_gc_init(8 << 20, 128 << 20, 0, 0, 20, 0);
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
pub(crate) fn every_allocation_is_reachable_and_no_two_overlap() {
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

pub(crate) static LIVE_HEAD: Slot<usize> = Slot(UnsafeCell::new(0));

/// Large live set: a 1.5 M-node list rooted through a root region, churned
/// with garbage so that each cycle marks the whole list.
#[test]
#[ignore]
pub(crate) fn bench_large_live_set() {
    const LIVE: usize = 1_500_000;
    const CHURN: usize = 30_000_000;
    unsafe {
        kelp_gc_init(8 << 20, 128 << 20, 0, 0, 20, 0);
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
pub(crate) static BYTES_16: KelpGcType = KelpGcType {
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
pub(crate) static REGIONS: Mutex<Vec<(usize, u64)>> = Mutex::new(Vec::new());

/// Registers `base` with `layout` and remembers its handle under the base address.
pub(crate) unsafe fn register_region(base: *mut u8, layout: *const KelpGcType) {
    let h = root(base, layout);
    REGIONS.lock().unwrap().push((base as usize, h));
}

/// Unregisters the region registered at `base` and checks that it was live.
pub(crate) unsafe fn unregister_region(base: *mut u8) {
    let h = {
        let mut g = REGIONS.lock().unwrap();
        let i = g.iter().rposition(|e| e.0 == base as usize).expect("a registered region");
        g.remove(i).1
    };
    assert_eq!(kelp_gc_unregister_root_region(h), KELP_GC_ROOT_OK);
}

/// Registers `base` as a root with `layout` and returns its handle.
pub(crate) unsafe fn root(base: *mut u8, layout: *const KelpGcType) -> u64 {
    let mut h = 0u64;
    assert_eq!(kelp_gc_register_root_region(base, layout, &mut h), KELP_GC_ROOT_OK);
    assert_ne!(h, 0);
    h
}

// ---- Step 1 (Tidepool): the growable root table, the counters and the messages. ----

/// A descriptor whose noscan flag is set and whose one slot holds a pointer.
/// The allocator refuses it (T51, a child-process test in tidepool_step1_fixb.rs).
pub(crate) static NOSCAN_OFF: [u32; 1] = [0];
pub(crate) static NOSCAN_PTR: KelpGcType = KelpGcType {
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
pub(crate) static DUP_ID: KelpGcType = KelpGcType {
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
pub(crate) fn a_stale_root_handle_does_nothing() {
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
pub(crate) fn the_root_table_that_cannot_grow_says_so() {
    let (code, stderr) = child_result("root_full", &[]);
    assert_eq!(code, Some(0), "stderr: {stderr}");
    assert!(stderr.contains("root table full at 1024 slots, charged 0"), "stderr: {stderr}");
}

#[repr(C)]
pub(crate) struct RLimit {
    pub(crate) cur: u64,
    pub(crate) max: u64,
}
unsafe extern "C" {
    pub(crate) fn setrlimit(resource: i32, rlim: *const RLimit) -> i32;
}
pub(crate) const RLIMIT_AS: i32 = 9;

pub(crate) unsafe fn child_root_full() {
    kelp_gc_init(0, 0, 0, 0, 20, 0);
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
pub(crate) fn t52_two_descriptors_for_one_id_are_fatal() {
    let (code, stderr) = child_result("dup_id", &[]);
    assert_eq!(code, Some(1), "stderr: {stderr}");
    assert_eq!(stderr, "panic: gc: two type descriptors share one id\n");
}

pub(crate) unsafe fn child_dup_id() {
    kelp_gc_init(0, 0, 0, 0, 20, 0);
    kelp_gc_thread_attach();
    // NODE is registered by the first two allocations, so the duplicate meets
    // the registered (fast) path as well as the first registration.
    black_box(kelp_gc_alloc(&NODE));
    black_box(kelp_gc_alloc(&NODE));
    black_box(kelp_gc_alloc(&DUP_ID));
}

/// T64: an array above the element limit is refused with its own message.
#[test]
pub(crate) fn t64_an_array_above_the_element_limit_is_named() {
    let (code, stderr) = child_result("array_too_large", &[]);
    assert_eq!(code, Some(1), "stderr: {stderr}");
    assert_eq!(
        stderr,
        "panic: array of 4294967296 elements is too large: a shared-pool array holds at most 4294967295 elements\n"
    );
}
