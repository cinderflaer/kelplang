//! The benchmark suite under `bench/suite/` keeps two programs per benchmark (`<name>.kp` and
//! `<name>.rs`) that must do the same work and print the same checksum line. Timing the two is
//! `bench/run.sh`'s job and takes minutes; this test is the fast half that keeps the two honest:
//! every benchmark takes its size as the first argument, so each is built once and run at a tiny
//! size. The Rust program (built by `rustc`) is the reference, and the Kelp program (built by the
//! LLVM backend) must print exactly the Rust line. A second size is also run to prove the size is
//! a parameter and not a constant in the source.
//!
//! Needs `rustc` on the PATH, which the dev shell provides. Every program runs under `ulimit -v`/`-t`
//! and `timeout`.

use std::path::{Path, PathBuf};
use std::process::Command;

/// (benchmark, a tiny size, a second tiny size). The default sizes are in the sources, not here.
const BENCHES: &[(&str, &str, &str)] = &[
    ("fib", "20", "21"),
    ("nbody", "1000", "1500"),
    ("spectral-norm", "50", "60"),
    ("mandelbrot", "60", "70"),
    ("binary-trees", "6", "7"),
    ("fannkuch-redux", "6", "7"),
    ("sieve", "10000", "20000"),
    ("matmul", "20", "30"),
    ("strings", "500", "600"),
    ("hashmap", "500", "600"),
    ("sort", "2000", "3000"),
    ("sort-builtin", "2000", "3000"),
    ("json-tokenizer", "5000", "6000"),
];

fn suite_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("bench").join("suite")
}

fn scratch() -> PathBuf {
    let base = std::env::var("CARGO_TARGET_TMPDIR")
        .unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().to_string());
    let p = PathBuf::from(base).join("bench_suite");
    std::fs::create_dir_all(&p).expect("create scratch dir");
    p
}

fn have(tool: &str) -> bool {
    Command::new(tool).arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

fn run_bounded(bin: &Path, arg: &str) -> Result<String, String> {
    let out = Command::new("sh")
        .arg("-c")
        .arg("ulimit -v 1048576; ulimit -t 20; exec timeout 20 \"$0\" \"$1\"")
        .arg(bin)
        .arg(arg)
        .output()
        .map_err(|e| format!("spawn {}: {e}", bin.display()))?;
    if !out.status.success() {
        return Err(format!(
            "{} {arg} exited {:?}: {}",
            bin.display(),
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

fn build_kelp(src: &Path, bin: &Path) -> Result<(), String> {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_kelpc"));
    cmd.env_remove("KELPC_BACKEND");
    let out = cmd
        .arg("build")
        .arg("-O2")
        .arg(src)
        .arg("-o")
        .arg(bin)
        .output()
        .map_err(|e| format!("spawn kelpc: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("kelpc build {} failed: {}", src.display(), String::from_utf8_lossy(&out.stderr)))
    }
}

fn check_one(name: &str, size: &str, size2: &str) -> Result<(), String> {
    let dir = suite_dir().join(name);
    let tmp = scratch();
    let rs_bin = tmp.join(format!("{name}.rs"));
    let rc = Command::new("rustc")
        .args(["-C", "opt-level=3", "-o"])
        .arg(&rs_bin)
        .arg(dir.join(format!("{name}.rs")))
        .output()
        .map_err(|e| format!("spawn rustc: {e}"))?;
    if !rc.status.success() {
        return Err(format!("rustc failed for {name}: {}", String::from_utf8_lossy(&rc.stderr)));
    }
    let want = run_bounded(&rs_bin, size)?;
    if !want.starts_with(name) || want.lines().count() != 1 {
        return Err(format!("{name}: the Rust version printed {want:?}, expected one line starting with the name"));
    }
    let want2 = run_bounded(&rs_bin, size2)?;
    if want == want2 {
        return Err(format!("{name}: sizes {size} and {size2} print the same line, so the size is not a parameter"));
    }

    let mut problems = Vec::new();
    let bin = tmp.join(format!("{name}.kelp-llvm"));
    match build_kelp(&dir.join(format!("{name}.kp")), &bin) {
        Err(e) => problems.push(e),
        Ok(()) => {
            for (sz, expect) in [(size, &want), (size2, &want2)] {
                match run_bounded(&bin, sz) {
                    Err(e) => problems.push(e),
                    Ok(got) if &got != expect => problems.push(format!(
                        "{name} [kelp-llvm] size {sz}: got {got:?}, the Rust version printed {expect:?}"
                    )),
                    Ok(_) => {}
                }
            }
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("\n"))
    }
}

#[test]
fn every_benchmark_agrees_with_rust_on_the_llvm_backend_at_a_tiny_size() {
    assert!(have("rustc"), "the benchmark suite test needs `rustc` on the PATH (run it inside the dev shell)");
    let mut failures = Vec::new();
    for (name, size, size2) in BENCHES {
        if let Err(e) = check_one(name, size, size2) {
            failures.push(e);
        }
    }
    assert!(failures.is_empty(), "benchmark disagreements:\n{}", failures.join("\n"));
}

#[test]
fn the_suite_directory_holds_exactly_the_listed_benchmarks_in_two_languages() {
    let mut on_disk: Vec<String> = std::fs::read_dir(suite_dir())
        .expect("bench/suite exists")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    on_disk.sort();
    let mut listed: Vec<String> = BENCHES.iter().map(|(n, _, _)| n.to_string()).collect();
    listed.sort();
    assert_eq!(on_disk, listed, "bench/suite and BENCHES in this test disagree");
    for (name, _, _) in BENCHES {
        for ext in ["kp", "rs"] {
            let f = suite_dir().join(name).join(format!("{name}.{ext}"));
            assert!(f.is_file(), "missing {}", f.display());
        }
    }
}
