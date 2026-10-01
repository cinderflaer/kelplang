/* The project's own `memcpy`, assembled by the test with `llvm-mc` and linked
   beside the object `kelpc` emits. The compiler's fallback is weak, so this
   strong definition is the one the linker keeps.

   `a4` is a fingerprint, not a copy register: it is what the test reads in the
   disassembly to tell this body from the compiler's fallback. The copy itself
   is the ordinary forward byte loop, so the image stays sane if it is called. */

	.text
	.globl	memcpy
	.type	memcpy, @function
memcpy:
	lui	a4, 0x5eed
	mv	a3, a0
	beqz	a2, 2f
1:
	lb	a5, 0(a1)
	sb	a5, 0(a3)
	addi	a3, a3, 1
	addi	a1, a1, 1
	addi	a2, a2, -1
	bnez	a2, 1b
2:
	ret
	.size	memcpy, . - memcpy
