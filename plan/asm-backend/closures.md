# MK7: how the asm backend lowers closures and function values

> **ARCHIVED 2026-09-20. Superseded, not deleted.** This was the last refusal row of the `asm`
> ranking. LLVM lowers function values and closures as ordinary indirect calls, so none of the
> fat-pointer and ABI work below is ours any more.

The MK7 §5 item 6 spec: `FnRef`, `ClosureLit`, `Capture`, `FnToClosure` and `IndirectCall`, the
last row of the refusal ranking that is one mechanism rather than a library. This file is a
specification lane: it changes no source, adds no fixture and commits nothing. Every claim below is
either a quoted line or a named site of the code it describes (`codegen_c.rs` and the C it
generates, the frontend's `infer.rs`/`borrowck.rs`/`types.rs`, and this backend's `codegen_asm.rs`),
or a hand-run measurement at the base commit; a `codegen_asm.rs` line number is an anchor read while
a concurrent lane was editing that file, so the name in the sentence is the authority and the
number a hint.

**Base.** Branch `main`, HEAD `bb2316e` (`plan: the MK7 status at the post-caeb98d tip`), measured
2026-09-20 in the shared checkout. The ranking test prints

```
asm backend refusal ranking (24 fixtures refused of 202 the C backend builds, 178 built)
  a heap type: 13
  a fixed-size array (A-18): 0
  an `enum`: 0
  a `datac` holding a float or heap field: 0
  a closure or a function value: 10
  a gated extern: 1
  total: 24
```

so the row this file specifies is exactly ten fixtures, and the brief it comes from is
`plan/asm-backend/next-slice.md` §A (measured at `c3a8759`, whose row was the same ten).

**Commands, once per fixture.**

```
./target/release/kelpc build --backend=asm kelpc/tests/fixtures/NAME.kp -o /tmp/mk7/NAME     # the refusal
./target/release/kelpc build --backend=c   kelpc/tests/fixtures/NAME.kp -o /tmp/mk7c/NAME    # the oracle, and NAME.c beside it
```

`driver.rs::compile_c` writes the generated C to `<out_path>.c` and keeps it, so every C line quoted
below is regenerable with the second command. The three kept for this brief are `closure_basic`,
`closure_datac_borrow_param` and `stdlib_closure_callbacks`; a fourth, `fn_literal`, is quoted for
the environment-free half. They lived in `/tmp/mk7c/c` while this file was written and are not part
of the repository.

**Every one of the ten builds under `--backend=c`** (measured, one by one). None is an oracle
rejection, and the other 24 fixtures in the corpus that mention `fun(` or `closure(` are frontend
rejections under both backends (measured for all 34), so they are not targets and are not counted.

## 1. The mechanism, read out of `codegen_c.rs`

Nothing in this section is invented; each paragraph names the function that decides it and quotes
the C it emits.

### 1.1 The value: a three-field aggregate, monomorphized per signature

`codegen_c.rs::write_closure_typedef` (`:1371`), quoted from `closure_basic.c:552`:

```c
typedef struct {
    int32_t (*fn)(void*);
    void* env;
    void (*drop)(void* env);
} kelp_Closure__i32;
```

Three pointers, 24 bytes, alignment 8. The name comes from `types::closure_mangle` /
`types::closure_c_type` (`types.rs:395`, `:407`), one instantiation per `(params, ret)` signature,
and the `fn` member is typed (`void*` first, then the signature's own parameters) only because of
that monomorphization: it is why an indirect call needs no cast. The type is **not `Copy`** and
`owns_heap()` is `true` for it (`types.rs:189`, `:201`).

### 1.2 The environment: always heap, one struct and one drop per literal

`ExprKind::ClosureLit` (`codegen_c.rs:2934`), quoted from `closure_basic.c:2023`:

```c
return ({ kelp_Env____funlit1* _xtmp9 = (kelp_Env____funlit1*)malloc(sizeof(kelp_Env____funlit1)); if (_xtmp9 == NULL) { kelp_panic("out of memory"); } _xtmp9->c0 = kelp_prefix; kelp_Closure__i32__i32 _xtmp10; _xtmp10.fn = kelp___funlit1; _xtmp10.env = _xtmp9; _xtmp10.drop = kelp_env_drop____funlit1; _xtmp10; });
```

So: `malloc(sizeof(Env))`, panic `"out of memory"` on `NULL`, one store per capture in field order,
then the three words of the fat pointer. The environment struct and its own drop are
`codegen_c.rs::write_env` (`:2359`), quoted from `closure_basic.c:1052` and, for an owned capture,
`stdlib_closure_callbacks.c:1275`:

```c
typedef struct {
    kelp_String c0;
} kelp_Env____funlit7;

static void kelp_env_drop____funlit7(void* raw) {
    kelp_Env____funlit7* e = (kelp_Env____funlit7*)raw;
    kelp_string_drop(&e->c0);
    free(e);
}
```

The names are `types::env_struct_name` / `types::env_drop_name` (`types.rs:421`, `:426`), keyed by
the literal's canonical key (`#funlit7`, which no `.kp` identifier can contain). The struct is
emitted with the literal's own generated function because nothing else names it. The drop is
field-order drops for every field `needs_drop` accepts, then `free(e)`: the same order a `datac`'s
generated drop uses, one level down. `types::needs_drop` (`types.rs:582`) is what decides which
fields that is, and it already answers `true` for a `String`, a `Bytes`, an owning `datac`, a
container and a nested `closure`.

### 1.3 The captures, and their order

`infer.rs::note_capture` (`:518`) records one entry per distinct name **in the order the body first
names it**, and `infer.rs:5970` through `:6041` turns that list into the environment's fields; the environment's
field order is the order `ExprKind::Capture`'s index numbers them. The field names are
`types::capture_field` (`types.rs:414`): `c0`, `c1`, ... A capture is a whole local binding, either
a `Copy` one (copied in, the binding stays usable) or an owned one (moved in). Four capture shapes
are refused by the frontend under both backends and are therefore outside this row: a borrow, a
field of the enclosing receiver, a passdown, and a read of the enclosing closure's own environment
field by value.

All ten fixtures capture **at most one** name, so every environment in the corpus is a one-field
struct at offset 0 (measured over the generated C of all ten). A two-field environment is not
covered by any fixture; see §6 and §7.

### 1.4 The five expression kinds

* **`FnRef(key)`** (`codegen_c.rs:2928`) is `c_name(key)`, the C function itself. A function value
  of type `fun(...)` is a bare code pointer with no environment, so there is nothing to allocate and
  nothing to drop.
* **`ClosureLit { key, captures }`** is 1.2's statement expression.
* **`Capture { index }`** (`:2959`) is `env->c{index}`, where `env` is the pointer the generated
  function's prologue bound from its hidden first parameter (`codegen_c.rs:2629`, quoted from
  `closure_basic.c:2122`):

  ```c
  int32_t kelp___funlit1(void* env_raw, int32_t kelp_x) {
      kelp_Env____funlit1* env = (kelp_Env____funlit1*)env_raw;
  ```
* **`FnToClosure { func }`** (`:2969`) has two shapes, and they are not the same cost. When the
  inner expression is an `FnRef` the value's function is statically known, so the C backend emits a
  **shim**: a per-(function, signature) static function that takes `void* env_raw` first, ignores it
  (`(void)env_raw;`), and calls the real function. The value is
  `{shim, NULL, NULL}` and nothing is allocated, which is why the `drop` member is `NULL` and why
  `write_closure`'s drop tests `env` first (`codegen_c.rs:1552`, quoted from
  `closure_basic.c:789`):

  ```c
  static void kelp_closure_drop__Closure__i32__i32(kelp_Closure__i32__i32* c) {
      if (c->env != NULL) {
          c->drop(c->env);
          c->env = NULL;
      }
  }
  ```

  When the inner expression is anything else (`chosen = pick(true)` then `list.map(xs, chosen)`),
  the code pointer is not known statically, so the value gets a one-field heap environment
  (`kelp_coerce_drop__...` = `free(raw)`, `kelp_coerce__...` = `return (e->f)(p0);`,
  `codegen_c.rs:1426`, `:1451`, `:1504`; generated at `stdlib_closure_callbacks.c:1194` and
  `:2355`).
* **`IndirectCall { callee, args }`** (`:2993`) binds the callee to a temporary of its own type
  first, then calls through it. `closure_access` (`:4044`) is what decides the spelling: `.fn` /
  `.env` when the callee's type is `closure(...)`, `->fn` / `->env` when it is a borrow of one.
  Quoted from `closure_basic.c:2030` (`add_k(1)`), `closure_datac_borrow_param.c:756` (a
  `&closure` parameter) and `closure_datac_borrow_param.c:773` (a temporary, borrowed and freed):

  ```c
  _xtmp13.fn = ...; (_xtmp13.fn)(_xtmp13.env, ((int32_t)(1)));
  ({ const kelp_Closure__refItem__unit* _xtmp0 = kelp_f; (_xtmp0->fn)(_xtmp0->env, ...); });
  ({ kelp_Closure__refItem__unit _xtmp1 = <the literal>; kelp_each((&kelp_xs), &_xtmp1); kelp_closure_drop__Closure__refItem__unit(&_xtmp1); });
  ```

  The environment is always the **first argument, before every declared parameter**, and the
  callee is evaluated once, into a temporary, so a field or a dereference is not re-evaluated.

### 1.5 What drops an escaping environment

The environment is owned by the closure value and freed by the `drop` pointer the value carries.
Nothing else frees it: the value's own per-signature drop (`kelp_closure_drop__<mangle>`, 1.4's
second quote) calls `c->drop(c->env)` and then nulls `c->env`, so a second drop frees nothing. The
drop runs wherever `borrowck.rs` put the binding's drop site, and `borrowck.rs` already records one
for a closure because `types::needs_drop` answers `true` for `Ty::Closure` (`borrowck.rs:1068`,
`:1171`, `:1301`, and the closure/capture arms at `:1319` through `:1366`). A closure that escapes
its frame therefore still carries everything needed to free its environment: the pointer, and the
function that frees it.

### 1.6 A parameter of closure type is a borrow unless it is written `move`

`closure_basic`'s two parameters come out as

```c
int32_t kelp_apply(const kelp_Closure__i32__i32* kelp_f, int32_t kelp_x);   /* f: closure(i32).R: i32 */
int32_t kelp_own(kelp_Closure__i32__i32 kelp_f, int32_t kelp_x);            /* f: move closure(i32).R: i32 */
```

so `&closure(...)` is the ordinary parameter spelling and a plain `closure(...)` is the `move`
one. Both are one eightbyte, or one `MEMORY`-class aggregate, in §2's terms.

## 2. The ABI decision this backend has to make

### 2.1 Is a stack-resident environment legal here?

**No, not as the mechanism**, for three separate reasons, and the spec is "always heap, exactly as
the C backend does":

1. **One fixture forces an escape, and it covers three escaping shapes.** `closure_basic` returns a
   closure out of the frame that built it (`make(prefix: move String)`), stores one in a `datac`
   field (`Holder(cb = ...)`), and stores one in a `Box` (`slot.set(...)`, `slot.take()`). An
   environment on the creating frame is dangling for all three. The other nine fixtures' environments
   are non-escaping (closure_basic's are the only ones that leave the frame; see §4), so a
   stack-only design would pass nine of ten and be wrong for the tenth, which is exactly the shape
   this backend refuses rather than guesses.
2. **The value carries a `drop` pointer whose C contract is "drop the fields, then `free`".** A stack
   environment cannot be freed, so a stack design needs either a second drop pointer (a stack
   variant) or a flag in the value, and the closure *type* is shared by every literal of one
   signature: the choice would have to be made per literal and recovered at the call site. That is an
   escape analysis this backend does not have, on top of a second drop contract.
3. **The valgrind gate watches the wrong thing for a stack environment.** A missed free of a heap
   environment is a leak that prints nothing, and **eight of the ten fixtures name themselves in the
   C suite's AddressSanitizer list** (`grep AddressSanitizer`: `closure_basic`, `closure_move`,
   `fn_literal`, `fn_value`, `stdlib_closure_callbacks`, `stdlib_combinators`,
   `stdlib_list_callbacks`, `try_drops_frame`; `fn_literal_capture` and `closure_datac_borrow_param`
   are the two that do not). The memory gate is what carries that property to this backend once the
   fixtures build. A stack environment has no free to miss, so the gate would stop pinning the
   property the fixtures were written for.

What stays free is the *allocation site*: `malloc@PLT` and the `"out of memory"` panic already
exist in this backend seven times over (the `Box`/`List`/`Map`/`Bytes` routines, `codegen_asm.rs`
`:5755`, `:5786`, `:7111`, `:7264`), so no new heap machinery is introduced.

### 2.2 The per-eightbyte ABI, in the classes this backend already uses

`AggClass` (`codegen_asm.rs:2192`) is the vocabulary: `One(EbClass{kind: Int, size: 8})` for one
INTEGER eightbyte, `Two` for a 9-16 byte pair, `Mem` for anything over sixteen bytes.

| value | size | class | transport |
| --- | --- | --- | --- |
| `fun(P).R: R` (`Ty::Fn`) | 8 | `One(Int)` | one integer argument register, `rax` on return, plain `mov` into a slot |
| `&fun(P).R: R` | 8 | `One(Int)` | one integer register, pointing at the code pointer |
| `closure(P).R: R` | 24 | `Mem` | in memory: an outgoing-area argument by value, and the hidden `rdi` pointer on return |
| `&closure(P).R: R` | 8 | `One(Int)` | one integer register, pointing at the 24-byte value |
| `Box<closure(P).R: R>` | 8 | `One(Int)` (the `$Box` header) | one integer register |
| a `datac` field of closure type (`Holder`) | 24 | `Mem` for `Holder` | classified by the containing `datac`, as for every aggregate field |
| the hidden environment parameter of a generated literal | 8 | `One(Int)` | `ARG64[0]`, or `ARG64[1]` when the call also returns a `Mem`-class value |

Neither pointer to the closure needs a new class: `Mem` is the third slice's and the fourth slice's
machinery (`memory_arg_area` at `codegen_asm.rs:2360`, `Dest`/`eval_struct_into`/`emit_copy`,
`RetClass::Agg(AggClass::Mem)` at `:3091`) and it is already load-bearing for `Bytes`, `String`,
`Map` and a wide `datac`. What the strategy needs is a **layout for the closure signature** so that
`struct_layout` (`:2711`), `arg_regs_for` (`:2808`), `value_kind` (`:2743`), `elem_size` (`:1709`)
and `Frame::declare` answer for it the way they answer for `$Map`.

### 2.3 The one numbering rule a writer has to get twice

A `Mem`-class **return** reserves `rdi` for the hidden pointer before any real parameter
(`codegen_asm.rs:4680`, `:10960`), and the generated literal's hidden environment is the first
*real* parameter after that. So at both ends of an indirect call the integer argument file is
numbered: hidden return pointer when the callee's signature returns a `Mem`-class value (a wide
`datac`, a `String`, a `List`, a `Map`), then the environment, then the declared parameters. A
*direct* call to a declared function that returns a closure has the hidden return pointer and no
environment, so the same rule answers for it with no environment term. The callee's prologue and
the caller's marshalling must come from one shared rule, exactly as `memory_arg_area` is shared
today; a second, hand-rolled copy of the rule is the defect this section exists to prevent.

## 3. The lifetime and drop rules the ten fixtures need

**`closure_move`.** Three paths, all already in the drop plan, none of them new: the closure local
dies at the end of its block (`{ val f = fun() .R: usize = first.len(); ... }`), the frame's closure
dies on an early `return` (`early(stop: bool)`, both paths), and one environment per loop iteration
dies at the end of the iteration's block. `borrowck.rs` already keys those three (block end,
`return`, loop-body end), and `codegen_asm.rs`'s `emit_drop`/`drop_site_for` is the emitter side;
what is missing is the `Ty::Closure` arm. The loop case also has to agree with
`drop_site_for`'s "checked twice, emitted once" property, which is why `closure_move` is the
fixture that pins it and `fn_literal_capture` alone is not enough.

**`try_drops_frame`.** No environment at all: its literal (`closure_frame`) captures nothing, so it
is a `Ty::Fn` value, and its `?` is inside a plain generated function whose own frame owns a
`String`. The requirement is therefore slice 1's (`Ty::Fn` + `IndirectCall`), and its drop plan is
keyed by the literal's own function name, which `DropPlan::at_try` already uses. It is in the
refusal row because the *expression* is refused, not because of `?`.

**Existing heap machinery or a new allocation shape.** Existing, with one new *routine* and no new
allocation shape: `malloc@PLT` and the `"out of memory"` panic, `free@PLT`, the generated drop
pattern (`emit_datac_drop`'s shape), the `DropSite` registry and the borrowed-temporary staging
(`Frame::scratch`, `register_borrowed_temporary`, `hoist_owned_temporary`) are all present. The new
routines are two, both codegen and neither an ABI: the per-literal **environment drop**
(`kelp_env_drop__#funlitN`: field drops in order, then `free`), and the per-signature **closure
drop** (`kelp_closure_drop__<mangle>`: `if env != NULL { call [drop](env); env = NULL }`, 1.4's
second quote). The `FnToClosure`-over-a-runtime-value case adds a third pair, the one-field coerce
environment, its `free` drop and its thunk; that pair is only reached by
`stdlib_closure_callbacks`'s `chosen`, measured (one `kelp_CoerceEnv__Closure__i32__i32` at
`stdlib_closure_callbacks.c:2355`, and no other fixture in the ten).

## 4. Per-fixture requirement list

For each fixture: what it uses, whether its environment must escape the creating frame, and the
first refusal **reproduced by hand at `bb2316e`** with

```
./target/release/kelpc build --backend=asm kelpc/tests/fixtures/NAME.kp -o /tmp/mk7/NAME
```

| fixture | what it uses | env escapes | first refusal at HEAD |
| --- | --- | --- | --- |
| `closure_basic` | `closure(i32).R: i32` written (8 times), a `Copy` capture, an owned `String` capture, an owned `datac` capture, a nested closure capture, a closure returned from `make`, a closure in a `datac` field, a `Box<closure>`, `Box.get`/`Box.set`/`Box.take`, a `move closure` parameter, a `&closure` parameter, indirect calls | **yes**, three shapes | `error: the 'asm' backend does not support the parameter `f: &closure(i32).R: i32` of `apply` yet` |
| `closure_datac_borrow_param` | a non-capturing literal widened into `closure(&Item)` (`FnToClosure` over `FnRef`), a `&closure(&Item)` parameter, an indirect call through it inside `each`, a `&List<Item>` parameter | no | `... the parameter `f: &closure(&Item)` of `each` yet` |
| `closure_move` | capturing literals in a block, in a function with an early `return`, and in a loop body; an owned `String` capture each time; drop-per-path | no | `... a function value yet` |
| `fn_literal` | no closure type at all: `fun(i32).R: i32` local/parameter/return/`datac` field, `FnRef`, an indirect call on a `Ty::Fn`, a literal with a `move String` parameter (a `Mem`-class argument through an indirect call), a literal inside a literal, `?` inside a literal | n/a | `... the parameter `f: fun(i32).R: i32` of `apply` yet` |
| `fn_literal_capture` | one `Copy` capture (`k`), the binding still read after the literal is built, an indirect call | no | `... a function value yet` |
| `fn_value` | `Ty::Fn` values only: a local, a parameter, a returned value, a `datac` field, a `mut`-borrow parameter, a `move String` parameter, assignment to a `var`, a module item as a value (`math.sqrt`), and a passdown `pd val doubler: fun(i32).R: i32` | n/a | `... the passdown `pd doubler` (its type is `fun(i32).R: i32`, and the asm backend has no storage for one, see plan/asm-backend/plan.md §5) yet` (the printed text carries an em dash before `see`; written here with a comma because this file carries none) |
| `stdlib_closure_callbacks` | capturing callbacks (a `Copy` capture and an owned `String` capture) passed as `&closure` to `std.list`/`std.option`/`std.result`, a declared function and a non-capturing literal widened by a shim, a runtime function value (`chosen = pick(true)`) widened by a coerce environment plus a thunk, `pick` returning `fun(i32).R: i32` | no (the `fun` return is environment-free; the coerce environment at `list.map(xs, chosen)` is heap but is created and freed inside the one statement) | `... the return type `fun(i32).R: i32` of `pick` yet` |
| `stdlib_combinators` | **no captures at all**: every callback is a non-capturing literal or a declared function (`half`, `parse_positive`) widened into `&closure(&String)`, `&closure(&i32)`, `&closure()`; `option`/`result` combinator bodies | no | `... the type `&closure(&String).R: usize` as an argument to `std.option.map<String, usize>` yet` |
| `stdlib_list_callbacks` | **no captures at all**: non-capturing literals widened into `&closure(i32)` / `&closure(&String)` for `list.map`/`filter`/`each`/`any`/`all`/`find_index`, an owned `String` element type | no | `... the type `&closure(i32).R: i32` as an argument to `std.list.map<i32, i32>` yet` |
| `try_drops_frame` | a non-capturing literal whose body uses `?` against its own return type and owns a `String`; an indirect call | n/a | `... a function value yet` |

Measured consequence of the middle column: **`closure_basic` is the only one of the ten that
cannot be compiled with a stack-resident environment**, and it covers, on its own, the three
escaping shapes the ten fixtures use: returned from a function, stored in a `datac` field, and
stored in a `Box`. One boundary shape is in no fixture at all, a hidden environment and
a `Mem`-class return on the same indirect call, and it is reachable with a wide `datac` return
(§6 trap 8); a closure returned through an indirect call is not reachable, because the frontend
refuses a function type or a closure type whose result is a function type.

Where each first refusal is raised (`codegen_asm.rs` at HEAD): the parameter check `:4504`, the
return-type check `:4541`, the closure-expression refusal `:8652`, the closure-capture refusal
`:8654`, the passdown storage check `:3121`, the call-argument type check `:10576`. All six are
first-refusal sites for one of the ten, and none is a fixture-specific special case.

## 5. Slice order

### 5.0 The primitives the whole row needs

These are the pieces every slice below shares. A writer should land them with slice 1 or 2 rather
than at the end.

* **`Ty::Fn` as one INTEGER eightbyte.** A small predicate (the twin of `str_ty`) and its use at
  `value_kind` (a `Width::W8` value, and `Ty::Ref{inner: Fn}` as the same one word), `arg_regs_for`
  (both spellings, `(1, 0)`), `RetClass` (`Scalar`), `try_build_layout` (a `Scalar` `Width::W8`
  field, so `datac Callbacks(val on_i32: fun(i32).R: i32)` has a layout), `pd_storage_size` (`:745`,
  8 bytes), `pd_init` (`:5096`, one `mov` to the global), and `Deref` (`:8788`, an 8-byte load, the
  passdown read).
* **`FnRef` and `IndirectCall` on a `Ty::Fn` callee.** `FnRef` is `lea rax, [rip+symbol]`; the
  indirect call evaluates the callee into `r11` and issues `call r11` with the ordinary argument
  marshalling and the ordinary return handling. No hidden parameter, no environment.
* **A closure layout, registered like `$Map`.** One synthesized `StructLayout` per closure signature:
  three `INTEGER` eightbytes at offsets 0, 8 and 16, size 24, align 8, class `Mem`. `build_layouts`
  (`:2634`) gains a key per signature from a `collect_closure_tys` walk beside `collect_array_tys`
  (`:1542`), `struct_layout` (`:2711`) and `layout_key` (`:1428`) gain the arm, and
  `types::closure_mangle` spells the key so `nm` and the C backend name the same symbol.
* **`Ty::Closure` in the three ownership tables.** `ty_owns` (`:2122`) answers `true` for it;
  `drop_site_for` (`:5088`) gains an arm returning a new `DropSite::Closure(params, ret)` whose label
  is `types::drop_fn_name`'s `kelp_closure_drop__<mangle>`; `hoist_owned_temporary` (`:7965`) and
  `register_borrowed_temporary` (`:5053`) reach it for a closure temporary; `require_aggregate_drop`
  (`:5581`) gains the `Ty::Closure` field arm so a `datac` holding one registers the closure drop.
* **The two generated routines of §3**: the per-literal environment drop and the per-signature
  closure drop.

### Slice 1: function values and a call through one

Reaches `fn_literal`, `fn_value`, `try_drops_frame`. Content: §5.0's first two bullets only. No
layout, no environment, no `malloc`, no new drop.

**Acceptance.** `./target/release/kelpc build --backend=asm kelpc/tests/fixtures/fn_literal.kp`
builds and its binary prints the C backend's stdout byte for byte, which the differential sweep
asserts; `ASM_BUILT_BASELINE` in `kelpc/tests/backend_coverage.rs` re-records `fn_literal`; the
ranking row loses `fn_literal` and, if the passdown arm landed with it, `fn_value` and
`try_drops_frame` as well; and any of the three that still refuses names its own next gap with the
`REFUSAL_PREFIX` (its second refusal is unmeasured; the sweep is what measures it). This slice is
independently verifiable: `fn_literal` is a complete program whose whole output is an integer, and
it prints after a `move String` argument has passed through an indirect call, which is the one
`Mem`-class interaction the slice has.

### Slice 2: the environment-free closure value

Reaches `closure_datac_borrow_param`, `stdlib_combinators`, `stdlib_list_callbacks`. Content:
§5.0's closure-layout bullet, `Ty::Closure` accepted in `arg_regs_for`/`value_kind`/`RetClass`,
`FnToClosure` over an `FnRef` emitting the per-signature **shim**, `IndirectCall` through a
`&closure` callee (`->fn`/`->env`), `ClosureLit` still refused by name, and the closure-value drop
routine (the `env == NULL` path is what this slice exercises, so the per-signature drop is needed
here even though `env` is always `NULL`: the C backend emits it for every signature and the
temporary's drop calls it). No capture, no `malloc`, no environment drop.

**Acceptance.** `closure_datac_borrow_param` builds and prints the C bytes (its output is
`1` from a callback over a `List<Item>`): it is the smallest complete program in the row that needs
the layout, the shim, the borrowed `&closure` parameter and the indirect call through it. The two
stdlib fixtures are the same slice with a `std` function in the middle, and they are named as
reached rather than promised: whether `std.list`/`std.option`/`std.result`'s own bodies contain a
further gap is not measured (§7). Ratchet and ranking as in slice 1.

### Slice 3: captures, and the environment they live in

Reaches `fn_literal_capture`, `closure_move`, and the capturing half of `stdlib_closure_callbacks`
(its owned `String` capture is the only owned capture passed as a callback in the row).
Content: `ClosureLit` (heap environment, capture stores in field order, the `"out of memory"`
panic, the three-word value), `ExprKind::Capture` (the hidden parameter bound in the prologue to a
frame slot, the field offset from the literal's environment layout, and the address of that field
for a borrow), the per-literal environment drop, and §5.0's `ty_owns`/`drop_site_for` arms so a
closure binding, a closure temporary and a closure field all drop.

**Acceptance.** All three build, and the three known positions of `closure_move` (block end, early
`return`, one per loop iteration) drop the environment once each, which the valgrind gate
(`cargo test --release --test memory_gate -- --ignored`) is what actually checks: a missed drop is a
leak that prints nothing, and a second one is a double free. `fn_literal_capture`'s
acceptance is subtler and cheap: `k` is printed after the literal is built, so the `Copy` capture
must have been copied rather than moved (a moved binding would be a frontend rejection, so this is
the fixture that pins "the environment owns a copy" and not the trap).

### Slice 4: the escaping and owned cases

Reaches `closure_basic` and the runtime-widening half of `stdlib_closure_callbacks`
(`list.map(xs, chosen)`). Content: `FnToClosure` over a non-`FnRef` (the one-field coerce
environment, its `free` drop and its per-signature thunk), a `Mem`-class closure **return** (already
in `RetClass`, reached once the layout exists), a closure in a `datac` field and in a `Box` (both
already work once the layout and the drop arms exist), a `move closure` parameter by value (the
`ArgSlot::Stack` path), and `Box.get`/`set`/`take` over a non-`Copy` element (already the pointer
form).

**Acceptance.** `closure_basic` builds and prints the C bytes, and it is the fixture that proves the
escape: the closure `make` returns is called after `make`'s frame is gone, so a stack environment
fails loudly (a garbage length) rather than silently, and the `Box` and `datac` shapes prove the
environment is freed by whichever binding owns it. The valgrind gate on `closure_basic` is the
second half of the acceptance: the frame's `move` parameter's environment, the `Box`'s and the
`datac`'s closures must each free once.

**Slice order rationale.** 1 is reachable and verifiable alone and needs no ABI. 2 is the smallest
step that introduces the fat pointer itself, and it deliberately does so with `env == NULL`, so the
first fixture that uses a closure has no heap allocation in it at all. 3 introduces the heap, after
the value shape is already byte-compared. 4 is the boundary cases, and it is the only slice whose
failures are a crash or a leak rather than a refusal, which is why it comes last with the memory
gate already producing a number.

## 6. TRAPS: what would be a silent miscompile here, and what catches it

This backend's two known silent-miscompile shapes are a **wrong stack slot** and a **wrong eightbyte
split**. Both are reachable from this row, and the list below says how each one is produced, what it
does, and the fixture or gate that would catch it.

1. **Environment field offsets (wrong stack slot).** Every environment in the corpus has one field
   at offset 0, so a layout that put `c1` at `8 * i` instead of at its natural-aligned offset would
   pass all ten fixtures and read the wrong bytes for any two-capture literal (an `i32` followed by
   an `i64` is the cheapest case: C pads the `i64` to offset 8, naive packing puts it at 4). A
   wrong read here is a wrong number, not a crash. The shape is legal and builds under the C
   backend today (measured probe: a literal capturing an `i32` and an `i64` prints `8`), so only
   the corpus is thin, not the language. **Catcher:** no fixture exists; a new
   `closure_two_captures.kp` with a `Copy` and an owned capture of different alignments, whose
   printed value names both fields, is the pin.
2. **The borrowed closure temporary's slot (wrong stack slot).** `f(g(...), h(...))` where both
   arguments are closure temporaries: each needs its own `Frame::scratch` slot. Reusing one slot (or
   `agg_slot`) hands the callee the second closure's `fn`/`env` for the first argument, which is a
   wrong number or a crash depending on which pointer lands where. **Catcher:** no fixture passes
   two closure temporaries to one call; a new fixture with a two-parameter function over two
   capturing literals is the pin, and it is the same defect the `nested_literal_staging` fixture
   pins for `datac` literals.
3. **The literal function's environment slot (wrong stack slot).** The hidden environment pointer
   has to live somewhere the body can reload it after any nested call. If it is kept in a
   caller-saved register, or parked in a slot that collides with `sp_slot`/`ret_ptr_slot`/`agg_slot`,
   a `Capture` read after an intervening call reads a stack pointer or a return address. A wrong
   number, or a crash if used as a place. **Catcher:** every capturing fixture, `fn_literal_capture`
   first (it calls `k` only through the environment, and one nested call happens between the literal
   and the read), plus the differential sweep.
4. **The closure's class (wrong eightbyte split).** 24 bytes is three INTEGER eightbytes and
   `AggClass::Mem`. Classifying it as `One(Int)` (a size-only or `is_copy`-shaped mistake) passes
   only the first word to a `move closure` parameter and the callee reads a garbage `env` and `drop`
   that are still the caller's stack: a wrong number at best, a free of a stack address at worst.
   Classifying it as `Two` drops the third word, so the `drop` pointer arrives as whatever was there
   and the environment leaks or the wrong function is called. **Catcher:** `closure_basic`'s
   `own(fun(x: i32).R: i32 = x - k, 4)` is the only by-value closure argument in the row; the
   differential sweep catches the wrong number, the valgrind gate catches the leak and the
   double free.
5. **The `datac` field's split (wrong eightbyte split).** `Holder` is one 24-byte field, so it is
   `Mem` whatever the field placement; the silent version needs a second field. A closure field
   followed by an `i32` field places the `i32` after 24 bytes, and a field walk that used the
   closure's *class* rather than its size would place it at 8 or 16. **Catcher:** no fixture has a
   closure field plus another field; a new `datac` fixture with `(cb: closure(...), n: i32)` pins
   the offset through `n`.
6. **A missed environment free behind a `clear_if_move` (wrong stack slot, inverted).** A closure
   moved out of a `var` leaves a 24-byte slot that the block-end drop still visits; the clear has to
   zero three words, not one. A one-word clear leaves `env` and `drop` live, so the block-end drop
   frees the environment a second time. **Catcher:** no fixture reassigns or moves a closure
   binding; `closure_move` pins the single-owner case, and a new fixture that reassigns a
   `var f: closure(...)` pins the moved-out one.
7. **Indirect-call argument order versus callee order.** The C backend stages some argument preludes
   before the callee temporary (`render_arguments` then the callee assignment), so a printing
   argument and a printing callee print in C's order. If the asm backend's indirect call evaluates
   the callee first and the argument has an effect that prints, the two backends disagree on stdout
   while both are self-consistent. **Catcher:** the differential sweep, but only if a fixture has
   an effecting argument at an indirect call; none does. A new fixture with `f(print_and_return())`
   at an indirect call is the pin.
8. **The environment number when the callee also returns a `Mem`-class value.** This is reachable,
   and with a **wide `datac` return rather than a closure return**: the frontend refuses a function
   type or a closure type whose result is a function type, so no indirect call can return a closure,
   but `fun apply(f: closure().R: Wide).R: Wide = f()` over a 24-byte `datac Wide` compiles. The C
   it generates is `(_xtmp0->fn)(_xtmp0->env)` with `struct_Wide` as the return type, and C's own
   ABI puts the hidden return pointer in `rdi` and the environment in `rsi`, not the other way
   round (measured probe `env_wide_ret`, which prints `9`; §2.3 is the rule, and this probe is what
   makes it reachable rather than hypothetical). An asm indirect call that numbers the environment as
   `ARG64[0]` regardless of the return class passes it where the callee expects the hidden return
   pointer, which is a wrong-address call. **Catcher:** no fixture; the probe's shape as a fixture
   is the pin.

## 7. What could not be determined

* **The second refusal of every one of the ten.** This backend stops at the first, so each fixture
  is a first-refusal measurement and not a build. The four slices above name their fixtures as
  reached, and the sweep is what measures whether a further gap sits behind each. The three stdlib
  fixtures are the ones most likely to hide one, because their callbacks run through
  `std.list`/`std.option`/`std.result` bodies this file did not read.
* **Whether a two-field environment is ever produced.** No fixture in the corpus has one (measured
  over all ten generated C files). The language allows it and it builds under the C backend today
  (a probe literal capturing an `i32` and an `i64` prints `8`), so its layout rule is specified
  from C's sequential natural alignment rather than pinned by a corpus fixture.
* **Whether the same is true of a closure field followed by another field.** No fixture has one
  either, and no probe was built for it; the offset rule is `try_build_layout`'s, which the fixture
  would only be pinning rather than defining.
* **Whether any fixture needs the escape analysis this spec declines to write.** Nine of the ten
  compile under an always-heap mechanism; whether a stack environment would compile them with the
  same output is not measured, because that mechanism is not proposed.
* **The exact set of files a writer touches.** The slices name the functions, and the primitives
  list names the sites at HEAD's line numbers, but two of the sites (`build_layouts` and
  `Gen::function`) are owned by other slices' code and a rebase can move them.
* **Whether the coerce path is reached by any fixture other than `stdlib_closure_callbacks`.** One
  `kelp_CoerceEnv` appears in that fixture's generated C and in none of the other nine; whether the
  same path is reachable from a program shape not in the corpus is not measured.
