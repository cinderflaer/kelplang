# Three CPU-bound workloads, the same algorithm in all four languages.
#
# `RESULT <name> <ops> <work_ns> <checksum>` per workload; the checksums must
# agree across languages, which is what makes the timings comparable.
import time

COLLATZ_N = 1_000_000
FLOAT_ITERS = 100_000_000
CALL_ITERS = 50_000_000
FIB_N = 34
FLOAT_STEP = 1e-8
FLOAT_THRESHOLD = 0.75


def collatz_total(n_end):
    """Branchy integer work: ~100 dependent steps per start value."""
    total = 0
    n = 1
    while n <= n_end:
        x = n
        steps = 0
        while x != 1:
            if x % 2 == 0:
                x //= 2
            else:
                x = x * 3 + 1
            steps += 1
        total += steps
        n += 1
    return total


def float_sum(iters):
    """Floating point: one division and one add per step, plus a count that
    consumes every value."""
    x = 0.0
    acc = 0.0
    above = 0
    i = 0
    while i < iters:
        x += FLOAT_STEP
        v = 1.0 / (1.0 + x * x)
        acc += v
        if v > FLOAT_THRESHOLD:
            above += 1
        i += 1
    return acc, above


def mix(a, b):
    return (a * 31 + b % 97) % 1_000_003


def call_chain(iters):
    """Call-heavy integer work: one small function call per iteration."""
    a = 12345
    i = 0
    while i < iters:
        a = mix(a, i)
        i += 1
    return a


FIB_CALLS = 0


def fib(n):
    """Recursion, naively exponential. The counter keeps every call
    observable, the same way the pointer parameter does in the other three."""
    global FIB_CALLS
    FIB_CALLS += 1
    if n < 2:
        return n
    return fib(n - 1) + fib(n - 2)


def main():
    t = time.perf_counter_ns()
    c = collatz_total(COLLATZ_N)
    print(f"RESULT collatz {COLLATZ_N} {time.perf_counter_ns() - t} {c}")

    t = time.perf_counter_ns()
    acc, above = float_sum(FLOAT_ITERS)
    print(f"RESULT float {FLOAT_ITERS} {time.perf_counter_ns() - t} {above}")
    print(f"VALUE {acc:.6f}")

    t = time.perf_counter_ns()
    c = call_chain(CALL_ITERS)
    print(f"RESULT mix {CALL_ITERS} {time.perf_counter_ns() - t} {c}")

    global FIB_CALLS
    FIB_CALLS = 0
    t = time.perf_counter_ns()
    c = fib(FIB_N)
    print(f"RESULT fib {FIB_CALLS} {time.perf_counter_ns() - t} {c}")


if __name__ == "__main__":
    main()
