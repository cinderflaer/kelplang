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

#[path = "threat_model/fs_grant.rs"]
mod fs_grant;

#[path = "threat_model/proc_env.rs"]
mod proc_env;

#[path = "threat_model/net.rs"]
mod net;

#[path = "threat_model/unsafe_mmio.rs"]
mod unsafe_mmio;
