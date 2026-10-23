use super::*;

// ---------------------------------------------------------------------------
// A-12 slice 2: traits with static dispatch. A trait is a named set of method
// signatures, an `impl` gives them to a `datac`/`enum`, and a generic function
// bounds its type parameter on one. Every call below is the concrete type's own
// method, so both backends emit the same direct call they would have emitted
// without the trait.

#[test]
pub(crate) fn a_trait_is_implemented_by_a_datac_and_an_enum_and_called_through_a_generic() {
    assert_compiles_and_runs(
        "tests/fixtures/traits_datac_and_enum.kp",
        "12.000000\n27.000000\n27.000000\ncircle\ntrue\n9.000000\n18.000000\n2\n9.000000\n0.000000\n",
    );
}

#[test]
pub(crate) fn a_generic_datac_method_calls_a_bounded_generic_with_its_type_parameter() {
    // The bound is resolved when the *method* is instantiated, so the call
    // inside `Pair<A, B>.total()` is checked once per pair of concrete
    // arguments, exactly as a free function's body is.
    assert_compiles_and_runs(
        "tests/fixtures/traits_generic_datac_method.kp",
        "7\n3\n",
    );
}

#[test]
pub(crate) fn a_bound_is_checked_at_every_call_and_transitively() {
    // A call whose type argument does not implement the bound, and a generic
    // function reaching a second bounded generic with its own parameter: the
    // inner call is refused where the outer one is instantiated with a type
    // that satisfies only the outer bound.
    assert_rejected_with(
        "tests/fixtures/traits_bound_unimplemented.kp",
        "needs 'Point' to implement the trait 'Shape'",
    );
    assert_rejected_with(
        "tests/fixtures/traits_bound_transitive.kp",
        "needs 'OnlyNamed' to implement the trait 'Doubled'",
    );
    // Two bounds on one parameter, only one of them satisfied.
    assert_rejected_with(
        "tests/fixtures/traits_bound_second.kp",
        "needs 'OnlyShape' to implement the trait 'Grow'",
    );
}

#[test]
pub(crate) fn an_impl_must_implement_exactly_the_traits_signatures() {
    assert_rejected_with(
        "tests/fixtures/traits_missing_method.kp",
        "does not implement it",
    );
    assert_rejected_with(
        "tests/fixtures/traits_wrong_signature.kp",
        "does not match trait 'Shape'",
    );
    assert_rejected_with(
        "tests/fixtures/traits_wrong_receiver.kp",
        "takes its receiver differently than trait 'Grow'",
    );
    assert_rejected_with(
        "tests/fixtures/traits_extra_method.kp",
        "'shrink' is not declared by trait 'Shape'",
    );
}

#[test]
pub(crate) fn a_trait_is_implemented_for_a_type_at_most_once() {
    assert_rejected_with(
        "tests/fixtures/traits_duplicate_impl.kp",
        "already has an impl for 'Circle'",
    );
}

#[test]
pub(crate) fn a_trait_method_and_an_explicit_method_cannot_share_a_name() {
    assert_rejected_with(
        "tests/fixtures/traits_inherent_collision.kp",
        "already has as an explicit method",
    );
}

#[test]
pub(crate) fn an_impl_naming_no_declared_trait_is_rejected() {
    assert_rejected_with(
        "tests/fixtures/traits_unknown.kp",
        "names no declared trait",
    );
}

#[test]
pub(crate) fn the_trait_shapes_this_slice_does_not_reach_are_refused_by_name() {
    assert_rejected_with(
        "tests/fixtures/traits_dyn.kp",
        "is not a type on its own",
    );
    assert_rejected_with(
        "tests/fixtures/traits_associated_type.kp",
        "default associated type is not in this slice",
    );
    // `impl Trait` in argument position was on this list and is now a shipped
    // sugar (A-20, 9.3);
    // `impl_trait_arguments_are_fresh_bounded_type_parameters` pins the
    // positive half, and the stored positions stay refused.
    // A `where` clause was on this list and is now the other spelling of a
    // bound (26.10.3); `a_trait_can_be_implemented_for_a_generic_type_under_bounds`
    // pins it.
    assert_compiles_and_runs("tests/fixtures/traits_where_clause.kp", "");
    assert_rejected_with(
        "tests/fixtures/traits_drop_declared.kp",
        "the trait 'Drop' is built in",
    );
}

/// A-12 slice 3: a trait method may carry a default body. An impl may take it
/// as written, override it, or omit every method of an all-default trait, and a
/// default may call the trait's other methods through `self`.
#[test]
pub(crate) fn a_default_method_is_inherited_overridden_and_may_call_the_traits_others() {
    assert_compiles_and_runs(
        "tests/fixtures/traits_default_body.kp",
        "9\n9\n18\n5\n",
    );
}

/// A-12 slice 3: `trait B: A` requires an `impl A for` the type, lets a
/// `<T: B>` call `A`'s methods, and lets a `T: B` satisfy a `T: A` bound.
#[test]
pub(crate) fn a_supertrait_is_required_and_its_methods_are_callable_through_the_subtrait() {
    assert_compiles_and_runs(
        "tests/fixtures/traits_supertrait.kp",
        "12\n21\n",
    );
}

/// A-12 slice 3: a trait implemented for a builtin type that already carries
/// methods (`i32`, `String`) is reached through a generic exactly as a
/// `datac`'s impl is.
#[test]
pub(crate) fn a_trait_implemented_for_a_builtin_is_called_through_a_generic() {
    assert_compiles_and_runs(
        "tests/fixtures/traits_impl_for_builtin.kp",
        "20\n36\na string\n",
    );
}

/// A-12 slice 3: a default body is resolved in the trait's module even when
/// the impl that inherits it lives in the implementing type's module, so it can
/// name the trait module's own types.
#[test]
pub(crate) fn a_default_body_resolves_in_the_traits_own_module() {
    assert_project_runs(
        "traits_default_cross",
        &project("traits_default_cross"),
        "10\n10\n",
    );
}

/// A-12 slice 3 rejections: a missing supertrait impl, a supertrait cycle, an
/// override that does not match the defaulted signature, a default body that
/// calls an undeclared method, and a builtin impl outside the trait's module.
#[test]
pub(crate) fn the_new_trait_shapes_are_refused_by_name() {
    assert_rejected_with(
        "tests/fixtures/traits_missing_supertrait.kp",
        "is missing its supertrait",
    );
    assert_rejected_with(
        "tests/fixtures/traits_supertrait_cycle.kp",
        "supertrait relation is cyclic",
    );
    assert_rejected_with(
        "tests/fixtures/traits_default_wrong_override.kp",
        "does not match trait 'Shape'",
    );
    assert_rejected_with(
        "tests/fixtures/traits_default_undeclared_method.kp",
        "has no method 'nope'",
    );
    assert_project_rejected(
        "traits_builtin_orphan",
        &project("traits_builtin_orphan"),
        "is a builtin type, which has no module",
    );
}

/// `Self` in a trait signature is the implementing type wherever it is written,
/// not only in the receiver: a parameter, a return type, and a `T` returned by a
/// bounded generic all resolve per instantiation.
#[test]
pub(crate) fn self_stands_for_the_implementing_type_everywhere_in_a_signature() {
    assert_compiles_and_runs(
        "tests/fixtures/traits_self_in_signature.kp",
        "1\n2\n",
    );
}

/// A-12 slice 4: two types implementing one trait, called through a `&dyn` in
/// a loop over borrowed bindings.
#[test]
pub(crate) fn two_types_are_dispatched_through_a_dyn_in_a_loop() {
    assert_compiles_and_runs(
        "tests/fixtures/traits_dyn_dispatch.kp",
        "circle\nsquare\n21.000000\n",
    );
}

/// A-12 slice 4: a default method and a supertrait method are both callable
/// through a `&dyn` of the subtrait.
#[test]
pub(crate) fn a_default_and_a_supertrait_method_are_callable_through_a_dyn() {
    assert_compiles_and_runs(
        "tests/fixtures/traits_dyn_default_super.kp",
        "circle\na shape\n16.000000\n",
    );
}

/// A-12 slice 4: `&mut dyn` mutates its target through the vtable.
#[test]
pub(crate) fn a_mut_dyn_mutates_its_target() {
    assert_compiles_and_runs("tests/fixtures/traits_dyn_mut.kp", "11\n18\n");
}

/// A-12 slice 4: a `&dyn` binding forwards on to another function.
#[test]
pub(crate) fn a_dyn_borrow_forwards_to_another_function() {
    assert_compiles_and_runs(
        "tests/fixtures/traits_dyn_forward.kp",
        "36.000000\n18.000000\n",
    );
}

/// A-12 slice 4: a trait implemented for a `Copy` builtin is reached through a
/// `&dyn`, so the thunk reads the scalar out of the erased pointer.
#[test]
pub(crate) fn a_builtin_impl_is_reached_through_a_dyn() {
    assert_compiles_and_runs("tests/fixtures/traits_dyn_builtin.kp", "84\n");
}

/// A-12 slice 4 rejections: an object-unsafe trait that returns `Self` or takes
/// another `Self`, a bare `dyn` that is neither borrowed nor boxed, a trait
/// object in a field, a returned `&dyn`, a type that does not implement the
/// trait, and an unknown trait.
#[test]
pub(crate) fn the_dyn_shapes_this_slice_does_not_reach_are_refused_by_name() {
    assert_rejected_with(
        "tests/fixtures/traits_dyn_unsafe_self.kp",
        "returns 'Self'",
    );
    assert_rejected_with(
        "tests/fixtures/traits_dyn_unsafe_param.kp",
        "takes a 'Self' parameter",
    );
    assert_rejected_with(
        "tests/fixtures/traits_dyn.kp",
        "is not a type on its own",
    );
    assert_rejected_with(
        "tests/fixtures/traits_dyn_field.kp",
        "is a borrowed type",
    );
    assert_rejected_with(
        "tests/fixtures/traits_dyn_return.kp",
        "cannot be a return type",
    );
    assert_rejected_with(
        "tests/fixtures/traits_dyn_not_impl.kp",
        "does not implement 'Shape'",
    );
    assert_rejected_with(
        "tests/fixtures/traits_dyn_unknown.kp",
        "names no declared trait",
    );
}

/// A-12 slice 5: a trait may declare an associated type, each impl binds it,
/// and `I::Item` is usable in a generic function's return type. Dispatch stays
/// static, so every call is the concrete type's own method.
#[test]
pub(crate) fn an_associated_type_is_bound_by_each_impl_and_projected_through_a_generic() {
    assert_compiles_and_runs(
        "tests/fixtures/traits_assoc_two_types.kp",
        "6\n12\n",
    );
}

/// A-12 slice 5: `type Entry: Named` is checked at the impl, and generic code
/// reaches the bound's method through the projection.
#[test]
pub(crate) fn a_bounded_associated_type_is_usable_inside_generic_code() {
    assert_compiles_and_runs("tests/fixtures/traits_assoc_bound.kp", "item\n5\n");
}

/// A-12 slice 5: `Self::Item` in a default method resolves through the impl's
/// binding when the concrete copy is synthesized.
#[test]
pub(crate) fn self_item_in_a_default_method_resolves_per_impl() {
    assert_compiles_and_runs(
        "tests/fixtures/traits_assoc_default_self.kp",
        "107\n107\n",
    );
}

/// A-12 slice 5: a `pub trait` with an associated type, implemented in another
/// module and used through a generic function whose return type names the
/// projection.
#[test]
pub(crate) fn an_associated_type_is_bound_across_modules() {
    assert_project_runs(
        "traits_assoc_cross",
        &project("traits_assoc_cross"),
        "1\n",
    );
}

/// A-12 slice 5 rejections: a missing binding, an extra one, a projection with
/// no bound to name it, an equality constraint, and a bound the impl's type
/// does not satisfy.
#[test]
pub(crate) fn the_associated_type_shapes_are_refused_by_name() {
    assert_rejected_with(
        "tests/fixtures/traits_assoc_missing.kp",
        "does not bind it",
    );
    assert_rejected_with(
        "tests/fixtures/traits_assoc_extra.kp",
        "is not declared by trait 'Src'",
    );
    assert_rejected_with(
        "tests/fixtures/traits_assoc_no_bound.kp",
        "is not a known associated type",
    );
    assert_rejected_with(
        "tests/fixtures/traits_assoc_equality.kp",
        "declares no associated type 'Missing'",
    );
    assert_rejected_with(
        "tests/fixtures/traits_assoc_bound_violated.kp",
        "does not satisfy the bound 'type Entry: Named'",
    );
    assert_rejected_with(
        "tests/fixtures/traits_assoc_const.kp",
        "associated consts are not in this slice",
    );
    assert_rejected_with(
        "tests/fixtures/traits_assoc_gat.kp",
        "generic associated type is not in this slice",
    );
}

/// A-12 slice 6: a trait may take type parameters, a receiverless method is a
/// static member, and `?` calls a declared `impl From<Source> for Target` when
/// the two failure payload types differ. Dispatch stays static, so both backends
/// emit the same direct calls.
#[test]
pub(crate) fn a_from_impl_converts_a_differing_failure_and_a_generic_trait_is_bounded() {
    assert_compiles_and_runs(
        "tests/fixtures/from_conversion.kp",
        "true\ntrue\n5\n9\n42\n5\n",
    );
}

/// A-12 slice 6: a static method written on the trait's own name
/// (`From.from(x)`), with the implementing type from the expected type.
#[test]
pub(crate) fn a_static_method_is_called_on_the_traits_own_name() {
    assert_compiles_and_runs("tests/fixtures/from_trait_name.kp", "5\n9\n");
}

/// A-12 slice 6: the `From` impl lives in the implementing type's module, and
/// the `?` that reaches it is in another module.
#[test]
pub(crate) fn a_from_impl_is_reached_across_modules() {
    assert_project_runs("convert_cross", &project("convert_cross"), "true\n");
}

/// A-12 slice 6 rejections: `?` with no conversion, a second impl of one trait
/// instance for one type, `&dyn` of a generic trait, and an impl that writes
/// the wrong number of trait arguments.
#[test]
pub(crate) fn the_generic_trait_shapes_this_slice_does_not_reach_are_refused_by_name() {
    assert_rejected_with(
        "tests/fixtures/from_no_impl.kp",
        "cannot convert a failure",
    );
    assert_rejected_with("tests/fixtures/from_no_impl.kp", "impl From<IoErr>");
    assert_rejected_with(
        "tests/fixtures/from_duplicate_impl.kp",
        "already has an impl for 'T'",
    );
    assert_rejected_with(
        "tests/fixtures/generic_trait_dyn.kp",
        "takes type parameters",
    );
    assert_rejected_with(
        "tests/fixtures/generic_trait_arity.kp",
        "takes 1 type argument(s)",
    );
}

/// A-12 slice 6: `std/convert` is pure language, so a bare-metal image uses
/// `From` and `?`'s conversion and boots with no undefined symbol.
#[test]
pub(crate) fn freestanding_convert_boots_with_a_converted_failure() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU) {
        println!(
            "skipping the freestanding convert boot: 'ld.lld' or 'qemu-system-riscv64' \
             is not on PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    boot_under_qemu(
        &RISCV64_QEMU,
        "freestanding_convert",
        &project("freestanding_convert"),
        &["converted 107", "done"],
    );
}

/// A-12 slice 4: a freestanding riscv64 image dispatches through a `&dyn` and a
/// `&mut dyn` over the UART, and the linked ELF references no undefined symbol,
/// which is to say the vtable is `.rodata`, the thunks are ordinary code, and
/// no `kelp_rt` or libc helper is reached.
#[test]
pub(crate) fn a_freestanding_image_dispatches_through_a_dyn_and_boots() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU) {
        println!(
            "skipping the freestanding dyn boot: 'ld.lld' or 'qemu-system-riscv64' is not on \
             PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = out_path("freestanding_dyn");
    let build = Command::new(kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .arg("build")
        .arg(project("freestanding_dyn"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the freestanding dyn build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let path = bin.to_str().expect("the artifact path is utf-8");
    let symbols = tool("llvm-nm", &[path]);
    let undefined: Vec<&str> = symbols
        .lines()
        .filter(|line| line.split_whitespace().next() == Some("U"))
        .collect();
    assert!(
        undefined.is_empty(),
        "the freestanding dyn image has undefined symbols:\n{}",
        undefined.join("\n")
    );
    boot_under_qemu(
        &RISCV64_QEMU,
        "freestanding_dyn",
        &project("freestanding_dyn"),
        &["hello from dyn", "1", "bye from dyn", "2"],
    );
}

/// A-12 slice 7: a `List<Box<dyn Shape>>` of two implementors, each element a
/// fat owned pointer dispatched through its own vtable and freed through it when
/// the list drops.
#[test]
pub(crate) fn a_list_of_owned_trait_objects_is_dispatched_through_each_vtable() {
    // Both implementors are memory, so a `Box<dyn Shape>` is memory and the list
    // may hold it in a hosted build (the classifier counts the coerced types).
    assert_compiles_and_runs(
        "tests/fixtures/traits_boxdyn_list.kp",
        "circle\nsquare\n21.000000\n",
    );
}

/// A-12 slice 7: a factory function returns a `Box<dyn Shape>` chosen by a flag,
/// and a `&dyn` borrowed back out of a box goes on to a free function.
#[test]
pub(crate) fn a_factory_returns_an_owned_trait_object_and_the_trait_is_borrowed_back() {
    assert_compiles_and_runs(
        "tests/fixtures/traits_boxdyn_factory.kp",
        "circle\nsquare\n12.000000\n9.000000\n",
    );
}

/// A-12 slice 7: a `datac` holds a `Box<dyn Trait>` field, a `mut Self` method is
/// dispatched through the box's vtable, and each concrete type's user `drop`
/// runs through the vtable when its box dies.
#[test]
pub(crate) fn a_datac_field_holds_an_owned_trait_object_and_drops_it_through_the_vtable() {
    // Decision 21: a hosted build refuses this program, so the test asserts the refusal.
    assert_fixture_refused("traits_boxdyn_field", "'Circle' cannot be the payload of a Box");
}

/// A-12 slice 7: an `enum` payload holds an owned trait object, and the live
/// variant's payload drops through the vtable when the binding that holds it
/// dies.
#[test]
pub(crate) fn an_enum_payload_holds_an_owned_trait_object_and_drops_it_through_the_vtable() {
    // Decision 21: a hosted build refuses this program, so the test asserts the refusal.
    assert_fixture_refused("traits_boxdyn_enum", "'Circle' cannot be the payload of a Box");
}

/// A-12 slice 7: a box moves between lists with `pop`/`push`, `Box.empty()` is
/// the null owned trait object, and a `move Box<dyn Trait>` parameter owns one
/// by value.
#[test]
pub(crate) fn an_owned_trait_object_moves_between_lists_and_through_a_move_parameter() {
    assert_compiles_and_runs(
        "tests/fixtures/traits_boxdyn_move.kp",
        "true\n12.000000\n0\nfalse\n12.000000\n0\n",
    );
}

/// The classifier gap, named: a `Box<dyn Trait>` whose implementors are all
/// memory is a memory value, so it may be a `List` element in a hosted build.
/// A trait with a resource implementor stays refused.
#[test]
pub(crate) fn a_box_of_a_trait_with_a_resource_implementor_is_still_refused_in_a_list() {
    let source = "trait Shape { fun Self.area() => f64 }\n\
datac Circle(val r: f64)\n\
fun mut Circle.drop() { print(\"drop\") }\n\
impl Shape for Circle { fun Self.area() => f64 = r }\n\
fun main() {\n\
    var xs: List<Box<dyn Shape>> = List.empty()\n\
    xs.push(Box.of(Circle(r = 1.0)))\n\
    print(xs.len())\n\
}\n";
    let name = "box_dyn_resource_implementor";
    let src = out_path(name).with_extension("kp");
    std::fs::write(&src, source).expect("failed to write the regression source");
    let build = kelpc()
        .arg("build")
        .arg(&src)
        .arg("-o")
        .arg(out_path(name))
        .output()
        .expect("failed to spawn kelpc");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );
    assert!(!build.status.success(), "a hosted build must refuse a resource implementor, got:\n{text}");
    assert!(
        text.contains("'Box<dyn Shape>' is a resource"),
        "expected the resource refusal for the trait object, got:\n{text}"
    );
}

/// A-12 slice 7: an owned trait object crosses a thread boundary (an owned value
/// with no borrows is what the crossing rule admits), and the worker drops it
/// through the vtable when it returns.
#[test]
pub(crate) fn an_owned_trait_object_crosses_a_thread_boundary() {
    // The worker owns the box, so its drop runs when `area_of` returns, before
    // the join hands the result back to the main thread to print.
    // Decision 21: a hosted build refuses this program, so the test asserts the refusal.
    assert_fixture_refused("traits_boxdyn_thread", "'Circle' cannot be the payload of a Box");
}

/// A-12 slice 7 rejections: a coercion of a type that does not implement the
/// trait, an object-unsafe trait behind `Box<dyn>`, `Box<dyn A + B>`, a use
/// after a move, a partial move out of a `datac` field, a borrowed trait object
/// crossing a thread boundary, and `take`/`set` on an owned trait object.
#[test]
pub(crate) fn the_owned_trait_object_shapes_this_slice_refuses_are_refused_by_name() {
    assert_rejected_with(
        "tests/fixtures/traits_boxdyn_not_impl.kp",
        "does not implement 'Shape'",
    );
    assert_rejected_with(
        "tests/fixtures/traits_boxdyn_object_safe.kp",
        "'Box<dyn Maker>' is not allowed",
    );
    assert_rejected_with(
        "tests/fixtures/traits_boxdyn_plus.kp",
        "`dyn` names one trait",
    );
    assert_rejected_with(
        "tests/fixtures/traits_boxdyn_use_after_move.kp",
        "use of moved value 'b'",
    );
    assert_rejected_with(
        "tests/fixtures/traits_boxdyn_partial_move.kp",
        "use of moved value 'h.shape'",
    );
    assert_rejected_with(
        "tests/fixtures/traits_boxdyn_thread_borrow.kp",
        "cannot send '&dyn Shape'",
    );
    assert_rejected_with(
        "tests/fixtures/traits_boxdyn_take.kp",
        "'Box<dyn Shape>.take' is not allowed",
    );
    assert_rejected_with(
        "tests/fixtures/traits_boxdyn_generic_trait.kp",
        "takes type parameters",
    );
    assert_rejected_with(
        "tests/fixtures/traits_boxdyn_mut_while_borrowed.kp",
        "already borrowed as shared",
    );
}

/// A-12 slice 7 in a bare-metal image: two owned trait objects built through the
/// project's own allocator are dispatched through `.rodata` vtables and freed
/// through each vtable's drop entry, so the user `drop`s reach the UART in drop
/// order and the linked ELF references no undefined symbol.
#[test]
pub(crate) fn a_freestanding_image_owns_trait_objects_and_frees_them_through_the_vtable() {
    if !have_ld_lld() || !have_qemu(&RISCV64_QEMU) {
        println!(
            "skipping the freestanding boxdyn boot: 'ld.lld' or 'qemu-system-riscv64' is not on \
             PATH (run the suite under dev-kernel.nix)"
        );
        return;
    }
    let bin = out_path("freestanding_boxdyn");
    let build = Command::new(kelpc_bin())
        .env_remove("KELPC_BACKEND")
        .arg("build")
        .arg(project("freestanding_boxdyn"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to spawn kelpc");
    assert!(
        build.status.success(),
        "the freestanding boxdyn build failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let path = bin.to_str().expect("the artifact path is utf-8");
    let symbols = tool("llvm-nm", &[path]);
    let undefined: Vec<&str> = symbols
        .lines()
        .filter(|line| line.split_whitespace().next() == Some("U"))
        .collect();
    assert!(
        undefined.is_empty(),
        "the freestanding boxdyn image has undefined symbols:\n{}",
        undefined.join("\n")
    );
    boot_under_qemu_in_order(
        &RISCV64_QEMU,
        "freestanding_boxdyn",
        &project("freestanding_boxdyn"),
        &[
            "hello from boxdyn",
            "1",
            "bye from boxdyn",
            "2",
            "hello from boxdyn",
            "drop hello",
            "drop bye",
            "drop hello",
            "boxdyn done",
        ],
    );
}

/// A trait declared in one module and implemented in another: the type's own
/// module implements the imported trait, and the trait's own module implements
/// it for a type from a third module that imports nothing. Both are at a
/// module boundary, so both need `pub` and the import that makes the name
/// reachable, and the call site in the entry module names neither trait.
#[test]
pub(crate) fn a_trait_crosses_a_module_boundary_on_both_allowed_sides() {
    assert_project_runs(
        "traits_cross",
        &project("traits_cross"),
        "12.000000\n27.000000\ncircle\nring\n12.000000\n",
    );
}

/// An impl in a third module, which owns neither the trait nor the type.
#[test]
pub(crate) fn an_impl_in_a_module_that_owns_neither_the_trait_nor_the_type_is_rejected() {
    assert_project_rejected(
        "traits_third_module",
        &project("traits_third_module"),
        "which owns neither",
    );
}

#[test]
pub(crate) fn a_declared_key_pair_that_is_half_or_misshaped_is_rejected() {
    // A-12: `hash` states that the type is a key, and a key needs both halves,
    // so a lone `hash`, or a signature the generated map code cannot call, is
    // refused at the declaration with the reason named rather than as a vague
    // "not a key" at a `Map` use site. A lone `eq` is *not* refused (H-5): it
    // declares the type's `==`/`!=` without making it a key, and
    // `map_key_pair_only_eq.kp` is an accepted fixture beside this one.
    assert_rejected_with(
        "tests/fixtures/map_key_pair_only_hash.kp",
        "'Half' declares 'hash' but not 'eq'",
    );
    assert_rejected_with(
        "tests/fixtures/map_key_pair_hash_ret.kp",
        "'Key.hash' must return 'u64', found 'i32'",
    );
    assert_rejected_with(
        "tests/fixtures/map_key_pair_eq_owned.kp",
        "'Key.eq' must take 'other: &Key'",
    );
    assert_rejected_with(
        "tests/fixtures/map_key_pair_eq_arity.kp",
        "'Key.eq' takes exactly one parameter",
    );
    assert_rejected_with(
        "tests/fixtures/map_key_pair_mut.kp",
        "'Key.hash' cannot take 'mut'",
    );
    assert_rejected_with(
        "tests/fixtures/map_key_pair_generic.kp",
        "'Pair.hash' cannot have type parameters",
    );
    // The pair is the escape hatch from the derived *key* set, not from the
    // deep copy the map's own entry needs.
    assert_rejected_with(
        "tests/fixtures/map_key_pair_no_deep_copy.kp",
        "'WithList' has no deep copy",
    );
}

#[test]
pub(crate) fn an_enum_whose_variant_payloads_are_keyable_is_a_map_key() {
    // Decision 21: a hosted build refuses this program, so the test asserts the refusal.
    assert_fixture_refused("map_enum_key", "describes the enum 'Key' only as a heap object");
}

#[test]
pub(crate) fn an_enum_map_value_is_refused_by_the_checker_with_a_list_hint() {
    // A `Map` value that is an `enum` is refused at the `Map` type, before codegen.
    // The message names the type and says to keep the values in a `List`, which
    // holds an `enum` as its own element; a `datac` wrapper is refused in codegen too.
    assert_fixture_refused(
        "map_enum_value",
        "'Status' cannot be a 'Map' value: a hosted build cannot describe an 'enum' inside a map's entry yet",
    );
    assert_fixture_refused("map_enum_value", "Keep the values in a 'List<Status>' and store each value's index in the map instead");
}

#[test]
pub(crate) fn a_map_is_not_copy_so_assigning_one_moves_it() {
    assert_rejected("tests/fixtures/map_move.kp");
}

#[test]
pub(crate) fn a_missing_map_key_panics() {
    assert_fixture_panics("map_missing_key", "Map.get: key not found");
    assert_fixture_panics("map_remove_missing", "Map.remove: key not found");
}

#[test]
pub(crate) fn a_map_grows_by_rehashing_and_keeps_insertion_order() {
    // Twenty-three inserts cross two index capacities, and the six removes
    // before the twenty-fourth insert leave tombstones the rebuild has to
    // skip; `keys()` must still be insertion order with the removed keys gone.
    assert_compiles_and_runs(
        "tests/fixtures/map_growth_and_rehash.kp",
        "23\n220\n50\n60\n70\n80\n90\n100\n17\nfalse\n18\ntrue\n1000\n18\n0\n1\n2\n3\n4\n11\n12\n13\n14\n15\n16\n17\n18\n19\n20\n21\n22\n100\n",
    );
}

#[test]
pub(crate) fn a_map_sized_with_with_capacity_grows_and_behaves_like_a_new_one() {
    // Sixty-four entries of capacity for two hundred inserts crosses both
    // growth thresholds, so the hint has to leave the map in the same state
    // `Map.new()` would: same insertion order, same tombstones, same lookups.
    assert_compiles_and_runs(
        "tests/fixtures/map_with_capacity.kp",
        "200\n597\nfalse\n70\n1\n20\n10\nfalse\n30\n200\n0\n199\n99\n",
    );
    assert_rejected("tests/fixtures/map_with_capacity_arity.kp");
}

#[test]
pub(crate) fn a_list_sized_with_with_capacity_grows_like_an_empty_one() {
    assert_compiles_and_runs(
        "tests/fixtures/list_with_capacity.kp",
        "0\n100\n0\n198\ntrue\n7\n2\nb\n",
    );
    assert_rejected("tests/fixtures/list_with_capacity_arity.kp");
}

#[test]
pub(crate) fn a_map_probe_steps_over_a_tombstone_and_still_finds_the_live_key() {
    // "k0", "k8" and "k13" share one probe chain, so the tombstone `remove`
    // leaves at the chain's head is what a probe has to step past.
    assert_compiles_and_runs(
        "tests/fixtures/map_tombstones_and_reuse.kp",
        "2\n1\n1\nfalse\ntrue\n2\n2\ntrue\n3\n2\n3\ntrue\n9\n2\n3\n",
    );
}

#[test]
pub(crate) fn a_map_entry_keeps_the_value_at_the_key_width_offset() {
    assert_compiles_and_runs(
        "tests/fixtures/map_key_widths.kp",
        "2\n1000000007\n1000000009\n2\n2000000011\n2000000013\n2\n3000000017\n3000000019\n2\n4000000023\n4000000029\n2\n5000000031\n5000000037\n",
    );
}

#[test]
pub(crate) fn replacing_a_map_value_drops_the_old_one() {
    // The stdout is the same whether or not the old value is dropped; the
    // replacement path is a valgrind check (`memory_gate.rs` sweeps every
    // fixture), and this run only makes each replacement observable.
    assert_compiles_and_runs(
        "tests/fixtures/map_replace_drops_old_value.kp",
        "2\n2\nONE\n2\n1\nTWO\n2\nTWO\ntwo again\n2\n2\n1\ntwo again\n",
    );
}

#[test]
pub(crate) fn map_string_values_survive_the_whole_ownership_path() {
    assert_compiles_and_runs(
        "tests/fixtures/map_owned_round_trip.kp",
        "3\ntwo\nthree\nfallback\none\n2\n2\nbeta\ngamma\n2\ntwo\nthree\n0\ntrue\n",
    );
}

#[test]
pub(crate) fn a_cleared_map_is_empty_and_reusable() {
    assert_compiles_and_runs(
        "tests/fixtures/map_clear_then_reuse.kp",
        "10\n81\n0\ntrue\nfalse\n2\n33\n44\ntrue\n44\n1\nfalse\n1\n3\n",
    );
}

#[test]
pub(crate) fn a_shared_borrow_of_a_temporary_is_allowed_for_a_call() {
    assert_compiles_and_runs("tests/fixtures/borrow_temp_arg.kp", "3\n18\n");
}

#[test]
pub(crate) fn each_borrowed_temporary_gets_a_slot_of_its_own() {
    // MK7 §5 item 4, fourth slice: a temporary's address has to survive the
    // whole enclosing expression, including when a second temporary is built
    // before the first call reads it (`pair_sum(Pair(..), Pair(..))`), when the
    // temporary is itself a call's result (`sum(make(7, 8))`), and when it is
    // wider than one eightbyte (`quad_sum(Quad(..))`).
    assert_compiles_and_runs(
        "tests/fixtures/borrow_temp_slots.kp",
        "3\n18\n15\n6\n30\n6\n6\n",
    );
}

#[test]
pub(crate) fn a_borrow_from_a_temporary_bound_to_a_local_is_allowed() {
    assert_compiles_and_runs("tests/fixtures/borrow_temp_returned.kp", "x\n");
}

#[test]
pub(crate) fn a_binding_keeps_its_temporary_borrow_owner_alive() {
    assert_compiles_and_runs("tests/fixtures/borrow_temp_binding.kp", "hello\n");
}

#[test]
pub(crate) fn a_datac_can_own_the_text_its_view_field_borrows() {
    assert_compiles_and_runs("tests/fixtures/aggregate_text_borrow.kp", "label\n");
    assert_rejected_with(
        "tests/fixtures/refused_aggregate_text_borrow.kp",
        "not moved into an earlier String/Bytes field",
    );
}

/// The field spelling of the same refusal: `mkName().name` is a field of an
/// rvalue, so a borrow that outlives the call has no storage to live in. It
/// is checked by message, not just by rejection, because the shape *does*
/// build whenever the borrow is only read inside the call. The two have to be
/// told apart, and this is the one that must not quietly start compiling.
#[test]
pub(crate) fn a_borrow_from_a_temporary_field_bound_to_a_local_is_allowed() {
    assert_compiles_and_runs("tests/fixtures/borrow_field_of_temp_returned.kp", "x\n");
}

/// And the explicit-borrow half: `&<field of a temporary>` is not a place, so
/// binding one is refused in the compiler's own voice rather than by `cc`
/// objecting to `&(rvalue).field`.
#[test]
pub(crate) fn an_explicit_borrow_of_a_field_of_a_temporary_is_rejected() {
    assert_rejected_with(
        "tests/fixtures/borrow_field_of_temp.kp",
        "cannot borrow a temporary value",
    );
}

/// The other side of the same rule (`syntax.md` 10.2): a chain that steps
/// through a *borrow* has storage that outlives the statement, so an explicit
/// `&` on it is legal even though the call's own result is not a place. The
/// shallow AST question refused this until the check became the typed one.
#[test]
pub(crate) fn an_explicit_borrow_of_a_field_of_a_borrow_is_accepted() {
    assert_compiles_and_runs(
        "tests/fixtures/borrow_chain_through_a_borrow.kp",
        "42\nforty-two\n43\n7\n",
    );
}

#[test]
pub(crate) fn fs_list_returns_a_granted_directories_entries() {
    // A granted `stdlib/fs` module listing a directory it owns: the entries
    // come back as a `List<String>`, sorted, and a recursive clear built on
    // the same function leaves the directory gone.
    assert_project_runs(
        "caps_list",
        &caps_project("caps_list").join("main.kp"),
        "3\na.txt,b.txt,sub\na.txt b.txt sub \nfalse\n",
    );
}

#[test]
pub(crate) fn fs_breadth_creates_walks_stats_copies_opens_and_removes_under_one_grant() {
    // MK5-S5: `mkdir_all`, `read_lines`, `metadata`, `copy`, `open` (its mode
    // string is not a path), `touch`, `walk`, `temp_path`, `remove_all`, all
    // behind the one `fs` grant, all in a scratch directory the program
    // creates and removes itself.
    let _scratch = TestScratch::new();
    let target = rerouted_copy(&project("caps_fs_breadth").join("main.kp"), "caps_fs_breadth_src");
    let expected = reroute_tmp(
        "2\nline one\nline two\n18\ntrue\nfalse\ntrue\nfalse\ntop\ntrue\n7\n/tmp/kelp_fs_breadth_test/a/b/c/note.txt\ntrue\ntrue\nfalse\n",
    );
    assert_project_runs("caps_fs_breadth", &target, &expected);
}

#[test]
pub(crate) fn path_stem_extension_and_component_helpers() {
    // `stem` allocates (a prefix cannot be a view); `split` drops empty
    // components; `join_all` applies `join`'s absolute-part rule.
    assert_compiles_and_runs(
        "tests/fixtures/path_polish.kp",
        "archive.tar\nMakefile\n.bashrc\ntrue\nfalse\n2\na\nb\n/a/b/c\n/etc/x\n",
    );
}

#[test]
pub(crate) fn path_absolute_resolves_a_relative_argument_against_a_base() {
    // MISSING-hW5rB: `fs.cwd` is the base a command-line tool has, and the
    // resolved path is normalized so the capability check accepts it. The
    // absolute cases are compared to literals; the cwd-dependent ones assert
    // the property instead, since the test's own directory is not fixed.
    assert_compiles_and_runs(
        "tests/fixtures/path_absolute.kp",
        "/a/b/c.txt\n/a/c.txt\n/abs/x\ntrue\ntrue\n",
    );
}

/// MK7 §5 item 7 step 1's other half: a call whose wide return value is
/// discarded. A return wider than one eightbyte is written by the callee
/// through a hidden pointer, so the caller supplies the storage even when the
/// value is dead, which is the compiler temporary passed at the same site. The gated fixtures needed the `unit` body's tail shape
/// (`fun main() { fs.read(p) }`); this fixture pins that plus the statement and
/// function-value shapes no fixture reached before.
#[test]
pub(crate) fn a_discarded_wide_return_still_builds() {
    assert_compiles_and_runs("tests/fixtures/discard_wide_return.kp", "discarded\n");
}

#[test]
pub(crate) fn a_narrow_fs_grant_still_reaches_cwd() {
    assert_project_runs(
        "caps_cwd",
        &caps_project("caps_cwd").join("main.kp"),
        "true\n",
    );
}

#[test]
pub(crate) fn a_symlink_is_seen_reported_and_left_alone_by_the_walk() {
    // MISSING-cK4hT: `is_symlink`/`read_link` classify a link, a broken link
    // still has a target text, `walk` reports a link without descending it (so
    // the link back to the root terminates the walk), and `remove_all` unlinks
    // a link instead of walking into it (BUG-a9Km4: the target's own file
    // survives).
    let _scratch = TestScratch::new();
    let target = rerouted_copy(&fixture("tests/fixtures/fs_symlink_kind.kp"), "fs_symlink_kind_src");
    assert_project_runs(
        "fs_symlink_kind",
        &target,
        &reroute_tmp("true\ntrue\ntrue\nfalse\n/tmp/kelp_fs_symlink_kind/note.txt\ntrue\nfalse\n8\nfalse\ntrue\nfalse\n"),
    );
}
