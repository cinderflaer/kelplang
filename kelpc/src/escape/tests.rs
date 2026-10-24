use super::*;
use crate::diag::SourceMap;
use crate::infer;
use crate::resolve;
use std::path::{Path, PathBuf};

/// Loads one `.kp` file, checks it, and runs the analysis on it.
fn analyze_file(path: &Path) -> Result<Report, String> {
    let mut sources = SourceMap::new();
    let loaded = resolve::load(path, &mut sources, None, None).map_err(|d| format!("{d:?}"))?;
    let typed = infer::check_program(&loaded.program, &loaded.project).map_err(|d| format!("{d:?}"))?;
    Ok(analyze(&typed, &sources))
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("escape").join(name)
}

/// The reasons refusing the `Box.of` sites of one fixture, in order.
fn box_reasons(name: &str) -> Vec<&'static str> {
    let report = analyze_file(&fixture(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
    report
        .sites
        .iter()
        .filter(|s| s.kind == SiteKind::BoxOf)
        .map(|s| s.reason())
        .collect()
}

/// Every `Box.of` site of one fixture is accepted (the T45 safe twins).
fn assert_accepted(name: &str) {
    let report = analyze_file(&fixture(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
    let boxes: Vec<_> = report.sites.iter().filter(|s| s.kind == SiteKind::BoxOf).collect();
    assert!(!boxes.is_empty(), "{name}: the twin must have a Box.of site");
    for s in boxes {
        assert!(s.accepted(), "{name}: refused as '{}' at {:?}", s.reason(), s.span);
    }
}

/// Every fixture is named by the sink it must reach (the T15 refusals).
fn assert_refused_by(name: &str, sink: Sink) {
    let report = analyze_file(&fixture(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
    let boxes: Vec<_> = report.sites.iter().filter(|s| s.kind == SiteKind::BoxOf).collect();
    assert!(!boxes.is_empty(), "{name}: the fixture must have a Box.of site");
    for s in boxes {
        assert!(s.sinks.contains(&sink), "{name}: expected '{}', got {:?}", sink.name(), s.sinks);
    }
}

/// Every container site of one fixture is refused, and one of them reaches
/// the named sink (the T94 and T95 refusals).
fn assert_container_refused_by(name: &str, sink: Sink) {
    let report = analyze_file(&fixture(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
    let containers: Vec<_> = report.sites.iter().filter(|s| s.kind == SiteKind::Container).collect();
    assert!(!containers.is_empty(), "{name}: the fixture must have a container site");
    for s in &containers {
        assert!(!s.accepted(), "{name}: accepted at byte {}", s.span.start);
    }
    assert!(
        containers.iter().any(|s| s.sinks.contains(&sink)),
        "{name}: expected '{}', got {:?}",
        sink.name(),
        containers.iter().map(|s| &s.sinks).collect::<Vec<_>>()
    );
}

/// Every container site of one fixture is accepted (the T95 twins).
fn assert_container_accepted(name: &str) {
    let report = analyze_file(&fixture(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
    let containers: Vec<_> = report.sites.iter().filter(|s| s.kind == SiteKind::Container).collect();
    assert!(!containers.is_empty(), "{name}: the twin must have a container site");
    for s in containers {
        assert!(s.accepted(), "{name}: refused as '{}' at byte {}", s.reason(), s.span.start);
    }
}

#[test]
fn t45_a_block_that_returns_a_number_is_accepted() {
    assert_accepted("twin_returns_number.kp");
}

#[test]
fn t45_a_block_value_used_only_inside_the_block_is_accepted() {
    assert_accepted("twin_local_use.kp");
}

#[test]
fn t45_a_borrowing_method_with_a_plain_result_is_accepted() {
    assert_accepted("twin_method_plain_result.kp");
}

#[test]
fn t45_a_number_stored_in_an_outer_variable_is_accepted() {
    assert_accepted("twin_number_outer.kp");
}

#[test]
fn t94_a_list_returned_from_its_body_is_refused() {
    assert_container_refused_by("list_returned.kp", Sink::Returned);
}

#[test]
fn t94_b_list_moved_into_a_call_is_refused() {
    assert_container_refused_by("list_moved_into_call.kp", Sink::MovedIntoCall);
}

#[test]
fn t94_c_list_mutably_borrowed_by_a_call_is_refused() {
    assert_container_refused_by("list_mut_borrow_into_call.kp", Sink::MutableBorrowIntoCall);
}

#[test]
fn t94_d_list_captured_by_a_closure_is_refused() {
    assert_container_refused_by("list_captured.kp", Sink::CapturedByClosure);
}

#[test]
fn t94_e_list_stored_in_a_shared_pool_list_is_refused() {
    assert_container_refused_by("list_stored_in_shared_pool.kp", Sink::StoredInSharedPool);
}

#[test]
fn t94_f_list_passed_to_a_thread_spawn_is_refused() {
    assert_container_refused_by("list_spawn_argument.kp", Sink::SpawnArgument);
}

#[test]
fn t95_a_grow_call_in_a_defer_is_refused() {
    assert_container_refused_by("grow_in_defer.kp", Sink::GrowInDefer);
}

#[test]
fn t95_b_mutable_borrow_into_a_call_is_refused() {
    assert_container_refused_by("list_mut_borrow_into_call.kp", Sink::MutableBorrowIntoCall);
}

#[test]
fn t95_c_shared_borrow_with_a_plain_result_is_accepted() {
    assert_container_accepted("list_borrow_twin.kp");
}

#[test]
fn t95_d_grow_call_in_the_owning_block_is_accepted() {
    assert_container_accepted("list_block_twin.kp");
}

#[test]
fn t15_a_returned_box_is_refused_as_returned() {
    assert_refused_by("sink_returned.kp", Sink::Returned);
}

#[test]
fn t15_a_box_moved_into_a_call_is_refused() {
    assert_refused_by("sink_moved_into_call.kp", Sink::MovedIntoCall);
}

#[test]
fn t15_a_box_stored_in_a_parameter_is_refused() {
    assert_refused_by("sink_stored_outside.kp", Sink::StoredOutside);
}

#[test]
fn t15_a_box_captured_by_a_closure_is_refused() {
    assert_refused_by("sink_captured.kp", Sink::CapturedByClosure);
}

#[test]
fn t15_a_box_stored_in_a_container_is_refused() {
    assert_refused_by("sink_shared_pool.kp", Sink::StoredInSharedPool);
}

#[test]
fn t15_a_borrow_in_a_call_result_is_refused() {
    assert_refused_by("sink_borrow_result.kp", Sink::BorrowIntoResult);
}

#[test]
fn t15_a_box_in_the_error_payload_of_try_is_refused() {
    assert_refused_by("sink_error_payload.kp", Sink::ErrorPayload);
}

#[test]
fn t15_a_box_passed_to_a_spawn_is_refused() {
    assert_refused_by("sink_spawn.kp", Sink::SpawnArgument);
}

#[test]
fn t15_a_box_returned_through_a_local_is_refused() {
    assert_refused_by("sink_through_local.kp", Sink::Returned);
}

#[test]
fn a_box_set_and_a_closure_are_not_block_constructors() {
    let report = analyze_file(&fixture("kinds.kp")).unwrap_or_else(|e| panic!("{e}"));
    let mut seen = std::collections::BTreeSet::new();
    for s in &report.sites {
        // A container of numbers may be accepted (R57); a closure and a
        // Box.set never are.
        assert!(!s.accepted() || matches!(s.kind, SiteKind::BoxOf | SiteKind::Container), "{s:?}");
        seen.insert(s.kind);
    }
    assert!(seen.contains(&SiteKind::BoxSet), "{seen:?}");
    assert!(seen.contains(&SiteKind::Closure), "{seen:?}");
    assert!(seen.contains(&SiteKind::Container), "{seen:?}");
}

#[test]
fn box_reasons_helper_names_the_first_sink() {
    assert_eq!(box_reasons("sink_returned.kp"), vec![Sink::Returned.name()]);
}

/// The bench directory the dry run reads.
fn bench_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("bench")
}

/// The programs the dry run counts: every suite program, the speed program
/// and the 20 GB write program.
fn bench_programs() -> Vec<PathBuf> {
    let root = bench_root();
    let mut programs: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(root.join("suite")).expect("bench/suite") {
        let dir = entry.expect("entry").path();
        if let Ok(files) = std::fs::read_dir(&dir) {
            for f in files.flatten() {
                let p = f.path();
                if p.extension().is_some_and(|x| x == "kp") {
                    programs.push(p);
                }
            }
        }
    }
    programs.push(root.join("speed").join("speed.kp"));
    programs.push(root.join("write_20gb").join("write20gb.kp"));
    programs.sort();
    programs
}

/// The 16 sites of the step 2a dry run (commit 0646ae4): program, function,
/// callee and the byte where the site starts. T103 checks the denominator
/// against this list, so it cannot change between runs.
const BASELINE: [(&str, &str, &str, usize); 16] = [
    ("suite/binary-trees/binary-trees.kp", "make", "Box#of", 665),
    ("suite/binary-trees/binary-trees.kp", "make", "Box#of", 694),
    ("suite/fannkuch-redux/fannkuch-redux.kp", "main", "List#empty", 505),
    ("suite/fannkuch-redux/fannkuch-redux.kp", "main", "List#empty", 544),
    ("suite/fannkuch-redux/fannkuch-redux.kp", "main", "List#empty", 584),
    ("suite/hashmap/hashmap.kp", "main", "Map#new", 645),
    ("suite/json-tokenizer/json-tokenizer.kp", "main", "String#empty", 1198),
    ("suite/matmul/matmul.kp", "main", "List#empty", 559),
    ("suite/matmul/matmul.kp", "main", "List#empty", 595),
    ("suite/matmul/matmul.kp", "main", "List#empty", 631),
    ("suite/sort/sort.kp", "main", "List#empty", 2087),
    ("suite/sort-builtin/sort-builtin.kp", "main", "List#empty", 598),
    ("suite/spectral-norm/spectral-norm.kp", "main", "List#empty", 1323),
    ("suite/spectral-norm/spectral-norm.kp", "main", "List#empty", 1359),
    ("suite/spectral-norm/spectral-norm.kp", "main", "List#empty", 1397),
    ("suite/strings/strings.kp", "main", "String#empty", 544),
];

/// T103: the gate counts the same 16 sites, each container site records its
/// form and its element type, and the refusals add up to the total.
#[test]
fn t103_the_gate_counts_the_same_sites_and_its_total_adds_up() {
    let root = bench_root();
    let mut found: Vec<(String, String, String, usize)> = Vec::new();
    let mut total = Report::default();
    for p in bench_programs() {
        let name = p.strip_prefix(&root).unwrap_or(&p).display().to_string();
        let report = analyze_file(&p).unwrap_or_else(|e| panic!("{name}: {e}"));
        for s in &report.sites {
            found.push((name.clone(), s.function.clone(), s.callee.clone(), s.span.start));
        }
        total.sites.extend(report.sites);
    }
    found.sort();
    let mut expected: Vec<_> =
        BASELINE.iter().map(|(p, f, c, b)| (p.to_string(), f.to_string(), c.to_string(), *b)).collect();
    expected.sort();
    assert_eq!(found, expected, "the denominator changed since 0646ae4");
    for s in total.sites.iter().filter(|s| s.kind == SiteKind::Container) {
        assert!(!s.form.is_empty() && !s.element.is_empty(), "{s:?}");
    }
    let refused: usize = total.refused_by_reason().values().sum();
    assert_eq!(total.total(), total.accepted() + refused);
}

/// The dry run of step 2a, kept for step 2b. It prints the count per
/// program and fails only when a program does not check. The gate line is
/// judged by the caller. Run with:
/// `cargo test -p kelpc --bin kelpc escape -- --include-ignored --nocapture`.
#[test]
#[ignore]
fn dry_run_bench_sites() {
    let root = bench_root();
    let programs = bench_programs();

    let mut total = Report::default();
    let (mut sites, mut accepted) = (0usize, 0usize);
    for p in &programs {
        let name = p.strip_prefix(&root).unwrap_or(p).display().to_string();
        match analyze_file(p) {
            Ok(report) => {
                let share = if report.total() == 0 {
                    0.0
                } else {
                    100.0 * report.accepted() as f64 / report.total() as f64
                };
                eprintln!(
                    "DRYRUN {name}: sites {} accepted {} share {:.1}%",
                    report.total(),
                    report.accepted(),
                    share
                );
                eprintln!("DRYRUN   grow calls on accepted containers: {}", report.grow_calls_on_accepted());
                for (kind, (n, a)) in report.by_kind() {
                    eprintln!("DRYRUN   kind {kind}: sites {n} accepted {a}");
                }
                for (reason, n) in report.refused_by_reason() {
                    eprintln!("DRYRUN   refused {n}: {reason}");
                }
                for site in report.sites.iter().filter(|s| !s.accepted()) {
                    eprintln!(
                        "DRYRUN   site {} {} at byte {}: {}",
                        site.function,
                        site.callee,
                        site.span.start,
                        site.reason()
                    );
                }
                sites += report.total();
                accepted += report.accepted();
                total.sites.extend(report.sites);
            }
            Err(e) => eprintln!("DRYRUN {name}: does not check: {e}"),
        }
    }
    let share = 100.0 * accepted as f64 / sites.max(1) as f64;
    eprintln!("DRYRUN TOTAL grow calls on accepted containers: {}", total.grow_calls_on_accepted());
    eprintln!("DRYRUN TOTAL: programs {} sites {sites} accepted {accepted} share {share:.1}%", programs.len());
    for (reason, n) in total.refused_by_reason() {
        eprintln!("DRYRUN TOTAL refused {n}: {reason}");
    }
    for (kind, (n, a)) in total.by_kind() {
        eprintln!("DRYRUN TOTAL kind {kind}: sites {n} accepted {a}");
    }
}
