// M8: a recursive `datac`. `next` is an owned pointer, so the type has a
// finite size; `Box.empty()` is the terminator, and `Box.of` prepends. The
// drop is recursive, which is why this fixture is in the AddressSanitizer
// pass: every node's box has to be freed exactly once.
datac Node(val value: i32, val next: Box<Node>)

fun sum_list(list: Node).R: i32 {
    if list.next.is_empty() {
        return list.value
    }
    return list.value + sum_list(list.next.get())
}

fun depth(list: Node).R: i32 {
    if list.next.is_empty() {
        return 1
    }
    return 1 + depth(list.next.get())
}

fun main() {
    var list = Node(value = 1, next = Box.empty())
    // `list = Node(next = Box.of(list))` moves the old head into the new box:
    // the assignment must not also free it (the value is not there any more).
    list = Node(value = 2, next = Box.of(list))
    list = Node(value = 3, next = Box.of(list))
    print(sum_list(list))
    print(depth(list))
    print(list.value)
    print(list.next.get().value)
    print(list.next.get().next.get().value)

    // A longer list, so the recursive drop and the recursive walk both do
    // some work.
    var i: i32 = 0
    while i < 200 {
        list = Node(value = i, next = Box.of(list))
        i = i + 1
    }
    print(depth(list))
    print(sum_list(list))

    // `take` moves the head out and leaves the box empty; `set` puts a
    // borrowed value back without a second owner.
    var head = list.next.take()
    print(head.value)
    print(list.next.is_empty())
    list.next.set(head)
    print(list.next.get().value)
}
