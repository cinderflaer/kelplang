# Traits with static dispatch (MK9 language round, `plan/roadmap.md` A-12 slice 2)

*Design note. A-12 slice 1 made a type a `Map` key by declaring `fun T.hash()`/`fun T.eq(other: &T)`;
this slice adds the `trait` keyword A-12 named as the next piece. It is deliberately the static
half of the item: a trait is a named set of method signatures, a type implements it with an
`impl` block, and a generic function may declare that its type parameter implements it. Dispatch
stays static, so monomorphization is unchanged and neither backend learns a new lowering.*

## 1. What a trait is

```
trait Shape {
    fun Self.area().R: f64
    fun mut Self.scale(f: f64)
}
```

- A trait is a top-level declaration. Its body is one or more method **signatures**, in the same
  receiver form a method is written in (`plan/syntax.md` 8), with `Self` in the receiver's type
  position standing for the implementing type. `Self` is also legal in the signature's other
  types (`fun Self.smaller_than(other: &Self).R: bool`), where it means the same thing.
- A signature is `fun [mut] Self.name([param, ...])[.R: T]`, with no body: no `=` and no `{ ... }`.
  `mut` on the receiver is part of the signature and the impl has to match it.
- The trait name is an ordinary top-level name: `pub trait Name` exports it like a `pub datac`
  (A-13), and a trait declared in another module is reached as `alias.Name`.
- A trait may not declare a type parameter, and neither may one of its methods: `trait Name<T>`
  and `fun Self.m<T>()` are both refused by name. A signature names only `Self`, the types of the
  declaring module and what that module exports.
- `Drop` is the one name a user cannot use: `trait Drop` is refused where it is written, with the
  surviving spelling (`fun mut T.drop()`, `plan/drop.md`) in the message. User `Drop` is
  unchanged, and so is every other compiler-known name pair (`hash`/`eq`).

## 2. What implements a trait

```
datac Circle(val r: f64)

impl Shape for Circle {
    fun Self.area().R: f64 = r * r * 3.14159
    fun mut Self.scale(f: f64) { r = r * f }
}
```

- The impl line is `impl Trait for Type { ... }`. The parser tells it from an import by the token
  after `impl`: a **string literal** is `impl "path"` (an import, `plan/syntax.md` 13.1) and an
  **identifier** is this form. An impl block declares methods only, each with a `Self` receiver:
  the receiver is written `Self`, not the type's own name, so the block reads the same whether it
  sits in the trait's module or the type's.
- The body is one method per declared signature. Each one is an ordinary method
  (`fun Self.scale(f: f64) { ... }`), so inside it the type's fields are read and written by their
  bare names exactly as an inherent method's are.
- **Coherence**: at most one impl of a trait for a type in the whole program. A second one is an
  error naming both.
- **The impl lives in the trait's module or in the type's module**, and nowhere else. That is what
  keeps the type's own module able to add a behaviour, and the trait's module able to supply an
  impl for a type it did not declare, without a third module inventing a relation the two owners
  never agreed to.
- **The type has to be a `datac` or an `enum`** declared in the program. A builtin (`i32`,
  `String`, `List<T>`, `Bytes`, an atomic, a handle, `View`, a function type), a tuple, an array,
  a borrow and a generic *instantiation* are each refused by name, and so is a name that resolves
  to no declared type at all.
- **Every declared signature must be implemented**, with exactly the declared signature: the same
  receiver spelling (`Self`, with or without `mut`), the same parameter names and types in the same
  order, and the same return type. A missing method, an extra method the trait does not declare,
  and a mismatched one are three distinct errors, each naming the trait and the method. Parameter
  *names* are part of the match because a method call may name its arguments (`s.scale(f = 2.0)`),
  and a body written against the bound has to have one spelling for every instantiation.
- **An inherent method of the same name on the same type is an error at the impl**, naming the
  trait and the method: `fun Circle.area()` already exists, so `impl Shape for Circle { fun
  Self.area() ... }` would leave two answers to `c.area()`. Rename one of them.

Because a trait impl's methods *are* the type's methods, nothing else changes: an impl method is
reached through a value exactly as an inherent one is, it is emitted by both backends through the
same per-type function, and a trait method on a type that also declares its own `drop` still drops
through the same generated `kelp_drop_<T>`.

## 3. Bounds, and how dispatch stays static

```
fun area_of<T: Shape>(s: &T).R: f64 = s.area()
fun describe<T: Shape + Named>(s: &T).R: String = ...
```

- A type parameter carries bounds after a `:`, separated by `+`: `fun f<T: A + B>(...)`.
- **Inside the body, a method call on a `T` resolves exactly as it would on the concrete type**,
  because a generic function's body is already checked once per instantiation with `T` bound to
  the concrete type (A-2, `plan/syntax.md` 9.3). The bound is what makes that resolution succeed;
  the emitted call is the ordinary direct call the type's method already had.
- **At every call, the type argument must implement every bound.** The check runs where the
  instantiation is created, so a satisfied call compiles to the same monomorphized function it
  would have without the bound, and an unsatisfied one is an error naming the callee, the type
  and the trait: `the call to 'area_of' needs 'Point' to implement the trait 'Shape', and it does
  not`.
- Bounds are checked transitively, and that needs no symbolic reasoning: `fun f<T: A>(x: &T) { g(x) }`
  where `g<U: A>` is fine for every type that satisfies `f`'s bound, and calling `g<U: B>` from the
  same body is an error at that call as soon as `f` is instantiated with a type that does not
  implement `B`. A generic function that is never called is never checked, exactly as today.
- Nothing in either backend changes. There is no vtable, no witness argument, no new `tir` node and
  no new C type, so a freestanding build has traits too (they are proven in
  `tests/projects/freestanding_min`, which is LLVM-only by design).

## 4. What is deliberately not in this slice

Each of these is refused where it is written, with the message naming the follow-up:

- **`dyn` and trait objects.** `dyn Name` in a type position is refused: a trait is a bound on a
  type parameter, not a type of its own, so there is no runtime tag and no dynamic dispatch.
  Substituting an `enum` (`plan/roadmap.md` A-1) is the workaround.
- **Default methods.** A signature with a body inside a `trait` is refused; every method is
  written in the impl.
- **Associated types and consts, and any non-`fun` item inside a trait.** Refused by name.
- **Trait-bounded `impl Trait` in argument position** (`fun f(x: impl Name)`). Refused by name:
  name the type parameter and bound it instead.
- **Implementing a trait for a builtin type** (`i32`, `String`, `List<T>`, ...). Refused: a
  builtin's methods come from the compiler and its representation is not a `datac`'s.
- **`where` clauses.** Refused: write the bound on the parameter (`fun f<T: Name>`).
- **Trait inheritance** (`trait A: B`). Refused: list both bounds at the use site
  (`fun f<T: A + B>`).
- **Operator overloading through traits.** `==`/`!=` still lower to a declared `eq` (H-5) and
  nothing else looks at a trait; there is no way to spell `+` or `<` through one.
- Also out: a bound on a method's own type parameters (a receiver's parameters are its type's,
  so there is nowhere to write one), an impl for a generic *instantiation*
  (`impl Name for Pair<i32, String>`), a trait that is generic over its types, and a second impl
  of a trait for a type.

## 5. What the compiler does not check

- A method call on a `T` is resolved against the type each instantiation binds, not against the
  bound's own signature list. The bound guarantees that the methods the trait declares exist; the
  compiler does not also insist the body name *only* those, so a call to an inherent method of the
  concrete type (or to a method of another bound on the same parameter) compiles too. Every type
  that ever reaches the body satisfies the bound, so this is a looser statement of the bound than
  "these are the only methods in scope", not an unsoundness.
- An impl's body is checked like any other method body. Nothing requires an `area` to be the area
  of anything, nothing requires two impls of different traits for one type to agree, and there is
  no way for a trait method to call another trait method through `Self` (it has no `Self` value).
- A trait signature's parameter names are compared with the impl's, so a rename is refused, but
  nothing checks the *order* of a `when` inside a body, or that an impl method's body does not
  panic.

## 6. How it is implemented

- `parser.rs` parses the two declarations and the bound list, and refuses every shape section 4
  names. A trait method signature is an ordinary `FunctionDecl` with a `Self` receiver and no
  body; an impl method is an ordinary `FunctionDecl` whose `trait_impl` field records the trait
  and the written `for` type.
- `resolve.rs` merges the traits and the impl records, and rewrites each impl method: `Self`
  becomes the written `for` type everywhere in its signature. Nothing downstream knows an `impl`
  block existed.
- `infer.rs` is where the whole feature lives: the trait table and its signature validation, the
  coherence and module rules, the exact-signature match against each impl (so an impl method's
  resolved `FnSig` *is* the trait's), the inherent-method collision message, and the bound check
  at `instantiate_function`, which is the one place a generic function is instantiated.
- `fmt.rs` needs no change: it is token-level, and a trait body, an impl body and a bound list are
  braces, suffixes and binary operators it already re-emits.
- `plan/kernel-ub-notes.md` carries this feature's note: no new way to reach undefined behaviour.

## 7. Tests

`kelpc/tests/compile_and_run.rs` runs each of these on the backend under test (LLVM by default,
the C oracle under `KELPC_ORACLE=c`), and `tests/fixtures/` is built by the coverage sweep as
well:

- a trait with two methods, implemented for a `datac` and for an `enum`, called through a generic
  function;
- two bounds on one parameter, and a call that satisfies only one of them;
- a method on a generic `datac` that calls a bounded generic function with the receiver's type
  parameter (`Pair<A, B>.sum_shapes()`);
- a bound satisfied at one call site and not at another;
- the rejections: missing method, wrong signature (type and receiver `mut`), extra method,
  duplicate impl, inherent collision, unknown trait, impl for a builtin, impl in a third module,
  unsatisfied bound, `dyn`, a default body, an associated type, `impl Trait` in argument position,
  a `where` clause, trait inheritance, `trait Drop`;
- the cross-module project `tests/projects/traits_cross` (trait in `shape.kp`, impl in the type's
  module `circle.kp`, used from the entry module), with `pub` on the trait, the generic function
  and the type;
- `tests/projects/freestanding_min` uses a trait, so the freestanding path proves static dispatch
  needs no runtime;
- `kelpc fmt` round-trips trait/impl/bound syntax (a `tests/fmt_corpus` pair plus the repository
  corpus the formatter already walks), and `kelpc lint` is untouched.
