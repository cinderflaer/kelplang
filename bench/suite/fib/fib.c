/* fib: naive recursive Fibonacci. Size = n (default 38). */
#include <stdio.h>
#include <stdlib.h>

static long long fib(int n) {
    if (n < 2) return n;
    return fib(n - 1) + fib(n - 2);
}

int main(int argc, char **argv) {
    int n = argc > 1 ? atoi(argv[1]) : 38;
    long long r = fib(n);
    printf("fib %d %lld\n", n, r);
    return 0;
}
