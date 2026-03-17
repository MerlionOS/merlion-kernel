/* LoongArch kernel linker script — QEMU virt loads at 0x9000000000200000 */
/* Direct-mapped window: virtual 0x9000000000000000 = physical 0x0 */
ENTRY(_start)
SECTIONS {
    . = 0x9000000000200000;
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
    __heap_start = .;
    /DISCARD/ : {
        *(.eh_frame*)
        *(.note*)
        *(.comment*)
    }
}
