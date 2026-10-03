/* sieve: sieve of Eratosthenes over a byte-per-number table; prints the prime count.
   Size = limit (default 50000000). */
#include <stdio.h>
#include <stdlib.h>

int main(int argc, char **argv) {
    long long n = argc > 1 ? atoll(argv[1]) : 50000000;
    unsigned char *composite = calloc((size_t)n + 1, 1);
    for (long long i = 2; i * i <= n; i++) {
        if (composite[i] == 0) {
            for (long long j = i * i; j <= n; j += i) composite[j] = 1;
        }
    }
    long long count = 0;
    for (long long i = 2; i <= n; i++) {
        if (composite[i] == 0) count++;
    }
    printf("sieve %lld %lld\n", n, count);
    free(composite);
    return 0;
}
