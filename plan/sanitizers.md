> **Removed 2026-10-10.** The C backend that built sanitized programs was removed
> in stage 2 step A. `--sanitize=` and `[build] sanitize` are now refused, and
> the text below describes the retired C-backend path.

# Sanitizer builds

```text
KELPC_ORACLE=c kelpc run   --sanitize=address file.kp
KELPC_ORACLE=c kelpc build --sanitize=undefined -o out file.kp
```

or `[build] sanitize = "address"` in `coral.toml` (the flag wins over the
manifest). Values: `address`, `undefined`, `thread`.

The C backend passes `-fsanitize=<kind> -fno-omit-frame-pointer -g` to `cc`,
which also links the sanitizer runtime; UBSan additionally gets
`-fno-sanitize-recover=undefined` so a finding ends the program with a nonzero
status instead of scrolling past. `-fwrapv` stays on, so signed overflow is
defined behavior and UBSan does not flag Kelp's wrapping arithmetic. Generated C
carries `#line` directives, so reports name the `.kp` file and line.

The LLVM backend (the default) does not link LLVM's sanitizer passes, so the
flag is refused there with a message naming `KELPC_ORACLE=c`. A freestanding
project cannot be sanitized (it is LLVM-only by design).

## What each catches in Kelp

The compiler is memory-safe by construction for safe code, so the value is in
the places that are not: `unsafe` blocks, `Ptr<T>` arithmetic and
`read`/`write`, `extern` FFI calls, inline `asm`, and the C runtime and extern
bodies (`kelpc/runtime/*.c`, `kelpc/src/externs.rs`).

- `address`: out-of-bounds and use-after-free through `Ptr`, FFI buffers, heap
  overflows in the runtime C, and leaks (LeakSanitizer is on).
- `undefined`: misaligned or too-small accesses through `Ptr`, bad shifts, null
  dereferences, `memcpy` with overlapping or null arguments in the runtime.
- `thread`: data races between `thread.spawn` threads, including races on state
  reached through `Ptr` or `static mut` that the safe-code rules do not cover.

`kelpc/tests/hazards/sanitizer_oob` is a project with a deliberate out-of-bounds
`Ptr` write; both ASan and UBSan stop it and name `main.kp:6`.

## The sweep

`kelpc/tests/run-sanitizers.sh [address|undefined|thread ...]` (inside the nix
shell, with `KELPC` pointing at a built compiler) picks runnable fixtures
automatically (no network, stdin, sleeps, or intentional failures; plain C build
runs to status 0 in 10 s), builds each with the sanitizer on the C backend, and
requires the same exit status and stdout as the plain build and no sanitizer
text on stderr. `SAN_COUNT` (default 40) is how many it must cover.
`kelpc/tests/sanitizers.rs` runs five fixtures through ASan and UBSan as part of
`cargo test`, plus the hazard project, the manifest key and the LLVM refusal.

Memory bound: sanitized programs run without `ulimit -v` (shadow memory needs
terabytes of address space) but under `ulimit -t 10`, `timeout 10`, and
`ASAN_OPTIONS=hard_rss_limit_mb=1024`.

## Findings

All three sanitizers were run over 376 runnable fixtures each (the whole
auto-selected set: of 807 fixtures, those without network, stdin, sleep or
intentional failure that exit 0 in the plain C build). Results:

- `undefined`: no findings. `thread`: no findings.
- `address`: two findings, `pool_drop_joins` and `pool_sum`, one root cause, a
  real runtime bug. A thread handle dropped without `thread.join` (what
  `std/pool` does for its workers) was documented as "detach the worker and leak
  the handle and its two slots" (`kelp_thread_drop` in `kelpc/runtime/kelp_rt.c`).
  LeakSanitizer reported 120 + 80 byte direct leaks from `kelp_thread_new`.
  Fixed: the handle carries a `state` word; the owner's drop and the worker's
  exit each swap in their own value, and whichever side finds the other already
  done frees the handle and its slots, so it is never freed while the worker may
  still write through it. A joined handle is unchanged. (A detached worker's
  produced result is still not dropped; only the three allocations are freed.)
  `pool_drop_joins` is among the fixtures `sanitizers.rs` runs under ASan, so
  the leak cannot return.

The sweep needs about 12 minutes per sanitizer on this machine for the full
set; the default `SAN_COUNT=40` takes about a minute.
