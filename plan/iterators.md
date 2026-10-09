# The iterator protocol and `std/iter` (language completeness 2 of 8)

*Design note. A-5 shipped the cheap half earlier: `for x in xs { }` over a
`List` was a parser desugar to an index and a `while` (`syntax.md` 6.2), and the
reusable `iter`/`next` protocol was deferred "until needed". This slice adds it,
because a `List` is not the only thing a program wants to walk: a range, a
cursor over a file, and a user's own state machine all want the same loop shape.
The two desugars stay: a `List`/array keeps its index walk, and everything that
implements `std.iter.Iterator` gets the `next()` loop. The choice is made in
`infer.rs`, which is the first pass that knows the iterable's type.*

## 1. What a `for` means now

```
for x in xs { body }          // xs: List<T> or [T; N]
for x in it  { body }          // it implements std.iter.Iterator
```

- **A `List<T>` or `[T; N]` keeps today's exact behaviour and speed.** `for` is
  still the index walk: an index bound at zero, `x = xs.get(i)` once per pass,
  the loop guarded by `i < xs.len()`, and the *never mutate the place being
  walked* rule (`syntax.md` 6.2). The index now advances **before** the body
  runs rather than after, which is the one change, and it is invisible except
  to `continue` (section 3). The element binding is still exactly what
  `List.get` gives: a copy for a `Copy` element and a shared borrow for an owned
  one.
- **Any other type must implement `std.iter.Iterator`.** The loop holds the
  iterator, calls `next()` once per pass, stops on `None`, and runs the body on
  `Some(x)`. `x` is the **owned** item, so a `Copy` element arrives as a value
  and an owned one arrives as an owned binding that the body may move.
- **Anything else is refused**, with a message that names the trait:
  `'for' walks a 'List<T>' or a type that implements 'Iterator', and 'i32' is
  neither`. The refusals are pinned by `for_not_a_list.kp` and
  `for_iter_no_impl.kp`.

The two desugars, as `infer.rs` writes them (spans are the `for` line's, so a
diagnostic points at the loop):

```kelp
// List<T> / [T; N] — unchanged except that the increment precedes the body
{ var #forN_i: usize = 0
  while #forN_i < <seq>.len() {
      { val x = <seq>.get(#forN_i)
        #forN_i = #forN_i + 1
        <body> } } }

// Iterator — a place is borrowed exclusively, a temporary is owned
{ val #forN = &mut <place>       // or: var #forN = <temporary>
  while true {
      when #forN.next() {
          Some(x) -> { <body> }
          None -> { break } } } }
```

A non-place iterable is staged into `#forN` first, so a call is evaluated
exactly once, the same rule the index desugar uses for `for x in make()`.

## 2. Why the choice lives in `infer.rs`

The parser cannot make it: at parse time nothing knows whether `xs` is a
`List<T>`, an `Iterator` or a typo, and the two desugars have different
semantics (one re-reads a place by index, the other advances a cursor). So the
parser keeps `for` as a statement of its own and `infer.rs` chooses once the
iterable has a type. A type-only probe (the same re-check `check_method_call`
already performs) decides which desugar to build; the desugar is then ordinary
`ast` statements, checked by the ordinary statement and expression paths, so
`borrowck.rs` and both backends never learn a new loop form. This is also what
keeps the `List` fast path byte-for-byte the code it emitted before: the same
`.len()`/`.get()` calls on the same place.

## 3. `break` and `continue`

The `None` arm of the iterator desugar leaves the loop, and the only statement
that does that is `break`, so this slice adds `break` and `continue` as
ordinary loop control. They are **not keywords** (`syntax.md` 2.3): a bare one
in statement position is the statement, and a name used in an expression
(`break + 1`) is still a name.

- `break` leaves the innermost loop; `continue` re-tests the loop's condition.
  Both are refused outside every loop, naming the statement.
- In the **iterator** loop the desugar is an ordinary `while`, so `continue`
  jumps to `while true`, which is to say it calls `next()` again: exactly what a
  cursor loop needs.
- In the **index** loop `continue` must reach the next element, which is why the
  increment moved **before** the body: `continue` jumps to the condition, and
  the index was already advanced. The `List` fixtures' observable output is
  unchanged.
- A `defer` in the body runs on the way out of `break`/`continue` as it does on
  the way out of any block (`defer.rs` splices the active actions before the
  jump), and a deferred action may not contain `break`/`continue` itself,
  because it runs while its block is being left.

Drops are not deferred to code generation's discretion: a `break`/`continue`
frees every owned local declared **inside the loop it leaves**, and only those,
because the frame continues to run. `borrowck.rs` records that set per jump
(`DropPlan::at_break`/`at_continue`, keyed by the loop mark), and both backends
emit it before the branch, exactly as they emit a `return`'s frame drops.

## 4. `std/iter`

`std/iter` is pure Kelp and builds in a freestanding image when `std/option`
does; the `for` over a range is an ordinary `while` over an ordinary method by
the time either backend sees it.

```kelp
pub trait Iterator {
    type Item
    fun mut Self.next().return: option.Option<Self::Item>
}

pub datac Range(var lo: i64, val hi: i64, val step: i64)
fun range(a: i64, b: i64).return: Range
fun range_step(a: i64, b: i64, step: i64).return: Range

fun count<I: Iterator>(it: move I).return: usize
fun sum_i64<I: Iterator>(it: move I).return: i64
fun collect_list<I: Iterator>(it: move I).return: List<I::Item>
```

- `Range` is half-open (`[lo, hi)`) and steps by one or by an explicit `step`,
  which may be negative for a descending walk. It owns nothing, so it is a
  plain value and needs no allocator.
- `count`/`sum_i64`/`collect_list` are free functions with the bound
  `I: Iterator`, which is what lets them name the item type `I::Item`. The
  three bind their owned parameter to a `var` before walking, because a
  parameter is an immutable binding and `for` borrows the iterator it walks.

### 4.1 Why `Iterator` and not a structural `next`

The task's design names the trait, and the trait is what a `for` looks for
(`implements_trait(env, "std.iter.Iterator", <type>)`). A type that merely
happens to have a method called `next` is not iterable, which is the same
strictness `Shape` has for `area`.

## 5. What is deliberately absent, and what blocks it

- **`map`, `filter`, and every closure-taking adapter.** The task marks these
  optional: they would store a `closure(T).return: U` whose `U` is the inner
  iterator's `Item`, and a `datac` field type cannot name a projection from its
  own type parameter. The two are refused today because the parser refuses a
  bound on a type declaration's parameter (`datac Take<I: Iterator>` is
  refused with "a bound cannot be written on a type declaration's parameter"),
  so the adapter could not write `I::Item` anywhere. This is the same blocker as
  the generic adapters below.
- **`take`/`skip`/`enumerate`/`zip`/`chain`.** Each is a generic `datac`
  implementing `Iterator`, which needs `impl<I: Iterator> Iterator for Take<I>`.
  The parser refuses a generic `for` type in an impl ("implementing a trait for
  a generic instantiation is not in this slice", and the type name parses as a
  dotted name with no parameter list), so there is no way to say the impl, and
  without it a composed adapter (`take(skip(range(0, 100), 10), 5)`) cannot
  satisfy `take`'s own `I: Iterator` bound, and the item projection
  `Skip<Range>::Item` has no binding to resolve.
- **`List.iter()` yielding `&T` as an iterator value.** `for x in xs.iter()` is
  accepted and desugars to `for x in xs` (the element is the shared borrow
  `List.get` gives, and the body may not mutate `xs`); no iterator value is built.
  A borrowing iterator value (`xs.iter().map(...)`) cannot outlive the iterator
  that handed it out, and v1 has no lifetime syntax to tie the two together (A-11).
  That needs a design note first (MISSING-Hd4Lr, option 3). The owned
  `List.into_iter()` is the honest form and it is
  itself blocked: it would be a method on the builtin `List<T>`, and the
  compiler's `List` surface is builtin, so a shipped module cannot add one.
- **`List.into_iter()`.** Same reason. A `List` method is either compiler-
  generated (`List#get`) or a receiver method on a *declared* generic type;
  `fun List<T>.into_iter()` is refused because `List` is not a generic type
  declaration, and a `ListIter<T>` implementing `Iterator` would be the generic
  impl this slice does not have.

Everything in this section is one feature away: **generic trait impls**
(`impl<I: Iterator> Iterator for Take<I> { type Item = I::Item ... }`), with
the bound on the impl's own parameter list and the associated-type binding
instantiated per use. That is the follow-up slice, and it unlocks all of it:
the adapters, the closure adapters' `I::Item` field, and a `ListIter<T>` for
`List.into_iter()`. Until then, `for` walks a `List` or a type whose `Iterator`
impl is written out by hand, and the free helpers above cover "drain this into
a list" and "sum this".

## 6. What the compiler checks, and what it does not

- The iterable must be a `List`/array or implement `Iterator`; the message names
  the trait. It does **not** check that the `next` the loop calls comes from the
  trait's own impl, only that the type has a reachable `next`; the trait's
  signature is what every shipped impl is checked against.
- A place iterator is borrowed exclusively for the loop, so naming it in the
  body is the ordinary "already borrowed" error. A temporary iterator is owned
  by the loop and cannot be named at all.
- The item type is whatever `next()` returns, `Option<Self::Item>` by the
  trait's signature. Nothing checks that an impl's `next` returns exactly
  `Option<Item>` beyond the signature check the trait machinery already runs at
  the impl.
- `break`/`continue` free only the loop's own owned locals; codegen's drop plan
  and the block-end drops of the scopes inside the loop are both applied, and a
  `break` that skips a `defer` in an inner block still runs that block's
  actions because `defer.rs` spliced them before the jump.

## 7. How it is implemented

- `ast.rs` gains `Stmt::For` (the raw loop) and `Stmt::Break`/`Stmt::Continue`;
  `tir.rs` gains `Stmt::Break`/`Stmt::Continue`.
- `parser.rs` emits the raw `Stmt::For` and reads a bare `break`/`continue` in
  statement position as the loop control statement; `break`/`continue` stay
  ordinary names everywhere else.
- `infer.rs` owns the choice: `Stmt::For` probes the iterable's type and builds
  one of the two desugars, which are then checked by the ordinary paths. It
  also tracks the enclosing loop depth so a stray `break` is refused.
- `defer.rs` handles `Stmt::For`, splices active actions before
  `break`/`continue`, and refuses a loop exit inside a deferred action.
- `borrowck.rs` records `DropPlan::at_break`/`at_continue` from the loop mark;
  both backends emit those drops before the branch.
- `codegen_c.rs` writes `break;`/`continue;`; `codegen_llvm.rs` keeps a stack of
  `(condition, exit)` blocks and branches to the right one.
- `stdlib.rs` ships `std/iter` and names the trait key; `freestanding.rs`
  admits it (the range loop and the three helpers are pure, `collect_list` waits
  for an allocator).
- `plan/kernel-ub-notes.md` carries this slice's note.

## 8. Tests

Run on both backends (`assert_compiles_and_runs`/`assert_rejected_with` under
the default LLVM backend and under `KELPC_ORACLE=c`), and built by the
`backend_coverage` sweep as `tests/fixtures/*.kp`:

- `for_iter_range.kp`: `for i in iter.range(0, 5)`, a positive step and a
  descending step;
- `for_iter_break.kp`: a user Fibonacci `Iterator` with `break`, and a `continue`
  that skips odd numbers;
- `for_iter_defer.kp`: a `defer` in the body runs once per iteration;
- `for_iter_nested.kp`: two iterator loops nest;
- `for_iter_helpers.kp`: `count`, `sum_i64`, `collect_list`;
- the `List` fast path: every existing `for_*` fixture passes untouched;
- refusals: `for_not_a_list.kp` and `for_iter_no_impl.kp` (naming `Iterator`),
  `for_iter_borrowed.kp` (using the iterator in its body), `for_iter_break_outside.kp`
  (`break` outside a loop), `defer_break_inside.kp` (`break` in a deferred
  action);
- `break_and_continue.kp`: the two statements in a written `while`;
- `tests/projects/freestanding_iter`: a kernel-style `for i in iter.range`
  builds through LLVM and `ld.lld` with `llvm-nm` clean.
