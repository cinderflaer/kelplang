// matmul: C = A * B for n x n f64 matrices, naive i-j-k loops over row-major flat storage.
// Prints the sum of all elements of C and its trace. Size = n (default 400).
fn main() {
    let n: i64 = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(400);
    let nu = n as usize;
    let mut a = vec![0.0f64; nu * nu];
    let mut b = vec![0.0f64; nu * nu];
    let mut c = vec![0.0f64; nu * nu];
    for i in 0..nu {
        for j in 0..nu {
            a[i * nu + j] = ((i + j) % 7) as f64 * 0.25;
            b[i * nu + j] = ((i * 3 + j) % 5) as f64 * 0.5;
        }
    }
    for i in 0..nu {
        for j in 0..nu {
            let mut s = 0.0f64;
            for k in 0..nu {
                s = s + a[i * nu + k] * b[k * nu + j];
            }
            c[i * nu + j] = s;
        }
    }
    let (mut sum, mut trace) = (0.0f64, 0.0f64);
    for i in 0..nu {
        for j in 0..nu {
            sum = sum + c[i * nu + j];
        }
        trace = trace + c[i * nu + i];
    }
    println!("matmul {} {:.3} {:.3}", n, sum, trace);
}
