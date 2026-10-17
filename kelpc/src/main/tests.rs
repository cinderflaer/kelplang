use super::*;

#[test]
fn a_crate_version_becomes_the_padded_release_name() {
    assert_eq!(release_name("26.9.0"), "26.09.0");
    assert_eq!(release_name("26.12.3"), "26.12.3");
    assert_eq!(release_name("26.1.10"), "26.01.10");
}

#[test]
fn a_version_that_is_not_this_scheme_is_left_alone() {
    // The numbering before `YY.MM.N`, and anything that is not three
    // numeric parts, is passed through rather than padded into nonsense.
    assert_eq!(release_name("0.1.6"), "0.1.6");
    assert_eq!(release_name("nightly"), "nightly");
    assert_eq!(release_name("26.9"), "26.9");
}

#[test]
fn this_builds_release_name_has_the_schemes_shape() {
    // Durable across a version bump: the shape is what the scheme pins,
    // not the number this tree happens to carry today.
    let name = release_name(VERSION);
    let parts: Vec<&str> = name.split('.').collect();
    assert_eq!(parts.len(), 3, "'{name}' is not YY.MM.N");
    assert_eq!(parts[0].len(), 2, "'{name}' has a one-digit year");
    assert_eq!(parts[1].len(), 2, "'{name}' has a one-digit month");
    assert!(
        parts[2].chars().all(|c| c.is_ascii_digit()) && !parts[2].is_empty(),
        "'{name}' has no release number"
    );
}
