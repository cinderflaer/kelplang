//! Which code generator `kelpc` uses.
//!
//! Since MK7 S7 (2026-09-21) `Backend::DEFAULT` is `Llvm`, and the user-facing
//! selector is gone: `kelpc` has **one backend for anyone building a program**.
//! There is no `--backend=` flag and no `KELPC_BACKEND`, and neither `usage()`
//! nor the README offers a choice. `c` is not offered because it is not a user
//! surface any more, not because it was deleted.
//!
//! The C backend is kept as the **project's own oracle**. It is the thing every
//! `llvm` build is compared against, by the differential sweep
//! (`kelpc/tests/backend_coverage.rs`), the fuzzer and conformance corpus
//! (`kelpc/tests/differential_fuzz.rs`) and `run-all-backends.sh`, and it stays
//! in the tree, in the suite and in CI. The one way to reach it is the internal
//! switch [`ORACLE_ENV`], which the project's own suite sets and which accepts
//! exactly `c`: a value it does not know is an error, because a run that
//! believes it measured the oracle and did not is worse than a refusal.
//!
//! The seam itself is unchanged from the retired hand-written track that built
//! it: the names, `Backend::DEFAULT`, and the one `match` that calls a generator
//! (at its single call site in `main.rs`). It becomes a trait only when a third
//! backend makes the interface worth naming.

/// A code generator `kelpc` can lower a typed program with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// Transpile to C, then hand the source to `cc`. The original backend, and
    /// the **differential oracle** every `llvm` build is checked against. It is
    /// never deleted and never stops being maintained, but it is reachable only
    /// through the project's oracle switch, never from a user surface.
    C,
    /// Lower `tir` to LLVM IR, let LLVM emit the object, and hand that to `cc`
    /// (MK7), with `cc` still the linker. Since MK7 S7 it is the default and the
    /// only backend a user can build with, and it lowers the whole corpus the C
    /// backend builds.
    Llvm,
}

/// The environment variable that points the compiler at the C backend.
///
/// This is the project's **oracle switch**, not a user surface: it exists so
/// the suite can build every fixture with the C backend and compare the result
/// with the `llvm` build. It accepts exactly one value, [`Backend::C`]'s name,
/// and anything else is an error rather than a silent fallback. It is
/// deliberately absent from `usage()` and the README, which describe only what
/// a person building a program can do.
pub const ORACLE_ENV: &str = "KELPC_ORACLE";

/// The environment variable that used to choose a backend. Retired, and named
/// here only so the refusal and its test can name it.
pub const RETIRED_ENV: &str = "KELPC_BACKEND";

/// The refusal for `--backend`, which the command line no longer accepts.
///
/// A retired spelling names its fix rather than reading as an unknown flag
/// (the parser's `impl "..." as name` refusal is the same shape), and it says
/// which backend the build will get, because the caller asked for a different
/// one.
pub const FLAG_RETIRED: &str =
    "--backend is gone: kelpc builds every program with the llvm backend, and the C \
     backend is kept as the project's own oracle rather than as a user choice";

/// The refusal for a set `KELPC_BACKEND`, which is no longer read.
///
/// Silently ignoring the variable is the one option that must not be taken: a
/// shell that still exports it would keep believing its builds used the C
/// backend while every one of them used `llvm`. The refusal says the same thing
/// the flag's does, for the same reason.
pub const ENV_RETIRED: &str =
    "KELPC_BACKEND is gone: kelpc builds every program with the llvm backend, and the C \
     backend is kept as the project's own oracle rather than as a user choice";

impl Backend {
    /// What a build with no backend setting uses.
    ///
    /// `llvm` became the default at MK7 S7, on the measurements §4.3 records:
    /// it builds all 223 corpus targets the C backend builds, with an empty
    /// refusal ranking and 0 mismatches, 0 crashes and 0 refusals at every
    /// optimization level from `-O0` to `-O3` and `-Os`, and the whole fixture
    /// suite is green under it. The C backend is not replaced by that: it is the
    /// differential oracle, so the suite keeps building with it and comparing the
    /// result.
    pub const DEFAULT: Backend = Backend::Llvm;

    /// The name a backend is spelled by: `--backend=` and `KELPC_BACKEND` when
    /// those existed, and [`ORACLE_ENV`] now. Both variants keep one, so a
    /// diagnostic names a backend in the same word the switch accepts.
    pub fn name(self) -> &'static str {
        match self {
            Backend::C => "c",
            Backend::Llvm => "llvm",
        }
    }
}

/// Resolves the backend from the environment.
///
/// `retired_set` says whether `KELPC_BACKEND` is set at all, and any set value
/// is refused ([`ENV_RETIRED`]); the value is not reported because the variable,
/// not its value, is what was retired. `oracle` is whatever `KELPC_ORACLE`
/// holds, and only `c` is a value: a wrong one is an error rather than a silent
/// fallback, so a suite run cannot believe it measured the oracle and measure
/// the default instead. An unreadable (non-UTF-8) value reads as unset, the
/// same way an empty one is reported rather than defaulted.
pub fn resolve(retired_set: bool, oracle: Option<&str>) -> Result<Backend, String> {
    if retired_set {
        return Err(ENV_RETIRED.to_string());
    }
    match oracle {
        None => Ok(Backend::DEFAULT),
        Some(name) if name == Backend::C.name() => Ok(Backend::C),
        Some("") => Err(format!(
            "{ORACLE_ENV} is set but empty; it is the project's oracle switch and accepts only '{}'",
            Backend::C.name()
        )),
        Some(other) => Err(format!(
            "{ORACLE_ENV} names '{other}', and it accepts only '{}': it is the project's \
             oracle switch, not a backend choice",
            Backend::C.name()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_is_the_llvm_backend() {
        assert_eq!(resolve(false, None).unwrap(), Backend::Llvm);
        assert_eq!(Backend::DEFAULT.name(), "llvm");
    }

    /// The oracle is still reachable after the selector was removed: the change
    /// takes away the choice, not the backend. `c` is what `KELPC_ORACLE`
    /// accepts, and a run of the suite through it is unchanged.
    #[test]
    fn the_oracle_switch_selects_the_c_backend() {
        assert_eq!(resolve(false, Some("c")).unwrap(), Backend::C);
    }

    /// Both variants keep a name, and each is what the switch or the default
    /// spells, so a diagnostic can name a backend in one word.
    #[test]
    fn every_backend_has_its_own_name() {
        assert_eq!(Backend::C.name(), "c");
        assert_eq!(Backend::Llvm.name(), "llvm");
        assert_ne!(Backend::C.name(), Backend::Llvm.name());
    }

    /// The retired variable is refused whatever it holds, including the name of
    /// a backend that still exists: the variable is what went away, and a value
    /// it could carry is not a way back in.
    #[test]
    fn the_retired_variable_is_refused_whatever_its_value() {
        for value in [None, Some("c"), Some("llvm"), Some("own"), Some("")] {
            let e = resolve(true, value).unwrap_err();
            assert_eq!(e, ENV_RETIRED);
        }
    }

    /// The two refusals name the change rather than reading as a typo, and each
    /// names itself.
    #[test]
    fn a_retired_spelling_names_itself_and_the_change() {
        assert!(FLAG_RETIRED.starts_with("--backend is gone:"), "{FLAG_RETIRED}");
        assert!(ENV_RETIRED.starts_with("KELPC_BACKEND is gone:"), "{ENV_RETIRED}");
        for message in [FLAG_RETIRED, ENV_RETIRED] {
            assert!(message.contains("llvm"), "{message}");
            assert!(message.contains("oracle"), "{message}");
        }
    }

    /// The switch accepts one value only. `llvm` is refused even though it names
    /// a backend, because the switch is the oracle switch and not a choice.
    #[test]
    fn an_oracle_value_other_than_c_is_refused() {
        for value in ["llvm", "own", "rust"] {
            let e = resolve(false, Some(value)).unwrap_err();
            assert!(
                e.contains("accepts only 'c'"),
                "'{value}' should be refused for not being the oracle: {e}"
            );
        }
    }

    /// An empty value is a mistake, not an instruction to use the default: a
    /// script that computes a name and produces an empty string must fail rather
    /// than silently measure the wrong backend.
    #[test]
    fn an_empty_oracle_value_is_reported() {
        let e = resolve(false, Some("")).unwrap_err();
        assert!(e.contains("KELPC_ORACLE is set but empty"), "{e}");
    }
}
