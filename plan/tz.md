# `std/tz`: time zones over the system TZif database in pure Kelp

*A design note for `kelpc/std/tz.kp` (standard library lane, task st5).
Import it with `impl "std/tz"` and call `tz.load(...)`.*

## Data

`tz.load(name)` reads a TZif file (RFC 8536), the compiled form of the tz
database: a header, a sorted list of transition instants with one time type
each, a table of time types (UTC offset, a DST flag, an abbreviation), and for
versions 2 to 4 a second block with 64-bit instants and a footer holding a
POSIX TZ string. Versions 1 to 4 are accepted; a version 1 file is read with
the 32-bit block, and a version 2+ file with the 64-bit block, which is the one
RFC 8536 tells an implementation to prefer. Leap seconds are skipped, not
applied: the whole module works on POSIX instants, and no tz database file
carries them anyway (`right/` is not read).

The file is looked for in `$TZDIR` when that variable is set, non-empty and an
absolute path without `..`, and in `/usr/share/zoneinfo` otherwise. A name with
a leading `/`, a `..` segment or a NUL byte is refused before any path is
built, so no name escapes the database directory.

Times before the first transition use the first time type whose DST flag is
false (type 0 when every type is DST), which is the rule the C library's
`localtime` follows. Times at or after the last transition, and every instant
in a file with no transitions, use the footer's POSIX TZ string when the file
has one; without a footer the last transition's type stands for all later
times, as it does in a version 1 file.

## API

```
tz.load(name: &str).return: result.Result<Zone, TzErr>
tz.load_in(dir: &str, name: &str).return: result.Result<Zone, TzErr>
tz.parse(data: &Bytes).return: result.Result<Zone, TzErr>
tz.utc().return: Zone
tz.fixed(offset_seconds: i32).return: Zone

enum TzErr { NotFound, Denied, Corrupt(val message: String) }

datac Offset(val seconds: i32, val abbrev: String, val is_dst: bool)
datac LocalTime(val year: i32, val month: i32, val day: i32,
                val hour: i32, val minute: i32, val second: i32,
                val weekday: i32, val yday: i32, val offset: Offset)

Zone.offset_at(unix_seconds: i64).return: Offset
Zone.local_parts(unix_seconds: i64).return: LocalTime
Zone.to_unix(y, m, d, h, mi, s: i32).return: Resolved
Zone.format(unix_seconds: i64, fmt: &str).return: String

enum Resolved {
    One(val unix: i64)
    Ambiguous(val first: i64, val second: i64)
    Gap
}
```

`weekday` is 0 for Sunday through 6 for Saturday, `yday` is 1 through 366, and
both follow `localtime`'s own numbering rather than an ISO one.

`to_unix` takes a wall clock reading and hands back the instant or instants it
names. A local time that happens twice, which is what a clock set back does,
is `Ambiguous` with both instants, earlier first; a local time that never
happened, which is what a clock set forward does, is `Gap`. The decision is
made by trying every distinct offset in the zone and keeping the candidates
whose instant really does carry that offset, which is why `Gap` needs no
special case: neither offset reproduces itself.

`format` implements `%Y %m %d %H %M %S %z %Z %a %b %j %%`, and copies any
other `%x` through unchanged, `%` included. `%z` is `+HHMM`/`-HHMM`, `%Z` the
abbreviation the database carries.

`tz.utc()` is the fixed zero offset under the name `UTC`; `tz.fixed(n)` is a
fixed offset with no name, which it spells as `UTC` for zero and as `+HH:MM`
otherwise.

## The capability

`std/tz` reads through the `fs` gate and reads `$TZDIR` through the `env` gate,
so a program that imports it grants both: `impl "std/tz" caps(fs = ["/**"],
env = ["TZDIR"])`. The module adds no authority of its own; it is the caller's
grant that decides which paths and which variable names are reachable, and the
grant must name `TZDIR` itself, because the `env` check is a name check.

`Denied` is what the module answers when it refuses to build a path (a leading
`/`, a `..` segment, a NUL) and when the read itself fails with `EACCES` or
`EPERM`, for example a `$TZDIR` directory the process may not read. A caller's
grant that is too narrow for a path is a *runtime panic* at the `std/tz` call
site rather than a returned `Denied`, because the capability check is compiled
in where the read is written and panics (open item 73, `plan/roadmap.md`): a
value-returning denial needs a non-panicking check mode in codegen, which is a
compiler change outside this task. The escape refusal above is the part of
`Denied` this module can decide on its own, and it is the part the tests pin.

`std/tz` imports `std/fs`, `std/env`, `std/io`, `std/str`, `std/option` and
`std/result`. `zoneinfo` on NixOS is a symlink into the store
(`/etc/zoneinfo` to `/etc/static/zoneinfo`), and the capability check resolves
symlinks before matching, so a grant that names the directory as written denies
every read; name the resolved path, or `/**` in a test.

## Limits

There are three things this module deliberately does not do. It never applies a
leap second: a POSIX instant is a count of seconds with no 23:59:60 in it, the
TZif leap-second table is skipped rather than consulted, and the difference
between UTC and TAI is left to whoever needs it. It never updates or validates
the tz database: the files it reads are whatever the host has installed, it
carries no embedded copy and no version check, and a zone whose rules changed
after that installation is described by the old file. And it never localizes a
name: month, weekday and abbreviation text are the database's own ASCII
abbreviations (`%a`, `%b`, `%Z`) or the English three-letter tables built into
the formatter, so there is no locale, no translated month name and no
`LC_TIME` input anywhere in the module.

Two smaller ones are worth a line. A footer this parser does not understand is
treated as absent rather than as corruption, so an exotic extension in a
newer footer falls back to the last transition's type instead of failing the
load; the one bound it enforces is two weeks on an offset or a rule's time,
which is past glibc's own ceiling of 167 hours and past anything a zone names.
And `to_unix` panics on a date that is not a real calendar date (month
13, day 31 in April, a leap day in a non-leap year) rather than answering
`Gap`, because a caller's own arithmetic error is not the zone's answer; as a
guard for the same arithmetic, every instant function panics outside the years
1 to 9999.

## Freestanding

`std/tz` is hosted-only and its row in `freestanding::std_admission` is
`Hosted`: it reads a file and an environment variable, neither of which a
bare-metal image has a form of, and `TzErr.Corrupt` and every `Offset` carry a
`String`, which is the derivation-and-runtime-call pair that keeps `std/regex`
out. A freestanding project that imports it is refused by name with the
allowed-module list (`tests/projects/freestanding_std_refused_tz`). The project
grants `std/tz` the two domains its own body reaches (the refusal it pins is
the freestanding one, which is decided after the capability checks).

## Tests

`kelpc/tests/fixtures/stdlib_tz_synthetic.kp` needs no database: it builds TZif
byte arrays in Kelp, so the footer rules (an `M<m>.<w>.<d>` window, a `J<n>`
rule, a bare `n`, a quoted abbreviation and a fixed offset with seconds in it),
the 64-bit and 32-bit blocks, the `Ambiguous`/`Gap` cases, the formatter and
every corrupt-data refusal are pinned on a machine with an empty
`/usr/share/zoneinfo`, and it runs on both backends.

`kelpc/tests/fixtures/stdlib_tz_zones.kp` reads the host database and prints a
verdict table for `America/New_York` (the 2024 and 2026 transitions, with the
real expected instants), `Europe/Berlin`, `Asia/Kolkata` (a half-hour offset),
`Australia/Lord_Howe` (a thirty-minute DST step), a 2100 date through the
footer rule and a 1969 date before the first 64-bit-era table. The Rust test
prints a skip and returns when the directory is not there, and the fixture
itself prints one skip line and exits, so the `backend_coverage` sweep runs it
either way.

One compiler change came with the module and is recorded in
`plan/kernel-ub-notes.md`: a function body that ends in `while true { ... }` has
no exit edge, so the LLVM backend now closes it with an `unreachable` terminator
instead of a fallthrough `ret` of a zero value, which an owned return type
cannot build one of. The shape is the abbreviation reader's, and
`tests/fixtures/owned_return_endless_loop.kp` pins it on both backends.
