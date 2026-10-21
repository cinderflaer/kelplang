use super::*;

/// The same cleanup, reached through the *rename* instead of the write: the
/// target here is a file that is a mount point, which `rename` may not replace
/// (EBUSY). The temporary exists by then, and must be gone afterwards.
#[test]
pub(crate) fn a_rename_that_fails_removes_its_temporary() {
    let dir = fresh_scratch("rename_fails");
    write_file(&dir.join("real.kp"), UNFORMATTED);
    write_file(&dir.join("x.kp"), UNFORMATTED);
    let script = format!(
        r#"d='{dir}'
mount --bind "$d/real.kp" "$d/x.kp" || {{ echo HARNESS_FAILED; exit 0; }}
echo HARNESS_OK
'{kelpc}' fmt -w "$d/x.kp"; echo "exit=$?"
echo FILE-LIST-BEGIN
ls -A "$d"
echo FILE-LIST-END
cat "$d/x.kp"
"#,
        dir = dir.display(),
        kelpc = env!("CARGO_BIN_EXE_kelpc"),
    );
    let Some(out) = in_mount_namespace(&script) else {
        eprintln!("SKIPPED (a rename that fails): unshare could not be run");
        return;
    };
    let Some((stdout, stderr)) = harness_ready("a rename that fails", &out) else {
        return;
    };

    assert!(
        stdout.contains("exit=1"),
        "a rename that failed was not reported as a failure:\n{stdout}"
    );
    assert!(
        stderr.contains("Device or resource busy"),
        "the failure was not the one this test sets up:\n{stderr}"
    );
    assert_eq!(
        between(&stdout, "FILE-LIST-BEGIN", "FILE-LIST-END"),
        vec!["real.kp".to_string(), "x.kp".to_string()],
        "the temporary file was left behind:\n{stdout}"
    );
    assert!(
        stdout.ends_with(UNFORMATTED),
        "the file was changed by a write that failed:\n{stdout}"
    );
}

/// A directory that cannot be written is refused, and the file it holds is left
/// exactly as it was. This failure lands before the temporary exists (a
/// read-only filesystem cannot hold one), which is the sibling of the two cases
/// above and worth pinning: `-w` must not fall back to writing the file in
/// place.
#[test]
pub(crate) fn a_read_only_directory_is_refused_and_nothing_is_written() {
    let dir = fresh_scratch("read_only");
    let script = format!(
        r#"d='{dir}'
mount -t tmpfs none "$d" || {{ echo HARNESS_FAILED; exit 0; }}
printf 'fun main() {{\n   print("hi")\n}}\n' > "$d/x.kp"
mount -o remount,ro "$d" || {{ echo HARNESS_FAILED; exit 0; }}
echo HARNESS_OK
'{kelpc}' fmt -w "$d/x.kp"; echo "exit=$?"
echo FILE-LIST-BEGIN
ls -A "$d"
echo FILE-LIST-END
cat "$d/x.kp"
"#,
        dir = dir.display(),
        kelpc = env!("CARGO_BIN_EXE_kelpc"),
    );
    let Some(out) = in_mount_namespace(&script) else {
        eprintln!("SKIPPED (a read-only directory): unshare could not be run");
        return;
    };
    let Some((stdout, stderr)) = harness_ready("a read-only directory", &out) else {
        return;
    };

    assert!(
        stdout.contains("exit=1"),
        "a read-only directory was not reported as a failure:\n{stdout}"
    );
    assert!(
        stderr.contains("Read-only file system"),
        "the failure was not the one this test sets up:\n{stderr}"
    );
    assert_eq!(
        between(&stdout, "FILE-LIST-BEGIN", "FILE-LIST-END"),
        vec!["x.kp".to_string()],
        "the temporary file was left behind:\n{stdout}"
    );
    assert!(
        stdout.ends_with(UNFORMATTED),
        "the file was changed by a write that failed:\n{stdout}"
    );
}

/// Two `fmt -w` runs against one path must not write the same temporary. With
/// one fixed name they would write the *same* file, and either run could rename
/// the other's half-written bytes into place, or fail on a path that had
/// already been renamed away. The temporary's name carries the process id, so
/// the runs write two files; the file must end up as one run's whole output,
/// with no temporary left over. The source is large so the two writes really do
/// overlap.
#[test]
pub(crate) fn concurrent_writes_to_one_path_do_not_collide() {
    let dir = fresh_scratch("concurrent");
    let file = dir.join("shared.kp");
    write_file(&file, &large_unformatted_source());

    let once = fmt_stdout(&file);
    assert!(once.status.success(), "the source was refused");
    let expected = String::from_utf8(once.stdout).expect("the formatted output is utf-8");

    let mut runs: Vec<std::process::Child> = (0..8)
        .map(|_| {
            kelpc()
                .args(["fmt", "-w"])
                .arg(&file)
                .spawn()
                .expect("spawning a concurrent kelpc fmt -w")
        })
        .collect();
    for run in &mut runs {
        let status = run.wait().expect("waiting for a concurrent kelpc fmt -w");
        assert!(status.success(), "a concurrent fmt -w failed: {status:?}");
    }

    assert_eq!(
        fs::read_to_string(&file).expect("reading the shared file"),
        expected,
        "two runs wrote the same path and mixed their output"
    );
    assert_eq!(
        entries(&dir),
        vec!["shared.kp".to_string()],
        "a temporary file was left behind"
    );
}

/// A source big enough (about 400 KiB) that two concurrent writes to it overlap
/// rather than queue behind each other, which is what makes the collision above
/// reachable at all.
pub(crate) fn large_unformatted_source() -> String {
    let mut src = String::from("fun main() {\n   print(\"hi\")\n");
    for i in 0..20_000 {
        src.push_str(&format!("   val v{i} = {i}\n"));
    }
    src.push_str("}\n");
    src
}

// ---------------------------------------------------------------------------
// the alias verdict (defect 5)
// ---------------------------------------------------------------------------

/// The verdict `kelpc build` reaches for a file or a project: its exit code and
/// its stderr, which is the reference `kelpc fmt` has to agree with. These files
/// fail in the front end, before any backend runs.
pub(crate) fn build_verdict(entry: &Path, out: &Path) -> (Option<i32>, String) {
    let output = kelpc()
        .arg("build")
        .env_remove("KELPC_BACKEND")
        .arg(entry)
        .arg("-o")
        .arg(out)
        .output()
        .expect("failed to spawn kelpc build");
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

/// An undeclared `_name` is an ordinary identifier, so `fmt` formats it the way
/// it formats any other unknown name; the type check in `kelpc build` is what
/// refuses it, and `fmt` never runs that check.
#[test]
pub(crate) fn an_undeclared_underscore_name_is_formatted_like_any_other_name() {
    let dir = fresh_scratch("undefined_alias");
    let file = dir.join("undefined.kp");
    let src = "fun main() {\n    print(_undefined)\n}\n";
    write_file(&file, src);

    let (code, build_stderr) = build_verdict(&file, &dir.join("undefined.out"));
    assert_ne!(code, Some(0), "kelpc build accepted an undefined name");
    assert!(
        build_stderr.contains("_undefined"),
        "kelpc build's diagnostic changed:\n{build_stderr}"
    );

    for (mode, out) in three_modes(&file) {
        assert!(
            out.status.success(),
            "kelpc fmt {mode} refused an ordinary identifier: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    assert_eq!(fs::read_to_string(&file).unwrap(), src);
    assert_eq!(entries(&dir), vec!["undefined.kp".to_string()]);
}

/// An alias another module declares, whose expansion is not valid syntax. One
/// file cannot see `util.kp`, so `fmt` asks the resolver for the project's alias
/// table, the one `kelpc build` expands with, and the project is refused with
/// the resolver's own diagnostic. Without that, the file formatted cleanly and
/// then failed to build.
#[test]
pub(crate) fn an_alias_from_another_module_that_supplies_a_bad_module_path_is_refused() {
    let dir = fresh_scratch("cross_file_alias");
    let project = dir.join("proj");
    fs::create_dir_all(project.join("src")).expect("creating the scratch project");
    write_file(&project.join("coral.toml"), "name = \"proj\"\n");
    write_file(&project.join("src/util.kp"), "alias _bad = no/such/module\n");
    let main = project.join("src/main.kp");
    let src = "impl \"util\"\nimpl \"_bad\"\n\nfun main() {\n    print(1)\n}\n";
    write_file(&main, src);

    let (code, build_stderr) = build_verdict(&project, &dir.join("proj.out"));
    assert_ne!(code, Some(0), "kelpc build accepted a bad alias expansion");
    assert!(
        build_stderr.contains("cannot find module 'no/such/module'"),
        "kelpc build's diagnostic changed:\n{build_stderr}"
    );

    for (mode, out) in three_modes(&main) {
        assert!(
            !out.status.success(),
            "kelpc fmt {mode} formatted a file whose alias expansion does not parse"
        );
        assert_eq!(
            String::from_utf8_lossy(&out.stderr),
            build_stderr,
            "kelpc fmt {mode} did not report what kelpc build reports"
        );
    }
    assert_eq!(fs::read_to_string(&main).unwrap(), src, "a refused file was rewritten");
    assert_eq!(
        entries(&project.join("src")),
        vec!["main.kp".to_string(), "util.kp".to_string()]
    );

    // The same project with a usable alias formats: the resolver is what makes
    // the difference, so this is not a blanket refusal of cross-file aliases.
    write_file(&project.join("src/util.kp"), "alias _bad = helper\n");
    write_file(&project.join("src/helper.kp"), "fun helper() => i32 = 1\n");
    let out = fmt_write(&main);
    assert!(
        out.status.success(),
        "a valid cross-file alias was refused:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        fs::read_to_string(&main).unwrap(),
        "impl \"util\"\nimpl \"_bad\"\n\nfun main() {\n    print(1)\n}\n",
        "the alias reference was not preserved"
    );
    assert_eq!(
        entries(&project.join("src")),
        vec!["helper.kp".to_string(), "main.kp".to_string(), "util.kp".to_string()]
    );
}
