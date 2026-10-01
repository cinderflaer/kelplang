/* The linker script the freestanding_arena project names. QEMU's `virt`
   machine loads a RISC-V ELF at 0x80000000, so `.text` is placed there; the
   boot section comes first so `_start` is the first instruction. `__stack_top`
   is the symbol the generated `_start` loads into `sp`. */

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

    .bss : {
        *(.bss .bss.*)
        *(COMMON)
    }

    . = ALIGN(16);
    __stack_top = . + 0x10000;
}
