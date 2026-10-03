// sort: n pseudo-random non-negative i32 (64-bit LCG, high bits), sorted by a hand-written quicksort
// (median-of-three pivot, Lomuto partition, recurse on the smaller side, insertion sort below 17
// elements), identical in Kelp, C and Rust. Prints the sorted flag and a hash of the sorted data.
// Size = n (default 5000000).
fn insertion(a: &mut [i32], lo: usize, hi: usize) {
    for i in (lo + 1)..hi {
        let x = a[i];
        let mut j = i;
        while j > lo && a[j - 1] > x {
            a[j] = a[j - 1];
            j -= 1;
        }
        a[j] = x;
    }
}

fn quicksort(a: &mut [i32], lo0: usize, hi0: usize) {
    let (mut lo, mut hi) = (lo0, hi0);
    while hi - lo > 16 {
        let mid = lo + (hi - lo) / 2;
        let (x, y, z) = (a[lo], a[mid], a[hi - 1]);
        let m = if (x < y && y < z) || (z < y && y < x) {
            mid
        } else if (y < x && x < z) || (z < x && x < y) {
            lo
        } else {
            hi - 1
        };
        a.swap(m, hi - 1);
        let p = a[hi - 1];
        let mut store = lo;
        for i in lo..(hi - 1) {
            if a[i] < p {
                a.swap(i, store);
                store += 1;
            }
        }
        a.swap(store, hi - 1);
        if store - lo < hi - (store + 1) {
            quicksort(a, lo, store);
            lo = store + 1;
        } else {
            quicksort(a, store + 1, hi);
            hi = store;
        }
    }
    insertion(a, lo, hi);
}

fn main() {
    let n: i64 = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(5000000);
    let mut a: Vec<i32> = Vec::new();
    let mut x: u64 = 12345;
    for _ in 0..n {
        x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        a.push((x >> 33) as i32);
    }
    let len = a.len();
    quicksort(&mut a, 0, len);
    let mut ok = true;
    let mut h: u64 = 0;
    for k in 0..len {
        if k > 0 && a[k - 1] > a[k] {
            ok = false;
        }
        h = h.wrapping_mul(31).wrapping_add(a[k] as u64);
    }
    println!("sort {} {} {}", n, ok, h);
}
