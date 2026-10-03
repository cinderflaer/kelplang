# Compile-time scaling

`kelpc/tests/scaling.rs` generates large programs (method chains, nested
calls, nested generics, nested blocks and function literals, long `+`/`&&`
chains, a 5,000-element list literal, long templates, a 300-arm `elif` ladder,
a 200-field datac, 300 statements) and runs `check`, `fmt`, `lint`, `doc` and
`build` on both backends over each, then runs both binaries. A step over 20 s
fails the test.

## The bug that was fixed

Type checking a method call inferred its receiver twice: once with no
expectation, to learn the receiver's type for method lookup, and again as the
`self` argument. In `a.f().f().f()...` each link therefore doubled the work of
the link below it (0.4 s at 16 links, 21 s at 22, a chain of 400 never ended).
Calls of generic functions and generic `datac`/`enum` constructions did the
same with their arguments (arguments probed for type inference, then checked
again against the instantiated signature), so `id(id(id(...)))` and
`W(v = W(v = ...))` were exponential too.

`Ctx::probes` now keeps the first result; `check_call_arg` reuses it when the
type is exactly the type the argument is expected to have, and checks again
(the old behaviour) otherwise. Entries are keyed by expression address and
span, consumed on use, and dropped whenever the checking that made them fails,
so a retry (the scoped-block exclusive-capture retry, a swallowed error) never
sees a stale one. No accepted or refused program and no diagnostic changed.

## Measured after the fix (debug build, this machine)

Nothing else was super-linear in `check`, `fmt`, `lint`, `doc`, or the C
backend. Every shape finishes each step in about a second or less except:

| input | step | time |
| --- | --- | --- |
| list literal, 5,000 elements | `build`, LLVM, -O2 | 4.0-4.6 s |
| list literal, 2,500 elements | `build`, LLVM, -O2 | 1.0 s |
| list literal, 1,250 elements | `build`, LLVM, -O2 | 0.5 s |
| 150 chained closures `val f_i = fun(x) = f_(i-1)(x) + 1` | `build`, C oracle | 1.6-2.2 s |
| 50 nested generic enum constructions | `build`, LLVM | 1.1-1.5 s |
| 150 chained closures | `build`, LLVM | 1.0 s |

Only the first row is super-linear: the time is quadratic in the length and
sits inside LLVM's optimizer, which is given the literal as one first-class
`[N x i32]` aggregate value (-O0 takes 0.85 s for 5,000 elements). The all
constant literal is now emitted as a single constant array instead of N
`insertvalue`s, which halved -O0 and left -O2 unchanged. Emitting a constant
array as a private global plus a copy would remove the rest; it was left alone
because it changes the shape of the IR every array literal produces.

The other rows scale linearly (the 150-closure and generic-enum timings double
when the size does).

## Limits that bound the rest

The parser refuses more than 800 links in a chain and more than 200 levels of
nesting (`plan/fuzzing.md`), so a 400-link chain, 90 nested calls and 60 nested
`if` blocks are about the largest shapes a program can reach; both limits were
kept.
