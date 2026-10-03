/* hashmap: insert n u64 keys (key_i = i * 0x9E3779B97F4A7C15, i = 1..n, value i), look every key up
   and sum the values, then look up n absent keys and count hits. Size = n keys (default 2000000).
   NOTE: there is no hash map in C's library, so this file carries a simple open-addressing table
   (linear probing, power-of-two capacity >= 2n, multiplicative hash, key 0 = empty; keys here
   are never 0). It is not tuned; Kelp uses its builtin Map, Rust its std HashMap (SipHash). */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

typedef struct { uint64_t *keys, *vals; size_t mask; size_t len; } Table;

static size_t slot_of(const Table *t, uint64_t k) {
    return (size_t)((k * 0xD6E8FEB86659FD93ULL) >> 20) & t->mask;
}

static void put(Table *t, uint64_t k, uint64_t v) {
    size_t i = slot_of(t, k);
    while (t->keys[i] != 0 && t->keys[i] != k) i = (i + 1) & t->mask;
    if (t->keys[i] == 0) { t->keys[i] = k; t->len++; }
    t->vals[i] = v;
}

static int find(const Table *t, uint64_t k, uint64_t *out) {
    size_t i = slot_of(t, k);
    while (t->keys[i] != 0) {
        if (t->keys[i] == k) { *out = t->vals[i]; return 1; }
        i = (i + 1) & t->mask;
    }
    return 0;
}

int main(int argc, char **argv) {
    long long n = argc > 1 ? atoll(argv[1]) : 2000000;
    size_t cap = 16;
    while (cap < (size_t)n * 2) cap *= 2;
    Table t;
    t.keys = calloc(cap, sizeof(uint64_t));
    t.vals = calloc(cap, sizeof(uint64_t));
    t.mask = cap - 1;
    t.len = 0;
    const uint64_t MUL = 0x9E3779B97F4A7C15ULL;
    for (long long i = 1; i <= n; i++) put(&t, (uint64_t)i * MUL, (uint64_t)i);
    uint64_t sum = 0, v = 0;
    for (long long i = 1; i <= n; i++) {
        if (find(&t, (uint64_t)i * MUL, &v)) sum += v;
    }
    long long hits = 0;
    for (long long i = n + 1; i <= 2 * n; i++) {
        if (find(&t, (uint64_t)i * MUL, &v)) hits++;
    }
    printf("hashmap %lld %zu %llu %lld\n", n, t.len, (unsigned long long)sum, hits);
    free(t.keys); free(t.vals);
    return 0;
}
