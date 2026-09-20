//! Coverage and refusal ranking for the `asm` backend (MK7 §5).
//!
//! `kelpc/tests/compile_and_run.rs` is the fixture suite and is backend-agnostic
//! by construction: it compiles a fixture with whichever backend `KELPC_BACKEND`
//! names and compares the run to a literal an author wrote. This file is the
//! other half of `plan/mk7-own-backend.md` §1.1 — it always builds *both*
//! backends and compares them to each other, so the C backend is an oracle
//! rather than a second opinion, and it records how much of the fixture corpus
//! the asm backend actually accepts so the number in the plan's ranking tables
//! cannot drift silently.
//!
//! Three things live here, all of them runnable and none of them C-specific:
//!
//! 1. The differential sweep: every fixture the C backend builds is built with
//!    `--backend=asm` too. If the asm backend builds it, both binaries are run
//!    and their stdout and exit status must be byte-identical. If the asm
//!    backend refuses it, the refusal message must begin with the exact prefix
//!    `REFUSAL_PREFIX` below — a fixture can never be refused for a reason the
//!    backend does not name, which is §1's refusal-over-silence discipline.
//! 2. A coverage ratchet: the sorted list of fixtures the asm backend builds is
//!    recorded in this file as `ASM_BUILT_BASELINE`, and the sweep asserts
//!    today's set is a superset of it.
//! 3. A ranking report: the refusals grouped into the categories §5's table
//!    uses, with counts and the fixtures in each. It is a `println!`, never an
//!    assertion, so a fixture moving between categories cannot break a run.
//!
//! **Ground rule 1.5** (`plan/mk7-own-backend.md`): the cheap oracle stays
//! cheap, and *nothing* is allowed to make the C backend's build or test time
//! worse. The sweep above builds 127 fixtures twice and runs 37 pairs, which
//! takes on the order of half a minute — far too much to add to the default
//! `cargo test`. Every test that touches the sweep is therefore `#[ignore]`d,
//! and the default suite compiles this file and runs only the cheap
//! baseline-shape check. Run the heavy ones explicitly:
//!
//! ```text
//! cargo test --test backend_coverage -- --ignored --nocapture
//! KELPC_BACKEND=asm cargo test --test backend_coverage -- --ignored --nocapture
//! ```
//!
//! The second command is the same sweep with the suite's own backend variable
//! pointed at `asm`: these tests pass the `--backend=` flag explicitly so the
//! environment cannot change what they measure, and the `--backend=asm` run is
//! here to prove exactly that.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;

/// The exact prefix every `asm` refusal must carry (`codegen_asm.rs::unsupported`
/// plus the driver's `error:` lead-in). Nothing else may be the first thing a
/// refused fixture's build prints.
const REFUSAL_PREFIX: &str = "error: the 'asm' backend does not support";

/// The fixtures the asm backend built when this ratchet was recorded — the
/// sorted output of the differential sweep at that commit.
///
/// **Re-record this list whenever a slice legitimately adds or removes a
/// fixture.** A slice that covers a new feature turns present-but-refused
/// fixtures into built ones (a superset, so this list only needs extending and
/// the sweep's own note says so), and a slice that adds a fixture the asm
/// backend is not expected to build yet does not change it at all. Removing an
/// entry is the one edit that can hide a regression, so the sweep reports which
/// fixtures it builds but this list does not, and the ratchet only fails on the
/// opposite direction.
///
/// The counts this list was recorded against: 187 fixtures the C backend
/// builds, 167 of them also built by the asm backend, 20 refused: which is the
/// 250-of-295 integration-test ratio in the plan's §0 table, measured at the
/// fixture level instead of the test level. (174/154 before the
/// field-of-a-temporary slice moved `heap_field_of_owned_temporary` into the
/// C backend's built column and the four names the previous run's note listed
/// (`else_if_let`, `list_clone`, `panic_bound_message`,
/// `panic_owned_message`) were recorded with it; see the re-recording notes
/// below.)
///
/// The last re-recording is the scrutinee-staging slice: `when` and `if let`
/// now stage a temporary scrutinee, and the *root* of a field chain over one,
/// into a local of its own, which is a plain `Stmt::Let` in TIR and so needs no
/// backend work at all. `enum_temp_scrutinee` left the refusal column with it,
/// and `scrutinee_field_of_temporary` is new: 184/164/20 -> 186/166/20. The
/// same note had listed `documentation_comment`, which the documentation-comment
/// slice built but did not record, so it is recorded here too and the counts
/// move once more: 186/166/20 -> 187/167/20.
///
/// Before it, the last re-recording is the `isize` slice: `isize_kinds` covers every
/// position the type can appear in, and both backends build it now that the C
/// preamble includes `<stddef.h>`: 183/163/20 -> 184/164/20.
///
/// Before it, the last re-recording was the fallible-conversion slice: `try_to_conversion`
/// is a plain Kelp module over `Option<u8>`/`i32`/`u32`/`u64`. `infer.rs`
/// lowers the builtin into a `Block`/`If`/`EnumLit` shape, so both backends
/// lower it with no conversion-specific code: 182/162/20 -> 183/163/20.
///
/// Before it, the last re-recording was the stdlib-encoding slice:
/// `stdlib_encoding` is a plain Kelp module over `Bytes`, `String`, `List<u8>`
/// and `Result`, all of which both backends already build, so it joins the
/// baseline and the counts move 181/161/20 -> 182/162/20.
///
/// Before it, the last re-recording was the literal-syntax slice: `int_literals` and
/// `byte_literals` are the first fixtures for `0x`/`0b` integer literals and
/// for byte literals, and both are ordinary narrow-integer programs, so both
/// backends build them: 179/159/20 -> 181/161/20.
///
/// Before it, the last re-recording was the field-of-a-temporary slice:
/// `heap_field_of_owned_temporary` left the C backend's rejected column: it
/// stages the temporary a borrowed field lives in and frees *it* after the
/// call, which the asm backend has always done. So the fixture is in the
/// differential sweep now, and with the four names the previous run's note had
/// listed the list stands at 159 entries and the measurement at 179/159/20.
/// Before it, the last re-recording was the sort/contains/slice slice: `stdlib_sort`,
/// `list_slice_insert` and `list_slice_bounds` left the heap row (three of its
/// twelve, now nine) and `list_sort_kinds` is new, so 173/150/23 ->
/// 174/154/20. That slice's own gate left one entry out and the sweep's note
/// said so: `heap_temporary_print` builds since the print slice, and the
/// supervisor recorded it here, which is the 153rd and 154th entry of this
/// list.
///
/// Before that, the print slice turned `heap_temporary_print` from a refusal
/// into a built fixture (the heap row went 13 -> 12) and added
/// `template_buffer_pieces` for the register numbering a `%.*s` interpolation
/// needs: 172/148/24 -> 173/150/23.
///
/// Before that, the drop-plan defect's fix added `owned_conditional_move` and
/// `owned_move_in_loop`: both are ordinary owned values moved on one path and
/// not another, so both backends build them, and the denominator moved from
/// 170 to 172 with the refused count unchanged at 24.
///
/// (This paragraph said 153/106/47 while the list beneath it had grown to 129
/// entries, which the plan's §5 blockquotes had already recorded as 160/129/31;
/// corrected here, at 163/132/31, in the commit that added `heap_narrow_fields`,
/// `list_narrow_element_pop` and `try_narrow_payload`. It then said 163/132/31
/// while the list had grown to 146 entries and the real measurement was
/// 170/146/24 — the float-aggregate slice and the temporary-drop slice each
/// added fixtures — corrected here, in the commit that recorded
/// `heap_field_of_temporary`.)
///
/// Re-recorded nine times so far, and the *reason* each time is worth keeping:
/// the first heap slice added `bytes_build`, `heap_branches`, `heap_bytes`,
/// `heap_drops_in_control_flow`, `heap_move_then_rebind`, `heap_param_borrow`,
/// `heap_reassign`, `heap_returned`, `heap_scope_drop` and `stdlib_mem` (37
/// entries at 135-of-248 before it, with `int_mod_narrow` arriving in the same
/// commit as the `%` fix its probe forced); the String/`View` slice added
/// `string_basic`, `string_build`, `string_view_basic`, `stdlib_str`,
/// `stdlib_path`, `path_parts`, `stdlib_fmt`, `literal_operand_typing`,
/// `float_integral_literal` and `view_released_at_scope_end` — nine fixtures
/// that were refused for a `String` type they no longer mention, plus the one
/// fixture the slice added to pin `View`'s register-pair ABI; and two more that
/// close the two bugs the heap spec's uncertainty list and the `%` probe turned
/// up — `int_literal_width` and `string_with_capacity_zero`, both fixed in the
/// C backend and in this one in the same commit that recorded them here; and the
/// heap-field slice added `string_field`, `generic_method_basic` — which was
/// refused only for the `String`-instantiated `Pair` it declares, so a generic
/// `datac` over owned fields moved with it for free — and
/// `datac_owned_fields`, the fixture that slice wrote to pin a `datac` holding a
/// `String`, a `Bytes` and a nested owning `datac` at once; and the box slice
/// added `box_recursive` and `box_empty_panic`, the two fixtures the builtin
/// owning pointer had been keeping out, plus `box_owned_elements`, that slice's
/// own fixture for replacing and taking an owned element; and the list slice
/// added nineteen — the fourteen `List` fixtures the heap row had been carrying
/// (`box_basic` and `box_field_take`, held out by a boxed `List`; `for_loop`,
/// `generic_function_basic`, `move_param`, `path_polish`, `recursive_datac`,
/// `stdlib_list` and `view`, each refused for a `List` type inside its own
/// shape; and `list_basic`, `list_datac_element`, `list_owned`,
/// `list_pop_empty` and `list_owned_replacement`) plus the five that slice wrote
/// for the element widths, the growth policy, the shifting operations and the
/// two panic messages nothing else reached (`list_element_widths`,
/// `list_growth_and_rebuild`, `list_insert_remove_shifts`, `list_insert_bounds`,
/// `list_remove_bounds`). What the list row still refuses is `sort`/`contains`/
/// `slice` (`list_slice_bounds`, `list_slice_insert`, `stdlib_sort`), an owning
/// passdown (`list_field`, `pd_owned`), and the `List<String>` a `str` coercion
/// feeds (`str_to_string_coercion`); and the map slice added twelve — five of
/// the six fixtures the heap row had been carrying for `Map` (`map_basic`,
/// `map_key_types`, `map_missing_key`, `map_owned`, `map_remove_missing`, but
/// **not** `str_to_string_coercion`, which is still refused for an `if` in value
/// position producing a `MEMORY`-class value and for an `enum`, neither of them
/// a `Map`), plus `map_datac` from the `datac` row — the slice resolved a `Map`
/// field through `struct_layout`, so the `FieldEntry::Aggregate` path placed it
/// with no new ABI code — plus the six fixtures the slice wrote for the growth
/// and rehash policy, tombstones and their reuse, the entry key widths, a
/// replace that must drop the old value, the whole owned round trip, and a clear
/// that is followed by reuse (`map_clear_then_reuse`, `map_growth_and_rehash`,
/// `map_key_widths`, `map_owned_round_trip`, `map_replace_drops_old_value`,
/// `map_tombstones_and_reuse`). That is also why the C denominator moved from
/// 139 to 145 rather than staying fixed; and the passdown borrow-mode slice
/// added one, `pd_write_reaches_callers` — a `pd var` write reached through a
/// chain, which this backend builds — and two it refuses for the owned-passdown
/// `atexit` handler `pd_owned` is already waiting on
/// (`pd_var_read_only_shared`, `pd_var_read_only_datac`), which is why the C
/// denominator moved to 152 and the refusal count to 52; `pd_var_mut_argument`
/// is that slice's third fixture, a passdown handed to a `mut` parameter, and
/// this backend builds it too. The passdown *drop* slice then built the rest of
/// the passdown row — `pd_owned`, `list_field` and `pd_argument_forwarding`,
/// which were waiting on the `atexit` handler, plus the two read-only fixtures
/// above — so the baseline gained five (and `block_comments`, from the block
/// comment slice), the built count moved to 105 and the refusal count to 47;
/// `pd_var_datac_reassign` is that slice's own fixture for an aggregate write
/// through a borrow, which took the built count to 106. The scalar-borrow slice
/// added `borrow_provenance_and_arms` and `stdlib_build` — two of the three
/// fixtures the plan had credited it with, and both reachable now because the
/// enum slice already landed behind their first refusal and the `str` parsers'
/// `mut` out-parameter is what this slice supplies — plus the two fixtures the
/// slice wrote for the mechanism itself (`scalar_borrow`, an integer out-parameter
/// with a pointer held across a call, and `scalar_borrow_f64`, the same borrow
/// with a float pointee). `stdlib_parse` is the third of the credited three and
/// is **not** here: with the parsers lowerable its first refusal is
/// `Result<f64, ParseErr>`, an `enum` whose payload is a float, which is the
/// float-aggregate row rather than this one. The enum slice's own defect fix
/// (a one-eightbyte aggregate field or element loaded as a whole `QWORD`, and a
/// `?` with a one-eightbyte `datac` payload in a one-eightbyte position) added
/// the three fixtures that pin it — `heap_narrow_fields`,
/// `list_narrow_element_pop` and `try_narrow_payload` — none of which any
/// existing fixture covered, which is why the denominator moved to 163 and the
/// refused count stayed at 31.
const ASM_BUILT_BASELINE: &[&str] = &[
    "aliases",
    "asm_float_print",
    "asm_float_scalars",
    "assert_fail",
    "bitwise",
    "block_comments",
    "borrow_basic",
    "borrow_provenance_and_arms",
    "borrow_temp_arg",
    "borrow_temp_slots",
    "box_basic",
    "box_empty_panic",
    "box_field_take",
    "box_owned_elements",
    "box_recursive",
"byte_literals",
"bytes_build",
    "char_literals",
    "consts",
    "datac_basic",
    "datac_embeds_datac",
    "datac_memory_class",
    "datac_memory_class_by_value",
    "datac_owned_fields",
    "datac_two_eightbytes",
    "documentation_comment",
    "else_if_let",
    "enum_basic",
    "enum_container",
    "enum_in_containers",
    "enum_layout",
    "enum_liveness",
    "enum_owned",
    "enum_recursive",
    "enum_temp_scrutinee",
    "escapes",
    "field_read_does_not_move",
    "float_conversions",
    "float_datac_boundary",
    "float_datac_containers",
    "float_datac_memory",
    "float_enum_register_file",
    "float_field_write",
    "float_integral_literal",
    "float_nonfinite_literal",
    "for_loop",
    "generic_explicit_args",
    "generic_function_basic",
    "generic_function_owned",
    "generic_method_basic",
    "generic_nested_inference",
    "generic_owned",
    "generic_recursive",
    "generic_type_basic",
    "heap_branches",
    "heap_bytes",
    "heap_drops_in_control_flow",
    "heap_drops_in_desugared_blocks",
    "heap_field_of_owned_temporary",
    "heap_field_of_temporary",
    "heap_move_then_rebind",
    "heap_narrow_fields",
    "heap_param_borrow",
    "heap_reassign",
    "heap_returned",
    "heap_scope_drop",
    "heap_temporary_print",
    "if_expr",
    "if_let_basic",
    "if_let_chain_same_scrutinee",
    "int_conversion_overflow",
    "int_conversions",
"int_literal_width",
"int_literals",
    "int_mod_narrow",
    "isize_kinds",
    "list_basic",
    "list_clone",
    "list_datac_element",
    "list_element_widths",
    "list_field",
    "list_growth_and_rebuild",
    "list_insert_bounds",
    "list_insert_remove_shifts",
    "list_narrow_element_pop",
    "list_owned",
    "list_owned_replacement",
    "list_pop_empty",
    "list_remove_bounds",
    "list_slice_bounds",
    "list_slice_insert",
    "list_sort_kinds",
    "literal_operand_typing",
    "map_basic",
    "map_clear_then_reuse",
    "map_datac",
    "map_growth_and_rehash",
    "map_key_types",
    "map_key_widths",
    "map_missing_key",
    "map_owned",
    "map_owned_round_trip",
    "map_remove_missing",
    "map_replace_drops_old_value",
    "map_tombstones_and_reuse",
    "move_param",
    "move_param_return",
    "move_then_reinit",
    "named_args",
    "nested_literal_staging",
    "nested_while_registers",
    "owned_conditional_move",
    "owned_move_in_loop",
    "panic_bound_message",
    "panic_msg",
    "panic_owned_message",
    "panic_tail",
    "passdown_datac",
    "passdown_globals",
    "passdowns",
    "path_parts",
    "path_polish",
    "pd_argument_forwarding",
    "pd_owned",
    "pd_shadowed",
    "pd_var_copy_read_twice",
    "pd_var_datac_reassign",
    "pd_var_mut_argument",
    "pd_var_read_only_datac",
    "pd_var_read_only_shared",
    "pd_var_write",
    "pd_write_reaches_callers",
    "receiver_methods",
    "recursive_datac",
    "scalar_borrow",
    "scalar_borrow_f64",
    "scrutinee_field_of_temporary",
    "std_str_order",
    "stdlib_build",
    "stdlib_encoding",
    "stdlib_fmt",
    "stdlib_list",
    "stdlib_math",
    "stdlib_mem",
    "stdlib_option_result",
    "stdlib_parse",
    "stdlib_path",
    "stdlib_sort",
    "stdlib_str",
    "str_to_string_coercion",
    "string_basic",
    "string_build",
    "string_field",
    "string_view_basic",
    "string_views",
    "string_with_capacity_zero",
    "template_buffer_pieces",
    "templates",
    "try_frame_drops",
    "try_narrow_payload",
    "try_operator",
    "try_to_conversion",
    "view",
    "view_released_at_scope_end",
    "when_expr",
    "when_in_args",
    "while_accumulator",
];

fn kelpc_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_kelpc"))
}

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures")
}

/// Where the sweep's two binaries per fixture are written. The backend is part
/// of the name so the C and asm binaries cannot race for one path, and the path
/// is rooted in `CARGO_TARGET_TMPDIR` the way `compile_and_run.rs`'s `out_path`
/// is.
fn out_path(tag: &str) -> PathBuf {
    let mut p = PathBuf::from(std::env::var("CARGO_TARGET_TMPDIR").unwrap_or_else(|_| {
        std::env::temp_dir().to_string_lossy().to_string()
    }));
    p.push(format!("backend_coverage_{tag}"));
    p
}

/// Builds one fixture with one backend, pinned by flag rather than by
/// environment, so the sweep's two halves cannot disagree about which backend
/// produced which binary (`compile_and_run.rs`'s `kelpc()` and the differential
/// test there make the same choice).
fn build(backend: &str, source: &Path, bin: &Path) -> Output {
    Command::new(kelpc_bin())
        .args(["build", &format!("--backend={backend}")])
        .arg(source)
        .arg("-o")
        .arg(bin)
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn kelpc to build '{}': {e}", source.display()))
}

struct Run {
    code: Option<i32>,
    stdout: Vec<u8>,
}

fn run(bin: &Path) -> Run {
    let out = Command::new(bin)
        .output()
        .unwrap_or_else(|e| panic!("failed to run '{}': {e}", bin.display()));
    Run {
        code: out.status.code(),
        stdout: out.stdout,
    }
}

/// Everything the sweep observes, computed once and shared by the heavy tests
/// (see `sweep`). Nothing here is asserted against, because the same data backs
/// three different tests and only one of them cares about any given field.
struct Sweep {
    /// Fixtures the C backend builds, sorted.
    c_built: Vec<String>,
    /// Fixtures the asm backend also builds, sorted.
    asm_built: Vec<String>,
    /// `(fixture, stderr)` for every fixture the C backend builds and the asm
    /// backend refuses.
    refusals: Vec<(String, String)>,
    /// One human-readable line per fixture both backends build but disagree on.
    disagreements: Vec<String>,
}

/// The sweep, run at most once per process.
///
/// The three heavy tests all need the same expensive data, and the test
/// harness runs tests in parallel threads in one process, so a `OnceLock` here
/// makes `--ignored` run the sweep once rather than three times while leaving
/// each test free to assert only its own concern.
fn sweep() -> &'static Sweep {
    static SWEEP: OnceLock<Sweep> = OnceLock::new();
    SWEEP.get_or_init(run_sweep)
}

fn run_sweep() -> Sweep {
    let dir = fixture_dir();
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("failed to read '{}': {e}", dir.display()))
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            if path.extension()? != "kp" {
                return None;
            }
            path.file_stem()?.to_str().map(str::to_string)
        })
        .collect();
    names.sort();

    let c_dir = out_path("c");
    let asm_dir = out_path("asm");
    std::fs::create_dir_all(&c_dir).expect("could not create the C artifact directory");
    std::fs::create_dir_all(&asm_dir).expect("could not create the asm artifact directory");

    let mut c_built = Vec::new();
    let mut asm_built = Vec::new();
    let mut refusals = Vec::new();
    let mut disagreements = Vec::new();

    for name in names {
        let source = dir.join(format!("{name}.kp"));
        let c_bin = c_dir.join(&name);
        if !build("c", &source, &c_bin).status.success() {
            // The C backend is the oracle; a fixture it rejects is not the asm
            // backend's to build, and the compile-error fixtures are rejected by
            // design. Nothing to compare.
            continue;
        }
        c_built.push(name.clone());

        let asm_bin = asm_dir.join(&name);
        let built = build("asm", &source, &asm_bin);
        if !built.status.success() {
            refusals.push((
                name.clone(),
                String::from_utf8_lossy(&built.stderr).to_string(),
            ));
            continue;
        }
        asm_built.push(name.clone());

        let c_run = run(&c_bin);
        let asm_run = run(&asm_bin);
        if c_run.stdout != asm_run.stdout {
            disagreements.push(format!(
                "'{name}': the 'c' and 'asm' backends print different stdout"
            ));
        }
        if c_run.code != asm_run.code {
            disagreements.push(format!(
                "'{name}': the 'c' and 'asm' backends exit with status {:?} and {:?}",
                c_run.code, asm_run.code
            ));
        }
    }

    Sweep {
        c_built,
        asm_built,
        refusals,
        disagreements,
    }
}

// ---------------------------------------------------------------------------
// The differential sweep and the refusal prefix.
// ---------------------------------------------------------------------------

/// The whole point of the second backend, run rather than argued: for every
/// fixture the C backend builds, the asm backend either builds it too and
/// agrees with the C binary byte for byte, or refuses it *by name*.
///
/// This is the fixture-level form of
/// `compile_and_run.rs::the_two_backends_agree_on_the_scalar_fixtures`, whose
/// hand-written list grows as the asm backend covers more and which a refused
/// fixture fails. This one never fails for a fixture the asm backend cannot
/// build yet — it fails only when a build it *does* accept disagrees with the C
/// backend (the miscompile the fixture literals cannot catch, because a wrong
/// literal and a wrong backend agree with each other) or when a refusal does
/// not begin with [`REFUSAL_PREFIX`], which would be the backend hiding a gap
/// instead of naming it.
#[test]
#[ignore = "the full sweep builds 127 fixtures twice; ground rule 1.5 keeps it out of the default suite"]
fn the_asm_backend_agrees_with_the_c_backend_or_refuses_by_name() {
    let s = sweep();

    let unnamed: Vec<&str> = s
        .refusals
        .iter()
        .filter(|(_, message)| !message.starts_with(REFUSAL_PREFIX))
        .map(|(name, _)| name.as_str())
        .collect();
    assert!(
        unnamed.is_empty(),
        "these fixtures were refused without the '{}' prefix in front of the refusal:\n{}",
        REFUSAL_PREFIX,
        unnamed
            .iter()
            .map(|name| format!("  {name}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );

    assert!(
        s.disagreements.is_empty(),
        "the two backends do not agree on {} of the {} fixtures both build:\n{}",
        s.disagreements.len(),
        s.asm_built.len(),
        s.disagreements.join("\n"),
    );
}

// ---------------------------------------------------------------------------
// The coverage ratchet.
// ---------------------------------------------------------------------------

/// The recorded set of fixtures the asm backend builds may never shrink. A
/// slice that removes coverage is a regression, and the point of writing the
/// list down is that the removal is what makes the run fail rather than a
/// number quietly dropping in the plan's tables.
///
/// Growing the set is fine and expected: this reports the fixtures it builds
/// that the baseline does not name, and the answer is to add them here. The
/// failure direction is one-sided on purpose — an entry that no longer appears
/// fails and names itself.
#[test]
#[ignore = "the full sweep builds 127 fixtures twice; ground rule 1.5 keeps it out of the default suite"]
fn the_asm_backend_builds_a_superset_of_the_recorded_baseline() {
    let s = sweep();
    let built: BTreeSet<&str> = s.asm_built.iter().map(String::as_str).collect();

    let regressed: Vec<&str> = ASM_BUILT_BASELINE
        .iter()
        .copied()
        .filter(|name| !built.contains(name))
        .collect();
    assert!(
        regressed.is_empty(),
        "the asm backend no longer builds {} fixture(s) this file records as covered: {}",
        regressed.len(),
        regressed.join(", "),
    );

    let added: Vec<&str> = built
        .iter()
        .copied()
        .filter(|name| !ASM_BUILT_BASELINE.contains(name))
        .collect();
    if !added.is_empty() {
        println!(
            "note: the asm backend builds {} fixture(s) the baseline does not name yet — \
             re-record ASM_BUILT_BASELINE: {}",
            added.len(),
            added.join(", "),
        );
    }
}

// ---------------------------------------------------------------------------
// The ranking report.
// ---------------------------------------------------------------------------

/// Which row of `plan/mk7-own-backend.md` §5's table a refusal belongs to.
///
/// §5 recovers the category as "the feature in the way, recovered from each
/// fixture's first refusal message", so this reads the same message and decides
/// from the construct it names. That is deliberately a report and not a
/// contract: the backend's refusal messages carry the construct's name, not a
/// machine-readable category, so a fixture can move rows when a message's
/// wording changes — which is why this is a `println!` and never an assertion.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Ranking {
    /// A heap type the backend cannot lower — `Bytes`/`String`/`View`/`List`/
    /// `Map`/`Box` as a type, a parameter, a return, a field or a passdown —
    /// plus an `extern fun` whose body this backend has no routine for. §5
    /// treats the heap as one row because the allocator is what unblocks almost
    /// all of it, and it is the row the plan's table calls the catch-all (a
    /// build stops at the first refusal it hits).
    Heap,
    /// An `enum` — through a value, or through a parameter or return, which is
    /// the same tagged-union gap seen from a signature.
    Enum,
    /// A `datac` holding a float or heap field (§5's "`Circle`, `Bag`'s nested
    /// shape" row). The backend refuses these with one message whichever field
    /// class it is, so the two are counted together here.
    Datac,
    /// A closure or a function value, however it is spelled.
    Closure,
    /// A capability-gated `extern fun`, refused for the gate rather than for
    /// the symbol.
    GatedExtern,
}

impl Ranking {
    fn title(self) -> &'static str {
        match self {
            Ranking::Heap => "a heap type",
            Ranking::Enum => "an `enum`",
            Ranking::Datac => "a `datac` holding a float or heap field",
            Ranking::Closure => "a closure or a function value",
            Ranking::GatedExtern => "a gated extern",
        }
    }

    /// The order §5's table uses.
    const ROWS: [Ranking; 5] = [
        Ranking::Heap,
        Ranking::Enum,
        Ranking::Datac,
        Ranking::Closure,
        Ranking::GatedExtern,
    ];
}

/// The heap type names the asm backend has no lowering for, as §5's heap row
/// lists them.
const HEAP_TYPES: &[&str] = &["Bytes", "String", "View", "List", "Map", "Box"];

/// `std` enums a fixture does not declare itself, so a by-reference parameter
/// of one reads as the `enum` gap it is.
const STDLIB_ENUMS: &[&str] = &["Option", "Result", "Res"];

/// The gated extern that is refused for its gate rather than its symbol — the
/// one `extern fun` §5's gated-extern row names.
const GATED_EXTERNS: &[&str] = &["std.time.now_unix_raw"];

/// The construct a refusal message names. The messages `codegen_asm.rs` emits
/// spell the offending thing one of a handful of ways, and the category follows
/// from which one this is.
enum Subject<'a> {
    Closure,
    Extern(&'a str),
    EnumName,
    DatacName,
    Type(&'a str),
    /// A construct the messages spell some other way — an lvalue it cannot
    /// assign, an operator it cannot lower, and so on.
    Other,
}

/// The first backtick-delimited token after `marker`, if there is one.
fn backticked_after<'a>(haystack: &'a str, marker: &str) -> Option<&'a str> {
    let start = haystack.find(marker)? + marker.len();
    let rest = &haystack[start..];
    Some(&rest[..rest.find('`')?])
}

fn subject(message: &str) -> Subject<'_> {
    let message = message.trim();
    if message.contains("function value")
        || message.contains("closure capture")
        || message.contains("a closure")
    {
        return Subject::Closure;
    }
    if let Some(symbol) = backticked_after(message, "the `extern fun ") {
        return Subject::Extern(symbol);
    }
    if backticked_after(message, "the enum `").is_some() {
        return Subject::EnumName;
    }
    if backticked_after(message, "the datac `").is_some() {
        return Subject::DatacName;
    }
    if let Some(binding) = backticked_after(message, "the parameter `") {
        if let Some((_, ty)) = binding.split_once(": ") {
            return Subject::Type(ty);
        }
    }
    if let Some(ty) = backticked_after(message, "the return type `") {
        return Subject::Type(ty);
    }
    if let Some(ty) = backticked_after(message, "its type is `") {
        return Subject::Type(ty);
    }
    if let Some(ty) = backticked_after(message, "the type `") {
        return Subject::Type(ty);
    }
    Subject::Other
}

/// The bare name of a type as a refusal message spells it: leading `&` and
/// `mut` stripped, generic arguments dropped, and any module path reduced to
/// its last segment (`std.result.Result<i64, String>` -> `Result`).
fn bare_type_name(ty: &str) -> String {
    let t = ty.trim().trim_start_matches('&').trim();
    let t = t.strip_prefix("mut ").unwrap_or(t).trim();
    let t = t.split('<').next().unwrap_or(t).trim();
    t.rsplit('.').next().unwrap_or(t).trim().to_string()
}

/// Does the fixture's own source declare `kind <name>`? Reading the source is
/// how an `enum` reached through a by-reference parameter (`&Shape`) is told
/// apart from a heap type reached the same way (`&Bytes`): the refusal message
/// prints the type's name in both cases, and only the source says which kind of
/// declaration produced it.
fn declares(source: &str, kind: &str, name: &str) -> bool {
    let needle = format!("{kind} {name}");
    source.contains(&needle)
}

fn type_category(source: &str, ty: &str) -> Ranking {
    if ty.contains("closure") || ty.contains("fun(") {
        return Ranking::Closure;
    }
    let bare = bare_type_name(ty);
    if HEAP_TYPES.contains(&bare.as_str()) {
        return Ranking::Heap;
    }
    if STDLIB_ENUMS.contains(&bare.as_str()) || declares(source, "enum", &bare) {
        return Ranking::Enum;
    }
    if declares(source, "datac", &bare) {
        return Ranking::Datac;
    }
    Ranking::Heap
}

fn category(source: &str, message: &str) -> Ranking {
    match subject(message) {
        Subject::Closure => Ranking::Closure,
        Subject::Extern(symbol) => {
            if GATED_EXTERNS.contains(&symbol) {
                Ranking::GatedExtern
            } else {
                Ranking::Heap
            }
        }
        Subject::EnumName => Ranking::Enum,
        Subject::DatacName => Ranking::Datac,
        Subject::Type(ty) => type_category(source, ty),
        // §5's heap row is the catch-all, so that is where an unplaced refusal
        // lands.
        Subject::Other => Ranking::Heap,
    }
}

/// Prints the refusals grouped into the categories §5's ranking table uses,
/// with a count and the fixtures in each, then the total. This is the report
/// the plan's "what is next, in the order the refusals rank themselves" section
/// is measured from, and it is a print on purpose — a fixture moving between
/// categories must not be able to break a run.
///
/// Run it with `cargo test --test backend_coverage -- --ignored --nocapture`;
/// without `--nocapture` the harness swallows stdout and the report is invisible.
#[test]
#[ignore = "the full sweep builds 127 fixtures twice; ground rule 1.5 keeps it out of the default suite"]
fn the_refusals_rank_by_the_categories_the_plan_uses() {
    let s = sweep();

    let mut by_row: BTreeMap<Ranking, Vec<String>> = BTreeMap::new();
    for (name, message) in &s.refusals {
        let source = std::fs::read_to_string(fixture_dir().join(format!("{name}.kp")))
            .unwrap_or_default();
        by_row
            .entry(category(&source, message))
            .or_default()
            .push(name.clone());
    }

    println!(
        "asm backend refusal ranking ({} fixtures refused of {} the C backend builds, {} built)",
        s.refusals.len(),
        s.c_built.len(),
        s.asm_built.len(),
    );
    for row in Ranking::ROWS {
        let fixtures = by_row.get(&row);
        println!(
            "  {}: {}",
            row.title(),
            fixtures.map_or(0, Vec::len),
        );
        if let Some(fixtures) = fixtures {
            for name in fixtures {
                println!("    {name}");
            }
        }
    }
    println!("  total: {}", s.refusals.len());
}

// ---------------------------------------------------------------------------
// A cheap shape check, so the default (non-`--ignored`) run still compiles and
// exercises this file without any of the sweep's cost.
// ---------------------------------------------------------------------------

/// The recorded baseline is the ratchet's whole state, so a hand edit that
/// unsorts or repeats it would make the superset check meaningless. This is the
/// one check here that costs nothing and therefore runs in the default suite.
#[test]
fn the_recorded_baseline_is_sorted_and_unique() {
    let mut sorted = ASM_BUILT_BASELINE.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        ASM_BUILT_BASELINE.len(),
        "ASM_BUILT_BASELINE must not repeat a fixture",
    );
    assert_eq!(
        sorted.as_slice(),
        ASM_BUILT_BASELINE,
        "ASM_BUILT_BASELINE must be sorted",
    );
}
