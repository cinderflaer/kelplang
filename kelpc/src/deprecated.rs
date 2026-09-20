//! Outdated syntax: what the compiler still accepts, and means to remove.
//!
//! A language change that replaces or deletes a spelling does not have to break
//! code the day it lands, and it should not: the old spelling keeps compiling
//! until its readers have had a cycle to move off it. This module is the
//! register of that state. An entry is *data* (an id, what is outdated, what to
//! write instead), and the pass that reads the construct raises the finding, so
//! the wording and the deadline of every deprecation live in one place.
//!
//! What an entry buys, and what it does not:
//!
//! - The old spelling **compiles**, exit status and all. `kelpc build` and
//!   `kelpc run` print the finding as an `outdated:` warning, and `kelpc fmt`
//!   copies the text exactly, because a formatter that never rewrites a token
//!   cannot rewrite this one either.
//! - The warning **names the removal**, and the deadline is one shared
//!   sentence ([`REMOVAL`]) rather than a phrase per entry: an entry is removed
//!   on the next development cycle, and nothing here can quietly promise a
//!   different one.
//! - `--deny-outdated` makes those findings **errors**: the same messages, and
//!   the build stops before anything is generated. That is the migration gate.
//!   It is off by default, because the compiler's job is to build a program
//!   that was written for the version it has, not to refuse the version before
//!   it.
//! - The compiler's **own** sources never carry an outdated spelling: an
//!   outdated construct in the shipped stdlib would warn every user of that
//!   module about code they cannot edit. The test at the bottom is what keeps
//!   that true.
//! - An entry here is **scheduled work with a deadline**, not an open defect.
//!   `ISSUES.md` is where what is wrong lives; a spelling that still works is
//!   not wrong, it is on its way out.
//!
//! Retiring an entry, in the order that keeps the tree consistent: delete the
//! spelling from the lexer or parser (and from the shipped stdlib, and from the
//! fixtures that were deliberately left on it), delete the entry here, and let
//! `kelpc/tests/outdated.rs` and the shipped-source test below say what is left.

use crate::diag::Diagnostic;
use crate::lexer::Span;

/// The deadline every entry carries, as one sentence so it cannot drift.
const REMOVAL: &str = "will be removed in the next development cycle";

/// One thing the compiler still accepts and will not accept later.
pub struct Outdated {
    /// Stable, kebab-case id. A test, a commit message and the plan name an
    /// entry by this, so its wording can change without the references to it
    /// changing.
    pub id: &'static str,
    /// What is outdated, in the words a reader searches the source for.
    pub what: &'static str,
    /// What to write instead, and why that means the same thing.
    pub instead: &'static str,
}

/// `impl "..." with caps(...)`: the keyword in front of a capability grant
/// (`syntax.md` 15.2).
pub const CAPS_WITH: &str = "caps-with";

/// Every outdated construct, in the order they were scheduled.
///
/// One entry so far. `impl "..." as name` was the other, and the first: it was
/// retired once a module's own name became its namespace, which is what the
/// clause already said (`syntax.md` 13.1). An entry lives here only while the
/// spelling still compiles, and the removal is the parser's, not this module's.
///
/// `with caps(...)` is the same shape of redundancy: `impl` already means "this
/// module declares what it imports", so the keyword states nothing the clause
/// does not, and the grant it introduces is unchanged when it goes.
pub const REGISTER: &[Outdated] = &[
    Outdated {
        id: CAPS_WITH,
        what: "`impl \"...\" with caps(...)`",
        instead: "drop the `with`: the grant is `impl \"std/fs\" caps(fs = [\"/var/log/**\"])`, and \
                  the patterns, the domains and the no-escalation rule are exactly the same",
    },
];

/// The register entry with this id.
pub fn find(id: &str) -> &'static Outdated {
    REGISTER
        .iter()
        .find(|entry| entry.id == id)
        .unwrap_or_else(|| panic!("'{id}' is not in the register of outdated syntax"))
}

/// The finding for one register entry, at the span that names the construct.
pub fn finding(id: &str, span: Span, module: &str) -> Diagnostic {
    let entry = find(id);
    Diagnostic::new(
        format!("{} is outdated and {REMOVAL}: {}", entry.what, entry.instead),
        span,
        module,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser::Parser;

    #[test]
    fn every_id_is_unique() {
        let mut seen = Vec::new();
        for entry in REGISTER {
            assert!(
                !seen.contains(&entry.id),
                "two register entries share the id '{}'",
                entry.id
            );
            seen.push(entry.id);
        }
    }

    #[test]
    fn every_finding_names_the_removal_and_the_fix() {
        for entry in REGISTER {
            let d = finding(entry.id, Span::new(0, 0), "m");
            assert!(d.message.contains("outdated"), "{}", d.message);
            assert!(
                d.message.contains("will be removed in the next development cycle"),
                "{}",
                d.message
            );
            assert!(
                d.message.contains(entry.instead),
                "the fix of '{}' is not in its own finding: {}",
                entry.id,
                d.message
            );
        }
    }

    #[test]
    fn an_unknown_id_is_a_compiler_bug() {
        // The only way to reach this is to raise a deprecation that was never
        // registered, which is a mistake to make loudly rather than to print.
        let caught = std::panic::catch_unwind(|| find("no-such-entry"));
        assert!(caught.is_err());
    }

    #[test]
    fn the_shipped_sources_carry_no_outdated_spelling() {
        // A deprecation in the stdlib is the worst kind: every program that
        // imports that module would be warned about a file it cannot edit.
        let mut ids: Vec<&str> = crate::stdlib::shipped().to_vec();
        ids.push(crate::stdlib::RT_MODULE);
        for id in ids {
            let text = crate::stdlib::source(id).expect("a shipped module has source");
            let tokens = Lexer::new(&text)
                .tokenize()
                .unwrap_or_else(|e| panic!("'{id}' does not lex: {}", e.message));
            let program = Parser::new(tokens)
                .parse_program()
                .unwrap_or_else(|e| panic!("'{id}' does not parse: {}", e.message));
            for imp in &program.impls {
                assert!(
                    imp.caps_with_span.is_none(),
                    "the shipped module '{id}' writes `impl \"{}\" with caps(...)`, which is \
                     outdated syntax: a program that imports it would be warned about a file it \
                     cannot edit",
                    imp.path
                );
            }
        }
    }
}
