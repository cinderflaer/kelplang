// Three CPU-bound workloads, the same algorithm in all four languages.
//
// `RESULT <name> <ops> <work_ns> <checksum>` per workload; the checksums must
// agree across languages, which is what makes the timings comparable.
#include <chrono>
#include <cstdint>
#include <cstdio>

static const int64_t COLLATZ_N = 1000000;
static const int64_t FLOAT_ITERS = 100000000;
static const int64_t CALL_ITERS = 50000000;
static const int64_t FIB_N = 34;
static const double FLOAT_STEP = 1e-8;
static const double FLOAT_THRESHOLD = 0.75;

static uint64_t now_ns() {
    return (uint64_t)std::chrono::duration_cast<std::chrono::nanoseconds>(
               std::chrono::steady_clock::now().time_since_epoch())
        .count();
}

// Branchy integer work: ~100 dependent steps per start value.
static int64_t collatz_total(int64_t n_end) {
    int64_t total = 0;
    for (int64_t n = 1; n <= n_end; n++) {
        int64_t x = n;
        int64_t steps = 0;
        while (x != 1) {
            if (x % 2 == 0) {
                x /= 2;
            } else {
                x = x * 3 + 1;
            }
            steps++;
        }
        total += steps;
    }
    return total;
}

// Floating point: one division and one add per step, plus a count that
// consumes every value (and keeps the loop from being optimised away).
static double float_sum(int64_t iters, int64_t* above_out) {
    double x = 0.0;
    double acc = 0.0;
    int64_t above = 0;
    for (int64_t i = 0; i < iters; i++) {
        x += FLOAT_STEP;
        double v = 1.0 / (1.0 + x * x);
        acc += v;
        if (v > FLOAT_THRESHOLD) {
            above++;
        }
    }
    *above_out = above;
    return acc;
}

// Call-heavy integer work: one small function call per iteration.
static inline int64_t mix(int64_t a, int64_t b) {
    return (a * 31 + b % 97) % 1000003;
}

static int64_t call_chain(int64_t iters) {
    int64_t a = 12345;
    for (int64_t i = 0; i < iters; i++) {
        a = mix(a, i);
    }
    return a;
}

// Recursion, naively exponential, with a counter that makes each call
// observable through a pointer. Without that side effect LLVM turns the
// recursion into a loop, which would make this a repetition of `mix` instead
// of a call-cost measurement. `calls` is also the operation count.
static int64_t fib(int64_t n, int64_t* calls) {
    (*calls)++;
    if (n < 2) {
        return n;
    }
    return fib(n - 1, calls) + fib(n - 2, calls);
}

int main() {
    uint64_t t = now_ns();
    int64_t c = collatz_total(COLLATZ_N);
    printf("RESULT collatz %lld %llu %lld\n", (long long)COLLATZ_N,
           (unsigned long long)(now_ns() - t), (long long)c);

    t = now_ns();
    int64_t above = 0;
    double acc = float_sum(FLOAT_ITERS, &above);
    printf("RESULT float %lld %llu %lld\n", (long long)FLOAT_ITERS,
           (unsigned long long)(now_ns() - t), (long long)above);
    printf("VALUE %.6f\n", acc);

    t = now_ns();
    c = call_chain(CALL_ITERS);
    printf("RESULT mix %lld %llu %lld\n", (long long)CALL_ITERS,
           (unsigned long long)(now_ns() - t), (long long)c);

    int64_t calls = 0;
    t = now_ns();
    c = fib(FIB_N, &calls);
    printf("RESULT fib %lld %llu %lld\n", (long long)calls,
           (unsigned long long)(now_ns() - t), (long long)c);
    return 0;
}
