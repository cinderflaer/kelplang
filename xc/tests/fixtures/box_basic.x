// M8: the builtin owning pointer. A `Box<T>` holds one value on the heap or
// nothing at all, which is what makes a recursive `datac` expressible.
datac Pair(val a: i32, val b: i32)

fun main() {
    var b: Box<i32> = Box.empty()
    print(b.is_empty())
    b.set(7)
    print(b.get())
    print(b.is_empty())
    b.set(9)
    print(b.take())
    print(b.is_empty())

    val text = Box.of(String.from_str("hi"))
    print(text.get().as_str())

    // A box of a box: the element is a container rather than a scalar, so
    // the instantiation nests and so do the generated names.
    val nested = Box.of(Box.of(5))
    print(nested.get().get())

    // A box of a list.
    var xs: List<i32> = List.empty()
    xs.push(1)
    xs.push(2)
    val held = Box.of(xs)
    print(held.get().len())

    // Reassigning a `var` that *holds* a shared borrow rebinds the pointer
    // (`p = &two`); it does not write through the borrow, so it needs no
    // exclusivity.
    val one = Pair(a = 1, b = 2)
    val two = Pair(a = 3, b = 4)
    var p = &one
    print(p.a)
    p = &two
    print(p.a)
}
