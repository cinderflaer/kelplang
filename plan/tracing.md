# Tracing and profiling hooks (A-16)

`std/trace` is the A-16 tracing and profiling surface. It is a normal shipped
library, not a backend facility: both C and LLVM lower the same Kelp source and
produce the same records.

## Authority and opt-in

Tracing changes a program's observable stderr output and reads a monotonic
clock. A module therefore opts in explicitly:

```kelp
impl "std/source"
impl "std/trace" caps(time = [])
```

`std/trace` belongs to the existing `time` capability domain. The empty pattern
list is intentional: time has no named resource, and holding the domain is the
authority to observe it. There is no global compiler flag, environment switch,
or implicit instrumentation that can make an ordinary program emit records.

## API

```kelp
trace.event(file: &str, line: usize, name: &str)
trace.begin(file: &str, line: usize, name: &str).R: trace.Span
trace.end(file: &str, line: usize, span: trace.Span)
```

The caller passes `source.file()` and `source.line()`:

```kelp
trace.event(source.file(), source.line(), "connected")
val request = trace.begin(source.file(), source.line(), "request")
// work
trace.end(source.file(), source.line(), request)
```

`source.file()` inside a wrapper resolves to the wrapper's own Kelp call site.
Kelp has no caller-location primitive or macro expansion yet, so pretending that
`trace.event(name)` could name the caller would produce false locations. The
explicit pair is verbose but correct, and a future macro can remove it without
changing the record format.

## Record format

Each call appends one line to stderr:

```text
trace<TAB>event|begin|end<TAB>monotonic_ns<TAB>file:line<TAB>name[<TAB>elapsed_ns]
```

`begin` and `event` omit `elapsed_ns`; `end` measures from the span's saved
monotonic timestamp. The timestamps make ordering and duration measurable, but
are not wall-clock time and must not be compared between processes. Backslashes,
tabs and newlines in `file` and `name` are escaped so one call remains one
record. The format is intentionally small and stable enough for a terminal,
`awk`, or a future profiling reader; it is not a tracing collector protocol.

Nested spans, multiple threads, sampling, allocation profiling and automatic
function instrumentation are deliberately outside this first hook set. The
first two need a concurrency and ordering contract, and the latter two would
need compiler instrumentation or allocator hooks rather than a library API.
