use super::*;

// ---------------------------------------------------------------------------
// The CLI itself (MK8-S1-M3's release added `kelpc version`).
// ---------------------------------------------------------------------------

#[test]
pub(crate) fn the_cli_reports_its_version() {
    // The shape is what the scheme pins: `YY.MM.N`, with the month padded,
    // which is what the tag and the release asset are written as. The number
    // itself changes on every release, so asserting the shape keeps this test
    // from going stale on the next bump.
    let out = kelpc()
        .arg("version")
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        out.status.success(),
        "'kelpc version' should succeed, got:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let printed = String::from_utf8_lossy(&out.stdout).to_string();
    let version = printed
        .trim_end()
        .strip_prefix("kelpc ")
        .unwrap_or_else(|| panic!("'kelpc version' printed '{printed}'"));
    let parts: Vec<&str> = version.split('.').collect();
    assert_eq!(parts.len(), 3, "'{version}' is not YY.MM.N");
    assert_eq!(parts[0].len(), 2, "'{version}' has a one-digit year");
    assert_eq!(parts[1].len(), 2, "'{version}' has a one-digit month");
    assert!(
        parts[2].chars().all(|c| c.is_ascii_digit()) && !parts[2].is_empty(),
        "'{version}' has no release number"
    );
    assert!(out.stderr.is_empty(), "'kelpc version' should not warn");
}

#[test]
pub(crate) fn an_unknown_command_prints_the_usage_and_fails() {
    // Including `kelpc` with no arguments at all, which is the case a new user
    // hits first, and the usage line has to name every command that exists.
    for args in [vec![], vec!["bogus"], vec!["build"]] {
        let out = kelpc()
            .args(&args)
            .output()
            .expect("failed to spawn kelpc");
        assert!(
            !out.status.success(),
            "'kelpc {}' should fail",
            args.join(" ")
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("usage: kelpc build <file.kp>") && stderr.contains("kelpc version"),
            "unexpected usage text for 'kelpc {}':\n{stderr}",
            args.join(" ")
        );
    }
}

// ---------------------------------------------------------------------------
// The backend seam. The user-facing selector is gone (the MK7 record in
// `kelp wiki page meta/milestones.md`), so what is asserted end-to-end here is the shape that replaced it: every
// build gets `llvm`, and the retired flag and the retired environment variables
// are refused by name. These are end-to-end rather than only `backend.rs` unit
// tests because what matters is that the refusal reaches the compiler.
// ---------------------------------------------------------------------------

/// The LLVM backend writes its textual IR beside the artifact as `<out>.ll` (MK7 §4.4). The program prints the same bytes whichever code generator built it, so the intermediate is where a build's backend shows, and it is what these tests read.
pub(crate) fn build_is_llvm(bin: &Path) -> bool {
    bin.with_extension("ll").is_file()
}

/// A build has one backend, the LLVM backend, and it writes the textual IR beside
/// the artifact. The artifact is not enough to assert this on its own, because
/// the program prints the same thing whichever code generator built it.
#[test]
pub(crate) fn the_default_backend_is_llvm() {
    let bin = out_path("backend_default");
    let _ = std::fs::remove_file(bin.with_extension("ll"));
    let out = Command::new(kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .env_remove("KELPC_ORACLE")
        .arg("build")
        .arg(fixture("tests/fixtures/while_accumulator.kp"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        out.status.success(),
        "a build with no backend setting should succeed, got:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        build_is_llvm(&bin),
        "a build with no backend setting should have used the llvm default, \
         which writes <out>.ll beside the artifact"
    );
    let run = Command::new(&bin).output().expect("failed to run the binary");
    assert_eq!(String::from_utf8_lossy(&run.stdout), "55\n");
}

/// A build with no `-o` writes into `build/<stem>`, and creates that directory
/// if it is missing.
///
/// This is a behaviour change with a history: the default used to be a bare
/// `<stem>` **relative to the working directory**, so a build run from a
/// checkout put the binary in the checkout's root. Ten of them were committed
/// across six commits before anyone noticed, and the generated C beside each
/// one was invisible because `*.c` is ignored, which is exactly the shape a
/// test has to pin, because the output is only wrong somewhere no one looks.
///
/// Both halves matter: the artifact has to be *in* `build/`, and the working
/// directory has to be left with no new file in it.
#[test]
pub(crate) fn a_build_with_no_output_path_lands_in_build() {
    let cwd = out_path("default_out_dir");
    let _ = std::fs::remove_dir_all(&cwd);
    std::fs::create_dir_all(&cwd).expect("could not create the test's working directory");

    let out = kelpc()
        .args(["build"])
        .arg(fixture("tests/fixtures/while_accumulator.kp"))
        .current_dir(&cwd)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        out.status.success(),
        "a build with no -o should succeed, got:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let built = cwd.join("build").join("while_accumulator");
    assert!(
        built.is_file(),
        "expected the build to land at '{}', and it did not exist",
        built.display()
    );
    let mode = std::fs::metadata(&built)
        .expect("the built artifact should be stat-able")
        .permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert!(
            mode.mode() & 0o111 != 0,
            "the artifact should be executable, mode was {:o}",
            mode.mode()
        );
    }
    // The backend's own intermediate follows the artifact, so it is inside
    // `build/` too rather than beside it in the working directory: the LLVM
    // backend writes its textual IR there.
    let intermediate = "ll";
    assert!(
        built.with_extension(intermediate).is_file(),
        "expected the generated .{intermediate} beside the artifact"
    );
    assert!(
        !cwd.join("while_accumulator").exists(),
        "the build must not write the artifact into the working directory"
    );
}

/// The two refusals that replace the selector, each carrying the exact text the
/// retired spellings are reported with. An exact comparison, not a search: a
/// message that drifted into "unknown flag" while a test only looked for the
/// word "backend" would keep passing while telling the caller the wrong thing.
pub(crate) const FLAG_GONE: &str = "--backend is gone: kelpc builds every program with the llvm backend, which is the only backend";
pub(crate) const ENV_GONE: &str = "KELPC_BACKEND is gone: kelpc builds every program with the llvm backend, which is the only backend";
pub(crate) const ORACLE_GONE: &str = "KELPC_ORACLE is gone: kelpc builds every program with the llvm backend, which is the only backend";

/// A caller who still passes `--backend=c` is asking for a choice that no longer
/// exists. The refusal names the change and says which backend the build gets,
/// rather than reading as a flag that was never heard of.
#[test]
pub(crate) fn the_backend_flag_is_gone() {
    let out = kelpc()
        .args(["build", "--backend=c"])
        .arg(fixture("tests/fixtures/while_accumulator.kp"))
        .arg("-o")
        .arg(out_path("backend_flag_gone"))
        .output()
        .expect("failed to spawn kelpc");
    assert!(!out.status.success(), "--backend must be refused");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        stderr.trim_end(),
        format!("error: {FLAG_GONE}"),
        "the refusal for --backend=c should name the change"
    );
}

/// A shell that still exports `KELPC_BACKEND` is the case silence would get wrong: it would believe its builds used another backend. The variable is refused by name whatever it holds, so the caller learns the truth.
#[test]
pub(crate) fn the_backend_environment_variable_is_gone() {
    for value in ["c", "llvm"] {
        let out = kelpc()
            .args(["build"])
            .arg(fixture("tests/fixtures/while_accumulator.kp"))
            .arg("-o")
            .arg(out_path("backend_env_gone"))
            .env("KELPC_BACKEND", value)
            .output()
            .expect("failed to spawn kelpc");
        assert!(
            !out.status.success(),
            "KELPC_BACKEND={value} must be refused"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(
            stderr.trim_end(),
            format!("error: {ENV_GONE}"),
            "the refusal for KELPC_BACKEND={value} should name the change"
        );
    }
}

/// `KELPC_ORACLE` selected a second code generator, which is gone. A set
/// variable is refused by name whatever it holds, because a suite run or a
/// script that still exports it would otherwise believe it measured another
/// backend.
#[test]
pub(crate) fn the_oracle_environment_variable_is_gone() {
    for value in ["c", "llvm", "own", ""] {
        let out = kelpc()
            .args(["build"])
            .arg(fixture("tests/fixtures/while_accumulator.kp"))
            .arg("-o")
            .arg(out_path("backend_oracle_gone"))
            .env("KELPC_ORACLE", value)
            .output()
            .expect("failed to spawn kelpc");
        assert!(
            !out.status.success(),
            "KELPC_ORACLE={value:?} must be refused"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(
            stderr.trim_end(),
            format!("error: {ORACLE_GONE}"),
            "the refusal for KELPC_ORACLE={value:?} should name the change"
        );
    }
}

// ---------------------------------------------------------------------------
// A local binding: a second `val`/`var` of one name in a block is a shadow.
// ---------------------------------------------------------------------------

/// BUG-v6Qe0: a backend that turned each local into a C variable named after the
/// binding, so a rebinding in one block reached `cc` as a redefinition of
/// `kelp_a`. Every shape the report named is in the fixture: a scalar, a `str`,
/// a `datac` and a container, and each read after the shadow has to see the
/// newest binding. It also pins the read `val n = n + 1` makes: the initializer
/// names the binding the new one shadows, so it reads the one already in scope.
#[test]
pub(crate) fn a_local_rebinding_shadows_the_earlier_binding() {
    assert_compiles_and_runs(
        "tests/fixtures/shadowed_local_in_one_block.kp",
        "a=2\ns=two\np=3,4\ncards=1 7\ninner=2\nouter=1\n",
    );
}

/// BUG-mW3tZ: a `Unit` value may be bound. The local is zero-sized, so it gets
/// no storage and no drop, and the initializer's call still runs: the two
/// backends used to answer this with a compiler-defect message (LLVM) and with
/// `variable has incomplete type void` from `cc`. The annotated form of the
/// same binding is `val y: unit` (the language spells the type `unit`), and the
/// generic case below is the reason the binding is allowed at all.
#[test]
pub(crate) fn a_unit_valued_call_can_be_bound_to_a_local() {
    assert_compiles_and_runs("tests/fixtures/unit_val_binding.kp", "1\n");
}

/// A generic function instantiated at `unit`: the instantiation takes the value
/// as an argument and returns it, a plain function declares a `unit` parameter,
/// and a bound `unit` is passed on to it. None of the three crosses a value in
/// either backend's machine signature.
#[test]
pub(crate) fn a_generic_instantiated_at_unit_can_be_bound_and_passed_on() {
    assert_compiles_and_runs(
        "tests/fixtures/unit_val_generic_binding.kp",
        "1\n2\n",
    );
}
