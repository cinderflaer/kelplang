// fannkuch-redux: the Benchmarks Game pancake-flipping benchmark. Prints the checksum and max flips.
// Size = n (default 10).
fn main() {
    let n: i32 = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(10);
    let nu = n as usize;
    let mut perm1: Vec<i32> = (0..n).collect();
    let mut perm: Vec<i32> = vec![0; nu];
    let mut count: Vec<i32> = vec![0; nu];
    let mut r: i32 = n;
    let mut perm_count: i64 = 0;
    let mut checksum: i64 = 0;
    let mut max_flips: i32 = 0;
    let mut done = false;
    while !done {
        while r != 1 {
            count[(r - 1) as usize] = r;
            r -= 1;
        }
        for i in 0..nu {
            perm[i] = perm1[i];
        }
        let mut flips: i32 = 0;
        let mut k = perm[0];
        while k != 0 {
            let (mut lo, mut hi) = (0i32, k);
            while lo < hi {
                let t = perm[lo as usize];
                perm[lo as usize] = perm[hi as usize];
                perm[hi as usize] = t;
                lo += 1;
                hi -= 1;
            }
            flips += 1;
            k = perm[0];
        }
        if flips > max_flips {
            max_flips = flips;
        }
        if perm_count % 2 == 0 {
            checksum += flips as i64;
        } else {
            checksum -= flips as i64;
        }
        let mut more = true;
        while more {
            if r == n {
                done = true;
                more = false;
            } else {
                let p0 = perm1[0];
                for i in 0..(r as usize) {
                    perm1[i] = perm1[i + 1];
                }
                perm1[r as usize] = p0;
                let c = count[r as usize] - 1;
                count[r as usize] = c;
                if c > 0 {
                    more = false;
                } else {
                    r += 1;
                }
            }
        }
        perm_count += 1;
    }
    println!("fannkuch-redux {} {} {}", n, checksum, max_flips);
}
