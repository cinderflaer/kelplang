use super::*;

/// The inode number of a path, so a test can tell "the same file, rewritten"
/// from "a new file at the same name".
pub(crate) fn inode(path: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    fs::metadata(path)
        .expect("statting a scratch file")
        .ino()
}

/// A group the caller belongs to besides its primary one, or `None` if there is
/// none. A file's group can be changed without privilege to any group the caller
/// is a member of, which makes the group the one attribute of a replacement file
/// a test here can vary (the owner would need privilege, and the mode bits are
/// measured separately).
pub(crate) fn a_secondary_group() -> Option<u32> {
    let ids = |flag: &str| -> Option<String> {
        let out = Command::new("id").arg(flag).output().ok()?;
        out.status
            .success()
            .then(|| String::from_utf8(out.stdout).ok())
            .flatten()
    };
    let primary: u32 = ids("-g")?.trim().parse().ok()?;
    ids("-G")?
        .split_whitespace()
        .filter_map(|group| group.parse::<u32>().ok())
        .find(|group| *group != primary)
}

/// `-w` through a symbolic link rewrites the file the link names and leaves the
/// link a link. `-w` used to write its temporary next to the link's own path and
/// rename it over the link: `link.kp` came back a *regular file* holding the
/// formatted bytes, and the real target never changed. Covers a relative
/// target, an absolute one, and a two-hop chain (a link to a link to a file).
#[test]
pub(crate) fn writing_through_a_symlink_keeps_the_link_and_formats_its_target() {
    let dir = fresh_scratch("symlink_write");

    let relative_target = dir.join("target_relative.kp");
    let absolute_target = dir.join("elsewhere/target_absolute.kp");
    let link = dir.join("link.kp");
    let chain = dir.join("chain.kp");
    let absolute_link = dir.join("absolute_link.kp");
    write_file(&relative_target, UNFORMATTED);
    write_file(&absolute_target, UNFORMATTED);
    symlink("target_relative.kp", &link);
    symlink("link.kp", &chain);
    symlink(&absolute_target, &absolute_link);

    let out = fmt_write_paths(&[&link, &chain, &absolute_link]);
    assert!(
        out.status.success(),
        "kelpc fmt -w refused a symlink:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Every link is still a link, pointing where it did.
    assert_eq!(link_target(&link).as_deref(), Some("target_relative.kp"));
    assert_eq!(link_target(&chain).as_deref(), Some("link.kp"));
    assert_eq!(
        link_target(&absolute_link).as_deref(),
        absolute_target.to_str(),
        "an absolute-target link was replaced by a copy"
    );

    // The files they name hold the formatted bytes.
    assert_eq!(
        fs::read_to_string(&relative_target).expect("reading the relative target"),
        FORMATTED,
        "the relative link's target was not formatted"
    );
    assert_eq!(
        fs::read_to_string(&absolute_target).expect("reading the absolute target"),
        FORMATTED,
        "the absolute link's target was not formatted"
    );

    // No link was turned into a regular file, and the directory holds a
    // temporary file's worth of nothing either.
    assert_eq!(
        entries(&dir),
        vec![
            "absolute_link.kp -> ".to_string() + absolute_target.to_str().unwrap(),
            "chain.kp -> link.kp".to_string(),
            "elsewhere".to_string(),
            "link.kp -> target_relative.kp".to_string(),
            "target_relative.kp".to_string(),
        ]
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("symbolic link"),
        "the write through a link was not reported:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A dangling link names nothing, so there is no file to format: it is refused
/// in every mode, nothing is written, and the link is left alone. Refusing is
/// the point, replacing it with a copy of itself would be the one thing a
/// formatter must never do to a link.
#[test]
pub(crate) fn a_dangling_symlink_is_refused_and_nothing_is_written() {
    let dir = fresh_scratch("symlink_dangling");
    let link = dir.join("dangling.kp");
    symlink("nowhere.kp", &link);
    let other = dir.join("other.kp");
    write_file(&other, UNFORMATTED);
    let before = entries(&dir);

    for (mode, out) in three_modes(&link) {
        assert!(!out.status.success(), "kelpc fmt {mode} accepted a dangling link");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("symbolic link") && stderr.contains("nowhere.kp"),
            "kelpc fmt {mode} did not say what was wrong with the link:\n{stderr}"
        );
    }

    assert_eq!(link_target(&link).as_deref(), Some("nowhere.kp"));
    assert_eq!(entries(&dir), before, "a refusal changed the directory");
    assert_eq!(fs::read_to_string(&other).unwrap(), UNFORMATTED);
}

/// A link that names a directory is refused in every mode, and the directory it
/// names is not reformatted: `fmt` writes files, and a directory command that
/// followed the link would rewrite a tree under a name that is not that tree's
/// own.
#[test]
pub(crate) fn a_symlink_to_a_directory_is_refused_and_nothing_is_written() {
    let dir = fresh_scratch("symlink_dir");
    let real = dir.join("real");
    let inside = real.join("a.kp");
    write_file(&inside, UNFORMATTED);
    let link = dir.join("linked_dir");
    symlink(&real, &link);

    for (mode, out) in three_modes(&link) {
        assert!(
            !out.status.success(),
            "kelpc fmt {mode} accepted a symlink to a directory"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("symbolic link") && stderr.contains("director"),
            "kelpc fmt {mode} did not say what was wrong with the link:\n{stderr}"
        );
    }

    assert_eq!(link_target(&link).as_deref(), Some(real.to_str().unwrap()));
    assert_eq!(
        fs::read_to_string(&inside).unwrap(),
        UNFORMATTED,
        "a refused directory link formatted a file inside it"
    );
    assert_eq!(entries(&real), vec!["a.kp".to_string()]);
}

/// Directory recursion never follows a symbolic link. A link may name a file
/// outside the tree the user named, or a directory that holds this one,
/// `ln -s .. up` here points at the walk's own parent, so following one could
/// format a file twice under two names, rewrite a file outside the tree, or
/// recurse without end. Each skipped link is reported, the run still succeeds
/// for the files that are really there, and nothing inside a link is touched.
#[test]
pub(crate) fn directory_recursion_does_not_follow_symlinks() {
    let dir = fresh_scratch("recursion");
    let tree = dir.join("tree");
    let outside = dir.join("outside");
    fs::create_dir_all(&tree).expect("creating the scratch directory");
    write_file(&tree.join("a.kp"), UNFORMATTED);
    write_file(&outside.join("b.kp"), UNFORMATTED);
    symlink("..", &tree.join("up"));
    symlink(outside.join("b.kp"), &tree.join("linked.kp"));
    symlink(&outside, &tree.join("linked_dir"));

    let out = fmt_write(&tree);
    assert!(
        out.status.success(),
        "a directory holding a link is not a failure:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    for name in ["up", "linked.kp", "linked_dir"] {
        assert!(
            stderr.contains(name),
            "the skipped link {name} was not reported:\n{stderr}"
        );
    }

    assert_eq!(
        fs::read_to_string(tree.join("a.kp")).unwrap(),
        FORMATTED,
        "the file that is really in the directory was not formatted"
    );
    assert_eq!(
        fs::read_to_string(outside.join("b.kp")).unwrap(),
        UNFORMATTED,
        "a file outside the named tree was rewritten through a link"
    );
    assert_eq!(entries(&tree).len(), 4, "{:?}", entries(&tree));
    assert_eq!(entries(&dir), vec!["outside".to_string(), "tree".to_string()]);
}

/// `-w` through a link must not *say* it rewrote the target before it has. The
/// note used to be printed before the write, so a write that then failed left
/// the run reporting a rewrite it had not performed: nonzero status, unchanged
/// bytes, and a note claiming the file named by the link was rewritten
/// (defect 6). The note is now printed only after the write lands, so a failure
/// prints the failure alone.
///
/// The failure is forced by making the resolved target's directory unwritable,
/// which stops the atomic write from creating its temporary beside the target.
/// A process that can write a 0555 directory anyway (root, or a permissive
/// ACL) cannot exercise this, and says so instead of asserting something false.
#[test]
pub(crate) fn a_failed_write_through_a_symlink_reports_only_the_failure() {
    let dir = fresh_scratch("symlink_failed_write");
    let inner = dir.join("inner");
    let target = inner.join("target.kp");
    write_file(&target, UNFORMATTED);
    let link = dir.join("link.kp");
    symlink("inner/target.kp", &link);

    let mut perms = fs::metadata(&inner).unwrap().permissions();
    perms.set_mode(0o555);
    fs::set_permissions(&inner, perms).unwrap();

    let probe = inner.join(".probe");
    if fs::write(&probe, "").is_ok() {
        let _ = fs::remove_file(&probe);
        let mut perms = fs::metadata(&inner).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&inner, perms).unwrap();
        eprintln!(
            "SKIPPED (a failed write through a symlink): this process may write to a \
             0555 directory, so the write cannot be made to fail"
        );
        return;
    }

    let out = fmt_write(&link);

    let mut perms = fs::metadata(&inner).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&inner, perms).unwrap();

    assert!(
        !out.status.success(),
        "kelpc fmt -w reported success for a write that failed"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("is rewritten"),
        "the run claimed a rewrite that did not happen:\n{stderr}"
    );
    assert!(
        stderr.contains("cannot write") && stderr.contains("link.kp"),
        "the failure was not reported against the link:\n{stderr}"
    );
    assert_eq!(
        link_target(&link).as_deref(),
        Some("inner/target.kp"),
        "the link was not left alone"
    );
    assert_eq!(
        fs::read_to_string(&target).expect("reading the target"),
        UNFORMATTED,
        "a write that failed changed the target"
    );
    assert_eq!(
        entries(&inner),
        vec!["target.kp".to_string()],
        "a write that failed left a temporary behind"
    );
}

/// A tab does not survive anywhere, including inside an open `(` where the
/// author's leading whitespace is otherwise kept. There is no canonical form
/// for an author-wrapped continuation line, so spaces are preserved as written
/// and a tab there becomes four spaces (defect 9: the `push_indent` doc claimed
/// the author's whitespace was kept, which was false for tabs).
#[test]
pub(crate) fn a_tab_in_a_continuation_line_becomes_four_spaces() {
    let dir = fresh_scratch("continuation_tabs");
    let file = dir.join("tabs.kp");
    let src = "fun main() {\n    print(\n\t\"hi\",\n\t\"there\"\n    )\n}\n";
    write_file(&file, src);

    let out = fmt_stdout(&file);
    assert!(
        out.status.success(),
        "a tab-indented continuation line was refused:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    assert_eq!(
        text,
        "fun main() {\n    print(\n    \"hi\",\n    \"there\"\n    )\n}\n",
        "a tab inside an open parenthesis did not become four spaces"
    );
    assert!(!text.contains('\t'), "a tab survived:\n{text}");

    let wrote = fmt_write(&file);
    assert!(
        wrote.status.success(),
        "kelpc fmt -w refused the file:\n{}",
        String::from_utf8_lossy(&wrote.stderr)
    );
    assert_eq!(fs::read_to_string(&file).unwrap(), text);
    assert!(
        fmt_check(&file).status.success(),
        "the formatted output is not a fixed point"
    );
}

// ---------------------------------------------------------------------------
// non-regular files (defect 12)
// ---------------------------------------------------------------------------

/// A path that is not a regular file is refused *before* anything reads it.
///
/// `-w` on a symbolic link to a FIFO used to return exit 0 with the success
/// note and replace the FIFO with a regular file, and with no writer both `fmt`
/// and `fmt -w` blocked in `open` forever (the audit measured exit 124 under
/// `timeout 3`). The type is now settled with `metadata`, which opens nothing,
/// so the refusal comes first and the node is left alone. Every mode is covered
/// because the check is in `fmt_file`, which all of them share.
///
/// The subprocess runs under a wall-clock limit: if the refusal ever regressed,
/// this test would fail instead of blocking the suite (see `run_within`).
#[test]
pub(crate) fn a_non_regular_file_is_refused_before_it_is_read() {
    let dir = fresh_scratch("non_regular");

    // A socket needs nothing but std; a character device is `/dev/null`; the
    // FIFO needs `mkfifo`, and only its cases are skipped without it.
    //
    // The socket is bound through a short alias of `dir` so the fixture's own
    // path stays `dir/socket.kp`; binding `dir/socket.kp` itself would be refused
    // as longer than `sun_path` on a deep enough checkout.
    let alias = ShortAlias::new(&dir);
    let socket = dir.join("socket.kp");
    let _listener = std::os::unix::net::UnixListener::bind(alias.entry("socket.kp"))
        .expect("binding a socket");
    let device = PathBuf::from("/dev/null");
    let fifo = dir.join("fifo.kp");
    let fifo_ready = mkfifo(&fifo);
    if !fifo_ready {
        eprintln!("SKIPPED (the FIFO half of defect 12): this machine has no `mkfifo`");
    }
    let link = dir.join("link.kp");
    if fifo_ready {
        symlink("fifo.kp", &link);
    }

    let mut cases: Vec<(PathBuf, &'static str)> = vec![
        (socket.clone(), "socket"),
        (device.clone(), "character device"),
    ];
    if fifo_ready {
        cases.push((fifo.clone(), "FIFO"));
        // The link is resolved first, so the refusal names the FIFO it found.
        cases.push((link.clone(), "FIFO"));
    }

    let mut failures = Vec::new();
    for (path, kind) in &cases {
        for (mode, flags) in [
            ("(stdout)", vec!["fmt"]),
            ("--check", vec!["fmt", "--check"]),
            ("-w", vec!["fmt", "-w"]),
        ] {
            let mut command = kelpc();
            command.args(&flags).arg(path);
            let Some(out) = run_within(std::time::Duration::from_secs(10), &mut command) else {
                failures.push(format!(
                    "{} {mode}: blocked (killed after 10s) instead of refusing",
                    brief(path)
                ));
                continue;
            };
            let stderr = String::from_utf8_lossy(&out.stderr);
            if out.status.success() {
                failures.push(format!(
                    "{} {mode}: accepted a non-regular file (exit 0)",
                    brief(path)
                ));
            } else if !stderr.contains(kind) || !stderr.contains("not a regular file") {
                failures.push(format!(
                    "{} {mode}: the diagnostic does not name a {kind}:\n    {}",
                    brief(path),
                    stderr.trim().replace('\n', "\n    ")
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{}",
        report(&failures, "non-regular file(s) were not refused cleanly")
    );

    // Nothing was read, so nothing was changed: every node is still the node it
    // was, the link is still a link, and no temporary was written beside them.
    assert_eq!(node_type(&socket), "socket", "the socket was replaced");
    assert_eq!(node_type(&device), "character device");
    if fifo_ready {
        assert_eq!(node_type(&fifo), "fifo", "the FIFO was replaced by a regular file");
        assert_eq!(node_type(&link), "symlink", "the link was replaced");
        assert_eq!(link_target(&link).as_deref(), Some("fifo.kp"));
    }
    let mut expected = vec!["socket.kp".to_string()];
    if fifo_ready {
        // `entries` renders a symbolic link as `name -> target`.
        expected.extend(["fifo.kp".to_string(), "link.kp -> fifo.kp".to_string()]);
    }
    expected.sort();
    assert_eq!(entries(&dir), expected, "a refusal left something behind");
}

// ---------------------------------------------------------------------------
// what `-w` replaces (defect 13)
// ---------------------------------------------------------------------------

/// `-w` replaces the file's inode, and that is the documented outcome.
///
/// The write is atomic (a temporary beside the target, then a rename), and the
/// rename lands a *new* inode: a hard link to the formatted file keeps the old
/// bytes, and `fmt.rs`'s limits list and `main.rs`'s `write_atomic` both say so.
/// `write_atomic` copies the *mode bits* onto the temporary and nothing else.
/// Of the four things a replacement file could carry, this test measures two:
/// the permission bits (set to a mode a fresh file does not get, so carrying them
/// over is an observation and not a coincidence) and the group (the file is moved
/// to a secondary group the caller belongs to, and the new inode must not be in
/// it). The owner cannot be varied without privilege, so "the owner does not
/// follow" rests on reading `write_atomic`, and so do ACLs and extended
/// attributes, `set_permissions` with mode bits has nowhere to put either, and
/// nothing here measures them. Preserving the inode would mean writing in place,
/// which is the half-written file the design exists to avoid. This test pins the
/// outcome so it cannot change silently.
#[test]
pub(crate) fn rewriting_a_file_replaces_its_inode_and_a_hard_link_keeps_the_old_bytes() {
    let dir = fresh_scratch("hard_link");
    let a = dir.join("a.kp");
    let b = dir.join("b.kp");
    write_file(&a, UNFORMATTED);
    // A mode a fresh file does not get by default, so "the permission bits are
    // carried over" is an observation rather than a coincidence.
    fs::set_permissions(&a, fs::Permissions::from_mode(0o640))
        .expect("setting the source file's mode");
    // The group, the one of the other three attributes a test can vary: a file's
    // group can be changed without privilege to any group the caller belongs to.
    let moved_group = match a_secondary_group() {
        Some(gid) => {
            chown(&a, None, Some(gid)).expect("moving a.kp to a secondary group");
            Some(gid)
        }
        None => {
            eprintln!(
                "SKIPPED: the caller belongs to no group besides its primary one, so the \
                 rewrite's effect on the group cannot be measured here"
            );
            None
        }
    };
    fs::hard_link(&a, &b).expect("hard-linking a.kp to b.kp");
    let shared = inode(&a);
    assert_eq!(inode(&b), shared, "the two names must start on one inode");

    let out = fmt_write(&a);
    assert!(
        out.status.success(),
        "kelpc fmt -w refused a regular file:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    assert_eq!(
        fs::read_to_string(&a).expect("reading a.kp"),
        FORMATTED,
        "the file the command named was not formatted"
    );
    assert_eq!(
        fs::read_to_string(&b).expect("reading b.kp"),
        UNFORMATTED,
        "b.kp holds the formatted bytes: the rewrite kept the inode, which is not \
         the outcome the limits list states"
    );
    assert_ne!(
        inode(&a),
        shared,
        "the rewrite kept a.kp's inode; the documented outcome is a new one"
    );
    assert_eq!(inode(&b), shared, "the other name lost the inode it held");
    assert_eq!(
        fs::metadata(&a).expect("statting a.kp").permissions().mode() & 0o777,
        0o640,
        "the permission bits did not follow the file through the rewrite"
    );
    if let Some(gid) = moved_group {
        assert_ne!(
            fs::metadata(&a).expect("statting a.kp").gid(),
            gid,
            "the group followed the file through the rewrite; write_atomic copies the mode \
             bits and nothing else"
        );
        assert_eq!(
            fs::metadata(&b).expect("statting b.kp").gid(),
            gid,
            "the old inode lost the group this test moved it to"
        );
    }
    assert_eq!(
        entries(&dir),
        vec!["a.kp".to_string(), "b.kp".to_string()],
        "the rewrite left a temporary behind"
    );
}

// ---------------------------------------------------------------------------
// a write that fails after the temporary exists (defect 4)
// ---------------------------------------------------------------------------

/// Runs `script` under `unshare -Urm`. An unprivileged process may not mount
/// anything, but inside a private user and mount namespace it is root and can,
/// which is what lets these tests build the filesystem conditions a failed write
/// needs: a full filesystem, a read-only one, and a file that is a mount point.
/// Each script says `HARNESS_OK` once its condition is in place, so a machine
/// that cannot run the harness at all is visible as a skip rather than as a
/// pass.
pub(crate) fn in_mount_namespace(script: &str) -> Option<Output> {
    Command::new("unshare")
        .args(["-Urm", "sh", "-c", script])
        .output()
        .ok()
}

/// The lines of `text` between the `begin` and `end` markers, exclusive. The
/// scripts below print a directory listing between two markers so the test can
/// assert on it after the namespace is gone.
pub(crate) fn between(text: &str, begin: &str, end: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        if line == begin {
            inside = true;
        } else if line == end {
            break;
        } else if inside {
            out.push(line.to_string());
        }
    }
    out.sort();
    out
}

/// Introduces a script and answers whether its condition was reached, with the
/// run's stdout and stderr, the scripts print their observations on stdout and
/// the formatter's diagnostic goes to stderr.
pub(crate) fn harness_ready(what: &str, out: &Output) -> Option<(String, String)> {
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    if !stdout.contains("HARNESS_OK") {
        eprintln!(
            "SKIPPED ({what}): this machine cannot set the condition up in a user \
             namespace, so the failed write could not be forced here.\nstdout: {stdout}\nstderr: {stderr}"
        );
        return None;
    }
    Some((stdout, stderr))
}

/// A write that fails *after* its temporary file exists must remove it: the
/// writer used to leave the temporary in the source directory, where nothing
/// owns it and `git status` reports it. The filesystem here is a 4 KiB tmpfs
/// that is already full, so creating the temporary succeeds (an empty file
/// needs no page) and writing it does not, the exact shape of the defect. It
/// is forced inside a user and mount namespace, because mounting a filesystem
/// is the only way an unprivileged test can own one.
#[test]
pub(crate) fn a_write_that_runs_out_of_space_removes_its_temporary() {
    let dir = fresh_scratch("enospc");
    let script = format!(
        r#"d='{dir}'
mount -t tmpfs -o size=4k none "$d" || {{ echo HARNESS_FAILED; exit 0; }}
echo HARNESS_OK
printf 'fun main() {{\n   print("hi")\n}}\n' > "$d/x.kp"
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
        eprintln!("SKIPPED (no space left on device): unshare could not be run");
        return;
    };
    let Some((stdout, stderr)) = harness_ready("no space left on device", &out) else {
        return;
    };

    assert!(
        stdout.contains("exit=1"),
        "a write that failed was not reported as a failure:\n{stdout}"
    );
    assert!(
        stderr.contains("No space left on device"),
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
