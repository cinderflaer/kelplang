use super::*;

/// The table has a row for every module the compiler ships. A new
/// `stdlib::shipped()` entry without a decision here is `Hosted` by the
/// catch-all, which would silently refuse a module nobody looked at, so the
/// list of ids the table does *not* call `Hosted` is pinned here instead:
/// adding a shipped module forces a line in this test and therefore a look
/// at `kelp wiki page language/freestanding.md`.
#[test]
fn the_verdict_table_admits_exactly_the_listed_modules() {
    let mut admitted: Vec<&str> = crate::stdlib::shipped()
        .iter()
        .copied()
        .filter(|id| !matches!(std_admission(id), StdAdmission::Hosted))
        .collect();
    admitted.sort_unstable();
    assert_eq!(
        admitted,
        vec![
            "std/arena",
            "std/atomic",
            "std/convert",
            "std/frame",
            "std/irq",
            "std/iter",
            "std/list",
            "std/math",
            "std/mem",
            "std/ops",
            "std/option",
            "std/regex",
            "std/result",
            "std/str",
        ]
    );
    assert_eq!(
        allowed_modules(),
        "std/arena, std/atomic, std/convert, std/frame, std/irq, std/iter, std/list, std/math, std/ops, std/option, std/regex, std/result, std/str"
    );
    // The injected runtime is not a library a program may write in an
    // `impl`, so it is present and absent from the allowed list.
    assert!(matches!(
        std_admission(crate::stdlib::RT_MODULE),
        StdAdmission::Present
    ));
}

/// Every name in an admission row is a real declaration in the module it
/// names. Without this a rename in `kelpc/std/` would turn an admitted call
/// into an "unavailable" one with no test failing.
#[test]
fn every_admitted_item_is_declared_in_its_module() {
    // A free function is `fun name(`, and a receiver method is
    // `fun Self.name(` / `fun mut Self.name(` - a method's declaration
    // carries the receiver, so the name alone never matches.
    let declares = |source: &str, name: &str| {
        let mut at = 0;
        while let Some(found) = source[at..].find(&format!("fun {name}")) {
            let after = at + found + "fun ".len() + name.len();
            match source[after..].chars().next() {
                Some('(') | Some('<') => return true,
                _ => at = after,
            }
        }
        let mut at = 0;
        while let Some(found) = source[at..].find(&format!(".{name}")) {
            let after = at + found + 1 + name.len();
            match source[after..].chars().next() {
                Some('(') | Some('<') => return true,
                _ => at = after,
            }
        }
        false
    };
    for id in crate::stdlib::shipped() {
        let StdAdmission::Subset {
            callable,
            with_allocator,
            helpers,
        } = std_admission(id)
        else {
            continue;
        };
        let source = crate::stdlib::source(id).expect("a shipped module has source");
        for name in callable.iter().chain(with_allocator).chain(helpers) {
            assert!(declares(&source, name), "'{id}' has no 'fun {name}'");
        }
    }
}

/// The allocator-only rows are exactly the ones whose call site needs
/// `[target] alloc`/`dealloc`, and `item_needs_allocator` agrees with
/// `item_is_callable` about which those are.
#[test]
fn only_the_allocator_rows_need_an_allocator() {
    for id in ["std/option", "std/result", "std/str", "std/math"] {
        let StdAdmission::Subset { with_allocator, .. } = std_admission(id) else {
            panic!("'{id}' should be a narrowed row");
        };
        for name in with_allocator {
            assert!(!item_is_callable(id, name, false), "{id}.{name}");
            assert!(item_is_callable(id, name, true), "{id}.{name}");
            assert!(item_needs_allocator(id, name), "{id}.{name}");
        }
    }
    assert!(matches!(std_admission("std/list"), StdAdmission::Allocator));
    assert!(item_needs_allocator("std/list", "map"));
    assert!(!item_is_callable("std/list", "map", false));
    assert!(item_is_callable("std/list", "map", true));
    assert!(matches!(std_admission("std/regex"), StdAdmission::Allocator));
    assert!(item_needs_allocator("std/regex", "compile"));
    assert!(!item_is_callable("std/regex", "compile", false));
    assert!(item_is_callable("std/regex", "compile", true));
    // A `Pure` module's items never need the pair, and an overloaded name
    // is matched through `bare_name`.
    assert!(!item_needs_allocator("std/math", "abs@i32"));
    assert!(item_is_callable("std/math", "abs@i32", false));
    assert!(!item_is_callable("std/math", "abs@f64", false));
    assert!(!item_is_callable("std/math", "min@f32,f32", false));
    assert!(!item_is_callable("std/math", "clamp@f32,f32,f32", false));
    assert!(!item_is_callable("std/math", "sign", false));
    // A generic call key names its declaration through `item_name`.
    assert_eq!(item_name("std.math.abs@i32"), "abs@i32");
    assert_eq!(item_name("std.option.is_some<u32>"), "is_some");
    assert_eq!(item_name("std.str.byte"), "byte");
    // A receiver method's key names the method, so the allocator table sees it
    // (issue #295: `o.map(f)` on an `Option` needs the allocator like `option.map`).
    assert_eq!(item_name("std.option.Option#map<i32, i32>"), "map");
    assert!(item_needs_allocator("std/option", item_name("std.option.Option#map<i32, i32>")));
    assert!(item_is_callable("std/option", item_name("std.option.Option#is_some"), false));
}
