# Incremental compilation: compilation units, slice 1

*Roadmap A-14. This is slice 1 of three: separate compilation. Slice 2 caches a
unit's object by the hash of its IR, slice 3 compiles units in parallel.*

## The decision

The measured cost of `kelpc build` is almost all backend: LLVM optimization,
code generation and the `cc` link. The front end (parse, check, borrow-check) is
3 to 4 ms on the whole corpus. So the front end stays **whole-program** and the
backend stops being one translation unit:

- the program is partitioned into **units**, one per source module id, plus one
  unit named `_support`;
- each unit becomes its own LLVM module, is optimized at the selected `-O` level
  with the same pass pipeline the whole-program build uses, and is compiled to
  its own object file;
- `driver.rs` links every unit object, the runtime object and the per-program
  `extern fun` object into the binary.

Slice 1 is the split and the link. Nothing is cached and nothing is parallel yet,
so an incremental build today costs *more* than a whole-program one (the same
work plus a second module's worth of declarations). The point is the shape the
next two slices need, and the equivalence it must preserve: the same program must
produce the same stdout and exit status either way.

## The flag

Three ways to turn the mode on, and one way it stays off:

- `kelpc build --incremental <target>` and `kelpc run --incremental <target>`;
- `[build] incremental = true` in `coral.toml`;
- default **off**: with the mode off, the backend emits exactly one LLVM module
  named `kelp`, exactly as before this slice, and every existing test passes
  untouched.

The C backend is the suite's oracle (`KELPC_ORACLE=c`), not a user surface, and
it **ignores the flag**: its generated C stays one translation unit, because the
whole point of the oracle is to be the other answer to the same question. A
`--incremental` build under the oracle is therefore an ordinary C build, and the
equivalence test compares the two backends' *programs*, not their object counts.

`KELPC_DUMP_UNITS=<dir>` writes each unit's unoptimized LLVM IR to
`<dir>/<unit>.ll`. It is how the determinism test reads the IR (identical
sources must produce byte-identical unit IR); it does nothing outside the mode.

## Unit assignment

One unit per source module id holds:

- that module's non-generic functions;
- the instances of **its** generic functions, whatever the type arguments (an
  instance of module `M`'s generic function is owned by `M`, not by the module
  that happened to instantiate it);
- its `statics` and its `section("...")` consts;
- the trampolines its `extern fun` declarations reach, which stay C in the
  per-program `externs.c` object (one object for the whole program, exactly as
  today: the C text is the C backend's own).

`main` and the program's own module are ordinary units.

One unit named `_support` holds everything with no single owning module:

- the compiler-generated per-type helpers: `drop`, `clone`, `eq`, `hash` and
  `order` for `datac`/`enum`, `List`, `Map`, `Box` and array instantiations;
- the string/bytes/list runtime helpers the backend emits as IR rather than
  taking from `kelp_rt.o`;
- closure thunks, the widening thunks and their environments, and the shims a
  known function value is widened through;
- the `&dyn` vtables and their thunks;
- panic helpers;
- the `pd` globals and the generated `kelp_init_pds`/`kelp_drop_pds`. A `pd`
  storage global is read by `main` and by the generated init function, so both
  readers sit outside the pd's own module; parking the storage in `_support`
  keeps the cross-unit reference in one direction instead of inventing an
  exported global per pd.

The name `_support` cannot collide with a module id: a module id is a path under
`src/` and a file must be a valid identifier, so it never starts with `_`.

## Cross-unit linkage

Every unit's module **declares** every function, static and pd global of the
program (a declaration costs no symbol in the object) and **defines** only what
it owns. Ownership is total: one symbol, one defining object.

- A symbol another unit references is external in its owner and a `declare` (or
  an `external global`) in the users; a symbol only its own unit uses is
  `internal`.
- The reference set is computed from the typed IR **before any module is
  built**: every callee, `FnRef`, closure literal, `StaticRef`, `pd` name, the
  dyn-vtable slot targets and the user `drop`/`hash`/`eq` methods the generated
  helpers call, plus two sources that are not in a unit's own bodies. The first
  is the widening of a *known* function value: its shim lives in `_support` and
  calls the function directly, so a function named as a value is referenced from
  outside its unit even when the reference was written inside it. The second is
  the per-program C object (`externs.c`): an `extern fun` body, the capability
  glue or a `std/rt` prototype can call a Kelp function across the object
  boundary in C, which no typed node shows, so the C text is scanned for the
  emitted symbol names. `main` is always external (the C runtime is what calls
  it). Everything `_support` defines is external, because a generated helper can
  be reached from any unit.
- An over-approximation is harmless (the symbol is merely visible); a missed
  reference is a link error, never a silent miscompile, which is why every
  equivalence test links the real binary.

Symbol names are already unique program-wide (module prefix plus type-argument
suffix) and stay exactly what they are today, so a unit's IR is reproducible:
emission order is a function of the program's own declaration order and of sorted
type/instantiation lists, never of a hash map's iteration order, and no counter or
pointer-derived name reaches a cross-unit symbol.

## Debug info and freestanding

- **Debug info is per unit.** The backend has no `-g` flag: DWARF is always on,
  and the builder is per lowering pass, so each unit gets its own compile unit
  naming its own first `.kp` file. A fault still resolves to the `.kp` file and
  line that caused it, and a multi-unit binary carries several compile units,
  which is ordinary DWARF.
- **Freestanding is refused by name** in this slice:
  `--incremental` with `[target] mode = "freestanding"` is an error naming the
  mode. The bare-metal path emits the entry stub, the panic state and the
  `probe-stack`/`noredzone` sweep at module scope, and its image is one ELF with
  no libc to link against; the split needs a single `_start` and one shared panic
  state rather than one per unit. That is its own slice. The refusal is a
  "not in this slice" decision, not a backend limitation: hosted LLVM is the
  default and the mode is defined for it.

## What this slice does not do

- It does not cache: stage 2 hashes each unit's IR and reuses the object.
- It does not compile in parallel: stage 3 does.
- It does not inline across units, and that is the documented cost of the mode:
  a call between two modules is a real call in the object, not an inlinable one.
  Inside a unit, the existing pipeline is unchanged.
- It does not change the front end, the borrow checker, the C backend, or any
  diagnostic.
- The unit artifacts are written to `<out>.units/<unit>.ll` and
  `<out>.units/<unit>.o`, next to the binary the build produces.
