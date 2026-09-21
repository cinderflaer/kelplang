#!/bin/sh
# The MK7 S3 gate, the rest of the runtime slice: `pd` globals, the capability
# check, and the last three refusals.
#
# S3 is "the runtime, the externs and the capability check"
# (plan/mk7-llvm-backend.md §5). S3A landed the `extern fun` bodies; this half
# lands what was left and empties the refusal ranking, which is §4.1's
# "the refusal ranking empty":
#
#   * `pd` globals: one module-scope global per passdown with internal linkage
#     and a zero initializer, the generated `kelp_init_pds` that runs the
#     initializers in declaration order (a chained `pd val doubled = base * 2`
#     reads what an earlier one wrote), the `atexit` registration of
#     `kelp_drop_pds` for an owned passdown, and the `argv` `main` saves into the
#     runtime's own globals.
#   * the capability check: the same call sites `codegen_c.rs::cap_plan` guards,
#     against the same compiled-in per-`(module, domain)` allowlist, with the
#     resource argument evaluated *once* into a value the check and the call
#     share (BUG-Kf7Rn). The check functions are `runtime/cap_glue.c` itself,
#     compiled into the per-program object, and the deny path's text is
#     therefore the C backend's byte for byte.
#   * the last three refusals: `String#into_bytes` through the runtime object's
#     pointer-spelled adapter, and a field or a method receiver of an owned
#     temporary, staged and dropped at the expression that read through it
#     (`codegen_c.rs::staged_field_value`).
#
# The list is closed by `--exact`: a fixture that has grown a construct outside
# this slice is refused by name and is deliberately *not* here. The earlier
# slices' lists are carried by `kelpc/tests/run-llvm-externs.sh`, which stays
# green: this gate is the new rows, not a replacement.
#
# The fixtures and tests that make this slice worth its own gate:
#
#   * `passdown_storage_is_a_global_that_main_initializes` (`passdown_globals.kp`)
#     is the chained initializer, a write through a callee that names no
#     passdown of its own, and a `datac` passdown.
#   * `an_owned_passdown_is_freed_at_exit` (`pd_owned.kp`) is the `atexit` drop
#     of a `String` passdown: without the drop, valgrind sees the leak.
#   * `a_list_works_as_a_datac_field_and_as_an_owned_passdown`
#     (`list_field.kp`) is a container-typed passdown.
#   * `a_path_outside_the_grant_is_denied_at_runtime` (`caps_denied`) and
#     `a_path_outside_the_grant_is_denied_at_a_primitive_too`
#     (`caps_raw_denied`) assert the *exact* deny text and exit status, so the
#     two backends' deny paths are compared character for character.
#   * `a_gated_resource_argument_is_evaluated_once_and_the_control_is_denied`
#     (`caps_double_eval`) and `a_primitive_checks_its_resource_argument_once`
#     (`caps_raw_double_eval`) are BUG-Kf7Rn: the argument prints `SIDE` once, so
#     two evaluations would show as two lines and as a read of `/etc/passwd`.
#   * `string_constructs_measures_and_round_trips_through_bytes`
#     (`string_basic.kp`) is `String#into_bytes`, whose C form passes a 24-byte
#     struct by value and therefore crosses through the pointer-spelled adapter.
#   * `a_field_of_an_owned_temporary_is_dropped_per_read`
#     (`heap_field_of_owned_temporary.kp`) is the owned-temporary staging, and
#     `heap_field_of_temporary.kp` is the same shape with an unowned `i64` field
#     read through it; both are read back out of the IR below.
#
# The profile is `--release`, as the full gate `cargo test --release` is, so the
# two share one build tree.
#
# Usage, from anywhere:  ./kelpc/tests/run-llvm-pd-caps.sh
set -e

cd "$(dirname "$0")/../.."

BACKEND="${BACKEND:-llvm}"

# The fixtures and projects this slice turns from a refusal into a built target,
# by their test names: the `pd` and passdown fixtures, the ones that reach a
# gated call with a resource argument (`fs`, `net`, `process`), the two
# owned-temporary shapes, and the capability tests, which name the denied call
# site and the exit status as literals.
TESTS="
passdowns_are_read_by_referencing_functions_only
a_datac_passdown_is_read_through_a_shared_borrow
passdown_storage_is_a_global_that_main_initializes
an_owned_passdown_is_freed_at_exit
a_pd_var_is_writable_from_a_function_and_from_main
a_pd_var_write_reaches_the_callers_that_hold_it
a_local_can_shadow_a_passdown
a_passdown_is_forwarded_through_a_caller_that_does_not_name_it
a_pd_var_of_a_two_word_datac_is_writable_through_its_borrow
reading_a_copy_passdown_twice_in_one_call_is_allowed
a_pd_var_no_function_writes_is_a_shared_borrow
a_list_works_as_a_datac_field_and_as_an_owned_passdown
a_function_name_is_a_value_and_a_value_is_callable
a_for_loop_walks_a_list_without_an_index
a_fallible_conversion_returns_an_option_instead_of_panicking
fs_has_fallible_read_and_open
the_fs_path_family_reads_writes_and_walks
a_symlink_is_seen_reported_and_left_alone_by_the_walk
io_writes_bytes_text_and_a_borrowed_view
the_entry_module_holds_net_as_the_whole_resource_wildcard
a_socket_reports_readiness_without_blocking
resolving_a_name_returns_its_addresses
path_absolute_resolves_a_relative_argument_against_a_base
process_runs_a_program_with_argv_and_spawns_one_with_pipes
a_pooled_client_reuses_one_connection_for_five_requests
a_relative_redirect_is_followed_on_the_pooled_connection
the_pool_ceiling_comes_from_the_manifest
a_redirect_to_a_host_the_grant_does_not_name_is_denied
the_download_example_writes_exactly_what_it_received
string_constructs_measures_and_round_trips_through_bytes
a_field_of_an_owned_temporary_is_dropped_per_read
a_path_outside_the_grant_is_denied_at_runtime
a_gated_resource_argument_is_evaluated_once_and_the_control_is_denied
a_path_outside_the_grant_is_denied_at_a_primitive_too
a_primitive_checks_its_resource_argument_once
a_symlink_whose_target_is_outside_the_grant_cannot_be_created
an_ungranted_env_name_is_denied_before_the_call
the_net_check_matches_the_host_as_written
"

# `--exact` with one filter per test: libtest runs a test when it matches any
# filter, so this is one invocation and one build. `--exact` is what keeps a new
# fixture whose name merely contains a listed name out of the gate.
# shellcheck disable=SC2086
KELPC_BACKEND="$BACKEND" cargo test --release --test compile_and_run -- \
    --exact $TESTS

# The shapes those fixtures exercise, read back out of the IR the backend writes
# beside the binary (`<out>.ll`, MK7 §4.4).
#
# The fixtures with no `compile_and_run` test of their own are built directly
# here: `fs_errno`, `net_timeout`, `io_stdio`, `io_streams` and
# `heap_field_of_temporary` are coverage-sweep targets, and the assertion that
# matters for them is the IR shape, not an expected stdout.
REF_OUT="$(mktemp -d)"
trap 'rm -rf "$REF_OUT"' EXIT
KELPC_BACKEND="$BACKEND" ./target/release/kelpc build \
    kelpc/tests/fixtures/passdowns.kp -o "$REF_OUT/pd" >/dev/null
KELPC_BACKEND="$BACKEND" ./target/release/kelpc build \
    kelpc/tests/fixtures/pd_owned.kp -o "$REF_OUT/pdown" >/dev/null
KELPC_BACKEND="$BACKEND" ./target/release/kelpc build \
    kelpc/tests/fixtures/fs_errno.kp -o "$REF_OUT/fs" >/dev/null
KELPC_BACKEND="$BACKEND" ./target/release/kelpc build \
    kelpc/tests/fixtures/net_loopback.kp -o "$REF_OUT/netl" >/dev/null
KELPC_BACKEND="$BACKEND" ./target/release/kelpc build \
    kelpc/tests/fixtures/net_resolve.kp -o "$REF_OUT/netr" >/dev/null
KELPC_BACKEND="$BACKEND" ./target/release/kelpc build \
    kelpc/tests/fixtures/process_spawn.kp -o "$REF_OUT/ps" >/dev/null
KELPC_BACKEND="$BACKEND" ./target/release/kelpc build \
    kelpc/tests/fixtures/string_basic.kp -o "$REF_OUT/sb" >/dev/null
KELPC_BACKEND="$BACKEND" ./target/release/kelpc build \
    kelpc/tests/fixtures/heap_field_of_owned_temporary.kp -o "$REF_OUT/hfb" >/dev/null
KELPC_BACKEND="$BACKEND" ./target/release/kelpc build \
    kelpc/tests/fixtures/heap_field_of_temporary.kp -o "$REF_OUT/hfv" >/dev/null
KELPC_BACKEND="$BACKEND" ./target/release/kelpc build \
    kelpc/tests/projects/caps_net_denied/main.kp -o "$REF_OUT/cnd" >/dev/null

# MK7 S4 attaches a `!dbg` location to every instruction and every function
# definition. The assertions below are about the shape of the instruction, not
# about the source line it came from, so each IR file is read through a copy
# with the debug metadata stripped. That a statement resolves to a `.kp` line is
# a separate claim, checked end to end by `cargo test --test llvm_debug_info`.
strip_dbg() {
    sed -E 's/, !dbg ![0-9]+//g; s/ !dbg ![0-9]+//g' "$1" > "$1.nodbg"
    echo "$1.nodbg"
}

PD_IR="$(strip_dbg "$REF_OUT/pd.ll")"
PDOWN_IR="$(strip_dbg "$REF_OUT/pdown.ll")"
FS_IR="$(strip_dbg "$REF_OUT/fs.ll")"
NETL_IR="$(strip_dbg "$REF_OUT/netl.ll")"
NETR_IR="$(strip_dbg "$REF_OUT/netr.ll")"
PS_IR="$(strip_dbg "$REF_OUT/ps.ll")"
SB_IR="$(strip_dbg "$REF_OUT/sb.ll")"
HF_IR="$(strip_dbg "$REF_OUT/hfb.ll")"
HFV_IR="$(strip_dbg "$REF_OUT/hfv.ll")"
CND_IR="$(strip_dbg "$REF_OUT/cnd.ll")"

# A `pd` is a module-scope global with internal linkage and a zero initializer,
# the C backend's `static <type> <name>;`. The initializer function runs the
# initializers in declaration order, which is what a chained one reads.
for want in \
    '@kelp_pd__app_name = internal global ptr null' \
    '@kelp_pd__hit_count = internal global i32 0' \
    '  store ptr @str[.0-9]*, ptr @kelp_pd__app_name, align 8' \
    '  store i32 0, ptr @kelp_pd__hit_count, align 4'
do
    if ! grep -qxE "$want" "$PD_IR"; then
        echo "the passdown storage in '$PD_IR' is missing: $want" >&2
        exit 1
    fi
done
# `main` runs the initializers before its first statement.
if ! grep -qF '  call void @kelp_init_pds()' "$PD_IR"; then
    echo "'$PD_IR' does not initialize its passdowns from main" >&2
    exit 1
fi

# An owned `pd` is freed at process exit: the init function registers
# `kelp_drop_pds` with `atexit`, and the handler frees the passdown. Without
# this the valgrind sweep sees the leak.
for want in \
    '@kelp_pd__banner = internal global %kelp_String zeroinitializer' \
    'define void @kelp_drop_pds() {' \
    '  call void @kelp_string_drop(ptr @kelp_pd__banner)' \
    '  %0 = call i32 @atexit(ptr @kelp_drop_pds)'
do
    if ! grep -qxF "$want" "$PDOWN_IR"; then
        echo "the owned passdown in '$PDOWN_IR' is missing: $want" >&2
        exit 1
    fi
done

# `main` takes the C standard's `argc`/`argv` and saves them into the runtime's
# own globals, which is how `process.args()` reaches this process's argv.
for want in \
    '@kelp_argc = external global i32' \
    '@kelp_argv = external global ptr' \
    'define i32 @main(i32 %0, ptr %1) {' \
    '  store i32 %0, ptr @kelp_argc, align 4' \
    '  store ptr %1, ptr @kelp_argv, align 8'
do
    if ! grep -qxF "$want" "$PS_IR"; then
        echo "the saved argv in '$PS_IR' is missing: $want" >&2
        exit 1
    fi
done

# The capability check: the five entry points of `runtime/cap_glue.c`, declared
# with the C signatures, and one call per resource argument carrying the domain,
# the resource, the module's compiled-in array, its pattern count, the calling
# module and the `.kp` call site. The IR names the symbols the per-program object
# defines.
for want in \
    'declare void @kelp_cap_check_path(ptr, ptr, ptr, i64, ptr, ptr)' \
    'declare void @kelp_cap_check_name(ptr, ptr, ptr, i64, ptr, ptr)' \
    'declare void @kelp_cap_check_host(ptr, ptr, ptr, i64, ptr, ptr)' \
    'declare void @kelp_cap_check_hostport(ptr, ptr, i16, ptr, i64, ptr, ptr)' \
    'declare void @kelp_cap_check_port(ptr, i16, ptr, i64, ptr, ptr)'
do
    if ! grep -qxF "$want" "$FS_IR"; then
        echo "the capability check declaration in '$FS_IR' is missing: $want" >&2
        exit 1
    fi
done
if ! grep -qxF '@kelp_caps_fs_errno_fs = external global ptr' "$FS_IR"; then
    echo "'$FS_IR' does not name the fs allowlist array the check passes" >&2
    exit 1
fi
for want in \
    'kelp_cap_check_path' \
    '@kelp_caps_fs_errno_fs, i64 1'
do
    if ! grep -qF "$want" "$FS_IR"; then
        echo "the checked fs call site in '$FS_IR' is missing: $want" >&2
        exit 1
    fi
done
# `net.listen` is the port-only check and `net.connect` the host-and-port one, so
# the two shapes the C glue distinguishes are both here. The literal `i16 0` is
# the port `net.listen(0)` names, and the array is the calling module's own.
if ! grep -qE 'call void @kelp_cap_check_port\(ptr [^,]+, i16 0, ptr @kelp_caps_net_loopback_net, i64 1' "$NETL_IR"; then
    echo "'$NETL_IR' has no port-only check for net.listen" >&2
    exit 1
fi
if ! grep -qE 'call void @kelp_cap_check_hostport\(ptr [^,]+, ptr [^,]+, i16 %port[0-9]*' "$NETL_IR"; then
    echo "'$NETL_IR' has no host-and-port check for net.connect" >&2
    exit 1
fi
if ! grep -qE 'call void @kelp_cap_check_host\(ptr [^,]+, ptr [^,]+, ptr @kelp_caps_net_resolve_net, i64 1' "$NETR_IR"; then
    echo "'$NETR_IR' has no host-only check for net.resolve" >&2
    exit 1
fi
# The module named in the check is the *declaring* module of the call site, not
# the grantee's importer: `caps_net_denied`'s grant belongs to `util`.
if ! grep -qF 'ptr @kelp_caps_util_net' "$CND_IR"; then
    echo "'$CND_IR' does not check against 'util's own allowlist" >&2
    exit 1
fi

# An aggregate over 16 bytes never crosses a C boundary by value (BUG-vN4tR, MK7
# §3.3): `String#into_bytes`'s C form takes a 24-byte `kelp_String` and returns a
# 24-byte `kelp_Bytes`, so the IR calls the runtime object's pointer-spelled
# adapter with the receiver staged -- the returned struct through the hidden
# first argument, never a by-value aggregate on either side.
for want in \
    'declare void @kelp_String__into_bytes_ptr(ptr, ptr)' \
    'call void @kelp_String__into_bytes_ptr(ptr %sret.out'
do
    if ! grep -qF "$want" "$SB_IR"; then
        echo "the String#into_bytes adapter in '$SB_IR' is missing: $want" >&2
        exit 1
    fi
done
if ! grep -qE 'call void @kelp_String__into_bytes_ptr\(ptr %sret\.out[0-9]+, ptr %arg\.byref\)' "$SB_IR"; then
    echo "'$SB_IR' does not stage the receiver by pointer" >&2
    exit 1
fi

# A borrow of a field of an owned temporary (`mkName().name`, `mkName().r`,
# `mkName().items.len()`) stages the *whole* temporary into a slot and drops it
# after the call or the `printf`, which is `codegen_c.rs::stage_borrowed_field`:
# at the temporary and not at the field, so a `datac` that owns two `String`s
# frees both. `Named` here has `r: String` and `name: String`, and `Bag` owns a
# `List`.
for want in \
    'call void @kelp_drop_Named(ptr %borrow.tmp)' \
    'call void @kelp_drop_Bag(ptr %borrow.tmp'
do
    if ! grep -qF "$want" "$HF_IR"; then
        echo "the staged borrow in '$HF_IR' is missing: $want" >&2
        exit 1
    fi
done

# A *value*-position read through an owned temporary (`print(mkName().r)` where
# `r` is an `i64`) has no borrow to hang the drop on, so the temporary is staged,
# the field read out of the copy, the temporary dropped, and the value handed on:
# `codegen_c.rs::staged_field_value`. This is the shape that has to be dropped
# once per expression rather than once per statement, which is what the loop
# condition in this fixture exercises.
#
# The `align 8` is the target's ABI alignment for an `i64`, which the store now
# carries because the module states the data layout it is emitted under
# (`llvm_ffi::Module::set_data_layout`). Before S6 the module had none, so an
# `i64` inherited the default layout's 4-byte alignment; MK7 S6 made the module
# carry the machine's layout, which the vectorizer and any struct-offset folder
# needs to agree with codegen.
for want in \
    'store %tag_Named %call, ptr %field.root, align 8' \
    'call void @kelp_drop_Named(ptr %field.root)' \
    'store i64 %r1, ptr %field.value, align 8'
do
    if ! grep -qF "$want" "$HFV_IR"; then
        echo "the staged value read in '$HFV_IR' is missing: $want" >&2
        exit 1
    fi
done

# The deny path, compared directly: a denied program under both backends, with
# stderr and the exit status byte for byte. The tests above already assert the
# text as a literal; this is the same claim against the oracle compiler rather
# than against a string an author wrote.
DENY=""
for project in caps_denied caps_raw_denied caps_env_denied caps_net_denied caps_net_resolve_denied caps_link_denied caps_double_eval caps_raw_double_eval
do
    # The two double-eval programs need their scratch files to exist, the way
    # `compile_and_run.rs` creates them; the rest deny on a path that is fixed.
    mkdir -p "/tmp/kelp_$project"
    printf 'FILECONTENT\n' > "/tmp/kelp_$project/a" 2>/dev/null || true
    target="kelpc/tests/projects/$project/main.kp"
    if ! KELPC_BACKEND=c ./target/release/kelpc build "$target" \
        -o "$REF_OUT/$project-c" >/dev/null 2>"$REF_OUT/$project.c.build"; then
        echo "the C backend no longer builds '$project', so the diff below proves nothing" >&2
        cat "$REF_OUT/$project.c.build" >&2
        exit 1
    fi
    if ! KELPC_BACKEND="$BACKEND" ./target/release/kelpc build "$target" \
        -o "$REF_OUT/$project-l" >/dev/null 2>"$REF_OUT/$project.l.build"; then
        echo "the $BACKEND backend no longer builds '$project'" >&2
        cat "$REF_OUT/$project.l.build" >&2
        exit 1
    fi
    set +e
    "$REF_OUT/$project-c" >"$REF_OUT/$project.c.out" 2>"$REF_OUT/$project.c.err"
    c_status=$?
    "$REF_OUT/$project-l" >"$REF_OUT/$project.l.out" 2>"$REF_OUT/$project.l.err"
    l_status=$?
    set -e
    if [ "$c_status" != "$l_status" ]; then
        echo "'$project' exits $c_status under 'c' and $l_status under '$BACKEND'" >&2
        exit 1
    fi
    if ! diff -u "$REF_OUT/$project.c.err" "$REF_OUT/$project.l.err"; then
        echo "'$project' prints a different denial under '$BACKEND' (above)" >&2
        exit 1
    fi
    if ! diff -u "$REF_OUT/$project.c.out" "$REF_OUT/$project.l.out"; then
        echo "'$project' prints a different stdout under '$BACKEND' (above)" >&2
        exit 1
    fi
    DENY="$DENY $project"
done

echo "$BACKEND S3 gate passed: $(printf '%s\n' $TESTS | wc -l) tests, the passdown globals, the init and atexit drop, the saved argv, the capability check declarations and the path/name/host/hostport/port call shapes, the String#into_bytes adapter, the owned-temporary staging and drop, and $(printf '%s\n' $DENY | wc -w) denied programs byte-identical to the C backend"
