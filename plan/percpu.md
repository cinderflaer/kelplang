# Per-CPU storage (kernel stage 8, MK9-S3-M3 / roadmap B-11)

*The design note for the `percpu` storage class and its two intrinsics.
`plan/kernel-path.md` is the stage list; this is row 8. The
undefined-behaviour half is `plan/kernel-ub-notes.md`'s stage 11 section.*

## What the stage adds

Four surface changes, one reason each:

- **`percpu static mut NAME: T = INIT`.** `percpu` is a contextual word, not a
  keyword: it is a marker only when `static` immediately follows it, exactly as
  `unsafe`/`naked` mark a `fun`. `pub` and `section("...")` behave as they do on
  an ordinary `static`, in either order round `pub`. `T` and `INIT` follow the
  existing `static` rules: a `Copy` scalar, an integer/`bool`/`char` array, a
  `datac` of those, or `Atomic<T>`, with an initializer that folds to a
  constant.
  - **`mut` is required.** A read-only per-CPU object is an ordinary `static`
    with extra address space, so `percpu static NAME` (no `mut`) is an error
    rather than a silently read-only global.
  - **Naming one is `unsafe`**, because it *is* a `static mut` in every way:
    reading, writing or taking its address happens inside `unsafe { ... }` or
    an `unsafe fun`, and the module must be listed in `coral.toml`'s
    `[unsafe] modules`.

- **`[target] cpus = N`.** An integer `1..=64`, default 1. It is the number of
  copies a `percpu` static is laid out as and the value `cpu_count()` folds to.
  `percpu` is a freestanding-only storage class (below), so both are refused in
  a hosted build; `cpus` itself is read in either mode, because a value that is
  simply unused is not a mistake.

- **`cpu_index() -> usize`.** The current CPU's index, read from the register
  the kernel sets at boot: `tp` on riscv64 (one `mv`), `TPIDR_EL1` on aarch64
  (one `mrs`). The compiler never sets the register and never assumes it: the
  kernel's own boot code loads it (the example's `kmain` does `li tp, 0`). The
  call is `unsafe`-free and takes no arguments. It is refused outside a
  freestanding build, and refused by name on a triple the compiler has no
  register read for.

- **`cpu_count() -> usize`.** The manifest's `cpus`, folded to a `usize`
  literal at check time, so it is the same constant to both backends and costs
  nothing at run time.

## The lowering

A `percpu` static is `N` copies laid out as one global array `[N x T]` in
section `.percpu`, aligned to 64 bytes so no two copies share a cache line. A
copy whose type is smaller than a cache line is padded: the element type is
`{ T, [pad x i8] }` with `pad = (64 - size_of(T) % 64) % 64`, so the stride is
`round_up(size_of(T), 64)` and every copy starts on a 64-byte boundary. A type
already a multiple of 64 (including one with a `repr(align)` larger than a line)
is used directly. The folded initializer is replicated into every copy, so the
image carries `N` copies of one constant and no code runs at startup.

Naming the static computes `&NAME[cpu_index()]`: a GEP indexed by the register
read, then (when padded) a project to field 0. A read loads from it, a write
stores through it, and `Ptr.from_mut`/`from_ref` take its address, exactly as
for an ordinary `static`. An explicit `section("...")` clause wins, for the same
reason it does on a `static`; the default is `.percpu`.

## Debug check, and why it exists

At `-O0` (the dev build) `cpu_index()` compares the register
against `cpus` and calls the project's own panic handler with
`cpu index out of range` when it is not below it, the same shape as every other
bounds check. A release build trusts the register: a kernel that leaves `tp`
wrong gets misbehaviour it cannot attribute, which is the UB this stage adds
(`plan/kernel-ub-notes.md` stage 11). The check is deliberately per optimization
level rather than always-on because the register is the kernel's contract, not
a value the language produced; the debug build is where a kernel author sees it.

## Both backends, and the exception

The `percpu` class and both intrinsics exist only in freestanding mode, which is
LLVM-only by design (the C backend's preamble is libc, `plan/kernel-path.md`).
Every front-end rule above (the `mut` requirement, the freestanding requirement,
the constant-initializer rule, the `static mut` unsafe gate, the manifest range)
runs in `infer.rs` and `resolve.rs`, before either backend is named, so the C
oracle refuses each fixture with the same message the LLVM backend does. Every
other feature kept its both-backends rule.

## The `_start` stub parks the other harts

A `percpu` image with `cpus > 1` is QEMU's `-smp 2` with `-bios none`, and
without firmware QEMU starts *every* hart at the entry. The riscv64 `_start`
stub therefore reads `mhartid` first and parks any hart whose id is not zero on
`wfi` **before** it loads `sp`, so a secondary never touches memory and never
reaches the entry. A single-hart image (the default `-smp 1`) reads 0 and falls
through unchanged.

## What the stage deliberately leaves out

- **Bringing a hart up.** The stub parks secondaries; it does not start them.
  A future stage (row 9's mapping/syscall work, or an explicit SMP stage) owns
  the IPI and the boot protocol a real kernel uses.
- **A per-CPU allocator or run queue.** Those are kernel designs on top of the
  storage class, not language work.
- **Stack protection per CPU.** Row 10's stack guards will want a per-CPU guard
  slot; this stage gives it the storage to put one in.

## How it was verified

`examples/kernel0-percpu/run.sh` boots a two-hart riscv64 `virt` with `-smp 2`,
prints `cpu0 4` (hart 0 wrote its copy), `cpu1 7` (setting `tp = 1` by hand reads
the untouched second copy), `idx 1` and `count 2`, and asserts them against
QEMU's own stdout. Two object tests read the layout back out of the `.o` at
`cpus = 1` and `cpus = 4` (a `.percpu` section aligned 64, a symbol of
`N * round_up(size_of(T), 64)` bytes for a `usize` and a `[u8; 100]`), and six
fixtures pin the refusals (percpu outside freestanding, percpu without `mut`,
a non-constant initializer, a name outside `unsafe`, `cpus = 0`, `cpus = 65`) on
both backends.
