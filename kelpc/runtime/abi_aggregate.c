/* The genuine C object for the BUG-vN4tR gate (`kelpc/tests/run-llvm-abi.sh`).

   An aggregate over two eightbytes does not cross a C boundary by value: clang
   lowers such an argument to `byval(%struct)` on the stack and such a return to
   `sret(%struct)`, and LLVM's backend does not derive either from an IR call
   that passes the aggregate by value (BUG-vN4tR, the MK7 record in plan/done.md).
   Both sides of an IR-to-IR call agree with each other, so only a real object
   compiled by `cc` can witness the mismatch. This file is that object.

   It is compiled with `cc`, not `clang`: the point is the ABI a real C compiler
   emits, and `cc` is the compiler `kelpc` itself links with. */

#include <stddef.h>
#include <stdlib.h>
#include <stdint.h>

/* A 24-byte aggregate, SysV MEMORY class. */
typedef struct { int64_t a, b, c; } kelp_abi_wide;

/* The C ABI's by-value spelling of a 24-byte argument. An IR call that passes
   the same struct by value does not reach this object's parameter, so the value
   it prints is not the return value the pointer spelling produces. */
int64_t kelp_abi_take(kelp_abi_wide w) { return 112233 + w.a + w.b + w.c; }

/* The pointer spelling, the only one an IR call can reach (MK7 3.3). */
int64_t kelp_abi_take_ptr(const kelp_abi_wide* w) {
    return 112233 + w->a + w->b + w->c;
}

/* The hidden-pointer spelling of a 24-byte return: clang gives the C function
   a destination as its first argument, so the IR declares `void @f(ptr, ...)`
   and passes the slot there. */
void kelp_abi_mk(kelp_abi_wide* out) {
    out->a = 112233;
    out->b = 2;
    out->c = 3;
}

/* `mem.alloc`'s extern body with the ABI the backend declares for it
   (`declare void @kelp_std__mem__alloc_raw(ptr, i64)`). The compiler's own body
   (`kelpc/src/externs.rs`) returns `kelp_bytes_alloc_raw(kelp_n)`, which is
   zero-filled and prints as an empty line. This one returns a buffer holding a
   known text, so a Kelp program linked against this object can print that text
   only if the 24-byte `Bytes` came back through the hidden pointer and not by
   value. */
typedef struct { unsigned char* data; size_t len; size_t cap; } kelp_Bytes;

void kelp_std__mem__alloc_raw(kelp_Bytes* out, size_t n) {
    unsigned char* p = malloc(n + 1);
    size_t i = 0;
    if (i < n) p[i++] = 'Z';
    if (i < n) p[i++] = 'Y';
    p[i] = 0;
    out->data = p;
    out->len = i;
    out->cap = n;
}
