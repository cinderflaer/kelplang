/* sort-builtin: the same n values as the sort benchmark, sorted by each language's standard sort:
   Kelp's List.sort (introsort), C's qsort, Rust's slice::sort_unstable. Same checksum line shape.
   Size = n (default 5000000). */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

static int cmp(const void *pa, const void *pb) {
    int32_t a = *(const int32_t *)pa, b = *(const int32_t *)pb;
    return (a > b) - (a < b);
}

int main(int argc, char **argv) {
    long long n = argc > 1 ? atoll(argv[1]) : 5000000;
    int32_t *a = malloc((size_t)n * sizeof(int32_t));
    uint64_t x = 12345;
    for (long long i = 0; i < n; i++) {
        x = x * 6364136223846793005ULL + 1442695040888963407ULL;
        a[i] = (int32_t)(x >> 33);
    }
    qsort(a, (size_t)n, sizeof(int32_t), cmp);
    int ok = 1;
    uint64_t h = 0;
    for (size_t k = 0; k < (size_t)n; k++) {
        if (k > 0 && a[k - 1] > a[k]) ok = 0;
        h = h * 31 + (uint64_t)a[k];
    }
    printf("sort-builtin %lld %s %llu\n", n, ok ? "true" : "false", (unsigned long long)h);
    free(a);
    return 0;
}
