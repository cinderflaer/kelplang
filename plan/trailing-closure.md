# Trailing blocks (open item 75): design and prototype

*Design note, the parser prototype, and the follow-up in `language/functions.md` (kelp wiki). Not a Mark;
`meta/roadmap.md` (kelp wiki) still owns the schedule.*

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
  appended as an unnamed `CallArg` holding a zero-parameter `fun` literal with no `.return:`.
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

1. ~~**Parameters.**~~ **Addressed** (`language/functions.md` (kelp wiki) part 1): `{ x, y -> ... }` declares
   the parameters by name only, and their types come from the expected function type of the
   parameter the block is passed to. The header is read only when the first tokens are
   identifiers, comma separated, followed by `->`, which appears nowhere else at the start of a
   block. Parameter modes follow the expected type, because a Kelp parameter type already carries
   its mode. Fixture `scoped_block_params.kp`.
2. ~~**Return values.**~~ **Addressed** (`language/functions.md` (kelp wiki) part 1): a block whose body ends
   in an expression returns it when the expected return type is non-unit; with a unit return the
   tail is discarded exactly as before. `sum(xs) { n -> n * 10 }` and `sum(xs) { n -> if n > 5 {
   n } else { 0 } }` are both in `scoped_block_params.kp`.
3. **Named arguments before the block.** `f(x = 1) { ... }` appends an *unnamed* argument after a
   named one. Whether the argument matcher accepts that was not tested.
4. **Grouping parentheses in a head.** `if (run { w() }) { ... }` is refused, because only call
   argument lists and blocks re-enable the flag, not a parenthesised expression.
5. **Formatting of long chains.** `fmt.rs` copies tokens and rewrites the whitespace between them;
   the tested cases are single-line. Nested multi-line trailing blocks were not stress-tested.
6. ~~**Shared mutable state.**~~ **Addressed for call-scoped callbacks** (`language/functions.md` (kelp wiki)
   part 2): a parameter written `scoped fun(T).return: U` accepts only a literal or trailing block
   written at the call site, and such a literal captures an enclosing binding **by shared
   borrow** into a stack environment. So `polyp.column { k -> k.text(s.title) }` reads a borrowed
   `s` that the view still owns, and the borrow is released when the call returns. Mutation
   through the capture (the `state.count += 1` shape) is refused: an exclusive capture is not
   implemented, so that stays a language question for Polyp.

## Recommendation

Done. The parser change stayed (it is additive and small), and (1), (2) and (6) landed together in
`language/functions.md` (kelp wiki), because a builder API needs all three. What is still open there is an
*exclusive* capture (a block that mutates what it borrowed) and `scoped` anywhere but a top-level
declaration parameter.
