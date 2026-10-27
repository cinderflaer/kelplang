//! The manifest's `[caps."<module id>"]` sections (issue #301): a grant written
//! once in `coral.toml` reaches every file that imports the named module, and
//! it is the ceiling for any `impl` clause on that module.
//!
//! Each test writes a small project into its own directory and runs
//! `kelpc check` over it. The front end runs the capability resolution, so a
//! refusal shows up here without building anything.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

fn kelpc() -> Command {
    Command::new(env!("CARGO_BIN_EXE_kelpc"))
}

/// A fresh project directory with `data/ok.txt` and `secret/s.txt`, named by
/// its real path, because grants are matched against resolved paths.
struct Project {
    root: PathBuf,
}

impl Project {
    fn new(name: &str) -> Project {
        let base = match option_env!("CARGO_TARGET_TMPDIR") {
            Some(dir) => PathBuf::from(dir),
            None => std::env::temp_dir().join("kelpc-manifest-caps-tests"),
        };
        let root = base.join("manifest-caps").join(name);
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("data")).expect("create data/");
        fs::create_dir_all(root.join("secret")).expect("create secret/");
        fs::write(root.join("data/ok.txt"), "fine\n").expect("write ok.txt");
        fs::write(root.join("secret/s.txt"), "TOKEN-7f3a").expect("write s.txt");
        let root = fs::canonicalize(&root).expect("canonicalize the project");
        Project { root }
    }

    /// Writes `coral.toml` and the `src/` files, replacing `__T__` with the
    /// project's absolute path in every text.
    fn write(&self, manifest: &str, files: &[(&str, &str)]) {
        let root = self.root.to_string_lossy().to_string();
        fs::write(self.root.join("coral.toml"), manifest.replace("__T__", &root))
            .expect("write the manifest");
        fs::create_dir_all(self.root.join("src")).expect("create src/");
        for (file, text) in files {
            fs::write(self.root.join("src").join(file), text.replace("__T__", &root))
                .expect("write a source file");
        }
    }

    fn check(&self) -> Output {
        kelpc().arg("check").arg(&self.root).output().expect("spawn kelpc")
    }

    fn checks(&self) {
        let out = self.check();
        let text = String::from_utf8_lossy(&out.stderr).to_string();
        assert!(out.status.success(), "expected the project to check, got:\n{text}");
    }

    fn refused(&self, needles: &[&str]) {
        let out = self.check();
        let text = String::from_utf8_lossy(&out.stderr).to_string();
        assert!(!out.status.success(), "expected a refusal, but the project checks");
        for n in needles {
            assert!(text.contains(n), "refusal lacks '{n}':\n{text}");
        }
    }
}

/// `util` reads `net` through `std/net` with a bare `impl`: no clause at all.
const UTIL_BARE_NET: &str = "impl \"std/result\"\nimpl \"std/net\"\n\npub fun go() {\n    val s = result.expect(net.connect(\"127.0.0.1\", 9), \"net.connect: the call failed\")\n    net.close(s)\n}\n";

/// `util` reads a file under `data/` through `std/fs` with a bare `impl`.
const UTIL_BARE_FS: &str = "impl \"std/fs\"\n\npub fun go() {\n    print(result.expect(fs.read_text(\"__T__/data/ok.txt\"), \"fs.read_text: the call failed\"))\n}\n";

fn main_importing_util(clause: &str) -> String {
    format!("impl \"util\"{clause}\n\nfun main() {{\n    util.go()\n}}\n")
}

/// A manifest grant reaches a bare `impl` line in another file.
#[test]
fn a_manifest_grant_reaches_a_bare_import() {
    let p = Project::new("bare_import");
    p.write(
        "name = \"m\"\n\n[caps.\"util\"]\nfs = [\"__T__/data/**\"]\n",
        &[("main.kp", &main_importing_util("")), ("util.kp", UTIL_BARE_FS)],
    );
    p.checks();
}

/// A module the manifest does not name still holds nothing from a bare
/// `impl` line: the grant is the manifest's, never a default.
#[test]
fn a_module_the_manifest_does_not_name_still_holds_nothing() {
    let p = Project::new("unnamed");
    p.write(
        "name = \"m\"\n\n[caps.\"other\"]\nnet = [\"*\"]\n",
        &[
            ("main.kp", "impl \"util\"\nimpl \"other\"\n\nfun main() {\n    util.go()\n    other.nothing()\n}\n"),
            ("util.kp", UTIL_BARE_NET),
            ("other.kp", "impl \"std/net\"\n\npub fun nothing() {\n}\n"),
        ],
    );
    p.refused(&["'util'"]);
}

/// A clause on a manifest-granted module that the manifest covers is accepted.
#[test]
fn a_clause_inside_the_manifest_ceiling_is_accepted() {
    let p = Project::new("inside_ceiling");
    p.write(
        "name = \"m\"\n\n[caps.\"util\"]\nfs = [\"__T__/data/**\"]\n",
        &[
            (
                "main.kp",
                &main_importing_util(" caps(fs = [\"__T__/data/ok.txt\"])"),
            ),
            ("util.kp", UTIL_BARE_FS),
        ],
    );
    p.checks();
}

/// A clause that asks for more than the manifest covers is refused, naming
/// the manifest.
#[test]
fn a_clause_beyond_the_manifest_ceiling_is_refused() {
    let p = Project::new("beyond_ceiling");
    p.write(
        "name = \"m\"\n\n[caps.\"util\"]\nfs = [\"__T__/data/**\"]\n",
        &[
            ("main.kp", &main_importing_util(" caps(fs = [\"__T__/**\"])")),
            ("util.kp", UTIL_BARE_FS),
        ],
    );
    p.refused(&["coral.toml", "[caps.\"util\"]"]);
}

/// A clause on a domain the manifest does not list for the module is refused.
#[test]
fn a_clause_on_an_unlisted_domain_is_refused() {
    let p = Project::new("unlisted_domain");
    p.write(
        "name = \"m\"\n\n[caps.\"util\"]\nfs = [\"__T__/data/**\"]\n",
        &[
            ("main.kp", &main_importing_util(" caps(net = [\"*\"])")),
            ("util.kp", UTIL_BARE_FS),
        ],
    );
    p.refused(&["[caps.\"util\"]"]);
}

/// A manifest grant the entry module does not hold is refused: the project
/// cannot delegate more than its own `[caps]` section gives `main`.
#[test]
fn a_manifest_grant_beyond_the_entry_module_is_refused() {
    let p = Project::new("beyond_entry");
    p.write(
        "name = \"m\"\n\n[caps]\nfs = [\"__T__/data/**\"]\n\n[caps.\"util\"]\nfs = [\"__T__/**\"]\n",
        &[("main.kp", &main_importing_util("")), ("util.kp", UTIL_BARE_FS)],
    );
    p.refused(&["entry module", "[caps.\"util\"]"]);
}

/// A manifest section for a module the program never imports is refused.
#[test]
fn a_section_for_an_unimported_module_is_refused() {
    let p = Project::new("unimported");
    p.write(
        "name = \"m\"\n\n[caps.\"util\"]\nfs = [\"__T__/data/**\"]\n",
        &[("main.kp", "fun main() {\n}\n")],
    );
    p.refused(&["'util'", "does not import"]);
}

/// The entry module's grant is `[caps]`, so a `[caps."main"]` section is refused.
#[test]
fn a_section_for_the_entry_module_is_refused() {
    let p = Project::new("entry_section");
    p.write(
        "name = \"m\"\n\n[caps.\"main\"]\nfs = [\"__T__/data/**\"]\n",
        &[("main.kp", "fun main() {\n}\n")],
    );
    p.refused(&["[caps]"]);
}

/// A module that reaches no capability domain cannot be granted anything.
#[test]
fn a_section_for_an_ungated_module_is_refused() {
    let p = Project::new("ungated");
    p.write(
        "name = \"m\"\n\n[caps.\"std/str\"]\nfs = [\"__T__/data/**\"]\n",
        &[("main.kp", "impl \"std/str\"\n\nfun main() {\n}\n")],
    );
    p.refused(&["'std/str'", "reaches no capability domain"]);
}

/// Each module id appears at most once across the `[caps."..."]` sections.
#[test]
fn a_module_named_twice_is_refused() {
    let p = Project::new("twice");
    p.write(
        "name = \"m\"\n\n[caps.\"util\"]\nfs = [\"__T__/data/**\"]\n\n[caps.\"util\"]\nfs = [\"__T__/data/**\"]\n",
        &[("main.kp", &main_importing_util("")), ("util.kp", UTIL_BARE_FS)],
    );
    p.refused(&["'util'", "more than once"]);
}

/// The domain and pattern vocabulary is the one `caps(...)` and `[caps]` use.
#[test]
fn an_unknown_domain_in_a_module_section_is_refused() {
    let p = Project::new("unknown_domain");
    p.write(
        "name = \"m\"\n\n[caps.\"util\"]\nsockets = [\"*\"]\n",
        &[("main.kp", &main_importing_util("")), ("util.kp", UTIL_BARE_FS)],
    );
    p.refused(&["'sockets' is not a capability domain"]);
}
