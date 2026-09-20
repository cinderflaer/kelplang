# MK7 §5 item 7: the capability check the asm backend must emit

> **ARCHIVED 2026-09-20. Superseded, not deleted.** The per-call-site allowlist check keeps its
> shape under LLVM (a call into the runtime's matcher), so this file's account of which half is C,
> which half is Kelp and what the deny path prints is still the brief for that work
> (`plan/mk7-llvm-backend.md` §3).

Specification lane: this file changes no source. It says what a gated call site in the `asm` backend
must emit, which half of the C it replaces becomes an assembly routine this backend owns, which
half is already a Kelp function in `std/rt`, which half stays C, the deny path's exact observable
text, the acceptance list, and the traps. It is written so that a writer can execute it without
re-deriving anything.

Measured 2026-09-20 against the shared checkout of `kelp` (`main`, tip `bb2316e`, with
`kelpc/src/codegen_asm.rs` modified in place by a concurrent lane), using `target/release/kelpc`.
Every C or asm behaviour below was reproduced with a command in §0. The reconnaissance brief is
`plan/asm-backend/next-slice.md` §C; the plan is `plan/asm-backend/plan.md` §5 item 7. Line numbers in
`codegen_asm.rs` are the ones I read while that file was mid-edit, so the quoted text is the
authority there and the line number is a hint.

Quotes are verbatim with one exception: two source comments and one refusal message carry an em
dash, which no file in this repository may carry. Those are written with a comma or a full stop and
marked `[dash]`.

## 0. What was measured

Commands, all from `kelp/`:

```
./target/release/kelpc build --backend=asm kelpc/tests/fixtures/NAME.kp -o /tmp/probe/asm_NAME
./target/release/kelpc build --backend=c   kelpc/tests/projects/NAME/main.kp -o /tmp/probe/c_NAME
./target/release/kelpc build --backend=asm bench/speed/speed.kp -o /tmp/probe/speed_asm
readelf -h /tmp/probe/asm_caps_cwd
```

1. **The gated-call refusal is not the first refusal of any fixture.** `caps_gated_path` refuses
   `the 'asm' backend does not support the `extern fun std.fs.read_raw` yet`, and `caps_time_rand`
   refuses `the 'asm' backend does not support the `extern fun std.time.now_unix_raw` yet`, because
   `generate`'s live-extern pre-pass runs before any body is generated. Which is the brief's §C
   measurement, unchanged at this tip.
2. **Every `caps_*` project is behind an extern body except one.** `caps_denied`, `caps_granted`,
   `caps_write`, `caps_symlink`, `caps_link`, `caps_list` and `caps_fs_breadth` refuse first on
   `std.fs.read_raw` or `std.fs.write_text_raw` or `std.io.read_raw`; `caps_link_denied` on
   `std.fs.symlink_raw`; `caps_env` and `caps_env_denied` on `std.env.has_raw`; `caps_net` and
   `caps_net_denied` on `std.net.connect_raw`; `caps_net_resolve_denied` on `std.net.resolve_raw`.
   **`caps_cwd` builds and runs under the asm backend today** and prints the C backend's bytes
   (`true`), which is the positive control that a gated call whose resource arguments are all
   non-`str` is already emitted unchecked-for-a-reason and matches: no check exists to emit.
3. **The C side of every caps project was built and run for this file**, so the denial text in §3 is
   transcribed output, not a reading of the source.
4. **A gated program with no named resource emits no call into the glue at all.**
   `grep -c 'kelp_cap_check_' /tmp/probe/c_caps_time_rand.c` counts only the five definitions that
   `CAP_GLUE` always carries; there is no call site. The fixture's own comment says why: "Neither
   has a named resource, so there is nothing to match, the gate alone is the whole check."
5. **Therefore the check is not what blocks `bench/speed/speed.kp`.** Its only gated module is
   `std/time`, `stdlib.rs::checked_args` gives `time` and `rand` no `CapCheck` at all
   (`_ => Vec::new()`), and `cap_check_refusal` returns early on an empty check set (its own unit
   test is `a_gated_domain_with_no_checked_arguments_is_not_refused`). Measured: `speed.kp` under
   `--backend=asm` refuses `the 'asm' backend does not support the `extern fun
   std.time.now_mono_ns_raw` yet`, which is the extern body and nothing else. This disagrees with
   `plan/asm-backend/plan.md` §13's annex, which calls the check the gate artifact's second
   prerequisite. The check is a prerequisite of the *dependency-gated* fixtures (§4), not of the
   gate artifact.
6. **The C backend evaluates a resource argument twice.** A probe module `impl "std/fs"` with
   `fun mk().R: str { print("SIDE") return "/tmp/kelp_caps_dbl/a" }` and
   `fun go().R: String = fs.read_text(mk())`, built with `--backend=c`, prints `SIDE` twice before
   the file's contents. Cause: `cap_checked_call` re-emits `self.expr_value(&args[i])` inside the
   statement expression, and the call's own argument list emits it again. §5 trap 4.
7. **The asm link is non-PIE.** `readelf -h /tmp/probe/asm_caps_cwd` says `Type: EXEC`, so the
   allowlist table can be a plain `.quad` array of local labels in `.rodata` with no dynamic
   relocation (only `stderr@GOTPCREL` and the PLT entries are dynamic today).

## 1. The emitted shape, read out of the C side

### 1.1 The call site

`codegen_c.rs::cap_checked_call` (codegen_c.rs:3407-3495, comments elided from the quote below)
wraps the call in a GNU statement expression, so a check works in value position as well as
statement position:

```rust
    /// A gated call (M7): every resource argument is checked against the
    /// calling module's compiled-in allowlist *before* the call runs, and the
    /// whole thing is a statement expression so it works in value position
    /// (`val b = fs.read(p)`) as well as statement position.
    fn cap_checked_call(&self, callee: &str, call: String, args: &[Expr], span: Span) -> String {
        use crate::stdlib::CapCheck;

        let Some(domain) = self.caps.gated.get(callee) else {
            return call;
        };
        let Some(patterns) = self.caps.patterns_for(&self.module, domain) else {
            return call;
        };
        let arg_is_str: Vec<bool> =
            args.iter().map(|a| crate::stdlib::is_str_like_ty(&a.ty)).collect();
        let bare = callee.rsplit('.').next().unwrap_or(callee);
        let checked = crate::stdlib::checked_args(domain, bare, &arg_is_str);
        if checked.is_empty() {
            return call;
        }
        let symbol = CapTable::symbol(&self.module, domain);
        let at = self
            .sources
            .locate(&self.module, span)
            .unwrap_or_else(|| self.module.clone());
```

and then, per variant, one `writeln!` into `checks` (verbatim, the five format strings):

```rust
"kelp_cap_check_path(\"{domain}\", {path}, {symbol}, {}, \"{}\", \"{at}\"); "
"kelp_cap_check_name(\"{domain}\", {name}, {symbol}, {}, \"{}\", \"{at}\"); "
"kelp_cap_check_host(\"{domain}\", {host}, {symbol}, {}, \"{}\", \"{at}\"); "
"kelp_cap_check_hostport(\"{domain}\", {host}, {port}, {symbol}, {}, \"{}\", \"{at}\"); "
"kelp_cap_check_port(\"{domain}\", {port}, {symbol}, {}, \"{}\", \"{at}\"); "
```

The `{}` before the module is `patterns.len()`, a literal count, not a table terminator. The whole
thing is returned as

```rust
        format!("({{ {checks}{call}; }})")
```

and the caller puts it inside the hoisted-argument prelude, frees included
(codegen_c.rs:3673-3674, `let call = self.cap_checked_call(callee, s, args, span);
self.hoisted_call(prelude, call, &hoisted, ret);`). Real emitted lines, one per variant, at the
call sites that reach them:

```c
// fs, one path argument
return ({ kelp_cap_check_path("fs", "/etc/passwd", kelp_caps_util_fs, 1, "util", "/mnt/bigdrive/projects/KelpLANG/kelp/kelpc/tests/projects/caps_denied/util.kp:3:27"); kelp_std__fs__read_text("/etc/passwd"); });
// fs, two path arguments (fs.symlink checks both ends; the target is checked first)
({ kelp_cap_check_path("fs", "/etc/passwd", kelp_caps_util_fs, 1, "util", ".../caps_link_denied/util.kp:6:5"); kelp_cap_check_path("fs", "/tmp/kelp_caps_link_denied/escape", kelp_caps_util_fs, 1, "util", ".../caps_link_denied/util.kp:6:5"); kelp_std__fs__symlink("/etc/passwd", "/tmp/kelp_caps_link_denied/escape"); });
// net, host and port together (the port is the call's own argument)
struct_std__net__Socket kelp_s = ({ kelp_cap_check_hostport("net", "localhost", ((uint16_t)(1)), kelp_caps_util_net, 1, "util", ".../caps_net_denied/util.kp:9:13"); kelp_std__net__connect("localhost", ((uint16_t)(1))); });
// net, a port alone (net.listen binds every interface, so the port is the resource)
struct_std__net__Listener kelp_listener = ({ kelp_cap_check_port("net", ((uint16_t)(0)), kelp_caps_peer_net, 1, "peer", ".../caps_net/peer.kp:23:20"); kelp_std__net__listen(((uint16_t)(0))); });
// net, a host alone (net.resolve: the pattern's port is ignored)
kelp_List__String kelp_addrs = ({ kelp_cap_check_host("net", "example.com", kelp_caps_util_net, 1, "util", ".../caps_net_resolve_denied/util.kp:7:17"); kelp_std__net__resolve("example.com"); });
// the module can be a stdlib module (std/http holds the grant, so std/http calls are checked)
struct_std__net__Socket kelp_s = ({ kelp_cap_check_hostport("net", kelp_host, kelp_port, kelp_caps_std_http_net, 1, "std/http", ".../http_redirect_denied/src/std/http.kp:818:13"); kelp_std__net__connect(kelp_host, kelp_port); });
```

Three properties of that shape matter and are not cosmetic:

* the fifth argument is the **calling** module (`self.module`), not the module the gated function
  lives in, and it is the same key `sources.locate` is given, so the grant is a property of the code
  that wrote the call;
* `at` is `<the .kp path as the compiler was invoked with>:<line>:<column>`, from
  `SourceMap::locate` (diag.rs:130-134, `Some(format!("{}:{line}:{col}", f.path.display()))`), and
  it falls back to the module id when the module's text was never registered. `codegen_c.rs` sets
  `module` per generated function (its `Gen` field doc: "The module the current function was
  declared in: capability checks use *that* module's patterns"); `line_directive` (diag.rs:145)
  also emits a `#line` marker per statement, but the denial does not use it;
* a check whose `checked_args` answer is empty is not emitted at all, which is why `time`, `rand`,
  `fs.cwd`, `net.send`, `net.recv`, `process.args` and every other no-named-resource call have the
  plain call shape.

Which argument is checked, per domain, is `stdlib.rs::checked_args` (stdlib.rs:200-210):

```rust
pub fn checked_args(domain: &str, callee: &str, arg_is_str: &[bool]) -> Vec<CapCheck> {
    match domain {
        "fs" => fs_checked_args(callee, arg_is_str),
        "env" => first_str_arg(arg_is_str)
            .map(|i| vec![CapCheck::Name(i)])
            .unwrap_or_default(),
        "process" => process_checked_args(callee, arg_is_str),
        "net" => net_checked_args(callee, arg_is_str),
        _ => Vec::new(),
    }
}
```

with `fs`: every `str` argument except `open`/`try_open`'s second (a mode string);
`process`: the program path, argument 1 for `spawn_in`/`run_in`/`try_run_in` and argument 0
otherwise; `net`: `connect` is `HostPort(0, 1)`, `listen` is `Port(0)`, `resolve` is `Host(0)`, and
everything else is empty. `arg_is_str` is `stdlib::is_str_like_ty` per argument, and that predicate
admits exactly `Ty::Str` and `Ty::Ref` of `Ty::Str` (stdlib.rs:181-189), so **every resource
argument is a bare pointer in one eightbyte**: no resource argument is ever a `MEMORY`-class
aggregate. The asm backend already builds the same vector at the same place
(`codegen_asm.rs:10455-10457`).

### 1.2 The grants table

`codegen_c.rs::CapTable` (codegen_c.rs:563-588, the doc comments elided) is the per-program table:
`gated` maps a function key to its domain, `patterns` maps `(module, domain)` to the patterns that
module holds.

```rust
struct CapTable {
    gated: HashMap<String, String>,
    patterns: HashMap<(String, String), Vec<String>>,
}
...
    /// The name of the compiled-in pattern array for one module and domain.
    fn symbol(module: &str, domain: &str) -> String {
        format!(
            "kelp_caps_{}_{}",
            crate::types::c_ident(&module.replace('/', "_")),
            domain
        )
    }
```

`program.gated` and `program.caps` come from `resolve.rs` (`Project::gated`,
`Project::caps`; the entry module holds `/**` for every domain, every other module holds exactly
what its importers delegated). The table lives in the program's own text, right after `CAP_GLUE`,
in the order `program.caps` iterates (codegen_c.rs:686-701):

```rust
    if cap_needed {
        out.push_str("// capability allowlists, compiled in per module\n");
        for g in &program.caps {
            writeln!(
                out,
                "static const char* {}[] = {{ {}NULL }};",
                CapTable::symbol(&g.module, &g.domain),
                g.patterns
                    .iter()
                    .map(|p| format!("{}, ", c_string_literal(p)))
                    .collect::<String>()
            )
            .unwrap();
        }
```

so a module holding a domain with no patterns is emitted as `{ NULL }` rather than skipped. Emitted
today (from the fixture and project builds above):

```c
// capability allowlists, compiled in per module
static const char* kelp_caps_util_fs[] = { "/tmp/kelp_caps_denied/**", NULL };
static const char* kelp_caps_std_http_net[] = { "127.0.0.1:*", NULL };
static const char* kelp_caps_caps_time_rand_time[] = { "*", NULL };
static const char* kelp_caps_main_process[] = { "/**", NULL };
```

The layout is an array of `const char*` terminated by a `NULL`, with the length passed separately as
an immediate, so the emitter and the check share one answer for "how many patterns" and no check
ever scans for the terminator. A stdlib module's id becomes underscores before `c_ident` runs
(`std/http` gives `kelp_caps_std_http_net`), which matters because `std/http` can hold the grant
(`http_redirect_denied/src/main.kp:10`).

### 1.3 The check functions and the C text that supplies them

The text is `kelpc/runtime/cap_glue.c` (182 lines). Its name in `codegen_c.rs` is **`CAP_GLUE`**,
not `CAP_RUNTIME` (there is no `CAP_RUNTIME` in the tree; the other big string there is `RUNTIME`,
codegen_c.rs:36), and it is spliced in as

```rust
/// Where the capability check's own half lives (MK7 §8): the deny path, the
/// buffered plumbing, and the allowlist loops. The *matching* and the *path
/// normalization* it calls are Kelp (`std/rt`), declared just before this text
/// is emitted, a comma here for an em dash in the source [dash], see
/// `runtime_prototypes` and `kelpc/runtime/cap_glue.c`.
const CAP_GLUE: &str = include_str!("../runtime/cap_glue.c");
```

and it is emitted only for a program that gates something
(`let cap_needed = !program.gated.is_empty();`, codegen_c.rs:619, used at :672-675). The file is
one helper, one deny routine and five checks, seven functions in all:

```c
static bool kelp_resolve_path(const char* path, char* out, size_t cap);
static void kelp_cap_deny(const char* domain, const char* path, const char* const* patterns,
                          size_t count, const char* module, const char* at);
static void kelp_cap_check_path(const char* domain, const char* path, const char* const* patterns,
                                size_t count, const char* module, const char* at);
static void kelp_cap_check_name(const char* domain, const char* name, const char* const* patterns,
                                size_t count, const char* module, const char* at);
static void kelp_cap_check_host(const char* domain, const char* host, const char* const* patterns,
                                size_t count, const char* module, const char* at);
static void kelp_cap_check_hostport(const char* domain, const char* host, uint16_t port,
                                    const char* const* patterns, size_t count,
                                    const char* module, const char* at);
static void kelp_cap_check_port(const char* domain, uint16_t port, const char* const* patterns,
                                size_t count, const char* module, const char* at);
```

There is nothing else in the file: those seven are the whole of the glue. `kelp_cap_deny` is the only
one that returns; the five checks return `void` and
never fall through to the call when they deny (the deny exits). Each check is an allowlist loop plus
one call into Kelp:

* `kelp_cap_check_path`: normalize into a `PATH_MAX + 1` buffer through a borrowed `Bytes` view,
  then `kelp_resolve_path`, then `kelp_std__rt__glob_match(patterns[i], resolved)`;
* `kelp_cap_check_name`: `kelp_std__rt__glob_match(patterns[i], name)`, no normalization;
* `kelp_cap_check_host`: `strcmp(pat, "*") == 0` first, then
  `kelp_std__rt__host_glob(pat, host)`, then the same with the pattern's own host part cut at its
  last `:` (a `PATH_MAX` buffer and a `memcpy`);
* `kelp_cap_check_hostport`: `snprintf(hostport, sizeof hostport, "%s:%u", host, (unsigned)port)`
  then `kelp_std__rt__host_match(patterns[i], hostport)`;
* `kelp_cap_check_port`: builds `"*:%u"` with two `snprintf` calls, returns on a `"*"` pattern,
  returns on a pattern with no `:` ("a host-only pattern constrains no port"), otherwise
  `kelp_std__rt__host_glob(colon + 1, number)`.

The two the brief calls the buffered plumbing and the symlink resolution, verbatim:

```c
static bool kelp_resolve_path(const char* path, char* out, size_t cap) {
    char real[PATH_MAX];
    if (realpath(path, real) != NULL) {
        snprintf(out, cap, "%s", real);
        return true;
    }
    char buf[PATH_MAX];
    snprintf(buf, sizeof buf, "%s", path);
    char* slash = strrchr(buf, '/');
    if (slash == NULL || slash == buf) {
        return false;
    }
    *slash = 0;
    char parent[PATH_MAX];
    if (!kelp_resolve_path(buf, parent, sizeof parent)) {
        return false;
    }
    snprintf(out, cap, "%s/%s", parent, slash + 1);
    return true;
}
```

```c
static void kelp_cap_check_path(const char* domain, const char* path, const char* const* patterns,
                             size_t count, const char* module, const char* at) {
    /* One byte past `PATH_MAX`, because the normalizer is handed a borrowed
       `kelp_Bytes` *view* of this array whose `len` is the capacity, and
       `Bytes` puts its NUL in the slot at `len`: an array of exactly `len`
       bytes would make that slot a lie. The view is never dropped, it is not
       an allocation, and dropping it would free a stack address. [dash] */
    char normalized[PATH_MAX + 1];
    kelp_Bytes view;
    view.data = (unsigned char*)normalized;
    view.len = PATH_MAX;
    view.cap = PATH_MAX;
    if (!kelp_std__rt__path_normalize(path, &view)) {
        return kelp_cap_deny(domain, path, patterns, count, module, at);
    }
    char resolved[PATH_MAX];
    if (!kelp_resolve_path(normalized, resolved, sizeof resolved)) {
        snprintf(resolved, sizeof resolved, "%s", normalized);
    }
    for (size_t i = 0; i < count; i++) {
        if (kelp_std__rt__glob_match(patterns[i], resolved)) {
            return;
        }
    }
    return kelp_cap_deny(domain, resolved, patterns, count, module, at);
}
```

Two readings of that body are load bearing and must be transliterated, not improved:
`path_normalize` failing denies with the **argument as written** (`path`), while a failed match
denies with the **resolved** string (`resolved`); and `kelp_resolve_path` failing falls back to
matching the normalized-but-unresolved path, so a path whose resolution fails is matched without its
symlinks resolved.

### 1.4 Where each piece sits in the emitted file

In order, from `generate`'s own pushes: `RUNTIME` (codegen_c.rs:36, the whole C runtime as a `&str`),
then, when `path_loaded || cap_needed`, `runtime_prototypes(program)`, then `CAP_GLUE` when
`cap_needed`, then the allowlist arrays, then the type declarations and function bodies.
`runtime_prototypes` (codegen_c.rs:2392-2405) writes one signature per function of `RT_MODULE`
(`std/rt`), which is the whole of the C half's knowledge of the Kelp half:

```rust
fn runtime_prototypes(program: &Program) -> String {
    let mut out = String::new();
    out.push_str("// The capability matcher, written in Kelp (`std/rt`): one\n");
    out.push_str("// implementation for every backend (MK7 \u{a7}8).\n");
    for f in program
        .functions
        .iter()
        .filter(|f| f.module == crate::stdlib::RT_MODULE)
    {
        write_signature(&mut out, f);
        out.push_str(";\n");
    }
    out
}
```

and the emitted declarations are the eight functions of `std/rt.kp`:

```c
bool kelp_std__rt__glob_match(const char* kelp_pat, const char* kelp_path);
bool kelp_std__rt__dir_wildcard_at(const char* kelp_s, size_t kelp_i);
bool kelp_std__rt__host_glob(const char* kelp_pat, const char* kelp_text);
bool kelp_std__rt__host_glob_at(const char* kelp_pat, size_t kelp_pstart, const char* kelp_text, size_t kelp_tstart, size_t kelp_tend);
bool kelp_std__rt__host_match(const char* kelp_pat, const char* kelp_hostport);
size_t kelp_std__rt__host_end(const char* kelp_hostport);
bool kelp_std__rt__has_colon(const char* kelp_pat);
bool kelp_std__rt__path_normalize(const char* kelp_s, kelp_Bytes* kelp_out);
```

The module is injected by the resolver, at the import that needs it
(`resolve.rs:739-741`: `if crate::stdlib::domain_of(&target).is_some() || target == "std/path" {
self.visit(crate::stdlib::RT_MODULE, imp.span, id, strict)?; }`), and `generate` asserts the
prototypes are non-empty rather than letting `ld` report an undefined symbol.

### 1.5 The Kelp half, which is already written and already lowers

`kelpc/std/rt.kp` (296 lines) holds the matcher and the normalizer, and the asm backend compiles it
today: `Routine::PathNormalizeRaw` calls `std.rt.path_normalize` by symbol, which is the proof. The
four functions a check needs, with the signatures the C calls:

```kelp
fun glob_match(pat: &str, path: &str).R: bool
fun host_glob(pat: &str, text: &str).R: bool
fun host_match(pat: &str, hostport: &str).R: bool
fun path_normalize(s: &str, out: mut Bytes).R: bool
```

`glob_match` is the path and the name matcher (`*` within one segment, `**` crossing segments);
`host_glob`/`host_match` are the `net` vocabulary; `path_normalize` writes into the caller's buffer
and allocates nothing, with `out.len()` as the capacity and the NUL at `data[len]`.

The matcher's differential control is `kelpc/tests/cap_matcher.rs`: its corpus drives
`kelpc/runtime/cap_matcher_ref.c` (the oracle, compiled by the test, no longer emitted into
programs) and a C-built driver over `std/rt`, and its second test
(`the_generated_capability_check_calls_the_kelp_matcher`) asserts that the generated C contains
`kelp_std__rt__glob_match(patterns[i], resolved)` and `kelp_std__rt__host_glob(pat, host)` and does
**not** contain `static bool kelp_glob_match`. That test is C-backend-only today; §4 gives its asm
analogue.

## 2. The split

**The asm backend compiles no C at all**, so "stay C" cannot mean a mixed build: `driver.rs`'s
`assemble_and_link` writes the generated `.s` next to the binary, assembles it with
`Command::new("as").arg("--64")` and links it with `Command::new("ld")` against `crt1.o`, `crti.o`,
`crtn.o`, `-lc` and `-lm`. Its own comment: "The point of this path is that no C compiler is
involved". The third category below is therefore about what this slice does *not* transliterate yet,
which must leave the asm backend refusing that call shape rather than emitting it.

### 2.1 What becomes an assembly routine this backend owns

One `Routine` arm per new routine, added to the existing enum (`codegen_asm.rs:849-931`,
`Routine::PathNormalizeRaw` is the model), labelled in the existing style
(`kelp_rt$cap_check_path`), registered at the call site with `require_routine`, and emitted once each
by the loop at the bottom of `generate` which already sorts routines by label and emits them after
every function body:

| routine | body says |
| --- | --- |
| `kelp_rt$cap_deny` | the deny path of §3: two or three `fprintf@PLT` calls and `exit@PLT` |
| `kelp_rt$cap_check_path` | the loop plus `path_normalize`, `kelp_rt$resolve_path`, `glob_match`, deny on failure |
| `kelp_rt$cap_check_name` | the loop plus `glob_match`, deny on failure |
| `kelp_rt$cap_check_host` | `strcmp`, `host_glob`, the last-colon head cut, deny |
| `kelp_rt$cap_check_hostport` | `snprintf@PLT` of `"%s:%u"`, `host_match`, deny |
| `kelp_rt$cap_check_port` | two `snprintf@PLT` calls, the `"*"` and no-colon rules, `host_glob`, deny |
| `kelp_rt$resolve_path` | `realpath@PLT`, `strrchr@PLT`, `snprintf@PLT`, and the recursion of §1.3 |

Each check routine calls its own deny routine, so the deny text has exactly one implementation; each
is a real function with its own frame, so `kelp_rt$resolve_path` can call itself (the C recurses
once per missing path component). Every buffer size in §1.3 is part of each routine's contract, so
they are transliterated rather than chosen: `PATH_MAX + 1` for the normalizer's array with a view of
`len == cap == PATH_MAX`, `PATH_MAX` for the resolved string, the fallback and each of
`kelp_rt$resolve_path`'s two locals, and `PATH_MAX` plus the `sizeof head - 1` clamp for the host
head (trap 2).

The call-site sequence itself is emitted by `Gen::call`, and it is the *same* shape the C has: the
check runs before the call, in the callee's frame, with the resource pointer as an argument. The
per-variant argument layout, from the C prototypes, is fixed SysV:

| variant | `rdi` | `rsi` | `rdx` | `rcx` | `r8` | `r9` | stack |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Path / Name / Host | domain | resource | patterns | count | module | at | none |
| HostPort | domain | host | port | patterns | count | module | `at` at `[rsp]` |
| Port | domain | port | patterns | count | module | at | none |

`domain`, `module` and `at` are interned string constants (`Gen::bytes_label`), `count` is an
immediate, and `patterns` is a `lea` of `kelp_caps_<module>_<domain>` into the register the table
above names (`rdx` for four variants, `rcx` for `_hostport`).
The 7th argument of `_hostport` lands on the stack, so that call must carve its outgoing area below
`rsp` (`sub rsp, 16; mov QWORD PTR [rsp], <at>; call ...; add rsp, 16`) rather than writing at
`[rsp]`, or it overwrites the last pushed argument. Alignment is the existing rule: `rsp` must be
16-byte aligned at the `call`, and the argument pushes have left it 8 mod 16 for an odd number of
slots.

The allowlist table is new data in `.rodata`, one array per `program.caps` entry, exactly the C's
symbol and layout:

```asm
    .section .rodata
kelp_caps_util_fs:
    .quad .LC41
    .quad 0
```

`Gen::data` is a `Vec<(String, Vec<u8>)>` and `finish` writes only `.byte` runs, so this needs one
small addition to the emitter (a second vector of pointer arrays, emitted after the byte runs, or
`.quad` lines written directly into that block). The link is `Type: EXEC` (§0 item 7), so a `.quad`
of a local label needs no dynamic relocation. An empty pattern list is `.quad 0` alone, matching
`{ NULL }`.

### 2.2 What is a call into the Kelp in `std/rt.kp`

Everything that *decides*: `glob_match` for the path and name loops, `path_normalize` for the path
check's first half, `host_glob` and `host_match` for the three `net` checks. None of it is ported.
The existing `Routine::PathNormalizeRaw` is the exact template for the call, and its body is what
"what the call site needs" means (codegen_asm.rs:11837-11860):

```asm
    push rbx
    push r12
    sub rsp, 4136
    mov rbx, rdi
    mov r12, rsi
    lea rax, [rsp+24]
    mov QWORD PTR [rsp], rax
    mov QWORD PTR [rsp+8], 4096
    mov QWORD PTR [rsp+16], 4096
    mov rdi, r12
    mov rsi, rsp
    call kelp_std__rt__path_normalize
    test al, al
    je .Lpath_norm_false
```

Four requirements, all of them already satisfied by that code:

1. **The key has to be a live-set root.** A routine inserts `RT_PATH_NORMALIZE` ("std.rt.path_normalize",
   codegen_asm.rs:759) into `Gen::rt_functions`, and `generate` re-reads that set as new call-site
   roots and regenerates the closure, which is why the loop below the function loop exists (its
   comment names `PathNormalizeRaw` and `std.rt.path_normalize`). The check routines must insert
   `std.rt.glob_match`, `std.rt.host_glob`, `std.rt.host_match` and `std.rt.path_normalize` the same
   way, at the call site, before the routine set is snapshotted.
2. **A `str` is a bare `const char*`**, so the matcher's two parameters are `rdi`/`rsi` with no
   marshalling, and the answer is `al`.
3. **`path_normalize`'s second parameter is a borrowed `kelp_Bytes` view** of the caller's own
   `PATH_MAX + 1` byte array, with `len == cap == PATH_MAX`: three eightbytes at `rsi`, three
   elements before the array. The existing routine's frame is that array at `[rsp+24]` and the view
   at `[rsp]`, 4136 bytes of frame. The view is never dropped, because it is not an allocation.
4. **The prototypes are not needed in the asm**, because the Kelp functions are emitted into the same
   object; a `call kelp_std__rt__glob_match` resolves at link time.

### 2.3 What stays C

Three senses, all of them meant:

1. **As the oracle, unchanged.** `kelpc/runtime/cap_glue.c` stays the C backend's emitted text
   (`CAP_GLUE`) and `kelpc/runtime/cap_matcher_ref.c` stays the matcher's reference, which
   `cap_matcher.rs` compiles itself. Nothing in this slice edits either, and the C backend's
   behaviour is the acceptance standard for the asm one.
2. **Deferred by this slice: the three `net` checks.** `kelp_cap_check_host`, `_hostport` and
   `_port` are `strcmp`/`strrchr`/`snprintf`/`memcpy` buffering plus the `net` vocabulary rules.
   Every fixture that reaches them is behind a `net` extern body anyway (`caps_net`,
   `caps_net_denied`, `caps_net_resolve_denied` on `std.net.connect_raw`/`resolve_raw`;
   `http_redirect_denied` on the whole HTTP stack), so step 1 does not need them, and the asm
   backend must keep **refusing** a `net` call whose check kind is not implemented yet, with the
   existing message shape, rather than emitting it unchecked. That refusal is the fail-closed
   direction and is the whole reason this split is allowed to be partial.
3. **Deferred indefinitely, with a reason: nothing else.** Everything left in `cap_glue.c` is
   transliterated. The piece worth naming is `kelp_resolve_path`: it is C in both backends today
   because `realpath` is a libc call and `std/rt` has no way to reach one (its own module doc lists
   what it holds, and it declares no `extern fun`; adding one would mean a compiler-supplied body in
   `externs.rs` for the C backend as well, which is a second backend's change for no correctness
   gain). `kelp_resolve_path` moves to Kelp with the filesystem half of the runtime, the same way
   `path_normalize` already moved.

### 2.4 What the call site itself needs

* `Gen` gains the module of the function being generated (`Function::module`, already available in
  `Gen::function`), because the allowlist symbol, the fifth argument and `at` all key on it. The asm
  backend never reads `Function::module` today.
* `Gen` gains `&SourceMap`, and `generate`'s third parameter stops being `_sources`
  (`pub fn generate(program: &Program, drops: &DropPlan, _sources: &SourceMap) -> Result<String,
  String>`). The location is `sources.locate(&self.module, span)` and nothing else: re-deriving a
  location from a span anywhere else is a byte difference in the deny text.
* `Gen::call` gains the call expression's `span`. `Expr.span` is the node's own span and `call` is
  reached from two dispatch sites (`codegen_asm.rs:8597`, `:10135`), both of which have the `Expr` in
  scope, so this is a parameter and two call sites.
* The decision function replaces `cap_check_refusal` (`codegen_asm.rs:1085-1097`, quoted in full
  because it is the whole current behaviour):

```rust
fn cap_check_refusal(
    gated: &std::collections::HashMap<String, String>,
    callee: &str,
    arg_is_str: &[bool],
) -> Option<String> {
    let domain = gated.get(callee)?;
    let bare = callee.rsplit('.').next().unwrap_or(callee);
    if crate::stdlib::checked_args(domain, bare, arg_is_str).is_empty() {
        return None;
    }
    Some(domain.clone())
}
```

  It becomes "which check kinds, or a refusal for the kinds not implemented yet", so its unit tests
  (`a_gated_call_with_a_resource_argument_is_refused`, `a_gated_call_with_no_resource_argument_is_not_refused`,
  `an_ungated_call_is_not_refused`, `a_gated_domain_with_no_checked_arguments_is_not_refused` at
  codegen_asm.rs:12386-12431) turn from "refused" into "checked", and the first one keeps a refusal
  only if step 1 defers its kind.
* The check is registered as a routine at the call site (`self.routines.insert(...)`,
  `self.rt_functions.insert(...)`), and `needs_cap` (or the existing `needs_panic` mechanism) decides
  whether `kelp_rt$cap_deny` is emitted at all. A program that never checks must not carry the
  routine, and a program that checks must not link against a symbol nothing emitted; the existing
  `needs_panic`/`emit_panic_routine` pair is the model, and the `ld`-failure mode is fail-closed but
  is a build failure rather than a diagnostic.
* The check must be emitted where the C emits it relative to the call: **before** the call, while
  the resource argument's storage is still alive and before any hoisted temporary is freed. The C's
  own order is check, call, then the frees (`hoisted_call` writes the drops after the call), and a
  check emitted after a free would read a freed buffer.

## 3. The deny path's exact observable text

One routine, `kelp_cap_deny`, three `fprintf` shapes and `exit(1)`:

```c
static void kelp_cap_deny(const char* domain, const char* path, const char* const* patterns,
                       size_t count, const char* module, const char* at) {
    fprintf(stderr, "%s: panic: '%s' is outside the %s capability granted to module '%s'\n",
            at, path, domain, module);
    if (count == 0) {
        fprintf(stderr, "  granted: (nothing)\n");
    } else {
        fprintf(stderr, "  granted:\n");
        for (size_t i = 0; i < count; i++) {
            fprintf(stderr, "    %s\n", patterns[i]);
        }
    }
    exit(1);
}
```

Every part, and where it comes from:

| part | source |
| --- | --- |
| `at` | `SourceMap::locate(&calling_module, call_span)`, `<path as the compiler was invoked with>:<line>:<col>`, or the module id when the file is unregistered |
| the path in quotes | `resolved` for a failed match, the argument as written when `path_normalize` refused it, the name as written for `_name`, the host as written for `_host`, `host:port` for `_hostport`, `*:port` for `_port` |
| the domain | the string literal `"fs"`/`"env"`/`"process"`/`"net"`, from `program.gated` |
| the module | the string literal of the calling module, the same key as `at` |
| the count and the patterns | the immediate count and the `kelp_caps_<module>_<domain>` array |
| the exit code | `exit(1)` |

Transcribed output, one line per denying project, from the C-built binaries:

```
caps_denied          .../caps_denied/util.kp:3:27: panic: '/etc/passwd' is outside the fs capability granted to module 'util'
                       granted:
                         /tmp/kelp_caps_denied/**
caps_symlink         .../caps_symlink/util.kp:3:26: panic: '/etc/passwd' is outside the fs capability granted to module 'util'
                       granted:
                         /tmp/kelp_caps_symlink/**
caps_link_denied     .../caps_link_denied/util.kp:6:5: panic: '/etc/passwd' is outside the fs capability granted to module 'util'
                       granted:
                         /tmp/kelp_caps_link_denied/**
caps_env_denied      .../caps_env_denied/util.kp:3:22: panic: 'HOME' is outside the env capability granted to module 'util'
                       granted:
                         PATH
caps_net_denied      .../caps_net_denied/util.kp:9:13: panic: 'localhost:1' is outside the net capability granted to module 'util'
                       granted:
                         127.0.0.1:*
caps_net_resolve_denied  .../caps_net_resolve_denied/util.kp:7:17: panic: 'example.com' is outside the net capability granted to module 'util'
                       granted:
                         127.0.0.1:*
```

and the two shapes no fixture reaches, both reproduced with a probe in `/tmp`:

```
# a module holding the domain with no patterns at all (impl "util" caps(env = []))
/tmp/capprobe2/util.kp:3:22: panic: 'HOME' is outside the env capability granted to module 'util'
  granted: (nothing)

# a relative path, so path_normalize refuses and the argument is printed as written
/tmp/capprobe3/util.kp:3:23: panic: 'relative/path.txt' is outside the fs capability granted to module 'util'
  granted:
    /tmp/kelp_caps_rel/**
```

`caps_symlink` is the one that shows the resolution: the fixture reads `/tmp/kelp_caps_symlink/escape`,
a link to `/etc/passwd`, and the denial names `/etc/passwd`, the resolved path.

Which tests assert on it, measured precisely, because none of them asserts the whole text:

* `compile_and_run.rs::assert_project_denied_at_runtime` builds, runs, asserts the exit status is
  non-zero, and asserts **`stderr.contains(needle)`**. The needles are
  `"outside the fs capability granted to module 'util'"` (`caps_denied`),
  `"/etc/passwd"` (`caps_symlink`),
  `"'/etc/passwd' is outside the fs capability granted to module 'util'"` (`caps_link_denied`),
  `"'HOME' is outside the env capability granted to module 'util'"` (`caps_env_denied`),
  `"127.0.0.1:*"` (`caps_net_denied`),
  `"is outside the net capability granted to module 'util'"` (`caps_net_resolve_denied`),
  `"is outside the process capability granted to module 'runner'"` (`process_denied`), and
  `"outside the net capability granted to module 'std/http'"` (`http_redirect_denied`, through
  `assert_project_served_denied`).
* The `at` prefix, the `granted:` line, the indentation and the `(nothing)` branch are asserted
  **nowhere**. The differential sweep compares stdout and exit status only
  (`backend_coverage.rs::run_sweep`: `if c_run.stdout != asm_run.stdout` and `if c_run.code !=
  asm_run.code`), and it sweeps `tests/fixtures/*.kp`, not the projects. So a denial that printed a
  different path or a different location would keep the whole suite green while changing behaviour,
  which is the gap §4's acceptance closes.

## 4. Slice order and the acceptance list

Three steps, each one a coherent build. Nothing here is a fixture edit or a source edit: this file
only says what the writer must make true.

### Step 0: the three plumbing facts (no behaviour change)

`Gen.module`, `Gen.sources` and the call's `span` (§2.4), with the decision function still refusing
everything it refuses today. Acceptance: the whole existing suite is unchanged, including the
refusal message text, which is byte-identical to today's.

### Step 1: the path and name checks, the deny path, the allowlist data, symlink resolution

`kelp_rt$cap_deny`, `kelp_rt$resolve_path`, `kelp_rt$cap_check_path`, `kelp_rt$cap_check_name`, the
`.quad` allowlist arrays, and the call-site emission for `CapCheck::Path` and `CapCheck::Name`. The
`net` kinds (`Host`, `HostPort`, `Port`) keep refusing by name.

**What step 1 unblocks, plainly: nothing that builds today.** Measured (§0 item 2): every fixture and
project that would exercise a path or name check is refused earlier, by an extern body. So step 1's
own acceptance is not a fixture that starts passing; it is the two assertions below plus the sweep,
and the fixtures it makes *reachable* arrive with MK7 §5 item 5/10's extern bodies (the `std.fs`,
`std.env` and `std.process` half). Say that in the write-up rather than claiming a fixture.

Acceptance for step 1, in the order to run it:

1. **The decision, at unit level.** `codegen_asm.rs`'s four `cap_check_refusal` unit tests, updated:
   a `fs` call with a `str` argument yields the path check, an `env` call with a `str` argument
   yields the name check, a `net` call with a host or port still refuses (step 1 defers it), a
   `time`/`rand` call and `fs.cwd` still yield nothing. The refusal message keeps the existing shape:
   `"the capability check that gates `{callee}` (domain `{domain}`), the asm backend cannot emit an
   unchecked call, so it refuses instead"`, where the source carries an em dash and this file writes
   a comma `[dash]`.
2. **The emission, read out of the assembly.** A new asm-only test in `compile_and_run.rs` beside
   `the_classifier_puts_each_aggregate_eightbyte_in_its_own_register_file` (which already reads
   `<bin>.s`, because `assemble_and_link` keeps it next to the binary): it builds `caps_gated_path`
   and asserts the `.s` contains a call to `kelp_rt$cap_check_path` **before** the call to
   `kelp_std__fs__read` in the same function body, that the call site loads
   `kelp_caps_caps_gated_path_fs` (the symbol rule of §1.2 for the entry module), and that
   `kelp_rt$cap_check_path`'s own body calls `kelp_std__rt__path_normalize`, then
   `kelp_std__rt__glob_match`, then `kelp_rt$cap_deny` in that order, which is the check's shape read
   back out of the backend rather than argued. This
   is the asm-side analogue of `cap_matcher.rs::the_generated_capability_check_calls_the_kelp_matcher`.
   It is buildable **after** `std.fs.read_raw`'s body exists, because that fixture's refusal is the
   extern; if step 1 lands first, run the equivalent reading against a project that builds, or defer
   this assertion to the step that lowers `std.fs.read_raw`.
3. **Nothing regressed.** `cargo test --test backend_coverage -- --ignored --nocapture` (the
   differential sweep and the refusal-prefix test), `KELPC_BACKEND=asm cargo test --test
   compile_and_run`, and `cargo test --test memory_gate -- --ignored --nocapture`, with
   `ASM_BUILT_BASELINE` re-recorded by the lane that owns `backend_coverage.rs` if the set grew.

**Fixtures and projects step 1 must be judged on, once their extern bodies exist** (the list to build
under both backends and compare stdout byte for byte, which is what the sweep and the suite do):

* grant-admitting, one per shape: `kelpc/tests/fixtures/caps_gated_path.kp` (an `fs.read` of an
  absolute path in the entry module, whose check admits and whose call then fails with errno:
  `panic: fs.read: cannot open file` and `  errno 2: No such file or directory`, exit 1, measured),
  `caps_granted`, `caps_write`, `caps_link`, `caps_cwd`, `caps_list`, `caps_fs_breadth`, `caps_env`;
* fixture-level: `kelpc/tests/fixtures/caps_time_rand.kp` (`true\ntrue\ntrue\n4\nslept\n`), which
  must keep printing that with the same generated bytes and no check emitted for it.

**Fixtures and projects that must be denied at runtime with the same message as the C backend:**
`caps_denied`, `caps_symlink`, `caps_link_denied` (`fs`), `caps_env_denied` (`env`), all four of them
step 1's kinds, with the needles of §3; `process_denied` (`process`) is step 1's kind too, and needs
`std.process`'s extern bodies. `caps_net_denied` and `caps_net_resolve_denied` are step 2's.

**Compile-time rejections that must stay rejections under both backends** (they are resolution
errors, so they are backend-independent, and the asm backend must not turn one into a build):
`caps_ungranted`, `caps_escalation`, `caps_delegate_from_nothing`, `caps_bad_pattern`,
`caps_bad_domain`, `caps_bad_path_pattern`, `caps_bad_net_pattern`, asserted by
`an_ungranted_module_cannot_name_the_gated_namespace`, `a_module_cannot_delegate_more_than_it_holds`,
`an_ungranted_importer_is_named_as_the_line_to_change`, `a_pattern_containing_dotdot_is_rejected`,
`an_unknown_capability_domain_is_rejected`, `a_grant_pattern_that_is_not_a_path_is_rejected` and
`a_net_pattern_that_is_not_a_host_is_rejected`.

### Step 2: the three `net` checks

`kelp_rt$cap_check_host`, `kelp_rt$cap_check_hostport`, `kelp_rt$cap_check_port` and their call-site
variants. Unblocks, once `std.net`'s extern bodies exist: `caps_net` (`peer` binds port 0, connects,
exchanges bytes, prints `4 4 +more pong loopback bound\n`), `caps_net_denied` (denied, needle
`127.0.0.1:*`) and `caps_net_resolve_denied` (denied, before `getaddrinfo`, so no network), plus
`http_redirect_denied` once the HTTP stack builds (denied, module `std/http`).

### The gate artifact

`bench/speed/speed.kp` must build and run with identical stdout under both backends
(`collatz 131434424`, `float 57735026`, `mix 542344`, `fib 5702887`, `VALUE 78539816.062926` per
`bench/speed/results.txt`). **This slice adds nothing to its path and must add nothing**: measured,
its only gated module is `std/time`, which has no named resource, so no check is emitted for it in
the C backend either (§0 items 4 and 5). What it needs is `std/time`'s three extern bodies
(`clock_gettime`, `clock_gettime`, `nanosleep`), which is §5 item 5/10's work. The acceptance here is
the negative: after this slice, `--backend=asm bench/speed/speed.kp` must still refuse with exactly
`the 'asm' backend does not support the `extern fun std.time.now_mono_ns_raw` yet` and not with a
capability message, and once the clock bodies land both backends must print the same bytes.

### The corpus, which must keep passing unchanged

`kelpc/tests/cap_matcher.rs` is the matcher's differential control and this slice does not touch it:
`the_kelp_matcher_agrees_with_the_c_reference_on_the_whole_corpus` compares
`cap_matcher_ref.c` against the C-built `std/rt` driver, and
`the_generated_capability_check_calls_the_kelp_matcher` reads the generated C for `caps_time_rand`
and asserts the glue calls `kelp_std__rt__glob_match(patterns[i], resolved)` and
`kelp_std__rt__host_glob(pat, host)` and does not emit the C matcher. Since the asm backend calls the
same two Kelp functions, the corpus' verdicts carry over by construction, which is exactly why §1.5's
asm analogue (step 1's item 2) is the assertion this slice owes.

## 5. Risks and traps

1. **Byte-exact deny text.** The sweep compares stdout and exit status, and the project tests compare
   a `contains` needle, so nothing in the suite fails on a wrong path, a wrong location or a wrong
   `granted:` block. Two ways to get it wrong are already visible in the C: the denied string is
   `resolved` for a failed match but the argument as written when `path_normalize` failed, and the
   `granted:` block is `(nothing)` only when the count is 0 (a module can hold the domain and no
   patterns). Every part must come from the same place the C takes it from, `at` included: use
   `sources.locate(&module, span)` and nothing else. A cheaper wrong answer is to build the message
   from `panic`, which prints `panic: <msg>` with no leading location, so it cannot produce this
   text.
2. **PATH_MAX and buffer bounds.** The C has four buffers with three different sizes and two
   truncation behaviours: `char normalized[PATH_MAX + 1]` with a view of `len == cap == PATH_MAX`
   (one byte past, because `Bytes` keeps its NUL at `data[len]`), `char resolved[PATH_MAX]`,
   `char real[PATH_MAX]` and `char buf[PATH_MAX]` inside `kelp_resolve_path`, `char hostport[PATH_MAX]`
   for the host and port check, and `char head[PATH_MAX]` with an explicit
   `if (n >= sizeof head) { n = sizeof head - 1; }` clamp for the host head. `snprintf` truncates
   silently, and the lexically normalized, the resolved and the fallback strings are all matched
   *after* truncation, so a shorter buffer is a different verdict, not a longer string. Getting
   `PATH_MAX + 1` vs `PATH_MAX` wrong on the normalizer's buffer writes one byte past a stack array.
3. **Argument evaluation order and where the check sits.** In the C the check is emitted *before* the
   call and its resource expression is evaluated again for the call (`SIDE` twice, §0 item 6). The
   asm backend pushes every argument before it loads the registers, so the natural site is: push
   the arguments, emit the checks reading each resource argument's own pushed slot, then pop into
   the registers. That checks the value the call actually uses, which is **not** what the C does:
   for a resource argument whose expression has an effect, the C checks the first evaluation and
   passes the second, which is a fail-open (`fs.read_text(next_path())` can be checked against one
   path and called with another), and no fixture in the corpus exercises it (measured: every
   resource argument in the caps corpus is a literal, a local, or a pure `as_str()`). **Decision
   required before the call site is written**: either check the pushed slot (once, safer, and
   invisible to every fixture), or reproduce the C's double evaluation for oracle identity. This
   file recommends the former and records the divergence; if the owner wants byte identity first,
   the fix belongs in `codegen_c.rs` (hoist the resource argument into a temporary once), not in the
   asm's choice of site. Whichever is chosen, the check must run while the argument's storage is
   alive, so it goes before any drop of a hoisted temporary, matching the C's check-then-call-then-free
   order. `is_str_like_ty` makes this tractable: a resource argument is always one eightbyte, so its
   pushed slot is exactly one 8-byte slot and never part of the `MEMORY`-class area. If the check is
   emitted while the pushes are outstanding, the outgoing area for the 7th argument of `_hostport`
   must be carved below `rsp` (§2.1) or it overwrites the last pushed argument, and `rsp` must be
   16-byte aligned at each `call`.
4. **Errno.** The check's routine calls `realpath`, `strrchr` and `fprintf`, all of which can set
   `errno`, so it must not be emitted between a failing syscall and the panic that reports it: the
   C's `kelp_panic_errno` reads `errno` first for exactly that reason ("a failed `fprintf` is
   entitled to overwrite it"), and the asm backend's hand-written copy does the same. No check
   routine may report through `kelp_panic_errno`; the deny path is `fprintf` plus `exit(1)` with no
   errno line. The call site's own errno reporting is unaffected as long as the check runs before the
   call and after the argument expressions.
5. **Where the check is *not* emitted is also a decision, and it is the fail-open direction.** The
   C emits nothing when `checked_args` is empty, and that is correct only because those calls name no
   resource. A writer who "simplifies" the decision to "gated module, so emit something" would
   over-refuse; a writer who widens the empty case to "emit nothing even when `checked_args` is not
   empty" would emit an unchecked call for a real resource. The single source of truth is
   `stdlib::checked_args`, the same function the C backend reads. The refusal for an unimplemented
   kind must stay a *compile-time* refusal: a grant that is never checked is a grant that is not
   there.
6. **The allowlist table and its symbol.** `kelp_caps_<c_ident(module.replace('/', "_'))>_<domain>`
   is derived twice (once for the array, once at the call site); a drift between the two is a
   wrong-grant bug that links and runs. `codegen_c.rs::CapTable::symbol` is private to that file, so
   the writer should either copy the format with a pointer to it or lift the one-line function into a
   shared place. The second option edits `codegen_c.rs`, which is allowed for that writer and is the
   one change that makes drift impossible. `kelp_std__rt__*`'s spelling is separate:
   `Gen::symbol(name)` is `kelp_` plus the `c_ident` of the key (`main` excepted,
codegen_asm.rs:4435-4441), so the key `std.rt.glob_match` gives
   the `kelp_std__rt__glob_match` the C emits.
7. **`.rodata` and the linker.** The allowlist is a `.quad` array of local labels. That is sound
   today (`Type: EXEC`, §0 item 7); if the asm backend ever links `-pie`, the array needs relative
   relocations and the check would match nothing (or crash) while still building. Emit it in
   `.rodata` beside the existing byte runs, and keep the `NULL` terminator even though the count is
   passed separately, so the layout is the C's.
8. **The deny routine must exist whenever a check is emitted.** `needs_panic` is the existing model:
   a flag set by the call site, and the routine emitted only for a program that can deny. Forget the
   flag and the program links against an undefined symbol, which `ld` reports as a build failure
   (fail-closed, but not a diagnostic a fixture can assert on).
9. **A fixture in the entry module is never denied by `fs` or `process`.** The entry module holds
   `/**` for every domain implicitly (`resolve.rs`'s `caps` doc), so `caps_gated_path` is an
   admitting check (`/**` matches, the call then fails on errno), not a denial. Do not "fix" that
   fixture into a denial test; the denial pins are the projects, whose grants go to a non-entry
   module. The mirror image is that `caps_time_rand` and `bench/speed` are also entry-module
   programs, which is part of why their checks never fire.

## 6. What could not be determined

* **Whether the check's own emission can be observed without §5 item 5/10's extern bodies.** Measured
  as no: every gated call with a resource argument refuses earlier on a missing body, so the
  `.s`-level assertion of step 1 item 2 needs `std.fs.read_raw`'s body (the smallest one) to run
  end to end. The step 1 unit tests run either way.
* **The size of the asm routines.** Estimated at 250 to 400 lines total for the seven routines plus
  the call-site emission and the `.rodata` table, which is a guess, not a measurement: nothing here
  was written.
* **Whether the net checks can be deferred without hurting the gate.** They can (their fixtures are
  behind extern bodies), but the slice's own acceptance list stays one step from done until they
  land, and the corpus' host cases only run against the C backend until then.
* **Whether the C side's double evaluation is a live hole.** It is reachable by construction (a
  resource argument that is an effectful expression) and unreachable by the corpus (no such argument
  exists), and I did not search the wider ecosystem in this repository (`fpl/`, `coral/`) for one.
