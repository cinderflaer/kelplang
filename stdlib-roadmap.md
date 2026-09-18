# X standard library — roadmap

What this file is: the plan for the part of X that makes it *usable* — `io`, `process`, `net`, HTTP, collections, text, and the runtime pieces those need. It is a companion to `plan.md`, not a replacement: `plan.md` stays the living status doc and the authority for milestones, capabilities and the stdlib; `syntax&shii.md` wins on anything about how `.x` source looks, and it is locked, so every item here that wants new *syntax* says so and names the section that would have to change.

Everything asserted about the current compiler below was checked against `HEAD` (`a059dba`) by building that revision in a clean checkout (`git archive HEAD`) and running the result, because the working tree was mid-M8 while this was written. Where a claim is code-reading rather than execution, it names the file and symbol.

## 0. How the milestones here relate to `plan.md`'s

The existing line is M0–M7 + C1 (done), M8 (user-definable owned types, stretch), M9 (own backend, far future). None of those is renamed or renumbered. This file adds a parallel line, `S1`–`S7`, for stdlib-and-runtime work, and the two lines interleave rather than replace each other:

- **S1–S5 need no M8 and no M9.** They are `.x` source in `xc/std/`, new `extern` bodies in `externs.rs`, and small, local compiler changes (a field on two builtin types, a few entries in the builtins table, one codegen signature change). That is the same shape as M6.2/M6.3/M7, which is what makes them cheap.
- **M8 pays off *here*, not before here.** Monomorphized `List<T>` already shows the pattern; `Map`/`Set` and `Option`/`Result` are the two places where real generics would replace hand-rolled monomorphism.
- **M9 (own backend) is orthogonal** but has one consequence worth stating: every new `extern` body is C. S1–S7 add roughly 40–60 new C bodies, which is exactly the pile M9 stage 4 would have to re-home.

Ordering is by *dependency*, not by interest: text that can be built (S1) is a prerequisite for building an HTTP request, and the `net` capability vocabulary (S3) is a prerequisite for any `net` call succeeding at all.

| # | Milestone | Depends on | Payoff |
| --- | --- | --- | --- |
| S1 | Text you can build, numbers you can format | — | Programs that *produce* output: format a number, concatenate, sort a list |
| S2 | `io` + `process` | — | **Run a command, read its output** — build scripts, task runners, tests-that-test |
| S3 | `net` for real: gate fix, sockets, DNS, `poll` | — (gate fix is compiler-side) | Send and receive bytes over TCP on loopback |
| S4 | HTTP client | S1, S3 | **Fetch a URL and print the body** — the headline payoff |
| S5 | Maps, fs/OS breadth, real `env` bodies | S1 (sort/fmt), S2 (`fs.open`) | Config, headers, directories, metadata, temp files |
| S6 | `Option`/`Result` (language-gated) | M8's neighbourhood | Branchable failures instead of panicking natives |
| S7 | Concurrency | language features that do not exist | Nothing yet — this milestone is an analysis plus `poll` |

The shortest path to a program that does something a shell script cannot: **S1 → S2 → S3 → S4**. S2 is the first end-to-end payoff; S4 is the one the user asked for.

---

## 1. Baseline: the walls, verified

These are the facts every API below is shaped by. Each is either a probe against the HEAD binary or a named symbol; none is a guess.

### 1.1 There is no prelude — `impl` is always required

`plan.md`'s namespace model says `math`, `str`, `path`, `mem`, `io` are "prelude, always in scope, no import needed". They are not: every stdlib name needs an `impl` line, because a namespace is a module like any other. The repo's own `xc/tests/projects/caps_list/main.x` had to gain `impl "std/str"` before `str.join` would resolve; without it the error is `undefined variable 'str'`. The real distinction in the compiler is **needs a grant** vs **needs no grant** (`stdlib.rs::domain_of`), not prelude vs gated.

Consequence for this roadmap: every API sketch below is prefixed by the `impl` line it needs, and "ungated" means "no `with caps(...)` clause", not "already in scope".

### 1.2 `panic` and `assert` do not exist

`infer.rs::PRINTABLE_BUILTINS` is `&["print", "print!"]` and nothing else; there is no `panic` or `assert` builtin anywhere. Probed:

```
error: call to undefined function 'panic'
error: call to undefined function 'assert'
```

So X code cannot fail deliberately today. It can only *find* a panic by calling something that panics (`fs.read` on a missing file, `List.pop` on empty, `str.byte` out of range). `x_panic(const char*)` already exists in the generated runtime — exposing it is a builtins-table entry plus one codegen arm, which is why it is in S1.

### 1.3 Nothing that allocates can be *built* incrementally

`String` and `Bytes` are `{ unsigned char* data; size_t len; }` with **no capacity field** (`codegen_c.rs::RUNTIME`, lines ~37 and ~117), and every constructor allocates exactly once: `from_str`, `from_bytes`, `clone`, `x_bytes_alloc_raw`, `into_bytes`. There is no `push`, no `append`, no `concat`, no `reserve`. The proof is in the stdlib itself: `str.to_upper` is a hand-rolled loop over `mem.alloc(n)` + `Bytes.set(i, ...)` (`xc/std/str.x`), and so are `prefix`, `repeat`, `copy_range`, `copy_into`, and `path.join`. Every one of those would be three lines with a growable `String`.

The cost of fixing it is small *and* coupled: the `cap` field has to be added to both types in `RUNTIME`, and **three extern bodies construct the struct field-by-field** (`externs.rs`: `fs/read_raw`, `fs/list_raw`, `path/normalize_raw` — `x_Bytes b = x_bytes_alloc_raw(...); ...; b.len = at;`). Those keep compiling but would silently under-report capacity; the honest change is `b.cap = ...` in each, and the test suite would catch a missed one only if a fixture grows such a buffer. Another reason to land it as its own commit with ASAN fixtures.

### 1.4 Methods on a builtin type are compiler-provided only

Probed: writing `fun String.push(ch: char)` in user code is rejected —

```
error: 'String' is a builtin type — its methods are provided by the compiler and cannot be declared
```

So every new method on `String`/`Bytes`/`List` is **compiler work**: an entry in `infer.rs::BUILTINS` (keyed `Type#name`) or `infer.rs::list_sig`, plus a body in `codegen_c.rs::RUNTIME`. Methods on X-declared `datac`s (`File`, `Socket`, `Response`) are ordinary X and cost nothing extra. This is the single most important scheduling fact in this document: it decides which items are "cheap X code" and which are "compiler change".

### 1.5 `main` takes no arguments

`codegen_c.rs::write_function` emits `int main(void)`, and `infer.rs` rejects calling `main` at all. So `argv` is unreachable from X today. Reading it needs a runtime change: stash `argc`/`argv` in a generated global at the top of `main`, and give `process.args()` an extern that reads it. Small, but it is compiler work in S2, not stdlib work.

### 1.6 stdin is effectively unreadable

`impl "std/io"` fails with `stdlib module 'io' is not implemented yet`. The only workaround is `fs.read("/dev/stdin")` with a `/dev/**` grant, and it is not a real workaround — probed with a pipe:

```
panic: fs.read: cannot seek          # exit 1
```

It works only when stdin is a *redirected regular file* (then `fseek`/`ftell` succeed), so interactive input and pipelines are both closed off. This is the strongest argument for `io` as a streaming API rather than a set of wrappers over `fs`.

### 1.7 The `net` capability is structurally gated but unusable

Three separate facts, all from code:

1. `resolve.rs::validate_cap_pattern` requires every pattern of a *path* domain to start with `/`, and `stdlib.rs::patterns_are_paths` returns true for `net`. So `plan.md`'s own example, `impl "std/net" with caps(net = ["api.example.com:443"])`, is a compile error — the same branch that rejects `fs = ["relative/dir/*"]` (probed).
2. The call-site check picks `x_cap_check_path` for any path domain (`codegen_c.rs::cap_checked_call` → `stdlib::patterns_are_paths`), and `x_cap_check_path` runs `x_path_normalize`, which returns false for anything not absolute — then denies. So `net.connect("api.example.com", 443)` would be denied even against the entry module's implicit `/**` grant.
3. `stdlib::checked_args("net", ...)` looks only at the first `str` argument, so a port is never matched even though the documented pattern form (`host:port`) contains one.

`process` shares the vocabulary (absolute paths) but is coherent by accident: a spawn allowlist of absolute executable paths is exactly what `x_cap_check_path` + `realpath` are good at. The fix for `net` is S3's first task, and it is compiler-side (`stdlib.rs` + `resolve.rs` + one new emitted check function), not stdlib-side.

### 1.8 The entry module is unrestricted, so grant denials only bite in library modules

Probed: `impl "std/rand"` with **no** caps clause, then `rand.u64()` from `main`, compiles and runs. `resolve.rs` gives the entry module `/**` (path domains) and `*` (name domains) for every domain, and gating is checked against the *calling* module's set. So a single-file fixture proves nothing about capability enforcement: the interesting test always puts the call in an imported module, which is what the `caps_*` projects do.

### 1.9 Multi-clause grants parse

Probed: `impl "std/fs" with caps(fs = ["/tmp/**"], env = ["*"])` is accepted. A module can therefore hold several domains, which matters for `process.which`-style APIs (S2) and for `std/http` holding `net` (S4).

### 1.10 Other walls that shape the APIs

| Wall | Evidence | What it forces |
| --- | --- | --- |
| No `Option`/`Result`, no `?` | `plan.md` decision 2; `infer.rs` has no match syntax for enums | Fallible-but-expected results are error `datac`s (`Match`, and the new `Line`/`ParseInt` below); everything else panics |
| `List<T>` is monomorphic | `infer.rs::list_sig`, `codegen_c.rs::write_list` | A function can take `List<String>` or `List<i32>` but not `List<T>`; signatures must name the element type |
| No `for` loop | `plan.md` open item 27 | Every list walk is `var i: usize = 0; while i < n { ... }` — the single biggest readability cost in list code |
| No `&`/`str` fields in a `datac` | `syntax&shii.md` §1; `plan.md` open item 28 | A zero-copy view type cannot be a `datac` field; views only exist as parameters/returns |
| No recursive `datac` | `plan.md` open item 12 | No intrusive linked list, no tree with owned children; collections must be builtins |
| `String` is not `str` | `plan.md` open item 21 | `str.*` on an owned `String` needs `.as_str()` (`str.len(t.as_str())` — see `std/fs.x::list`) |
| Owned `datac` fields are allowed | M6.2 record; `List`/`String` fields work | `datac File(val fd: i32)`, `datac BufReader(..., var buf: Bytes, ...)` and `Map`-like types are all expressible without new machinery |
| `print` rejects `datac` and `List` | probed: `cannot pass a value of type 'P' to 'print'`, `... of type 'List<String>' ...` | No way to inspect a struct or a list; a debugging printer is its own item (gap 8) |
| Signed overflow wraps (`-fwrapv`) | `driver.rs:26` | `str.from_i64` for `i64::MIN` needs care (`0 - n` overflows); `parse_i64` bounds-check must not rely on overflow trapping |

---

## 2. S1 — text you can build, numbers you can format

**Deliverable:** `String`/`Bytes` grow in place, numbers convert both ways, formatted output exists, lists sort, and a program can fail deliberately.

**Gate:** none (ungated modules; `str`, `fmt`, `mem`).

```x
impl "std/str"
impl "std/fmt"

// --- growable buffers: builtin methods on builtin types (compiler work) ---
// These cannot live in std/str.x: a method on a builtin type may only come
// from the compiler (probed: "'String' is a builtin type — its methods are
// provided by the compiler and cannot be declared").
fun mut String.push(ch: char)                    // appends one byte
fun mut String.push_str(s: &str)                 // appends a borrowed view
fun mut String.reserve(n: usize)                 // grows, never shrinks
fun String.with_capacity(n: usize).R: String     // associated constructor
fun mut Bytes.append(other: &Bytes)
fun mut Bytes.push(b: u8)
fun mut String.clear()
fun mut Bytes.clear()

// --- concatenation is ordinary X (std/str.x), not a builtin ---
fun str.concat(a: &str, b: &str).R: String       // owned result; no `+` operator yet (Q4)

// --- number <-> text (X source in std/str.x) ---
fun str.from_i64(n: i64).R: String
fun str.from_u64(n: u64).R: String
fun str.from_f64(x: f64, decimals: i32).R: String    // one extern: snprintf("%.*f")
fun str.parse_i64(s: &str).R: ParseInt               // datac ParseInt(val ok: bool, val value: i64)
fun str.parse_u64(s: &str).R: ParseUint
fun str.parse_f64(s: &str).R: ParseFloat             // one extern: strtod
fun str.replace(s: &str, from: &str, to: &str).R: String
fun str.char_at(s: &str, i: usize).R: u32            // codepoint at char index i, 0 past the end
fun str.byte_index(s: &str, i: usize).R: usize       // byte offset of char i

// --- formatting (std/fmt.x, ungated, pure) ---
fun fmt.int(n: i64).R: String
fun fmt.uint(n: u64).R: String
fun fmt.float(x: f64, decimals: i32).R: String
fun fmt.hex(n: u64).R: String                        // lowercase, no prefix
fun fmt.pad_left(s: &str, width: usize, fill: char).R: String
fun fmt.pad_right(s: &str, width: usize, fill: char).R: String
fun fmt.bytes(b: Bytes).R: String                    // "1f 8b 08", for debugging binary

// --- List additions (builtin methods, monomorphized) ---
fun mut List.sort()                                  // numeric/bool: value order; String: byte order
fun mut List.reverse()
fun List.contains(v: T).R: bool                      // String elements use str.eq, not `==`
fun List.index_of(v: T).R: i64                       // -1 when absent (a datac would be nicer; see Q3)
fun List.slice(from: usize, to: usize).R: List<T>    // an owned copy
fun mut List.insert(i: usize, v: T)
fun List.remove(i: usize).R: T                       // moves the element out; panics if out of range

// --- failing deliberately (builtins; `x_panic` already exists) ---
fun panic(msg: &str)                                 // prints "panic: <msg>" to stderr, exits 1
fun assert(cond: bool)                               // panics with "assertion failed" if false
```

**Language features forced**

| Feature | Kind | Cost |
| --- | --- | --- |
| `cap` field on `x_Bytes`/`x_String` + growth | compiler (`codegen_c.rs::RUNTIME`, `infer.rs::BUILTINS`) | Small–medium: ~8 construction sites in RUNTIME, ~8 new builtins, reassign the 3 extern bodies that build the struct by field |
| `panic`/`assert` builtins | compiler (`infer.rs` builtins table + one codegen arm) | Small: the C exists (`x_panic`), only the name is missing |
| `List#sort` and friends | compiler (`list_sig` + `write_list`) | Small–medium: `sort` needs one comparison per element *kind*, which is exactly what monomorphization gives for free |
| `snprintf`/`strtod` for float text | extern | Small, same shape as `std/math`'s libm bodies |
| A new stdlib module (`std/fmt`) | plumbing | Small but easy to forget: `stdlib.rs::source` (`include_str!`), `stdlib.rs::shipped`, and the file under `xc/std/`. A module not listed in `domain_of` is ungated by default, which is what `fmt` wants |

No new syntax. `syntax&shii.md` §5 already sketches `fun push(s: mut String, ch: char)`, so the *intent* is in the authority file even though the receiver form is what will ship.

**Cost and risk**

- The real risk is the growth invariant: every constructor today NUL-terminates (`malloc(len + 1)`, `data[len] = 0`) because `str` is a C string and `String.as_str()` hands that pointer out. A growable `String` must preserve that after every `push`, which makes `push` a `realloc` + `data[len] = 0`. Miss it in one path and `as_str()` hands out a non-terminated buffer — invisible until a `printf("%s")`.
- Safety of growth against live views is free from the existing borrow rules: `val v = s.as_str(); s.push('x')` is an exclusive borrow while `v` holds a shared one. Probed with the equivalent today: `error: cannot assign to 's' while it is borrowed`. It should still get its own fixture, because it is the property that makes a growable buffer sound here at all.
- `List#sort` is where monomorphization starts to bite: a comparator per element kind means a generated sort per `T`, and `List<String>`'s comparator is `memcmp`-with-length, not `strcmp` (owned buffers are not necessarily NUL-safe mid-list — actually they are, but comparing `data` pointers with `strcmp` would stop at an embedded NUL in the *other* string only; the honest rule is length-aware compare).

**Ordering:** first. S4 needs `String` you can build; S5's `fs` breadth wants `fmt` for diagnostics; `List#sort` wants `str.eq` (shipped).

**How it is tested**

- Pure fixtures, one per area: `stdlib_build` (push/push_str/concat/reserve over a loop, printing lengths), `stdlib_fmt` (`fmt.float` precision, padding, hex, `str.from_i64` on `i64` min/max), `stdlib_parse` (`parse_i64` good/bad/overflow → datac fields), `stdlib_sort` (numeric, `String`, stable-enough order pinned by output), `list_slice_insert` (insert/remove/slice over a list), `panic_msg` and `assert_fail` (both run through `assert_fixture_panics` with golden message text).
- ASAN: every one of those added to `heap_fixtures_are_double_free_and_leak_clean`'s list in `xc/tests/compile_and_run.rs`. Growth is exactly the code path where a leak hides, so a fixture that pushes 1,000 times and prints the length must be in that list.
- Rejected fixtures: `grow_while_borrowed` (a live `as_str` blocks `push`), `push_to_val_string` (`s.push` on a `val` binding is rejected: it needs an exclusive receiver).

---

## 3. S2 — `io` and `process`: the first real programs

**Deliverable:** stdin/stdout/stderr as streams, line reading, and running programs with argv, captured output, and exit codes. This is the milestone where X stops being a calculator.

**Gate:** `io` ungated (per `plan.md` open item 6, recommendation: keep it ungated, revisit with `x.toml [caps]`); `process` gated on `process`.

```x
// ---- std/io.x — ungated ---------------------------------------------------
impl "std/mem"
impl "std/str"

// One concrete type over a file descriptor. This is the honest answer to
// "what does a reader/writer abstraction cost without traits and generics":
// there is exactly one stream type, and files, pipes, sockets and stdio are
// all the same thing at POSIX level. No vtable, no trait, no generics.
datac File(val fd: i32)

fun File.is_open().R: bool
fun File.read(max: usize).R: Bytes                 // one read; empty Bytes at EOF (no Option)
fun File.read_into(buf: mut Bytes, at: usize).R: usize   // fills buf[at..], returns bytes read, 0 at EOF
fun File.write(data: Bytes).R: usize
fun File.write_text(text: String).R: usize
fun File.flush()
fun File.close()
fun mut File.set_nonblocking(on: bool)
fun File.wait_readable(timeout_ms: u64).R: bool    // poll(2); the event-loop primitive
fun File.wait_writable(timeout_ms: u64).R: bool

fun io.stdin().R: File
fun io.stdout().R: File
fun io.stderr().R: File

datac Line(val ok: bool, val text: String)         // ok = false at EOF
fun io.read_line().R: Line                         // strips the trailing '\n'
fun io.read_all().R: Bytes
fun io.read_lines().R: List<String>
fun io.eprint(s: &str)                             // to stderr, with newline
fun io.eprint_bare(s: &str)
fun io.print(s: &str)
fun io.print_bare(s: &str)                         // `print`/`print!` already do these

// A buffered reader is an owned datac (owned fields are allowed since M6.2),
// which is also the "buffered IO" answer without generics.
datac BufReader(val file: File, var buf: Bytes, var start: usize, var end: usize)
fun io.buf_reader(f: File).R: BufReader
fun mut BufReader.read_line().R: Line
fun mut BufReader.read_all().R: Bytes

// ---- std/process.x — gated: process --------------------------------------
impl "std/mem"
impl "std/str"

// `program` is a separate argument, not `argv[0]`, because that is what makes
// the capability check possible: the grant matches one absolute path
// (`process = ["/bin/sh"]`), and argv stays a list.
// Panics: if program is not absolute, or outside the granted capability.
fun process.run(program: &str, args: List<String>).R: RunResult
datac RunResult(val code: i32, val out: Bytes, val err: Bytes)

fun process.spawn(program: &str, args: List<String>).R: Child
datac Child(val pid: i32, val stdin: File, val stdout: File, val stderr: File)
fun mut Child.wait().R: i32                        // exit code, or 128+signal
fun Child.kill(signal: i32)
fun Child.is_running().R: bool

fun process.args().R: List<String>                 // from the runtime's saved argv
fun process.arg_count().R: usize
fun process.exit(code: i32)                        // ungated in spirit: it is your own process
fun process.getpid().R: i64
```

**Language features forced**

| Feature | Kind | Cost |
| --- | --- | --- |
| `main(argc, argv)` + saved argv global | compiler (`codegen_c.rs::write_function`, RUNTIME) | Small: change the emitted signature, store the two values, one extern to read them |
| `List<String>` marshalling to `char**` | extern body | Small, but couples C to a monomorphized layout: the body iterates `x_argv->data[i].data`, so it must name `x_List__String` explicitly. That coupling is worth one comment in `externs.rs` next to the body (the file already lists `n -> x_n` for the same reason) |
| `File` over an fd | none — plain `datac` | Free; owned fields and `i32` fields both work today |
| `poll(2)` readiness | extern | Small |

**Cost and risk**

- **argv marshalling is the sharp edge.** `execvp` wants a NUL-terminated `char* const[]`; the list holds `x_String` values with their own NUL-terminated `data`. The extern must build a temporary array of pointers — `malloc` it, `free` it after the child is forked (not after `exec`, which replaces the image). A mistake here is a use-after-free or a leak that only a failing test shows, so the ASAN pass should include a spawn fixture (ASAN does not follow `exec`, but the array is freed before `exec` in the parent).
- **Gated API vs inherited fds.** `File` hands out an `int`. That is a capability value in all but name: a child process inherits whatever fds it is given, and a `File` can be passed as an argument. The plan's rule is "no stdlib function takes a capability as a value" — an fd is not the grant (the syscall that produced it checked the grant), but it is worth stating explicitly in the docs, and worth *not* adding any function that opens a path without a check.
- **`process.run`'s two-output model** (`out` and `err` captured) requires two pipes + two readers + `waitpid`; a naive implementation deadlocks when the child fills the err pipe buffer (~64 KiB) while the parent blocks reading out. The fix is `poll` over both fds (from `std/io`) or "read out to EOF, then err" for the small-output case. Recommend implementing it with `poll` and documenting the bound (`process.run` buffers whole outputs; `process.spawn` is the streaming path).
- **A `chdir` is deliberately absent from the sketch.** A process-wide `chdir` is ambient state the capability model does not describe: which directory a relative path in *another* module resolves against becomes a global variable, and it changes the meaning of every grant pattern at once. Recommend: no `process.set_cwd` in S2, and `process.spawn_in(dir, program, args)` later if a real program needs it, since a child's cwd affects only the child.
- **NixOS test environment.** `/bin/echo`, `/bin/cat`, `/bin/true` do not exist on this machine (only `/bin/sh`). Tests must use `/bin/sh` with argv, or a helper binary built by the harness itself.

**Ordering:** can be done in parallel with S1 (different files: `std/io.x`, `std/process.x`, `externs.rs` vs `RUNTIME`/`BUILTINS`), except that `io.read_line` wants `String.push` for the buffer path (so either S1 first, or `read_line` allocates from a `BufReader`'s fixed buffer and copies once with `str.copy_range`, which needs nothing new).

**How it is tested**

- New harness helper (the one real addition to `xc/tests/compile_and_run.rs`): `assert_fixture_runs_with_stdin(name, stdin_bytes, expected)` — `Command::stdin(Stdio::piped())`, write the bytes, read stdout, compare. That is the only way to test `io.read_line`/`read_all`, and it is deterministic.
- `process` tests as a project (`xc/tests/projects/process_run/`) because the gate needs a grant, and the interesting call must live in a granted *library* module (the entry module is unrestricted — see 1.8). Recipes: `/bin/sh -c "printf hi"` (stdout capture), `/bin/sh -c "exit 3"` (exit code), `/bin/sh -c "printf e >&2"` (stderr capture), and one spawn+stdin case (`/bin/sh -c "cat"` then write then close). One negative case through `assert_project_denied_at_runtime`: grant `process = ["/bin/sh"]` and call `/bin/sh` (allowed) vs a second module granted `process = ["/bin/true"]` calling `/bin/sh` (denied) — this is the first fixture pair that exercises a *process* denial, which does not exist today.
- A non-shell case, to prove argv passing without a shell: build a tiny helper `.x` fixture, and have the harness pass its absolute path to the program through a scratch file the program reads with `fs.read_text` (there is no env-var API and no argv to the *test* binary). Slightly awkward, and worth it: it is the difference between "argv works" and "argv works for `/bin/sh`".
- ASAN: any fixture that allocates around stream buffers.

---

## 4. S3 — `net` for real: the gate fix, sockets, DNS, `poll`

**Deliverable:** `net.connect`/`send`/`recv`/`close`, a listener, DNS, and a *working* `net` capability vocabulary. Prerequisite for S4 and the one item here that is mostly compiler work.

### 4.1 The capability fix (do this first)

| Layer | Today | Required |
| --- | --- | --- |
| Pattern vocabulary | `patterns_are_paths("net") == true`, so patterns must be absolute paths → `api.example.com:443` is rejected (`resolve.rs::validate_cap_pattern`) | Net patterns are `host`, `host:port`, `*.host`, `*:port`, `*`; `*` matches within one host label or the whole host; the port must be numeric or `*`; no `/` |
| Pattern validation | path rules (`..` segments, `**` legality) | a `net`-specific validator: split on the last `:`, validate host glob and port; reject `/`, empty host, non-numeric non-`*` port |
| Runtime matcher | `x_cap_check_path` → `x_path_normalize` → denies any non-absolute input, so *every* host is denied | a new `x_cap_check_hostport(domain, host, port, patterns, count, module, at)` that formats `host:port` and glob-matches with the existing `x_glob_match` (name semantics: no `*` crossing `:`; recommend `*` matching within a label and `**` unused for net, or simply "match the string with `*` not crossing `.` or `:`") |
| Checked arguments | `stdlib::checked_args("net", is_str)` → first `str` argument only | host (argument 0, a `str`) *and* the port (argument 1, a number) — which means `checked_args` cannot stay purely "which args are `str`"; it needs the port index for `net` |
| Entry module wildcard | `/**` for net (`resolve.rs`) | `*` (name vocabulary) — otherwise the entry module's own `net` calls are denied |
| `pattern_covered` | `/**`-prefix rule for path domains | for net: identical, or held `*`; `*.example.com:443` does not cover `api.example.com:443` under the conservative rule (fine — under-granting only) |

Design note to keep: **the check matches the host as the code wrote it.** No DNS resolution before matching (`getaddrinfo` happens inside the call, after the check), so a grant names a *hostname* or an *IP literal* exactly as written, and `net.connect("93.184.216.34", 80)` is not covered by a grant for `example.com`. Document that in the pattern section; it is the same "don't resolve before matching" rule `fs` already follows (which resolves *symlinks* before matching, for the opposite reason: a link is not a name the author chose).

### 4.2 The API

```x
// ---- std/net.x — gated: net ----------------------------------------------
impl "std/io"
impl "std/mem"

datac Socket(val fd: i32)

// Panics: if the host does not resolve, the connect fails, or host:port is
// outside the granted capability.
fun net.connect(host: &str, port: u16).R: Socket
// Panics: if the program is outside the granted capability, or the bind fails.
fun net.listen(port: u16).R: Listener
datac Listener(val fd: i32)

fun net.accept(l: Listener).R: Socket
fun net.close(s: Socket)
fun net.shutdown_write(s: Socket)

fun net.send(s: Socket, data: Bytes).R: usize       // may be short; `send_all` is the common case
fun net.send_all(s: Socket, data: Bytes)
fun net.send_text(s: Socket, text: &str)
fun net.recv(s: Socket, max: usize).R: Bytes        // one read, empty at EOF
fun net.recv_into(s: Socket, buf: mut Bytes, at: usize).R: usize

// Readiness, so one thread can serve many sockets (the v1 concurrency answer).
fun Socket.wait_readable(timeout_ms: u64).R: bool
fun Socket.wait_writable(timeout_ms: u64).R: bool
fun mut Socket.set_timeout(ms: u64)                 // SO_RCVTIMEO/SO_SNDTIMEO

// Testability and diagnostics
fun Listener.port().R: u16                          // getsockname: bind to 0, learn the port
fun Socket.peer_addr().R: String
fun net.resolve(host: &str).R: List<String>         // dotted-quad / v6 strings, not a getaddrinfo blob
```

**How a layered module grants (verified — and it is the mechanism S4 relies on).** A library module does **not** re-grant. `helper.x` with a plain `impl "std/fs"` (no caps clause) and a call to `fs.exists` works when its importer wrote `impl "helper" with caps(fs = ["/tmp/xc_lay/**"])`, and the runtime check uses *the caller's received patterns*: the run printed `true` for a path inside the grant and panicked for `/etc/hosts` with `granted: /tmp/xc_lay/**`. Two consequences: `std/http` can write a plain `impl "std/net"` and be governed entirely by the pattern its importer gave it (the client carries no policy of its own), and the delegation/no-escalation rule only applies to clauses a module actually writes. The denial printed the *symlink-resolved* path (`/etc/hosts` → `/nix/store/...`), which S3's tests must expect: assert on the module name or the granted pattern, never on the path as written.

**Ordering inside S3:** (1) the capability fix with a *test that fails before it* — a project whose grant is `net = ["127.0.0.1:*"]` and whose call is `net.connect("127.0.0.1", p)`; (2) `connect`/`send`/`recv`/`close` + the loopback self-test; (3) `listen`/`accept`/`port`; (4) `poll` readiness; (5) `resolve` and timeouts. Each is independently testable and each is a separate commit.

**Cost and risk**

- The `recv` model is the crux: `recv(s, max)` returns a fresh fixed-size `Bytes`, so "read until Content-Length" means either a loop that appends (needs S1's growable `Bytes`) or `recv_into(buf, at)` into a **pre-sized** buffer. `recv_into` is the API that makes known-length bodies work without growth, and it is why it is in the sketch next to `recv`. Chunked transfer encoding has no such bound and is deferred with it (S4).
- `poll` over `int` fds is easy; `poll` over *owned* `Socket` values is where a real event loop gets awkward: a `List<Socket>` holding owned fds is fine, but a readiness *set* that the user mutates while iterating is the classic borrow-checker fight (`for` does not exist, and `val s = list.get(i)` borrows the list for the block). Expect the first real event-loop program to be ugly; that is a language-shape cost, not a stdlib bug, and it belongs in the open questions (Q10).
- DNS: `getaddrinfo` allocates a result list that must be freed; recommend resolving straight into a `List<String>` and `freeaddrinfo`-ing inside the extern. Timeouts on DNS are not controllable (blocking `getaddrinfo`) — document it, and note that a resolver timeout needs a thread or `c-ares`, i.e. not v1.
- IPv6: `connect` should try the `getaddrinfo` list in order and return the first that connects, so `::1` and `127.0.0.1` both work. Cheap, and it prevents a class of "works on my machine".

**How it is tested (no internet, no fixed ports, no flakiness)**

- Single-process loopback, in one `.x` program: `net.listen(0)` → `Listener.port()` → `net.connect("127.0.0.1", port)` (completes from the backlog) → `net.accept(listener)` → server writes → client reads → assert bytes and both directions of `send`/`recv`. Deterministic, no external process, no fixed port.
- Grant shape for that test: `impl "std/net" with caps(net = ["127.0.0.1:*"])` — which is *only expressible after the fix in 4.1*, so the fixture doubles as the regression test for the vocabulary (`assert_project_runs`), plus a `caps_net_denied` project pointing at `127.0.0.2:*/`... more precisely: a grant for `127.0.0.1:*` and a call to `net.connect("localhost", port)` → denied, because the checked string is the host *as written*.
- For a *server* test (S5's HTTP server), the harness spawns the compiled binary in the background with `Stdio::piped()`, then connects with bounded retry: try `net.connect` up to ~200 times with `time.sleep_ms(5)` between attempts (~1 s ceiling), and `kill()` the child in a `Drop`-ish guard so a failed assertion cannot leak a process. Never bind a fixed port (collision → flake): the server prints its port on stdout and the client reads it, or the server takes the port as argv (but that needs S2's argv).
- Timeout tests use `Socket.set_timeout(100)` + a peer that never reads; assert the call returns/panics within a bounded time, not on a specific errno string.

---

## 5. S4 — HTTP client (the payoff)

**Deliverable:** `http.get("http://host/path")` returns a parsed response; redirects, timeouts and header lookup work; the body is a `Bytes`.

**Gate:** `net` (the module holds it, delegated by the importer), plus `process` only if the shell-out fallback in 5.1 is used.

Caveat on the line below: `caps(net = ["api.example.com:443"])` is the *intended* spelling and is rejected by today's compiler — `net` patterns go through the path validator, so this example only compiles after S3.1. The host is matched exactly as the code writes it, so the client's grant and the URL in the program have to name the same host string.

```x
// ---- std/http.x — ungated module that holds `net` ------------------------
// The importer delegates: impl "std/http" with caps(net = ["api.example.com:443"])
impl "std/net"
impl "std/str"
impl "std/fmt"

datac Url(val scheme: String, val host: String, val port: u16, val path: String)
// Panics: if the URL has no scheme or host, or the scheme is not supported.
fun http.parse_url(s: &str).R: Url

datac Header(val name: String, val value: String)
datac Response(val status: i32, val headers: List<Header>, val body: Bytes)
fun Response.header(name: &str).R: &str            // "" when absent (no Option)
fun Response.has_header(name: &str).R: bool
fun Response.is_ok().R: bool                       // 200..299
fun Response.text().R: String                      // panics on invalid utf-8

datac Request(val method: String, val url: String, val headers: List<Header>, val body: Bytes)
datac Client(val timeout_ms: u64, val max_redirects: i32, val follow_redirects: bool)

// Panics: if the connection fails, the capability denies the host, or the
// response is malformed. Follows up to `max_redirects` redirects.
fun Client.send(req: Request).R: Response

fun http.get(url: &str).R: Response                // the one-liner, default client
fun http.post_text(url: &str, body: &str, content_type: &str).R: Response
fun http.default_client().R: Client
```

What is deliberately *not* in S4: `https` (see 5.1), chunked transfer encoding, gzip, HTTP/2, cookies, connection pooling, proxy support, and any server.

**Language features forced**

| Feature | Kind | Cost |
| --- | --- | --- |
| Growable `String` (request building) | S1 | — (dependency) |
| `str.from_i64` for `Content-Length` | S1 | — |
| `str.index_of`/`split`/`trim_start` for parsing (shipped in C1/M6.3) | none | — |
| `List<Header>` as a `datac` field (owned field, shipped) | none | — |
| Nothing else | — | **S4 adds no compiler work at all**, which is the point of doing S1 and S3 first |

**Cost and risk**

- Parsing without a view type is the honest cost: `str.substring`/`trim_start` are *suffix* views, but a header line's *name* prefix is not expressible as a view (see `plan.md` open item 22). So each header parse allocates two `String`s, and a 1 KB response with 20 headers allocates ~40 times. Acceptable at v1 scale, and the reason a length-carrying view is the highest-value *language* item after generics (Q9).
- Body reading must handle three cases: `Content-Length` (use `recv_into` into a pre-sized buffer), `Connection: close` (read until EOF, appending — needs S1 growth), and `Transfer-Encoding: chunked` (deferred; return a clear panic naming chunked as unsupported). A silent wrong answer here is the worst outcome, so "unsupported" must be an explicit panic, not an empty body.
- Redirect handling is where the capability model gets interesting and *good*: a redirect to another host is a request the code did not name, so it must be denied unless the grant covers the new host. Implement redirects by re-running the same connect path (so the check applies again) rather than by reusing the socket, and document that a redirect to an ungranted host panics — that is the feature working, not a limitation.
- `Connection: keep-alive` and pooling: skip. One request, one socket, closed after. Simple, correct, slow.

**How it is tested**

- A canned server in the same program: `net.listen(0)` → `net.connect` → `accept` → write a fixed `HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello\r\n` → then run `http.get` against `127.0.0.1:port` and assert status, header lookup (`Content-Type` present/absent), `text()`, and `body.len()`. One process, deterministic, no internet.
- A second fixture for the failure paths: a 301 to a second loopback port that the grant *does not* cover → the run is denied (`assert_project_denied_at_runtime`), and a `404` response → `is_ok()` false but a normal return (it is not an error, it is a response).
- A third for timeouts: server accepts and then never writes; client with `timeout_ms = 100`; assert the panic message contains the timeout, bounded by `time.sleep_ms` in the harness (`caps(time)` for the assertion program).
- The harness runs the whole thing with **no network access assumed**: if a test ever needs the internet it is a test that will flake, so the rule is "loopback only", including for the redirect case.
- ASAN: the request builder and the header list are the new allocation sites; put the redirect and body fixtures in the ASAN list.

### 5.1 The `https` question (make it a decision, not a surprise)

The user asked for HTTP requests; almost every real URL is `https`. There is no TLS in X, and hand-rolling one is out of the question. Three options, with the honest trade:

1. **Link the system TLS library** (`-lssl -lcrypto`, or `-ltls`/`libressl` where present). `std/tls`'s externs are the only OpenSSL-touching code, exactly like `std/math` and libm today. Cost: `driver.rs` gains a link flag, the build acquires a real dependency, and the "no third-party deps" rule of `plan.md` decision ("no `[stdlib]` section, no registry, no third-party deps in v1") becomes "no third-party deps except the system's TLS". **Recommended**, because the alternative is an HTTP client that cannot fetch anything on today's web.
2. **`http` only, and document it.** Zero new dependencies; `https` URLs are rejected with a message pointing at option 1 or 3. Cheapest, and honest.
3. **Shell out to a `curl`-shaped tool** through `process.run` (gated), i.e. a `http.get_via_tool()` fallback. Ten lines, works for `https`, but it moves the network capability to the *tool's* ambient authority, which quietly defeats the headline feature. Recommended only as a documented escape hatch, never as `http.get`'s implementation.

Recommendation: ship S4 as `http`-only, and treat TLS as its own milestone (**S4b**) that starts with the dependency decision above. Do not let `https` block the client.

---

## 6. S5 — maps, fs/OS breadth, and the real `env`

**Deliverable:** a keyed container, plus the filesystem/OS functions that are now cheap because C1 landed, plus `env` bodies that are not stubs.

**Gate:** none for `Map` (ungated, pure), `fs` for the filesystem items, `env` for the environment items.

```x
// ---- Map (compiler builtin, `Ty::Map(Box<Ty>)`, String keys) -------------
// Same monomorphization as List: one C struct + one function set per value
// type. Keys are String because hashing a key needs the key's bytes, and a
// generic key would need real generics (M8).
fun Map.new().R: Map<V>                       // element type from the binding: var m: Map<i64> = Map.new()
fun Map.len().R: usize
fun Map.is_empty().R: bool
fun Map.has(key: &str).R: bool
fun Map.get(key: &str).R: V                   // a copy when V is Copy...
fun Map.get(key: &str).R: &V                  // ...a borrow when V owns memory; panics when absent
fun Map.get_or(key: &str, fallback: V).R: V   // V by value: scalar or a move-in
fun mut Map.insert(key: &str, v: V)           // copies the key, moves the value in
fun mut Map.remove(key: &str).R: V            // moves the value out; panics when absent
fun mut Map.clear()
fun Map.keys().R: List<String>                // insertion order, not hash order
fun Map.values().R: List<V>

// ---- fs breadth (X source over one new extern each) ----------------------
// `File` is std/io's type, so this module also needs `impl "std/io"`.
impl "std/io"
impl "std/fs" with caps(fs = ["/var/lib/app/**"])

fun fs.mkdir_all(path: &str)                  // split on '/', mkdir each prefix
fun fs.remove_all(path: &str)                 // recursive; already written by hand in tests/projects/caps_list
fun fs.walk(path: &str).R: List<String>       // recursive names; joins with path.join
fun fs.copy(from: &str, to: &str)             // one extern: sendfile(2)
fun fs.touch(path: &str)
fun fs.open(path: &str, mode: &str).R: File    // "r", "w", "a", "rw"; from S2's File
fun fs.read_lines(path: &str).R: List<String>  // split of read_text; no extern
fun fs.temp_path(prefix: &str).R: String       // mkstemp; the file is created
datac Metadata(val size: u64, val mtime: i64, val mode: u32, val is_dir: bool, val is_file: bool)
fun fs.metadata(path: &str).R: Metadata

// ---- path polish ---------------------------------------------------------
fun path.stem(s: &str).R: &str
fun path.has_extension(s: &str, ext: &str).R: bool
fun path.join_all(parts: List<String>).R: String
fun path.split(s: &str).R: List<String>

// ---- env, real bodies (gated: env, variable-name allowlist) -------------
fun env.has(name: &str).R: bool
fun env.get(name: &str).R: String              // "" when unset (no Option)
fun env.set(name: &str, value: &str)
fun env.unset(name: &str)
fun env.names().R: List<String>                // what is in the environment, as names
```

**Notes and costs**

- `Map` is the second builtin generic, and it is deliberately the cheap one: `String` keys only, value type monomorphized, FNV-1a over the key's bytes in the runtime, open addressing with tombstones for `remove`. Budget: comparable to C1 — a `Ty::Map`, a `map_sig` next to `list_sig`, a `write_map` next to `write_list`, a `Map#` prefix in the call-naming switch, and drop (free each key, then each value if it needs it, then the table). The one thing that is *not* free: `Map<Map<i32>>` and `Map<List<i32>>` are as impossible as `List<List<i32>>` (`is_list_element`-style checks need a `is_map_value` sibling).
- `env.names()` is a real capability question: listing the environment is reading all of it, which a grant of `["PATH"]` should not permit. Recommend either `env.names()` requires the `*` pattern (i.e. it is the one function whose check is "must hold a wildcard") or it is left out. Recommend leaving it out of v1 and keeping the allowlist honest.
- `fs.walk` returns *paths*, so it needs `path.join` per entry — 20 lines of X, no extern, and it inherits `fs.list`'s sorted order (which is what makes a test deterministic).
- `fs.remove_all` is already proven in the repo: `xc/tests/projects/caps_list/main.x::clear` does a recursive remove in ~15 X lines using `fs.list` + `fs.is_dir` + `path_of` (which is a hand-rolled `join` because `str.concat` did not exist). After S1, that is `fs.remove_all` in 8 lines.
- `fs.copy` is the only place a syscall is genuinely nicer than a read/write loop (`sendfile`); `fs.read` + `fs.write` would double the memory for a 20 GiB file, and the repo already has a benchmark that writes 20 GiB.

**Ordering:** `Map` is independent and can go in parallel. The fs half depends on S1 (for `str.concat`-free joins) and on S2 (for `File`), but `mkdir_all`/`remove_all`/`walk` depend on nothing new at all — they could ship today.

**How it is tested**

- `Map`: fixture with scalar values (`Map<i64>`) and owned values (`Map<String>`), asserting insert/get/overwrite/remove/keys order/len and a missing-key panic (`assert_fixture_panics`); an ASAN entry for the `Map<String>` case (keys are copies, values are moves).
- fs: a project under `xc/tests/projects/` with a grant on a scratch directory that `mkdir_all`s a three-deep path, writes files, `walk`s it, checks `metadata`, copies, `remove_all`s it, and asserts the directory is gone — the `caps_list` pattern, extended. Cleanup in the same program (so runs are repeatable) and a harness-side `remove_dir_all` as a belt-and-braces.
- env: a project granted `env = ["PATH"]` asserting `env.has("PATH")` and `env.get("PATH")` are non-empty, plus a denial for `HOME` (`assert_project_denied_at_runtime`) — the *first* real `env` behaviour test, since `caps_env` today only proves the stub panics.

---

## 7. S6 — `Option`/`Result`, when tagged unions exist

**Deliverable (when it lands):** branchable fallible APIs; error `datac`s replaced by `Result<T, E>`.

This is not stdlib work, and pretending otherwise would be the mistake `plan.md` decision 2 already warns about. The honest content of S6 is:

- **What it needs, concretely.** A representation for a tagged union (`datac` cannot express it: every field exists, there is no tag), a way to *destroy* a value and get another out without leaking (so `match`/`when` patterns on constructors, not just booleans), and generics for the payload. That is three language features, each with parser/type/borrowck/codegen surface. It is M8's neighbourhood, not a stdlib milestone.
- **What is genuinely painful today, measured.** Not much: `Match`, `ParseInt`, `Line`, `RunResult` and `Response` cover the branchable cases, and `plan.md` decision 2's convention works. The cost shows up in *composition* — no `?`, no `if let`, no `map` — so nested fallible calls become nested `if`s. That is an ergonomics cost, not a correctness one.
- **The cheap thing to do now (recommended).** Standardize the error-datac *convention* so the eventual migration is mechanical: every error datac has `ok` first, a value field named for its type (`value`, `text`, `bytes`, `at`), and a `message: String` when the failure is textual; document it in the stdlib naming section; and add `error.code().R: i32` (errno) plus `error.message()`. Panics should print errno and `strerror`, which is a one-line change to `x_panic` call sites and a large diagnostic win for `fs`/`net`/`process` (Q11).
- **Migration shape, for later.** Each error datac becomes a `Result<T, E>` with the same field names inside `Ok`/`Err`, and the old name stays as a wrapper — which only works if the field names were standardized now. That is the whole reason to do the cheap part early.

---

## 8. S7 — concurrency: what is possible now, and what is not

**Deliverable: an answer, plus `poll`.** No threads, no async, and the reasons are language-shaped rather than runtime-shaped.

What works today and needs no new language feature:

1. **Multi-process.** `process.spawn` (S2) gives real parallelism and real isolation: a build runner, a parallel test runner, a pipeline. This is the honest v1 answer to "concurrency" and it is a shell-script capability, which is what the roadmap is for.
2. **Single-threaded event loops.** `poll(2)` readiness on `File`/`Socket` (S2/S3) plus `time.sleep_ms` is enough for a server that handles many connections in one thread, and it is what an HTTP server needs at this scale.
3. **`atexit` + scope drops** remain the only cleanup points; there is no `defer`, no cancellation, no unwinding (`x_panic` calls `exit(1)`, so a panic in one "task" takes the process).

What a thread would need, and why each piece is missing:

| Need | Why v1 cannot express it |
| --- | --- |
| A function value to run | No first-class functions anywhere in the parser; `thread.spawn` would need a *name* resolved at compile time plus monomorphized stubs, i.e. a special case, not a feature |
| Passing owned data into the thread | Parameters are always borrows in v1 (`plan.md` open item 1: there is no move-taking parameter spelling), so a spawned thread cannot be handed a `Bytes` to own; it could only borrow something whose lifetime the borrow checker cannot relate to the thread |
| Preventing `&mut` sharing across threads | The borrow checker has lexical, function-local lifetimes (C1's refinements made temporaries shorter, not lifetimes longer). A `Send`-like discipline would have to be a new rule about *what may cross a spawn boundary* |
| A stack and cleanup for the child thread | Needs a per-thread frame story, and v1's drops are keyed to lexical blocks in one function |

Conclusion: **no threads until function values *and* move-taking parameters exist**, which is a language milestone (call it M10's neighbourhood) and not schedulable from the stdlib side. `async`/`await` needs those plus a state-machine transform in codegen and a runtime; it is further away still. The recommendation is to write S7 down as "processes + `poll`", ship the `poll` API, and revisit when a real program demands more.

---

## 9. Language features these milestones force

Extending `plan.md`'s table shape. "Where" is the compiler file that would change; "std" means stdlib-only work.

| Feature | Needed by | Kind | Cost | Where |
| --- | --- | --- | --- | --- |
| Growable `Bytes`/`String` (`cap` + `push`/`append`/`reserve`) | S1, S4, S5 | compiler builtin | Small–medium | `codegen_c.rs::RUNTIME`, `infer.rs::BUILTINS`, 3 `externs.rs` bodies |
| `panic(msg)` / `assert(cond)` | S1, every program with a deliberate failure | compiler builtin | Small | `infer.rs` builtins, `x_panic` exists |
| `List#sort`/`reverse`/`slice`/`insert`/`remove` | S1, S5, deterministic output | compiler builtin | Small–medium | `infer.rs::list_sig`, `codegen_c.rs::write_list` |
| `main(argc, argv)` + saved argv | S2 (`process.args`) | compiler codegen | Small | `codegen_c.rs::write_function`, RUNTIME |
| `File` over an fd | S2, S3, S4, S5 | none (plain `datac`) | Free | `xc/std/io.x` |
| `Map<V>` with String keys | S5, headers/config/counters | compiler builtin generic | Medium (≈ C1) | `types.rs::Ty::Map`, `infer.rs::map_sig`, `codegen_c.rs::write_map` |
| net capability vocabulary (`host[:port]`, matcher, checked args) | S3, S4 | compiler (capability layer) | Medium | `stdlib.rs`, `resolve.rs::validate_cap_pattern`/`pattern_covered`, `codegen_c.rs` (new `x_cap_check_hostport`) |
| `poll` readiness | S3, S5 (single-threaded servers) | extern only | Small | `externs.rs` |
| errno in panic messages + `error.code()` | S2/S5 diagnostics | runtime + builtin | Small | RUNTIME `x_panic`, externs |
| TLS | S4b (`https`) | dependency decision, then externs | Large-ish (a real link dependency) | `driver.rs`, `std/tls.x` |
| `Option<T>`/`Result<T,E>` | S6 | language: tags, patterns, generics | Large | lexer/parser/infer/tir/codegen — M8's neighbourhood |
| Function values + move-taking parameters | S7 (threads) | language | Large | parser/types/borrowck/codegen |
| A length-carrying borrowed view (`&[u8]`-shaped) | zero-copy parsing (S4, S5), arena allocation, `String.slice` | language + syntax | Large | `types.rs`, `syntax&shii.md` §1/§13 |
| `for` loop / iteration over a sequence | every list walk in S1–S5 | syntax + desugar | Small | lexer/parser, and `syntax&shii.md` §10 |

Two of these need the authority file: a byte-view type and a `for` loop. Both are flagged in Q9/Q10 with the section that would have to change.

---

## 10. Testing: how each item is verified in this repo

The harness that exists (`xc/tests/compile_and_run.rs`, 109 tests) is: compile a fixture with the real `xc` binary, run the result, diff stdout (`assert_compiles_and_runs`); assert a compile error (`assert_rejected`, `assert_project_rejected`); assert a runtime panic's stderr (`assert_fixture_panics`); for multi-file/granted work, a project directory plus `assert_project_runs` / `assert_project_denied_at_runtime`; and `heap_fixtures_are_double_free_and_leak_clean`, which rebuilds every listed fixture with `-fsanitize=address` and runs it.

Additions this roadmap needs, in order of cost:

1. **`assert_fixture_runs_with_stdin(name, stdin, expected)`** — `Command::output` with `stdin(Stdio::piped())`. ~25 lines, and the only way to test `io`. Used by every `io` fixture.
2. **`assert_project_denied_at_runtime` reuse for `process`/`net`/`env`** — the helper already exists; the work is writing the projects (two modules each: the entry module, and a library module that received a grant).
3. **A background-server helper** (`spawn helper binary, poll it with bounded retry, kill on drop`) — only needed for the HTTP server in S5. The client tests in S4 do not need it: one process can listen, connect to itself, accept, serve a canned response and then be the client.
4. **A golden-panic helper** (`assert_panic_message_matches`) — `plan.md`'s "golden files for every `Panics:` message" idea, cheaply done as a `needle` assertion (already the shape of `assert_fixture_panics`). Do this as messages are written, not later.

Per domain, the recipe that stays deterministic:

| Domain | Recipe | Never |
| --- | --- | --- |
| `str`/`fmt`/`List` (S1) | Pure fixtures, exact stdout, ASAN for the allocating ones | Locale-dependent float text (pin `%.*f` behaviour, not `NUMBER` grouping) |
| `io` (S2) | Fixture + harness-fed stdin; `io.read_line` over a 3-line input including one without a trailing newline | Reading the real terminal |
| `process` (S2) | `/bin/sh` with argv (`["/bin/sh", "-c", "printf hi"]`), exit-code case, stderr case; one spawn+stdin case; denial case via two modules | `/bin/echo`, `/usr/bin/*` (absent on NixOS — verified), shell *strings* in the API |
| `net` (S3) | One-process loopback: `listen(0)` → `port()` → `connect` → `accept` → exchange; grant `127.0.0.1:*` | Fixed ports, external hosts, real DNS |
| `http` (S4) | Same loopback with a canned raw response; redirect-to-ungranted-host as a denial; timeout with a silent peer | The internet |
| `fs` (S5) | Project + scratch dir created and removed by the harness (`caps_scratch` exists), and by the program itself where possible | Writing outside the temp dir |
| `env` (S5) | Grant `["PATH"]`, assert non-empty; denial for `HOME` | Mutating variables the harness needs |
| `Map` (S5) | Fixture with scalar and owned values; missing-key panic; ASAN | Asserting hash *order* — assert insertion order of `keys()` |

---

## 11. What would make X genuinely useful for real programs

Ranked by how much pain each absence causes, with the shortest path to each. "Pain" here is the question "could I write a program I would actually want to run?"

1. **Text you can build** (`String.push`/`concat`, `str.from_i64`, `fmt.float`). Every program that produces output hits this, and there is no workaround that is not a byte loop. Pain: *high, constant*. Path: S1, ~2 days. This is the single highest-value item in the file.
2. **stdin/stdout/stderr as streams** (`io`). Reading input is impossible today (probed: `/dev/stdin` panics on a pipe). stdin is how programs get input; a language without it cannot be used as a filter or interactively. Pain: *high, structural*. Path: S2, ~2 days. Combined with (1), X can write a real CLI.
3. **Running programs** (`process.run`/`spawn` + argv). Automating anything real means calling other tools; it is also the only concurrency available. Pain: *high*. Path: S2, ~3 days including the `main(argc, argv)` change.
4. **`panic`/`assert`.** A program that cannot fail deliberately cannot be tested or safely guard an invariant, and its error messages are whatever a stdlib panic says. Pain: *moderate, embarrassing*. Path: S1, an afternoon.
5. **The `net` capability fix.** Not a feature gap but a *headline-feature* gap: the documented net pattern form is rejected and every host would be denied. Until this is fixed, the capability system has a domain that cannot be used, which is worse than one that does not exist. Pain: *high for credibility, zero for the current fixtures*. Path: S3.1, ~1 day.
6. **TCP** (`net.connect`/`send`/`recv`). Client-only networking unlocks APIs, sockets, and the HTTP client. Pain: *high, blocked on 5*. Path: S3, ~3 days.
7. **HTTP client.** The user's stated goal. Pain: *high in intent, and it is the payoff*. Path: S4, ~4 days after S1+S3 — most of the work is parsing and the three body-length cases.
8. **A keyed container** (`Map<String,V>`). Today a lookup is a linear scan over `List<String>`; headers, config, counters and caches all want a map. Pain: *moderate*. Path: S5, ~3 days (the second monomorphized builtin).
9. **Being able to *see* your data.** `print` rejects a `datac` and rejects a `List` (both probed). There is no debug printer, no inspection, and nothing in the shipped stdlib substitutes for one (`fmt.bytes` as proposed in S1 would be the first step). Pain: *moderate, and it makes debugging everything else worse*. Path: a compiler-generated `x_debug_<Type>` per datac plus `fmt.debug(value)` — doable as builtins; ~2 days; not in the milestone table above on purpose, because it is the most obviously missing *developer* feature and belongs in S1 if S1 has room.
10. **`fs` breadth** (`mkdir_all`, `remove_all`, `walk`, metadata, temp files). All cheap now that `List` exists — mostly an afternoon of X. Pain: *low, high frequency*. Path: S5; the first three could ship today.
11. **`env`**. The stub is gated and real, so the *shape* is proven; the bodies are one extern each. Pain: *low*. Path: S5, half a day.
12. **`Option`/`Result` and views and threads.** Real, deferred, and not what is blocking anything. Pain: *low today, and it will rise*.

What is a toy gap versus a real one, bluntly:

- **Real:** 1, 2, 3, 6 (and 5 as a precondition for 6). Without these, X cannot replace a shell script, and a shell script is the lowest bar there is.
- **Real but not blocking:** 8, 9, 10, 11.
- **Tolerable:** 12 — the error-datac convention genuinely works; it is verbose, not broken. The `for`-loop gap is the same shape: ugly (`while` + index), not impossible.
- **Toy left alone on purpose:** a big stdlib. `json`, `regex`, `log`, `cli`, `test` frameworks are all downstream of views/generics/collections and would each be a rewrite once those land. Write them after S5, not before.

---

## 12. Open questions for the owner

Each with the trade-off and a recommendation. (Continues `plan.md`'s numbering in spirit; these are new items, not amendments.)

**Q1. TLS for `https`: link the system library, or ship `http`-only?** Trade-off: linking OpenSSL makes `https` work and breaks "no third-party deps"; not linking means an HTTP client that cannot fetch most URLs. Recommendation: ship `http`-only in S4, decide TLS as S4b with option 1 (system library, externs only) as the default. Do not shell out to `curl` as the implementation — it moves the capability to ambient authority.

**Q2. Does `io` stay ungated?** `plan.md` open item 6 recommends ungated, and WASI gates stdin. Trade-off: gating stdin makes a filter program need a grant (`impl "std/io" with caps(io = ["stdin"])`), which is purity; ungating it keeps the common case free and matches "writing your own stdout is not authority". Recommendation: ungated, and add the `io` domain to `stdlib::DOMAINS` only when `x.toml [caps]` can restrict `main` (open item 18), so the *option* exists without the ceremony.

**Q3. Is argv `process.args()` (ungated) or `env.args()` (gated)?** `plan.md` puts it under `env`. Trade-off: argv is your own invocation, like `getpid`, which the plan explicitly calls not-a-capability; putting it under `env` means a program that wants its arguments needs an environment grant, which conflates two authorities. Recommendation: `process.args()`/`process.arg_count()`, ungated, and drop `env.args` from the sketch.

**Q4. Is string concatenation an operator (`+` / `+=`) or a function (`str.concat`)?** Trade-off: `+` reads better and hides an allocation; a function makes the allocation visible and needs no syntax change. `syntax&shii.md` §10 has no such operator, so an operator means editing the authority file. Recommendation: `str.concat(a, b)` + `String.push_str` in S1 (no syntax change); revisit `+` as a deliberate syntax decision later, and never make `+=` on a `String` implicit (it would be the only operator that allocates).

**Q5. Multiple `with caps(...)` clauses on one `impl` line.** Verified to parse (`caps(fs = [...], env = [...])`). Trade-off: none, except that it is undocumented. Recommendation: document it in the capability section as legal, since `process.which`-shaped APIs need two domains and a single-domain-only rule would force a fake module boundary.

**Q6. `Map.get` on a missing key: panic, or a `has`+`get` pair, or an error datac?** Trade-off: panicking keeps `Map<String,Header>` lookups one line and matches `List.get`; a `has` check before every lookup is verbose; an error datac per lookup is unusable in an expression. Recommendation: `get` panics, `has` exists, `get_or(key, default)` covers the common fallback — the same shape as `List.pop`.

**Q7. `Set` and `Map` keys.** Trade-off: a `Set` as a separate builtin is a third monomorphized generic for a container that `Map<bool>` covers; generic keys need M8. Recommendation: no `Set` in S5; document `Map<bool>` as the workaround, and reconsider when generics land.

**Q8. Does `List` gain `&T`/`str` elements once a view type exists?** Trade-off: `List<&str>` is what a zero-copy HTTP header list wants; but an element outliving its borrow is exactly what `is_list_element` refuses, and allowing it needs a lifetime story finer than "lexical, function-local". Recommendation: no — keep the refusal, and solve zero-copy parsing with a view type (Q9) rather than with borrowed elements.

**Q9. A length-carrying borrowed view (`&[u8]` / `Bytes.view`): v1.1 or later?** Trade-off: it is the difference between a parser that copies every token and one that does not (HTTP headers, `str.split` into views, `String.slice`, `path.stem`, arena-friendly APIs), and it is what `plan.md` open item 22 says would collapse `path.parent`/`str.trim`'s allocations. Cost: a new type with a pointer+length representation, a borrow rule that allows it as a field/param/return, and a syntax decision (`syntax&shii.md` §1 says a `datac` holds owned data only; §13 says references do not appear in field types). Recommendation: schedule it as the *first* language feature after S5, because S4 and S5 both pay for its absence, and decide the spelling with the authority file (the cheapest spelling that does not touch §1: a builtin `View` type like `Bytes`, with `View.of(b, from, to)` constructing it and functions taking/returning it — no `&` in the grammar).

**Q10. Is a `for` loop worth its own milestone?** Trade-off: every list walk in S1–S5 is a 4-line `while` with an index; `for x in list { }` would remove the index and the `usize` type in most loops. Cost: lexer + parser + a desugar to `while` (the pattern `when` and compound assignment already use), and an update to `syntax&shii.md` §10, which lists the control-flow forms and does not include `for`. Recommendation: yes, but *tiny* — a single-purpose commit, desugared in the parser exactly like `when`, with the desugar pinned by a fixture; and it should land *after* S1 so that the first list-heavy programs show whether it is enough or whether iteration needs a real protocol (`iter`/`next`) that would be far more expensive.

**Q11. Do panics carry errno and `strerror`?** Trade-off: adding `errno` to `fs`/`net`/`process` panic messages makes every failure diagnosable and is a small runtime change; it also makes panic text less stable for golden tests. Recommendation: yes, but as a two-line addition (`x_panic_errno(const char* msg)`) used by the syscall externs, keeping the message prefix stable and the errno detail on a second line.

**Q12. `main(argc, argv)` versus a `pd` global for arguments?** Trade-off: a generated `pd val argv: List<String>` would need no change to `main`'s signature, but `.x` `main` cannot take passdown parameters (M5.2's restriction: `main` is `int main(void)`) — so this does not actually work, and the runtime has to stash `argc`/`argv` either way. Recommendation: change the emitted `main` signature to `int main(int argc, char** argv)`, store them in generated globals, and expose them only through `process.args()`.

**Q13. Should streaming reads (`recv_into`/`read_into`) be the primary API, with the allocating form as sugar?** Trade-off: the allocating form (`recv(s, max) -> Bytes`) is the one people will write first and the one that cannot express "read exactly Content-Length bytes" without growth; `*_into` needs a buffer the caller sized, which requires knowing the length. Recommendation: both, with `*_into` documented as the correct choice for known-length bodies and the difference spelled out in `std/net.x`'s doc comment. Do not add a `read_to_end()` that grows until S1's growable `Bytes` exists — that is precisely its use case.

**Q14. Buffered IO: a `BufReader` datac in the stdlib, or unbuffered reads only?** Trade-off: unbuffered `read_line` over a pipe is one syscall per byte (unusable); buffering needs a buffer, i.e. an owned `Bytes` field in a `datac`, which is legal but is *state* the user must hold and pass. Recommendation: yes, `datac BufReader` as sketched, and make `io.read_lines()` the convenience that hides it.

**Q15. Unicode scope.** Trade-off: real case mapping and grapheme clusters need tables (megabytes) and a text-segmentation algorithm; `char`-level handling (codepoints) is a table-free decode of the same UTF-8 rules `x_utf8_valid` already implements. Recommendation: codepoint-level (`str.char_at`, `str.byte_index`, validated `String`), ASCII-only case operations (documented), no grapheme awareness, no case folding. `String.char_count` stays "non-continuation bytes" and the doc comment says so.

**Q16. `process.run`'s capture model.** Trade-off: whole-output capture is what people want and can deadlock on a chatty child if implemented naively; `poll`-based capture fixes it at the cost of a more complex extern; `spawn`-only pushes the whole model onto the caller. Recommendation: `process.run` with `poll`-based capture and a documented memory cost ("buffers both streams"), `spawn` for the streaming case.

---

## 13. Findings: where `plan.md` is stale or wrong

Verified against a clean build of `HEAD` (`a059dba`) plus code reading. Each is worth a line of correction in `plan.md`; none is a compiler bug except 1.7, which is a capability-layer gap (net is documented but unusable).

1. **"Prelude, always in scope" does not exist.** The namespace model lists `print`, `print!`, `panic`, `assert`, `math`, `str`, `path`, `mem`, `io` as "no import needed". Reality: every stdlib namespace needs an `impl` line (the repo's own `caps_list` project had to add `impl "std/str"`), and `panic`/`assert` are not builtins at all (probed: `call to undefined function 'panic'`). The usable distinction is *ungated* vs *gated*, not *prelude* vs *imported*.
2. **`net`'s grant example cannot compile, and its matcher would deny everything.** `impl "std/net" with caps(net = ["api.example.com:443"])` is rejected because `patterns_are_paths` includes `net` and patterns must start with `/` (`resolve.rs::validate_cap_pattern`); the emitted check is `x_cap_check_path`, which denies any non-absolute argument; and `checked_args("net", ...)` never matches the port. Net needs its own vocabulary, matcher and argument list (S3.1).
3. **The `env`/`time`/`rand` sketch's "ungated in spirit" prose contradicts the code.** `time.sleep_ms` is inside the gated `std/time` module, so sleeping needs a `time` grant in any module but the entry module (the M7 record admits this). `process.exit`/`process.getpid` are listed as ungated conveniences but there is no `std/process` at all, so they do not exist (probed: `impl "std/process"` → not shipped).
4. **`plan.md`'s own `impl "std/rand"` example is misleading**: it shows an uncapped import with no note that `rand.u64()` then resolves *only because the entry module holds everything* (probed: it compiles from `main`, and would not from a library module without a grant). Worth a sentence, because it is the exact confusion open item 18 describes.
5. **The `str` API sketch's `trim`/`trim_end` return types are wrong** for the shipped code: the sketch says `R: &str`, the M6.3 record and `xc/std/str.x` say `R: String` (truncation cannot be a view of a NUL-terminated string). The sketch block should be corrected or marked historical, since it is the first thing an implementer would copy.
6. **The compiler-pipeline header is stale**: "Done: M0–M5.3. Next: M6" — the record below it includes M7 and C1.
7. **M9's arithmetic is stale**: "the 97 end-to-end tests, which are backend-agnostic by construction" — the suite is 109 today (`cargo test -p xc`: 65 unit + 109 e2e).
8. **The stdlib constraints table still has a "No `const` yet" row** ("`math.PI` is not expressible without a new declaration form"), but `const` shipped in M6.3 and `std/math.x` uses it.
9. **`fs.rename`'s two-path check is still unexercised** — `plan.md` says so under "Testing the stdlib", and it is still true: nothing grants one path of a rename and not the other. Cheap to fix in S5's fs work.
10. **`print` cannot show a `datac` or a `List`** (probed, both rejected). Not claimed by `plan.md`, but it is absent from the "Not shipped, and known" list, where it belongs: it is the one gap that makes every other feature harder to test by hand.
11. **A capability denial prints a blank "granted:" line.** `codegen_c.rs::CAP_RUNTIME`'s `x_cap_deny` writes `"  granted: %s\n"` with `count == 0 ? "(nothing)" : ""`, so when patterns exist the output is `  granted: ` — an empty replacement — followed by a newline and then the indented patterns. Cosmetic, and worth one line of fix while S3 is in that file, because the message is fixture-asserted text.

---

## 14. Non-goals

Deliberately out of scope for this roadmap, with the reason:

- **TLS and `https` in S4.** Their own milestone (S4b) because the dependency decision is the owner's, not an implementation detail. Hand-rolled TLS is never in scope.
- **`async`/`await`, green threads, thread pools.** Blocked on function values and move-taking parameters (S7); also the C backend would need a state-machine transform, which is M9-shaped work.
- **A JSON library.** It wants views (zero-copy parsing), a `Map`-shaped DOM, and tagged unions (`null`/number/string/array/object); written now it would be rewritten three times.
- **`regex`.** Its own engine project; nothing in X needs it yet, and a backtracking one written in X with no views would be slow enough to mislead.
- **`log`, `cli` argument parsing, a test framework, `fmt`-with-variadics, `xc fmt`.** All downstream of S1–S5 and all currently better done by the program itself. Variadic formatting in particular needs either generics or a macro system, neither of which exists.
- **HTTP servers beyond a minimal one, HTTP/2, WebSockets, gzip/brotli.** Server-side work is S5+ and the minimal server exists only to test the client and to prove `poll`.
- **Generic containers and `Option`/`Result`.** M8's neighbourhood; the roadmap's monomorphic `Map<V>` is the deliberate stopgap.
- **Grapheme-aware Unicode, case folding, normalization.** Tables and algorithms out of proportion to the value here (Q15).
- **An arena/bump allocator, `defer`, custom allocators.** Arena without views is a copying allocator; `defer` needs syntax (`syntax&shii.md` has none) and the drop plan already covers scope exit. Revisit with views.
- **Replacing `cc`, `#line`-annotated C, the own backend.** M9; this file adds ~40–60 C bodies that M9 stage 4 would inherit, which is a reason to keep them small and one-line wherever possible.
- **Third-party dependencies, a package registry, the stdlib versioned separately from the compiler.** `plan.md` decision 3 stands; TLS is the one place where "system library" has to be distinguished from "third-party package", and that is Q1.
