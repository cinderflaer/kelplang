# Open issues

The running list of bugs and missing features found while working in this
checkout. Operated by agents: the rule lives in `AGENTS.md`, "Issues and
findings".

## Rules

- **Open items only.** Resolving one means deleting it here and adding a line to
  the resolved repository's pending changelog: `kelp/CHANGELOG.pending.md`,
  `coral/CHANGELOG.pending.md` or `lighthouse/CHANGELOG.pending.md`. From there
  the release writes the published wiki entry. Wipe on resolution, do not
  annotate.
- One entry per finding, under its id as the heading. `BUG-<id>` for something
  that is wrong, `MISSING-<id>` for something that is not there yet.
- `<id>` is five mixed-case alphanumerics, like `MISSING-djHo0`. It is opaque
  and not a sequence, so there is no highest-used number to read or continue:
  pick a fresh one that appears nowhere in this file or in any
  `CHANGELOG.pending.md`. Ids are permanent and never reused. The numeric ids
  are history: the highest ever used were `BUG-13` and `MISSING-19`, and
  `BUG-1` to `BUG-3` belonged to the retired public known-issues page. Do not
  continue a numeric series or reuse a numeric id.
- Cite the source: a repro, or the file and section that states it. An entry
  without a source is a claim to remove. If the source is a public page, copy the
  repro in here before that page is cleaned up, and name where it came from.
- **`+++++ HALF FINISHED +++++`** under a heading means the item is partly
  shipped: some of what it asks for is in the compiler and some is not. It is
  still an open item, and it is the one exception to "open items only, no
  annotations": the marker says which half is which.
- `repo:` names where the fix belongs: `kelp`, `coral`, `lighthouse` or `wiki`.
- `effort:` on the `repo:` line is `S`, `M`, `L` or `XL` (defined under
  "Missing features"). Every `MISSING-` entry carries one, and the section is
  kept sorted easiest first: a new entry goes at the end of its tier, and an
  entry whose size changes moves to the right tier. `BUG-` entries are not
  tiered; they go in a `## Bugs` section above "Missing features".
- Date format `YYYY-MM-DD`. Delete a section when nothing is under it.
- **Nothing in this file is public, and nothing like it may be.** A public page
  that names a live defect and hands over a repro tells someone hunting for an
  exploit where to start. Do not publish an entry, and do not put one back in the
  wiki: the public pages describe the language and the released compiler only.
  The wiki's `/meta/known-issues` page was retired on 2026-09-20 for this reason,
  and its one live finding moved here as BUG-11.

This file is tracked in the `kelp` repository (moved here from the `KelpLANG/`
root on 2026-10-06; `KelpLANG/AGENTS.md` is still outside any repository). A fixed
finding lives in that repository's `CHANGELOG.pending.md` and, at release, in its
wiki changelog page.

## Missing features

Sorted easiest first. `effort:` is S (a stdlib function or one small diagnostic), M (one subsystem, one lane), L (touches a hotspot file or needs a design call inside a lane), XL (mark-sized, or blocked on a decision). Within a tier the order is a rough guess; dependencies are named in the entries.

### MISSING-Hd4Lr kelp: no iterator adapters, no `Map` iterator, no `List.iter()`

repo: kelp | found: 2026-10-06 | effort: M

source: wiki `language/limits.md` (unreleased), "Containers": "The iterator
protocol shipped, the adapters did not"; `kelp/plan/iterators-status.md`.

`for` walks a `List`, an array or an `Iterator` implementor. There is no `map`,
`filter`, `take`, `skip`, `enumerate`, `zip` or `chain`, no `List.iter()` /
`into_iter()`, and a `Map` has no iterator (callers iterate `keys()` and look
each one up). The adapters need `impl<I: Iterator> Iterator for Take<I>`, which
is MISSING-Ve8Nc, so this entry waits on that one. The `Map` iterator and
`List.iter()` do not need Ve8Nc but do need a compiler `for` desugar (a library
`MapIter<K, V>` would need the same generic impl) and, for `for (k, v)`, the
tuple destructuring of MISSING-Gt6Qc (shipped), and a `(K, V)` element that is a
borrow, which a tuple cannot hold today: a tuple holds owned data only.

### MISSING-Gt6Qc kelp: tuple patterns cannot hold a literal or a nested pattern

repo: kelp | found: 2026-10-06 | effort: S

source: wiki `language/limits.md` (unreleased), "Data and generics".

Shipped in 26.10.3: `t.0`, `for (a, b) in pairs`, and tuple patterns in `when`
(variant, name or `_` positions). Still open: a literal position (`(1, v)`), a
tuple pattern nested in a tuple pattern, and a tuple value in a `const`
initializer. `for (k, v)` over a `Map` still waits for the `Map` iterator of
MISSING-Hd4Lr, which needs a tuple element that can be a borrow.

### MISSING-Lk9Xs kelp: no `loop`, labelled `break`, or `break` with a value

repo: kelp | found: 2026-10-06 | effort: M

source: wiki `language/limits.md` (unreleased), "Control flow". Also: a
`defer` body cannot contain `break` or `continue`.

`break`/`continue` act on the innermost loop only, so leaving a nested loop
needs a flag variable or a helper function. Unlike `elif` (decided and shipped)
this one has no decision recorded either way.

### MISSING-Cn2Ym kelp: channels have no `select` and no zero-capacity form

repo: kelp | found: 2026-10-06 | effort: M

source: wiki `language/limits.md` (unreleased), "Concurrency".

`sync.bounded`/`unbounded` only. `must_use fun` shipped in 26.10.2 and
`Sender.send` carries it. A thread handle that is bound and never joined is
detached with its record leaked (a bare `thread.spawn(...)` statement is
already refused). `std/pool` bounds concurrency, not thread creation (no
reusable workers).

### MISSING-Tb8Vh kelp: thread-local storage

repo: kelp | found: 2026-10-06 | effort: M

source: roadmap B-11 / MK9-S3-M3; wiki `language/limits.md` (unreleased) calls it
"a named backlog item rather than ruled out, it is simply not built". Per-CPU
storage shipped (freestanding only); hosted thread-local did not.

### MISSING-vT4Kd coral: no `.ckp` (compiled Kelp) archive for an application

repo: coral | found: 2026-10-06 | effort: M

source: the owner's wish, 2026-10-06; today's behaviour is `kelpc build`
(`wiki/tooling/kelpc-cli.md`, "Artifacts") and `coral build` (`wiki/tooling/coral.md`),
and `coral publish` is listed there as an unknown command.

Wish: `coral build` can package an application into one `.ckp` file (compiled
Kelp), an archive in the role a `.jar` plays for Java and Kotlin. It is **not**
portable bytecode: the contents are the same natively compiled code the build
makes today, gathered into one file instead of left as separate files. The
extension is decided; the archive's internal layout is not.

What exists: `kelpc build` emits a native executable (`<out>`, plus `<out>.ll`
and `<out>.o` beside it), and `coral build [--release]` leaves it under
`build/<mode>/`. Nothing is archived: a program with data files or `[extern]`
native libraries ships as a loose set of files.

Open design points for the lane that takes it:

- Contents: the executable, declared resource files, the native libraries named
  in `[extern."..."]`, and a small manifest (name, `YY.MM.N` version, entry,
  target `os`/`arch`, and the capability grants the program declares).
- Format: an ordinary archive (tar or zip) so existing tools can list and unpack
  it; `coral` writes it, so no new `kelpc` surface is needed.
- Running: `coral run app.ckp` unpacks to a cache directory and executes, which
  needs no compiler change. A self-contained form (the archive appended to the
  executable, resources read through a new `std` call) is a later step.
- A `.ckp` is per target, because the code inside is native.

### MISSING-oH9AB coral: `test`, `fmt`, `lint`, `bench`, workspaces, publishing, version ranges

repo: coral | found: 2026-09-20 | effort: L

source: `kelp/plan/roadmap.md`, the Coral row.

**+++++ HALF FINISHED +++++**

Shipped: `coral fmt` and `coral lint` (2026-09-24, MK13-S5-S2), which forward to
`kelpc fmt`/`kelpc lint`, add `--check` for CI, and default to the project's own
sources when no path is named. Nothing is duplicated in Coral for the lint's
settings: `kelpc lint` already reads `coral.toml`'s `naming_case` itself.

Still open: `coral bench` (MK13-S5-M3), workspaces (MK13-S4-M1), publishing and
CI recipes (MK13-S6-M3), and version ranges, which the plan leaves to
MK13-S3-M3's decision. `coral test` (MK13-S5-M1) shipped its first slice the same
day: fixtures under `tests/`, expected stdout and exit code beside them, refused
fixtures, `--json`, and `--valgrind`; a sanitizer profile beyond valgrind and the
bench-shaped half of MK13-S5-M1 are still open. Coral today is
`init`/`build`/`run`/`check`/`clean`, `[tasks]`, the fingerprint cache,
`[dependencies]` with `sync`, `docs`, `fmt`/`lint` and `test`, and the
one-version toolchain store.

### MISSING-Mta7v kelp: user-declarable `extern fun` — syntax, linkage, variadics, function pointers

repo: kelp | found: 2026-09-20 | effort: L

source: `kelp/plan/roadmap.md`, backlog item B-1, MK9-S1-M1.

**+++++ HALF FINISHED +++++**

The capability question is implemented: `coral.toml`'s `[extern."<module
id>"]` section assigns a user module a capability domain, native library and
exact symbol allowlist. An `extern fun` is accepted only when its name is
allowlisted and is gated through the same `gated`/`check_capability` path as a
stdlib extern.

Shipped since this entry was opened: C variadics, `header`, `include_dirs`,
`library_dirs` and `pkg_config` manifest settings, `kelp_name=c_symbol` symbol
renames, and typed plain `fun(...)` function pointers in user extern parameters
and returns. The `ffi_fn_pointer` regression passes a Kelp callback to C and
receives one back on both backends.

Still open:

- No `extern "C"` block syntax: a user module's `extern fun` declarations
  are gated one at a time by manifest domain assignment, not grouped under a
  linkage-block keyword the way the roadmap's prose imagined.
- No import/export symbol controls or platform-specific linkage attributes.
- By-value C layout for a value that crosses the boundary remains tracked with
  the layout work in roadmap B-2.

TLS (MISSING-XkcSp) still needs a real design pass of its own if it turns out
to want any of the above; it does not depend on this entry.

`BUG-kQ7Rv`, the seam a user extern inherits, is fixed: an `extern fun`
signature could not name a Kelp `enum` or `datac` on the default backend, and
the prelude now emits those typedefs from the C backend's own layout writers.

### MISSING-Pw3Fz kelp: a method cannot declare type parameters of its own

repo: kelp | found: 2026-10-06 | effort: L

source: wiki `language/limits.md` (unreleased), "Data and generics": a receiver
on a generic type must write the type's own parameters in order, and a method
cannot add more. A generic `enum`'s template also carries no method, so every
`Option`/`Result` combinator is a free function.

So `opt.map(f)` cannot be written as a method; it stays `option.map(opt, f)`.
Repro shape: `fun Pair<A, B>.map_first<C>(f: fun(A).R: C).Pair<C, B>`.

### MISSING-Ey4Mt kelp: const generics stop at plain integer and `bool` parameters

repo: kelp | found: 2026-10-06 | effort: L

source: wiki `language/limits.md` (unreleased), "Const generics".

Refused: arithmetic on the parameter inside a type (`[T; N + 1]`), a const
parameter on a trait, a non-integer non-`bool` const parameter. Related
compile-time limits: no `comptime` block, no compile-time `List`/`Map`, no
`size_of`/`align_of` in a `const`, and no method or builtin call in a constant
(`str.len("x")`).

### MISSING-Sw9Hd kelp: scoped blocks cannot capture disjoint fields

repo: kelp | found: 2026-10-06 | effort: L

source: wiki `language/limits.md` (unreleased), "Scoped blocks".

Borrow roots are whole variables, so a block writing `s.count` while the callee
borrows `s.items` is refused. Also open: a block inside a block cannot capture
the outer block's exclusive capture, and `kelpc fmt` cannot re-wrap a trailing
block (open item 75). The soundness half is closed: `g(s, s)` with a `mut` and a
`&` parameter is refused in either order, as two `mut` parameters already were.

### MISSING-Xo5Ga kelp: no Unicode case mapping, and no `str` slicing that is codepoint aware

repo: kelp | found: 2026-10-06 | effort: L

source: wiki `language/limits.md` (unreleased), "Text": `to_upper`/`to_lower` are
ASCII-only; most string functions count bytes, with the `char_*` helpers as a
separate set. A `std/unicode` table module (case, width, grapheme boundaries,
normalization) has no entry in the plan.

### MISSING-Zq1Pe kelp: `std/ws` is client-only, and `std/http` is HTTP/1.1 only

repo: kelp | found: 2026-10-06 | effort: L

source: wiki `language/limits.md` (unreleased), "Networking".

Absent: a WebSocket server, `Sec-WebSocket-Extensions` (so `permessage-deflate`
and the `zlib-stream` transport a compressed gateway asks for),
`Sec-WebSocket-Protocol`, redirects, automatic reconnect, a timeout so `close`
can wait for the peer's `Close`, and an IPv6 literal in `ws.parse_url` (and in
`net` capability patterns, which split the port at the first `:`). HTTP/2 is
absent: ALPN names `h2` but the server speaks HTTP/1.1. Not on the roadmap.

### MISSING-Jy7Df kelp: `Drop` does not run on the panic path (no unwinding)

repo: kelp | found: 2026-10-06 | effort: L

source: wiki `language/limits.md` (unreleased), "Concurrency": a user `Drop`
exists but "does not run on the panic path: there is still no unwinding"; a
panicking worker's frame is abandoned. `plan/panic-semantics.md`.

A panic leaks every live owner in the frames above it, including a held `Mutex`
guard and an open file in a worker. Needs a decision (unwind, or run drops
without unwinding, or document the leak as permanent) before it is a task.

### MISSING-n6Bsc kelp: `Result<unit, E>` has no constructible success value

repo: kelp | found: 2026-09-25 | effort: L

source: `kelp/plan/syntax.md` 6.4 and 7: `unit` is the absent value's type, and
no expression writes that value.

Repro: `fun f().R: result.Result<unit, String> { return result.Result.Ok(value = ()) }`
is refused. `()`, `Result<(), String>` and `Result<unit, String>` type-check (the
type is accepted), but writing the success payload is not:

    error: '()' is not a value: a tuple has two or more elements, and the absent
    value is 'unit', which no expression writes

So the natural port of Rust code that returns `Result<(), E>` has no spelling.
Every `coral` seam returns `Result<_, String>`, and the port answers with
`Result<i32, String>` and `ok0()` (0 for success) instead
(`coral/kelp/src/base.kp`).

Re-tiered S to L on 2026-10-06: a `Result<unit, E>` payload needs a `unit` value
stored in an enum payload, which `plan/syntax.md` section 3 refuses on purpose
(`unit` "cannot be an element", the same rule as a `datac` field). Writing `()`
is therefore a design call (a zero-size payload, or a sugar that makes
`Result<unit, E>` carry no payload) plus codegen in both backends, not one small
diagnostic.

### MISSING-Ve8Nc kelp: generic trait impls, `where` clauses and blanket impls

repo: kelp | found: 2026-10-06 | effort: XL

source: wiki `language/limits.md` (unreleased), "Not present at all", Traits;
roadmap A-12.

Still absent: `impl<T: A> B for Pair<T>`, blanket impls, `where` clauses, a
bound on an associated type (`I: Iterator<Item = i64>`), default or generic
associated types, associated consts, generic trait methods, `impl Trait` in
return position, an upcast between trait objects, and `dyn` of a generic trait or
of a trait with an associated type. Generic impls are the one that blocks the
iterator adapters (MISSING-Hd4Lr) and `impl Iterator for Ring<3>` (the
"iterators with const generics" row of the feature-interaction table).

### MISSING-Ra5Wn kelp: no async, no coroutines, no weaker atomic orderings

repo: kelp | found: 2026-10-06 | effort: XL

source: wiki `language/limits.md` (unreleased), "Concurrency": "no async and no
coroutines, with no roadmap item tracking either yet"; only `SeqCst` is shipped,
`Relaxed`/`Acquire`/`Release` are reserved.

Two separate gaps. Async/coroutines: nothing in `plan/` names a design, so a
server is thread-per-connection (`serve_threaded`, `std/pool` creates one thread
per job). Weaker orderings: reserved syntax, no semantics; they need the memory
model in `plan/mk9-memory-model.md` to say what they promise before they ship.
`AtomicPtr` is absent for the same reason.

### MISSING-Bm8Wk kelp: no named nested functions, no lifetime syntax, no borrows in containers

repo: kelp | found: 2026-10-06 | effort: XL

source: wiki `language/limits.md` (unreleased), "Not present at all".

Three open design questions, grouped because none has a decision: (1) a named
nested `fun` that can recurse or be used before its definition (closures cover
most uses); (2) explicit lifetime syntax `'a`, enum-payload and container
borrows, raw reference fields and closures that capture a borrow, deliberate
non-goals "until a separate owner-join design exists" (roadmap A-11); (3)
provenance and an address-space model, so a granted MMIO range still admits any
address reachable by arithmetic from it; `transmute` and reading a non-`Copy`
element are absent too.

### MISSING-Iq2Tz kelp: a library in the middle cannot narrow a capability, and no revocation or attenuation at runtime

repo: kelp | found: 2026-10-06 | effort: XL

source: roadmap C-1 (MK10-S2) and C-4 (MK10-S3); extends MISSING-t9Qmz, which
records the decision to wait. This is the part of the headline feature that is
still static: grants are source text, a plugin or worker cannot be handed a
narrower grant at runtime, and a grant cannot be revoked or bounded in lifetime
or holder count. Listed so the capability story is not read as complete.

### MISSING-t9Qmz kelp: a module in the middle cannot forward a narrowed grant

repo: kelp | found: 2026-09-24 | effort: XL

source: repro run against `kelpc` on 2026-09-24, prompted by a library that
wants to send for its caller; `kelpc/src/resolve.rs` (the delegation rule).

A grant is source text, and a delegator may grant a module less than it holds
but never more (`resolve.rs`: "a module can delegate less than it has, never
more"). So a library that performs the gated call on its caller's behalf must
write the pattern it forwards, and a wildcard is the only pattern that fits
every caller:

```kelp
// src/lib.kp
impl "std/ws" caps(net = ["*"], rand = [], time = [])
```

and then the caller cannot narrow it. The repro, in a project whose entry is
`src/main.kp`:

```kelp
// src/main.kp, narrow: refused
impl "lib" caps(net = ["127.0.0.1:80"], rand = [], time = [])
```

```
error: module 'lib' cannot grant 'net' access to '*': it does not hold that
pattern itself (held: 127.0.0.1:80); a module can delegate less than it has,
never more
```

and with the grant left off entirely:

```
error: module 'lib' cannot grant 'net' access to '*': it does not hold that
pattern itself (held: none); 'lib' holds nothing here because the module that
imports it wrote no 'caps(net = [...])' grant on that `impl` line; a module can
delegate less than it has, never more
```

So a program that delegates its networking to a library cannot say "this
library may reach discord.com and nothing else": it says `net = ["*"]` or it
writes the loop itself. The same shape hits any middleware module (an HTTP
client wrapper, a database driver, a telemetry sink), so this is a property of
the model rather than of one library, and the entry is here rather than in a
library's docs because a fix is a language or manifest change:

- a **capability value** a caller passes (the runtime-derived-pattern work MK10
  names), which is the only route that keeps one compiled program's patterns
  from being fixed at the library's source;
- a `[caps]` section in `coral.toml` that narrows `main` itself (open item 18),
  which does not help a *library* in the middle but does at least let the
  program's own line be checked;
- or an admitted limitation, stated in the plan: a library that touches a
  resource is granted the resource, not a slice of it.

Nothing here is scheduled, and the workarounds ship today: keep the I/O in the
caller (the `httpx` pattern, where a library builds a request and the caller
sends it), or accept the wildcard. The owner chose the wildcard side on
2026-09-24 and asked for a one-token spelling of it, so `impl "x" caps(*)`
(shipped 2026-09-24) grants everything the delegator holds; that makes the broad
case ergonomic but does not restore narrowing through a library, which is what
this entry records.

**Decided, 2026-09-26: accept the limitation for now, do not build a stopgap.**
The real fix is the first option above (a runtime capability value, MK10-S2 /
C-1), which is mark-sized on its own; a partial narrowing mechanism built ahead
of it would just be thrown away once C-1 lands properly. Until then: a library
that touches a resource is granted the resource, not a slice of it, and the
`httpx` keep-the-I/O-in-the-caller pattern is the documented way around it.
Still open, still tracked here; no new code planned before C-1.

### MISSING-Dl7Ps kelp: verification duty: no semantics, no UB catalogue, no Miri-equivalent

repo: kelp | found: 2026-10-06 | effort: XL

source: roadmap block D (D-1 to D-3, MK11): no formal operational semantics or
aliasing model for `&mut`, no catalogue of defined behaviour (overflow is
`-fwrapv` from the toolchain, not a language rule), and no dynamic checker for the
move/borrow rules, so soundness is a reading of `infer.rs`/`borrowck.rs` rather
than a test result. Matters more now that `unsafe` and raw pointers have shipped.
