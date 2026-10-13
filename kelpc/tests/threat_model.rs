//! The capability threat model's attack tests (`reference/capability-threat-model.md` (kelp wiki)).
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

    fn build(&self) -> Output {
        let mut cmd = kelpc();
        cmd.arg("build").env_remove("KELPC_BACKEND");
        let entry = if self.root.join("coral.toml").exists() {
            self.root.clone()
        } else {
            self.root.join("main.kp")
        };
        cmd.arg(entry)
            .arg("-o")
            .arg(self.root.join("prog"))
            .current_dir(&self.root)
            .output()
            .expect("spawn kelpc")
    }

    fn run(&self, args: &[&str]) -> Output {
        let prog = "prog";
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
        let out = self.build();
        let text = String::from_utf8_lossy(&out.stderr).to_string();
        assert!(!out.status.success(), "expected a compile refusal, but it built");
        for n in needles {
            assert!(text.contains(n), "refusal lacks '{n}':\n{text}");
        }
        self.assert_untouched();
    }

    /// The program compiles and every run fails with a
    /// message holding the needle; nothing leaked.
    fn denied(&self, needle: &str) {
        let b = self.build();
        assert!(
            b.status.success(),
            "expected the build to succeed:\n{}",
            String::from_utf8_lossy(&b.stderr)
        );
        let out = self.run(&[]);
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        assert!(!stdout.contains(TOKEN), "the secret leaked: {stdout}");
        assert!(
            !out.status.success() && stderr.contains(needle),
            "expected a denial containing '{needle}', got status {:?}\nstdout:{stdout}\nstderr:{stderr}",
            out.status.code()
        );
        self.assert_untouched();
    }

    /// The program compiles and runs; the output is
    /// returned (per backend) for the test to check.
    fn runs(&self, args: &[&str]) -> Vec<Output> {
        let mut all = Vec::new();
        let b = self.build();
        assert!(
            b.status.success(),
            "expected the build to succeed:\n{}",
            String::from_utf8_lossy(&b.stderr)
        );
        all.push(self.run(args));
        all
    }

    /// The program runs and prints exactly `want`.
    fn prints(&self, want: &str) {
        for out in self.runs(&[]) {
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            assert!(!stdout.contains(TOKEN), "the secret leaked: {stdout}");
            assert!(
                out.status.success() && stdout == want,
                "expected '{want}', got status {:?}\nstdout:{stdout}\nstderr:{stderr}",
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

    /// A project with a manifest: `coral.toml` is `manifest`, and each of
    /// `files` goes under `src/`.
    fn manifest_project(&self, manifest: &str, files: &[(&str, &str)]) {
        fs::write(self.root.join("coral.toml"), manifest.replace("__T__", &self.root.to_string_lossy()))
            .expect("write the manifest");
        fs::create_dir_all(self.root.join("src")).unwrap();
        for (file, text) in files {
            let text = text.replace("__T__", &self.root.to_string_lossy());
            fs::write(self.root.join("src").join(file), text).expect("write a source file");
        }
    }

    /// `kelpc check` over the project directory: the front end only.
    fn check(&self) -> Output {
        kelpc().arg("check").arg(&self.root).output().expect("spawn kelpc")
    }

    fn check_refused(&self, needles: &[&str]) {
        let out = self.check();
        let text = String::from_utf8_lossy(&out.stderr).to_string();
        assert!(!out.status.success(), "expected a refusal, but the project checks");
        for n in needles {
            assert!(text.contains(n), "refusal lacks '{n}':\n{text}");
        }
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
        let out = lab.build();
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
    for out in lab.runs(&[]) {
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
    let b = lab.build();
    assert!(b.status.success(), "{}", String::from_utf8_lossy(&b.stderr));
    let out = lab.run(&[&pid]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success() && !stdout.contains("signalled") && stderr.contains("not a child"),
        "kill reached a process this program did not start: {stdout}{stderr}"
    );
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
        "    val args = process.args()\n    val pid = result.expect(r = str.parse_u64(args.get(1).as_str()), message = \"pid\")\n    val c = process.Child(pid = pid.to_i32(), stdin = io.File(fd = -1), stdout = io.File(fd = -1), stderr = io.File(fd = -1), exited = false, code = 0, signal = 0)\n    c.kill(0)\n    print(\"signalled a process that is not a child\")",
    );
    let pid = std::process::id().to_string();
    let b = lab.build();
    assert!(b.status.success(), "{}", String::from_utf8_lossy(&b.stderr));
    let out = lab.run(&[&pid]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success() && !stdout.contains("signalled") && stderr.contains("not a child"),
        "a forged handle signalled a foreign pid: {stdout}{stderr}"
    );
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
        for out in lab.runs(&[]) {
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
            let port: u16 = stdout.lines().next().unwrap().trim().parse().unwrap();
            let line = listener_line(&stdout, port)
                .unwrap_or_else(|| panic!("no LISTEN line for port {port}:\n{stdout}"));
            assert_eq!(
                line.split(':').next().unwrap(),
                want,
                "grant '{grant}' bound {line}"
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
        "impl \"std/fs\"\nimpl \"std/io\"\n\npub fun table() => String {\n    val f = fs.open(\"/proc/net/tcp\", \"r\")\n    return String.from_bytes(f.read_all())\n}\n",
    );
    for out in lab.runs(&[]) {
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let port: u16 = stdout.lines().next().unwrap().trim().parse().unwrap();
        let line = listener_line(&stdout, port).expect("a LISTEN line");
        assert_eq!(line.split(':').next().unwrap(), "0100007F", "{line}");
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

/// N6b (open item 55): `net.listen_on(host, port)` checks the host as well as
/// the port, so a grant for `127.0.0.1:8080` refuses the wildcard address and
/// every other host and port, where `net.listen(8080)` only compared the port.
#[test]
fn n06b_listen_on_checks_the_host_against_the_grant() {
    for (host, port) in [("0.0.0.0", 8080u16), ("*", 8080), ("127.0.0.2", 8080), ("127.0.0.1", 8081)] {
        let lab = Lab::new("n06b");
        lab.simple(
            "net = [\"127.0.0.1:8080\"]",
            NET,
            &format!("    val l = net.listen_on(\"{host}\", {port})\n    print(\"bound\")\n    l.close()"),
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
        let b = lab.build();
        assert!(b.status.success(), "{}", String::from_utf8_lossy(&b.stderr));
        let out = lab.run(&[&url]);
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

/// N9: TLS trust roots never come from a path the runtime opens. The old
/// `ca_file`/`ca_dir` options let a module with only a `net` grant make the
/// handshake open any readable path (and trust whatever it held). They are gone:
/// the options take PEM bytes (`with_ca_pem`), and the only way to turn a path
/// into bytes is `std/tlsfile`, which needs an `fs` grant.
#[test]
fn n09_a_net_only_module_cannot_name_a_ca_path() {
    for body in [
        "    val o = tls.client_options().with_ca_file(\"__T__/secret/s.txt\")",
        "    val o = tls.TlsOptions(ca_file = String.from_str(\"__T__/secret/s.txt\"), ca_dir = String.empty(), timeout_ms = 0)",
        "    val o = tls.TlsOptions(ca_pem = Bytes.empty(), ca_dir = String.from_str(\"__T__/secret\"), timeout_ms = 0)",
    ] {
        let lab = Lab::new("n09a");
        lab.simple("net = [\"127.0.0.1:9\"]", &["std/net", "std/tls"], body);
        let out = lab.build();
        assert!(!out.status.success(), "a CA path option compiled: {body}");
        lab.assert_untouched();
    }
}

/// N9b: reading a CA file is an `fs` read: a module with a `net` grant and no
/// `fs` grant cannot use `std/tlsfile`, and the refusal names `fs`.
#[test]
fn n09b_reading_a_ca_file_needs_the_fs_capability() {
    let lab = Lab::new("n09b");
    lab.simple(
        "net = [\"127.0.0.1:9\"]",
        &["std/net", "std/tls", "std/tlsfile"],
        "    when tlsfile.ca_pem(\"__T__/secret/s.txt\") {\n        Ok(_) -> print(\"read\")\n        Err(_) -> print(\"denied\")\n    }",
    );
    lab.refused(&["needs the 'fs' capability"]);
}

/// N9c: with an `fs` grant the CA path is checked by the `fs` matcher (same
/// normalisation and link rules as `fs.read`): a path outside the grant,
/// existing, missing or reached through `..`, is the standard `fs` denial and
/// the file is never opened; one inside is read.
#[test]
fn n09c_a_ca_path_outside_the_fs_grant_is_denied_like_fs_read() {
    for path in ["__T__/secret/s.txt", "__T__/secret/missing.pem", "__T__/data/../secret/s.txt"] {
        let lab = Lab::new("n09c");
        let grant = format!("fs = [\"{}/data/**\"]", lab.root.display());
        lab.src("main.kp", &format!("impl \"util\" caps({grant})\n\nfun main() {{\n    util.go()\n}}\n"));
        lab.src(
            "util.kp",
            &format!("impl \"std/tls\"\nimpl \"std/tlsfile\" caps({grant})\n\npub fun go() {{\n    when tlsfile.ca_pem(\"{path}\") {{\n        Ok(b) -> print(b.len())\n        Err(e) -> print(\"typed error\")\n    }}\n}}\n"),
        );
        lab.denied("is outside the fs capability");
    }
    let lab = Lab::new("n09c_in");
    let grant = format!("fs = [\"{}/data/**\"]", lab.root.display());
    lab.src("main.kp", &format!("impl \"util\" caps({grant})\n\nfun main() {{\n    util.go()\n}}\n"));
    lab.src(
        "util.kp",
        &format!("impl \"std/tls\"\nimpl \"std/tlsfile\" caps({grant})\n\npub fun go() {{\n    when tlsfile.ca_pem(\"__T__/data/ok.txt\") {{\n        Ok(b) -> print(b.len())\n        Err(e) -> print(\"typed error\")\n    }}\n}}\n"),
    );
    lab.prints("5\n");
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
            "    val c = process.Child(pid = 1, stdin = io.File(fd = 0), stdout = io.File(fd = 1), stderr = io.File(fd = 2), exited = false, code = 0, signal = 0)\n    c.kill(9)",
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
            "    val c = process.Child(pid = 1, stdin = io.File(fd = 0), stdout = io.File(fd = 1), stderr = io.File(fd = 2), exited = false, code = 0, signal = 0)",
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
/// explicit, gated `from_fd`; see `reference/capability-threat-model.md` (kelp wiki), Known gaps.
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
    let b = lab.build();
    if b.status.success() {
        let out = lab.run(&[]);
        assert!(
            !String::from_utf8_lossy(&out.stdout).contains(TOKEN),
            "the secret leaked through a forged descriptor"
        );
    }
}

// ---------------------------------------------------------------------------
// unsafe audit list
// ---------------------------------------------------------------------------

const UNSAFE_MANIFEST: &str = "name = \"u\"\n\n[unsafe]\nmodules = [\"main\", \"listed\"]\n";
const NOT_LISTED: &str = "is not listed in coral.toml [unsafe] modules";

/// Builds a project whose `evil` module (not on the audit list) holds `evil`,
/// and asserts the build is refused with `needle`.
fn unsafe_attack(name: &str, evil: &str, needle: &str) {
    let lab = Lab::new(name);
    lab.manifest_project(
        UNSAFE_MANIFEST,
        &[
            ("main.kp", "impl \"evil\"\n\nfun main() {\n    evil.go()\n}\n"),
            ("evil.kp", evil),
        ],
    );
    lab.refused(&[needle]);
}

/// U1: an `unsafe` block or an `unsafe fun` in a module that is not listed.
#[test]
fn u01_unsafe_in_an_unlisted_module_is_refused() {
    unsafe_attack(
        "u01a",
        "pub fun go() {\n    var x: i32 = 1\n    val p: Ptr<i32> = Ptr.from_mut(x)\n    print(unsafe { p.read() })\n}\n",
        NOT_LISTED,
    );
    unsafe_attack(
        "u01b",
        "unsafe fun poke(p: Ptr<i32>) => i32 = p.read()\n\npub fun go() {\n}\n",
        NOT_LISTED,
    );
}

/// U2: the same operation hidden in a position the audit walk has to reach:
/// a closure, a generic function, a `when` arm, an `if`/`else` branch, a
/// template, a call argument, a tuple, an array, a loop condition, a `for`
/// iterable, a `defer`.
#[test]
fn u02_unsafe_hidden_in_an_expression_position_is_still_found() {
    let head = "pub fun go() {\n    var x: i32 = 1\n    val p: Ptr<i32> = Ptr.from_mut(x)\n";
    for (what, body) in [
        ("closure", "    val f = fun() => i32 = unsafe { p.read() }\n    print(f())"),
        ("if_cond", "    if unsafe { p.read() } == 1 {\n        print(1)\n    }"),
        ("else_branch", "    if x == 2 {\n        print(0)\n    } else {\n        print(unsafe { p.read() })\n    }"),
        ("when_arm", "    val o: i32 = 1\n    val v = when o {\n        _ -> unsafe { p.read() }\n    }"),
        ("template", "    print(\"v={unsafe { p.read() }}\")"),
        ("call_arg", "    print(abs_of(unsafe { p.read() }))"),
        ("tuple", "    val t = (1, unsafe { p.read() })"),
        ("array", "    val a = [1, unsafe { p.read() }]"),
        ("while_cond", "    while unsafe { p.read() } == 0 {\n        print(1)\n    }"),
        ("for_iterable", "    for i in unsafe { make_list(p) } {\n        print(i)\n    }"),
        ("defer", "    defer print(unsafe { p.read() })\n    print(2)"),
        ("assign", "    var y: i32 = 0\n    y = unsafe { p.read() }"),
        ("return", "    print(helper(p))"),
    ] {
        let evil = format!(
            "fun abs_of(n: i32) => i32 = n\nfun make_list(p: Ptr<i32>) => List<i32> {{\n    var l: List<i32> = List.empty()\n    return l\n}}\nfun helper(p: Ptr<i32>) => i32 {{\n    return unsafe {{ p.read() }}\n}}\n{head}{body}\n}}\n"
        );
        unsafe_attack(&format!("u02_{what}"), &evil, NOT_LISTED);
    }
}

/// U3: a trait's default method lives in the declaration and not in the
/// module's function list, and the audit walk used to skip it, so a module
/// outside the list could keep an `unsafe` body that every impl inherited.
#[test]
fn u03_unsafe_in_a_trait_default_method_is_found() {
    unsafe_attack(
        "u03",
        "pub trait Poke {\n    fun Self.addr() => usize\n    fun Self.poke() => i32 {\n        val p: Ptr<i32> = Ptr.from_addr(self.addr())\n        return unsafe { p.read() }\n    }\n}\n\npub datac A(val a: usize)\n\nimpl Poke for A {\n    fun Self.addr() => usize = a\n}\n\npub fun go() {\n    var x: i32 = 7\n    val p: Ptr<i32> = Ptr.from_mut(x)\n    val a = A(a = p.addr())\n    print(a.poke())\n}\n",
        NOT_LISTED,
    );
}

/// U4: an impl method and a generic function with an `unsafe` body.
#[test]
fn u04_unsafe_in_an_impl_method_or_a_generic_is_found() {
    unsafe_attack(
        "u04a",
        "pub trait Get {\n    fun Self.get() => i32\n}\n\npub datac A(val p: Ptr<i32>)\n\nimpl Get for A {\n    fun Self.get() => i32 = unsafe { p.read() }\n}\n\npub fun go() {\n}\n",
        NOT_LISTED,
    );
    unsafe_attack(
        "u04b",
        "fun peek<T>(p: Ptr<i32>, x: T) => i32 = unsafe { p.read() }\n\npub fun go() {\n    var x: i32 = 1\n    print(peek(Ptr.from_mut(x), 1))\n}\n",
        NOT_LISTED,
    );
}

/// U5: `asm` and `naked fun` are unsafe operations: an unlisted module cannot
/// contain either.
#[test]
fn u05_asm_and_naked_fun_need_the_audit_list() {
    unsafe_attack(
        "u05a",
        "pub fun go() {\n    unsafe { asm(\"nop\") }\n}\n",
        NOT_LISTED,
    );
    unsafe_attack(
        "u05b",
        "naked fun forty_two() => i64 {\n    asm(\"mov rax, 42\\nret\")\n}\n\npub fun go() {\n    print(forty_two())\n}\n",
        NOT_LISTED,
    );
    unsafe_attack(
        "u05c",
        "pub fun go() {\n    asm(\"nop\")\n}\n",
        NOT_LISTED,
    );
}

/// U6: an `unsafe` initializer of a `const`, `static` or `pd` is found too.
#[test]
fn u06_unsafe_in_a_module_level_initializer_is_found() {
    unsafe_attack(
        "u06a",
        "static mut N: i32 = 0\n\npub fun go() {\n    unsafe { N = 1 }\n}\n",
        NOT_LISTED,
    );
    unsafe_attack(
        "u06b",
        "pd val V: i32 = unsafe { read_it() }\n\nunsafe fun read_it() => i32 = 1\n\npub fun go() {\n    print(V)\n}\n",
        NOT_LISTED,
    );
}

/// U7: an unlisted module cannot call a listed module's `unsafe fun`, with or
/// without an `unsafe` block of its own, and a `static mut` it does not own is
/// out of reach the same way. The listed module's *safe* wrapper is the audit
/// design working: it is callable and does its job (the positive control).
#[test]
fn u07_an_unlisted_module_reaches_unsafe_only_through_listed_wrappers() {
    let listed = "pub unsafe fun raw(p: Ptr<i32>) => i32 = p.read()\n\npub fun safe_read(x: i32) => i32 {\n    var y: i32 = x\n    val p: Ptr<i32> = Ptr.from_mut(y)\n    return unsafe { raw(p) }\n}\n";
    for (what, evil, needle) in [
        (
            "no_block",
            "impl \"listed\"\n\npub fun go() {\n    var x: i32 = 1\n    print(listed.raw(Ptr.from_mut(x)))\n}\n",
            "is an 'unsafe fun'",
        ),
        (
            "with_block",
            "impl \"listed\"\n\npub fun go() {\n    var x: i32 = 1\n    print(unsafe { listed.raw(Ptr.from_mut(x)) })\n}\n",
            NOT_LISTED,
        ),
    ] {
        let lab = Lab::new(&format!("u07_{what}"));
        lab.manifest_project(
            UNSAFE_MANIFEST,
            &[
                ("main.kp", "impl \"evil\"\n\nfun main() {\n    evil.go()\n}\n"),
                ("listed.kp", listed),
                ("evil.kp", evil),
            ],
        );
        lab.refused(&[needle]);
    }
    let lab = Lab::new("u07_wrapper");
    lab.manifest_project(
        UNSAFE_MANIFEST,
        &[
            ("main.kp", "impl \"evil\"\n\nfun main() {\n    evil.go()\n}\n"),
            ("listed.kp", listed),
            ("evil.kp", "impl \"listed\"\n\npub fun go() {\n    print(listed.safe_read(41))\n}\n"),
        ],
    );
    lab.prints("41\n");
}

// ---------------------------------------------------------------------------
// mmio (freestanding)
// ---------------------------------------------------------------------------

const MMIO_LINKER: &str = "ENTRY(_start)\nSECTIONS\n{\n    . = 0x80000000;\n    .text : { *(.text.boot) *(.text .text.*) }\n}\n";

fn mmio_lab(name: &str, caps: &str, files: &[(&str, &str)]) -> Lab {
    let lab = Lab::new(name);
    lab.manifest_project(
        &format!(
            "name = \"mmio_attack\"\n\n[target]\nmode = \"freestanding\"\ntriple = \"riscv64-unknown-none-elf\"\nentry = \"kmain\"\nlinker_script = \"kernel.ld\"\n\n[unsafe]\nmodules = [\"main\", \"dev\"]\n\n{caps}"
        ),
        files,
    );
    fs::write(lab.root.join("kernel.ld"), MMIO_LINKER).unwrap();
    lab
}

fn poke_at(expr: &str) -> String {
    format!(
        "pub fun poke() {{\n    val p: Ptr<u8> = unsafe {{ Ptr.from_addr({expr}) }}\n    unsafe {{ p.write_volatile(65) }}\n}}\n"
    )
}

const KMAIN_DEV: &str = "impl \"dev\" caps(mmio = [\"0x10000000+0x100\"])\n\nsection(\".text.boot\") fun kmain() {\n    dev.poke()\n    while true { unsafe { asm(\"wfi\") } }\n}\n";

/// M1: a driver module with no `mmio` grant cannot make a pointer from an
/// address, constant or not.
#[test]
fn m01_a_module_without_an_mmio_grant_cannot_take_an_address() {
    let lab = mmio_lab(
        "m01",
        "[caps]\nmmio = [\"0x10000000+0x100\"]\n",
        &[
            (
                "main.kp",
                "impl \"dev\"\n\nsection(\".text.boot\") fun kmain() {\n    dev.poke()\n    while true { unsafe { asm(\"wfi\") } }\n}\n",
            ),
            ("dev.kp", &poke_at("0x10000000")),
        ],
    );
    lab.check_refused(&["needs an 'mmio' grant", "module 'dev' holds none"]);
}

/// M2: constant folding must mean what the emitted code computes. A shift by
/// 64 or more folds to 0 in the compiler's 128-bit arithmetic but is a shift
/// by `n mod 64` on the target, so `0x10000000 + (0x100000 << 70)` looked like
/// the in-range `0x10000000` and addressed `0x14000000`. A shift that wide is
/// now refused where it is written (a constant shift by the width or more is a
/// compile error; before tl7 it was "cannot fold", which a range grant refused).
#[test]
fn m02_an_oversized_shift_does_not_fold_into_the_granted_range() {
    for expr in [
        "0x10000000 + (0x100000 << 70)",
        "0x10000000 + (0x100000 << 64)",
        "0x10000000 | (0x4000000 << 128)",
        "0x10000000 + (0x1 >> 70)",
    ] {
        let lab = mmio_lab(
            "m02",
            "[caps]\nmmio = [\"0x10000000+0x100\"]\n",
            &[("main.kp", KMAIN_DEV), ("dev.kp", &poke_at(expr))],
        );
        lab.check_refused(&["a shift by", "is not defined for a 'usize'"]);
    }
}

/// M3: an address computed from another pointer, a parameter or a runtime
/// value is not a constant, so only the whole-address-space grant admits it.
#[test]
fn m03_a_computed_address_is_refused_under_a_range_grant() {
    let dev = "pub fun poke(base: usize) {\n    val p: Ptr<u8> = unsafe { Ptr.from_addr(base + 4) }\n    unsafe { p.write_volatile(65) }\n}\n";
    let main = "impl \"dev\" caps(mmio = [\"0x10000000+0x100\"])\n\nsection(\".text.boot\") fun kmain() {\n    dev.poke(0x10000000)\n    while true { unsafe { asm(\"wfi\") } }\n}\n";
    let lab = mmio_lab("m03", "[caps]\nmmio = [\"0x10000000+0x100\"]\n", &[("main.kp", main), ("dev.kp", dev)]);
    lab.check_refused(&["cannot fold"]);
}

/// M4: a module cannot delegate a bigger window than it holds, nor the whole
/// address space from a range.
#[test]
fn m04_a_range_cannot_be_widened_by_delegation() {
    for wider in ["0x10000000+0x1000", "0x0F000000+0x100", "*"] {
        let main = format!(
            "impl \"mid\" caps(mmio = [\"0x10000000+0x100\"])\n\nsection(\".text.boot\") fun kmain() {{\n    mid.go()\n    while true {{ unsafe {{ asm(\"wfi\") }} }}\n}}\n"
        );
        let mid = format!("impl \"dev\" caps(mmio = [\"{wider}\"])\n\npub fun go() {{\n    dev.poke()\n}}\n");
        let lab = mmio_lab(
            "m04",
            "[caps]\nmmio = [\"0x10000000+0x1000\"]\n",
            &[("main.kp", &main), ("mid.kp", &mid), ("dev.kp", &poke_at("0x10000000"))],
        );
        lab.check_refused(&["cannot grant"]);
    }
}

/// M5 (known gap): the range is checked where a pointer is *born*. Arithmetic
/// on the pointer afterwards reaches any address, so a granted window is not a
/// fence. The design is provenance (roadmap D-1): a pointer born from a range
/// carries it, and `offset`/`byte_offset`/`cast` keep it. The honest record is
/// in `language/capabilities.md` (kelp wiki) ("Born-at only"); this is its reproduction.
#[test]
#[ignore = "known gap: mmio ranges are checked at the pointer's birth, not after offset arithmetic (plan/device-caps.md, plan/threat-model.md M5)"]
fn m05_offset_arithmetic_leaves_the_granted_window() {
    let dev = "pub fun poke() {\n    val base: Ptr<u8> = unsafe { Ptr.from_addr(0x10000000) }\n    val far: Ptr<u8> = base.byte_offset(0x10000000)\n    unsafe { far.write_volatile(65) }\n}\n";
    let lab = mmio_lab(
        "m05",
        "[caps]\nmmio = [\"0x10000000+0x100\"]\n",
        &[("main.kp", KMAIN_DEV), ("dev.kp", dev)],
    );
    lab.check_refused(&["outside"]);
}

// ---------------------------------------------------------------------------
// indirect use
// ---------------------------------------------------------------------------

const NEEDS_FS: &str = "needs the 'fs' capability, which module 'evil' was not granted";

/// Runs a three-module project: `main` grants `util` the `data/**` window and
/// leaves `evil` ungranted; `evil` is the attacker.
fn indirect(name: &str, util: &str, evil: &str) -> Lab {
    let lab = Lab::new(name);
    lab.src(
        "main.kp",
        "impl \"util\" caps(fs = [\"__T__/data/**\"])\nimpl \"evil\"\n\nfun main() {\n    evil.go()\n}\n",
    );
    lab.src("util.kp", util);
    lab.src("evil.kp", evil);
    lab
}

/// I1: a function value or closure made by a granted module and handed to an
/// ungranted one is still checked against the *maker's* grant when called.
#[test]
fn i01_a_closure_from_a_granted_module_keeps_its_makers_grant() {
    let lab = Lab::new("i01");
    lab.src(
        "main.kp",
        "impl \"util\" caps(fs = [\"__T__/data/**\"])\nimpl \"evil\"\n\nfun main() {\n    evil.go(util.reader())\n}\n",
    );
    lab.src(
        "util.kp",
        "impl \"std/fs\"\n\npub fun reader() => fun(&str) => String = fun(p: &str) => String = fs.read_text(p)\n",
    );
    lab.src("evil.kp", "pub fun go(f: fun(&str) => String) {\n    print(f(\"__T__/secret/s.txt\"))\n}\n");
    lab.denied("granted to module 'util'");
}

/// I2: a generic function in a granted module is checked against that module's
/// grant whatever type the caller instantiates it with; a generic function in
/// an ungranted module cannot name `fs` at all.
#[test]
fn i02_generics_do_not_change_whose_grant_applies() {
    let lab = indirect(
        "i02a",
        "impl \"std/fs\"\n\npub fun get<T>(p: &str, x: T) => String = fs.read_text(p)\n",
        "impl \"util\"\n\npub fun go() {\n    print(util.get(\"__T__/secret/s.txt\", 1))\n}\n",
    );
    lab.denied("granted to module 'util'");
    let lab = indirect(
        "i02b",
        "pub fun x() {\n}\n",
        "impl \"std/fs\"\n\nfun get<T>(p: &str, x: T) => String = fs.read_text(p)\n\npub fun go() {\n    print(get(\"__T__/secret/s.txt\", 1))\n}\n",
    );
    lab.refused(&[NEEDS_FS]);
}

/// I3: an ungranted module cannot reach `fs` through a trait impl, a trait
/// default body supplied by a granted module, a `Drop` impl, a `defer`, a
/// scoped block or a module-level initializer: each is code of the module that
/// writes it (or of the impl's module), and is checked as such.
#[test]
fn i03_traits_drop_defer_scoped_and_initializers_do_not_launder_a_grant() {
    let util = "impl \"std/fs\"\n\npub trait Loader {\n    fun Self.path() => String\n    fun Self.load() => String {\n        return fs.read_text(self.path().as_str())\n    }\n}\n\npub fun with(cb: scoped fun() => i32) => i32 = cb()\n";
    for (what, evil) in [
        (
            "trait_default",
            "impl \"util\"\n\npub datac L(val p: String)\n\nimpl util.Loader for L {\n    fun Self.path() => String = String.from_str(p.as_str())\n}\n\npub fun go() {\n    val l = L(p = String.from_str(\"__T__/secret/s.txt\"))\n    print(l.load())\n}\n",
        ),
        (
            "trait_impl",
            "impl \"std/fs\"\nimpl \"util\"\n\npub datac L(val p: String)\n\nimpl util.Loader for L {\n    fun Self.path() => String = fs.read_text(\"__T__/secret/s.txt\")\n}\n\npub fun go() {\n}\n",
        ),
        (
            "drop",
            "impl \"std/fs\"\n\ndatac Guard(val n: i32)\n\nfun mut Guard.drop() {\n    print(fs.read_text(\"__T__/secret/s.txt\"))\n}\n\npub fun go() {\n    val g = Guard(n = 1)\n}\n",
        ),
        (
            "defer",
            "impl \"std/fs\"\n\npub fun go() {\n    defer print(fs.read_text(\"__T__/secret/s.txt\"))\n    print(1)\n}\n",
        ),
        (
            "scoped",
            "impl \"util\"\nimpl \"std/fs\"\n\npub fun go() {\n    print(util.with(fun() => i32 = fs.read_text(\"__T__/secret/s.txt\").len()))\n}\n",
        ),
        (
            "pd_initializer",
            "impl \"std/fs\"\n\npd val S: String = fs.read_text(\"__T__/secret/s.txt\")\n\npub fun go() {\n    print(S.len())\n}\n",
        ),
    ] {
        let lab = indirect(&format!("i03_{what}"), util, evil);
        lab.refused(&["capability"]);
    }
}

/// I4: re-export tricks: an ungranted module cannot reach `fs` through the
/// import of a module that holds it (`util.fs.read_text`), and cannot delegate
/// itself a grant on its own `impl` line.
#[test]
fn i04_importing_a_granted_module_does_not_import_its_grant() {
    let lab = indirect(
        "i04a",
        "impl \"std/fs\"\n\npub fun x() {\n}\n",
        "impl \"util\"\n\npub fun go() {\n    print(util.fs.read_text(\"__T__/secret/s.txt\"))\n}\n",
    );
    lab.refused(&["util"]);
    let lab = indirect(
        "i04b",
        "pub fun x() {\n}\n",
        "impl \"std/fs\" caps(fs = [\"/**\"])\n\npub fun go() {\n    print(fs.read_text(\"__T__/secret/s.txt\"))\n}\n",
    );
    lab.refused(&["cannot grant"]);
}

/// I5: a user `extern fun` has no body unless the manifest binds it, so an
/// ungranted module cannot declare the library's own raw primitive or libc's
/// `system` and call it.
#[test]
fn i05_a_user_extern_needs_a_manifest_entry() {
    for decl in [
        "extern fun read_raw(path: &str) => Bytes",
        "extern fun system(cmd: &str) => i32",
    ] {
        let lab = indirect(
            "i05",
            "pub fun x() {\n}\n",
            &format!("{decl}\n\npub fun go() {{\n}}\n"),
        );
        lab.refused(&["has no compiler-supplied body"]);
    }
}

// ---------------------------------------------------------------------------
// delegation and the manifest
// ---------------------------------------------------------------------------

/// D1: a module cannot hand out more than it holds: a wider pattern, a parent
/// directory, the whole-resource wildcard, in any domain.
#[test]
fn d01_a_module_cannot_delegate_more_than_it_holds() {
    for (grant_line, domain_import) in [
        ("fs = [\"/**\"]", "std/fs"),
        ("fs = [\"__T__/**\"]", "std/fs"),
        ("fs = [\"__T__/data/../**\"]", "std/fs"),
        ("net = [\"*\"]", "std/net"),
        ("env = [\"*\"]", "std/env"),
        ("process = [\"/**\"]", "std/process"),
    ] {
        let lab = Lab::new("d01");
        lab.src(
            "main.kp",
            "impl \"util\" caps(fs = [\"__T__/data/**\"], net = [\"127.0.0.1:1\"], env = [\"A\"], process = [\"/bin/true\"])\n\nfun main() {\n    util.go()\n}\n",
        );
        lab.src(
            "util.kp",
            &format!("impl \"{domain_import}\" caps({grant_line})\n\npub fun go() {{\n}}\n"),
        );
        lab.refused(&[]);
    }
}

/// D2: `caps(*)` is the whole resource and only a module that holds the whole
/// resource may hand it over: from a narrow importer it is refused.
#[test]
fn d02_the_wildcard_delegation_is_bounded_by_the_importer() {
    let lab = Lab::new("d02");
    lab.src(
        "main.kp",
        "impl \"mid\" caps(fs = [\"__T__/data/**\"])\n\nfun main() {\n    mid.go()\n}\n",
    );
    lab.src("mid.kp", "impl \"leaf\" caps(*)\n\npub fun go() {\n    leaf.go()\n}\n");
    lab.src(
        "leaf.kp",
        "impl \"std/fs\"\n\npub fun go() {\n    print(fs.read_text(\"__T__/secret/s.txt\"))\n}\n",
    );
    lab.refused(&["whole-resource wildcard"]);
}

/// D3: the manifest's `[caps]` narrows the entry module itself: it cannot read
/// outside its own window, and cannot grant an import more than the manifest
/// gave it.
#[test]
fn d03_the_manifest_narrows_the_entry_module() {
    let lab = Lab::new("d03a");
    lab.manifest_project(
        "name = \"d\"\n\n[caps]\nfs = [\"__T__/data/**\"]\n",
        &[(
            "main.kp",
            "impl \"std/fs\"\n\nfun main() {\n    print(fs.read_text(\"__T__/secret/s.txt\"))\n}\n",
        )],
    );
    lab.denied("granted to module 'main'");
    let lab = Lab::new("d03b");
    lab.manifest_project(
        "name = \"d\"\n\n[caps]\nfs = [\"__T__/data/**\"]\n",
        &[
            ("main.kp", "impl \"util\" caps(fs = [\"__T__/**\"])\n\nfun main() {\n    util.go()\n}\n"),
            ("util.kp", "impl \"std/fs\"\n\npub fun go() {\n    print(fs.read_text(\"__T__/secret/s.txt\"))\n}\n"),
        ],
    );
    lab.refused(&["cannot grant"]);
}

/// D4: module ids cannot escape `src/` or shadow the library: `..`, absolute
/// and backslash paths are refused, and a project file named like a shipped
/// module (`src/std/rt.kp`, whose `glob_match` says yes to everything) is
/// never read in place of the shipped one, so the matcher cannot be replaced.
#[test]
fn d04_module_ids_cannot_escape_or_shadow_the_library() {
    for id in ["../outside", "/etc/passwd", "a\\\\b", "std/../std/fs", "util/../util"] {
        let lab = Lab::new("d04a");
        lab.src("main.kp", &format!("impl \"{id}\"\n\nfun main() {{\n}}\n"));
        let out = lab.build();
        assert!(!out.status.success(), "the module id '{id}' was accepted");
    }
    let lab = Lab::new("d04b");
    lab.manifest_project(
        "name = \"d\"\n",
        &[
            (
                "main.kp",
                "impl \"util\" caps(fs = [\"__T__/data/**\"])\n\nfun main() {\n    util.go()\n}\n",
            ),
            (
                "util.kp",
                "impl \"std/fs\"\n\npub fun go() {\n    print(fs.read_text(\"__T__/secret/s.txt\"))\n}\n",
            ),
        ],
    );
    fs::create_dir_all(lab.root.join("src/std")).unwrap();
    let real = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/std/rt.kp")).unwrap();
    let evil = real.replacen(
        "fun glob_match(pat: &str, path: &str) => bool {",
        "fun glob_match(pat: &str, path: &str) => bool {\n    return true",
        1,
    );
    fs::write(lab.root.join("src/std/rt.kp"), evil).unwrap();
    lab.denied("granted to module 'util'");
}

/// D5: a user extern section is the manifest author's decision, but within it
/// the allowlist holds: a symbol that is not listed is refused, and a module
/// the manifest does not name cannot bind the symbol of one it does.
#[test]
fn d05_the_extern_allowlist_is_per_module_and_per_symbol() {
    let manifest = "name = \"x\"\n\n[extern.\"sys\"]\ndomain = \"process\"\nlibrary = \"c\"\nsymbols = [\"getpid\"]\n";
    let lab = Lab::new("d05a");
    lab.manifest_project(
        manifest,
        &[
            ("main.kp", "impl \"sys\" caps(process = [\"/**\"])\n\nfun main() {\n    print(sys.getppid() > 0)\n}\n"),
            ("sys.kp", "pub extern fun getppid() => i32\n"),
        ],
    );
    lab.refused(&["getppid"]);
    let lab = Lab::new("d05b");
    lab.manifest_project(
        manifest,
        &[
            ("main.kp", "impl \"evil\"\n\nfun main() {\n    print(evil.getpid() > 0)\n}\n"),
            ("evil.kp", "pub extern fun getpid() => i32\n"),
        ],
    );
    lab.refused(&["has no compiler-supplied body"]);
    // And the manifest's domain gate applies to the module that does bind it.
    let lab = Lab::new("d05c");
    lab.manifest_project(
        manifest,
        &[
            ("main.kp", "impl \"util\"\n\nfun main() {\n    print(util.pid() > 0)\n}\n"),
            ("util.kp", "impl \"sys\"\n\npub fun pid() => i32 = sys.getpid()\n"),
            ("sys.kp", "pub extern fun getpid() => i32\n"),
        ],
    );
    lab.refused(&["needs the 'process' capability"]);
}
