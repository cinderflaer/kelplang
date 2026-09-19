//! Which code generator `kelpc build` uses.
//!
//! This is the backend *seam* (MK7 stage 1, `plan/mk7-own-backend.md`). Until
//! there is a second backend there is nothing to abstract over, so this module
//! deliberately holds only the selector: how a backend is named, which names
//! exist, and how the choice is resolved from the command line and the
//! environment. The `match` that calls a generator stays at its single call
//! site in `main.rs`, and it becomes a trait only when a third backend makes
//! the interface worth naming.
//!
//! Why the seam exists at all before the second backend does: the fixture
//! suite is backend-agnostic by construction (compile, run, compare stdout),
//! so a second backend is validated by running the suite again with
//! `KELPC_BACKEND` set to it. That means the *only* thing this branch needs
//! before any generator work starts is a way to say which backend to use —
//! and a suite that cannot be pointed at a second backend cannot validate one.

/// A code generator `kelpc` can lower a typed program with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// Transpile to C, then hand the source to `cc`. The original backend, and
    /// the one the whole test suite is written against.
    C,
    /// Emit x86-64 assembly text, then hand it to `as` and `ld`. MK7 tier 1:
    /// this drops the C compiler from the chain while still linking libc.
    Asm,
}

/// Names a later tier will claim. Recognized, deliberately not selectable.
const RESERVED: &[&str] = &["own"];

impl Backend {
    /// What `--backend` and `KELPC_BACKEND` mean when neither is given.
    ///
    /// The C backend stays the default for the whole of MK7: it is the
    /// differential oracle every other backend is checked against, so making
    /// the new one default would remove the reference the comparison is
    /// against.
    pub const DEFAULT: Backend = Backend::C;

    /// Every backend that can actually be selected. The integration suite
    /// loops over this to run the fixtures once per backend.
    pub const IMPLEMENTED: &'static [Backend] = &[Backend::C, Backend::Asm];

    pub fn name(self) -> &'static str {
        match self {
            Backend::C => "c",
            Backend::Asm => "asm",
        }
    }

    /// The value that appears in `--backend=`/`KELPC_BACKEND` diagnostics: the
    /// implemented names, then the reserved ones called out separately, so an
    /// error never suggests a name that cannot work.
    fn expected_list() -> String {
        let implemented: Vec<&str> = Self::IMPLEMENTED.iter().map(|b| b.name()).collect();
        format!(
            "expected one of: {}",
            implemented.join(", ")
        )
    }

    /// Parses a backend name. The error text is the whole point of this
    /// function: a reserved name must not read as a typo, and a typo must
    /// list what does work.
    pub fn parse(name: &str) -> Result<Backend, String> {
        match name {
            "c" => Ok(Backend::C),
            "asm" => Ok(Backend::Asm),
            reserved if RESERVED.contains(&reserved) => Err(format!(
                "backend '{reserved}' is planned but not implemented yet \
                 (see plan/mk7-own-backend.md); {}",
                Self::expected_list()
            )),
            other => Err(format!(
                "unknown backend '{other}'; {}",
                Self::expected_list()
            )),
        }
    }
}

/// Resolves the backend from the command line and the environment.
///
/// The explicit `--backend=` argument wins over `KELPC_BACKEND`, which is what
/// makes a one-off override of a whole test run possible without editing the
/// command that sets it. An empty value is a mistake worth reporting rather
/// than silently reading as "unset".
pub fn resolve(flag: Option<&str>, env: Option<&str>) -> Result<Backend, String> {
    match (flag, env) {
        (Some(name), _) => Backend::parse(name),
        (None, Some(name)) => {
            if name.is_empty() {
                return Err(format!(
                    "KELPC_BACKEND is set but empty; {}",
                    Backend::expected_list()
                ));
            }
            Backend::parse(name)
        }
        (None, None) => Ok(Backend::DEFAULT),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_is_the_c_backend() {
        assert_eq!(resolve(None, None).unwrap(), Backend::C);
        assert_eq!(Backend::DEFAULT.name(), "c");
    }

    #[test]
    fn the_flag_wins_over_the_environment() {
        assert_eq!(resolve(Some("c"), Some("own")).unwrap(), Backend::C);
    }

    #[test]
    fn an_implemented_backend_is_selectable_from_either_source() {
        assert_eq!(resolve(Some("c"), None).unwrap(), Backend::C);
        assert_eq!(resolve(None, Some("c")).unwrap(), Backend::C);
    }

    /// A reserved name must not be reported as unknown: the two messages mean
    /// different things, and only one of them is the user's mistake.
    #[test]
    fn a_reserved_backend_says_planned_instead_of_unknown() {
        let e = resolve(Some("own"), None).unwrap_err();
        assert!(e.contains("planned but not implemented"), "{e}");
        assert!(!e.contains("unknown"), "{e}");
    }

    #[test]
    fn an_unknown_backend_lists_the_implemented_ones() {
        let e = resolve(Some("llvm"), None).unwrap_err();
        // Exact equality, not a search. Comparing the whole message is what
        // makes this a real assertion: `contains("own")` would match the word
        // "unknown", which is how this test first went wrong, and no
        // subtraction over `RESERVED` avoids the same trap for short names.
        let offered: Vec<&str> = Backend::IMPLEMENTED.iter().map(|b| b.name()).collect();
        assert_eq!(
            e,
            format!(
                "unknown backend 'llvm'; expected one of: {}",
                offered.join(", ")
            )
        );
    }

    #[test]
    fn an_empty_environment_value_is_reported() {
        let e = resolve(None, Some("")).unwrap_err();
        assert!(e.contains("KELPC_BACKEND is set but empty"), "{e}");
    }

    /// Every selectable backend must round-trip through its own name, so the
    /// suite's per-backend loop can pass the name it read back in.
    #[test]
    fn every_implemented_backend_round_trips_its_name() {
        for b in Backend::IMPLEMENTED {
            assert_eq!(Backend::parse(b.name()).unwrap(), *b);
        }
    }

    /// The reserved list and the implemented list must not overlap, or a name
    /// would be both suggested and refused.
    #[test]
    fn reserved_and_implemented_names_are_disjoint() {
        for b in Backend::IMPLEMENTED {
            assert!(!RESERVED.contains(&b.name()), "{}", b.name());
        }
    }
}
