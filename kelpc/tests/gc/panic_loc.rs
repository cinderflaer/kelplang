use super::*;

/// T74: a Rust panic in the collector names its location, then aborts.
#[test]
pub(crate) fn t74_a_rust_panic_in_the_collector_names_its_location() {
    let (code, stderr) = child_result("panic_location", &[]);
    assert_eq!(code, None, "the child must abort, not exit: {stderr}");
    assert_eq!(stderr, "panic: gc: internal error at heap.rs:42\n");
}

/// The counter hook reports the live roots and the high-water index.
#[test]
pub(crate) fn the_stats_hook_reports_roots() {
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
pub(crate) fn m7_scan_roots_hw_1_and_hw_1024() {
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
pub(crate) unsafe fn m7_table_a(cells: *mut usize) {
    reset();
    kelp_gc_thread_attach();
    st().scan_stacks = false;
    root(cells as *mut u8, &ROOT_PTR);
}

/// Table with 1024 registrations and 1023 clearings, one root left live
/// (high-water index 1024). Stacks are not scanned.
pub(crate) unsafe fn m7_table_b(cells: *mut usize) {
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

pub(crate) unsafe fn m7_stats() -> KelpGcStats {
    let mut s = KelpGcStats::default();
    kelp_gc_stats(&mut s);
    s
}

/// Mean nanoseconds per `scan_roots` call over `rounds` calls.
pub(crate) unsafe fn m7_time_scans(rounds: usize) -> f64 {
    let t0 = std::time::Instant::now();
    for _ in 0..rounds {
        crate::threads::scan_roots();
    }
    t0.elapsed().as_nanos() as f64 / rounds as f64
}

pub(crate) fn m7_median(v: &[f64]) -> f64 {
    let mut s = v.to_vec();
    s.sort_by(|x, y| x.partial_cmp(y).unwrap());
    s[s.len() / 2]
}
