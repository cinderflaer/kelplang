// MK5-S5: `Map<String>`. `get` borrows the value (copying it out would be a
// second owner), `get_or` clones the existing value and drops the fallback it
// no longer needs, `remove` moves the value out, and `values` clones. The
// borrow of `m` from `get` is scoped so an `insert` can follow it.
impl "std/str"

fun main() {
    var m: Map<String> = Map.new()
    m.insert("alpha", String.from_str("one"))
    m.insert("beta", String.from_str("two"))
    print(m.len())
    {
        val v = m.get("beta")
        print(v.as_str())
    }
    val existing = m.get_or("beta", String.from_str("fallback"))
    print(existing.as_str())
    val absent = m.get_or("gamma", String.from_str("fallback"))
    print(absent.as_str())
    m.insert("beta", String.from_str("TWO"))
    {
        val v = m.get("beta")
        print(v.as_str())
    }
    val taken = m.remove("alpha")
    print(taken.as_str())
    print(m.len())
    val keys = m.keys()
    print(keys.get(0).as_str())
    val values = m.values()
    print(values.get(0).as_str())
    m.clear()
    print(m.is_empty())
}
