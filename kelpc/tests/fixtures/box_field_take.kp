// The way out of that rule: an owned field held in a `Box` can be *moved out*
// with `take()` and put back with `set()`, so a field can change owner
// without a partial move. Everything here is freed exactly once under
// AddressSanitizer.
datac Bag(val xs: Box<List<i32>>)

fun main() {
    var bag = Bag(xs = Box.of(List.of(1)))
    var taken = bag.xs.take()
    print(taken.len())
    print(bag.xs.is_empty())
    bag.xs.set(taken)
    print(bag.xs.get().len())
    // Reading *through* the box is a shared borrow, so growth has to go the
    // same way it came: take, change, set.
    var again = bag.xs.take()
    again.push(2)
    bag.xs.set(again)
    print(bag.xs.get().len())
}
