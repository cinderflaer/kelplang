/* The linker script the freestanding_memcpy project names. QEMU's `virt`
   machine loads a RISC-V ELF at 0x80000000, so `.text` is placed there; the
   boot section comes first so `_start` is the first instruction. `__stack_top`
   is the symbol the generated `_start` loads into `sp`, and `__bss_start` /
   `__bss_end` are the range its clear loop zeroes (kernel gap 4). */

ENTRY(_start)

SECTIONS
{
    . = 0x80000000;

    .text : {
        *(.text.boot)
        *(.text .text.*)
    }

    .rodata : {
        *(.rodata.boot)
        *(.rodata .rodata.*)
    }

    .data : {
        *(.data .data.*)
    }

    /* The range `_start` zeroes before it calls the entry (kernel gap 4);
       the generated clear loop declares both weak, so this is optional. */
    .bss : ALIGN(8) {
        __bss_start = .;
        *(.sbss .sbss.*)
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(8);
        __bss_end = .;
    }

    . = ALIGN(16);
    __stack_top = . + 0x10000;
}
