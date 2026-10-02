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
- `triple` is the LLVM triple the project targets. `riscv64-unknown-none-elf` and
  `aarch64-unknown-none` are the two kernel 0 boots, and the two the compiler
  has a target machine and a `_start` stub for; any other triple is refused by
  name at the stub, so an x86-64 image has to wait for its own entry stub
  (a 32-bit PVH/Multiboot one that enters long mode first).
- `entry` is a bare `fun` name in the entry module, with no parameters and no
  return.
- `linker_script` is a path relative to the directory holding `coral.toml`.
- `panic` is optional: the bare name of a handler that never returns
  normally. Two shapes are accepted: `fun(msg: str)` and the
  location-carrying `fun(msg: str, file: str, line: u32)`. Any other shape is a
  compile error that names both. Without the key, the compiler generates a
  handler whose body loops forever.

## The allowed set

There is no libc and no `kelp_rt.c` behind a freestanding image, so everything
that would reach either is a compile error rather than an undefined symbol the
linker reports. `kelpc check` enforces the same set `kelpc build` does, because
the check lives in the front end (`src/freestanding.rs`).

**Allowed.** Every integer width, `bool`, `char`, `f32`/`f64`, `unit`; `str` as
a borrowed literal in read-only data; user `datac` and `enum` values; arrays
`[T; N]`; borrows `&T`/`&mut T`; `if`, `when`, loops, functions, and closures
that capture nothing (a capturing literal owns a heap environment that only the
runtime could free); `Atomic<T>` and its methods, plus the admitted standard
modules (kernel stage 19, `plan/freestanding-std.md`): the pure `std/atomic`,
`std/arena`, `std/irq` and `std/frame`, `std/option`'s and `std/result`'s value
helpers, `std/math`'s integer `abs`/`min`/`max`/`clamp`, and the five `std/str`
operations the
compiler lowers itself (`len`, `byte`, `eq`, `starts_with` and `ptr`, kernel
stage 7); `Ptr<T>` and the compile-time
`size_of`/`align_of`/`offset_of`; a user `extern fun` whose module has its own
`[extern."<id>"]` section; a call to the builtin `panic(msg)`, which lowers
to a call to the same handler as the checks; and arithmetic, including the
overflow and bounds checks, which call the project's panic handler (below).
With `[target] alloc`/`dealloc` configured, `Box<T>`, `List<T>` and `Map<V>` of
allowed element types are allowed too, and so are `String` and `Bytes` (the
next section), with only the operations that have a libc-free lowering -
`String.empty`/`from_str`/`len`/`is_empty`/`as_str`/`clone`/`push`/`push_str`/
`reserve`/`with_capacity`/`clear`, `Bytes.empty`/`from_str`/`len`/`is_empty`/
`get`/`set`/`push`/`clone`/`reserve`/`clear`, `str.concat` and the integer
builders, and drop - callable; every other `String`/`Bytes` method and every
other `std/str` function is refused by name with "needs the hosted runtime"
(`plan/freestanding-string.md`). The same pair admits `std/list`, whose every
function takes a closure, and the `std/option`/`std/result` callback
combinators, whose environments are heap boxes; without it each of those is
refused by name with the two manifest keys.

**Refused, each naming the construct.** `print`/`print!` (the `printf` call),
`assert` (the runtime's `kelp_assert`), a defined `main` (the hosted entry point
and its `argc`/`argv` globals), `Ptr.alloc`/`Ptr.free` (the hosted
`malloc`/`free`), passdowns (`pd`, whose initializers run through the generated
`kelp_init_pds` that only the hosted entry point calls), `View` and `Thread`
(which reach the hosted runtime whatever the manifest says), `String` and
`Bytes` when no allocator is configured, and `Box`, `List` and `Map` when no
`[target] alloc`/`dealloc` pair is configured. Any `std/*` module whose row in
`freestanding::std_admission` is `Hosted` is refused by name with "needs a
hosted runtime" and the list of modules that are admitted: they all reach libc
or the allocator.

`std/str` is the one module that is partly allowed, because a kernel that
prints anything has to walk text. The listed operations are the whole of it:
the five raw primitives (`byte_len_raw`, `byte_at_raw`, `byte_ptr_raw`) have
freestanding lowerings emitted into the module by the LLVM backend
(`codegen_llvm::emit_freestanding_str_bodies`), as byte loops marked `optnone`
so the target's own loop-idiom pass cannot rewrite the NUL scan into a `strlen`
call; `eq` and `starts_with` are the hosted bodies on top of the same loops;
and `concat`/`from_u64`/`from_i64` are the hosted Kelp bodies, kept only when
an allocator is configured because they build an owned `String`. The rest of the
module is refused both at the call site (by name) and, because a dead body is
still a `strlen`/`malloc` relocation at `-O0`, in emission:
`freestanding::prune` drops the module's other declarations, and `std/mem`
(which it imports), from the AST before inference, so the concrete types they
would instantiate never exist either. The hosted `std/str` is unchanged.

The list is deliberately a *closure over reachability*, not the language's
surface: `print` is refused as a call, `String`/`Bytes` as a type wherever they
appear without an allocator (a local, a parameter, a field, a return),
`View`/`Thread` always, and a `std/*` module as both. A shape not on the allowed
list is refused rather than lowered, so a program that compiles freestanding
cannot reference a symbol the link would miss.

## The entry stub and sections

The compiler emits a `_start` symbol as module-level assembly (the LLVM
`module asm` string), because loading `sp` from a linker-script symbol is not
expressible in the language yet. The stub is per-architecture, selected by the
manifest's `triple`; the riscv64 one is:

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

and the aarch64 one is the same shape in AArch64 assembly:

```asm
    .section .text.boot, "ax", @progbits
    .weak __bss_start
    .weak __bss_end
    .globl _start
    .type _start, @function
_start:
    ldr x0, =__stack_top
    mov sp, x0
    ldr x0, =__bss_start
    ldr x1, =__bss_end
.Lkelp_bss_loop:
    cmp x0, x1
    b.hs .Lkelp_bss_done
    str xzr, [x0], #8
    b .Lkelp_bss_loop
.Lkelp_bss_done:
    bl kelp_kmain
1:
    wfe
    b 1b
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

It loops on the target's wait instruction (`wfi` on riscv64, `wfe` on aarch64)
if the entry function returns. `kelp_kmain` is the entry
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

## The allocator hook (kernel stage 7, MK9-S5-M1)

A freestanding project that wants the heap types names the functions that own
its memory:

```toml
[target]
mode = "freestanding"
# ... triple, entry, linker_script, panic ...
alloc = "kmalloc"
dealloc = "kfree"
realloc = "krealloc"      # optional
```

All three are bare names of `fun`s in the entry module,
`extern`-free, with fixed signatures: `fun alloc(size: usize, align: usize).R:
Ptr<u8>` (the null pointer on failure), `fun dealloc(p: Ptr<u8>, size: usize,
align: usize)`, and `fun realloc(p: Ptr<u8>, old: usize, new: usize, align:
usize).R: Ptr<u8>`. `alloc` and `dealloc` are required together; `realloc` needs
the pair. A wrong signature is a compile error naming the expected one.

With a pair configured, `Box<T>`, `List<T>` and `Map<V>` are allowed whenever
their element/key/value types are allowed, and `String` and `Bytes` themselves
are allowed with the operation subset above (`plan/freestanding-string.md`).
Every allocation and free those types make in the LLVM output calls the
project's functions instead of libc's `malloc`, `realloc` and `free`, and the
two memory primitives a freestanding image has no libc for (`memcpy`,
`memset`) are emitted as module-private `kelp_memcpy` and `kelp_memset`
definitions. Without a `realloc`, growth is alloc + copy + dealloc. The
generated object still references no libc symbol, which the acceptance test
checks with `llvm-nm -u`. With no pair configured the containers and
`String`/`Bytes` are refused exactly as before, with a message that names the
`alloc`/`dealloc` keys; `View` and `Thread` are refused either way.

`Ptr.alloc`/`Ptr.free` stay refused: they are the hosted `malloc`/`free` and
this stage does not reroute them. A failed allocation (the project's `alloc`
returns null) calls the project's panic handler with `"out of memory"`, so it is
a panic and never a null dereference.

`plan/kernel-ub-notes.md` carries the stage's undefined behaviour: everything
the allocator does with the memory it returns is a promise the compiler does not
check (alignment, size, aliasing, double free).

## Panic semantics with no OS (kernel stage 15, MK9-S6-M2)

Every compiler-inserted check calls one compiler-emitted entry,
`kelp_panic_entry(msg, file, line)`, never the project's handler directly.
`plan/panic-semantics.md` is the design; this is what it means for the mode.

- The handler may be `fun(msg: str)` or `fun(msg: str, file: str, line: u32)`.
  The `file` is the source file name (`main.kp`) and the `line` is the 1-based
  line of the statement the check was inserted into, from the same
  `SourceMap` the hosted line tables use.
- The entry exchanges a `.bss` flag and calls `kelp_double_panic` on re-entry.
  That symbol is always defined, **weakly**, as a `wfi`/`wfe` loop, so an image
  links with no undefined symbol and a project's own strong definition (an
  object linked beside the compiler's, the way `freestanding_memcpy_user`
  supplies `memcpy`) wins. `examples/kernel0-panic2` proves exactly that: its
  handler indexes out of bounds and the boot prints `double panic` from the
  project's assembly definition rather than wedging.
- The location reaches checks inside *generated* bodies (`Array.get`'s bounds
  check, the container sets) through the two globals `kelp_panic_file` and
  `kelp_panic_line`, which each `.kp` statement stores before the check runs.
  A `naked fun` gets no store: its body is one `asm` and an instruction before
  it would break the ABI (this is why `examples/kernel0-trap` still boots).
- With no `[target] panic`, the entry still exists and its first-panic branch
  calls the generated infinite-loop handler; the image prints nothing and
  stops, exactly as before.

The acceptance is `examples/kernel0-panic2`, booted by its own `run.sh` and by
`kernel0_panic2_names_each_checks_message_and_location`: six phases, each
asserting the exact `panic: <message> at <file>:<line>` line, with the last one
asserting `double panic`.

## The target machine and the link

`kelpc build` in freestanding mode is LLVM-only. `KELPC_ORACLE=c` refuses with a
message naming the mode and the backend, because the C backend's preamble is
libc. The LLVM path:

- loads the target's backend by name (`LLVMInitializeRISCV*` for riscv64,
  `LLVMInitializeAArch64*` for aarch64), rather than
  through LLVM's `LLVMInitializeAllTargets` macro: that macro is a C static
  inline whose expansion depends on the LLVM headers' own target list, and this
  host's headers registered no RISC-V through it.
- creates the `TargetMachine` for the triple. riscv64 gets the `generic-rv64`
  CPU, `+m,+a,+c` features and the **medium** code model; aarch64 gets the
  `generic` CPU, no extra features and the **small** code model, because
  LLVM refuses `medium` on AArch64 (`only small, tiny and large code models are
  allowed`) and the small model still reaches an image linked at 0x40000000.
  The relocation model is **static** for both, and the module's triple and data
  layout are set from the machine.
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
- No `Ptr.alloc`/`Ptr.free` hook. The global allocator of kernel stage 7 is
  `[target] alloc`/`dealloc`; the `Ptr` methods stay the hosted `malloc`/`free`
  and are still refused, even though `Ptr<T>` itself is allowed.
- ~~`String` and `Bytes` stay refused even with an allocator configured~~
  Closed by kernel stage 12: with an allocator, the operations with a libc-free
  lowering are legal and the rest are refused by name
  (`plan/freestanding-string.md`).
- A large aggregate copy can make LLVM emit a `memcpy` or `memset` call, which
  it assumes libc provides. ~~Nothing here maps those to an in-language loop, so
  `ld.lld` reports the undefined symbol and the project has to supply it.~~
  Closed by kernel gap 4: all five `mem*` routines are defined in every
  freestanding object as weak loops (above), so an aggregate copy of any size
  links.
- `section` on a `datac`/`enum`, and per-field placement, are not planned.
