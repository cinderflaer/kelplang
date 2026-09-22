# Differential testing as a language feature

*Proposal, not commitment and not a Mark. `roadmap.md` owns the plan and the schedule; nothing here is
scheduled, no work has started, and no lane is open. What follows is the idea and the design questions it
would have to answer, not an answer to them.*

## What it is

`kelpc`'s own test suite rests on a differential mechanism. Two independent implementations of one
behaviour are run and compared: the C backend is the project's **oracle** that the LLVM backend is
measured against (`plan/architecture.md`), `kelpc/tests/backend_coverage.rs` sweeps the corpus through
both, and `differential_fuzz.rs` generates programs and builds each with both backends, comparing stdout
and exit status. The project trusts the mechanism enough to gate the default backend on it.

The idea is to **turn that mechanism outwards**: let a writer say that two functions must behave the
same, and have the compiler generate cases and check it.

```kelp
[equivalent_to(slow_but_obviously_correct_sort)]
fun fast_sort(xs: List<i32>).R: List<i32> {
    ...
}
```

The compiler generates cases, runs both, and reports a disagreement.

**On the spelling.** The example above is written in Kelp's own bracket-head shape, `[docs(<kind>:
<name>)]` (`syntax.md` 206-216), and not in Rust's `#[...]` form. That mechanism exists today, but it
exists for `docs` alone, which is documentation and carries no semantics for the compiler to act on.
This would be the first head whose presence changes what the compiler *does*, so it is a language change
and not a comment: the head has to be parsed, checked, and given a meaning, and it has to be decided
whether an unknown head is an error.

## The design work it needs

Four questions, in the order the last one is hardest.

1. **How are cases generated?** From the parameter types, from the function's other arguments, from a
   seed the build fixes? Is there shrinking when a disagreement is found, and does a failing case get
   written out as a source-level reproducer?
2. **What does "equivalent" mean for a function that is not deterministic or not pure?** A function that
   reads the clock, draws from `rand`, or reads a file can disagree with itself between two calls
   without either implementation being wrong. Either those functions are out of scope, or the head has
   to say which effects are being ignored and how.
3. **Where do the cases run?** At build time, so a disagreement is a build failure; at test time, in a
   suite the program runs; or both. A build that generates and runs cases is a build that executes
   arbitrary program code, which is a different promise from compiling.
4. **How does it compose with the capability system?** This is the hard one. Running either function is
   an *effect*, and a grant is checked at a call site a program wrote. A compiler-generated call site is
   not one a program wrote, so the questions are whose authority it runs under, whether an equivalence
   check may run an effectful function at all, and what the compiler is allowed to do with a grant that
   the manifest did not give it. Nothing in the capability model answers that today, which is why this
   needs a design pass before anyone writes code.

## Why it is interesting

No mainstream language ships this as a built-in. Differential testing is normally an external fuzzer
bolted onto a project that has to build the harness, the corpus and the comparison itself. A language
that can *declare* the relationship has the property the compiler's own suite already relies on, and
that is the same argument the checker makes elsewhere: a fact the compiler can check is better than a
fact a test hoped to catch.

## Out of scope

- Any implementation work.
- Whether it is a head at all, rather than something only a test harness understands.

## Status

Behind the current critical path (threads, then FFI). It also sits next to MK11-S3's testing stage (D-4),
which is the plan's own differential and fuzz work: if this ever ships, it is the user-facing half of
that stage rather than a separate idea.
