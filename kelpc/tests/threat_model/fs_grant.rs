use super::*;

/// F1: `..` segments climbing out of the granted root.
#[test]
pub(crate) fn f01_dotdot_segments_cannot_leave_the_grant() {
    let lab = Lab::new("f01");
    lab.simple(&data_grant(&lab), FS, &read_prog("\"__T__/data/../secret/s.txt\""));
    lab.denied("is outside the fs capability");
}

/// F2: `.` segments and doubled separators are collapsed, so a path that is
/// inside the grant works (the positive control) and one that is outside after
/// collapsing is denied.
#[test]
pub(crate) fn f02_dot_segments_and_double_slashes_collapse_before_matching() {
    let lab = Lab::new("f02a");
    lab.simple(&data_grant(&lab), FS, &read_prog("\"__T__/./data//./ok.txt\""));
    lab.prints("fine\n\n");
    let lab = Lab::new("f02b");
    lab.simple(&data_grant(&lab), FS, &read_prog("\"__T__/data/.//../secret//s.txt\""));
    lab.denied("is outside the fs capability");
}

/// F3: the grant `data/**` does not admit the sibling `data2`, nor the prefix
/// `data` followed by more name characters.
#[test]
pub(crate) fn f03_a_sibling_sharing_the_prefix_is_outside_the_grant() {
    let lab = Lab::new("f03");
    fs::create_dir_all(lab.root.join("data2")).unwrap();
    fs::write(lab.root.join("data2/s.txt"), TOKEN).unwrap();
    lab.simple(&data_grant(&lab), FS, &read_prog("\"__T__/data2/s.txt\""));
    lab.denied("is outside the fs capability");
}

/// F4: a relative path is refused outright, whatever the process's working
/// directory is (the test runs the program with `data/` as its cwd, so a
/// relative `s.txt`-style name would resolve inside the grant if it were
/// accepted and resolved against the cwd).
#[test]
pub(crate) fn f04_a_relative_path_is_never_matched_against_the_cwd() {
    let lab = Lab::new("f04");
    lab.simple(&data_grant(&lab), FS, &read_prog("\"ok.txt\""));
    assert!(lab.build().status.success());
    let out = Command::new(lab.root.join("prog"))
        .current_dir(lab.root.join("data"))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success() && stderr.contains("is outside the fs capability"),
        "a relative path was accepted: {stderr}"
    );
}

/// F5: case is significant: `DATA` is not `data` on a case-sensitive
/// filesystem, and the matcher must not fold it either.
#[test]
pub(crate) fn f05_the_grant_is_case_sensitive() {
    let lab = Lab::new("f05");
    lab.simple(&data_grant(&lab), FS, &read_prog("\"__T__/DATA/ok.txt\""));
    lab.denied("is outside the fs capability");
}

/// F6: a path whose C string ends early. The check and the syscall both see the
/// text up to the NUL, so nothing past it can be smuggled in. (`\x00` cannot be
/// written in a literal; the NUL comes from `String.from_bytes` over a byte
/// buffer, the one way to make a `str` that contains one.)
#[test]
pub(crate) fn f06_an_embedded_nul_cannot_hide_a_second_path() {
    let lab = Lab::new("f06");
    lab.simple(
        &data_grant(&lab),
        &["std/fs", "std/mem", "std/str"],
        "    val head = \"__T__/data/ok.txt\"\n    val tail = \"/../../secret/s.txt\"\n    val hv = head.view()\n    val tv = tail.view()\n    val hn = str.len(head)\n    val tn = str.len(tail)\n    var b = mem.alloc(hn + 1 + tn)\n    var i: int = 0\n    while i < hn {\n        b.set(i, hv.get(i))\n        i = i + 1\n    }\n    b.set(hn, 0)\n    var j: int = 0\n    while j < tn {\n        b.set(hn + 1 + j, tv.get(j))\n        j = j + 1\n    }\n    val p = String.from_bytes(b)\n    print(fs.read_text(p.as_str()))",
    );
    // Either the check sees the whole text (outside: denied) or the NUL ends
    // both views (inside: reads ok.txt); the one thing it must not do is read
    // the secret, which `denied`/`prints` both assert.
    let b = lab.build();
    assert!(b.status.success(), "{}", String::from_utf8_lossy(&b.stderr));
    for out in lab.runs(&[]) {
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(!stdout.contains(TOKEN), "the secret leaked");
    }
    lab.assert_untouched();
}

/// F7: a path longer than `PATH_MAX` is denied rather than truncated into the
/// grant.
#[test]
pub(crate) fn f07_a_path_longer_than_path_max_is_denied() {
    let lab = Lab::new("f07");
    let long = "a/".repeat(2600);
    lab.simple(
        &data_grant(&lab),
        FS,
        &read_prog(&format!("\"__T__/data/{long}../../../../secret/s.txt\"")),
    );
    lab.denied("is outside the fs capability");
}

/// F8: look-alike Unicode (fullwidth dots and solidus, an overlong-style
/// escape) is just a file name: it does not collapse like `..` and does not
/// reach the secret.
#[test]
pub(crate) fn f08_unicode_lookalikes_are_ordinary_names() {
    let lab = Lab::new("f08");
    lab.simple(
        &data_grant(&lab),
        FS,
        "    print(fs.exists(\"__T__/data/\u{ff0e}\u{ff0e}\u{ff0f}secret/s.txt\"))\n    print(fs.exists(\"__T__/data/..\u{2215}secret/s.txt\"))",
    );
    lab.prints("false\nfalse\n");
}

/// F9: `fs.symlink` cannot create a link in the grant that points out of it,
/// because both of its paths are checked.
#[test]
pub(crate) fn f09_a_symlink_out_of_the_grant_cannot_be_created() {
    let lab = Lab::new("f09");
    lab.simple(
        &data_grant(&lab),
        FS,
        "    fs.symlink(\"__T__/secret\", \"__T__/data/esc\")\n    print(fs.read_text(\"__T__/data/esc/s.txt\"))",
    );
    lab.denied("is outside the fs capability");
    assert!(!lab.root.join("data/esc").exists());
}

/// F10: a link that was already there (created by another tool) inside the
/// grant and pointing out of it: a read through it, a write through a
/// *dangling* one that would create a file at its target, a directory link and
/// `read_link` of it are all denied. The dangling write was a real hole (the
/// resolver stopped at the deepest existing ancestor and appended the rest
/// verbatim, so `open(O_CREAT)` created `secret/new.txt`).
#[test]
pub(crate) fn f10_links_out_of_the_grant_are_followed_before_matching() {
    for (what, body) in [
        ("read", "    print(fs.read_text(\"__T__/data/esc/s.txt\"))"),
        (
            "dangling_write",
            "    fs.write_text(\"__T__/data/dangling\", String.from_str(\"pwn\"))\n    print(\"wrote\")",
        ),
        (
            "dangling_write_in_dir",
            "    fs.write_text(\"__T__/data/dangling_dir/new.txt\", String.from_str(\"pwn\"))\n    print(\"wrote\")",
        ),
        (
            "dangling_relative",
            "    fs.write_text(\"__T__/data/rel\", String.from_str(\"pwn\"))\n    print(\"wrote\")",
        ),
        (
            "dangling_chain",
            "    fs.write_text(\"__T__/data/chain1\", String.from_str(\"pwn\"))\n    print(\"wrote\")",
        ),
        (
            "mkdir",
            "    fs.mkdir_all(\"__T__/data/dangling_dir/sub\")\n    print(\"made\")",
        ),
        ("read_link", "    print(fs.read_link(\"__T__/data/esc\"))"),
        (
            "touch",
            "    fs.touch(\"__T__/data/dangling\")\n    print(\"touched\")",
        ),
    ] {
        let lab = Lab::new(&format!("f10_{what}"));
        lab.symlink(&lab.p("secret"), "data/esc");
        lab.symlink(&lab.p("secret/new.txt"), "data/dangling");
        lab.symlink(&lab.p("secret/newdir"), "data/dangling_dir");
        lab.symlink("../secret/new_rel.txt", "data/rel");
        lab.symlink("chain2", "data/chain1");
        lab.symlink(&lab.p("secret/new_chain.txt"), "data/chain2");
        lab.simple(&data_grant(&lab), FS, body);
        lab.denied("is outside the fs capability");
    }
}

/// F11: `fs.read_link` of an in-grant link is allowed only when the link's own
/// resolution stays in the grant (a link out of it is denied, F10); a link whose
/// target is inside reports its text.
#[test]
pub(crate) fn f11_read_link_reports_only_links_that_resolve_inside() {
    let lab = Lab::new("f11");
    lab.symlink(&lab.p("data/ok.txt"), "data/inside");
    lab.simple(&data_grant(&lab), FS, "    print(fs.read_link(\"__T__/data/inside\"))");
    lab.prints(&format!("{}/data/ok.txt\n", lab.root.display()));
}

/// F12: there is no hard-link primitive to name (`fs.link` does not exist), so
/// authority cannot be copied across the grant boundary by `link(2)`.
#[test]
pub(crate) fn f12_there_is_no_hard_link_function() {
    let lab = Lab::new("f12");
    lab.simple(
        &data_grant(&lab),
        FS,
        "    fs.link(\"__T__/secret/s.txt\", \"__T__/data/h\")",
    );
    lab.refused(&["link"]);
}

/// F13: `/proc/self/...` and `/dev/fd/...` are paths like any other: they are
/// matched after resolution, and a grant on `data/**` admits none of them.
#[test]
pub(crate) fn f13_proc_and_dev_fd_are_outside_a_narrow_grant() {
    for (what, path) in [
        ("proc_root", "/proc/self/root/__T__/secret/s.txt"),
        ("proc_cwd", "/proc/self/cwd/secret/s.txt"),
        ("dev_fd", "/dev/fd/0"),
        ("proc_fd", "/proc/self/fd/0"),
        ("data_to_proc", "__T__/data/../../../../proc/self/environ"),
    ] {
        let lab = Lab::new(&format!("f13_{what}"));
        lab.simple(&data_grant(&lab), FS, &read_prog(&format!("\"{path}\"")));
        lab.denied("is outside the fs capability");
    }
}

/// F14: `*` does not cross `/`, `**` does, and neither is applied to the
/// *resource* (a path containing `*` or `?` is a literal name).
#[test]
pub(crate) fn f14_glob_characters_in_a_path_are_literal() {
    let lab = Lab::new("f14a");
    let grant = format!("fs = [\"{}/data/*\"]", lab.root.display());
    fs::create_dir_all(lab.root.join("data/sub")).unwrap();
    fs::write(lab.root.join("data/sub/deep.txt"), "deep").unwrap();
    lab.simple(&grant, FS, &read_prog("\"__T__/data/sub/deep.txt\""));
    lab.denied("is outside the fs capability");

    let lab = Lab::new("f14b");
    lab.simple(&data_grant(&lab), FS, &read_prog("\"__T__/data/*/../../secret/s.txt\""));
    lab.denied("is outside the fs capability");

    let lab = Lab::new("f14c");
    lab.simple(&data_grant(&lab), FS, &read_prog("\"__T__/d?ta/ok.txt\""));
    lab.denied("is outside the fs capability");

    let lab = Lab::new("f14d");
    lab.simple(&data_grant(&lab), FS, &read_prog("\"__T__/[d]ata/ok.txt\""));
    lab.denied("is outside the fs capability");
}

/// F15: a grant pattern that tries to name its way out is refused where it is
/// written: a `..` segment, a relative pattern, and a pattern that is not a
/// path at all.
#[test]
pub(crate) fn f15_a_grant_pattern_cannot_contain_dotdot_or_be_relative() {
    for pat in ["__T__/data/../secret/**", "data/**", "__T__/data/**/../..", ""] {
        let lab = Lab::new("f15");
        let grant = format!("fs = [\"{}\"]", pat.replace("__T__", &lab.root.to_string_lossy()));
        lab.simple(&grant, FS, "    print(1)");
        let out = lab.build();
        assert!(!out.status.success(), "the grant '{pat}' was accepted");
    }
}

/// F16: a module with no `fs` grant cannot name any `fs` item, under any
/// spelling (`fs.read_text`, the raw primitive, an aliased import).
#[test]
pub(crate) fn f16_a_module_without_the_grant_cannot_name_fs() {
    for body in [
        "    print(fs.read_text(\"__T__/secret/s.txt\"))",
        "    print(fs.read_raw(\"__T__/secret/s.txt\").len())",
        "    print(fs.exists(\"__T__/secret/s.txt\"))",
    ] {
        let lab = Lab::new("f16");
        lab.simple("", FS, body);
        lab.refused(&["needs the 'fs' capability"]);
    }
}

/// F17: `rename` checks both ends: moving the secret into the grant, or a
/// granted file out of it, is denied.
#[test]
pub(crate) fn f17_rename_checks_both_paths() {
    for body in [
        "    fs.rename(\"__T__/secret/s.txt\", \"__T__/data/stolen.txt\")\n    print(fs.read_text(\"__T__/data/stolen.txt\"))",
        "    fs.rename(\"__T__/data/ok.txt\", \"__T__/secret/planted.txt\")",
    ] {
        let lab = Lab::new("f17");
        lab.simple(&data_grant(&lab), FS, body);
        lab.denied("is outside the fs capability");
    }
}

/// F18: `copy` checks both ends the same way.
#[test]
pub(crate) fn f18_copy_checks_both_paths() {
    for body in [
        "    fs.copy(\"__T__/secret/s.txt\", \"__T__/data/stolen.txt\")\n    print(fs.read_text(\"__T__/data/stolen.txt\"))",
        "    fs.copy(\"__T__/data/ok.txt\", \"__T__/secret/planted.txt\")",
    ] {
        let lab = Lab::new("f18");
        lab.simple(&data_grant(&lab), FS, body);
        lab.denied("is outside the fs capability");
    }
}

/// F19: directory walks cannot climb or follow links out: `list` of the parent
/// is denied; `walk` and `remove_all` over a tree holding a link to `secret/`
/// report or unlink the link and never descend; the secret survives.
#[test]
pub(crate) fn f19_walk_and_remove_all_do_not_follow_links_out() {
    let lab = Lab::new("f19a");
    lab.simple(&data_grant(&lab), FS, "    print(fs.list(\"__T__\").len())");
    lab.denied("is outside the fs capability");

    let lab = Lab::new("f19b");
    lab.symlink(&lab.p("secret"), "data/esc");
    lab.simple(
        &data_grant(&lab),
        FS,
        "    val all = fs.walk(\"__T__/data\")\n    print(all.len())",
    );
    // `walk` reports the link as one entry and does not descend: the entries
    // are `esc` and `ok.txt`.
    lab.prints("2\n");

    let lab = Lab::new("f19c");
    lab.symlink(&lab.p("secret"), "data/esc");
    lab.simple(&data_grant(&lab), FS, "    fs.remove_all(\"__T__/data\")\n    print(\"gone\")");
    // `remove_all` panics on the link out of the grant or unlinks it; either
    // way the secret is untouched (checked by the helper).
    for out in lab.runs(&[]) {
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(!stdout.contains(TOKEN));
        // recreate what the first run removed, for the second backend
        let _ = fs::create_dir_all(lab.root.join("data"));
        let _ = std::os::unix::fs::symlink(lab.root.join("secret"), lab.root.join("data/esc"));
    }
    lab.assert_untouched();
}

/// F20: `fs.temp_path`, whose argument is a prefix that gets six random
/// characters appended: the prefix is the thing checked, so a prefix that
/// normalizes out of the grant is denied, and one inside creates its file
/// inside.
#[test]
pub(crate) fn f20_temp_path_cannot_create_outside_the_grant() {
    let lab = Lab::new("f20a");
    lab.simple(&data_grant(&lab), FS, "    print(fs.temp_path(\"__T__/secret/t-\"))");
    lab.denied("is outside the fs capability");

    let lab = Lab::new("f20b");
    lab.simple(&data_grant(&lab), FS, "    print(fs.temp_path(\"__T__/data/../secret/t-\"))");
    lab.denied("is outside the fs capability");

    // A prefix that is a directory-ish name ending in `..` cannot make the
    // suffix land elsewhere: `..XXXXXX` is one name.
    let lab = Lab::new("f20c");
    lab.simple(&data_grant(&lab), FS, "    print(fs.temp_path(\"__T__/data/..\"))");
    lab.denied("is outside the fs capability");
}

/// F21: `open` takes a mode as its second string argument: the mode is not a
/// path, and the path is still checked, so the secret cannot be opened.
#[test]
pub(crate) fn f21_open_checks_the_path_and_not_the_mode() {
    let lab = Lab::new("f21");
    lab.simple(
        &data_grant(&lab),
        FS,
        "    val f = fs.open(\"__T__/secret/s.txt\", \"r\")\n    print(1)",
    );
    lab.denied("is outside the fs capability");
}

// ---------------------------------------------------------------------------
// process
// ---------------------------------------------------------------------------
