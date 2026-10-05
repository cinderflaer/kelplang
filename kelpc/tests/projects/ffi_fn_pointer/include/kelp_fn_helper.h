static inline int kelp_apply(int (*callback)(int), int value) {
    return callback(value) + 1;
}

static inline int kelp_triple(int value) {
    return value * 3;
}

static inline int (*kelp_callback(void))(int) {
    return kelp_triple;
}
