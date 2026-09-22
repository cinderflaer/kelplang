# The Kelp language

Kelp is a small compiled language. `kelpc` lexes and parses a Kelp source tree, checks types and ownership, emits C, and calls `cc` to produce a native binary. Programs link nothing beyond libc, have no garbage collector, and reach the outside world only through capabilities a module declares.

This file is the language reference. It is written to be read start to finish by someone who wants to write a program, so it starts with the toolchain and ends with a complete example. Where another plan file disagrees with this one about syntax or style, this file is correct.

| File | Contents |
| --- | --- |
| `plan/syntax.md` | this file: syntax, types, semantics, standard library |
| `plan/architecture.md` | compiler layout, capability model, non-syntax decisions |
| `plan/roadmap.md` | status, critical path, milestone scheme, open items |
| `plan/roadmap.md` | the plan: marks, open items, and its Backlog part for what is named but unscheduled |
| `plan/done.md` | record of what has shipped |

## 1. Getting started

### 1.1 Build the compiler

```sh
cargo build --release -p kelpc  # produces target/release/kelpc
```

`kelpc` takes three commands:

```sh
kelpc build <file.kp|project-dir> [-o <output>]   # default output: build/<file stem>
kelpc run   <file.kp|project-dir>                 # build, then run
kelpc version                                     # the compiler's version
```

`kelpc run` does not forward arguments to the program. Build first and run the binary directly when the program needs `argv`.

The version `kelpc version` prints is the crate's `[package] version`, read at compile time, so it always names the release the binary was built from.

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
    alias.kpconf             # optional, see 13.3
```

```sh
kelpc build myproject/
```

A loose `.kp` file that is not inside a project still compiles, and is then its own module root. A file *inside* a project's `src/` finds the manifest by walking up, and is compiled as a module of that project (so `impl "shapes/circle"` still means `src/shapes/circle.kp`, whatever directory you are standing in).

```sh
kelpc build src/main.kp -o myproject/main    # from the project root: same module graph
```

With no `-o`, the binary is written into the **current working directory**, named after the last path segment you passed: `kelpc build myproject/` writes `./myproject` and `kelpc build src/main.kp` writes `./main`, wherever you are standing. The emitted C lands beside it as `<name>.c`. Pass `-o` when that is not where you want the output — the fixtures and `bench/` always do.

What a project buys you, concretely:

- **`impl` paths and module ids are relative to `src/`**, not to the working directory or to the importing file. `impl "shapes/circle"` is always `<root>/src/shapes/circle.kp`, however deep the importer sits (§13.3).
- **The entry module can be somewhere other than `src/main.kp`**, and it is the only module whose top-level names are unprefixed — everything else is reached as `alias.Item` (§13.3).
- **A `alias.kpconf` at the project root** (beside `coral.toml`, not in `src/`).
- **The program-level settings** in the manifest — today `http_pool`.

### 1.4 `coral.toml`

The manifest is flat: one `key = value` per line, `#` starts a comment, and an unknown key is an error rather than an ignored line. Every value is a double-quoted string except `http_pool`, which is a number.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `name` | quoted string | none | project name |
| `version` | quoted string | none | project version, unvalidated free text |
| `entry` | quoted string | `"src/main.kp"` | path of the entry file, **relative to the project root** |
| `http_pool` | number | `4` | kept-alive connections per `host:port` in `std/http`; `0` disables pooling |

```toml
# myproject/coral.toml
name = "myproject"          # recorded; not used by the compiler yet
version = "0.1.0"           # free text, unvalidated
entry = "src/main.kp"       # relative to this file's directory
http_pool = 4               # the one numeric key

# Anything from the first [section] on belongs to Coral (the project-layer
# build tool), not to kelpc, so the compiler reads no further. Today that means
# kelpc ignores it; it does not reject it.
[package]
```

How it is used, end to end:

1. `kelpc build myproject/` reads `myproject/coral.toml`, resolves `entry` against that directory, and requires the file to exist.
2. Everything reachable from the entry file's `impl` lines is loaded from `<root>/src/`. A path that leaves `src/`, names a missing file, or forms a cycle is a diagnostic (§13.1).
3. `http_pool` is **spliced into the shipped `std/http` source** before it is compiled, so it changes the connection ceiling for the whole program rather than for one call site. Nothing else in the manifest reaches the compiler.
4. `name` and `version` are parsed and recorded; no pass reads them today. Coral owns what they mean.

Rules and errors:

| What you wrote | What happens |
| --- | --- |
| a bare `key value` with no `=` | `expected 'key = "value"' in coral.toml, found '<line>'` |
| `entry = src/main.kp` (no quotes) | `coral.toml value for 'entry' must be a double-quoted string` |
| an unknown key | `unknown key 'wat' in <path to coral.toml>` — a typo is an error, not a silent default |
| `http_pool = "4"` or `http_pool = -1` | `coral.toml value for 'http_pool' must be a non-negative whole number, found '"4"'` |
| `entry = "src/nope.kp"` | `cannot read the manifest's entry point 'src/nope.kp': <os error>` |
| `kelpc build <dir>` where the directory has no manifest | `'<dir>' is a directory but has no coral.toml (pass a .kp file, or add a manifest)` |
| a `.kp` file under a project but outside `src/` | `entry file '<path>' is not inside '<root>/src'` — the layout is not advisory |
| `[tasks]` and anything after it | ignored by `kelpc` (Coral's surface); the four keys above must come **before** the first `[` |

Blank lines and `#` comments are free, keys may appear in any order, and a manifest with only `entry` is a complete manifest. `kelpc build` and `kelpc run` both take a project directory or a `.kp` file; `kelpc run <dir>` compiles and runs it in one step.

#### Coral's own sections

Everything from the first `[section]` on is the project-layer tool's surface (Coral), and `kelpc` reads none of it. The one that changes how *this* compiler is chosen is the per-project toolchain pin:

```toml
name = "jsonlibary"          # the flat keys kelpc reads
version = "0.1.0"
entry = "src/main.kp"

[toolchain]                  # Coral's; kelpc stops at the '[' above
kelp = "v0.1.2"             # the kelpc version this project builds with
```

Written by `coral toolchain kelp pin v0.1.2`, printed by `coral toolchain kelp pin`, removed by `--clear`. It is a line edit, so comments and section order survive, and a pinned version that is not installed yet is installed on the spot (`coral toolchain kelp install <ver>`, which prefers a release binary and falls back to clone-and-build). The store holds one version per component, so installing or using a version drops the others; a pin that is not the stored one is reinstalled the next time the project builds.

Coral resolves the compiler in this order: `$KELPC`, then the project's pin, then `~/.coral/bin/kelpc`, then `PATH`. Two things follow, and both bite:

- **A pin does nothing for a direct `kelpc` call.** Running `kelpc run src/main.kp` yourself uses whatever `~/.coral/bin/kelpc` points at (the machine default, set by `coral toolchain kelp use <ver>`), not the pin. Build through Coral — `coral build` / `coral run` — for the pin to apply.
- **The store holds one version at a time.** `install` and `use` delete the other versions under `~/.coral/toolchains/` once the new binary is in place, so a pin that is not the stored version displaces it: two projects pinned to different versions reinstall on each switch (a release download when the tag has one, a clone-and-build when it does not). A pin still selects the compiler — it just does not keep both.
- **`kelpc build`/`run` writes its output into the current directory** by default (1.3), while `coral build` writes `build/`. An IDE run configuration should point at `coral run`, or at the `build/` artifact.

`coral toolchain kelp list` prints the versions available on Gitea; `coral toolchain kelp installed` prints what is under `~/.coral/toolchains/` — at most one version, since the store keeps one per component.

## 2. Lexical structure

### 2.1 Comments

`//` starts a line comment, and `/* ... */` a block comment:

```kelp
// a line comment, ending at the newline

/* a block comment: one line, or several
   lines of prose */

val limit = 10   // a trailing comment is the same kind of thing
```

A block comment ends at its own `*/`, and block comments **nest**: an inner `/*`
needs its own `*/`, so a region that already contains a block comment can be
commented out as a unit. C's rule (the first `*/` wins) is what turns that case
into a parse error somewhere further down the file, which is the failure mode
this rule exists to remove; nested comments are also what Rust and Kotlin do. The
cost of nesting is that an extra `*/` no longer "closes twice" silently: an
unbalanced `/*` is an unterminated block comment, reported at the opening
delimiter.

Neither form is a token. Both are trivia, so a comment may sit wherever
whitespace may, and `kelpc fmt` copies it verbatim (19). A `//` inside a block
comment is text, and delimiters inside a string or character literal are text
too (`"a/*b*/c"` is the bytes `a/*b*/c`).

**Documentation comments.** `//!` and `///` are documentation comments: they
document the declaration that follows them (a `fun`, a `datac`, an `enum`, an
`extern fun`, a `const`, a `datac` field or an `enum` variant), and a run of
them before the first declaration documents the file itself. A `//!` is read as
the header of a block, so it has to start a line: consecutive `//!` lines are
one foldable unit, and a `//!` with code before it on the same line documents
nothing and is a **compiler error** rather than a silent comment:

```kelp
//! The buffer owns its bytes and frees them once, at the end of the owning
//! scope. Nothing here is a view: every getter copies.
datac Buffer(val bytes: Bytes)

val n = 1   //! error: a '//!' is a documentation comment and must start a line
```

Nothing else reads a documentation comment: the marker is trivia to the token
stream, and the parser has no doc-comment node, so the language half is the
rule above and the head below. The tooling reads them: `coral docs get`
renders one file's documentation with the declaration each block documents,
`coral docs list` prints every documented thing in a project, and
`coral docs search` finds one by name or by prose.

**Naming what a block documents.** A block documents the declaration below it,
which is what most blocks need. A block that documents something else says so in
a **head**: a line `[docs(<kind>: <name>)]`, the first line of the block, where
`<kind>` is one of `module`, `fun`, `datac`, `enum`, `const`, `field`,
`variant` and `title`. Every kind but `title` takes a name as written in the
source; `title` takes a quoted string, and is the kind for prose that belongs to
no declaration at all:

```kelp
//! [docs(title: "Error handling")]
//! Every fallible call returns a Result: there is no panic path here.

//! [docs(fun: Frame.decode)]
//! The test function that checks a truncated frame is refused. The head is what
//! lets this block sit next to the code it is about, or far away from it.
```

A head is one line. The closing `]` is what ends it, and a head written open
across lines is closed by a `]` line of its own, which carries nothing and is
dropped: the comment run is what ends the block either way, so the `]` is
allowed and not required.

A head is read by the tooling and never by the compiler. A head that cannot be
read (an unknown kind, a `title:` without quotes, a name with a space in it) is
a warning from `coral docs` with the file and line, and is not a compile error,
because the compiler does not see inside a comment.

**What a block documents without a head.** The key is derived from the block's
place in the file:

| Block sits above | Key |
| --- | --- |
| a `fun` (or an `extern fun`) | `fun:name` |
| a `datac` / an `enum` | `datac:name` / `enum:name` |
| a `const` | `const:NAME` |
| a field (`val` / `var`) | `field:name` |
| a variant | `variant:Name` |
| the first declaration of a file, or its `impl` lines | `module:<file stem>` |

Either way, a `//!` line is exempt from the comment-run rule (19) and does not
count toward a plain `//` run: a run of doc lines is already one block.

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
as  closure  const  datac  elif  else  enum  extern  false  for  fun  if  impl
move  mut  pd  return  true  val  var  when  while
```

`fn`, `let`, `struct`, `use`, `loop`, `break`, `continue`, `match`, `trait` and `self` are not keywords. Writing `fn`, as in Rust or Kotlin, is a parse error, and `self` is an ordinary identifier (see 8). `in` is not a keyword either: `for x in xs` reads it as a separator, the way `if let` reads `let` (6.1), so a binding may still be named `in`.

### 2.4 Literals

```kelp
val a = 42              // integer literal
val b = 3.5             // float literal
val c = true            // bool literal
val d = "text\n"        // string literal
val e = 'x'             // character literal
val g = b'x'            // byte literal
val f = "n = $a, sum = ${a + 1}"   // string template
```

- Integer literals are decimal, or hexadecimal after `0x` and binary after `0b`: `0xff`, `0b1010`. The prefix letter may be written in either case; there is no octal form. A radix literal is the same value as its decimal spelling (`0xff` is `255`) and has no width of its own either, so it takes the type its context asks for exactly as `255` does, and is refused at its own line when the value does not fit that type. A literal that nothing gives a type to is `i32`, so `val big = 4294967295` is an error rather than a wrap, and the message names a type that holds the value (`val big: u64 = 4294967295`). There is no suffix and no digit separator, and anything else after the digits is a lex error that names the shape rather than splitting the token: `0b12` is `invalid digit '2' in a binary literal`, and `5u8` is `a literal takes no suffix`. A literal has to fit `i64`, the largest value the lexer holds.
- Float literals are `digits.digits`, and are `f64` unless the context requires `f32`.
- A **character literal** is one Unicode scalar value written in single quotes: `'x'`, `' '`, `'"'`, `'é'`, `'中'`. A backslash is never a quote escape in a *string*, so the two literal forms never collide — `''` is not a string, and `"a"` is not a character.
- A character literal takes the integer type its context requires, exactly as an integer literal does, and is `char` when nothing determines one. That one rule is what makes byte-level code read the way it thinks:

```kelp
val c: u8 = str.byte(text, i)
if c == '"' { }            // not `c == 34`
if c == '\\' { }            // not `c == 92`
out.push(' ')               // not `out.push(32)`
val digit: u8 = '0' + n     // not `48 + n`
val big: i64 = 'z'          // the same literal, in a wider slot
```

- A code point that does not fit the type the context asks for is a **compile error**, not a truncation: `val c: u8 = '中'` fails with the character and its code point named.
- A `char` is a code point, and it is *not* an integer: `'a' + 1` and `'a' == 97` are both errors. Give the literal the type you mean (`val a: u8 = 'a'`) when you are doing arithmetic on it.
- The escape set for all three literal forms is closed. In a **character** literal: `\n`, `\t`, `\r`, `\0`, `\'`, `\"`, `\\`, `\xNN` and `\u{...}`: `'\u{4e2d}'` is U+4E2D, braced so the escape has an unambiguous end. In a **byte** literal: `\n`, `\t`, `\r`, `\0`, `\'`, `\"`, `\\` and `\xNN` (not `\u{...}`: a byte literal is one byte, not a code point). In a **string** literal: `\n`, `\t`, `\r`, `\"`, `\\` and `\xNN` up to the ASCII range: a string literal is NUL-terminated and UTF-8, so `\0` and `\x00` are refused there and a byte above `0x7f` is not text (that is what `Bytes` is for); `\u{...}` is not an escape either (write the character itself: `"中"`).
- `\xNN` is **exactly two hex digits**, so it cannot run on into the text after it: `"\x41a"` is `"Aa"`, and `\x0d` followed by `a` is a carriage return and the letter (C's hex escape swallows the `a` and overflows the byte). In a character literal it is the code point, so `'\x41'` is `'A'`; in a byte literal it is the byte, so `b'\x41'` is `65` and `b'\xff'` is `255`.
- A **byte literal** is one ASCII character or a `\xNN` escape, and it is a `u8` **by construction**: `val nl = b'\n'` needs no type written anywhere, which is the whole difference from a character literal. It does not take the type its context asks for, so `val n: i64 = b'x'` is refused rather than widened (write `val n: i64 = 'x'` for that), and a non-ASCII character belongs in a character literal (`b'é'` is refused, and `b'\xe9'` is the byte 233, the value `'\u{e9}'` has in a `u8` position).
- A string literal is a `str`: a borrowed, NUL-terminated byte sequence. It is not owned and is not freed. It is converted to an owned `String` implicitly where one is expected (see 5.1).

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
&  |  ^  <<  >>  ~    bitwise: and, or, xor, shift left, shift right, complement
==  !=  <  <=  >  >=   comparison
&&  ||  !              logic
=  +=  -=  *=  /=  %=  assignment and compound assignment
&  &mut  *             reference, exclusive reference, dereference
?                      propagate a failure (5.2)
.  ,  :  (  )  {  }    member access, separators
->                     `when` arms only
'                      character literal (2.4)
```

`&` is both the borrow prefix and the bitwise-and operator, and position decides which: `&x` borrows, `a & b` masks. C++ reads `&` the same way. `<<` and `>>` are written as two adjacent `<`/`>` characters — the lexer does not fold them into one token, so a nested type argument list (`List<List<i32>>`) still closes.

`[` and `]` are lexed but no expression uses them; they exist for capability grant lists.

## 3. Types

| Type | Kind | Notes |
| --- | --- | --- |
| `i32` `i64` `u8` `u16` `u32` `u64` `usize` `isize` | Copy | integers, all widths distinct |
| `f32` `f64` | Copy | IEEE floats |
| `bool` | Copy | `true` / `false` |
| `char` | Copy | one Unicode scalar value, written `'x'` (2.4). A code point, not a byte: it is not an integer and does not mix with one |
| `str` | borrowed | NUL-terminated UTF-8 byte view, points into storage owned elsewhere. No length: deriving one costs a `strlen` |
| `View` | borrowed | a **length-carrying** borrowed text view — a pointer *and* a length. Slices without allocating, scans without a `strlen` (11.5) |
| `String` | owned | UTF-8 text, validated on construction, freed at scope exit |
| `Bytes` | owned | raw byte buffer, freed at scope exit |
| `List<T>` | owned | growable sequence, one instantiation per element type |
| `Map<K, V>` | owned | keyed container, insertion order; `Map<V>` is a `String` key (12.2) |
| `Box<T>` | owned | one heap cell that holds a `T` or nothing |
| `[T; N]` | Copy / owned | fixed-size array: `N` elements held **inline**, with nothing allocated. `Copy` when `T` is, and an owner that drops each element when it is not (12.4) |
| `datac` name | owned | record with per-field `val`/`var`, optionally with type parameters (9.3) |
| `enum` name | owned | tagged union: one variant is live, and patterns match on it (9.2), optionally with type parameters (9.3) |
| `fun(T, U).R: V` | Copy | a function value: a function pointer, with the parameters it takes and what it returns (7.4) |
| `closure(T, U).R: V` | owned | a *closure*: a function value plus the environment its captures were moved into (7.4). What a `fun` literal becomes when it names a binding from the enclosing scope |
| `Unit` | Copy | absent value, the implicit return type |

Rules that follow from the table:

- There is no implicit numeric conversion, not even between two integer widths. `u64 % 10` produces a `u64` and cannot be pushed into a `u8` slot without `to_u8()`. The one widening that goes to a float is explicit too: `n.to_f64()` (5.0).
- Owned types are not `Copy`: assigning one moves it, and every owned value is dropped at the end of the scope that owns it, including inside `datac` fields, `List` elements and `Map` values.
- `str` **and `View`** are borrows, so neither can live in a `datac` field, an `enum` payload, a `List` element, a `Map` value, an array element or a `Box`; only owned types can. A borrow in a field would outlive the value it points at, and v1 has no lifetime syntax to say otherwise (17). An owned `String` is what to store instead.
- No `List` or `Map` may hold another `List` or `Map` directly, because elements are stored inline. Put it in a `Box`.
- `[T; N]` holds its elements inline too, and it is the one container that may hold another *array*: `[[u8; 4]; 8]` is a page table. It may not hold a `List`, a `Map` or a `Box` (12.4).
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
| 5 | `\|` (bitwise or) | left |
| 6 | `^` (bitwise xor) | left |
| 7 | `&` (bitwise and) | left |
| 8 | `<<` `>>` | left |
| 9 | `+` `-` | left |
| 10 | `*` `/` `%` | left |
| 11 | unary `-` `!` `~` `&` `&mut` `*` | prefix |
| 12 | `.field`, `.method(...)`, `f(...)`, `expr?` | postfix |
| 13 | `=` `+=` `-=` `*=` `/=` `%=` | right |

```kelp
val m = (a + b) * 2
val ok = n > 0 && n < 10 || force
val neg = -n
val r = &c                       // shared borrow of a local
val e = &mut c                   // exclusive borrow of a local
val field = (*r).count           // dereference, then field access
val masked = flags & 3           // bitwise-and, not a borrow
val high = byte >> 4
val low = byte & 15
val flipped = ~byte
```

**The bitwise operators bind tighter than the comparisons, unlike C.** That is deliberate, and it is the one place this table differs from C's:

```kelp
if flags & MASK == 0 { }        // this language: (flags & MASK) == 0
                                // C: flags & (MASK == 0) — the classic bug
```

`|` stays looser than `^`, which stays looser than `&`, so masking expressions still read the way C programmers expect (`a | b & c` is `a | (b & c)`); only the comparison boundary moved.

Facts worth knowing:

- Integer `/` truncates toward zero and `%` follows the dividend's sign, as in C. Division by zero is undefined rather than checked.
- Integer `+`, `-` and `*` overflow is not checked.
- Bitwise `& | ^ << >> ~` require two integers **of the same type** (or one, for `~`) and produce that type. `bool` is not a bitwise domain: `a & b` on two booleans reads as a non-short-circuiting `and`, and `&&` is right there. A shift count is the same type as the value, as every other binary operator requires; a shift by ≥ the width is undefined, exactly as in C. `>>` on a signed type is arithmetic (sign-propagating), and `~x` is `0 - x - 1`, so on a `u32` it flips all 32 bits — mask it (`~x & 255`) when you wanted a byte.
- Float arithmetic is IEEE, so `nan`, `inf` and `-0.0` behave as usual. `math.is_nan` and `math.is_finite` exist.
- `==` and `!=` work for scalars. Comparing two `str` values is refused (it would compare addresses); use `str.eq`. Comparing `View` values is refused too — use `view.eq(other)`, which compares the bytes and the lengths. Owned types have no structural equality.
- `if` and `when` are expressions (see 7), so they can appear on the right of `=` and inside a call.
- A block `{ ... }` is an expression whose value is its last expression when used where a value is expected.
- Every integer type has `to_<type>()` for every other integer type, and `to_f64()`/`to_f32()` to the float types. Widening is exact; narrowing panics when the value does not fit, naming the call: `panic: i32.to_u64: value out of range`. `to_f32` rounds (it is the one conversion allowed to lose value without a check, because C defines it for every input). A float does **not** have `to_i64`: that conversion loses the fraction, so write `math.trunc`/`math.floor` first.
- `print` is the one builtin call that accepts a template; see 12.

#### 5.0 Numeric conversion, in full

Operands must match exactly, so the language never converts a number for you — but it does give you the conversion to write:

```kelp
val n: i64 = 7
val x: f64 = n.to_f64()        // 7.0 — exact for every i64
val y = n.to_f64() / 2.0       // 3.5
val z = n.to_f32()             // rounds to the nearest f32

val b: u8 = 200
val idx: usize = b.to_usize()  // a byte used as an index
val back: u8 = idx.to_u8()     // panics if it does not fit
```

`n * 1.0` is a **type error**, not a conversion. There is exactly one way to widen an integer to a float, and it is `to_f64()`.

#### 5.1 The one implicit conversion: `str`/`View` -> `String`

A string literal is a borrowed `str`. An owned field, element or binding is a `String`. Where a `String` is expected, a `str` or a `View` is converted for you by copying it:

```kelp
datac Doc(val title: String, val body: String)

val d = Doc(title = "a title", body = "text")   // no String.from_str
val s: String = "a literal"                      // the written type converts it
var acc = String.empty()
acc = "assigned"                                 // an assignment converts it

fun shout(s: String).R: String = s               // a return converts it
var xs: List<String> = List.empty()
xs.push("pushed")                                // an element converts it
var m: Map<String, String> = Map.new()
m.insert(key = "k", v = "v")                     // a key and a value convert

val picked: String = if flag { "yes" } else { other_str }
val both: String = when o { Some(x) -> x else -> "none" }
```

What *does* convert, and what does not:

- It fires on an **exact `String` expectation**, and nowhere else. `str.len(s)`, `str.eq(a, b)`, `fs.write_text(path, text)` and every other `str` parameter keep taking a borrowed view with no allocation.
- A `str` or a `View` each copy the right number of bytes: a `View` may be a *slice* with no NUL after it, so it is copied by its length (`View.to_string`), never with `strlen`.
- **Nothing converts the other way.** `String` -> `str` is a borrow with a lifetime, so it is written out: `s.as_str()`. `str` -> `View` and `String` -> `View` are written `s.view()`. `print(x)` (22.4) accepting a `String` directly is not an exception to this — `print` is its own builtin that takes a scalar, `str`, `String`, `Bytes` or `View` outright, not a call whose parameter converts; a declared function's `str` parameter still needs `.as_str()` from a `String` argument, the same as `str.concat`, `str.eq`, or anything else in the stdlib.
- A `View` is not a `str`: a slice has no NUL after it, so passing one where a `str` is expected is an error rather than a silent `strlen` past the end. Take `v.to_string()` when you need an owned `String`, or keep the `View` and use the length-aware operations (11.5).

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
} elif temp > 20 {
    print("warm")
} else {
    print("cool")
}
```

Braces are always required. Parentheses around the condition are neither needed nor the house style. An `if` used as a value must have an `else`; an `if` without an `else` must produce `Unit`, and the compiler says so: `if without an else branch must evaluate to 'unit'`.

#### `elif` and `else if`

The link between one condition and the next has two spellings, and they mean exactly the same thing:

```kelp
if n > 2 {
    print("big")
} elif n > 1 {
    print("medium")
} elif n > 0 {
    print("small")
} else {
    print("none")
}
```

- **`elif` is the preferred spelling**, and `else if` is what `kelpc lint` warns about (a cosmetic warning: findings on stderr, exit 0, and nothing written). `else if` stays legal, compiles identically, and is never rewritten by `kelpc fmt`, which is token-faithful; switching spelling is the author's edit.
- A chain may mix them. `if a { .. } else if b { .. } elif c { .. } else { .. }` is one chain of four links, and the parser reads each link the same way.
- `elif` takes the same grammar as `if`: a condition and a block, or a pattern and a block (`elif let`). `if let A(n) = x { .. } elif let B(m) = x { .. } else { .. }` is the pattern form of the chain above, and its links read the same scrutinee once each (see `if let` below).
- `elif` is one token, so `else elif` is not a chain: after an `else` the grammar expects a block or an `if`.

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
} elif let Circle(r) = shape {
    print(3.14159 * r * r)
} else {
    print(0.0)
}

// `shape` is read by both links above and is still owned here: matching it
// only ever copies or borrows the matched *field*, never the whole value, so
// reading the same scrutinee more than once across a chain is exactly as
// legal as a bare `when shape { ... }` reading it once and testing it
// repeatedly.
if let Circle(r) = shape {
    print(r)
}
```

- It is exactly the pattern form of `when` (6.3) with a single arm and an `else`, which is why the arm rules are the same: the variant must exist, the number of bindings must match the payload, `_` ignores one, and a payload binds as a copy when it is `Copy`, by value when it is owned and the scrutinee is an owned place, and as a shared borrow otherwise (9.2).
- The scrutinee may be **any expression**, temporary or place. A *place* (a name, a `.field` chain, `*expr`) is matched directly, so the place itself is not consumed: only the payload that was read out by value is gone from it (9.2). A *temporary* (a call's result, a literal) is bound to a local first, since `when`'s pattern form needs a place to read the payload out of; that local is a fresh binding no other code can see, so it never collides with anything and there is nothing to reuse it across links of a chain. A chain over a temporary stages its **root**: `if let Circle(r) = mk().shape` binds `mk()` and matches its `shape`, so the field is projected out of a place and the whole temporary, not the field alone, is freed when the block ends.
- `elif let` and `else if let` chains are both allowed, and a plain `else if`/`elif` link may sit in the same chain; without any `else` the whole expression is `Unit`, the same as an `if` without one. Chaining on the *same place* (`if let A = x { .. } else if let B = x { .. }`) is legal for the reason above: a link that binds nothing reads only the tag, so `x` is not consumed and the next link's own read of it is not a use of a moved value. A link that binds an owned payload by value does take it out, so that payload cannot be bound a second time (9.2).
- `let` is an ordinary identifier everywhere else in the language. It is read as the pattern form only directly after `if`, so a binding named `let` cannot be the condition of an `if` without parentheses.

### 6.2 `for` and `while`

`for` walks a `List` front to back:

```kelp
for x in xs {
    print!(x)
    print!(" ")
}
```

- The iterable must be a `List<T>` or a `[T; N]` (12.4). `for` is **desugared by the parser** into the index-and-`while` walk below, so anything with `.len()` and `.get(i)` walks the same way; no later pass knows it exists, and there is no protocol to implement (`iter`/`next` is a post-v1 question).
- The element binding gives exactly what `List.get` gives (12.1): a copy of a `Copy` element, and a shared borrow of an owned one. So `for n in numbers` reads `n: i32` directly, while `for name in names` reads `name: &String` and a `str.*` call wants `name.as_str()` (the same wart as 12.1).
- The iterable is *borrowed*, not moved, when it is a place (`xs`, `self.items`): `for x in xs` inside a function that takes `xs: List<i32>` is the ordinary case. Anything else — a call, a literal — is bound to a temporary that the loop owns and frees, so it is evaluated **once**.
- The loop variable is a `val`. Copy it into a `var` to mutate it, and note that the list is still borrowed for the duration of the body, so the body cannot `push` to what it walks.
- There is no `break` and no `continue`; the body runs from the first element to the last.

Walking a `List` by index is still available, and is what `for` becomes:

```kelp
var i: usize = 0
while i < xs.len() {
    print!(xs.get(i))
    i += 1
}
```

`while` is the only loop besides `for`, and it is the general one — a condition, re-evaluated before each pass. `str.split`, `Map.keys()` and `Map.values()` hand back `List`s so iteration over text and over maps reads the same way as over a list.

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

- The scrutinee is a **place** when it is one (a name, a `.field` chain, `*expr`): it is matched directly, with nothing moved out of it as a whole, and only the matched projection is ever copied, borrowed or (for an owned payload of an owned place) moved. A **temporary** (a call's result, a literal) is staged into a local of its own first, because a payload borrow has to outlive the arm it is bound in and there is nothing to outlive if the scrutinee does not survive the match, and that staged local is an owned place like any other; the staged local is freed at the end of the match. `if let` (6.1) stages its one scrutinee the same way. A chain over a temporary stages its **root**: `when mk().t { ... }` binds `mk()` and matches its `t`, never the field alone, so the whole temporary is freed exactly once.
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

A block whose **last statement is `panic(...)`** diverges, so it satisfies any declared return type — including a borrowed one, where it matters twice over: the `panic` is not the returned value, so the elision rule has nothing to say about it.

```kelp
fun unwrap<T>(o: &Option<T>).R: T {
    if let Some(v) = o {
        return v
    }
    panic("Option.unwrap: value is None")
}

fun as_str(v: &Json).R: &str {
    if let Str(s) = v {
        return s.as_str()
    }
    panic("json.as_str: value is not a string")
}
```

This is the shape every accessor over an owned payload ends with, and it is the one deliberate piece of divergence analysis in the language (an `if` arm cannot be a `panic`).

The message is a `str`, or an owned `String` read without being consumed: `panic(built_up_string)` and `panic(fmt.int(n))` both work, so an error path does not have to bind a `val` or spell `.as_str()` first.

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

Inside the body a capture is read *through a borrow of the environment*, so an owned capture can be used in place — a method called on it, a field read, passed to a parameter — but cannot be handed out by value. That is a move out of a place this frame does not own, which 9 refuses, and the message says so.

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
- **Capture a field of the receiver it was written in.** Reading a field out of a borrowed receiver by value is a move out of a borrow, refused (9): bind a `Copy` field to a local first, or take the whole receiver as a `move` parameter.
- **Be a `List` or `Map` element.** A closure owns its environment and those elements are held inline; a `Box<closure(...)>` (the optional-callback shape) or a one-field `datac` holds it instead.
- **Be borrowed with `&`.** A closure is an owned value, so a parameter of its type already borrows it and `move closure(...)` takes it over.

A literal nested in another literal can copy a `Copy` capture out of the enclosing closure's environment. An owned one cannot be *moved* out of it: the environment is not a local of this frame, and a partial move needs a local the frame owns.

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

Reading an owned field *by value* out of an owned place **moves the field out**: `val s = node.text` takes the field's value and clears the slot it held, so `node` still owns the fields that stayed, and its end-of-scope drop frees those and nothing where the moved value now lives. The owned place is a `val`/`var` binding or a `move` parameter, with the field read directly off it. Reading the moved field again, moving the same field twice, and using `node` as a whole are each an error; reading the field by value out of a *borrow* is refused too, because the borrowed value still owns it. Take it through a borrow (pass it to a parameter, call a method on it), or hold it in a `Box` and `take()` it out. An owned `enum` payload read by value obeys the same rule, which is what a `when` arm binds by value (9.2).

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

- A payload binds as a copy when it is `Copy`, and **by value** when it is owned and the scrutinee is an owned place (a `val`/`var` binding or a `move` parameter): the binding then owns it and the read is the partial move 9.1 states, so the enum's own drop frees the payloads the move left behind. A payload still binds as a shared borrow when the scrutinee is a borrow, so matching one moves nothing. `r` above is an `f64`; a `Word(val text: String)` payload binds as an owned `String` out of an owned `Word`, and as `&String` out of a `&Word`.
- The scrutinee is matched where it is when it is a place (a binding, a field path, or a dereference): `when s { ... }` reads through `s` and moves nothing out of it, and a temporary scrutinee (a call's result, a field of one, a literal) is staged into a local first, because the arm reads the payload out of that local, and a borrow of a temporary would not outlive the arm's body. A chain over a temporary stages its root, so `when mk().t { ... }` frees the whole `mk()` result once.
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

- A borrow lives until the end of its scope, not until its last use — with one statement-sized exception: a borrow taken **inside an initializer** ends with that statement, because the bound value does not borrow it. `val back = str.parse_i64(cand.as_str())` borrows `cand` for the length of one call and keeps nothing of it, so `return cand` a few lines later is fine. A binding that *is* the borrow (`val v = l.get(0)`) still pins its owner for as long as `v` lives.
- While a value is borrowed exclusively, nothing else may borrow it and the owner may not be assigned or moved.
- While a value is borrowed shared, it cannot be borrowed exclusively, and it cannot be assigned or moved — but a **`Copy`** value can always be read, because reading one is a copy rather than a move. `self` inside `fun mut T.m()` is a `&mut T`: a pointer, and `Copy`, so naming a field and then calling another method on `self` is legal.
- A method call's receiver is the one borrow that starts **after** its own arguments are checked, so an argument may read the place the receiver is about to borrow exclusively: `counts.insert(key = k, v = counts.get_or(key = k, fallback = 0) + 1)` is one statement, and was two with a temporary (MISSING-n4Bz7). The deferral permits reading and shared-borrowing the receiver. Moving it, borrowing it exclusively again inside the argument list, or holding a shared borrow of it that is still live when the receiver's own borrow begins (`p.copy_from(p)`) is still refused.
- Dereferencing a `&mut` borrow gives write access; dereferencing a `&` does not.
- A function whose `.R: T` is a borrow must return something derived from its first reference parameter, and **a payload bound by a pattern counts as derived from the scrutinee it came out of**: `if let Str(s) = v { return s.as_str() }` returns a view of `v`, exactly as `v.x.as_str()` does. One caveat, from the same rule: when a whole `when` is the returned value, the arm bindings are out of scope before the merged root is computed, so return *from inside the arm* (`if let S(s) = v { return s.as_str() }`) rather than returning the `when`.
- Arms that differ only in the `str`/`&str` spelling are one type, so `when v { S(s) -> s.as_str()  N -> "n" }` and `if c { name(v) } else { "none" }` type-check; a branch whose value is a `str` literal has static storage, so it needs no owner pinned either.
- A borrow cannot be stored: no reference fields in a `datac`, no references as `List` elements or `Map` values.
- A call that returns a borrow of its receiver needs the receiver to be a variable or a field, not a temporary:

```kelp
val text = String.from_str("hi")
val view = text.as_str()          // ok
print(str.len(text.as_str()))     // ok: the temporary lives for the call
val bad = String.from_str("hi").as_str()   // error: borrow of a temporary
```

The last line is the chained form of the same rule, and the chain is not a second
case: a call result is one temporary for the whole expression, so
`fmt.int(n).as_str()` and `t.labels_text().as_str()` are refused exactly as
`String.from_str("hi").as_str()` is, and the error names the borrow rather than
the chain. Binding the call to a `val` first is the fix, or leaving the
`.as_str()` inside the enclosing call (`print(str.len(...))`), where the
temporary lives for that call.

A **field of a temporary is the same case one projection deeper**: `mkName().name`
is a field of an rvalue, so it has no storage of its own and the temporary it
lives in is freed at the end of the statement. The compiler stages that
temporary for the length of one call, and frees it after the call rather than
freeing the field alone. That is what makes a field of a returned `datac`
usable wherever the borrow does not outlive the call:

```kelp
print(mkName().name.len())      // ok: staged for the call, freed after it
take(mkName().name)             // ok: the parameter borrows, for the call
val n = mkName().name.len()     // ok: the borrow ends with the call
val r = &mkName().name          // error: cannot borrow a temporary value
val v = mkName().name.as_str()  // error: the returned borrow outlives it
```

A chain that steps through a borrow is not a temporary case at all:
`headers.get(i).name` lives in whatever `get` pointed at, so its address is
durable and `&headers.get(i).name` needs no staging.

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
print(fmt.pad_left("7", 3, '0'))    // fill byte '0'

val parsed = str.parse_i64("41")    // Result<i64, str.ParseErr>
if let Ok(v) = parsed {
    print(v + 1)
}

val big: i64 = 3
val x: f64 = big.to_f64()           // 3.0 — the only integer->float widening
```

- `to_<T>()` exists on every integer type for every integer type, and `to_f64()`/`to_f32()` widen to a float. Widening is exact; narrowing panics when the value does not fit. `to_f32` rounds to nearest, and is the one lossy conversion that does not panic (C defines it for every input). A float has no `to_<int>`: that would silently drop the fraction.
- Integer conversion is the only way to change width. There is no implicit promotion, so `u8` to `usize` for an index needs `n.to_usize()`, and `n * 1.0` is a type error rather than a conversion.
- `str.from_i64`, `str.from_u64`, `str.from_f64(x, decimals)` return `String`; `fmt.int`, `fmt.uint`, `fmt.float`, `fmt.hex`, `fmt.bytes`, `fmt.pad_left`, `fmt.pad_right` wrap them for printing.
- `str.parse_i64`, `str.parse_u64`, `str.parse_f64` return `Result<T, str.ParseErr>` where `ParseErr` is `Empty` or `Invalid`. Unpack with `?`, `if let Ok(v) = ...`, or `result.is_ok(...)`; `str.parse_i64(x)` on `"nul"` is an `Err`, not a zero.
- `fmt.pad_left`/`pad_right` take a *fill byte*, so write it as a character literal (`'0'`) rather than `48`.

### 11.5 `View` — a slice of text that costs nothing

`str` is a bare `const char*`. It has no length, which is cheap for a whole string and expensive for anything that walks one:

- every scan re-derives a length with `str.len` (a `strlen`, once — but the length is then yours to thread through every function);
- every slice allocates (`str.copy_range(s, from, to).R: String`), even when the caller only wants to look at the bytes;
- a `datac` cannot hold a `str` at all, so text you are part-way through cannot be carried in your own type.

A `View` is the same borrowed text **with its length attached**: a pointer and a count, 16 bytes, `Copy`.

```kelp
val text = "the quick brown fox"
val v = text.view()             // one strlen, once

print(v)                        // prints with its length, not with %s
print(v.len())                  // 19
print(v.is_empty())             // false
print(v.get(0))                 // 116  ('t'; an index out of bounds panics)
print(v.slice(4, 9))            // "quick" — a view, no String allocated
print(v.slice(1, v.len()))      // "he quick brown fox"
print(v.eq("quick".view()))     // byte-wise, length-aware
print(v.slice(0, 3).eq("the".view()))

val owned: String = v.slice(4, 9)   // a String where one is expected copies by length
print(v.to_string().len())          // explicit, same copy
```

| Call | Returns | Notes |
| --- | --- | --- |
| `s.view()` | `View` | a `str` value. One `strlen` — the cost this type exists to stop paying |
| `string.view()` | `View` | an owned `String` receiver. **No** `strlen`: the length is already there |
| `v.len()` | `usize` | the byte count |
| `v.is_empty()` | `bool` | `v.len() == 0` |
| `v.get(i)` | `u8` | the byte at `i`. Panics out of range: `View.get: index out of bounds` |
| `v.slice(from, to)` | `View` | `from` and `to` are byte offsets, `to` exclusive. Panics unless `from <= to` and `to <= v.len()` |
| `v.eq(other)` | `bool` | byte-wise and length-aware, so `"ab"` is not `"abc"` |
| `v.to_string()` | `String` | the length-aware copy, the safe way to make owned text of a slice |

**Every operation is length-aware, and there is no implicit conversion to `str`.** That is the point, not an omission: a view is usually a *slice*, and a slice has no NUL after it, so `strlen`/`strcmp`/`%s` on one would read into whatever follows and report a different string than the program holds. `print(v)` and `%.*s` use the length; `v.to_string()` copies the length; `v.eq` compares the length. Passing a `View` where a `str` is expected is a compile error, because the language will not silently do the wrong thing.

A `View` **borrows**, so it obeys `str`'s rules exactly:

```kelp
// A `View` parameter is a reference parameter, so a function may return a
// view derived from one — the same elision rule an `&str` return follows.
fun head(text: View, n: usize).R: View = text.slice(0, n)
fun tail(text: View).R: View = text.slice(1, text.len())

// A view pins its owner: `v` borrows `text`, so `text` cannot be moved or
// freed while `v` lives.
val v = head(text, 3)
print(v)
```

What is refused, and the reason each time:

- **A `View` field in a `datac` or an `enum` payload.** A view points at text somebody else owns, and the field outlives the borrow; v1 has no lifetime syntax to tie the two (17). Store an owned `String` and take a view when you need one.
- **`List<View>` / `Box<View>` / a `Map` key.** The container owns its elements and outlives the text; same reason as the field.
- **Returning a view of a local.** A function's declared return type borrows from its first reference parameter (the elision rule, 7.1) — `R: View` means "a view of one of my parameters", and anything else is refused by name.
- **A view of a temporary.** `String.from_str("x").view()` is refused: the temporary is freed before the view could be used. Bind it first (`val s = String.from_str("x")`), or take a view of the literal itself (`"x".view()`, whose storage is static).
- **Capturing a `View` in a closure.** The environment outlives the frame the borrow points into. Move an owned `String` in instead.
- **`==` on two views.** The operator would compare pointers; `v.eq(w)` compares the bytes.

Using a `View` for a scan looks like this — no `strlen`, no allocation per step, and the bounds come along:

```kelp
// Count the bytes equal to `want`, without threading a length parameter.
fun count_byte(text: View, want: u8).R: usize {
    var i: usize = 0
    var n: usize = 0
    while i < text.len() {
        if text.get(i) == want {
            n += 1
        }
        i += 1
    }
    return n
}
```

When a whole `str` really is what the caller has, take the `&str` at the boundary and call `.view()` once inside:

```kelp
fun parse(text: &str).R: Result<Json, ParseError> {
    var pos: usize = 0
    val src = text.view()           // the only strlen
    return parse_value(text = src, pos = pos)   // `src` travels, not a copy
}
```

`pos` is a `var` because a `mut T` parameter borrows it exclusively; the call site writes the name and nothing else (`pos = pos`, never `mut pos`).

## 12. Collections

### 12.1 `List<T>`

```kelp
var xs: List<i32> = List.empty()      // element type from the annotation
var zs = List.empty()                 // element type from the first `push`
zs.push(1)                            // ... which is this one
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
| `List.empty()` | nothing | empty list, element type from the context or from the first `push`/`insert`/`set` |
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
| `clone()` | shared | a copy of the whole list, for clonable elements |

`get` and `pop` move an owned element out and return a copy for a `Copy` element, so `List<i32>.get(i)` is an `i32` while `List<String>.get(i)` is a `&String`. Out-of-range indices and popping an empty list panic.

The element type may be a scalar, `String`, `Bytes` or a `datac`. It may not be a borrow, a `str`, `Unit`, a `List` or a `Map`; put a container in a `Box` instead. `sort`, `contains`, `slice`, `clone` and `Map.values` are offered only for element types that support them.

A `datac`/`enum` element takes a **derived deep copy and a derived order from its fields**. It is clonable (`clone`/`slice`) when every field it holds is, and sortable when every field has an order, so `datac Digest(val path: String, val size: u64)` gives `List<Digest>` both. The order is field by field in declaration order: a `String`/`Bytes` byte-wise (length-aware), an `enum` by tag and then by the live variant's payload. A `List`, `Box`, `Map` or owned-array field has neither, so a record that holds one is not clonable or sortable. `contains` still needs equality, which a `datac` does not have, and `Map`'s cloning operations (`get_or`, `values`) are still scalar-only.

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
| `Map.new()` / `Map.empty()` | nothing | empty map, key and value types from the binding or from the first `insert` |
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

### 12.4 `[T; N]`

A fixed-size array: `N` elements of `T`, held inline. Nothing is allocated, and the length is part of the type, so `[u8; 4]` and `[u8; 8]` are different types. It is the type for a buffer whose size is known at compile time (a page table, an IDT/GDT, a register-save frame), and it is the reason those can exist with no allocator in the path at all.

```kelp
const PAGE_SIZE: usize = 4096
const ENTRY_SIZE: usize = 8
const ENTRIES: usize = PAGE_SIZE / ENTRY_SIZE

fun main() {
    val xs: [i32; 3] = [10, 20, 30]
    print(xs.len())                 // 3, from the type: nothing is loaded
    print(xs.get(i = 1))

    var buf: [u8; 4] = [0; 4]       // the repeat form: a zeroed buffer
    buf.set(i = 2, v = 7)

    val table: [u64; ENTRIES] = [0; ENTRIES]
    val grid: [[u8; 2]; 3] = [[1, 2], [3, 4], [5, 6]]
    print(grid.get(i = 2).get(i = 1))
}
```

| Method | Takes | Returns |
| --- | --- | --- |
| `len()` | shared | `usize`, as a compile-time constant |
| `is_empty()` | shared | `bool`, always `false` |
| `get(i)` | shared, `usize` | the element, or a borrow of it |
| `set(i, v)` | exclusive, `usize` | `Unit` |

- **The length is a compile-time constant**, written as a literal, another `const`, or an expression over them (A-19): `[u64; PAGE_SIZE / ENTRY_SIZE]`. A `val` computed at runtime is refused, however obviously constant its initializer looks.
- **The element type may be a scalar, `String`, `Bytes`, a `datac`, an `enum`, or another array.** It may not be a borrow (a `str`, a `View` or a `&T`), a `Unit`, or a `List`/`Map`/`Box`, for the reason 12.1 gives: an element is held inline and owned by the array.
- **`Copy` when the element is.** `[i32; 3]` is a value you can assign freely; `[String; 3]` owns its three strings, so assigning it moves it and dropping it frees each element. `set` drops the element it replaces.
- **Both literals.** `[a, b, c]` takes the length from the number of elements, and its type is the one the context declares or, with no declaration, `[T; n]` for the type its elements agree on. `[x; N]` fills `N` slots with `x`, and copied means `x` has to **own nothing**: a scalar, or a `datac` whose fields are all scalars. A `String` element is refused by name, because `[s; 4]` would be four owners of one buffer.
- **No `[T; 0]`.** A zero-length array would be a C array with no elements, which is not standard C; there is no empty form.
- **Indexing is `get`/`set`, not `a[i]`.** The language has no `[]` subscript anywhere, and `a[i]` on an array is the same as on a `List` (12.1): a method call.
- **There is no `delete`/`insert`/`push`.** The length is the type, so an array cannot grow; a growable sequence is a `List<T>`.
- **`for` walks one**, exactly as it walks a `List`: the desugar is `.len()` and `.get(i)`, and both exist here (6.2).
- A `datac` field may be one (`val entries: [Entry; 512]`), which is the shape a kernel's IDT or GDT has, and `Box<[T; N]>` holds one on the heap when it has to outlive its frame.

A parameter follows the rule every other type follows (22.1): an array whose element owns something is an owner, so a plain parameter **borrows** it and the caller keeps its array, exactly as a `String` parameter does. A `Copy` array (`[i32; 3]`) is passed by value like a scalar, `mut [T; N]` is the writable form, and `move [T; N]` takes the value over. An array returned from a function is copied out by value.

## 13. Modules and imports

### 13.1 `impl`

Every file is a module, and a module starts by importing what it uses:

```kelp
impl "shapes/circle"
impl "std/fs" caps(fs = ["/var/log/myapp/*"])
impl "std/str"
impl "std/time"
```

- The path is a string, relative to `src/` for project modules and to the shipped standard library for `std/...`. The `.kp` extension is not written.
- `impl` lines come first in the file.
- The namespace is the module's own name: the last path segment, so `impl "shapes/circle"` gives `circle.area(...)` and `impl "std/fs"` gives `fs.read(...)`. A **dependency's entry module** is named after the directory Coral fetched it into: a path ending in `/lib` under `deps/` takes the name of its folder, which is the `[dependencies]` key (`impl "deps/argparse/lib"` is `argparse.parse(...)`) or, for a whole-repository dependency, the library's own folder inside it (`impl "deps/fpl/json/lib"` is `json.write(...)`). Everything else under `deps/` is an internal module with its own name. `as name`, which used to override this, is **removed**: `impl "std/path" as p` is a parse error naming the fix, and the name to change is the one at the call sites.
- A capability grant is written in the same clause: `impl "std/fs" caps(fs = ["/var/log/myapp/*"])` (15.2). The `with` that used to precede `caps` is **removed** as well, and the grant under it is unchanged: `impl "std/fs" with caps(...)` is a parse error that names the surviving spelling (15.2, 23).
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
const PAGE_SIZE: usize = 4096
const ENTRY_SIZE: usize = 8
const ENTRIES_PER_PAGE: usize = PAGE_SIZE / ENTRY_SIZE

fun entries_for(pages: usize).R: usize {
    var total: usize = 0
    var i: usize = 0
    while i < pages {
        total += ENTRIES_PER_PAGE
        i += 1
    }
    total
}

const TABLE_ENTRIES: usize = entries_for(pages = 64)
```

A `const` is a top-level compile-time value. The type is mandatory, the initializer is **evaluated while the compiler runs**, and each use is substituted as a literal, so there is no storage and no initialization at startup.

What an initializer may be:

- a literal, including a character, a byte and a `str` literal;
- another `const`, written bare or as `module.NAME`, **declared before it**;
- the operators: arithmetic, comparison, `&&`/`||`, the bitwise operators and the unary forms, each at the width of the declared type;
- an integer conversion (`n.to_usize()`), with the same out-of-range check the runtime performs;
- `if`/`else`, including one nested in another;
- **a call to a plain `fun`** whose own body stays inside this list: parameters, `val`/`var` locals, `if`, `while`, `return`, arithmetic, and further calls. The call is evaluated like any other: named and positional arguments bind to the parameters, and a recursive call works as long as it terminates. The function does not have to be declared before the `const` that calls it.

What it may not be: a method call on a value, a builtin (`print`, `panic`, `assert`), an `extern fun`, a generic function, an array or a `datac` value, a `when`, a `?`, a borrow or a template. Each is refused by name rather than folded to something that is not the program's value.

The result has to be a scalar (an integer, a float, a `bool`, a `char` or a `str`), because a use site is a literal. A `const` whose declared type is anything else is refused.

Two budgets keep evaluation bounded, because it is the compiler doing the running: one million evaluation steps, and sixty-four nested calls. Exceeding either is a diagnostic naming the call, not a compiler that hangs.

The arithmetic is the declared type's, so a `const` folds the way the generated code would compute: `u8` arithmetic wraps at 256, `i64` at its two's-complement range, `>>` on a signed type is arithmetic, and a shift by the width or more is refused rather than folded to whatever this machine would do. Division by zero is a diagnostic.

An array's length is the same evaluated expression (12.4), which is what makes `[u64; PAGE_SIZE / ENTRY_SIZE]` and `[Gate; entries_for(pages = 64)]` one type each rather than a number written out by hand.

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
- The reference is a borrow, and the borrow's mutability is what the function does with the passdown: a function that writes it — directly, or through a function it calls — holds an exclusive borrow, and one that only reads it holds a shared one. `pd var` is therefore writable from inside functions, two functions cannot hold the same `pd var` exclusively at once, and a `pd var` no function writes is a shared borrow everywhere, so passing it to a function that also reads it is allowed.
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
impl "std/http" caps(net = ["api.example.com:443", "127.0.0.1:*"])
impl "logger" caps(fs = ["/var/log/myapp/**"], env = ["LOG_*"])
```

- The entry module holds the wildcard for every domain (`/**` for path domains, `*` for name domains), so a single-file program needs no grant for its own calls.
- Every other module holds exactly what its importers grant it, and nothing else.
- A module can delegate less than it holds, never more. Granting a pattern the importer does not hold is an error.
- Referencing a gated function without holding the domain is a compile error naming the missing domain and the grant to write.
- `net` patterns are `host`, `host:port`, `*.host`, `*:port` or `*`, matched as names, never as resolved addresses. `127.0.0.1:*` does not cover `localhost`.
- `fs` and `process` patterns are absolute paths, normalized lexically and resolved through symlinks before matching, so a symlink inside the grant that points outside it is denied.
- An empty pattern list denies everything: `caps(process = [])` is a grant that permits no program.
- The clause stands on its own: `impl "std/fs" caps(fs = ["/var/log/**"])` is the whole grant. `with caps(...)` was the older spelling of the same grant, and it is **removed**, not deprecated (`23`): `impl "std/fs" with caps(...)` is a parse error that names the surviving spelling, and the domains, the patterns and the no-escalation rule are exactly the same without the keyword. `impl` already means "this module declares what it imports", so the keyword said nothing the clause does not.

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

`str.len` and `str.byte` are byte-oriented; `str.char_count` and `str.char_at` are the codepoint-oriented pair. `str.substring` returns a view into the original, so it borrows its argument. `str.to_upper`/`to_lower`/`concat`/`trim`/`copy_range` allocate — the `View` operations (11.5) are the ones that do not.

A `str` **value** has one method of its own, `s.view()`, which makes a length-carrying `View` of it; an owned `String` has `s.view()` too, and that one needs no `strlen`. See 11.5.

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
fun unwrap_or<T>(o: move Option<T>, fallback: move T).R: T
fun unwrap<T>(o: move Option<T>).R: T               // panics on None
fun expect<T>(o: move Option<T>, message: &str).R: T   // panics with `message`

fun map<T, U>(o: &Option<T>, f: closure(&T).R: U).R: Option<U>
fun and_then<T, U>(o: &Option<T>, f: closure(&T).R: Option<U>).R: Option<U>
fun or_else<T>(o: &Option<T>, f: fun().R: Option<T>).R: Option<T>   // `T` must be Copy
fun each<T>(o: &Option<T>, f: closure(&T))
```

The enum is the point: `Some`/`None` is one of the two shapes `?` (5) and `if let` (6.1) understand, so a function that can fail one way returns `Option<T>` and its caller writes `val x = f(...)?`. Nothing in the compiler knows this module exists — the *shape* is the contract, so a program's own `Option` works identically.

The **callbacks borrow the payload** (`closure(&T).R: U`, so a plain function value is widened into one), which is what makes the combinators work at every instantiation: a `when` arm copies a `Copy` payload and auto-borrows it at the call, and an owned one is auto-borrowed out of the owned option the helper took with `move`, so the callback still never takes ownership:

```kelp
val text = option.Option.Some(value = String.from_str("hello"))
val len = option.map(o = text, f = fun(s: &String).R: usize = s.len())
val shout = option.map(o = text, f = fun(s: &String).R = str.to_upper(s.as_str()))
```

What hands the payload *out by value* takes the option with **`move`**: `unwrap`, `expect` and `unwrap_or` read the payload of the owned option, which is a partial move of the local the caller gave up, so `Option<String>.unwrap()` works. The move form is the caller's contract too: the option is consumed, so a caller that wants it afterwards passes a fresh expression, or matches it with `when`/`if let` (which borrows). `or_else` rebuilds `Some` from a payload it was handed through a borrow, so it stays `Copy`-only.

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

fun map<T, U, E>(r: move Result<T, E>, f: closure(&T).R: U).R: Result<U, E>
fun map_err<T, E, F>(r: move Result<T, E>, f: closure(&E).R: F).R: Result<T, F>
fun and_then<T, U, E>(r: &Result<T, E>, f: closure(&T).R: Result<U, E>).R: Result<U, E>
fun or_else<T, E, F>(r: &Result<T, E>, f: closure(&E).R: Result<T, F>).R: Result<T, F>
fun each<T, E>(r: &Result<T, E>, f: closure(&T))
```

`Ok`/`Err` is the shape that carries a reason, and `?` requires the operand's failure payload type and the enclosing function's to be *identical* — there is no error conversion, which is where a trait (or a `From`-shaped builtin) would go.

The `Copy` requirements left here are on the side a helper has to **rebuild** through a borrow: `unwrap_or`/`unwrap`/`unwrap_err` hand a payload out of a borrowed result, and `and_then`/`or_else` rebuild the other variant out of one. `map` and `map_err` take the result with **`move`** instead, so the side they rebuild may be owned: `result.map` over a `Result<T, MyErr>` moves the error payload into the fresh `Err` (17's partial moves).

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

`filter` is the exception, and for the same reason `or_else` is: an accepted element is read *out* of the borrowed list by value, which is a move out of a borrow, refused (17). `list.filter` over `List<String>` is a compile error saying so; `find_index`/`any`/`all` cover the predicates that only need to know *which* one, and a `Copy` element type filters directly.

## 17. Limits in v1

These are deliberate, and each one has a workaround in this document:

- No `break` or `continue`. Iteration is a `for` over a `List` (6.2), or a `while` plus an index when the loop is not a front-to-back walk.
- `?` propagates only an *identical* failure type (5.2): there is no error conversion, no `From`, and no boxing. `Option<T>`/`Result<T, E>` ship in `std/option`/`std/result`, and the stdlib's older error `datac`s remain as wrappers.
- A receiver on a generic type must write the type's own parameters (`fun Pair<A, B>.first()`, 9.3); there is no way to declare a method over the template with parameters of its own. An `enum` carries no methods at all, generic or not.
- A `Map` key must be `String`, an integer or `bool` (12.2): the compiler generates the key's hash and its equality, and a key type of your own would need a trait or a comparator function, neither of which exists.
- No user overloading (7.2) and no user `extern` bodies: `extern fun` is reserved for compiler-shipped stdlib modules.
- Partial moves cover one projection of an owned place: reading an owned field, or an owned `enum` payload, by value out of a `val`/`var` binding or a `move` parameter moves it out (9), and a `when`/`if let` over an owned place binds an owned payload by value for it. A projection read out of a borrowed structure or out of a nested place is still refused, so the helpers whose receiver stays a borrow are `Copy`-only: `list.filter` over a `List<String>` reads an accepted element out of a borrow, which is refused. The helpers that only needed the move (`option.unwrap` on an `Option<String>`, `result.map` over a `Result<T, MyErr>`) take their receiver with `move` now.
- No references -- and no `View` -- in fields, `List` elements, `Map` values or `Box` contents. `str` and `View` are borrows, so text that must be stored is a `String`; a view is taken at the point of use (11.5).
- A `View` never becomes a `str`: a slice has no NUL after it, so `str.len`/`str.eq`/`%s` on one would read past the slice. Take `v.to_string()` when you need owned text, and keep `str` and `View` apart otherwise.
- No `str.eq`/`==` on a `View`; `str.eq` takes a `str`, so compare two views with `view.eq(other)`.
- No string concatenation operator, and no implicit `String` to `str`: use `str.concat` and `.as_str()`. The other direction *is* implicit (5.1): a `str` or a `View` becomes an owned `String` where one is expected.
- No sized integer literal and no digit separator: a literal has no width of its own, so a `u8` byte is written `val b: u8 = 0xff`. Hexadecimal and binary are `0x` and `0b` (2.4); there is no octal form and no suffix.
- A `char` is a code point and does not mix with the integer types: `'a' + 1` and `'a' == 97` are errors. Give the literal the type the arithmetic needs (`val a: u8 = 'a'`).
- No float-to-integer conversion: `to_i64` does not exist on `f64` (it would drop the fraction silently). Use `math.trunc`/`math.floor` and then an integer conversion.
- No exceptions, no `defer`, no destructors beyond the automatic drop at scope exit.
- A `[T; N]` array is fixed at its written length: there is no `[T; 0]`, no `a[i]` subscript (use `.get`/`.set`, as on a `List`), and no growth (`push`/`insert` are a `List`'s). The repeat form `[x; N]` copies its value into every slot, so it needs an element that owns nothing, and it is the one array shape a `datac` element cannot take (write the elements out, or start from a literal and `set` each one).
- Compile-time evaluation is the language's own operators plus calls to plain `fun`s. There is no `comptime` block, no compile-time `List`/`Map`, no `size_of`/`align_of`, and a `const` cannot read a `pd` or call anything `extern`-backed.
- Function values (7.4) are declared functions, `fun` literals, and closures: a literal becomes a closure when it names a binding from the enclosing scope, captures are **move-only** (a `Copy` binding is copied, an owned one is moved), and a borrow cannot be captured at all. A function value cannot be called as the result of another call (`f()(x)`) — bind it first.
- A function value cannot be a container element or the return type of a function *type*, a function that reads a passdown is not a value, and a closure cannot be a `List`/`Map` element (a `Box` or a one-field `datac` holds it).
- A closure's captures are read out of its environment through a borrow, so an owned capture cannot be handed out by value (9's partial-move rule: the environment is not a local this frame owns). Its environment is dropped where the closure dies, so a closure cannot outlive anything it captured — which is why a borrow can never be a capture.
- A nested literal can copy a `Copy` capture out of the enclosing closure's environment, but not move an owned one out of it.
- `kelpc run` cannot pass arguments to the program, so a program that reads `argv` is tested through `coral run -- <args>` or the built binary.
- Capability patterns are static text: they cannot be computed, narrowed or revoked at runtime.

## 18. Errors you will hit

Compiler diagnostics print as `file:line:col`, a message, and often a note with the fix. The common ones, with the fix:

| Message | Cause and fix |
| --- | --- |
| `expected 'fun', found Ident("fn")` | the keyword is `fun` |
| `expression-bodied function requires a '.R: T' return type` | add `.R: T`, or use a block body with `return` |
| `if without an else branch must evaluate to 'unit'` | an `if`, `if let` or `when` used as a value needs an `else` arm |
| `pattern 'Circle(a, b)' has 2 binding(s) but variant 'Circle' carries 1 payload field(s)` | `if let` and a `when` arm bind one name per payload field, in order |
| `expected 'in' between the 'for' binding and what it iterates, found 'xs'` | a `for` is `for name in list { ... }`; the `in` is required (6.2) |
| `type 'i32' has no method 'len'` (the span on the iterable) | `for` walks a `List<T>` or a `[T; N]` only, and the desugar asks it for `.len()` (6.2) |
| `cannot assign to 'x': not declared 'var'` | declare the binding `var` |
| `'id' is not a 'var' field` | the `datac` field must be declared `var` too |
| `use of moved value 'a'` | an owned value was moved earlier; reorder, or borrow instead |
| `cannot borrow 'c' as exclusive: already borrowed as shared` | end the earlier borrow (an inner block does it), because borrows last to the end of the scope |
| `cannot borrow 'c': already borrowed exclusively` | same rule, other direction |
| `this is a borrow of a borrow` (with the operand named) | a parameter is already a borrow (§10.2), so `&o` on one makes two: pass the parameter itself, `o` |
| `cannot borrow a temporary value` | `&expr` needs a place, and a call's result is not one, nor is a field of one (`&mkName().name`); bind the call to a `val` and borrow the field of that (§10.2). A chain that *steps through* a borrow is a place and needs neither: `&headers.get(i).name` is durable |
| `operator requires matching numeric operands, found 'mut i64' and 'i32'` with the `write '*sink'` note | a `mut T` parameter is a borrow: read and write it through `*`, the same rule as §10's `*pos` |
| `cannot move the field 'name' out of 'l': the value is borrowed` | a partial move needs an owned place, a `val`/`var` binding or a `move` parameter; use the field through the borrow, or hold it in a `Box` and `take()` it (9, 22.2) |
| `cannot read the owned field 'name' by value: the base is not a local this frame owns` | the field is read through a nested place or another base that is not an owned local; only a field read directly off an owned local moves out (9, 22.2) |
| `cannot use 'bag' as a whole value: its field 'xs' was moved out` | a field was read out by value; put a fresh value back in the field, or use only the fields that stayed (9, 22.2) |
| `use of moved value 'h.text'` | that `enum` payload was read out by value already; a payload of an owned place is taken once, exactly as a field is (9, 22.2) |
| `cannot use 'h' as a whole value: its payload 'text' was moved out` | a payload was read out by value; match only the payloads that stayed, or match a borrow of the enum instead (9, 22.2) |
| `this call returns a borrow of its parameter 'self', so that argument has to be a variable or a field` | bind the receiver first, then borrow from the binding; a field of a returned `datac` (`mkName().name.as_str()`) is the same case one projection deeper (§10.2) |
| `argument 'b' has type 'String', expected 'str'` | the parameter is `str` and the value is owned: `str.eq(a, b)`, `fs.write_text`, `path.join` and every other `str` parameter need a view — add `.as_str()` |
| `argument 'c' has type 'char', expected 'u8'` (with the note about writing the type) | a character literal with nothing asking for a type is a `char`, and a `char` is not an integer; write the type (`val c: u8 = '"'`) or use the byte literal, which is a `u8` by construction (`b'"'`, §2.4) |
| `a byte literal is a 'u8', not a 'i64'` | a byte literal is a `u8` and does not take the type its context asks for, which is the difference from a character literal: write `'x'` when the type should come from where the literal sits (§2.4) |
| `a byte literal is one ASCII character or a '\xNN' escape` | a non-ASCII character is a code point, so it belongs in a character literal; `b'\xe9'` is the byte 233 (§2.4) |
| `a hex escape is written '\xNN', with two hex digits` | the escape is exactly two hex digits, and a string literal stops at the ASCII range: `"\xff"` is not text (§2.4) |
| `the character '中' (U+4E2D) does not fit in 'u8'` | a character literal takes the type its context asks for, and this one does not fit; use a wider type, or the byte you meant |
| `operator requires matching integer operands, found 'u8' and 'bool'` | `&` `\|` `^` `<<` `>>` `~` are integer-only (`bool` has `&&`/`\|\|`), and both operands must be the *same* type |
| `cannot compare 'View' values with '=='` | use `v.eq(other)` — it compares the bytes and the lengths |
| `argument 'x' has type 'View', expected 'str'` | a slice has no NUL after it, so a view is not a `str`; take `v.to_string()` (11.5) |
| `field 'text' is a borrowed type ('View')` | a view cannot be stored: a field outlives the borrow, and v1 has no lifetime syntax (11.5, 17). Store a `String` |
| `'View' is a borrowed view` (as a `List` element) | a container owns its elements; store `String`s and view them at the point of use |
| `cannot return this value as 'View': ... only a 'str' literal can be returned` | a returned view has to be derived from a reference parameter (the elision rule). Return an owned `String`, or take the view as a parameter |
| `this call returns a borrow of its parameter 'self', so that argument has to be a variable or a field` (on `.view()`) | a view of a temporary: bind the `String` first, or view the literal itself |
| `unknown escape '\q' in a string literal` | the escapes are `\n`, `\t`, `\r`, `\"`, `\\` |
| `unknown escape '\q' in a character literal` | the escapes are `\n`, `\t`, `\r`, `\0`, `\'`, `\"`, `\\`, `\u{...}` |
| `a character literal holds exactly one character` / `cannot be empty` / `cannot span lines` | `'ab'`, `''` and a raw newline are not character literals; write `'a'` or `'\n'` |
| `argument 's' has type '&String', but this call moves its argument in` | the callee is a `move T` parameter (or a container's `push`-shaped builtin): a borrow cannot be moved out of, so pass an owned value — `Box.take()` moves one out of a box |
| `'move' cannot be written on the borrowed type 'str'` | a `str`/`&T` is already borrowed, so there is nothing to own: write the owned type (`move String`). A `fun` value is already `Copy` for the same reason; `move closure(...)` *is* legal, because a closure owns its environment |
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
| `a 'fun' literal cannot capture 'n': it is a field of 'Counter'` | a receiver is a borrow, and a field cannot be moved out of a borrow; bind a `Copy` field to a local first, or take the receiver as a `move` parameter |
| `cannot capture 'text' into this literal: it belongs to the enclosing closure's environment` | an environment is read through a borrow, so an owned capture cannot be moved out of it; capture it from the frame that owns it, or copy a `Copy` one |
| `cannot read a captured value out of the closure's environment by value` | a capture is read through a borrow; use it in place, or capture a `Copy` value |
| `this is a closure: its body captured a binding from the enclosing scope` | the parameter's type has to say `closure(...)`; a `fun(...)` value has no room for an environment |
| `this is a plain function value, not a closure` | the literal captured nothing, so it has no environment; write the expected type as `fun(...)`, or name a binding to capture |
| `'&' cannot be written on the closure type` | a closure is an owned value; a parameter of that type already borrows it, and `move closure(...)` takes it over |
| `'closure().R: i32' cannot be a 'List' element yet` | a closure owns its environment and a list's elements are inline; use `Box<closure(...)>` or a one-field `datac` |
| `a 'fun' literal cannot read passdown 'base'` | a passdown is a hidden argument; pass the value as a parameter |
| `a 'fun' literal cannot return a borrow ('&str')` | an indirect call has no parameter an elided lifetime could bind to; return an owned value |
| `a 'fun' literal needs a body` | write `= expr` or `{ ... }` |
| `a string template can only be printed` | a template is not a value; build the text with `str.concat`, `fmt.*` or a `String` |
| `an array of 3 element(s) is expected here, and this literal has 2` | the length is in the type, so `[T; N]` and the literal have to agree (12.4) |
| `'k' is not a constant here` | an array's length is evaluated at compile time; a `val` computed at runtime is not a constant, and a `const` can only name constants declared before it (14.1) |
| `the repeat form '[x; N]' needs an element that owns nothing` | `[x; N]` copies `x` into every slot, so `x` cannot own memory; write the elements out, or use a scalar or a scalar-only `datac` (12.4) |
| `'&i32' cannot be a container element` | a container owns its elements, so a borrow cannot be one (3, 12.1) |
| `an array length has to be at least 1` | there is no `[T; 0]`; a zero-length C array is not standard C (12.4) |
| `Array.get: index out of bounds` (at run time) | the index was checked against the array's own length; a longer index is a bug at the call site |
| `'print' cannot be called in a constant expression` | a `const` is folded inside the compiler, so it cannot call a builtin, an `extern fun` or a method, only a plain `fun` that does the same (14.1) |
| `this constant expression did not finish within 1000000 steps` | a `const` may call a `fun`, and that function has to terminate on the values the constant gives it (14.1) |
| `unknown escape '\q' in a string literal` | the escapes are `\n`, `\t`, `\r`, `\"`, `\\` |
| `function 'f' is defined more than once for the same parameter types` | user code cannot overload; rename one |
| `cannot tell what 'List.empty()' holds` | write the element type on the binding (`var xs: List<i32> = List.empty()`), or let the first `push` decide it |
| `'str' is a borrowed view` | an owned field, element or value is needed; use `String` |
| `'View' is a borrowed view` | same: a `View` points at text somebody else owns, so it cannot be stored or be an element (11.5) |
| `cannot find module 'x': expected '<path>'` | the `impl` path is wrong, or the file is missing |
| `import cycle: a -> b -> a` | remove the cycle |
| `unknown key 'x' in <path>` | the manifest is flat and its keys are `name`, `version`, `entry`, `http_pool` (1.4); a typo is an error rather than a default |
| `coral.toml value for 'entry' must be a double-quoted string` | every value is quoted except `http_pool`, which is a number (1.4) |
| `'<dir>' is a directory but has no coral.toml` | `kelpc build <dir>` needs a manifest in that directory; pass a `.kp` file instead, or add one (1.4) |
| `entry file '<path>' is not inside '<root>/src'` | a project's modules live under `src/`; the manifest's `entry` is resolved against the project root, not against `src/` (1.3) |
| `no 'main' function found` | the entry module needs `fun main()` |
| `'fs.read' needs the 'fs' capability, which module 'm' was not granted` | grant it at the `impl` line: `impl "m" caps(fs = ["/path/**"])` |
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
- File order: `impl`, then `const`, then `pd`, then `datac`/`enum`/`fun` (13.2). The parser enforces every step but the order inside the last group: an `impl` written after any declaration, a `const` written after a `pd`, `datac`, `enum` or `fun`, and a `pd` written after a `datac`, `enum` or `fun` are parse errors. Whether a `datac` or an `enum` comes before a `fun` remains a review convention.
- No semicolons. Do not wrap an expression across lines unless the wrap is inside parentheses or an argument list.
- Standard library modules, functions and datac fields are `snake_case` (`fs.read_text`, `str.is_empty`, `Line.text`), and types are `PascalCase` (`String`, `Bytes`, `Box`, `Metadata`, `Url`).
- User code case is configurable rather than fixed: `kelpc lint` reports a declared name that is not the project's case, and either case keeps compiling (see below). The examples and this plan use `snake_case` for functions and lowercase for bindings, matching the standard library.
- Named arguments are used wherever a call would otherwise be ambiguous, which in practice means every `datac` construction with two or more fields, as in `Circle(r = 2.0)`.
- Comments are `//`, written as full sentences when they explain a decision and never to restate the next line. `/* ... */` is for the two things a line comment cannot do: a note that spans lines, and commenting out a region of code; a block comment nests, so a region that already contains one still works. Two or more consecutive `//` lines with no blank line and no code between them is the pattern this rule is about written the other way, so `kelpc lint` warns about it. The warning never guesses intent (the author may have meant every line), so it is a warning and not an error. A documentation comment (`//!`, `///`, 2.1) is not part of a run either: a run of doc lines is already one block, and a `//!` line does not count toward a plain `//` run.
- A chain link is written `elif` (6.1). `else if` is the same link, stays legal and compiles identically; `kelpc lint` warns about it, and `kelpc fmt` never rewrites it.
- The case of the project's own names is **configurable**, and it is the one style rule that has a setting: `coral.toml`'s `naming_case`, `0` for `camelCase` (the default) or `1` for `snake_case`. It is `kelpc lint`-only, so either case keeps compiling, and a file with no manifest to read gets the default. The rule reads the names a file **declares**: a `fun` name, a parameter, a `val`/`var`/`pd` binding, a `datac` field, an enum payload field, a loop binding, and a pattern binding (`if let Circle(r) = s`, `when s { Circle(r) -> .. }`). A name the project did not write is not its business, and that is the whole of the standard library's exception: the stdlib keeps its locked `snake_case`, so a `camelCase` project is never warned about its call into `str.is_empty` and a `snake_case` project is never warned about the stdlib's `fs.read_text` either, because a call is a use and this rule is about declarations. Type names stay `PascalCase` whatever the setting says, and a `const` name stays `UPPER_SNAKE`.

`kelpc fmt` enforces the indentation, spacing, trailing-whitespace, newline and
comment rules of this section and nothing else: it caps a run of blank lines
between top-level declarations at one but never inserts a blank line the author
did not write, it preserves the author's own line wrapping instead of
re-wrapping to the 100-column limit, and the 100-column limit itself, the order
inside the last group and the naming/case rules remain review conventions that
need a lint, not a formatter. `kelpc lint` is that command, and it is
warning-only by design: it exits zero when it finds something, because a style
finding is not a build failure. It implements the three rules above that a
formatter cannot: the comment run, the `else if` link, and the configurable
naming case. The naming case is the one rule with a setting, it is read from the
project's `coral.toml` rather than from a flag, and the names it reads are the
ones the file declares and nothing else (19 says which, and why).
`kelpc fmt` holds four invariants: formatting is **idempotent** (formatting a
formatted file changes
nothing), **token-faithful** (no token's text is ever rewritten, so the formatted
file is the same program), **comment-preserving** (every comment survives
verbatim), and it **never writes a file it refuses** (a file that does not parse
is reported, not reformatted). `kelpc/src/fmt.rs` lists, as data, every rule it
enforces and the file-refusal reasons it names through `RefusalKind`. The
command's argument checks — an unknown flag, `-w` with `--check`, `fmt` with no
mode — and the `no '.kp' files found` case and an unreadable path inside a
directory walk are printed as raw strings and are not in that data.

## 20. Grammar summary

```
program      := line* EOF
line         := impl | const | pd | datac | enum | fun

impl         := "impl" string [ "caps" "(" grant ("," grant)* ")" ]
                // `as name` was removed (13.1) and `with caps(...)` with it (15.2):
                // the parser refuses each by name, and `as` stays a keyword
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
                       | "fun" "(" [ fparam ("," fparam)* ] ")" [ ".R" "：" type ]
                       | "[" type ";" expr "]" )        // a fixed-size array: [T; N]
fparam       := [ "mut" | "move" ] type
```

(`View` is an ordinary `ident` here — it is a builtin type name like `str` or
`String`, not a keyword.)

```
stmt         := "val" | "var" | "while" | "for" | "return" | expr
val          := "val" ident [ ":" type ] "=" expr
var          := "var" ident [ ":" type ] "=" expr
while        := "while" expr block
for          := "for" ident "in" expr block   // the `in` is contextual, not a keyword
return       := "return" [ expr ]
block        := "{" (line-separated stmt)* "}"

expr         := assign
assign       := binary ( ( "=" | "+=" | "-=" | "*=" | "/=" | "%=" ) assign )?
binary       := unary ( binop binary )?
binop        := "||" | "&&" | "==" | "!=" | "<" | "<=" | ">" | ">="
              | "|" | "^" | "&" | "<<" | ">>"       // << and >> are two adjacent < / > characters
              | "+" | "-" | "*" | "/" | "%"
unary        := ( "-" | "!" | "~" | "&" [ "mut" ] | "*" ) unary | postfix
postfix      := primary ( "?" | "." ident [ typeargs ] [ "(" args ")" ] )*
primary      := int | float | string | char | template | "true" | "false"
              | "fun" "(" [ param ("," param)* ] ")" [ ".R" "：" type ]
                ( "=" expr | block )
              | "[" expr ("," expr)* "]"       // an array literal: length is the count
              | "[" expr ";" expr "]"          // a repeat: [0; 4096]
              | ident [ typeargs ] [ "(" args ")" ] | "(" expr ")" | block
              | "if" expr block chain
              | "if" "let" ident [ "(" (ident | "_") ("," (ident | "_"))* ")" ] "=" expr
                block chain
              | "when" "{" ( expr "->" expr )* "else" "->" expr "}"
char         := "'" ( <any Unicode scalar value except ' and newline>
                    | "\\n" | "\\t" | "\\r" | "\\0" | "\\'" | "\\"" | "\\\\"
                    | "\\u{" hexdigit+ "}" ) "'"
args         := [ arg ("," arg)* ]
arg          := [ ident "=" ] expr
chain        := [ "elif" expr block chain
                | "elif" "let" ident [ "(" (ident | "_") ("," (ident | "_"))* ")" ] "=" expr
                  block chain
                | "else" ( block | "if" expr block chain
                         | "if" "let" ident [ "(" (ident | "_") ("," (ident | "_"))* ")" ] "=" expr
                           block chain ) ]
```

`chain` is what follows the first block of an `if` or an `if let`: nothing, an `else` block, or another link, which is `elif` (6.1) or the `else if` spelling of it. The `let` form of a link is the pattern form, and a chain may mix the two spellings and the two forms freely.

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
    for raw in words {
        val word = str.trim(raw.as_str())
        if !str.is_empty(word.as_str()) {
            bump(counts, word.as_str())
        }
    }

    var keys = counts.keys()
    keys.sort()

    var total: i64 = 0
    for key in keys {
        val n = counts.get(key.as_str())
        total += n
        print("${key}: ${n}")
    }
    val total_text = fmt.int(total)
    print(str.concat("words: ", total_text.as_str()))
}
```

What the program shows, in order: `impl` lines for four modules, a free function that takes an exclusive borrow of a `Map`, a capability-checked `fs` call whose argument has to be absolute, `Map` and `List` builtins walked with `for` (the element binding is `List.get`'s, hence the `.as_str()` calls), `String` ownership passed to a `mut` parameter, and a template that prints two values.

## 22. Writing a program: the rules that explain most errors

Sections 1–21 are a reference. This one is the part a reference leaves out: what to reach for, and why the compiler says what it says. Read it once before writing a few hundred lines.

### 22.1 Every parameter of an owned type is a borrow

This is the rule that explains the most diagnostics, so it comes first.

```kelp
fun show(s: String)         // `s` is a &String inside the body: the caller keeps it
fun fill(m: mut String)     // `m` is a &mut String: the callee may write to it
fun take(s: move String)    // `s` owns what it was given: the call site moves it in
```

So a `fun` that reads text takes `s: String` (a borrow) or `s: &str` (a borrowed view), and **neither one copies**:

```kelp
fun len_of(s: String).R: usize = s.len()          // no copy: s is a borrow
fun len_view(s: &str).R: usize = str.len(s)       // no copy: str is already a view
```

Consequences you will meet immediately:

- At the call site a `String` argument is written bare: `len_of(name)`, never `len_of(&name)`. The `&` is only for taking a borrow of a **local** on purpose (`val p = &counter`).
- Inside the body, `s.field` and `s.method()` work directly. A scalar parameter of a **`mut`** type is the exception: it is a borrow, so a read needs `*s` and a write needs `*s = ...` (see 22.3).
- A `mut T` argument must be a `var` (or a field of one) — an exclusive borrow of an immutable binding is refused.
- `move T` is what a call that *stores* its argument needs. A container's `push` is the builtin example: `xs.push(s)` moves `s` in, so `s` is gone afterwards.

### 22.2 An owned value moved once

Assigning an owned value moves it. This is not an error to work around; it is how the language knows when to free things.

```kelp
val a = String.from_str("hi")
val b = a                     // `a` is moved into `b`
print(a.as_str())             // error: use of moved value 'a'
```

Three ways out, in the order to try them:

1. **Borrow instead of move.** `use_it(a)` where the parameter is `String` borrows it, so `a` is still yours afterwards.
2. **Copy the text.** `val b = a.clone()` copies the buffer, so both `a` and `b` own one.
3. **Reorder.** Do the last use last.

The related rule is **partial moves**: reading an owned field *by value* out of an owned place moves the field out and clears the slot it held. The owned place is a `val`/`var` binding or a `move` parameter, and the field is read directly off it. Reading the same field again, moving it twice, reading a field out of a *borrow*, and using the base as a whole are each refused.

An owned `enum` payload obeys the same rule: a `when`/`if let` over an owned place binds the payload **by value** and takes it out of the enum, so matching the same payload twice, and using the enum as a whole afterwards, are refused. Over a borrow the payload binds as a borrow and nothing moves, which is why an owned value cannot be read out of one.

```kelp
datac Line(val text: String, val n: usize)

fun use_line(l: Line) {
    val s = l.text            // error: cannot move the field 'text' out of 'l': the value is borrowed
    print(l.text.as_str())    // fine: a view of the field
    print(l.n)                // fine: `usize` is Copy
}

fun take_line(l: move Line).R: String {
    return l.text             // fine: `l` is an owned local, so the field moves out
}

enum Held {
    Word(val text: String)
    Blank
}

fun take_word(h: move Held).R: String {
    return when h {
        Word(text) -> text    // fine: the payload moves out of the owned `h`
        Blank -> String.from_str("")
    }
}

fun peek_word(h: &Held).R: usize {
    return when h {
        Word(text) -> text.len()   // fine: `text` is a `&String` here, and nothing moved
        Blank -> 0
    }
}
```

To *take* an owned field out of a value the frame does not own, hold it in a `Box` and `take()` it. A `move` parameter is an ordinary owned local in the callee, so reading a field out of it by value is the same partial move a `val` binding allows, while the borrowed `Line` above is refused because the field is not this frame's to give away.

### 22.3 Scanning text: `View` plus a cursor

A hand-written parser needs two pieces of state: the text and a position. Neither belongs in a `datac` — a `View` is a borrow, and a borrow cannot be a field (11.5). Pass them:

```kelp
fun scan_word(text: View, pos: mut usize).R: result.Result<String, ScanError> {
    val start = *pos                        // `pos` is a `mut usize`: read with `*`
    while *pos < text.len() && is_alpha(text.get(*pos)) {
        *pos += 1                           // and write with `*`
    }
    if *pos == start {
        return result.Result.Err(error = ScanError(message = "expected a word", at = *pos))
    }
    return result.Result.Ok(value = text.slice(start, *pos))   // `View` -> `String`
}
```

Facts that make this pattern work, and the traps in it:

- **`*pos`, not `pos`.** A `mut T` parameter is a borrow. A local `var pos` is not, so the same loop written with a local uses `pos` with no `*`. Mixing the two up gives `cannot compare 'mut usize' with 'usize'`, which is the diagnostic telling you which one you have.
- **`text.get(i)` is a `u8`, and a character literal takes that type**, so the comparisons read as characters: `c == '"'`, `c == '\\'`, `c >= '0' && c <= '9'`.
- **`View.slice` never allocates.** Slicing is what a scanner does most; the owned `String` is made once, at the point the token becomes data (`text.slice(a, b)` into a `String` parameter or return, which copies by length).
- **The text is borrowed, not copied.** `fun parse(text: &str)` takes the caller's view and calls `text.view()` once inside. Nothing is copied before the scan starts.
- **No `break` and no `continue`.** Write the loop condition so it covers every exit: `while pos < text.len() { ... if pos < text.len() { ... } }` is the shape a "skip blanks, then maybe stop" loop needs.
- **`List.get(i)` gives a copy for a `Copy` element and a shared borrow otherwise**, which is why `xs.get(i).as_str()` has the `.as_str()` — `xs.get(0)` is a `&String`, not a `String`.
- **Byte-indexed, not character-indexed.** `View.get`, `str.byte` and `str.len` are bytes; `str.char_count`/`str.char_at` are the codepoint pair. A UTF-8 continuation byte is a byte like any other, so scan to a delimiter or to the end rather than counting characters.

### 22.4 Text in, text out

```kelp
// Reading
fun parse_all(text: &str).R: result.Result<Thing, ParseError> {
    var pos: usize = 0
    val src = text.view()            // the only strlen in the program
    return parse_value(text = src, pos = pos)
}
```

- A literal is a `str`. Where a `String` is expected it becomes one **by itself** (5.1): `datac Tok(val text: String)` accepts `Tok(text = "abc")`. Do not write `String.from_str` at a `String` position.
- A `str` position needs a view: `.as_str()` on a `String`/`&String`, and a `View` is *not* a `str` (11.5) — take `v.to_string()` when you need owned text of a slice.
- Building owned text: `String.empty()`, then `push(c: u8)`, `push_str(s: &str)`, `concat`, or `fmt.*` for a number. `str.concat(a, b)` allocates a new `String`.
- Formatting a number: `fmt.int(n)`, `fmt.uint(n)`, `fmt.float(x, decimals)`, `fmt.hex(n)`. All return `String`.
- `print(x)` handles every scalar, `str`, `String`, `Bytes` and `View`; a template (`"$x"`) is the only way to print a mixture, and a template is not a value.
- **`.as_str()` on a temporary is refused** (`String.from_str("x").as_str()`): bind the `String` first. The same applies to `.view()`.
- UTF-8 by hand is expressible directly now: `0xE0 | (cp >> 12)`, `0x80 | ((cp >> 6) & 0x3F)`, `0x80 | (cp & 0x3F)` (5, and 2.4 for the literal). `%` and `/` are no longer the only arithmetic available for it.

### 22.5 Records and tags

```kelp
datac Point(val x: i32, val y: i32)          // every field `val`: read-only
datac Counter(var n: i64, val name: String)  // `n` is writable through a `var` binding

enum Shape {
    Circle(val r: f64)
    Rect(val w: f64, val h: f64)
    Empty
}
```

- Construction names every field: `Point(x = 1, y = 2)`, `Shape.Circle(r = 1.0)`, `Shape.Empty`. The order does not matter, and the names are checked.
- `val`/`var` on a **field** is independent of `val`/`var` on the **binding**: writing `c.n = 5` needs the field to be `var` *and* the path to `c` to be mutable, and the two failures have different messages.
- Passing the same value twice to one call is refused (aliasing), and so is writing through a `&` borrow.
- Reading a variant requires a pattern:

```kelp
fun area(s: &Shape).R: f64 = when s {
    Circle(r) -> 3.141592653589793 * r * r
    Rect(w, h) -> w * h
    Empty -> 0.0
}
```

- `when` is exhaustive: leave a variant out and the message names it. `else ->` is the catch-all and must come last.
- A payload binds as a copy when it is `Copy`, as an **owned value** when it is owned and the scrutinee is an owned place (a `val`/`var` binding or a `move` parameter), and as a shared borrow otherwise — that is why the stdlib callbacks take `closure(&T)`. Reading an owned payload out of an owned place moves it (9.2, 22.2); reading one out of a borrow does not, and stays a borrow.
- `if let Some(x) = opt { }` is the one-arm form, and `?` is the propagate form. Both need the enum to be `Ok`/`Err`- or `Some`/`None`-shaped, and `?` needs the enclosing function to return the *same* shape with the *same* failure type.
- A function that can fail more than one way returns your own error `datac`; a `datac` with a `message: String` and a position is the convention (see `str.ParseErr`).

### 22.6 Collections

```kelp
var xs: List<i64> = List.empty()      // the element type is written on the binding
xs.push(3)                            // moves the argument in
val first = xs.get(0)                 // a copy: i64 is Copy
xs.sort()

var counts: Map<i64> = Map.new()      // `Map<V>` is a String key; `Map<K, V>` names both
counts.insert("a", 1)
if counts.has("a") { print(counts.get("a")) }
```

- `List.empty()` and `Map.new()` need the element type from somewhere. Write it on the binding, or let the first `push`/`insert`/`set` decide it: `var xs = List.empty()` followed by `xs.push(1)` is a `List<i32>`.
- `Map` keys are `String`, an integer or `bool`, and only those.
- Both keep insertion order; `Map` has no `Map<String, Map<i32>>` or `List<List<i32>>` — elements are inline, so wrap a nested container in a `Box`.
- `for x in xs { }` walks a `List` front to back. There is no `break`; to stop early, put the condition in a `while` with an index.

### 22.7 Capabilities are declared where you import

```kelp
impl "std/fs" caps(fs = ["/project/out/**", "/project/data/**"])
```

- A module gets nothing it did not `impl`, and the checker knows each module's authority statically.
- Every path passed to a gated call is matched against the patterns, so it must be absolute and must not contain `..`.
- `main` in the entry module holds the whole resource for its domain, which is why the interesting module sits behind one level of delegation when the point is to *test* a restriction.
- The grant is the ceiling for the whole module, so a helper that reads a file needs the module holding the `impl` to have been granted it.

### 22.8 A complete scanner

Everything above, in one program. It scans `count = _a1 + 42` into tokens, each an owned `String`, and reports a failure the way a real parser does.

```kelp
// A tiny scanner: the shape every hand-written parser has.
impl "std/str"
impl "std/fmt"
impl "std/result"

datac ScanError(val message: String, val at: usize)

fun is_blank(c: u8).R: bool = c == ' ' || c == '\t' || c == '\n' || c == '\r'

fun is_digit(c: u8).R: bool = c >= '0' && c <= '9'

fun is_alpha(c: u8).R: bool {
    return (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || c == '_'
}

fun skip_blanks(text: View, pos: mut usize) {
    while *pos < text.len() && is_blank(text.get(*pos)) {
        *pos += 1
    }
}

fun scan_word(text: View, pos: mut usize).R: result.Result<String, ScanError> {
    skip_blanks(text, pos)
    val start = *pos
    while *pos < text.len() && is_alpha(text.get(*pos)) {
        *pos += 1
    }
    if *pos == start {
        return result.Result.Err(error = ScanError(message = "expected a word", at = *pos))
    }
    return result.Result.Ok(value = text.slice(start, *pos))
}

fun scan_number(text: View, pos: mut usize).R: result.Result<i64, ScanError> {
    skip_blanks(text, pos)
    val start = *pos
    var n: i64 = 0
    while *pos < text.len() && is_digit(text.get(*pos)) {
        n = n * 10 + (text.get(*pos) - '0').to_i64()
        *pos += 1
    }
    if *pos == start {
        return result.Result.Err(error = ScanError(message = "expected a number", at = *pos))
    }
    return result.Result.Ok(value = n)
}

fun tokenize(text: View).R: result.Result<List<String>, ScanError> {
    var out: List<String> = List.empty()
    var pos: usize = 0
    while pos < text.len() {
        skip_blanks(text, pos)
        if pos < text.len() {
            val c = text.get(pos)
            if is_alpha(c) {
                val word = scan_word(text, pos)?
                out.push(word)
            } else if is_digit(c) {
                val n = scan_number(text, pos)?
                out.push(fmt.int(n))
            } else {
                out.push(text.slice(pos, pos + 1))
                pos += 1
            }
        }
    }
    return result.Result.Ok(value = out)
}

fun main() {
    val r = tokenize("count = _a1 + 42".view())
    if let Ok(tokens) = r {
        for t in tokens {
            print(t.as_str())
        }
    } else {
        print("scan failed")
    }
}
```

It prints `count`, `=`, `_a`, `1`, `+`, `42`, one per line. Worth noticing: `tokenize` passes `pos` (a local `var`) with no `*` while the helpers take `*pos`; `scan_word` returns an owned `String` even though it only ever sliced a view; `is_alpha(c)` receives the `u8` from `text.get` and compares it to character literals; and `if let Ok(tokens) = r` moves the owned `List<String>` out of `r`, because an owned payload of an owned place binds by value (9.2), so the loop walks a list the arm owns and `t.as_str()` is what `print` needs.

## 23. Deprecated syntax

A spelling that is on its way out is **still accepted, and still compiles**. The
compiler keeps a register of those spellings (`kelpc/src/deprecated.rs`), and
`kelpc build` and `kelpc run` print one finding for each one a program uses:

```
outdated: `impl "..." with caps(...)` is outdated and will be removed in the next development cycle: drop the `with`: the grant is `impl "std/fs" caps(fs = ["/var/log/**"])`, and the patterns, the domains and the no-escalation rule are exactly the same
  --> src/main.kp:2:13
```

That finding is the shape an entry produces, shown here from the entry the
register last held; **the register is empty today** (23.1), so no program
produces one.

A program that uses an outdated spelling **more than once gets one finding per
use**, and one finding per use is all it gets: a load reads each module twice
(once to discover the module graph, once to decide), so a finding collected by
that reading is deduplicated at the end of it.

- **It is not an error, and the build succeeds.** The label is `outdated: `, which is neither `error:` nor `warning: `: a lint's `warning: ` is a style opinion (19), this is a promise about a later version, and nothing is removed on the day its warning first appears.
- **The warning names the removal.** Every entry is removed on the next development cycle, and that sentence is shared by every entry, so no entry can promise a different deadline.
- **`--deny-outdated` turns the findings into errors** (`kelpc build <path> --deny-outdated`, and the same flag on `run`). That is the migration gate: a project (or its CI) that wants to hear about its own code fails the build instead of reading a warning. A program with nothing on the list builds exactly as it did before the flag; with the register empty, every program is such a program.
- **A removed spelling is not a deprecation.** Once an entry's cycle is over the spelling is deleted from the parser, and what a stale program meets is a parse error that names the fix, not an `outdated:` finding. That is the state of both entries below.
- **`kelpc fmt` never rewrites one spelling into another**, for the reason it never rewrites anything: it copies the token it was given. The edit is the author's.
- **`kelpc lint` does not report deprecations.** A lint is a style pass over tokens; a deprecation is reported where the syntax is read, which is the load a build performs.
- **The compiler's own sources carry none.** The shipped stdlib is migrated before an entry is written, because a deprecation there would warn every program that imports that module about a file its reader cannot edit. A unit test in `deprecated.rs` keeps that true.
- **The register is not `ISSUES.md`.** An entry here is scheduled work with a deadline; an open defect is something that does not work today.

### 23.1 The register, as it stands

**Empty.** Both entries it held were removed on the one-cycle deadline their
shared sentence gave them, and the mechanism stays for the next spelling:

| id | Outdated | Write instead |
| --- | --- | --- |
| `impl-as` (retired) | `impl "path" as name` | `impl "path"`, and reach the module by its own name (13.1). |
| `caps-with` (retired) | `impl "path" with caps(...)` | `impl "path" caps(...)`, with the same domains and the same patterns: `impl "std/fs" caps(fs = ["/var/log/**"])` (15.2). |

The first entry, `impl "path" as name`, was retired first: the clause is gone
from the parser, so it is refused rather than warned about, its fix lives in 13.1,
and a stale clause is a parse error that names it. The second,
`impl "path" with caps(...)`, went the same way: `with` carries no information
(`impl` already means "this module declares what it imports"), the grant keeps
every domain and pattern without it, and 15.2 carries the surviving spelling.
Keeping it for its own cycle would have meant a finding that named a deadline
that had already passed, which is the drift the shared sentence exists to
prevent.
