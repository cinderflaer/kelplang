# Fuzzing threat note

Two fuzzers cover different ground.

## What is fuzzed

1. `kelpc/tests/differential_fuzz.rs` (older): generates valid programs from a
   seed and compares the C oracle with the LLVM backend byte for byte. It
   answers "do the backends agree", never "does the compiler survive junk".
2. `kelpc/tests/frontend_fuzz.rs` (this note): mutates the 800+ fixtures in
   `kelpc/tests/fixtures/` and feeds the result to `kelpc check`. The property
   is survival: no panic, abort, stack overflow, hang or memory blowup. The
   outcome is either exit 0 or a diagnostic and exit 1.

Mutations (all deterministic, xorshift64*, mutation `i` is a pure function of
`FUZZ_SEED_BASE + i`): delete, duplicate or swap a token; delete, duplicate or
swap a line; truncate at a character boundary or at a byte (splitting a code
point); insert a keyword, type word, operator or bracket; nest brackets, prefix
operators, type arguments or blocks up to 100,000 deep (closed or not); replace a
number with a boundary or huge literal; insert odd Unicode, NUL bytes, bidi
controls, stray escapes; insert raw invalid UTF-8 bytes. One to three mutations
are stacked per mutant. Each mutant runs as a subprocess under
`ulimit -v 2097152; ulimit -t 20` plus a 40 s wall clock, half of them with
`--json` (the renderer's column arithmetic is a classic panic site). Fifteen of
sixteen go through `check --text` so imports of the original fixture still
resolve; the sixteenth is written to a file, which is the only route for
invalid UTF-8.

The front end covered is everything `check` runs: lexer, preprocessor, module
resolution, parser, type inference, borrow check, freestanding checks, and the
diagnostic renderers.

## What it found and what changed

Deeply nested or very long left-leaning input overflowed the stack (SIGABRT).
The parser now refuses more than 200 levels of nesting (expression, unary,
type, type argument, block, binding pattern) and more than 800 links in an
operator or access chain with a normal diagnostic, and the compiler does its
work on a 512 MiB-stack thread so a tree at the limit is safe in a debug build.
The inputs are kept, small, in `kelpc/tests/fuzz-corpus/frontend/` and replayed
by `the_frontend_regression_inputs_stay_survivable`. (The conformance corpus
test only reads `corpus_<seed>.kp` in the directory above, so these do not
mix.) After the fix 12,000 mutations on seeds 1,000,000.. and the default 2,000
were clean.

## What is NOT fuzzed

- LLVM backend IR validity beyond what the LLVM verifier checks; codegen is only
  reached by the differential fuzzer's valid programs, not by mutants (a mutant
  that passes `check` is not built).
- The linker and `cc`, the C compiler's handling of generated C, and the
  compiled programs' behavior (the sanitizer sweep, `plan/sanitizers.md`, covers
  runtime behavior of existing fixtures only).
- `kelpc build` option parsing, `coral.toml` manifest parsing (not mutated), the
  `fmt` and `lint` commands, the incremental cache.
- Semantic differences between mutants: only survival is checked, not that the
  diagnostic is a good one.

## Running it

```text
# default suite: 2,000 mutations, seeds 1..2000 (about 30 s)
cargo test --test frontend_fuzz

# replay one mutant
FUZZ_SEED_BASE=1004247 FUZZ_MUTATIONS=1 cargo test --test frontend_fuzz fuzz_frontend

# the maintainer's run: 100,000 mutations, seeds 1,000,000.. (an hour or more)
cargo test --release --test frontend_fuzz fuzz_frontend_long -- --ignored --nocapture
```

`FUZZ_SEED_BASE` and `FUZZ_MUTATIONS` override either test. A failing mutant is
saved as `FAIL_mutant_<seed>.kp` under `target/tmp/frontend_fuzz/<short|long>/`
and named in the failure message. To fix one: make the compiler print a proper
diagnostic (or add a depth limit with a clear error), then copy a minimized
version into `kelpc/tests/fuzz-corpus/frontend/`.
