// mandelbrot: escape-time count over an n x n grid of [-1.5,0.5] x [-1,1], 50 iterations max.
// Prints the number of points that never escape and the total iteration count. Size = n (default 1000).
fn main() {
    let n: i64 = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(1000);
    let nf = n as f64;
    let mut inside: i64 = 0;
    let mut total: i64 = 0;
    for py in 0..n {
        let ci = 2.0 * (py as f64) / nf - 1.0;
        for px in 0..n {
            let cr = 2.0 * (px as f64) / nf - 1.5;
            let (mut zr, mut zi) = (0.0f64, 0.0f64);
            let mut it: i64 = 0;
            while it < 50 && zr * zr + zi * zi <= 4.0 {
                let t = zr * zr - zi * zi + cr;
                zi = 2.0 * zr * zi + ci;
                zr = t;
                it += 1;
            }
            total += it;
            if it == 50 {
                inside += 1;
            }
        }
    }
    println!("mandelbrot {} {} {}", n, inside, total);
}
