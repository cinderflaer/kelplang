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

// ---------------------------------------------------------------------------
// process
// ---------------------------------------------------------------------------

const PROC: &[&str] = &["std/process"];

/// Two scripts the test writes: `tools/ok.sh` (echoes its arguments) and
/// `tools/evil.sh`, whose only effect is to create the marker `pwned` in the
/// lab. Neither runs anything but `echo`.
fn tools(lab: &Lab) {
    use std::os::unix::fs::PermissionsExt;
    fs::create_dir_all(lab.root.join("tools")).unwrap();
    for (name, text) in [
        ("ok.sh", "#!/bin/sh\necho ok \"$@\"\n"),
        ("evil.sh", "#!/bin/sh\necho pwned > \"$(dirname \"$0\")/../pwned\"\necho evil\n"),
    ] {
        let p = lab.root.join("tools").join(name);
        fs::write(&p, text).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn run_prog(program_expr: &str, args: &[&str]) -> String {
    let mut s = String::from("    var args: List<String> = List.empty()\n");
    for a in args {
        s.push_str(&format!("    args.push(String.from_str(\"{a}\"))\n"));
    }
    s.push_str(&format!(
        "    val r = process.run({program_expr}, args)\n    print(r.code)\n    print(r.out)"
    ));
    s
}

fn no_marker(lab: &Lab) {
    assert!(!lab.root.join("pwned").exists(), "the unlisted program ran");
}

/// P1: a module without the grant cannot name `process`.
#[test]
fn p01_a_module_without_the_grant_cannot_name_process() {
    let lab = Lab::new("p01");
    tools(&lab);
    lab.simple("", PROC, &run_prog("\"__T__/tools/ok.sh\"", &[]));
    lab.refused(&["needs the 'process' capability"]);
}

/// P2: a program outside the grant is denied before the fork.
#[test]
fn p02_a_program_outside_the_grant_is_not_started() {
    let lab = Lab::new("p02");
    tools(&lab);
    lab.simple(
        &format!("process = [\"{}/tools/ok.sh\"]", lab.root.display()),
        PROC,
        &run_prog("\"__T__/tools/evil.sh\"", &[]),
    );
    lab.denied("is outside the process capability");
    no_marker(&lab);
}

/// P3: a bare name is never looked up on `PATH`: it is not absolute, so it is a
/// denial, not a search.
#[test]
fn p03_a_bare_program_name_is_not_searched_on_path() {
    let lab = Lab::new("p03");
    tools(&lab);
    lab.simple(
        &format!("process = [\"{}/tools/*\"]", lab.root.display()),
        PROC,
        &run_prog("\"evil.sh\"", &[]),
    );
    lab.denied("is outside the process capability");
    no_marker(&lab);
}

/// P4: `..` and a file-as-directory segment cannot name another program.
#[test]
fn p04_dotdot_cannot_name_another_program() {
    for path in [
        "__T__/tools/../tools/evil.sh",
        "__T__/tools/ok.sh/../evil.sh",
        "__T__/tools/./ok.sh/../evil.sh",
    ] {
        let lab = Lab::new("p04");
        tools(&lab);
        lab.simple(
            &format!("process = [\"{}/tools/ok.sh\"]", lab.root.display()),
            PROC,
            &run_prog(&format!("\"{path}\""), &[]),
        );
        lab.denied("is outside the process capability");
        no_marker(&lab);
    }
}

/// P5: a name inside the grant that is a symlink to a program outside it is
/// matched after resolution, so it is denied.
#[test]
fn p05_a_granted_name_that_links_to_another_program_is_denied() {
    let lab = Lab::new("p05");
    tools(&lab);
    lab.symlink("evil.sh", "tools/okl");
    lab.simple(
        &format!("process = [\"{}/tools/ok*\"]", lab.root.display()),
        PROC,
        &run_prog("\"__T__/tools/okl\"", &[]),
    );
    lab.denied("is outside the process capability");
    no_marker(&lab);
}

/// P6: arguments are not interpreted by a shell: metacharacters reach the
/// granted program as literal text, so no second command runs.
#[test]
fn p06_argument_injection_is_inert() {
    let lab = Lab::new("p06");
    tools(&lab);
    lab.simple(
        &format!("process = [\"{}/tools/ok.sh\"]", lab.root.display()),
        PROC,
        &run_prog(
            "\"__T__/tools/ok.sh\"",
            &["; sh __T__/tools/evil.sh", "$(sh __T__/tools/evil.sh)", "`sh __T__/tools/evil.sh`", "| sh __T__/tools/evil.sh"],
        ),
    );
    for (_, out) in lab.runs(&[]) {
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        assert!(stdout.starts_with("0\nok ; sh "), "the arguments were not literal: {stdout}");
        no_marker(&lab);
    }
}

/// P7: the documented breadth of the grant: naming a shell lets the module run
/// whatever that shell runs. The grant is on the *program*, and the arguments
/// are the module's business (`std/process.kp` says so). The test pins it so a
/// change in either direction is a decision, and runs only `echo`.
#[test]
fn p07_granting_a_shell_grants_what_the_shell_runs() {
    let lab = Lab::new("p07");
    tools(&lab);
    lab.simple(
        "process = [\"/bin/sh\", \"/nix/store/**\", \"/usr/bin/**\"]",
        PROC,
        &run_prog("\"/bin/sh\"", &["-c", "echo through-the-shell"]),
    );
    lab.prints("0\nthrough-the-shell\n\n");
}

/// P8: `process.kill` signals only a child this process started. Signal 0 to
/// the test's own pid (argv[1]) is harmless if it were delivered, and it must
/// not be: a narrow program grant is not authority over every process of the
/// user (the Minecraft server, a shell, the parent).
#[test]
fn p08_kill_reaches_only_children_this_program_started() {
    let lab = Lab::new("p08");
    tools(&lab);
    lab.simple(
        &format!("process = [\"{}/tools/ok.sh\"]", lab.root.display()),
        &["std/process", "std/str", "std/result"],
        "    val args = process.args()\n    val pid = result.expect(r = str.parse_u64(args.get(1).as_str()), message = \"pid\")\n    process.kill_raw(pid.to_i32(), 0)\n    print(\"signalled a process that is not a child\")",
    );
    let pid = std::process::id().to_string();
    for oracle in [false, true] {
        let b = lab.build(oracle);
        assert!(b.status.success(), "{}", String::from_utf8_lossy(&b.stderr));
        let out = lab.run(oracle, &[&pid]);
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            !out.status.success() && !stdout.contains("signalled") && stderr.contains("not a child"),
            "kill reached a process this program did not start (oracle={oracle}): {stdout}{stderr}"
        );
    }
}

/// P9: a forged `Child` handle carries the same fate as P8: `Child.kill` goes
/// through the same primitive, so naming a foreign pid in a handle does not
/// reach it either.
#[test]
fn p09_a_forged_child_handle_cannot_signal_a_foreign_pid() {
    let lab = Lab::new("p09");
    tools(&lab);
    lab.simple(
        &format!("process = [\"{}/tools/ok.sh\"]", lab.root.display()),
        &["std/process", "std/io", "std/str", "std/result"],
        "    val args = process.args()\n    val pid = result.expect(r = str.parse_u64(args.get(1).as_str()), message = \"pid\")\n    val c = process.Child(pid = pid.to_i32(), stdin = io.File(fd = -1), stdout = io.File(fd = -1), stderr = io.File(fd = -1), exited = false, code = 0)\n    c.kill(0)\n    print(\"signalled a process that is not a child\")",
    );
    let pid = std::process::id().to_string();
    for oracle in [false, true] {
        let b = lab.build(oracle);
        assert!(b.status.success(), "{}", String::from_utf8_lossy(&b.stderr));
        let out = lab.run(oracle, &[&pid]);
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            !out.status.success() && !stdout.contains("signalled") && stderr.contains("not a child"),
            "a forged handle signalled a foreign pid (oracle={oracle}): {stdout}{stderr}"
        );
    }
}

// ---------------------------------------------------------------------------
// env
// ---------------------------------------------------------------------------

const ENV: &[&str] = &["std/env"];

/// E1: a name outside the grant can be neither read, probed, set nor unset.
#[test]
fn e01_a_name_outside_the_grant_is_denied_for_every_operation() {
    for body in [
        "    print(env.get(\"HOME\"))",
        "    print(env.has(\"HOME\"))",
        "    env.set(\"HOME\", \"x\")",
        "    env.unset(\"HOME\")",
        "    print(env.get_raw(\"HOME\"))",
    ] {
        let lab = Lab::new("e01");
        lab.simple("env = [\"APP_MODE\"]", ENV, body);
        lab.denied("is outside the env capability");
    }
}

/// E2: two modules with disjoint grants: neither can read or write the other's
/// variable, even though both are granted `env`.
#[test]
fn e02_one_modules_variable_is_not_anothers() {
    let lab = Lab::new("e02");
    lab.src(
        "main.kp",
        "impl \"a\" caps(env = [\"VAR_A\"])\nimpl \"b\" caps(env = [\"VAR_B\"])\n\nfun main() {\n    a.set()\n    b.steal()\n}\n",
    );
    lab.src("a.kp", "impl \"std/env\"\n\npub fun set() {\n    env.set(\"VAR_A\", \"secret-a\")\n}\n");
    lab.src("b.kp", "impl \"std/env\"\n\npub fun steal() {\n    print(env.get(\"VAR_A\"))\n}\n");
    lab.denied("is outside the env capability");
}

/// E3: a wildcard name grant does not cross `/`, and a name that merely starts
/// like the pattern but is a different variable is outside it.
#[test]
fn e03_a_name_wildcard_matches_names_only() {
    for name in ["APP_/../HOME", "APP", "XAPP_MODE", "app_mode"] {
        let lab = Lab::new("e03");
        lab.simple("env = [\"APP_*\"]", ENV, &format!("    print(env.get(\"{name}\"))"));
        lab.denied("is outside the env capability");
    }
}

/// E4: the environment cannot be enumerated: `env.names` does not exist, so a
/// one-name grant cannot be widened by listing.
#[test]
fn e04_the_environment_cannot_be_listed() {
    let lab = Lab::new("e04");
    lab.simple("env = [\"APP_MODE\"]", ENV, "    print(env.names().len())");
    lab.refused(&["names"]);
}

/// E5: a one-name grant does not reach `LD_PRELOAD` or `PATH`: the variables
/// that change what a *later* process start does are ordinary names under the
/// ordinary check.
#[test]
fn e05_loader_variables_need_their_own_grant() {
    for name in ["LD_PRELOAD", "LD_LIBRARY_PATH", "PATH"] {
        let lab = Lab::new("e05");
        lab.simple(
            "env = [\"APP_MODE\"]",
            ENV,
            &format!("    env.set(\"{name}\", \"/tmp/x\")"),
        );
        lab.denied("is outside the env capability");
    }
}

/// E6: a module that holds both `env` and `process` is the composition the
/// model documents rather than prevents: with `env = ["*"]` it can set
/// `LD_PRELOAD` for the programs it may start. That is two grants the
/// manifest author wrote, each narrow, and the test pins the check that does
/// hold: with `env` granted only a harmless name, the child sees nothing it
/// set under another name.
#[test]
fn e06_a_child_only_sees_variables_the_module_could_name() {
    let lab = Lab::new("e06");
    tools(&lab);
    lab.simple(
        &format!(
            "env = [\"APP_MODE\"], process = [\"{}/tools/ok.sh\"]",
            lab.root.display()
        ),
        &["std/env", "std/process"],
        "    env.set(\"LD_PRELOAD\", \"/nonexistent/evil.so\")\n    print(\"unreachable\")",
    );
    lab.denied("is outside the env capability");
}

// ---------------------------------------------------------------------------
// net
// ---------------------------------------------------------------------------

const NET: &[&str] = &["std/net"];

fn connect_prog(host: &str, port: u16) -> String {
    format!("    val s = net.connect(\"{host}\", {port})\n    print(\"connected\")\n    net.close(s)")
}

/// N1: a host the grant does not name is denied before any dial: another
/// loopback address, a name that is the same machine, a numeric spelling of the
/// same address, a suffix trick, and an injected `:port` in the host text.
#[test]
fn n01_a_host_outside_the_grant_is_denied_before_the_dial() {
    for host in [
        "127.0.0.2",
        "localhost",
        "127.1",
        "0x7f.0.0.1",
        "2130706433",
        "127.0.0.1.invalid",
        "127.0.0.1:9",
        " 127.0.0.1",
        "127.0.0.1 ",
        "LOCALHOST",
    ] {
        let lab = Lab::new("n01");
        lab.simple("net = [\"127.0.0.1:9\"]", NET, &connect_prog(host, 9));
        lab.denied("is outside the net capability");
    }
}

/// N2: a grant on a *name* does not cover that name's address: matching is on
/// the string the code wrote, so `localhost:*` does not admit `127.0.0.1`.
#[test]
fn n02_a_name_grant_does_not_cover_the_address() {
    let lab = Lab::new("n02");
    lab.simple("net = [\"localhost:9\"]", NET, &connect_prog("127.0.0.1", 9));
    lab.denied("is outside the net capability");
}

/// N3: the port is part of the grant: another port on the granted host is
/// outside it, and the port cannot be smuggled through the host text.
#[test]
fn n03_a_port_outside_the_grant_is_denied() {
    for (host, port) in [("127.0.0.1", 10u16), ("127.0.0.1:9", 9u16), ("127.0.0.1:9:", 9u16)] {
        let lab = Lab::new("n03");
        lab.simple("net = [\"127.0.0.1:9\"]", NET, &connect_prog(host, port));
        lab.denied("is outside the net capability");
    }
}

/// N4: a numeric-looking host pattern is a statement about addresses, and a
/// host *name* that fits it is not one: `127.0.0.*` must not admit
/// `127.0.0.zzz`, which is a DNS name its owner can point anywhere. The
/// matcher's verdict (`host_match`) is `true` for it, so the refusal is the
/// capability check's own rule that a numeric pattern admits only an IPv4
/// literal. Run through `net.connect` (denied before any lookup).
#[test]
fn n04_a_numeric_pattern_admits_only_ip_literals() {
    for host in ["127.0.0.zzz", "127.0.0.1x", "127.0.0.example"] {
        let lab = Lab::new("n04");
        lab.simple("net = [\"127.0.0.*:9\"]", NET, &connect_prog(host, 9));
        lab.denied("is outside the net capability");
    }
}

/// N4b: `net.resolve` is held to the same rule, since resolving a name the
/// numeric pattern should not admit is already a lookup of an attacker-chosen
/// name.
#[test]
fn n04b_resolve_applies_the_same_numeric_rule() {
    let lab = Lab::new("n04b");
    lab.simple(
        "net = [\"127.0.0.*\"]",
        NET,
        "    print(net.resolve(\"127.0.0.zzz\").len())",
    );
    lab.denied("is outside the net capability");
}

/// The text of `/proc/net/tcp` lines for `port`, as `local_address st` pairs.
fn listener_line(stdout: &str, port: u16) -> Option<String> {
    let want = format!(":{port:04X}");
    stdout.lines().find_map(|l| {
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() > 3 && f[1].ends_with(&want) && f[3] == "0A" {
            Some(f[1].to_string())
        } else {
            None
        }
    })
}

/// N5: a loopback-only grant gives a loopback-only listener. `net.listen`
/// has no host argument and used to bind `0.0.0.0` whatever the grant said, so
/// a module granted `127.0.0.1:*` could serve the whole network. The program
/// reads its own `/proc/net/tcp` to see the address it is really bound to.
#[test]
fn n05_a_loopback_grant_binds_loopback_only() {
    for (grant, want) in [
        ("127.0.0.1:*", "0100007F"),
        ("localhost:*", "0100007F"),
        ("127.0.0.1:0", "0100007F"),
        ("*", "00000000"),
    ] {
        let lab = Lab::new("n05");
        lab.simple(
            &format!("net = [\"{grant}\"], fs = [\"/proc/*/net/tcp\"]"),
            &["std/net", "std/fs", "std/io"],
            "    val l = net.listen(0)\n    print(l.port())\n    val f = fs.open(\"/proc/net/tcp\", \"r\")\n    print(String.from_bytes(f.read_all()))\n    l.close()",
        );
        for (oracle, out) in lab.runs(&[]) {
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
            let port: u16 = stdout.lines().next().unwrap().trim().parse().unwrap();
            let line = listener_line(&stdout, port)
                .unwrap_or_else(|| panic!("no LISTEN line for port {port}:\n{stdout}"));
            assert_eq!(
                line.split(':').next().unwrap(),
                want,
                "grant '{grant}' bound {line} (oracle={oracle})"
            );
        }
    }
}

/// N5b: the HTTP server goes through the same `net.listen`, so the same
/// loopback grant gives a loopback server.
#[test]
fn n05b_the_http_server_binds_loopback_under_a_loopback_grant() {
    let lab = Lab::new("n05b");
    lab.src(
        "main.kp",
        "impl \"std/http\" caps(net = [\"127.0.0.1:*\"])\nimpl \"probe\" caps(fs = [\"/proc/*/net/tcp\"])\n\nfun main() {\n    val s = http.listen(0)\n    print(s.port())\n    print(probe.table())\n    s.close()\n}\n",
    );
    lab.src(
        "probe.kp",
        "impl \"std/fs\"\nimpl \"std/io\"\n\npub fun table().R: String {\n    val f = fs.open(\"/proc/net/tcp\", \"r\")\n    return String.from_bytes(f.read_all())\n}\n",
    );
    for (oracle, out) in lab.runs(&[]) {
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let port: u16 = stdout.lines().next().unwrap().trim().parse().unwrap();
        let line = listener_line(&stdout, port).expect("a LISTEN line");
        assert_eq!(line.split(':').next().unwrap(), "0100007F", "oracle={oracle}: {line}");
    }
}

/// N6: the listen port is checked against the grant's port: another port, and
/// the "any port" request `0` under a one-port grant, are denied.
#[test]
fn n06_a_listen_port_outside_the_grant_is_denied() {
    for port in [0u16, 8081, 80] {
        let lab = Lab::new("n06");
        lab.simple(
            "net = [\"127.0.0.1:8080\"]",
            NET,
            &format!("    val l = net.listen({port})\n    print(\"bound\")\n    l.close()"),
        );
        lab.denied("is outside the net capability");
    }
}

/// A one-shot loopback HTTP server: answers the first request with a 302 to
/// `location` (`PORT` is replaced by the server's own port), and reports how
/// many connections it took in the following second.
fn redirect_server(location: &str) -> (u16, std::thread::JoinHandle<usize>) {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().unwrap().port();
    let response = format!(
        "HTTP/1.1 302 Found\r\nLocation: {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        location.replace("PORT", &port.to_string())
    );
    let handle = std::thread::spawn(move || {
        let mut taken = 0;
        let (mut c, _) = listener.accept().expect("accept");
        taken += 1;
        let mut buf = [0u8; 2048];
        let mut seen = Vec::new();
        while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = c.read(&mut buf).unwrap_or(0);
            if n == 0 {
                break;
            }
            seen.extend_from_slice(&buf[..n]);
        }
        let _ = c.write_all(response.as_bytes());
        drop(c);
        listener.set_nonblocking(true).unwrap();
        for _ in 0..10 {
            if listener.accept().is_ok() {
                taken += 1;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        taken
    });
    (port, handle)
}

/// N7: a redirect cannot lead an `http` client to a host its grant does not
/// name: the follow-up request is a fresh `net.connect` under the same grant.
/// Another loopback address and a name for this machine are both refused, and
/// the server saw exactly the one connection.
#[test]
fn n07_a_redirect_to_an_unlisted_host_is_refused() {
    for location in ["http://127.0.0.2:PORT/loot", "http://localhost:PORT/loot"] {
        let lab = Lab::new("n07");
        lab.src(
            "main.kp",
            "impl \"std/http\" caps(net = [\"127.0.0.1:*\"])\nimpl \"std/process\"\n\nfun main() {\n    val args = process.args()\n    val r = http.get(args.get(1).as_str())\n    print(r.status)\n}\n",
        );
        let (port, server) = redirect_server(location);
        let url = format!("http://127.0.0.1:{port}/start");
        let b = lab.build(false);
        assert!(b.status.success(), "{}", String::from_utf8_lossy(&b.stderr));
        let out = lab.run(false, &[&url]);
        let taken = server.join().unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            !out.status.success() && stderr.contains("is outside the net capability"),
            "redirect to {location} was not refused: {stderr}"
        );
        assert_eq!(taken, 1, "a second connection reached the server");
    }
}

/// N8: `std/ws` and `std/tls` are in the `net` domain: a module without the
/// grant cannot name `ws.connect`, `tls.handshake` or `tls.accept`, and with a
/// narrow grant `ws.connect` to another host is denied by the same check.
#[test]
fn n08_ws_and_tls_are_behind_the_net_gate() {
    for (import, body) in [
        ("std/ws", "    val c = ws.connect(\"127.0.0.1\", 9, \"/\")"),
        ("std/tls", "    print(tls.server_options().floor)"),
    ] {
        let lab = Lab::new("n08a");
        lab.simple("", &[import], body);
        lab.refused(&["needs the 'net' capability"]);
    }
    let lab = Lab::new("n08b");
    lab.src(
        "main.kp",
        "impl \"util\" caps(net = [\"127.0.0.1:9\"], rand = [])\n\nfun main() {\n    util.go()\n}\n",
    );
    lab.src(
        "util.kp",
        "impl \"std/ws\" caps(net = [\"127.0.0.1:9\"], rand = [])\n\npub fun go() {\n    val c = ws.connect(\"127.0.0.1\", 10, \"/\")\n}\n",
    );
    lab.denied("is outside the net capability");
}

// ---------------------------------------------------------------------------
// handles
// ---------------------------------------------------------------------------

/// H1: a module that holds no grant cannot build a descriptor-carrying handle
/// from a number. `io.File(fd = n)` was a constructor any module could call, so
/// a dependency with no authority at all could read, write or close every
/// descriptor another module had opened (a secret file, a socket). The refusal
/// is at compile time, and names the capability that would be needed.
#[test]
fn h01_a_module_without_a_grant_cannot_forge_a_descriptor_handle() {
    for (what, imports, body, needle) in [
        (
            "file_scan",
            &["std/io", "std/str"][..],
            "    var i: i32 = 3\n    while i < 40 {\n        val f = io.File(fd = i)\n        if f.is_open() {\n            print(String.from_bytes(f.read(16)))\n        }\n        i = i + 1\n    }",
            "wraps an operating-system descriptor",
        ),
        (
            "socket",
            &["std/net"][..],
            "    val s = net.Socket(fd = 5)\n    net.send_text(s, \"x\")",
            "needs the 'net' capability, which module 'util' was not granted",
        ),
        (
            "listener",
            &["std/net"][..],
            "    val l = net.Listener(fd = 5)\n    print(l.port())",
            "needs the 'net' capability, which module 'util' was not granted",
        ),
        (
            "child",
            &["std/process", "std/io"][..],
            "    val c = process.Child(pid = 1, stdin = io.File(fd = 0), stdout = io.File(fd = 1), stderr = io.File(fd = 2), exited = false, code = 0)\n    c.kill(9)",
            "wraps an operating-system descriptor",
        ),
    ] {
        let lab = Lab::new(&format!("h01_{what}"));
        // The secret is opened (and, as programs do, not closed) by a module
        // that was granted it; the module under test has no grant.
        lab.src(
            "main.kp",
            "impl \"opener\" caps(fs = [\"__T__/secret/**\"])\nimpl \"util\"\n\nfun main() {\n    opener.open_and_leak()\n    util.go()\n}\n",
        );
        lab.src(
            "opener.kp",
            "impl \"std/fs\"\n\npub fun open_and_leak() {\n    val f = fs.open(\"__T__/secret/s.txt\", \"r\")\n}\n",
        );
        let mut util = String::new();
        for i in imports {
            util.push_str(&format!("impl \"{i}\"\n"));
        }
        util.push_str(&format!("\npub fun go() {{\n{body}\n}}\n"));
        lab.src("util.kp", &util);
        lab.refused(&[needle]);
    }
}

/// H2: holding one domain does not let a module forge a handle of another's:
/// a module with only `fs` cannot build a socket or a child.
#[test]
fn h02_a_domain_mints_only_its_own_handles() {
    for (grant, imports, body) in [
        ("fs = [\"/**\"]", &["std/fs", "std/net"][..], "    val s = net.Socket(fd = 5)"),
        (
            "net = [\"*\"]",
            &["std/net", "std/process", "std/io"][..],
            "    val c = process.Child(pid = 1, stdin = io.File(fd = 0), stdout = io.File(fd = 1), stderr = io.File(fd = 2), exited = false, code = 0)",
        ),
    ] {
        let lab = Lab::new("h02");
        lab.simple(grant, imports, body);
        lab.refused(&["needs the"]);
    }
}

/// H3 (known gap): two modules that each hold a domain can still forge
/// between each other's descriptors. `io.File(fd = n)` is open to any module
/// that holds `fs`, `net` or `process`, so a module granted only `data/**` can
/// read a descriptor another module opened on `secret/`. Closing it needs
/// opaque handle types (no integer constructor outside the library) and an
/// explicit, gated `from_fd`; see `plan/threat-model.md`, Known gaps.
#[test]
#[ignore = "known gap: descriptors are small integers, forgeable between two modules that both hold a domain (plan/threat-model.md H3)"]
fn h03_a_granted_module_can_read_a_descriptor_another_module_opened() {
    let lab = Lab::new("h03");
    lab.src(
        "main.kp",
        "impl \"opener\" caps(fs = [\"__T__/secret/**\"])\nimpl \"util\" caps(fs = [\"__T__/data/**\"])\n\nfun main() {\n    opener.open_and_leak()\n    util.go()\n}\n",
    );
    lab.src(
        "opener.kp",
        "impl \"std/fs\"\n\npub fun open_and_leak() {\n    val f = fs.open(\"__T__/secret/s.txt\", \"r\")\n}\n",
    );
    lab.src(
        "util.kp",
        "impl \"std/fs\"\nimpl \"std/io\"\n\npub fun go() {\n    var i: i32 = 3\n    while i < 40 {\n        val f = io.File(fd = i)\n        if f.is_open() {\n            print(String.from_bytes(f.read(16)))\n        }\n        i = i + 1\n    }\n}\n",
    );
    // The attack is expected to be refused; today it leaks.
    let b = lab.build(false);
    if b.status.success() {
        let out = lab.run(false, &[]);
        assert!(
            !String::from_utf8_lossy(&out.stdout).contains(TOKEN),
            "the secret leaked through a forged descriptor"
        );
    }
}
