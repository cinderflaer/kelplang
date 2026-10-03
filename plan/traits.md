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

## 9. Trait objects, `&dyn` (A-12 slice 4)

So far a trait is a bound on a type parameter: the concrete type is known at every call, so
dispatch is static and neither backend lowers anything new. This slice adds the borrowed trait
object, `&dyn Name` and `&mut dyn Name`, so one function can call one of several types through a
runtime vtable. The value is still a borrow: the compiler never owns a trait object.

```
trait Shape {
    fun Self.area().R: f64
    fun Self.describe().R: String { return "a shape" }
    fun mut Self.scale(f: f64)
}

trait Named: Shape { fun Self.name().R: str }

datac Circle(val r: f64)
impl Shape for Circle { fun Self.area().R: f64 = r * r * 3.14159 }

fun describe(s: &dyn Shape).R: String = s.describe()
fun grow(s: &mut dyn Shape) { s.scale(2.0) }

fun main() {
    val c = Circle(r = 2.0)
    print(describe(&c))         // the coercion happens here
    val d: &dyn Shape = &c      // or at a typed binding
    print(describe(d))          // a `&dyn` forwards to another function
}
```

### 9.1 Where a trait object may be written

- The two type spellings are `&dyn Name` and `&mut dyn Name`. A bare `dyn Name` (no `&`) is
  refused where it is not the argument of a `Box`: an owned trait object needs a place to own its
  data, which is `Box<dyn Name>` (slice 7, section 12), and a bare `dyn Name` is an unsized value
  with nowhere to live.
- A trait object is legal **as a parameter type and as a local binding type** (`val`/`var`). A
  parameter includes a `fun` literal's parameter and the parameter of a function *type*, which is
  a parameter position too. It is refused everywhere else by name: as a `.R:` return type, as a
  `datac`/`enum` field or payload, as a `List`/`Box`/`Map`/`Atomic`/array element or argument, as
  a tuple element, and as a `static`'s type. A function type cannot return one either, which is
  the same borrow-return rule. A bare `dyn Name` is refused by name; `Box<dyn Name>` is slice 7
  (section 12). Nothing else changed: a borrow still cannot be returned or stored.
- `Name` must be a declared trait. `&dyn Unknown` is refused where it is written.
- The `mut` in `&mut dyn Name` is the whole mutability story: `&mut dyn Name` permits a `mut Self`
  method and `&dyn Name` does not. There is no `mut &dyn` and no `dyn` without a borrow.

### 9.2 Object safety

A trait used behind `dyn` must be object safe. The rule is exact and the compiler refuses the trait
by name, naming the offending method:

- No method may **return `Self`**. A return type that mentions `Self` anywhere (including nested in
  a container) is refused.
- No method may **take another `Self`**: no parameter other than the receiver may mention `Self`.
  The receiver itself is always `Self` (or `mut Self`) and is what the erased pointer stands for,
  so it is not counted.
- Two methods of the same name are refused too, because the vtable names one field per method and
  two methods cannot share it. This can only happen through a supertrait that repeats a name.
- A generic method is already impossible (`traits.md` 1): a trait method declares no type
  parameters, so there is nothing to erase.

Object safety is checked where the `dyn` type is written, so the error names the trait and the
method even if that `dyn` value is never built.

### 9.3 Coercion

A `&T` where `T` implements `Trait` coerces to `&dyn Trait` at exactly two places, and both are
implicit, that is, the caller writes the ordinary borrow and nothing else:

- a **call argument**: `describe(&circle)` where `describe(s: &dyn Shape)`;
- a **typed `val`/`var` binding**: `val d: &dyn Shape = &circle`.

The rules, in full:

- The operand is an ordinary `&x` or `&mut x` (or a value that already is a `&T`, e.g. a parameter
  of a borrow type). A plain place with no `&` is *not* coerced: `describe(circle)` is an error and
  the message says to write `&circle`. This is the one place the slice is narrower than "an
  implicit borrow everywhere", and it keeps the coercion visible at the call site.
- `&T` coerces to `&dyn Trait`. `&mut T` coerces to both `&dyn Trait` and `&mut dyn Trait`.
  `&T` never coerces to `&mut dyn Trait`.
- `T` must implement `Trait` (an `impl Trait for T` anywhere in the program). A type that does not
  is refused by name at the coercion site.
- A `&dyn Trait` value coerces to the **same** `&dyn Trait` type, so one can be passed on to
  another function. It does not coerce to a different trait, and there is no upcast from
  `&dyn B` to `&dyn A` when `B: A`; that is out of this slice.
- Everything else is an explicit error: a type that implements the trait but arrives as an owned
  value (or as a bare place with no `&`) is not coerced, and neither is a `&dyn` where a concrete
  `&T` is expected. An `&mut` operand also has to be a durable place, exactly as an `&mut`
  argument of a declared parameter does: `&mut <a temporary>` is refused.

### 9.4 Representation: the fat pair and the vtable

A `&dyn Trait` value is **two pointers**: the address of the borrowed value (the *erased data
pointer*, `void*`) and the address of a static, read-only vtable. It is `Copy` (copying two
pointers moves nothing), it owns nothing, and dropping it drops nothing: it is a borrow, exactly
like a `Ty::Ref`, and the value it points at is still owned by whoever it was borrowed from.

- **One vtable per `(Trait, Type)` pair actually used in a coercion.** A pair that is never
  coerced to has no vtable. Each vtable is emitted once per program, as `static const` data in
  `.rodata` on both backends.
- **One slot per method, in one fixed order**: the trait's supertraits first, recursively in
  declaration order, then the trait's own methods in source order. So `&dyn Named` has `Shape`'s
  methods before `Named`'s, and a supertrait method is callable on a `&dyn Named`.
- **Each slot is a thunk** whose first parameter is the erased data pointer and whose remaining
  parameters and return are the method's own. The thunk calls the concrete method
  (`Type#method`, the same `Type#name` key an ordinary receiver call uses), including a
  defaulted method's synthesized per-impl copy and a supertrait's impl on the type. The concrete
  method's receiver is read from the erased pointer: a borrowed receiver is the pointer itself,
  and a `Copy` builtin receiver (an integer scalar) is a load from it.
- **A method call goes through the vtable**: `s.area()` where `s: &dyn Shape` becomes
  `s.vtable.area(s.data)`. The call is bounds-free and allocation-free: no libc, no allocator, no
  witness argument and no runtime type tag. A freestanding image can use `&dyn`, and the vtables
  cost only `.rodata`.
- **A `mut Self` method needs a `&mut dyn`**: calling `scale` through a `&dyn Shape` is refused by
  name (the receiver is a shared borrow), while a `&mut dyn Shape` permits it. A non-`mut`
  method is callable through either.

### 9.5 Borrows and soundness

The coercion is not a new borrow: the `&x` inside it is the borrow the language already had, and
the borrow checker sees the same node. So:

- a `val d: &dyn Shape = &c` keeps `c` borrowed for the rest of the scope, exactly as `val p =
  &c` does, and `c` cannot be moved or reassigned while `d` lives;
- `&mut c` coerced to a `&mut dyn` is an exclusive borrow, so it excludes every other borrow,
  shared or exclusive;
- a `&dyn` passed as a call argument is a call-scoped borrow, released after the call, exactly as
  the `&x` it came from is;
- a `&dyn` never outlives its referent because it can never be stored (9.1) and never returned.
  There is no lifetime syntax in v1 and this slice does not add one.

### 9.6 What is deliberately not in this slice

Each refused where it is written:

- **`Box<dyn Name>`**, an owned trait object. **Shipped in slice 7** (section 12); a bare
  `dyn Name` stays refused by name.
- **A bare `dyn Name`** (an unsized value in place). Refused by name.
- **`&dyn` in a field, a payload, a container element, a return type or a `static`.** These are
  the storage positions a borrow may not take, and the refusal is the ordinary borrowed-type one.
- **Upcasting `&dyn B` to `&dyn A`** when `B: A`. Refused: a `&dyn` coerces only to its own type.
- **Object-unsafe traits** (returning `Self` or taking another `Self`). Refused by name.
- **A trait object over a trait with a lifetime or an associated type.** Neither exists in v1.
- **A `dyn` of a trait implemented for a builtin with no addressable receiver.** The supported
  builtin receivers are the ones a trait may already be implemented for (`traits.md` 4), and the
  thunk loads a `Copy` scalar or passes the borrow for every other one.

### 9.7 What the compiler does not check

- The compiler does not check that a vtable's slot order matches any other vtable's. It is one
  order for the (trait, type) pair, computed the same way at every call site, so a mismatch would
  be a compiler bug rather than a program's mistake.
- Nothing requires a trait method's body to be consistent across the types that implement it,
  exactly as static dispatch does not (6).
- The vtable is emitted per pair used; a call through a `&dyn` always knows the trait from the
  type, so there is no dynamic type tag and no way to ask a `&dyn` what concrete type it holds.
- A `&dyn Trait` cannot be compared, printed, hashed or used as a container element; the compiler
  refuses each position for the ordinary reason (there is no such method, or it is a borrow).

### 9.8 How it is implemented

- `parser.rs` parses `&dyn Name` / `&mut dyn Name` in a type position and records it on
  `TypeExpr`; the `dyn` refusal that shipped in slice 2 is replaced by the object-safety-aware
  parse path. `fmt.rs` needs no change (token-level).
- `infer.rs` resolves the type to `Ty::Dyn { mutable, trait_name }`, checks object safety, records
  each `(trait, type)` coercion pair, lowers a coercion to a `tir::ExprKind::DynCoerce` and a
  method call on a `&dyn` to a `tir::ExprKind::DynCall`. The vtable list lands in `tir::Program`.
- `types.rs` gives `Ty::Dyn` its `c_type`/`display`/`mangle`/`is_copy`/`owns_heap` answers and its
  refusal as a stored type.
- `codegen_c.rs` emits the vtable struct typedef per trait, the `&dyn` struct typedef, one thunk
  and one `static const` vtable per pair, and lowers `DynCoerce`/`DynCall`. `codegen_llvm.rs`
  mirrors all of it as an identified struct, a `[N x ptr]` global and thunk functions.
- `borrowck.rs` looks through `DynCoerce` to the `&x` node inside it, so every borrow, move and
  drop rule applies unchanged.

### 9.9 Tests

`kelpc/tests/compile_and_run.rs` runs each of these on the backend under test (LLVM by default,
the C oracle under `KELPC_ORACLE=c`):

- two types implementing one trait, called through `&dyn` in a loop over borrowed bindings;
- a default method and a supertrait method through `&dyn`;
- `&mut dyn` mutating its target;
- passing a `&dyn` on to another function;
- a trait implemented for `i32` used through `&dyn`;
- a freestanding riscv64 project that uses `&dyn`, built with `llvm-nm -u` clean and booted under
  QEMU with a timeout;
- the rejections: an object-unsafe trait returning `Self`, `Box<dyn T>`, `dyn` in a `datac` field,
  returning a `&dyn`, coercing a type that does not implement the trait, and `&dyn` of an unknown
  trait;
- `kelpc fmt` round-trips the `&dyn` spelling.

A `&dyn` parameter-only variant of the slice is recorded in `plan/traits-dyn-status.md` if the
local-binding half has to be cut; this file is the decision either way.

## 10. Associated types (A-12 slice 5)

A trait may name a **type** that each impl supplies, so a trait can promise "I produce a `next`
value" without promising what that value is. The trait writes `type Item`, the impl writes
`type Item = i32`, and a bound type parameter reaches the type as `I::Item`. Dispatch stays
static: `I::Item` is a concrete type at every instantiation, so nothing in either backend
changes and a freestanding image may use one.

```
trait Iterator {
    type Item
    fun mut Self.next().R: Option<Self::Item>
}

datac Counter(val n: i32)

impl Iterator for Counter {
    type Item = i32
    fun mut Self.next().R: Option<i32> {
        n += 1
        return Option.Some(value = n)
    }
}

// `I::Item` is usable in a parameter, a return and a local type.
fun first_of<I: Iterator>(it: mut I).R: Option<I::Item> {
    return it.next()
}
```

### 10.1 Where an associated type may be written

- A trait body may declare one or more associated types, before, between or after its methods:
  `type Item`, in the same body, with an optional bound list after a `:` (`type Item: Named`,
  `type Item: Named + Other`). An associated type is a bare name; the trait may not repeat one.
- An impl must define **every** associated type the trait declares, as `type Name = Type`, and may
  not add one the trait does not declare. A missing one, an extra one, and a duplicate one are
  three errors, each naming the trait and the type.
- `Self::Item` is legal in a trait method signature and in the body of a default method, where
  `Self` stands for the implementing type. The impl's own methods may write `Self::Item` too, and
  it means the type the impl bound.
- `T::Item` is legal wherever `T` is a type parameter in scope: a generic function's parameter,
  return, local and field-adjacent types, `fun f<I: Iterator>(it: mut I).R: Option<I::Item>`. It
  is resolved at every instantiation, from the impl the concrete argument names.
- An associated type is **projected from a bound**, so `I::Item` needs `I` to carry a bound whose
  trait declares `Item`. A projection through a type that has no such bound is refused where it is
  written, naming the association.

### 10.2 A bounded associated type

`type Item: Named` bounds what an impl may bind. The bound is checked **at every impl**: the
concrete type of `type Item = X` must implement every trait the declaration names, or the impl is
refused naming the trait, the associated type and the concrete type. Inside generic code the bound
needs no separate machinery: the body is checked once per concrete type, so `I::Item` is already
the concrete type and a `Named` method resolves on it directly.

### 10.3 What is deliberately not in this slice

Each refused where it is written, with the message naming the follow-up:

- **Default associated types** (`type Item = i32` inside a trait). Refused by name: an associated
  type has no default in v1, so the impl must bind it.
- **Associated consts** (`const N: i32` inside a trait). Refused by name.
- **Generic associated types** (`type Item<T>`, and a bound that names one). Refused by name.
- **Equality constraints on an associated type** (`I: Iterator<Item = i32>`). Refused by name:
  it needs the projection to be a first-class type in a bound, which is a later slice; a plain
  bound plus a projection at the use site is the workaround.
- **An associated type reached through `&dyn`.** A trait that declares an associated type is
  **not object safe**: a `&dyn Name` of it is refused by name, because the erased value has no
  single `Item` to name. Static dispatch through a bounded type parameter is the workaround.
- **A generic trait** (`trait Name<T>`) and a `where` clause stay refused, exactly as in section 5.

### 10.4 What the compiler does not check

- An impl's binding is a type like any other and nothing relates it to the trait's methods beyond
  the signatures the impl writes. `Iterator for Counter` may bind `Item = i32` and return a
  `String` from a method, and nothing refuses it unless the trait's own signature fixes the
  return type and is compared at the impl.
- A bound on an associated type is checked at the impl and nowhere else. A generic function that
  is never instantiated is never checked, exactly as a bound on a type parameter is not.
- Two impls of one trait bind their associated types independently. Nothing requires them to
  agree, and nothing checks that an associated type's name is meaningful.

### 10.5 How it is implemented

- `lexer.rs` and `fmt.rs` learn the `::` token, which is glued on both sides exactly as `.` is.
- `parser.rs` accepts `type Name[: Bound + Bound]` in a trait body and `type Name = Type` in an
  impl body, records the projection on `TypeExpr::assoc`, and refuses every shape 10.3 names.
- `resolve.rs` substitutes `Self::Item` in an impl method (and in the synthesized copy of a
  default) with the type the impl bound, so the ordinary signature loop sees a concrete type.
- `infer.rs` holds the trait's associated-type declarations in `TraitInfo` and the concrete
  bindings in one `(trait, type) -> name -> Ty` table. It checks the impl against the declaration
  (a missing, extra or duplicate binding, and each declared bound), refuses a projection that no
  bound names, and adds the projection to the instantiation's parameter map so `I::Item` resolves
  to the concrete binding for that instantiation. `check_dyn_object_safety` refuses a trait that
  declares an associated type. Nothing downstream knows an associated type existed: the instance's
  signatures already name concrete types.
- `plan/kernel-ub-notes.md` carries this feature's note: no new way to reach undefined behaviour.

## 11. Generic traits, `From`, and error conversion for `?` (A-12 slice 6, roadmap A-3 and A-12)

*Design note. Slice 2 gave a trait methods with `Self` receivers, slice 3 gave it default bodies,
supertraits and impls for builtins, slice 4 gave it `&dyn`, and slice 5 gave it associated types.
This slice lets a trait take **type parameters of its own**, so a trait can say "I convert a `T`"
rather than "I have these methods". Dispatch stays static, so neither backend learns a new
lowering and a freestanding image may use one. On top of it, `?` converts its failure through a
`From` impl, which is the last piece roadmap A-3 named.*

### 11.1 A trait may take type parameters

```
pub trait From<T> {
    fun from(value: T).R: Self
}

pub trait Into<T> {
    fun Self.into().R: T
}

trait Store<K, V: Named> {
    fun Self.get(key: K).R: V
}
```

- A trait's type-parameter list is the same one a `datac`, an `enum` and a `fun` take, written
  after the trait's name and before any `:` supertrait list: `trait Name<A, B: Bound>`. A
  parameter is in scope in every method signature and default body of the trait, alongside `Self`,
  and a trait may not declare the same parameter twice. A trait's own parameters are *not*
  monomorphized into hidden instances; they are substituted at each `impl` and each bound, exactly
  as a generic function's parameters are substituted at each call.
- The **trait name plus its arguments** identifies a *trait instance* - `From<IoErr>` and
  `From<String>` are two different traits to implement and to bound, while sharing one declaration
  and one method list.
- A trait type parameter's own bound (`V: Named`) is checked where the trait is instantiated: the
  concrete type an `impl`'s argument names has to implement the bound, and so does a bound's own
  argument, with the same rule a bound on a function's type parameter uses.
- A generic trait may name **supertraits**, and a supertrait without arguments works exactly as it
does for an ordinary trait (`trait Sorted<T>: Named`), so `impl Sorted<i32> for X` needs `impl
Named for X` and a `<T: Sorted<i32>>` may call `Named`'s methods. A supertrait *with* arguments
(`trait Sorted<T>: Compare<T>`) is a follow-up, refused by name: the substitution a parameterized
supertrait needs is a second rule on top of the impl's own arguments.
- **Refused by name.** The trait arguments a bound or an impl writes are a *named* type (a
  `datac`/`enum`, dotted through an alias as usual) or a builtin type; a container, a tuple, an
  array, a borrow, a function type and a handle are refused where they are written, with the
  follow-up named. A generic trait may not declare an associated type
  (`trait Name<T> { type Item }`) and may not be used behind `dyn`: an associated type has no
  single binding to name and the vtable has no slot for a parameter, so both are follow-ups. A
  `where` clause and an equality constraint on a parameter stay refused, exactly as in section 5.

### 11.2 A static method has no receiver

An ordinary trait method is reached through a value and writes a `Self` receiver. A method that
has no `self` at all is **static**, and is written with no receiver:

```
pub trait From<T> {
    fun from(value: T).R: Self
}

datac AppErr(val code: i32, val note: String)

impl From<i32> for AppErr {
    fun from(value: i32).R: AppErr = AppErr(code = value, note = "int")
}

fun main() {
    val e = AppErr.from(7)          // the static method, on the type name
    print(e.code)
}
```

- A static method is a trait signature with no body, or an impl method with no `Self` receiver and
  exactly the declared signature. It is called on the **type**: `AppErr.from(7)`, and through a
  **bounded type parameter**: `X.from(7)` for `fun f<X: From<i32>>(v: i32).R: X`. The return type
  `Self` is the implementing type, so the call's type is `AppErr` (or `X`).
- `AppErr.from(7)` is resolved against the impls of every trait the type has: the compiler finds
  the impl whose trait declares a static `from` and whose substituted parameter types match the
  call. A type that implements two different instantiations of the same trait (`From<i32>` and
  `From<String>`) is dispatched by the argument type, so `AppErr.from(7)` and
  `AppErr.from("x")` are different methods of the same name.
- A **trait-name call** (`From.from(7)`) is allowed only when the implementing type is inferable:
  the trait name alone does not name one, so the expected type has to fix `Self` (`val e: AppErr =
  From.from(7)`), and the call then resolves against that type's impls exactly as `AppErr.from(7)`
  does. A call whose implementing type is not fixed by the context is refused by name, with the
  `Type.from(...)` spelling named instead.
- An impl may not mix receiver and static methods of the same trait incorrectly: a method the
  trait declares with a receiver has to be written with one, and a static member has to be written
  without. The mismatch is the ordinary signature error.
- A static method shares the trait method's name with any other method the *type* declares, so a
  second static method of the same name (from another impl of the same instance) is the ordinary
  duplicate error, and an inherent receiver method of that name collides at the impl exactly as it
  does today.

### 11.3 Coherence and the impl's trait arguments

- The `impl` line writes the trait's arguments after its name: `impl From<IoErr> for AppErr { ... }`.
  **One impl per (trait instance, type) pair**: `impl From<IoErr> for AppErr` and
  `impl From<String> for AppErr` are two legal impls, and a second `impl From<IoErr> for AppErr` is
  the coherence error naming both.
- The impl's methods are resolved with the trait's parameters substituted by the impl's arguments
  and `Self` by the implemented type, so the same trait signature is checked against every impl
  exactly as it is today.
- Everything else in sections 2 to 4 holds unchanged: the type has to be a `datac`/`enum` or a
  builtin that carries methods, the impl lives in the trait's module or the type's (the trait's only,
  for a builtin), and every non-defaulted signature has to be implemented with exactly the declared
  shape.

### 11.4 `std/convert`

The shipped `std/convert` declares the two conversion traits and nothing else:

```
// `std/convert`
pub trait From<T> {
    fun from(value: T).R: Self
}

pub trait Into<T> {
    fun Self.into().R: T
}
```

- **There are no blanket impls.** `impl<T> Into<U> for T where U: From<T>` would need a
  compiler-known blanket impl, which this slice does not add, so a type that wants `into` writes
  its own `impl Into<U> for X` (or uses `From` directly). The reason is the same one section 5
  gives for `where` clauses: a blanket impl is a second inference rule in a slice whose whole point
  is that `?` can call `From` with no new machinery.
- `?` uses `From` directly, never `Into`, so a program that only propagates errors writes only
  `impl From<...> for ...`.

### 11.5 The `?` operator converts through `From`

The operator's existing shape is unchanged: the operand is an enum shaped `Ok`/`Err` or
`Some`/`None` held by value, and a failure returns immediately out of the enclosing function. What
is new is only the `Ok`/`Err` failure payload:

- When the operand's failure payload type **is** the enclosing function's failure payload type,
  `?` moves it across as before; no conversion call is emitted.
- When the two differ, and an `impl From<E1> for E2` exists where `E1` is the operand's failure
  payload and `E2` the function's, `?` calls `E2.from(e1)` and returns the result in the failure.
  The signature is checked: the impl's `from` has to take exactly `E1` and return exactly `E2`, so
  the call is the ordinary static call of section 11.2.
- When no such impl exists, the error names both failure types and names `From`, and says to write
  `impl From<E1> for E2 { fun from(value: E1).R: E2 { ... } }`.
- `Some`/`None` has no payload to convert, so `Option` to `Option` is unchanged: the two have to
  be the same shape and there is nothing else to check.
- The two enums may be different declarations (`Result` propagated out of a function that returns
  a program's own `Ok`/`Err` enum), exactly as the shape rule already allowed; only the failure
  payload type is now allowed to differ, and only through `From`.

### 11.6 What is deliberately not in this slice

Each refused where it is written, with the message naming the follow-up:

- **A blanket impl** (`impl<T> Into<U> for T`, `impl<T: From<T>> ...`). There is no blanket impl
  of any trait in this slice; `std/convert`'s `Into` is declared but not derived for anything.
- **`Into` in `?`.** `?` uses `From` only, so an `Into` impl with no matching `From` does not
  convert.
- **An associated type in a generic trait**, and **`&dyn` of a generic trait.** Both are refused
  by name: the projection and the vtable each have no slot for a trait parameter.
- **A generic trait argument that is not a named type or a builtin** (a container, a tuple, an
  array, a borrow, a function type, a handle). Refused by name.
- **An equality constraint** (`I: Iterator<Item = i32>`) and a **`where` clause**, as before.
- **A generic method of a trait** (`fun Self.map<U>(...)`), and a **generic static method**.

### 11.7 What the compiler does not check

- A static method's arguments are matched by the *resolved* parameter types, not by the written
  argument names alone; a call that matches two impls of the same name is refused as ambiguous
  rather than picked by source order, but a call whose arguments match exactly one impl has no
  other check that the impl is "the intended" one.
- A `From` impl's body is checked like any other method body: nothing requires the returned `Self`
  to be built from `value`, and a `from` that ignores its argument compiles.
- The trait's own type parameters are substituted structurally at each impl; nothing checks that
  two impls of different instances of one trait agree about anything.
- An uninstantiated generic function whose body would fail a bound is never checked, exactly as
  before.

### 11.8 How it is implemented

- `parser.rs` parses a trait's type-parameter list, a receiverless method (a static signature or a
  static impl method), the `impl From<IoErr> for AppErr` argument list, and a bound with
  arguments (`<X: From<i32>>`). It refuses the shapes 11.6 names where the tokens are, and it
  keeps the trait argument list in `TraitImplDecl`.
- `resolve.rs` canonicalizes each trait argument (a dotted name by the usual `canon`, a builtin
  left alone) and stores the resulting **trait instance key** (`From<pkg.IoErr>`) on the impl
  record and on every method's `TraitImplSite`. A static impl method's function key gets the
  instance appended (`AppErr#from@From<pkg.IoErr>`), so the two instances of one trait for one type
  are two functions rather than a collision; `types::c_ident` already mangles the `@` and the
  angle brackets, so both backends emit them unchanged.
- `infer.rs` holds the trait's parameters in `TraitInfo` and keys the `impls` table by the trait
  instance. It substitutes the impl's arguments into the trait's signature when it checks the
  impl, checks each declared parameter bound, and resolves a static call by searching the impls of
  the receiver type (or of the concrete type a bounded parameter is instantiated with). The `?`
  node carries an optional conversion key, which codegen lowers as the ordinary static call.
- `codegen_c.rs` and `codegen_llvm.rs` need no new node: a static method is an ordinary free
  function keyed by its mangled name, and the `?` conversion is one more function call in the
  failure arm.
- `std/convert.kp` is added to `stdlib.rs`'s source table and to the freestanding allowed set (it
  is pure language, so a kernel may use it).
- `plan/kernel-ub-notes.md` carries this feature's note: no new way to reach undefined behaviour.

### 11.9 Tests

`kelpc/tests/compile_and_run.rs` runs each of these on the backend under test (LLVM by default,
the C oracle under `KELPC_ORACLE=c`), and `tests/fixtures/` is built by the coverage sweep as well:

- `From<A>` and `From<B>` implemented for one error `enum`, used by `?` on a function returning
  each, with the same type implementing two instances of one trait;
- a generic function bounded `<X: From<i32>>` that calls `X.from(x)`;
- a `From` impl reached across modules (the `tests/projects/convert_cross` project);
- the existing `?` fixtures pass untouched;
- the rejections: `?` with no `From` impl (the message names both failure types and `From`), two
  identical `From<A> for T` impls, a generic trait used as `&dyn`, and a wrong arity of trait
  arguments;
- `kelpc fmt` round-trips generic-trait syntax (`tests/fmt_corpus/traits_generic`);
- the freestanding project `tests/projects/freestanding_convert` uses `std/convert` and boots under
  QEMU with no undefined symbol.

## 12. Owned trait objects, `Box<dyn>` (A-12 slice 7)

Slice 4 gave a trait object a *borrowed* spelling, `&dyn Name` / `&mut dyn Name`. This slice adds the
owned one: `Box<dyn Name>`, a fat owned pointer that knows how to drop what it points at. Nothing
else about a trait changes, and no new syntax is introduced beyond the `Box<...>` spelling the
container already has.

```
trait Shape {
    fun Self.area().R: f64
    fun Self.kind().R: str
    fun mut Self.scale(f: f64)
}

datac Circle(val r: f64)
impl Shape for Circle { fun Self.area().R: f64 = r * r * 3.14159 }

datac Holder(val shape: Box<dyn Shape>)

fun make(big: bool).R: Box<dyn Shape> {
    if big {
        return Box.of(Circle(r = 2.0))   // the coercion happens here
    }
    return Box.of(Circle(r = 1.0))
}

fun describe(s: &dyn Shape).R: str = s.kind()

fun main() {
    var shapes: List<Box<dyn Shape>> = List.empty()
    shapes.push(make(true = true))
    val h = Holder(shape = make(big = false))
    var i = 0
    while i < shapes.len() {
        print(shapes.get(i).area())      // through the vtable
        i = i + 1
    }
    print(describe(h.shape.get()))       // a `&dyn` borrowed from the box
    print(shapes.get(i = 0).kind())
}
```

### 12.1 Representation

- `Box<dyn Name>` is a **fat owned pointer**: the address of the heap value (the erased data
  pointer) and the address of the same static, read-only vtable a `&dyn Name` carries. It is two
  machine words, laid out exactly as `&dyn Name` is, and the two share one C struct and one LLVM
  identified struct per trait.
- It is **not `Copy`**: it owns the allocation `Box.of(x)` made, and it drops it. Its type is
  `Ty::BoxDyn`, a type of its own rather than a `Box<T>` whose element is a `Ty::Dyn`, because the
  element of a `Box` is owned by value and a trait object is not a sized value.
- The **vtable** is the one slice 4 defined (the supertraits' methods first, recursively in
  declaration order, then the trait's own methods in source order, one slot each, one thunk per
  slot over the erased data pointer) with three entries appended: a **drop** entry, a **size** and
  an **align**. The extra entries are the same for every pair of one trait's layout, and the drop
  entry is the concrete type's drop glue. A vtable is still one per `(trait, type)` pair actually
  used, emitted as read-only data (`.rodata`) on both backends.
- There is deliberately no runtime type tag: `Box<dyn Name>` is not a value with a dynamic type, it
  is a borrow of the trait's method set plus the ownership of the storage behind it.
- `Box<dyn Name>` is an owned value with no borrows, so it is a legal `datac`/`enum` field, a
  `List` element, a parameter (by move), a return type and a local. It is refused as a `Map` key
  and as an `Atomic`/`Mutex` element for the reasons those sets are closed, and refused as a
  `Thread<T>` element only by the ordinary Send-shaped rule (12.5).

### 12.2 Construction: the coercion, and where it happens

There is one way to make a `Box<dyn Name>`: coerce a `Box<T>` where `T` implements `Name`. The
coercion is **implicit at every position that knows an expected type**, which is the same list a
`&T` used in slice 4 and the same mechanism: a call argument, a typed `val`/`var` binding, a
`return`, a `datac`/`enum` field initializer, and a container's element argument
(`xs.push(Box.of(circle))`).

- The operand must be a `Box<T>` whose `T` implements `Name`. A `Box<T>` whose `T` does not is
  refused by name at the coercion site; a value that is not a box at all (a bare `Box.empty()` with
  nothing to decide its element, or a `List`) is an ordinary type mismatch.
- **The coercion is a move, not a copy**: the box's allocation becomes the trait object's
  allocation, and the operand's binding is moved out exactly as `val b2 = b` moves a box. So the
  factory above builds one allocation and no second one, and a `Box<dyn>` in a `return` is the box
  the callee already had.
- `Box.new(x)` is spelled `Box.of(x)` in this language; the move is the same one.
- A `Box<dyn Name>` value coerces to the same `Box<dyn Name>` type, so one can be moved on to
  another binding or another function. It does not coerce to a different trait: there is no upcast
  from `Box<dyn B>` to `Box<dyn A>` when `B: A`, exactly as a `&dyn B` does not.

### 12.3 Calls, borrows, and getting a `&dyn` back

- A method call on a `Box<dyn Name>` goes **through the vtable**, exactly as it does on a
  `&dyn Name`: `b.area()` is `b.vtable[slot](b.data)`. A `mut Self` method needs the receiver to be
  a writable place (a `var` binding, a field, or a `&mut Box<dyn Name>`), the same rule an
  `&mut x` argument follows. A call through a `&Box<dyn Name>` or `&mut Box<dyn Name>` works too:
  the borrow is read (or writes through) to reach the box, and the contents are reached through the
  box's own pointer.
- A `&dyn Name` is borrowed from the box with `b.get()`, the same spelling `Box<T>::get` already
  uses: `b.get()` on a `Box<dyn Name>` yields `&dyn Name` (a fat pair pointing at the box's data,
  which is a borrow, not a second owner). That is how a boxed shape is passed to a function taking
  `&dyn Name`. `Box.get` on a `&dyn`-shaped element is the one method the box adds here; `get`,
  `is_empty` and the drop are its whole method surface. `take`/`set` are refused by name because
  they would have to move an unsized value in or out.
- `Box.empty()` is legal for a `Box<dyn Name>` written type: it is the null fat pair, the same
  "nothing at all" a `Box<T>` has, and an empty box panics from `get()` exactly as an empty
  `Box<T>` does.

### 12.4 Dropping through the vtable

- Dropping a `Box<dyn Name>` runs the concrete type's drop glue **through the vtable**: the drop
  entry calls the same `kelp_drop_<T>` a `Box<T>` calls (the user `drop` first, then the fields in
  declaration order), and the box's own drop then frees the allocation. So `List<Box<dyn Shape>>`
  drops its elements through each element's own vtable, and a `datac` field of the type drops the
  same way.
- The drop entry is emitted per `(trait, type)` pair, so it names the concrete method's own drop
  function. A type that needs no drop (a `Copy` scalar) gets a thunk that does nothing, and the
  box still frees its allocation.
- The size and align entries are emitted as data beside the drop entry. The box's own free reads
  them when the project supplies an allocator whose free takes a size and an alignment (the
  freestanding hook), because the size of an erased value is not known at the free site; libc's
  `free` does not need them. They exist so the table describes the concrete type completely, which
  is what a later slice that needs the layout of an erased value would reach for.

### 12.5 Moves, borrows and threads

- The borrow checker treats a `Box<dyn Name>` exactly as it treats any owned `Box`: reading it as a
  whole moves it, a partial move out of it is refused by the ordinary rules, and its drop runs at
  the drop points the plan already records (block end, before a `return`, reassignment of an owned
  `var`, a container's element drop, an `atexit` pd drop).
- A **thread** may move a `Box<dyn Name>` because the existing crossing rule admits an owned value
  with no reachable borrow (MK9 section 5, rule 4). A `&dyn Name` — a borrow — is refused by rule 2
  exactly as any other borrow of a frame local is. This is the whole of the `Send`-shaped story in
  v1: there is still no `Send` marker to implement.
- The vtable is emitted per `(trait, type)` pair and the concrete type's drop is reached through it,
  so a `Box<dyn Name>` in a freestanding image costs the same `.rodata` a `&dyn` did and needs no
  libc beyond the allocator the project supplies.

### 12.6 Object safety, and what is refused by name

Object safety is unchanged from slice 4 and is checked where the `dyn` is written, so `Box<dyn
Name>` refuses the same traits `&dyn Name` does, with the same messages:

- a method returning `Self`, a parameter other than the receiver that mentions `Self`, and two
  methods of one name;
- a **generic trait** (`trait Name<T>`), because the vtable has no slot for a type parameter;
- a trait that declares an **associated type**, because the erased value has no single binding to
  name.

And the spelling itself:

- **`Box<dyn A + B>`** is refused by name: a trait object names one trait, and the trait that
  combines two is a declared trait with `A` and `B` as supertraits.
- **A bare `dyn Name`** outside a `Box<...>` stays refused by name.
- **`Box<dyn>` of a trait that is never implemented** is legal to *name* (a parameter may take a
  boxed trait nothing implements in this program), but a coercion site whose type is not an impl is
  refused by name, with the `impl Name for T` line to write.

### 12.7 What is deliberately not in this slice

- **Upcasting** `Box<dyn B>`/`&dyn B` to a supertrait object. Refused by name.
- **`Box<dyn>` of a generic trait** and of a trait with an associated type, for the reason 12.6
  gives.
- **Moving the value out of a box** (`Box.take` on a `Box<dyn>`, or a `*b` dereference that would
  produce an unsized place). Refused by name: the concrete value's size is not known at the use
  site, which is what `get()` and the vtable are for.
- **`Box<dyn Name>` printed, compared, hashed or used as a `Map` key.** Each position refuses it
  for the reason it already refuses a `&dyn`.

### 12.8 What the compiler does not check

- The vtable's slot order is computed the same way at the coercion site and at the call site, so
  nothing checks that two vtables of one trait agree beyond the single flattening routine.
- Nothing checks that the concrete type's methods are consistent with the trait's *intent*, exactly
  as no trait method body is checked for meaning.
- A `Box<dyn Name>` whose box is empty is not tracked as such: `get()` panics at runtime, `is_empty`
  is how a program asks, and a `Box<dyn Name>` never dereferenced needs no check.
- The size and align entries are emitted but not read; a slice that reads them would be the first
  to depend on them.

### 12.9 How it is implemented

- `parser.rs` parses a bare `dyn Name` as a `TypeExpr` mark (rather than refusing it where the
  tokens are), so `Box<dyn Name>` reaches `infer.rs`; it refuses `dyn A + B` by name where the `+`
  is, and the `&dyn` path is unchanged.
- `types.rs` gives `Ty::BoxDyn` its `c_type` (the same fat-pair struct as `Ty::Dyn`), `display`,
  `mangle`, `is_copy` (false), `owns_heap` (true), its layout (two words) and its drop-function
  name. `drop_fn_name` is `kelp_box_dyn_drop__<Trait>`, one function per trait.
- `infer.rs` resolves `Box<dyn Name>` to `Ty::BoxDyn`, reusing the object-safety check, and lowers
  the coercion to a `tir::ExprKind::BoxDynCoerce` (a move of the box plus the pair's vtable). A
  method call on a `Box<dyn Name>` reuses `check_dyn_method_call`, and `Box.get`/`Box.is_empty`/
  `Box.empty` are added to the receiver's method surface. Each coercion records its `(trait, type)`
  pair in the existing `dyn_pairs` set, and each `dyn` type resolved records its trait so the pair's
  typedef exists even with no coercion.
- `tir.rs` carries `DynVtable::concrete` (the concrete `Ty`, so the drop entry can name its drop
  glue) and the `BoxDynCoerce` node.
- `borrowck.rs` sees `BoxDynCoerce` as a move of its operand, exactly as a `&x`-based `DynCoerce`
  is seen as the borrow inside it.
- `codegen_c.rs` and `codegen_llvm.rs` emit the appended vtable entries, the per-trait
  `kelp_box_dyn_drop__<Trait>`, and lower `BoxDynCoerce`. The LLVM table becomes the literal struct
  `{ [N x ptr], ptr, i64, i64 }` (the method array first, so the call site's slot index is
  unchanged, then the drop, the size and the align), and the C struct puts the same three entries
  after the same method slots, so the two backends' tables agree field for field.
- `plan/kernel-ub-notes.md` carries this feature's note.

### 12.10 Tests

`kelpc/tests/compile_and_run.rs` runs each of these on the backend under test (LLVM by default, the
C oracle under `KELPC_ORACLE=c`), and `tests/fixtures/` is built by the coverage sweep as well:

- a `List<Box<dyn Shape>>` of two implementors, summed through the vtable;
- a factory function returning `Box<dyn Shape>` chosen by a flag;
- a `datac` holding a `Box<dyn Trait>` field, and a `&dyn` borrowed back out with `get()`;
- drop order with a user `drop` on the concrete types, asserted by printed output;
- moving a box between two lists, and a method call through `&Box<dyn Trait>`/`&mut`;
- a box moved into a spawned thread (accepted) and a `&dyn` crossing refused;
- a freestanding riscv64 project (allocator hook) that builds with `llvm-nm -u` clean and boots
  under QEMU with a timeout;
- the rejections: a coercion of a type that does not implement the trait, an object-unsafe trait,
  `Box<dyn A + B>`, a partial move out of a box, and a use after move;
- `kelpc fmt` round-trips the `Box<dyn>` spelling.
