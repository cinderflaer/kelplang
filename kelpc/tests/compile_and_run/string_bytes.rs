use super::*;

// std/str's byte iterator (issue #261): `for b in str.bytes(s)` walks the
// bytes of a str in order. The fixture covers the empty string, a multi-byte
// character, `iter.count` on the iterator, and `fmt.bytes` beside `str.bytes`.

#[test]
fn for_over_str_bytes_walks_every_byte_in_order() {
    assert_fixture_runs("tests/fixtures/str_bytes_iter.kp");
}
