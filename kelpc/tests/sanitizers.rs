//! `--sanitize=address|undefined|thread` (plan/sanitizers.md): five existing
//! fixtures build and run clean under ASan and UBSan on the C backend and print
//! what the plain build prints, a deliberate out-of-bounds `Ptr` write is caught
//! and reported against its `.kp` line, the manifest key works, and the LLVM
//! backend refuses the flag naming the oracle switch. The wider sweep over 40+
//! fixtures is `tests/run-sanitizers.sh`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const KELPC: &str = env!("CARGO_BIN_EXE_kelpc");

const FIXTURES: &[&str] = &[
    "box_recursive",
    "box_owned_elements",
    "bytes_build",
    "closure_move",
    "chan_bounded_mpsc",
];

fn tests_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests")
}

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("sanitizers").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn kelpc(args: &[&str], oracle: bool) -> Output {
    let mut c = Command::new(KELPC);
    c.args(args).env_remove("KELPC_BACKEND");
    if oracle {
        c.env("KELPC_ORACLE", "c");
    } else {
        c.env_remove("KELPC_ORACLE");
    }
    c.output().expect("could not start kelpc")
}

/// Runs a built program bounded by CPU time and wall time, with no stdin. The
/// address-space limit is left off on purpose: a sanitizer maps terabytes of
/// shadow memory, so its own RSS limit is the memory bound.
fn run(prog: &Path) -> Output {
    Command::new("sh")
        .arg("-c")
        .arg("ulimit -t 10; exec timeout 10 \"$0\" </dev/null")
        .arg(prog)
        .env("ASAN_OPTIONS", "hard_rss_limit_mb=1024:detect_leaks=1")
        .env("UBSAN_OPTIONS", "print_stacktrace=1")
        .output()
        .expect("could not run the program")
}

fn build(src: &Path, out: &Path, sanitize: Option<&str>) -> Output {
    let mut args = vec!["build", src.to_str().unwrap(), "-o", out.to_str().unwrap()];
    let flag;
    if let Some(s) = sanitize {
        flag = format!("--sanitize={s}");
        args.push(&flag);
    }
    kelpc(&args, true)
}

#[test]
fn fixtures_run_clean_under_address_and_undefined_sanitizers() {
    let dir = scratch("fixtures");
    for name in FIXTURES {
        let src = tests_dir().join("fixtures").join(format!("{name}.kp"));
        let plain = dir.join(format!("{name}_plain"));
        let b = build(&src, &plain, None);
        assert!(b.status.success(), "{name}: {}", String::from_utf8_lossy(&b.stderr));
        let reference = run(&plain);
        assert!(reference.status.success(), "{name}: the plain run failed");
        for san in ["address", "undefined"] {
            let prog = dir.join(format!("{name}_{san}"));
            let b = build(&src, &prog, Some(san));
            assert!(
                b.status.success(),
                "{name} with {san}: {}",
                String::from_utf8_lossy(&b.stderr)
            );
            let out = run(&prog);
            let err = String::from_utf8_lossy(&out.stderr);
            assert!(
                out.status.success() && !err.contains("Sanitizer") && !err.contains("runtime error"),
                "{name} under {san} reported:\n{err}"
            );
            assert_eq!(out.stdout, reference.stdout, "{name} under {san} printed differently");
        }
    }
}

#[test]
fn a_deliberate_out_of_bounds_write_is_caught_and_named_by_line() {
    let dir = scratch("oob");
    let project = tests_dir().join("hazards").join("sanitizer_oob");
    for (san, needle) in [
        ("address", "AddressSanitizer: stack-buffer-overflow"),
        ("undefined", "runtime error: store to address"),
    ] {
        let prog = dir.join(san);
        let b = build(&project, &prog, Some(san));
        assert!(b.status.success(), "{san}: {}", String::from_utf8_lossy(&b.stderr));
        let out = run(&prog);
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{san} did not stop the program");
        assert!(err.contains(needle), "{san} report lacks '{needle}':\n{err}");
        assert!(err.contains("main.kp:6"), "{san} report does not name main.kp:6:\n{err}");
    }
}

#[test]
fn the_llvm_backend_refuses_a_sanitizer_and_names_the_oracle() {
    let dir = scratch("llvm");
    let src = tests_dir().join("fixtures").join("box_basic.kp");
    let out = kelpc(
        &["build", src.to_str().unwrap(), "-o", dir.join("x").to_str().unwrap(), "--sanitize=address"],
        false,
    );
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("KELPC_ORACLE=c"), "{err}");
    assert!(err.contains("LLVM backend"), "{err}");
}

#[test]
fn an_unknown_sanitizer_is_refused() {
    let src = tests_dir().join("fixtures").join("box_basic.kp");
    let out = kelpc(&["build", src.to_str().unwrap(), "--sanitize=memory"], true);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("address, undefined or thread"));
}

#[test]
fn the_manifest_key_turns_a_sanitizer_on_and_the_flag_overrides_it() {
    let dir = scratch("manifest");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("coral.toml"),
        "name = \"sanmanifest\"\n[build]\nsanitize = \"address\"\n[unsafe]\nmodules = [\"main\"]\n",
    )
    .unwrap();
    std::fs::copy(
        tests_dir().join("hazards").join("sanitizer_oob").join("src").join("main.kp"),
        dir.join("src").join("main.kp"),
    )
    .unwrap();
    // From the manifest alone: ASan catches it.
    let prog = dir.join("by_manifest");
    let b = kelpc(&["build", dir.to_str().unwrap(), "-o", prog.to_str().unwrap()], true);
    assert!(b.status.success(), "{}", String::from_utf8_lossy(&b.stderr));
    let err = String::from_utf8_lossy(&run(&prog).stderr).into_owned();
    assert!(err.contains("AddressSanitizer"), "{err}");
    // The flag wins: UBSan's report, not ASan's.
    let prog = dir.join("by_flag");
    let b = kelpc(
        &["build", dir.to_str().unwrap(), "-o", prog.to_str().unwrap(), "--sanitize=undefined"],
        true,
    );
    assert!(b.status.success());
    let err = String::from_utf8_lossy(&run(&prog).stderr).into_owned();
    assert!(err.contains("runtime error") && !err.contains("AddressSanitizer"), "{err}");
}
