use super::*;

#[test]
pub(crate) fn enum_tag_is_read_as_u32_and_selects_the_variant() {
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
pub(crate) fn heap_cap_of_256_mib_fails_with_out_of_memory() {
    let (code, stderr) = child_result("cap", &[]);
    assert_eq!(code, Some(1), "stderr: {stderr}");
    assert!(stderr.contains("panic: out of shared-pool memory: heap_max is 256 MiB, "), "got: {stderr}");
}

/// Item 7: the installed hook receives the message and its exit status is kept.
#[test]
pub(crate) fn panic_hook_receives_the_out_of_memory_message() {
    let (code, stderr) = child_result("hook", &[]);
    assert_eq!(code, Some(3), "the hook exits 3; stderr: {stderr}");
    assert!(stderr.contains("hooked: out of shared-pool memory: heap_max is 8 MiB, "), "stderr: {stderr}");
    assert!(!stderr.contains("panic: out of shared-pool memory"), "default text must not print when a hook is set");
}

/// Item 1: an allocation on a thread that never attached fails loudly.
#[test]
pub(crate) fn allocation_without_attach_fails_loudly() {
    let (code, stderr) = child_result("unattached", &[]);
    assert_eq!(code, Some(1), "stderr: {stderr}");
    assert!(
        stderr.contains("allocation on a thread that is not attached"),
        "stderr: {stderr}"
    );
}

/// Item 2: a thread that never reaches a safepoint is named after the timeout.
#[test]
pub(crate) fn stop_timeout_names_the_thread_that_did_not_stop() {
    let (code, stderr) = child_result("stop_timeout", &[("KELP_GC_STOP_TIMEOUT_MS", "300")]);
    assert_eq!(code, Some(1), "stderr: {stderr}");
    assert!(
        stderr.contains("gc: thread 0x") && stderr.contains("did not reach a safepoint within 300 ms"),
        "stderr: {stderr}"
    );
}

/// Item 8: the verifier fails when a survivor carries a mark bit into the next cycle.
#[test]
pub(crate) fn verifier_rejects_a_mark_bit_left_set() {
    let (code, stderr) = child_result("verify_mark", &[]);
    assert_eq!(code, Some(1), "stderr: {stderr}");
    assert!(stderr.contains("gc: heap verify failed: mark bit set after the sweep"), "stderr: {stderr}");
}

#[test]
pub(crate) fn heap_region_is_exactly_the_default_cap() {
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
pub(crate) fn stack_only_object_of_a_native_thread_survives_collections() {
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
pub(crate) fn watchdog(what: &'static str, secs: u64) {
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
pub(crate) fn threads_stop_the_world_and_keep_their_roots() {
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
pub(crate) fn cyclic_graphs_across_four_threads_have_an_exact_live_set() {
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
pub(crate) unsafe fn check_round(slots: &[*const usize], rings: usize, len: usize, expected: usize) -> usize {
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
pub(crate) unsafe fn build_ring(len: usize) -> usize {
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
pub(crate) unsafe fn check_ring(head: usize, len: usize) {
    let mut p = head;
    for i in 0..len {
        assert_eq!(core::ptr::read((p + 8) as *const usize), i, "ring tag");
        p = core::ptr::read(p as *const usize);
    }
    assert_eq!(p, head, "ring closes on its head");
}

#[inline(never)]
pub(crate) unsafe fn churn(n: usize) {
    for _ in 0..n {
        black_box(kelp_gc_alloc(&LINK));
    }
}

// ---- Measurements (hardening items 3 and 4). They print numbers and assert
// only that the runs stay correct. ----
