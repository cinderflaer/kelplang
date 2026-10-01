# Freestanding mode (MK9-S4-M1/M2, kernel stage 4)

*A design note, not a Mark. `plan/kernel-path.md` owns the stage list; this is
the record of what the stage decided and why.*

## What it is

A project opts into bare metal with a `[target]` section in `coral.toml`:

```toml
[target]
mode = "freestanding"
triple = "riscv64-unknown-none-elf"
entry = "kmain"
linker_script = "kernel.ld"
panic = "kp_panic"          # optional
```

- `mode` is `"hosted"` or `"freestanding"` and is required in the section. A
  project with no `[target]` section, or with `mode = "hosted"`, builds for the
  host exactly as every project before this stage did; that is the whole
  compatibility promise.
- `triple` is any LLVM triple string. `riscv64-unknown-none-elf` is the one
  kernel 0 targets; `aarch64-unknown-none` and an x86-64 triple work as far as
  target-machine creation goes, but only riscv64 is exercised today.
- `entry` is a bare `fun` name in the entry module, with no parameters and no
  return.
- `linker_script` is a path relative to the directory holding `coral.toml`.
- `panic` is optional: the bare name of a `fun(msg: str)` that never returns
  normally. Without it, the compiler generates a handler whose body loops
  forever.

## The allowed set

There is no libc and no `kelp_rt.c` behind a freestanding image, so everything
that would reach either is a compile error rather than an undefined symbol the
linker reports. `kelpc check` enforces the same set `kelpc build` does, because
the check lives in the front end (`src/freestanding.rs`).

**Allowed.** Every integer width, `bool`, `char`, `f32`/`f64`, `unit`; `str` as
a borrowed literal in read-only data; user `datac` and `enum` values; arrays
`[T; N]`; borrows `&T`/`&mut T`; `if`, `when`, loops, functions, and closures
that capture nothing (a capturing literal owns a heap environment that only the
runtime could free); `Atomic<T>` and its methods, plus the pure `std/atomic`,
`std/option` and `std/result` modules; `Ptr<T>` and the compile-time
`size_of`/`align_of`/`offset_of`; a user `extern fun` whose module has its own
`[extern."<id>"]` section; and arithmetic, including the overflow and bounds
checks, which call the project's panic handler (below).

**Refused, each naming the construct.** `print`/`print!` (the `printf` call),
the builtin `panic` and `assert` (the runtime's `kelp_panic`/`kelp_assert`), a
defined `main` (the hosted entry point and its `argc`/`argv` globals),
passdowns (`pd`, whose initializers run through the generated `kelp_init_pds`
that only the hosted entry point calls), and the heap types `String`, `Bytes`,
`List`, `Map`, `Box`, `View` and `Thread`. Any `std/*` module other than the
pure three above is refused by name: they all reach libc or the allocator.

The list is deliberately a *closure over reachability*, not the language's
surface: `print` is refused as a call, `String` as a type wherever it appears
(a local, a parameter, a field, a return), and a `std/str` import as a module.
A shape not on the allowed list is refused rather than lowered, so a program
that compiles freestanding cannot reference a symbol the link would miss.

## The entry stub and sections

The compiler emits a `_start` symbol as module-level assembly (the LLVM
`module asm` string), because loading `sp` from a linker-script symbol is not
expressible in the language yet:

```asm
    .section .text.boot, "ax", @progbits
    .globl _start
_start:
    la sp, __stack_top
    call kelp_kmain
1:
    wfi
    j 1b
```

It clears nothing else, and if the entry function returns it loops on `wfi`.
`kelp_kmain` is the entry `fun`'s emitted symbol. `.text.boot` is where a
linker script should place the stub first.

`section("<name>")` is the clause that lets a script place other things: an
optional leading clause on a top-level `const` or `fun`, in either order round
`pub`, refused on any other item and on an `extern fun`. On a `fun` the
backend sets the function's section (LLVM `section`, C
`__attribute__((section(...)))`). On a `const` it must be a fixed-size array of
integers (`[1, 2, 3]` or `[0; 16]`), which is emitted as read-only data in that
section; any other const shape with a section is refused. Uses of the const
still fold to the literal, so the global is a symbol a script or another object
can reach, not a load.

RISC-V calls `section` and `repr` "contextual words": like `unsafe`, the word is
an identifier unless the following `(` or `fun` makes it a marker, so a program
may still use `section` as a variable name.

## The target machine and the link

`kelpc build` in freestanding mode is LLVM-only. `KELPC_ORACLE=c` refuses with a
message naming the mode and the backend, because the C backend's preamble is
libc. The LLVM path:

- loads the target's backend by name (`LLVMInitializeRISCV*`), rather than
  through LLVM's `LLVMInitializeAllTargets` macro: that macro is a C static
  inline whose expansion depends on the LLVM headers' own target list, and this
  host's headers registered no RISC-V through it.
- creates the `TargetMachine` for the triple with the `generic-rv64` CPU and
  `+m,+a,+c` features (riscv64 only; other triples keep the portable baseline
  and no features), the **static** relocation model and the **medium** code
  model, and sets the module's triple and data layout from it.
- writes the object, then links it with `ld.lld -T <script> -o <out> --static
  <obj>`. A missing `ld.lld` is an error naming it; `kelpc` never falls back to
  `cc`, which would link the hosted startup files back in. No runtime object, no
  per-program C object, no `-lm`.

`kelpc run` refuses a freestanding project: the artifact is a bare-metal ELF to
be loaded by a boot loader or an emulator, not a host process.

## Acceptance for this stage

`kelpc/tests/projects/freestanding_min` is a riscv64 project whose `kmain` loops
forever, with the entry in `.text.boot` and a `[u8; 4]` const in
`.rodata.boot`. `kernel.ld` places `.text` at 0x80000000 and defines
`__stack_top`. The test builds it and checks with `llvm-readelf` that the image
is ELF64 RISC-V with a 0x80000000 entry, with `llvm-objdump` that `_start`
disassembles to RISC-V including the `wfi` halt, and with `llvm-nm` that there
is no undefined symbol. `freestanding_print`, `freestanding_no_entry`,
`freestanding_main` and `freestanding_ptr_alloc` are the rejection fixtures. All
of these need `dev-kernel.nix`'s `ld.lld` and llvm
tools; where those are absent the acceptance test prints a skip rather than
failing, so the ordinary suite stays green.

There is no kernel yet: booting the image under QEMU with serial output is the
milestone the next stages reach, not this one.

## What this stage does not do

- No QEMU run. The artifact is proven to be a well-formed image, not a boot.
- No inline asm and no naked functions (stage 5): `_start` is a compiler-emitted
  assembly blob, not something the language can write. `sp` can only come from
  the linker script.
- No allocator hook. Anything that needs the heap is refused rather than served;
  `Ptr.alloc`/`Ptr.free` are refused (they are `malloc`/`free`) even though
  `Ptr<T>` itself is allowed.
- A large aggregate copy can make LLVM emit a `memcpy` or `memset` call, which
  it assumes libc provides. Nothing here maps those to an in-language loop, so
  `ld.lld` reports the undefined symbol and the project has to supply it. The
  acceptance program is a loop and reaches neither.
- `section` on a `datac`/`enum`, and per-field placement, are not planned.
