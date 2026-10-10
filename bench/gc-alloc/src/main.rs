//! Allocation micro benchmark for the managed collector's C ABI
//! (kelpc/gc/include/kelp_gc.h). Single thread, world always running.
//!
//! Each case allocates `N` objects that are all garbage except the last one,
//! so the heap cycles and the sweep path is exercised. Prints allocations per
//! second and nanoseconds per allocation.

use std::hint::black_box;
use std::time::Instant;

#[repr(C)]
pub struct KelpGcType {
    pub id: u32,
    pub size: u32,
    pub align: u32,
    pub flags: u32,
    pub n_ptrs: u32,
    pub ptr_off: *const u32,
    pub tag_off: u32,
    pub n_variants: u32,
    pub variants: *const core::ffi::c_void,
}
unsafe impl Sync for KelpGcType {}

const NOSCAN: u32 = 0x1;

// Type ids are unique per descriptor; the collector rejects two descriptors on one id.
static CELL16: KelpGcType = KelpGcType { id: 1, size: 16, align: 8, flags: NOSCAN, n_ptrs: 0, ptr_off: core::ptr::null(), tag_off: 0, n_variants: 0, variants: core::ptr::null() };
static ELEM8: KelpGcType = KelpGcType { id: 2, size: 8, align: 8, flags: NOSCAN, n_ptrs: 0, ptr_off: core::ptr::null(), tag_off: 0, n_variants: 0, variants: core::ptr::null() };
static OBJ200: KelpGcType = KelpGcType { id: 3, size: 200, align: 8, flags: NOSCAN, n_ptrs: 0, ptr_off: core::ptr::null(), tag_off: 0, n_variants: 0, variants: core::ptr::null() };

unsafe extern "C" {
    fn kelp_gc_init(heap_min: usize, heap_max: usize);
    fn kelp_gc_thread_attach();
    fn kelp_gc_collect();
    fn kelp_gc_alloc(t: *const KelpGcType) -> *mut u8;
    fn kelp_gc_alloc_array(elem: *const KelpGcType, count: usize) -> *mut u8;
}

#[inline(never)]
unsafe fn run_cell16(n: usize) -> usize {
    let mut last = 0usize;
    for i in 0..n {
        let p = unsafe { kelp_gc_alloc(&CELL16) };
        unsafe { (p as *mut usize).add(1).write(i) };
        last = p as usize;
    }
    last
}

#[inline(never)]
unsafe fn run_array8x8(n: usize) -> usize {
    let mut last = 0usize;
    for i in 0..n {
        let p = unsafe { kelp_gc_alloc_array(&ELEM8, 8) };
        unsafe { (p as *mut usize).write(i) };
        last = p as usize;
    }
    last
}

#[inline(never)]
unsafe fn run_obj200(n: usize) -> usize {
    let mut last = 0usize;
    for i in 0..n {
        let p = unsafe { kelp_gc_alloc(&OBJ200) };
        unsafe { (p as *mut usize).write(i) };
        last = p as usize;
    }
    last
}

fn report(label: &str, n: usize, secs: f64) {
    let rate = n as f64 / secs;
    println!(
        "{label:<22} {n:>10} allocs  {:>8.2} M allocs/s  {:>7.2} ns/alloc",
        rate / 1e6,
        secs * 1e9 / n as f64
    );
}

fn main() {
    // Modes: `cycles` (default) runs the default 8 MiB trigger, so the timed
    // loop includes collections. `nogc` raises the trigger, pre-faults the
    // region with garbage, collects, and times allocations that cannot start a
    // cycle, so the figure is the allocation path alone.
    let args: Vec<String> = std::env::args().collect();
    let nogc = args.get(1).map(|s| s == "nogc").unwrap_or(false);
    let n: usize = args.get(2).and_then(|a| a.parse().ok()).unwrap_or(if nogc { 1_500_000 } else { 10_000_000 });
    let rounds = 3;
    unsafe {
        if nogc {
            kelp_gc_init(400 << 20, 512 << 20);
        } else {
            kelp_gc_init(0, 0);
        }
        kelp_gc_thread_attach();
        println!("mode {}", if nogc { "nogc (no cycle in the timed loop)" } else { "cycles (default trigger)" });
        if nogc {
            // Pre-fault about 300 MiB of region, then free it all.
            for _ in 0..1_300_000 {
                black_box(kelp_gc_alloc_array(&ELEM8, 28));
            }
            kelp_gc_collect();
        } else {
            // Warm up: map the blocks and the type table once.
            black_box(run_cell16(100_000));
        }
        for (label, f) in [
            ("cell16 (24 B)", run_cell16 as unsafe fn(usize) -> usize),
            ("array8x8 (72 B)", run_array8x8 as unsafe fn(usize) -> usize),
            ("obj200 (208 B)", run_obj200 as unsafe fn(usize) -> usize),
        ] {
            let mut best = f64::MAX;
            for _ in 0..rounds {
                if nogc {
                    kelp_gc_collect();
                }
                let t = Instant::now();
                black_box(f(n));
                best = best.min(t.elapsed().as_secs_f64());
            }
            report(label, n, best);
        }
    }
}
