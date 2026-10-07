# `std/iter` status: what shipped, what is missing

The shipped half is the protocol itself, and every fixture and test named in
`plan/iterators.md` 8 passes on both backends:

- `std/iter` declares `pub trait Iterator { type Item  fun mut
  Self.next().return: option.Option<Self::Item> }`, ships `Range` with `range`/
  `range_step`, and the drain helpers `count`/`sum_i64`/`collect_list`;
- `for x in <expr>` keeps the index walk for a `List`/`[T; N]` and becomes a
  `next()` loop for an `Iterator`; anything else is refused naming the trait;
- `break`/`continue` are ordinary loop control, refused outside every loop and
  free of the iterated place's mutation rule, with their inside-the-loop drops
  planned by `borrowck.rs` and emitted by both backends;
- the freestanding project `tests/projects/freestanding_iter` boots scope:
  it builds through LLVM and `ld.lld` with `llvm-nm` clean.

## What is missing, and exactly why

The generic adapters the task asks for — `take`, `skip`, `enumerate`, `zip`,
`chain`, and the association `List.into_iter()` that `enumerate` is tested
over — are **not** delivered. They are blocked on one language feature:

**Generic trait impls.** Each adapter is a generic `datac` implementing
`Iterator`:

```kelp
datac Take<I>(val inner: I, val remaining: usize)

impl<I: Iterator> Iterator for Take<I> {
    type Item = I::Item
    fun mut Self.next().return: option.Option<I::Item> { ... }
}
```

Neither half of that is accepted today:

- `impl<I: ...> ...` is a parse error (the parser expects `impl Trait for Type`
  with no parameter list), and
- `impl Iterator for Take<I>` is refused by name: "implementing a trait for a
  generic instantiation is not in this slice", because the `for` type is read
  as a dotted name with no argument list.

Without the impl, a composed adapter cannot even be built: `take(skip(range(0,
100), 10), 5)` has `I = Skip<Range>` in `take`, and `take`'s own bound
(`I: Iterator`) then fails because `Skip<Range>` implements nothing. The item
projection `Skip<Range>::Item` also has no impl binding to resolve, so the
adapter's own `next` return type cannot be written. The same missing feature
blocks a `ListIter<T>` for `List.into_iter()` and the closure adapters'
`I::Item` field.

A second, smaller gap is visible in the same place: `datac Take<I: Iterator>`
(a bound on a *type declaration's* parameter) is refused too, so even a
structural adapter that forwarded `next` without naming the trait could not
write `I::Item` in its method's return type. If a follow-up adds only the
generic impl, the closure adapters (`map`/`filter`) still need the bound on the
impl's parameter list, which the `impl<I: Iterator>` spelling provides, so the
one feature is likely enough for both.

## What was deliberately not done

- No eager stand-in (`take` returning a `List<T>`) was added: it would make a
  finite range behave, but it drains an infinite iterator, so it is a different
  feature with the same name. Refusing the shape and naming the missing feature
  keeps the surface honest.
- No test was weakened or deleted to make the gap look smaller; the existing
  `for_*` fixtures are untouched, and the three adapter tests are simply absent
  rather than passing for the wrong reason.

## The follow-up slice

Generic trait impls, in the shape `plan/traits.md` 10 already sketches:

1. `parser.rs`: parse `impl<TypeParams> Trait for Type<Args>` and record the
   parameters, their bounds, and the `for` type's arguments.
2. `resolve.rs`: substitute `Self` with the generic instance (`Take<I>`) and
   `Self::Item` with the impl's binding (`I::Item`), and carry the impl's bounds
   onto the method's type scope.
3. `infer.rs`: `implements_trait` for an instance (base name plus a bound check
   on the arguments), the assoc bindings as templates instantiated per use, and
   the impl-signature check against the trait with a generic `Self`.
4. Then the adapters, `List.into_iter()`, and `map`/`filter` are ordinary
   `std/iter` code with no further compiler work.
