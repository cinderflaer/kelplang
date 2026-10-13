# Generic trait impls, `where` clauses and blanket impls (MISSING-Ve8Nc)

*Design note, written in 26.10.3 ahead of the implementation, then built in two releases.
**Built (26.10.3):** steps 1 to 3 of section 4 for an impl on a generic type
(`impl<A: Bound, B: Bound> Trait for Type<A, B>`, template methods, conditional satisfaction,
associated types over the parameters, the failing argument named in the error) and `where` as a
second spelling of a bound on a function or an impl. `std/iter`'s adapters ship through a simpler
by-shape rule (a generic type whose template declares an iterator-shaped `next` is an
`Iterator`). **Built (26.10.4):** step 4 of section 4, coherence (the overlap check) and blanket
impls (section 2 item 5), as recorded under step 4 below; and step 5 of section 4, bounds on
projections and equality constraints, as recorded under step 5 below (built 2026-10-09). **Not
built:** an equality constraint or a projection bound on a blanket impl, an impl for one
instantiation, `dyn` of a generic impl or of a blanket impl, default methods in a generic impl or a
blanket impl, operators through a generic impl (MISSING-Lh6Yp). The section 6 exclusions stay refused
by name (their own entries in the wiki's `meta/open-issues`).*

## 1. What is missing and what it blocks

Today an `impl Trait for Type` names one concrete type (`plan/traits.md` 2), a bound is written
only in the parameter list (`fun f<T: A + B>`), and a generic instantiation cannot carry an
impl. So none of these can be written:

```kelp
impl<T: Display> Display for Pair<T> { ... }            // impl for a generic type
impl<T: Display> Describe for T { ... }                  // blanket impl
fun f<I>(it: I).return: i64 where I: Iterator<Item = i64>   // where clause, equality constraint
impl<I: Iterator> Iterator for Take<I> { ... }           // what iterator adapters need
```

It blocks the adapters `map/filter/take/skip/enumerate/zip/chain` (an adapter is a generic type
implementing `Iterator` over another `Iterator`), `List.iter()` (a `ListIter<T>` that implements
`Iterator`), and `impl Iterator for Ring<3>` (a const-generic type with an impl). It is the one
item that unlocks the rest of MISSING-Hd4Lr.

## 2. The rule, in the order a reader meets it

1. **An impl may declare type parameters** after the keyword: `impl<T, U: Bound> Trait for Type<T, U>`.
   The parameters are the impl's own; the trait's arguments and the type are written in terms of them.
2. **The type position is a pattern**: a type constructor applied to the impl's parameters
   (`Pair<T>`, `Take<I>`, `Ring<N>` for a const parameter) or a bare parameter (`T`, a blanket impl).
3. **A bound on an impl parameter is checked where the impl is used**, not where it is written:
   `Pair<Foo>: Display` holds only if `Foo: Display`. A bound that names the impl's own trait
   (`impl<T: Describe> Describe for Vec2<T>`) is how recursion terminates.
4. **`where` is a second spelling of a bound**, on a function or an impl, for the cases the parameter
   list cannot say: a bound on a projection (`where I::Item: Display`) and an equality constraint
   (`where I: Iterator<Item = i64>`). `fun f<T: A>` and `fun f<T>(...) where T: A` mean the same.
5. **Coherence is overlap-free, with no specialization.** Two impls of one trait overlap when their
   type patterns unify, whatever their bounds (bounds are not used to prove two impls disjoint: that
   is what makes the rule decidable and the error local). A blanket impl `impl<T> Tr for T` therefore
   overlaps every other impl of `Tr` and is accepted only when it is the trait's single impl. The
   orphan rule stays as in `traits.md` 2: the trait or the type constructor is in the impl's module.
6. **Dispatch stays static.** A generic impl is a template; using it for a concrete type instantiates
   its methods exactly as a generic function is instantiated, so neither backend and no freestanding
   image learns anything new (`traits.md` 3). Dynamic dispatch (`&dyn`) of a generic impl is the
   existing vtable for the instantiation; `dyn` of a *generic trait* or a trait with an associated
   type stays refused until a separate slice.

## 3. How it fits the existing machinery

- **Registry.** `TypeEnv::impls` maps a trait key to the set of concrete type names that implement
  it. It gains `impl_templates: trait key -> Vec<ImplTemplate>` (parameters, bounds, type pattern,
  method declarations). `implements_trait(env, trait, ty)` first asks the concrete table, then
  matches the type against each template's pattern (`bind_type_params` already does the structural
  match for function templates) and checks the bounds recursively with a depth limit.
- **Instantiation.** The 26.10.3 work already instantiates a method on a generic type with the
  receiver's arguments plus the method's own (`fun Pair<A, B>.map_first<C>`). An impl method of a
  generic impl is that same template with the impl's parameters standing for the receiver's, so the
  work is wiring the impl table to the method-template table, not a new instantiator.
- **Associated types.** `env.assoc` is keyed by (trait, type name) and is filled per concrete impl.
  For a generic impl the binding is computed at instantiation (`type Item = I::Item`), and an
  equality constraint `I: Iterator<Item = i64>` is checked against that binding.
- **Method lookup.** `x.m()` already resolves an inherent method before a trait method. A blanket
  impl adds candidates for every receiver type, so lookup order is: inherent, then trait methods in
  scope of the module that imports the trait, with an ambiguity between two traits refused by name.
- **Diagnostics.** The error for an unmet bound names the instantiation chain ("`Take<Filter<Foo>>:
  Iterator` needs `Filter<Foo>: Iterator`, which needs `Foo: Iterator`"), because the failure is
  otherwise several instantiations away from the call a reader wrote.

## 4. Order of work

1. Parse `impl<...>`, `where`, and `Trait<Item = T>` equality arguments; no checking.
2. Impl templates and matching in `implements_trait` for a generic type (no blanket impls yet).
3. Instantiate impl methods through the method-template table; `impl<T: Display> Display for Pair<T>`
   end to end, on both backends.
4. Coherence (overlap check) and blanket impls.
5. `where` clauses on functions and impls, projections, equality constraints.
6. `std/iter`: `ListIter<T>`, then the adapters, each with the differential corpus behind it.

**Step 4, as built (26.10.4).**

- *Coherence.* Two impls of one trait overlap when their type patterns unify, whatever their
  bounds are. A blanket impl overlaps every other impl of its trait, and two impls of one generic
  type overlap. The error is reported at the later impl, with the earlier one named as a note. Two
  concrete impls for one type keep the "already has an impl" error, now with the other impl named
  as a note. No bound is used to tell two impls apart, so there is no specialization.
- *Blanket impls.* `impl<T: A, ...> B for T` takes exactly one type parameter, names no trait
  arguments, binds no associated type, and has no static member and no trait default. It lives in
  the trait's module. A type implements `B` when every bound on `T` holds for it, which is the rule
  `implements_trait` applies. Each method is a template keyed `impl#B#name` and is instantiated
  with `T` bound to the receiver type, builtins included, exactly as a generic method is. Method
  lookup tries inherent, concrete, generic and builtin methods first and a blanket method last.
  Two blanket impls of different traits that both provide one method name for one receiver are
  refused as ambiguous.
- *Refused by name.* A bound that cannot be checked on the receiver: a builtin container such as
  `List<i32>` answers to its bare name, so a bound on its element fails by name and the call is
  refused, not read as "does not apply". A blanket impl whose bound leads back to its own trait,
  directly or through other blanket impls. A `dyn` coercion of a type whose trait comes from a
  blanket impl (the blanket impl has no vtable of its own). A trait default in a blanket impl, and
  an associated type or a trait argument on one.
- *Refused by name.* An equality constraint on a generic trait's own parameter, on a supertrait or
  on an associated type's bound; an equality constraint or a projection bound on a blanket impl; a
  negative bound (`T: !Name`, section 6).

**Step 5, as built (26.10.4, 2026-10-09).**

- *Equality constraints.* `I: Iterator<Item = i64>`, inline or in a `where` clause, on a function
  or a generic impl. The trait must declare the associated type; the constraint is checked at each
  instance on the concrete binding (`I::Item` of the argument, through its impl or its iterator
  shape), and the right-hand side is read in the declaring module's spelling. A body is checked per
  instance, so it reads `Item` as the bound's type.
- *Bounds on projections.* `where I::Item: Display` on a function or a generic impl. It is checked
  at each call (or instance) on the concrete associated type: a type that binds no such associated
  type is refused, and so is a type whose associated type does not implement the bound.
- *Generic impl bounds govern an iterator.* A generic type with an explicit generic `impl Iterator`
  satisfies `Iterator` only through that impl's bounds; the iterator-shape rule applies to a
  generic type with no such impl (the `std/iter` adapters), as before.
- *Refused by name:* a trait argument on a projection bound (`where I::Item: From<i32>`), a
  constraint naming an associated type the trait does not declare, and the forms in the list above.

Each step keeps the previous tests green and adds its own fixtures; steps 1 to 3 are one release,
4 to 5 the next, 6 the one after, so a slip moves the adapters and nothing else.

## 5. Risks

- **Instantiation blow-up.** `Take<Skip<Map<Filter<...>>>>` is a legitimate program. The existing
  `MAX_FUNCTION_INSTANTIATIONS` guard stays, and the diagnostic for hitting it must name the type chain.
- **Compile time.** Each adapter instantiation is a fresh set of methods through both backends. The
  adapters ship only when a representative pipeline compiles in time comparable to the hand-written loop.
- **Error quality.** A trait-bound failure through three adapters is hard to read. The chain message
  above is part of step 3, not a later nicety.

## 6. What is deliberately not here

Specialization, negative bounds, associated consts, generic associated types, higher-ranked bounds,
`impl Trait` in return position, and an upcast between trait objects. Each is refused by name with
the follow-up in the message, as every earlier slice does.
