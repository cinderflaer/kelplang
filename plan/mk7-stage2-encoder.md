# MK7 Stage 2: the encoder inventory

`plan/mk7-own-backend.md` §6.1 lists the instruction families Stage 2 must encode, and it says so
plainly that the list comes from memory. This file replaces the guess with a table derived from the
emitter that Stage 2 has to replace. It is an inventory, not a design: every number below came from a
command recorded here, and anything that could not be pinned down is labelled undetermined in §7.

## 0. What was counted, and against which revision

Two facts make the measurement reproducible, and one of them is awkward.

| item | value |
| --- | --- |
| revision of `kelpc/src/codegen_asm.rs` recounted here | 12339 lines, sha256 `ca476256c2163a84d434bb92103f5cac01633315080a9d61c955bbf371369af6` |
| that revision is | the working tree as of 2026-09-20 16:12, not `HEAD` |
| `c3a8759` copy of the same file (the `HEAD` while this was written) | 12175 lines, sha256 `ee560c64c4a91560005115f93fb727a60ad96802d820ed1ff65f7b846ae4cf03` |
| difference between them | 164 lines: `Routine::PathNormalizeRaw` and `Routine::FsCwdRaw` and their emitters, plus 46 shifted lines |
| the tip once this landed | `caeb98d` (now `8d86d8e`) is 12432 lines, sha256 `21cef3b9e189e5b7807609caf4d56414cf969dead3a279d84fa951fa326e0525`, 93 lines past the reading above |

Another lane was editing `codegen_asm.rs` throughout this inventory. It changed at 16:11 and again at
16:12; a third reading after that would disagree with the line numbers here. Every line number in this
file therefore belongs to the 12339 line revision above, and the awkward fact is that this revision is
uncommitted work in flight rather than a tag; the slice landed as `caeb98d` at 16:37 with 93 more
lines, so the tip's line numbers are higher than these. To re-derive it:

```text
cp -r kelpc Cargo.toml /tmp/rev && cd /tmp/rev && cargo build
sha256sum kelpc/src/codegen_asm.rs    # ca476256... , 12339 lines
git -C /mnt/bigdrive/projects/KelpLANG/kelp diff c3a8759 -- kelpc/src/codegen_asm.rs | head
```

The two added routines matter for the count in one direction only: `diff` of the two revisions shows
the added lines contain `mov`, `call`, `lea`, `push`, `sub`, `pop`, `xor`, `test`, `ret`, `je` and
`add`, every one of them already in the inventory. The removed lines contain no instruction literal at
all. The mnemonic set is therefore the same at both revisions, and only line numbers move.

### The corpus

The emitter's own output, kept rather than thrown away (`driver.rs` already writes the `.s` and the
`.o` beside the binary for exactly this reason, `driver.rs:66-76`). All 357 fixture files under
`kelpc/tests/fixtures/` were built with `--backend=asm`; 177 built, 180 were refused by the front end
or by the emitter. Their `.s` files are the corpus.

```text
for f in kelpc/tests/fixtures/*.kp; do
  ./target/debug/kelpc build --backend=asm "$f" -o /tmp/corpus/$(basename "$f" .kp) || true
done
```

| corpus figure | value |
| --- | --- |
| `.s` files | 177 |
| instruction lines (non directive, non label) | 103615 |
| distinct mnemonics observed | 63 |
| distinct mnemonics the emitter can write | 71 |
| distinct normalized `(mnemonic, operand form)` pairs observed | 165 |
| distinct relocation types in the `.o` | 3 |
| undefined symbols across all `.o` | 20 (19 libc symbols plus `_GLOBAL_OFFSET_TABLE_`) |
| distinct `kelp_rt$*` routines observed | 52 of the 53 `Routine` variants (`codegen_asm.rs:882-937`) |
| distinct `.globl` symbols | 326 |

The plan's own baseline says 226 of 283 end-to-end tests pass at §5. 177 fixture files build here.
The two numbers measure different things (a test can cover several fixtures, and the plan's figure is
from an earlier revision), so neither is a correction of the other; the fixture-level number is the
one this inventory is derived from.

---

## 1. The mnemonic inventory

Three derivations, intersected so that nothing rests on one method:

1. **Static, literals.** Every string literal in the emitter whose first token sits at four-space
   instruction indentation (`grep -oE '(\"|\\n) {4}[a-z][a-z0-9]*'`).
2. **Static, producers.** The six places where the mnemonic is computed rather than written:
   `Width::extend_named` (`:277`), `Width::setcc` (`:293`), `FWidth::mov` (`:331`),
   `FWidth::suffix` (`:339`), `FWidth::ucomi` (`:351`), and the `mnemonic` matches inside `binary`
   (`:8801`), `binary_float` (`:8943`), `float_op_from_xmm0` (`:9076`), `conversion` (`:11028`) and
   `assign_float_in_place`.
3. **Empirical.** The 63 mnemonics that actually appear as the first token of an instruction line in
   the 177 `.s` files.

The union of (1) and (2) is 71. The 8 difference is in §1.3.

### 1.1 The 71, grouped as §6.1 groups them

**Integer data movement (7).** `mov`, `movabs`, `movsx`, `movsxd`, `movzx`, and the two SSE
crossovers `movq` and `movd`. The `mov` inside `Width::extend_named`'s W4 unsigned arm (`:287`) is
the same mnemonic, not a separate one.

**Address computation (1).** `lea`.

**Integer arithmetic, binary (11).** `add`, `sub`, `imul`, `idiv`, `div`, `and`, `or`, `xor`, `shl`,
`shr`, `sar`.

**Integer arithmetic, unary (2).** `not`, `neg` (`codegen_asm.rs:8370-8374`).

**Comparison primitives (4).** `cmp`, `test`, `ucomiss`, `ucomisd`.

**`setcc` (12 distinct).** `sete`, `setne`, `setl`, `setle`, `setg`, `setge`, `setb`, `setbe`, `seta`,
`setae` (the ten `Width::setcc` returns, `:293-311`), plus `setp` and `setnp` (`:9058`, `:9064`,
`:9099`, `:9101`).

**`jcc` (11 distinct) and `jmp` (1).** `je`, `jne`, `ja`, `jae`, `jb`, `jbe`, `jg`, `jl`, `jle`, `jp`,
`js`. Not `jge`, not `jnp`, not `jns`, not `jz`, not `jnz`: none of those appears in the source at all.

**Calls and return (2).** `call`, `ret`.

**Stack (2).** `push`, `pop`.

**Sign extension into the dividend (4).** `cbw`, `cwd`, `cdq`, `cqo` (`:8872-8875`; W1, W2, W4, W8
before `idiv`).

**Exchange (1).** `xchg`.

**SSE2 scalar (13 here, 15 with the two UCOMIs under comparison).** Moves `movss`, `movsd`;
arithmetic `addss`, `addsd`, `subss`, `subsd`, `mulss`, `mulsd`, `divss`, `divsd` (the four base
words in `binary_float` and `float_op_from_xmm0` crossed with `FWidth::suffix`); convert `cvtsi2ss`,
`cvtsi2sd`, `cvtss2sd`. `ucomiss` and `ucomisd` are this family too and are counted in the
comparison row above.

**Counted:** 7 + 1 + 11 + 2 + 4 + 12 + 12 + 2 + 2 + 4 + 1 + 13 = **71 distinct mnemonics**. Every
row is disjoint: the two `ucomi` and the two `movs{s,d}` spellings appear in exactly one row each.

### 1.2 What §6.1 lists, and which of it the corpus reaches

Nothing. Every mnemonic §6.1 names appears in the emitter: `mov`, `lea`, `add`, `sub`, `imul`, `idiv`,
`and`, `or`, `xor`, `shl`, `sar`, `cmp`, `test`, `jmp`, `call`, `ret`, `movsx`, `movzx`, and the
`setcc`, `jcc` and SSE2 families. §6.1's other three items are not mnemonics and all three are
required: ModRM/SIB/displacement forms (§2.2), REX prefixes for the four register widths and the
extended registers (§2.4), and immediates (§2.3).

Eight of the named and reachable mnemonics are **not** in the observed corpus, because no fixture
exercises the shape that reaches them:

| mnemonic | reached from | why no fixture reached it |
| --- | --- | --- |
| `movsx` | `extend_named` W1/W2 signed (`:280`, `:282`) | every narrow signed fixture that builds ends in `movsxd` or `movzx` |
| `setge` | `Width::setcc` `(GtEq, true)` (`:300`) | a signed `>=` comparison, not raised as its own value by any fixture |
| `jp` | the float element comparator inside `emit_list_routine` (`:6328`, `:6558`) | a `List<f32>` or `List<f64>` `cmp`/`contains`/`sort`; no corpus fixture sorts or searches a float list |
| `cbw`, `cwd` | the W1 and W2 dividend sign-extend (`:8872-8873`) | no fixture divides at 8 or 16 bits |
| `subss`, `mulss`, `divss` | `FWidth::suffix` with `F4` (`:339-346`) | every f32 fixture adds, compares or converts; none subtracts, multiplies or divides |

These eight are exactly the kind of gap a Stage 2 gate cannot find by running fixtures; they are
recorded here so the encoder table covers them deliberately rather than by accident.

### 1.3 What the emitter needs that §6.1 does not name

Fifteen mnemonics, in descending order of how much work each is:

| mnemonic | why it is needed | source |
| --- | --- | --- |
| `push`, `pop` | the stack machine's operand spill, the callee-saved prologue and epilogue, and `push QWORD PTR [mem]` for the outgoing argument area | `:4604`, `:4611`, `:4778`, `:8416`, `:10596`, `:12285-12303` |
| `movabs` | a 64-bit immediate that does not fit sign-extended 32 bits: the two FNV constants and the mix constant | `:11885`, `:11888`, `:11909` |
| `movsxd` | `i32` to `i64` sign extension, the commonest single extension in the corpus (390 in the corpus) | `:284` |
| `div` | unsigned divide; §6.1 names `idiv` only | `:8882` |
| `shr` | logical right shift; §6.1 names `shl` and `sar` only | `:8912` |
| `cbw`, `cwd`, `cdq`, `cqo` | the dividend's sign extension before `idiv`; four separate instructions, one per width | `:8872-8875` |
| `not`, `neg` | unary bitwise not and arithmetic negation | `:8370-8374` |
| `xchg` | `xchg rsi, rdx` in `emit_panic_errno_routine` | `:11664` |
| `movq`, `movd` | the integer and `xmm` file crossover, because there is no `mov xmm, imm` | `:8356-8363`, `:11284`, `:12275` |

Two of these are worth flagging for the encoder table's shape: `push QWORD PTR [mem]` is `FF /6`
with a full ModRM and possible displacement, and `pop` is only ever a register, so `pop r/m` is not
needed. And `movabs` is a *mnemonic* only where the emitter writes it literally; the larger family is
`mov r64, imm64`, which the emitter writes as `mov` and lets `as` choose `B8+rd io`. There are 144
`mov r64, <immediate>` lines in the corpus whose value needs the 64-bit form, including every `f64`
bit pattern (`:8585-8591`), so the encoder must choose `MOV r64, imm64` from the immediate's value,
not from the mnemonic.

---

## 2. The operand-form inventory

### 2.1 Register classes and widths

Only two register files are touched. General purpose, in all four widths (`Width`, `:112-260`):
`r8` (`al` to `r15b`), `r16` (`ax` to `r15w`), `r32` (`eax` to `r15d`), `r64` (`rax` to `r15`). SSE,
the same 16 registers at both value widths (`FWidth`, `:315-357`): `xmm0` to `xmm15`.

Widths actually observed in the corpus, counted as register tokens anywhere in an instruction's
operands (so a memory base counts as `r64` there, which is why a few rows carry `r64` for families
whose operands are not 64-bit):

| family | r8 | r16 | r32 | r64 | xmm |
| --- | --- | --- | --- | --- | --- |
| moves (`mov`, `movabs`, `movsx`, `movsxd`, `movzx`, `movq`, `movd`, `movss`, `movsd`, `lea`) | 1436 | 63 | 4286 | 98233 | 653 |
| arithmetic (`add`, `sub`, `imul`, `idiv`, `div`, `neg`, the eight SSE arithmetic forms) | 88 | 1 | 290 | 5796 | 90 |
| bitwise and shift (`and`, `or`, `xor`, `not`, `shl`, `shr`, `sar`) | 133 | 2 | 1659 | 5583 | - |
| compare (`cmp`, `test`, `ucomiss`, `ucomisd`) | 324 | 4 | 343 | 5879 | 30 |
| `setcc` | 12 distinct, all `r8` only (`al`) | - | - | - | - |
| convert | - | - | - | 23 | 8 (`cvtss2sd`) |
| `push` / `pop` | - | - | - | 8255 / 7568 | - |

`setcc` is `r8` only because every caller reads `al` afterwards (`movzx rax, al`, `:8922`); a general
`setcc r/m8` is not needed.

### 2.2 Memory forms

The distinct base and index combinations in the corpus, reduced to shapes. `disp` means either a
`disp8` or a `disp32`; both occur and `as` picks the narrow encoding.

| shape | example | count | encoder note |
| --- | --- | --- | --- |
| `[base]`, mod=00 | `[rdi]`, `[rbx]`, `[r8]`, `[r11]`, `[rcx]`, `[rdx]`, `[rsi]`, `[rax]` | `[rdi]` 1020, `[r11]` 474 | when base is `rbp` or `r13`, mod=00 is not encodable and `disp8 = 0` must be used instead |
| `[base+disp]`, mod=01 or mod=10 | `[rbp-8]`, `[rsp+16]`, `[rdi+8]`, `[r11+0]`, `[r12+1]` | `[rbp-8]` 7064 | `[rsp+disp]` and `[r12+disp]` need a SIB byte with index=none; `[r12]` needs REX.B inside the SIB |
| `[rip+sym]` | `[rip+.LC0]`, `[rip+kelp_pd__banner]` | `[rip+.LC0]` 177, `[rip]` 126 | mod=00 rm=101, disp32, no SIB, no REX |
| `[base+index]`, scale 1 | `[rax+rcx]`, `[rbx+r12]`, `[rcx+r10]`, `[rdi+r9]` | 198, over 17 distinct pairs | SIB with scale=00; REX.X if the index is `r8` to `r15` |
| `[base+index*scale]`, no disp | `[rcx+rax*8]` (108), `[rdi+rax*8]` (56), `[rcx+rdx*8]` (56), `[rax+rax*2]` (28), `[r13+r13*2]` (1) | 249 | scale 8 and scale 2 only; scale 1 never written as `*1` |
| `[base+index*scale+disp]` | none | 0 | a shape §6.1 names and this emitter never produces; the encoder can refuse it in v1 |

The absence of `[base+index*scale+disp]` is worth stating as a fact rather than an omission: the
addressing sites are `emit_elem_addr`, `emit_array_elem_addr` and `emit_map_entry_addr`
(`:5862`, `:7788`, `:6803`), and each computes the byte offset into a register first and then
addresses `[r11+off]` or `[r11]`. The scaled forms come only from the fixed-stride list and map code
(`:7107`, `:7168`, `:7241`, `:7457`).

`rip`-relative is used for three things: a `.rodata` constant (`lea` or a memory operand), a `.bss`
passdown global, and the `stderr@GOTPCREL` load. All three are the same encoding; only the relocation
differs (§3.4).

### 2.3 Immediates

Observed, with the minimum and maximum value each mnemonic was handed across the whole corpus:

| mnemonic | observed range | smallest encoding that fits | source |
| --- | --- | --- | --- |
| `mov` | -2147483648 to 18444492273895866368 | `imm8` 4284 uses, `imm32` sign-extended 250, `imm64` 144 | `:8585`, `:11105-11115` |
| `add` | 1 to 4136 | `imm8` 748, `imm32` 1 | `:8855` |
| `sub` | 1 to 4136 | `imm8` 1804 and `imm32` 152 (`sub rsp, <frame size>`) | `:4614`, `:8855` |
| `cmp` | -1 to 1114111 | `imm8` 220 and `imm32` 171 | `:8918`, `:11105-11115` |
| `and` | -16 to 2047 | `imm8` 5067 (`and rsp, -16`, `:4079`) and `imm32` 62 | `:4079`, `:8855` |
| `imul` | 1 to 40 | `imm8`; `imul r64, imm` is written with two operands and encodes as three | `:5866`, `:6808` |
| `shl`, `shr` | 2 to 8 and 1 to 52 | `imm8`; the variable form is `cl`, never an immediate in a register | `:8914`, `:11063` |

**No immediate is ever 16 bits**, and no operand-size prefix `0x66` is ever emitted for an immediate.
The one place a value that does not fit `imm32` is compared is `conversion` (`:11103-11115`), which
deliberately does `mov r11, <bound>` and then `cmp rax, r11` rather than `cmp rax, <bound>`, because
`u32::MAX` sign-extended through an `imm32` would compare against `-1`. The encoder therefore needs
exactly three immediate widths: `imm8`, `imm32` sign-extended, and `imm64` for `MOV r64, imm64`.

### 2.4 REX and legacy prefixes

A byte-level histogram over every instruction in the 177 `.o` files (`objdump -dw`, first prefix
byte group). These are the combinations that actually occur, not the ones x86 permits:

| prefix | count | what produces it |
| --- | --- | --- |
| `48` REX.W | 55855 | any 64-bit operand |
| `4c` REX.W+R | 4802 | 64-bit operand with a register in `r8` to `r15` |
| `49` REX.W+B | 3614 | 64-bit operand with `r8` to `r15` as the r/m or a memory base |
| `41` REX.B | 2340 | 8-bit or 32-bit operand with an extended register, and `push`/`pop` of one |
| `4d` REX.W+R+B | 763 | both extended bits at 64 bits |
| `42` REX.X | 59 | SIB index in `r8` to `r15` |
| `4e` REX.W+X | 30 | the same at 64 bits |
| `44` REX.R | 26 | extended register in the ModRM reg field at 32 or 8 bits |
| `46` REX.W+X | 15 | at 64 bits |
| `45` REX.R+B | 4 | both |
| `4b` REX.W+X+B | 1 | all three |
| `40` bare REX | 0 | the forms that would need it (`spl`, `bpl`, `sil`, `dil`) do not occur |
| no prefix at all | 36657 | every 8-bit, 16-bit and 32-bit instruction with no extended register, and every 64-bit one whose bytes already fit without REX |
| `66` operand size | 227 | `movzx r64, r16`, `cmp r16, r16`, `xor r16, r16` |
| `f2` SSE double | 468 | every `sd` mnemonic |
| `f3` SSE single | 63 | every `ss` mnemonic |
| `67` address size | 0 | all addressing is 64-bit |
| segment prefixes | 0 | none |

The ModRM groups used are reg/rm with 32-bit and 8-bit registers, the `/0` immediate group, the
`/6` and `/7` unary groups, and `FF /6` for `push`. No `0F 38` or `0F 3A` three-byte opcode occurs
at all; the two-byte `0F` opcodes that do are `0F B6`/`0F B7`/`0F BE`/`0F BF` (`movzx`/`movsx`),
the `0F 4x` to `0F 8x` rows (`setcc`/`jcc`), `0F 2E`/`0F 2F` (`ucomi`), `0F 2A` (`cvtsi2s`), and the
`0F 10` to `0F 2D` range together with `F2 0F 5x`/`F3 0F 5x` for the SSE scalar arithmetic.

Encoded instruction lengths observed (`objdump -dw`, which is what makes them trustworthy where
plain `objdump` wraps long lines): 1 byte (`push`, `pop`, `ret`, `cdq`), 2 (`jmp` short, `xor`,
`test`), 3 (`mov r64, r64`, `movsxd`, `movzx`), 4 to 7 (`mov` with a displacement, `lea` with
`rip`), 8 to 11 (`mov QWORD PTR [r11+disp32], imm32` at 11), 10 (`movabs`).

---

## 3. The object and ELF inventory

### 3.1 What is invoked today

`driver.rs` is 187 lines and is the only code in the compiler that shells out. `assemble_and_link`
(`driver.rs:66-148`) does exactly this, and nothing else:

```text
as --64 -o <out>.o <out>.s
ld -o <out> -dynamic-linker <ld-linux-x86-64.so.2>
   <lib>/crt1.o <lib>/crti.o <out>.o <lib>/crtn.o -lc -lm -L<lib>
```

`<lib>` is found by `glibc_lib_dir` (`driver.rs:151-187`): `KELPC_GLIBC_DIR` if set, otherwise
`cc -print-file-name=crt1.o`, otherwise `/usr/lib/x86_64-linux-gnu`, `/usr/lib64`, `/usr/lib`. The
interpreter path is `<lib>/ld-linux-x86-64.so.2` or `<lib>/../lib64/ld-linux-x86-64.so.2`. On this
machine `<lib>` is `/nix/store/rkiwrqh8g6gs5rws4bf1s3mrdcs15gm9-glibc-2.42-84/lib`.

Three things follow for Stage 2. The own linker's inputs are the same four files plus `-lc` and
`-lm`, so it needs an ELF reader for `crt1.o`, `crti.o`, `crtn.o` and `libc.so.6` unless it
synthesizes its own startup (§4.4). `-lm` is passed unconditionally, which is why even `hello` has
`DT_NEEDED libm.so.6`. And the interpreter path is not a constant; the own linker must do the same
lookup.

### 3.2 Sections, symbols and visibility in the produced `.o`

Measured on `heap_bytes.o` (11 section headers) and across the corpus:

| section | what puts it there | note |
| --- | --- | --- |
| `.text` | every function and every `kelp_rt$*` routine | `SHF_ALLOC|SHF_EXECINSTR`, alignment 1 |
| `.rela.text` | `as`, one entry per relocation site | `sh_link` = symtab index, `sh_info` = the `.text` index |
| `.rodata` | `finish` (`:4384-4394`), a `.byte` run per interned constant | created even when empty by `as` |
| `.bss` | the passdown globals, `.p2align 3` plus `.zero <bytes>` (`:3282-3287`) | `NOBITS`, so the own writer emits no file bytes |
| `.data` | only `__dso_handle`, 8 bytes of zero (`:5481-5482`) | `SHF_ALLOC|SHF_WRITE` |
| `.note.GNU-stack` | emitted literally by the source (`:3102`) | empty, `as` keeps it |
| `.note.gnu.property` | added by `as` | optional; the own writer may omit it |
| `.symtab`, `.strtab`, `.shstrtab` | `as` | the own writer must build all three |

Symbols. A Kelp function becomes a `.globl` `@function` symbol, `main` unstripped and everything
else `kelp_` prefixed (`symbol`, `:4405`), with `.globl`/`.type` at `:4601-4602` and `.size` at
`:4779`. A `kelp_rt$*` routine gets `.type` and `.size` but no `.globl`, so it is a local `FUNC`
symbol. `__dso_handle` is `.globl` with `.hidden`. A passdown global (`kelp_pd__<name>`) gets no
directive at all: it is a local `NOTYPE` symbol in `.bss`. A `.LCn` constant gets no symbol at all,
which is why the relocations in §3.4 name the section instead.

Measured across the corpus: `.globl` 688 uses over 326 distinct names, `.type` 2331, `.size` 1389,
`.section` 375, `.p2align` 24, `.hidden` 5 (one per program that has an owned passdown drop, all on
`__dso_handle`), `.zero` 24, `.byte` 3628, `.quad` 5 (all of them `__dso_handle`). `.local`, `.weak`,
`.comm`, `.set`, `.string` and `.ascii` never appear at all.

Byte counts by section for `heap_bytes.o`: `.text` 0x3e5, `.rodata` 0xa2, `.rela.text` 0x2e8, 21
symbols.

### 3.3 Literals

**String and format literals.** `bytes_label` (`:4360`) interns a NUL-terminated byte string as
`.LCn`; `finish` writes it as `.byte` runs of 16 per line. Every literal is NUL-terminated and the
terminator belongs to the emitter, not to the assembler.

**Float literals.** Two different homes. A literal that can be an instruction's second operand is
interned raw by `const_bytes_label` (`:4375`, deduplicated against everything already interned) and
addressed as `addsd xmm0, [rip+.LC7]`. A literal in an expression position has no `mov xmm, imm` to
reach for, so it travels as an integer: `mov rax, 0x3ff0000000000000` then `movq xmm0, rax`, or
`mov eax, <bits>` then `movd xmm0, eax` for `f32` (`:8585-8591`).

**Postmortem strings.** `panic: %s\n`, the runtime's own bounds and OOM messages, and the conversion
overflow message are all `.bytes_label` interning and all reached through `lea rdi, [rip+.LCn]`.

### 3.4 Relocations, site by site

Measured across the 177 `.o` files (`readelf -rW`):

| relocation | count | sites |
| --- | --- | --- |
| `R_X86_64_PLT32` | 3865 | every `call`, local and libc alike |
| `R_X86_64_PC32` | 3313 | every `rip`-relative reference to `.rodata`, `.bss` or `.data` |
| `R_X86_64_REX_GOTPCRELX` | 127 | the `stderr@GOTPCREL[rip]` load in the panic routines |
| `R_X86_64_64` | 0 | none |
| `R_X86_64_32`, `R_X86_64_32S` | 0 | none |

This contradicts §6.2 in two places, and the code wins.

**A local call is `PLT32`, not `PC32`.** `while_accumulator.o` has three relocations:
`R_X86_64_PLT32 kelp_sum_to - 4`, `R_X86_64_PC32 .rodata - 4`, `R_X86_64_PLT32 printf - 4`. A
defined, globally visible symbol called with `call` gets `PLT32` from `as` because the symbol is
preemptible. §6.2's "`R_X86_64_PC32` for local calls" is wrong for this emitter's output. It is not
wrong as a *choice*: the own writer may use `PC32` for a call to a symbol it defines and is not
exporting. It is wrong as a description of what `as` produces, which is what the differential gate
compares against.

**`PC32` is for data, and it names the section, not the label.** Every `.rodata` reference is
`R_X86_64_PC32 .rodata + <addend>`, because `as` folded the local `.LCn` label away. A `.bss`
reference is `R_X86_64_PC32 .bss - 4` or with an addend, and `__dso_handle` is `.data + <addend>`.
An own object writer has a free choice here: emit `.LCn` as a local symbol in the symtab and
relocate against the symbol (closer to what a hand-written linker wants), or fold into the section
symbol (identical to `as`). The differential test in §5 must accept either, which means it compares
the linked result's behaviour and the disassembly, not the relocation symbol index.

**`REX_GOTPCRELX` is a linker relaxation hint.** It comes from exactly one source line, `:11934`
(`mov rdi, QWORD PTR stderr@GOTPCREL[rip]`). In the linked executable it becomes a `.got` slot with
an `R_X86_64_GLOB_DAT` dynamic relocation. The own writer may emit `R_X86_64_GOTPCREL` instead and
skip relaxation entirely; nothing in the corpus depends on the relaxed form.

**The linked executable needs two more types.** `readelf -rW heap_bytes` shows
`R_X86_64_GLOB_DAT` for `__libc_start_main`, `__gmon_start__` and `stderr`, and
`R_X86_64_JUMP_SLOT` for each of the 7 PLT-called libc functions. If the own linker chooses `ET_DYN`
(a PIE) instead of the `ET_EXEC` that `ld` produces here, it also needs `R_X86_64_RELATIVE` for the
GOT slots and the `.data` pointers. Choosing `ET_EXEC` keeps the relocation set at five types.

So the distinct relocation count is **3 in the object** and **5 to link an `ET_EXEC`** (3 + `JUMP_SLOT`
+ `GLOB_DAT`).

### 3.5 What the plan expects and the emitter does not have

**There are no capability tables.** `codegen_asm.rs` never emits a grant or an allowlist; it refuses
a gated call by name (`:10420`, the message names the domain). The C backend compiles each module's
granted patterns to constant data (`codegen_c.rs:687-700`). So §6.2's "`R_X86_64_64` for the
capability tables and string literals" describes the C backend's data, not this one's: the string
literals here are `rip`-relative and therefore `PC32`, and the capability tables will not exist in
the asm output until §5 item 7 lands. Stage 2 will need `R_X86_64_64` only when a gated call is
emitted rather than refused, which is not a Stage 2 deliverable.

**Passdown globals.** One local `NOTYPE` symbol per `pd`, in `.bss`, `.p2align 3`, `.zero <n>` bytes
(`:3282-3287`). `pd_storage_size` sizes them. 21 distinct passdown names appear in the corpus. They
are read and written through `rip`-relative `PC32` relocations, and they are read and written by
name, so an own writer must keep `kelp_pd__<name>` as a symbol for every one.

**The `main`-registered `atexit` drop.** When any passdown owns something, `main`'s prologue does
`lea rdi, [rip+kelp_drop_pds]` and `call atexit@PLT` (`:4804`), and the generator also emits the
local `kelp_drop_pds` routine (`:5458`) and a hidden `__dso_handle` in `.data` (`:5481-5482`). The
reason is glibc's own: `atexit` here is `__cxa_atexit(fn, NULL, __dso_handle)` living in
`libc_nonshared.a`, and nothing else defines `__dso_handle` on this link line. Stage 2 must keep all
three pieces: the `atexit` PLT call, the local routine, and the hidden `__dso_handle` object. Five
fixtures in the corpus reach this path.

### 3.6 Three fixtures kept as the object inventory's witnesses

| fixture | source | what it pins |
| --- | --- | --- |
| `while_accumulator` | `kelpc/tests/fixtures/while_accumulator.kp` | the minimum: two functions, `main` global, `printf` through the PLT, one `.rodata` literal, three relocations |
| `stdlib_parse` | `kelpc/tests/fixtures/stdlib_parse.kp` | the extern path: `kelp_rt$str_parse_i64_raw` calls `__errno_location@PLT` and `strtoll@PLT`, and stores through a caller `mut` out-parameter |
| `heap_bytes` | `kelpc/tests/fixtures/heap_bytes.kp` | the heap type and the runtime: 7 `kelp_rt$*` routines, `malloc`/`free`/`memcpy`/`strlen`, the hand-written `kelp_panic`, and therefore `stderr@GOTPCREL`, `fprintf`, `exit` |

`stdlib_math` is worth adding as a fourth witness because it is the `Extern::Libc` path rather than
the `Routine` path: `call sqrt@PLT` and `call floor@PLT` appear with no wrapper at all
(`extern_target`, `:1009`).

The full undefined-symbol surface across the corpus, which is the set the own linker must resolve
from libc and `libm`: `atexit`, `__errno_location`, `exit`, `floor`, `fprintf`, `free`, `malloc`,
`memcmp`, `memcpy`, `memset`, `printf`, `realloc`, `snprintf`, `sqrt`, `stderr`, `strlen`, `strtod`,
`strtoll`, `strtoull`, plus `_GLOBAL_OFFSET_TABLE_`. `strerror` is reachable through
`emit_panic_errno_routine` (`:11957`) without any fixture in the corpus reaching it, so it belongs
in the list of libc names the linker must resolve.

---

## 4. The dynamic-linking decision of §6.3,

written out against a real link rather than against the plan's summary. The measurements are from
the `ld` output for `heap_bytes` and `hello` at the revision above, which is the shape the own
linker has to reproduce.

### 4.1 `PT_INTERP`

One `PT_INTERP` program header whose content is the interpreter path plus a NUL, `p_filesz = 0x53`
and `p_memsz = 0x53` for the Nix store path on this machine. The path must come from the same lookup
`glibc_lib_dir` does today; hard-coding `/lib64/ld-linux-x86-64.so.2` would be wrong on this host.
Without a valid `PT_INTERP` the kernel returns `ENOEXEC` for a `PT_DYNAMIC` binary, so this is the
first header to get right.

### 4.2 `PT_DYNAMIC` and the tags

`PT_DYNAMIC` must point at the same bytes as the `.dynamic` section, must be inside a writable
`PT_LOAD`, and must carry at least these entries. Measured tag set for `hello` (23 entries, one of
them the terminating `NULL`), with what each has to contain for the loader to accept the file:

| tag | content for the smallest program | note |
| --- | --- | --- |
| `DT_NEEDED` | `libc.so.6`, and `libm.so.6` because `-lm` is always passed | an index into `DT_STRTAB`, not an offset into the file |
| `DT_STRTAB`, `DT_STRSZ` | `.dynstr` and its size | every name in `DT_NEEDED` and every dynsym name lives here |
| `DT_SYMTAB`, `DT_SYMENT` | `.dynsym` and 24 | the loader walks it by index |
| `DT_HASH`, `DT_GNU_HASH` | `.hash`, `.gnu.hash` | glibc on this machine still reads both; `DT_GNU_HASH` is the one that must be a valid bloom filter and bucket array, and a malformed one is a silent lookup failure rather than a load error |
| `DT_VERSYM`, `DT_VERNEED`, `DT_VERNEEDNUM` | `.gnu.version`, `.gnu.version_r`, 1 | every undefined symbol here is versioned (`printf@GLIBC_2.2.5`); without the version sections the loader rejects the file |
| `DT_PLTGOT` | the address of `.got.plt` | three 8-byte entries: the `_DYNAMIC` pointer that the lazy resolver expects at `[0]`, then one slot per `JUMP_SLOT` |
| `DT_JMPREL`, `DT_PLTRELSZ`, `DT_PLTREL` | `.rela.plt`, `24 * n`, `RELA` | for `heap_bytes`, 7 entries and `PLTRELSZ = 168` |
| `DT_RELA`, `DT_RELASZ`, `DT_RELAENT` | `.rela.dyn`, `24 * n`, 24 | `GLOB_DAT` entries; for `heap_bytes`, 3 entries and `RELASZ = 72` |
| `DT_INIT`, `DT_FINI` | `_init` in `.init`, `_fini` in `.fini` | both come from `crti.o`/`crtn.o`; see §4.4 |
| `DT_RUNPATH` | whatever `-L` expanded to | `ld` writes the lib directory here, which is how a Nix store path is found at run time |

`DT_DEBUG` is written by the linker as zero and filled by the loader; it can be present with value
zero, which is what `ld` does here.

### 4.3 The GOT and the PLT stubs

The measured `.got` for `heap_bytes` is three 8-byte slots: `__libc_start_main` and
`__gmon_start__` as `GLOB_DAT`, and `stderr` as `GLOB_DAT` because the panic routine reads it
through the GOT. The measured `.got.plt` is `_DYNAMIC`, then one slot per `JUMP_SLOT`.

The `.plt` layout is 16 bytes per stub plus a 16-byte header, and `objdump -j .plt` shows exactly the
sequence the own writer must reproduce:

```asm
401020:  push 0x2fca(%rip)          ; GOT[1]
401026:  jmp  *0x2fcc(%rip)          ; GOT[2], the resolver
40102c:  nopl 0x0(%rax)              ; padding to 16 bytes
401030:  jmp  *0x2fca(%rip)          ; <free@plt>: GOT slot for free
401036:  push $0x0                   ; the relocation index
40103b:  jmp  401020                 ; back to the header
```

`.rela.plt` carries one `R_X86_64_JUMP_SLOT` per stub, in the same order as the push immediates, and
`.plt` alignment is 16 with `.plt` entry size 16.

Consequence for §6.1's encoder: three operand forms the PLT needs that the emitter never writes.
`jmp QWORD PTR [rip+disp32]` is `FF /4` with a memory operand, where every `jmp` in the corpus is a
relative branch; `push imm32` is `68 id`, where every `push` in the corpus is a register or a memory
operand; and the 4-byte `nop` is `0F 1F 40 00`. All three are trivial, and all three belong in the
own-linker slice rather than in the codegen table, because nothing in the emitter produces them.

### 4.4 The startup problem, which the plan does not settle

`ld` gets `_start` from `crt1.o`, which calls `__libc_start_main(main, argc, argv, init, fini,
rtld_fini, stack_end)`. §6.3 says dynamic linking is the cheap path because static libc would mean
owning a `crt1.o` equivalent, which is Tier 3's item 7. That is right, and it leaves one choice the
plan does not name: the own linker can either

1. consume `crt1.o`, `crti.o` and `crtn.o` as input files, exactly as the `ld` command line does
   today, and concatenate their sections into the output, or
2. synthesize `_start` and call `__libc_start_main` itself, which is a short hand-written prologue
   and makes `crt1.o` unnecessary.

Option 1 is the one §6.3's reasoning implies: the linker becomes a program that reads ELF
relocatables and writes an executable, and the C library's startup stays the C library's. It also
keeps every fixture's exit status and `argc`/`argv` behaviour identical for free. Option 2 is
strictly more work and belongs with Tier 3's own startup. This inventory recommends option 1 and
flags it as the one decision §4 of a Stage 2 plan must state explicitly rather than discover.

### 4.5 What the static alternative would drag in

For completeness, since §6.3 rejects it: static linking (`ld -static`) removes `PT_INTERP`,
`PT_DYNAMIC`, `DT_NEEDED`, the GOT and the PLT, and links `libc.a` plus `crt1.o`. The cost is that
`libc.a` reaches the syscall layer through `libc_nonshared.a` and, on this host, needs the static
flavours of `libm.a` and the NSS stubs; `getaddrinfo`, `strerror`'s locale tables and every `stdio`
buffer come along as objects and constructor sections. The corpus does not test any of that today,
but the plan's §7 items 3 to 6 are exactly what would have to move into Stage 2 to make static
honest, which is why §6.3's answer stands.

---

## 5. The test plan for §6.4's gate

### 5.1 `readelf` and `objdump` assertions

The gate's point is that a malformed binary fails the build rather than crashing at `exec`. Each
assertion below names a property a broken writer gets wrong, and each is a single `readelf` or
`objdump` invocation a test can run and compare.

**Header and layout (`readelf -hW`).** `e_ident` magic is `7f 45 4c 46`, class 2, data 1, version 1,
OSABI 0; `e_type` is `EXEC` (or `DYN`, if the writer chooses a PIE); `e_machine` is `EM_X86_64`
(62); `e_version` is 1; `e_entry` is non-zero; `e_phoff` is 64; `e_shoff` is inside the file;
`e_phnum`, `e_shnum`, `e_shentsize` (64), `e_phentsize` (56) and `e_shstrndx` all describe what is
actually there. A wrong `e_shoff` or `e_shstrndx` is what makes `readelf` print a section table of
garbage, and it is the first thing a hand-written writer gets wrong.

**Segments (`readelf -lW`).** Exactly one `PT_INTERP`, `p_filesz` equal to the path length plus one,
and the bytes at `p_offset` equal to the path the build used. At least one `PT_LOAD` with `PF_X`, and
`e_entry` inside that segment's address range. Every `PT_LOAD` satisfies `p_offset % p_align ==
p_vaddr % p_align`; this is the invariant a naive writer breaks and the kernel answers with a
segfault before `main`. `p_memsz >= p_filesz`. A `PT_GNU_STACK` with `PF_R|PF_W` and not `PF_X`. One
`PT_DYNAMIC` lying inside a writable `PT_LOAD`.

**Dynamic section (`readelf -dW`).** `DT_NEEDED` for `libc.so.6` (and `libm.so.6` while `-lm` is
passed); `DT_STRTAB`/`DT_SYMTAB`/`DT_HASH`/`DT_GNU_HASH`/`DT_VERSYM`/`DT_VERNEED` addresses inside
`PT_LOAD` ranges; `DT_STRSZ` and `DT_SYMENT` consistent with the sections; `DT_PLTGOT` inside a
`PT_LOAD`; `DT_PLTREL == DT_RELA`; `DT_PLTRELSZ == 24 * (number of `.rela.plt` entries)`;
`DT_RELASZ == 24 * (number of `.rela.dyn` entries)`; `DT_JMPREL` inside `PT_LOAD`. A file that
passes the header checks and fails these loads but dies with "cannot resolve symbol" or an
immediate crash.

**Relocations (`readelf -rW`).** Every relocation's `r_offset` lies inside a section with content; a
`JUMP_SLOT` offset is a slot inside `.got.plt` and the matching PLT stub's `ff 25` displacement
points at that slot; a `GLOB_DAT` offset is inside `.got`; the dynsym index of each relocation
resolves to a name in `.dynstr`.

**Symbols (`readelf -sW`).** `.dynsym` has one entry per PLT-called libc function plus
`__libc_start_main` and `__gmon_start__`; each has the right `st_info` type (`FUNC`) and binding; the
`.gnu.version` array length equals the `.dynsym` count (a mismatch is the classic versioned-symbol
bug); `sh_link` of `.dynsym` is the `.dynstr` index and of `.gnu.version` the `.dynsym` index.

**Code (`objdump -dw`).** The `.plt` header is the `push`/`jmp`/`nop` sequence of §4.3 and each stub
is 16 bytes with a `ff 25` pointing into `.got.plt`; `_start` disassembles without a `(bad)`
mnemonic; a `objdump -dw ... | grep -c '(bad)'` of zero; the disassembly of every `kelp_*` and
`kelp_rt$*` symbol matches the disassembly of the same symbol from the Stage 1 `.o` for the same
fixture, byte for byte. That last one is the strongest encoder assertion available without an
oracle: it compares the encoder's bytes against `as` assembling the same text, symbol by symbol, and
it is what catches a wrong ModRM, a missing REX.B or a truncated displacement.

**Running it.** The end-to-end assertion stays what it is today: build, run, compare stdout with the
C backend, compare the exit status. A malformed ELF does not get that far, which is the point; the
structural assertions above are what tell the developer *why* when it does not.

### 5.2 The differential suite and `--backend=own`

Five files are involved; four need an edit and one does not.

**`kelpc/src/backend.rs`.** `RESERVED` (`backend.rs:29`) currently holds `["own"]` and `parse`
refuses it with "planned but not implemented yet". A Stage 2 first slice replaces the reserved entry
with `Backend::Own`, adds the `name()` arm `"own"`, and adds it to `IMPLEMENTED` (`backend.rs:43`).
The unit tests in that file then cover it for free: `every_implemented_backend_round_trips_its_name`
and `reserved_and_implemented_names_are_disjoint` both iterate `IMPLEMENTED`. The reserved-name test
`a_reserved_backend_says_planned_instead_of_unknown` needs a replacement reserved name or removal,
which is the honest signal that the name is no longer reserved.

**`kelpc/src/main.rs`.** The `match backend` at `main.rs:277` gains a `Backend::Own` arm. Until the
own linker lands, that arm can emit the object and hand it to `ld`, which keeps the seam honest and
lets the fixture suite run before the linker exists.

**`kelpc/tests/backend_coverage.rs`.** The sweep builds hard-coded `"c"` and `"asm"` (`build("c",
...)`, `build("asm", ...)`, and `out_path("c")`/`out_path("asm")`). Stage 2's form is a third
directory and a third `build` call, with two comparisons (C against asm as today, C against own as
the new one) and the same `REFUSAL_PREFIX` requirement, which would have to become
backend-specific because the message currently spells `the 'asm' backend does not support`.
`ASM_BUILT_BASELINE`'s ratchet is the pattern to copy: an `OWN_BUILT_BASELINE` whose growth is
recorded as slices land.

**`kelpc/tests/compile_and_run.rs`.** `backend()` (`:28`) reads `KELPC_BACKEND` with `"c"` as the
default, so pointing the whole suite at the new backend needs no edit at all:
`KELPC_BACKEND=own cargo test --test compile_and_run`. The hand-written differential test
`the_two_backends_agree_on_the_scalar_fixtures` (`:4668`) has a fixed list and a `for backend in
["c", "asm"]` loop; Stage 2 adds `"own"` to that array and keeps the list as the coverage assertion
it already is.

**`kelpc/tests/run-all-backends.sh`.** `BACKENDS` already defaults to `c` and already documents the
override, so the Stage 2 invocation is `BACKENDS="c asm own" ./kelpc/tests/run-all-backends.sh`,
with no edit. That is the one place the Stage 0 seam pays off exactly as §4 promised.

**The heavy sweep's cost.** `backend_coverage.rs` builds every fixture the C backend accepts twice,
and is `#[ignore]`d for that reason (ground rule 1.5); its own note records 195 C-built fixtures at
the last re-recording, and this inventory's run builds 177 of 357 with the asm backend. A third
backend makes it three builds per fixture. That is a straightforward 50 percent increase in an
already-ignored test, and it is the reason the own sweep should reuse the same `OnceLock<Sweep>`
shape rather than adding a fourth heavy test.

---

## 6. Build order

Sized like §5's slices: each is one mechanism, each ends in a named acceptance criterion, and each
leaves a working compiler behind it. Ground rule 4 (§1) applies unchanged: no slice starts before the
one below it passes.

### S1 (first slice): the encoder core and its round-trip harness

**What.** `kelpc/src/enc.rs` (or `enc/mod.rs`): the encoding primitives, meaning REX computation from
the two operands' registers and widths, ModRM with the `rsp`/`r12` SIB rule and the `rbp`/`r13`
`disp8 = 0` rule, SIB with scale, `rip`-relative, `imm8`/`imm32`/`imm64` selection, and the opcode
table for one provisional family (`mov r/m64, r64` and its reverse, `mov r/m64, imm32`, `push`/`pop`,
`ret`, `lea`, `movzx`, `movsx`, `movsxd`). Beside it, a test that reads the 177 kept `.s` files from
§0, encodes every instruction line, and compares `objdump -dw` of the result with `as`'s own `.o` for
the same file. The harness needs a decoder to read the objects, and the cheapest one is `objdump`
itself, so no own decoder is required at this point.

**Answering the task's decoder question directly:** the encoder can be built and tested without an
own decoder, because `objdump` is the decoder and `as` is still present throughout Stage 1 and Stage
2. An own decoder is worth writing but is not on the critical path; put it in this slice only if the
harness is to run without binutils, otherwise it belongs after S2, beside the object writer, where it
also serves as the ELF reader §4.4 needs.

**Acceptance.** Every instruction line in the 177 `.s` files round-trips: `as` on the original text
and the encoder produce byte-identical `.text` for the implemented families, and the count of
implemented lines is reported as a number that only rises.

### S2: the integer instruction table

**What.** The remaining integer mnemonics of §1.1: `add`, `sub`, `imul`, `idiv`, `div`, `and`, `or`,
`xor`, `not`, `neg`, `shl`, `shr`, `sar`, `cmp`, `test`, `cbw`/`cwd`/`cdq`/`cqo`, `xchg`, the twelve
`setcc`, the twelve `jcc`, `movabs`, and the `push QWORD PTR [mem]` form.

**Acceptance.** 100 percent of the integer instruction lines in the corpus round-trip, including the
eight unobserved mnemonics of §1.2, which get their own hand-written test lines because no fixture
reaches them.

### S3: the SSE2 scalar table

**What.** The thirteen SSE2 mnemonics of §1.1, plus `movq`/`movd`, plus the `f2`/`f3` prefix handling
and the immediate float path of §3.3.

**Acceptance.** 100 percent of the corpus's instruction lines round-trip; `asm_float_scalars`,
`asm_float_print` and `float_datac_memory` disassemble identically from the own `.o` and from `as`.

### S4: the object writer

**What.** ELF64 relocatable output: `.text`, `.rodata`, `.bss`, `.data`, `.symtab`, `.strtab`,
`.shstrtab`, `.rela.text`, and the relocation forms of §3.4. `--backend=own` emits the object and
hands it to `ld` with the unchanged argument list of §3.1.

**Acceptance.** All 177 fixtures build under `--backend=own`, run, and print the C backend's bytes;
`readelf -rW` on the own `.o` shows only `PLT32`, `PC32` and `GOTPCREL`; the section and symbol
inventory matches §3.2 modulo the `.LCn`-as-symbol choice.

### S5: the own linker

**What.** Read the `.o` plus `crt1.o`, `crti.o`, `crtn.o` and the `libc.so.6`/`libm.so.6` symtabs;
lay out the segments; write `PT_INTERP`, the `PT_LOAD`s, `PT_DYNAMIC`, the GOT, the PLT stubs and the
`.dynsym`/`.dynstr`/`.gnu.version`/`.gnu.version_r`/`.gnu.hash` blocks of §4. The `ld` invocation is
deleted from `driver.rs`.

**Acceptance.** The same 177 fixtures build with no `as` and no `ld` on the path (`PATH` scrubbed of
both), run, and print the C backend's bytes; `readelf -hW` and `readelf -lW` show the §5.1 positive
properties for every one of them.

### S6: the §6.4 structural gate

**What.** The `readelf`/`objdump` assertions of §5.1 in the test suite, so a malformed binary fails
the build. Every assertion from §5.1 that can be a positive property in one command.

**Acceptance.** `cargo test` fails, with the offending property named, when the writer is mutated to
produce an off-by-one `e_entry`, a `PT_INTERP` without its NUL, a `DT_PLTRELSZ` that disagrees with
`.rela.plt`, a `JUMP_SLOT` pointing outside `.got.plt`, or a `disp32` truncated to `disp8`. Each
mutation is listed in the test so the gate's sensitivity is recorded rather than assumed.

### S7: the differential wiring

**What.** §5.2: `Backend::Own`, the third arm in `main.rs`, the third sweep in
`backend_coverage.rs` with an `OWN_BUILT_BASELINE`, `"own"` in the differential test's backend array,
and `BACKENDS="c asm own"` as the documented invocation.

**Acceptance.** `BACKENDS="c asm own" ./kelpc/tests/run-all-backends.sh` passes; `cargo test --test
backend_coverage -- --ignored` reports the own sweep's built and refused counts; a fixture the own
backend refuses fails the run if its message does not carry the own backend's refusal prefix.

### S8: the speed gate

**What.** `bench/speed/` under `--backend=own`, against §5's fourth item and §6's definition of done:
checksums match and every workload is no worse than the `-O0` column.

**Acceptance.** The four workloads run, all four checksums match the C backend's, and no elapsed time
exceeds the recorded `-O0` column.

### What is deliberately not here

No optimizer, no capability table (§3.5), no `_start` synthesis (§4.4 option 2), no PIE, and no
decoder beyond what §S1's harness needs. Each of those is named because leaving it unnamed is how a
Stage 2 that is already large becomes a Stage 2 that is never finished.

---

## 7. Undetermined

1. **Whether the own linker should be `ET_EXEC` or `ET_DYN`.** The measured `ld` output here is
   `ET_EXEC` with 5 relocation types; a PIE adds `R_X86_64_RELATIVE` and changes the GOT layout.
   §6.3 does not say. `ld` was invoked without `-pie`, so `ET_EXEC` is the default this branch
   already ships, and that is the recommendation, but it is a decision rather than a measurement.
2. **The exact set of mnemonics a *later* slice will add.** §5 items 6 and 7 (closures, indirect
   calls, capability checks) are refused today, and a refusal emits nothing, so this inventory cannot
   see them. `FnRef`/`IndirectCall` will need `call r/m64`, and a capability check will need
   `R_X86_64_64` data. Pin it with `grep -n 'unsupported("a function value")' kelpc/src/codegen_asm.rs`
   plus the emitted `.s` of a fixture written for the slice, once the slice exists.
3. **The relocation type `as` would choose for a call to a symbol the own writer does not export.**
   `PLT32` for a global and `PC32` for a local is what the corpus shows, but no fixture produces a
   non-exported, non-local function symbol, so the boundary between the two is inferred from three
   `.o` files rather than measured across the corpus. Pin it with `as` on a two-line `.s` that calls
   a `.local` symbol and a `.globl` one, and read `readelf -rW`.
4. **The exact `.gnu.hash` shape glibc requires for a symbol set this small.** `ld` writes 28 bytes
   for `hello`; the bucketing rule that size comes from was not determined here. Pin it with
   `readelf -x .gnu.hash hello` plus `readelf --dyn-syms hello` and the documented GNU hash layout;
   §4.2 named it because a malformed one fails silently.
5. **Whether every one of the 53 `Routine` variants is reachable from a fixture.** 52 appear in the
   corpus. The odd one out was not identified, since a routine can be emitted with its label present
   and its call site in a fixture the corpus does not contain. Pin it with
   `comm -23 <(grep -o 'kelp_rt\$[A-Za-z0-9_]*' kelpc/src/codegen_asm.rs | sort -u) <(cat
   /tmp/corpus/*.s | grep -o 'kelp_rt\$[A-Za-z0-9_]*' | sort -u)`.
6. **The instruction lengths above 7 bytes.** The first histogram was taken with plain `objdump`,
   whose byte column wraps for a long instruction; it was redone with `objdump -dw`, which is where
   the 10-byte `movabs` comes from. Any length not listed in §2.4 is undetermined rather than absent.
7. **A mnemonic the emitter writes only through a path no fixture and no source literal expose.**
   The three derivations in §1 agree, and the mnemonic producers in §1's method 2 are enumerated by
   name, so a fourth source would have to be a `format!` that composes a mnemonic from two strings.
   The scan for that shape (`grep -n 'let mnemonic' kelpc/src/codegen_asm.rs`) finds nine sites, all
   read here. Confidence is high, not proof.
