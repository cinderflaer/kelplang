/* Step 0 (D53): a C helper that calls a closure, then its destroy notify.
 * The closure's environment is valid only while this call runs. */
static inline int kelp_call_twice(void (*fn)(int, void*), void* data, void (*destroy)(void*), int x) {
    fn(x, data);
    fn(x + 1, data);
    if (destroy) destroy(data);
    return 2;
}
