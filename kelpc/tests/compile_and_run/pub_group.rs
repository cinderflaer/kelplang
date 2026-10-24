//! A group of items written `pub { ... }` exports each item as if `pub` were
//! written in front of it (issue #290).

use super::*;

/// Every item in the group is reachable by its alias from another module, and
/// the item outside the group is not used here.
#[test]
pub(crate) fn a_pub_group_exports_each_item_to_another_module() {
    assert_project_runs("pub_group", &project("pub_group"), "10\n7\n20\n2\nsquare\n");
}

/// An item written outside the group stays private to its module.
#[test]
pub(crate) fn an_item_outside_a_pub_group_stays_private() {
    assert_project_rejected("pub_group_hidden", &project("pub_group_hidden"), "'shapes.hidden' is private");
}
