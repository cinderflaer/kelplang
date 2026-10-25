use super::*;

#[test]
pub(crate) fn multi_file_project_with_manifest_runs() {
    // `main.kp` imports `shapes/circle` and `geom`; `shapes/circle.kp` itself
    // imports `geom`, exercising a nested import with its own alias table.
    assert_project_runs(
        "shapes",
        &project("shapes"),
        "12.566360\n6.000000\n50.265440\n28.274310\n",
    );
}

#[test]
pub(crate) fn manifest_less_sibling_import_runs() {
    // No `coral.toml`: the entry file's own directory is the module root.
    assert_project_runs(
        "loose",
        &project("loose").join("main.kp"),
        "42\n",
    );
}

#[test]
pub(crate) fn import_namespace_reference_runs() {
    // `impl "util"` plus a qualified type in a signature (`util.Tally`): the
    // namespace is the module's own name, which is the spelling the `as` clause
    // used to override.
    assert_project_runs("namespace", &project("namespace"), "7\n4\n");
}

#[test]
pub(crate) fn same_name_in_two_modules_does_not_collide() {
    // Strict module-qualified names are what make this legal: `left.Item`
    // and `right.Item` are distinct structs, and `left.describe` /
    // `right.describe` are distinct functions.
    assert_project_runs(
        "same_name",
        &project("same_name").join("main.kp"),
        "6\n9\n4\n13\n",
    );
}

#[test]
pub(crate) fn import_cycle_is_rejected() {
    assert_project_rejected(
        "cycle",
        &project("cycle").join("a.kp"),
        "import cycle: a -> b -> a",
    );
}

#[test]
pub(crate) fn missing_module_is_rejected() {
    assert_project_rejected(
        "unknown_module",
        &project("unknown_module").join("main.kp"),
        "cannot find module 'nope'",
    );
}

#[test]
pub(crate) fn duplicate_import_alias_is_rejected() {
    assert_project_rejected(
        "duplicate_alias",
        &project("duplicate_alias").join("main.kp"),
        "alias 'circle' is already used by another import",
    );
}

#[test]
pub(crate) fn parent_directory_import_is_rejected() {
    assert_project_rejected(
        "dotdot",
        &project("dotdot").join("main.kp"),
        "must not contain '.' or '..' segments",
    );
}

#[test]
pub(crate) fn non_identifier_module_path_is_rejected() {
    assert_project_rejected(
        "bad_path",
        &project("bad_path").join("main.kp"),
        "is not a valid module name",
    );
}

#[test]
pub(crate) fn unknown_module_member_is_rejected() {
    assert_project_rejected(
        "unknown_member",
        &project("unknown_member").join("main.kp"),
        "module 'prov' has no item 'Nope'",
    );
}

#[test]
pub(crate) fn unqualified_imported_item_is_rejected() {
    // Strict module-qualified namespace: a bare `Thing(...)` never resolves
    // to another module's `Thing`, and the error suggests the qualified form.
    assert_project_rejected(
        "unqualified_import",
        &project("unqualified_import").join("main.kp"),
        "did you mean 'prov.Thing'?",
    );
}

#[test]
pub(crate) fn an_exported_item_is_reachable_from_another_module() {
    // A-13: `pub` on a `const`, a `datac` and a `fun` is what makes each one
    // reachable through the module alias. The receiver method carries no
    // `pub`: it follows its declaring type, and `p.sum()` reaches it through
    // the value.
    assert_project_runs(
        "visibility_import",
        &project("visibility_import").join("main.kp"),
        "10\n42\n",
    );
}

#[test]
pub(crate) fn a_private_item_is_not_reachable_from_another_module() {
    // Private by default: `hidden` exists, so the refusal is about its
    // visibility, not about a missing item (`unknown_member` pins that text).
    assert_project_rejected(
        "visibility_private",
        &project("visibility_private").join("main.kp"),
        "'lib.hidden' is private",
    );
}

#[test]
pub(crate) fn a_private_item_is_not_offered_by_the_did_you_mean_hint() {
    let target = project("visibility_private_hint").join("main.kp");
    let build = kelpc()
        .arg("build")
        .arg(&target)
        .arg("-o")
        .arg(out_path("visibility_private_hint"))
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        !build.status.success(),
        "expected project 'visibility_private_hint' to be rejected, but it built"
    );
    let stderr = String::from_utf8_lossy(&build.stderr);
    assert!(
        stderr.contains("call to undefined function 'Thing'"),
        "expected the unqualified name to be undefined, got:\n{stderr}"
    );
    assert!(
        !stderr.contains("did you mean"),
        "the hint must not offer a private item, got:\n{stderr}"
    );
}

#[test]
pub(crate) fn a_private_type_is_not_nameable_in_a_type_position() {
    // The name resolver returned early for a canonical key before running the
    // export check, so a `datac` could cross a module boundary in a type
    // position (`describe`'s parameter) with no refusal at all.
    assert_project_rejected(
        "visibility_type_private",
        &project("visibility_type_private").join("main.kp"),
        "'lib.Hidden' is private",
    );
}

#[test]
pub(crate) fn an_exported_type_is_nameable_in_every_type_position() {
    // The acceptance half of the same gate: `pub` types in a parameter type, a
    // return type, a field type, a local annotation, a generic argument, and a
    // `List`, `Box` and array element. It also pins the bare names that must
    // keep resolving: `local` is the entry module's own private function and
    // `add` is `lib`'s, and both are called bare from inside their module.
    assert_project_runs(
        "visibility_type_pub",
        &project("visibility_type_pub").join("main.kp"),
        "1\n5\n8\n21\n3\n1\n42\n42\n",
    );
}

#[test]
pub(crate) fn the_entry_modules_private_items_are_not_reachable_by_their_bare_name() {
    // The entry module is the only one whose keys carry no prefix, so a lookup
    // on the name as written reached its private `helper` from any module. A
    // bare name is the calling module's own, so the call is undefined here, not
    // private (`visibility_private` pins the private text).
    assert_project_rejected(
        "visibility_bare_call",
        &project("visibility_bare_call").join("main.kp"),
        "call to undefined function 'helper'",
    );
}

#[test]
pub(crate) fn a_project_module_under_a_std_path_is_not_a_shipped_module() {
    // The module id `std/main` looks shipped and is not: the file is a project
    // file, so its items are private by default like any other module's. The
    // entry module itself still names `Hidden`; another module names it in a
    // type position, and a bare name there is that module's own, so the
    // reference is undefined rather than private. The shipped-module predicate
    // that decides this is pinned directly by the unit test beside it in
    // `infer.rs`.
    assert_project_rejected(
        "visibility_std_path",
        &project("visibility_std_path").join("src/std/main.kp"),
        "unknown type 'Hidden'",
    );
}

#[test]
pub(crate) fn a_public_function_may_not_name_a_private_parameter_type() {
    // A-13 private-in-public: an item another module can reach must not name,
    // in its signature, a type its own module does not export. The private
    // type is in the *same* module here, so nothing in the cross-module name
    // gate ever sees the reference; the signature is what leaks it.
    assert_rejected_with(
        "tests/fixtures/visibility_pub_fun_param.kp",
        "'describe' is exported but its signature names 'Hidden'",
    );
}

#[test]
pub(crate) fn a_public_function_may_not_name_a_private_return_type() {
    // The return half of the same signature (A-13).
    assert_rejected_with(
        "tests/fixtures/visibility_pub_fun_return.kp",
        "'make' is exported but its signature names 'Secret'",
    );
}

#[test]
pub(crate) fn a_public_datac_may_not_name_a_private_field_type() {
    // A field is reachable through a value of the exported type (A-13).
    assert_rejected_with(
        "tests/fixtures/visibility_pub_datac_field.kp",
        "'Boxed' is exported but its signature names 'Hidden'",
    );
}

#[test]
pub(crate) fn a_method_on_an_exported_datac_may_not_name_a_private_type() {
    // A method is reached through a value of its declaring type, so an
    // exported `datac` makes the method's signature reachable too (A-13).
    assert_rejected_with(
        "tests/fixtures/visibility_pub_method_type.kp",
        "'Boxed.stash' is exported but its signature names 'Hidden'",
    );
}

#[test]
pub(crate) fn a_public_enum_may_not_name_a_private_payload_type() {
    // A payload is matched out of the exported variant (A-13).
    assert_rejected_with(
        "tests/fixtures/visibility_pub_enum_payload.kp",
        "'Shape' is exported but its signature names 'Hidden'",
    );
}

#[test]
pub(crate) fn a_public_const_may_not_name_a_private_type() {
    // `pub const LIMIT: lib.Hidden` names a type module 'lib' does not export
    // (A-13). The rule is checked before a const's initializer is folded, so a
    // const whose type is the one that leaks is refused for that reason first.
    assert_project_rejected(
        "visibility_const_type",
        &project("visibility_const_type").join("main.kp"),
        "'LIMIT' is exported but its signature names 'lib.Hidden'",
    );
}

#[test]
pub(crate) fn a_public_extern_function_may_not_name_a_private_type() {
    // An `extern fun` has no body, so its parameters and return type are its
    // whole signature (A-13).
    assert_rejected_with(
        "tests/fixtures/visibility_pub_extern_type.kp",
        "'poke' is exported but its signature names 'Hidden'",
    );
}

#[test]
pub(crate) fn a_public_function_may_not_name_a_private_type_inside_a_list() {
    // A container names the types inside it, so the rule recurses (A-13).
    assert_rejected_with(
        "tests/fixtures/visibility_pub_list_type.kp",
        "'lookup' is exported but its signature names 'Hidden'",
    );
}

#[test]
pub(crate) fn a_public_function_may_not_name_a_private_type_inside_a_map() {
    // Both of a `Map`'s arguments are named types (A-13).
    assert_rejected_with(
        "tests/fixtures/visibility_pub_map_type.kp",
        "'tally' is exported but its signature names 'Hidden'",
    );
}

#[test]
pub(crate) fn a_public_function_may_not_name_a_private_type_inside_an_array() {
    // An array is its element and its length, and an array of arrays recurses
    // through the element (A-13).
    assert_rejected_with(
        "tests/fixtures/visibility_pub_array_type.kp",
        "'grid' is exported but its signature names 'Hidden'",
    );
}

#[test]
pub(crate) fn a_public_function_may_not_name_a_private_borrowed_type() {
    // A borrow names its pointee (A-13).
    assert_rejected_with(
        "tests/fixtures/visibility_pub_borrow.kp",
        "'peek' is exported but its signature names 'Hidden'",
    );
}

#[test]
pub(crate) fn a_public_function_may_not_return_a_private_type_inside_a_box() {
    // `Box` names its element type, so the walk reaches it like any other
    // container's argument (A-13).
    assert_rejected_with(
        "tests/fixtures/visibility_pub_box_type.kp",
        "'stash' is exported but its signature names 'Hidden'",
    );
}

#[test]
pub(crate) fn a_public_datac_may_not_name_a_private_type_inside_a_function_type() {
    // A function type names its parameters and its return, so the walk
    // recurses through both (A-13).
    assert_rejected_with(
        "tests/fixtures/visibility_pub_fn_type.kp",
        "'Callbacks' is exported but its signature names 'Hidden'",
    );
}

#[test]
pub(crate) fn an_exported_signature_may_not_name_a_private_type_declared_after_it() {
    // The check runs over the merged program with the whole export set in
    // hand, so the order of two declarations in one file does not decide the
    // answer (A-13, `design/syntax.md (kelp wiki)` 13.4).
    assert_rejected_with(
        "tests/fixtures/visibility_pub_late_type.kp",
        "'describe' is exported but its signature names 'Hidden'",
    );
}

#[test]
pub(crate) fn an_exported_signature_may_name_every_type_its_module_exports() {
    // The acceptance half: a `pub` item naming a `pub` type in its own module,
    // another module's `pub` type through its alias, a generic instantiation
    // of an exported type, a compiler-shipped `std/...` type (which the
    // transitional rule exports wholesale), a generic's own parameter, and a
    // private item naming a private type, all compile and run.
    assert_project_runs(
        "visibility_pub_signature",
        &project("visibility_pub_signature").join("main.kp"),
        "0\n2\n2\n5\n4\n7\n",
    );
}

#[test]
pub(crate) fn unshipped_stdlib_module_is_rejected() {
    // `std/tls` shipped in MK5-S4b-M1 (open item 37) and is no longer the
    // example here; `std/zzz_unshipped` is a placeholder name reserved by
    // this fixture alone (never a real module), so this test needs no
    // updating the next time a real one ships. The error names what *is*
    // shipped so the reservation stays useful.
    assert_project_rejected(
        "stdlib_import",
        &project("stdlib_import").join("main.kp"),
        "stdlib module 'zzz_unshipped' is not implemented yet (shipped today: std/mem",
    );
}

#[test]
pub(crate) fn the_compiler_internal_std_namespace_is_unavailable_to_projects() {
    assert_project_rejected(
        "std_internal_import",
        &project("std_internal_import").join("main.kp"),
        "compiler-reserved std/internal namespace",
    );
    assert_project_rejected(
        "std_internal_entry",
        &project("std_internal_entry"),
        "below the compiler-reserved std/internal namespace",
    );
}

#[test]
pub(crate) fn a_local_that_shadows_a_module_names_the_shadowing() {
    // The local wins, so the call is not the module's. The message says that
    // much, and names the module only when that module really has the method
    // the call was written for.
    assert_rejected_with(
        "tests/fixtures/shadowed_module_alias.kp",
        "'path' is a local binding here, so 'path.join' is not the module 'path'",
    );
}

#[test]
pub(crate) fn a_grant_pattern_that_is_not_a_path_is_rejected() {
    assert_project_rejected(
        "caps_bad_path_pattern",
        &project("caps_bad_path_pattern").join("main.kp"),
        "capability patterns are absolute paths",
    );
}

// ---------------------------------------------------------------------------
// M5.2: passdowns (`pd`) and string templates.
// ---------------------------------------------------------------------------

#[test]
pub(crate) fn passdowns_are_read_by_referencing_functions_only() {
    // `banner`/`label` reference passdowns (and become wired); `unrelated`
    // references none; `main` reads the generated globals directly.
    assert_fixture_runs("tests/fixtures/passdowns.kp");
}

#[test]
pub(crate) fn a_datac_passdown_is_read_through_a_shared_borrow() {
    assert_fixture_runs("tests/fixtures/passdown_datac.kp");
}

#[test]
pub(crate) fn a_local_can_shadow_a_passdown() {
    assert_fixture_runs("tests/fixtures/pd_shadowed.kp");
}

#[test]
pub(crate) fn a_passdown_is_forwarded_through_a_caller_that_does_not_name_it() {
    // The call site's hidden argument is a borrow of the storage in scope:
    // the caller's own hidden parameter when it has one (so the forwarded
    // value stays a pointer, not a pointer to a pointer), the global's
    // address when it does not, `main`, or a function that only calls a
    // function reading the passdown.
    assert_fixture_runs("tests/fixtures/pd_argument_forwarding.kp");
}

#[test]
pub(crate) fn a_pd_var_is_writable_from_a_function_and_from_main() {
    // M6.4 replaced M5.2's "read-only inside a function": a `pd var` is
    // reached through an exclusive borrow, so writes work and stay checked.
    assert_fixture_runs("tests/fixtures/pd_var_write.kp");
}

#[test]
pub(crate) fn a_pd_var_no_function_writes_is_a_shared_borrow() {
    // Roadmap item 17: the borrow a function holds is computed from whether
    // it writes the passdown, so a read-only `pd var` is shared and can be
    // both an argument and a hidden parameter of the same call.
    assert_fixture_runs("tests/fixtures/pd_var_read_only_shared.kp");
    assert_fixture_runs("tests/fixtures/pd_var_read_only_datac.kp");
}

#[test]
pub(crate) fn a_pd_var_write_reaches_the_callers_that_hold_it() {
    // The upgrade follows the call graph, and the edge is followed even when
    // the callee does not name the passdown (a pure forwarder takes the
    // global's address itself), a shared borrow held across that write is
    // the hazard the mode exists to prevent.
    assert_fixture_runs("tests/fixtures/pd_write_reaches_callers.kp");
    // The other trigger a callee's own references cannot show: the passdown is
    // handed to a `mut` parameter, so the caller writes it without `add` ever
    // naming a passdown.
    assert_fixture_runs("tests/fixtures/pd_var_mut_argument.kp");
}

#[test]
pub(crate) fn a_pd_var_of_a_two_word_datac_is_writable_through_its_borrow() {
    // The write is `*p = v` on the passdown's hidden borrow, and the operand is
    // a use: missing it puts the write in another parameter's storage instead
    // of refusing it.
    assert_fixture_runs("tests/fixtures/pd_var_datac_reassign.kp");
}

#[test]
pub(crate) fn a_scalar_local_can_be_borrowed_and_written_through_the_pointer() {
    // MK7 §5: the caller's half of a `mut` out-parameter, `&mut x` on a scalar
    // local, plus `*p` and `*p = v` through the pointer it produces, a pointer
    // binding held across a call, and two live locals around the store (the
    // live-range probe: a missed touch hands the pointer's register to `keep`
    // and the store writes through it).
    assert_fixture_runs("tests/fixtures/scalar_borrow.kp");
}

#[test]
pub(crate) fn a_float_scalar_can_be_borrowed_and_written_through_the_pointer() {
    // The same borrow with an `f64` pointee: only the load and the store change
    // register file, so a wrong mnemonic prints a plausible wrong number rather
    // than failing to build.
    assert_fixture_runs("tests/fixtures/scalar_borrow_f64.kp");
}

#[test]
pub(crate) fn passdown_storage_is_a_global_that_main_initializes() {
    // What a backend without the C backend's hidden-argument plumbing does
    // instead: one global per `pd`, assigned by a sequence at the top of
    // `main`, and a hidden parameter whose value is that object's address. An
    // initializer that reads an earlier passdown pins the initialization order,
    // and `bump` pins the case where the caller has no hidden parameter and so
    // passes the global's own address.
    assert_fixture_runs("tests/fixtures/passdown_globals.kp");
}

#[test]
pub(crate) fn writing_a_pd_val_inside_a_function_is_rejected() {
    assert_rejected("tests/fixtures/pd_val_write.kp");
}

#[test]
pub(crate) fn a_pd_var_cannot_be_borrowed_twice_in_one_call() {
    // The documented cost of M6.4's design, pinned so it reads as a rule.
    let bin = out_path("pd_var_double_borrow");
    let build = kelpc()
        .arg("build")
        .arg(fixture("tests/fixtures/pd_var_double_borrow.kp"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        !build.status.success(),
        "expected a `pd var` borrowed exclusively and shared in one call to be rejected"
    );
    let stderr = String::from_utf8_lossy(&build.stderr);
    assert!(
        stderr.contains("already borrowed exclusively"),
        "expected a borrow-conflict message, got:\n{stderr}"
    );
}

#[test]
pub(crate) fn reading_a_copy_passdown_twice_in_one_call_is_allowed() {
    assert_fixture_runs("tests/fixtures/pd_var_copy_read_twice.kp");
}

#[test]
pub(crate) fn a_method_receiver_is_borrowed_after_its_own_arguments() {
    // MISSING-n4Bz7: `counts.insert(k, counts.get_or(..) + 1)` and its List and
    // String-keyed spellings. Each was "already borrowed exclusively" before,
    // because the receiver's exclusive borrow started before the arguments were
    // checked.
    assert_fixture_runs("tests/fixtures/borrow_two_phase.kp");
}

#[test]
pub(crate) fn two_phase_borrows_still_reject_aliasing() {
    // The other half: a shared borrow of the receiver that outlives the
    // arguments is still a conflict, and so is a second exclusive borrow.
    assert_rejected_with(
        "tests/fixtures/borrow_two_phase_alias.kp",
        "already borrowed as shared",
    );
    assert_rejected_with(
        "tests/fixtures/borrow_two_phase_nested.kp",
        "already borrowed exclusively",
    );
}

#[test]
pub(crate) fn reading_a_passdown_before_it_is_declared_is_rejected() {
    assert_rejected("tests/fixtures/pd_forward_read.kp");
}

#[test]
pub(crate) fn templates_print_literals_and_values() {
    assert_fixture_runs("tests/fixtures/templates.kp");
}

#[test]
pub(crate) fn a_template_outside_print_is_a_string_value() {
    // A template outside `print` is a String value: the bound text prints as written.
    assert_fixture_runs("tests/fixtures/template_not_printed.kp");
}

#[test]
pub(crate) fn a_template_string_binds_and_passes_as_a_string() {
    assert_fixture_runs("tests/fixtures/template_string_val.kp");
    assert_fixture_runs("tests/fixtures/template_string_arg.kp");
}

#[test]
pub(crate) fn a_passdown_is_not_visible_from_another_module() {
    assert_project_rejected(
        "pd_module_scope",
        &project("pd_module_scope").join("main.kp"),
        "is a passdown of module 'util'",
    );
}

// ---------------------------------------------------------------------------
// M5.3: `_alias` text substitution (`[alias]` in coral.toml + inline `alias`).
// ---------------------------------------------------------------------------

#[test]
pub(crate) fn aliases_substitute_text_in_expressions_and_strings() {
    // Also pins the pure-text semantics: `_doubled` is `40 + 2 * 2`.
    assert_fixture_runs("tests/fixtures/aliases.kp");
}

#[test]
pub(crate) fn an_underscore_name_nobody_declares_is_a_plain_identifier() {
    assert_fixture_runs("tests/fixtures/alias_plain_ident.kp");
}

#[test]
pub(crate) fn an_alias_in_the_manifest_can_be_an_impl_path() {
    assert_project_runs("alias_kpconf", &project("alias_kpconf"), "42\n");
}

#[test]
pub(crate) fn an_alias_from_an_imported_module_is_visible() {
    // The table is flat and global: `_greeting` is declared in `util.kp`
    // and used in `main.kp`.
    assert_project_runs(
        "alias_cross_file",
        &project("alias_cross_file"),
        "hello from an imported alias\n",
    );
}

#[test]
pub(crate) fn an_alias_cycle_is_rejected_even_when_unused() {
    assert_rejected("tests/fixtures/alias_cycle.kp");
}

#[test]
pub(crate) fn an_unknown_alias_is_rejected() {
    assert_rejected("tests/fixtures/alias_unknown.kp");
}

#[test]
pub(crate) fn an_alias_cycle_across_two_files_names_both_sites() {
    // `_a` is defined in coral.toml's [alias], `_b` in `main.kp`; the diagnostic points
    // at the reference that closed the cycle and notes the definition, so
    // both files appear.
    let bin = out_path("alias_kpconf_cycle");
    let build = kelpc()
        .arg("build")
        .arg(project("alias_kpconf_cycle"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(!build.status.success(), "expected a rejection");
    let stderr = String::from_utf8_lossy(&build.stderr);
    for needle in ["alias cycle: _a -> _b -> _a", "coral.toml:", "main.kp:"] {
        assert!(
            stderr.contains(needle),
            "expected the error to mention '{needle}', got:\n{stderr}"
        );
    }
}
