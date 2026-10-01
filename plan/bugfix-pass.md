# Bugfix pass (release goal 2)

*One systematic pass over the ignored tests, the backend differential fuzz, the
compiler's own warnings and the handled-runtime hazards, as
`plan/release-goals.md` §2 asks. Every fix ships with the regression fixture
that failed first; nothing in the suite was deleted, skipped or weakened. LLVM
is the default backend and C is the oracle (`KELPC_ORACLE=c`), so each fix
below changes only the backend that was wrong and every new run-test passes on
both.*

## 1. Ignored tests

Run per binary, in the nix shell, one cargo process at a time.

| Binary | Ignored tests | Result |
|--------|---------------|--------|
| `differential_fuzz` | `the_generator_agrees_between_the_two_backends`, `the_conformance_corpus_agrees_between_the_two_backends`, `emit_the_conformance_corpus` | see §2 |
| `fmt` | `formatting_preserves_a_programs_output` | **pass** (174 s) |
| `memory_gate` | 5 sweeps | **skipped by environment**: `valgrind` is not on `PATH`. The sweeps do not fail; each prints the same missing-tool message the cheap non-ignored half prints, and passes |
| `backend_coverage` | 4 sweeps | **not run**, by the task's safety rule (they need over 5 GB). Left to the maintainer |

`gdb` is also absent, which is why the one known failure,
`llvm_debug_info::a_fault_resolves_to_a_kp_file_and_line`, still fails; it is
not this pass's.

## 2. Differential fuzz

The knob is `FUZZ_SEED_START`/`FUZZ_SEED_COUNT` (default `1`/`256`), read by
`tests/differential_fuzz.rs::fuzz_seeds`. Ran in `--release` with
`FUZZ_SEED_START=20000 FUZZ_SEED_COUNT=1500`, so 1500 programs beyond the
default, each built with both backends and compared byte-for-byte on stdout and
exit status, plus the 24-program checked-in corpus. (A first run of
`1000..3000` was stopped partway and superseded by this one on the fixed
source.)

Result: **1500 of 1500 built by both backends and agreed, 0 refused, 0
LLVM-only failures, 0 mismatches, every shape drawn.** The checked-in
24-program corpus also agreed. No divergence was hidden in the generator's
surface: the generator has no `u8`/`u16` shape, which is why the narrow-width
bug in §5 needed a hand-written probe rather than this sweep.

## 3. Compiler warnings

`cargo build` printed 20 warnings on `kelpc`'s own source, now zero. Read and
disposed of one by one:

| Warning | Disposition |
|---------|-------------|
| `codegen_c.rs:1444` unused local `names` in `write_coerce_env` | dead local (its twin in `write_coerce_thunk` is the used one), removed |
| `ast.rs:233` field `Param::span` never read | dead field; removed with its `parser.rs` initializer |
| `infer.rs:3792` static `EMPTY_DATACS` never used | dead; removed |
| `lint.rs` `comment_runs` / `else_if_links` never used | dead public wrappers; removed, the `_in` halves `findings` calls stay |
| `opt.rs:57` `OptLevel::name` never used | used only by its own round-trip test; `#[cfg(test)]` |
| `pretty.rs`, 16 items never used | the whole AST-printer module is dead (its own `fmt.rs` header names it as the rejected design); file and `mod pretty;` removed, stale reference in the `fmt.rs` header updated |
| `resolve.rs:107` `Project::domain_of` never used | dead; every caller uses `domain_of_with_extern`, whose doc no longer claims otherwise |
| `tests/fmt.rs` `Span::to` never used | **test-only artifact**: the file `#[path]`-includes `src/lexer.rs` on its own, so the parser's only caller is absent. `#[allow(dead_code)]` on the `mod`, with that reason |
| `tests/compile_and_run.rs` unused `TcpStream` | dead import, removed |

No warning pointed at a real defect: the unused `Result`s and unreachable
branches the task asks about were not present.

## 4. Handled-runtime hazards

Where the generated code and the runtime reach for an OS underneath. Not
changed in this pass, listed so the freestanding work can trip over them
knowingly.

| Assumption | Where | What a kernel path must supply |
|------------|-------|--------------------------------|
| `kelp_panic` family: `fprintf(stderr, ...)` then `exit`/`pthread_exit` | `runtime/kelp_rt.c` (definitions), `codegen_llvm.rs:10883-10887` (declarations), 27 `kelp_panic(...)` sites emitted by `codegen_c.rs` (Array/List/Map/Box bounds, checked conversions, `panic`), 62 in `externs.rs` std bodies | a console writer and a panic path with no OS underneath |
| `malloc`/`realloc`/`free` | `runtime/kelp_rt.c` (Bytes/String/List/Map growth and drops), `codegen_c.rs` (Box, Map, List, closure-environment allocation; 8 `malloc`, 65 `free`), `codegen_llvm.rs` (16 `malloc`, 90 `free`) | a user global/arena allocator, and a fallible allocation story |
| `atexit(kelp_drop_pds)` | `codegen_c.rs:865`, `codegen_llvm.rs:1282` (declared at `:5508`) | running the `pd` drops at process end; no atexit in a kernel |
| `printf`/`fprintf`/`snprintf` | `codegen_c.rs` `print` lowering (`printf_piece` and the `print`/`print!` arm), `runtime/kelp_rt.c` (12/7/4), `externs.rs` (11) | a formatted-output sink rather than stdio |
| `pthread_*` and `_Thread_local` | `runtime/kelp_rt.c` (21 uses), the `kelp_Thread` machinery | the kernel's own threads and per-CPU storage |
| `<stdatomic.h>` / `__atomic_*` | `codegen_c.rs` (14), `codegen_llvm.rs` (6), `Atomic<T>` | atomic primitives |
| `strlen`/`memcpy`/`memset`/`strcmp` | `runtime/kelp_rt.c`, `externs.rs` (the `str` builtins), generated code | freestanding string/memory primitives |
| sockets, files, process, time | `C_HEADERS` (`codegen_c.rs:86`), the std bodies in `externs.rs` (`fs.*`, `net.*`, `process.*`, `time.*`) | a syscall ABI with untrusted-pointer validation |

## 5. Adversarial programs

48 programs under `scratch/adv/` named `a*.kp` plus 8 more named `b*.kp`
(untracked), each built with both backends and run under `ulimit -v 1048576;
ulimit -t 10; timeout 10`: closures capturing the loop binding, moves out of
`Box`/`List`/`Map`, mutating a `List` inside `for` over it (bounded by the
desugar, always with the limits above), nested-`when` exhaustiveness, generic
enums, integer boundaries and narrowing, `str`/`View` bounds, `f32`/`f64`
arithmetic, `char`/`bool`, and empty collections. 54 compile and are compared;
2 of the `a*` are refused on purpose (a closure capturing a loop borrow, a
non-exhaustive nested `when`) and the refusal is the correct behaviour. **Six
programs found a divergence: one real compiler bug (fixed) and two classes of
unspecified value (left, with the reason).**

- **Real bug, fixed: `u8`/`u16` arithmetic was not narrowed to its declared
  width in the C backend** (`a14`, `a31`; minimal repro `b01`). C promotes
  both operands to `int`, so `u8 + u8` of 300 printed and compared as 300 where
  LLVM (working at `i8`) wrapped to 44, and `u16 * u16` widened to 3,000,000,000
  where the type says 24064. It affects `+ - * / % & | ^ << >> ~` and unary `-`
  and is observable without `print` too (a comparison, a container element).
  Fixed in `codegen_c.rs` by casting a narrow result back to
  `uint8_t`/`uint16_t`; `plan/syntax.md` already says `u8` arithmetic wraps at
  256. Regression fixture `tests/fixtures/int_narrow_width_arithmetic.kp`
  (fails before, passes after, both backends). The fix also makes a narrow
  value read from a field, a closure capture, an array or a `Map` come out
  narrowed (`b05`, `b06`, `b07`).
- **Unspecified, left: `i32::MIN / -1`** (and `% -1` at the edge) (`a35`,
  `a46`). This is undefined overflow: `plan/syntax.md` calls division by zero
  undefined, lists `+`/`-`/`*` as unchecked and never defines division
  overflow, and C's `INT_MIN / -1` is UB. LLVM's lowering and the C backend's
  `-fwrapv` fold pick different values. Left rather than defined in a bugfix
  pass; division by any other negative agrees (`a47_div_by_neg_one.kp`).
- **Unspecified, left: a `NaN`'s sign bit** (`b03`, `b08`). `0.0 / 0.0` prints
  `nan` from an LLVM build and `-nan` from the C build, because the two
  constant folders choose a different valid IEEE NaN (the runtime `divsd`
  result and the folded one differ in the sign bit, which IEEE leaves
  unspecified). `plan/syntax.md` says `nan` behaves "as usual" and names
  `math.is_nan` as the defined test, so this is not a bug; a program that
  prints a raw NaN is relying on an unspecified byte.

## 6. Docs touched

- `plan/bugfix-pass.md` (this file).
- `plan/user-ffi-gap-analysis.md`: no gap closed in this pass (nothing here
  touches the FFI surface).
