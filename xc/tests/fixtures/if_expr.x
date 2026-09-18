fn classify(n: i32) -> i32 {
    let result = if n < 0 {
        -1
    } else if n == 0 {
        0
    } else {
        1
    };
    return result;
}

fn main() {
    println(classify(-5));
    println(classify(0));
    println(classify(42));
}
