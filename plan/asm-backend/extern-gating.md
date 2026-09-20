# MK7 extern gating: the raw primitive, the gate table, and the three naming sites

> **ARCHIVED 2026-09-20. Superseded, not deleted.** Written against the `asm` backend's naming
> sites. The bug it records (`ISSUES.md`, `BUG-wQ9tL`) is a `codegen_asm.rs` defect and goes when
> that file goes, not before.

This is the specification lane for `KelpLANG/ISSUES.md` entry `BUG-wQ9tL`, "a `*_raw` extern
bypasses the capability check". It is written at `0053bc2` ("feat: the asm backend lowers the
std.fs path family (MK7 §5 item 5, slice S4)"), it changes no source, and it is the input to a fix
lane that follows it. It sits beside `plan/asm-backend/capability-check.md` (MK7 §5 item 7, the check the
asm backend must emit): both are security rows of the same model, and neither is on the
`bench/speed` gate path.

Every build, every run, every diagnostic and every count below was measured with the release
binary rebuilt at that commit (`cargo build --release`, `target/release/kelpc`, `kelpc 26.09.0`).
Probe projects were written under `/tmp` and never inside the checkout. Where a claim is a read of
the source rather than a measurement, the sentence says so.

```
# a probe project: /tmp/DIR/src/main.kp plus /tmp/DIR/src/util.kp
kelpc check   src/main.kp
kelpc build   src/main.kp --backend=c   -o OUT     # the oracle, and the C it keeps next to OUT
kelpc build   src/main.kp --backend=asm -o OUT     # the second backend, and OUT.s
./OUT
# the corpus, by hand and by sweep
kelpc build kelpc/tests/fixtures/NAME.kp --backend=asm -o /tmp/x
cargo test --release --test backend_coverage -- --ignored --nocapture
```

`resolve.rs` line numbers are anchors as of this read (`:405-450` is the capability block). A
citation that has drifted is found by the name in the same sentence. `stdlib.rs` and `externs.rs`
line numbers were stable across the last three plan commits.

### One thing moved while this file was written

`kelpc/src/codegen_asm.rs` is modified in this checkout by another lane implementing
`plan/asm-backend/capability-check.md` (MK7 §5 item 7 step 1, the asm backend's own capability check). The
facts below are stated for two states, and every claim says which one it is:

- **`0053bc2`, the committed HEAD**, which is what all of section 1 and the corpus counts in
  BLAST RADIUS are measured against. There the asm backend has no check to emit and refuses a
  gated call that carries a resource (`cap_check_refusal`, `codegen_asm.rs:1286`, called at
  `:11512`).
- **The working tree at the time of writing**, `codegen_asm.rs` at md5 `3887f6c3b55019fa0fb69cecfbe86fbd`
  (550 insertions, 59 deletions against HEAD), where that function is now `cap_decision`
  (`:11680` at that fingerprint) and the check **is** emitted. Measured there:
  `fs_symlink_kind` and `path_absolute` build under `--backend=asm`, `fs_path_family` builds under
  both backends and its stdout is byte-identical between them, and the issue's narrowed-grant repro
  built with `fs.read` instead of the primitive is **denied at run time** by the asm backend with
  the same `outside the fs capability granted to module 'util'` panic the C backend prints. Two
  things that lane has not finished are visible at that fingerprint and are not this lane's: the
  asm denial names the module as the call site string (`module '/tmp/.../util.kp:4:32'`, where the C
  backend names `util`), and `caps_gated_path` now refuses at a struct-returning call rather than at
  the check. Neither changes any count below.

The rule comparison reads the asm cost against that second state, because that is the state the fix
lane will start from. The measured consequence is stated in rule (b): the raw sites get the same
emitted check the wrappers get, so rule (b) costs the corpus nothing on the asm side.

## FILE

`plan/asm-backend/extern-gating.md`, this file. One new file, no source changes, nothing else touched.

## THE BYPASS

### 1.1 The repro, by hand

Two files, as the issue entry writes them. `util` is granted one directory under `/tmp`; it reads
`/etc/hostname`, which is outside that grant.

```kelp
// main.kp
impl "util" caps(fs = ["/tmp/kelp_narrow_allowed/**"])

fun main() {
    print(util.snoop_raw())
}
```

```kelp
// util.kp
impl "std/fs"

fun snoop_raw().R: usize = fs.read_raw("/etc/hostname").len()
fun snoop_checked().R: Bytes = fs.read("/etc/hostname")
```

The grant the module claimed is the grant in the first line of `main.kp`: the `fs` domain with the
one pattern `/tmp/kelp_narrow_allowed/**`, compiled into the program as constant data.

```
$ kelpc check src/main.kp                                  # exit 0, no diagnostic
$ kelpc build src/main.kp --backend=c   -o bypass_c   && ./bypass_c      # 6   (exit 0)
$ kelpc build src/main.kp --backend=asm -o bypass_asm && ./bypass_asm    # 6   (exit 0)
```

`/etc/hostname` is a six-byte name on this machine, so both backends print `6`. The narrowed grant
is never consulted.

The sibling call is what makes the denial real rather than a miss: with `main` calling
`snoop_checked` instead, the C backend refuses at run time and the asm backend refuses at build
time.

```
$ kelpc build src/main.kp --backend=c -o checked_c && ./checked_c
/tmp/kelp_bug_wq9tl_checked/src/util.kp:4:32: panic: '/nix/store/...-etc-hostname' is outside the fs capability granted to module 'util'
  granted:
    /tmp/kelp_narrow_allowed/**
$ kelpc build src/main.kp --backend=asm -o checked_asm
error: the 'asm' backend does not support the capability check that gates `std.fs.read` (domain `fs`) [...] the asm backend cannot emit an unchecked call, so it refuses instead yet
```

(the `[...]` elides one em dash that is in the message itself; the halves before and after it are
verbatim)

### 1.2 The emitted C and assembly, at the offending site

The C backend, `bypass_c.c`. `snoop_raw` is a plain call to the extern with no check in front of
it; `snoop_checked` is the statement expression `cap_checked_call` builds. The module's allowlist
array is in the file and nothing at the raw site names it.

```c
static const char* kelp_caps_util_fs[] = { "/tmp/kelp_narrow_allowed/**", NULL };   // :745

size_t kelp_util__snoop_raw(void) {                                                 // :4036
    #line 3 "/tmp/kelp_bug_wq9tl/src/util.kp"
    return ({ kelp_Bytes _xtmp21 = kelp_std__fs__read_raw("/etc/hostname");
size_t _xtmp22 = kelp_Bytes__len(&_xtmp21);
kelp_bytes_drop(&_xtmp21);
_xtmp22;
});
}

kelp_Bytes kelp_util__snoop_checked(void) {                                         // :4045
    #line 4 "/tmp/kelp_bug_wq9tl/src/util.kp"
    return ({ kelp_cap_check_path("fs", "/etc/hostname", kelp_caps_util_fs, 1, "util", "/tmp/kelp_bug_wq9tl/src/util.kp:4:32"); kelp_std__fs__read("/etc/hostname"); });
}
```

The assembly, `bypass_asm.s`, at HEAD. The raw call is a direct `call` into the backend's own
routine for that extern, and there is no check to miss: the whole file contains no `cap` symbol at
all.

```
    .globl kelp_util__snoop_raw          # :29
    .type kelp_util__snoop_raw, @function
kelp_util__snoop_raw:
    ...
    call kelp_rt$fs_read_raw             # :41
```

```
$ grep -ci cap bypass_asm.s
0
```

In the working tree (see the note on the in-flight asm check above) the checked sibling gains a
check the HEAD version did not have, and the file therefore carries the allowlist data, while the
**raw site is unchanged**: rebuilt against that tree, `bypass_asm.s` still has
`call kelp_rt$fs_read_raw` at the same offset with no check in front of it, and the program prints
`6`. The hole in section 1 is not the asm backend's check arriving or not arriving; it is the
extern never entering the gate table, so there is nothing for either backend to emit a check from.

Both backends share the defect because neither decides it: the gate table is built in `resolve.rs`
(`:405-450`), consulted in `infer.rs` (`check_capability`, `:530`, called from one place,
`:8692`), and read by both emitters through `program.gated` (`tir.rs:30`,
`codegen_c.rs:600`, `codegen_asm.rs:3437` at HEAD). The filter that makes the extern invisible is
`resolve.rs:433`:

```rust
if f.receiver.is_none() && !f.extern_ {
```

with the comment above it (`:428-432`) stating the intent: an extern is the primitive inside the
gated module, and gating it would mean the module wrapping it needed a grant to call its own body.
The comment is right about the *inside*. It is silent about the *outside*, and resolution lets a
user module name another module's extern.

### 1.3 Does the bypass need a narrowed grant? No.

Measured, with `main.kp` reduced to a bare `impl "util"` and a `util.kp` that names only the raw
extern. There is no `caps(...)` line anywhere in the program.

```kelp
// main.kp
impl "util"
fun main() { print(util.snoop_raw()) }
```

```kelp
// util.kp
impl "std/fs"
fun snoop_raw().R: usize = fs.read_raw("/etc/hostname").len()
```

```
$ kelpc check src/main.kp                                 # exit 0
$ kelpc build src/main.kp --backend=c   -o nogrant_c   && ./nogrant_c     # 6
$ kelpc build src/main.kp --backend=asm -o nogrant_asm && ./nogrant_asm   # 6
```

`util` holds no domain at all, and the call is still emitted unchecked by both backends. The
narrowed grant in the issue's repro is a red herring about reach: the hole is wider than "a
narrowed grant is advisory". It is "a grant is advisory for a module that holds any grant, and
absent for a module that holds none". The same held for the other five gated domains by
construction (`gated` is built from the same loop for `fs`, `net`, `process`, `env`, `time`,
`rand`), and `plan/asm-backend/extern-bodies.md` section 2.3 records the same design from the write side:
"the externs are not gated; the wrappers are".

Re-measured against the working tree above, where the asm backend does emit the check for gated
calls: the same no-grant probe still builds and prints `6` under both backends, and its `.s`
carries six capability symbols (the glue and the allowlists) with no check at the call site. The
rule about externs is what is missing, not the emission.

### 1.4 The same hole is wider: three more measured routes

Found while measuring the above, not by the issue entry. All three are the same class: the rule the
fix lane writes has to say where the gate is consulted, or the fix is a patch on one of three sites.

**A function value naming a gated item is never gated at all.** `infer.rs:6307` builds a `FnRef`
through `function_value` (`:1463`, `function_value_at`, `:1479`) and no capability check is reached
from there; `check_capability` has exactly one caller, the direct-call arm at `:8692`. Measured
with no grant line in the program:

```kelp
impl "util"                       // main.kp, no caps(...)
```

```kelp
impl "std/fs"                     // util.kp

fun leak().R: usize {
    val f: fun(&str).R: Bytes = fs.read_raw   // the primitive
    return f("/etc/hostname").len()
}
```

```
$ kelpc check src/main.kp                              # exit 0
$ kelpc build src/main.kp --backend=c   -o fn_c   && ./fn_c      # 6
```

The emitted C is the whole story: `kelp_Bytes (*kelp_f)(const char*) = kelp_std__fs__read_raw;`
then `(kelp_f)("/etc/hostname")`, no check anywhere. The same probe with the **wrapper** substituted
for the primitive, `val f: fun(&str).R: Bytes = fs.read`, also compiles with no grant, also prints
`6`, and also builds under the asm backend and runs unchecked. So the *domain gate itself*, not
only the extern filter, is skipped for a named function used as a value: a gated item is reachable,
uncalled, by any module.

The asm backend adds one more outcome for the raw case: `val f = fs.read_raw` emits a reference to
the C backend's symbol name `kelp_std__fs__read_raw` (`fn_asm.s:36`), which this backend never
defines (it defines `kelp_rt$fs_read_raw`), so the program dies in `ld` with
`undefined reference to 'kelp_std__fs__read_raw'`. A link error is not the "refused by name" a
construct the backend cannot lower owes the reader. The wrapper as a value builds and runs, which is
what the probe above measured. Both halves are unchanged in the working tree, which is the sharper
statement of the finding: there the asm backend emits the check for gated calls, and `val f =
fs.read` still builds and still prints `6` with no check, because the check is emitted where a
callee is written and a function value is not a call site.

**A receiver method of a gated module is not in the gate table either**, because of the same
`f.receiver.is_none()` half of the filter (`resolve.rs:433`). Measured with no grant line at all:

```kelp
// util.kp
impl "std/io"
impl "std/process"

fun probe().R: bool {
    var c = process.Child(pid = 1, stdin = io.File(fd = 0), stdout = io.File(fd = 1),
                          stderr = io.File(fd = 2), exited = false, code = 0)
    c.kill(signal = 0)
    return true
}
```

```
$ kelpc check src/main.kp                              # exit 0
$ kelpc build src/main.kp --backend=c -o meth_c && ./meth_c
panic: process.kill: kill failed
  errno 1: Operation not permitted
```

The syscall ran: `kill(1, 0)` returned `EPERM`, which is what the panic reports. With
`signal = 9` instead of `0` that is a `SIGKILL` a module holding no `process` grant was never
allowed to send, to any pid it can name. `net.Listener(fd = 3).port()` and
`net.Socket(fd = 3).peer_addr()` compile with no `net` grant the same way (measured: `check` exits
0), and both reach `local_port_raw`/`peer_addr_raw` on whatever descriptor the module wrote, which
the public `datac` constructor makes forgeable. `std/net` methods that this covers:
`Listener.close`, `Listener.port`, `Socket.wait_readable`, `Socket.wait_writable`,
`Socket.set_timeout`, `Socket.peer_addr`; `std/process`: `Child.wait`, `Child.is_running`,
`Child.kill`.

**And on the runtime half, a `pd` initializer's gated call is not checked.** The compile-time gate
does fire (a `pd` initializer is checked with its module's `caps` and the same `gated` table), but
the C emitter builds `kelp_init_pds` with an empty table: `codegen_c.rs:967`, `no_caps`, whose
comment says "A `pd` initializer is not a function, so it can hold no gated call either". That is
false as of this commit. Measured with the grant held and the path outside it:

```kelp
// util.kp
impl "std/fs"
pd val leak: bool = fs.exists("/etc/hostname")
fun show().R: bool = leak
```

```
$ kelpc check src/main.kp                              # exit 0
$ kelpc build src/main.kp --backend=c -o pd_c && ./pd_c
true
$ grep -n 'pd util.leak' -A 1 pd_c.c
1574:    kelp_pd__util__leak = kelp_std__fs__exists("/etc/hostname"); // pd util.leak
```

No `kelp_cap_check_path`, so the pattern check is skipped and the file outside the grant is
reached. `true` is printed, and at HEAD the asm backend refuses the same program by name
(`the 'asm' backend does not support the capability check that gates \`std.fs.exists\``), so at
HEAD this one is C-only. In the working tree the asm backend builds and runs it, and its emitted
check uses the entry module's allowlist rather than `util`'s, which is trap (c) below. A closure
body is the control that shows the emitter *can* carry the table: a
`fun(path: &str).R: Bytes = fs.read(path)` literal inside a narrow-grant module emits its check and
denies at run time (measured).

### 1.5 The measured matrix

`direct` is the name written at a call site, `value` is the name used as a function value, `no
grant` is a program with no `caps(...)` line anywhere, `narrow` is the issue's repro grant.

| route | grant | frontend verdict | C backend, run time | asm backend |
| --- | --- | --- | --- | --- |
| `fs.read_raw` direct | none | compiles | reads the file, no check | builds, reads the file, no check |
| `fs.read_raw` direct | narrow | compiles | reads the file, no check | builds, reads the file, no check |
| `fs.read` direct | none | **compile error**, names the missing domain | (not reached) | (not reached) |
| `fs.read` direct | narrow | compiles | **denied** at run time, exit 1 | **refused** by name at HEAD, **denied** at run time in the working tree |
| `fs.read_raw` value | none | compiles | reads the file, no check | `ld` error, undefined symbol |
| `fs.read` value | none | compiles | reads the file, no check | builds, reads the file, no check |
| `process.Child.kill` (method) | none | compiles | `kill` syscall runs | (not measured) |
| `fs.exists` in a `pd` initializer | narrow | compiles | reads the file, no check | at HEAD refused by name; in the working tree builds, and its check uses the entry module's grant |
| `fs.read` in a closure body | narrow | compiles | **denied** at run time | (not measured) |

Two rows are the sanctioned model working exactly as documented (both the direct `fs.read` call and
the closure body it sits in); the other seven are the grant being advisory.

## BLAST RADIUS

Measured, not guessed. Every `.kp` file in the four named places was read by a script that collects
the `extern fun` names of each `kelpc/std/*.kp` module, maps each file's `impl "..."` lines to
aliases, and reports every `<alias>.<name>_raw(` whose name is an extern of the target module; a
second pass reports every `_raw` reference that is *not* a call (a function value), and a third
reports bare `name_raw(` uses inside the stdlib itself. The counts below are that script's output,
confirmed by hand with `grep -rn '_raw'`.

### Inside `kelpc/std/**`: all same-module, no cross-module case

Twelve shipped modules declare `extern fun`: 97 declarations. Six are gated modules and declare 59
of them (`std/fs` 27, `std/net` 13, `std/process` 10, `std/env` 4, `std/time` 3, `std/rand` 2); six
are ungated and declare 38 (`std/math` 16, `std/io` 11, `std/str` 7, `std/fmt` 2, `std/mem` 1,
`std/path` 1). `std/rt`, `std/http`, `std/list`, `std/option`, `std/result` and `std/encoding`
declare none. The 97 is the 71 of `plan/asm-backend/extern-bodies.md` section 0 plus `std/math`'s 16,
`std/str`'s 7, `std/fmt`'s 2 and `std/mem`'s 1, which that file scoped out as already lowerable.

Every extern call site in the stdlib is **same-module**: 113 sites, each one a wrapper body calling
the primitive its own module declares (`std/fs` 34, `std/io` 14, `std/net` 14, `std/math` 16,
`std/process` 13, `std/str` 8, `std/env` 4, `std/rand` 3, `std/fmt` 2, `std/mem` 1, `std/path` 1,
`std/time` 3). **Zero cross-module sites.** These are the same-module wrappers the stdlib is built
from. `std/http` reaches `std/net`'s *wrappers* (`net.connect` at `:818`, `net.recv`, `net.close`,
`net.send_all`), which is the sanctioned route and already gated.

Five extern names exist in two gated modules at once (`read_raw` in `fs` and `io`, `write_text_raw`
in `fs` and `io`, `close_raw` in `io` and `net`, `exists_raw` in `fs` and `process`, `is_dir_raw`
in `fs` and `process`). Each is called bare inside its declaring module, and a bare name inside a
module resolves to its own declaration, so none of them is cross-module evidence. `io` and `net`
are ungated, which is the only reason the collision is currently harmless.

### Fixtures: two files, 37 sites, 16 distinct externs, all cross-module

Both are the compiler's own pins for the primitives, and both call them from the **entry** module.

- `kelpc/tests/fixtures/fs_path_family.kp`: 34 call sites naming 13 distinct `std/fs` externs:
  `is_symlink_raw`, `remove_raw`, `exists_raw`, `mkdir_raw`, `is_dir_raw`, `is_file_raw`,
  `write_text_raw`, `read_raw`, `mkdir_all_raw`, `write_raw`, `symlink_raw`, `read_link_raw`,
  `list_raw`. Its own comment says why it exists: "The gated wrappers (`fs.read`, `fs.write_text`,
  ...) are refused in the `asm` backend until the capability check is emitted at the call site, so
  this fixture pins the syscall bodies themselves". It is in `ASM_BUILT_BASELINE` and in the C
  fixture test `the_fs_path_family_reads_writes_and_walks`
  (`compile_and_run.rs:3204`), which asserts its stdout byte for byte.
- `kelpc/tests/fixtures/io_write_bodies.kp`: 3 call sites naming 3 distinct `std/io` externs:
  `write_bytes_raw`, `write_text_raw`, `write_str_raw`. It pins the borrow ABI of a by-value
  `Bytes`/`String` parameter in those bodies. `std/io` holds no domain, so it is cross-module but
  not gated-module: no rule here can make these three calls a capability question.

`caps_gated_path.kp` contains the string `_raw` in a comment only. No other fixture names a foreign
extern.

### `tests/projects/**`, `bench/`, `examples/`: none

```
$ grep -rn '_raw' kelpc/tests/projects bench examples      # no match
```

48 project directories, 21 of them `caps_*`, and every one of them exercises the gate through the
*wrappers* (`caps_granted` and `caps_write` run inside a grant, `caps_denied`, `caps_symlink`,
`caps_link_denied`, `caps_net_denied`, `caps_net_resolve_denied`, `caps_env_denied` are denied at
run time, `caps_ungranted`, `caps_escalation`, `caps_delegate_from_nothing` are compile errors).
`bench/speed/speed.kp` imports `std/time` and calls `time.now_mono_ns()`; it names no extern and no
`_raw` string at all. A function value naming a foreign extern appears nowhere: the only non-call
`_raw` reference in the whole corpus is a comment in `std/mem.kp:4`.

### The answer, directly

The stdlib is built from same-module wrappers over its own primitives, and it has no cross-module
extern use at all, so **no rule here changes what the shipped stdlib must be able to do**. The
cross-module cases are 37 call sites in 2 fixtures, 34 of them naming a gated module's extern, 3
naming an ungated module's, and every one of those 37 is the *entry* module, the one module that
holds the wildcard for every domain and needs no grant. `fpl` was grepped as well and has no
`<alias>.<name>_raw(` to a std module (`json`'s `parse_string_raw` and `url`'s `decode_or_raw` are
their own local functions), but `fpl` is a different repository and that grep is the whole of what
this lane measured about it.

## THE RULES COMPARED

The three candidates are the issue's own two plus documenting it. Each is stated with its exact
effect, whether it can be a compile error or has to be a runtime check, which of the two backends
change, and what it does to `bench/speed` and to the stdlib build. The filter at
`resolve.rs:433` is `f.receiver.is_none() && !f.extern_`, so "externs" and "receiver methods" are
two different questions and a rule that says only "externs" leaves the `Child.kill` row of section
1.4 standing. Each rule below says which of the two it covers.

### Rule (a): refuse a module naming another module's `*_raw` extern

**Exact effect.** The name does not resolve outside its declaring module. A call, a function value
and (if written as such) any other use are one and the same failure, because the verdict is at the
name.

**Compile error, always.** There is no runtime half: the module has no name to reach the resource
with, which is the mechanism `plan/architecture.md:129` describes ("Without a grant, the
corresponding built-in namespace simply isn't a resolvable name inside that module ... not by a
runtime permission check that could be forgotten or bypassed").

**Sub-decisions the rule does not settle, each measurable.** (i) Which externs: every module's, or
only the six gated modules'? Gated-only keeps `io_write_bodies` working and breaks only
`fs_path_family`. (ii) The entry module: the entry module is the only module that can name the
primitives today without being the primitive's owner, and it is also the module that holds `/**`
for every domain. An identity exemption ("the entry module may name any extern") keeps both
fixtures and closes every non-entry case. A pattern exemption ("a module holding the domain's whole
wildcard may name it") is the same rule without the identity hack, but it puts a grant pattern in
charge of *name resolution*, which no other part of the model does. (iii) Whether the rule reads
as "the primitive is internal" or as "*_raw is internal": the suffix is a convention, not a
mechanism. All 97 externs end in `_raw` today, and nothing enforces it: no test, no lint, no
`stdlib.rs` check. A rule keyed on the suffix would fail open for the first extern named without it.

**Backends.** Both, identically: the refusal is in the front end, so no emitter changes. The cost
lands in the corpus instead.

**`bench/speed`: unchanged.** It names no extern.

**The stdlib build: unchanged.** No cross-module extern use exists to refuse.

**Cost, measured against the corpus.** `fs_path_family.kp` stops building under both backends (29
sites), and `io_write_bodies.kp` stops under the "every module's externs" reading (3 sites). A user
module cannot declare its own `extern fun` reactivation either: measured,
`extern function 'not_a_real_primitive' has no compiler-supplied body`, continuing
`only compiler-shipped stdlib modules may declare 'extern fun'` (fixture `extern_no_body.kp`). So the primitives become
reachable by nothing in the corpus except the stdlib, and the fixture has to be rewritten onto the
wrappers. Two obstacles:
- A rewrite changes what the fixture pins, and for `fs_path_family` it changes the *stdout*:
  `print(fs.list_raw(root))` prints one newline-separated buffer while `fs.list` prints a
  `List<String>`; `fs.mkdir_all` and `fs.mkdir_all_raw` are different functions; `fs.read` returns
  the same `Bytes`, so that one line survives. Editing an expected stdout is exactly what the lane
  rules forbid.
- A rewrite onto the wrappers makes the fixture a check-carrying one, which at HEAD the asm backend
  refused outright and in the working tree it builds (measured: `fs_symlink_kind` and
  `path_absolute` build under `--backend=asm` there). So rule (a) still removes the fixture, not
  because the backend cannot build its replacement but because the replacement prints something
  else. The `ASM_BUILT_BASELINE` entry goes with it either way.

**Cost in code.** Small: one refusal in `resolve.rs` (or a new arm in `check_capability`), plus a
new diagnostic whose exact text the fix lane has to pin. Its real price is coverage and the new
concept ("a primitive is internal") that the plan does not currently have.

### Rule (b): put externs in the gate table too, and (as the rule, not the patch) every other item a gated module declares

**Exact effect.** The gate table stops excluding a gated module's externs, so `std.fs.read_raw`
becomes a gated key like `std.fs.read`. Two consequences, in the two places the check already
lives:
- the compile-time half (`infer.rs::check_capability`) now refuses any module that does not hold
  the domain, which turns section 1.3's no-grant bypass into the same compile error
  `caps_ungranted` already asserts, and the module's own body stays exempt through the existing
  prefix exemption (`infer.rs:544`);
- the runtime half (`codegen_c.rs::cap_plan`, `:3413`) now emits the pattern check at a direct
  call to a primitive, against the calling module's compiled-in allowlist.

**Both a compile error and a runtime check**, and that is the point: the domain decides whether the
name resolves (compile error), and the grant's patterns are enforced where the resource argument is
visible (runtime check). Rule (b) is the same mechanism the model already documents for wrappers,
applied to the items the filter skipped, so it adds no concept.

**The position tables are part of the rule, not an optional extra.** `checked_args`
(`stdlib.rs:200`) is keyed on the *bare name as the author wrote it*, and the `_raw` names are not
in it. Three of the six per-domain helpers happen to do the right thing by accident and three do
not (source read, `stdlib.rs:230-281`):
- `fs`: the default branch checks *every* `str` argument as a path, which is right for
  `read_raw`, `write_raw`, `remove_raw`, `rename_raw`, `copy_raw`, `symlink_raw`, `list_raw`,
  `read_link_raw` and the rest, and wrong for `open_raw`/`try_open_raw`, whose second argument is a
  mode. A check emitted for `"r"` is a *false denial*: measured, a relative path denies with
  `panic: 'w' is outside the fs capability granted to module 'util'` because `path_normalize`
  refuses anything without a leading `/`. So `open_raw` and `try_open_raw` need the same
  `first_str_arg` entry `open` and `try_open` already have.
- `process`: the default is argument 0, which is right for `exists_raw`, `executable_raw` and
  `is_dir_raw`, and **wrong for `spawn_raw`**, whose argument 0 is the working directory and whose
  program is argument 1. A check on argument 0 matches the directory against a program allowlist:
  a false denial for a well-formed call, and the program itself unchecked.
- `net`: `net_checked_args` is an explicit list, so `connect_raw`, `listen_raw` and `resolve_raw`
  get **no check at all**. That leaves the same class of hole one level down: a module holding
  `caps(net = ["api.example.com:443"])` could call `net.connect_raw("evil.example", 443)`, pass the
  domain gate, and reach an unchecked host. Six entries total (`open_raw`, `try_open_raw`,
  `spawn_raw`, `connect_raw`, `listen_raw`, `resolve_raw`).
- `env` (first `str` argument, which is the variable name) and `time`/`rand` (no named resource) are
  already correct for their raw names.

**If the rule is "every item a gated module declares", it also covers the receiver methods** of
section 1.4 (`Child.kill`, `Listener.port`, `Socket.peer_addr`, and the rest). Nothing in
`checked_args` applies to them: their domain gate is the whole check, exactly as it is for
`time.now_unix` or `process.args`. Their insertion is the same one-line filter change
(`f.receiver.is_none()`). Leaving them out is a defensible narrower rule, but then the fix has to
say so, because "a grant is advisory" remains true for `Child.kill`.

**Backends.** The C backend needs no emitter change: `cap_plan` and `cap_checked_call` already read
`program.gated` and the same position tables, which is why rule (b) is the cheap half of the
security fix. The asm backend needs no code change either. At HEAD its *refusal set* would have
grown: `cap_check_refusal` (`codegen_asm.rs:1286` at HEAD, called at `:11512`) reads the same gated
table, so a primitive with a resource argument became a refusal, and source-read that set was
exactly `fs_path_family`'s 34 `fs` sites (`io_write_bodies` does not move, because `std/io` has no
domain and its externs never enter the table). In the working tree, where the check itself is being
emitted (see the note above), that cost is gone: the raw sites go through the same `cap_decision`
call the wrappers do, which is measured to emit a check and to deny at run time, so
`fs_path_family` keeps building under `--backend=asm` with its stdout unchanged. Either way the
fixture stays, and the honest form if the baseline does move is a note saying why rather than a
silent entry removal: the ratchet "only fails on the opposite direction"
(`backend_coverage.rs:60-70`), and removing an entry is the one edit that can hide a regression.

**`bench/speed`: byte-identical, measured for the current build and read for the change.** Today
`speed.kp`'s generated C contains no `kelp_cap_check_*` call site at all (the glue and the entry
module's six allowlists are emitted, nothing calls them), because its only gated call is
`time.now_mono_ns()`, which carries no named resource. Under rule (b) the only new call sites a
program can gain are at direct calls to a gated module's externs, and the one in `speed.kp`'s
closure is `std/time`'s own body calling `now_unix_raw`, which emits no check because
`patterns_for("std/time", "time")` is `None` (nothing granted `std/time` anything). So the bench's
generated C, its assembly and its numbers do not move.

**The stdlib build: unchanged**, after the six table entries. The stdlib has no cross-module extern
use to gate, and its own primitive calls stay exempt by prefix. One consequence worth naming:
under rule (b) the stdlib's own calls *would* be checked against whatever its importer granted it,
for any gated module its importer granted a domain to. `std/http` is the corpus case
(`impl "std/http" caps(net = ["127.0.0.1:*"])`), and its `net.connect` calls are already checked
today, so nothing changes; a future stdlib module granted a domain by a project would double-check
its own primitives once at the primitive and once at the wrapper. Correct, and one extra call per
wrapper call, on the stdlib's own frames only.

**Cost.** `resolve.rs`: the filter and its comment (6 lines). `stdlib.rs`: six position entries plus
comments (about 20 lines). `codegen_c.rs`, `codegen_asm.rs`: none. New tests: the work in the
acceptance list, which is where a security row's cost belongs.

### Rule (c): leave it, and document it as a language rule

**Exact effect.** No code. The rule written down would have to be: "a gated module's primitive
(`*_raw`) and its receiver methods are not gated; a module that imports a gated module can name
them, and no grant is consulted". A function value naming any gated item would have to be stated in
it too, since that reaches the *wrappers* with no grant.

**Neither a compile error nor a runtime check**: unenforced, and the documentation is the
enforcement.

**Backends: unchanged. `bench/speed`: unchanged. The stdlib build: unchanged.**

**Cost.** It contradicts the two authorities the model is built on. `plan/architecture.md:129`
says the mechanism is name resolution, "not by a runtime permission check that could be forgotten
or bypassed". `wiki/language/capabilities.md` says "the *only* way to reach the outside is through
a stdlib module that holds it", "Referencing a gated function without holding the domain is a
**compile error**", and
"A grant is checked statically *and* enforced at runtime against the pattern list, so neither a
compiler bug nor a library trick widens authority". Each of those becomes false, and the fix for a
documented hole is then a wiki rewrite plus an `ISSUES.md` entry that stays open, because the hole
is still a hole. The library consequence is the concrete one: any `fpl` library, or any third
party library, that imports `std/fs` reads the whole filesystem with no grant, and a consumer that
granted it `/var/log/myapp/**` has no way to see that from its own source.

## RECOMMENDATION

**Rule (b), stated as one rule about items and one about naming sites, with the six position
entries, and with the `FnRef` and `pd` rows either closed in the same lane or excluded in writing.**

The reasoning:

1. **It is the model the plan already has, not a new one.** The plan's own text says a gated item
   needs the domain to be *named* and the patterns to be *called*, and both halves live in code
   already: `check_capability` for the domain, `cap_plan`/`cap_checked_call` for the pattern. The
   defect is the `!f.extern_` filter and the one-caller `check_capability`, not the design. Rule
   (b) deletes a filter instead of adding a concept, which is what makes it reviewable as a
   security change.
2. **It is a rule, not a patch, because it is stated over items, not over `*_raw` names.** The
   filter has two halves and both are defects (section 1.4: `!f.extern_` is the issue's bug,
   `f.receiver.is_none()` is `Child.kill`). The rule reads: every item a gated module declares is
   gated; every module's own items are exempt; the domain decides whether a name resolves, and the
   patterns are checked wherever a resource argument is visible. A rule keyed on the `_raw` suffix
   would fail open on the first extern that does not end in it.
3. **It keeps the corpus. This is the decisive difference from rule (a).** Under rule (b),
   `fs_path_family` keeps building under C, its stdout stays byte for byte what
   `the_fs_path_family_reads_writes_and_walks` asserts (the entry module holds `/**`, so every new
   check passes), and the C-side corpus loses nothing at all. The asm side loses something only if
   the fix lands against HEAD rather than against the tree where the check is being emitted: there
   the raw route becomes a refusal, which is the honest direction (refused by name, not emitted
   unchecked), and in the working tree the check is emitted at the raw site like any other gated
   call, so the fixture keeps its asm entry and its identical stdout. Under rule (a) the same
   fixture has to be rewritten onto the wrappers, which changes its stdout (not allowed) or loses
   its coverage anyway.
4. **It closes the hole for a module with no grant at all**, which is the larger half of the
   finding: the compile-time domain gate is what stops `impl "util"` from reaching the primitive.
5. **Its cost is a table, and the table is the security-critical part.** The six entries are not
   bookkeeping: `spawn_raw` checked on argument 0 would match the working directory against a
   program allowlist and leave the program unchecked, and `open_raw` would deny every `"r"`/`"w"`
   mode as a relative path. Both are worse than the hole for a program that uses them, so the
   acceptance list below pins them.

Rule (a) is the second choice and would be the right one if the decision were "the primitives are
internal to the stdlib", which is a defensible language position; it is not the cheapest one, it
needs the entry-module (or holder-of-the-wildcard) exemption spelled out, and it costs the corpus
two fixtures rather than none. Rule (c) is not a candidate: the language's headline claim would
become "a grant is enforced at the call sites someone remembered to check", and the entry has to
stay open.

**What the recommendation does not cover, and the fix lane has to say which way it goes.** The
`FnRef` route reaches a gated *wrapper* with no grant, in both backends, so a fix that closes the
primitive and leaves the function value alone still ships "a grant is advisory". The `pd`
initializer emits no runtime check in the C backend, so a fix that closes the primitive and leaves
`write_pd_globals` alone leaves the pattern check skipped in one frame (and the asm side of the same
shape, mid-change in the working tree, emits one with the entry module's allowlist, which is trap
(c)). Both are small (a
`check_capability` call in `function_value_at`, and a `CapTable` threaded into `write_pd_globals`),
both are inside the same rule, and both are outside what `BUG-wQ9tL` reports, so the lane that
follows this file must either take them or write the exclusion down in the plan and in the wiki.
The third row found in section 1.4, the asm backend's `ld` error for a primitive as a value, is
also a named-refusal obligation rather than a capability question, and it is the one row that any
version of the fix has to address, because the project rule is that a construct the backend cannot
lower is refused by name.

## ACCEPTANCE LIST FOR THE FIX

The fix is held to all of the following. "Exists" means the test or fixture is in the tree at
`0053bc2` and must stay green; "write" means it does not exist and belongs to the fix.

**Tests and fixtures that exist and must stay green, unchanged in verdict and in expected output:**

- `cargo test --release` as a whole, plus the two gates by name:
  `cargo test --release --test backend_coverage -- --ignored --nocapture` and
  `cargo test --release --test compile_and_run`.
- The C-side gate, every one of them:
  `an_ungranted_module_cannot_name_the_gated_namespace` (the message shape the new extern refusal
  must reuse), `a_module_cannot_delegate_more_than_it_holds`,
  `an_ungranted_importer_is_named_as_the_line_to_change`, `a_granted_module_reads_inside_its_grant`,
  `a_path_outside_the_grant_is_denied_at_runtime`, `a_symlink_inside_the_grant_pointing_outside_is_denied`,
  `a_gated_resource_argument_is_evaluated_once_and_the_control_is_denied`, and the `caps_write`,
  `caps_cwd`, `caps_env`, `caps_env_denied`, `caps_net`, `caps_net_denied`,
  `caps_net_resolve_denied`, `caps_link`, `caps_link_denied`, `caps_fs_breadth`, `caps_list`,
  `caps_double_eval` cases behind them.
- `the_fs_path_family_reads_writes_and_walks` and the `io_write_bodies` test, with their stdout
  byte for byte. This is the positive control that a module naming a foreign primitive still works
  when it holds the whole domain, and it is the one that fails outright under rule (a).
- `kelpc/tests/cap_matcher.rs` and `kelpc/tests/path_normalize.rs` (the `std/rt` corpus). Rule (b)
  must not gate `std/rt`, which has no domain (`domain_of`, `stdlib.rs:124`) and declares no
  extern.
- The asm unit tests on `cap_check_refusal` (`a_gated_call_with_a_resource_argument_is_refused`,
  `an_ungated_call_is_not_refused`, `a_gated_domain_with_no_checked_arguments_is_not_refused`),
  which are the smallest statement of what the asm backend refuses at HEAD. The lane that is landing
  the check in the same checkout renames that function; the tests are named here by their HEAD
  spelling, and the point of the item is the three shapes, not the name.
- The `ASM_BUILT_BASELINE` ratchet, in whichever direction the measured sweep lands, with its note
  and its 207/196/11 arithmetic rewritten to the new numbers if the entry moves (at HEAD the reason
  would be `fs_path_family` refusing at `std.fs.read_raw`; in the working tree the fixture keeps
  building, so nothing moves). If the fix chooses rule (a), both fixture tests above change
  verdicts, and that is not an acceptable outcome of this lane.

**Tests that must be written (a fix lane's negative and positive controls):**

1. **Negative, the bypass is gone: a non-entry module with no grant cannot name a primitive.**
   A new project directory in `kelpc/tests/projects/` beside `caps_ungranted`, with the issue's two
   files and no `caps(...)` line, asserted with `assert_project_rejected` on the domain message
   (`needs the 'fs' capability, which module 'util' was not granted`). The name it must assert is
   the diagnostic the fix actually emits, so the fix pins its text in the same commit. Under rule
   (b) this is the compile-time half.
2. **Negative, the pattern half: a module with a narrow grant is denied at run time for a
   primitive.** The same directory with the grant added and the primitive used, asserted with
   `assert_project_denied_at_runtime` on `outside the fs capability granted to module 'util'`.
   This is the one that cannot be satisfied by the compile-time gate, so it is the test that
   distinguishes rule (b) from rule (a).
3. **Positive, a module's own externs and the stdlib wrappers still work, on both backends.**
   `fs_path_family` stays the wrapper-independent pin (it names the primitives, from the entry
   module, and now carries checks); `caps_granted` and `caps_write` are the wrappers' control. Add
   a positive project whose non-entry module holds the domain and calls a primitive inside its
   grant, so both halves of the rule have a positive case as well as a negative one.
4. **Positive, the position tables.** Two cases, because the entries are the part that can deny a
   correct program: `fs.open_raw(path, "r")` inside the grant passes, and `process.spawn_raw(dir,
   program, ...)` with a granted program and an ungranted directory passes (and its mirror: an
   ungranted program with a granted directory is denied). Neither exists in the corpus today, and
   both are exactly what a mechanical `checked_args` extension would break.
5. **The eval-once invariant for the new call sites.** `a_gated_resource_argument_is_evaluated_once_and_the_control_is_denied`
   pins it for `fs.read`; the primitive path goes through the same `cap_plan`, so extend that
   project (or add a sibling) rather than assuming the plumbing is shared.
6. **The `FnRef` and `pd` rows, if the lane takes them** (recommended): a project that names a
   primitive and a wrapper as function values with no grant, rejected or denied; and a project whose
   `pd` initializer reaches outside its grant, denied. If the lane does not take them, the plan and
   `wiki/language/capabilities.md` must say which routes remain exempt instead of leaving the claim
   standing.
7. **The asm side of the new call sites.** Whichever state the fix starts from, the raw sites must
   either emit the same check the wrappers emit (the working-tree behaviour, measured for
   `fs.read`: the check denies at run time under `--backend=asm`) or refuse by name; what is not
   acceptable is the state at HEAD, where `fs_path_family` builds and runs under `--backend=asm`
   with no check at all. The `ld` error in section 1.4 becomes a named refusal in the same pass: a
   link error is a test-suite failure with no message a user can act on, which is what the
   "refused by name" rule exists to prevent. If the fix starts from HEAD, the
   `ASM_BUILT_BASELINE` change is one entry for `fs_path_family` with a note naming
   `std.fs.read_raw` as the new refusal.

**Documentation the fix owes in the same session (not this lane's to write):**

- `wiki/language/capabilities.md`: the sentence in "The model" that says a gated name is a compile
  error without the domain, and the "so neither a compiler bug nor a library trick widens
  authority" sentence, both have to be true after the fix. It also says `std/rt` is "not
  importable" and it is importable on purpose, which is wrong today and should be corrected in the
  same sweep.
- `wiki/reference/capability-patterns.md` if it repeats the enforcement claim.
- `kelp/CHANGELOG.pending.md`: one line for the fix, and one for `BUG-wQ9tL`'s removal from
  `ISSUES.md`.

**Committed instruction: this lane commits only its own file.**

## TRAPS

**a. `std/rt` is importable on purpose and must stay so.** `RT_MODULE` (`stdlib.rs:20`) is injected
by the resolver for a program that imports a gated module or `std/path` (`resolve.rs:731-741`), and
it is *also* importable by ordinary code, deliberately: the differential corpora
(`kelpc/tests/cap_matcher.rs:324`, `kelpc/tests/path_normalize.rs:217`) generate `impl "std/rt"` and
call the matcher directly, which is the only way its Kelp version can be compared with its C
reference. Measured: a module with `impl "std/rt"` and no grant builds under C and prints
`rt.glob_match("/a/**", "/a/b/c")` as `true`. A rule that hid another module's internals by prefix
("only the declaring module may name these items") would be fine for `rt` (its eight functions are
pure: `glob_match`, `dir_wildcard_at`, `host_glob`, `host_glob_at`, `host_match`, `host_end`,
`has_colon`, `path_normalize`), but a rule that gated "anything the compiler injected" would break
the corpus and, worse, would make the matcher's replacement untestable, which is the reason the
exception exists. Rule (b) leaves `rt` alone for the right reason: it has no domain and no extern,
so nothing in the rule reaches it. The wiki's "compiler-internal and not importable" is wrong; the
source's comment (`stdlib.rs` and `std/rt.kp`'s header) is right.

**b. The compiler's own injected module, and its conditions.** Injection is keyed on
`domain_of(&target).is_some() || target == "std/path"`, so a program that imports a gated module
always carries `std/rt`. A rule that changed which modules are injected would change what is
defined at link time; neither candidate rule touches that. Note also that `caps` gets an entry only
for the entry module and for a grant's target, so an injected module is never in it:
`patterns_for("std/rt", ...)` is always `None`, which is what keeps the runtime half from emitting a
check inside the runtime's own functions.

**c. Passdowns are the runtime half's blind spot, measured (section 1.4).** `write_pd_globals`
(`codegen_c.rs:949`) builds `no_caps` (`:967`) with a comment claiming a `pd` initializer cannot
hold a gated call. It can, and does: the compile-time gate fires (a `pd` is checked with its
module's caps) and the runtime check does not, because the emitter has no table and no module. At
HEAD this is C-only, because the asm backend refused a gated direct call wherever it appeared. In
the working tree the asm backend no longer refuses, and it gets the same case **wrong in the other
direction**: measured at the fingerprint above, the same project builds under `--backend=asm`, runs,
prints `true`, and its `pd_asm2.s:108` shows a check that was emitted with the *entry module's*
identity and allowlist (`kelp_caps_main_fs`, module string `main`) rather than `util`'s, so it
admits `/etc/hostname` under `/**`. A check with the wrong module is worse than no check, because it
looks checked. Both halves are the same missing thing, the `pd`'s declaring module, and that lane is
mid-change there; this file records the measurement, not a fix. The closure path is the
counter-example that shows the fix is to thread the table: a closure body in a narrow-grant module
emits its check and denies. So a rule about *naming* does not close this; it is a codegen table that
has to be passed for both backends, or a gated call in a `pd` initializer has to be refused by name,
which the "a construct the backend cannot lower is refused by name" rule already prefers over
silence.

**d. A module's own gated free functions: unreachable for user modules, so the exemption is safe.**
`domain_of` (`stdlib.rs:124`) knows six `std/...` ids and nothing else, so no user module can
declare a gated item, and `check_capability`'s prefix exemption (`infer.rs:544`) cannot widen a
user module's authority. That is why rule (b) can keep "a module may always call its own items"
without a second condition.

**e. `%*alias*%` and `impl` aliases: no second hole, but no evidence either.** Alias substitution is
textual and runs before parsing (`plan/syntax.md` 13.3), so an alias that expands to a call is the
same key as the call written out. Measured: `alias %*SNOOP*% = fs.read_raw("/etc/hostname")` in a
no-grant module compiles, runs, and prints `6` (the hole, unchanged); `alias %*READ*% = fs.read`
with `%*READ*%("/etc/hostname")` is refused by the same domain message as the direct call. An
`impl` alias is the same: `alias %*SFS*% = "std/fs"` then `impl %*SFS*%` still yields the module
alias `fs`, because `default_alias` (`resolve.rs:780`) reads the expanded path. The trap is not a
bypass, it is the blast-radius method: `grep` for `fs.read_raw` misses
`%*SNOOP*%(...)`, so a corpus claim based on a grep alone is weaker than the name-resolution claim
the fix makes. The counts in BLAST RADIUS were made from the `impl` lines and the extern
declarations, not from a name grep.

**f. A function value bypasses the whole gate, wrappers included (section 1.4).** `check_capability`
has one caller, the direct-call arm (`infer.rs:8692`); `function_value_at` (`infer.rs:1479`) does
not consult it. Measured with no grant: `val f: fun(&str).R: Bytes = fs.read` compiles, builds under
both backends, and reads `/etc/hostname`. This is not `BUG-wQ9tL`'s shape but it is the same
sentence ("the grant is advisory"), and it is the trap that decides whether the fix is a rule: a
rule about items has two naming sites to enforce it at, and a rule about `*_raw` names has one.

**g. The position tables, and the mechanical extension that breaks programs.** Covered in rule (b):
`open_raw` and `try_open_raw` must not check their mode argument (a relative mode denies, measured
with `fs.exists("w")`), and `spawn_raw` must check argument 1, not argument 0. `checked_args` is
keyed on the bare name as written, so `net.recv`'s `Socket` argument and `net.connect`'s host are
distinguished by name and the `_raw` names have to be named too. A fix that drops the extern filter
without touching the tables leaves `net.connect_raw`, `net.listen_raw`, `net.resolve_raw` and
`process.spawn_raw`'s program unchecked while looking closed.

**h. The entry module holds the wildcard, so the fixtures pass for a reason worth naming.** Every
new check the fixtures gain under rule (b) is checked against `/**` or `*`. That is not a loophole,
it is the model's first rule ("the entry module holds the wildcard for every domain"), but it means
the fixtures are not evidence that a *narrow* grant is enforced at a primitive: acceptance item 2
above is the test that is.

**i. `io`, `str`, `mem`, `path`, `fmt` and `math` externs stay ungated, and must.** 38 of the 97
externs belong to modules with no domain, and there is no grant to check them against: `mem.alloc_raw`
and `str.byte_at_raw` are arithmetic and byte shuffling, and `io`'s descriptor functions take a
descriptor the language has no capability domain for (the six domains are `fs`, `net`, `process`,
`env`, `time` and `rand`, and a raw file descriptor is none of them; this lane found no entry for
that gap in `ISSUES.md`, so it is recorded here rather than filed). Rule (b) is keyed on
`domain_of`, so it leaves them alone;
a rule that gated every extern because it is an extern would refuse `io_write_bodies` (3 sites) and
every `io` program for no security gain.

**j. A non-syscall extern inside a gated module becomes domain-gated.** `fs.utf8_valid_raw` reads
no file; `env.has_raw` reads no variable (it only asks whether one is set). Under rule (b) they
need the domain, which is the same treatment `fs.cwd` and `process.args` already get (both are
reachable with no pattern). It is consistent, and it is a small widening of what "the `fs`
capability" means to a reader; the fix's changelog line should say "every item a gated module
declares", not "every syscall".

**k. The denial message and its location are a fixture contract.** The check's `at` string is the
path the compiler was invoked with plus line and column (measured above:
`/tmp/kelp_bug_wq9tl_checked/src/util.kp:4:32`), and `caps_double_eval` asserts a whole denial byte
for byte. The new primitives reuse `cap_checked_call`, so their denial text is free, and that is one
more reason to prefer rule (b) over a new refusal in the front end.

**l. The `_raw` suffix is a convention with no enforcement.** All 97 externs end in `_raw`
(measured), and nothing in the compiler, the tests or the linter requires it. A rule written over
the suffix is either a new naming rule the language does not have, or a rule that fails open.

## UNDETERMINED

- **What the asm backend's check would cost at the primitive call sites.** `plan/asm-backend/capability-check.md`
  section 4 step 1 estimates the whole check at moderate size; this lane did not measure the
  marginal cost of the sites rule (b) adds (the same emission as a wrapper call, at more sites).
  Against HEAD, rule (b) moves `fs_path_family` from "builds and runs unchecked" to "refused by
  name", which is the right direction but is a coverage loss of one baseline entry. Against the
  working tree, where the check is being emitted, the same fixture keeps building and its stdout is
  unchanged (measured for the fixture and for a wrapper call; the raw-site half of that claim is a
  source read of `cap_decision`). Which of the two the fix lane starts from decides whether the
  baseline moves, so the fix has to measure it rather than inherit it.
- **Whether rule (a) can keep any corpus coverage of the 97 externs.** A user module cannot declare
  `extern fun` (measured), so under rule (a) the only module that can name a primitive is the
  stdlib, and the stdlib's own fixtures do not exist. A fixture *generator* that emits the wrapper
  call and reads the primitive's effect would be a new harness, not a fixture; nobody measured it.
- **The FnRef route's scope.** Whether the fix takes it (a `check_capability` call in
  `function_value_at`, plus a `FnRef` case in the asm backend's refusal so the symbol is not a bare
  `ld` error) or files it as its own row is a decision this lane did not have authority to make.
  Either way its `ld` half is a defect under the project's "refused by name" rule today.
- **The `pd` route's fix shape.** Threading a `CapTable` into `write_pd_globals` needs the `pd`'s
  declaring module, which the emitter currently drops for initializers; whether that is one field
  or a per-`pd` table was not measured.
- **Whether any gated-module method other than `Child.kill` reaches a resource a module could not
  otherwise reach.** `Listener.port` and `Socket.peer_addr` were measured to compile with no grant
  and reach `local_port_raw`/`peer_addr_raw` on a forged descriptor; `Child.wait`'s `reap_raw` and
  `net.Socket.set_timeout`'s `set_timeout_raw` were read, not run. The list of gated-module methods
  is `std/net`'s six and `std/process`'s three (measured by listing `fun Type.name(`).
- **`fpl`, `coral` and `lighthouse`.** `fpl` was grepped for `<alias>.<name>_raw(` and has none of
  the std modules' primitives; `coral` and `lighthouse` were not searched at all, and this lane's
  blast radius stops at the four places the task named.
- **Whether the language wants the "primitives are internal" position anyway**, independent of cost.
  That is a product decision for the supervisor, not a measurement: rule (a) is a smaller public
  surface (the stdlib's primitives stop being part of the module's API at all), and rule (b) is a
  larger one (a module that holds the domain may name anything the module declares, including a
  primitive, and the pattern check is the guard).
