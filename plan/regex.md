# `std/regex`: a linear-time regular expression engine in pure Kelp

*A design note for `kelpc/std/regex.kp` (standard library lane, task st4).
Import it with `impl "std/regex"` and call `regex.compile(...)`.*

## Engine

The pattern is parsed to an AST, compiled to a Thompson NFA program and run by
a Pike VM: a list of threads in priority order, the capture slots carried per
thread, over **bytes**. Matching costs O(program size x input length). No
pattern can backtrack, so `(a+)+$`, `(a|aa)+$` and `(x+x+)+y` are as cheap as
any other pattern. There is no recursion proportional to the input: the VM is
iterative with an explicit stack, and the parser and emitter recurse only on
group nesting, which is limited to 100.

Semantics are leftmost-first (Perl-like): the match starts at the leftmost
position where any match exists, and among matches there the first one in
priority order wins (alternation prefers the left branch, greedy repeats prefer
more, lazy repeats prefer fewer).

Refused at `compile` with an error carrying a message and the byte offset:

| Construct | Message |
| --- | --- |
| backreference `\1`..`\9` | `backreferences are not supported` |
| lookaround `(?=` `(?!` `(?<=` `(?<!` | `lookaround is not supported` |
| named group `(?<n>` `(?P<n>` | `named groups are not supported` |
| pattern over 4096 bytes | `pattern is too long (limit 4096 bytes)` |
| groups nested over 100 deep | `nesting is too deep (limit 100)` |
| counted repeat over 1000 | `repeat count is too large (limit 1000)` |
| `{n,m}` with n > m | `invalid repeat: the lower bound is greater than the upper bound` |
| `{` that is not `{n}` `{n,}` `{n,m}` | `invalid repeat: expected {n}, {n,} or {n,m}` |
| `*` `+` `?` `{` with nothing before | `nothing to repeat` |
| `a**`, `a+?+` | `nested repetition operator` |
| `(` without `)` | `unclosed group` |
| stray `)` | `unmatched ')'` |
| `[` without `]` | `unclosed character class` |
| `[z-a]`, `[a-\d]` | `invalid character class range` |
| `\q` and other unknown letters | `invalid escape '\q'` |
| `\x` without two hex digits | `invalid \x escape: two hexadecimal digits are required` |
| pattern ending in `\` | `trailing backslash` |
| unknown flag `(?x)` | `unknown group flag 'x'` |
| program over 50000 instructions, or instructions x capture slots over 2000000 | `pattern is too large (...)` |

## Syntax (exactly this subset)

- Literals: any byte except the metacharacters `\ . [ ] ( ) * + ? { | ^ $`
  (a `]` or `}` on its own is literal). `\` followed by any ASCII punctuation
  byte is that byte (`\.`, `\{`, `\-`, ...).
- `.`: any byte except `\n`; with `(?s)` any byte.
- Classes: `[abc]`, `[a-z0-9_]`, `[^...]`. A `]` right after `[` or `[^` is
  literal; `-` first or last is literal. Inside a class: the escapes `\d \D \w
  \W \s \S`, `\n \r \t`, `\xHH` and escaped punctuation. No POSIX `[:alpha:]`,
  no nested classes, no `\b` (it is an error there).
- Escapes: `\d` `[0-9]`; `\w` `[A-Za-z0-9_]`; `\s` space and `\t \n \v \f \r`;
  `\D \W \S` their complements; `\b` a word boundary and `\B` its negation
  (word = `\w`, ASCII); `\n \r \t`; `\xHH`; `\\`.
- Anchors: `^` start of input, `$` end of input (no trailing-newline
  allowance). With `(?m)`: `^` also after any `\n`, `$` also before any `\n`.
- Groups: `( )` capturing, numbered by opening parenthesis from 1; `(?: )` not.
- Alternation `|`.
- Repetition: `* + ? {n} {n,} {n,m}` and the lazy forms `*? +? ?? {n}? {n,}?
  {n,m}?`. Counted repeats are expanded into the program (`x{2,4}` is `xx(x(x)?)?`).
- Flags: `(?i)` ASCII case-insensitive, `(?s)` dot matches newline, `(?m)`
  multi-line; combinable (`(?is)`); `(?i:...)` scopes them to one group. A bare
  `(?i)` holds until the end of the enclosing group (it carries across `|`).
  There is no `(?-i)`.
- UTF-8: pattern and input are bytes. `.` matches one byte; a non-ASCII literal
  matches its byte sequence and a repeat after it applies to the whole
  character; inside a class its bytes are separate members. Case folding is
  ASCII only. A `&str` has no NUL, so `\x00` never matches there.

An empty alternative is an empty match (`a|`, `()`). The empty pattern matches
the empty string at the start.

## API

```
regex.compile(pattern: &str).R: result.Result<Regex, RegexErr>
datac RegexErr(message: String, offset: usize)
datac Match(start: usize, end: usize, text: String)     // [start, end)

Regex.is_match(&str).R: bool
Regex.find(&str).R: option.Option<Match>
Regex.find_at(&str, start: usize).R: option.Option<Match>
Regex.captures(&str).R: option.Option<Captures>
Regex.find_all(&str).R: List<Match>
Regex.replace(&str, replacement: &str).R: String
Regex.replace_all(&str, replacement: &str).R: String
Regex.split(&str).R: List<String>
Regex.steps().R: i64        // VM steps since compile / reset_steps()
Regex.reset_steps()
Regex.group_count().R: usize
Captures.get(i: usize).R: option.Option<String>
Captures.span(i: usize).R: option.Option<Match>
Captures.len().R: usize     // groups including group 0
regex.is_match(pattern: &str, text: &str).R: bool   // panics on a bad pattern
```

Decisions the design left open:

- **The matching methods take the `Regex` exclusively** (`fun mut`): the scratch
  lives in the `Regex` and a match writes it, so the caller holds a `var`.
- `find_at` searches from byte `start`, but `^`, `\b` and `\B` still see the
  whole text. A `start` past the end is `None`.
- `find_all`, `replace_all` and `split` resume at `end` after a non-empty match
  and at `end + 1` after an empty one, so `a*` over `baac` matches at 0 (empty),
  1..3, 3 (empty) and 4 (empty). `split` over an empty-matching pattern gives
  `"abc"` -> `["", "a", "b", "c", ""]`. `split` returns one more piece than there are
  matches.
- Replacement: `$0` the whole match, `$1`..`$9` a group, `$$` a dollar sign.
  A group that did not take part, or does not exist, is empty; a `$` followed by
  anything else is literal. There is no `${name}`.
- `Captures.get(i)`/`span(i)` for a group that did not take part, or an `i` past
  the last group, is `None`. A group inside a repeat reports its last iteration.
- A `Match.text` that cuts a multi-byte character (a `.` matches one byte) is a
  byte slice; it is built with `View.to_string`, which copies the bytes.

## Resources

A compiled regex owns its scratch, allocated once in `compile`, never per input
byte or per match call. With `N` instructions and `G` groups (including group
0) the scratch is about `4 * (2N * 2G + 2N + 2N + 4N)` bytes of `List<i32>`/`List<i64>`:
two thread lists with their capture slots, the visited marks, and a stack of
`2N + 8` entries. The program itself is `3 * 4 * N` bytes plus 32 bytes per
class. `N * 2G` is capped at 2,000,000 (about 16 MiB of thread slots) and `N` at
50,000, so a compiled regex stays below roughly 24 MiB; a small pattern is a few
kilobytes. Matching an input of length `L` takes at most `N * (L + 1)` thread
steps plus the same number of instruction expansions, and `Regex.steps()`
reports the count.

## Freestanding

`std/regex` is hosted-only. It was built into a riscv64 image with the allocator
pair to see whether the allocator subset (`plan/freestanding-std.md`) was enough.
The parser, the program and the VM link clean: they are `List` and integer code.
Two things do not: `Match`, `RegexErr` and `Option<String>` each carry a `String`,
and the derived equality the compiler emits for such a `datac`/`enum` calls the
hosted `kelp_map_key_eq`; and `View.slice`/`View.to_string` are runtime calls that
a freestanding build does not define. Both are compiler work outside this task,
so the module is `Hosted` in `freestanding::std_admission` and a freestanding
project that imports it is refused by name with the allowed-module list
(`tests/projects/freestanding_std_refused_regex`). A future stage that gives
freestanding a `String`-free result type (spans only) or the two runtime pieces
can admit the engine without changing it.

## Tests

`tests/fixtures/stdlib_regex_*.kp`, run on both backends by
`compile_and_run.rs`: a 150+ case table, the error table, the API (captures,
find_all, replace, split), the pathological patterns, the 1 MiB linearity check
on the `steps()` counter, and a differential check against a slow independent
backtracking reference on seeded random inputs.
