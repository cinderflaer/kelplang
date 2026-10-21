use super::*;

/// Tidepool step 2, T40a at the pool-block level (design section 9): with
/// `start_low = 1` MiB and `start_high = 4` MiB at headroom 20, the trigger is
/// 3 MiB. A `NODE` takes 24 bytes (8 header, 16 payload), and a 32 KiB pool
/// block holds 1365 of them with an 8-byte tail. The 131072nd allocation takes
/// bytes_since to exactly 3 MiB, and the 131041st object is the first one in
/// pool block 97. The clearing is checked at the start of `alloc_common`, so it
/// comes in the call after that one, and that call must start with 97 pool
/// blocks in use. The blocks in use are read before each call.
/// Exit 0 when the trigger, the bytes and the block count all hold. Otherwise
/// 7 (trigger value or block count), 5 (bytes), 6 (no clearing).
pub(crate) unsafe fn child_first_clear_block() {
    const TRIGGER: usize = 3 << 20;
    const OBJ: usize = 24;
    const PER_BLOCK: usize = BLOCK / OBJ; // 1365
    const OBJ_AT_TRIGGER: usize = TRIGGER / OBJ; // 131072
    const BLOCKS_AT_TRIGGER: usize = OBJ_AT_TRIGGER.div_ceil(PER_BLOCK); // 97
    kelp_gc_init(0, 0, 1 << 20, 4 << 20, 20, 0);
    if st().trigger != TRIGGER {
        std::process::exit(7);
    }
    kelp_gc_thread_attach();
    let mut prev = 0usize;
    for _ in 0..10_000_000u32 {
        let before = st().bytes_since;
        let cycles = st().cycles;
        let blocks = st().used_blocks;
        black_box(kelp_gc_alloc(&NODE));
        if st().cycles > cycles {
            if !(before >= TRIGGER && prev < TRIGGER) {
                std::process::exit(5);
            }
            if blocks != BLOCKS_AT_TRIGGER {
                std::process::exit(7);
            }
            std::process::exit(0);
        }
        prev = before;
    }
    std::process::exit(6);
}

/// Keep every node live until the heap cap is reached.
pub(crate) unsafe fn fill_until_full() {
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

pub(crate) static CHILD_READY: AtomicBool = AtomicBool::new(false);

/// A worker attaches and spins without polling. The collector must give up
/// after the timeout and name the thread.
pub(crate) unsafe fn child_stop_timeout() {
    kelp_gc_init(0, 0, 0, 0, 20, 0);
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
pub(crate) unsafe fn child_verify_mark() {
    kelp_gc_init(0, 0, 0, 0, 20, 0);
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
pub(crate) fn child_result(mode: &str, env: &[(&str, &str)]) -> (Option<i32>, String) {
    run_child(mode, env)
}

#[test]
pub(crate) fn type_id_round_trips_through_header_and_descriptor() {
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
pub(crate) fn type_ids_up_to_the_v1_limit_round_trip() {
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
pub(crate) fn type_id_over_the_v1_limit_fails_loudly() {
    let (code, stderr) = child_result("type_over_limit", &[]);
    assert_eq!(code, Some(1), "stderr: {stderr}");
    assert!(
        stderr.contains("panic: gc: type id 1048576 is over the v1 type table limit (ids 0 to 1048575 are accepted)"),
        "stderr: {stderr}"
    );
}

#[test]
pub(crate) fn every_payload_is_eight_byte_aligned() {
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
pub(crate) fn churn_ten_million_sixteen_byte_cells_keeps_heap_bounded() {
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

pub(crate) unsafe fn wr_scalar(p: usize, v: usize) {
    core::ptr::write((p + 8) as *mut usize, v);
}

#[inline(never)]
pub(crate) unsafe fn build_cycles() {
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
pub(crate) fn mark_keeps_rooted_cycle_and_frees_garbage_cycle() {
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
pub(crate) unsafe fn build_enum_roots() {
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
pub(crate) static ENUM_TRACED: Slot<usize> = Slot(UnsafeCell::new(0));
pub(crate) static ENUM_UNTRACED: Slot<usize> = Slot(UnsafeCell::new(0));

/// Builds the enum fixture in its own frame, so no pointer local outlives it.
#[inline(never)]
pub(crate) unsafe fn enum_fixture() {
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
pub(crate) unsafe fn build_shared_chain(n: usize) -> usize {
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
pub(crate) fn mark_counts_each_shared_object_once() {
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
pub(crate) static DEAD_NODES: Slot<[usize; 2]> = Slot(UnsafeCell::new([0; 2]));

#[inline(never)]
pub(crate) unsafe fn build_interleaved(n: usize) -> usize {
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
pub(crate) fn sweep_clears_only_dead_starts_in_mixed_blocks() {
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
pub(crate) unsafe fn churn_garbage(n: usize) {
    for _ in 0..n {
        black_box(kelp_gc_alloc(&LINK));
    }
}

#[test]
pub(crate) fn sweep_frees_every_empty_block_without_residue() {
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
