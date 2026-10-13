# Capability threat model (task tl3)

Attack tests: `kelpc/tests/threat_model.rs`, one test per numbered item below
(an item with several shapes loops over them). Every attack touches only files
under a directory the test creates, dials only `127.0.0.1` (a refused name is
refused before any dial), and runs nothing but tiny `echo` scripts it writes.

## Assets

- Files outside a module's `fs` grant (the lab's `secret/s.txt`).
- Programs a module may not start; processes it may not signal.
- Environment variables outside the `env` grant (and the loader variables that
  change what a later process does).
- Hosts, ports and interfaces outside the `net` grant.
- Device authority: `mmio` ranges, `irq` lines, `frames`.
- The audit lists: `[unsafe] modules`, `[extern."module"]`.
- Descriptors another module opened.

## Attacker

1. A program author whose manifest grants a narrow window and who wants more.
2. A dependency author: a module in `src/` that holds fewer (or no) grants than
   the module that imports it, and can write any Kelp source. (There is no
   separate `deps/` loader: a dependency is a module of the project, and its
   authority is exactly what its importers delegate.)

## In scope / out of scope

In scope: the compiler's gates (resolution, call-site checks, the glue in
`cap_glue.c`, the matcher in `std/rt.kp`), the std modules' own bodies and
externs, the manifest validators.

Out of scope: the C compiler and linker; the OS and other processes; native
code reached through a granted FFI section or a granted program (a shell grant
is a grant of everything the shell runs, P7); races with another process
changing the filesystem between check and use (TOCTOU); a granted module's own
bugs (a deputy that reads any path for its caller uses *its* grant); DNS
trust for name grants (a grant on `example.com` trusts the resolver); the
manifest author (root of trust).

## Attack list

Numbers are the test names (`f01_...`).

fs: F1 `..`; F2 `.` and `//`; F3 sibling sharing the prefix; F4 relative path
vs cwd; F5 case; F6 embedded NUL; F7 path over `PATH_MAX`; F8 Unicode
look-alikes; F9 `fs.symlink` out of the grant; F10 existing links: read,
dangling write, dangling in a directory, relative, chained, `mkdir_all`,
`read_link`, `touch`; F11 `read_link`; F12 no hard-link primitive; F13
`/proc/self/*`, `/dev/fd`; F14 glob characters in a resource; F15 grant
patterns with `..`/relative; F16 no grant, every spelling; F17 `rename`; F18
`copy`; F19 `list`/`walk`/`remove_all` through links; F20 `temp_path`
prefix; F21 `open` mode vs path.

process: P1 no grant; P2 unlisted program; P3 bare name / `PATH`; P4 `..`
and file-as-directory; P5 granted name that links elsewhere; P6 argument
injection; P7 a shell grant (documented breadth); P8 `kill` of a foreign pid;
P9 forged `Child`.

env: E1 every operation outside the grant; E2 another module's variable; E3
wildcard names; E4 no listing; E5 `LD_PRELOAD`/`PATH`; E6 `env` + `process`.

net: N9 TLS CA paths; N1 hosts outside the grant (aliases, numeric spellings, suffix, injected
port, whitespace); N2 name vs address; N3 ports; N4/N4b numeric pattern vs a
DNS name; N5/N5b loopback grant and the bound interface (raw and http); N6
listen ports; N7 redirect to an unlisted host; N8 `ws`/`tls` behind the gate.

handles: H1 forging a descriptor handle with no grant; H2 one domain mints only
its own handles; H3 forging between two granted modules (known gap).

unsafe/asm: U1 block and `unsafe fun`; U2 twelve expression positions; U3 trait
default method; U4 impl method and generic; U5 `asm`, `naked fun`; U6
initializers; U7 unsafe reached through a listed wrapper only.

mmio: M1 no grant; M2 oversized shifts in constant folding; M3 computed
address; M4 widening by delegation; M5 offset after birth (known gap).

indirect: I1 closures from a granted module; I2 generics; I3 trait defaults,
impls, `Drop`, `defer`, scoped blocks, `pd` initializers; I4 re-export and
self-grant; I5 user `extern`.

delegation/manifest: D1 delegating more than held; D2 `caps(*)`; D3 `[caps]`
narrows the entry; D4 module ids and shadowing `std/rt`; D5 extern allowlist.

`irq`/`frames` are covered by `tests/irq_frames.rs` (forged `Line`/`Frame`
sealed, ungranted, outside the range); `[extern]` header/library injection and
`ui` by `compile_and_run` fixtures (`ffi_bad_*`, `ffi_ui_*`).

## Findings fixed

1. **F10 write through a dangling symlink** (`cap_glue.c`). The resolver
   resolved the deepest existing ancestor and appended the rest verbatim, so
   `fs.write_text("data/dangling", ..)` with `dangling -> secret/new.txt`
   created a file outside the grant. The resolver now `readlink`s the first
   missing component and restarts (40 hops, loop not recursion).
2. **P8 `process.kill` of any pid.** A grant for one harmless program let a
   module signal every process of the user (`kill_raw`, or a forged `Child`).
   The runtime now keeps the pids `spawn` started; `kill` and `wait` accept only
   those, and a reaped pid leaves the table.
3. **N5 loopback grant, public listener.** `net.listen` bound `0.0.0.0`
   whatever the grant said (also `http.listen`). `kelp_cap_check_port` now
   records whether any grant host admits every interface; if not, `listen_raw`
   binds `127.0.0.1`.
4. **N4 numeric pattern matching a DNS name.** `10.0.0.*:80` admitted
   `10.0.0.xyz`, a name its owner can point anywhere. A numeric-shaped pattern
   now admits only a dotted-quad IPv4 literal (`connect`, `resolve`).
5. **H1 forged descriptor handles.** `io.File(fd = n)` (and `net.Socket`,
   `Listener`, `process.Child`, the http/ws/tls connection types) could be built
   by a module with no grant at all, giving it every descriptor the process held.
   Constructing one outside the library now needs a domain that mints that kind.
6. **U3 `unsafe` in a trait default method** bypassed `[unsafe] modules`: the
   audit walk skipped trait declarations. It now scans them.
7. **M2 oversized shift folded into an mmio range.** `0x10000000 + (0x100000 <<
   70)` folded to the granted address while the target shifts by `n mod 64`.
   Shifts of 64 or more bits are no longer constants (also used by `irq` and
   `frames`).
8. **Narrow-type shifts** (`tl7`). A shift by the width or more was poison on
   LLVM and 0 or garbage in C. It is now one rule on both backends, hosted and
   freestanding: an amount outside `0..width` (negative included) panics with
   `shift amount out of range`; a constant amount out of range is a compile error
   naming the type and the amount; `x.wrapping_shl(n)`/`wrapping_shr(n)` mask the
   amount. The `mmio`/`irq`/`frames` constant folder now folds at the operand's
   own width (a `u32` line wraps at 32 bits, as the program does) instead of 64.
   Tests: `shift_*` fixtures in `compile_and_run.rs` (both backends, freestanding
   boot) and `a_line_folds_at_its_own_width` in `irq_frames.rs`.
9. **TLS `ca_file`/`ca_dir`** (`tl7`, N9). The handshake opened these paths with
   no `fs` check. They are removed: `TlsOptions`/`TlsClientOptions` take PEM
   bytes (`ca_pem`, `with_ca_pem`), the runtime never opens a path, and a file
   is read with `tlsfile.ca_pem(path)`, which is the `fs` gate: a module with no
   `fs` grant is refused at compile time ("needs the 'fs' capability"), a path
   outside the grant is the ordinary `fs` denial and is never opened. `ca_dir`
   has no bytes form and is gone. Tests: `n09`, `n09b`, `n09c`.

## Known gaps

- **H3 descriptors between two granted modules** (`h03`, ignored). Module B with
  `fs = data/**` runs `io.File(fd = 3).read(..)` and reads a descriptor module A
  opened on `secret/`. Design: make the handle types opaque (no integer
  constructor outside the library; `extern type` handles already exist for
  `TlsHandle`), add a gated `from_fd` for the legitimate hand-over to a thread
  (`tests/hazards/tls_server_matrix.kp` does this today), and consider `O_CLOEXEC`
  everywhere.
- **M5 pointer offsets after birth** (`m05`, ignored): the mmio check is on the
  address a pointer is born at. Design: provenance, roadmap D-1
  (`language/capabilities.md` (kelp wiki) "Born-at only").
- **Confused deputies and unions** are by design: a module that reads any path
  with a broad grant serves any caller; grants made to one module by several
  importers are unioned, each bounded by what its importer holds.
- **Process children** inherit open descriptors (no close-on-exec sweep).

## Table

| domain | enforced by | attacks (tests / shapes) | refused | found + fixed | known gaps |
|---|---|---|---|---|---|
| fs | resolution gate + `kelp_cap_check_path` (normalize, resolve links, glob) | 21 / ~60 | all but 1 | 1 (F10) | 0 |
| process | gate + program path check + child table | 9 / 17 | all but 1 | 1 (P8) | 0 (P7 is documented breadth) |
| env | gate + `kelp_cap_check_name` | 6 / 17 | all | 0 | 0 |
| net | gate + host/port/hostport checks + bind verdict; TLS CA bytes only via `fs` | 12 / 33 | all but 2 | 3 (N4, N5, N9) | 0 |
| handles | constructor rule | 3 / 7 | 7 of 8 shapes refused | 1 (H1) | 1 (H3) |
| unsafe/asm | `[unsafe] modules` audit walk, `unsafe` blocks | 7 / 24 | all but 1 | 1 (U3) | 0 |
| mmio/irq/frames | `Ptr.from_addr` fold + range check, sealed constructors | 5 / 12 (+ `irq_frames.rs`) | all but 1 | 2 (M2, narrow shifts) | 1 (M5) |
| indirect use | lexical module of the call site | 5 / 17 | all | 0 | 0 |
| delegation/manifest | `pattern_covered`, validators, shipped-only `std/` ids | 5 / 20 | all | 0 | 0 |
