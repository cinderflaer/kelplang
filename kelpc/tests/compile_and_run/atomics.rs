use super::*;

// ---------------------------------------------------------------------------
// MK9-S3-M1 stage A1: the atomic cell `Atomic<T>`, and the one ordering v1 has.
// ---------------------------------------------------------------------------

/// A counter incremented and read through an atomic matches a plain integer's
/// result, and a `compare_exchange` retry loop converges on the same total. The
/// atomic is a `pd val`, which is how a counter is shared once threads exist:
/// a passdown declared `val` hands a function a shared borrow, which is what an
/// atomic mutates through.
#[test]
pub(crate) fn an_atomic_counter_matches_a_plain_integer_and_a_compare_exchange_loop_converges() {
    assert_compiles_and_runs(
        "tests/fixtures/atomic_counter.kp",
        "100\n0\n100\n200\n200\n0\n",
    );
}

/// Every `fetch_*` over the widths an emitter can get wrong, each returning the
/// value that was there and leaving the new one behind.
#[test]
pub(crate) fn the_atomic_fetch_operations_report_the_value_that_was_there() {
    assert_compiles_and_runs(
        "tests/fixtures/atomic_int_fetch.kp",
        "200\n200\n210\n205\n192\n195\n194\n1\n40001\n-3\n-3\n-3\n-2\n9000000000\n8999999999\n0\n1000000\n1000001\n0\n-8\n1\n4294967295\n",
    );
}

/// `AtomicBool` has new/load/store/swap/compare_exchange and no arithmetic, and
/// a test-and-set through a `pd val` flag lets exactly one attempt see `false`.
#[test]
pub(crate) fn an_atomic_bool_has_no_arithmetic_and_a_test_and_set_wins_once() {
    assert_compiles_and_runs(
        "tests/fixtures/atomic_bool.kp",
        "true\ntrue\nfalse\nfalse\ntrue\ntrue\nfalse\ntrue\n",
    );
}

/// The `Err` half of `compare_exchange` carries the value that was actually
/// there, and a failed exchange leaves the cell alone.
#[test]
pub(crate) fn a_failed_compare_exchange_reports_the_value_that_was_there() {
    assert_compiles_and_runs(
        "tests/fixtures/atomic_cas_err.kp",
        "5\n5\n5\n8\n",
    );
}

/// The ordering slot is a real argument of a real enum, so naming an ordering
/// that does not exist yet is the ordinary unknown-variant error rather than a
/// special refusal: a second variant is a widening of `Ordering`.
#[test]
pub(crate) fn an_ordering_variant_that_does_not_exist_is_the_ordinary_unknown_variant_error() {
    assert_rejected("tests/fixtures/atomic_ordering_reserved.kp");
}

/// The element set is closed: `bool` and the integer types, so a float (which
/// has no atomic arithmetic to lower to) is refused by name.
#[test]
pub(crate) fn a_float_atomic_element_is_refused() {
    assert_rejected("tests/fixtures/atomic_element_refused.kp");
}

/// A-10 slice 1 closed MK9 9.3's open question: an atomic stays refused as a
/// container element, because a container's own routines read and write an
/// element as a plain location and every access to an atomic has to be one
/// atomic operation.
#[test]
pub(crate) fn an_atomic_cannot_be_a_list_element() {
    assert_rejected_with(
        "tests/fixtures/atomic_list_element.kp",
        "'Atomic<i32>' cannot be a 'List' element",
    );
}

/// The same refusal per container, so each phrasing of the one reason is
/// pinned: a map value is read and written as a plain location by the map's own
/// routines, a box's own routines read and write its value the same way, and an
/// array holds its elements inline.
#[test]
pub(crate) fn an_atomic_cannot_be_a_map_value_or_a_box_or_array_element() {
    assert_rejected_with(
        "tests/fixtures/atomic_map_value.kp",
        "'Atomic<i32>' cannot be a 'Map' value",
    );
    assert_rejected_with(
        "tests/fixtures/atomic_box_element.kp",
        "'Atomic<i32>' cannot be a 'Box' element",
    );
    assert_rejected_with(
        "tests/fixtures/atomic_array_element.kp",
        "'Atomic<i32>' cannot be an array element",
    );
}

// ---------------------------------------------------------------------------
// MK9-S3-M1 stage A2: `thread.fence`, the standalone fence.
// ---------------------------------------------------------------------------

/// `thread.fence(order)` compiles, runs, and changes no stdout: a fence orders
/// no location, so a single-threaded program has nothing for it to order. The
/// value is that the *call* is exercised at all, since a fence that lowered to
/// nothing would run exactly the same way; the shape test below is what pins the
/// instruction.
#[test]
pub(crate) fn a_thread_fence_compiles_runs_and_changes_no_stdout() {
    assert_compiles_and_runs("tests/fixtures/thread_fence.kp", "0\n0\n7\n0\n");
}

/// The instruction `thread.fence` lowers to, in the artifact the backend wrote:
/// `fence seq_cst` in the emitted IR (MK9 section 6, stage A2). The file sits
/// beside the binary, which is the one place the backend's own output is
/// readable, so a program that merely printed the same stdout cannot pass this.
#[test]
pub(crate) fn a_thread_fence_lowers_to_the_fence_instruction_of_its_backend() {
    let source = fixture("tests/fixtures/thread_fence.kp");
    let bin = out_path("thread_fence");
    let build = kelpc()
        .arg("build")
        .arg(&source)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "expected 'thread_fence.kp' to compile, but it failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let artifact = bin.with_extension("ll");
    let generated = std::fs::read_to_string(&artifact).unwrap_or_else(|e| {
        panic!("the LLVM backend writes no '{}': {e}", artifact.display())
    });
    assert!(
        generated.contains("fence seq_cst"),
        "the LLVM backend's artifact has no 'fence seq_cst':\n{generated}"
    );
}

// ---------------------------------------------------------------------------
// MK9-S3-M1 stages B1 and C1: the `thread.spawn` / `thread.join` surface, the
// argument rules of MK9 section 5, and the two gates section 7 asks for. Stage
// B1 landed the checker and every rule as a *rejection*; stage C1 lowers both
// calls, so the four rule probes are still refused by name and the accepted
// shapes now build and run.
// ---------------------------------------------------------------------------

/// Rule 1: an exclusive borrow never crosses to a worker, because the worker
/// outlives the frame that made the borrow and the borrow is the promise that
/// no other path reaches the value.
#[test]
pub(crate) fn a_mut_borrow_argument_to_spawn_is_refused() {
    assert_rejected_with(
        "tests/fixtures/thread_spawn_mut_borrow.kp",
        "'thread.spawn' cannot send '&mut Atomic<i64>'",
    );
}

/// Rule 2: a shared borrow may cross only when the place it borrows has process
/// lifetime, which in v1 means a `pd` global, so a borrow of a frame local is
/// refused.
#[test]
pub(crate) fn a_borrow_of_a_frame_local_is_refused() {
    assert_rejected_with(
        "tests/fixtures/thread_spawn_local_borrow.kp",
        "borrows a frame local that ends before the worker does",
    );
}

/// Rule 2's other shape: a `str` (and a `View`, the same thing with a length)
/// is borrowed text with no place of its own.
#[test]
pub(crate) fn a_str_argument_to_spawn_is_refused() {
    assert_rejected_with(
        "tests/fixtures/thread_spawn_str_arg.kp",
        "'thread.spawn' cannot send a 'str'",
    );
}

/// Rule 3: a shared borrow of a `pd` whose pointee has a reachable `var` field
/// is refused, because two threads could write it through the same borrow.
#[test]
pub(crate) fn a_shared_borrow_reaching_a_var_field_is_refused() {
    assert_rejected_with(
        "tests/fixtures/thread_spawn_var_field.kp",
        "the borrow reaches a 'var' field",
    );
}

/// Rule 4: a moved value that carries a borrow cannot be constructed in the
/// first place, so `thread.spawn` opens no hole there.
#[test]
pub(crate) fn a_moved_value_that_carries_a_borrow_is_still_refused() {
    assert_rejected_with(
        "tests/fixtures/thread_spawn_borrowed_value.kp",
        "construct it from a string literal or from an earlier String/Bytes field",
    );
}

/// Rules 2 and 3 in their accepted shape: a `&Atomic<T>` of a `pd` global is
/// the one borrow that crosses to a worker, and stage C1 lowers the call.
#[test]
pub(crate) fn a_shared_pd_atomic_is_the_one_borrow_that_crosses() {
    assert_compiles_and_runs("tests/fixtures/thread_spawn_pd_atomic.kp", "1\n");
}

/// The accepted shapes: a declared `fun`, a capturing closure, and a handle
/// typed as a parameter, a `datac` field and an `enum` payload. Stage C1 lowers
/// all of them, so each builds and runs.
#[test]
pub(crate) fn an_accepted_spawn_or_join_builds_and_runs() {
    assert_compiles_and_runs("tests/fixtures/thread_spawn_accepted.kp", "42\n");
    assert_compiles_and_runs("tests/fixtures/thread_spawn_closure.kp", "42\n");
    assert_compiles_and_runs("tests/fixtures/thread_join_accepted.kp", "1\n");
}

/// The stress gate (MK9 section 7): eight workers, each doing a thousand
/// `fetch_add`s of one shared `pd val Atomic<i64>`, every one of them joined
/// before the total is read. A lost update anywhere makes the total something
/// other than 8000, which is the one thing a single-threaded counter test
/// cannot catch.
#[test]
pub(crate) fn a_thread_stress_gate_produces_the_exact_total() {
    assert_compiles_and_runs("tests/fixtures/thread_stress.kp", "8000\n");
}

/// B-5's mutex stress gate: eight workers, each taking the lock a thousand
/// times to increment one shared `pd val Mutex<i64>`. A missing lock shows up
/// as a total other than 8000.
#[test]
pub(crate) fn a_mutex_stress_gate_produces_the_exact_total() {
    assert_compiles_and_runs("tests/fixtures/mutex_stress.kp", "8000\n");
}

/// B-5: a `Mutex<List<i32>>` shared across four workers, each pushing through
/// the lock. The element's own drop reaches the list through the runtime
/// record, so the whole list is dropped when the mutex dies.
#[test]
pub(crate) fn a_mutex_guards_a_list_pushed_from_several_workers() {
    assert_compiles_and_runs("tests/fixtures/mutex_list_push.kp", "40\n");
}

/// B-5: `m.lock { v -> ... }` evaluates to the block's tail, and `into_inner`
/// consumes the mutex and hands the guarded value out.
#[test]
pub(crate) fn a_mutex_block_value_is_returned_and_into_inner_consumes_the_mutex() {
    assert_compiles_and_runs("tests/fixtures/mutex_lock_value.kp", "8\n14\n7\n");
    assert_compiles_and_runs("tests/fixtures/mutex_into_inner.kp", "2\n41\n");
}

/// B-5: a `return` inside a lock block ends the block, the runtime releases
/// the lock on that exit, and a second `lock` afterwards succeeds. A `?`
/// inside the block is the same kind of exit, and a second call that would
/// deadlock if the first leaked the lock proves it.
#[test]
pub(crate) fn a_return_inside_a_lock_block_releases_the_mutex() {
    assert_compiles_and_runs("tests/fixtures/mutex_lock_return.kp", "2\n");
    assert_compiles_and_runs("tests/fixtures/mutex_lock_try.kp", "10\n");
}

/// B-5: a worker's panic poisons the mutex through the runtime's cleanup
/// handler, and the main thread's next `lock` panics with "mutex poisoned".
#[test]
pub(crate) fn a_panic_inside_a_lock_block_poisons_the_mutex() {
    assert_fixture_panics("mutex_poison", "mutex poisoned");
}

/// B-5: a `&Mutex<T>` of a `pd` global crosses to a worker, the `Sync` half of
/// the crossing rule. A plain `var` datac stays refused
/// (`thread_spawn_var_field.kp`).
#[test]
pub(crate) fn a_shared_pd_mutex_crosses_to_a_worker() {
    assert_compiles_and_runs("tests/fixtures/mutex_spawn_borrow.kp", "1\n");
}

/// B-5: the block's `v` cannot escape. Returning it is a type mismatch, and
/// capturing it into a closure's environment is refused by the same borrow
/// rule a scoped block follows.
#[test]
pub(crate) fn a_lock_blocks_mutable_borrow_cannot_escape() {
    assert_rejected_with(
        "tests/fixtures/mutex_lock_borrow_returned.kp",
        "expected 'unit', found 'mut i64'",
    );
    assert_rejected_with(
        "tests/fixtures/mutex_lock_borrow_stored.kp",
        "cannot capture 'v': it is a borrow",
    );
}

/// B-5: a freestanding build refuses `Mutex` and names the spin lock a kernel
/// would use instead. Freestanding is LLVM-only by design, so the refusal is
/// reached before either backend.
#[test]
pub(crate) fn a_freestanding_mutex_is_refused() {
    assert_rejected_with(
        "tests/projects/freestanding_mutex",
        "a freestanding program has no threads and no libc",
    );
}

/// `RwLock`: read/write blocks, `try_*` returning `Option`, and `into_inner`.
#[test]
pub(crate) fn an_rwlock_returns_block_values_and_takes_try_and_into_inner() {
    assert_compiles_and_runs("tests/fixtures/rwlock_basic.kp", "8\n10\n10\n");
    assert_compiles_and_runs("tests/fixtures/rwlock_try.kp", "1\ntrue\n1\ntrue\n5\n");
}

/// A `&RwLock<T>` of a `pd` global crosses to a worker, the `Sync` half of the
/// crossing rule.
#[test]
pub(crate) fn a_shared_pd_rwlock_crosses_to_a_worker() {
    assert_compiles_and_runs("tests/fixtures/rwlock_spawn_borrow.kp", "1\n");
}

/// Two readers hold the lock at once, and a writer already waiting goes before
/// a reader that arrives afterwards. The ordering is recorded with atomics, so
/// no sleep is used as proof.
#[test]
pub(crate) fn readers_overlap_and_a_waiting_writer_goes_first() {
    assert_compiles_and_runs("tests/fixtures/rwlock_concurrent.kp", "true\n1\n");
}

/// A worker's panic inside `write` poisons the rwlock; a later `write` panics.
#[test]
pub(crate) fn a_panic_inside_a_write_poisons_the_rwlock() {
    assert_fixture_panics("rwlock_poison", "rwlock poisoned");
}

/// A worker's panic inside `read` releases the read without poisoning, so a
/// later `read` succeeds and the value is unchanged.
#[test]
pub(crate) fn a_panic_inside_a_read_releases_without_poisoning() {
    assert_compiles_and_runs(
        "tests/fixtures/rwlock_read_releases.kp",
        "worker failed\n7\n",
    );
}

/// The rwlock block's borrow cannot escape: returning it is a type mismatch,
/// capturing it into a closure is refused by the borrow rule.
#[test]
pub(crate) fn an_rwlock_block_borrow_cannot_escape() {
    assert_rejected_with(
        "tests/fixtures/rwlock_read_borrow_returned.kp",
        "expected 'unit', found '&i64'",
    );
    assert_rejected_with(
        "tests/fixtures/rwlock_read_borrow_stored.kp",
        "cannot capture 'v': it is a borrow",
    );
}

/// A freestanding build refuses `RwLock` and names the spin lock a kernel would
/// use instead.
#[test]
pub(crate) fn a_freestanding_rwlock_is_refused() {
    assert_rejected_with(
        "tests/projects/freestanding_rwlock",
        "a freestanding program has no threads and no libc",
    );
}

/// `Condvar`: a handshake wakes a waiter, and the timed wait returns `None` at
/// the deadline and `Some` once the predicate is false.
#[test]
pub(crate) fn a_condvar_handshake_wakes_a_waiter() {
    assert_compiles_and_runs("tests/fixtures/condvar_handshake.kp", "42\n");
    assert_compiles_and_runs("tests/fixtures/condvar_timeout.kp", "true\n7\n");
}

/// A freestanding build refuses `Condvar`.
#[test]
pub(crate) fn a_freestanding_condvar_is_refused() {
    assert_rejected_with(
        "tests/projects/freestanding_condvar",
        "a freestanding program has no threads and no libc",
    );
}

/// `Mutex.share`/`Condvar.share` make another handle to the same record, and
/// `Mutex.swap` exchanges the guarded value, which is how an owned value gets
/// into a mutex from inside a scoped block.
#[test]
pub(crate) fn a_shared_mutex_handle_and_a_swap() {
    assert_compiles_and_runs("tests/fixtures/mutex_share_swap.kp", "6\n6\n40\na\n0\n");
}

/// Channels (`std/sync`): four producers feed one consumer through a bounded
/// channel, and the receiver's `for` ends by itself when the last sender drops.
/// A Sender or Receiver may cross into a worker (decision 125).
#[test]
pub(crate) fn a_bounded_channel_carries_four_producers_to_one_consumer() {
    assert_compiles_and_runs("tests/fixtures/chan_bounded_mpsc.kp", "1000\n124500\n");
}

/// Closing in both directions: every sender gone drains then fails, a clone
/// keeps the channel open, and a send to a dropped receiver gets the value back.
#[test]
pub(crate) fn a_channel_closes_when_its_senders_or_its_receiver_drop() {
    assert_compiles_and_runs(
        "tests/fixtures/chan_close.kp",
        "6\n1\n2\n3\n-1\nopen\nclosed\nback\n",
    );
}

/// A rendezvous send completes only when a receive takes its value; a receiver
/// that leaves while the sender is parked hands the value back.
#[test]
pub(crate) fn a_rendezvous_send_completes_only_when_a_receive_takes_it() {
    assert_compiles_and_runs("tests/fixtures/chan_rendezvous.kp", "blocked\n7\ndone\nhanded back\n");
}

/// `select2` answers with the one ready case, and a closed empty channel is ready too.
#[test]
pub(crate) fn a_select_takes_whichever_channel_is_ready() {
    assert_compiles_and_runs("tests/fixtures/chan_select.kp", "1\n5\n1\n6\n0\nclosed\n");
}

/// The select engine under the `select` form: a receive case finds nothing, a send case
/// queues its value, the receive case takes it, and a timed wait ends at its deadline.
#[test]
pub(crate) fn a_select_engine_polls_sends_and_times_out() {
    assert_compiles_and_runs("tests/fixtures/chan_select_engine.kp", "false\ntrue\ntrue\n7\ntimed out\n");
}

/// `select3` with nothing ready waits for the one channel that sends.
#[test]
pub(crate) fn a_select_with_no_ready_case_blocks_until_one_is() {
    assert_compiles_and_runs("tests/fixtures/chan_select_blocks.kp", "2\n42\n");
}

/// A select over channels of two element types is refused.
#[test]
pub(crate) fn a_select_over_two_element_types_is_refused() {
    assert_rejected_with(
        "tests/fixtures/chan_select_mixed.kp",
        "argument 'b' has type '&std.sync.Receiver<String>', expected '&std.sync.Receiver<i64>'",
    );
}

/// `recv_timeout` times out, takes a late value, and reports a closed channel.
#[test]
pub(crate) fn a_channel_recv_timeout_times_out_and_then_receives() {
    assert_compiles_and_runs("tests/fixtures/chan_recv_timeout.kp", "timeout\n99\nclosed\n");
}

/// A zero-capacity bounded channel is a named panic.
#[test]
pub(crate) fn a_zero_capacity_channel_panics() {
    assert_fixture_panics("chan_zero_capacity", "sync.bounded: the capacity must be at least 1");
}

/// A channel carries owned, movable values: a borrowed `str` and a lock are
/// both refused.
#[test]
pub(crate) fn a_channel_refuses_a_borrowed_or_a_lock_element() {
    assert_rejected_with("tests/fixtures/chan_non_sendable.kp", "'str' is a borrowed view");
    assert_rejected_with("tests/fixtures/chan_send_a_lock.kp", "'Mutex<i64>' cannot be a 'List' element");
}

/// A freestanding build refuses the channel module by name.
#[test]
pub(crate) fn a_freestanding_channel_is_refused() {
    assert_rejected_with(
        "tests/projects/freestanding_channel",
        "the module 'std/sync' is unavailable in a freestanding build",
    );
}

/// The thread pool (`std/pool`): 1000 jobs on four workers with a sum check.
#[test]
pub(crate) fn a_pool_runs_a_thousand_jobs() {
    assert_compiles_and_runs("tests/fixtures/pool_sum.kp", "499500\n0\n");
}

/// A panicking job is counted and its worker survives to run the rest.
#[test]
pub(crate) fn a_pool_counts_a_panicking_job_and_keeps_its_worker() {
    assert_compiles_and_runs("tests/fixtures/pool_panics.kp", "27\n4\n5\n");
}

/// Dropping a pool joins it, an idle or twice-joined pool shuts down, and jobs
/// own what they capture (here a channel's sender).
#[test]
pub(crate) fn a_pool_shuts_down_without_deadlock() {
    // Decision 21: a hosted build refuses this program, so the test asserts the refusal.
    assert_fixture_refused("pool_drop_joins", "cannot be a value captured by a closure environment");
}

/// Queueing on a joined pool and a pool with no workers are both named panics.
#[test]
pub(crate) fn a_pool_panics_on_work_after_join_and_on_zero_workers() {
    assert_fixture_panics("pool_spawn_after_join", "Pool.spawn: the pool has been joined");
    assert_fixture_panics("pool_zero_workers", "pool.new: a pool needs at least one worker");
}

/// A worker that owns a Sender and panics still closes its channel: its drop runs
/// on the way out, so the receiver gets the queued value and then `Closed`.
/// Decision 131: a closure value cannot be a thread argument. Its environment is
/// collected memory that the start record does not root, so the refusal names the
/// argument, and the closure entry itself is still accepted.
#[test]
pub(crate) fn a_closure_value_is_refused_as_a_thread_argument() {
    assert_fixture_refused("thread_arg_closure_value", "cannot be a thread's argument");
}

#[test]
pub(crate) fn a_worker_that_panics_closes_the_sender_it_was_given() {
    assert_compiles_and_runs("tests/fixtures/thread_arg_sender_panic.kp", "1\nclosed\nworker gives up\n");
}

/// A Sender that is a temporary in a call argument is still owned when a later
/// argument panics, so the worker's frame drops it and the receiver sees `Closed`.
#[test]
pub(crate) fn a_temporary_sender_in_an_argument_closes_when_a_later_argument_panics() {
    assert_compiles_and_runs("tests/fixtures/thread_arg_temp_panic.kp", "closed\nboom\n");
}

/// Decision 127: a `fun` literal written directly as the `thread.spawn` entry may
/// capture a Sender. The worker's environment owns it, and it closes when the
/// worker returns, so main's receive loop ends.
#[test]
pub(crate) fn a_closure_entry_may_capture_a_sender() {
    assert_compiles_and_runs("tests/fixtures/closure_sender_send.kp", "5\n15\n");
}

/// Decision 127: a closure entry may capture a Mutex handle; the worker fills the
/// list and main reads the same list after join.
#[test]
pub(crate) fn a_closure_entry_may_capture_a_mutex() {
    assert_compiles_and_runs("tests/fixtures/closure_mutex_list.kp", "1000\nitem-999\n");
}

/// Decision 127 and the drop-on-panic guarantee: a worker whose closure environment
/// holds a Sender and then panics still closes the channel, through the env drop.
#[test]
pub(crate) fn a_closure_entry_that_panics_still_closes_its_sender() {
    assert_compiles_and_runs("tests/fixtures/closure_sender_panic.kp", "1\nclosed\nworker gives up\n");
}

/// The planner's risk note: the worker panics while holding a captured Mutex, which
/// poisons it. The environment drop must still run, and the captured Sender's drop
/// must close the channel without locking the poisoned Mutex.
#[test]
pub(crate) fn a_closure_entry_that_poisons_a_captured_mutex_still_closes_its_sender() {
    assert_compiles_and_runs("tests/fixtures/closure_mutex_poison.kp", "1\nclosed\nworker gives up\n");
}

/// Three producers send 300 Strings through a bounded channel of two; the count and
/// the byte total (3 * 490) come out exact.
#[test]
pub(crate) fn a_channel_of_strings_carries_three_producers() {
    assert_compiles_and_runs("tests/fixtures/chan_string_producers.kp", "300\n1470\n");
}

/// A worker holds the other handle to a `Mutex<List<String>>` and fills the list; main
/// reads the same list after join.
#[test]
pub(crate) fn a_mutex_list_is_shared_with_a_worker() {
    assert_compiles_and_runs("tests/fixtures/mutex_list_worker.kp", "1000\nitem-999\n");
}

/// A pool of three workers pushes 300 strings onto a `pd` mutex-guarded list.
#[test]
pub(crate) fn a_pool_fills_a_shared_list_of_strings() {
    assert_compiles_and_runs("tests/fixtures/pool_gate_strings.kp", "300\n1\n");
}

/// A freestanding build refuses the pool module by name.
#[test]
pub(crate) fn a_freestanding_pool_is_refused() {
    assert_rejected_with(
        "tests/projects/freestanding_pool",
        "the module 'std/pool' is unavailable in a freestanding build",
    );
}

/// The panic gate (MK9 section 7): a worker panics, its joiner sees `Err`
/// carrying the panic text, a second worker's work is intact, and the process
/// exits 0. A panic ends the thread it happened in; `main` still decides what to
/// do about it (MK9 section 2).
#[test]
pub(crate) fn a_panicking_worker_is_seen_as_an_error_by_its_joiner() {
    assert_compiles_and_runs(
        "tests/fixtures/thread_panic.kp",
        "worker exploded\n0\n42\n",
    );
}

/// Tidepool step 1 (issue 233): a panicking worker runs the user `Drop` of each
/// owned local, newest first, before its joiner sees the panic.
#[test]
pub(crate) fn a_panicking_worker_drops_its_owned_locals_newest_first() {
    assert_compiles_and_runs("tests/fixtures/panic_drop_order.kp", "drop\n2\ndrop\n1\nboom\n");
}

/// Tidepool step 1 (issue 233): a value moved out on one branch is not dropped
/// again on the panic path; the branch that does not move keeps both drops.
#[test]
pub(crate) fn a_value_moved_out_on_one_branch_is_not_dropped_twice_on_panic() {
    assert_compiles_and_runs(
        "tests/fixtures/panic_drop_moved.kp",
        "drop\n2\ndrop\n1\nboom\nkeep\ndrop\n2\ndrop\n1\nboom\n",
    );
}

/// Tidepool step 1 (issue 233): a file held by a worker's owned wrapper is closed
/// on panic, because the wrapper's user `Drop` closes it.
#[test]
pub(crate) fn a_file_held_by_a_panicking_worker_is_closed_by_its_wrapper() {
    assert_compiles_and_runs("tests/fixtures/panic_drop_handle.kp", "closed\nboom\n");
}

/// Tidepool step 4 (issue 233): a user `Drop` value is a thread argument. The
/// worker's frame drops it on panic, and the worker's drop runs once on return.
#[test]
pub(crate) fn a_user_drop_value_passed_to_a_worker_is_dropped_by_the_worker() {
    assert_compiles_and_runs(
        "tests/fixtures/panic_drop_arg.kp",
        "drop\n5\nboom\nok\ndrop\n6\n0\n",
    );
}

/// Tidepool step 2 (issue 234): each thread has its own copy of a `tl`, starting
/// from its literal, so the workers count without a lock and main keeps its own.
#[test]
pub(crate) fn thread_local_copies_are_per_thread() {
    assert_compiles_and_runs("tests/fixtures/tl_per_thread.kp", "1000\n1000\n0\n5\n");
}

/// Tidepool step 2 (issue 234): a worker's copy starts from the literal on first
/// access, whatever the main thread has stored in its own copy.
#[test]
pub(crate) fn a_worker_reads_its_own_copy_from_the_literal() {
    assert_compiles_and_runs("tests/fixtures/tl_lazy_init.kp", "3\n5\n3\n");
}

/// Tidepool step 2 (issue 234): eight workers increment their own copies 100000
/// times each, with no lock; each join sees its own count.
#[test]
pub(crate) fn eight_workers_increment_their_own_copies_without_a_lock() {
    assert_compiles_and_runs(
        "tests/fixtures/tl_stress.kp",
        &format!("{}7\n", "100000\n".repeat(8)),
    );
}

/// Tidepool step 2 (issue 234): a borrow of a `tl` names the spawning thread's
/// copy, so it is refused as a thread argument.
#[test]
pub(crate) fn a_borrow_of_a_thread_local_does_not_cross_to_a_worker() {
    assert_rejected_with("tests/fixtures/tl_refuse_borrow.kp", "cannot send '&i64'");
}

/// Tidepool step 2 (issue 234): a heap-owning `tl` is refused by name.
#[test]
pub(crate) fn a_heap_owning_thread_local_is_refused_by_name() {
    assert_rejected_with("tests/fixtures/tl_refuse_heap.kp", "takes a scalar in this release");
}

/// Tidepool step 2 (issue 234): a `tl` initializer that is not a literal is refused.
#[test]
pub(crate) fn a_thread_local_initializer_that_is_not_a_literal_is_refused() {
    assert_rejected_with("tests/fixtures/tl_refuse_nonliteral.kp", "is not a literal");
}

/// `MISSING-r7Wqb`: a worker that returns `unit` can be spawned *and* joined.
/// `thread.join` on a `Thread<unit>` builds `Result<unit, ThreadErr>`, whose
/// `Ok` variant now lays out payload-less rather than refusing `unit`.
#[test]
pub(crate) fn a_worker_that_returns_unit_can_be_joined() {
    assert_compiles_and_runs("tests/fixtures/thread_join_unit.kp", "1\njoined\n");
}

/// `MISSING-r7Wqb`'s general case: a `Result<unit, E>` written by hand, with
/// no thread anywhere. `Ok`'s field is `unit`; constructing one still runs the
/// value expression for its side effect even though nothing is stored.
#[test]
pub(crate) fn a_hand_written_result_of_unit_lays_out_and_runs() {
    assert_compiles_and_runs("tests/fixtures/result_unit_payload.kp", "ok\n1\n7\n");
}

/// MK9-S3-M1 stage C2: `thread.sleep_ms` sleeps the calling thread (main or a
/// worker) and returns control to it, and `thread.current_id` is stable
/// across two calls on the same thread and distinct between the main thread
/// and a worker it spawned.
#[test]
pub(crate) fn thread_sleep_ms_and_current_id_work_on_both_threads() {
    assert_compiles_and_runs("tests/fixtures/thread_sleep.kp", "42\n");
    assert_compiles_and_runs("tests/fixtures/thread_current_id.kp", "true\nfalse\n");
}

/// perf-env-cache: `env.has`/`env.get`/`env.set`/`env.unset` from three
/// threads at once, each on a name none of the others touches. This is what
/// caught the real bug in the first version of the environment read cache:
/// an unguarded scan of `environ` (built once per lookup, twice per
/// `set`/`unset` here) racing a concurrent thread's own `setenv`/`unsetenv`
/// segfaulted reliably (glibc may `realloc` the whole `environ` array), which
/// a single-threaded `set`-then-`get` test has no way to see. The three
/// per-worker names keep the joined output deterministic no matter how the
/// threads interleave.
#[test]
pub(crate) fn env_reads_and_writes_from_several_threads_at_once_are_safe() {
    assert_compiles_and_runs("tests/fixtures/env_thread_stress.kp", "true\ntrue\ntrue\n");
}

/// BUG-hT3wK: `str.byte` bounds-checks with a scan of `index + 1` bytes instead
/// of a `strlen`; the answers at the edges, and the panics one past them, are
/// unchanged.
#[test]
pub(crate) fn str_byte_keeps_its_answers_and_bounds_at_the_edges() {
    assert_compiles_and_runs("tests/fixtures/str_byte_edges.kp", "104\n111\n5\n0\n");
    assert_fixture_panics("str_byte_at_len", "str.byte: index out of bounds");
    assert_fixture_panics("str_byte_empty", "str.byte: index out of bounds");
}

/// BUG-fY3nQ: rebuilds free the table they replace while readers keep probing.
/// The readers' answers do not depend on the interleaving (one name stays set,
/// one is never set); the leak half of the fix is gated by `memory_gate.rs`,
/// which runs this fixture under valgrind with every other one.
#[test]
pub(crate) fn env_table_rebuilds_do_not_disturb_concurrent_readers() {
    assert_compiles_and_runs("tests/fixtures/env_thread_churn.kp", "200000\n200000\n3000\n3000\n");
}

/// `thread.join` consumes a `Thread<T>` and nothing else: a handle is the one
/// thing `thread.spawn` makes.
#[test]
pub(crate) fn join_refuses_a_value_that_is_not_a_thread_handle() {
    assert_rejected_with(
        "tests/fixtures/thread_join_not_a_handle.kp",
        "takes a 'Thread<T>' handle",
    );
}

/// A `Thread<T>` is refused as a `List`/`Map`/`Box`/array element, the rule an
/// `Atomic<T>` already has: a handle names one running worker and is joined
/// once, while a container's own routines copy and drop an element like a plain
/// location.
#[test]
pub(crate) fn a_thread_handle_cannot_be_a_container_element() {
    for (fixture, container) in [
        ("thread_spawn_list_element", "a 'List' element"),
        ("thread_spawn_map_element", "a 'Map' value"),
        ("thread_spawn_box_element", "a 'Box' element"),
        ("thread_spawn_array_element", "an array element"),
    ] {
        assert_rejected_with(
            &format!("tests/fixtures/{fixture}.kp"),
            &format!("'Thread<i64>' cannot be {container}"),
        );
    }
}
