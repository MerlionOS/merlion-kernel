/* Raspberry Pi aarch64 linker script.
 * Pi firmware loads kernel8.img at 0x80000.
 */
ENTRY(_start)

SECTIONS {
    . = 0x80000;

    .text : {
        KEEP(*(.text._start))
        *(.text .text.*)
    }

    . = ALIGN(4096);
    .rodata : {
        *(.rodata .rodata.*)
    }

    . = ALIGN(4096);
    .data : {
        *(.data .data.*)
    }

    . = ALIGN(4096);
    __bss_start = .;
    .bss : {
        *(.bss .bss.*)
        *(COMMON)
    }
    __bss_end = .;

    . = ALIGN(4096);
    __stack_top = . + 0x80000;  /* 512KB stack */

    /DISCARD/ : {
        *(.eh_frame*)
        *(.note*)
        *(.comment*)
    }
}
