#![allow(dead_code)] // Additive: no caller until the stage 3 link switch lands.

//! Finds what a hosted link needs without running `cc`: the `ld.lld` program,
//! the glibc crt and libc directories, the gcc library directory and the
//! dynamic loader. It also builds the `ld.lld` argv for a hosted program.
//!
//! This module is additive. Nothing calls it yet; the link paths in `driver.rs`
//! still run `cc` (hosted) and `ld.lld` (freestanding) exactly as before.
//!
//! Lookup order for `ld.lld`: `KELPC_LLD` (an absolute path to a file), then
//! `ld.lld` on `PATH`, then a named error. For the crt, libc and gcc directories:
//! the directories `build.rs` recorded at build time (from `cc -print-file-name`),
//! then the usual system locations. The builder itself is pure: it takes a
//! `Sysroot` and the link inputs and returns argv, with no environment or
//! filesystem access.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

/// Environment variable that names an `ld.lld` binary to use instead of PATH.
pub const LLD_ENV: &str = "KELPC_LLD";

/// The directories and programs a hosted link uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sysroot {
    pub ld_lld: PathBuf,
    /// Holds `Scrt1.o`, `crti.o` and `crtn.o`.
    pub crt_dir: PathBuf,
    /// Holds `crtbeginS.o`, `crtendS.o` and `libgcc.a` (the gcc library directory).
    pub gcc_lib_dir: PathBuf,
    /// Holds `libc.so` and `libm.so`.
    pub libc_dir: PathBuf,
    /// The dynamic loader, passed as `--dynamic-linker`.
    pub ld_linux: PathBuf,
}

/// Where to look. `discover_here` fills this from the real environment; tests
/// fill it from a fake directory tree.
#[derive(Debug, Clone, Default)]
pub struct Search {
    /// Value of `KELPC_LLD`, if set.
    pub lld_override: Option<OsString>,
    /// Value of `PATH`, if set.
    pub path: Option<OsString>,
    /// Directories `build.rs` recorded, tried first. Each is checked like any other candidate.
    pub baked_crt_dir: Option<PathBuf>,
    pub baked_gcc_lib_dir: Option<PathBuf>,
    pub baked_libc_dir: Option<PathBuf>,
    /// Directories searched after the baked ones, in order.
    pub system_dirs: Vec<PathBuf>,
    /// Roots whose child directories are searched for gcc's library directory
    /// (`<root>/<version>/crtbeginS.o`), newest name first.
    pub gcc_roots: Vec<PathBuf>,
    /// Absolute fallback dynamic loaders, tried after the glibc directory.
    pub ld_linux_fallbacks: Vec<PathBuf>,
}

const CRT_FILES: &[&str] = &["Scrt1.o", "crti.o", "crtn.o"];
const GCC_FILES: &[&str] = &["crtbeginS.o", "crtendS.o", "libgcc.a"];
const LIBC_FILES: &[&str] = &["libc.so", "libm.so"];
const LOADER: &str = "ld-linux-x86-64.so.2";

/// The `ld.lld` lookup with its inputs passed in. An override that is set and
/// non-empty wins and must name an existing file; otherwise the first executable
/// `ld.lld` on `path` is used.
pub fn find_ld_lld(override_var: Option<&OsStr>, path: Option<&OsStr>) -> Result<PathBuf, String> {
    if let Some(value) = override_var.filter(|v| !v.is_empty()) {
        let candidate = PathBuf::from(value);
        return if is_executable(&candidate) {
            Ok(candidate)
        } else {
            Err(format!(
                "could not find 'ld.lld': {LLD_ENV} is set to '{}', which is not an executable file",
                candidate.display()
            ))
        };
    }
    if let Some(path) = path {
        for dir in std::env::split_paths(path) {
            let candidate = dir.join("ld.lld");
            if is_executable(&candidate) {
                return Ok(candidate);
            }
        }
    }
    Err(format!(
        "could not find 'ld.lld': a build needs LLVM 21's lld on PATH (or set {LLD_ENV}); \
         the freestanding build already needs it"
    ))
}

/// The first candidate directory that holds every file in `required`. The error
/// names the first required file missing from the first candidate, and lists
/// the directories searched.
pub fn find_dir(candidates: &[PathBuf], required: &[&str], what: &str) -> Result<PathBuf, String> {
    for dir in candidates {
        if required.iter().all(|f| dir.join(f).is_file()) {
            return Ok(dir.clone());
        }
    }
    let searched: Vec<String> = candidates.iter().map(|d| d.display().to_string()).collect();
    let first = candidates.first();
    let missing = required
        .iter()
        .find(|f| first.map_or(true, |d| !d.join(f).is_file()))
        .copied()
        .unwrap_or(required[0]);
    Err(format!(
        "could not find '{missing}' for the {what} directory; searched [{}]. \
         Install the glibc development files (libc6-dev, or glibc's dev output on Nix)",
        searched.join(", ")
    ))
}

/// Resolves every piece of a hosted link from `search`. No process is spawned.
pub fn discover(search: &Search) -> Result<Sysroot, String> {
    let ld_lld = find_ld_lld(search.lld_override.as_deref(), search.path.as_deref())?;

    let crt_candidates = with_baked(&search.baked_crt_dir, &search.system_dirs);
    let crt_dir = find_dir(&crt_candidates, CRT_FILES, "crt")?;

    let libc_candidates = with_baked(&search.baked_libc_dir, &search.system_dirs);
    let libc_dir = find_dir(&libc_candidates, LIBC_FILES, "libc")?;

    let mut gcc_candidates: Vec<PathBuf> = search.baked_gcc_lib_dir.iter().cloned().collect();
    for root in &search.gcc_roots {
        let mut children: Vec<PathBuf> = std::fs::read_dir(root)
            .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_dir()).collect())
            .unwrap_or_default();
        children.sort();
        children.reverse();
        gcc_candidates.extend(children);
    }
    let gcc_lib_dir = find_dir(&gcc_candidates, GCC_FILES, "gcc library")?;

    let ld_linux = find_loader(&libc_dir, &search.ld_linux_fallbacks)?;

    Ok(Sysroot { ld_lld, crt_dir, gcc_lib_dir, libc_dir, ld_linux })
}

/// `discover` with the real environment and the usual Linux locations.
pub fn discover_here() -> Result<Sysroot, String> {
    let dir = |v: Option<&str>| v.filter(|s| !s.is_empty()).map(PathBuf::from);
    let search = Search {
        lld_override: std::env::var_os(LLD_ENV),
        path: std::env::var_os("PATH"),
        baked_crt_dir: dir(option_env!("KELPC_CRT_DIR")),
        baked_gcc_lib_dir: dir(option_env!("KELPC_GCC_LIB_DIR")),
        baked_libc_dir: dir(option_env!("KELPC_LIBC_DIR")),
        system_dirs: [
            "/usr/lib/x86_64-linux-gnu",
            "/usr/lib64",
            "/usr/lib",
        ]
        .iter()
        .map(PathBuf::from)
        .collect(),
        gcc_roots: ["/usr/lib/gcc/x86_64-linux-gnu", "/usr/lib/gcc/x86_64-pc-linux-gnu"]
            .iter()
            .map(PathBuf::from)
            .collect(),
        ld_linux_fallbacks: ["/lib64/ld-linux-x86-64.so.2", "/lib/x86_64-linux-gnu/ld-linux-x86-64.so.2"]
            .iter()
            .map(PathBuf::from)
            .collect(),
    };
    discover(&search)
}

fn with_baked(baked: &Option<PathBuf>, system: &[PathBuf]) -> Vec<PathBuf> {
    baked.iter().cloned().chain(system.iter().cloned()).collect()
}

fn find_loader(libc_dir: &Path, fallbacks: &[PathBuf]) -> Result<PathBuf, String> {
    let beside_libc = libc_dir.join(LOADER);
    if beside_libc.exists() {
        return Ok(beside_libc);
    }
    for path in fallbacks {
        if path.exists() {
            return Ok(path.clone());
        }
    }
    Err(format!(
        "could not find the dynamic loader '{LOADER}' in '{}' or at the usual system paths",
        libc_dir.display()
    ))
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// The inputs of one hosted link, the same ones `driver.rs::link_llvm_objects`
/// receives today.
pub struct HostedLink<'a> {
    pub out: &'a Path,
    /// The unit objects, in order.
    pub objects: &'a [PathBuf],
    /// The runtime object's path (written to a temp file by the caller).
    pub runtime_obj: &'a Path,
    pub externs_obj: Option<&'a Path>,
    /// `coral.toml` library directories, as `-L` values.
    pub library_dirs: &'a [String],
    /// Package flags from pkg-config. Only `-L` and `-l` survive; compile-only
    /// flags such as `-I` or `-D` mean nothing to a linker.
    pub pkg_config_flags: &'a [String],
    pub tls: bool,
    pub link_libs: &'a [String],
}

/// The argv for `ld.lld` (the program name is not included). Mirrors the gcc
/// link line that the `cc` path produces on glibc: PIE with `Scrt1.o`, the crt
/// objects around the program, `libgcc` with `--as-needed` around `libc`, and
/// `-rpath` for every `/nix/store` library directory, since the gcc wrapper
/// that would add them is not in the path.
///
/// Differences from the `cc` argv: no `-O<n>` (it does nothing at link time)
/// and no `-pthread` on the link, which becomes `-lpthread`.
pub fn hosted_link_argv(sys: &Sysroot, link: &HostedLink) -> Vec<OsString> {
    let mut a: Vec<OsString> = Vec::new();
    let push = |a: &mut Vec<OsString>, s: &str| a.push(OsString::from(s));

    push(&mut a, "-m");
    push(&mut a, "elf_x86_64");
    push(&mut a, "-pie");
    push(&mut a, "--eh-frame-hdr");
    push(&mut a, "--dynamic-linker");
    a.push(sys.ld_linux.as_os_str().to_owned());
    push(&mut a, "-o");
    a.push(link.out.as_os_str().to_owned());

    a.push(sys.crt_dir.join("Scrt1.o").into_os_string());
    a.push(sys.gcc_lib_dir.join("crtbeginS.o").into_os_string());
    a.push(sys.crt_dir.join("crti.o").into_os_string());

    for object in link.objects {
        a.push(object.as_os_str().to_owned());
    }
    a.push(link.runtime_obj.as_os_str().to_owned());
    if let Some(externs) = link.externs_obj {
        a.push(externs.as_os_str().to_owned());
    }

    let mut lib_dirs: Vec<PathBuf> = vec![
        sys.crt_dir.clone(),
        sys.gcc_lib_dir.clone(),
        sys.libc_dir.clone(),
    ];
    lib_dirs.extend(link.library_dirs.iter().map(PathBuf::from));
    for dir in &lib_dirs {
        let mut flag = OsString::from("-L");
        flag.push(dir.as_os_str());
        a.push(flag);
    }
    for flag in link.pkg_config_flags {
        if flag.starts_with("-L") || flag.starts_with("-l") {
            a.push(OsString::from(flag));
        }
    }

    push(&mut a, "-lm");
    push(&mut a, "-lpthread");
    if link.tls {
        push(&mut a, "-lssl");
        push(&mut a, "-lcrypto");
    }
    for lib in link.link_libs {
        a.push(OsString::from(format!("-l{lib}")));
    }

    for part in [
        "-lgcc", "--as-needed", "-lgcc_s", "--no-as-needed", "-lc", "-lgcc", "--as-needed",
        "-lgcc_s", "--no-as-needed",
    ] {
        push(&mut a, part);
    }
    a.push(sys.gcc_lib_dir.join("crtendS.o").into_os_string());
    a.push(sys.crt_dir.join("crtn.o").into_os_string());

    let mut rpaths: Vec<&Path> = Vec::new();
    for dir in &lib_dirs {
        if dir.starts_with("/nix/store") && !rpaths.contains(&dir.as_path()) {
            rpaths.push(dir.as_path());
        }
    }
    for dir in rpaths {
        push(&mut a, "-rpath");
        a.push(dir.as_os_str().to_owned());
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A fresh scratch directory under the system temp dir, removed by the caller.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kelpc-sysroot-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch(dir: &Path, names: &[&str]) {
        fs::create_dir_all(dir).unwrap();
        for n in names {
            fs::write(dir.join(n), b"").unwrap();
        }
    }

    fn make_exec(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        fs::write(path, b"#!/bin/sh\n").unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// A fake tree: glibc under `glibc/lib`, gcc under `gcc/lib/gcc/x86_64/15`,
    /// a loader beside libc, and a bin dir with `ld.lld`.
    struct Tree {
        root: PathBuf,
    }

    impl Tree {
        fn new(name: &str) -> Tree {
            let root = scratch(name);
            touch(&root.join("glibc/lib"), &["Scrt1.o", "crti.o", "crtn.o", "libc.so", "libm.so", LOADER]);
            touch(&root.join("gcc/lib/gcc/x86_64/15"), &["crtbeginS.o", "crtendS.o", "libgcc.a"]);
            fs::create_dir_all(root.join("bin")).unwrap();
            make_exec(&root.join("bin/ld.lld"));
            Tree { root }
        }
        fn glibc(&self) -> PathBuf {
            self.root.join("glibc/lib")
        }
        fn gcc_root(&self) -> PathBuf {
            self.root.join("gcc/lib/gcc/x86_64")
        }
        fn bin(&self) -> PathBuf {
            self.root.join("bin")
        }
        fn search(&self) -> Search {
            Search {
                path: Some(self.bin().into_os_string()),
                baked_crt_dir: Some(self.glibc()),
                baked_libc_dir: Some(self.glibc()),
                gcc_roots: vec![self.gcc_root()],
                ..Search::default()
            }
        }
        fn drop_tree(self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn sysroot_env_override_wins_over_path() {
        let t = Tree::new("override");
        let other = t.root.join("other-ld.lld");
        make_exec(&other);
        let found = find_ld_lld(Some(other.as_os_str()), Some(t.bin().as_os_str())).unwrap();
        assert_eq!(found, other);
        t.drop_tree();
    }

    #[test]
    fn sysroot_override_to_a_missing_file_names_the_variable() {
        let t = Tree::new("missing-override");
        let missing = t.root.join("nope/ld.lld");
        let err = find_ld_lld(Some(missing.as_os_str()), Some(t.bin().as_os_str())).unwrap_err();
        assert!(err.contains(LLD_ENV), "{err}");
        t.drop_tree();
    }

    #[test]
    fn sysroot_empty_override_is_ignored() {
        let t = Tree::new("empty-override");
        let found = find_ld_lld(Some(OsStr::new("")), Some(t.bin().as_os_str())).unwrap();
        assert_eq!(found, t.bin().join("ld.lld"));
        t.drop_tree();
    }

    #[test]
    fn sysroot_path_lookup_finds_ld_lld_and_skips_non_executables() {
        let t = Tree::new("path");
        let empty = t.root.join("empty");
        fs::create_dir_all(&empty).unwrap();
        fs::write(empty.join("ld.lld"), b"not executable").unwrap();
        let path = std::env::join_paths([empty.as_path(), t.bin().as_path()]).unwrap();
        assert_eq!(find_ld_lld(None, Some(&path)).unwrap(), t.bin().join("ld.lld"));
        t.drop_tree();
    }

    #[test]
    fn sysroot_missing_ld_lld_is_a_named_error() {
        let t = Tree::new("no-lld");
        let err = find_ld_lld(None, Some(t.glibc().as_os_str())).unwrap_err();
        assert!(err.contains("ld.lld") && err.contains(LLD_ENV), "{err}");
        assert!(find_ld_lld(None, None).unwrap_err().contains("could not find 'ld.lld'"));
        t.drop_tree();
    }

    #[test]
    fn sysroot_discovers_every_directory_from_the_fake_tree() {
        let t = Tree::new("discover");
        let sys = discover(&t.search()).unwrap();
        assert_eq!(sys.ld_lld, t.bin().join("ld.lld"));
        assert_eq!(sys.crt_dir, t.glibc());
        assert_eq!(sys.libc_dir, t.glibc());
        assert_eq!(sys.gcc_lib_dir, t.gcc_root().join("15"));
        assert_eq!(sys.ld_linux, t.glibc().join(LOADER));
        t.drop_tree();
    }

    #[test]
    fn sysroot_missing_scrt1_names_the_file() {
        let t = Tree::new("no-scrt");
        fs::remove_file(t.glibc().join("Scrt1.o")).unwrap();
        let err = discover(&t.search()).unwrap_err();
        assert!(err.contains("Scrt1.o"), "{err}");
        t.drop_tree();
    }

    #[test]
    fn sysroot_missing_gcc_files_are_named() {
        let t = Tree::new("no-gcc");
        fs::remove_dir_all(t.gcc_root()).unwrap();
        let err = discover(&t.search()).unwrap_err();
        assert!(err.contains("crtbeginS.o"), "{err}");
        t.drop_tree();
    }

    #[test]
    fn sysroot_system_dirs_are_used_when_nothing_is_baked() {
        let t = Tree::new("system");
        let search = Search {
            baked_crt_dir: None,
            baked_libc_dir: None,
            system_dirs: vec![t.root.join("nowhere"), t.glibc()],
            ..t.search()
        };
        let sys = discover(&search).unwrap();
        assert_eq!(sys.crt_dir, t.glibc());
        t.drop_tree();
    }

    fn sample_sysroot(nix: bool) -> Sysroot {
        let base = if nix { "/nix/store/aaa-glibc/lib" } else { "/usr/lib/x86_64-linux-gnu" };
        let gcc = if nix { "/nix/store/bbb-gcc/lib/gcc/x86_64/15" } else { "/usr/lib/gcc/x86_64-linux-gnu/15" };
        Sysroot {
            ld_lld: PathBuf::from("/usr/bin/ld.lld"),
            crt_dir: PathBuf::from(base),
            gcc_lib_dir: PathBuf::from(gcc),
            libc_dir: PathBuf::from(base),
            ld_linux: PathBuf::from(format!("{base}/{LOADER}")),
        }
    }

    fn strings(v: &[OsString]) -> Vec<String> {
        v.iter().map(|s| s.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn sysroot_hosted_argv_has_the_pie_crt_order_and_no_optimisation_flag() {
        let sys = sample_sysroot(false);
        let objects = vec![PathBuf::from("/tmp/u0.o")];
        let libs: Vec<String> = vec![];
        let dirs: Vec<String> = vec![];
        let pkg: Vec<String> = vec![];
        let link = HostedLink {
            out: Path::new("/tmp/hello"),
            objects: &objects,
            runtime_obj: Path::new("/tmp/rt.o"),
            externs_obj: None,
            library_dirs: &dirs,
            pkg_config_flags: &pkg,
            tls: false,
            link_libs: &libs,
        };
        let argv = strings(&hosted_link_argv(&sys, &link));
        assert!(argv.iter().all(|a| !a.starts_with("-O")), "{argv:?}");
        assert!(argv.contains(&"-pie".to_string()));
        let loader = argv.iter().position(|a| a == "--dynamic-linker").unwrap();
        assert_eq!(argv[loader + 1], sys.ld_linux.to_string_lossy());
        // Scrt1.o comes first after the output name, then crtbeginS.o, then crti.o.
        let after_out = argv.iter().position(|a| a == "-o").unwrap() + 2;
        assert!(argv[after_out].ends_with("/Scrt1.o"), "{argv:?}");
        assert!(argv[after_out + 1].ends_with("/crtbeginS.o"), "{argv:?}");
        assert!(argv[after_out + 2].ends_with("/crti.o"), "{argv:?}");
        let object = argv.iter().position(|a| a == "/tmp/u0.o").unwrap();
        assert!(after_out + 2 < object);
        assert!(argv.iter().any(|a| a == "-lpthread") && argv.iter().any(|a| a == "-lm"));
        assert!(!argv.iter().any(|a| a == "-lssl" || a == "-lcrypto"));
        assert!(!argv.iter().any(|a| a == "-rpath"), "Debian-style tree has no rpath: {argv:?}");
        assert!(argv.last().unwrap().ends_with("/crtn.o"));
    }

    #[test]
    fn sysroot_hosted_argv_adds_externs_tls_libs_and_nix_rpaths() {
        let sys = sample_sysroot(true);
        let objects = vec![PathBuf::from("/tmp/u0.o"), PathBuf::from("/tmp/u1.o")];
        let libs = vec!["foo".to_string()];
        let dirs = vec!["/nix/store/ccc-ssl/lib".to_string(), "/opt/local".to_string()];
        let pkg = vec!["-I/x".to_string(), "-L/nix/store/ddd-gtk/lib".to_string(), "-lgtk4".to_string()];
        let link = HostedLink {
            out: Path::new("/tmp/prog"),
            objects: &objects,
            runtime_obj: Path::new("/tmp/rt.o"),
            externs_obj: Some(Path::new("/tmp/prog.externs.o")),
            library_dirs: &dirs,
            pkg_config_flags: &pkg,
            tls: true,
            link_libs: &libs,
        };
        let argv = strings(&hosted_link_argv(&sys, &link));
        let ext = argv.iter().position(|a| a == "/tmp/prog.externs.o").unwrap();
        let rt = argv.iter().position(|a| a == "/tmp/rt.o").unwrap();
        // Units, then the runtime object, then the externs object.
        assert!(argv.iter().position(|a| a == "/tmp/u1.o").unwrap() < rt && rt < ext, "{argv:?}");
        assert!(!argv.iter().any(|a| a == "-I/x"), "compile-only flags must not reach the linker");
        assert!(argv.contains(&"-lgtk4".to_string()) && argv.contains(&"-L/nix/store/ddd-gtk/lib".to_string()));
        let ssl = argv.iter().position(|a| a == "-lssl").unwrap();
        assert_eq!(argv[ssl + 1], "-lcrypto");
        assert!(argv.contains(&"-lfoo".to_string()));
        let rpaths: Vec<&String> = argv.windows(2).filter(|w| w[0] == "-rpath").map(|w| &w[1]).collect();
        assert!(rpaths.iter().any(|p| p.as_str() == "/nix/store/aaa-glibc/lib"), "{argv:?}");
        assert!(rpaths.iter().any(|p| p.as_str() == "/nix/store/ccc-ssl/lib"), "{argv:?}");
        assert!(!rpaths.iter().any(|p| p.as_str() == "/opt/local"));
        let mut sorted = rpaths.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), rpaths.len(), "rpaths are not repeated: {argv:?}");
    }

    #[test]
    fn sysroot_argv_keeps_the_libgcc_as_needed_pairs_in_order() {
        let sys = sample_sysroot(false);
        let objects: Vec<PathBuf> = vec![];
        let (dirs, pkg, libs): (Vec<String>, Vec<String>, Vec<String>) = Default::default();
        let link = HostedLink {
            out: Path::new("/tmp/p"),
            objects: &objects,
            runtime_obj: Path::new("/tmp/rt.o"),
            externs_obj: None,
            library_dirs: &dirs,
            pkg_config_flags: &pkg,
            tls: false,
            link_libs: &libs,
        };
        let argv = strings(&hosted_link_argv(&sys, &link));
        let n = argv.len();
        assert_eq!(
            argv[n - 11..],
            [
                "-lgcc",
                "--as-needed",
                "-lgcc_s",
                "--no-as-needed",
                "-lc",
                "-lgcc",
                "--as-needed",
                "-lgcc_s",
                "--no-as-needed",
                "/usr/lib/gcc/x86_64-linux-gnu/15/crtendS.o",
                "/usr/lib/x86_64-linux-gnu/crtn.o",
            ]
        );
    }
}
