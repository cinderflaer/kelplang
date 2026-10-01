/* Polyp's message queue: the one channel between a GTK callback and the loop.
 *
 * A GTK callback cannot have `State`, because a closure owns its captures and
 * Kelp has no shared-ownership type, and it cannot borrow one either, because
 * the callback outlives the frame that would hold the borrow. So the callback
 * carries a small integer instead, posts it here, and the loop (which does own
 * the state) turns the integer back into a `Msg` with the tree it built.
 *
 * Both ends run on the thread that owns the GTK main context: `g_signal_connect`
 * invokes the handler from `g_main_context_iteration`, which the loop calls. So
 * there is no locking, and the two counters below are the whole of the state.
 *
 * A ring of fixed size is deliberate: it bounds what a burst of clicks can
 * buffer, and the head/count pair makes the full and empty cases distinguishable
 * without a third sentinel.
 */
#ifndef POLYP_QUEUE_H
#define POLYP_QUEUE_H

#define POLYP_QUEUE_CAPACITY 64

static int polyp_queue_ids[POLYP_QUEUE_CAPACITY];
static int polyp_queue_head;
static int polyp_queue_count;

static inline void polyp_post(int id) {
    if (polyp_queue_count == POLYP_QUEUE_CAPACITY) {
        return; /* full: the click is dropped rather than overwriting a pending one */
    }
    polyp_queue_ids[(polyp_queue_head + polyp_queue_count) % POLYP_QUEUE_CAPACITY] = id;
    polyp_queue_count++;
}

/* The oldest id, or -1 when the queue is empty. -1 is `polyp_poll`'s empty
 * marker rather than one of the loop's own ids, so `Polyp.QUIT_ID` is -2:
 * button ids are tree positions and are never negative. */
static inline int polyp_poll(void) {
    if (polyp_queue_count == 0) {
        return -1;
    }
    int id = polyp_queue_ids[polyp_queue_head];
    polyp_queue_head = (polyp_queue_head + 1) % POLYP_QUEUE_CAPACITY;
    polyp_queue_count--;
    return id;
}

#endif
