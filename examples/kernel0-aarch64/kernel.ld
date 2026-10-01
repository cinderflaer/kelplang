/* The kernel image for QEMU's aarch64 `virt` machine. RAM begins at
   0x40000000, and `-kernel` loads the ELF's segments there and starts it at the
   ELF entry, so `.text` is placed first and the compiler's `_start` stub
   (kernel stage 4) is what the entry names. `_start` loads `sp` from
   `__stack_top`, so the stack 16 KiB at the end of the image is the one the
   kernel runs on. */

ENTRY(_start)

SECTIONS
{
    . = 0x40000000;

    .text : {
        *(.text.boot)
        *(.text .text.*)
    }

    /* The unwinding tables LLVM emits for the driver functions are read-only
       data, and lld would otherwise place them as an orphan before `.text`:
       the boot loader starts at the ELF entry, but `.text.boot` has to hold the
       image's first byte for the entry to be the entry. */
    .eh_frame : ALIGN(8) {
        *(.eh_frame)
    }

    .rodata : ALIGN(8) {
        *(.rodata .rodata.*)
    }

    .data : ALIGN(8) {
        *(.data .data.*)
    }

    /* `_start` zeroes this range before it calls the entry, so the bounds are
       exported for the generated clear loop (kernel gap 4). They are declared
       weak in that loop's assembly, so a script without them still links. */
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
    . = . + 0x4000;
    __stack_top = .;
}
