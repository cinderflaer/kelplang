// std/mem — allocation primitives (M6.2).
//
// The only thing here that X cannot express is the allocation itself, so that
// is the one `extern`: `alloc_raw`'s body is supplied by the compiler and
// produces an owned, zero-filled byte buffer. Everything a caller sees is the
// ordinary function below it, written in X and reviewable as such.
//
// There is deliberately no `mem.free`: owned values are freed by the drop
// rules, so an explicit free would be a double free.

extern fun alloc_raw(n: usize).R: Bytes

// An owned, zero-filled buffer of `n` bytes. Fixed size for its whole life
// (stdlib decision 5) — growable buffers are the collections milestone.
fun alloc(n: usize).R: Bytes = alloc_raw(n)
