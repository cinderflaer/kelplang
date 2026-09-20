# MK7 next slice: reconnaissance brief

Branch `mk7/own-backend`, HEAD `c3a8759` ("a `datac`/`enum` list element gets a derived copy and
order"), measured 2026-09-20. This brief measures the candidate slices below against the backend at
that commit. It is a specification lane: it changes no source. Every refusal text below was
reproduced by hand with

```
./target/release/kelpc build --backend=asm kelpc/tests/fixtures/NAME.kp -o /tmp/next_NAME
```

and the C backend was built for every fixture named reachable, because a fixture the oracle rejects
is not a target. Every number in this file is `c3a8759`'s, measured in a detached worktree of this
checkout (`git worktree add --detach /tmp/mk7-meas-HEAD c3a8759`), because three other lanes were
working in the shared checkout at the time; the worktree was removed afterwards. The plan is
`plan/mk7-own-backend.md`, and this file is the reconnaissance its next round is chosen from.

**The brief before this one measured a different backend, and its numbers are history.** It was
written at `3a2f6f9` and addended three times, last at the float-aggregate slice, and it measured
`144 built of 168 fixtures the C backend builds, 24 refused`, an asm suite of `240 of 289`, and an
`enum` row of 0. Every one of those is superseded below. The old text is kept rather than deleted,
because §9's first lesson is that a slice's own brief is what its acceptance is judged against, and
because two of its four candidate measurements were corrected by measurement rather than by
argument. It is the appendix at the end of this file; its section letters are the previous round's,
and its numbers are the round it was written in.

## 0. The refusal ranking, verbatim

Command: `cargo test --release --test backend_coverage -- --ignored --nocapture` (36.9 s). The
`the_refusals_rank_by_the_categories_the_plan_uses` test prints:

```
asm backend refusal ranking (25 fixtures refused of 201 the C backend builds, 176 built)
  a heap type: 14
    borrow_chain_through_a_borrow
    fs_errno
    fs_fallible
    fs_symlink_kind
    io_stdio
    io_streams
    list_datac_clone_sort
    net_loopback
    net_poll
    net_resolve
    net_timeout
    path_absolute
    path_normalize
    process_spawn
  a fixed-size array (A-18): 0
  an `enum`: 0
  a `datac` holding a float or heap field: 0
  a closure or a function value: 10
    closure_basic
    closure_datac_borrow_param
    closure_move
    fn_literal
    fn_literal_capture
    fn_value
    stdlib_closure_callbacks
    stdlib_combinators
    stdlib_list_callbacks
    try_drops_frame
  a gated extern: 1
    caps_time_rand
  total: 25
```

The same run prints one note the ratchet has to act on:

```
note: the asm backend builds 1 fixture(s) the baseline does not name yet, re-record ASM_BUILT_BASELINE: borrow_two_phase
```

`borrow_two_phase` is the fixture `609d7df` wrote for "a method receiver is borrowed after its own
arguments". This backend builds it and prints the C backend's bytes; `ASM_BUILT_BASELINE` in
`kelpc/tests/backend_coverage.rs` does not name it yet, so the recorded list is one behind the
measurement rather than ahead of it.

Six rows are printed and three of them are empty, which is the part of this table worth reading
first: a fixed-size array (0), an `enum` (0) and a `datac` holding a float or heap field (0) have left
the ranking since the last brief. What remains is three rows, and the first of them is not one gap.

> **Re-measured 2026-09-20 at `caeb98d` (now `8d86d8e`), after this brief was written; this
> blockquote is the tip's state and §0 above is `c3a8759`'s, kept as the record.** The ranking is **24
> fixtures refused of the 202 the C backend builds, 178 built**: a heap type 13, a fixed-size array 0,
> an `enum` 0, a `datac` holding a float or heap field 0, a closure or a function value 10, a gated
> extern 1. The heap row is where the movement is: `path_normalize` and `path_absolute` build now,
> because this round compiles `std/rt` and lowers `std.path.normalize_raw` and `std.fs.cwd_raw`, and
> `caps_gated_path` joins the row as the pin for the gated `fs.read` a capability check has to reach,
> so 14 becomes 13. The note §0 quotes one paragraph up is settled: `borrow_two_phase` is recorded in
> `ASM_BUILT_BASELINE`, so the sweep names no unrecorded build. The asm suite is **288 of 336** (the C
> backend passes all 336), and valgrind is clean over **380 runs / 358 fixtures** (`c: 202 checked, 0
> findings`; `asm: 178 checked, 24 refused, 0 findings`). The target set this brief measures is
> therefore 24 fixtures rather than 25, and the heap row within it is 13 rather than 14. Commands:
> `cargo test --test backend_coverage -- --ignored --nocapture`, `cargo test --test compile_and_run`
> with `KELPC_BACKEND=asm`, and `cargo test --test memory_gate -- --ignored --nocapture`.

## 1. Each remaining refusal, reproduced by hand

The command is the one at the top, once per fixture. **All 25 build under the C backend at this
commit**, so every one is a target and none is an oracle rejection (measured by building each of the
25 with `--backend=c`). The three groups, with the first refusal each fixture prints:

**a heap type (14).** Twelve of these name an `extern fun` whose body this backend has no routine
for; one names a `List` method; one names a borrow. The ranking puts all 14 in one row because the
heap row is the catch-all, not because they are one mechanism.

```
fs_errno                       error: the 'asm' backend does not support the `extern fun std.fs.read_raw` yet
fs_fallible                    error: the 'asm' backend does not support the `extern fun std.io.is_open_raw` yet
fs_symlink_kind                error: the 'asm' backend does not support the `extern fun std.fs.write_text_raw` yet
io_stdio                       error: the 'asm' backend does not support the `extern fun std.io.is_open_raw` yet
io_streams                     error: the 'asm' backend does not support the `extern fun std.io.is_open_raw` yet
net_loopback                   error: the 'asm' backend does not support the `extern fun std.net.connect_raw` yet
net_poll                       error: the 'asm' backend does not support the `extern fun std.io.wait_readable_raw` yet
net_resolve                    error: the 'asm' backend does not support the `extern fun std.net.resolve_raw` yet
net_timeout                    error: the 'asm' backend does not support the `extern fun std.net.connect_raw` yet
path_absolute                  error: the 'asm' backend does not support the `extern fun std.path.normalize_raw` yet
path_normalize                 error: the 'asm' backend does not support the `extern fun std.path.normalize_raw` yet
process_spawn                  error: the 'asm' backend does not support the `extern fun std.io.read_raw` yet
list_datac_clone_sort          error: the 'asm' backend does not support the method `List.clone` on a `List` yet
borrow_chain_through_a_borrow  error: the 'asm' backend does not support a borrow of anything but a named local (a scalar, a `datac` or a heap type), a `datac` temporary, or a field of one yet
```

**a closure or a function value (10).**

```
closure_basic                  error: the 'asm' backend does not support the parameter `f: &closure(i32).R: i32` of `apply` yet
closure_datac_borrow_param     error: the 'asm' backend does not support the parameter `f: &closure(&Item)` of `each` yet
closure_move                   error: the 'asm' backend does not support a function value yet
fn_literal                     error: the 'asm' backend does not support the parameter `f: fun(i32).R: i32` of `apply` yet
fn_literal_capture             error: the 'asm' backend does not support a function value yet
fn_value                       error: the 'asm' backend does not support the passdown `pd doubler` (its type is `fun(i32).R: i32`, and the asm backend has no storage for one - see plan/mk7-own-backend.md §5) yet
stdlib_closure_callbacks       error: the 'asm' backend does not support the return type `fun(i32).R: i32` of `pick` yet
stdlib_combinators             error: the 'asm' backend does not support the type `&closure(&String).R: usize` as an argument to `std.option.map<String, usize>` yet
stdlib_list_callbacks          error: the 'asm' backend does not support the type `&closure(i32).R: i32` as an argument to `std.list.map<i32, i32>` yet
try_drops_frame                error: the 'asm' backend does not support a function value yet
```

**a gated extern (1).**

```
caps_time_rand                 error: the 'asm' backend does not support the `extern fun std.time.now_unix_raw` yet
```

Two quotations are written with a comma or a hyphen where the printed text carries an em dash,
because this file carries no em dash: the sweep's `note:` line above, and `fn_value`'s message before
"see plan/mk7-own-backend.md §5". Nothing else about either is changed.

The refusal sites, for the reader who wants to see how much of each gap is a refusal and how much is
a missing body: the parameter check is `codegen_asm.rs:4364`, the return-type check `:4401`, the
function-value and closure-capture refusals `:8487`/`:8494`/`:8496`, the borrow refusal `:8671`, the
passdown storage check `:3015`, the gated-call refusal `:10353`, and the live-extern check that runs
before all of them `:2966`.

## A. Closures and function values (§5 item 6)

**Exact current refusals.** The ten in group two of §1. One fixture (`fn_value`) is refused at the
passdown storage check rather than at a closure expression, because a `pd` of function type has no
generated storage; the rest are refused at the expression or at a signature.

**Reach, verified: 10.** All ten build under the C backend (measured one by one), and they are the
whole row, so no other slice has to land first for them. This is measured rather than assumed: none
of the ten imports a gated module (checked with `grep` over the C-built set, which is how the gated
count in §C was taken), none calls an `extern fun` this backend lacks, and every heap type they use
(`String`, `Bytes`, `Box`, `datac`, `enum`) already lowers.

**Exact C-side thing to mirror.** The frontend half is done and is in the trees both backends walk:
`tir`'s `FnRef`, `ClosureLit`, `Capture`, `FnToClosure` and `IndirectCall` nodes already exist, and
this backend's own liveness and drop-plan walkers already traverse them
(`codegen_asm.rs:1067`, `:1170-1180`, `:1530`, `:1547`, `:1578`). What is missing is emission, and
`codegen_c.rs` is the mirror:

- the instantiations `Instantiations.closures` (one closure struct per signature, `{ fn, env, drop }`),
  `.coerced` (a plain function value widened into a closure) and `.shims` (a widened value whose
  function is known), `codegen_c.rs:2160-2196`;
- `ExprKind::FnRef` as the C function itself, `ExprKind::ClosureLit` as "allocate the environment,
  move or copy each capture into it, hand back the fat pointer", `ExprKind::Capture` as `env->k{n}`,
  `ExprKind::FnToClosure` as a thunk or a shim with `env == NULL`, and `ExprKind::IndirectCall` as a
  call through the fat pointer's `fn` with its `env` as the hidden first argument:
  `codegen_c.rs:2938-3010`;
- the environment's own struct and generated drop, named by `types::env_struct_name` and
  `types::env_drop_name` with `types::capture_field` for the offsets.

**The mechanism, one paragraph.** A closure signature is a three-field aggregate, so it is a
`MEMORY`-class value exactly like `$Map` and rides the aggregate marshalling that is complete. A
capturing literal allocates its environment on the heap (the PLT `malloc` this backend already
calls), writes each capture at its field offset, and drops it through a generated per-environment
drop that this backend already knows how to build for a `datac`. A named function as a value needs no
environment: it is a shim whose signature ignores the hidden argument. A call through a function value
is an indirect `call rax` after the fat pointer is evaluated into a slot once, with `env` passed as
the first argument; that is the one genuinely new ABI shape in the row, and it is a signature this
backend generates at both ends. The one extra piece is item 9's passdown storage, which must accept a
function-typed value (`fn_value`).

**Size and risk.** Estimated 600 to 900 lines in `codegen_asm.rs`, the largest single mechanism left.
`codegen_c.rs` names closure, thunk, shim or capture 116 times; this backend names them 25 times, and
those are mostly the walkers above. Risk is high and of the memory kind rather than the wrong-number
kind: an environment drop that misses a field is a leak the differential sweep cannot see (the
environment is heap and prints nothing), and `closure_basic`, `closure_datac_borrow_param` and
`try_drops_frame` are in the valgrind pass for exactly this. The mitigation already exists: the
differential stdout sweep, the valgrind gate (§ measured below), and the C backend as the oracle for
every capture order.

## B. The heap-bodied externs (§5 items 5 and 10)

**Exact current refusals.** The twelve in group one of §1 that name an `extern fun`: `fs_errno`
(`std.fs.read_raw`), `fs_fallible` (`std.io.is_open_raw`), `fs_symlink_kind`
(`std.fs.write_text_raw`), `io_stdio` and `io_streams` (`std.io.is_open_raw`), `net_loopback` and
`net_timeout` (`std.net.connect_raw`), `net_poll` (`std.io.wait_readable_raw`), `net_resolve`
(`std.net.resolve_raw`), `path_absolute` and `path_normalize` (`std.path.normalize_raw`), and
`process_spawn` (`std.io.read_raw`).

**Reach, and it is two numbers rather than one.** The first refusal of 12 fixtures is one of these
bodies, and `caps_time_rand` (the gated row) needs `std.time`'s bodies as well, so the extern family's
first-refusal reach is **12, or 13 with the gated row's time externs**. Only three of the twelve are
ungated: `io_stdio`, `io_streams` and `path_normalize` import `std/io` and `std/path` only, and
`stdlib.rs:124-134`'s `domain_of` lists `fs`, `net`, `process`, `env`, `time` and `rand`, so those two
modules gate nothing. The other nine (`fs_errno`, `fs_fallible`, `fs_symlink_kind`, `net_loopback`,
`net_poll`, `net_resolve`, `net_timeout`, `path_absolute`, `process_spawn`) call a free function of a
gated module, and **the capability check is refused at the call site, so they need §C as well**. That
is measured, not argued: `resolve.rs:418-434` fills the gate table with a gated module's non-extern
free functions, so an extern is never itself gated, and the live-extern pre-pass in `generate`
(`codegen_asm.rs:2966-2976`) runs before the call-site refusal (`:10353`), which is why every one of
these names a body today and none names the gate.

**Exact C-side thing to mirror.** `externs.rs` holds the bodies, and they are not one-liners.
`std.fs.read_raw` (`externs.rs:152`) is a `fopen`/`fseek`/`ftell`/`fread` sequence with three errno
paths; `std.fs.list_raw` (`:212`) calls `scandir`; `std.io.read_raw` (`:349`) is a `read` into a
fresh `Bytes` with its length rewritten; `std.process.spawn_raw` (`:418`) builds two pipes and forks
with `execvp`; `std.net.connect_raw` and `std.net.listen_raw` (`:440` onward) build an `addrinfo` and
call `getaddrinfo`, and `std.net.local_port_raw` reads a `sockaddr_storage` back through
`getsockname`. `std.path.normalize_raw` (`:312`) is the one whose body is already Kelp in spirit: it
allocates a `Bytes` and calls `kelp_std__rt__path_normalize`, which is `std/rt.kp`'s
`path_normalize`.

**Size and risk.** The largest work item on the board by far, and its size is countable: the modules
involved declare 67 `extern fun`s (`std/fs` 27, `std/io` 11, `std/net` 13, `std/process` 10,
`std/path` 1, `std/time` 3, `std/rand` 2, from `grep -c 'extern fun'`). Not all of them are live for
any one fixture, but the bodies that are live are hand-written assembly over C struct layouts this
backend has no notion of (`addrinfo`, `sockaddr_storage`, `timeval`, `stat`), plus process control
(`fork`, `execvp`, `waitpid`, `pipe`) and a directory scan. Risk is high: a wrong offset in a
`sockaddr` is a wrong port or a wrong address, and the net fixtures are loopback tests, so a
self-consistent mistake can pass two ends of the same wrong thing.

**Anything on the gate's path.** `std/time`'s three bodies are small and are the only ones the
`bench/speed` gate needs (see the gate section below), and `std/rand`'s two are `getrandom`-shaped.
Those five plus §C are a separable, small first half.

## C. The capability check (§5 item 7)

**Measured: 0 fixtures on its own.** No fixture's first refusal names it. The text is
`codegen_asm.rs:10355`'s ("the capability check that gates ... the asm backend cannot emit an
unchecked call, so it refuses instead"), and it appears nowhere in the 25 messages of §1. The plan's
§5 calls this row "7 fixtures"; measured, ten C-built fixtures import a gated module (the ten listed
in §B) and the ranking's own heuristic names exactly one of them, `caps_time_rand`, because its first
refusal is `std.time.now_unix_raw`.

**Everything that imports a gated module, measured.** `caps_time_rand`, `fs_errno`, `fs_fallible`,
`fs_symlink_kind`, `net_loopback`, `net_poll`, `net_resolve`, `net_timeout`, `path_absolute`,
`process_spawn`. Every one of them needs the check, and every one is masked behind an extern body,
so the capability row will stay at 0 refusals until §B's time and syscall bodies land, and then
become the first refusal of all ten at once.

**Why it is smaller than the plan assumed.** The matcher is already Kelp (`std/rt.kp`, 296 lines:
`glob_match`, `dir_wildcard_at`, `host_glob`, `host_glob_at`, `host_match`, `host_end`, `has_colon`
and `path_normalize`), and it **lowers on this backend today**. Measured: a probe module holding
those functions copied verbatim behind `impl "std/str"`, with a `main` that calls three of them,
builds under both backends and prints the C backend's bytes (`true`, `true`, `true`). What this
backend refuses is calling them, because `generate` skips every definition in `std/rt.kp`
(`codegen_asm.rs:3083`) and `call` refuses the call (`:10346`). So the new code is not the
matcher: it is the call-site emission (the same shape as `codegen_c.rs:3419`'s `cap_checked_call`,
which passes the domain, the resource argument, the module and the site as arguments to a check
function), the check routines themselves as assembly, and the allowlist data. The C side of that glue
is `kelpc/runtime/cap_glue.c`, 182 lines: pattern buffering, the deny path (`fprintf` plus `exit`),
the allowlist loop and `kelp_resolve_path`.

**Size and risk.** Moderate: no new matcher code, no new ABI, and the allowlist is data. Risk is the
security kind rather than the wrong-number kind, which is why it should not ride inside another
slice: a check emitted leniently widens every grant in every program. The existing
`kelpc/tests/cap_matcher.rs` corpus is the differential control for the matcher, and the ten fixtures
above are the control for the call sites.

## D. `List`'s derived copy and order: the asm half of `c3a8759`

**Exact current refusal.** `list_datac_clone_sort`: `error: the 'asm' backend does not support the
method \`List.clone\` on a \`List\` yet`. The same fixture also refuses `.sort()` and `.slice()` on a
`datac` element (measured by splitting its statements into probes):
`List<Digest>.clone()`, `.sort()` and `.slice(0, 1)` each refuse with the `List` method message,
while `List<i32>.clone()` and `List<String>.clone()` build.

**Measurement, and it is the branch's own newest commit.** `c3a8759`'s subject is "a `datac`/`enum`
list element gets a derived copy and order": it added the frontend fixpoints (`types.rs:504`
`value_is_clonable`, `:513` `value_is_sortable`, `:528`/`:536` the `clonable_types`/`sortable_types`
least fixpoints), widened `infer.rs`'s `list_sig` to use them, and made the C backend emit
`kelp_clone_<T>` and `kelp_cmp_<T>` beside every `datac` and `enum` typedef
(`codegen_c.rs:1142`/`:1163` and `:1261`/`:1287`), with `write_list_s1` (`codegen_c.rs:2041`)
gating on `value_is_clonable`/`value_is_sortable` (`:2050`). This backend's dispatch still gates on
the old, element-only predicates (`codegen_asm.rs:10694-10704` against `types.rs:483`
`list_sortable_elem` and `:497` `list_clonable_elem`), which answer false for a `datac`, so the
`clone`/`sort`/`slice` arms fall through to the `_` refusal arm. So the two backends currently
disagree about a surface the language already has, and the C backend is the one that is right.

**Reach, verified: 1** (`list_datac_clone_sort`, which is the only fixture in the corpus with a
`datac`/`enum` list element that is cloned, sorted or sliced).

**The mechanism, one paragraph.** Emit a clone and a compare routine per clonable/sortable user type
beside the generated drops this backend already emits: the clone walks the fields (a scalar is a
load/store, an owned field calls its own clone), the compare walks them in declaration order
(numeric compare, `memcmp` for `String`/`Bytes` and for an `enum` tag first), and both are the
`codegen_c.rs` bodies transliterated. Then widen the four dispatch guards to the fixpoint predicates,
which means threading `clonable_types`/`sortable_types` into the dispatch the way `build_layouts`
already threads layouts. `ListFn::Clone` exists (`codegen_asm.rs:1735-1738`, forwarding to `Slice`)
and `List`'s element marshalling already handles an aggregate element.

**Size and risk.** Estimated 300 to 500 lines, most of it generated-routine emission rather than new
code paths. Risk is moderate: a wrong field offset in a clone is a leak or a double free that prints
nothing, and `list_datac_clone_sort` is in the valgrind pass for exactly that (its own comment says
so). The compare half is covered by the differential sweep.

**Why it is worth naming even at one fixture.** It is the only candidate that is a *debt* rather than
a feature: the newest commit on this branch ships a language surface the second backend refuses, and
the plan's own §0 table currently implies otherwise by not naming it.

## E. A borrow of a field chain

**Exact current refusal.** `borrow_chain_through_a_borrow`: `error: the 'asm' backend does not
support a borrow of anything but a named local (a scalar, a \`datac\` or a heap type), a \`datac\`
temporary, or a field of one yet`.

**Exact C-side thing to mirror.** The fixture's own comment cites the rule: `syntax.md` 10.2 says a
chain that steps through a borrow is durable, so `&xs.get(i).v` has an address. The C backend never
had to write a rule for this (a C expression's address is whatever it is). This backend's `Addr`-like
path places a named local's slot, a `datac` temporary and a field of one, and refuses a chain rooted
in a call's *result* rather than in a borrow; the refusal is at `codegen_asm.rs:8671`, one projection
down from the case the fixture is about.

**Reach, verified: 1.**

**Size and risk.** Small: the estimate is 30 to 80 lines in `ExprKind::Ref`'s place-resolution, with
the lifetime argument already made by `borrowck.rs` (the borrow's extent is the enclosing statement
or the pointee's own frame). Risk low.

## The §13 gate, restated at `c3a8759`

The annex (`plan/mk7-own-backend.md` §13) was measured at `d0a485f` and names two prerequisites for
the gate artifact. Measured now:

```
$ ./target/release/kelpc build --backend=asm bench/speed/speed.kp -o /tmp/sp/speed_asm
error: the 'asm' backend does not support the `extern fun std.time.now_mono_ns_raw` yet
```

That is the extern-body refusal, and the annex's other prerequisite, "the scalar `mut` borrow that
`float` and `fib` take as a counter", **has landed**: the scalar-borrow slice is in this backend.
Behind the clock extern, the next refusal would be the capability check, because `std/time` is a gated
module and the gate refusal is at the call site (measured: no fixture reaches it yet, §C).

Everything else in the artifact lowers. Measured: the same file with `impl "std/time"` removed and
`time.now_mono_ns()` replaced by `0` builds under both backends, runs, and prints identical stdout,
with the four checksums and the float value matching `bench/speed/results.txt` exactly (`collatz
131434424`, `float 57735026`, `mix 542344`, `fib 5702887`, `VALUE 78539816.062926`). Wall clock for
that probe, whole artifact, one run each: 0.84 s under this backend against 0.38 s for the C backend's
`-O2` binary. That is not the gate's measurement (one run, no `-O0` column, self-timing removed), and
it is recorded only as evidence that nothing else in `speed.kp` refuses.

So the gate is two prerequisites away, not two of which one is a borrow: `std/time`'s three extern
bodies (small: `clock_gettime`, `clock_gettime`, `nanosleep`) and the capability check, which is §C.
The annex's "What could not be determined" item ("whether the asm backend has further refusals behind
the two above") is now determined: none.

## Close

**Cheapest per line.** **E** (a borrow of a field chain): 30 to 80 lines, one fixture. **D** is the
next cheapest at 300 to 500 lines and one fixture, and it buys something E does not: agreement with
the language's own newest surface.

**Largest per mechanism.** **A** (closures): 10 fixtures, one mechanism, and the only remaining row
that needs nothing from another row first. **B** (the extern bodies) reaches 12 first refusals, one
more than A, but it is not one mechanism: it is 67 declared externs of hand-written assembly over C
struct layouts the backend has no notion of, and 9 of its 12 fixtures also need **C** (the capability
check), which is a second mechanism in its own right.

**Recommendation for the next round: A, closures and function values**, scoped to the whole row, so
it reaches all ten fixtures. Reasons, in this plan's own terms:

1. It is the largest single remaining mechanism on the board: 10 fixtures against D's and E's 1 each
   and B's 12 spread over two mechanisms.
2. It is the only remaining row measured to need nothing else first. Its fixtures import no gated
   module, call no extern this backend lacks, and use no heap type that does not already lower.
3. It is the last *language construct* this backend refuses outright. B, C and D are stdlib surface
   and completeness debt; a program that captures a value in a `fun` literal cannot be built by this
   backend at all, which is a different kind of gap from a stdlib call.
4. Its ABI is already done: a closure signature is a three-field `MEMORY`-class aggregate like `$Map`,
   so the aggregate marshalling, the generated drops and the PLT `malloc` it needs all exist. The new
   primitives are an environment with a generated drop and an indirect call, both codegen rather than
   ABI.
5. The plan's §0 ranks rows by first-refusal count, which puts B first; that ranking is a count of
   refusals and not of mechanisms, and the heap row is the catch-all. The alternative is honest and
   worth stating: if the fixture ratio is the only thing that matters, B plus C is 12 fixtures against
   A's 10, at two to three times the code and with the security-facing half of it in C.

**Must-pass fixtures for A (all ten, and nothing else):** `closure_basic`,
`closure_datac_borrow_param`, `closure_move`, `fn_literal`, `fn_literal_capture`, `fn_value`,
`stdlib_closure_callbacks`, `stdlib_combinators`, `stdlib_list_callbacks`, `try_drops_frame`. All ten
are C-buildable at this commit, and all ten are byte-comparable through the differential sweep. Do not
name a fixture from another row: the acceptance list of the float-aggregate slice named
`try_drops_frame` as an `enum` fixture, and measurement moved it here.

**If a short round is wanted instead: D.** One fixture, the branch's own newest commit's feature on
the other backend, and the smallest change that removes a place where the two backends disagree about
the language rather than about a library.

## Measurement disagreements with the plan, collected

1. **§0's and §5's current-tense numbers.** §0's status table says `283 of 329` integration tests,
   `all 175 fixtures`, `168 C and 144 asm` valgrind and `329 runs over 309 fixtures`; its prose ends
   on `240 of 289`, `144 built of 168, 24 refused` and `heap 13, enum 0, datac 0, closure 10, gated 1`.
   Measured at `c3a8759`: the asm suite is **285 of 336** with the C backend passing all **336**, the
   fixture ratchet is **176 built of 201, 25 refused**, the ranking is **heap 14, array 0, enum 0,
   datac 0, closure 10, gated extern 1**, and valgrind is clean over **377 runs / 357 fixtures, c: 201
   checked, asm: 176 checked, 25 refused, 0 findings**. Commands are in §0 and the gate section.
2. **§0's "What is next" row 1.** It says the heap row is 13 fixtures made of 9 extern bodies, the
   three `List` S1 methods and one template interpolation. Measured: the heap row is 14, and it is 12
   extern bodies, one `List` derived clone/order (`list_datac_clone_sort`) and one borrow chain
   (`borrow_chain_through_a_borrow`). The three `List` S1 methods and the template interpolation have
   landed, and the row's remaining `List` entry is the newest commit's own feature.
3. **§0's rows 2 to 5.** The float-payload row, the passdown row, the `List` S1 leftovers and the
   `Map` row are all marked as landed there; measurement agrees with all four, and the ranking's
   array, `enum` and `datac` rows at 0 are the same statement from the other end.
4. **§0's row 6 and §5's "capability-gated row (7 fixtures)".** Measured: the capability check is the
   first refusal of **0** of the 25, ten C-built fixtures import a gated module, and the ranking's
   gated-extern row is **1** fixture (`caps_time_rand`) because that heuristic keys on
   `std.time.now_unix_raw`. The plan's masking claim is correct and measured (the live-extern pre-pass
   runs first); its count is not.
5. **§13's annex.** It says the gate needs the `std.time` externs *and* the scalar `mut` borrow. The
   borrow landed; the second prerequisite is the capability check, which the annex does not name. Its
   "whether the asm backend has further refusals behind the two above" is now answered: none, with the
   clock calls removed. Its claim that `float` and `fib` "cannot be built at all" is false at this
   commit: both build and run, with matching checksums.
6. **The recorded baseline's own header note.** `ASM_BUILT_BASELINE`'s comment says `195 fixtures the
   C backend builds, 174 of them also built by the asm backend, 21 refused`. Measured: 201 / 176 / 25,
   and the sweep reports one fixture it builds that the list does not name (`borrow_two_phase`).
7. **The old brief's own addenda.** Their numbers are history and are kept in the appendix; nothing in
   them is wrong as a measurement of the round they were written in, and two of their corrections
   (`try_drops_frame` is a closures fixture; `stdlib_parse` needed the float row) are still the record.

## What I could not determine

- **Whether each of the twelve extern-body fixtures builds once the externs and the check land.** This
  backend stops at the first refusal, and the live extern set of a fixture is not machine-readable, so
  the twelve are first-refusals rather than builds. Nine of the twelve also need the capability check,
  and what remains behind that is unmeasured.
- **Whether the closure slice needs anything past the fat pointer, the environment and the indirect
  call.** The second refusal of each of the ten is unmeasurable until the first is lowered; three of
  them (`stdlib_closure_callbacks`, `stdlib_list_callbacks`, `stdlib_combinators`) route through
  stdlib functions whose own bodies might meet another gap.
- **Whether `List.clone` is all of `list_datac_clone_sort`.** Its clone, sort and slice statements each
  refuse at the same message when probed alone, and whether the derived compare for an `enum` element
  (`Mark`) needs anything past the tag-then-payload rule the C backend uses is not measurable until the
  clone exists.
- **Which tests the 51 failures of the asm suite are.** The count is measured (`285 passed; 51 failed`
  of 336); the mapping from each failing test to a row of §0's ranking is not, and the suite names
  tests rather than fixtures.
- **The gate's own number.** `bench/speed` cannot be evaluated as §5's definition of done writes it
  until the clock extern and the capability check land; the probe in the gate section above is not the
  gate, and it uses one run per backend with the self-timing removed.

---

# Appendix: the previous rounds' briefs, kept as the record

The text below is this file as it stood before the `c3a8759` measurement, with two edits and no
others: each em dash it carried is written as a comma, because this file carries no em dash, and
every heading under `##` is demoted one level so the appendix does not collide with the brief above.
Its section letters are the previous round's, and its numbers are the measurement of the round it was
written in. It is kept rather than deleted for two reasons: §9's first lesson is that a slice's own
brief is what its acceptance is judged against, and two of its measurements were corrected by
measurement rather than by argument (`try_drops_frame` is a closures fixture, and `stdlib_parse`'s
missing float payload), which is the record this file exists to carry.
Branch `mk7/own-backend`, HEAD `3a2f6f9`. This brief measures the four candidate
slices below against the current backend. It is a specification lane: it changes
no source. Every refusal text was reproduced by hand with
`./target/release/kelpc build --backend=asm kelpc/tests/fixtures/NAME.kp -o /tmp/next_NAME`,
and the C backend was built for every fixture named reachable, because a fixture
the oracle rejects is not a target.

**Addendum, recorded after the fact (2026-09-19, when the enum slice that section
D recommends landed):** the recommendation was taken and D is done, so every count
in this file is now history rather than a measurement, kept, because §9's first
lesson is that a slice's own brief is what its acceptance is judged against. The
current numbers are **125 built of the 158 fixtures the C backend builds, 33
refused**, the asm suite is **219 of 278**, and the ranking is `heap 16, enum 4,
a `datac` holding a float field 2, closure 10, gated extern 1`. Two of this
brief's measurements were corrected by measurement rather than by argument, and
both are recorded in `plan/mk7-own-backend.md`'s enum blockquote: `try_drops_frame`
is **not** reachable by D, its last function wraps the same `?` in a `fun`
literal, so the closure is its first refusal, which makes it a closures-row fixture
and means that of the fourteen fixtures the slice's acceptance list spells out, the
thirteen below build and that one does not; and the four float-payload fixture
names in section D's split are exactly the four that remain, which is the one part
of that split that held exactly.

**Second addendum (2026-09-19, when section A's own slice landed).** A was taken
next, in the form the brief defines it: the scalar borrow *and* the three `str`
parsers together, since section A itself measures that the borrow alone reaches
zero fixtures. The counts above are history now. The current numbers are **129
built of the 160 fixtures the C backend builds, 31 refused**, the asm suite is
**223 of 280**, and the ranking is `heap 13, enum 5, a `datac` holding a float
field 2, closure 10, gated extern 1`. Section A's central measurement, that
`stdlib_build`, `stdlib_parse` and `borrow_provenance_and_arms` all refuse first
on `extern fun std.str.parse_i64_raw`, and that A alone reaches nothing, held
exactly, as did its statement that all three also needed the `enum` slice (which
had landed by then). What did **not** hold is its implicit reading of them as an
undifferentiated three: `stdlib_build` and `borrow_provenance_and_arms` now build,
and `stdlib_parse` is still refused, for `Result<f64, ParseErr>`, an `enum` whose
payload is a float, which is neither section A's row nor D's. Section A also
predicted this: its "Fixtures it is credited with but cannot reach" paragraph is
the reason the slice was specified with the parsers attached, and the one fixture
it did not foresee is the float payload behind the third. Section A's "Section 13
gate" ordering is unchanged except that prerequisite 3 (the scalar borrow) is now
done: `bench/speed/speed.kp` still refuses, now at the `std.time.now_mono_ns_raw`
extern, and the capability check is still ahead of it.

**Third addendum (2026-09-20, when the float-aggregate slice landed).** The row
section D expected-blocked and section A could not reach is closed, and with it
the last shape item 4 names: a `datac` field or `enum` payload may now be an
`f32`/`f64`. The classifier is per-eightbyte (`INTEGER`/`SSE`, `MEMORY` over
sixteen bytes, an `enum`'s payload through its union), because the measurement in
§1's trap, that all seven reachable fixtures would build with the size-only
classifier, is exactly the self-consistent wrong ABI the plan's §10 warns about.
The current numbers are **144 built of the 168 fixtures the C backend builds, 24
refused**, the asm suite is **240 of 289**, and the ranking is `heap 13, enum 0, a
`datac` holding a float or heap field 0, closure 10, gated extern 1`. The seven
this brief named must-pass, `enum_basic`, `enum_container`, `if_let_basic`,
`if_let_chain_same_scrutinee`, `receiver_methods`, `templates`, `stdlib_parse` , 
all build, run and print the C backend's bytes exactly, which is one more than the
brief's own count of six plus `stdlib_parse`: it measured the reachable set as
exactly those seven and it was right. Everything else in this file is history and
kept as such; the counts it measures differently from the plan are recorded in
`plan/mk7-own-backend.md`'s blockquotes.

---

### 0. The refusal ranking, verbatim

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

### A. A borrow of a scalar local

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
  `std.str.ParseErr` (`stdlib_parse.kp:11-19`), the `enum` row.
- `stdlib_build` calls `str.parse_u64(...)` then `result.unwrap(r = big)`
  (`stdlib_build.kp:16-19`), `Result` is an `enum`.
- `borrow_provenance_and_arms` declares `enum J { S(val x: String) N }` and uses
  `if let S(s) = v` (`borrow_provenance_and_arms.kp:5-20`), the `enum` row.

**Exact C-side thing to mirror.** The scalar out-parameter is a `mut i64`/`mut
f64` parameter, which the C backend passes as a plain pointer. `mut` parameters
are pointers in C; the borrow is the caller's job. The body to mirror is
`kelpc/src/externs.rs:44-52` (`parse_i64_raw`: `*kelp_out = (int64_t)v;`) and its
siblings at `externs.rs:53-67`. The C-side call shape is in
`codegen_c.rs:3223`'s neighbourhood for the address, but the important C fact is
that a `mut` scalar parameter is `int64_t*` and the callee stores through it , 
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
`Routine` bodies (~20 lines each) and a scalar deref-store path. Risk is low , 
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
2. the capability check (§5 item 7), `std/time` is a gated module
   (`kelpc/std/time.kp:1-6`), and the asm backend refuses every gated call at
   `codegen_asm.rs:5440-5444`; with the extern body added, the refusal moves
   here, which is what `caps_time_rand` already reports (`a gated extern: 1`);
3. A's scalar borrow, for the counters.

So A is on the gate's critical path but is the *third* of three prerequisites,
and unblocks zero fixtures on its own.

---

### B. `List.sort`, `List.contains`, `List.slice`, and the generated comparator

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

**The mechanism, one paragraph.** Add four `ListFn` variants, `Cmp`,
`Sort`, `Contains`, `Slice`, emitted per element instantiation the way the
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
leaked/double-freed clone shows up as a *memory* bug, `list_slice_insert` is
already in the AddressSanitizer/valgrind pass, so it is pinned. No ABI/Variadic
surface. Estimated ~150-250 lines.

---

### C. Printing an owned `String`/`View`, the real gap is the template, not `print(s)`

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
interpolation, which is what byte-for-byte prints `String`/`View`.

**The mechanism, one paragraph.** In `Gen::template`, accept `string_ty`/
`bytes_ty`/`view_ty` interpolations: reserve a frame scratch slot
(`Frame::scratch`, already used for aggregate temporaries), evaluate the value
into it, append `%.*s` with `(int)len, data`, and, for an owned `String`/`Bytes`
only, emit the drop (`kelp_string_drop`/`kelp_bytes_drop`) after the `printf`;
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
  this explicitly, "which no fixture reaches", so the criterion must not name
  it.

**Size and risk.** Smallest of the four: ~40-70 lines, all in one function, with
a working mirror and a working single-arg implementation beside it. Risk low;
the only hazard is dropping an owned interpolation before the `printf` reads it,
which the `print_buffer` order already solves.

---

### D. `enum`, the tagged union

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
- Layout and tag: `codegen_c.rs:1040` `write_enum`, `typedef struct { int32_t
  kelp_tag; union { struct { ...fields } kelp_v_Variant; ... } kelp_v; }`,
  tag = variant index; a payload-less variant gets a `char kelp_none` field.
- `EnumLit`: `codegen_c.rs:2667-2697` (`{ .kelp_tag = variant, .kelp_v = { .member = { .field = ... } } }`).
- `EnumTag`: `codegen_c.rs:2698-2700` via `enum_tag_access` (`codegen_c.rs:3223`).
- `EnumField`: `codegen_c.rs:2701-2716` via `enum_member_access` (`codegen_c.rs:3234`).
- `Try`: `codegen_c.rs:2723-2780`, evaluate operand once, test the failure
  tag, rebuild the failure in the target type, else yield the success payload.
- Exhaustiveness: not a backend check, it is enforced in the frontend at
  `infer.rs:6029` ("this 'when' is not exhaustive"), so the backend may assume
  a total `when`.
- Per-enum drop: `codegen_c.rs:1066-1090`, `switch (v->kelp_tag)` with one
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
drops, the datac generated drop (`codegen_c.rs`'s sibling, already emitted for
owned datacs) with a tag switch in front. The ABI needs nothing new for a
scalar or owned payload: the enum is an aggregate like any `datac`, so
`AggClass`/`marshal_elem`/`eval_struct_into` cover it once its layout exists.

**The 18 split by payload, verified against the fixtures.**
- *Float payload* → depends on the float-in-aggregate slice (the same leftover
  §5 item 4 names: "a `datac` holding a float ... at any size"; `agg_class` at
  `codegen_asm.rs` only classifies INTEGER eightbytes): **4 fixtures** , 
  `enum_basic` (`Shape.Circle(r: f64)`), `enum_container` (`Shape` f64 +
  `Token`), `if_let_basic`, `if_let_chain_same_scrutinee`.
- *Closure/function-value payload or callback* → depends on the closures row
  (§5 item 6, 7 fixtures): **1 fixture**, `stdlib_combinators`
  (`stdlib_combinators.kp:38` takes `fun(s: &String) -> String`).
- *Owning payload* (`String`, `Box<Tree>`) → in D's own scope **if D lands the
  generated per-enum drop**, which item 5 explicitly pairs with "an `enum` that
  holds one". The owning types themselves are done (`String`, `Box`, `List`),
  so nothing else has to land first: **10 fixtures**, `enum_owned`,
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

**Expected blocked: 5**, `enum_basic`, `enum_container`, `if_let_basic`,
`if_let_chain_same_scrutinee` (float payload; blocked on the float-aggregate
slice) and `stdlib_combinators` (closures).

**Size and risk.** Largest: layout + `EnumLit`/`EnumTag`/`EnumField`/`Try`
lowering + the generated tag-switching drop + `when`/`if let` codegen. High risk
,  this is the row the risk register calls the home of silent miscompiles
("Aggregate ABI mismatch ... a wrong eightbyte split is a silent miscompile"),
and a wrong tag offset or a union sized to the wrong variant is wrong output
with no crash. The mitigations are the differential stdout sweep (already live)
and `try_drops_frame`/`enum_owned`/`enum_recursive`, which are in the valgrind
pass. Estimated several hundred lines, the biggest of the four.

---

### Section 13 gate, stated fully

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
checksum, one estimator across columns, idle machine), none of which any
candidate slice addresses.

---

### Close

**Cheapest per line of code.** **C** (template interpolation of an owned
`String`/`View`): ~40-70 lines in one function, with both the C mirror
(`codegen_c.rs:2620-2650`) and the asm single-argument implementation
(`print_buffer`/`print_view`) already written. Caveat: A's *borrow arm alone* is
an even smaller diff (~15 lines) but reaches **0** fixtures and unblocks no gate
step, so it is not a slice; C is the cheapest slice that reaches anything.

**Largest per fixture.** **D** (`enum`): 13 reachable fixtures in one mechanism,
against B's 3 and C's 1. It is also the plan's §0 ranking `2` ("the largest
single row at 18") and §5 item 4's remaining headline.

**Recommendation for the next round: D, `enum`**, but scoped to include both
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

### Measurement disagreements with the plan, collected

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
