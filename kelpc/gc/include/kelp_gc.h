/* kelp_gc.h: the C view of the managed-mode collector (WP1, design paper
   section 3.2 and 3.4). The implementation is Rust (kelpc/gc, staticlib
   libkelp_gc.a); this header is the contract WP3 emits calls against.

   Pointers handed to and returned by kelp_gc_alloc point at the PAYLOAD. The
   8-byte header sits immediately before it. Payload alignment is 8.

   Limits (checked by the collector and by kelpc/tests/gc):
   - Type ids: the header holds 24 bits, so ids run to KELP_GC_TYPE_ID_MAX. The
     v1 type table holds ids below KELP_GC_TYPE_ID_V1_LIMIT. An id at or above
     the limit is a fatal error ("panic: gc: type id N is over the v1 type table
     limit"), not a silent miss.
   - Enum tags: a u32 at tag_off, KELP_GC_ENUM_TAG_BYTES wide. The lowering must
     emit a 4-byte tag for every enum whose descriptor it writes. A repr(u8) or
     repr(u64) enum tag does not match this descriptor.
   - Objects whose total size (header included) is above KELP_GC_LARGE_MIN go to
     the run space. The collector decides this from the size; WP3 does not
     need to know the threshold.

   Threads: every thread that allocates, or that may be running Kelp code,
   calls kelp_gc_thread_attach before its first allocation. An allocation on an
   unattached thread is fatal. An attached thread must reach a safepoint
   (kelp_gc_safepoint) or a native bracket (kelp_gc_enter_native) in every loop
   that can run long; a collection that waits longer than the stop timeout
   (KELP_GC_STOP_TIMEOUT_MS, default 10000) is fatal and names the thread.

   Environment (read once, at the first initialization):
   - KELP_GC_STRESS=N: a full cycle every N allocations (0 or unset: off).
   - KELP_GC_VERIFY=1: the heap verifier runs after every cycle.
   - KELP_GC_STOP_TIMEOUT_MS=N: the stop timeout in milliseconds. */
#ifndef KELP_GC_H
#define KELP_GC_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Limits. */
#define KELP_GC_TYPE_ID_MAX 16777215u
#define KELP_GC_TYPE_ID_V1_LIMIT 1048576u
#define KELP_GC_ENUM_TAG_BYTES 4u
#define KELP_GC_LARGE_MIN 8192u

/* Descriptor flags. */
#define KELP_GC_NOSCAN 0x1u /* payload holds no GC pointer slots; never traced */
#define KELP_GC_ENUM 0x2u   /* enum: tag at tag_off selects a variant */

/* Header word layout (64 bits, one word before the payload). */
#define KELP_GC_HDR_MARK (1ull << 0)    /* mark bit: equals the collector epoch when live */
#define KELP_GC_HDR_STICKY (1ull << 1)  /* phase 3 only; zero in phase 1 */
#define KELP_GC_HDR_NOSCAN (1ull << 2)
#define KELP_GC_HDR_ARRAY (1ull << 3)   /* count is in bits 32..63 */
#define KELP_GC_HDR_LARGE (1ull << 4)   /* object is above KELP_GC_LARGE_MIN: run space */
#define KELP_GC_HDR_TYPE_SHIFT 8        /* type id: bits 8..31 (24 bits) */
#define KELP_GC_HDR_COUNT_SHIFT 32      /* element count: bits 32..63 */

typedef struct kelp_gc_variant {
    uint32_t n_ptrs;
    const uint32_t* ptr_off; /* byte offsets from the payload start */
} kelp_gc_variant;

typedef struct kelp_gc_type {
    uint32_t id;                    /* 24-bit type id stored in the header; the v1 table holds ids below KELP_GC_TYPE_ID_V1_LIMIT */
    uint32_t size;                  /* fixed object size, or element size for arrays (bytes) */
    uint32_t align;                 /* power of two, at most 8 in v1 */
    uint32_t flags;                 /* KELP_GC_NOSCAN, KELP_GC_ENUM */
    uint32_t n_ptrs;
    const uint32_t* ptr_off;        /* datac fields, Box payload, closure env, array elements */
    uint32_t tag_off;               /* enums only: u32 (KELP_GC_ENUM_TAG_BYTES) tag at this payload offset */
    uint32_t n_variants;            /* enums only; indexed by the tag value */
    const kelp_gc_variant* variants;
} kelp_gc_type;

/* Phase 1 API. heap_max of 0 selects the default 256 MiB; heap_min of 0
   selects 8 MiB (the collection trigger floor). Call before any allocation.
   A later call is ignored. */
void kelp_gc_init(size_t heap_min, size_t heap_max);
void* kelp_gc_alloc(const kelp_gc_type* t);                          /* one zeroed object of type t */
void* kelp_gc_alloc_array(const kelp_gc_type* elem, size_t count);   /* zeroed array of elem */
void kelp_gc_collect(void);                                          /* full stop-the-world cycle */
void kelp_gc_safepoint(void);                                        /* poll; one atomic load when idle */
void kelp_gc_enter_native(void);                                     /* before a blocking call */
void kelp_gc_leave_native(void);                                     /* after it returns */
void kelp_gc_register_root_region(void* base, const kelp_gc_type* layout);
void kelp_gc_unregister_root_region(void* base);
int kelp_gc_is_heap_ptr(const void* p);                              /* conservative and debug use */

/* Additions beyond the paper's list, needed by the thread model:
   every thread that allocates or may be running Kelp code calls attach once
   before its first allocation and detach before it exits. The collector sees
   only attached threads' stacks; an allocation on an unattached thread is
   fatal. */
void kelp_gc_thread_attach(void);
void kelp_gc_thread_detach(void);

/* Out-of-memory and internal-error hook. kelp_rt's kelp_panic is static, so the
   runtime installs its own handler here. The hook receives a NUL-terminated
   message and is expected not to return. With no hook the collector writes
   "panic: <msg>" to stderr and exits with status 1, matching kelp_rt's main
   thread output. */
void kelp_gc_set_panic_hook(void (*hook)(const char* msg));

#ifdef __cplusplus
}
#endif

#endif /* KELP_GC_H */
