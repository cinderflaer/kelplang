# X — syntax & style (locked)

Supersedes the syntax decisions written in `plan.md`. `plan.md` stays as-is for now; this file is the authority on how `.x` source looks. Reconciling `plan.md` is a separate later edit.

Theme: Kotlin-shaped surface — no semicolons, `val`/`var`, types on the right, expression bodies — over a Rust-shaped memory model (ownership, `&`, `&mut`).

## 1. Type definitions: `datac`

`struct` is gone. Fields are declared `val` (immutable) or `var` (mutable). A `datac` holds **owned data only** — no reference fields in v1.

```x
datac Circle(val r: f64)
datac Vec2(val x: f64, val y: f64)
datac User(val name: String, var age: i32)
```

## 2. Bindings: `val` / `var`

- `val` — immutable binding
- `var` — mutable binding

`let` and `let mut` are dropped entirely.

```x
val before = c.area()
var c = Circle(r = 2.0)
```

## 3. Functions: `fun` + `.R: T`

- Keyword is `fun`, not `fn`.
- Return type is `.R: T`, written after the full signature. `.R` marks "this returns"; `: T` gives the type.
- No `.R` = returns `Unit`.
- `->` is never a return marker.

```x
fun Circle.area().R: f64 = 3.14159 * r * r
fun mut Circle.grow(by: f64) { r += by }
fun main() { ... }
```

## 4. Return: `return expr`

A function with `.R: T` must end with `return expr` producing a `T`. The signature is the contract; `return expr` delivers the value. Expression bodies (`= expr`) remain shorthand for single-expression functions.

```x
fun sum_to(n: i32).R: i32 {
    var acc = 0
    var i = 1
    while i <= n {
        acc += i
        i += 1
    }
    return acc
}

fun Circle.area().R: f64 = 3.14159 * r * r     // shorthand, no return needed

fun classify(n: i32).R: &str = when {
    n > 0 -> "pos"
    n < 0 -> "neg"
    else  -> "zero"
}
```

## 5. Receivers replace `self` / `impl` blocks / explicit `&`

- No `self` keyword. No `impl` method block.
- Receiver is declared in the signature: `fun Circle.area()`.
- Shared borrow is implicit — write nothing.
- Exclusive borrow is marked `mut`: `fun mut Circle.grow(by: f64)`.
- Free-function parameters: write `mut` only when mutation is needed; everything else is a shared borrow by default.

```x
fun Circle.area().R: f64 = 3.14159 * r * r        // shared receiver
fun mut Circle.grow(by: f64) { r += by }          // exclusive receiver

fun len(s: String).R: usize = ...                 // shared borrow, inferred
fun push(s: mut String, ch: char) { ... }         // exclusive, marked
```

## 6. Print

- `print(...)` — prints with a trailing newline.
- `print!(...)` — prints with no trailing newline.

Replaces `println` entirely.

```x
print!("loading")   // loading
print!(".")         // loading.
print("done")       // loading.done\n
```

## 7. `->` is reserved for `when`

`->` appears only in `when` branches, as `condition -> value`. It never means "returns".

```x
fun classify(n: i32).R: &str = when {
    n > 0 -> "pos"
    n < 0 -> "neg"
    else  -> "zero"
}
```

## 8. Module imports: `impl`

Module imports use `impl`, not `use`. Shape is otherwise unchanged.

```x
impl "shapes/circle"
impl "logger" with caps(fs = ["/var/log/myapp/*"])
impl "net/http"
impl "time" as t
```

Note: `impl` means import. There is no `impl` method block (see 5) — the receiver form replaced it, so the keyword is free.

## 9. Passdowns: `pd`

A passdown is a top-level binding available to every function without being passed explicitly. The compiler wires it in **only in functions that actually reference it** — functions that never mention it pay no cost.

**Syntax**

```x
pd val name = expr
pd var name = expr
pd val name: T = expr
```

- Legal only at the top of a file, before any `fun` or `datac`.
- `pd val` immutable, `pd var` mutable — same meaning as local `val`/`var`.
- Type inferred, or written explicitly with `: T`.
- A function referencing a passdown name gets it wired in automatically.
- A local `val`/`var` with the same name shadows the passdown inside that scope.
- Nothing is written at the function side — the function just uses the name.

```x
pd val app_name = "shapes"
pd var request_count = 0

fun log_area(c: Circle) {
    request_count += 1
    print("[$app_name #$request_count] area = ${c.area()}")
}

fun classify(n: i32).R: &str = when {
    n > 0 -> "pos"
    n < 0 -> "neg"
    else  -> "zero"
}
```

`log_area` uses both passdowns — wired in. `classify` uses neither — no wiring, no cost.

Open question: with no closures and no `unsafe` in v1, `pd var` is mutable global state reachable from any function. Borrow checking has to say what a passdown's lifetime is and whether two functions can hold a `&mut` to it at once. Decide when the passdown wiring pass is built — options are (a) `pd var` is per-function-call scratch, (b) `pd var` is only mutable through functions that declare it explicitly, (c) `pd var` is rejected in v1 and only `pd val` ships.

## 10. Retained Kotlin-look rules

- No semicolons. Newline ends a statement.
- Types on the right with `:`, e.g. `r: f64`.
- No parens on `if` / `while` conditions. Braces always required.
- String templates: `"x = $before, area = ${c.area()}"`.
- Named arguments at call sites when 2+ params: `Circle(r = 2.0)`.
- `Unit` is the void type, implicit.
- `when` replaces long `if`/`else if` chains.
- Blank line between functions. Import lines grouped at top. 4 spaces. Hard 100-col.

## 11. Full example (`main.x`)

```x
impl "shapes/circle"
impl "logger" with caps(fs = ["/var/log/myapp/*"])

pd val app_name = "shapes"
pd var request_count = 0

datac Circle(val r: f64)

fun Circle.area().R: f64 = 3.14159 * r * r

fun mut Circle.grow(by: f64) {
    r += by
}

fun log_area(c: Circle) {
    request_count += 1
    print("[$app_name #$request_count] area = ${c.area()}")
}

fun classify(n: i32).R: &str = when {
    n > 0 -> "pos"
    n < 0 -> "neg"
    else  -> "zero"
}

fun sum_to(n: i32).R: i32 {
    var acc = 0
    var i = 1
    while i <= n {
        acc += i
        i += 1
    }
    return acc
}

fun main() {
    var c = Circle(r = 2.0)
    val before = c.area()
    c.grow(by = 1.0)
    print("before = $before, after = ${c.area()}")
    log_area(c)
    print(classify(n = 0))
}
```

## 12. Milestone deltas

- **M0** — newline-sensitive statement terminator; `val`/`var`; `fun`; `.R: T`; `return expr`; `datac`; `print`/`print!`; `when` with `->`; `impl` imports; `pd` passdowns; no-paren conditions; braces required.
- **M1** — receiver-form extension functions (`fun T.m()` / `fun mut T.m()`); implicit shared borrow; `mut`-marked exclusive borrow; ownership + borrow checking.
- **M2** — string templates; named arguments at call sites with 2+ params; `pd` compiler wiring pass (wire passdowns only into functions that reference them).

### Conflicts with `plan.md` milestones (unresolved)

1. `plan.md` M1 is "types + C codegen + first running binary"; the M1 delta above wants receivers, ownership, and borrow checking there. `plan.md` reaches ownership at M3 and borrowing at M4. One of the two orderings has to give.
2. The M0 delta list is much wider than `plan.md` M0 ("functions, `let`, arithmetic, `if`, `return`, `println`"). `when`, `pd`, receivers, and the newline terminator are all new parser surface in the first milestone.
3. `&mut T` is no longer written at call sites or in receiver position (it became `mut`), but `plan.md` M4 fixtures are built around explicit `&mut T`. Restate those fixtures in the new spelling.

## 13. Divergence from Kotlin (kept intentionally)

- No exceptions. Error handling stays Rust-shaped.
- No GC. Ownership and borrows remain in the model.
- No collections in v1.
- References do not appear in field types in v1.

## 14. Loose ends in this spec

1. `String` and `usize` appear in the examples above but are not in `plan.md`'s v1 primitive list, and owned heap `String` was deferred until ownership makes it meaningful. Either `String` is v1 with an owned-buffer implementation, or the examples should read `&str`.
2. `pd` before `impl`, or `impl` before `pd`? Section 9 says "top of a file, before any `fun` or `datac`" — it does not fix the order relative to imports. Pick one canonical order.
3. `mut` on a binding vs `mut` on a type: `var c = Circle(...)` is a mutable binding, `fun mut Circle.grow()` and `s: mut String` are exclusive borrows. Same keyword, two jobs. Legal, but the error messages must distinguish them clearly.
4. `%*alias*%` substitution is specified in `plan.md` and is not mentioned here. It still applies, including inside `impl "..."` path strings.
5. `caps(domain = [...])` uses `=` rather than the `:` in `plan.md`. Confirmed deliberate here; `plan.md` needs updating.
6. `datac` constructor call style is `Circle(r = 2.0)` with positional allowed. Field mutability inside a `datac` (`var age: i32`) is distinct from `var` on a binding holding the `datac`. Say which one `c.r = 5` requires.
