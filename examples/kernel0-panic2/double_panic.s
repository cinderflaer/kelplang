/* The project's own answer to a panic that fires while the handler is already
   running. The compiler always emits a *weak* `kelp_double_panic` (a loop on
   `wfi`), so a strong definition in an object linked beside the image wins at
   link time, exactly as `tests/projects/freestanding_memcpy_user` wins the
   `memcpy` link. `run.sh` assembles this file for the same triple and relinks
   the object `kelpc` emitted beside it.

   `kelp_kp_double_panic` is the mangled symbol of `kp_double_panic` in
   `src/main.kp`; a jump, not a call, because the handler never returns. */
    .section .text.double, "ax", @progbits
    .globl kelp_double_panic
    .type kelp_double_panic, @function
kelp_double_panic:
    j kelp_kp_double_panic
    .size kelp_double_panic, . - kelp_double_panic
