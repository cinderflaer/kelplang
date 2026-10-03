// strings: build one string of n comma-separated pieces ("p<i>,"), split it into n owned strings,
// then fold every byte of every piece into a hash. Size = n pieces (default 1000000).
// The pieces are collected as owned `String`s (like Kelp's `List<String>`), not borrowed `&str`s.
fn main() {
    let n: i64 = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(1000000);
    let mut s = String::new();
    for i in 0..n {
        s.push_str("p");
        s.push_str(&i.to_string());
        s.push_str(",");
    }
    let text_len = s.len();
    let pieces: Vec<String> = s.split(',').map(String::from).collect();
    let mut count: i64 = 0;
    let mut total_len: i64 = 0;
    let mut h: u64 = 0;
    for p in pieces.iter() {
        let b = p.as_bytes();
        if b.len() > 0 {
            count += 1;
            total_len += b.len() as i64;
            for &c in b {
                h = h.wrapping_mul(31).wrapping_add(c as u64);
            }
        }
    }
    println!("strings {} {} {} {} {}", n, text_len, count, total_len, h);
}
