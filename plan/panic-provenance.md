# Panic provenance: a trace, not just a line

*Proposal, not commitment and not a Mark. `roadmap.md` owns the plan and the schedule; nothing here is
scheduled, no work has started, and no lane is open. This file records the shape of an idea before it is
worth designing, the way `plan/polyp-ui.md` does for one far-future proposal and
`plan/asm-backend/default-switch.md` did for one proposed sequence.*

## What it is

A panic already says **where** it happened: `panic: <msg>`, then the `.kp` file and line, and since
MK5-S6-M1 the `errno` and its `strerror` text on a second line below. What it does not say is **how the
program got there**, which value came from which caller and led to the failure. The idea is to print
that chain as a trace:

```
panic: division by zero
  at compute_average (fault.kp:16)
  called from process_batch (fault.kp:42), with total=0 from an empty List
  called from main (fault.kp:8)
```

The difference from a stack trace is the middle line. A stack trace names the frames and leaves a value
to whatever the debugger can still read out of a local; a provenance trace names the route a value took
to get there, which is a fact about the program's data flow rather than about its call stack.

## What it depends on, and one honest qualification

It depends on **A-11's provenance work, open item 33**, and it does not start before that lands.

The premise recorded with the idea is that this is *presentation* of data the checker already has, and
that no new tracking infrastructure is needed. That premise deserves one qualification, because A-11's
provenance as the plan describes it is narrower than the trace above: item 33 asks for a **borrow's
root**, tracked in the borrow checker's `VarState` so that `borrow_root` follows a reference-typed
binding back to the owner it came from. That is a compile-time fact about one borrow and one owner.

The example trace needs two things beyond it:

- **a call chain at run time.** Knowing that a borrow's root is `list` is not knowing that
  `process_batch` was called from `main`; the frames are a runtime fact.
- **values.** `total=0 from an empty List` is a statement about a value at the moment of the panic, not
  about which owner a borrow came from.

Whether A-11's root tracking is rich enough to reconstruct a chain, or whether this needs its own
tracking, is the first thing to check when A-11 does land. That is the honest form of the premise: the
idea is cheap if the checker's provenance already carries it and is a larger feature if it does not, and
nothing here claims which it is.

## Out of scope

- Any implementation work.
- The exact format of the trace, which is a design question with a lot of taste in it.
- Whether the chain is captured continuously or only assembled when a panic is about to print, which is
  where the cost of the idea lives and which no one has measured.

## Status

Behind the current critical path (threads, then FFI), and behind A-11's provenance work specifically. It
blocks nothing and is not a Mark.
