static inline int kelp_call_twice(void (*fn)(int, void*), void* data, void (*destroy)(void*), int x) {
    fn(x, data);
    fn(x + 1, data);
    if (destroy) destroy(data);
    return 2;
}
