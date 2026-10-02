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
  refused: an owned trait object would need a place to own its data, which is `Box<dyn Name>`, and
  that is deliberately out of this slice.
- A trait object is legal **as a parameter type and as a local binding type** (`val`/`var`). A
  parameter includes a `fun` literal's parameter and the parameter of a function *type*, which is
  a parameter position too. It is refused everywhere else by name: as a `.R:` return type, as a
  `datac`/`enum` field or payload, as a `List`/`Box`/`Map`/`Atomic`/array element or argument, as
  a tuple element, and as a `static`'s type. A function type cannot return one either, which is
  the same borrow-return rule. `Box<dyn Name>` and a bare `dyn Name` are refused by name.
  Nothing else changed: a borrow still cannot be returned or stored.
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

- **`Box<dyn Name>`**, an owned trait object. Refused by name; the workaround is an `enum`.
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
