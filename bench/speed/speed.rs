// Three CPU-bound workloads, the same algorithm in all four languages.
//
// `RESULT <name> <ops> <work_ns> <checksum>` per workload; the checksums must
// agree across languages, which is what makes the timings comparable.
use std::time::Instant;

const COLLATZ_N: i64 = 1_000_000;
const FLOAT_ITERS: i64 = 100_000_000;
const CALL_ITERS: i64 = 50_000_000;
const FIB_N: i64 = 34;
const FLOAT_STEP: f64 = 1e-8;
const FLOAT_THRESHOLD: f64 = 0.75;

// Branchy integer work: ~100 dependent steps per start value.
fn collatz_total(n_end: i64) -> i64 {
    let mut total = 0i64;
    let mut n = 1i64;
    while n <= n_end {
        let mut x = n;
        let mut steps = 0i64;
        while x != 1 {
            if x % 2 == 0 {
                x /= 2;
            } else {
                x = x * 3 + 1;
            }
            steps += 1;
        }
        total += steps;
        n += 1;
    }
    total
}

// Floating point: one division and one add per step, plus a count that
// consumes every value (and keeps the loop from being optimised away).
fn float_sum(iters: i64) -> (f64, i64) {
    let mut x = 0.0f64;
    let mut acc = 0.0f64;
    let mut above = 0i64;
    let mut i = 0i64;
    while i < iters {
        x += FLOAT_STEP;
        let v = 1.0 / (1.0 + x * x);
        acc += v;
        if v > FLOAT_THRESHOLD {
            above += 1;
        }
        i += 1;
    }
    (acc, above)
}

// Call-heavy integer work: one small function call per iteration.
fn mix(a: i64, b: i64) -> i64 {
    (a * 31 + b % 97) % 1_000_003
}

fn call_chain(iters: i64) -> i64 {
    let mut a = 12345i64;
    let mut i = 0i64;
    while i < iters {
        a = mix(a, i);
        i += 1;
    }
    a
}

// Recursion, naively exponential, with a counter that makes each call
// observable through a pointer. Without that side effect LLVM turns the
// recursion into a loop (measured: 0.5 ns "per call"), which would make this
// a repetition of the `mix` workload instead of a call-cost measurement.
// `calls` is also the operation count, so it is real work, not bookkeeping.
fn fib(n: i64, calls: &mut i64) -> i64 {
    *calls += 1;
    if n < 2 {
        n
    } else {
        fib(n - 1, calls) + fib(n - 2, calls)
    }
}

fn main() {
    let t = Instant::now();
    let c = collatz_total(COLLATZ_N);
    println!("RESULT collatz {COLLATZ_N} {} {c}", t.elapsed().as_nanos());

    let t = Instant::now();
    let (acc, above) = float_sum(FLOAT_ITERS);
    let ns = t.elapsed().as_nanos();
    println!("RESULT float {FLOAT_ITERS} {ns} {above}");
    println!("VALUE {acc:.6}");

    let t = Instant::now();
    let c = call_chain(CALL_ITERS);
    println!("RESULT mix {CALL_ITERS} {} {c}", t.elapsed().as_nanos());

    let mut calls = 0i64;
    let t = Instant::now();
    let c = fib(FIB_N, &mut calls);
    let ns = t.elapsed().as_nanos();
    println!("RESULT fib {calls} {ns} {c}");
}
