# A tour of the language as of 2026-10

*Source for the wiki pages. Every program below is a file in `kelpc/tests/fixtures/` (named above each listing); each was built and run on both the default LLVM backend and the C oracle (`KELPC_ORACLE=c`), the two outputs were identical, and the output shown is the real one. Breaking changes are fine in Kelp, so this describes the language as it is, not as it was.*

## Traits: default methods, supertraits, builtin impls

A trait names methods with `Self` receivers. A method with a body is a default an impl may keep or override; `trait B: A` makes `impl B` require `impl A`; a trait may be implemented for the builtin types that carry methods (`i32`, `String`, ...). Dispatch is static: a generic function `<T: Shape>` is monomorphized per type, so there is no runtime cost (`plan/traits.md` 1-4).

`traits_default_body.kp`

```kelp
// A trait method may carry a default body (A-12 slice 3). An impl may take the
// default as written, override it, or omit every method of an all-default
// trait.

trait Shape {
    fun Self.area().return: i32
    fun Self.describe().return: i32 { return 7 }
    fun Self.summary().return: i32 { return self.area() + self.describe() }
}

trait Tagged {
    fun Self.tag().return: i32 { return 5 }
}

datac Circle(val r: i32)

impl Shape for Circle {
    fun Self.area().return: i32 = r * r
    fun Self.describe().return: i32 = 9
}

// `Tagged` declares only a default, so an empty block implements it.
impl Tagged for Circle {}

fun main() {
    val c = Circle(r = 3)
    print(c.area())
    print(c.describe())
    print(c.summary())
    print(c.tag())
}
```

Output (both backends):

```
9
9
18
5
```

`traits_supertrait.kp`

```kelp
// `trait Named: Shape` (A-12 slice 3): implementing `Named` requires
// implementing `Shape`, a `<T: Named>` may call `Shape`'s methods, and a
// `T: Named` satisfies a `T: Shape` bound transitively.

trait Shape {
    fun Self.area().return: i32
}

trait Named: Shape {
    fun Self.name().return: i32 { return 3 }
}

datac Circle(val r: i32)

impl Shape for Circle {
    fun Self.area().return: i32 = r * r
}

impl Named for Circle {}

fun area_of<T: Shape>(x: &T).return: i32 = x.area()
fun describe<T: Named>(x: &T).return: i32 = x.area() + x.name()
fun chained<T: Named>(x: &T).return: i32 = area_of(x) + describe(x)

fun main() {
    val c = Circle(r = 3)
    print(describe(c))
    print(chained(c))
}
```

Output (both backends):

```
12
21
```

`traits_impl_for_builtin.kp`

```kelp
// A trait implemented for builtin types that already carry methods (A-12 slice
// 3): the integer scalars and `String`. The impl has to be in the trait's own
// module, and a default may call a method of the same trait.

trait Doubler {
    fun Self.double().return: i32
    fun Self.triple().return: i32 { return self.double() + self.double() + self.double() }
}

trait Named {
    fun Self.label().return: String
}

impl Doubler for i32 {
    fun Self.double().return: i32 = self + self
}

impl Named for String {
    fun Self.label().return: String = String.from_str("a string")
}

fun twice<T: Doubler>(x: T).return: i32 = x.double() + x.double()
fun label_of<T: Named>(s: &T).return: String = s.label()

fun main() {
    val n = 6
    print(twice(5))
    print(n.triple())
    val s = String.from_str("hi")
    print(label_of(s))
}
```

Output (both backends):

```
20
36
a string
```

## Trait objects: `&dyn` and `Box<dyn>`

`&dyn Trait` is a borrowed fat pointer (data plus vtable); `Box<dyn Trait>` owns what it points at and drops it through the vtable. Both coerce implicitly from `&T` / `Box<T>` where the expected type is known. A trait with an associated type, a generic trait, or a method returning `Self` is not object safe and is refused by name (`plan/traits.md` 9, 12).

`traits_dyn_dispatch.kp`

```kelp
// A-12 slice 4: two types implementing one trait, called through `&dyn` in a
// loop over borrowed bindings.
trait Shape {
    fun Self.area().return: f64
    fun Self.kind().return: str
}

datac Circle(val r: f64)
datac Square(val s: f64)

impl Shape for Circle {
    fun Self.area().return: f64 = r * r * 3.0
    fun Self.kind().return: str = "circle"
}

impl Shape for Square {
    fun Self.area().return: f64 = s * s
    fun Self.kind().return: str = "square"
}

fun report(s: &dyn Shape).return: f64 {
    print(s.kind())
    return s.area()
}

fun main() {
    val c = Circle(r = 2.0)
    val q = Square(s = 3.0)
    var i = 0
    var total: f64 = 0.0
    while i < 2 {
        if i == 0 {
            total = total + report(&c)
        } else {
            total = total + report(&q)
        }
        i = i + 1
    }
    print(total)
}
```

Output (both backends):

```
circle
square
21.000000
```

`traits_boxdyn_factory.kp`

```kelp
// A-12 slice 7: a factory function returns a `Box<dyn Shape>` chosen by a flag.
// Each `return` coerces the box it built, moving the allocation rather than
// copying it, and a `&dyn` borrowed back out of the box goes on to a free
// function.
trait Shape {
    fun Self.area().return: f64
    fun Self.kind().return: str
}

datac Circle(val r: f64)
impl Shape for Circle {
    fun Self.area().return: f64 = r * r * 3.0
    fun Self.kind().return: str = "circle"
}

datac Square(val s: f64)
impl Shape for Square {
    fun Self.area().return: f64 = s * s
    fun Self.kind().return: str = "square"
}

fun describe(s: &dyn Shape).return: str = s.kind()

fun make(big: bool).return: Box<dyn Shape> {
    if big {
        return Box.of(Circle(r = 2.0))
    }
    return Box.of(Square(s = 3.0))
}

fun main() {
    val a = make(big = true)
    val b = make(big = false)
    print(describe(a.get()))
    print(describe(b.get()))
    print(a.area())
    print(b.area())
}
```

Output (both backends):

```
circle
square
12.000000
9.000000
```

`traits_boxdyn_list.kp`

```kelp
// A-12 slice 7: a `List<Box<dyn Shape>>` of two implementors, summed through
// the vtable. Each element is a fat owned pointer, and the list frees each one
// through its own vtable on the way out.
trait Shape {
    fun Self.area().return: f64
    fun Self.kind().return: str
}

datac Circle(val r: f64)
impl Shape for Circle {
    fun Self.area().return: f64 = r * r * 3.0
    fun Self.kind().return: str = "circle"
}

datac Square(val s: f64)
impl Shape for Square {
    fun Self.area().return: f64 = s * s
    fun Self.kind().return: str = "square"
}

fun main() {
    var xs: List<Box<dyn Shape>> = List.empty()
    xs.push(Box.of(Circle(r = 2.0)))
    xs.push(Box.of(Square(s = 3.0)))
    var i: usize = 0
    var total: f64 = 0.0
    while i < xs.len() {
        print(xs.get(i).kind())
        total = total + xs.get(i).area()
        i = i + 1
    }
    print(total)
}
```

Output (both backends):

```
circle
square
21.000000
```

## Associated types

A trait declares `type Item`, each impl binds it, and generic code names it as `I::Item`. The binding is checked against the declared bounds at the impl (`plan/traits.md` 10).

`traits_assoc_bound.kp`

```kelp
// A bounded associated type (A-12 slice 5): `type Entry: Named` is checked at
// the impl, and generic code reaches the bound's method through `R::Entry`.

trait Named {
    fun Self.name().return: str
}

trait Repo {
    type Entry: Named
    fun Self.entry().return: Self::Entry
}

datac Item(val n: i32)

impl Named for Item {
    fun Self.name().return: str = "item"
}

datac Store(val v: i32)

impl Repo for Store {
    type Entry = Item
    fun Self.entry().return: Item = Item(n = v)
}

fun describe<R: Repo>(r: &R).return: str {
    val e = r.entry()
    return e.name()
}

fun main() {
    val s = Store(v = 5)
    print(describe(s))
    print(s.entry().n)
}
```

Output (both backends):

```
item
5
```

`traits_assoc_two_types.kp`

```kelp
// A trait with an associated type (A-12 slice 5), implemented by two types and
// used through a generic function whose return type names the projection
// `S::Out`. Dispatch stays static: each call is the concrete type's own method.

trait Src {
    type Out
    fun Self.produce().return: Self::Out
}

datac A(val n: i32)
datac B(val n: i64)

impl Src for A {
    type Out = i32
    fun Self.produce().return: i32 = n * 2
}

impl Src for B {
    type Out = i64
    fun Self.produce().return: i64 = n * 3
}

fun produce_of<S: Src>(x: &S).return: S::Out = x.produce()

fun main() {
    val a = A(n = 3)
    val b = B(n = 4)
    print(produce_of(a))
    print(produce_of(b))
}
```

Output (both backends):

```
6
12
```

## Generic traits, `From` and `?`

A trait takes type parameters (`From<T>`); one type implements several instances. `?` converts a failure through `impl From<E1> for E2`, so a function returning `Result<_, AppErr>` propagates an `IoErr` with no hand-written `match` (`plan/traits.md` 11).

`from_conversion.kp`

```kelp
impl "std/result"
impl "std/convert"

enum IoErr {
    Bad(val code: i32)
}

enum ParseErr {
    Letter(val ch: char)
}

enum AppErr {
    Io(val code: i32, val note: String)
    Parse(val note: String)
}

impl convert.From<IoErr> for AppErr {
    fun from(value: IoErr).return: AppErr {
        return when value {
            Bad(code) -> AppErr.Io(code = code, note = "io")
        }
    }
}

impl convert.From<ParseErr> for AppErr {
    fun from(value: ParseErr).return: AppErr {
        return when value {
            Letter(ch) -> AppErr.Parse(note = "parse")
        }
    }
}

impl convert.From<i32> for AppErr {
    fun from(value: i32).return: AppErr = AppErr.Io(code = value, note = "int")
}

impl convert.Into<i32> for AppErr {
    fun Self.into().return: i32 = 42
}

fun io_fail().return: result.Result<i32, IoErr> {
    return result.Result.Err(error = IoErr.Bad(code = 7))
}

fun parse_fail().return: result.Result<i32, ParseErr> {
    return result.Result.Err(error = ParseErr.Letter(ch = 'x'))
}

fun lift_io().return: result.Result<i32, AppErr> {
    val v = io_fail()?
    return result.Result.Ok(value = v)
}

fun lift_parse().return: result.Result<i32, AppErr> {
    val v = parse_fail()?
    return result.Result.Ok(value = v)
}

fun make<X: convert.From<i32>>(v: i32).return: X {
    return X.from(value = v)
}

fun main() {
    print(result.is_err(r = lift_io()))
    print(result.is_err(r = lift_parse()))
    val a = make<AppErr>(v = 5)
    when a {
        Io(code, note) -> print(code)
        Parse(note) -> print(0)
    }
    val e = AppErr.from(value = IoErr.Bad(code = 9))
    when e {
        Io(code, note) -> print(code)
        Parse(note) -> print(0)
    }
    val e2 = AppErr.from(value = IoErr.Bad(code = 11))
    val n: i32 = e2.into()
    print(n)
    print(5)
}
```

Output (both backends):

```
true
true
5
9
42
5
```

`traits_generic_datac_method.kp`

```kelp
// A method on a generic `datac` whose body calls a bounded generic function
// with the receiver's own type parameter (A-12 slice 2). The bound is resolved
// when the *method* is instantiated, so `Pair<A, B>.total()` needs whatever `A`
// and `B` are to implement `Counted`, and the call inside it is checked per
// instantiation exactly as a free function's would be.

trait Counted {
    fun Self.count().return: i32
}

datac Meter(val n: i32)

impl Counted for Meter {
    fun Self.count().return: i32 = n
}

datac Pair<A, B>(val first: A, val second: B)

fun counted<T: Counted>(x: &T).return: i32 = x.count()

fun Pair<A, B>.total().return: i32 = counted(first) + counted(second)

fun main() {
    val p = Pair(first = Meter(n = 3), second = Meter(n = 4))
    print(p.total())
    print(counted(x = p.first))
}
```

Output (both backends):

```
7
3
```

## Operators, ordering and `Display`

`std/ops` supplies `Add`/`Sub`/`Mul`/`Div`/`Rem`/`Neg` (the operator calls the method with a borrowed right-hand side) and `Ord` (`<`, `<=`, `>`, `>=` through `cmp`); `==` still comes from a declared `eq`. A `Display` impl lets a value appear in `"${x}"` and `print` (`plan/syntax.md` 9.5.4).

`ops_arithmetic.kp`

```kelp
// Language-completeness 5: the arithmetic operators come from the shipped
// `std/ops` traits. `a + b` is `a.add(&b)` and the chain `a + b - c` is
// `a.add(&b).sub(&c)`, so each step is an ordinary method call and each operand
// is only borrowed, never moved.
impl "std/ops"

datac Vec2(val x: i64, val y: i64)

impl ops.Add for Vec2 {
    fun Self.add(rhs: &Self).return: Vec2 = Vec2(x = x + rhs.x, y = y + rhs.y)
}

impl ops.Sub for Vec2 {
    fun Self.sub(rhs: &Self).return: Vec2 = Vec2(x = x - rhs.x, y = y - rhs.y)
}

impl ops.Mul for Vec2 {
    fun Self.mul(rhs: &Self).return: Vec2 = Vec2(x = x * rhs.x, y = y * rhs.y)
}

impl ops.Div for Vec2 {
    fun Self.div(rhs: &Self).return: Vec2 = Vec2(x = x / rhs.x, y = y / rhs.y)
}

impl ops.Rem for Vec2 {
    fun Self.rem(rhs: &Self).return: Vec2 = Vec2(x = x % rhs.x, y = y % rhs.y)
}

impl ops.Neg for Vec2 {
    fun Self.neg().return: Vec2 = Vec2(x = 0 - x, y = 0 - y)
}

fun main() {
    val a = Vec2(x = 1, y = 2)
    val b = Vec2(x = 3, y = 4)
    val c = Vec2(x = 5, y = 6)

    val sum = a + b
    print(sum.x)
    print(sum.y)

    val chain = a + b - c
    print(chain.x)
    print(chain.y)

    val neg = -a
    print(neg.x)
    print(neg.y)

    val product = a * b
    print(product.x)
    print(product.y)

    val q = Vec2(x = 8, y = 9)
    val d = Vec2(x = 2, y = 3)
    print((q / d).x)
    print((q % d).y)

    // The operands are only borrowed, so both are still owned and usable.
    print(a.x)
    print(b.x)
}
```

Output (both backends):

```
4
6
-1
0
-1
-2
3
8
4
0
1
3
```

`ops_ord.kp`

```kelp
// Language-completeness 5: `Ord` gives `<`, `<=`, `>` and `>=` through `cmp`.
// The comparison is the `i32` `cmp` returns against zero, so a hand-written
// sort with a `while` loop is ordinary code.
impl "std/ops"

datac Version(val major: i32, val minor: i32)

impl ops.Ord for Version {
    fun Self.cmp(other: &Self).return: i32 {
        if major < other.major {
            return -1
        }
        if major > other.major {
            return 1
        }
        if minor < other.minor {
            return -1
        }
        if minor > other.minor {
            return 1
        }
        return 0
    }
}

fun main() {
    var a = Version(major = 2, minor = 0)
    var b = Version(major = 1, minor = 9)
    var c = Version(major = 2, minor = 1)

    print(a > b)
    print(b >= c)
    print(a <= b)
    print(c >= c)

    // A bubble pass repeated until nothing moves: three owned values and a
    // `while` loop, sorted by the overloaded `<` alone.
    var sorted = false
    while !sorted {
        sorted = true
        if b < a {
            val t = a
            a = b
            b = t
            sorted = false
        }
        if c < b {
            val t = b
            b = c
            c = t
            sorted = false
        }
    }
    print(a.major)
    print(a.minor)
    print(b.major)
    print(b.minor)
    print(c.major)
    print(c.minor)
}
```

Output (both backends):

```
true
false
false
true
1
9
2
0
2
1
```

`ops_display.kp`

```kelp
// Language-completeness 5: a type that implements `Display` may appear in
// `"${x}"` and as the argument of `print`, lowered to `x.to_text()`. The
// receiver is a shared borrow, so the value is only read, and a `to_text` may
// call another type's `to_text` to nest.
impl "std/fmt"

datac Vec2(val x: i64, val y: i64)

impl fmt.Display for Vec2 {
    fun Self.to_text().return: String {
        var out = String.empty()
        val xs = fmt.int(x)
        val ys = fmt.int(y)
        out.push_str("(")
        out.push_str(xs.as_str())
        out.push_str(", ")
        out.push_str(ys.as_str())
        out.push_str(")")
        return out
    }
}

datac Line(val a: Vec2, val b: Vec2)

impl fmt.Display for Line {
    fun Self.to_text().return: String {
        var out = String.empty()
        val at = a.to_text()
        val bt = b.to_text()
        out.push_str(at.as_str())
        out.push_str(" -> ")
        out.push_str(bt.as_str())
        return out
    }
}

fun main() {
    val v = Vec2(x = 3, y = 4)
    print("v = ${v}")
    print(v)
    // Still owned after both: the desugar only borrows it.
    print(v.x)

    val l = Line(a = v, b = Vec2(x = 5, y = 6))
    print("line = ${l}")
    print(l)
    print(l.to_text())
}
```

Output (both backends):

```
v = (3, 4)
(3, 4)
3
line = (3, 4) -> (5, 6)
(3, 4) -> (5, 6)
(3, 4) -> (5, 6)
```

`matrix_display_generic.kp`

```kelp
// Interaction: `Display` x a generic function. `<T: fmt.Display>` lets the body
// call `to_text`, and the same body prints through a template.
impl "std/fmt"

datac Vec2(val x: i64, val y: i64)

impl fmt.Display for Vec2 {
    fun Self.to_text().return: String {
        var out = String.empty()
        val xs = fmt.int(x)
        val ys = fmt.int(y)
        out.push_str("<")
        out.push_str(xs.as_str())
        out.push_str(",")
        out.push_str(ys.as_str())
        out.push_str(">")
        return out
    }
}

datac Tag(val id: i64)

impl fmt.Display for Tag {
    fun Self.to_text().return: String {
        var out = String.empty()
        val s = fmt.int(id)
        out.push_str("#")
        out.push_str(s.as_str())
        return out
    }
}

fun show<T: fmt.Display>(x: &T).return: String = x.to_text()

fun twice<T: fmt.Display>(x: &T) {
    print("${x} ${x}")
}

fun main() {
    val v = Vec2(x = 1, y = 2)
    val t = Tag(id = 9)
    print(show<Vec2>(x = &v))
    print(show<Tag>(x = &t))
    twice<Vec2>(x = &v)
}
```

Output (both backends):

```
<1,2>
#9
<1,2> <1,2>
```

## Iterators, `break` and `continue`

`for x in expr` walks a `List`/array by index and anything else through `std/iter`'s `Iterator` (`type Item`, `fun mut Self.next().return: Option<Self::Item>`). `break` and `continue` target the innermost loop; `defer` runs once per iteration (`plan/iterators.md`).

`for_iter_range.kp`

```kelp
// `plan/iterators.md`: `for` over a `List`/array keeps the index walk, and
// everything else is the `Iterator` protocol. `Range` is the shipped iterator,
// and the loop is its `next()` desugar.
impl "std/iter"

fun main() {
    for i in iter.range(0, 5) {
        print(i)
    }
    // A step and a descending range are the same loop with a different step.
    for i in iter.range_step(0, 6, 2) {
        print(i)
    }
    for i in iter.range_step(3, 0, -1) {
        print(i)
    }
}
```

Output (both backends):

```
0
1
2
3
4
0
2
4
3
2
1
```

`for_iter_break.kp`

```kelp
// A user iterator whose sequence never ends: `break` is what stops the loop, and
// `continue` skips to the next `next()` call.
impl "std/option"
impl "std/iter"

datac Fib(var a: i64, var b: i64)

impl iter.Iterator for Fib {
    type Item = i64
    fun mut Self.next().return: option.Option<i64> {
        val v = self.a
        val n = self.a + self.b
        self.a = self.b
        self.b = n
        return option.Option.Some(value = v)
    }
}

fun main() {
    var f = Fib(a = 0, b = 1)
    for x in f {
        if x > 20 {
            break
        }
        print(x)
    }
    // `continue` reaches the next element: the even numbers below ten.
    var even_sum: i64 = 0
    for i in iter.range(0, 10) {
        if i % 2 != 0 {
            continue
        }
        even_sum += i
    }
    print(even_sum)
}
```

Output (both backends):

```
0
1
1
2
3
5
8
13
20
```

`matrix_iter_defer_return.kp`

```kelp
// Interaction: a user iterator x `defer`. A `defer` in the loop body runs once
// per iteration, including the iteration a `return` leaves, and a `defer` in the
// function runs after the loop is done.
impl "std/iter"
impl "std/option"

datac Countdown(var n: i32)

impl iter.Iterator for Countdown {
    type Item = i32
    fun mut Self.next().return: option.Option<i32> {
        if n <= 0 {
            return option.Option.None
        }
        val v = n
        n -= 1
        return option.Option.Some(value = v)
    }
}

fun first_even(limit: i32).return: i32 {
    defer print("leave")
    for x in Countdown(n = limit) {
        defer print("iter")
        if x % 2 == 0 {
            return x
        }
    }
    return -1
}

fun main() {
    print(first_even(5))
    print(first_even(1))
}
```

Output (both backends):

```
iter
iter
leave
4
iter
leave
-1
```

## Const generics

`datac Buf<const N: usize>` and `fun sum<const N: usize>(a: &[i32; N])` take a compile-time value; `N` is inferred from an array argument and each value is its own monomorphized copy. A type with a const parameter cannot itself implement a trait, so a small non-generic cursor walks it (`plan/const-generics.md`, `plan/traits.md` 14).

`const_generics_basic.kp`

```kelp
// A-20: a `const` type parameter. `Buf<const N: usize>` is one template whose
// length is a compile-time value, and `sum(a: &[i32; N])` infers `N` from the
// array argument's own length. `N` is a value inside the body (`i < N`), an
// array length (`[i32; N]`) and a repeat expression (`[0; N]`), and each
// distinct value monomorphizes into its own copy exactly as a type argument
// does.
const SIZE: usize = 2

datac Buf<const N: usize>(var data: [i32; N], var len: usize)

// A `bool` const parameter, read as a value in the body.
datac Flag<const ON: bool>(var value: i32)

fun Buf<const N: usize>.total().return: i32 {
    var sum: i32 = 0
    var i: usize = 0
    while i < N {
        sum = sum + data.get(i = i)
        i = i + 1
    }
    return sum
}

fun sum<const N: usize>(a: &[i32; N]).return: i32 {
    var total: i32 = 0
    var i: usize = 0
    while i < N {
        total = total + a.get(i = i)
        i = i + 1
    }
    return total
}

fun zeros<const N: usize>().return: [i32; N] = [0; N]

fun flag<const ON: bool>().return: i32 {
    if ON {
        return 1
    }
    return 0
}

fun main() {
    val b = Buf(data = [1, 2, 3], len = 3)
    print(b.data.len())
    print(b.len)
    print(b.total())

    val arr: [i32; 4] = [10, 20, 30, 40]
    print(sum(a = &arr))

    // `N` inferred from the argument's type, with nothing else to go on.
    val pair: [i32; 2] = [3, 4]
    print(sum(a = &pair))

    // A repeat expression over the parameter, and the parameter as a value.
    val z = zeros<3>()
    print(z.len())
    print(z.get(i = 0))

    // A const argument written as a `const`, and a `bool` const parameter.
    val sized: Buf<SIZE> = Buf(data = [7, 8], len = 2)
    print(sized.total())
    print(flag<true>() + flag<false>())
    print(Flag<true>(value = 5).value)
}
```

Output (both backends):

```
3
3
6
100
7
3
0
15
1
5
```

`matrix_iter_const_generic_ok.kp`

```kelp
// Interaction: iterators x const generics, the way that works. The const-generic
// buffer keeps its length in its type; a small non-generic cursor implements
// `Iterator` over indexes, and a generic function walks any `Ring<N>` with it.
impl "std/iter"
impl "std/option"

datac Ring<const N: usize>(var data: [i32; N])

datac Cursor(var at: usize, val len: usize)

impl iter.Iterator for Cursor {
    type Item = usize
    fun mut Self.next().return: option.Option<usize> {
        if at >= len {
            return option.Option.None
        }
        val v = at
        at += 1
        return option.Option.Some(value = v)
    }
}

fun Ring<const N: usize>.cursor().return: Cursor = Cursor(at = 0, len = N)

fun total<const N: usize>(r: &Ring<N>).return: i32 {
    var sum: i32 = 0
    for i in r.cursor() {
        sum += r.data.get(i = i)
    }
    return sum
}

fun main() {
    val a: Ring<3> = Ring(data = [4, 5, 6])
    val b: Ring<2> = Ring(data = [10, 20])
    print(total(r = &a))
    print(total(r = &b))
    for i in a.cursor() {
        print(a.data.get(i = i))
    }
}
```

Output (both backends):

```
15
30
4
5
6
```

## Scoped trailing blocks and exclusive captures

`f { x -> ... }` passes a block to a `scoped fun(...)` parameter. The block may read what it captures by shared borrow and, new in this round, write it: a `var` or `mut` parameter written inside the block is borrowed exclusively for the call, so the Elm-style drain loop and tallies need no message type. A scoped block still cannot be stored (`plan/scoped-blocks.md`, `plan/exclusive-captures.md`).

`scoped_block_params.kp`

```kelp
// `plan/scoped-blocks.md` part 1 and part 2: a trailing block declares its
// parameters by name only, takes their types from the `scoped fun(...)` the
// block is passed to, and returns its tail when that type's return is
// non-unit. The callee calls it during the call and only during the call.
fun each(xs: List<i32>, f: scoped fun(i32)) {
    var i: usize = 0
    while i < xs.len() {
        f(xs.get(i))
        i += 1
    }
}

fun sum(xs: List<i32>, f: scoped fun(i32).return: i32).return: i32 {
    var total = 0
    var i: usize = 0
    while i < xs.len() {
        total += f(xs.get(i))
        i += 1
    }
    return total
}

// One scoped parameter forwarded to another: the value stays inside the call.
fun twice(f: scoped fun(i32)) {
    f(1)
    f(2)
}

fun main() {
    var xs: List<i32> = List.empty()
    xs.push(4)
    xs.push(5)
    xs.push(6)

    each(xs) { n -> print(n) }
    print(sum(xs) { n -> n * 10 })
    twice { n -> print(n + 100) }

    // A value-returning block, and one whose tail is an `if` expression.
    print(sum(xs) { n -> n - 1 })
    print(sum(xs) { n -> if n > 5 { n } else { 0 } })
}
```

Output (both backends):

```
4
5
6
150
101
102
12
6
```

`excl_capture_field.kp`

```kelp
// A scoped block may write through a captured `var`: `s.count += n` makes the
// capture exclusive for the call (`plan/exclusive-captures.md`). The callee calls
// the block twice; the owner is usable again as soon as the call returns.
datac State(var count: i32, var label: String)

fun each(f: scoped fun(i32)) {
    f(1)
    f(2)
}

fun main() {
    var s = State(count = 10, label = String.from_str("x"))
    each { n ->
        s.count += n
    }
    print(s.count)
    s.count += 100
    print(s.count)
}
```

Output (both backends):

```
13
113
```

`excl_capture_copy_var.kp`

```kelp
// A `Copy` `var` the block writes is captured by exclusive borrow too, so a
// plain counter works: the writes are visible after the call, and a read of the
// same name inside the block sees the running value.
fun each(f: scoped fun(i32)) {
    f(1)
    f(2)
    f(3)
}

fun main() {
    var total = 0
    var calls = 0
    each { n ->
        total += n
        calls += 1
        if total > 3 {
            total += 1000
        }
    }
    print(total)
    print(calls)
}
```

Output (both backends):

```
1006
3
```

`excl_capture_drain.kp`

```kelp
// The Elm-style drain loop without threading the model through the callback:
// `pump` captures its `mut` parameter, `main` a local `var`, and both pass the
// capture on to `update`/`mark` as an ordinary `mut` argument.
datac Model(var count: i32, var marks: List<i32>)
enum Msg {
    Inc
    Add(val n: i32)
}
datac Queue(var pending: List<i32>)

fun mut Queue.drain(f: scoped fun(Msg)) {
    var i: usize = 0
    while i < pending.len() {
        val n = pending.get(i)
        if n == 0 { f(Msg.Inc) } else { f(Msg.Add(n = n)) }
        i += 1
    }
    pending = List.empty()
}

fun update(state: mut Model, msg: Msg) {
    when msg {
        Inc -> { state.count += 1 }
        Add(n) -> { state.count += n }
    }
}

fun mark(state: mut Model) { state.marks.push(state.count) }

fun pump(state: mut Model, q: mut Queue) {
    q.drain { msg ->
        update(state, msg)
        mark(state)
    }
}

fun main() {
    var m = Model(count = 0, marks = List.empty())
    var q = Queue(pending = List.empty())
    q.pending.push(0)
    q.pending.push(5)
    pump(m, q)
    q.pending.push(2)
    q.drain { msg ->
        update(m, msg)
        mark(m)
    }
    var i: usize = 0
    while i < m.marks.len() {
        print(m.marks.get(i))
        i += 1
    }
    print(m.count)
}
```

Output (both backends):

```
1
6
8
8
```

## What the compiler says when you get it wrong

Each of these is a fixture that is refused; the text is the compiler's, verbatim.

- **An associated type cannot be boxed as a trait object.** (`matrix_assoc_boxdyn.kp`)

  ```
  error: 'Box<dyn Source>' is not allowed: the trait declares the associated type 'Item', and a trait object does not know which type an implementor binds; use a bounded type parameter ('<T: Source>') instead
  ```

- **An operator trait takes no type argument.** (`matrix_generic_trait_operator.kp`)

  ```
  error: trait 'std.ops.Add' takes no type arguments, and 'impl std.ops.Add for Vec2' writes 1: write 'impl std.ops.Add for Vec2' (an operator trait's right-hand side is always '&Self', so there is no 'Add<Other>')
  ```

- **A const-generic type cannot implement a trait.** (`matrix_iter_const_generic.kp`)

  ```
  error: implementing a trait for a generic instantiation is not in this slice: a trait is implemented for a declared 'datac' or 'enum' name, or for a builtin type that already carries methods; wrap the instantiation in a non-generic 'datac' and implement the trait for that, or give the generic type an inherent method instead
  ```

- **`?` inside `next` must keep the `Option` shape.** (`matrix_from_try_in_next.kp`)

  ```
  error: '?' here propagates a carrying failure, but the enclosing function returns 'std.option.Option<i32>', the two have to be the same shape; match the value instead (`when r { Ok(v) -> ..., Err(e) -> return <your failure> }`), or make the function return the same kind of enum
  ```

- **A block inside a block cannot hold the outer block's exclusive capture.** (`excl_refused_nested.kp`)

  ```
  error: cannot capture 's' into this nested block: it is an exclusive capture of the enclosing block, and a block inside a block cannot hold it; pass 's' to the inner call as an ordinary argument, or capture it in the outer function
  ```

- **A block's exclusive capture conflicts with the call's own arguments.** (`excl_refused_same_call_arg.kp`)

  ```
  error: cannot borrow 's': already borrowed exclusively
  ```

- **A bound the type does not meet.** (`traits_bound_unimplemented.kp`)

  ```
  error: the call to 'area_of' needs 'Point' to implement the trait 'Shape', and it does not: write 'impl Shape for Point { ... }' in the trait's module or in the type's
  ```

- **`for` over something that is not iterable.** (`for_iter_no_impl.kp`)

  ```
  error: 'for' walks a 'List<T>' or a type that implements 'Iterator', and 'i32' is neither: implement 'std.iter.Iterator' for it ('type Item' and 'fun mut Self.next().return: option.Option<Self::Item>'), or walk a 'List' by index
  ```

- **`?` with no `From` impl.** (`from_no_impl.kp`)

  ```
  error: '?' cannot convert a failure: 'std.result.Result<i32, IoErr>' carries 'IoErr' and the enclosing function returns 'std.result.Result<i32, AppErr>', whose failure carries 'AppErr', write 'impl From<IoErr> for AppErr { fun from(value: IoErr).return: AppErr { ... } }' so the operator has a conversion to call
  ```

- **An operator with no trait impl.** (`ops_arith_no_add.kp`)

  ```
  error: cannot use '+' on 'P': it does not implement 'Add' from 'std/ops' (write 'impl Add for P { fun Self.add(rhs: &Self).return: P }', importing 'std/ops')
  ```

