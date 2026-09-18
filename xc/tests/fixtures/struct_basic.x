struct Point {
    x: i32,
    y: i32,
}

fn dist_sq(p: Point) -> i32 {
    return p.x * p.x + p.y * p.y;
}

fn main() {
    let p = Point { x: 3, y: 4 };
    println(p.x);
    println(p.y);
    println(dist_sq(p));

    let mut q = Point { x: 1, y: 1 };
    q.x = 10;
    println(q.x + q.y);
}
