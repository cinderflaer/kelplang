// Gap 7: two real variadic C functions, one reading ints and one reading a
// double and a const char*. Both are "static inline" so a project-local header
// is enough and nothing has to be linked. The generated wrapper forwards its
// own fixed arguments into the "..." position, which is where the C compiler
// applies the default argument promotions (a Kelp "i32"/"u8" arrives as an
// int, a Kelp "f32" as a double) that a native va_arg then reads back.
#include <stdarg.h>
#include <string.h>

static inline int kelp_va_sum(int count, ...) {
    va_list ap;
    va_start(ap, count);
    int total = 0;
    for (int i = 0; i < count; i++) {
        total += va_arg(ap, int);
    }
    va_end(ap);
    return total;
}

static inline int kelp_va_mix(int count, ...) {
    va_list ap;
    va_start(ap, count);
    double d = va_arg(ap, double);
    const char* text = va_arg(ap, const char*);
    va_end(ap);
    return (int)(d * count) + (int)strlen(text);
}
