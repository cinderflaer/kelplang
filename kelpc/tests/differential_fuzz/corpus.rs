use super::*;

/// The cheap default-suite check: the generator is deterministic, and one
/// pinned seed builds with the `llvm` backend and runs to an exit code. It also
/// proves the generator's own output is valid Kelp, because the compiler builds it.
#[test]
pub(crate) fn the_generator_and_corpus_helpers_agree() {
    // Determinism: the same seed is the same bytes, twice.
    let a = generate(7);
    let b = generate(7);
    assert_eq!(a.source, b.source, "the generator is not deterministic");
    assert_ne!(
        generate(8).source,
        a.source,
        "two seeds produced one program, so the seed does not reach the text"
    );

    let dir = scratch("helpers");
    std::fs::create_dir_all(&dir)
        .unwrap_or_else(|e| panic!("could not create '{}': {e}", dir.display()));
    let source = dir.join("pinned_seed_7.kp");
    std::fs::write(&source, &a.source)
        .unwrap_or_else(|e| panic!("could not write '{}': {e}", source.display()));
    match check(7, &a.shapes, &source, &dir) {
        Outcome::Ran => {}
        Outcome::Refused(m) | Outcome::BuildCrashed(m) | Outcome::Mismatch(m) => {
            panic!("{m}")
        }
    }
}

/// The corpus files are exactly the generator's output for the seeds they are
/// named for, and together they cover every shape. No compilation here, so it
/// is cheap enough for the default suite; it is what stops the checked-in
/// corpus from drifting away from the generator that made it.
#[test]
pub(crate) fn the_corpus_files_are_the_generators_own_output() {
    let mut covered: BTreeSet<&'static str> = BTreeSet::new();
    for seed in CORPUS_SEEDS {
        let path = corpus_path(*seed);
        let on_disk = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "'{}' is missing ({e}); regenerate it with FUZZ_EMIT=1 \
                 `cargo test --release --test differential_fuzz -- --ignored \
                 emit_the_conformance_corpus`",
                path.display()
            )
        });
        let generated = generate(*seed);
        assert_eq!(
            on_disk, generated.source,
            "'{}' is not the generator's output for seed {seed}; regenerate the \
             corpus (FUZZ_EMIT=1) if the generator changed on purpose",
            path.display(),
        );
        covered.extend(generated.shapes);
    }
    for shape in SHAPES {
        assert!(
            covered.contains(shape),
            "the conformance corpus never exercises the '{shape}' shape; add a \
             seed that draws it to CORPUS_SEEDS",
        );
    }
}

/// The heavy half: the fixed seed list, built and run with the `llvm` backend.
/// Every program is written under the scratch directory before it is built, so a
/// failure is a file someone can open. The counts are printed with `--nocapture`.
#[test]
#[ignore = "generates and compiles hundreds of programs; run with --ignored"]
pub(crate) fn the_generator_builds_and_runs_every_seed() {
    let dir = scratch("generated");
    std::fs::create_dir_all(&dir)
        .unwrap_or_else(|e| panic!("could not create '{}': {e}", dir.display()));
    let (start, count) = fuzz_seeds();
    let mut ran = 0usize;
    let mut refused = 0usize;
    let mut build_crashed = 0usize;
    let mut mismatches = 0usize;
    let mut failures: Vec<String> = Vec::new();
    let mut covered: BTreeSet<&'static str> = BTreeSet::new();
    for seed in start..start + count {
        let program = generate(seed);
        covered.extend(program.shapes.iter().copied());
        let source = dir.join(format!("seed_{seed:05}.kp"));
        std::fs::write(&source, &program.source)
            .unwrap_or_else(|e| panic!("could not write '{}': {e}", source.display()));
        match check(seed, &program.shapes, &source, &dir) {
            Outcome::Ran => ran += 1,
            Outcome::Refused(m) => {
                refused += 1;
                failures.push(m);
            }
            Outcome::BuildCrashed(m) => {
                build_crashed += 1;
                failures.push(m);
            }
            Outcome::Mismatch(m) => {
                mismatches += 1;
                failures.push(m);
            }
        }
    }
    let missing: Vec<&str> = SHAPES
        .iter()
        .copied()
        .filter(|s| !covered.contains(s))
        .collect();
    println!(
        "differential fuzz: seeds {start}..{} generated {count} programs; \
         {ran} built and ran, {refused} refused by the compiler, {build_crashed} build \
         crash(es), {mismatches} run crash(es); shapes not drawn: {}",
        start + count,
        if missing.is_empty() {
            "none".to_string()
        } else {
            missing.join(", ")
        },
    );
    assert!(
        failures.is_empty(),
        "{} failing program(s) out of {count} generated (seed {start} upward); \
         the programs are under '{}':\n\n{}",
        failures.len(),
        dir.display(),
        failures.join("\n\n"),
    );
}

/// The checked-in corpus, built with the `llvm` backend and compared with its
/// golden: the exit status and the stdout the C oracle printed for the same file
/// before the C backend was removed. Each golden is `tests/fuzz-corpus/<name>.golden`:
/// its first line is `exit <code>` (or `signal <n>`), and the rest is the stdout,
/// byte for byte.
#[test]
#[ignore = "builds and runs the whole fuzz corpus; run with --ignored"]
pub(crate) fn the_conformance_corpus_matches_its_goldens() {
    let dir = scratch("corpus");
    std::fs::create_dir_all(&dir)
        .unwrap_or_else(|e| panic!("could not create '{}': {e}", dir.display()));
    let mut entries: Vec<(u64, PathBuf)> = std::fs::read_dir(corpus_dir())
        .unwrap_or_else(|e| panic!("could not read '{}': {e}", corpus_dir().display()))
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("kp") {
                return None;
            }
            Some((corpus_seed_of(&path)?, path))
        })
        .collect();
    entries.sort();
    assert!(
        !entries.is_empty(),
        "'{}' holds no corpus programs",
        corpus_dir().display(),
    );

    let mut failures: Vec<String> = Vec::new();
    for (seed, source) in &entries {
        let shapes = generate(*seed).shapes;
        match check(*seed, &shapes, source, &dir) {
            Outcome::Ran => {}
            Outcome::Refused(m) | Outcome::BuildCrashed(m) | Outcome::Mismatch(m) => {
                failures.push(m);
                continue;
            }
        }
        let golden_path = source.with_extension("golden");
        let golden = std::fs::read(&golden_path).unwrap_or_else(|e| {
            panic!("'{}' is missing ({e}); the corpus has no golden for seed {seed}", golden_path.display())
        });
        let (header, frozen_stdout) = match golden.iter().position(|b| *b == b'\n') {
            Some(at) => (&golden[..at], &golden[at + 1..]),
            None => panic!("'{}' has no header line", golden_path.display()),
        };
        let header = String::from_utf8_lossy(header).to_string();
        let observed = run(&dir.join("llvm").join(format!("seed_{seed:05}")));
        let observed_header = match (observed.code, observed.signal) {
            (Some(code), _) => format!("exit {code}"),
            (None, Some(signal)) => format!("signal {signal}"),
            (None, None) => "unknown".to_string(),
        };
        if observed_header != header || observed.stdout != frozen_stdout {
            failures.push(format!(
                "seed {seed}: '{}' differs from its golden\n  golden: {header}, {:?}\n  llvm:   {observed_header}, {:?}",
                source.display(),
                String::from_utf8_lossy(frozen_stdout),
                String::from_utf8_lossy(&observed.stdout),
            ));
        }
    }
    println!(
        "conformance corpus: {} program(s) checked against their goldens, {} failure(s)",
        entries.len(),
        failures.len(),
    );
    assert!(
        failures.is_empty(),
        "{} corpus program(s) failed:\n\n{}",
        failures.len(),
        failures.join("\n\n"),
    );
}

/// Rewrites `tests/fuzz-corpus/` from [`CORPUS_SEEDS`]. Guarded by `FUZZ_EMIT`
/// so a stray `--ignored` run cannot touch the tree; this is the only way the
/// corpus changes.
#[test]
#[ignore = "writes the conformance corpus; needs FUZZ_EMIT=1"]
pub(crate) fn emit_the_conformance_corpus() {
    if std::env::var("FUZZ_EMIT").as_deref() != Ok("1") {
        println!(
            "FUZZ_EMIT is not 1: {} corpus file(s) would be written to '{}'",
            CORPUS_SEEDS.len(),
            corpus_dir().display(),
        );
        return;
    }
    std::fs::create_dir_all(corpus_dir())
        .unwrap_or_else(|e| panic!("could not create '{}': {e}", corpus_dir().display()));
    for seed in CORPUS_SEEDS {
        let program = generate(*seed);
        let path = corpus_path(*seed);
        std::fs::write(&path, &program.source)
            .unwrap_or_else(|e| panic!("could not write '{}': {e}", path.display()));
        println!("wrote {} ({})", path.display(), program.shapes.join(" "));
    }
}
