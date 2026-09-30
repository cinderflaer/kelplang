#ifndef KELP_LAYOUT_HELPER_H
#define KELP_LAYOUT_HELPER_H

#include <stddef.h>
#include <stdint.h>

// Kernel stage 2 (MK9-S1-M2): the packed header Kelp's `repr(packed)` datac
// declares, byte for byte. Every helper below reads a field or a size at the
// offset *this* compiler gives it, so a layout disagreement on either backend
// changes the number the program prints.
typedef struct __attribute__((packed)) kelp_layout_header {
    uint8_t kind;
    uint32_t length;
    uint16_t flags;
} kelp_layout_header;

static inline uint64_t kelp_layout_size(void) {
    return (uint64_t)sizeof(kelp_layout_header);
}

static inline uint64_t kelp_layout_align(void) {
    return (uint64_t)_Alignof(kelp_layout_header);
}

static inline uint64_t kelp_layout_offset_length(void) {
    return (uint64_t)offsetof(kelp_layout_header, length);
}

static inline uint64_t kelp_layout_offset_flags(void) {
    return (uint64_t)offsetof(kelp_layout_header, flags);
}

// Reads each field at C's own offset and folds them into one value, so reading
// a field from the wrong byte cannot produce the same number. The parameter is
// `const void*` so the header never has to name the generated C typedef for
// the Kelp datac.
static inline uint64_t kelp_layout_read(const void* raw) {
    const kelp_layout_header* h = (const kelp_layout_header*)raw;
    return (uint64_t)h->kind
         | ((uint64_t)h->length << 8)
         | ((uint64_t)h->flags << 40);
}

#endif
