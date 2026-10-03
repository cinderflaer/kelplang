// binary-trees: the Benchmarks Game allocation benchmark (min depth 4, stretch tree, long-lived tree).
// Prints the sum of every tree check. Size = max depth (default 16). One Box per node.
struct Node {
    left: Option<Box<Node>>,
    right: Option<Box<Node>>,
}

fn make(d: i32) -> Node {
    if d == 0 {
        return Node { left: None, right: None };
    }
    Node { left: Some(Box::new(make(d - 1))), right: Some(Box::new(make(d - 1))) }
}

fn check(n: &Node) -> i64 {
    match (&n.left, &n.right) {
        (Some(l), Some(r)) => 1 + check(l) + check(r),
        _ => 1,
    }
}

fn main() {
    let max_depth: i32 = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(16);
    let min_depth: i32 = 4;
    let mut total: i64 = 0;
    let stretch = make(max_depth + 1);
    total += check(&stretch);
    drop(stretch);
    let long_lived = make(max_depth);
    let mut d = min_depth;
    while d <= max_depth {
        let iters = 1i32 << (max_depth - d + min_depth);
        let mut sum: i64 = 0;
        for _ in 0..iters {
            let t = make(d);
            sum += check(&t);
        }
        total += sum;
        d += 2;
    }
    total += check(&long_lived);
    println!("binary-trees {} {}", max_depth, total);
}
