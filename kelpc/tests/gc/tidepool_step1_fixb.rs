//! Tidepool step 1 tests written by fixer B: T7, T25a, T25b, T30, T31, T39, T51
//! (the allocator refusal), T70 (a foreign thread) and T78 (the descriptor
//! against the Rust struct and the LLVM golden). Included from `gc/mod.rs`, which
//! supplies the helpers and descriptors through `use super::*`.
//!
//! The threads here are plain `std` threads that attach to the collector, so
//! they do not depend on Kelp's spawn path. Assertions made on a worker thread
//! panic on that thread, and `on_thread` turns that panic into a test failure.

use super::*;
use crate::state::{st, BLOCK, MAX_THREADS};
use crate::threads::THREADS;
use crate::types::{KelpGcStats, KelpGcType, KELP_GC_ROOT_OK};

/// Runs `f` on a new thread with a 256 KiB stack and returns its result.
fn on_thread<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(256 << 10)
        .spawn(f)
        .expect("spawn thread")
        .join()
        .expect("worker thread panicked")
}

/// Resident set size in kB, from `/proc/self/status`.
fn rss_kb() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    status
        .lines()
        .find_map(|l| l.strip_prefix("VmRSS:"))
        .and_then(|v| v.trim().trim_end_matches("kB").trim().parse().ok())
        .expect("VmRSS")
}

/// The number of clearings run so far.
fn collections() -> u64 {
    let mut s = KelpGcStats::default();
    unsafe { kelp_gc_stats(&mut s) };
    s.collections
}

// ---- Child-process bodies (selected by child_entry in gc/mod.rs). ----

/// T51: a descriptor that is noscan and has a pointer slot must be refused.
pub(super) unsafe fn child_noscan_with_pointers() {
    kelp_gc_init(0, 0, 0, 0, 20);
    kelp_gc_thread_attach();
    black_box(kelp_gc_alloc(&NOSCAN_PTR));
}

/// A descriptor whose element size is u32::MAX, for the T64 product check.
static WIDE: KelpGcType = KelpGcType {
    id: 8,
    size: u32::MAX,
    align: 8,
    flags: 0,
    n_ptrs: 0,
    ptr_off: core::ptr::null(),
    tag_off: 0,
    n_variants: 0,
    variants: core::ptr::null(),
};

/// T64: 2^63 elements of u32::MAX bytes each. The byte size is above 2^64, so
/// the product overflows usize. A u32 count and a u32 size cannot overflow
/// 64 bits, so the count must be a u64.
pub(super) unsafe fn child_array_overflows() {
    kelp_gc_init(0, 0, 0, 0, 20);
    kelp_gc_thread_attach();
    black_box(kelp_gc_alloc_array(&WIDE, 1usize << 63));
}

unsafe extern "C" fn foreign_alloc() -> *mut u8 {
    kelp_gc_alloc(&NODE)
}

/// T70: a C-ABI callback that allocates, called on a thread Kelp did not start.
pub(super) unsafe fn child_foreign_thread() {
    kelp_gc_init(0, 0, 0, 0, 20);
    kelp_gc_thread_attach();
    let f: unsafe extern "C" fn() -> *mut u8 = foreign_alloc;
    let addr = std::thread::spawn(move || unsafe { f() as usize })
        .join()
        .expect("join");
    black_box(addr);
}

// ---- Tests. ----

/// T7 (section 8, with the 20.7 amendment 4): a producer thread allocates and
/// drops 16-byte cells, and a consumer thread allocates and drops them, for 20
/// rounds of 2 MiB of cells each. The pass line, fixed before the run: resident
/// memory after round 20 is at most resident memory after round 2 plus 16 MiB.
/// The test also requires that clearings ran, so it cannot pass because
/// nothing was collected.
#[test]
fn t7_producer_and_consumer_keep_resident_memory_flat() {
    let _g = serial();
    unsafe { reset() };
    const CELLS_PER_ROUND: usize = (2 << 20) / 16;
    let c0 = collections();
    let mut round2_kb = 0;
    for round in 1..=20 {
        // The producer's cells are garbage as soon as they are allocated.
        on_thread(|| unsafe {
            kelp_gc_thread_attach();
            for _ in 0..CELLS_PER_ROUND {
                black_box(kelp_gc_alloc(&NODE));
            }
            kelp_gc_thread_detach();
        });
        // The consumer allocates the same amount and runs a clearing.
        on_thread(|| unsafe {
            kelp_gc_thread_attach();
            for _ in 0..CELLS_PER_ROUND {
                black_box(kelp_gc_alloc(&NODE));
            }
            kelp_gc_collect();
            kelp_gc_thread_detach();
        });
        if round == 2 {
            round2_kb = rss_kb();
        }
    }
    let last_kb = rss_kb();
    assert!(collections() > c0, "no clearing ran, so the test proves nothing");
    assert!(
        last_kb <= round2_kb + 16 * 1024,
        "resident memory grew from {round2_kb} kB after round 2 to {last_kb} kB after round 20 (line: 16 MiB)"
    );
}

/// T25a (R25): a thread detaches, another thread reuses its registry slot, and
/// the registry entry the second thread reads is its own thread id. Verify mode
/// is on (reset sets it), so the collection checks the heap as well.
#[test]
fn t25a_a_reused_registry_slot_is_the_threads_own() {
    let _g = serial();
    unsafe { reset() };
    let first = on_thread(|| unsafe {
        kelp_gc_thread_attach();
        let i = crate::threads::my_index().expect("attached");
        black_box(kelp_gc_alloc(&NODE));
        kelp_gc_thread_detach();
        i
    });
    let second = on_thread(|| unsafe {
        kelp_gc_thread_attach();
        let i = crate::threads::my_index().expect("attached");
        let tid = (*THREADS.get())[i].tid;
        assert_eq!(tid, crate::sys::pthread_self() as u64, "the index names another thread's record");
        let mut slot = [0usize; 1];
        let h = root(slot.as_mut_ptr() as *mut u8, &ROOT_PTR);
        slot[0] = kelp_gc_alloc(&LINK) as usize;
        kelp_gc_collect();
        assert_eq!(kelp_gc_is_heap_ptr(slot[0] as *const u8), 1, "the rooted object survives");
        assert_eq!(kelp_gc_unregister_root_region(h), KELP_GC_ROOT_OK);
        kelp_gc_thread_detach();
        i
    });
    assert_eq!(second, first, "the second thread must reuse the slot, or the test proves nothing");
}

/// T25b (R19): a thread holding a bump run is stopped by clearings, each one
/// resets the run, and verify finds no overlap. Runs with KELP_GC_STRESS=1, so
/// every allocation is a clearing. The main thread is attached and parked in
/// native code while the worker builds a 2000-node list rooted in one slot.
#[test]
fn t25b_a_clearing_resets_the_thread_run_and_objects_do_not_overlap() {
    let _g = serial();
    unsafe {
        reset();
        st().stress_n = 1;
    }
    const N: usize = 2000;
    let head: &'static mut [usize; 1] = Box::leak(Box::new([0usize; 1]));
    let addr = head.as_mut_ptr() as usize;
    unsafe {
        kelp_gc_thread_attach();
        kelp_gc_enter_native();
    }
    let handle = on_thread(move || unsafe {
        kelp_gc_thread_attach();
        let h = root(addr as *mut u8, &ROOT_PTR);
        for _ in 0..N {
            let n = kelp_gc_alloc(&LINK) as usize;
            core::ptr::write(n as *mut usize, *(addr as *const usize));
            *(addr as *mut usize) = n;
        }
        kelp_gc_thread_detach();
        h
    });
    unsafe {
        kelp_gc_leave_native();
        st().stress_n = 0;
    }
    let walk = || unsafe {
        let mut count = 0usize;
        let mut p = *(addr as *const usize);
        while p != 0 {
            assert_eq!(kelp_gc_is_heap_ptr(p as *const u8), 1, "node {count} is not a live object");
            count += 1;
            p = core::ptr::read(p as *const usize);
        }
        count
    };
    assert_eq!(walk(), N, "the list lost nodes");
    unsafe {
        kelp_gc_collect();
    }
    assert_eq!(walk(), N, "the list lost nodes in a clearing");
    unsafe {
        assert_eq!(kelp_gc_unregister_root_region(handle), KELP_GC_ROOT_OK);
        kelp_gc_thread_detach();
    }
}

/// T30 (R28): a thread takes a block, allocates a few objects, and detaches,
/// with its objects still rooted. A second thread then allocates until the next
/// clearing. The first thread's objects keep their values and are not
/// overwritten by the second thread's allocations.
#[test]
fn t30_a_detached_threads_block_is_not_reused_before_the_clearing() {
    let _g = serial();
    unsafe { reset() };
    let owned = on_thread(|| unsafe {
        kelp_gc_thread_attach();
        let mut out: Vec<(usize, u64)> = Vec::new();
        for i in 0..4usize {
            let slot: &'static mut [usize; 1] = Box::leak(Box::new([0usize; 1]));
            let h = root(slot.as_mut_ptr() as *mut u8, &ROOT_PTR);
            let o = kelp_gc_alloc(&LINK) as usize;
            core::ptr::write((o + 8) as *mut usize, 0xA0 + i);
            slot[0] = o;
            out.push((slot.as_mut_ptr() as usize, h));
        }
        kelp_gc_thread_detach();
        out
    });
    let before = collections();
    on_thread(move || unsafe {
        kelp_gc_thread_attach();
        let mut guard = 0usize;
        while collections() == before && guard < 2_000_000 {
            black_box(kelp_gc_alloc(&LINK));
            guard += 1;
        }
        kelp_gc_thread_detach();
    });
    assert!(collections() > before, "no clearing ran after the first thread detached");
    for (i, (slot, h)) in owned.iter().enumerate() {
        let o = unsafe { *(*slot as *const usize) };
        assert_eq!(unsafe { kelp_gc_is_heap_ptr(o as *const u8) }, 1, "object {i} was lost");
        let marker = unsafe { core::ptr::read((o + 8) as *const usize) };
        assert_eq!(marker, 0xA0 + i, "object {i} was overwritten by another thread");
        assert_eq!(unsafe { kelp_gc_unregister_root_region(*h) }, KELP_GC_ROOT_OK);
    }
}

/// T31 (R29, and M9's run): 256 threads each take a block and allocate one
/// rooted object, then stop together. The shared pool never exceeds heap_max
/// minus the charged bytes, the bytes allocated past the trigger are at most
/// 256 pool blocks, and the first clearing after the threads stop returns every
/// block. The main thread is not attached, because 256 attached threads is the
/// limit.
#[test]
fn t31_256_threads_each_take_a_block_within_heap_max() {
    let _g = serial();
    unsafe {
        reset();
        st().stress_n = 0;
    }
    let max = unsafe { st().heap_max };
    let n = MAX_THREADS;
    let b1 = Arc::new(Barrier::new(n + 1));
    let b2 = Arc::new(Barrier::new(n + 1));
    // (bytes in use, bytes charged, bytes past the trigger) at the first clearing.
    let report = Arc::new(Mutex::new((0usize, 0usize, 0usize)));
    let mut handles = Vec::new();
    for i in 0..n {
        let (b1, b2, report) = (b1.clone(), b2.clone(), report.clone());
        handles.push(
            std::thread::Builder::new()
                .stack_size(256 << 10)
                .spawn(move || unsafe {
                    kelp_gc_thread_attach();
                    let slot: &'static mut [usize; 1] = Box::leak(Box::new([0usize; 1]));
                    let h = root(slot.as_mut_ptr() as *mut u8, &ROOT_PTR);
                    let o = kelp_gc_alloc(&LINK) as usize;
                    core::ptr::write((o + 8) as *mut usize, i + 1);
                    slot[0] = o;
                    kelp_gc_enter_native();
                    b1.wait();
                    if i == 0 {
                        // The first thread runs the clearing for all of them.
                        kelp_gc_leave_native();
                        {
                            let s = st();
                            let mut r = report.lock().unwrap();
                            r.0 = s.used_blocks * BLOCK + s.charged_bytes;
                            r.1 = s.charged_bytes;
                            r.2 = s.bytes_since.saturating_sub(s.trigger);
                        }
                        kelp_gc_collect();
                        b2.wait();
                    } else {
                        b2.wait();
                        kelp_gc_leave_native();
                    }
                    assert_eq!(kelp_gc_is_heap_ptr(slot[0] as *const u8), 1, "thread {i}: object lost");
                    assert_eq!(core::ptr::read((slot[0] + 8) as *const usize), i + 1, "thread {i}: object overwritten");
                    assert_eq!(kelp_gc_unregister_root_region(h), KELP_GC_ROOT_OK);
                    kelp_gc_thread_detach();
                })
                .expect("spawn"),
        );
    }
    b1.wait();
    b2.wait();
    for h in handles {
        h.join().expect("worker panicked");
    }
    let (in_use, charged, past_trigger) = *report.lock().unwrap();
    assert!(
        in_use <= max,
        "blocks in use plus charged bytes ({in_use}) exceed heap_max ({max})"
    );
    assert!(
        past_trigger <= n * BLOCK,
        "{past_trigger} bytes were allocated past the trigger, more than 256 pool blocks ({})",
        n * BLOCK
    );
    let _ = charged;
    let (used_after, charged_after) = on_thread(|| unsafe {
        kelp_gc_thread_attach();
        clobber_stack();
        kelp_gc_collect();
        let r = (st().used_blocks, st().charged_bytes);
        kelp_gc_thread_detach();
        r
    });
    assert_eq!(used_after, 0, "the first clearing after the threads stopped left blocks in use");
    assert_eq!(charged_after, 0, "the first clearing after the threads stopped left bytes charged");
}

/// T39 (R35), observable part. Objects spread over many blocks, with rooted
/// objects in them, are all reached by each clearing, and objects allocated
/// after a clearing still verify. The white-box check (the high-water index
/// after a refill) needs a field that the runtime does not have yet, so this
/// test asserts that the allocations really span several blocks.
#[test]
fn t39_a_clearing_reaches_objects_in_every_block_in_use() {
    let _g = serial();
    unsafe {
        reset();
        kelp_gc_thread_attach();
    }
    let mut kept: Vec<(usize, u64, usize)> = Vec::new();
    for round in 0..3usize {
        for i in 0..200_000usize {
            let o = unsafe { kelp_gc_alloc(&LINK) } as usize;
            unsafe { core::ptr::write((o + 8) as *mut usize, i) };
            if i % 5000 == 0 {
                let slot: &'static mut [usize; 1] = Box::leak(Box::new([0usize; 1]));
                let h = unsafe { root(slot.as_mut_ptr() as *mut u8, &ROOT_PTR) };
                slot[0] = o;
                kept.push((slot.as_mut_ptr() as usize, h, round * 1_000_000 + i));
            }
        }
        unsafe { kelp_gc_collect() };
        for (slot, _, tag) in &kept {
            let o = unsafe { *(*slot as *const usize) };
            assert_eq!(unsafe { kelp_gc_is_heap_ptr(o as *const u8) }, 1, "rooted object {tag} lost");
            let i = tag % 1_000_000;
            assert_eq!(unsafe { core::ptr::read((o + 8) as *const usize) }, i, "rooted object {tag} overwritten");
        }
    }
    let peak = unsafe { st().peak_blocks };
    assert!(peak > 1, "the allocations stayed in one block ({peak}); the test proves nothing about many blocks");
    for (_, h, _) in &kept {
        assert_eq!(unsafe { kelp_gc_unregister_root_region(*h) }, KELP_GC_ROOT_OK);
    }
    unsafe { kelp_gc_thread_detach() };
}

/// T51 (R20, R47): `noscan` comes only from the descriptor, and a noscan
/// descriptor that has pointer slots is refused by the allocator with a named
/// message. The refusal text is not fixed by the design; the test requires the
/// `panic: gc:` prefix and the word `noscan`.
#[test]
fn t51_a_noscan_descriptor_with_pointer_slots_is_refused() {
    let (code, stderr) = super::child_result("noscan_with_pointers", &[]);
    assert_eq!(code, Some(1), "exit status, stderr: {stderr}");
    assert!(stderr.starts_with("panic: gc: "), "stderr: {stderr}");
    assert!(stderr.contains("noscan"), "the refusal must name noscan: {stderr}");
}

/// T70 (18.15, D53): a C-ABI callback that allocates, called on a thread Kelp
/// did not start, gives the same fatal message as an unattached allocation,
/// exactly once, with exit status 1.
#[test]
fn t70_an_allocation_on_a_foreign_thread_gives_the_unattached_message() {
    let (code, stderr) = super::child_result("foreign_thread", &[]);
    assert_eq!(code, Some(1), "stderr: {stderr}");
    assert_eq!(
        stderr,
        "panic: gc: allocation on a thread that is not attached (call kelp_gc_thread_attach before the first allocation)\n",
        "exit {code:?}, stderr: {stderr}"
    );
}

/// T64 (18.3): a count times the element size that overflows is named with its
/// own message. The count is 2^63 (a u64), so the product exceeds 2^64 and the
/// check fires before the count limit (array_too_large).
#[test]
fn t64_an_array_whose_byte_size_overflows_is_named() {
    let (code, stderr) = super::child_result("array_overflows", &[]);
    assert_eq!(code, Some(1), "stderr: {stderr}");
    assert_eq!(
        stderr,
        "panic: array size overflows: 9223372036854775808 elements of 4294967295 bytes each\n",
        "exit {code:?}, stderr: {stderr}"
    );
}

/// The position of `needle` in `text` as a byte offset, or panics with the text.
fn at(text: &str, needle: &str) -> usize {
    text.find(needle).unwrap_or_else(|| panic!("no `{needle}` in: {text}"))
}

/// T78 (18.25, D35, C15): the Rust `KelpGcType` field types, the LLVM
/// `%kelp_gc_type` the backend declares (taken from the managed golden IR), and
/// the Rust struct agree field by field, with the same byte offsets. The Node
/// descriptor in the golden IR holds the values the Rust `NODE` describes.
#[test]
fn t78_the_descriptor_fields_match_the_rust_struct_and_the_llvm_golden() {
    let golden = include_str!("../managed_lower/node.managed.ll");

    // The field types of the Rust `KelpGcType`, in declaration order (the C
    // header this test read before is gone; the Rust struct is the contract).
    let header_types: Vec<&str> = vec!["i32", "i32", "i32", "i32", "i32", "ptr", "i32", "i32", "ptr"];
    assert_eq!(header_types.len(), 9);

    // The LLVM type the backend declares.
    let decl = golden
        .lines()
        .find(|l| l.starts_with("%kelp_gc_type = type {"))
        .expect("the golden declares %kelp_gc_type");
    let llvm_types: Vec<&str> = decl
        .split('{')
        .nth(1)
        .and_then(|s| s.split('}').next())
        .expect("fields")
        .split(',')
        .map(str::trim)
        .collect();
    assert_eq!(llvm_types, header_types, "the LLVM type and the header disagree");

    // Byte offsets under the natural layout, against the Rust struct.
    let mut off = 0usize;
    let mut offs = Vec::new();
    for t in &llvm_types {
        let (size, align) = if *t == "ptr" { (8, 8) } else { (4, 4) };
        off = off.div_ceil(align) * align;
        offs.push(off);
        off += size;
    }
    assert_eq!(
        offs,
        vec![
            offset_of!(KelpGcType, id),
            offset_of!(KelpGcType, size),
            offset_of!(KelpGcType, align),
            offset_of!(KelpGcType, flags),
            offset_of!(KelpGcType, n_ptrs),
            offset_of!(KelpGcType, ptr_off),
            offset_of!(KelpGcType, tag_off),
            offset_of!(KelpGcType, n_variants),
            offset_of!(KelpGcType, variants),
        ],
        "the LLVM field offsets differ from the Rust struct"
    );
    assert_eq!(off.div_ceil(8) * 8, size_of::<KelpGcType>());

    // The Node descriptor in the golden IR holds the values of the Rust NODE.
    let def = golden
        .lines()
        .find(|l| l.starts_with("@kelp_gc_type_Node = "))
        .expect("the Node descriptor in the golden");
    let inside = def
        .split("%kelp_gc_type {")
        .nth(1)
        .and_then(|s| s.rsplit_once('}'))
        .map(|(a, _)| a)
        .expect("the Node initializer");
    let vals: Vec<&str> = inside.split(',').map(str::trim).collect();
    assert_eq!(vals.len(), 9, "the Node initializer has nine fields");
    let num = |v: &str| -> u64 { v.split_whitespace().nth(1).unwrap().parse().unwrap() };
    assert_eq!(num(vals[0]), NODE.id as u64, "id");
    assert_eq!(num(vals[1]), NODE.size as u64, "size");
    assert_eq!(num(vals[2]), NODE.align as u64, "align");
    assert_eq!(num(vals[3]), NODE.flags as u64, "flags");
    assert_eq!(num(vals[4]), NODE.n_ptrs as u64, "n_ptrs");
    assert_eq!(vals[5], "ptr @kelp_gc_ptrs_Node", "ptr_off names the pointer table");
    assert_eq!(num(vals[6]), NODE.tag_off as u64, "tag_off");
    assert_eq!(num(vals[7]), NODE.n_variants as u64, "n_variants");
    assert_eq!(vals[8], "ptr null", "variants");

    // The pointer table the Node descriptor names holds the offsets NODE_OFF.
    let table = golden
        .lines()
        .find(|l| l.starts_with("@kelp_gc_ptrs_Node = "))
        .expect("the Node pointer table in the golden");
    let list = &table[at(table, "[i32 ")..];
    let offsets: Vec<u32> = list
        .trim_end_matches(']')
        .rsplit('[')
        .next()
        .unwrap()
        .split(',')
        .map(|v| v.trim().trim_start_matches("i32 ").parse().unwrap())
        .collect();
    assert_eq!(offsets, NODE_OFF.to_vec(), "the golden pointer table and NODE differ");
}

/// T78 (18.25, the link part): the `kelp_gc_` functions the collector exports
/// are the set pinned here. The C header that declared them is gone, so a
/// change to the exported surface must change this list too.
#[test]
fn t78_the_export_names_are_pinned() {
    let src = include_str!("../../gc/src/lib.rs");
    let exported: std::collections::BTreeSet<&str> = src
        .lines()
        .filter_map(|l| {
            let rest = l.trim_start().strip_prefix("pub unsafe extern \"C\" fn ")?;
            let name = rest.split('(').next()?;
            name.starts_with("kelp_gc_").then_some(name)
        })
        .collect();
    let pinned: std::collections::BTreeSet<&str> = [
        "kelp_gc_alloc",
        "kelp_gc_alloc_array",
        "kelp_gc_collect",
        "kelp_gc_enter_native",
        "kelp_gc_init",
        "kelp_gc_is_heap_ptr",
        "kelp_gc_leave_native",
        "kelp_gc_register_root_region",
        "kelp_gc_safepoint",
        "kelp_gc_set_panic_hook",
        "kelp_gc_stats",
        "kelp_gc_test_spawn_pause",
        "kelp_gc_thread_attach",
        "kelp_gc_thread_detach",
        "kelp_gc_unregister_root_region",
    ]
    .into_iter()
    .collect();
    assert_eq!(pinned.len(), 15);
    assert_eq!(exported, pinned, "the exported collector surface changed");
}

/// T74 (18.20, the handler routing): the non-test panic handler names the
/// location of the panic and aborts through the same formatter that T74's child
/// uses. The handler itself is compiled only without `cfg(test)`, so this
/// checks its source text. A run through the built staticlib is not part of
/// this test.
#[test]
fn t74_the_panic_handler_passes_the_location_to_the_formatter() {
    let src = include_str!("../../gc/src/lib.rs");
    let at = |needle: &str| src.find(needle).unwrap_or_else(|| panic!("lib.rs lacks `{needle}`"));
    let handler = at("#[panic_handler]");
    let body = &src[handler..];
    assert!(body.contains("info.location()"), "the handler must read the location");
    assert!(
        body.contains("Some(loc) => sys::report_and_abort(loc.file(), loc.line())"),
        "the handler must pass the file and line to report_and_abort"
    );
    assert!(
        body.contains("None => sys::report_and_abort(\"?\", 0)"),
        "a panic without a location must still abort through report_and_abort"
    );
}
