// json-tokenizer: generate a JSON-style text of at least n bytes (records
// {"id":I,"name":"userJ","tags":["a","b\"c"],"score":K.5,"ok":true,"n":null}, one per line), then
// tokenize it with a byte-at-a-time state machine, counting punctuation, strings, numbers and
// literals and hashing the token stream. Generation is included in the timing, in every language.
// Size = n bytes (default 20000000).
fn is_digit(c: u8) -> bool {
    c >= 48 && c <= 57
}

fn is_num_char(c: u8) -> bool {
    (c >= 48 && c <= 57) || c == 46 || c == 101 || c == 69 || c == 43 || c == 45
}

fn is_alpha(c: u8) -> bool {
    c >= 97 && c <= 122
}

fn main() {
    let target: usize = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(20000000);
    let mut s = String::new();
    let mut i: i64 = 0;
    while s.len() < target {
        s.push_str("{\"id\":");
        s.push_str(&i.to_string());
        s.push_str(",\"name\":\"user");
        s.push_str(&(i % 1000).to_string());
        s.push_str("\",\"tags\":[\"a\",\"b\\\"c\"],\"score\":");
        s.push_str(&(i % 97).to_string());
        s.push_str(".5,\"ok\":true,\"n\":null},\n");
        i += 1;
    }
    let text_len = s.len();
    let t = s.as_bytes();
    let (mut punct, mut strings, mut numbers, mut literals, mut errors, mut str_bytes) =
        (0i64, 0i64, 0i64, 0i64, 0i64, 0i64);
    let mut h: u64 = 0;
    let mut p: usize = 0;
    while p < text_len {
        let ch = t[p];
        if ch == 32 || ch == 10 || ch == 13 || ch == 9 {
            p += 1;
        } else if ch == 123 || ch == 125 || ch == 91 || ch == 93 || ch == 58 || ch == 44 {
            punct += 1;
            h = h.wrapping_mul(31).wrapping_add(ch as u64);
            p += 1;
        } else if ch == 34 {
            p += 1;
            let start = p;
            while p < text_len && t[p] != 34 {
                if t[p] == 92 {
                    p += 1;
                }
                p += 1;
            }
            strings += 1;
            str_bytes += (p - start) as i64;
            h = h.wrapping_mul(31).wrapping_add(34);
            p += 1;
        } else if is_digit(ch) || ch == 45 {
            while p < text_len && is_num_char(t[p]) {
                p += 1;
            }
            numbers += 1;
            h = h.wrapping_mul(31).wrapping_add(48);
        } else if is_alpha(ch) {
            let start = p;
            while p < text_len && is_alpha(t[p]) {
                p += 1;
            }
            literals += 1;
            h = h.wrapping_mul(31).wrapping_add((p - start) as u64);
        } else {
            errors += 1;
            p += 1;
        }
    }
    println!(
        "json-tokenizer {} {} {} {} {} {} {} {} {}",
        target, text_len, punct, strings, str_bytes, numbers, literals, errors, h
    );
}
