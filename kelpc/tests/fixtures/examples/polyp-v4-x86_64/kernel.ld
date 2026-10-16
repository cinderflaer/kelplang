/* The kernel image for QEMU's x86-64 `-kernel` boot, which loads an ELF64
   image through the Xen PVH note and jumps to the 32-bit entry the note names
   (`kelp wiki page language/kernel.md`). The image is placed at 1 MiB, the classic low-memory
   kernel base, and `.text.boot` comes first so the compiler's `_start` stub is
   the ELF entry the boot loader reaches. `_start` loads `sp` from
   `__stack_top`, so the stack at the end of the image is the one the kernel
   runs on. Polyp's diff, layout and paint recursion is deeper than kernel 0's
   driver, so the stack is 64 KiB rather than 16. */

ENTRY(_start)

SECTIONS
{
    . = 0x100000;

    .text : ALIGN(16) {
        *(.text.boot)
        *(.text .text.*)
    }

    /* The unwinding tables LLVM emits for the driver functions are read-only
       data, and lld would otherwise place them as an orphan before `.text`:
       `.text.boot` has to hold the image's first byte so `_start` is where the
       PVH note points. */
    .eh_frame : ALIGN(8) {
        *(.eh_frame)
    }

    .note.Xen : ALIGN(4) {
        *(.note.Xen)
    }

    .rodata : ALIGN(8) {
        *(.rodata .rodata.*)
        *(.rodata.boot)
    }

    .data : ALIGN(8) {
        *(.data .data.*)
    }

    /* The boot page tables the 32-bit half of `_start` builds before paging is
       on. They are NOBITS, placed outside `[__bss_start, __bss_end)`, and the
       stub zeroes them itself, because the 64-bit `.bss` clear runs under the
       page tables it would otherwise be clearing. */
    .boot_pt (NOLOAD) : ALIGN(4096) {
        *(.boot_pt)
    }

    /* `_start` zeroes this range before it calls the entry, so the bounds are
       exported for the generated clear loop. They are declared weak in that
       loop's assembly, so a script without them still links. */
    .bss : ALIGN(8) {
        __bss_start = .;
        *(.sbss .sbss.*)
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(8);
        __bss_end = .;
    }

    . = ALIGN(16);
    __stack_bottom = .;
    . = . + 0x10000;
    __stack_top = .;
}
