# Heap runtime specification

*What the asm backend must own to lower the six heap types. Every claim
carries a `file:line` citation. Where code and I disagree, see
[Uncertainties](#uncertainties). No code here is changed; this is a reading of
`codegen_c.rs` and `types.rs`.*

Sources read: `kelpc/src/codegen_c.rs` (`RUNTIME` at :36-542, `write_struct`
:979, `write_enum` :1027, `write_container_typedef` :1089, `write_tags` :1355,
`write_box` :1406, `write_map` :1467, `write_list` :1659, `write_list_s1`
:1734, `call_value` :3019), `kelpc/src/types.rs`, `kelpc/src/externs.rs`.

## 0. Naming rule (how codegen spells a type)

- `Ty::c_type` (`types.rs:222`) names the C type: `Bytes` -> `kelp_Bytes`,
  `String` -> `kelp_String`, `View` -> `kelp_View` (`types.rs:239-241`).
- `List<T>` -> `list_c_type` (`types.rs:572`) = `kelp_List__{ty_mangle(T)}`.
- `Box<T>` -> `box_c_type` (`types.rs:577`) = `kelp_Box__{ty_mangle(T)}`.
- `Map<K,V>` -> `map_c_type` (`types.rs:582`) =
  `kelp_Map__{ty_mangle(K)}__{ty_mangle(V)}`.
- `ty_mangle` (`types.rs:545`): `i32`->`i32`, `String`->`String`,
  `Bytes`->`Bytes`, a `datac` `shapes.circle.Circle` -> `shapes__circle__Circle`
  (via `c_ident`, `types.rs:656`), nested `Box<List<i32>>` -> `Box__List__i32`.
  An integer element is a compact name: `kelp_List__i32` (`types.rs:568-571`
  comments).
- `drop_fn_name` (`types.rs:461`): `Bytes`->`kelp_bytes_drop`,
  `String`->`kelp_string_drop`, `datac`->`kelp_drop_{c_ident(name)}`,
  `List`->`kelp_list_drop__{m}` , `Box`->`kelp_box_drop__{m}`,
  `Map`->`kelp_map_drop__{k}__{v}`. `View` has **no** drop arm (it is `Copy`;
  `is_copy` `types.rs:169`), and reaching `drop_fn_name(View)` is
  `unreachable!`.
- A `datac`/`enum` element inside a container typedef is spelled by its **tag**
  (`container_elem_c_type`, `types.rs:631`): `struct tag_{c_ident(name)}`,
  forward-declared by `write_tags` (`codegen_c.rs:1355-1365`). Every other
  element is `c_type()`.

## 1. Exact C layout, offsets, alignment, SysV class

All offsets/sizes below were also verified empirically with `offsetof` on the
typedefs (`/tmp`, not in the repo); they match the source.

### kelp_Bytes (`codegen_c.rs:37-44`)
```
typedef struct { unsigned char* data; size_t len; size_t cap; } kelp_Bytes;
```
| field | type | offset | width |
| --- | --- | --- | --- |
| data | `unsigned char*` | 0 | 8 |
| len | `size_t` | 8 | 8 |
| cap | `size_t` | 16 | 8 |

size 24, align 8. 3 INTEGER eightbytes + size > 16 -> **MEMORY** class.
Element-type dependent: no.

### kelp_String (`codegen_c.rs:316-321`)
Identical layout to `kelp_Bytes` (`unsigned char* data; size_t len; size_t
cap;`, `codegen_c.rs:316-321`). size 24, align 8, **MEMORY**. No.

### kelp_View (`codegen_c.rs:115-118`)
| field | type | offset | width |
| --- | --- | --- | --- |
| ptr | `const char*` | 0 | 8 |
| len | `size_t` | 8 | 8 |

size 16, align 8, two INTEGER eightbytes -> **register pair** (arg
`rdi`+`rsi`, return `rax`+`rdx`). No.

### kelp_List__* (`codegen_c.rs:1096-1101`)
```
typedef struct { <T-elem>* data; size_t len; size_t cap; } kelp_List__<m>;
```
| field | type | offset | width |
| --- | --- | --- | --- |
| data | `T*` (`container_elem_c_type(T)`) | 0 | 8 |
| len | `size_t` | 8 | 8 |
| cap | `size_t` | 16 | 8 |

size 24, align 8, **MEMORY**. The struct's size/alignment do **not** depend on
`T`: only the pointee type of `data` changes (`container_elem_c_type`), which is
`struct tag_Foo*` for a `datac`. The *function names* are monomorphized
(`ty_mangle(T)`), the layout is not.

### kelp_Box__* (`codegen_c.rs:1106-1111`)
```
typedef struct { <T-elem>* value; } kelp_Box__<m>;
```
value at offset 0, width 8. size 8, align 8, one INTEGER eightbyte ->
`rdi`/`rax`. No dependence on `T`.

### kelp_Map__K__V (`codegen_c.rs:1121-1134`)
```
typedef struct { K key; V* value; uint8_t live; } kelp_Map__K__V_entry;
typedef struct {
    <entry>* entries;
    size_t entry_count;
    size_t entry_cap;
    size_t len;
    size_t* index;
    size_t index_cap;
    size_t index_used;
} kelp_Map__K__V;
```
| field | type | offset | width |
| --- | --- | --- | --- |
| entries | entry* | 0 | 8 |
| entry_count | size_t | 8 | 8 |
| entry_cap | size_t | 16 | 8 |
| len | size_t | 24 | 8 |
| index | size_t* | 32 | 8 |
| index_cap | size_t | 40 | 8 |
| index_used | size_t | 48 | 8 |

size 56, align 8, **MEMORY**. The map header does **not** depend on `K`/`V`.
The **entry** struct does, because the key is inline:

| entry form | key | value | live | size | align |
| --- | --- | --- | --- | --- | --- |
| `K=String` (`key.c_type()` = `kelp_String`, 24) | 0..24 | 24 | 32 | 40 | 8 |
| `K` scalar (`i32`/`i64`/`bool`, `codegen_c.rs:1126`) | 0..8 | 8 | 16 | 24 | 8 |

`value` is always a pointer (a `datac` value is `struct tag_Foo*`); `live` is
`uint8_t` at `align_up(sizeof(K),8)+8`. An entry is only ever an array element
(`entries[e]`), never a by-value argument, so its SysV class is not exercised —
but its size is what `realloc(entries, cap*sizeof(entry))`
(`codegen_c.rs:1583`) and `memset(index, ...)` count on.

**SysV rule used throughout:** <=8 bytes -> one INTEGER eightbyte; 9-16 -> two
INTEGER eightbytes (register pair); >16 -> MEMORY. This mirrors
`codegen_asm.rs::agg_class` (`:807-816`) and `enum AggClass` (`:757`), and is
sound here because every field class is INTEGER (all fields are integers or
pointers; no SSE eightbyte exists — same argument the plan makes for `datac`,
§5 item 4, third slice). So: `Box` = one eightbyte; `View` = a register pair;
`Bytes`/`String`/`List`/`Map` = MEMORY (returned through the hidden first-arg
pointer; by-value parameters go in the caller's outgoing argument area).

## 2. Field invariants and who maintains them

### kelp_Bytes / kelp_String

| invariant | statement | maintained by (cite) |
| --- | --- | --- |
| cap+1 allocation | the buffer is always `cap + 1` bytes; `cap` counts usable bytes **excluding** the NUL slot | comment `codegen_c.rs:41-43`; `kelp_buf_reserve` `:169` does `realloc(*data, want + 1)` `:180`; constructors set `cap = len` exactly: `from_str` `:229`, `clone` `:265`, `alloc_raw` `:529` |
| `data[len] == 0` | the byte at `len` is always NUL, so a `str` view / `printf("%s")` of the buffer is valid | `kelp_buf_reserve` writes `p[len] = 0` after every `realloc` `:184`; `__push` `:283`, `__append` `:291`, `__push_str` `:422`; `__empty` `:212` (`p[0]=0`); `from_str` copies `n+1` `:233`; `clone` `:270`; `alloc_raw` `memset(p,0,n+1)` `:534` |
| empty value points at a NUL byte, never NULL | `Bytes.empty()`/`String.empty()` = `malloc(1)` with `p[0]=0`, `len=0`, `cap=0` | comments `codegen_c.rs:212-216`, `:324`; bodies `:217-227`, `:324-335` |
| `len` = byte count | number of live bytes; a `str` view of the buffer is `len` bytes | `__len` `:243`, `kelp_String__len` `:373`; `char_count` `:381` counts code points separately |
| text validity | `String` bytes must be valid UTF-8 | `kelp_String__from_bytes` `:353-355`; `kelp_Bytes__as_str` `:482-484`; `kelp_utf8_valid` `:186` |

`kelp_String__with_capacity(n)` (`codegen_c.rs:429`) sets `data=NULL, len=0,
cap=0` then reserves; for `n=0` the reserve early-returns (`len+extra <= *cap`)
and leaves `data==NULL` — see Uncertainties.

### kelp_View
| invariant | statement | maintained by |
| --- | --- | --- |
| borrow, no ownership | `{ptr,len}`; owns nothing, never dropped | `is_copy` `types.rs:169`; no `drop_fn_name` arm |
| `len` is exact | every op is length-aware; no reliance on a NUL after the slice | comment `codegen_c.rs:103-113`; `__eq` `:147` (memcmp over exact len), `__to_string` `:466` copies `len` |

### kelp_List__T
| invariant | statement | maintained by |
| --- | --- | --- |
| `len` = live element count | elements `0..len` are initialized | comment `codegen_c.rs:1089`; `__push` `:1695`, `__pop` `:1705`, `__insert` `:1773`, `__remove` `:1783` all keep it |
| `cap` >= `len`; buffer is `cap*sizeof(T)` bytes | `data` may be NULL iff `cap==0` | `__empty` `:1670`, `__reserve` `:1675` |
| owned elements dropped before reuse | `set`/`clear`/`drop` free the element they overwrite/remove | `__set` `:1702`, `__clear` `:1710`, `kelp_list_drop__*` `:1728` |

### kelp_Box__T
| invariant | statement | maintained by |
| --- | --- | --- |
| `value == NULL` means empty | `empty()` `:1414`, `take()` clears it `:1450` | `write_box` `codegen_c.rs:1406` |
| `value != NULL` -> one live `T` at `malloc(sizeof(T))` | `of`/`set` allocate | `:1417`, `:1436` |
| drop/replace frees the held value | `set` drops the old `T` before overwrite `:1439`; drop drops then frees `:1455` | `write_box` |

### kelp_Map__K__V
| invariant | statement | maintained by |
| --- | --- | --- |
| entries array in insertion order | `entries[0..entry_count]`; live entries have `live==1` | comment `:1471-1477`, `write_map` `:1467` |
| `len` = live key count | `insert++` `:1598`, `remove--` `:1618`, `clear=0` `:1626` | `write_map` |
| `index` open-addressed, `entry index + 1` | slot holds `e+1`; `0` = empty; `SIZE_MAX` = tombstone | comment `:1471-1477`, `:1123-1127`; probe `__find` `:1540`; `remove` writes `SIZE_MAX` `:1617` |
| `index_cap` a power of two | probe masks by `index_cap - 1` | `__rehash` `:1562-1564` |
| `index_used` = live indexed slots | load factor `(index_used+1)*4 >= index_cap*3` triggers rehash | `__rehash` `:1571`, `__insert` `:1576` |
| `live` flag | `1` live, `0` dead (removed/cleared); the slot's `value` pointer is NULL when dead | `insert` `:1597`, `remove` `:1616`, `clear` `:1623` |
| a `String` key owns its buffer | stored via `kelp_String__from_str`/`__clone`; freed via `kelp_string_drop` | `store_key` `:1521`, `clone_key` `:1528`, `drop_key` `:1536` |

## 3. Runtime functions per type

`L` = a `List` C type, `B` a `Box` type, `M` a `Map` type, `T`/`V`/`K` the
element/value/key C types, `Karg` = `const char*` for a `String` key else `K`
(`codegen_c.rs:1484-1488`). Bodies are emitted by `write_list`/`write_box`/
`write_map` unless marked *(fixed)* = the `RUNTIME` string.

### Bytes — 13 methods + 1 allocator, 2 shared helpers *(fixed)*
| C name | signature | returns | panics with | libc | does |
| --- | --- | --- | --- | --- | --- |
| `kelp_Bytes__empty` `:212` | `(void)` | `kelp_Bytes` | `out of memory` | malloc | one NUL byte, len=0,cap=0 |
| `kelp_Bytes__from_str` `:229` | `(const char*)` | `kelp_Bytes` | `out of memory` | strlen, malloc, memcpy | copy a C string |
| `kelp_Bytes__len` `:243` | `(const kelp_Bytes*)` | `size_t` | — | — | byte length |
| `kelp_Bytes__is_empty` `:247` | `(const kelp_Bytes*)` | `bool` | — | — | `len==0` |
| `kelp_Bytes__get` `:251` | `(const kelp_Bytes*, size_t)` | `uint8_t` | `Bytes.get: index out of bounds` | — | bounds-checked byte read |
| `kelp_Bytes__set` `:258` | `(kelp_Bytes*, size_t, uint8_t)` | `void` | `Bytes.set: index out of bounds` | — | bounds-checked byte write |
| `kelp_Bytes__clone` `:265` | `(const kelp_Bytes*)` | `kelp_Bytes` | `out of memory` | malloc, memcpy | deep copy |
| `kelp_Bytes__push` `:279` | `(kelp_Bytes*, uint8_t)` | `void` | `out of memory` (via reserve) | realloc | append one byte, re-NUL |
| `kelp_Bytes__append` `:285` | `(kelp_Bytes*, const kelp_Bytes*)` | `void` | `out of memory` | realloc, memcpy | append another buffer |
| `kelp_Bytes__reserve` `:295` | `(kelp_Bytes*, size_t)` | `void` | `out of memory` | realloc | grow to fit `extra` |
| `kelp_Bytes__clear` `:299` | `(kelp_Bytes*)` | `void` | — | — | len=0, `data[0]=0` |
| `kelp_Bytes__as_str` `:482` | `(const kelp_Bytes*)` | `const char*` | `Bytes.as_str: input is not valid utf-8` | — | checked borrowed view |
| `kelp_bytes_drop` `:306` | `(kelp_Bytes*)` | `void` | — | free | free, data=NULL, len=0 |
| `kelp_bytes_alloc_raw` `:529` | `(size_t n)` | `kelp_Bytes` | `out of memory` | malloc, memset | zeroed NUL-terminated buffer of `n` |
| `kelp_buf_reserve` `:169` *(shared)* | `(unsigned char**, size_t, size_t*, size_t)` | `void` | `out of memory` | realloc | doubling growth + re-NUL |
| `kelp_utf8_valid` `:186` *(shared)* | `(const unsigned char*, size_t)` | `bool` | — | — | UTF-8 well-formedness check |

### String — 16 functions *(fixed)*
| C name | signature | returns | panics with | libc | does |
| --- | --- | --- | --- | --- | --- |
| `kelp_String__empty` `:323` | `(void)` | `kelp_String` | `out of memory` | malloc | one NUL byte |
| `kelp_String__from_str` `:337` | `(const char*)` | `kelp_String` | `out of memory` | strlen, malloc, memcpy | copy text |
| `kelp_String__from_bytes` `:353` | `(kelp_Bytes)` | `kelp_String` | `String.from_bytes: input is not valid utf-8` | kelp_utf8_valid | adopt a valid `Bytes` buffer |
| `kelp_String__into_bytes` `:365` | `(kelp_String)` | `kelp_Bytes` | — | — | hand the buffer out |
| `kelp_String__len` `:373` | `(const kelp_String*)` | `size_t` | — | — | byte length |
| `kelp_String__is_empty` `:377` | `(const kelp_String*)` | `bool` | — | — | `len==0` |
| `kelp_String__char_count` `:381` | `(const kelp_String*)` | `size_t` | — | — | count code points |
| `kelp_String__clone` `:391` | `(const kelp_String*)` | `kelp_String` | `out of memory` | malloc, memcpy | deep copy |
| `kelp_String__push` `:408` | `(kelp_String*, char)` | `void` | `out of memory` | realloc | append one byte |
| `kelp_String__push_str` `:414` | `(kelp_String*, const char*)` | `void` | `out of memory` | strlen, realloc, memcpy | append C text |
| `kelp_String__reserve` `:425` | `(kelp_String*, size_t)` | `void` | `out of memory` | realloc | grow |
| `kelp_String__with_capacity` `:429` | `(size_t)` | `kelp_String` | `out of memory` | realloc | reserve up front |
| `kelp_String__clear` `:438` | `(kelp_String*)` | `void` | — | — | len=0, `data[0]=0` |
| `kelp_String__as_str` `:447` | `(const kelp_String*)` | `const char*` | — | — | borrowed `str` |
| `kelp_String__view` `:454` | `(const kelp_String*)` | `kelp_View` | — | — | `{data,len}` view |
| `kelp_string_drop` `:489` | `(kelp_String*)` | `void` | — | free | free, data=NULL, len=0 |

### View — 7 functions *(fixed; no drop)*
| C name | signature | returns | panics with | libc | does |
| --- | --- | --- | --- | --- | --- |
| `kelp_View__len` `:120` | `(kelp_View)` | `size_t` | — | — | length |
| `kelp_View__is_empty` `:124` | `(kelp_View)` | `bool` | — | — | `len==0` |
| `kelp_View__get` `:128` | `(kelp_View, size_t)` | `uint8_t` | `View.get: index out of bounds` | — | bounds-checked read |
| `kelp_View__slice` `:135` | `(kelp_View, size_t, size_t)` | `kelp_View` | `View.slice: index out of bounds` | — | sub-view |
| `kelp_View__eq` `:147` | `(kelp_View, kelp_View)` | `bool` | — | memcmp | length-aware equality |
| `kelp_View__to_string` `:466` | `(kelp_View)` | `kelp_String` | `out of memory` | malloc, memcpy | copy `len` bytes + NUL |
| `kelp_str__view` `:158` | `(const char*)` | `kelp_View` | — | strlen | view a whole `str` |

### List — 18 per instantiation + 1 drop (some conditional)
Conditional set is decided by `list_sortable_elem` / `list_comparable_elem` /
`list_clonable_elem` (`types.rs:430-448`), same predicates `infer.rs` uses.

| C name (per `L`) | signature | returns | panics with | libc | does |
| --- | --- | --- | --- | --- | --- |
| `<L>__empty` `:1670` | `(void)` | `L` | — | — | data=NULL,len=0,cap=0 |
| `<L>__reserve` `:1675` | `(L*, size_t)` | `void` | `out of memory` | realloc | grow to `want` |
| `<L>__of` `:1684` | `(T)` | `L` | `out of memory` | realloc | one-element list |
| `<L>__len` `:1689` | `(const L*)` | `size_t` | — | — | element count |
| `<L>__is_empty` `:1690` | `(const L*)` | `bool` | — | — | `len==0` |
| `<L>__push` `:1695` | `(L*, T)` | `void` | `out of memory` | realloc | append |
| `<L>__bounds` `:1698` | `(const L*, size_t)` | `void` | `List.get: index out of bounds` | — | bounds check |
| `<L>__get` `:1702` | `(const L*, size_t)` | `T` | bounds msg | — | copy out a `Copy` elem |
| `<L>__get_const` `:1705` | `(const L*, size_t)` | `const T*` | bounds msg | — | borrow an owned elem |
| `<L>__set` `:1710` | `(L*, size_t, T)` | `void` | bounds msg | — | overwrite (drops old elem) |
| `<L>__pop` `:1714` | `(L*)` | `T` | `List.pop: the list is empty` | — | remove last |
| `<L>__clear` `:1719` | `(L*)` | `void` | — | — | drop elems `0..len`, len=0 |
| `<L>__cmp` `:1741` | `(const T*, const T*)` | `int` | — | memcmp (String/Bytes) | comparator (if sortable) |
| `<L>__sort` `:1759` | `(L*)` | `void` | — | — | stable insertion sort (if sortable) |
| `<L>__reverse` `:1777` | `(L*)` | `void` | — | — | in-place reverse |
| `<L>__contains` `:1794` | `(const L*, <param>)` | `bool` | — | memcmp (String/Bytes) | membership (if comparable) |
| `<L>__slice` `:1807` | `(const L*, size_t, size_t)` | `L` | `List.slice: range out of bounds` | realloc | clone range (if clonable) |
| `<L>__insert` `:1827` | `(L*, size_t, T)` | `void` | `List.insert: index out of bounds` | realloc | shift + insert |
| `<L>__remove` `:1836` | `(L*, size_t)` | `T` | `List.remove: index out of bounds` | — | shift out, return value |
| `kelp_list_drop__{m}` `:1728` | `(L*)` | `void` | — | free | `__clear` + free buffer |

### Box — 6 per instantiation + 1 drop
| C name (per `B`) | signature | returns | panics with | libc | does |
| --- | --- | --- | --- | --- | --- |
| `<B>__empty` `:1414` | `(void)` | `B` | — | — | value=NULL |
| `<B>__of` `:1417` | `(T)` | `B` | `out of memory` | malloc | allocate one `T` |
| `<B>__is_empty` `:1427` | `(const B*)` | `bool` | — | — | `value==NULL` |
| `<B>__get` `:1431` | `(const B*)` | `T` | `Box.get: the box is empty` | — | copy out a `Copy` T |
| `<B>__get_const` `:1434` | `(const B*)` | `const T*` | `Box.get: the box is empty` | — | borrow an owned T |
| `<B>__set` `:1438` | `(B*, T)` | `void` | `out of memory` | malloc | allocate-if-empty, drop old, store |
| `<B>__take` `:1450` | `(B*)` | `T` | `Box.take: the box is empty` | free | move value out, free box |
| `kelp_box_drop__{m}` `:1455` | `(B*)` | `void` | — | free | drop value, free box, NULL |

### Map — 12 methods + 1 drop (+3 shared key helpers)
| C name (per `M`) | signature | returns | panics with | libc | does |
| --- | --- | --- | --- | --- | --- |
| `<M>__new` `:1539` | `(void)` | `M` | — | — | all-NULL/0 header |
| `<M>__find` `:1544` | `(const M*, Karg, bool*)` | `size_t` | — | memcmp/strlen (String key) | probe; slot or insert-at |
| `<M>__rehash` `:1562` | `(M*, size_t want)` | `void` | `out of memory` | free, malloc, memset | rebuild index, drop tombstones |
| `<M>__insert` `:1576` | `(M*, Karg, V)` | `void` | `out of memory` | realloc, malloc | insert/replace; drops old value |
| `<M>__has` `:1600` | `(const M*, Karg)` | `bool` | — | — | key present |
| `<M>__get` `:1605` | `(const M*, Karg)` | `V` | `Map.get: key not found` | — | copy a `Copy` value |
| `<M>__get_const` `:1608` | `(const M*, Karg)` | `const V*` | `Map.get: key not found` | — | borrow an owned value |
| `<M>__get_or` `:1611` | `(const M*, Karg, V)` | `V` | — | clone helpers | value or fallback (if clonable) |
| `<M>__remove` `:1617` | `(M*, Karg)` | `V` | `Map.remove: key not found` | free | mark dead, tombstone, return value |
| `<M>__clear` `:1622` | `(M*)` | `void` | — | free, memset | drop all live, keep buffers |
| `<M>__len` `:1628` | `(const M*)` | `size_t` | — | — | live count |
| `<M>__is_empty` `:1629` | `(const M*)` | `bool` | — | — | `len==0` |
| `<M>__keys` `:1632` | `(const M*)` | `kelp_List__{m(K)}` | `out of memory` | realloc | insertion-order key list (clones keys) |
| `<M>__values` `:1644` | `(const M*)` | `kelp_List__{m(V)}` | `out of memory` | realloc | insertion-order values (if clonable) |
| `kelp_map_drop__{k}__{v}` `:1651` | `(M*)` | `void` | — | free | drop live values/keys, free both arrays |
| `kelp_map_hash` `:502` *(shared)* | `(const char*, size_t)` | `uint64_t` | — | — | FNV-1a over key bytes |
| `kelp_map_key_eq` `:511` *(shared)* | `(const kelp_String*, const char*, size_t)` | `bool` | — | memcmp | length-aware `String` key equality |
| `kelp_map_mix` `:520` *(shared)* | `(uint64_t)` | `uint64_t` | — | — | scalar-key 64-bit finalizer |

Counts: **Bytes 14 (+2 shared), String 16, View 7, List 19 (18 + drop, up to 4
conditional), Box 7, Map 15 (14 + drop, 1 conditional) (+3 shared key helpers).**

## 4. Drop order

The fixed drop entry points are `kelp_bytes_drop` / `kelp_string_drop`; every
other type's drop is **generated per instantiation** and named by
`drop_fn_name` (`types.rs:461`). The caller is `drop_call`
(`codegen_c.rs:3356`), driven by `borrowck.rs`'s `DropPlan`.

| type | what a drop frees, in order | generated? |
| --- | --- | --- |
| `Bytes` | `free(data)`; then `data=NULL`, `len=0` (`cap` left stale) `:306-310` | fixed |
| `String` | `free(data)`; then `data=NULL`, `len=0` (`cap` left stale) `:489-493` | fixed |
| `View` | nothing (Copy borrow) | no drop |
| `List<T>` | `__clear`: drop elements `data[0..len]` in order (only if `needs_drop(T)`), then `free(data)`, `data=NULL`, `cap=0` `:1728-1733` | yes, per `T` |
| `Box<T>` | if `value!=NULL`: drop `*value` (if `needs_drop(T)`), then `free(value)`, `value=NULL` `:1455-1462` | yes, per `T` |
| `Map<K,V>` | for each `e` in `0..entry_count`: skip `!live`; drop `*entries[e].value` (if `needs_drop(V)`), `free(entries[e].value)`; drop the key (`kelp_string_drop`) if `K==String`; then `free(entries)`, `free(index)`, zero all header fields `:1651-1658` | yes, per `K,V` |
| `datac` | drop each owned field in declaration order, calling `drop_fn_name(field.ty)` `:995-1002` | yes, per `datac` |
| `enum` | switch on `v->kelp_tag`; drop the live variant's owned payload fields only `:1027-1072` | yes, per `enum` |

Recursive cases:
- `List<String>`: `elem_drops = needs_drop(String) = true`; `__clear` calls
  `kelp_string_drop(&data[i])` per element, then drops the buffer (`:1710`,
  `:1728`).
- `List<Shape>` where `Shape` is an `enum`: `owned_types` (`types.rs:385`)
  marks `Shape` owned if any variant owns; `__clear` then calls
  `kelp_drop_Shape(&data[i])`, which switches on each element's tag.
- recursive `Box` chain (`Node` holds `Box<Node>`): `kelp_box_drop__Node`
  calls `kelp_drop_Node` on the held value (because `needs_drop(Node)`), and
  `kelp_drop_Node` calls `kelp_box_drop__Node` on its field — one frame per
  link, terminated by the `NULL` (`Box.empty()`).
- `Map` whose values own memory: `kelp_map_drop__K__V` drops each live
  `*value`; dead entries were already freed by `remove`/`clear`.
- `enum`, exactly one variant live: the drop *must* be generated and must
  switch on `kelp_tag`, because the compiler cannot know statically which
  payload owns memory; only the live variant's owned fields are dropped (no
  `default:` arm, by design — `codegen_c.rs:1067-1069`).

Which cases need a generated per-type drop (not a fixed routine): every
`List<T>`, `Box<T>`, `Map<K,V>`, `datac` that owns memory, and every `enum` any
of whose variants owns memory. The switch a generated function has to make is
compile-time for `List`/`Box`/`Map` (does the element/value/key need a drop,
`needs_drop`, `types.rs:451`) and **run-time for `enum`** (which variant's tag
is set).

## 5. Terminal primitives the heap runtime depends on

Not declared in `externs.rs` — the generated C calls them directly and the asm
backend must supply them (libc `@PLT` at first, raw syscalls later). Counted
over the `RUNTIME` text (`codegen_c.rs:36-542`):

`malloc`, `realloc`, `free`, `memcpy`, `memset`, `memcmp`, `strlen`
(allocation/copy/compare). Also on the print/panic path the same text uses
`fprintf`, `printf`, `stderr`, `stdout`, `fflush`, `exit`, `strerror`, `errno`,
and the extern bodies add `snprintf`, `strtoll`, `strtoull`, `strtod`,
`getenv`, `setenv`, `unsetenv`, `fopen`/`fread`/`fwrite`/`fclose`/`fseek`/
`ftell`/`rewind`/`remove`/`rename`/`mkdir`/`stat`/`scandir`, `clock_gettime`,
`nanosleep`, `pipe`/`fork`/`execvp`/`waitpid`/`kill`, and the socket family.

`externs.rs` bodies that **depend on** these heap routines (so the routines
must exist before those externs can be lowered): `std/mem.alloc_raw` ->
`kelp_bytes_alloc_raw`; `std/str.from_f64_raw` -> `kelp_bytes_alloc_raw`,
`kelp_String__from_bytes`; `std/fmt.hex_raw` -> `kelp_String__from_str`;
`std/fmt.bytes_raw` -> `kelp_String__empty`, `kelp_bytes_alloc_raw`,
`kelp_String__from_bytes`; `std/fs.read_raw`/`list_raw` -> `kelp_bytes_alloc_raw`,
`kelp_String__from_bytes`; `std/env.get_raw` -> `kelp_String__empty`,
`kelp_String__from_str`; `std/io.read_raw`/`std/net.recv_raw` ->
`kelp_bytes_alloc_raw`; `std/process.arg_at_raw` -> `kelp_String__from_str`;
`std/path.normalize_raw` -> the `kelp_Bytes` view layout, `kelp_bytes_alloc_raw`,
`kelp_String__from_bytes`. Bodies that read `kelp_Bytes`/`kelp_String` fields
directly depend on the §1 layout: `std/io.read_into_raw`/`write_bytes_raw`,
`std/net.send_raw`/`recv_into_raw`, `std/fs.write_raw`/`write_text_raw`, and
`std/process.spawn_raw` (which names `kelp_List__String` and `kelp_String.data`
literally — `externs.rs:390`).

## Uncertainties

1. **`String.with_capacity(0)`.** `kelp_String__with_capacity` (`:429`) starts
   with `data=NULL` and `reserve` early-returns when `len+extra <= cap`, so a
   zero capacity leaves `data==NULL` — breaking the "an empty value never
   points at NULL" invariant the other constructors keep. I could not find a
   call site that passes 0, and no fixture pins it. Treat as an edge to decide,
   not a guaranteed difference.
2. **`cap` after a drop.** `kelp_bytes_drop`/`kelp_string_drop` reset `data`
   and `len` but **not** `cap` (`:306-310`, `:489-493`). Harmless (the pointer
   is NULL) but the struct is not fully re-initialized; a backend that re-reads
   `cap` on a dropped value would see the old value.
3. **SysV threshold.** I state `>16 bytes -> MEMORY` following
   `codegen_asm.rs::agg_class` (`:807-816`) and the plan's own §5 item 4
   ("over sixteen bytes ... `MEMORY`"). The literal SysV wording is subtler
   (an aggregate over two eightbytes is MEMORY *unless* its first eightbyte is
   SSE/SSEUP); since no field here is SSE-classified, both readings agree for
   these six types. A rule that every field is INTEGER/pointer is what makes
   the simple threshold sound.
4. **`Map` entry SysV class** is never exercised (entries are only array
   elements), so I did not classify it; only its size/offset matter.
5. **`Map#get_or` / `values` conditions.** Their emission is keyed on
   `list_clonable_elem(value)` (`:1611`, `:1644`), the same predicate
   `infer.rs` uses; if the two ever diverge, the call site would name a symbol
   `codegen_c.rs` did not emit. I did not re-verify `infer.rs` against it.
6. **Conditional `List` methods** (`__cmp`/`__sort`/`__contains`/`__slice`)
   follow `types.rs`'s predicates (`:430-448`); the count "19" assumes the
   maximal element kind (a sortable+comparable+clonable element such as
   `String`). A `List<datac>` emits fewer.
