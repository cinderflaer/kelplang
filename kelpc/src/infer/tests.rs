use super::is_shipped_module;

/// The export-everything rule keys on this predicate, and a project file
/// under a `std/` path is a project file: its id must not pass for the
/// compiler's own text, or privacy stops applying to it (A-13).
#[test]
fn only_the_compilers_own_text_is_shipped() {
    assert!(is_shipped_module("std/mem"));
    assert!(is_shipped_module("std/list"));
    // The injected runtime (`stdlib::RT_MODULE`) ships like the rest.
    assert!(is_shipped_module(crate::stdlib::RT_MODULE));
    assert!(!is_shipped_module("std/mine"));
    assert!(!is_shipped_module("std"));
    assert!(!is_shipped_module("lib"));
}
