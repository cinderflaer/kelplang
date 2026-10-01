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
`std/option` and `std/result` modules, and the five `std/str` operations the
compiler lowers itself (`len`, `byte`, `eq`, `starts_with` and `ptr`, kernel
stage 7); `Ptr<T>` and the compile-time
`size_of`/`align_of`/`offset_of`; a user `extern fun` whose module has its own
`[extern."<id>"]` section; a call to the builtin `panic(msg)`, which lowers
to a call to the same handler as the checks; and arithmetic, including the
overflow and bounds checks, which call the project's panic handler (below).

**Refused, each naming the construct.** `print`/`print!` (the `printf` call),
`assert` (the runtime's `kelp_assert`), a defined `main` (the hosted entry point
and its `argc`/`argv` globals), `Ptr.alloc`/`Ptr.free` (the hosted
`malloc`/`free`), passdowns (`pd`, whose initializers run through the generated
`kelp_init_pds` that only the hosted entry point calls), and the heap types
`String`, `Bytes`, `List`, `Map`, `Box`, `View` and `Thread`. Any `std/*`
module other than the pure three above is refused by name: they all reach libc
or the allocator.

`std/str` is the one module that is partly allowed, because a kernel that
prints anything has to walk text. The five operations above are the whole of
it: their raw primitives (`byte_len_raw`, `byte_at_raw`, `byte_ptr_raw`) have
freestanding lowerings emitted into the module by the LLVM backend
(`codegen_llvm::emit_freestanding_str_bodies`), as byte loops marked `optnone`
so the target's own loop-idiom pass cannot rewrite the NUL scan into a `strlen`
call; `eq` and `starts_with` are the hosted bodies on top of the same loops.
The rest of the module is refused both at the call site (by name) and, because
a dead body is still a `strlen`/`malloc` relocation at `-O0`, in emission:
`freestanding::prune` drops the module's other declarations, and `std/mem`
(which it imports), from the AST before inference, so the concrete types they
would instantiate never exist either. The hosted `std/str` is unchanged.

The list is deliberately a *closure over reachability*, not the language's
surface: `print` is refused as a call, `String` as a type wherever it appears
(a local, a parameter, a field, a return), and a `std/*` module as both.
A shape not on the allowed list is refused rather than lowered, so a program
that compiles freestanding cannot reference a symbol the link would miss.

## The entry stub and sections

The compiler emits a `_start` symbol as module-level assembly (the LLVM
`module asm` string), because loading `sp` from a linker-script symbol is not
expressible in the language yet:

```asm
    .section .text.boot, "ax", @progbits
    .weak __bss_start
    .weak __bss_end
    .globl _start
    .type _start, @function
_start:
    la sp, __stack_top
    la t0, __bss_start
    la t1, __bss_end
.Lkelp_bss_loop:
    bgeu t0, t1, .Lkelp_bss_done
    sd zero, 0(t0)
    addi t0, t0, 8
    j .Lkelp_bss_loop
.Lkelp_bss_done:
    call kelp_kmain
1:
    wfi
    j 1b
    .size _start, . - _start
```

`sp` is loaded first, and nothing between it and the entry call touches memory
except the clear loop itself, so a kernel's first instruction already has a
stack and already has zeroed statics; no register is read before it is written
and no stack frame exists yet.

The two bounds are `.weak` (kernel gap 4): a linker script that does not assign
them still links, both resolve to 0, and the loop runs zero times, which is
exactly the pre-gap-4 behaviour. A script that does assign them should put both
around the same word-aligned range, with `.bss.dirty`, `.sbss`, `.bss` and
`COMMON` inside it, e.g.

```ld
    .bss : ALIGN(8) {
        __bss_start = .;
        *(.sbss .sbss.*)
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(8);
        __bss_end = .;
    }
```

`examples/kernel0*` and the `freestanding_*` test projects carry exactly that.
The `.sbss` half is placed too, because a small-data build puts its small
statics there and a clear that misses them would be a kernel that boots with
garbage in a `static` it never wrote.

It loops on `wfi` if the entry function returns. `kelp_kmain` is the entry
`fun`'s emitted symbol. `.text.boot` is where a linker script should place the
stub first. Naming it first is not enough on its own: the script has to name
the other sections too, because lld places an unnamed orphan (`.eh_frame` is
the one a program with unwinding tables emits) before `.text`, and QEMU's
`-bios none` starts the image at 0x80000000 rather than the ELF entry, so
`_start` has to hold that address (`examples/kernel0/kernel.ld`,
`plan/kernel0-status.md`).

## The compiler's `mem*` fallbacks

A bare-metal image has no libc, but LLVM's own lowering of a large aggregate
copy, a whole-array fill or a generated clone calls `memcpy`, `memmove`,
`memset`, `memcmp` or `bcmp`. The compiler therefore defines all five in every
freestanding object, as byte loops generated as IR
(`codegen_llvm::emit_freestanding_mem_bodies`):

- `memcpy` and `memset` copy forward, `memmove` picks the direction from the two
  addresses, and `memcmp`/`bcmp` return the first differing byte's difference
  (so `bcmp`'s nonzero answer is also `memcmp`'s sign).
- Each is **weak** (`LLVMWeakAnyLinkage`), the same as C's
  `__attribute__((weak))`: an object the image links beside this one that
  defines any of them strongly wins, and a project's own `memcpy` is simply a
  strong definition in that object (`tests/projects/freestanding_memcpy_user`).
  A program cannot define one in Kelp, because a Kelp `fun`'s symbol is mangled.
- Each is marked `nobuiltin`, so LLVM's loop-idiom pass cannot rewrite the loop
  *inside* it into a call to the library function of the same name. The
  definition's own name is the other half of the same guard: LLVM refuses to
  turn a loop into a call to the function the loop is already in, so a body
  named `memcpy` survives while one LLVM renamed to `memcpy.1` would not.

The declarations are `declare_libc`'s own (the ones the compiler's aggregate
copies already call), so the widths are whatever that table uses: `size_t` is
`i64` on every triple today. A freestanding image that does not use any of them
still carries the five small loops; a project that links an object with a
strong definition of one of the names takes over every call LLVM emitted for
that name, and the other four stay weak.

`tests/projects/freestanding_memcpy` is the acceptance artifact: a 4 KiB
`datac` and a `[u8; 4096]` array copied whole, linked with zero undefined
symbols (`llvm-nm -u`), and `llvm-objdump -d` shows no `jal`/`call` inside any
of the five bodies.

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

Kernel gap 4's own acceptance is four more tests in
`kelpc/tests/compile_and_run.rs`, all skipped without `ld.lld` (and `llvm-mc`
for the weak-linkage one): `freestanding_memcpy` links a 4 KiB `datac` and a
`[u8; 4096]` array with zero undefined symbols, `llvm-nm` shows the five `mem*`
symbols weak and `llvm-objdump -d` shows no self-call in any body,
`examples/kernel0-bss` boots with `bss clear` only after the entry stub has
erased a 64-byte pattern the image itself loaded into `.bss`, and
`freestanding_memcpy_user` links a project's own strong `memcpy` in place of the
compiler's weak one. The `_start` clear loop is also checked by decoding its two
`la` sequences and comparing them to the `llvm-nm` addresses of `__bss_start`
and `__bss_end`.

## What this stage does not do

- No QEMU run. The artifact is proven to be a well-formed image, not a boot.
- No inline asm and no naked functions (stage 5): `_start` is a compiler-emitted
  assembly blob, not something the language can write. `sp` can only come from
  the linker script.
- No allocator hook. Anything that needs the heap is refused rather than served;
  `Ptr.alloc`/`Ptr.free` are refused (they are `malloc`/`free`) even though
  `Ptr<T>` itself is allowed.
- A large aggregate copy can make LLVM emit a `memcpy` or `memset` call, which
  it assumes libc provides. ~~Nothing here maps those to an in-language loop, so
  `ld.lld` reports the undefined symbol and the project has to supply it.~~
  Closed by kernel gap 4: all five `mem*` routines are defined in every
  freestanding object as weak loops (above), so an aggregate copy of any size
  links.
- `section` on a `datac`/`enum`, and per-field placement, are not planned.
