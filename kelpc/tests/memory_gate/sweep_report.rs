use super::*;

/// Says what the ignored sweep would cover, and whether the tool it needs is
/// here. It costs one `valgrind --version` spawn and never fails: valgrind is
/// not a declared dependency of this repository, so a machine without it must
/// skip the gate rather than fail on it.
#[test]
pub(crate) fn the_sweep_reports_what_it_would_cover() {
    let fixtures = fixture_names().len();
    assert!(
        fixtures > 0,
        "'{}' holds no .kp fixture, so the sweep below would check nothing",
        fixture_dir().display(),
    );
    match valgrind_version() {
        Some(version) => println!(
            "memory gate: {version} is on PATH; the ignored sweep builds every one of these {fixtures} \
             fixtures, then runs each binary under `--leak-check=full`. Run it with:\n    \
             cargo test --test memory_gate -- --ignored --nocapture"
        ),
        None => println!(
            "memory gate: valgrind is not on PATH, so the ignored sweep is skipped; install valgrind \
             to check the binaries' drops for leaks, double frees and invalid accesses"
        ),
    }
}
