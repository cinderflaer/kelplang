# Traits with static dispatch (MK9 language round, `plan/roadmap.md` A-12 slice 3)

*Design note. Slice 1 made a type a `Map` key by declaring `fun T.hash()`/`fun T.eq(other: &T)`,
slice 2 added the `trait` keyword with method signatures, `impl` blocks and bounds. This slice adds
default methods, supertraits and impls for builtin types. Dispatch stays static, so
monomorphization is unchanged and neither backend learns a new lowering.*

## 1. What a trait is

```
trait Shape {
    fun Self.area().R: f64
    fun Self.describe().R: String {
        return "a shape"
    }
    fun mut Self.scale(f: f64)
}

trait Named: Shape {
    fun Self.name().R: str
}
```

- A trait is a top-level declaration. Its body is one or more methods, in the same receiver form a
  method is written in (`plan/syntax.md` 8), with `Self` in the receiver's type position standing
  for the implementing type. `Self` is also legal in the other types a method names
  (`fun Self.smaller_than(other: &Self).R: bool`).
- A method with **no body** is a **signature**: `fun [mut] Self.name([param, ...])[.R: T]`, no `=`
  and no `{ ... }`. `mut` on the receiver is part of the signature.
- A method **with a body** is a **default method**: `fun Self.describe().R: String { ... }`,
  written exactly like an ordinary method, with `Self` standing for the implementing type. A
  default body may call the trait's other methods through `self` (`self.area()`), read no field it
  did not get from a method, and use the trait's own module's types. It is checked once per
  implementing type, exactly as a generic function's body is, so it can call methods that only the
  concrete type has (section 3 and section 5).
- A trait may name **supertraits** after a `:`: `trait B: A + C { ... }`, one or more trait names
  separated by `+`, each resolved in the trait's own module. Implementing `B` for a type requires an
  `impl A for` and an `impl C for` that type; the relation is transitive and must be acyclic.
- The trait name is an ordinary top-level name: `pub trait Name` exports it like a `pub datac`
  (A-13), and a trait declared in another module is reached as `alias.Name`.
- A trait may not declare a type parameter, and neither may one of its methods: `trait Name<T>` and
  `fun Self.m<T>()` are both refused by name. A signature names only `Self`, the types of the
  declaring module and what that module exports.
- `Drop` is the one name a user cannot use: `trait Drop` is refused where it is written, with the
  surviving spelling (`fun mut T.drop()`, `plan/drop.md`) in the message. User `Drop` is unchanged,
  and so is every other compiler-known name pair (`hash`/`eq`).

## 2. What implements a trait

```
datac Circle(val r: f64)

impl Shape for Circle {
    fun Self.area().R: f64 = r * r * 3.14159
    fun mut Self.scale(f: f64) { r = r * f }
    // `describe` is omitted: it comes from the trait's default.
}

trait Counts {
    fun Self.count().R: i32 { return 0 }
}

// A trait whose methods are all defaulted is implemented by an empty block.
impl Counts for Circle {}
```

- The impl line is `impl Trait for Type { ... }`. The parser tells it from an import by the token
  after `impl`: a **string literal** is `impl "path"` (an import, `plan/syntax.md` 13.1) and an
  **identifier** is this form. An impl block declares methods only, each with a `Self` receiver.
- **A defaulted method may be omitted**: the impl gets the trait's default, monomorphized per
  implementing type. It may instead **override** it, with exactly the declared signature. A method
  with no default must be implemented. An impl block whose trait declares only defaulted methods may
  be empty (`impl T for X {}`).
- **Coherence**: at most one impl of a trait for a type in the whole program. A second one is an
  error naming both.
- **Where an impl may live.** For a `datac`/`enum`, the impl lives in the trait's module or in the
  type's module, and nowhere else. For a **builtin type** (section 4), it lives in the trait's
  module, because a builtin has no module of its own and the trait's module is the only owner the
  relation can be attributed to.
- **The type has to be a `datac`, an `enum`, or one of the builtin types the compiler already gives
  methods to** (section 4). A container (`List<T>`, `Bytes`, an atomic, a handle, `View`), a tuple,
  an array, a borrow and a generic *instantiation* are refused by name, and so is a name that
  resolves to no declared type at all.
- **Every non-defaulted declared signature must be implemented**, with exactly the declared
  signature: the same receiver spelling (`Self`, with or without `mut`), the same parameter names
  and types in the same order, and the same return type. A missing method, an extra method the
  trait does not declare, and a mismatched one are three distinct errors, each naming the trait and
  the method. Parameter *names* are part of the match because a method call may name its arguments
  (`s.scale(f = 2.0)`).
- **An inherent method of the same name on the same type is an error at the impl**, naming the
  trait and the method: `fun Circle.area()` already exists, so `impl Shape for Circle { fun
  Self.area() ... }` would leave two answers to `c.area()`.

Because a trait impl's methods *are* the type's methods, nothing else changes: an impl method is
reached through a value exactly as an inherent one is, it is emitted by both backends through the
same per-type function, and a trait method on a type that also declares its own `drop` still drops
through the same generated `kelp_drop_<T>`.

## 3. Bounds, supertraits and how dispatch stays static

```
fun area_of<T: Shape>(s: &T).R: f64 = s.area()
fun describe<T: Shape + Named>(s: &T).R: String = ...

// `name_of` may call `Shape`'s methods on its argument because `Named: Shape`.
fun name_then_area<T: Named>(s: &T).R: f64 {
    print(s.name())
    return s.area()
}
```

- A type parameter carries bounds after a `:`, separated by `+`: `fun f<T: A + B>(...)`.
- **Inside the body, a method call on a `T` resolves exactly as it would on the concrete type**,
  because a generic function's body is checked once per instantiation with `T` bound to the concrete
  type (A-2, `plan/syntax.md` 9.3). The bound is what makes that resolution succeed; the emitted
  call is the ordinary direct call the type's method already had. For `T: B` where `B: A`, a call
  of `A`'s method is resolved the same way, because implementing `B` required an `impl A for` the
  concrete type.
- **At every call, the type argument must implement every bound.** The check runs where the
  instantiation is created, so a satisfied call compiles to the same monomorphized function it
  would have without the bound, and an unsatisfied one is an error naming the callee, the type and
  the trait.
- **A supertrait is satisfied transitively**: a type that implements `B`, where `B: A`, satisfies a
  `T: A` bound, because the `impl B for` the type had to come with an `impl A for` it. A cycle in
  the supertrait relation (`trait A: B` and `trait B: A`) is refused where the cycle closes.
- Nothing in either backend changes. There is no vtable, no witness argument, no new `tir` node and
  no new C type, so a freestanding build has traits too (they are proven in
  `tests/projects/freestanding_min`).

## 4. Impls for builtin types

```
trait Doubler {
    fun Self.double().R: i32
}

impl Doubler for i32 {
    fun Self.double().R: i32 = self * 2
}

fun twice<T: Doubler>(x: T).R: i32 = x.double() + x.double()
```

- A trait may be implemented **for a builtin type that already carries methods**, which is exactly
  the set the compiler's method-key table names: the integer scalars (`i32`, `i64`, `u32`, `u64`,
  `u8`, `u16`, `usize`, `isize`), `String`, `str`, `Bytes`, `View`, and an `Atomic<T>` alias. Those
  are the types an ordinary receiver method can already be resolved on, so a trait method on one is
  reached through the same `Type#name` key every other builtin method is.
- A builtin that carries **no** methods (`bool`, `char`, `f32`, `f64`, `unit`) is refused by name: a
  trait body has no field to read on a scalar, and adding one would invent a method surface the
  language does not otherwise have. A container (`List<T>`, `Bytes` through its element, `Map`,
  `Box`, `Atomic`), a tuple, an array, a borrow, a function type and a handle are refused too: they
  are not a `datac`/`enum`, and a generic instantiation has no single `for` name.
- **Coherence for a builtin**: the trait has to be declared in the same module as the impl (the
  orphan rule). The impl is the one impl of that trait for that builtin in the whole program.
- The receiver of an impl method on a builtin is `self`, passed by value for a `Copy` scalar and
  borrowed for `String`/`Bytes`, exactly as the compiler's own methods on those types are.

## 5. What is deliberately not in this slice

Each of these is refused where it is written, with the message naming the follow-up:

- **`dyn` and trait objects.** `dyn Name` in a type position is refused: a trait is a bound on a
  type parameter, not a type of its own, so there is no runtime tag and no dynamic dispatch.
  Substituting an `enum` (`plan/roadmap.md` A-1) is the workaround.
- **Associated types and consts, and any non-`fun` item inside a trait.** Refused by name.
- **Trait-bounded `impl Trait` in argument position** (`fun f(x: impl Name)`). Refused by name:
  name the type parameter and bound it instead.
- **A generic trait** (`trait Name<T>`) and a bound on a method's own type parameters.
- **`where` clauses.** Refused: write the bound on the parameter (`fun f<T: Name>`).
- **Operator overloading through traits.** `==`/`!=` still lower to a declared `eq` (H-5) and
  nothing else looks at a trait; there is no way to spell `+` or `<` through one.
- **A trait implemented for a generic instantiation** (`impl Name for Pair<i32, String>`), and a
  second impl of a trait for a type.

## 6. What the compiler does not check

- A method call on a `T` is resolved against the type each instantiation binds, not against the
  bound's own signature list. The bound guarantees that the methods the trait declares exist; the
  compiler does not also insist the body name *only* those, so a call to an inherent method of the
  concrete type (or to a method of another bound on the same parameter) compiles too. Every type
  that ever reaches the body satisfies the bound, so this is a looser statement of the bound than
  "these are the only methods in scope", not an unsoundness.
- A **default body is only checked for the impls that use it**. A default that no impl omits is
  never type-checked, exactly as a generic function that is never called is never checked. When it
  is used, it is checked once per implementing type, so an undeclared method it calls is an error at
  that impl.
- An impl's body is checked like any other method body. Nothing requires an `area` to be the area of
  anything, nothing requires two impls of different traits for one type to agree, and there is no
  way for a trait method to call another trait method through `Self` (it has no `Self` value).
- A trait signature's parameter names are compared with the impl's, so a rename is refused, but
  nothing checks the *order* of a `when` inside a body, or that an impl method's body does not
  panic.
- A trait method whose name matches a compiler-provided method of a builtin (`i32#to_u8`,
  `String#len`) shadows the builtin at a call site, because a declaration is looked up before the
  builtin table. Nothing refuses the shadowing.
- A default body's **types, free function calls and consts** resolve in the trait's module, but its
  *capabilities* and *passdowns* are read in the implementing module: only the type/name scope is
  swapped. A default that reaches a capability-gated call or a `pd` the trait's module owns is
  therefore checked against the impl's module, which is a narrower surface than the trait module's
  and may refuse the call.

## 7. How it is implemented

- `parser.rs` parses the two declarations, the bound list and the supertrait list, and refuses
  every shape section 5 names. A trait method is an ordinary `FunctionDecl` whose body is
  `FunctionBody::Signature` when none was written; a default method keeps its `Block`/`Expr` body.
  An impl method is an ordinary `FunctionDecl` whose `trait_impl` field records the trait and the
  written `for` type. `impl Trait for i32` is no longer refused at the parser.
- `resolve.rs` merges the traits and the impl records, canonicalizes both names of an impl line
  (leaving a builtin `for` name unqualified, since it has no module), and rewrites each impl method:
  `Self` becomes that canonical type everywhere in its signature. It then **synthesizes a concrete
  copy of every defaulted method an impl omits**, with `Self` substituted, tagged with the impl site
  and resolved in the impl's module for its signature and in the trait's module for its body (the
  new `FunctionDecl::body_module`), so the ordinary declaration loop checks and emits it exactly
  like a written impl method. Nothing downstream knows a trait default existed.
- `infer.rs` is where the whole feature lives: the trait table (with its supertraits), its
  signature validation, the coherence and module rules, the exact-signature match against each impl,
  the inherent-method collision message, the builtin `for`-type check, the supertrait requirement
  at an impl and the transitive supertrait check at a bound, and the bound check at
  `instantiate_function`, which is the one place a generic function is instantiated.
- `fmt.rs` needs no change to its emitter: it is token-level, and the new syntax is a `:`, `+`, a
  body and a signature it already re-emits. The one edit is extending the corpus pair
  (`tests/fmt_corpus/traits_syntax`) with a default method, a supertrait and a builtin impl.
- `plan/kernel-ub-notes.md` carries this feature's note: no new way to reach undefined behaviour.

## 8. Tests

`kelpc/tests/compile_and_run.rs` runs each of these on the backend under test (LLVM by default,
the C oracle under `KELPC_ORACLE=c`), and `tests/fixtures/` is built by the coverage sweep as well:

- a default method used as written, one overridden by an impl, and a default that calls another
  trait method;
- an empty impl of a trait whose methods are all defaulted;
- a supertrait call through a `<T: B>` bound and a `T: B` satisfying a `T: A` bound transitively;
- a trait implemented for `i32` and for `String` and used through a generic function;
- the rejections: a missing supertrait impl, a supertrait cycle, a builtin impl when the trait is
  declared in another module, an override with the wrong signature, and a default body that calls an
  undeclared method;
- the cross-module project `tests/projects/traits_cross` and the freestanding
  `tests/projects/freestanding_min`, which uses a default method;
- `kelpc fmt` round-trips trait/impl/bound/default/supertrait syntax.
