/* sort: n pseudo-random non-negative i32 (64-bit LCG, high bits), sorted by a hand-written quicksort
   (median-of-three pivot, Lomuto partition, recurse on the smaller side, insertion sort below 17
   elements), identical in Kelp, C and Rust. Prints the sorted flag and a hash of the sorted data.
   Size = n (default 5000000). */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

static void insertion(int32_t *a, size_t lo, size_t hi) {
    for (size_t i = lo + 1; i < hi; i++) {
        int32_t x = a[i];
        size_t j = i;
        while (j > lo && a[j - 1] > x) {
            a[j] = a[j - 1];
            j--;
        }
        a[j] = x;
    }
}

static void swap(int32_t *a, size_t i, size_t j) {
    int32_t t = a[i];
    a[i] = a[j];
    a[j] = t;
}

static void quicksort(int32_t *a, size_t lo, size_t hi) {
    while (hi - lo > 16) {
        size_t mid = lo + (hi - lo) / 2;
        int32_t x = a[lo], y = a[mid], z = a[hi - 1];
        size_t m;
        if ((x < y && y < z) || (z < y && y < x)) m = mid;
        else if ((y < x && x < z) || (z < x && x < y)) m = lo;
        else m = hi - 1;
        swap(a, m, hi - 1);
        int32_t p = a[hi - 1];
        size_t store = lo;
        for (size_t i = lo; i < hi - 1; i++) {
            if (a[i] < p) {
                swap(a, i, store);
                store++;
            }
        }
        swap(a, store, hi - 1);
        if (store - lo < hi - (store + 1)) {
            quicksort(a, lo, store);
            lo = store + 1;
        } else {
            quicksort(a, store + 1, hi);
            hi = store;
        }
    }
    insertion(a, lo, hi);
}

int main(int argc, char **argv) {
    long long n = argc > 1 ? atoll(argv[1]) : 5000000;
    int32_t *a = malloc((size_t)n * sizeof(int32_t));
    uint64_t x = 12345;
    for (long long i = 0; i < n; i++) {
        x = x * 6364136223846793005ULL + 1442695040888963407ULL;
        a[i] = (int32_t)(x >> 33);
    }
    quicksort(a, 0, (size_t)n);
    int ok = 1;
    uint64_t h = 0;
    for (size_t k = 0; k < (size_t)n; k++) {
        if (k > 0 && a[k - 1] > a[k]) ok = 0;
        h = h * 31 + (uint64_t)a[k];
    }
    printf("sort %lld %s %llu\n", n, ok ? "true" : "false", (unsigned long long)h);
    free(a);
    return 0;
}
