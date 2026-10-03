// sort-builtin: the same n values as the sort benchmark, sorted by each language's standard sort:
// Kelp's List.sort (introsort), C's qsort, Rust's slice::sort_unstable. Same checksum line shape.
// Size = n (default 5000000).
fn main() {
    let n: i64 = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(5000000);
    let mut a: Vec<i32> = Vec::new();
    let mut x: u64 = 12345;
    for _ in 0..n {
        x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        a.push((x >> 33) as i32);
    }
    a.sort_unstable();
    let mut ok = true;
    let mut h: u64 = 0;
    for k in 0..a.len() {
        if k > 0 && a[k - 1] > a[k] {
            ok = false;
        }
        h = h.wrapping_mul(31).wrapping_add(a[k] as u64);
    }
    println!("sort-builtin {} {} {}", n, ok, h);
}
