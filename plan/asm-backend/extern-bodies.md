# MK7 extern bodies: the syscall surface the asm backend refuses

> **ARCHIVED 2026-09-20. Superseded, not deleted.** The `asm` backend's missing `extern fun`
> routines never got written and now never will: under LLVM every `extern fun` is a declared call,
> as it already is in the C backend. The inventory of the syscall surface is still useful.

This is the specification lane for MK7 section 5 item 5 and the heap row of the ranking: the
`extern fun` bodies this backend has no routine for. It is written at `bb2316e` ("plan: the MK7
status at the post-`caeb98d` tip"), the checkout the brief `plan/asm-backend/next-slice.md` was last
re-cut in, and it changes no source. Every refusal, every C-build and every "this call site
reaches that extern" claim below was measured with the release binary at that commit; the places
that are a source read rather than a measurement say so.

Commands used, from the repository root:

```
./target/release/kelpc build --backend=asm kelpc/tests/fixtures/NAME.kp -o /tmp/x     # the refusal, by hand
./target/release/kelpc build --backend=c   kelpc/tests/fixtures/NAME.kp -o /tmp/xc    # the oracle
cargo test --release --test backend_coverage -- --ignored --nocapture                 # the ranking (37.8 s)
```

A probe file per call site (55 of them, written under `/tmp`, one `main` each) was built with
`--backend=asm` to measure which extern is the *first live* one a call site reaches. The compiler
stops at the first refusal, so a probe gives the head of a call site's closure and not the tail;
the tails below are read off `kelpc/std/*.kp`. Where the two disagree the file says so.

The `codegen_asm.rs` line numbers below are anchors as of this read, not contracts: a separate
closeout lane is editing that file in the same checkout (it grew 985 lines while this file was
written, moving every anchor in it), so a citation that no longer lands is found by the name in
the same sentence. `externs.rs`'s entry lines and `codegen_c.rs`'s are stable.

## 0. The row, and two corrections to the task's own numbers

The ranking at this commit, verbatim for the heap row:

```
asm backend refusal ranking (24 fixtures refused of 202 the C backend builds, 178 built)
  a heap type: 13
    borrow_chain_through_a_borrow
    caps_gated_path
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
    process_spawn
  a gated extern: 1
    caps_time_rand
```

Two corrections to the task text, both measured:

1. The heap row's extern-headed members are **11, not 12**: the 13 are `caps_gated_path`,
   `fs_errno`, `fs_fallible`, `fs_symlink_kind`, `io_stdio`, `io_streams`, `net_loopback`,
   `net_poll`, `net_resolve`, `net_timeout`, `process_spawn` (11, each refused on an `extern fun`)
   plus `list_datac_clone_sort` (a `List.clone` refusal) and `borrow_chain_through_a_borrow` (a
   borrow refusal). The 12th extern-headed *program* on the board is `caps_time_rand`, the gated
   row's only member, refused on `std.time.now_unix_raw`. This file covers all 12 that name an
   extern, which is where the task's 12 comes from.
2. The extern count that matters is not 67. The seven syscall modules declare 67 `extern fun`s
   (`std/fs` 27, `std/io` 11, `std/net` 13, `std/process` 10, `std/path` 1, `std/time` 3,
   `std/rand` 2), and `std/env` declares 4 more (71 in eight modules). The twelve extern-headed
   programs reach **46** of the 71. Adding the `caps_*`, `process_*` and `http_*` test *projects*
   (measured: 21 of them build under C and refuse at an extern), which are the same suite and reach
   the same surface, brings it to **66 of 71**. Only 5 externs are reached by nothing in the
   corpus: see section 4.

Every one of the 13 heap-row fixtures builds under the C backend at this commit (measured one by
one), so none is an oracle rejection. `caps_time_rand` builds under C too.

One ordering rule this file leans on, measured and worth stating once: the refusal names the
**earliest live extern in the walked order**, and the walked order is the loaded modules in the
compiler's own order with source order inside a module. Four cross-module cases and about thirty
intra-module cases agree with it:

- within `std/fs.kp`, `write_text_raw` (line 31) is refused before `is_dir_raw` (34) before
  `list_raw` (42) before `is_symlink_raw` (50);
- within `std/io.kp`, `is_open_raw` (32) before `read_raw` (33) before `read_into_raw` (34) before
  `close_raw` (38) before `wait_readable_raw` (40) before `wait_writable_raw` (41) before
  `errno_text_raw` (42);
- within `std/net.kp`, `connect_raw` (31) before `listen_raw` (32);
- within `std/time.kp`, `now_unix_raw` (287) before `now_mono_ns_raw` (292);
- `std/io` before `std/fs`, `std/io` before `std/net`, `std/io` before `std/process`,
  `std/fs` before `std/str`.

The rule is what lets a slice's acceptance say what the *next* refusal will be after it lands.

## 1. Each fixture, its first refusal, and its transitive extern set

Notation: `U(fixture)` is the set of externs with no lowering at this commit that the fixture
reaches. Lowerable externs the fixtures also reach are named separately, because they cost
nothing: `std.str.byte_len_raw` and `std.str.byte_at_raw` (`codegen_asm.rs:1074-1076`),
`std.mem.alloc_raw` (`:1104`), `std.path.normalize_raw` (`:1125`), `std.fs.cwd_raw` (`:1126`).

| fixture | first refusal (verbatim) | |U| | gate sites (section 2.5) |
| --- | --- | --- | --- | --- |
| `caps_gated_path` | ``the `extern fun std.fs.read_raw` `` | 1 | `fs.read` |
| `fs_errno` | ``the `extern fun std.fs.read_raw` `` | 1 | `fs.read` |
| `fs_fallible` | ``the `extern fun std.io.is_open_raw` `` | 9 | 6 `fs` functions, 8 sites |
| `fs_symlink_kind` | ``the `extern fun std.fs.write_text_raw` `` | 11 | 11 `fs` calls |
| `io_stdio` | ``the `extern fun std.io.is_open_raw` `` | 4 | none (`std/io` is ungated) |
| `io_streams` | ``the `extern fun std.io.is_open_raw` `` | 5 | none |
| `net_loopback` | ``the `extern fun std.net.connect_raw` `` | 7 | `net.listen`, `net.connect` |
| `net_poll` | ``the `extern fun std.io.wait_readable_raw` `` | 10 | `net.listen`, `net.connect` |
| `net_resolve` | ``the `extern fun std.net.resolve_raw` `` | 1 | `net.resolve` |
| `net_timeout` | ``the `extern fun std.net.connect_raw` `` | 6 | `net.listen`, `net.connect` |
| `process_spawn` | ``the `extern fun std.io.read_raw` `` | 10 | `process.run`, `process.spawn` |
| `caps_time_rand` | ``the `extern fun std.time.now_unix_raw` `` | 5 | none (see 2.5) |

The exact wording of an asm refusal is `error: the 'asm' backend does not support the `extern fun
NAME` yet`; the table quotes the name alone.

### 1.1 `caps_gated_path` and `fs_errno`: one extern, and the gate besides

Both are `impl "std/fs"` with a `main` whose only statement is `fs.read(PATH)`
(`caps_gated_path.kp:16-18`, `fs_errno.kp:5-7`), so both trace to the same thing:

- `std/fs.kp:72` `fun read(path: &str).R: Bytes = read_raw(path)` gives `std.fs.read_raw`.

`U` = { `std.fs.read_raw` }. Measured heads agree (`std.fs.read_raw` for both). These two are the
only fixtures where one extern body is the whole of the missing work, and both still need the
capability check emitted at the `fs.read` call site (section 2.5), which is what
`caps_gated_path`'s own comment (`:1-13`) says it pins.

### 1.2 `fs_fallible`: the widest io/fs mix

Call sites (`fs_fallible.kp:8-59`), with the wrapper each reaches and the extern behind it:

- `fs.write_text` (`fs.kp:115`) -> `fs.write_text_raw`.
- `mem.alloc` (`mem.kp:16`) -> `std.mem.alloc_raw`, lowerable. The `raw.set(0, 255)` is a `Bytes`
  method, not an extern.
- `fs.write` (`fs.kp:110`) -> `fs.write_raw`.
- `fs.try_read` (`fs.kp:79`) -> `try_read_raw` plus `io.errno_text` -> `std.io.errno_text_raw`
  (`io.kp:53`).
- `fs.try_read_text` (`fs.kp:95`) -> `try_read_raw`, `utf8_valid_raw`, and `io.errno_text_raw`.
- `fs.try_open` (`fs.kp:333`) -> `try_open_raw` and `io.errno_text_raw`; the `Ok` branch calls
  `file.is_open()` (`io.kp:74`) -> `std.io.is_open_raw` and `file.close()` (`io.kp:132`) ->
  `std.io.close_raw`.
- `fs.remove` (`fs.kp:149`) -> `fs.remove_raw`.

`U` = { `std.fs.write_text_raw`, `std.fs.write_raw`, `std.fs.try_read_raw`,
`std.fs.utf8_valid_raw`, `std.fs.try_open_raw`, `std.fs.remove_raw`, `std.io.errno_text_raw`,
`std.io.is_open_raw`, `std.io.close_raw` }, and `std.mem.alloc_raw` lowerable. Measured head
`std.io.is_open_raw` is consistent: it is the earliest live io extern in `io.kp` order among
`is_open_raw` (32), `close_raw` (38), `errno_text_raw` (42).

The `Ok` branches of the three `if let` forms are live (both arms are compiled and
`io.is_open_raw` is measured as the head, which it could not be otherwise), so nothing here is
ambiguous. `Result` and `IoErr` construction is frontend work that already lowers.

### 1.3 `fs_symlink_kind`: eleven fs externs, two already-lowerable helpers

Call sites (`fs_symlink_kind.kp:10-63`):

- `fs.exists` (`fs.kp:128`) -> `exists_raw`.
- `fs.remove_all` (`fs.kp:236`) -> `is_symlink` -> `is_symlink_raw`, `is_dir` -> `is_dir_raw`,
  `list` (`fs.kp:193`) -> `list_raw`, and `remove` -> `remove_raw`; `list` also runs `str.len` and
  `str.split` (`str.kp:277`), and `split` runs `starts_with_at`/`byte`/`copy_range`
  (`str.kp:75`, `:23`, `:262`), so `std.str.byte_len_raw`, `std.str.byte_at_raw` (both lowerable)
  and `std.mem.alloc_raw` (through `copy_range`'s `mem.alloc`) are in the set. `remove_all` calls
  `path.join` (`path.kp:153`), which uses `str.len`, `str.byte`, `mem.alloc`.
- `fs.mkdir_all` (`fs.kp:218`) -> `mkdir_all_raw` (twice).
- `fs.write_text` -> `write_text_raw` (four times).
- `fs.mkdir` (`fs.kp:164`) -> `mkdir_raw`.
- `fs.symlink` (`fs.kp:309`) -> `symlink_raw` (five times, each a two-resource check).
- `fs.is_symlink` (`fs.kp:137`), `fs.is_file` (`:129`), `fs.is_dir` (`:130`), `fs.read_link`
  (`:143`) -> `is_symlink_raw`, `is_file_raw`, `is_dir_raw`, `read_link_raw`.
- `fs.walk` (`fs.kp:262`) -> `list` -> `list_raw`, `is_symlink_raw`, `is_dir_raw`, `path.join`.

`U` = { `std.fs.exists_raw`, `std.fs.remove_raw`, `std.fs.is_symlink_raw`,
`std.fs.is_dir_raw`, `std.fs.is_file_raw`, `std.fs.list_raw`, `std.fs.mkdir_raw`,
`std.fs.mkdir_all_raw`, `std.fs.write_text_raw`, `std.fs.symlink_raw`,
`std.fs.read_link_raw` } (11), plus lowerable `std.str.byte_len_raw`, `std.str.byte_at_raw`,
`std.mem.alloc_raw`. Measured head `std.fs.write_text_raw` is the earliest live fs extern
(`write_text_raw` 31 < `is_dir_raw` 34 < `list_raw` 42 < `is_symlink_raw` 50), which also proves
`read_raw` (29) and `write_raw` (30) are *not* live for this fixture. Two probes agree
independently: `fs.walk` alone refuses `std.fs.is_dir_raw` and `fs.remove_all` alone refuses
`std.fs.is_dir_raw`.

Ambiguity: this fixture is the one whose *check sites* are not fully determined, because
`remove_all`/`walk` call `fs.is_symlink`/`fs.is_dir`/`fs.list` from inside `std/fs` itself. The C
backend emits **no** check for those calls (measured: the generated
`kelp_std__fs__remove_all` in `build/fs_symlink_kind.c` calls `kelp_std__fs__is_symlink`,
`kelp_std__fs__is_dir` and `kelp_std__fs__list` with no `kelp_cap_check_path` around any of them),
because a check is emitted only for a calling module that holds a grant for the domain
(`codegen_c.rs:3415`, `patterns_for` at `:576`). The asm side's `cap_check_refusal`
(`codegen_asm.rs:1146-1159`) is a pure function of `(gated table, callee, arg shapes)` with no
caller module, so as written it would refuse those three internal calls. Whether it does is not
measurable today (the extern pre-pass at `:3327-3331` refuses first); the measurement that settles
it is building this fixture once its externs and the check land.

### 1.4 `io_stdio`: four io externs, no gate

Call sites (`io_stdio.kp:5-35`):

- `io.read_line` (`io.kp:269`) -> `stdin_reader.read_line` (`:230`) -> `BufReader.read_line_into`
  (`:196`) -> `read_into_raw`. The `pd var stdin_reader` initializer (`io.kp:30`) runs
  `mem.alloc(8192)` -> `std.mem.alloc_raw` (lowerable); `mem.alloc_raw` is a liveness root anyway
  (`codegen_asm.rs:1184`).
- `io.read_lines` (`io.kp:274`) -> `read_line_into` -> `read_into_raw`, plus `String.clone` and
  `List.push`, both builtins.
- `io.print` (`io.kp:301`) -> the builtin `print`; a probe with only `io.print("x")` **builds**.
- `io.stdout` (`io.kp:160`), `File.write_str` (`:122`) -> `write_str_raw`.
- `File.is_open` (`:74`) -> `is_open_raw`.
- `io.eprint`/`io.eprint_bare` (`:309`, `:315`) -> `write_str_raw`.
- `File.wait_writable` (`:155`) -> `wait_writable_raw`.

`U` = { `std.io.read_into_raw`, `std.io.write_str_raw`, `std.io.is_open_raw`,
`std.io.wait_writable_raw` }, with `std.mem.alloc_raw` lowerable. Measured head
`std.io.is_open_raw` is the earliest live io extern (32 < 34 < 37 < 41). Note that `read_raw` is
**not** in this set: the line reader uses `read_into_raw` only (measured: the `io.read_line` probe
refuses `std.io.read_into_raw`).

### 1.5 `io_streams`: the same four plus `read_raw` and `close_raw`

Call sites (`io_streams.kp:6-38`):

- `io.buf_reader` (`io.kp:184`) -> `mem.alloc` (lowerable).
- `BufReader.read_line_into` -> `read_into_raw`.
- `BufReader.read_all` (`io.kp:243`) -> `read_raw` for the rest of the descriptor.
- `File.is_open` -> `is_open_raw`; `File.flush` (`:128`) is a no-op with no extern.
- `File.read_into` (`:87`) -> `read_into_raw`.
- `File.wait_writable` -> `wait_writable_raw`.
- `File.close` -> `close_raw`.

`U` = { `std.io.read_into_raw`, `std.io.read_raw`, `std.io.is_open_raw`,
`std.io.wait_writable_raw`, `std.io.close_raw` }, `std.mem.alloc_raw` lowerable. Measured head
`std.io.is_open_raw`; consistent because `is_open_raw` (32) precedes `read_raw` (33).

### 1.6 `net_loopback`: seven net externs

Call sites (`net_loopback.kp:10-23`):

- `net.listen` (`net.kp:77`) -> `listen_raw`; `Listener.port` (`:179`) -> `local_port_raw`.
- `net.connect` (`:62`) -> `connect_raw`; `net.accept` (`:83`) -> `accept_raw`.
- `net.send_text` (`:128`) -> `send_all` (`:122`) -> `send_all_raw`; `Bytes.from_str` is a builtin.
- `net.recv` (`:138`) -> `recv_raw`.
- `net.close` (`:86`) and `Listener.close` (`:96`) -> `close_raw`.

`U` = { `std.net.listen_raw`, `std.net.local_port_raw`, `std.net.connect_raw`,
`std.net.accept_raw`, `std.net.send_all_raw`, `std.net.recv_raw`, `std.net.close_raw` }.
Measured head `std.net.connect_raw` is the earliest live net extern (`connect_raw` 31 <
`listen_raw` 32 < `accept_raw` 33 < `close_raw` 34 < `send_all_raw` 37 < `recv_raw` 38).

### 1.7 `net_poll`: the net seven plus the poll pair

`net_poll.kp:6-33` repeats the loopback sequence and adds `s.wait_readable` (`net.kp:154`),
`c.wait_writable` (`:162`), both of which build an `io.File` over the socket's fd and call the io
method, and `net.shutdown_write` (`:105`):

`U` = { `std.net.listen_raw`, `std.net.local_port_raw`, `std.net.connect_raw`,
`std.net.accept_raw`, `std.net.send_all_raw`, `std.net.recv_raw`,
`std.net.shutdown_write_raw`, `std.net.close_raw`, `std.io.wait_readable_raw`,
`std.io.wait_writable_raw` }. Measured head `std.io.wait_readable_raw`: the io module precedes
net, and within io `wait_readable_raw` (40) precedes `wait_writable_raw` (41).

### 1.8 `net_resolve`: one net extern and two lowerable str helpers

`net_resolve.kp:8-15` calls `net.resolve` (`net.kp:192`), which is `resolve_raw` plus
`str.split` (`str.kp:277`), so `std.net.resolve_raw` with `std.str.byte_len_raw`,
`std.str.byte_at_raw` and `std.mem.alloc_raw` (all lowerable). Measured head
`std.net.resolve_raw`. This is the smallest fixture of the family: one extern body plus the gate
at `net.resolve`.

### 1.9 `net_timeout`: six net externs, no `close`

`net_timeout.kp:7-16` calls `net.listen`, `Listener.port`, `net.connect`, `net.accept`,
`Socket.set_timeout` (`net.kp:171`) -> `set_timeout_raw`, and `net.recv` -> `recv_raw`. No close,
so `net.close_raw` is not live here. `U` = { `listen_raw`, `local_port_raw`, `connect_raw`,
`accept_raw`, `set_timeout_raw`, `recv_raw` }. Measured head `std.net.connect_raw`.

### 1.10 `process_spawn`: ten, six of them process externs

Call sites (`process_spawn.kp:15-83`):

- `process.run` (`process.kp:206`) -> `run_in` (`:166`) -> `spawn_in` (`:130`) -> `spawn_raw`;
  then `child.stdin.close()` -> `io.close_raw`, `child.stdout.wait_readable(50)` ->
  `io.wait_readable_raw`, `child.stdout.read(8192)`/`stderr.read` -> `io.read_raw`,
  `child.stdout.close()`/`stderr.close()` -> `io.close_raw`, `child.wait()` (`:246`) ->
  `reap_raw`.
- `process.spawn` (`:155`) -> `spawn_in` -> `spawn_raw`.
- `child.stdin.write_text` (`io.kp:116`) -> `io.write_text_raw`.
- `blocked.is_running` (`process.kp:260`) -> `reap_raw`; `blocked.kill(9)` (`:278`) ->
  `kill_raw`; `blocked.wait` -> `reap_raw`.
- `process.args` (`:94`) -> `arg_count_raw` and `arg_at_raw`; `process.getpid` (`:289`) ->
  `getpid_raw`.
- `str.starts_with` (`str.kp:87`) and `str.len` (`str.kp:19`) -> the two lowerable str externs.
- `Bytes.empty`/`Bytes.append` are builtins.

`U` = { `std.process.spawn_raw`, `std.process.reap_raw`, `std.process.kill_raw`,
`std.process.arg_count_raw`, `std.process.arg_at_raw`, `std.process.getpid_raw`,
`std.io.close_raw`, `std.io.wait_readable_raw`, `std.io.read_raw`, `std.io.write_text_raw` }.
Measured head `std.io.read_raw`: it is the earliest live io extern (`read_raw` 33 < `write_text_raw`
36 < `close_raw` 38 < `wait_readable_raw` 40), and io precedes process. `process.exists_raw`,
`executable_raw` and `is_dir_raw` are **not** in this fixture: they belong to `try_run_in`
(`process.kp:225`), which it never calls.

### 1.11 `caps_time_rand`: five externs, and no gate

`caps_time_rand.kp:7-15` calls `time.now_unix` (`time.kp:13`), `time.now_mono_ns` (`:16`),
`rand.below` (`rand.kp:12`) -> `u64_raw`, `rand.bytes` (`:20`) -> `mem.alloc` plus `fill_raw`, and
`time.sleep_ms` (`:18`). `U` = { `std.time.now_unix_raw`, `std.time.now_mono_ns_raw`,
`std.time.sleep_ms_raw`, `std.rand.u64_raw`, `std.rand.fill_raw` }, with `std.mem.alloc_raw`
lowerable. Measured head `std.time.now_unix_raw` (287 < 292 < 297). This fixture needs **no**
capability check: `checked_args` (`stdlib.rs:200`) has no `time` or `rand` arm and every one of
these calls takes no resource, so `cap_check_refusal` returns `None` and the C backend emits the
plain call. `bench/speed/speed.kp` is the same story with one extern: measured, it refuses
`std.time.now_mono_ns_raw`, and the same file with the clock constant-folded **builds** under
`--backend=asm`, so the section 13 gate artifact is one extern body away and needs no check.

## 2. The extern catalogue

`externs.rs` is a table of `(module, bare name, C body)`; the line below is the entry's first line
(`        "std/fs",`) and the name's line is the next one. `std/...kp:` is the declaration.

Shapes are the SysV facts the routine has to honour; the existing machinery is named where it is
what the body calls.

### 2.1 `std/fs`

- **`read_raw`** `externs.rs:152`, `std/fs.kp:29`. `fopen`/`fseek`/`ftell`/`rewind`/`fread`/
  `fclose`. In: `const char*` path. Out: owned `Bytes` (`MEMORY` class, destination in `rdi`,
  as `PathNormalizeRaw` does). Allocates through `kelp_bytes_alloc_raw` (the existing
  `Routine::BytesAllocRaw`). Three errno panic paths plus a short-read panic; `kelp_panic_errno`
  exists (`codegen_asm.rs:12903`, errno read through `__errno_location@PLT` at `:12910`).
- **`write_raw`** `:157`, `std/fs.kp:30`. `fopen("wb")`/`fwrite`/`fclose`. In: path, `Bytes` by
  value (borrow of the caller's). No allocation. errno panic.
- **`write_text_raw`** `:162`, `std/fs.kp:31`. Same with `String`. No allocation.
- **`exists_raw`** `:167`, `std/fs.kp:32`. `struct stat` on the stack, `stat()`, `== 0`. Reads no
  errno. Returns `bool`.
- **`is_file_raw`** `:172`, `std/fs.kp:33`. `stat` plus `S_ISREG(st.st_mode)`: the routine needs
  the `st_mode` offset of `struct stat` (8 bytes at offset 24 on x86-64 glibc), which no other
  body in this backend has needed.
- **`is_dir_raw`** `:177`, `std/fs.kp:34`. `stat` plus `S_ISDIR`.
- **`size_raw`** `:182`, `std/fs.kp:35`. `stat` plus `st_size` (offset 48). errno panic.
- **`remove_raw`** `:187`, `std/fs.kp:36`. `remove()`, errno panic. One `const char*`.
- **`rename_raw`** `:192`, `std/fs.kp:37`. `rename(from, to)`, errno panic. Two paths.
- **`mkdir_raw`** `:197`, `std/fs.kp:38`. `mkdir(path, 0777)`, errno panic.
- **`mkdir_all_raw`** `:202`, `std/fs.kp:39`. A `char buf[PATH_MAX]` on the stack, `strlen` +
  `memcpy`, then a prefix loop calling `mkdir` and treating `EEXIST` as success. `PATH_MAX` is
  4096; the buffer is 4 KiB on the frame.
- **`append_raw`** `:207`, `std/fs.kp:40`. `fopen("ab")`/`fwrite`/`fclose`. Reached by nothing
  (section 4).
- **`list_raw`** `:212`, `std/fs.kp:42`. `scandir` with `alphasort`, a `struct dirent**` whose
  `d_name` offset the routine must know (offset 19 in `struct dirent` on x86-64), `strcmp` against
  `.` and `..`, `strlen`/`memcpy` into `kelp_bytes_alloc_raw`, `free` per entry and for the array,
  and a hand-written `b.len = at` and NUL. Must leave `{data, len, cap}` consistent, then hand the
  allocation to `kelp_String__from_bytes` (`Routine::StringFromBytes`).
- **`sync_raw`** `:217`, `std/fs.kp:41`. `open`/`fsync`/`close`, errno panic. Reached by nothing.
- **`open_raw`** `:222`, `std/fs.kp:43`. A `strcmp` mode table to `O_*` flags, then
  `open(path, flags, 0666)`. Returns an `i32` fd. errno panic; a bad mode panics with a plain
  message. Two arguments, one of which is a path and one of which is not (section 2.5).
- **`symlink_raw`** `:227`, `std/fs.kp:45`. `symlink(target, link)`, errno panic. Two paths.
- **`copy_raw`** `:232`, `std/fs.kp:44`. `open`/`fstat`/`open`/`sendfile` in a loop/`close`, three
  errno paths. Needs `struct stat`'s `st_size` and two fds in one frame.
- **`touch_raw`** `:237`, `std/fs.kp:46`. `open(O_WRONLY|O_CREAT, 0666)`/`close`/`utimensat`,
  errno panic. `AT_FDCWD` is `-100`.
- **`temp_path_raw`** `:242`, `std/fs.kp:47`. `snprintf` into a `PATH_MAX` stack template,
  `mkstemp`, `close`, `String` built by copy.
- **`mtime_raw`** `:247`, `std/fs.kp:48`. `stat` plus `st_mtime` (offset 88, seconds).
- **`mode_raw`** `:252`, `std/fs.kp:49`. `stat` plus `st_mode & 07777`.
- **`is_symlink_raw`** `:257`, `std/fs.kp:50`. `lstat` plus `S_ISLNK`.
- **`read_link_raw`** `:262`, `std/fs.kp:51`. `char buf[PATH_MAX]`, `readlink`, a length check, NUL,
  `String` by copy.
- **`cwd_raw`** `:267`, `std/fs.kp:52`. Landed (`Routine::FsCwdRaw`, `codegen_asm.rs:1126`), and
  it is the working example of a routine that panics with errno.
- **`try_read_raw`** `:272`, `std/fs.kp:58`. The `read_raw` body with `*kelp_err` written on every
  path (`0`, then the errno of whichever call failed) and an empty `Bytes` returned on failure;
  one path `kelp_bytes_drop`s the buffer it just allocated before returning the empty one. Out
  parameter `mut i32` (a scalar borrow; the lowering exists, `codegen_asm.rs` `ExprKind::Ref`).
- **`try_open_raw`** `:277`, `std/fs.kp:59`. The `open_raw` body with `*kelp_err = EINVAL` (22) for
  an unknown mode and `0` on success.
- **`utf8_valid_raw`** `:282`, `std/fs.kp:63`. `kelp_utf8_valid(data, len)`, which the backend
  already has as `Routine::Utf8Valid` (`codegen_asm.rs:871`); the body is one call to it. Takes
  `&Bytes` (a borrow of the 24-byte `{data,len,cap}`).

### 2.2 `std/io` (ungated: no call in this module carries a capability check)

- **`is_open_raw`** `:344`, `std/io.kp:32`. `fcntl(fd, F_GETFD) >= 0`. `F_GETFD` is 1. Reads no
  errno. This is the only extern that is the first refusal of three fixtures.
- **`read_raw`** `:349`, `std/io.kp:33`. `kelp_bytes_alloc_raw(max)`, `read`, errno panic, then
  `b.len = n` and a NUL at `data[len]`. Owned `Bytes` out.
- **`read_into_raw`** `:354`, `std/io.kp:34`. **No allocation**: `room = at < len ? len - at : 0`,
  an early `return 0` when `room == 0` (no syscall, so errno must not be disturbed), `read` into
  the caller's buffer at `at`, NUL at `at + n`, returns the count. The caller's `Bytes` length is
  unchanged, which is the invariant the fixture prints (`buf.len()`).
- **`write_bytes_raw`** `:359`, `std/io.kp:35`. `kelp_io_flush_stdio(fd)`, then an
  EINTR-tolerant `write` loop. Reached by nothing (section 4).
- **`write_text_raw`** `:364`, `std/io.kp:36`. Same with `String`.
- **`write_str_raw`** `:369`, `std/io.kp:37`. Same with `strlen` first.
- **`close_raw`** `:374`, `std/io.kp:38`. `close`, errno panic.
- **`set_nonblocking_raw`** `:379`, `std/io.kp:39`. `fcntl(F_GETFL)`/`F_SETFL` plus `O_NONBLOCK`.
  Reached by nothing.
- **`wait_readable_raw`** `:384`, `std/io.kp:40`. `struct pollfd` on the stack (`fd`, `events =
  POLLIN`, `revents`), a timeout above `INT_MAX` mapped to `-1`, a `poll` loop treating `EINTR` as
  a retry, then `revents & (POLLIN|POLLHUP|POLLERR)`.
- **`wait_writable_raw`** `:389`, `std/io.kp:41`. The same with `POLLOUT`.
- **`errno_text_raw`** `:394`, `std/io.kp:42`. `strerror(errno)`, and the result is copied into a
  `String` (`Routine::StringFromStr`); `strerror` returns a static buffer, so a body that treated
  it as owned would free a static.

### 2.3 `std/net` (the externs are not gated; the wrappers are, section 2.5)

- **`connect_raw`** `:448`, `std/net.kp:31`. `struct addrinfo hints` zeroed on the stack, `snprintf`
  of the service, `getaddrinfo` (which **allocates** a list), a loop over `ai_next` calling
  `socket`/`connect`, `freeaddrinfo`, then either `kelp_panic_gai` or `kelp_panic_at` with the last
  errno. Needs `ai_family`, `ai_socktype`, `ai_protocol`, `ai_addr`, `ai_addrlen`, `ai_next`.
- **`listen_raw`** `:453`, `std/net.kp:32`. The same, `AF_INET`/`AI_PASSIVE`, plus
  `setsockopt(SO_REUSEADDR)` and `bind` then `listen(fd, 128)`.
- **`accept_raw`** `:458`, `std/net.kp:33`. `accept(fd, NULL, NULL)` in an EINTR loop, errno panic.
- **`close_raw`** `:463`, `std/net.kp:34`. `close`, errno panic.
- **`shutdown_write_raw`** `:468`, `std/net.kp:35`. `shutdown(fd, SHUT_WR)`, errno panic.
- **`send_raw`** `:473`, `std/net.kp:36`. `send(..., MSG_NOSIGNAL)` in an EINTR loop.
- **`send_all_raw`** `:478`, `std/net.kp:37`. The short-write loop around it.
- **`recv_raw`** `:483`, `std/net.kp:38`. `kelp_bytes_alloc_raw(max)`, `recv` in an EINTR loop,
  `EAGAIN`/`EWOULDBLOCK` mapped to the `net.recv: timed out` panic (this is what
  `net_timeout` prints), then the length and NUL rewrite.
- **`recv_into_raw`** `:488`, `std/net.kp:39`. As `read_into_raw` but `recv`, with the same timeout
  mapping.
- **`set_timeout_raw`** `:493`, `std/net.kp:40`. `struct timeval` on the stack and two
  `setsockopt(SO_RCVTIMEO, SO_SNDTIMEO)`, errno panic.
- **`local_port_raw`** `:498`, `std/net.kp:41`. `struct sockaddr_storage` on the stack,
  `getsockname`, then `ntohs` of `sin_port` (offset 2) for `AF_INET` or `sin6_port` for `AF_INET6`.
- **`peer_addr_raw`** `:503`, `std/net.kp:42`. `getpeername`, `inet_ntop` into two stack buffers,
  `snprintf` of `host:port`/`[host]:port`, `String` by copy.
- **`resolve_raw`** `:508`, `std/net.kp:43`. `hints` with no service, `getaddrinfo`, a 4096-byte
  stack output buffer, a 16-entry `INET6_ADDRSTRLEN` dedup table, `inet_ntop` per address,
  `freeaddrinfo`, `String` by copy.

### 2.4 `std/process` (gated module) and `std/time`/`std/rand`/`std/env`

- **`arg_count_raw`** `:400`, `std/process.kp:32`. No libc: it reads the saved `kelp_argc`
  (`codegen_c.rs:81`). The asm backend has **no argv storage at all** (no `argc`/`argv` name exists
  in `codegen_asm.rs`), so these two bodies also need the emitted `main` to spill its entry `rdi`
  and `rsi` into two globals, at the same point the passdown initializers run
  (`codegen_asm.rs:5155`).
- **`arg_at_raw`** `:405`, `std/process.kp:33`. `kelp_argv[i]`, a bounds panic, `String` by copy.
- **`exists_raw`** `:410`, `std/process.kp:39`. `stat`, `== 0`.
- **`executable_raw`** `:415`, `std/process.kp:40`. `stat` plus `S_ISREG` plus `access(X_OK)`.
- **`is_dir_raw`** `:420`, `std/process.kp:41`. `stat` plus `S_ISDIR`.
- **`spawn_raw`** `:425`, `std/process.kp:34`. The largest body on the board: three `pipe`s,
  `fork`, in the child `dup2` three times, `close` six times, an optional `chdir` (exit 126 with a
  message), `malloc((n + 2) * 8)` for `argv`, a loop reading each `kelp_String.data` out of the list
  (stride 24), `execvp`, `free(argv)`, a message and `_exit(127)`; in the parent three `close`s and
  four out-parameters (`pid`, and the three fds) written through `mut i32`. `List<String>` is the
  24-byte `{data,len,cap}` header; its elements are 24-byte `String`s.
- **`reap_raw`** `:430`, `std/process.kp:35`. `waitpid(pid, &status, WNOHANG or 0)` in an EINTR
  loop, `return -1` for "still running", then `WIFEXITED`/`WEXITSTATUS`/`WIFSIGNALED`/`WTERMSIG`
  (all four are bit tests on the status word the routine must write out).
- **`kill_raw`** `:435`, `std/process.kp:36`. `kill(pid, sig)`, errno panic.
- **`exit_raw`** `:438`, `std/process.kp:37`. `exit(code)`. Reached by nothing (section 4).
- **`getpid_raw`** `:439`, `std/process.kp:38`. `getpid()`. This one is an exact `Extern::Libc`
  match (no arguments, `i64` out), as are `exit_raw`; every other body in this file needs the
  panic behaviour and so is a `Routine`.
- **`now_unix_raw`** `:287`, `std/time.kp:9`. `struct timespec` on the stack,
  `clock_gettime(CLOCK_REALTIME)`, errno panic, `tv_sec` out. `CLOCK_REALTIME` is 0.
- **`now_mono_ns_raw`** `:292`, `std/time.kp:12`. `CLOCK_MONOTONIC` (1), and
  `tv_sec * 1000000000 + tv_nsec`. This is the one extern `bench/speed/speed.kp` needs.
- **`sleep_ms_raw`** `:297`, `std/time.kp:18`. `struct timespec` and `nanosleep` in a loop that
  retries on interruption (the C body's loop body is empty on purpose).
- **`u64_raw`** `:302`, `std/rand.kp:6`. `fopen("/dev/urandom")`/`fread`/`fclose`, panic on either
  failure. Not `getrandom`.
- **`fill_raw`** `:307`, `std/rand.kp:7`. The same into the caller's `mut Bytes`.
- **`has_raw`/`get_raw`/`set_raw`/`unset_raw`** `:323`, `:328`, `:333`, `:338`,
  `std/env.kp`. `getenv`, `getenv` (or an empty `String`), `setenv(name, value, 1)`, `unsetenv`.
  One `str` argument (two for `set_raw`), errno on the setters. Only the `caps_env` projects
  reach these.

### 2.5 The capability check, per fixture

The extern's own body never carries a check: `resolve.rs:418-436` puts only a gated module's
*non-extern* free functions in the gate table, and the check belongs at the call site where a
user module names one of them. What a fixture needs is therefore a property of its own call sites,
and the fixtures split three ways.

1. **Needs the check**, because the fixture's own module calls a gated free function whose resource
   argument is a `str` (so `checked_args` names one, `stdlib.rs:200-210` and the per-domain
   helpers at `:214`, `:230`, `:255`, `:274`):
   - `caps_gated_path`, `fs_errno`: `fs.read` (`Path(0)`).
   - `fs_fallible`: `fs.write_text`, `fs.write`, `fs.try_read`, `fs.try_read_text`, `fs.try_open`,
     `fs.remove`.
   - `fs_symlink_kind`: the eleven calls of section 1.3, with two checks on each of the five
     `fs.symlink` calls, because both of its arguments are paths (`fs_checked_args` returns both,
     `stdlib.rs:255-264`).
   - `net_loopback`, `net_poll`, `net_timeout`: `net.listen` (`Port(0)`), `net.connect`
     (`HostPort(0,1)`). Everything else in `std/net` names no resource (`net_checked_args`,
     `stdlib.rs:274-281`).
   - `net_resolve`: `net.resolve` (`Host(0)`).
   - `process_spawn`: `process.run` and `process.spawn` (`Path(0)` of the program;
     `process_checked_args`, `stdlib.rs:230-244`).
2. **Needs no check**: `io_stdio`, `io_streams` (`std/io` is not in `domain_of`,
   `stdlib.rs:124-134`) and `caps_time_rand` (time and rand name no resource, so
   `cap_check_refusal` returns `None`; measured controls: the unit tests
   `an_ungated_call_is_not_refused` and `a_gated_domain_with_no_checked_arguments_is_not_refused`,
   `codegen_asm.rs:13336-13352`, and a probe calling `fs.cwd()`, which is the same shape and
   **builds**).
3. **The check itself is the mechanism the plan calls section C.** Two facts the writer needs:
   - The refusal today is at `codegen_asm.rs:11334-11339`, and the pre-pass at `:3327-3331` runs
     first, which is why all eleven fixtures report an extern rather than the gate.
   - The C mirror keys the emission on the *calling* module's grant
     (`codegen_c.rs:3415`, `self.caps.patterns_for(&self.module, domain)`), and measured, that is
     what keeps `std/fs`'s internal `is_symlink`/`is_dir`/`list` calls unchecked. The asm
     `cap_check_refusal` has no caller parameter, so a slice that ports it as written over-refuses
     inside gated modules and `fs_symlink_kind` will not build even after its eleven externs land.

## 3. Slice order

Sized like the plan's other slices: line estimates are for `codegen_asm.rs` additions, one
`Routine` variant per body plus its emission arm, with `require_routine` (`:8345`) declaring each
body's calls into the existing heap routines.

Every fixture named as reached below is C-buildable at this commit (measured), so it is a valid
acceptance target, and every project named is C-buildable too. The standing acceptance for a slice
is: build each fixture it reaches with `--backend=asm`, run it, and compare its stdout byte for
byte against its `--backend=c` binary (the differential sweep is
`cargo test --release --test backend_coverage -- --ignored --nocapture`); then
`KELPC_BACKEND=asm cargo test --test compile_and_run` for the projects, and
`cargo test --test memory_gate -- --ignored --nocapture` for valgrind. A slice that reaches a
fixture must also add it to `ASM_BUILT_BASELINE` in `kelpc/tests/backend_coverage.rs`, which is the
closeout lane's file.

### S1. Clock and entropy (5 externs, ~150-250 lines)

Owns `std.time.now_unix_raw`, `now_mono_ns_raw`, `sleep_ms_raw`; `std.rand.u64_raw`, `fill_raw`.
Mechanism: one libc call plus a two-field `timespec`, or `fopen("/dev/urandom")` and `fread`.
Reaches: `caps_time_rand` (fixture, with no gate work) and `bench/speed/speed.kp`, so this slice
is also what makes the section 13 gate artifact measurable.
Acceptance: `caps_time_rand` builds and prints the C backend's bytes; `--backend=asm
bench/speed/speed.kp` builds and its `RESULT` lines match `bench/speed/results.txt`. Risk low; no
allocation, no errno beyond the panic, one EINTR loop (`nanosleep`).

### S2. The descriptor family (11 externs, ~350-500 lines)

Owns all of `std/io`: `is_open_raw`, `read_raw`, `read_into_raw`, `write_bytes_raw`,
`write_text_raw`, `write_str_raw`, `close_raw`, `set_nonblocking_raw`, `wait_readable_raw`,
`wait_writable_raw`, `errno_text_raw`. Mechanism: an `i32` fd in a register, one libc call,
`kelp_io_flush_stdio` for the writes, an EINTR rule per body, a `Bytes` return whose length is
rewritten, and the two `pollfd` bodies. The family is taken whole even though two of its members
(`write_bytes_raw`, `set_nonblocking_raw`) are reached by nothing, because the seam is the
descriptor and a partial family leaves the next fixture inside it.
Reaches: `io_stdio` and `io_streams` complete (2 fixtures). Moves the head of three more:
`fs_fallible`'s next refusal becomes `std.fs.write_text_raw`, `process_spawn`'s becomes
`std.process.arg_count_raw`, `net_poll`'s becomes `std.net.connect_raw`. Also on the path of the
projects whose own head is `std.io.read_raw`: `caps_fs_breadth`, `process_run`, `process_in`,
`process_denied`.
Acceptance: `io_stdio` and `io_streams` build, run under the harness's piped stdin, and match C
bytes; valgrind clean over both (they are in the gate's fixture list); the three predicted next
refusals are confirmed by hand.

### S3. The capability check (~300-500 lines, no extern bodies)

The gate has its own specification, written in the same round against the same tip:
`plan/asm-backend/capability-check.md` (the emitted shape at the call site, the grants table, the check
routines, the deny path's exact text, and its own two-step order). This slice is the same seam seen
from the extern side. Two facts it adds to that file's acceptance list: the condition that makes the
emission match the oracle, and which fixtures and projects it turns into builds.

Owns the call-site emission, the check routines, the allowlist data, and the condition that makes
it match the oracle: emit a check only when the calling module holds patterns for the domain
(`codegen_c.rs:3415`), mirroring `codegen_c.rs:3409-3470` and `kelpc/runtime/cap_glue.c`. The
allowlist symbols the C side generates (`CapTable::symbol`) and the check's arguments (domain,
resource, patterns, count, module, site) are the shape to copy.
Reaches: no fixture by itself, and that is the point of keeping it separate: it is a prerequisite
of nine fixtures and of thirteen `caps_*` projects, and it is the one mechanism whose mistake
widens a grant rather than producing a wrong number.
Acceptance: with S4 landed, `caps_gated_path` and `fs_errno` build and print the C backend's bytes
(the emission half); with the extern family each deny project uses landed, `caps_denied`,
`caps_symlink`, `caps_link_denied`, `caps_env_denied`, `caps_net_denied` and
`caps_net_resolve_denied` build and print the C backend's denial message (the deny half);
`kelpc/tests/cap_matcher.rs` stays the matcher's control; and `fs_symlink_kind` builds
once S4 to S6 are in, which is the measurement that settles section 1.3's ambiguity.

### S4. The `fopen`/`stat` path family (13 externs, ~400-600 lines)

Owns `std.fs.read_raw`, `write_raw`, `write_text_raw`, `append_raw`, `size_raw`, `mtime_raw`,
`mode_raw`, `exists_raw`, `is_file_raw`, `is_dir_raw`, `remove_raw`, `rename_raw`, `touch_raw`.
Mechanism: one path in `rdi`, one or two C stdio/stat calls, `struct stat` on the frame with its
three fields that matter (`st_mode` 24, `st_size` 48, `st_mtime` 88), errno through
`__errno_location@PLT`, and an owned `Bytes` out of `read_raw`.
Reaches: with S3, `caps_gated_path` and `fs_errno` complete (2 fixtures), and so do the three
projects whose whole set this family plus S3 is: `caps_granted`, `caps_denied` and `caps_symlink`,
all three `fs.read_text` -> `read_raw` (plus `exists`/`is_file` in `caps_granted`). `caps_write`
needs S6 as well, for `fs.mkdir`; `caps_link` and `caps_fs_breadth` need S5 and S6 too. This
family is also the head mover for `fs_symlink_kind` (already moved to `write_text_raw`) and part
of `fs_fallible`.
Acceptance: both fixtures build and match C bytes; `fs_fallible`'s next refusal becomes
`std.io.errno_text_raw` if S2 is in, otherwise `std.fs.try_read_raw`; the three projects named
above build.

### S5. The open, copy and sync family (3 externs, ~200-300 lines)

Owns `std.fs.open_raw`, `copy_raw`, `sync_raw`. Mechanism: a mode table to `O_*` flags, a
descriptor returned into the program, and `sendfile` over two descriptors in one frame.
Reaches: no fixture. The only project that reaches any of it is `caps_fs_breadth`, through
`fs.open`; `sync_raw` is reached by nothing and `copy_raw` only by that project's `fs.copy`.
Acceptance: `caps_fs_breadth` builds and matches C bytes once S3, S4, S6 and S7 are in;
`--backend=asm` on the project is the check.

### S6. The directory and link family (7 externs, ~250-400 lines)

Owns `std.fs.mkdir_raw`, `mkdir_all_raw`, `list_raw`, `symlink_raw`, `read_link_raw`,
`is_symlink_raw`, `temp_path_raw`. Mechanism: a fixed `PATH_MAX` stack buffer and a prefix loop
(`mkdir_all`), `scandir` with a `struct dirent` whose `d_name` offset the routine needs, and
`lstat`/`readlink`/`symlink`/`mkstemp`.
Reaches: with S3 and S4, `fs_symlink_kind` completes (1 fixture, and the trap in section 1.3 is
the risk). It also carries `caps_list`, `caps_link`, `caps_link_denied`, `caps_fs_breadth`.
Acceptance: the fixture builds, matches C bytes, and valgrind is clean (it is in the memory gate);
`caps_list` builds.

### S7. The fallible pair (3 externs, ~200-300 lines)

Owns `std.fs.try_read_raw`, `std.fs.try_open_raw`, `std.fs.utf8_valid_raw`. Mechanism: the S4
bodies with an out-parameter (`mut i32`) written on every path, one `kelp_bytes_drop` on the
failure path, and one call to the existing `Routine::Utf8Valid`.
Reaches: with S2, S3 and S4, `fs_fallible` completes (1 fixture). Acceptance: it builds and
matches C bytes, including the errno values it prints (2 and 22) and the UTF-8 failure whose errno
is 0; valgrind clean (the dropped buffer is exactly the leak this slice can introduce).

### S8. The socket family (13 externs, ~600-900 lines)

Owns all of `std/net`: `connect_raw`, `listen_raw`, `accept_raw`, `close_raw`,
`shutdown_write_raw`, `send_raw`, `send_all_raw`, `recv_raw`, `recv_into_raw`, `set_timeout_raw`,
`local_port_raw`, `peer_addr_raw`, `resolve_raw`. Mechanism: the `addrinfo`/`sockaddr`/`sockaddr_in`
layouts (which no other body has needed), `getaddrinfo` and its `freeaddrinfo`, `inet_ntop`,
`sockaddr_storage` plus `getsockname`, and three timeout/errno rules (`EINTR` retry,
`EAGAIN`/`EWOULDBLOCK` becoming the timed-out panic, `MSG_NOSIGNAL`).
Reaches: `net_loopback`, `net_timeout`, `net_resolve` and `net_poll` complete (4 fixtures, with
S2 for the poll pair and S3 for the gate), plus `caps_net`, `caps_net_denied`,
`caps_net_resolve_denied`, `http_get`, `http_pool_off`, `http_redirect`, `http_redirect_denied`.
Acceptance: the four fixtures build, run and match C bytes; `caps_net`'s `send`/`recv_into`/
`peer_addr` lines match; the loopback fixtures are self-consistent by construction (section 5c),
so the C byte comparison is the only real check.

### S9. The process family (10 externs, ~400-600 lines)

Owns all of `std/process`: `arg_count_raw`, `arg_at_raw`, `spawn_raw`, `reap_raw`, `kill_raw`,
`exit_raw`, `exists_raw`, `executable_raw`, `is_dir_raw`, `getpid_raw`. Mechanism: three pipes,
`fork`, `dup2`, `execvp`, `waitpid` status decoding, and the two `argc`/`argv` globals the emitted
`main` must fill (new prologue work at `codegen_asm.rs:5155`, mirroring `codegen_c.rs:81-82`).
Reaches: `process_spawn` completes (1 fixture, with S2 and S3), plus `process_run`, `process_in`,
`process_denied`, `process_argv_only`. Acceptance: the fixture builds, matches C bytes (codes 0,
7, 127, 137 and the captured streams), and valgrind is clean: `spawn_raw`'s child-side `argv`
allocation is the leak the C body's own comment says the memory gate already found on this exact
fixture.

### S10. The environment family (4 externs, ~100-150 lines)

Owns `std.env.has_raw`, `get_raw`, `set_raw`, `unset_raw`. Mechanism: `getenv` (a static `char*`,
so the `String` copy matters), `setenv`, `unsetenv`, errno on the setters.
Reaches: `caps_env`, `caps_env_denied` (projects, with S3 for the deny one). No fixture.
Acceptance: both projects build and match C bytes.

**Total: 69 extern bodies (64 that the corpus reaches and 5 it does not) and one mechanism, in ten
slices, about 3,000 to 4,200 lines.** The cheapest useful round is S1 plus S2: 16 externs,
~500-750 lines, three fixtures complete (`caps_time_rand`, `io_stdio`, `io_streams`), three more
have their refusal moved, the section 13 gate artifact becomes measurable, and no
capability-check work is needed. The heap row's other two members are other slices' work and not
this file's: `list_datac_clone_sort` is the `List` derived clone/order slice and
`borrow_chain_through_a_borrow` is the borrow-chain slice.

## 4. Leverage, with the arithmetic

Two different counts answer "unblocks the most", and they are not the same question.

**(a) First-refusal reach.** A slice's first extern is what moves a fixture one step. Distribution
over the eleven heap-row extern fixtures:

| first extern | fixtures | count |
| --- | --- | --- |
| `std.io.is_open_raw` | `fs_fallible`, `io_stdio`, `io_streams` | 3 |
| `std.fs.read_raw` | `caps_gated_path`, `fs_errno` | 2 |
| `std.net.connect_raw` | `net_loopback`, `net_timeout` | 2 |
| `std.fs.write_text_raw` | `fs_symlink_kind` | 1 |
| `std.io.read_raw` | `process_spawn` | 1 |
| `std.io.wait_readable_raw` | `net_poll` | 1 |
| `std.net.resolve_raw` | `net_resolve` | 1 |

`std.io.is_open_raw` is the single extern that advances the most fixtures at once: 3 of 11, the
only extern above 2. It is also the first member of the family that S2 takes whole.

**(b) Completion reach.** A fixture builds when *every* extern in its set and, where section 2.5
says so, the gate are in. Counted that way, no single extern builds more than one fixture except:

- `std.fs.read_raw` is the **whole** set of `caps_gated_path` and `fs_errno`, so with S3 landed it
  completes 2 fixtures at once, which is the largest completion count any one extern has;
- every other extern is one of several for its fixture, so landing it alone changes no fixture's
  `BUILDS` column. `io.is_open_raw`, on this count, completes 0.

So the honest answer is: **`std.fs.read_raw` (2 completions, after the capability check)**, and if
the gate is not in the round, **`std.io.is_open_raw` (3 advances, 0 completions)**. Neither builds
a fixture by itself, and that is a property of the whole surface rather than of any extern.

**(c) Whole-set completion, which is how a round should be sized.**

| set | externs | fixtures it completes (with S3 where needed) |
| --- | --- | --- |
| S1 time+rand | 5 | 1 (`caps_time_rand`), no gate |
| S2 io | 11 | 2 (`io_stdio`, `io_streams`), no gate |
| S1+S2 | 16 | 3, no gate |
| S1+S2+S3+S4+S5+S6 | 39 | 6 (`caps_time_rand`, `io_stdio`, `io_streams`, `caps_gated_path`, `fs_errno`, `fs_symlink_kind`; `fs_fallible` needs S7 too) |
| all ten slices | 69 | the 12 extern-headed programs: the 11 heap-row fixtures plus `caps_time_rand` |

`fs_fallible` needs S2 (three io externs), S3, S4 and S7, which is the only fixture whose set
spans three slices after S2 and S3.

**(d) Dead weight, and the something else it waits for.**

- Dead for the whole corpus (5 externs, reached by no fixture and no project): `std.fs.append_raw`,
  `std.fs.sync_raw`, `std.io.write_bytes_raw`, `std.io.set_nonblocking_raw`,
  `std.process.exit_raw`. They are body-shaped work with no acceptance criterion, so they should be
  written when their family lands (S4, S2, S2, S2, S9) and not before.
- Dead until `std/fs`'s own path family lands (S4): `std.fs.utf8_valid_raw`, `write_raw`,
  `try_read_raw`, `try_open_raw` and `std.io.errno_text_raw`. None of them is anybody's first
  refusal, and each is needed only by `fs_fallible`, whose head is `std.io.is_open_raw` (S2) and
  which additionally needs S3. Landing them first changes no build.
- Dead until the socket family's core lands (S8's `listen_raw`/`connect_raw`/`accept_raw`/
  `send_all_raw`/`recv_raw`/`close_raw`): `std.net.shutdown_write_raw`, `set_timeout_raw`,
  `resolve_raw`, `send_raw`, `recv_into_raw`, `peer_addr_raw`. The first three are tails of the
  net fixtures; the last three are reached by `caps_net` only.
- Dead until S2's `close_raw`/`read_raw`/`write_text_raw`/`wait_readable_raw` land:
  `std.process.arg_count_raw`, `arg_at_raw`, `getpid_raw` and `kill_raw`. `process_spawn`'s head is
  `std.io.read_raw`, so none of the six process externs moves anything before S2.
- Dead until S3 lands, in the sense that landing it changes no build: the gate itself. It is the
  one entry here that is a prerequisite rather than dead weight, and the reason to take it third
  rather than last.

## 5. The traps

### a. Bodies that can move owned memory (leak or double free)

- `std.fs.read_raw` and `try_read_raw` allocate before they know whether the read succeeds.
  `try_read_raw` (`externs.rs:272`) allocates, fails a short read, `kelp_bytes_drop`s that buffer
  and returns a *second*, empty allocation: two allocations and one drop on one path. A routine
  that drops the returned value leaks, and one that returns a `Bytes` whose `data` points into the
  frame is a use-after-return the valgrind gate sees as "invalid read" only if something reads it.
- `std.fs.list_raw` (`:212`) `free`s the `scandir` array and each `dirent`, allocates a `Bytes`
  sized from the names, writes the names, rewrites `b.len` and the NUL, and hands the allocation to
  `String.from_bytes`. Both the `free` loop and the `{data, len, cap}` invariants (len < cap, NUL at
  `data[len]`) have to be right, or the later `String` drop frees the wrong thing.
- `std.io.read_raw`, `std.net.recv_raw`, `std.net.recv_into_raw` allocate then shorten; the result
  is a value the caller's drop plan owns, so it must be a real allocation, exactly once.
  `read_into_raw`/`recv_into_raw` are the opposite trap: they write through the caller's buffer and
  must not allocate, grow, or touch the length.
- `std.fs.read_link_raw`, `cwd_raw`, `temp_path_raw`, `std.net.peer_addr_raw`,
  `resolve_raw`, `std.process.arg_at_raw`, `std.io.errno_text_raw`, `std.env.get_raw` all build a
  `String` from a stack buffer, a static buffer, or `argv`. The C copies in every case; a routine
  that aliases instead hands the caller a pointer into a frame or a static, and `errno_text_raw`'s
  `strerror` buffer is a static a later `strerror` overwrites.
- `std.process.spawn_raw` (`:425`): the child's `malloc`ed `argv` must be freed before `_exit`
  (the body's comment names `fixtures/process_spawn.kp` as the case the valgrind gate caught), the
  six child-side pipe ends and three parent-side ends must be closed, and the parent must hand back
  the *parent's* ends in the three out-parameters. A leak here is memory, a wrong end is a hang.
- `std.path.normalize_raw` (`:312`, landed) is the model for the one case where nothing may be
  dropped: its `kelp_Bytes` is a view of a stack array and is deliberately never dropped.

### b. Bodies that consult errno

Nearly all of them, and the traps are the ones with a rule rather than a single check:

- errno must be read through `__errno_location@PLT` (`codegen_asm.rs:12910` is the existing
  example), and the failing call's value must be captured *before* any cleanup call
  (`fclose`/`close`) can overwrite it, which is why the C's `kelp_panic_at` takes the code as an
  argument.
- `std.io.wait_readable_raw`/`wait_writable_raw` (`:384`, `:389`): `poll` in an `EINTR` retry loop,
  and the answer is a `revents` bit test that includes `POLLHUP|POLLERR` for readability.
- `std.net.recv_raw`/`recv_into_raw` (`:483`, `:488`): `EINTR` retries, and `EAGAIN`/`EWOULDBLOCK`
  becomes the `net.recv: timed out` panic that `net_timeout` prints.
- `std.io.read_into_raw` (`:354`): `room == 0` returns 0 **without a syscall**, so a routine that
  reads errno unconditionally would panic on a stale value.
- `std.fs.mkdir_all_raw` (`:202`): `mkdir` failures are tolerated when `errno == EEXIST`.
- `std.fs.try_open_raw` (`:277`): an unknown mode writes `EINVAL` (22) without a syscall, and
  success writes 0.
- `std.process.reap_raw` (`:430`): `waitpid`'s `EINTR` retry, and the `status` word then decoded by
  four macros the routine has to expand itself.
- `std.time.sleep_ms_raw` (`:297`): `nanosleep` restarts with the remainder on interruption.
- `std.rand.u64_raw`/`fill_raw` (`:302`, `:307`): errno only for the `fopen` failure, which panics.

### c. Out-parameters the fixture reads back, where the differential sweep is weakest

- `std.fs.try_read_raw`/`try_open_raw`'s `mut i32 err`. `fs_fallible` prints `e.errno.to_i64()` for
  a missing file (2) and for a bad mode (22), so it is the strongest stdout check in the family and
  the only one that pins a scalar out-parameter. A routine that writes `err` only on failure
  prints the same values for the failing calls and is still wrong on the succeeding ones.
- `std.io.read_into_raw`'s effect on the caller's buffer plus its returned count.
  `io_streams` prints the count (0 at end of file) and `buf.len()` (unchanged by design), so the
  bytes written into the buffer are never printed: a `read_into` that writes to the wrong offset is
  invisible to stdout.
- `std.process.spawn_raw`'s four out-parameters. `process_spawn` writes to `child.stdin` and reads
  `child.stdout`, so a swapped parent-side end usually hangs or prints plausible text rather than
  the wrong bytes, and the fd numbers themselves are never printed.
- `std.net.local_port_raw`'s `getsockname` read-back. Every loopback fixture connects to the port
  the listener reports, so both ends are the same program: a wrong `sin_port` offset or a wrong
  `ntohs` is still *self-consistent* and works. Only the C byte comparison catches it, which is
  what makes the net fixtures the weakest place for a stdout-only sweep.
- `std.process.reap_raw`'s status decode is read back: `process_spawn` prints exit codes 0, 7, 127
  and 128 + 9, so `WIFEXITED`/`WEXITSTATUS`/`WIFSIGNALED`/`WTERMSIG` are pinned by stdout.
- `std.fs.list_raw`'s byte layout is read back through `str.split`: a missing trailing-newline trim
  shows up as one extra entry in a printed count (`fs_symlink_kind` prints `found.len()`), so a
  list-body mistake is visible, but a *sorted* order mistake (the `alphasort` argument) moves which
  fixture's entries are walked first and is only visible in `caps_list`'s output.
- `std.fs.try_read_text`'s UTF-8 branch prints errno 0 with a message, so `utf8_valid_raw` is
  pinned by stdout rather than by a syscall.

## 6. What I could not determine

- **Whether each fixture builds once its set and the gate land.** The compiler stops at the first
  refusal, so every fixture is a first refusal and not a build; the tails in section 1 are source
  reads with a measured head. The settling measurement is one build per fixture after each slice.
- **Whether the asm capability check must be conditioned on the calling module.** The C side is
  measured (`codegen_c.rs:3415`, the internal `std/fs` calls carry no check in the generated C),
  and the asm `cap_check_refusal` as written has no caller parameter, but the refusal is masked by
  the extern pre-pass today. `fs_symlink_kind` after S4 to S6 is the measurement.
- **The walked order beyond the five cases measured.** `std/fs`-before-`std/str`, `std/io` before
  `std/fs`, `std/io` before `std/net` and `std/io` before `std/process` are measured; the
  "predicted next refusal" lines in section 3 rest on that rule and not on a build.
- **The socket families other than `AF_INET` loopback.** `connect_raw` walks a whole `addrinfo`
  list and `local_port_raw`/`recv_into_raw` have `AF_INET6` arms; every fixture and project that
  runs is `127.0.0.1` over `AF_INET`, so those arms have no acceptance target. `net_resolve` only
  asserts `len > 0`.
- **Whether the 66-extern union is complete for the asm suite.** It is the union of the 202
  C-built fixtures' extern reach and the 21 projects that refuse at an extern, plus the projects
  that build; a project that refuses for another reason first could hide externs behind its own
  gap.
- **Whether the five unreached externs are needed by anything outside `kelpc/tests`** (`fpl`,
  `coral`, `lighthouse`, `bench/`). Only `kelpc/tests/fixtures` and `kelpc/tests/projects` were
  swept.
- **Whether the `net` bodies in section 2.3 are the ones a later round wants.**
  `kelpc/runtime/resolver_conf.kp` (MK7 section 7 item 6) is a stub resolver written in Kelp whose
  transport half is explicitly blocked on these syscall externs. If that slice lands first,
  `std.net.resolve_raw` and the resolution inside `std.net.connect_raw` may be replaced rather than
  transliterated, and S8's `resolve_raw`/`connect_raw` entries describe the C bodies as they stand
  at this tip.
- **The line estimates in section 3** are this plan's usual kind of estimate and not a measurement;
  the extern counts, the fixture sets and the refusal order in this file are measurements.
