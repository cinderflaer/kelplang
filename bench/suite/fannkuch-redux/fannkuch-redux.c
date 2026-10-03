/* fannkuch-redux: the Benchmarks Game pancake-flipping benchmark. Prints the checksum and max flips.
   Size = n (default 10). */
#include <stdio.h>
#include <stdlib.h>

int main(int argc, char **argv) {
    int n = argc > 1 ? atoi(argv[1]) : 10;
    int *perm1 = malloc(n * sizeof(int));
    int *perm = malloc(n * sizeof(int));
    int *count = malloc(n * sizeof(int));
    for (int i = 0; i < n; i++) { perm1[i] = i; perm[i] = 0; count[i] = 0; }
    int r = n;
    long long perm_count = 0, checksum = 0;
    int max_flips = 0;
    int done = 0;
    while (!done) {
        while (r != 1) {
            count[r - 1] = r;
            r = r - 1;
        }
        for (int i = 0; i < n; i++) perm[i] = perm1[i];
        int flips = 0;
        int k = perm[0];
        while (k != 0) {
            int lo = 0, hi = k;
            while (lo < hi) {
                int t = perm[lo];
                perm[lo] = perm[hi];
                perm[hi] = t;
                lo++;
                hi--;
            }
            flips++;
            k = perm[0];
        }
        if (flips > max_flips) max_flips = flips;
        if (perm_count % 2 == 0) checksum += flips; else checksum -= flips;
        int more = 1;
        while (more) {
            if (r == n) {
                done = 1;
                more = 0;
            } else {
                int p0 = perm1[0];
                for (int i = 0; i < r; i++) perm1[i] = perm1[i + 1];
                perm1[r] = p0;
                int c = count[r] - 1;
                count[r] = c;
                if (c > 0) more = 0; else r = r + 1;
            }
        }
        perm_count++;
    }
    printf("fannkuch-redux %d %lld %d\n", n, checksum, max_flips);
    free(perm1); free(perm); free(count);
    return 0;
}
