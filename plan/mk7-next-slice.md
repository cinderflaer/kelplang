# MK7 next slice — reconaissance brief

Branch `mk7/own-backend`, HEAD `3a2f6f9`. This brief measures the four candidate
slices below against the current backend. It is a specification lane: it changes
no source. Every refusal text was reproduced by hand with
`./target/release/kelpc build --backend=asm kelpc/tests/fixtures/NAME.kp -o /tmp/next_NAME`,
and the C backend was built for every fixture named reachable, because a fixture
the oracle rejects is not a target.

---

## 0. The refusal ranking, verbatim

Command: `cargo test --test backend_coverage -- --ignored --nocapture` (24.7 s).
The `the_refusals_rank_by_the_categories_the_plan_uses` test prints:

```
asm backend refusal ranking (54 fixtures refused of 139 the C backend builds, 85 built)
  a heap type: 25
    borrow_provenance_and_arms
    fs_errno
    heap_temporary_print
    io_stdio
    io_streams
    list_field
    list_slice_bounds
    list_slice_insert
    map_basic
    map_key_types
    map_missing_key
    map_owned
    map_remove_missing
    net_loopback
    net_poll
    net_resolve
    net_timeout
    path_normalize
    pd_argument_forwarding
    pd_owned
    process_spawn
    stdlib_build
    stdlib_parse
    stdlib_sort
    str_to_string_coercion
  an `enum`: 18
    enum_basic
    enum_container
    enum_owned
    enum_recursive
    generic_explicit_args
    generic_function_owned
    generic_nested_inference
    generic_owned
    generic_recursive
    generic_type_basic
    if_let_basic
    if_let_chain_same_scrutinee
    move_param_return
    stdlib_combinators
    stdlib_option_result
    string_views
    try_drops_frame
    try_operator
  a `datac` holding a float or heap field: 3
    map_datac
    receiver_methods
    templates
  a closure or a function value: 7
    closure_basic
    closure_move
    fn_literal
    fn_literal_capture
    fn_value
    stdlib_closure_callbacks
    stdlib_list_callbacks
  a gated extern: 1
    caps_time_rand
  total: 54
```

This agrees with plan §0 and §5's own counts (85 built, 54 refused) and is the
current measurement; the older 90/80/48 numbers in §0's prose are stale.

---

## A. A borrow of a scalar local

**Exact current refusal.** All three fixtures named in the task refuse at the
same place, and it is *not* the scalar borrow:

```
stdlib_build               error: the 'asm' backend does not support the `extern fun std.str.parse_i64_raw` yet
stdlib_parse               error: the 'asm' backend does not support the `extern fun std.str.parse_i64_raw` yet
borrow_provenance_and_arms error: the 'asm' backend does not support the `extern fun std.str.parse_i64_raw` yet
```

The scalar-borrow message named in the task exists in the source but is not on
any of the three fixtures' path today:

`kelpc/src/codegen_asm.rs:4682`
`a borrow of anything but a named \`datac\` local or a \`datac\` temporary`

(It is reached by `ExprKind::Ref` at `codegen_asm.rs:4601`; `now_mono_ns_raw`'s
and `parse_*_raw`'s out-parameter all go through it.)

**Measurement disagrees with the plan text, plainly.** Plan §0's "what is next"
and §5's heap-blockquote credit this slice with `stdlib_build`, `stdlib_parse`
and `borrow_provenance_and_arms` ("the `str` parsers ... need a scalar borrow the
backend cannot address yet"). The command above shows the *current* first
refusal of all three is the missing `std.str.parse_i64_raw` extern body, not the
borrow. And the borrow is not the thing behind it either:

- `stdlib_parse` uses `if let Err(e) = r` and `if let Empty/Invalid = e` over
  `std.str.ParseErr` (`stdlib_parse.kp:11-19`) — the `enum` row.
- `stdlib_build` calls `str.parse_u64(...)` then `result.unwrap(r = big)`
  (`stdlib_build.kp:16-19`) — `Result` is an `enum`.
- `borrow_provenance_and_arms` declares `enum J { S(val x: String) N }` and uses
  `if let S(s) = v` (`borrow_provenance_and_arms.kp:5-20`) — the `enum` row.

**Exact C-side thing to mirror.** The scalar out-parameter is a `mut i64`/`mut
f64` parameter, which the C backend passes as a plain pointer. `mut` parameters
are pointers in C; the borrow is the caller's job. The body to mirror is
`kelpc/src/externs.rs:44-52` (`parse_i64_raw`: `*kelp_out = (int64_t)v;`) and its
siblings at `externs.rs:53-67`. The C-side call shape is in
`codegen_c.rs:3223`'s neighbourhood for the address, but the important C fact is
that a `mut` scalar parameter is `int64_t*` and the callee stores through it —
the asm backend has no store-through-pointer for a scalar borrow today
(`ExprKind::Deref` in `codegen_asm.rs` covers a passdown read, not a store).

**The mechanism, one paragraph.** `ExprKind::Ref` gains one shape: a named
scalar local gets its address with `lea rax, [rbp+slot]` (the same instruction
it already emits for a named `datac` local; no slot is needed and nothing is
copied). The address is then an ordinary `int_ty` pointer value, so the call
marshalling needs no change. Separately the three `parse_*_raw` externs need
bodies: each is the `externs.rs` C body transliterated to a `Routine`, calling
`strtoll`/`strtoull`/`strtod` through `libc@PLT`, with a whitespace check loop
and one store through the out pointer. That store is the second new primitive
(`*p = v` for an integer/float pointee), not the borrow itself.

**Fixtures it can reach, verified.** Measured reach: **0**. The borrow arm alone
builds no fixture. Combined with the `enum` slice (D), the three would become
reachable (they are the fixtures' *next* refusal), so A is a *dependency of D*,
not an independent slice, for `stdlib_parse`/`stdlib_build`/`borrow_provenance_and_arms`.

**Fixtures it is credited with but cannot reach.** All three named in the task.
`stdlib_build`, `stdlib_parse`, `borrow_provenance_and_arms` each need `enum`
(D) as well; C builds all three (verified).

**Size and risk.** Small: one `ExprKind::Ref` arm (~15 lines) plus three
`Routine` bodies (~20 lines each) and a scalar deref-store path. Risk is low —
the borrow produces a pointer into the current frame and the lifetime is the
enclosing statement, exactly what `borrowck.rs` already guarantees. The real
risk is *crediting*: the plan has now credited this slice with three fixtures it
cannot reach, the same error §9 lesson 1 records.

**Section 13 gate.** The annex (`plan/mk7-own-backend.md:2153`) records
`speed.kp` refusing on `extern fun std.time.now_mono_ns_raw`; measured again it
is the same (`./target/release/kelpc build --backend=asm bench/speed/speed.kp`
gives that error). So A unblocks **none of the gate artifact by itself**: the
`float` and `fib` kernels' `above: mut i64`/`calls: mut i64` counters are refused
by A's borrow, but `speed.kp` never reaches them because `impl "std/time"`
refuses first. Before the gate can be evaluated, all three of these must land:
1. the `std.time.now_mono_ns_raw` extern body (`externs.rs:262-268`,
   `clock_gettime`), which is A's extern half plus a fourth routine;
2. the capability check (§5 item 7) — `std/time` is a gated module
   (`kelpc/std/time.kp:1-6`), and the asm backend refuses every gated call at
   `codegen_asm.rs:5440-5444`; with the extern body added, the refusal moves
   here, which is what `caps_time_rand` already reports (`a gated extern: 1`);
3. A's scalar borrow, for the counters.

So A is on the gate's critical path but is the *third* of three prerequisites,
and unblocks zero fixtures on its own.

---

## B. `List.sort`, `List.contains`, `List.slice`, and the generated comparator

**Exact current refusals.**

```
stdlib_sort         error: the 'asm' backend does not support the method `List.sort` on a `List` yet
list_slice_insert   error: the 'asm' backend does not support the method `List.sort` on a `List` yet
list_slice_bounds   error: the 'asm' backend does not support the method `List.slice` on a `List` yet
```

**Exact C-side thing to mirror.** `codegen_c.rs:1747` `write_list_s1` generates
all four pieces, gated by the same predicates the frontend uses:
- `types.rs:430` `list_sortable_elem` (numeric/`bool`/`String`/`Bytes`) gates
  `{list}__cmp` (`codegen_c.rs:1752`) and `{list}__sort` (`codegen_c.rs:1773`,
  a stable in-place insertion sort);
- `types.rs:437` `list_comparable_elem` (same set) gates `{list}__contains`
  (`codegen_c.rs:1784`);
- `types.rs:445` `list_clonable_elem` (`is_copy` or `String`/`Bytes`) gates
  `{list}__slice` (`codegen_c.rs:1798`, which clones each element with
  `kelp_String__clone`/`kelp_Bytes__clone`, `codegen_c.rs:1804-1809`).

**Existing List routines to build on.** `codegen_asm.rs:1209` `ListFn` already
has `empty/of/len/is_empty/push/bounds/get/get_const/set/pop/clear/insert/
remove/reverse/drop`, plus the non-monomorphized growth helper
`kelp_rt$list_reserve`, and `emit_elem_addr`/`emit_elem_return`/
`marshal_elem` (`codegen_asm.rs:~3575-3620`) are exactly the element-addressing
primitives sort/contains/slice need. `Bytes#clone`/`String#clone` already exist
as routines (`codegen_asm.rs:661,683,739,752`), so `slice`'s clone half is
already emitted.

**The mechanism, one paragraph.** Add four `ListFn` variants — `Cmp`,
`Sort`, `Contains`, `Slice` — emitted per element instantiation the way the
existing fifteen are. `Cmp` is one generated routine per element kind matching
`codegen_c.rs:1752-1771` (value compare for numerics/`bool`; length-aware
`memcmp` for `String`/`Bytes`); `Sort` is the same insertion sort, so the stores
are element moves (already handled by the existing element copy); `Contains`
compares against a borrowed needle for owned elems (the fixture depends on this:
`list_slice_insert.kp:19`); `Slice` allocates through `kelp_rt$list_reserve`
and clones each element. The dispatch sites are the `List#` match at
`codegen_asm.rs:5667-5736`, whose `_` arm currently refuses by name, and the
predicates come from `types.rs` so the two backends gate identically.

**Fixtures it can reach, verified.** All three C-build OK:
`stdlib_sort`, `list_slice_insert`, `list_slice_bounds`. Reach = **3**.

**Fixtures credited but not reached.** None claimed; this is the plan's §0 rating
`4` in full (`stdlib_sort`, `list_slice_insert`, `list_slice_bounds`).

**Size and risk.** Moderate: four routines plus a `ListFn` extension, reusing
existing addressing/clone/growth code. Risk moderate and of a different kind from
D: sort/slice move owned `String` elements, so a wrong element width or a
leaked/double-freed clone shows up as a *memory* bug — `list_slice_insert` is
already in the AddressSanitizer/valgrind pass, so it is pinned. No ABI/Variadic
surface. Estimated ~150-250 lines.

---

## C. Printing an owned `String`/`View` — the real gap is the template, not `print(s)`

**Exact current refusal** (`heap_temporary_print`):

```
error: the 'asm' backend does not support printing a value of type `String` yet
```

**Measurement disagrees with the task's framing, plainly.** The task says the
gap is `print(s)` rather than `print(s.as_str())`. `print(s)` for an owned
`String` already builds. I split `heap_temporary_print.kp`'s four statements
into one-line probes:

```
print(Bytes.from_str("x"))            builds (exit 0)
print(String.from_str("y"))           builds (exit 0)
print(str.to_upper("ab"))             builds (exit 0)
print("w = ${str.to_upper("cd")}")    error: ... printing a value of type `String` yet
```

So the first refusal is the **template interpolation** `print("w = ${...}")` of
an owned `String`, not the single-argument `print`. The single-argument path is
already `print_buffer` (`codegen_asm.rs:6682`) and `print_view`
(`codegen_asm.rs:6727`), reached from `builtin_print` (`codegen_asm.rs:6630`).

**Exact C-side thing to mirror.** `codegen_c.rs:2620-2650` (`ExprKind::Template`):
an owned `Bytes`/`String` piece is held in a C temporary, its `%.*s` specifier
plus `(int)len, data` arguments are appended, and the temporary is dropped right
after the `printf` (`codegen_c.rs:2629-2638`). The asm side has only the
`printable_ty` path (`codegen_asm.rs:6763-6776`), and it refuses any non-`printable_ty`
interpolation — which is what byte-for-byte prints `String`/`View`.

**The mechanism, one paragraph.** In `Gen::template`, accept `string_ty`/
`bytes_ty`/`view_ty` interpolations: reserve a frame scratch slot
(`Frame::scratch`, already used for aggregate temporaries), evaluate the value
into it, append `%.*s` with `(int)len, data`, and — for an owned `String`/`Bytes`
only — emit the drop (`kelp_string_drop`/`kelp_bytes_drop`) after the `printf`;
a `View` is a borrow and is not dropped. This is the same hold-then-free shape
`print_buffer` already implements for the single-argument case, so most of it is
already written.

**Fixtures it can reach, verified.** `heap_temporary_print` (C-builds OK).
Reach = **1**.

**Section 5 item 8's other two gaps, honestly.** Neither has a fixture.
- *A `Bytes` value interpolated into a template* (`print("x=${b}")`): grep over
  all fixtures finds no template interpolation of a `Bytes`/`String` except
  `heap_temporary_print.kp:12` (a `String`). So no fixture reaches it.
- *The five-interpolation limit*: the asm template numbers integer arguments
  across `value_regs = ["rsi","rdx","rcx","r8","r9"]` (five) in
  `codegen_asm.rs:~6790`; no fixture reaches six interpolations. The plan says
  this explicitly — "which no fixture reaches" — so the criterion must not name
  it.

**Size and risk.** Smallest of the four: ~40-70 lines, all in one function, with
a working mirror and a working single-arg implementation beside it. Risk low;
the only hazard is dropping an owned interpolation before the `printf` reads it,
which the `print_buffer` order already solves.

---

## D. `enum` — the tagged union

**Exact current refusals** (first refusal per fixture, all 18 reproduced):

```
enum_basic                 the parameter `s: &Shape` of `area`
enum_container             the parameter `s: &Shape` of `area`
enum_owned                 the parameter `t: &Token` of `width`
enum_recursive             the parameter `xs: &IntList` of `sum`
generic_explicit_args      the parameter `o: &Option<i64>` of `unwrap_or`
generic_function_owned     the parameter `o: &Option<String>` of `describe`
generic_nested_inference   the parameter `t: &Tree<i32>` of `depth`
generic_owned              the parameter `o: &Option<String>` of `text_len`
generic_recursive          the parameter `t: &Tree<i32>` of `label`
generic_type_basic         the parameter `o: &Option<i32>` of `unwrap_or`
if_let_basic               the parameter `s: &Shape` of `radius`
if_let_chain_same_scrutinee the enum `Shape`
move_param_return          the return type `std.option.Option<usize>` of `taken`
stdlib_combinators         the return type `std.option.Option<i32>` of `half`
stdlib_option_result       the return type `std.result.Result<i64, String>` of `half`
string_views               the return type `std.option.Option<usize>` of `std.str.index_of`
try_drops_frame            the return type `Res<i64, String>` of `ok`
try_operator               the return type `Result<i64, String>` of `half`
```

(each prefixed `error: the 'asm' backend does not support `; the parameter/
return-type checks are at `codegen_asm.rs:2679`/`:2711`). The `enum` value
itself is refused at `codegen_asm.rs:4587-4599` (`EnumLit`, `EnumTag`,
`EnumField`, `Try`). `if let` is desugared in the parser
(`parser.rs:1466,1500,1597`), so it arrives as `EnumTag`/`EnumField` and needs
no separate node.

**Exact C-side thing to mirror.**
- Layout and tag: `codegen_c.rs:1040` `write_enum` — `typedef struct { int32_t
  kelp_tag; union { struct { ...fields } kelp_v_Variant; ... } kelp_v; }`,
  tag = variant index; a payload-less variant gets a `char kelp_none` field.
- `EnumLit`: `codegen_c.rs:2667-2697` (`{ .kelp_tag = variant, .kelp_v = { .member = { .field = ... } } }`).
- `EnumTag`: `codegen_c.rs:2698-2700` via `enum_tag_access` (`codegen_c.rs:3223`).
- `EnumField`: `codegen_c.rs:2701-2716` via `enum_member_access` (`codegen_c.rs:3234`).
- `Try`: `codegen_c.rs:2723-2780` — evaluate operand once, test the failure
  tag, rebuild the failure in the target type, else yield the success payload.
- Exhaustiveness: not a backend check — it is enforced in the frontend at
  `infer.rs:6029` ("this 'when' is not exhaustive"), so the backend may assume
  a total `when`.
- Per-enum drop: `codegen_c.rs:1066-1090` — `switch (v->kelp_tag)` with one
  `case i` per variant that owns, dropping each owning field; no `default`
  arm on purpose (an unknown tag is a compiler-visible hole).

**The mechanism, one paragraph.** An `enum` is a `Ty::Struct` whose layout is
`tag` (4 bytes, offset 0) followed by a union sized to the largest variant
payload; build its `StructLayout` beside the `datac` layouts (`build_layouts`,
`codegen_asm.rs:1410`) with a `FieldEntry` for the tag and per-variant payload
offsets. `EnumLit` writes the tag then builds the variant's payload into the
same slot exactly as `struct_lit` does today; `EnumTag`/`EnumField` are a tag
compare and an offset field access on the base address, which
`addr_of_struct`/`field_read` already do. `Try` is a branch on the live tag that
copies the moved payload into the target variant. The generated per-enum drop is
a jump table or compare chain on `kelp_tag` that calls the variant's owning-field
drops — the datac generated drop (`codegen_c.rs`'s sibling, already emitted for
owned datacs) with a tag switch in front. The ABI needs nothing new for a
scalar or owned payload: the enum is an aggregate like any `datac`, so
`AggClass`/`marshal_elem`/`eval_struct_into` cover it once its layout exists.

**The 18 split by payload, verified against the fixtures.**
- *Float payload* → depends on the float-in-aggregate slice (the same leftover
  §5 item 4 names: "a `datac` holding a float ... at any size"; `agg_class` at
  `codegen_asm.rs` only classifies INTEGER eightbytes): **4 fixtures** —
  `enum_basic` (`Shape.Circle(r: f64)`), `enum_container` (`Shape` f64 +
  `Token`), `if_let_basic`, `if_let_chain_same_scrutinee`.
- *Closure/function-value payload or callback* → depends on the closures row
  (§5 item 6, 7 fixtures): **1 fixture**, `stdlib_combinators`
  (`stdlib_combinators.kp:38` takes `fun(s: &String) -> String`).
- *Owning payload* (`String`, `Box<Tree>`) → in D's own scope **if D lands the
  generated per-enum drop**, which item 5 explicitly pairs with "an `enum` that
  holds one". The owning types themselves are done (`String`, `Box`, `List`),
  so nothing else has to land first: **10 fixtures** — `enum_owned`,
  `enum_recursive`, `generic_function_owned`, `generic_nested_inference`,
  `generic_owned`, `generic_recursive`, `stdlib_option_result`,
  `try_drops_frame`, `try_operator`, plus `enum_container` (already counted
  float). If D is scoped to scalar payloads only, these 10 move to a
  companion slice.
- *Scalar payload only*: `generic_explicit_args`, `generic_type_basic`,
  `move_param_return`, `string_views`.

**Reachable must-pass list (this slice's own scope, all C-build OK): 13**
`enum_owned`, `enum_recursive`, `generic_explicit_args`,
`generic_function_owned`, `generic_nested_inference`, `generic_owned`,
`generic_recursive`, `generic_type_basic`, `move_param_return`,
`stdlib_option_result`, `string_views`, `try_drops_frame`, `try_operator`.

**Expected blocked: 5** — `enum_basic`, `enum_container`, `if_let_basic`,
`if_let_chain_same_scrutinee` (float payload; blocked on the float-aggregate
slice) and `stdlib_combinators` (closures).

**Size and risk.** Largest: layout + `EnumLit`/`EnumTag`/`EnumField`/`Try`
lowering + the generated tag-switching drop + `when`/`if let` codegen. High risk
— this is the row the risk register calls the home of silent miscompiles
("Aggregate ABI mismatch ... a wrong eightbyte split is a silent miscompile"),
and a wrong tag offset or a union sized to the wrong variant is wrong output
with no crash. The mitigations are the differential stdout sweep (already live)
and `try_drops_frame`/`enum_owned`/`enum_recursive`, which are in the valgrind
pass. Estimated several hundred lines, the biggest of the four.

---

## Section 13 gate, stated fully

The annex is explicit (`plan/mk7-own-backend.md:2153-2285`): the gate artifact
`bench/speed/speed.kp` "does not build under `--backend=asm` at HEAD". Measured
again at `3a2f6f9`, unchanged: refusal is `extern fun std.time.now_mono_ns_raw`.

A alone does **not** unblock the gate. The gate needs, in order of first refusal:
1. `std.time.now_mono_ns_raw` extern body (`externs.rs:262`);
2. the capability check (§5 item 7, `codegen_asm.rs:5440`) because `std/time` is
   gated;
3. A's scalar `mut` borrow, for `float`'s and `fib`'s counters.

The annex's supplementary probe already shows `collatz` and `mix` kernels build
and sit at/under `-O0` (0.97 and 0.84); `float`/`fib` need the borrow; the
gate artifact needs all three. The annex also names four measurement conditions
the real gate run must fix (external wall clock, `float`'s weak integer
checksum, one estimator across columns, idle machine) — none of which any
candidate slice addresses.

---

## Close

**Cheapest per line of code.** **C** (template interpolation of an owned
`String`/`View`): ~40-70 lines in one function, with both the C mirror
(`codegen_c.rs:2620-2650`) and the asm single-argument implementation
(`print_buffer`/`print_view`) already written. Caveat: A's *borrow arm alone* is
an even smaller diff (~15 lines) but reaches **0** fixtures and unblocks no gate
step, so it is not a slice; C is the cheapest slice that reaches anything.

**Largest per fixture.** **D** (`enum`): 13 reachable fixtures in one mechanism,
against B's 3 and C's 1. It is also the plan's §0 ranking `2` ("the largest
single row at 18") and §5 item 4's remaining headline.

**Recommendation for the next round: D, `enum`** — but scoped to include both
halves, the tag/union codegen *and* the generated per-enum drop, so it reaches
all 13. Reasons, in this plan's own terms:
1. The plan ranks by first refusal and by mechanism; `enum` is the largest
   single remaining mechanism (18 fixtures), and after the list slice the only
   aggregates left are `enum` and `Map`. It is the biggest coverage delta per
   *mechanism* on the board.
2. The plan's own risk note says the enum ABI rides the aggregate machinery that
   is now complete ("The ABI work ... is now complete for every shape this
   backend admits at all"). An enum is a tagged `datac`; landing it adds
   matching, the tag switch, and a generated drop, not a new ABI path.
3. §9 lesson 1 (an acceptance criterion may only name what the slice can reach)
   is honoured by naming exactly the 13 above and explicitly excluding the 5
   blocked.
4. A and C are too small to be a round on their own (0 and 1 fixtures); B is a
   clean 3 but leaves the largest row untouched. D also *subsumes* the fixtures
   A was wrongly credited with, once A's borrow/extern halves ride along.

Do **not** name `enum_basic` as a must-pass: it is the fixture most associated
with "enum", and it is blocked on the float-aggregate slice by measurement.

**Must-pass fixtures for D (only what D's own scope can reach): 13**
`enum_owned`, `enum_recursive`, `generic_explicit_args`,
`generic_function_owned`, `generic_nested_inference`, `generic_owned`,
`generic_recursive`, `generic_type_basic`, `move_param_return`,
`stdlib_option_result`, `string_views`, `try_drops_frame`, `try_operator`.

---

## Measurement disagreements with the plan, collected

1. **A's fixtures (plan §0/§5).** Plan credits the scalar-borrow slice with
   `stdlib_build`, `stdlib_parse`, `borrow_provenance_and_arms`. Measured, all
   three refuse first on `extern fun std.str.parse_i64_raw`, and each also needs
   `enum`. Command: the four `kelpc build --backend=asm` invocations at the top
   of section A. A reaches 0 fixtures alone.
2. **C's shape (task framing / §5 item 8).** `print(s)` for an owned `String`
   already builds; the refusal is template interpolation. Command: the four
   one-line `print(...)` probes in section C.
3. **§5 item 8's other two gaps.** `Bytes` interpolated into a template and the
   five-interpolation limit have **no** fixture (grep over
   `kelpc/tests/fixtures/*.kp`). The plan says so for the limit; the `Bytes`
   case is likewise unreached.
4. **Stale counts in §0's prose.** §0 says "the refused set is 80 ... 48 fixtures
   now build" earlier and "85 build, 54 refused" at the list slice; the current
   test output is 85 built / 54 refused / 139 C-builds. The test table above is
   the authority.
