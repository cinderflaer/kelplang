/* The kernel image for QEMU's riscv64 `virt` machine. RAM begins at 0x80000000,
   and `-kernel` loads the ELF's segments there and starts it at the ELF entry,
   so `.text` is placed first and the compiler's `_start` stub (kernel stage 4)
   is what the entry names. `_start` loads `sp` from `__stack_top`, so the stack
   16 KiB at the end of the image is the one the boot runs on; every task has
   its own stack in the task table's `[u8; 4096]`, which lands in `.bss`. */

ENTRY(_start)

SECTIONS
{
    . = 0x80000000;

    .text : {
        *(.text.boot)
        /* `mtvec` holds the vector's address with its low two bits as a mode,
           so the trap entry's own section is aligned to 4. */
        . = ALIGN(4);
        *(.text.trap)
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

    /* The task table and the boot frame, both all-zero, are here, and `_start`
       zeroes the range before it calls the entry. */
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
