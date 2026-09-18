# The Kelp language

Kelp is a small compiled language. `kelpc` lexes and parses a Kelp source tree, checks types and ownership, emits C, and calls `cc` to produce a native binary. Programs link nothing beyond libc, have no garbage collector, and reach the outside world only through capabilities a module declares.

This file is the language reference. It is written to be read start to finish by someone who wants to write a program, so it starts with the toolchain and ends with a complete example. Where another plan file disagrees with this one about syntax or style, this file is correct.

| File | Contents |
| --- | --- |
| `plan/syntax.md` | this file: syntax, types, semantics, standard library |
| `plan/basicstructure.md` | compiler layout, capability model, non-syntax decisions |
| `plan/roadmap.md` | status, critical path, milestone scheme, open items |
| `plan/backlog.md` | named-but-unscheduled backlog: the `A`-`F` item lists, Coral, regulated domains |
| `plan/done.md` | record of what has shipped |

## 1. Getting started

### 1.1 Build the compiler

```sh
cargo build --release -p kelpc  # produces target/release/kelpc
```

`kelpc` takes two commands:

```sh
kelpc build <file.kp|project-dir> [-o <output>]   # default output: the file stem
kelpc run   <file.kp|project-dir>                 # build, then run
```

`kelpc run` does not forward arguments to the program. Build first and run the binary directly when the program needs `argv`.

The compiler needs a C compiler on `PATH` (`cc`, or `clang` through `CC`). The C stage is how a binary is produced today.

### 1.2 Hello world

```kelp
// hello.kp
fun main() {
    print("hello, world")
}
```

```sh
kelpc run hello.kp
```

`main` is the entry point. It takes no parameters and returns nothing, and it is reached through `process.args()` rather than an argument list.

### 1.3 Projects

A project is a directory with a `coral.toml`, sources under `src/`, and optionally an `alias.kpconf`:

```
myproject/
    coral.toml
    src/main.kp              # entry module, the only one with unprefixed names
    src/shapes/circle.kp     # impl "shapes/circle"
    alias.kpconf             # optional, see 15.3
```

```sh
kelpc build myproject/
```

A loose `.kp` file that is not inside a project still compiles, and is then its own module root.

### 1.4 `coral.toml`

The manifest is flat: one `key = value` per line, `#` starts a comment, sections are refused, and an unknown key is an error rather than an ignored line.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `name` | quoted string | none | project name |
| `version` | quoted string | none | project version, unvalidated free text |
| `entry` | quoted string | `"src/main.kp"` | path of the entry file, relative to the project root |
| `http_pool` | number | `4` | kept-alive connections per `host:port` in `std/http`; `0` disables pooling |

```toml
name = "myproject"
version = "0.1.0"
entry = "src/main.kp"
http_pool = 4
```

`http_pool` is the one numeric key. It is spliced into the shipped `std/http` source before it is compiled, so changing it changes the connection ceiling for the whole program.

## 2. Lexical structure

### 2.1 Comments

`//` starts a line comment. There are no block comments.

### 2.2 Statements and whitespace

There are no semicolons. A newline ends a statement, and a statement must fit on one line unless it is inside parentheses:

```kelp
val c = Circle(r = 2.0, x = 1.0)     // ok
val area = c.area()                  // ok

// multi-line calls, datac field lists and parenthesized expressions are fine:
print(math.pow(base = 2.0,
               exp = 8.0))
```

Inside `(` `)` newlines are insignificant, which is what lets a field list or an argument list wrap. Two statements on one line with no separator is an error. A wrapped expression must be parenthesized: a trailing operator does not continue the line.

### 2.3 Identifiers and keywords

An identifier starts with a letter or `_`. These words are keywords:

```
as  closure  const  datac  else  enum  extern  false  fun  if  impl  move
mut  pd  return  true  val  var  when  while
```

`fn`, `let`, `struct`, `use`, `for`, `loop`, `break`, `continue`, `match`, `trait` and `self` are not keywords. Writing `fn`, as in Rust or Kotlin, is a parse error, and `self` is an ordinary identifier (see 8).

### 2.4 Literals

```kelp
val a = 42              // integer literal
val b = 3.5             // float literal
val c = true            // bool literal
val d = "text\n"        // string literal
val e = "n = $a, sum = ${a + 1}"   // string template
```

- Integer literals are decimal digits only: no suffix, no hex, no digit separators. A literal has no width of its own; it takes the integer type its context requires, and `i32` when nothing determines it.
- Float literals are `digits.digits`, and are `f64` unless the context requires `f32`.
- The escape set is closed: `\n`, `\t`, `\r`, `\"`, `\\`. Any other escape is a diagnostic instead of the bare character, and `\0` is refused because a `str` is NUL-terminated.
- There is no character literal. `char` exists as a type but has no values in v1; use `u8` for a byte and `u32` for a codepoint (`str.byte`, `str.char_at`).
- A string literal is a `str`: a borrowed, NUL-terminated byte sequence. It is not owned and is not freed.

### 2.5 String templates

A template is a string literal containing `$name` or `${expr}`:

```kelp
print("area = ${c.area()}")
print("r = $c.r")            // $name names an identifier, nothing more
```

`$name` interpolates one identifier; `${...}` interpolates any expression, and the expression may contain a nested template or span several lines. A template is not a value: the only place it may appear is as the single argument of `print` or `print!`. To build a string with the same content, use `str.concat`, `fmt.*`, or a `String`.

### 2.6 Operators

```
+  -  *  /  %          arithmetic (and unary -)
==  !=  <  <=  >  >=   comparison
&&  ||  !              logic
=  +=  -=  *=  /=  %=  assignment and compound assignment
&  &mut  *             reference, exclusive reference, dereference
?                      propagate a failure (5.2)
.  ,  :  (  )  {  }    member access, separators
->                     `when` arms only
```

`[` and `]` are lexed but no expression uses them; they exist for capability grant lists.

## 3. Types

| Type | Kind | Notes |
| --- | --- | --- |
| `i32` `i64` `u8` `u16` `u32` `u64` `usize` `isize` | Copy | integers, all widths distinct |
| `f32` `f64` | Copy | IEEE floats |
| `bool` | Copy | `true` / `false` |
| `char` | Copy | declared, but no values exist in v1 |
| `str` | borrowed | NUL-terminated UTF-8 byte view, points into storage owned elsewhere |
| `String` | owned | UTF-8 text, validated on construction, freed at scope exit |
| `Bytes` | owned | raw byte buffer, freed at scope exit |
| `List<T>` | owned | growable sequence, one instantiation per element type |
| `Map<K, V>` | owned | keyed container, insertion order; `Map<V>` is a `String` key (12.2) |
| `Box<T>` | owned | one heap cell that holds a `T` or nothing |
| `datac` name | owned | record with per-field `val`/`var`, optionally with type parameters (9.3) |
| `enum` name | owned | tagged union: one variant is live, and patterns match on it (9.2), optionally with type parameters (9.3) |
| `fun(T, U).R: V` | Copy | a function value: a function pointer, with the parameters it takes and what it returns (7.4) |
| `closure(T, U).R: V` | owned | a *closure*: a function value plus the environment its captures were moved into (7.4). What a `fun` literal becomes when it names a binding from the enclosing scope |
| `Unit` | Copy | absent value, the implicit return type |

Rules that follow from the table:

- There is no implicit numeric conversion, not even between two integer widths. `u64 % 10` produces a `u64` and cannot be pushed into a `u8` slot without `to_u8()`.
- Owned types are not `Copy`: assigning one moves it, and every owned value is dropped at the end of the scope that owns it, including inside `datac` fields, `List` elements and `Map` values.
- `str` is a borrow, so it cannot live in a `datac` field, a `List` element, a `Map` value or a `Box`; only owned types can.
- No `List` or `Map` may hold another `List` or `Map` directly, because elements are stored inline. Put it in a `Box`.
- A user-declared `datac` or `enum` may take type parameters, and every use site writes them (`Pair<i32, String>`); see 9.3. Two instantiations of one template are different types.
- `Unit` is not a value and cannot be stored.
- A function type is Copy and owns nothing: a function value is a plain function pointer, so copying one is copying a pointer. It cannot be borrowed (`&`/`mut`), stored in a container, or used as a type argument's *element* — the placements 7.4 lists.
- A closure type is **not** Copy: it owns the environment its captures were moved into, and dropping it frees that environment. A parameter of the closure type borrows it (as an owned type's parameter does), and `move closure(...)` takes it over. It cannot be borrowed with `&`, cannot be a `List`/`Map` element, and *may* be a `Box` element (`Box<closure(...)>`, the optional-callback shape) or a `datac`/`enum` field.

## 4. Bindings and assignment

```kelp
val n = 10               // immutable binding, type inferred
var total: i64 = 0       // mutable binding, explicit type
total += n               // compound assignment: total = total + n
```

- `val` cannot be assigned after the declaration; `var` can.
- The type is inferred from the initializer, or written after `:`.
- A `datac` field is assigned only when the field is `var` in the `datac` **and** the path to it is mutable. Both conditions are separate errors.
- Compound assignment exists for `+ - * / %`, and desugars to a read, the operation, and a write, so it obeys the same mutability and borrow rules.
- Assigning to an owned binding moves the new value in and drops the old one.

## 5. Operators and expressions

Precedence, loosest first:

| Level | Operators | Associativity |
| --- | --- | --- |
| 1 | `\|\|` | left |
| 2 | `&&` | left |
| 3 | `==` `!=` | left |
| 4 | `<` `<=` `>` `>=` | left |
| 5 | `+` `-` | left |
| 6 | `*` `/` `%` | left |
| 7 | unary `-` `!` `&` `&mut` `*` | prefix |
| 8 | `.field`, `.method(...)`, `f(...)`, `expr?` | postfix |
| 9 | `=` `+=` `-=` `*=` `/=` `%=` | right |

```kelp
val m = (a + b) * 2
val ok = n > 0 && n < 10 || force
val neg = -n
val r = &c                       // shared borrow of a local
val e = &mut c                   // exclusive borrow of a local
val field = (*r).count           // dereference, then field access
```

Facts worth knowing:

- Integer `/` truncates toward zero and `%` follows the dividend's sign, as in C. Division by zero is undefined rather than checked.
- Integer `+`, `-` and `*` overflow is not checked.
- Float arithmetic is IEEE, so `nan`, `inf` and `-0.0` behave as usual. `math.is_nan` and `math.is_finite` exist.
- `==` and `!=` work for scalars. Comparing two `str` values is refused (it would compare addresses); use `str.eq`. Owned types have no structural equality.
- `if` and `when` are expressions (see 7), so they can appear on the right of `=` and inside a call.
- A block `{ ... }` is an expression whose value is its last expression when used where a value is expected.
- Every integer type has `to_<type>()` for every other integer type. Widening is exact; narrowing panics when the value does not fit, naming the call: `panic: i32.to_u64: value out of range`.
- `print` is the one builtin call that accepts a template; see 12.

#### The `?` operator

`expr?` evaluates `expr` and, if it carries a failure, returns that failure out of the enclosing function. Otherwise the expression's value is the success payload.

```kelp
fun quarter(n: i64).R: Result<i64, String> {
    val h = half(n = n)?          // `half` returns Result<i64, String>
    return Result.Ok(value = h / 2)
}

fun loud(n: i64).R: Option<String> {
    val w = maybe_word(n = n)?    // `maybe_word` returns Option<String>
    return Option.Some(value = String.from_str(s = w.as_str()))
}
```

- The operand must be an `enum` **held by value** and shaped either `Ok`/`Err` (the failure carries a payload) or `Some`/`None` (it does not). The shape is the contract: the enum's *name* does not matter, so `std/result`'s `Result`, `std/option`'s `Option` and a program's own declaration of either all work.
- The enclosing function must return an enum of the **same shape**, and for `Ok`/`Err` with the **same failure payload type** — v1 has no error conversion, so `Result<i64, String>` cannot be propagated out of a function returning `Result<i64, IoErr>`. A function returning `Unit` (including `main`) has nowhere to propagate to.
- The value of `expr?` is the success payload, so it composes: `1 + half(n = n)?`. A failure returns immediately, which means the enclosing function's locals are dropped on the way out, exactly as any `return` drops them.
- `?` is postfix and binds tighter than every binary operator. Because it moves a payload out of the operand, a borrow cannot be an operand: match that with `when` (6.3) or `if let` (6.1).

## 6. Control flow

### 6.1 `if` / `else`

```kelp
if n < 2 {
    return n
}

val label = if n > 0 { "pos" } else { "neg" }

if temp > 30 {
    print("hot")
} else if temp > 20 {
    print("warm")
} else {
    print("cool")
}
```

Braces are always required. Parentheses around the condition are neither needed nor the house style. An `if` used as a value must have an `else`; an `if` without an `else` must produce `Unit`, and the compiler says so: `if without an else branch must evaluate to 'unit'`.

#### `if let`

`if let` is the same conditional with a *pattern* instead of a boolean condition: it runs the branch when the variant matches, and binds the payload for that branch.

```kelp
if let Circle(r) = shape {
    print(r)
}

if let Word(text) = token {
    return text.len()
}

if let Rect(w, h) = shape {
    print(w * h)
} else if let Circle(r) = shape {
    print(3.14159 * r * r)
} else {
    print(0.0)
}
```

- It is exactly the pattern form of `when` (6.3) with a single arm and an `else`, which is why the arm rules are the same: the variant must exist, the number of bindings must match the payload, `_` ignores one, and a payload binds as a copy when it is `Copy` and as a shared borrow otherwise.
- The scrutinee may be **any expression**, temporary or place, because it is bound to a local before it is matched. (A bare `when` needs a *place*, since a payload borrow has to outlive the arm.)
- `else if let` chains, and a plain `else if`, are both allowed; without any `else` the whole expression is `Unit`, the same as an `if` without one.
- `let` is an ordinary identifier everywhere else in the language. It is read as the pattern form only directly after `if`, so a binding named `let` cannot be the condition of an `if` without parentheses.

### 6.2 `while`

`while` is the only loop. There is no `for`, `break` or `continue`.

```kelp
var i: usize = 0
while i < xs.len() {
    print!(xs.get(i))
    i += 1
}
```

Walking a `List` is therefore an index and a `while`, and `str.split`, `Map.keys()` and `Map.values()` hand back `List`s so that the same pattern works over text and over maps.

### 6.3 `when`

`when` is a chain of conditions that reads as a value:

```kelp
fun classify(n: i32).R: &str = when {
    n > 0 -> "pos"
    n < 0 -> "neg"
    else -> "zero"
}
```

Each arm is `condition -> value` and `->` appears nowhere else in the language. Arms are evaluated in order, `else` is required when a value is produced, and there is no exhaustiveness analysis beyond that. `when` desugars to nested `if`/`else if`/`else`.

With a scrutinee, `when` becomes pattern matching over an `enum`, and `->` is the same arrow:

```kelp
fun area(s: &Shape).R: f64 = when s {
    Circle(r) -> math.PI * r * r
    Rect(w, h) -> w * h
    Empty -> 0.0
}
```

- The scrutinee's type is an `enum`, and every arm head is one of its variants (9.2). An `else ->` arm matches every variant not named above it.
- Matching must be exhaustive: either every variant has an arm, or an `else` arm is present. A missing variant is a compile error that names it.
- There are no guards, no literal patterns and no nested patterns in v1; an arm head is a variant name with one binding per payload field, and `_` ignores a payload.
- `when` without a scrutinee is the boolean form above. The two forms cannot be mixed: `when` with a scrutinee never evaluates a condition.

### 6.4 `return`

In a block-bodied function, `return expr` delivers the value the signature promises:

```kelp
fun sum_to(n: i32).R: i32 {
    var acc = 0
    var i = 1
    while i <= n {
        acc += i
        i += 1
    }
    return acc
}
```

`return` with no value is allowed in a function with no `.R: T`. An expression-bodied function (see 7.1) does not use `return`.

A block whose **last statement is `panic(...)`** diverges, so it satisfies any declared return type:

```kelp
fun unwrap<T>(o: &Option<T>).R: T {
    if let Some(v) = o {
        return v
    }
    panic("Option.unwrap: value is None")
}
```

That is the whole of the divergence rule: it applies to a function body's tail and nowhere else, so a `panic` cannot be the arm of an `if`/`when` used as a value. It exists because it is what makes a "this cannot happen" accessor expressible at all (`std/option`'s `unwrap` is the first one).

## 7. Functions

### 7.1 Declaration

```kelp
fun add(a: i32, b: i32).R: i32 = a + b        // expression body, requires .R

fun greet(name: &str) {                       // no .R: returns Unit
    print(str.concat("hello, ", name))
}

fun abs_diff(a: i32, b: i32).R: i32 {         // block body
    if a > b {
        return a - b
    }
    return b - a
}
```

- The keyword is `fun`.
- The return type is written `.R: T` after the parameter list. `.R` marks "this returns"; there is no `->` return marker, no trailing type, and no inferred return type for block bodies.
- No `.R` means `Unit`.
- An expression body (`= expr`) requires `.R: T`; a block body may end with `return expr`.
- Parameters are `name: T`. A parameter is a shared borrow of its argument. `name: mut T` is an exclusive borrow, which is the only spelling for mutation in parameter position: `&mut T` there is a parse error. `name: move T` takes ownership: the call site moves the argument in, and the callee frees it when its scope ends (10.1). `mut` and `move` are mutually exclusive, `move` is refused on a `str` or a `&T`, and a receiver is always borrowed — there is no `fun move Type.name()`.
- Named arguments are allowed at any call site when there is more than one parameter, and are matched by name first, then positionally:

```kelp
val p = Circle(r = 2.0, x = 1.0)
val d = math.pow(base = 2.0, exp = 8.0)
print(str.concat("a", "b"))            // positional is fine too
```

A name that the function does not declare is an error; so is naming an argument to `print`, which takes one positional argument.

### 7.2 Overloading, recursion, closures

- A user program cannot overload a name. Two `fun pick(a: i32)` and `fun pick(a: f64)` collide: `function 'pick' is defined more than once for the same parameter types`. Overloading exists only inside the shipped standard library, and only for `abs`, `min`, `max` and `clamp`, which is how `math.min` accepts `i32`, `i64` and `f64`.
- Recursion is unrestricted, including mutual recursion.
- A function name *is* a value: see 7.4. A `fun` literal is an anonymous function, and a literal that names a binding from the enclosing scope is a **closure** — a function value that carries the environment it captured. There is still no way to write a named nested function, and no `dyn`-style boxing of one.
- A function may take type parameters: `fun first<T>(xs: List<T>).R: T`. A receiver method on a generic type takes the *type's* parameters instead: `fun Pair<A, B>.first().R: A`. Each call site is instantiated for the types it uses (9.3).

### 7.3 `main`

```kelp
fun main() { ... }
```

`main` must exist in the entry module, takes no parameters, returns `Unit`, and cannot be called. A program with no `main` fails with `no 'main' function found`.

### 7.4 Function values

A bare name of a declared `fun` is a value, and its type is written `fun(T, U).R: V` — the `fun` keyword, the parameter types, and the same `.R:` return clause a declaration uses. No `.R` means `Unit`.

```kelp
fun double(x: i32).R: i32 = x * 2

fun apply(f: fun(i32).R: i32, x: i32).R: i32 = f(x)     // takes a callback

val f = double                     // a function value
print(f(21))                       // ...called through the binding
print(apply(double, 5))            // ...and passed
print(apply(f, 6))
```

The parameter types of a function type follow the same rule a declaration's do, so a written type and a signature are the same type when they spell the same thing: `fun(String)` is a parameter that borrows, `fun(mut Counter)` one that borrows exclusively, `fun(move String)` one that takes ownership (7.1).

```kelp
fun consume(s: move String).R: usize = s.len()

val g: fun(move String).R: usize = consume
print(g(String.from_str("abcd")))
```

A function value can live anywhere a `Copy` value can, and is called with ordinary call syntax wherever it is held:

```kelp
datac Handlers(val on_i32: fun(i32).R: i32)

fun call_through(h: Handlers, x: i32).R: i32 = h.on_i32(x)   // a field, called

fun pick(which: bool).R: fun(i32).R: i32 {                   // returned
    if which {
        return double
    }
    return triple
}

val p = pick(true)
print(p(9))
```

An **anonymous function** is written where an expression is expected, with the same header a declaration has and no name. It is a function value like any other:

```kelp
val f = fun(x: i32).R: i32 = x * 2
print(f(21))
print(apply(fun(x: i32).R: i32 = x + 1, 5))

val g = fun(x: i32) {              // no `.R` means Unit
    print(x)
}
```

A body is required (`= expr` or `{ ... }`), and everything else is a declaration's: the same parameter list with the same `mut`/`move` qualifiers, the same optional `.R: T`, the same tail-versus-return check, and `?` propagates against the literal's own `.R`. Each literal becomes one function, so a literal inside a literal's body is just another one.

### Captures, and the closure type

A literal that names a binding from the enclosing scope **captures** it, and that makes the literal a *closure*: a function value plus the environment those captures were moved into. Its type is written `closure(T, U).R: V` — the function type under the `closure` keyword, called exactly the same way.

```kelp
fun make(step: i32).R: closure(i32).R: i32 {
    return fun(x: i32).R: i32 = x * step
}

val f = make(3)
print(f(7))                        // 21 — `step` outlives `make`
```

A capture is **move-only**, and `Copy` is what decides what that means:

- A `Copy` binding (a number, `bool`, a function value) is *copied* into the environment; the enclosing binding stays usable.
- An owned binding (`String`, `Bytes`, `List`, `Box`, `Map`, a `datac`/`enum`, another closure) is *moved* into the environment; the enclosing binding is gone from that point on, exactly as after `val b = a`.
- A borrow (`str`, `&T`) **cannot** be captured: the environment outlives the frame the borrow points into, and there is no lifetime syntax to say otherwise. Pass it as a parameter of the literal instead.

Inside the body a capture is read *through a borrow of the environment*, so an owned capture can be used in place — a method called on it, a field read, passed to a parameter — but cannot be handed out by value. That is 9's no-partial-moves rule, and the message says so.

A closure **owns** its environment and frees it where the closure dies: the end of the block that holds it, an early `return`, one per loop iteration. So a closure may outlive the frame that made it (that is the point), and nothing it captured may be a borrow.

```kelp
val k = 10
val add_k = fun(x: i32).R: i32 = x + k     // Copy capture: `k` is still usable
print(add_k(1))
print(k)
```

A literal whose body captures nothing is *not* a closure: it stays a plain `fun` value — `Copy`, no environment, no drop — so nothing about the phase before this one changes. The two types are separate, and one widening connects them:

- A plain **function value used where a closure is expected** (`val f: closure(i32).R: i32 = double`, `Holder(cb = fun(x: i32).R: i32 = x)`, `list.map(xs, inc)`) is wrapped in a one-field environment holding the pointer, with a generated thunk calling through it. When the value *names* a function — a declared `fun`, or a literal that captured nothing — the wrapper needs no environment at all, so the compiler generates a shim with the ignored hidden parameter and the conversion allocates nothing; only a function value whose identity is known at the call (`val g = pick(true)` then `list.map(xs, g)`) pays for the heap wrapper. The signatures must match, and there is no conversion in the other direction, because a closure carries an environment a `fun` value has no room for.

What a literal still cannot do:

- **Read a passdown.** A passdown is a hidden argument of the function that names it, and a closure's generated function has no room for one. Pass the value as a parameter, or capture it.
- **Return a borrow.** The elision rule that says which argument a borrow comes from has nowhere to be written for a value called through a pointer.
- **Capture a field of the receiver it was written in.** Reading a field out of a borrowed receiver by value is a partial move (9): bind a `Copy` field to a local first, or take the whole receiver as a `move` parameter.
- **Be a `List` or `Map` element.** A closure owns its environment and those elements are held inline; a `Box<closure(...)>` (the optional-callback shape) or a one-field `datac` holds it instead.
- **Be borrowed with `&`.** A closure is an owned value, so a parameter of its type already borrows it and `move closure(...)` takes it over.

A literal nested in another literal can copy a `Copy` capture out of the enclosing closure's environment. An owned one cannot be *moved* out of it — that is the same partial move.

Rules and limits of this phase:

- **A call through a function value is positional.** A function *type* has no parameter names, so `f(x = 1)` is refused and the argument count is checked against the callee's own list.
- **A call's result cannot be called directly.** `pick(true)(4)` is not the grammar; bind it (`val p = pick(true)`) and call `p(4)`.
- **A module's item is named with its qualified spelling** — `val f = math.sqrt` — and a bare name means a function of the current module. An *overloaded* item (`math.abs`, which is `i32`/`i64`/`f64`) is refused: its parameter types are part of its name, so it is several functions, not one.
- **A declared `fun` is always a plain function value.** A generic function (no single signature), a function that reads a passdown (its C signature has a hidden argument), and `main` are each refused with a message that says which. A receiver method is not reachable as a value at all, and an overloaded standard-library name (`math.abs`) is refused because its parameter types are part of its name.
- **Placements with no C spelling are refused**: a function type may not return a function type and may not be borrowed. A function type *is* fine as a parameter, a `val`/`var`, a `datac`/`enum` field, a passdown, a function's return type, and a type argument. A closure type is fine in all of those as well, plus `Box<closure(...)>`.
- There is no `print` for a function value or a closure.

## 8. Methods and receivers

A method is a function whose first parameter is a receiver, written as `Type.name`:

```kelp
datac Counter(val id: i32, var count: i32)

fun Counter.total().R: i32 = id + count          // shared receiver
fun mut Counter.bump() {                          // exclusive receiver
    count += 1
}
```

Calling is ordinary postfix syntax:

```kelp
var c = Counter(id = 1, count = 0)
print(c.total())
c.bump()
print(c.total())
```

- Inside the method, fields are read and written by their bare names. `self` also works as the whole receiver, for the rare method that needs to hand the value on.
- `fun Type.name()` takes a shared borrow of the receiver, so it may be called on a `val` binding.
- `fun mut Type.name()` takes an exclusive borrow, so the receiver must be reachable mutably: a `var` binding, a `mut` parameter, or a field of one. Calling it on a `val` binding fails with `cannot borrow 'c' as exclusive` (see 19).
- Passing a receiver to a method does not move it.
- Builtin types have methods too, listed in 12 and 13 (`String.push_str`, `List.push`, `n.to_u64`, and so on).

## 9. `datac` and `enum`

### 9.1 `datac`

A `datac` is a record: every field exists, in every value.

```kelp
datac Point(val x: i32, val y: i32)
datac Counter(val id: i32, var count: i32)
datac Node(val value: i32, val next: Box<Node>)
```

- Fields are declared `val` (immutable) or `var` (mutable), one field per comma-separated entry.
- A `datac` holds owned data only. A `str` field, a reference field, or a `Unit` field is illegal, because the `datac` owns its fields and must be able to free them.
- A field of another `datac` type is stored inline, so that type must be declared earlier in the same file. A self-referencing or forward-referencing field must be behind a `Box` or a `List`:

```kelp
datac List2(val head: i32, val tail: Box<List2>)   // recursive, finite size
datac Tree(val kids: List<Tree>)                   // recursive, finite size
datac Bad(val next: Bad)                           // error: infinite size
```

Construction is a call with the `datac` name, and field access is `.`:

```kelp
val p = Point(x = 3, y = 4)
print(p.x)

var c = Counter(id = 1, count = 0)
c.count = 5              // ok: field is var and c is a var binding
```

Reading an owned field *by value* is refused, because the language has no partial moves: `let s = node.text` would produce two owners of the same buffer. Borrow it instead (pass it to a parameter, call a method on it), or store it in a `Box` and `take()` it out.

`datac` values are dropped at the end of the owning scope, fields recursively, in the order that frees what each field points at.

### 9.2 `enum`

An `enum` is a tagged union: exactly one variant is live in a value.

```kelp
enum Shape {
    Circle(val r: f64)
    Rect(val w: f64, val h: f64)
    Empty
}
```

- Variants are newline-separated, and each is a name, optionally followed by `val`/`var` payload fields in parentheses.
- A payload field obeys every `datac` field rule: owned data only, and a field of another type is stored inline, so that type is declared earlier in the same file. A self-referencing or forward-referencing payload must be behind a `Box` or a `List`.
- An `enum` value is owned and not `Copy`, like a `datac`. Assigning or passing one moves it, and dropping it drops the live variant's payloads.
- Construction names the variant, qualified by the `enum`: `Shape.Circle(r = 1.0)`, `Shape.Rect(w = 2.0, h = 3.0)`, `Shape.Empty`. Arguments may be named or positional, as in a `datac` construction.
- Matching reads the live variant (6.3):

```kelp
fun area(s: &Shape).R: f64 = when s {
    Circle(r) -> 3.14159 * r * r
    Rect(w, h) -> w * h
    Empty -> 0.0
}

fun label(s: &Shape).R: &str = when s {
    Circle(_) -> "circle"
    Rect(_, _) -> "rect"
    Empty -> "empty"
}
```

- A payload binds as a copy when it is `Copy` and as a shared borrow otherwise, so matching never partially moves the scrutinee: `r` above is an `f64`, and a `Word(val text: String)` payload would bind as `&String`.
- The scrutinee must be a place (a binding, a field path, or a dereference) — not a call's result — because a payload that is not `Copy` binds as a borrow of it and that borrow has to outlive the arm's body. Bind the value to a `val` first when you have an expression.
- The number of bindings must equal the number of payload fields, and `_` ignores one.
- Matching is exhaustive over the variants, or covered by an `else` arm (6.3).

### 9.3 Generic types

A `datac` or an `enum` may declare type parameters, and each use site instantiates them into a concrete type:

```kelp
datac Pair<A, B>(val first: A, val second: B)

enum Option<T> {
    Some(val value: T)
    None
}

val p: Pair<i32, String> = Pair(first = 1, second = String.from_str(s = "one"))
val n: Option<i32> = Option.Some(value = 7)
```

- The parameter list is `<` name (`,` name)* `>` between the declared name and the declaration's own `(` or `{`. A parameter is in scope in every field or payload type of that declaration, and nowhere else.
- A parameter name must not be a builtin type, and a generic type must not share its name with a non-generic one.
- A generic type is written with its arguments wherever a type is written: `Pair<i32, String>`, `Option<i32>`, `List<Option<i32>>`, `Box<Pair<i32, i32>>`. The argument count is checked, and each argument must be a concrete type — there is no way to write a parameter at a use site, because generic functions do not exist yet.
- Two instantiations of one template are two different types: `Pair<i32, i32>` cannot be passed where `Pair<String, String>` is expected, and each gets its own generated code.
- A type argument may not be `str` or a reference, because that would make a field (or a payload) borrowed, which 9.1 already forbids.
- A generic type is a template, not a type: writing `Pair` without arguments is an error naming the arguments it needs.
- A generic type may refer to itself through a `Box` or a `List`, exactly as a non-generic one may.

**Type arguments are inferred, not written, at a construction.** The arguments of the construction determine the parameters, and so does the expected type:

```kelp
val p = Pair(first = 1, second = "one")     // A = i32, B = str is illegal —
                                            // use String for stored text
val q = Pair(first = 1, second = String.from_str(s = "one"))   // A = i32, B = String

val a: Option<i32> = Option.Some(value = 7) // T from the payload
val b: Option<i32> = Option.None            // T from the expected type — the only
                                            // thing that determines it here
```

- The expected type is consulted first, then each argument. A parameter that neither determines is an error that names it, with the fix (`val x: Option<i32> = ...`). It also means an integer literal takes its type from the expected type or from the other argument, not from the default (`Option<i32>`).
#### Generic methods

A receiver method on a generic type writes the type's own parameters, in order, so the body can name them:

```kelp
datac Pair<A, B>(val first: A, var second: B)

fun Pair<A, B>.first_of().R: A = first
fun mut Pair<A, B>.replace_second(v: B) { second = v }
```

- The parameters must be exactly the type's, in order — a rename would make the body's `T` mean something the declaration does not say, and is refused with the spelling to write.
- The type arguments are the receiver's own instantiation, so `p.first_of()` on a `Pair<i32, String>` is instantiated once for `A = i32, B = String`. There is no inference to do, and no way to call a method on the template itself.
- A method on an `enum` is still refused: an `enum` carries no methods in v1. Write a free function over the instantiation instead (`fun is_some<T>(o: &Option<T>).R: bool`).
- A method's declared parameters are its own declared list; a receiver method cannot add type parameters of its own beyond the type's.

#### Generic functions

A `fun` may take type parameters after its name, and each call site instantiates them:

```kelp
fun identity<T>(x: T).R: T = x
fun repeat<T>(x: T, n: i32).R: T = if n <= 0 { x } else { repeat(x = x, n = n - 1) }
fun empty_of<T>().R: List<T> = List.empty()

identity(x = 5)
identity(x = "text")
repeat(x = 3, n = 2)
var xs: List<String> = empty_of()
```

- A parameter or a return type that names `T` is resolved for each instantiation, so `identity` above is checked and compiled once for `i32` and once for `str`.
- The arguments are inferred from the argument types, and the return type from the expected type — that is the only thing `empty_of()` has to go on. A parameter neither determines is an error that names it.
- The body is checked once per instantiation, because `T` is a concrete type inside it. A generic function that is never called is never checked.
- `main` may not take type parameters.
- A parameter of a non-`Copy` type is a shared borrow (10.2), exactly as it is in a non-generic function, so `fun first<T>(xs: List<T>).R: T` cannot return an owned element — `xs.get(i = 0)` borrows it. Only a `Copy` type can be returned from a borrow.

#### Explicit type arguments

Inference covers the ordinary cases, and when it cannot, the instantiation is written at the call site:

```kelp
val p = Pair<i32, String>(first = 1, second = String.from_str(s = "one"))
val some = Option.Some<i64>(value = 7)
print(identity<i32>(x = 5))
```

- The form is `Name<...>(` — the `<` written **against** the name and the `(` written **against** the `>`. Write a space before `<` when you mean a comparison: `a < b > (c)` is a comparison chain, `a<b>(c)` is read as an instantiation and then fails on `b` not being a type. This is the one place the two spellings could collide, and adjacency is what separates them.
- An explicit instantiation with the wrong number of arguments is an error naming the count, and so is one written on something that is not generic — a non-generic type, a non-generic function, or a method (whose parameters come from its receiver).
- A payload-less variant cannot take one, because there is no call to attach it to: write the expected type instead (`val none: Option<i32> = Option.None`).

## 10. Ownership, moves and borrowing

### 10.1 Moves

An owned value has exactly one owner. Rebinding, reassigning or returning one moves it:

```kelp
val a = String.from_str("x")
val b = a
print(b)        // ok
print(a)        // error: use of moved value 'a'
```

Passing an owned value to a parameter is **not** a move: parameters borrow (10.2), so this is fine:

```kelp
fun greet(s: String) { print(s) }

val name = String.from_str("bob")
greet(name)
print(name)               // still owned here
```

`move T` is the parameter form that *does* take the value (A-4). The call site moves the argument in, exactly as `val b = a` does, and the callee owns it from then on: it may move it again, return it, or drop it at the end of its scope.

```kelp
fun consume(s: move String).R: usize = s.len()

val a = String.from_str("bob")
print(consume(a))
print(a)                  // error: use of moved value 'a'
```

A temporary is already an owner, so `consume(String.from_str("bob"))` needs no binding. A *borrow* cannot be moved out of — `fun forward(s: String).R: usize = consume(s)` is a compile error, because `s` belongs to the caller — and an owned field cannot be moved out of a borrowed structure either (9.1). The way out of a value that is behind a borrow is `Box.take()`.

An assignment to an existing `var` moves the new value in and frees the old value, which is how a loop can rebuild a recursive structure:

```kelp
var list = Node(value = 1, next = Box.empty())
list = Node(value = 2, next = Box.of(list))
```

### 10.2 Borrowing

```kelp
val r = &c            // shared borrow
var w = &mut c        // exclusive borrow
print((*r).count)     // dereference, then read a field
(*w).count = 9        // dereference, then write a field
```

Rules the checker enforces:

- A borrow lives until the end of its scope, not until its last use. Two borrows in the same block overlap, so a shared borrow must be in an inner block before an exclusive one is taken.
- While a value is borrowed exclusively, nothing else may borrow it and the owner may not be assigned or moved.
- While a value is borrowed shared, it cannot be borrowed exclusively, and it cannot be assigned or moved.
- Dereferencing a `&mut` borrow gives write access; dereferencing a `&` does not.
- A function whose `.R: T` is a borrow must return something derived from its first reference parameter. There is no lifetime syntax to say otherwise: `fun f(a: &str, b: &str).R: &str` may only return a view of `a`.
- A borrow cannot be stored: no reference fields in a `datac`, no references as `List` elements or `Map` values.
- A call that returns a borrow of its receiver needs the receiver to be a variable or a field, not a temporary:

```kelp
val text = String.from_str("hi")
val view = text.as_str()          // ok
print(str.len(text.as_str()))     // ok: the temporary lives for the call
val bad = String.from_str("hi").as_str()   // error: borrow of a temporary
```

### 10.3 Drops

Owned values are freed at the end of the scope that owns them, on every exit path including `return` and either branch of an `if`. A `datac` drops its fields, a `List` drops its elements and then its buffer, and a `Box` drops what it holds if it holds anything. An owned `pd` is freed once at process exit. There is no `free`, no `deinit`, and no way to leak deliberately.

## 11. Text, printing and conversion

### 11.1 Printing

`print` and `print!` are builtins; no import is needed.

```kelp
print("done")            // "done" and a newline
print!("loading")        // no newline
print(42)                // integers
print(1.5)               // floats, six decimals: 1.500000
print(true)              // bools
print(some_str)          // a str
print(owned_string)      // a String, read without being consumed
print(owned_bytes)       // a Bytes buffer
print(c.area())          // the result of a call
```

`print` takes exactly one positional argument. It accepts scalars, `str`, `String`, `Bytes` and a template. It does not accept a `datac`, a `List`, a `Map` or a `Box`, and it takes no named arguments. Integers print in decimal, floats print with six decimals (`print(1.5)` prints `1.500000`), and `bool` prints `true` or `false`. Formatting with a width or a chosen number of decimals goes through `fmt` (11.4).

`std/io` adds `print_bare`, `eprint` and `eprint_bare` for the same job with control over the newline and the stream, plus `File` writes for anything else.

### 11.2 `str`, `String` and `Bytes`

`str` is a borrowed view with a NUL terminator: string literals, slices of a `String`, and the return of `str.substring`. `String` is an owned UTF-8 buffer that must be built (11.3). `Bytes` is an owned raw buffer.

A `String` never converts to `str` implicitly. Write `.as_str()`:

```kelp
val s = String.from_str("hello")
print(str.len(s.as_str()))          // ok
print(str.len(s))                   // error: expected 'str', found 'String'
val b = s.as_str()                  // b borrows s and cannot outlive it
```

`Bytes` and `String` are validated on the way in: `String.from_bytes` and `Bytes.as_str` panic if the bytes are not valid UTF-8. `Bytes` is the type for binary data, and `fs.read`/`fs.write` use it so a file never goes through UTF-8 validation.

### 11.3 Building text

```kelp
var out = String.empty()
out.push_str("wrote ")
out.push_str(size_text.as_str())
print(out)
```

`String` methods: `String.empty()`, `String.with_capacity(n)`, `String.from_str(&str)`, `String.from_bytes(Bytes)`, `push(u8)`, `push_str(&str)`, `reserve(n)`, `clear()`, `len()`, `char_count()`, `is_empty()`, `clone()`, `into_bytes()`, `as_str()`.

`Bytes` methods: `Bytes.empty()`, `Bytes.from_str(&str)`, `push(u8)`, `append(&Bytes)`, `reserve(n)`, `clear()`, `len()`, `is_empty()`, `get(i)`, `set(i, b)`, `clone()`, `as_str()`.

Growing a buffer takes an exclusive borrow, so a live `str` view of it blocks the growth. That rule is what makes reallocation safe without lifetime syntax.

### 11.4 Conversions and formatting

```kelp
val n: i32 = -7
print(n.to_i64())            // widening, exact
print(n.to_u64())            // panics: i32.to_u64: value out of range

print(fmt.int(n.to_i64()))          // "-7" as a String
print(fmt.uint(1024))
print(fmt.float(1.5, 2))            // two decimals
print(fmt.hex(255))
print(fmt.pad_left("7", 3, 48))     // fill byte 48 is '0'

val parsed = str.parse_i64("41")
if parsed.ok {
    print(parsed.value + 1)
}
```

- `to_<T>()` exists on every integer type for every integer type. Widening is exact; narrowing panics when the value does not fit.
- Integer conversion is the only way to change width. There is no implicit promotion, so `u8` to `usize` for an index needs `n.to_usize()`.
- `str.from_i64`, `str.from_u64`, `str.from_f64(x, decimals)` return `String`; `fmt.int`, `fmt.uint`, `fmt.float`, `fmt.hex`, `fmt.bytes`, `fmt.pad_left`, `fmt.pad_right` wrap them for printing.
- `str.parse_i64`, `str.parse_u64`, `str.parse_f64` return a `datac` with `ok` and `value`, because there is no `Option` yet.

## 12. Collections

### 12.1 `List<T>`

```kelp
var xs: List<i32> = List.empty()      // element type from the annotation
var ys = List.of(String.from_str("a"))   // element type from the argument

xs.push(3)
xs.push(1)
print(xs.len())
print(xs.get(0))
xs.set(0, 30)
xs.sort()
print(xs.contains(30))
val first_two = xs.slice(0, 2)
while !xs.is_empty() {
    print(xs.pop())
}
```

| Method | Takes | Returns |
| --- | --- | --- |
| `List.empty()` | nothing | empty list, element type from the context |
| `List.of(x)` | one value | list holding that value |
| `len()` / `is_empty()` | shared | `usize` / `bool` |
| `get(i)` | shared, `usize` | the element, or a borrow of it |
| `set(i, v)` | exclusive | `Unit` |
| `push(v)` | exclusive | `Unit`, moves the element in |
| `pop()` | exclusive | the last element, moved out |
| `insert(i, v)` / `remove(i)` | exclusive | `Unit` / the removed element |
| `clear()` | exclusive | `Unit` |
| `sort()` | exclusive | `Unit`, for sortable elements |
| `reverse()` | exclusive | `Unit` |
| `contains(v)` | shared | `bool`, for comparable elements |
| `slice(from, to)` | shared | a copy of the range, for clonable elements |

`get` and `pop` move an owned element out and return a copy for a `Copy` element, so `List<i32>.get(i)` is an `i32` while `List<String>.get(i)` is a `&String`. Out-of-range indices and popping an empty list panic.

The element type may be a scalar, `String`, `Bytes` or a `datac`. It may not be a borrow, a `str`, `Unit`, a `List` or a `Map`; put a container in a `Box` instead. `sort`, `contains`, `slice` and `Map.values` are offered only for element types that support them.

The element-wise methods above are builtin. The callback-shaped operations
(`map`, `filter`, `each`, `find_index`, `any`, `all`) live in `std/list`,
because a callback's result type cannot be generated per element type: see
16.15.

### 12.2 `Map<K, V>`

A keyed container: keys of type `K`, values of type `V`. Entries keep insertion order, which is why `keys()` and `values()` are lists and not hash order.

```kelp
var counts: Map<String, i64> = Map.new()
counts.insert(key = "one", v = 1)
counts.insert(key = "two", v = 2)
print(counts.has(key = "two"))
print(counts.get(key = "two"))
print(counts.get_or(key = "nope", fallback = 0))
counts.insert(key = "two", v = 22)
print(counts.remove(key = "one"))
print(counts.keys().len())

var runs: Map<i32, i64> = Map.new()        // a scalar key
runs.insert(key = 7, v = 3)
print(runs.get(key = 7))
```

- **`Map<V>` written with one argument means `Map<String, V>`.** That is the shorthand every program written before keys were generic uses, and the two spellings are the same type.
- **A key must be `String`, an integer, or `bool`.** The compiler generates the key's hash and its equality from whatever `K` is, so the set is closed: a float is refused because `nan != nan` would make an inserted key unfindable, and a `datac`, container or borrow is refused because v1 has no equality for them — a key that needs a user-supplied comparison is a trait or a comparator function, and v1 has neither (17).
- `Map<K, V>` is neither a list element nor a map value, so `Map<String, Map<i32>>` stays impossible, like `List<List<i32>>`.

| Method | Takes | Returns |
| --- | --- | --- |
| `Map.new()` / `Map.empty()` | nothing | empty map, key and value types from the binding |
| `len()` / `is_empty()` | shared | `usize` / `bool` |
| `has(key)` | shared | `bool` |
| `get(key)` | shared | the value, or a borrow of it; panics when absent |
| `get_or(key, fallback)` | shared | the value or the fallback, for clonable values |
| `insert(key, v)` | exclusive | `Unit`, copies a `String` key in and moves the value in |
| `remove(key)` | exclusive | the value, moved out; panics when absent |
| `clear()` | exclusive | `Unit` |
| `keys()` / `values()` | shared | `List<K>` / `List<V>` |

`Map.of` does not exist: a map starts empty, so `Map.new()` needs the type on the binding.

### 12.3 `Box<T>`

A box holds one value or nothing. It is the owning pointer, and it is what makes recursive types and moves out of a borrowed structure expressible.

```kelp
var slot: Box<i32> = Box.empty()
print(slot.is_empty())
slot.set(5)
print(slot.get())
print(slot.take())          // moved out, box is empty again

var list = Node(value = 1, next = Box.empty())
list = Node(value = 2, next = Box.of(list))
```

`Box` methods: `Box.empty()`, `Box.of(v)`, `is_empty()`, `get()` (a copy for a `Copy` element, otherwise a borrow), `set(v)`, `take()` (moves the value out and leaves the box empty). `take` on an empty box panics.

## 13. Modules and imports

### 13.1 `impl`

Every file is a module, and a module starts by importing what it uses:

```kelp
impl "shapes/circle"
impl "std/fs" with caps(fs = ["/var/log/myapp/*"])
impl "std/str"
impl "std/time" as t
```

- The path is a string, relative to `src/` for project modules and to the shipped standard library for `std/...`. The `.kp` extension is not written.
- `impl` lines come first in the file.
- The namespace is the last path segment: `impl "shapes/circle"` gives `circle.area(...)`, and `impl "std/fs"` gives `fs.read(...)`. `as name` overrides it.
- Imports are transitive: a file that imports a module also loads what that module imports.
- Import cycles and missing modules are errors, and the missing-module message names the file it expected.
- Only the entry module has unprefixed names. Every other module is reached through its namespace.

### 13.2 Canonical file order

```kelp
impl "std/fs"          // 1. imports
alias %*debug*% = true // 2. aliases, anywhere a statement can start
const LIMIT: i32 = 10  // 3. constants and passdowns, in any mix
pd val app = "demo"
datac Point(val x: i32, val y: i32)   // 4. types and functions, in any mix
fun main() { ... }
```

An `impl` after a declaration is a parse error, and so is a `const` or `pd` after a `datac` or a `fun`.

### 13.3 Import paths and aliases

Alias substitution is textual and runs before parsing, so it can rewrite an import path:

```kelp
alias %*db*% = "storage/postgres"
impl %*db*%
```

Aliases come from two places: an inline `alias %*name*% = <raw text to end of line>` declaration, and `alias.kpconf` at the project root, one `name="replacement text"` per line. Substitution has no parameters, no scoping and no hygiene; `%*a*%` defined as `%*b*% * 2` with `%*b*%` defined as `40 + 2` prints `84`, because precedence is whatever the expanded text happens to produce.

## 14. Constants and passdowns

### 14.1 `const`

```kelp
const LIMIT: i32 = 10
const NAME: str = "x"
const PI2: f64 = math.PI * 2.0
```

A `const` is a top-level compile-time value. The type is mandatory, the initializer must be foldable at compile time, and each use is substituted as a literal, so there is no storage and no initialization at startup. A `const` may only name constants declared before it.

### 14.2 `pd` passdowns

A passdown is a top-level binding that every function in the module can name without it being passed:

```kelp
pd val app_name = "shapes"
pd var requests = 0

fun log_area(c: Circle) {
    requests += 1
    print("[$app_name #$requests] area = ${c.area()}")
}
```

- `pd val name = expr`, `pd var name = expr`, or with an explicit type: `pd val name: T = expr`.
- A function that mentions a passdown gets it wired in as a hidden parameter. A function that never mentions one pays nothing.
- The reference is a borrow: `pd var` is an exclusive borrow and therefore writable from inside functions, while `pd val` is shared. Two functions cannot hold the same `pd var` exclusively at once, and all the ordinary borrow rules apply.
- A passdown is module-scoped. A local `val`/`var` of the same name shadows it inside that scope, and there is no spelling for reaching another module's passdown.
- An owned passdown is freed once, at process exit.

## 15. Capabilities

A module reaches the outside world only through a capability. The key is that the grant is checked at compile time, so an ungranted call does not resolve at all.

### 15.1 Domains

| Domain | Gated modules | Patterns name |
| --- | --- | --- |
| `fs` | `std/fs` | absolute paths |
| `net` | `std/net` | hosts and `host:port` |
| `process` | `std/process` | absolute program paths |
| `env` | `std/env` | environment variable names |
| `time` | `std/time` | nothing, the domain is the whole grant |
| `rand` | `std/rand` | nothing |

`std/mem`, `std/str`, `std/path`, `std/math`, `std/fmt` and `std/io` are ungated: they cannot touch anything shared. `std/http` is ungated too, and holds `net`, which its importer grants it.

### 15.2 Grants

```kelp
impl "std/fs"
impl "std/http" with caps(net = ["api.example.com:443", "127.0.0.1:*"])
impl "logger" with caps(fs = ["/var/log/myapp/**"], env = ["LOG_*"])
```

- The entry module holds the wildcard for every domain (`/**` for path domains, `*` for name domains), so a single-file program needs no grant for its own calls.
- Every other module holds exactly what its importers grant it, and nothing else.
- A module can delegate less than it holds, never more. Granting a pattern the importer does not hold is an error.
- Referencing a gated function without holding the domain is a compile error naming the missing domain and the grant to write.
- `net` patterns are `host`, `host:port`, `*.host`, `*:port` or `*`, matched as names, never as resolved addresses. `127.0.0.1:*` does not cover `localhost`.
- `fs` and `process` patterns are absolute paths, normalized lexically and resolved through symlinks before matching, so a symlink inside the grant that points outside it is denied.
- An empty pattern list denies everything: `with caps(process = [])` is a grant that permits no program.

### 15.3 Runtime checks

Each gated call site checks its resource arguments against the calling module's compiled-in allowlist:

```kelp
fs.read("/etc/passwd")     // denied unless the module holds a pattern matching it
process.run("/bin/sh", args)
net.connect("api.example.com", 443)
```

A denial panics with the call site, the attempted resource and the granted patterns:

```
demo.kp:12:10: panic: '/etc/passwd' is outside the fs capability granted to module 'demo'
  granted:
    /var/log/myapp/**
```

A relative path is not covered by a path pattern, because patterns are absolute: pass an absolute path, or build one with `path.join`. `time`, `rand` and the ungated modules have no resource argument to check.

## 16. Standard library

Sixteen modules ship inside the compiler binary. Import them with `impl "std/<name>"`; the namespace is the module name.

| Module | Domain | Contents |
| --- | --- | --- |
| `std/mem` | none | `alloc(n)` |
| `std/str` | none | text measurement, search, slicing, building, numbers |
| `std/path` | none | path splitting, joining, normalization |
| `std/math` | none | floats, `abs`, `min`, `max`, `clamp`, `PI`/`E`/`TAU` |
| `std/fmt` | none | number and byte formatting, padding |
| `std/io` | none | `File`, `BufReader`, stdio, `print`/`eprint` variants |
| `std/fs` | `fs` | files, directories, metadata, temp files |
| `std/time` | `time` | wall clock, monotonic clock, sleep |
| `std/rand` | `rand` | `u64`, bounded values, random bytes |
| `std/env` | `env` | read and write environment variables |
| `std/process` | `process` | argv, spawn, wait, kill, exit |
| `std/net` | `net` | TCP sockets, listener, readiness, DNS |
| `std/http` | none (holds `net`) | HTTP client, request and response types, connection pool |
| `std/list` | none | the callback-shaped operations on `List<T>` (`map`, `filter`, `each`, …) |
| `std/option` | none | `Option<T>` and its helpers |
| `std/result` | none | `Result<T, E>` and its helpers |

### 16.1 `std/str`

```kelp
fun len(s: &str).R: usize
fun byte(s: &str, i: usize).R: u8
fun byte_index(s: &str, i: usize).R: usize
fun is_empty(s: &str).R: bool
fun char_count(s: &str).R: usize
fun char_at(s: &str, i: usize).R: u32
fun substring(s: &str, from: usize).R: &str
fun index_of(s: &str, needle: &str).R: Option<usize>   // None when absent
fun starts_with(s: &str, prefix: &str).R: bool
fun starts_with_at(s: &str, needle: &str, at: usize).R: bool
fun ends_with(s: &str, suffix: &str).R: bool
fun contains(s: &str, needle: &str).R: bool
fun eq(a: &str, b: &str).R: bool
fun is_space(c: u8).R: bool
fun trim(s: &str).R: String
fun trim_start(s: &str).R: &str
fun trim_end(s: &str).R: String
fun prefix(s: &str, n: usize).R: String
fun copy_range(s: &str, from: usize, to: usize).R: String
fun to_upper(s: &str).R: String
fun to_lower(s: &str).R: String
fun repeat(s: &str, n: usize).R: String
fun replace(s: &str, from: &str, to: &str).R: String
fun split(s: &str, sep: &str).R: List<String>
fun join(parts: List<String>, sep: &str).R: String
fun concat(a: &str, b: &str).R: String
fun from_i64(n: i64).R: String
fun from_u64(n: u64).R: String
fun from_f64(x: f64, decimals: i32).R: String
fun parse_i64(s: &str).R: Result<i64, ParseErr>   // enum ParseErr { Empty Invalid }
fun parse_u64(s: &str).R: Result<u64, ParseErr>
fun parse_f64(s: &str).R: Result<f64, ParseErr>
```

`str.len` and `str.byte` are byte-oriented; `str.char_count` and `str.char_at` are the codepoint-oriented pair. `str.substring` returns a view into the original, so it borrows its argument.

### 16.2 `std/path`

```kelp
fun is_absolute(s: &str).R: bool
fun normalize(s: &str).R: String
fun file_name(s: &str).R: &str
fun extension(s: &str).R: &str
fun stem(s: &str).R: String
fun has_extension(s: &str, ext: &str).R: bool
fun split(s: &str).R: List<String>
fun join(a: &str, b: &str).R: String
fun join_all(parts: List<String>).R: String
fun parent(s: &str).R: String
```

### 16.3 `std/math`

```kelp
const PI: f64   const E: f64   const TAU: f64

fun sqrt(x: f64).R: f64        fun pow(base: f64, exp: f64).R: f64
fun floor(x: f64).R: f64       fun ceil(x: f64).R: f64
fun round(x: f64).R: f64       fun trunc(x: f64).R: f64
fun exp(x: f64).R: f64         fun ln(x: f64).R: f64
fun log2(x: f64).R: f64        fun log10(x: f64).R: f64
fun sin(x: f64).R: f64         fun cos(x: f64).R: f64
fun tan(x: f64).R: f64         fun atan2(y: f64, x: f64).R: f64
fun is_nan(x: f64).R: bool     fun is_finite(x: f64).R: bool
fun sign(x: f64).R: f64

fun abs(x: i32).R: i32         fun abs(x: i64).R: i64        fun abs(x: f64).R: f64
fun min(a: i32, b: i32).R: i32 fun min(a: i64, b: i64).R: i64 fun min(a: f64, b: f64).R: f64
fun max(a: i32, b: i32).R: i32 fun max(a: i64, b: i64).R: i64 fun max(a: f64, b: f64).R: f64
fun clamp(x: i32, lo: i32, hi: i32).R: i32   // and the i64 and f64 forms
```

### 16.4 `std/fmt`

```kelp
fun int(n: i64).R: String
fun uint(n: u64).R: String
fun float(x: f64, decimals: i32).R: String
fun hex(n: u64).R: String
fun bytes(b: Bytes).R: String
fun pad_left(s: &str, width: usize, fill: u8).R: String
fun pad_right(s: &str, width: usize, fill: u8).R: String
```

### 16.5 `std/mem`

```kelp
fun alloc(n: usize).R: Bytes      // zero-filled buffer of n bytes; panics on failure
```

### 16.6 `std/io` (ungated)

```kelp
datac File(val fd: i32)
fun File.is_open().R: bool
fun File.read(max: usize).R: Bytes
fun File.read_into(buf: mut Bytes, at: usize).R: usize
fun File.read_all().R: Bytes
fun File.write(data: Bytes).R: usize
fun File.write_text(text: String).R: usize
fun File.write_str(text: &str).R: usize
fun File.flush()
fun File.close()
fun mut File.set_nonblocking(on: bool)
fun File.wait_readable(timeout_ms: u64).R: bool
fun File.wait_writable(timeout_ms: u64).R: bool

fun stdin().R: File      fun stdout().R: File      fun stderr().R: File

datac BufReader(val file: File, var buf: Bytes, var start: usize, var end: usize)
fun buf_reader(f: File).R: BufReader
fun mut BufReader.read_line_into(out: mut String).R: bool
fun mut BufReader.read_line().R: Option<String>   // None at end of file
fun mut BufReader.read_all().R: Bytes
fun read_line().R: Option<String>   // buffered stdin
fun read_lines().R: List<String>
fun read_all().R: Bytes

fun print(s: &str)        fun print_bare(s: &str)
fun eprint(s: &str)       fun eprint_bare(s: &str)
```

A `File` is one descriptor: a file, a pipe, a socket and a terminal are the same type. A line reader returns `Option<String>` — `None` at end of input — since end of file carries no reason for a `Result`. `BufReader.read_line_into(out)` is the form that reuses one buffer.

### 16.7 `std/fs` (domain `fs`)

```kelp
fun read(path: &str).R: Bytes
fun read_text(path: &str).R: String
fun read_lines(path: &str).R: List<String>
fun write(path: &str, data: Bytes)
fun write_text(path: &str, text: String)
fun append(path: &str, data: Bytes)
fun copy(from: &str, to: &str)
fun rename(from: &str, to: &str)
fun remove(path: &str)
fun remove_all(path: &str)
fun mkdir(path: &str)
fun mkdir_all(path: &str)
fun sync(path: &str)
fun touch(path: &str)
fun exists(path: &str).R: bool
fun is_file(path: &str).R: bool
fun is_dir(path: &str).R: bool
fun size(path: &str).R: u64
fun list(path: &str).R: List<String>
fun walk(dir: &str).R: List<String>
fun open(path: &str, mode: &str).R: io.File       // "r", "w", "a", "rw"
fun temp_path(prefix: &str).R: String             // creates the file
datac Metadata(val size: u64, val mtime: i64, val mode: u32, val is_dir: bool, val is_file: bool)
fun metadata(path: &str).R: Metadata
```

### 16.8 `std/time` (domain `time`) and `std/rand` (domain `rand`)

```kelp
fun time.now_unix().R: i64
fun time.now_mono_ns().R: i64
fun time.sleep_ms(ms: u64)

fun rand.u64().R: u64
fun rand.below(n: u64).R: u64          // 0 .. n-1
fun rand.bytes(n: usize).R: Bytes
```

### 16.9 `std/env` (domain `env`)

```kelp
fun has(name: &str).R: bool
fun get(name: &str).R: String          // "" when unset, there is no Option yet
fun set(name: &str, value: &str)
fun unset(name: &str)
```

### 16.10 `std/process` (domain `process`)

```kelp
fun args().R: List<String>
fun arg_count().R: usize
fun spawn(program: &str, args: List<String>).R: Child
fun run(program: &str, args: List<String>).R: RunResult
fun mut Child.wait().R: i32
fun mut Child.is_running().R: bool
fun Child.kill(signal: i32)
fun exit(code: i32)
fun getpid().R: i64

datac Child(val pid: i32, val stdin: io.File, val stdout: io.File, val stderr: io.File,
            var exited: bool, var code: i32)
datac RunResult(val code: i32, val out: Bytes, val err: Bytes)
```

The grant names the program path itself, so `process = ["/bin/sh"]` is one path checked before the fork. `run` buffers both output streams whole; use `spawn` for a child whose output is unbounded.

### 16.11 `std/net` (domain `net`)

```kelp
fun connect(host: &str, port: u16).R: Socket
fun listen(port: u16).R: Listener
fun accept(l: Listener).R: Socket
fun close(s: Socket)
fun Listener.close()
fun shutdown_write(s: Socket)
fun send(s: Socket, data: Bytes).R: usize
fun send_all(s: Socket, data: Bytes).R: usize
fun send_text(s: Socket, text: &str)
fun recv(s: Socket, max: usize).R: Bytes
fun recv_into(s: Socket, buf: mut Bytes, at: usize).R: usize
fun Socket.wait_readable(timeout_ms: u64).R: bool
fun Socket.wait_writable(timeout_ms: u64).R: bool
fun mut Socket.set_timeout(ms: u64)
fun Listener.port().R: u16
fun Socket.peer_addr().R: String
fun resolve(host: &str).R: List<String>

datac Socket(val fd: i32)
datac Listener(val fd: i32)
```

### 16.12 `std/http` (ungated, holds `net`)

```kelp
fun get(url: &str).R: Response
fun post_text(url: &str, body: &str, content_type: &str).R: Response
fun make_get(url: &str).R: Request
fun Client.send(req: Request).R: Response
fun default_client().R: Client
fun parse_url(s: &str).R: Url

datac Url(val scheme: String, val host: String, val port: u16, val path: String)
datac Header(val name: String, val value: String)
datac Request(val method: String, val url: String, val headers: List<Header>, val body: Bytes)
datac Response(val status: i32, val headers: List<Header>, val body: Bytes, val reusable: bool)
datac Client(val timeout_ms: u64, val max_redirects: i32, val follow_redirects: bool)

fun Response.is_ok().R: bool
fun Response.has_header(name: &str).R: bool
fun Response.header(name: &str).R: String
fun Response.text().R: String
```

Responses frame their body by `Content-Length`, chunked encoding, a bodyless rule or end-of-file, and a reusable connection returns to the per-`host:port` pool sized by `http_pool` in `coral.toml`. `http.get` panics on a URL it cannot parse, a connection failure, or a capability denial; a response with a non-2xx status is not a panic, it is a `Response`.

### 16.13 `std/option`

```kelp
enum Option<T> {
    Some(val value: T)
    None
}

fun is_some<T>(o: &Option<T>).R: bool
fun is_none<T>(o: &Option<T>).R: bool
fun unwrap_or<T>(o: &Option<T>, fallback: T).R: T   // `T` must be Copy
fun unwrap<T>(o: &Option<T>).R: T                   // `T` must be Copy; panics on None

fun map<T, U>(o: &Option<T>, f: closure(&T).R: U).R: Option<U>
fun and_then<T, U>(o: &Option<T>, f: closure(&T).R: Option<U>).R: Option<U>
fun or_else<T>(o: &Option<T>, f: fun().R: Option<T>).R: Option<T>   // `T` must be Copy
fun each<T>(o: &Option<T>, f: closure(&T))
```

The enum is the point: `Some`/`None` is one of the two shapes `?` (5) and `if let` (6.1) understand, so a function that can fail one way returns `Option<T>` and its caller writes `val x = f(...)?`. Nothing in the compiler knows this module exists — the *shape* is the contract, so a program's own `Option` works identically.

The **callbacks borrow the payload** (`closure(&T).R: U`, so a plain function value is widened into one), which is what makes the combinators work at every instantiation: a `when` arm copies a `Copy` payload and auto-borrows it at the call, and an owned one is already a borrow, so `map` needs no `Copy` and hands nothing on:

```kelp
val text = option.Option.Some(value = String.from_str("hello"))
val len = option.map(o = text, f = fun(s: &String).R: usize = s.len())
val shout = option.map(o = text, f = fun(s: &String).R = str.to_upper(s.as_str()))
```

What is still offered for a **`Copy` payload only** is anything that hands the payload *out by value*: `unwrap_or`, `unwrap`, and `or_else` (which rebuilds `Some` from the payload it was handed). That is not a missing function but a missing *rule*: reading an owned payload out of an enum is a partial move, which v1 does not have (see 17), and a `move` parameter does not lift it — the payload would still have to move out of the enum. A program with an owned payload reads it with `if let`/`when`, which borrows it.

### 16.14 `std/result`

```kelp
enum Result<T, E> {
    Ok(val value: T)
    Err(val error: E)
}

fun is_ok<T, E>(r: &Result<T, E>).R: bool
fun is_err<T, E>(r: &Result<T, E>).R: bool
fun unwrap_or<T, E>(r: &Result<T, E>, fallback: T).R: T   // `T` must be Copy
fun unwrap<T, E>(r: &Result<T, E>).R: T                   // `T` must be Copy; panics on Err
fun unwrap_err<T, E>(r: &Result<T, E>).R: E               // `E` must be Copy; panics on Ok

fun map<T, U, E>(r: &Result<T, E>, f: closure(&T).R: U).R: Result<U, E>      // `E` must be Copy
fun map_err<T, E, F>(r: &Result<T, E>, f: closure(&E).R: F).R: Result<T, F>  // `T` must be Copy
fun and_then<T, U, E>(r: &Result<T, E>, f: closure(&T).R: Result<U, E>).R: Result<U, E>
fun or_else<T, E, F>(r: &Result<T, E>, f: closure(&E).R: Result<T, F>).R: Result<T, F>
fun each<T, E>(r: &Result<T, E>, f: closure(&T))
```

`Ok`/`Err` is the shape that carries a reason, and `?` requires the operand's failure payload type and the enclosing function's to be *identical* — there is no error conversion, which is where a trait (or a `From`-shaped builtin) would go.

The `Copy` requirements here are on the *other* side of the result: the arm that has to **rebuild** a variant takes a copy (so `map` needs a `Copy` error and `map_err` a `Copy` success). That is the consequential one in practice, because a `datac` is never `Copy`: `result.map` over a `Result<T, MyErr>` needs the partial-move rule 17 lists, and until then the error type has to be an integer or another scalar. When an instantiation fails there, the diagnostic points inside `std/result.kp` and says that the helper is written for a `Copy` payload.

### 16.15 `std/list`

The element-wise API of `List<T>` (`len`, `get`, `push`, `sort`, …) is builtin, because a list is monomorphized and its methods are generated C per element type (12.1). The operations that need a **callback** cannot be generated without knowing what the callback returns, so they are free functions in a module — `list.map(xs, f)` — the same shape `str.*` and `math.*` have.

```kelp
fun map<T, U>(xs: &List<T>, f: closure(&T).R: U).R: List<U>
fun filter<T>(xs: &List<T>, f: closure(&T).R: bool).R: List<T>          // `T` must be Copy
fun each<T>(xs: &List<T>, f: closure(&T))
fun find_index<T>(xs: &List<T>, f: closure(&T).R: bool).R: Option<usize>
fun any<T>(xs: &List<T>, f: closure(&T).R: bool).R: bool
fun all<T>(xs: &List<T>, f: closure(&T).R: bool).R: bool
```

Every callback borrows the element, so nothing is ever moved out of the list and the same functions work for a `Copy` and an owned element type:

```kelp
var names: List<String> = List.empty()
names.push(String.from_str("ab"))
val lens = list.map(xs = names, f = fun(s: &String).R: usize = s.len())
print(names.len())                       // still owned here
```

`filter` is the exception, and for the same reason `or_else` is: an accepted element is read *out* of the list by value, which is the partial move v1 does not have. `list.filter` over `List<String>` is a compile error saying so; `find_index`/`any`/`all` cover the predicates that only need to know *which* one, and a `Copy` element type filters directly.

## 17. Limits in v1

These are deliberate, and each one has a workaround in this document:

- No `for`, `break` or `continue`. Iteration is `while` plus an index (6.2).
- `?` propagates only an *identical* failure type (5.2): there is no error conversion, no `From`, and no boxing. `Option<T>`/`Result<T, E>` ship in `std/option`/`std/result`, and the stdlib's older error `datac`s remain as wrappers.
- A receiver on a generic type must write the type's own parameters (`fun Pair<A, B>.first()`, 9.3); there is no way to declare a method over the template with parameters of its own. An `enum` carries no methods at all, generic or not.
- A `Map` key must be `String`, an integer or `bool` (12.2): the compiler generates the key's hash and its equality, and a key type of your own would need a trait or a comparator function, neither of which exists.
- No user overloading (7.2) and no user `extern` bodies: `extern fun` is reserved for compiler-shipped stdlib modules.
- No partial moves: an owned field cannot be read out of a borrowed structure (9), and an owned `enum` payload cannot be read out of an owned one either, so move it with `Box.take` or pass a borrow. This is what makes the `Copy`-only helpers in 16.13/16.14/16.15 `Copy`-only: `option.unwrap` on an `Option<String>`, `result.map` over a `Result<T, MyErr>`, and `list.filter` over a `List<String>` each need a move out that does not exist yet.
- No references in fields, `List` elements, `Map` values or `Box` contents. `str` is a borrow, so text that must be stored is a `String`.
- No string concatenation operator, and no implicit `String` to `str`: use `str.concat` and `.as_str()`.
- No character literal, and no `char` values.
- No exceptions, no `defer`, no destructors beyond the automatic drop at scope exit.
- Function values (7.4) are declared functions, `fun` literals, and closures: a literal becomes a closure when it names a binding from the enclosing scope, captures are **move-only** (a `Copy` binding is copied, an owned one is moved), and a borrow cannot be captured at all. A function value cannot be called as the result of another call (`f()(x)`) — bind it first.
- A function value cannot be a container element or the return type of a function *type*, a function that reads a passdown is not a value, and a closure cannot be a `List`/`Map` element (a `Box` or a one-field `datac` holds it).
- A closure's captures are read out of its environment through a borrow, so an owned capture cannot be handed out by value (9's no-partial-moves rule). Its environment is dropped where the closure dies, so a closure cannot outlive anything it captured — which is why a borrow can never be a capture.
- A nested literal can copy a `Copy` capture out of the enclosing closure's environment, but not move an owned one out of it.
- `kelpc` has no formatter or linter command yet, and `kelpc run` cannot pass arguments to the program.
- Capability patterns are static text: they cannot be computed, narrowed or revoked at runtime.

## 18. Errors you will hit

Compiler diagnostics print as `file:line:col`, a message, and often a note with the fix. The common ones, with the fix:

| Message | Cause and fix |
| --- | --- |
| `expected 'fun', found Ident("fn")` | the keyword is `fun` |
| `expression-bodied function requires a '.R: T' return type` | add `.R: T`, or use a block body with `return` |
| `if without an else branch must evaluate to 'unit'` | an `if`, `if let` or `when` used as a value needs an `else` arm |
| `pattern 'Circle(a, b)' has 2 binding(s) but variant 'Circle' carries 1 payload field(s)` | `if let` and a `when` arm bind one name per payload field, in order |
| `cannot assign to 'x': not declared 'var'` | declare the binding `var` |
| `'id' is not a 'var' field` | the `datac` field must be declared `var` too |
| `use of moved value 'a'` | an owned value was moved earlier; reorder, or borrow instead |
| `cannot borrow 'c' as exclusive: already borrowed as shared` | end the earlier borrow (an inner block does it), because borrows last to the end of the scope |
| `cannot borrow 'c': already borrowed exclusively` | same rule, other direction |
| `cannot read the owned field 'name' by value: v1 has no partial moves` | pass a borrow, or hold the field in a `Box` and `take()` it |
| `this call returns a borrow of its parameter 'self', so that argument has to be a variable or a field` | bind the receiver first, then borrow from the binding |
| `argument 'b' has type 'String', expected 'str'` | add `.as_str()` |
| `argument 's' has type '&String', but this call moves its argument in` | the callee is a `move T` parameter (or a container's `push`-shaped builtin): a borrow cannot be moved out of, so pass an owned value — `Box.take()` moves one out of a box |
| `'move' cannot be written on the borrowed type 'str'` | a `str`/`&T` is already borrowed, so there is nothing to own: write the owned type (`move String`) |
| `'mut' and 'move' cannot be combined on one parameter` | `mut T` is an exclusive borrow, `move T` takes ownership — pick the one the callee needs |
| `'move' cannot be written on a receiver` | a receiver is always borrowed; a move-taking parameter is `name: move T` |
| `'first' is a generic function` (as a value) | a function value needs one concrete signature; wrap the call in a small non-generic function |
| `'f' reads a passdown, so it cannot be a function value` | a passdown is a hidden argument, so its C signature is not a plain function pointer; call it directly |
| `a call through 'f' cannot use named arguments` | a function type has no parameter names; pass the arguments positionally |
| `a call through 'f' takes 1 argument(s), found 2` | an indirect call is counted against the callee's own parameter list |
| `'mut'/'&' cannot be written on the function type` | a function value is Copy, so a borrow of one is a pointer to a pointer |
| `a function type cannot return another function type` | C cannot write that declarator; return a `datac` holding the function value |
| `'fun(i32).R: i32' cannot be a 'List' element yet` | a container's generated C type names its element type as a prefix; hold it in a one-field `datac` |
| `a 'fun' literal cannot capture 's': it is a borrow ('str')` | the environment outlives the frame the borrow points into; capture an owned value, or pass the borrow as a parameter of the literal |
| `a 'fun' literal cannot capture 'n': it is a field of 'Counter'` | reading a field out of a borrowed receiver by value is a partial move (A-11); bind a `Copy` field to a local first, or take the receiver as a `move` parameter |
| `cannot capture 'text' into this literal: it belongs to the enclosing closure's environment` | moving an owned value out of an environment is a partial move; capture it from the frame that owns it, or copy a `Copy` one |
| `cannot read a captured value out of the closure's environment by value` | a capture is read through a borrow; use it in place, or capture a `Copy` value |
| `this is a closure: its body captured a binding from the enclosing scope` | the parameter's type has to say `closure(...)`; a `fun(...)` value has no room for an environment |
| `this is a plain function value, not a closure` | the literal captured nothing, so it has no environment; write the expected type as `fun(...)`, or name a binding to capture |
| `'&' cannot be written on the closure type` | a closure is an owned value; a parameter of that type already borrows it, and `move closure(...)` takes it over |
| `'closure().R: i32' cannot be a 'List' element yet` | a closure owns its environment and a list's elements are inline; use `Box<closure(...)>` or a one-field `datac` |
| `a 'fun' literal cannot read passdown 'base'` | a passdown is a hidden argument; pass the value as a parameter |
| `a 'fun' literal cannot return a borrow ('&str')` | an indirect call has no parameter an elided lifetime could bind to; return an owned value |
| `a 'fun' literal needs a body` | write `= expr` or `{ ... }` |
| `a string template can only be printed` | a template is not a value; build the text with `str.concat` or a `String` |
| `unknown escape '\q' in a string literal` | the escapes are `\n`, `\t`, `\r`, `\"`, `\\` |
| `function 'f' is defined more than once for the same parameter types` | user code cannot overload; rename one |
| `cannot tell what 'List.empty()' holds` | write the element type on the binding: `var xs: List<i32> = List.empty()` |
| `'str' is a borrowed view` | an owned field, element or value is needed; use `String` |
| `cannot find module 'x': expected '<path>'` | the `impl` path is wrong, or the file is missing |
| `import cycle: a -> b -> a` | remove the cycle |
| `coral.toml sections are not supported` / `unknown key 'x' in coral.toml` | the manifest is flat: `name`, `version`, `entry`, `http_pool` |
| `no 'main' function found` | the entry module needs `fun main()` |
| `'fs.read' needs the 'fs' capability, which module 'm' was not granted` | grant it at the `impl` line: `impl "m" with caps(fs = ["/path/**"])` |
| `datac 'Bad' cannot hold itself by value` | a recursive field needs indirection: `Box<Bad>` or `List<Bad>` |
| `'Pair' is a generic type — it needs 2 type argument(s)` | write the arguments where the type is used: `Pair<i32, String>` (9.3) |
| `cannot infer type parameter 'T' of 'Option' here` | neither the arguments nor the expected type determines `T`; annotate the binding (`val x: Option<i32> = ...`, or `val xs: List<String> = empty_of()`) |
| `'main' cannot take type parameters` | the entry point is not called, so nothing can instantiate it |
| `receiver 'Pair' must write the type's own type parameters, in order` | a method on a generic type is `fun Pair<A, B>.first()`; the receiver cannot omit or rename the parameters |
| `'Option' is an enum — an enum carries no methods in v1` | write a free function over the instantiation instead (9.3) |
| `'Point' does not take type arguments` | only a generic type, a generic function, or a generic `enum`'s variant takes one (9.3) |
| `'f64' cannot be a Map key` | a key must be `String`, an integer or `bool` — the compiler generates its hash and equality (12.2) |
| `'?' cannot convert a failure` | the operand's failure payload type and the enclosing function's have to be the same (5.2) |
| `'?' propagates a failure out of the enclosing function, so that function has to return an enum shaped 'Ok'/'Err' or 'Some'/'None'` | `?` in a function returning `Unit` (including `main`), or in one returning something that is not that shape |
| `'?' needs a value shaped 'Ok'/'Err' or 'Some'/'None' held by value` | the operand is not an enum of that shape, or it is a borrow — match it with `when`/`if let` |
| `'first_of' does not take type arguments — a method's type parameters come from its receiver` | write the parameters on the receiver: `fun Pair<A, B>.first_of()` |
| `in 'Holder<str>': field 'x' is a borrowed type` | a type argument may not be `str` or a reference; use `String` |
| `datac 'Bad' is defined more than once` / `field 'x' is defined more than once` | duplicate declaration |

Panics happen at the point the program asked for something impossible, and exit non-zero:

```
panic: assertion failed
panic: i32.to_u64: value out of range
panic: Bytes.get: index out of bounds
panic: String.from_bytes: input is not valid utf-8
```

## 19. Style

- 4 spaces, no tabs. Hard limit 100 columns.
- One blank line between top-level declarations, and between functions.
- Canonical file order: `impl`, then `const`/`pd`, then `datac`, then `fun` (13.2).
- No semicolons. Do not wrap an expression across lines unless the wrap is inside parentheses or an argument list.
- Standard library modules, functions and datac fields are `snake_case` (`fs.read_text`, `str.is_empty`, `Line.text`), and types are `PascalCase` (`String`, `Bytes`, `Box`, `Metadata`, `Url`).
- User code case is not enforced. The examples and this plan use `snake_case` for functions and lowercase for bindings, matching the standard library.
- Named arguments are used wherever a call would otherwise be ambiguous, which in practice means every `datac` construction with two or more fields, as in `Circle(r = 2.0)`.
- Comments are `//`, written as full sentences when they explain a decision and never to restate the next line.

No formatter ships yet. `kelpc` currently has no `fmt` or `lint` command, so these rules are a review convention rather than a check.

## 20. Grammar summary

```
program      := line* EOF
line         := impl | const | pd | datac | enum | fun

impl         := "impl" string [ "as" ident ] [ "with" "caps" "(" grant ("," grant)* ")" ]
grant        := ident "=" "[" string ("," string)* "]"
const        := "const" ident ":" type "=" expr
pd           := "pd" ("val" | "var") ident [ ":" type ] "=" expr
datac        := "datac" ident [ typeparams ] "(" [ field ("," field)* ] ")"
field        := ("val" | "var") ident ":" type
enum         := "enum" ident [ typeparams ] "{" ( ident [ "(" [ field ("," field)* ] ")" ] )* "}"
typeparams   := "<" ident ("," ident)* ">"
typeargs     := "<" type ("," type)* ">"   // in an expression: adjacent to the name and to "("
fun          := [ "extern" ] "fun" [ "mut" ] ( ident [ typeparams ] "." )? ident [ typeparams ]
                "(" [ param ("," param)* ] ")" [ ".R" ":" type ] body
param        := ident ":" [ "mut" | "move" ] type
body         := "=" expr | block | <nothing for extern>

type         := [ "&" ] ( ident [ "<" type ("," type)* ">" ] [ "." ident ]
                       | "fun" "(" [ fparam ("," fparam)* ] ")" [ ".R" ":" type ] )
fparam       := [ "mut" | "move" ] type

stmt         := "val" | "var" | "while" | "return" | expr
val          := "val" ident [ ":" type ] "=" expr
var          := "var" ident [ ":" type ] "=" expr
while        := "while" expr block
return       := "return" [ expr ]
block        := "{" (line-separated stmt)* "}"

expr         := assign
assign       := binary ( ( "=" | "+=" | "-=" | "*=" | "/=" | "%=" ) assign )?
binary       := unary ( binop binary )?
binop        := "||" | "&&" | "==" | "!=" | "<" | "<=" | ">" | ">="
              | "+" | "-" | "*" | "/" | "%"
unary        := ( "-" | "!" | "&" [ "mut" ] | "*" ) unary | postfix
postfix      := primary ( "?" | "." ident [ typeargs ] [ "(" args ")" ] )*
primary      := int | float | string | template | "true" | "false"
              | "fun" "(" [ param ("," param)* ] ")" [ ".R" ":" type ]
                ( "=" expr | block )
              | ident [ typeargs ] [ "(" args ")" ] | "(" expr ")" | block
              | "if" expr block [ "else" ( block | if ) ]
              | "if" "let" ident [ "(" (ident | "_") ("," (ident | "_"))* ")" ] "=" expr
                block [ "else" ( block | if ) ]
              | "when" "{" ( expr "->" expr )* "else" "->" expr "}"
args         := [ arg ("," arg)* ]
arg          := [ ident "=" ] expr
```

## 21. A complete program

`wordcount.kp` counts how often each word appears in a file, prints the words in order with their counts, and prints the total. It compiles with `kelpc build wordcount.kp -o wordcount` and runs as `./wordcount /abs/path/input.txt`.

```kelp
// wordcount: count how often each word appears in a file.
impl "std/fs"
impl "std/str"
impl "std/fmt"
impl "std/process"

fun bump(counts: mut Map<i64>, word: &str) {
    if counts.has(word) {
        counts.insert(word, counts.get(word) + 1)
    } else {
        counts.insert(word, 1)
    }
}

fun main() {
    val args = process.args()
    if args.len() != 2 {
        print("usage: wordcount <file>")
        process.exit(2)
    }

    val path = args.get(1)
    val text = fs.read_text(path.as_str())
    val words = str.split(text.as_str(), " ")

    var counts: Map<i64> = Map.new()
    var i: usize = 0
    while i < words.len() {
        val word = str.trim(words.get(i).as_str())
        if !str.is_empty(word.as_str()) {
            bump(counts, word.as_str())
        }
        i += 1
    }

    var keys = counts.keys()
    keys.sort()

    var total: i64 = 0
    var j: usize = 0
    while j < keys.len() {
        val key = keys.get(j)
        val n = counts.get(key.as_str())
        total += n
        print("${key}: ${n}")
        j += 1
    }
    val total_text = fmt.int(total)
    print(str.concat("words: ", total_text.as_str()))
}
```

What the program shows, in order: `impl` lines for four modules, a free function that takes an exclusive borrow of a `Map`, a capability-checked `fs` call whose argument has to be absolute, `Map` and `List` builtins in a loop, `String` ownership passed to a `mut` parameter, and a template that prints two values.
