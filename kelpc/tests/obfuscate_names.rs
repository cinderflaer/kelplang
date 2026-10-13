//! Obfuscation step 1: `[build] obfuscate = "names"` (DECISIONS.md 68 and 96).
//!
//! Names mode keeps a hosted build's intermediate IR and objects in a temporary
//! directory and links the binary with `ld.lld -s`, which drops its symbol table
//! and debug sections. It is not security: a backtrace shows no names and the
//! binary has no line information. These tests check the three claims the step
//! makes: nothing but the binary is left beside the output, the binary has no
//! `.symtab` and no `.debug_*` section, and the program behaves as the default
//! build does. A freestanding project and an unknown value are refused by name.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const PROGRAM: &str = "fun main() {\n    print(\"obfuscated names build\")\n}\n";

fn kelpc() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_kelpc"));
    cmd.env_remove("KELPC_BACKEND");
    cmd
}

/// A fresh directory under the test's target tmp dir, named by `tag`.
fn scratch(tag: &str) -> PathBuf {
    let mut dir = PathBuf::from(
        std::env::var("CARGO_TARGET_TMPDIR")
            .unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().to_string()),
    );
    dir.push(format!("kelpc_obfuscate_{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)
        .unwrap_or_else(|e| panic!("could not create '{}': {e}", dir.display()));
    dir
}

/// Writes a hosted project whose manifest is `name` plus `build_section`.
fn hosted_project(tag: &str, build_section: &str) -> PathBuf {
    let root = scratch(tag).join("proj");
    std::fs::create_dir_all(root.join("src")).expect("create src");
    std::fs::write(
        root.join("coral.toml"),
        format!("name = \"obfuscate{}\"\n{build_section}", tag.replace('_', "")),
    )
    .expect("write coral.toml");
    std::fs::write(root.join("src/main.kp"), PROGRAM).expect("write main.kp");
    root
}

fn build(project: &Path, out: &Path) -> Output {
    kelpc()
        .arg("build")
        .arg(project)
        .arg("-o")
        .arg(out)
        .output()
        .expect("failed to spawn kelpc")
}

fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("could not list '{}': {e}", dir.display()))
        .map(|entry| entry.expect("dir entry").file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// The names of every section in a 64-bit little-endian ELF image.
fn section_names(elf: &[u8]) -> Vec<String> {
    assert_eq!(&elf[..4], b"\x7fELF", "the output is not an ELF image");
    assert_eq!(elf[4], 2, "expected a 64-bit ELF image");
    let u16_at = |o: usize| u16::from_le_bytes([elf[o], elf[o + 1]]) as usize;
    let u32_at = |o: usize| u32::from_le_bytes(elf[o..o + 4].try_into().unwrap()) as usize;
    let u64_at = |o: usize| u64::from_le_bytes(elf[o..o + 8].try_into().unwrap()) as usize;
    let shoff = u64_at(0x28);
    let shentsize = u16_at(0x3a);
    let shnum = u16_at(0x3c);
    let shstrndx = u16_at(0x3e);
    let header = |i: usize| shoff + i * shentsize;
    // `sh_offset` is at 0x18 in each Elf64_Shdr; the names live in the string
    // table section that `e_shstrndx` names.
    let strtab = u64_at(header(shstrndx) + 0x18);
    (0..shnum)
        .map(|i| {
            let start = strtab + u32_at(header(i));
            let len = elf[start..].iter().position(|&b| b == 0).expect("unterminated name");
            String::from_utf8_lossy(&elf[start..start + len]).into_owned()
        })
        .collect()
}

#[test]
fn names_build_leaves_only_the_binary_beside_the_output() {
    let project = hosted_project("names_out", "[build]\nobfuscate = \"names\"\n");
    let out_dir = scratch("names_out_dir");
    let bin = out_dir.join("app");
    let built = build(&project, &bin);
    assert!(
        built.status.success(),
        "expected the names build to succeed:\n{}",
        String::from_utf8_lossy(&built.stderr)
    );
    assert_eq!(
        entries(&out_dir),
        vec!["app".to_string()],
        "a names build must leave nothing but the binary beside the output"
    );
}

#[test]
fn names_binary_has_no_symbol_table_and_no_debug_sections() {
    let names_project = hosted_project("names_sections", "[build]\nobfuscate = \"names\"\n");
    let names_bin = scratch("names_sections_out").join("app");
    assert!(build(&names_project, &names_bin).status.success());
    let names = section_names(&std::fs::read(&names_bin).expect("read names binary"));
    assert!(!names.iter().any(|s| s == ".symtab"), "names binary kept .symtab: {names:?}");
    assert!(
        !names.iter().any(|s| s.starts_with(".debug_")),
        "names binary kept debug sections: {names:?}"
    );

    // The same check must be able to see a symbol table: the default build has one.
    let plain_project = hosted_project("plain_sections", "");
    let plain_bin = scratch("plain_sections_out").join("app");
    assert!(build(&plain_project, &plain_bin).status.success());
    let plain = section_names(&std::fs::read(&plain_bin).expect("read default binary"));
    assert!(plain.iter().any(|s| s == ".symtab"), "default binary has no .symtab: {plain:?}");
}

#[test]
fn names_build_runs_like_the_default_build() {
    let names_project = hosted_project("names_run", "[build]\nobfuscate = \"names\"\n");
    let names_bin = scratch("names_run_out").join("app");
    assert!(build(&names_project, &names_bin).status.success());
    let plain_project = hosted_project("plain_run", "");
    let plain_bin = scratch("plain_run_out").join("app");
    assert!(build(&plain_project, &plain_bin).status.success());

    let names_run = Command::new(&names_bin).output().expect("run names binary");
    let plain_run = Command::new(&plain_bin).output().expect("run default binary");
    assert!(names_run.status.success(), "names binary failed: {:?}", names_run.status);
    assert_eq!(names_run.status.code(), plain_run.status.code());
    assert_eq!(names_run.stdout, plain_run.stdout, "stdout differs from the default build");
    assert_eq!(
        String::from_utf8_lossy(&names_run.stdout),
        "obfuscated names build\n",
        "unexpected output"
    );
}

#[test]
fn a_freestanding_project_is_refused_by_name_for_names_mode() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("projects")
        .join("freestanding_alloc");
    let root = scratch("freestanding_names").join("proj");
    copy_dir(&fixture, &root);
    let manifest = root.join("coral.toml");
    let mut text = std::fs::read_to_string(&manifest).expect("read manifest");
    text.push_str("\n[build]\nobfuscate = \"names\"\n");
    std::fs::write(&manifest, text).expect("write manifest");
    let out = scratch("freestanding_names_out").join("app");
    let built = build(&root, &out);
    let stderr = String::from_utf8_lossy(&built.stderr);
    assert!(!built.status.success(), "a freestanding names build must be refused");
    assert!(
        stderr.contains("obfuscate = \"names\"") && stderr.contains("freestanding"),
        "the refusal must name the key and the freestanding project:\n{stderr}"
    );
}

#[test]
fn an_unknown_obfuscate_value_names_both_allowed_values() {
    let project = hosted_project("bad_value", "[build]\nobfuscate = \"strip\"\n");
    let out = scratch("bad_value_out").join("app");
    let built = build(&project, &out);
    let stderr = String::from_utf8_lossy(&built.stderr);
    assert!(!built.status.success(), "an unknown obfuscate value must be refused");
    assert!(
        stderr.contains("\"off\" or \"names\""),
        "the refusal must name both allowed values:\n{stderr}"
    );
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("create copy directory");
    for entry in std::fs::read_dir(from).expect("read fixture") {
        let entry = entry.expect("fixture entry");
        let dest = to.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_dir(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), &dest).expect("copy fixture file");
        }
    }
}
