# Polyp: a native UI framework for Kelp

*Proposal, not commitment and not a Mark. `roadmap.md` owns the plan and the schedule; nothing here is
scheduled, no work has started, and no lane is open. This file exists because the idea is concrete enough
to write down and far enough away to be worth keeping out of the live plan. What follows is what Polyp
is, what a program would look like in it, what it would owe a reader that Tkinter and Compose do not, and
the one thing it cannot start without.*

## What it is

Polyp is a native UI framework for Kelp. The name fits the ocean and reef scheme the rest of the project
uses: kelp, coral, lighthouse, polyp.

It **wraps the platform's own widgets** rather than drawing its own. GTK first, on Linux. That is the
whole of the visual argument: a Polyp window looks native because it is native, and the framework does
not re-implement widget rendering the way Flutter and Skia do. A custom-drawn engine buys identical
pixels on every platform and pays for them with a drawing and layout stack; Polyp takes the other trade.

## The programming model: declarative, not imperative

**A view is a function of state, re-rendered when the state changes.** A program describes what the
screen should be for the current state and lets the framework reconcile it; it does not reach for a
widget and update it by hand. That is the line that rules out a Tkinter-style API by construction:
`label.set("...")` is an imperative update of one widget, and it is not the shape Polyp would offer.

Composition is builder-shaped, with the block as a trailing closure:

```kelp
polyp.column {
    polyp.text(title)
    polyp.button("+1") { state.count += 1 }
}
```

This does not compile today, and not only because Polyp does not exist: the trailing block is
trailing-closure syntax, which the language does not have. Written in Kelp as it stands, the same view
hands the callback over as an ordinary argument, `f = fun() { state.count += 1 }`, and the sugar is what
the example above is asking for.

**That is a prerequisite, and it is not scoped.** It is recorded as open item 75 in `roadmap.md` and it
depends on nothing else. Polyp cannot start before it exists, and the syntax question is a language
decision rather than a UI one.

## What sets it apart, concretely

Three things a Kelp UI framework would have that Tkinter and Kotlin's Compose Multiplatform do not. Each
one is a consequence of a language property Kelp already has, not a feature Polyp would have to invent.

1. **Borrow-checked callbacks.** A closure that captures state by borrow cannot hold a stale copy of it.
   In Compose or React, a callback can close over a value that has since changed, and the bug is a
   runtime one: the handler runs, reads its captured snapshot, and does the wrong thing. Kelp's checker
   asks whether the borrow is still valid at the point the callback runs, so a callback whose lifetime
   does not check is a compile error. That turns a whole class of UI defects, the stale closure and the
   use of an outdated captured value, into compiler-caught ones.
2. **No garbage collector.** Kelp is ownership and drop, with no GC, so there is no collector to pause
   during an animation or an interaction. Compose Multiplatform runs on the JVM, where a GC pause during
   a frame is a real and measurable event. This is a structural difference in the runtime, not a claim
   about smoothness.
3. **Exhaustive `when` over UI state enums.** A screen's states are a Kelp `enum` and the view matches
   them with `when`, which is exhaustiveness-checked. Add a state and forget to handle it in a view and
   the build breaks at that match, in every view that matches it, instead of the program showing a
   blank screen at run time. It is the same guarantee the compiler already enforces on its own internal
   matches, extended to application code.

## Explicitly out of scope for now

- Any implementation work, anywhere.
- Any platform beyond Linux.
- Whether it wraps GTK directly or through an abstraction layer of its own. That is a design decision
  for whoever starts it, and it interacts with the widget-wrapping choice above.
- The trailing-closure syntax design itself, which is open item 75 and a language question first.

## Status

**Far later in Kelp's development, after the current critical path (threads, FFI).** It blocks nothing
and nothing blocks it except trailing-closure syntax existing first. It is not a Mark, it has no stages,
and promoting it is a decision to take when a real UI program is wanted rather than a plan to run now.
