use super::{jobs_from, panic_message};

#[test]
fn jobs_default_to_two_and_accept_a_positive_count() {
    assert_eq!(jobs_from(None), 2);
    assert_eq!(jobs_from(Some("")), 2);
    assert_eq!(jobs_from(Some("1")), 1);
    assert_eq!(jobs_from(Some("4")), 4);
    assert_eq!(jobs_from(Some(" 3 ")), 3);
}

#[test]
fn a_bad_jobs_value_is_the_default() {
    for value in ["0", "many", "-1", "2.5"] {
        assert_eq!(jobs_from(Some(value)), 2, "'{value}' should be the default");
    }
}

#[test]
fn a_panics_message_is_read_from_its_payload() {
    let owned = std::panic::catch_unwind(|| panic!("boom {}", 1)).unwrap_err();
    assert_eq!(panic_message(&*owned), "boom 1");
    let borrowed = std::panic::catch_unwind(|| std::panic::panic_any("plain")).unwrap_err();
    assert_eq!(panic_message(&*borrowed), "plain");
    let opaque = std::panic::catch_unwind(|| std::panic::panic_any(7u8)).unwrap_err();
    assert_eq!(panic_message(&*opaque), "unknown panic");
}
