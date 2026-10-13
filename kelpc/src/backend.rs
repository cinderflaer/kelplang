//! The backend selectors that no longer exist.
//!
//! `kelpc` has one code generator, the LLVM backend, and it builds every
//! program. The `--backend` flag, the `KELPC_BACKEND` variable and the
//! `KELPC_ORACLE` switch went with the other code generators.
//!
//! A retired spelling is refused by name rather than ignored. A shell that still
//! exports one of the variables would otherwise believe its builds used a
//! backend it did not get, so the refusal says which backend the build gets and
//! why the choice is gone.

/// The flag that used to choose a backend.
pub const FLAG_RETIRED: &str =
    "--backend is gone: kelpc builds every program with the llvm backend, which is the only \
     backend";

/// The environment variable that used to choose a backend.
pub const BACKEND_ENV: &str = "KELPC_BACKEND";

/// The project's oracle switch, which selected a second code generator. Retired with it.
pub const ORACLE_ENV: &str = "KELPC_ORACLE";

/// Refuses either retired variable, whatever its value. Presence is what counts:
/// a set variable is refused even when it is empty or names the llvm backend,
/// because the variable is what went away.
pub fn check_retired_env(backend_set: bool, oracle_set: bool) -> Result<(), String> {
    if backend_set {
        return Err(format!(
            "{BACKEND_ENV} is gone: kelpc builds every program with the llvm backend, which is \
             the only backend"
        ));
    }
    if oracle_set {
        return Err(format!(
            "{ORACLE_ENV} is gone: kelpc builds every program with the llvm backend, which is \
             the only backend"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_set_is_accepted() {
        assert!(check_retired_env(false, false).is_ok());
    }

    #[test]
    fn either_retired_variable_is_refused_by_name() {
        let e = check_retired_env(true, false).unwrap_err();
        assert!(e.starts_with("KELPC_BACKEND is gone:"), "{e}");
        let e = check_retired_env(false, true).unwrap_err();
        assert!(e.starts_with("KELPC_ORACLE is gone:"), "{e}");
    }

    #[test]
    fn the_flag_refusal_names_the_llvm_backend() {
        assert!(FLAG_RETIRED.starts_with("--backend is gone:"), "{FLAG_RETIRED}");
        assert!(FLAG_RETIRED.contains("llvm"), "{FLAG_RETIRED}");
    }
}
