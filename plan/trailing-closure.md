# Trailing blocks (open item 75): design and prototype

*Design note plus a parser-only prototype. Not a Mark; `roadmap.md` still owns the schedule.*

## The proposal in one rule

A `{ ... }` on the same line as the end of a call is that call's **last argument**. It is written
the way `f(fun() { ... })` is written today, so the callback is a function value of the ordinary
kind and everything downstream of the parser is unchanged.

```kelp
twice { print("a") }          // twice(fun() { print("a") })
labelled("b") { print("c") }  // labelled("b", fun() { print("c") })
ui.column { ui.text("x") }    // ui.column(fun() { ui.text("x") })
```

## What the prototype does (`kelpc/src/parser.rs`)

- After a call's `)` (or after a bare name / `.method`), a same-line `{` is parsed as a block and
  appended as an unnamed `CallArg` holding a zero-parameter `fun` literal with no `.R:`.
- The `(` is optional when the block is the only argument: `column { ... }`, `ui.column { ... }`.
- It is **off** in the head of `if`, `elif`, `if let`, `while`, `for` and `when`, where a `{` after
  an expression already means "the body". A call's argument list and a block turn it back on, so
  `if f(run { work() }) { ... }` and `if cond { run { work() } }` both parse.
- A `{` on the next line is not attached: `run(1)` then `{ work() }` is two statements, as before.
- It only accepts text that was a parse error before, apart from the head cases above, which the
  flag keeps unchanged. The 208 unit tests, the 490 `compile_and_run` tests and the fmt tests
  passed on this branch.
- `kelpc fmt` leaves the fixtures byte-for-byte unchanged (checked on
  `tests/fixtures/trailing_block.kp`).
- A block that names an enclosing binding becomes a capturing literal, so it fits a `closure()`
  parameter (`tests/fixtures/trailing_block_capture.kp`).

## Why this is smaller than the roadmap's estimate

The roadmap says a trailing block leaves the callback parameter unnamed, so "the compiler has to
choose it by function type, which is a selection rule the language does not have". That holds only
if the block may bind to *any* function-typed parameter. Binding it to the **last positional
argument**, which is what Kotlin and Swift do, needs no selection rule: the parser appends it and
the existing argument-to-parameter matching does the rest. A function whose callback is not its
last parameter takes the parenthesised form, as it does today.

## What the prototype does not do

1. **Parameters.** `{ x -> ... }` is not supported, so a callback taking arguments still needs
   `fun(x: i32) { ... }`. A block header `{ x, y -> ... }` looks unambiguous, since `->` appears
   only in `when` arms, but the parameter types would have to come from the expected type, which
   is an `infer.rs` change (a `fun` literal's parameters are typed by hand today).
2. **Return values.** The literal has no `.R:`, so it returns Unit. A value-returning block
   (`map { x -> x + 1 }`) needs the return type taken from the expected function type.
3. **Named arguments before the block.** `f(x = 1) { ... }` appends an *unnamed* argument after a
   named one. Whether the argument matcher accepts that was not tested.
4. **Grouping parentheses in a head.** `if (run { w() }) { ... }` is refused, because only call
   argument lists and blocks re-enable the flag, not a parenthesised expression.
5. **Formatting of long chains.** `fmt.rs` copies tokens and rewrites the whitespace between them;
   the tested cases are single-line. Nested multi-line trailing blocks were not stress-tested.
6. **Shared mutable state.** Closures here capture by move or copy and own their environment
   (`plan/syntax.md` 7.4), so `polyp.button("+1") { state.count += 1 }` cannot mutate a `state`
   that the view also reads. That is a language question for Polyp, not a parsing one.

## Recommendation

Keep the parser change (it is additive and small), then take (1) and (2) together as the next
step, because a builder API needs both.
