// sieve: sieve of Eratosthenes over a byte-per-number table; prints the prime count.
// Size = limit (default 50000000).
fn main() {
    let n: i64 = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(50000000);
    let mut composite = vec![0u8; (n + 1) as usize];
    let mut i: i64 = 2;
    while i * i <= n {
        if composite[i as usize] == 0 {
            let mut j = i * i;
            while j <= n {
                composite[j as usize] = 1;
                j += i;
            }
        }
        i += 1;
    }
    let mut count: i64 = 0;
    let mut i: i64 = 2;
    while i <= n {
        if composite[i as usize] == 0 {
            count += 1;
        }
        i += 1;
    }
    println!("sieve {} {}", n, count);
}
