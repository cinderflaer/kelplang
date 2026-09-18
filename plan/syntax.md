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
as  const  datac  else  enum  extern  false  fun  if  impl  mut
pd  return  true  val  var  when  while
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
| `Map<V>` | owned | keys are always `String`, values are `V`, insertion order |
| `Box<T>` | owned | one heap cell that holds a `T` or nothing |
| `datac` name | owned | record with per-field `val`/`var` |
| `enum` name | owned | tagged union: one variant is live, and patterns match on it (9.2) |
| `Unit` | Copy | absent value, the implicit return type |

Rules that follow from the table:

- There is no implicit numeric conversion, not even between two integer widths. `u64 % 10` produces a `u64` and cannot be pushed into a `u8` slot without `to_u8()`.
- Owned types are not `Copy`: assigning one moves it, and every owned value is dropped at the end of the scope that owns it, including inside `datac` fields, `List` elements and `Map` values.
- `str` is a borrow, so it cannot live in a `datac` field, a `List` element, a `Map` value or a `Box`; only owned types can.
- No `List` or `Map` may hold another `List` or `Map` directly, because elements are stored inline. Put it in a `Box`.
- `Unit` is not a value and cannot be stored.

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
| 8 | `.field`, `.method(...)`, `f(...)` | postfix |
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
- Parameters are `name: T`. A parameter is a shared borrow of its argument. `name: mut T` is an exclusive borrow, which is the only spelling for mutation in parameter position: `&mut T` there is a parse error.
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
- There are no closures and no function values: a function name is not a value, and a function cannot be passed to another function.
- There are no generics. `List`, `Map` and `Box` are builtins with one instantiation per element type, and a function that takes a `List<i32>` cannot take a `List<String>`.

### 7.3 `main`

```kelp
fun main() { ... }
```

`main` must exist in the entry module, takes no parameters, returns `Unit`, and cannot be called. A program with no `main` fails with `no 'main' function found`.

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

### 12.2 `Map<V>`

Keys are always `String`; values are `V`. Entries keep insertion order, which is why `keys()` and `values()` are lists and not hash order.

```kelp
var counts: Map<i64> = Map.new()
counts.insert("one", 1)
counts.insert("two", 2)
print(counts.has("two"))
print(counts.get("two"))
print(counts.get_or("nope", 0))
counts.insert("two", 22)
print(counts.remove("one"))
print(counts.keys().len())
```

| Method | Takes | Returns |
| --- | --- | --- |
| `Map.new()` / `Map.empty()` | nothing | empty map, value type from the binding |
| `len()` / `is_empty()` | shared | `usize` / `bool` |
| `has(key)` | shared, `&str` | `bool` |
| `get(key)` | shared, `&str` | the value, or a borrow of it; panics when absent |
| `get_or(key, fallback)` | shared | the value or the fallback, for clonable values |
| `insert(key, v)` | exclusive | `Unit`, copies the key and moves the value in |
| `remove(key)` | exclusive | the value, moved out; panics when absent |
| `clear()` | exclusive | `Unit` |
| `keys()` / `values()` | shared | `List<String>` / `List<V>` |

`Map.of` does not exist: a map starts empty, so `Map.new()` needs the value type on the binding.

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

Thirteen modules ship inside the compiler binary. Import them with `impl "std/<name>"`; the namespace is the module name.

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

### 16.1 `std/str`

```kelp
fun len(s: &str).R: usize
fun byte(s: &str, i: usize).R: u8
fun byte_index(s: &str, i: usize).R: usize
fun is_empty(s: &str).R: bool
fun char_count(s: &str).R: usize
fun char_at(s: &str, i: usize).R: u32
fun substring(s: &str, from: usize).R: &str
fun index_of(s: &str, needle: &str).R: Match          // datac Match(val ok: bool, val at: usize)
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
fun parse_i64(s: &str).R: ParseInt              // datac ParseInt(val ok: bool, val value: i64)
fun parse_u64(s: &str).R: ParseUint
fun parse_f64(s: &str).R: ParseFloat
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

datac Line(val ok: bool, val text: String)
datac BufReader(val file: File, var buf: Bytes, var start: usize, var end: usize)
fun buf_reader(f: File).R: BufReader
fun mut BufReader.read_line_into(out: mut String).R: bool
fun mut BufReader.read_line().R: Line
fun mut BufReader.read_all().R: Bytes
fun read_line().R: Line            // buffered stdin
fun read_lines().R: List<String>
fun read_all().R: Bytes

fun print(s: &str)        fun print_bare(s: &str)
fun eprint(s: &str)       fun eprint_bare(s: &str)
```

A `File` is one descriptor: a file, a pipe, a socket and a terminal are the same type. `Line.ok` is `false` at end of input. Moving text out of a `Line` is not possible (no partial moves), so the owning form is `BufReader.read_line_into(out)`.

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

## 17. Limits in v1

These are deliberate, and each one has a workaround in this document:

- No `for`, `break` or `continue`. Iteration is `while` plus an index (6.2).
- No `Option` or `Result`, so there is no `?` and no `if let`. Fallible stdlib calls return a `datac` with an `ok` flag, or panic. `Option<T>` waits on generics.
- No generics, so an `enum` cannot carry a type parameter: `enum Option<T>` is not expressible yet. `List`, `Map` and `Box` are builtins.
- No user overloading (7.2) and no user `extern` bodies: `extern fun` is reserved for compiler-shipped stdlib modules.
- No partial moves: an owned field cannot be read out of a borrowed structure (9), so move it with `Box.take` or pass a borrow.
- No references in fields, `List` elements, `Map` values or `Box` contents. `str` is a borrow, so text that must be stored is a `String`.
- No string concatenation operator, and no implicit `String` to `str`: use `str.concat` and `.as_str()`.
- No character literal, and no `char` values.
- No exceptions, no `defer`, no destructors beyond the automatic drop at scope exit.
- `kelpc` has no formatter or linter command yet, and `kelpc run` cannot pass arguments to the program.
- Capability patterns are static text: they cannot be computed, narrowed or revoked at runtime.

## 18. Errors you will hit

Compiler diagnostics print as `file:line:col`, a message, and often a note with the fix. The common ones, with the fix:

| Message | Cause and fix |
| --- | --- |
| `expected 'fun', found Ident("fn")` | the keyword is `fun` |
| `expression-bodied function requires a '.R: T' return type` | add `.R: T`, or use a block body with `return` |
| `if without an else branch must evaluate to 'unit'` | an `if` or `when` used as a value needs an `else` arm |
| `cannot assign to 'x': not declared 'var'` | declare the binding `var` |
| `'id' is not a 'var' field` | the `datac` field must be declared `var` too |
| `use of moved value 'a'` | an owned value was moved earlier; reorder, or borrow instead |
| `cannot borrow 'c' as exclusive: already borrowed as shared` | end the earlier borrow (an inner block does it), because borrows last to the end of the scope |
| `cannot borrow 'c': already borrowed exclusively` | same rule, other direction |
| `cannot read the owned field 'name' by value: v1 has no partial moves` | pass a borrow, or hold the field in a `Box` and `take()` it |
| `this call returns a borrow of its parameter 'self', so that argument has to be a variable or a field` | bind the receiver first, then borrow from the binding |
| `argument 'b' has type 'String', expected 'str'` | add `.as_str()` |
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
line         := impl | const | pd | datac | fun

impl         := "impl" string [ "as" ident ] [ "with" "caps" "(" grant ("," grant)* ")" ]
grant        := ident "=" "[" string ("," string)* "]"
const        := "const" ident ":" type "=" expr
pd           := "pd" ("val" | "var") ident [ ":" type ] "=" expr
datac        := "datac" ident "(" [ field ("," field)* ] ")"
field        := ("val" | "var") ident ":" type
fun          := [ "extern" ] "fun" [ "mut" ] ( ident "." )? ident
                "(" [ param ("," param)* ] ")" [ ".R" ":" type ] body
param        := ident ":" [ "mut" ] type
body         := "=" expr | block | <nothing for extern>

type         := [ "&" ] ident [ "<" type ">" ] [ "." ident ]

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
postfix      := primary ( "." ident [ "(" args ")" ] )*
primary      := int | float | string | template | "true" | "false"
              | ident [ "(" args ")" ] | "(" expr ")" | block
              | "if" expr block [ "else" ( block | if ) ]
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
