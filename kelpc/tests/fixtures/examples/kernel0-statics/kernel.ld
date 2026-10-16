/* The kernel image for QEMU's riscv64 `virt` machine. RAM begins at 0x80000000,
   and `-kernel` loads the ELF's segments there and starts it at the ELF entry,
   so `.text` is placed first and the compiler's `_start` stub (kernel stage 4)
   is what the entry names. `_start` loads `sp` from `__stack_top`, so the stack
   16 KiB at the end of the image is the one the kernel runs on. */

ENTRY(_start)

SECTIONS
{
    . = 0x80000000;

    .text : {
        *(.text.boot)
        *(.text .text.*)
    }

    /* The unwinding tables LLVM emits for the driver functions are read-only
       data, and lld would otherwise place them as an orphan before `.text`:
       QEMU's `-kernel` starts at 0x80000000 rather than the ELF entry, so
       `.text.boot` has to hold that address. */
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
        *(.bss .bss.*)
        *(COMMON)
    }

    . = ALIGN(16);
    __stack_bottom = .;
    . = . + 0x4000;
    __stack_top = .;
}
