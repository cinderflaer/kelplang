use super::*;

// ---------------------------------------------------------------------------
// Evaluation order: every call argument, template interpolation, operator
// operand and aggregate field is evaluated strictly left to right on both
// backends (`design/syntax.md (kelp wiki)`, section 5). The fixtures record the call order
// into a passdown and print it, so a C compiler's own order shows up as a diff.
// ---------------------------------------------------------------------------

#[test]
pub(crate) fn eval_order_template_interpolations_run_left_to_right() {
    assert_fixture_runs("tests/fixtures/eval_order_template.kp");
}

#[test]
pub(crate) fn ordered_binary_conditions_run_left_to_right() {
    // An effectful binary operation evaluates its left operand first. In a condition that has to hold whatever the operand's shape.
    assert_fixture_runs("tests/fixtures/eval_order_condition.kp");
}

#[test]
pub(crate) fn eval_order_basic_transcript_is_left_to_right() {
    assert_fixture_runs("tests/fixtures/eval_order_basic.kp");
}

#[test]
pub(crate) fn eval_order_aggregates_operators_and_indirect_calls_are_left_to_right() {
    assert_fixture_runs("tests/fixtures/eval_order_more.kp");
}

// ---------------------------------------------------------------------------
// Several lock-typed `pd val` globals in one program. The LLVM module
// initialiser used to restart every global in the entry block, leaving the
// first global's `alloc.ok` block without a terminator.
// ---------------------------------------------------------------------------

#[test]
pub(crate) fn two_mutex_a_rwlock_and_a_condvar_global_initialise_together() {
    assert_fixture_runs("tests/fixtures/pd_lock_globals_pair.kp");
}

#[test]
pub(crate) fn six_lock_globals_are_used_from_several_threads() {
    assert_fixture_runs("tests/fixtures/pd_lock_globals_many.kp");
}

// ---------------------------------------------------------------------------
// `Result<unit, E>` has a constructible success value (MISSING-n6Bsc): `()` is
// the absent value, and `Ok(value = ())` builds the success arm, which returns
// from a function and matches like any other `Ok`.
// ---------------------------------------------------------------------------

#[test]
pub(crate) fn result_of_unit_builds_its_success_arm_from_unit_and_matches_it() {
    assert_fixture_runs("tests/fixtures/result_unit_ok.kp");
}

#[test]
pub(crate) fn unit_is_not_stored_in_a_tuple_even_though_unit_is_a_value() {
    // `()` is a value now, but the absent value is still not an element: a
    // tuple element is field-like, the same rule a `datac` field obeys.
    assert_rejected_with(
        "tests/fixtures/tuple_unit_element_value.kp",
        "produces no value ('unit')",
    );
}

/// `std/path.split` on its edge cases: leading, trailing and doubled separators,
/// empty and separator-only paths, non-ASCII components, and a 2000-component
/// path. The components are pinned to the 26.10.4 output.
#[test]
pub(crate) fn the_path_split_edges_are_unchanged() {
    assert_fixture_runs("tests/fixtures/path_split_edges.kp");
}

/// `std/encoding` base64 on its edge cases: the RFC 4648 vectors, every byte
/// value (so all 64 alphabet characters appear), a 3001-byte round trip, and the
/// decode failures. The encoder's character lookup changed; the text it writes
/// is pinned to the 26.10.4 output.
#[test]
pub(crate) fn the_base64_edges_are_unchanged() {
    assert_fixture_runs("tests/fixtures/base64_edges.kp");
}

/// `str.from_f64` and `fmt.float` on zero and negative zero, rounding at the last
/// digit, a huge magnitude and 200 decimals (both longer than the stack buffer the
/// C body formats into), NaN and the infinities. The text is pinned to the
/// 26.10.4 output.
#[test]
pub(crate) fn the_float_formatting_edges_are_unchanged() {
    assert_fixture_runs("tests/fixtures/float_format_edges.kp");
}

/// `std/encoding` base32 on its edge cases: the RFC 4648 vectors, every byte value
/// (so all 32 alphabet characters appear), each padding length, and the decode
/// failures. The encoder's character lookup changed; the text it writes is pinned
/// to the 26.10.4 output.
#[test]
pub(crate) fn the_base32_edges_are_unchanged() {
    assert_fixture_runs("tests/fixtures/base32_edges.kp");
}

/// `kelpc doc` never writes an em dash (U+2014) or an en dash (U+2013) into a
/// generated page, whatever the source says. The fixture puts both dashes, and
/// the HTML entities that display them, on every doc path: a module doc, a
/// constant, a static, a passdown, a datac with field docs, an enum with variant
/// docs, a trait with its items, a trait impl, a function and a method. Every
/// format and visibility is generated, and every file of every run is searched.
#[test]
pub(crate) fn the_doc_generator_never_emits_an_em_or_en_dash() {
    let src = fixture("tests/fixtures/doc_no_dashes.kp");
    let base = std::env::temp_dir().join(format!("kelpc_doc_nodash_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let mut pages = 0;
    for (format, private) in [("md", false), ("md", true), ("html", false), ("html", true)] {
        let name = format!("{format}{}", if private { "-private" } else { "" });
        let out = base.join(&name);
        let mut cmd = kelpc();
        cmd.arg("doc").arg(&src).args(["--format", format]);
        if private {
            cmd.arg("--private");
        }
        cmd.arg("-o").arg(&out);
        let r = cmd.output().expect("spawn kelpc");
        assert!(r.status.success(), "kelpc doc {name} failed: {}", String::from_utf8_lossy(&r.stderr));
        let mut saw_fixture_prose = false;
        for entry in std::fs::read_dir(&out).expect("read the output directory") {
            let path = entry.expect("read a directory entry").path();
            let text = std::fs::read_to_string(&path).expect("read a generated page");
            for dash in ['\u{2014}', '\u{2013}'] {
                assert!(
                    !text.contains(dash),
                    "{name}: {} contains U+{:04X}",
                    path.display(),
                    dash as u32
                );
            }
            for entity in ["&mdash;", "&ndash;", "&#8212;", "&#8211;"] {
                assert!(!text.contains(entity), "{name}: {} contains {entity}", path.display());
            }
            if text.contains("The reading") {
                saw_fixture_prose = true;
            }
            pages += 1;
        }
        assert!(saw_fixture_prose, "{name}: the fixture's doc comments were not generated");
    }
    assert!(pages >= 8, "expected at least 8 generated pages, got {pages}");
    let _ = std::fs::remove_dir_all(&base);
}

// Tidepool step 2a, T45: safe twins. Each is an ordinary program that compiles
// and prints the expected line, and the escape analysis (kelpc/src/escape.rs)
// accepts its block values. Their refusal twins are unit tests in that file.
#[test]
pub(crate) fn escape_twin_block_value_returns_a_number() {
    assert_fixture_runs("tests/escape/twin_returns_number.kp");
}

#[test]
pub(crate) fn escape_twin_block_value_used_only_inside_its_body() {
    assert_fixture_runs("tests/escape/twin_local_use.kp");
}

#[test]
pub(crate) fn escape_twin_borrowing_method_with_a_plain_result() {
    assert_fixture_runs("tests/escape/twin_method_plain_result.kp");
}

#[test]
pub(crate) fn escape_twin_number_stored_in_an_outer_variable() {
    assert_fixture_runs("tests/escape/twin_number_outer.kp");
}

// Tidepool step 2, T34: manifest refusals. The `[tidepool]` block is not read
// yet, so each project below builds, and every test here fails until step 5.
// The needles are the golden texts of the design, section 14.7 (G14).
// RED (job 59c3aa60): build succeeds; [tidepool] is skipped by parse_manifest.
#[test]
pub(crate) fn tidepool_heap_max_under_tidepool_is_refused_by_name() {
    assert_project_rejected(
        "tidepoolheapmaxblock",
        &project("tidepoolheapmaxblock"),
        "coral.toml's [tidepool] key 'heap_max' belongs in [target]; move it there",
    );
}

// RED (job 59c3aa60): build succeeds; unknown header [TIDEPOOL] is skipped silently.
#[test]
pub(crate) fn tidepool_misspelled_header_is_refused_with_a_did_you_mean_line() {
    assert_project_rejected(
        "tidepoolmisspelled",
        &project("tidepoolmisspelled"),
        "unknown section '[TIDEPOOL]' in coral.toml; did you mean '[tidepool]'?",
    );
}

// RED (job 59c3aa60): build succeeds; unknown header [gc] is skipped silently.
#[test]
pub(crate) fn tidepool_old_gc_header_is_refused_with_a_did_you_mean_line() {
    assert_project_rejected(
        "tidepoolstaleheader",
        &project("tidepoolstaleheader"),
        "unknown section '[gc]' in coral.toml; did you mean '[tidepool]'?",
    );
}

// Tidepool step 2, T41: `heap_max` parsing (R37). The refusals already exist
// on this branch (`parse_target_line` in kelpc/src/resolve.rs), so these three
// may pass; they are kept as guards. The default of 256 MiB is the marker below.
// GREEN on this branch (job 59c3aa60): the refusal exists in parse_target_line; kept as a guard.
#[test]
pub(crate) fn tidepool_heap_max_quoted_value_is_refused() {
    assert_project_rejected(
        "tidepoolheapmaxquoted",
        &project("tidepoolheapmaxquoted"),
        "coral.toml value for '[target] heap_max' must be a whole number of MiB between 1 and 65536, found '\"64\"'",
    );
}

// GREEN on this branch (job 59c3aa60): the refusal exists in parse_target_line; kept as a guard.
#[test]
pub(crate) fn tidepool_heap_max_zero_is_refused() {
    assert_project_rejected(
        "tidepoolheapmaxzero",
        &project("tidepoolheapmaxzero"),
        "coral.toml value for '[target] heap_max' must be between 1 and 65536 MiB, found 0",
    );
}

// GREEN on this branch (job 59c3aa60): the refusal exists in parse_target_line; kept as a guard.
#[test]
pub(crate) fn tidepool_heap_max_65537_is_refused() {
    assert_project_rejected(
        "tidepoolheapmax65537",
        &project("tidepoolheapmax65537"),
        "coral.toml value for '[target] heap_max' must be between 1 and 65536 MiB, found 65537",
    );
}

// Marker (job 59c3aa60): reported as ignored; it has no body to run yet.
#[test]
#[ignore = "Tidepool step 2 marker: the 256 MiB default is observed through the budget, which needs step 1's kelp-gc-stats hook"]
pub(crate) fn tidepool_default_heap_max_is_256_mib() {
    unimplemented!("needs step 1's kelp-gc-stats hook; no test body yet");
}

// Tidepool step 2, T42: `[tidepool]` key checks (R38). Each message is G14 in
// section 14.7.
// RED (job 59c3aa60): build succeeds; unknown [tidepool] key is not read.
#[test]
pub(crate) fn tidepool_key_one_letter_off_is_refused_by_name() {
    assert_project_rejected(
        "tidepoolkeytypo",
        &project("tidepoolkeytypo"),
        "unknown key 'headrom_pct' in coral.toml's [tidepool] section; its keys are 'headroom_pct', 'start_low_mib', 'start_high_mib' and 'trigger_rule'",
    );
}

// Tidepool trigger rule: a name no rule has is refused by name, with the names
// it may take (design section 4, Challenge 2).
#[test]
pub(crate) fn tidepool_trigger_rule_unknown_name_is_refused_by_name() {
    assert_project_rejected(
        "tidepoolrulename",
        &project("tidepoolrulename"),
        "coral.toml's [tidepool] key 'trigger_rule' names no rule 'mean'; its rules are 'two-size', 'last-size' and 'gogc-style'",
    );
}

// RED (job 59c3aa60): build succeeds; [tidepool] values are not read.
#[test]
pub(crate) fn tidepool_string_value_is_refused() {
    assert_project_rejected(
        "tidepoolstringvalue",
        &project("tidepoolstringvalue"),
        "coral.toml's [tidepool] key 'headroom_pct' must be a whole number, found '\"20\"'",
    );
}

// RED (job 59c3aa60): build succeeds; [tidepool] values are not read.
#[test]
pub(crate) fn tidepool_start_low_zero_is_refused() {
    assert_project_rejected(
        "tidepoolstartlowzero",
        &project("tidepoolstartlowzero"),
        "coral.toml's [tidepool] key 'start_low_mib' must be at least 1, found '0'",
    );
}

// Tidepool settings (manifest refusals by name, each with a fixture). A
// section about Tidepool under any other name is refused, not skipped.
#[test]
pub(crate) fn tidepool_section_under_target_is_refused_by_name() {
    assert_project_rejected(
        "tidepoolsectionname",
        &project("tidepoolsectionname"),
        "unknown section '[target.tidepool]' in coral.toml; did you mean '[tidepool]'?",
    );
}

#[test]
pub(crate) fn tidepool_double_bracket_header_is_refused_by_name() {
    assert_project_rejected(
        "tidepoolarraytable",
        &project("tidepoolarraytable"),
        "unknown section '[[tidepool]]' in coral.toml; did you mean '[tidepool]'?",
    );
}

#[test]
pub(crate) fn tidepool_headroom_pct_above_1000_is_refused_by_name() {
    assert_project_rejected(
        "tidepoolheadroomrange",
        &project("tidepoolheadroomrange"),
        "coral.toml's [tidepool] key 'headroom_pct' must be from 0 to 1000, found '1001'",
    );
}

#[test]
pub(crate) fn tidepool_start_high_above_heap_max_is_refused_by_name() {
    assert_project_rejected(
        "tidepoolstarthighover",
        &project("tidepoolstarthighover"),
        "coral.toml's [tidepool] key 'start_high_mib' must not exceed [target] heap_max (8 MiB), found '9'",
    );
}

#[test]
pub(crate) fn tidepool_duplicate_key_is_refused_by_name() {
    assert_project_rejected(
        "tidepoolduplicate",
        &project("tidepoolduplicate"),
        "'headroom_pct' appears more than once in coral.toml's [tidepool] section",
    );
}

/// The `define` bodies of one unit's IR, as `(header, body lines)`.
pub(crate) fn ir_function_bodies(text: &str) -> Vec<(String, Vec<String>)> {
    let mut out = Vec::new();
    let mut current: Option<(String, Vec<String>)> = None;
    for line in text.lines() {
        if line.starts_with("define ") {
            current = Some((line.to_string(), Vec::new()));
        } else if line == "}" {
            if let Some(done) = current.take() {
                out.push(done);
            }
        } else if let Some((_, body)) = current.as_mut() {
            body.push(line.trim_start().to_string());
        }
    }
    out
}

/// Decision 125, item 8: a spawning program gives every function with a body one
/// cleanup frame, entered once at its start and left once before each return.
/// Checked on the unoptimized unit IR of a fixture that spawns.
#[test]
pub(crate) fn spawn_frames_enter_once_and_leave_before_every_return() {
    let units = std::env::temp_dir().join(format!("kelpc-frames-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&units);
    std::fs::create_dir_all(&units).expect("create the units directory");
    let bin = units.join("prog");
    let build = kelpc()
        .env("KELPC_DUMP_UNITS", &units)
        .env("KELPC_CACHE_DIR", units.join("cache"))
        .arg("build")
        .arg(fixture("tests/fixtures/thread_spawn_closure.kp"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));

    let mut framed = 0;
    for entry in std::fs::read_dir(&units).expect("read the units directory") {
        let path = entry.expect("directory entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("ll") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("read a unit's IR");
        for (header, body) in ir_function_bodies(&text) {
            let enters = body.iter().filter(|l| l.starts_with("call void @kelp_cleanup_enter(")).count();
            let leaves = body.iter().filter(|l| l.starts_with("call void @kelp_cleanup_leave(")).count();
            let rets = body.iter().filter(|l| l.starts_with("ret ")).count();
            if enters == 0 {
                assert_eq!(leaves, 0, "a leave without an enter in {header}");
                continue;
            }
            framed += 1;
            assert_eq!(enters, 1, "one enter per function in {header}");
            assert_eq!(leaves, rets, "a leave before every return in {header}");
        }
    }
    assert!(framed > 0, "a spawning program emits at least one framed function");
    let _ = std::fs::remove_dir_all(&units);
}
