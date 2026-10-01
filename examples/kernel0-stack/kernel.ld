/* The kernel image for QEMU's riscv64 `virt` machine (kernel stage 14). RAM
   begins at 0x80000000 and `-kernel` loads the ELF's segments there and starts
   it at the ELF entry, so `.text` is placed first and the compiler's `_start`
   stub (kernel stage 4) is what the entry names.

   Three stacks and one hole matter here. `_start` loads `sp` from
   `__stack_top`, so the kernel stack is the 8 KiB just below it. Directly
   below that stack is `__guard_bottom`..`__guard_top`, one page that neither
   `map_range` in `main.kp` maps, so a stack access there traps instead of
   walking into the trap stack. The 16 KiB below the hole is the trap stack:
   `sscratch` names its top, so the S-mode handler runs there even when the
   kernel stack is spent. */

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

    /* The trap stack, the guard page and the kernel stack, in that order. The
       trap stack is the region below the hole, so it is mapped with the rest of
       the image; the kernel stack is above the hole and mapped separately. */
    . = ALIGN(16);
    __trap_stack_bottom = .;
    . = . + 0x4000;
    __trap_stack_top = .;

    . = ALIGN(4096);
    __guard_bottom = .;
    . = . + 0x1000;
    __guard_top = .;
    __stack_bottom = .;
    . = . + 0x2000;
    __stack_top = .;

    /* The page frames the page-table walker allocates. The region is plain RAM
       after the image, so the loader neither zeroes it nor copies it, which is
       why `vm.alloc_frame` zeroes every frame it hands out. */
    . = ALIGN(4096);
    __frames_start = .;
    . = . + 0x10000;
    __frames_end = .;
}
