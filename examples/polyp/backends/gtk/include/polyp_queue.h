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

/* The edit channel: a program with no user asks the loop to set an entry's
 * text, which fires the entry's real `changed` signal. A second ring, with the
 * text copied in, for the same reason as the first: the loop owns the widgets,
 * the scheduler does not. */
#define POLYP_EDIT_CAPACITY 8
#define POLYP_EDIT_TEXT_MAX 256

static char polyp_edit_text[POLYP_EDIT_CAPACITY][POLYP_EDIT_TEXT_MAX];
static int polyp_edit_entry[POLYP_EDIT_CAPACITY];
static int polyp_edit_head;
static int polyp_edit_count;
static int polyp_edit_last = -1;

static inline void polyp_post_edit(int index, const char* text) {
    if (polyp_edit_count == POLYP_EDIT_CAPACITY) {
        return; /* full: the edit is dropped rather than overwriting a pending one */
    }
    int slot = (polyp_edit_head + polyp_edit_count) % POLYP_EDIT_CAPACITY;
    int i = 0;
    while (text[i] != '\0' && i < POLYP_EDIT_TEXT_MAX - 1) {
        polyp_edit_text[slot][i] = text[i];
        i++;
    }
    polyp_edit_text[slot][i] = '\0';
    polyp_edit_entry[slot] = index;
    polyp_edit_count++;
}

/* The entry index of the oldest pending edit, or -1 when none is pending. The
 * text of the edit just polled is read with `polyp_poll_edit_text`. */
static inline int polyp_poll_edit(void) {
    if (polyp_edit_count == 0) {
        polyp_edit_last = -1;
        return -1;
    }
    polyp_edit_last = polyp_edit_head;
    polyp_edit_head = (polyp_edit_head + 1) % POLYP_EDIT_CAPACITY;
    polyp_edit_count--;
    return polyp_edit_entry[polyp_edit_last];
}

static inline const char* polyp_poll_edit_text(void) {
    if (polyp_edit_last < 0) {
        return "";
    }
    return polyp_edit_text[polyp_edit_last];
}

/* The focus channel: a program with no user asks the loop to grab the focus
 * for an entry, so a smoke run can prove a patch keeps it. The payload is the
 * entry's position in the tree the loop built last, exactly as the edit
 * channel's is. */
#define POLYP_FOCUS_CAPACITY 8

static int polyp_focus_index[POLYP_FOCUS_CAPACITY];
static int polyp_focus_head;
static int polyp_focus_count;

static inline void polyp_post_focus(int index) {
    if (polyp_focus_count == POLYP_FOCUS_CAPACITY) {
        return; /* full: the request is dropped rather than overwriting a pending one */
    }
    polyp_focus_index[(polyp_focus_head + polyp_focus_count) % POLYP_FOCUS_CAPACITY] = index;
    polyp_focus_count++;
}

/* The entry index of the oldest pending focus request, or -1 when none is. */
static inline int polyp_poll_focus(void) {
    if (polyp_focus_count == 0) {
        return -1;
    }
    int id = polyp_focus_index[polyp_focus_head];
    polyp_focus_head = (polyp_focus_head + 1) % POLYP_FOCUS_CAPACITY;
    polyp_focus_count--;
    return id;
}

#endif
