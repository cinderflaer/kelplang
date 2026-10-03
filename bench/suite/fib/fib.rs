// fib: naive recursive Fibonacci. Size = n (default 38).
fn fib(n: i32) -> i64 {
    if n < 2 {
        return n as i64;
    }
    fib(n - 1) + fib(n - 2)
}

fn main() {
    let n: i32 = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(38);
    let r = fib(n);
    println!("fib {} {}", n, r);
}
