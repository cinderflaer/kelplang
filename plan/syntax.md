# The "X" language — syntax and style

The syntax authority. Where any other plan file disagrees with this one about syntax or style, this file is correct.

Theme: Kotlin-shaped surface over a Rust-shaped memory model.

### Types: `datac`

`struct` is gone. Fields are declared `val` (immutable) or `var` (mutable). A `datac` holds **owned data only** — no reference fields in v1. The only owned heap type in v1 is the builtin `Bytes` (see "Standard library"); `&str` fields are therefore illegal.

```x
datac Circle(val r: f64)
datac Vec2(val x: f64, val y: f64)
datac User(val name: String, var age: i32)
```

`String` in that example is the owned heap string type, so it lands with the heap milestone (MK1-S4-M1) — `syntax.md` uses it as a `datac` field, and owned fields are the only kind a `datac` may hold.

Field assignment needs both: the field declared `var` in the `datac`, and the binding holding the `datac` reachable mutably.

```x
datac Counter(val id: i32, var count: i32)

fun bump(c: mut Counter) {
    c.count += 1        // legal: `count` is var in the datac, `c` is an exclusive borrow
}
```

`c.id = 5` is an error at any call site — `id` is `val` in the `datac`, and no binding mutability can fix that.

### Bindings: `val` / `var`

- `val` — immutable binding
- `var` — mutable binding

`let` and `let mut` are dropped entirely.

```x
val before = c.area()
var c = Circle(r = 2.0)
```

### Functions: `fun` + `.R: T`

- Keyword is `fun`, not `fn`.
- Return type is `.R: T`, written after the full signature. `.R` marks "this returns"; `: T` gives the type.
- No `.R` = returns `Unit`.
- `->` is never a return marker.

```x
fun Circle.area().R: f64 = 3.14159 * r * r
fun mut Circle.grow(by: f64) { r += by }
fun main() { ... }
```

### Return: `return expr`

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

### Receivers replace `self` / `impl` blocks / explicit `&`

- No `self` keyword. No `impl` method block.
- Receiver is declared in the signature: `fun Circle.area()`.
- Shared borrow is implicit — write nothing.
- Exclusive borrow is marked `mut`: `fun mut Circle.grow(by: f64)`.
- Free-function parameters: write `mut` only when mutation is needed; everything else is a shared borrow by default.

```x
fun Circle.area().R: f64 = 3.14159 * r * r        // shared receiver
fun mut Circle.grow(by: f64) { r += by }          // exclusive receiver

fun len(s: String).R: usize                       // shared borrow, inferred
fun push(s: mut String, ch: char) { ... }         // exclusive, marked
```

`&` / `&mut` survive only outside signatures: taking a borrow of a local (`val p = &c`, `val q = &mut c`) and dereferencing (`*p`). Parameter and receiver positions use the bare/marked forms above.

**Gap, resolved in MK1-S4-M1:** parameters are shared borrows by default, so there is no spelling for a parameter that *takes ownership* (a move-in) — and per the MK1-S4-M1 milestone entry below, there isn't one in v1. Callers pass a borrow or a local; a `datac` is only ever moved by local rebinding.

### Print

- `print(...)` — prints with a trailing newline.
- `print!(...)` — prints with no trailing newline.

Replaces `println` entirely.

```x
print!("loading")   // loading
print!(".")         // loading.
print("done")       // loading.done\n
```

### `->` is reserved for `when`

`->` appears only in `when` branches, as `condition -> value`. It never means "returns".

```x
fun classify(n: i32).R: &str = when {
    n > 0 -> "pos"
    n < 0 -> "neg"
    else  -> "zero"
}
```

`when` in v1 is the boolean/`else` form only. Exhaustiveness checking, enum subjects, and value patterns wait for enums.

### Module imports: `impl`

Module imports use `impl`, not `use`. `impl` means import; the `impl` method block it used to suggest is gone (receivers replaced it), so the keyword is free.

```x
impl "shapes/circle"
impl "logger" with caps(fs = ["/var/log/myapp/*"])
impl "net/http"
impl "time" as t
```

### Passdowns: `pd`

A passdown is a top-level binding available to every function without being passed explicitly. The compiler wires it in **only in functions that actually reference it** — functions that never mention it pay no cost.

```x
pd val name = expr
pd var name = expr
pd val name: T = expr
```

- Legal only in the file's top block, before any `fun` or `datac`, and **after** the `impl` lines (canonical order: `impl`, then `pd`, then `datac`/`fun`).
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
```

`log_area` uses both passdowns — wired in. `classify` uses neither — no wiring, no cost.

**Resolved (was: Unresolved):** `pd var` is mutable global state reachable from any function, and v1 has no closures and no `unsafe` to make that safe. The decision, recorded in full under "Current state": **`pd var` is read-only inside functions** — it can only be assigned at its own top-level declaration, so no function ever holds an exclusive borrow of a passdown and no cross-function exclusivity rule is needed. That is closer to option (b) than to (a) or (c): the feature is kept, but writability is confined to the top level rather than exposed inside functions. Implemented in MK2-S1-M2 — see that milestone entry for the wiring mechanism (hidden parameters) and for the module-scoping answer.

### Retained Kotlin-look rules

- No semicolons. Newline ends a statement. *(`syntax.md` §10 says exactly this. A later revision of this copy added "a line ending in a binary operator (`.`, `+`, `,`, `&&`, `=`) continues onto the next line" — that is **not implemented**: the lexer emits a statement-terminating newline at paren depth 0 regardless of a trailing operator, so a wrapped expression must be parenthesized. Kept here as an aspirational rule rather than deleted, but see open item 11.)*
- Types on the right with `:`, e.g. `r: f64`.
- No parens on `if` / `while` conditions. Braces always required.
- String templates: `"x = $before, area = ${c.area()}"`.
- Named arguments at call sites when 2+ params: `Circle(r = 2.0)`, `math.pow(base = 2.0, exp = 8.0)`.
- `Unit` is the void type and the implicit return type (replaces `unit`).
- `when` replaces long `if`/`else if` chains.
- Blank line between functions. `impl` lines grouped at top. 4 spaces. Hard 100-col.
- Format is canonical, so `xc fmt` is the enforcement story (later milestone, not v1).
- **User-code naming case is a configurable lint, not a fixed rule.** A project-level setting (name TBD — e.g. an `x.toml` field or an `xc fmt`/lint flag, whichever ends up hosting `xc fmt`'s config) picks the preferred case for function/variable/param names; whichever case is *not* selected produces a warning, never a compile error. Options, numbered so a config value can just be an index: `0` = `camelCase` (default when unset — `fun computeArea()`, `val itemCount`), `1` = `snake_case` (`fun compute_area()`, `val item_count`). Room to extend the list later (`PascalCase`, `SCREAMING_SNAKE_CASE` for consts once `const` exists, etc.) without renumbering — append, don't reorder. Far-future, lint-only either way: this is a *beauty warning* (`xc fmt`/a future lint pass), and code in any case must keep compiling regardless of the configured preference. Not scheduled to a milestone yet. Tension to resolve whenever this is actually built: the stdlib naming rule below already locks `snake_case` for stdlib functions/namespaces (`fs.read`, `math.checked_add`) — decide then whether the stdlib is simply exempt from the project's configured case (it reads as its own namespace, not user code) or should track it too.

### Full example (`main.x`)

```x
impl "shapes/circle"
impl "std/fs" with caps(fs = ["/var/log/myapp/*"])

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
