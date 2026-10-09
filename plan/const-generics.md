# Const generics and `impl Trait` in argument position (A-2 continuation, A-20)

*Language-completeness slice 6. The design was decided before the work started; this file records
what shipped, what it cost, and the two things it deliberately does not do.*

## What this slice adds

Two spellings that remove a reason to write a converter:

1. **Const type parameters.** A parameter of a `datac`, an `enum` or a `fun` may be a compile-time
   *value* rather than a type: `datac Buf<const N: usize>(var data: [i32; N], var len: usize)`,
   `fun sum<const N: usize>(a: &[i32; N]).return: i32`, `fun Buf<const N: usize>.push(...)`. The value
   is one of `usize`, `u8`, `u16`, `u32`, `u64`, `i32`, `i64`, `bool` and `char` (MISSING-Ey4Mt). A
   `char` value is a code point: `Tag<'x'>` is the instantiation with `'x'`, read as a `char` in the body.
2. **`impl Trait` in argument position.** `fun f(x: impl Shape, y: &impl Shape)` is sugar for two
   fresh bounded type parameters, one per `impl` occurrence.

A const parameter is the generic machinery already in place (A-2) with a value in the argument
slot, and `impl Trait` is the trait machinery (A-12) with a parameter the compiler names. Neither
adds a code path to a code generator.

## Const parameters

**A value rides the argument vector.** `Ty` gained one variant, `ConstInt { value, ty }` (A-20).
It is a `Ty` only so that the one monomorphization path can carry both kinds of generic argument:
it appears in an instantiation's argument list, in the instance key (`Buf<16>`), and in the
bindings while the template's fields and body are resolved. It is not a type, and it never
survives an instantiation as one:

- `resolve_base` refuses a `ConstInt` bound where a type is expected, with a message naming the
  value positions it may take.
- `array_len` reads a bare const parameter as an array length, and refuses arithmetic over one in
  a type by name (`[T; N + 1]`, `Buf<N * 2>`).
- `c_type`, `ty_mangle` and `type_layout` treat it as unreachable, because a real field, parameter,
  return or element type can never be one.

**The argument is a compile-time expression.** It is written exactly like a type argument
(`Buf<16>`, `Buf<SIZE * 2>`, `capacity<twice(n = 4)>()`), which means the parser has to tell a
const argument from a type argument before it knows the declaration. The rule is lookahead: an
integer or boolean literal, a leading `-`, or a name that is called or used in an arithmetic
expression is a const argument; everything else is a type. A bare or dotted name (`Buf<SIZE>`,
`Buf<math.PAGE>`) parses as a type and is reconstructed as the constant expression where the
parameter is a `const`. The folding is the existing constant evaluation (A-19) with the
parameter's declared type as the expected type, so a literal, a `const` of the same or another
module and a plain `fun` call all work.

**Inference is the same fixpoint as a type parameter.** `bind_type_params` gained the array shape:
a written `[T; N]` against an `[i32; 4]` binds `T = i32` and `N = 4`, so `sum(a = &arr)` needs no
annotation. A construction's array field binds the same way (`Buf(data = [1, 2, 3], ...)` infers
`N = 3`). A parameter neither determines is the ordinary "cannot infer" error, and the instance
key includes the value, so `Ring<4>` and `Ring<8>` are two types with two sets of generated
functions.

**`N` is a value in the body.** The instantiation's bindings are the function's `type_args` map, so
the identifier `N` resolves to a literal of the parameter's declared type before any expression
rule sees it: `i < N` is `i < 4`. `[0; N]` is an array length the same way any other length is,
and `Ring<N>` as a return type folds the parameter's own value as the const argument.

**A written `&[T; N]` parameter is a borrow.** A parameter's `&` is normally what the bare name
already means, but an array is a by-value type, so `&[i32; N]` is the one way to write "a borrow of
the caller's array". It resolves to a `Ty::Ref`, which is what makes the const-generic examples
(`fun sum<const N: usize>(a: &[i32; N])`, called as `sum(a = &arr)`) read the way they were
planned.

**Refused by name:** arithmetic on a const parameter in a type, a value that does not fit the
declared type, a const parameter used as a type, a type written where a const argument belongs and
a const argument where a type belongs, a `bool` parameter given a non-boolean value, a `char`
parameter given a number (and a number or `bool` parameter given a character), a `char` parameter used
as an array length, a const parameter of any other type, and a trait with a const parameter.

## `impl Trait` in argument position

The parser accepts `impl Name` as a whole parameter type (bare or behind `&`) and rewrites it at
the declaration: each occurrence becomes `#implN`, added to the function's type parameters with the
trait as its bound. Every later pass sees an ordinary generic function, so the bound check, the
monomorphization and both backends need no new case.

**Refused by name:** a return position (the parser refuses it where it is written, naming
`fun f<T: Trait>(...).return: T` and `Box<dyn Trait>` as the alternatives), a `let`, a field, an enum
payload, a container element, a nested position (`List<impl Shape>`), a receiver method's
parameter, and a trait method's signature.

## Backends

Both. There is no new codegen: a const instantiation is an ordinary `Ty::Struct` with an ordinary
per-instance function set, and the body only ever contains literals where the parameter was. The
freestanding half is LLVM-only for the same reason every freestanding build is (the C preamble is
libc).

## Tests

- `tests/fixtures/const_generics_basic.kp`: a `datac` `Buf<const N: usize>`, a method reading `N`,
  `sum` inferring `N` from an array argument, `[0; N]`, `N` as a value, a `const` argument and a
  `bool` const parameter. Runs on both backends.
- `tests/fixtures/const_generics_two_sizes.kp`: `Ring<4>` and `Ring<8>` written in a loop, both
  keeping their own state. Runs on both backends, and
  `a_const_generics_two_instantiations_emit_two_copies` reads the oracle's generated C back to
  prove each `kelp_Ring__...__l__N__g__` symbol is emitted twice.
- `tests/fixtures/const_generics_expr_arg.kp`: `Buf<SIZE * 2>`, `Buf<WIDTH>` computed by a `fun`,
  `capacity<twice(n = 4)>()`. Runs on both backends.
- `tests/fixtures/const_generics_mismatch.kp` (rejected): `[i32; 3]` where `[i32; 4]` is required,
  with both lengths in the diagnostic.
- `tests/fixtures/const_generics_arithmetic.kp` (rejected): `[0; N + 1]`, refused by name.
- `tests/fixtures/impl_trait_args.kp`: two `impl Shape` parameters taking two different shapes,
  one behind `&`, calling trait methods. Runs on both backends.
- `tests/fixtures/impl_trait_return_refused.kp` and `impl_trait_field_refused.kp` (rejected).
- `tests/projects/freestanding_constgen`: a kernel-style fixed buffer (`Ring<const N: usize>` with
  two instantiations) linked with `ld.lld`, with `llvm-nm -u` clean and both mangled names present,
  under `dev-kernel.nix`.
- Parser unit tests for the const spelling, the const-argument lookahead and the `impl Trait`
  desugar; `tests/fmt_corpus/const_generics.kp` for the formatter round trip.

## Not in this slice

- **Arithmetic on a const parameter in a type** (`[T; N + 1]`, `Buf<N * 2>`). The length an
  instantiation takes has to be foldable by the existing constant evaluation; a value computed from
  a parameter would need the evaluator to carry parameter bindings through every type position.
  Refused by name.
- **A const parameter on a trait, and a const argument in a trait's own argument list.** A trait is
  implemented for a type; a const there would be a second binding to infer.
- **A const parameter that is not an integer, `bool` or `char`** (a length is a count, and every other
  type is either not foldable or not comparable at compile time). `char` is in scope since
  MISSING-Ey4Mt: a code point folds, compares and prints like a character literal.
