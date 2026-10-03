//! The capability threat model's attack tests (`plan/threat-model.md`).
//!
//! One test per numbered item of the model. Every test plays a program author
//! who holds a *narrow* grant (or none) and tries to obtain authority the
//! manifest did not give, and asserts that the attempt is refused: at compile
//! time (`refused`), at run time by the capability check (`denied`), or that it
//! is *inert* (the program runs but the thing the author wanted did not
//! happen, `inert`). An attack that succeeds is a bug; its test stays as the
//! regression test, and a hole too large to close is `#[ignore = "known gap:
//! ..."]`, never deleted.
//!
//! Safety rules the tests keep, because they are attacks:
//!   * every file a program touches is inside a directory the test creates;
//!   * the only network address is `127.0.0.1` (a denied name is refused before
//!     any dial, so a name like `localhost` or `127.0.0.2` never connects);
//!   * the only programs that run are tiny scripts the test writes into its
//!     directory, which `echo` and nothing else;
//!   * every compiled program runs under `ulimit -v 1048576; ulimit -t 10;
//!     timeout 10`.
//!
//! The secret is `secret/s.txt` (content `TOKEN-7f3a`): no stdout may contain
//! it, and `secret/` must hold exactly that one file afterwards.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

const TOKEN: &str = "TOKEN-7f3a";

fn kelpc() -> Command {
    Command::new(env!("CARGO_BIN_EXE_kelpc"))
}

/// A fresh scratch directory holding `data/ok.txt` and `secret/s.txt`, named by
/// its real path (symlinks resolved), because grants are matched against
/// resolved paths.
struct Lab {
    root: PathBuf,
}

impl Lab {
    fn new(name: &str) -> Lab {
        let base = match option_env!("CARGO_TARGET_TMPDIR") {
            Some(dir) => PathBuf::from(dir),
            None => std::env::temp_dir().join("kelpc-threat-model-tests"),
        };
        let root = base.join("threat-model").join(name);
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("data")).expect("create data/");
        fs::create_dir_all(root.join("secret")).expect("create secret/");
        fs::write(root.join("data/ok.txt"), "fine\n").expect("write ok.txt");
        fs::write(root.join("secret/s.txt"), TOKEN).expect("write s.txt");
        let root = fs::canonicalize(&root).expect("canonicalize the lab");
        Lab { root }
    }

    /// The absolute path of `rel` inside the lab.
    fn p(&self, rel: &str) -> String {
        format!("{}/{}", self.root.display(), rel)
    }

    /// Writes a Kelp source file, replacing `__T__` with the lab's path.
    fn src(&self, file: &str, text: &str) {
        let text = text.replace("__T__", &self.root.to_string_lossy());
        fs::write(self.root.join(file), text).expect("write a source file");
    }

    /// The usual shape: `main` delegates `grant` (a `caps(...)` body) to `util`
    /// and calls `util.go()`, which is `body` under the given imports.
    fn simple(&self, grant: &str, imports: &[&str], body: &str) {
        let main = if grant.is_empty() {
            "impl \"util\"\n\nfun main() {\n    util.go()\n}\n".to_string()
        } else {
            format!("impl \"util\" caps({grant})\n\nfun main() {{\n    util.go()\n}}\n")
        };
        self.src("main.kp", &main);
        let mut util = String::new();
        for i in imports {
            util.push_str(&format!("impl \"{i}\"\n"));
        }
        util.push_str(&format!("\npub fun go() {{\n{body}\n}}\n"));
        self.src("util.kp", &util);
    }

    fn build(&self, oracle: bool) -> Output {
        let mut cmd = kelpc();
        cmd.arg("build").env_remove("KELPC_BACKEND");
        if oracle {
            cmd.env("KELPC_ORACLE", "c");
        } else {
            cmd.env_remove("KELPC_ORACLE");
        }
        cmd.arg(self.root.join("main.kp"))
            .arg("-o")
            .arg(self.root.join(if oracle { "prog_c" } else { "prog_llvm" }))
            .current_dir(&self.root)
            .output()
            .expect("spawn kelpc")
    }

    fn run(&self, oracle: bool, args: &[&str]) -> Output {
        let prog = if oracle { "prog_c" } else { "prog_llvm" };
        Command::new("sh")
            .arg("-c")
            .arg(format!(
                "ulimit -v 1048576; ulimit -t 10; exec timeout 10 ./{prog} \"$@\""
            ))
            .arg("sh")
            .args(args)
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .output()
            .expect("run the program")
    }

    /// The program must not compile; the message holds every needle.
    fn refused(&self, needles: &[&str]) {
        let out = self.build(false);
        let text = String::from_utf8_lossy(&out.stderr).to_string();
        assert!(!out.status.success(), "expected a compile refusal, but it built");
        for n in needles {
            assert!(text.contains(n), "refusal lacks '{n}':\n{text}");
        }
        self.assert_untouched();
    }

    /// The program compiles with both backends and every run fails with a
    /// message holding the needle; nothing leaked.
    fn denied(&self, needle: &str) {
        for oracle in [false, true] {
            let b = self.build(oracle);
            assert!(
                b.status.success(),
                "expected the build to succeed (oracle={oracle}):\n{}",
                String::from_utf8_lossy(&b.stderr)
            );
            let out = self.run(oracle, &[]);
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            assert!(!stdout.contains(TOKEN), "the secret leaked (oracle={oracle}): {stdout}");
            assert!(
                !out.status.success() && stderr.contains(needle),
                "expected a denial containing '{needle}' (oracle={oracle}), got status {:?}\nstdout:{stdout}\nstderr:{stderr}",
                out.status.code()
            );
        }
        self.assert_untouched();
    }

    /// The program compiles with both backends and runs; the output is
    /// returned (per backend) for the test to check.
    fn runs(&self, args: &[&str]) -> Vec<(bool, Output)> {
        let mut all = Vec::new();
        for oracle in [false, true] {
            let b = self.build(oracle);
            assert!(
                b.status.success(),
                "expected the build to succeed (oracle={oracle}):\n{}",
                String::from_utf8_lossy(&b.stderr)
            );
            all.push((oracle, self.run(oracle, args)));
        }
        all
    }

    /// The program runs on both backends and prints exactly `want`.
    fn prints(&self, want: &str) {
        for (oracle, out) in self.runs(&[]) {
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            assert!(!stdout.contains(TOKEN), "the secret leaked (oracle={oracle}): {stdout}");
            assert!(
                out.status.success() && stdout == want,
                "expected '{want}' (oracle={oracle}), got status {:?}\nstdout:{stdout}\nstderr:{stderr}",
                out.status.code()
            );
        }
        self.assert_untouched();
    }

    /// `secret/` holds exactly `s.txt`, unchanged.
    fn assert_untouched(&self) {
        let mut names: Vec<String> = fs::read_dir(self.root.join("secret"))
            .expect("read secret/")
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        names.sort();
        assert_eq!(names, vec!["s.txt".to_string()], "secret/ was changed");
        assert_eq!(
            fs::read_to_string(self.root.join("secret/s.txt")).unwrap(),
            TOKEN,
            "secret/s.txt was changed"
        );
    }

    fn symlink(&self, target: &str, link: &str) {
        std::os::unix::fs::symlink(target, self.root.join(link)).expect("create a symlink");
    }
}

/// The grant used by most `fs` attacks: `data/**` and nothing else.
fn data_grant(lab: &Lab) -> String {
    format!("fs = [\"{}/data/**\"]", lab.root.display())
}

fn read_prog(path_expr: &str) -> String {
    format!("    print(fs.read_text({path_expr}))")
}

const FS: &[&str] = &["std/fs"];

// ---------------------------------------------------------------------------
// fs
// ---------------------------------------------------------------------------

/// F1: `..` segments climbing out of the granted root.
#[test]
fn f01_dotdot_segments_cannot_leave_the_grant() {
    let lab = Lab::new("f01");
    lab.simple(&data_grant(&lab), FS, &read_prog("\"__T__/data/../secret/s.txt\""));
    lab.denied("is outside the fs capability");
}

/// F2: `.` segments and doubled separators are collapsed, so a path that is
/// inside the grant works (the positive control) and one that is outside after
/// collapsing is denied.
#[test]
fn f02_dot_segments_and_double_slashes_collapse_before_matching() {
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
fn f03_a_sibling_sharing_the_prefix_is_outside_the_grant() {
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
fn f04_a_relative_path_is_never_matched_against_the_cwd() {
    let lab = Lab::new("f04");
    lab.simple(&data_grant(&lab), FS, &read_prog("\"ok.txt\""));
    for oracle in [false, true] {
        assert!(lab.build(oracle).status.success());
        let out = Command::new(lab.root.join(if oracle { "prog_c" } else { "prog_llvm" }))
            .current_dir(lab.root.join("data"))
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            !out.status.success() && stderr.contains("is outside the fs capability"),
            "a relative path was accepted (oracle={oracle}): {stderr}"
        );
    }
}

/// F5: case is significant: `DATA` is not `data` on a case-sensitive
/// filesystem, and the matcher must not fold it either.
#[test]
fn f05_the_grant_is_case_sensitive() {
    let lab = Lab::new("f05");
    lab.simple(&data_grant(&lab), FS, &read_prog("\"__T__/DATA/ok.txt\""));
    lab.denied("is outside the fs capability");
}

/// F6: a path whose C string ends early. The check and the syscall both see the
/// text up to the NUL, so nothing past it can be smuggled in. (`\x00` cannot be
/// written in a literal; the NUL comes from `String.from_bytes` over a byte
/// buffer, the one way to make a `str` that contains one.)
#[test]
fn f06_an_embedded_nul_cannot_hide_a_second_path() {
    let lab = Lab::new("f06");
    lab.simple(
        &data_grant(&lab),
        &["std/fs", "std/mem", "std/str"],
        "    val head = \"__T__/data/ok.txt\"\n    val tail = \"/../../secret/s.txt\"\n    val hv = head.view()\n    val tv = tail.view()\n    val hn = str.len(head)\n    val tn = str.len(tail)\n    var b = mem.alloc(hn + 1 + tn)\n    var i: usize = 0\n    while i < hn {\n        b.set(i, hv.get(i))\n        i = i + 1\n    }\n    b.set(hn, 0)\n    var j: usize = 0\n    while j < tn {\n        b.set(hn + 1 + j, tv.get(j))\n        j = j + 1\n    }\n    val p = String.from_bytes(b)\n    print(fs.read_text(p.as_str()))",
    );
    // Either the check sees the whole text (outside: denied) or the NUL ends
    // both views (inside: reads ok.txt); the one thing it must not do is read
    // the secret, which `denied`/`prints` both assert.
    let b = lab.build(false);
    assert!(b.status.success(), "{}", String::from_utf8_lossy(&b.stderr));
    for (oracle, out) in lab.runs(&[]) {
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(!stdout.contains(TOKEN), "the secret leaked (oracle={oracle})");
    }
    lab.assert_untouched();
}

/// F7: a path longer than `PATH_MAX` is denied rather than truncated into the
/// grant.
#[test]
fn f07_a_path_longer_than_path_max_is_denied() {
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
fn f08_unicode_lookalikes_are_ordinary_names() {
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
fn f09_a_symlink_out_of_the_grant_cannot_be_created() {
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
fn f10_links_out_of_the_grant_are_followed_before_matching() {
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
fn f11_read_link_reports_only_links_that_resolve_inside() {
    let lab = Lab::new("f11");
    lab.symlink(&lab.p("data/ok.txt"), "data/inside");
    lab.simple(&data_grant(&lab), FS, "    print(fs.read_link(\"__T__/data/inside\"))");
    lab.prints(&format!("{}/data/ok.txt\n", lab.root.display()));
}

/// F12: there is no hard-link primitive to name (`fs.link` does not exist), so
/// authority cannot be copied across the grant boundary by `link(2)`.
#[test]
fn f12_there_is_no_hard_link_function() {
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
fn f13_proc_and_dev_fd_are_outside_a_narrow_grant() {
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
fn f14_glob_characters_in_a_path_are_literal() {
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
fn f15_a_grant_pattern_cannot_contain_dotdot_or_be_relative() {
    for pat in ["__T__/data/../secret/**", "data/**", "__T__/data/**/../..", ""] {
        let lab = Lab::new("f15");
        let grant = format!("fs = [\"{}\"]", pat.replace("__T__", &lab.root.to_string_lossy()));
        lab.simple(&grant, FS, "    print(1)");
        let out = lab.build(false);
        assert!(!out.status.success(), "the grant '{pat}' was accepted");
    }
}

/// F16: a module with no `fs` grant cannot name any `fs` item, under any
/// spelling (`fs.read_text`, the raw primitive, an aliased import).
#[test]
fn f16_a_module_without_the_grant_cannot_name_fs() {
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
fn f17_rename_checks_both_paths() {
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
fn f18_copy_checks_both_paths() {
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
fn f19_walk_and_remove_all_do_not_follow_links_out() {
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
    for (_, out) in lab.runs(&[]) {
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
fn f20_temp_path_cannot_create_outside_the_grant() {
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
fn f21_open_checks_the_path_and_not_the_mode() {
    let lab = Lab::new("f21");
    lab.simple(
        &data_grant(&lab),
        FS,
        "    val f = fs.open(\"__T__/secret/s.txt\", \"r\")\n    print(1)",
    );
    lab.denied("is outside the fs capability");
}
