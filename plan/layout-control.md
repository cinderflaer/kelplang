# Layout control (kernel stage 2, MK9-S1-M2 / roadmap B-2)

*The design note for `repr(...)`, the compile-time layout queries, and explicit
enum discriminants. `language/kernel.md` (kelp wiki) is the stage list; this is stage 2.*

## What a `repr(...)` clause means

A `datac` may carry one clause before it:

- `repr(c)`: declaration order, the C alignment and padding rules. This is
  **also what a declaration with no clause already is**, and the point of the
  spelling is that it is now a guarantee rather than an accident: the C typedef
  and the LLVM struct body are the same layout, and `size_of`/`offset_of` report
  it. The default and `repr(c)` are the same declaration.
- `repr(packed)`: every field at alignment 1, no padding anywhere. No field's
  own alignment is raised or lowered; the whole type's alignment is 1.
- `repr(align(N))`: the C field layout, with the *type's own* alignment raised
  to `N`, a power of two literal. The size rounds up to a multiple of `N`, and a
  type that embeds it places it at an `N`-aligned offset.

One clause per declaration; a second one is a parse error. `repr(align(N))` and
`repr(packed)` are refused **on an enum**: an enum's only layout knob here is
the tag's storage type below.

Nothing in the language takes a reference to a `datac` *field* that outlives the
value, and v1 has no pointer-to-field that escapes (a borrow is call-scoped and
a `Ptr` is built only from a whole place), so `repr(packed)` has no
"reference to a misaligned field" hazard to state: the fields are simply read at
their packed offsets through a value or a borrow of the whole value.

## How the two backends write it

Both must agree byte for byte, so the clause reaches both emitters:

- C (`codegen_c.rs::write_struct`): `__attribute__((packed))` after `struct`, or
  `__attribute__((aligned(N)))` after the closing brace, which is the GNU C for
  the same two facts.
- LLVM (`codegen_llvm.rs::declare_aggregates`): a `packed` struct body
  (`LLVMStructSetBody`'s flag) for `repr(packed)`, and for `repr(align(N))` a
  **zero-length alignment anchor** as the body's first field: `[0 x iN]` for
  `N <= 16`, `[0 x <N x i8>]` above it. The anchor has size 0 and alignment `N`,
  so it raises the struct's alignment without moving a field, which is what the
  C attribute does. A GEP's field index is read from the name table the anchor is
  inserted into (`field_index`), so the shift is invisible to the rest of the
  backend.
- Layout queries are computed by `types.rs::type_layout` from the same rules, and
  fold to a `usize` literal in `infer.rs`, so **no code generator lowers a
  layout query**: both backends see an integer.

The check that this is not just self-consistency is
`tests/projects/ffi_layout`: a `repr(packed)` datac is built in Kelp, its address
is passed through a user `extern fun` to a project-local C helper, and C's
`sizeof`/`_Alignof`/`offsetof` and the helper's field reads are printed beside
Kelp's own `size_of`/`align_of`/`offset_of`. The C compiler is the judge.

## The queries

`size_of<T>()`, `align_of<T>()` and `offset_of<T>("field")` each fold to a
`usize`. `T` is a scalar, `bool`, `char`, an `extern type` handle, `Ptr<U>`, a
`datac` or an `enum`; everything else is refused by name. They are ordinary
expressions, so they work in a body, and they also fold in a `const` initializer
and an array length (`ConstEvaluator::layout_query`). Canonical order puts every
`const` before every `datac`, so a `const` fold has no type table and can only
query a builtin; a query of a declared type belongs in a body.

## Explicit enum discriminants

A **field-less** enum may give a variant an integer value:

```kelp
repr(u8) enum Mode { Off = 0, On = 1, Fast = 10 }
```

- The values are integer literals (a leading `-` allowed), unique across the
  enum, and each must fit the tag's storage type. An unwritten discriminator
  continues from the previous value plus one, starting at 0.
- An enum with a payload may not have any explicit discriminant; it is refused
  by name.
- `repr(u8|u16|u32|u64|i8|i16|i32|i64)` before the enum picks the tag's storage
  width; the default is `i32`. The tag is stored in that width in both backends
  (the C `kelp_tag` member and the LLVM outer struct's field 0), and the hash's
  widening and the derived order's comparison use its signedness.
- `m.to_int()` returns the variant's value as the tag's integer type;
  `Mode.from_int(n)` returns `Option<Mode>`, `None` when no variant holds `n`
  (so the program imports `std/option`). Both are lowered with shapes that
  already exist (`EnumTag` tests for `to_int`, `EnumLit` arms for `from_int`),
  so neither backend has a new node to lower. The in-memory tag stays the
  variant's ordinal; `to_int`/`from_int` are the translation to the declared
  values, and both backends translate identically.
- Kelp has no `i8`/`i16` *value* type, so `repr(i8)`/`repr(i16)` store a one- or
  two-byte tag but report it as `i32`, the narrowest Kelp integer that holds it.
  `repr(u8)`/`repr(u16)` report themselves.

## Not supported here

- `repr(...)` on a generic `datac`/`enum`: an instantiation takes the default
  `repr(c)` layout, and the clause is refused by name rather than ignored.
- `repr(packed)`/`repr(align(N))` on an enum.
- A layout query naming an owned container (`String`, `List<T>`, ...); the
  queried set is the machine-level types above.

## What this adds to the UB catalogue

See stage 2 of `language/unsafe-ub-catalogue.md` (kelp wiki).
