// hashmap: insert n u64 keys (key_i = i * 0x9E3779B97F4A7C15, i = 1..n, value i), look every key up
// and sum the values, then look up n absent keys and count hits. Rust's std HashMap (default SipHash).
// Size = n keys (default 2000000).
use std::collections::HashMap;

fn main() {
    let n: i64 = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(2000000);
    const MUL: u64 = 0x9E3779B97F4A7C15;
    let mut m: HashMap<u64, u64> = HashMap::new();
    for i in 1..=n {
        let u = i as u64;
        m.insert(u.wrapping_mul(MUL), u);
    }
    let mut sum: u64 = 0;
    for i in 1..=n {
        sum = sum.wrapping_add(m[&(i as u64).wrapping_mul(MUL)]);
    }
    let mut hits: i64 = 0;
    for i in (n + 1)..=(2 * n) {
        if m.contains_key(&(i as u64).wrapping_mul(MUL)) {
            hits += 1;
        }
    }
    println!("hashmap {} {} {} {}", n, m.len(), sum, hits);
}
