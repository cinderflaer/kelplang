// spectral-norm: the Benchmarks Game spectral norm, 10 power iterations. Size = n (default 1000).
fn eval_a(i: i64, j: i64) -> f64 {
    1.0 / (((i + j) * (i + j + 1) / 2 + i + 1) as f64)
}

fn mul_av(v: &[f64], out: &mut [f64], n: i64) {
    for i in 0..n {
        let mut s = 0.0f64;
        for j in 0..n {
            s = s + eval_a(i, j) * v[j as usize];
        }
        out[i as usize] = s;
    }
}

fn mul_atv(v: &[f64], out: &mut [f64], n: i64) {
    for i in 0..n {
        let mut s = 0.0f64;
        for j in 0..n {
            s = s + eval_a(j, i) * v[j as usize];
        }
        out[i as usize] = s;
    }
}

fn mul_atav(v: &[f64], out: &mut [f64], tmp: &mut [f64], n: i64) {
    mul_av(v, tmp, n);
    mul_atv(tmp, out, n);
}

fn main() {
    let n: i64 = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(1000);
    let mut u = vec![1.0f64; n as usize];
    let mut v = vec![0.0f64; n as usize];
    let mut tmp = vec![0.0f64; n as usize];
    for _ in 0..10 {
        mul_atav(&u, &mut v, &mut tmp, n);
        mul_atav(&v, &mut u, &mut tmp, n);
    }
    let (mut vbv, mut vv) = (0.0f64, 0.0f64);
    for i in 0..n as usize {
        vbv = vbv + u[i] * v[i];
        vv = vv + v[i] * v[i];
    }
    println!("spectral-norm {} {:.9}", n, (vbv / vv).sqrt());
}
