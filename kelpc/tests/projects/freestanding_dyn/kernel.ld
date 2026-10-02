/* The image for QEMU's riscv64 `virt` machine, the same shape
   `examples/kernel0/kernel.ld` uses: RAM begins at 0x80000000, `.text.boot`
   holds `_start` first, and the stack is 16 KiB at the end of the image. */

ENTRY(_start)

SECTIONS
{
    . = 0x80000000;

    .text : {
        *(.text.boot)
        *(.text .text.*)
    }

    .eh_frame : ALIGN(8) {
        *(.eh_frame)
    }

    .rodata : ALIGN(8) {
        *(.rodata .rodata.*)
    }

    .data : ALIGN(8) {
        *(.data .data.*)
    }

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
